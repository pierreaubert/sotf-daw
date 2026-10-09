// ============================================================================
// Loudness Monitor Analyzer Plugin
// ============================================================================

use crate::analyzer::{
    IntegratedLoudnessMode, LoudnessData, LoudnessQueryError, LoudnessRangeConfig,
    LoudnessRangeData, RealTimeCache,
};
use crate::analyzer_channel_correlation::ChannelCorrelationMonitor;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginDrainResult,
    PluginInfo, PluginResult, ProcessContext,
};
use crate::speaker_config::{ChannelLayout, ChannelRole};
use math_audio_dsp::ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

mod loudness_range;
use loudness_range::LoudnessRangeHistory;

const TRUE_PEAK_TAPS: usize = 12;
const TRUE_PEAK_SINC_TAPS: usize = 64;
const TRUE_PEAK_MIN_RATE_HZ: f64 = 8_000.0;
const TRUE_PEAK_MAX_RATE_HZ: f64 = 2_822_400.0;
const TRUE_PEAK_TARGET_RATE_HZ: f64 = 192_000.0;
const TRUE_PEAK_MAX_OVERSAMPLING: usize = 32;
pub const INTEGRATED_HISTORY_SECONDS: u32 = 3_600;
const EXACT_GATING_BLOCK_CAPACITY: usize = INTEGRATED_HISTORY_SECONDS as usize * 10;
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
const RELATIVE_GATE_DB: f64 = -10.0;
// EBU Tech 3341's first-minute display state uses accepted active audio time,
// independent of the backend's rounded 100 ms observation grid.
const LRA_STABILITY_SECONDS: u64 = 60;
const INTEGRATED_CONTROL_COMMAND_MAX_BYTES: usize = 50;
static NEXT_LOUDNESS_CONTROL_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntegratedControlOperation {
    Start,
    Pause,
    Continue,
    Reset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IntegratedControlCommand {
    instance_id: u64,
    request_id: u64,
    operation: IntegratedControlOperation,
}

fn allocate_loudness_control_instance_id(counter: &AtomicU64) -> Result<u64, String> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current != 0).then(|| current.checked_add(1)).flatten()
        })
        .map_err(|_| "loudness monitor runtime instance ID exhausted".to_string())
}

fn parse_canonical_control_id(value: &str, allow_zero: bool) -> Result<u64, &'static str> {
    if value.is_empty()
        || (value.len() > 1 && value.as_bytes()[0] == b'0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("control IDs must be canonical unsigned decimals");
    }
    let parsed = value.parse::<u64>().map_err(|_| "control ID exceeds u64")?;
    if !allow_zero && parsed == 0 {
        return Err("control ID must be nonzero");
    }
    Ok(parsed)
}

fn parse_integrated_control_command(value: &str) -> Result<IntegratedControlCommand, &'static str> {
    if value.len() > INTEGRATED_CONTROL_COMMAND_MAX_BYTES || !value.is_ascii() {
        return Err("integrated control command is oversized or non-ASCII");
    }
    let mut fields = value.split(':');
    let instance_id =
        parse_canonical_control_id(fields.next().ok_or("missing target instance ID")?, false)?;
    let request_id = parse_canonical_control_id(fields.next().ok_or("missing request ID")?, false)?;
    let operation = match fields.next().ok_or("missing operation")? {
        "start" => IntegratedControlOperation::Start,
        "pause" => IntegratedControlOperation::Pause,
        "continue" => IntegratedControlOperation::Continue,
        "reset" => IntegratedControlOperation::Reset,
        _ => return Err("unknown integrated control operation"),
    };
    if fields.next().is_some() {
        return Err("integrated control command has extra fields");
    }
    Ok(IntegratedControlCommand {
        instance_id,
        request_id,
        operation,
    })
}

fn required_momentary_frames(sample_rate: f64) -> u64 {
    (sample_rate * 0.4).ceil() as u64
}

fn required_shortterm_frames(sample_rate: f64) -> u64 {
    (sample_rate * 3.0).ceil() as u64
}

fn update_finite_maximum(maximum: &mut Option<f64>, observed: f64) {
    if observed.is_finite() {
        *maximum = Some(maximum.map_or(observed, |current| current.max(observed)));
    }
}

fn energy_to_loudness(energy: f64) -> f64 {
    -0.691 + 10.0 * energy.log10()
}

fn loudness_to_energy(loudness: f64) -> f64 {
    10.0_f64.powf((loudness + 0.691) / 10.0)
}

/// Exact two-pass BS.1770 gating over every retained overlapping 400 ms
/// block. Capacity is prepared before processing; history is never evicted.
struct WholeProgramIntegrated {
    gating_blocks: Vec<f64>,
    cached_loudness: f64,
    dirty: bool,
    capacity_exceeded: bool,
    capacity_error_published: bool,
}

impl WholeProgramIntegrated {
    fn new(capacity: usize) -> Self {
        Self {
            gating_blocks: Vec::with_capacity(capacity),
            cached_loudness: f64::NEG_INFINITY,
            dirty: false,
            capacity_exceeded: false,
            capacity_error_published: false,
        }
    }

    fn push(&mut self, energy: f64) {
        if self.capacity_exceeded {
            return;
        }
        if self.gating_blocks.len() == self.gating_blocks.capacity() {
            self.capacity_exceeded = true;
            self.cached_loudness = f64::NEG_INFINITY;
            self.dirty = false;
            return;
        }
        self.gating_blocks.push(energy);
        self.dirty = true;
    }

    fn refresh(&mut self) {
        if !self.dirty || self.capacity_exceeded {
            return;
        }
        self.cached_loudness = gated_loudness(&self.gating_blocks);
        self.dirty = false;
    }

    fn reset(&mut self) {
        self.gating_blocks.clear();
        self.cached_loudness = f64::NEG_INFINITY;
        self.dirty = false;
        self.capacity_exceeded = false;
        self.capacity_error_published = false;
    }
}

fn gated_loudness(blocks: &[f64]) -> f64 {
    gated_loudness_iter(blocks.iter().copied())
}

fn gated_loudness_iter<I>(blocks: I) -> f64
where
    I: Iterator<Item = f64> + Clone,
{
    let absolute_gate = loudness_to_energy(ABSOLUTE_GATE_LUFS);
    let (absolute_sum, absolute_count) = blocks
        .clone()
        .filter(|&energy| energy > absolute_gate)
        .fold((0.0, 0_u64), |(sum, count), energy| {
            (sum + energy, count + 1)
        });
    if absolute_count == 0 {
        return f64::NEG_INFINITY;
    }
    let relative_gate =
        absolute_sum / absolute_count as f64 * 10.0_f64.powf(RELATIVE_GATE_DB / 10.0);
    let (relative_sum, relative_count) = blocks
        .filter(|&energy| energy > relative_gate)
        .fold((0.0, 0_u64), |(sum, count), energy| {
            (sum + energy, count + 1)
        });
    if relative_count == 0 {
        f64::NEG_INFINITY
    } else {
        energy_to_loudness(relative_sum / relative_count as f64)
    }
}

/// Rolling integrated loudness for an explicit layout.
///
/// `math-dsp::EbuR128` owns this history for its built-in channel map. An
/// explicit layout uses one mono meter per non-LFE channel instead, so the
/// aggregate 400 ms energies need an equivalent bounded history here.
struct RollingIntegrated {
    gating_blocks: VecDeque<f64>,
    cached_loudness: f64,
    dirty: bool,
}

impl RollingIntegrated {
    fn new(capacity: usize) -> Self {
        Self {
            gating_blocks: VecDeque::with_capacity(capacity),
            cached_loudness: f64::NEG_INFINITY,
            dirty: false,
        }
    }

    fn push(&mut self, energy: f64) {
        if self.gating_blocks.len() == self.gating_blocks.capacity() {
            self.gating_blocks.pop_front();
        }
        self.gating_blocks.push_back(energy);
        self.dirty = true;
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.cached_loudness = gated_loudness_iter(self.gating_blocks.iter().copied());
        self.dirty = false;
    }

    fn reset(&mut self) {
        self.gating_blocks.clear();
        self.cached_loudness = f64::NEG_INFINITY;
        self.dirty = false;
    }
}

/// Loudness meters for an explicit layout wider than 5.1.
///
/// The dependency's EBU meter has a fixed positional channel map and assigns
/// zero weight to channels beyond index five. A mono meter has an unambiguous
/// unity internal weight, so applying the semantic BS.1770 amplitude gain
/// before each per-channel meter preserves the layout's energy without
/// relying on that positional map.
struct ExplicitLoudnessMeters {
    meters: Vec<EbuR128>,
    source_indices: Vec<usize>,
    gains: Vec<f32>,
    scratch: Vec<f32>,
    rolling_integrated: Option<RollingIntegrated>,
}

impl ExplicitLoudnessMeters {
    fn new(
        layout: &ChannelLayout,
        sample_rate: f64,
        integrated_mode: IntegratedLoudnessMode,
        accumulate_integrated: bool,
    ) -> Result<Self, String> {
        let mut meters = Vec::new();
        let mut source_indices = Vec::new();
        let mut gains = Vec::new();
        for index in 0..layout.channels.len() {
            let role = layout
                .role_at(index)
                .ok_or_else(|| format!("explicit channel layout is missing channel {index}"))?;
            if role == ChannelRole::Lfe {
                continue;
            }
            meters.push(
                EbuR128::new(1, sample_rate, Mode::M | Mode::S)
                    .map_err(|error| format!("{error:?}"))?,
            );
            source_indices.push(index);
            gains.push(role.bs1770_weight().sqrt());
        }

        Ok(Self {
            meters,
            source_indices,
            gains,
            // Explicit-layout chunks are bounded to 256 frames by the
            // separate sample-peak meter in LoudnessMonitor::add_frames.
            scratch: vec![0.0; 256],
            rolling_integrated: (accumulate_integrated
                && integrated_mode == IntegratedLoudnessMode::Rolling)
                .then(|| RollingIntegrated::new(EXACT_GATING_BLOCK_CAPACITY)),
        })
    }

    fn add_chunk(&mut self, input: &[f32], input_channels: usize) -> Result<(), String> {
        let frames = input.len() / input_channels;
        if frames > self.scratch.len() {
            return Err(format!(
                "explicit loudness chunk has {frames} frames; maximum is {}",
                self.scratch.len()
            ));
        }
        for ((meter, &source_index), &gain) in self
            .meters
            .iter_mut()
            .zip(&self.source_indices)
            .zip(&self.gains)
        {
            for frame in 0..frames {
                self.scratch[frame] = input[frame * input_channels + source_index] * gain;
            }
            meter
                .add_frames_f32(&self.scratch[..frames])
                .map_err(|error| {
                    format!("EBU R128 explicit loudness add_frames failed: {error:?}")
                })?;
        }
        Ok(())
    }

    fn aggregate_loudness<F>(&self, query: F) -> Result<f64, String>
    where
        F: Fn(&EbuR128) -> Result<f64, String>,
    {
        let mut energy = 0.0;
        for meter in &self.meters {
            let loudness = query(meter)?;
            if loudness.is_nan() {
                return Ok(f64::NAN);
            }
            if loudness.is_finite() {
                energy += loudness_to_energy(loudness);
            }
        }
        if energy > 0.0 {
            Ok(energy_to_loudness(energy))
        } else {
            Ok(f64::NEG_INFINITY)
        }
    }

    fn momentary(&self) -> Result<f64, String> {
        self.aggregate_loudness(EbuR128::loudness_momentary)
    }

    fn shortterm(&self) -> Result<f64, String> {
        self.aggregate_loudness(EbuR128::loudness_shortterm)
    }

    fn push_integrated_block(&mut self, energy: f64) {
        if let Some(integrated) = &mut self.rolling_integrated {
            integrated.push(energy);
        }
    }

    fn refresh_integrated(&mut self) {
        if let Some(integrated) = &mut self.rolling_integrated {
            integrated.refresh();
        }
    }

    fn integrated(&self) -> Option<f64> {
        self.rolling_integrated
            .as_ref()
            .map(|integrated| integrated.cached_loudness)
    }

    fn reset(&mut self) {
        for meter in &mut self.meters {
            meter.reset();
        }
        if let Some(integrated) = &mut self.rolling_integrated {
            integrated.reset();
        }
    }
}

// ITU-R BS.1770-5 Annex 2, printed pp. 18–19 (also present in BS.1770-4).
// Each published column is reversed for oldest-to-newest history storage:
// the first published row multiplies the newest input sample. Keep the
// phase numbers in published order: all four at 4x and the 0/2 subset at 2x.
// https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf
const TRUE_PEAK_PHASES: [[f64; TRUE_PEAK_TAPS]; 4] = [
    [
        -0.0083007812500,
        0.0148925781250,
        -0.0266113281250,
        0.0476074218750,
        -0.1022949218750,
        0.9721679687500,
        0.1373291015625,
        -0.0594482421875,
        0.0332031250000,
        -0.0196533203125,
        0.0109863281250,
        0.0017089843750,
    ],
    [
        -0.0189208984375,
        0.0330810546875,
        -0.0582275390625,
        0.1015625000000,
        -0.2003173828125,
        0.7797851562500,
        0.4650878906250,
        -0.1665039062500,
        0.0891113281250,
        -0.0517578125000,
        0.0292968750000,
        -0.0291748046875,
    ],
    [
        -0.0291748046875,
        0.0292968750000,
        -0.0517578125000,
        0.0891113281250,
        -0.1665039062500,
        0.4650878906250,
        0.7797851562500,
        -0.2003173828125,
        0.1015625000000,
        -0.0582275390625,
        0.0330810546875,
        -0.0189208984375,
    ],
    [
        0.0017089843750,
        0.0109863281250,
        -0.0196533203125,
        0.0332031250000,
        -0.0594482421875,
        0.1373291015625,
        0.9721679687500,
        -0.1022949218750,
        0.0476074218750,
        -0.0266113281250,
        0.0148925781250,
        -0.0083007812500,
    ],
];

fn true_peak_oversampling_factor<S: Into<f64>>(sample_rate: S) -> Option<usize> {
    let sample_rate = sample_rate.into();
    // The upper bound matches math-dsp's current EBU R128 constructor range.
    if !(TRUE_PEAK_MIN_RATE_HZ..=TRUE_PEAK_MAX_RATE_HZ).contains(&sample_rate) {
        return None;
    }

    // Two-times interpolation remains useful for high-rate input so the
    // detector still checks inter-sample values instead of sample maxima.
    let mut factor = 2;
    while sample_rate * (factor as f64) < TRUE_PEAK_TARGET_RATE_HZ
        && factor < TRUE_PEAK_MAX_OVERSAMPLING
    {
        factor *= 2;
    }

    (sample_rate * factor as f64 >= TRUE_PEAK_TARGET_RATE_HZ).then_some(factor)
}

fn blackman_sinc_phase(oversampling_factor: usize, phase: usize) -> [f64; TRUE_PEAK_SINC_TAPS] {
    // The 64-tap window has 63 source intervals of support. A symmetric even
    // tap count gives an exactly representable half-sample center; because
    // each factor is a power of two, every requested phase is exact in f64.
    let center = (TRUE_PEAK_SINC_TAPS - 1) as f64 * 0.5;
    let fraction = phase as f64 / oversampling_factor as f64;
    let mut coefficients = [0.0; TRUE_PEAK_SINC_TAPS];
    let mut dc_gain = 0.0;

    for (tap, coefficient) in coefficients.iter_mut().enumerate() {
        let window_position = tap as f64 / (TRUE_PEAK_SINC_TAPS - 1) as f64;
        // Standard three-term Blackman window constants. This narrows the
        // sinc truncation sidelobes while keeping a smooth finite support.
        let window = 0.42 - 0.5 * (std::f64::consts::TAU * window_position).cos()
            + 0.08 * (2.0 * std::f64::consts::TAU * window_position).cos();
        let distance = tap as f64 - center - fraction;
        let sinc = if distance == 0.0 {
            1.0
        } else {
            (std::f64::consts::PI * distance).sin() / (std::f64::consts::PI * distance)
        };
        *coefficient = sinc * window;
        dc_gain += *coefficient;
    }

    for coefficient in &mut coefficients {
        *coefficient /= dc_gain;
    }
    coefficients
}

enum Bs1770TruePeakKernel {
    Published12 {
        history: Vec<[f64; TRUE_PEAK_TAPS]>,
        phase_indices: &'static [usize],
    },
    BlackmanSinc64 {
        history: Vec<[f64; TRUE_PEAK_SINC_TAPS]>,
        phase_coefficients: Vec<[f64; TRUE_PEAK_SINC_TAPS]>,
    },
}

fn process_blackman_sinc64_sample(
    history: &mut [f64; TRUE_PEAK_SINC_TAPS],
    phase_coefficients: &[[f64; TRUE_PEAK_SINC_TAPS]],
    sample: f64,
) -> f64 {
    history.copy_within(1.., 0);
    history[TRUE_PEAK_SINC_TAPS - 1] = sample;
    phase_coefficients
        .iter()
        .map(|phase| {
            phase
                .iter()
                .zip(history.iter())
                .map(|(coefficient, value)| coefficient * value)
                .sum::<f64>()
                .abs()
        })
        .fold(0.0, f64::max)
}

impl Bs1770TruePeakKernel {
    fn add_frames(&mut self, samples: &[f32], channels: usize, interval_peaks: &mut [f64]) {
        match self {
            Self::Published12 {
                history,
                phase_indices,
            } => {
                for frame in samples.chunks_exact(channels) {
                    for (channel, &sample) in frame.iter().enumerate() {
                        let history = &mut history[channel];
                        history.copy_within(1.., 0);
                        history[TRUE_PEAK_TAPS - 1] = f64::from(sample);
                        let peak = phase_indices
                            .iter()
                            .map(|&phase_index| {
                                TRUE_PEAK_PHASES[phase_index]
                                    .iter()
                                    .zip(history.iter())
                                    .map(|(coefficient, value)| coefficient * value)
                                    .sum::<f64>()
                                    .abs()
                            })
                            .fold(0.0, f64::max);
                        interval_peaks[channel] = interval_peaks[channel].max(peak);
                    }
                }
            }
            Self::BlackmanSinc64 {
                history,
                phase_coefficients,
            } => {
                for frame in samples.chunks_exact(channels) {
                    for (channel, &sample) in frame.iter().enumerate() {
                        let peak = process_blackman_sinc64_sample(
                            &mut history[channel],
                            phase_coefficients,
                            f64::from(sample),
                        );
                        interval_peaks[channel] = interval_peaks[channel].max(peak);
                    }
                }
            }
        }
    }

    fn finish_channel(&mut self, channel: usize) -> f64 {
        match self {
            Self::Published12 {
                history,
                phase_indices,
            } => {
                let history = &mut history[channel];
                let mut peak = 0.0_f64;
                for _ in 1..TRUE_PEAK_TAPS {
                    history.copy_within(1.., 0);
                    history[TRUE_PEAK_TAPS - 1] = 0.0;
                    peak = peak.max(
                        phase_indices
                            .iter()
                            .map(|&phase_index| {
                                TRUE_PEAK_PHASES[phase_index]
                                    .iter()
                                    .zip(history.iter())
                                    .map(|(coefficient, value)| coefficient * value)
                                    .sum::<f64>()
                                    .abs()
                            })
                            .fold(0.0, f64::max),
                    );
                }
                history.fill(0.0);
                peak
            }
            Self::BlackmanSinc64 {
                history,
                phase_coefficients,
            } => {
                let history = &mut history[channel];
                let mut peak = 0.0_f64;
                for _ in 1..TRUE_PEAK_SINC_TAPS {
                    peak = peak.max(process_blackman_sinc64_sample(
                        history,
                        phase_coefficients,
                        0.0,
                    ));
                }
                history.fill(0.0);
                peak
            }
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Published12 { history, .. } => history.fill([0.0; TRUE_PEAK_TAPS]),
            Self::BlackmanSinc64 { history, .. } => {
                history.fill([0.0; TRUE_PEAK_SINC_TAPS]);
            }
        }
    }
}

struct Bs1770TruePeakMeter {
    kernel: Option<Bs1770TruePeakKernel>,
    interval_peaks: Vec<f64>,
    compliant: bool,
}

impl Bs1770TruePeakMeter {
    fn new<S: Into<f64>>(channels: usize, sample_rate: S) -> Self {
        let sample_rate = sample_rate.into();
        const FOUR_PHASES: &[usize] = &[0, 1, 2, 3];
        const TWO_PHASES: &[usize] = &[0, 2];
        let kernel = match true_peak_oversampling_factor(sample_rate) {
            Some(2) => Some(Bs1770TruePeakKernel::Published12 {
                history: vec![[0.0; TRUE_PEAK_TAPS]; channels],
                phase_indices: TWO_PHASES,
            }),
            Some(4) => Some(Bs1770TruePeakKernel::Published12 {
                history: vec![[0.0; TRUE_PEAK_TAPS]; channels],
                phase_indices: FOUR_PHASES,
            }),
            Some(factor) => Some(Bs1770TruePeakKernel::BlackmanSinc64 {
                history: vec![[0.0; TRUE_PEAK_SINC_TAPS]; channels],
                phase_coefficients: (0..factor)
                    .map(|phase| blackman_sinc_phase(factor, phase))
                    .collect(),
            }),
            None => None,
        };
        Self {
            compliant: kernel.is_some(),
            kernel,
            interval_peaks: vec![0.0; channels],
        }
    }

    fn add_frames(&mut self, samples: &[f32], channels: usize) {
        let Some(kernel) = &mut self.kernel else {
            return;
        };
        kernel.add_frames(samples, channels, &mut self.interval_peaks);
    }

    fn take_interval_peak(&mut self, channel: usize) -> Option<f64> {
        self.compliant
            .then(|| std::mem::take(&mut self.interval_peaks[channel]))
    }

    fn finish(&mut self) {
        let Some(kernel) = &mut self.kernel else {
            return;
        };
        for (channel, peak) in self.interval_peaks.iter_mut().enumerate() {
            *peak = peak.max(kernel.finish_channel(channel));
        }
    }

    fn reset(&mut self) {
        if let Some(kernel) = &mut self.kernel {
            kernel.reset();
        }
        self.interval_peaks.fill(0.0);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoudnessInfo {
    pub momentary_lufs: f64,
    pub shortterm_lufs: f64,
    pub integrated_lufs: f64,
    pub peak: f64,
}

pub struct LoudnessMonitor {
    /// Live meter lane: always sees accepted input and never accumulates I.
    ebur128: EbuR128,
    explicit_loudness: Option<ExplicitLoudnessMeters>,
    /// Programme lane: receives input only while Integrated/LRA are running.
    integrated_ebur128: Option<EbuR128>,
    integrated_explicit_loudness: Option<ExplicitLoudnessMeters>,
    /// Raw, unscaled meter used only for sample/true peaks when an explicit
    /// layout requires role-dependent loudness scaling.
    peak_meter: Option<EbuR128>,
    channels: u32,
    sample_rate: f64,
    channel_layout: Option<ChannelLayout>,
    loudness_channels: usize,
    loudness_gains: Vec<f32>,
    loudness_channel_indices: Vec<usize>,
    weighted_scratch: Vec<f32>,
    /// When true, also maintain a full inter-channel Pearson r matrix and
    /// write it into `LoudnessData.correlation_matrix` on each update.
    /// Off by default — only the output-side LoudnessMonitor that feeds the
    /// spatial-spider widget needs to opt in. CLI tools, JSON dumps, and
    /// per-meter LoudnessMonitors keep the field empty for zero cost.
    spatial_enabled: bool,
    /// Full inter-channel correlation matrix accumulator. Lazily exercised:
    /// when `spatial_enabled == false`, `add_frames` skips it entirely and
    /// `update_loudness_data` leaves `LoudnessData.correlation_matrix` as the
    /// empty `Arc<Vec<f32>>` the caller constructed.
    correlation_matrix: ChannelCorrelationMonitor,
    /// Scratch buffer used to read the matrix into a contiguous slice for
    /// `LoudnessData::update_correlation_matrix`. Reused across calls to
    /// keep the audio-thread allocation count at zero.
    matrix_scratch: crate::analyzer::CorrelationData,
    /// Pre-allocated per-channel peak buffers sized to `channels`. Reused on
    /// every `update_loudness_data` call so >32-channel layouts (22.2, Atmos
    /// beds) do not silently truncate.
    peaks_buf: Vec<f64>,
    true_peaks_buf: Vec<f64>,
    true_peak_meter: Bs1770TruePeakMeter,
    maximum_true_peak_dbtp: Option<f64>,
    maximum_momentary_lufs: Option<f64>,
    maximum_shortterm_lufs: Option<f64>,
    query_error_generation: u64,
    frames_seen: u64,
    sub_block_frames: usize,
    frames_into_sub_block: usize,
    integrated_measurement_running: bool,
    integrated_frames_seen: u64,
    integrated_frames_into_sub_block: usize,
    integrated_completed_sub_blocks: u64,
    integrated_mode: IntegratedLoudnessMode,
    whole_program_integrated: Option<WholeProgramIntegrated>,
    loudness_range: Option<LoudnessRangeHistory>,
}

impl LoudnessMonitor {
    pub fn new<S: Into<f64>>(channels: u32, sr: S) -> Result<Self, String> {
        Self::new_inner(channels, sr.into(), None, IntegratedLoudnessMode::Rolling)
    }

    pub fn new_with_integrated_mode<S: Into<f64>>(
        channels: u32,
        sr: S,
        integrated_mode: IntegratedLoudnessMode,
    ) -> Result<Self, String> {
        Self::new_inner(channels, sr.into(), None, integrated_mode)
    }

    pub fn new_with_layout<S: Into<f64>>(
        channels: u32,
        sr: S,
        channel_layout: ChannelLayout,
    ) -> Result<Self, String> {
        Self::new_inner(
            channels,
            sr.into(),
            Some(channel_layout),
            IntegratedLoudnessMode::Rolling,
        )
    }

    pub fn new_with_layout_and_integrated_mode<S: Into<f64>>(
        channels: u32,
        sr: S,
        channel_layout: ChannelLayout,
        integrated_mode: IntegratedLoudnessMode,
    ) -> Result<Self, String> {
        Self::new_inner(channels, sr.into(), Some(channel_layout), integrated_mode)
    }

    fn new_inner(
        channels: u32,
        sr: f64,
        channel_layout: Option<ChannelLayout>,
        integrated_mode: IntegratedLoudnessMode,
    ) -> Result<Self, String> {
        if channels == 0 {
            return Err("loudness monitor requires at least one channel".to_string());
        }
        if !sr.is_finite() || sr < 10.0 {
            return Err("loudness monitor sample rate must be at least 10 Hz".to_string());
        }
        if let Some(layout) = &channel_layout {
            layout.validate_for_width(channels as usize)?;
        }
        // math-dsp's 5/6-channel meters embed a fixed assumed order. Use a
        // seven-channel unity-weight meter for explicit 5.0/5.1 layouts, then
        // supply role weights through sample scaling. Wider explicit layouts
        // use one mono meter per non-LFE channel because the dependency's
        // fixed map drops channels at index six and above.
        let explicit_wide_layout = channel_layout.is_some() && channels > 6;
        let loudness_channels = if explicit_wide_layout {
            1
        } else if channel_layout.is_some() && matches!(channels, 5 | 6) {
            7
        } else {
            channels as usize
        };
        let live_loudness_mode = if channel_layout.is_some() {
            Mode::M | Mode::S
        } else {
            Mode::M | Mode::S | Mode::SAMPLE_PEAK
        };
        let ebur = EbuR128::new(loudness_channels as u32, sr, live_loudness_mode)
            .map_err(|e| format!("{:?}", e))?;
        let integrated_ebur128 = (!explicit_wide_layout)
            .then(|| {
                EbuR128::new(loudness_channels as u32, sr, Mode::M | Mode::S | Mode::I)
                    .map_err(|error| format!("{error:?}"))
            })
            .transpose()?;
        let explicit_loudness = channel_layout
            .as_ref()
            .filter(|_| explicit_wide_layout)
            .map(|layout| ExplicitLoudnessMeters::new(layout, sr, integrated_mode, false))
            .transpose()?;
        let integrated_explicit_loudness = channel_layout
            .as_ref()
            .filter(|_| explicit_wide_layout)
            .map(|layout| ExplicitLoudnessMeters::new(layout, sr, integrated_mode, true))
            .transpose()?;
        let peak_meter = channel_layout
            .as_ref()
            .map(|_| EbuR128::new(channels, sr, Mode::SAMPLE_PEAK))
            .transpose()
            .map_err(|e| format!("{e:?}"))?;
        let loudness_gains = channel_layout
            .as_ref()
            .map(|layout| {
                (0..channels as usize)
                    .map(|index| {
                        layout
                            .role_at(index)
                            .expect("validated channel layout covers every index")
                            .bs1770_weight()
                            .sqrt()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let loudness_channel_indices = if explicit_wide_layout {
            Vec::new()
        } else {
            channel_layout
                .as_ref()
                .map(|layout| {
                    (0..channels as usize)
                        .map(|index| {
                            if loudness_channels != 7 {
                                return index;
                            }
                            match layout.role_at(index) {
                                Some(ChannelRole::FrontLeft) => 0,
                                Some(ChannelRole::FrontRight) => 1,
                                Some(ChannelRole::FrontCenter) => 2,
                                Some(ChannelRole::Lfe) => 3,
                                Some(ChannelRole::SideLeft | ChannelRole::BackLeft) => 4,
                                Some(ChannelRole::SideRight | ChannelRole::BackRight) => 5,
                                _ => index.min(loudness_channels.saturating_sub(1)),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(Self {
            ebur128: ebur,
            explicit_loudness,
            integrated_ebur128,
            integrated_explicit_loudness,
            peak_meter,
            channels,
            sample_rate: sr,
            channel_layout,
            loudness_channels,
            loudness_gains,
            loudness_channel_indices,
            // Fixed-size chunking keeps arbitrary callback sizes allocation
            // free while bounding control-time scratch allocation.
            weighted_scratch: vec![0.0; loudness_channels * 256],
            spatial_enabled: false,
            correlation_matrix: ChannelCorrelationMonitor::new(channels as usize, sr),
            matrix_scratch: crate::analyzer::CorrelationData::new(channels as usize),
            peaks_buf: vec![0.0; channels as usize],
            true_peaks_buf: vec![0.0; channels as usize],
            true_peak_meter: Bs1770TruePeakMeter::new(channels as usize, sr),
            maximum_true_peak_dbtp: None,
            maximum_momentary_lufs: None,
            maximum_shortterm_lufs: None,
            query_error_generation: 0,
            frames_seen: 0,
            sub_block_frames: sr as usize / 10,
            frames_into_sub_block: 0,
            integrated_measurement_running: true,
            integrated_frames_seen: 0,
            integrated_frames_into_sub_block: 0,
            integrated_completed_sub_blocks: 0,
            integrated_mode,
            whole_program_integrated: (integrated_mode == IntegratedLoudnessMode::WholeProgram)
                .then(|| WholeProgramIntegrated::new(EXACT_GATING_BLOCK_CAPACITY)),
            loudness_range: None,
        })
    }

    pub fn integrated_mode(&self) -> IntegratedLoudnessMode {
        self.integrated_mode
    }

    /// Whether Integrated Loudness and Loudness Range currently accept audio.
    pub fn integrated_measurement_running(&self) -> bool {
        self.integrated_measurement_running
    }

    /// Set the programme-measurement clock. Live meters continue independently.
    pub fn set_integrated_measurement_running(&mut self, running: bool) {
        self.integrated_measurement_running = running;
    }

    pub fn pause_integrated_measurement(&mut self) {
        self.set_integrated_measurement_running(false);
    }

    pub fn continue_integrated_measurement(&mut self) {
        self.set_integrated_measurement_running(true);
    }

    /// Clear the full measurement epoch and resume Integrated/LRA accumulation.
    pub fn start_integrated_measurement(&mut self) -> Result<(), String> {
        self.reset()?;
        self.continue_integrated_measurement();
        Ok(())
    }

    /// Prepare optional LRA storage before accepting audio.
    ///
    /// Existing constructors leave LRA off. Configuration persists across reset.
    /// Preparation allocates and belongs on the control thread.
    ///
    /// # Errors
    /// Returns an error for invalid capacity, allocation failure, or a changed
    /// configuration after audio has been accepted. An unchanged policy is a no-op.
    pub fn with_loudness_range(
        mut self,
        config: Option<LoudnessRangeConfig>,
    ) -> Result<Self, String> {
        self.configure_loudness_range(config)?;
        Ok(self)
    }

    /// Return the prepared LRA policy, or `None` when disabled.
    pub fn loudness_range_config(&self) -> Option<LoudnessRangeConfig> {
        self.loudness_range
            .as_ref()
            .map(LoudnessRangeHistory::config)
    }

    fn configure_loudness_range(
        &mut self,
        config: Option<LoudnessRangeConfig>,
    ) -> Result<(), String> {
        if self.loudness_range_config() == config {
            return Ok(());
        }
        if self.frames_seen != 0 {
            return Err("loudness range must be configured before accepting audio".into());
        }
        let prepared = config
            .map(|config| LoudnessRangeHistory::new(config, self.sample_rate))
            .transpose()?;
        self.loudness_range = prepared;
        Ok(())
    }

    fn empty_loudness_range_data(&self) -> Option<LoudnessRangeData> {
        let is_stable = self.loudness_range_is_stable();
        self.loudness_range.as_ref().map(|history| {
            let mut data = history.empty_data();
            data.is_stable = is_stable;
            data
        })
    }

    fn loudness_range_is_stable(&self) -> bool {
        self.integrated_frames_seen
            >= (self.sample_rate * LRA_STABILITY_SECONDS as f64).ceil() as u64
    }

    pub fn channel_layout(&self) -> Option<&ChannelLayout> {
        self.channel_layout.as_ref()
    }

    /// Enable / disable the inter-channel Pearson r matrix.
    ///
    /// Default is `false`. When enabled, `add_frames` accumulates correlation
    /// state and `update_loudness_data` writes the matrix into
    /// `LoudnessData.correlation_matrix`. When disabled, both paths skip the
    /// extra work and the matrix stays empty.
    pub fn set_spatial_enabled(&mut self, enabled: bool) {
        if !enabled && self.spatial_enabled {
            // Leaving the on-state: clear so the next enable starts fresh.
            self.correlation_matrix.reset();
        }
        self.spatial_enabled = enabled;
    }

    /// Builder-style helper for `set_spatial_enabled(true)`.
    pub fn with_spatial(mut self) -> Self {
        self.set_spatial_enabled(true);
        self
    }

    /// True when the spatial correlation matrix is being maintained.
    pub fn spatial_enabled(&self) -> bool {
        self.spatial_enabled
    }

    pub fn add_frames(&mut self, samples: &[f32]) -> Result<(), String> {
        if !samples.len().is_multiple_of(self.channels as usize) {
            return Err(format!(
                "loudness input has {} samples, not a whole number of {}-channel frames",
                samples.len(),
                self.channels
            ));
        }
        // Stereo width and the optional spatial matrix share one centered,
        // per-frame EMA accumulator, so callback partitioning cannot change
        // the published result.
        if self.channels == 2 || self.spatial_enabled {
            self.correlation_matrix.add_frames(samples);
        }
        self.true_peak_meter
            .add_frames(samples, self.channels as usize);

        if let Some(peak_meter) = &mut self.peak_meter {
            peak_meter
                .add_frames_f32(samples)
                .map_err(|error| format!("EBU R128 peak add_frames failed: {error:?}"))?;
        }
        let input_channels = self.channels as usize;
        let total_frames = samples.len() / input_channels;
        let momentary_window_frames = required_momentary_frames(self.sample_rate);
        let shortterm_window_frames = required_shortterm_frames(self.sample_rate);
        let mut frame_offset = 0;
        let mut integrated_lane_received_audio = false;
        while frame_offset < total_frames {
            let live_until_boundary = self.sub_block_frames - self.frames_into_sub_block;
            let integrated_until_boundary = if self.integrated_measurement_running {
                self.sub_block_frames - self.integrated_frames_into_sub_block
            } else {
                usize::MAX
            };
            let mut chunk_frames = (total_frames - frame_offset)
                .min(live_until_boundary)
                .min(integrated_until_boundary);
            if self.peak_meter.is_some() {
                chunk_frames = chunk_frames.min(256);
            }
            let sample_start = frame_offset * input_channels;
            let sample_end = (frame_offset + chunk_frames) * input_channels;
            self.add_loudness_chunk(&samples[sample_start..sample_end])?;
            frame_offset += chunk_frames;
            self.frames_seen = self.frames_seen.saturating_add(chunk_frames as u64);
            self.frames_into_sub_block += chunk_frames;

            if self.integrated_measurement_running {
                integrated_lane_received_audio = true;
                self.integrated_frames_seen = self
                    .integrated_frames_seen
                    .saturating_add(chunk_frames as u64);
                self.integrated_frames_into_sub_block += chunk_frames;
            }

            if self.frames_into_sub_block == self.sub_block_frames {
                self.frames_into_sub_block = 0;
                if self.frames_seen >= shortterm_window_frames
                    && let Ok(observed) = self.query_shortterm()
                {
                    update_finite_maximum(&mut self.maximum_shortterm_lufs, observed);
                }
                if self.frames_seen >= momentary_window_frames
                    && let Ok(momentary) = self.query_momentary()
                {
                    update_finite_maximum(&mut self.maximum_momentary_lufs, momentary);
                }
            }

            if self.integrated_measurement_running
                && self.integrated_frames_into_sub_block == self.sub_block_frames
            {
                self.integrated_frames_into_sub_block = 0;
                self.integrated_completed_sub_blocks =
                    self.integrated_completed_sub_blocks.saturating_add(1);
                if self.integrated_completed_sub_blocks >= 30 && self.loudness_range.is_some() {
                    let shortterm = self.query_integrated_shortterm();
                    if let Some(history) = &mut self.loudness_range {
                        history.observe(shortterm);
                    }
                }
                if self.integrated_completed_sub_blocks >= 4 {
                    let momentary = self.query_integrated_momentary().map_err(|error| {
                        format!("EBU R128 integrated block query failed: {error:?}")
                    })?;
                    let energy = if momentary.is_finite() {
                        loudness_to_energy(momentary)
                    } else {
                        0.0
                    };
                    if let Some(exact) = &mut self.whole_program_integrated {
                        exact.push(energy);
                    }
                    if let Some(explicit) = &mut self.integrated_explicit_loudness {
                        explicit.push_integrated_block(energy);
                    }
                }
            }
        }
        if integrated_lane_received_audio && let Some(exact) = &mut self.whole_program_integrated {
            // At most one bounded two-pass scan per callback, irrespective of
            // how many 100 ms boundaries an offline block crossed.
            exact.refresh();
        }
        if integrated_lane_received_audio
            && let Some(explicit) = &mut self.integrated_explicit_loudness
        {
            explicit.refresh_integrated();
        }
        Ok(())
    }

    /// Complete the true-peak interpolation response of a finite segment.
    ///
    /// Measures the eleven trailing zero-input intervals without adding audio
    /// frames to loudness, LRA, sample-peak or correlation measurements. Pending
    /// peaks remain available to the next query. Repeated calls add nothing;
    /// later input starts a new interpolation segment in the same loudness epoch.
    /// This bounded operation reuses prepared storage and does not allocate.
    pub fn finish_true_peak(&mut self) {
        self.true_peak_meter.finish();
    }

    fn add_loudness_chunk(&mut self, input: &[f32]) -> Result<(), String> {
        let input_channels = self.channels as usize;
        if let Some(explicit) = &mut self.explicit_loudness {
            explicit.add_chunk(input, input_channels)?;
            if self.integrated_measurement_running {
                self.integrated_explicit_loudness
                    .as_mut()
                    .expect("prepared integrated explicit meters")
                    .add_chunk(input, input_channels)?;
            }
            return Ok(());
        }

        let meter_input = if self.peak_meter.is_some() {
            let frames = input.len() / input_channels;
            let scratch_len = frames * self.loudness_channels;
            let scratch = &mut self.weighted_scratch[..scratch_len];
            scratch.fill(0.0);
            for (input_frame, output_frame) in input
                .chunks_exact(input_channels)
                .zip(scratch.chunks_exact_mut(self.loudness_channels))
            {
                for (channel, sample) in input_frame.iter().enumerate().take(input_channels) {
                    let output_channel = self.loudness_channel_indices[channel];
                    output_frame[output_channel] = if self.loudness_channels == 7 {
                        *sample
                    } else {
                        *sample * self.loudness_gains[channel]
                    };
                }
            }
            &*scratch
        } else {
            input
        };

        self.ebur128
            .add_frames_f32(meter_input)
            .map_err(|error| format!("EBU R128 loudness add_frames failed: {error:?}"))?;
        if self.integrated_measurement_running {
            self.integrated_ebur128
                .as_mut()
                .expect("prepared integrated EBU R128 meter")
                .add_frames_f32(meter_input)
                .map_err(|error| format!("EBU R128 integrated add_frames failed: {error:?}"))?;
        }
        Ok(())
    }

    fn query_momentary(&self) -> Result<f64, String> {
        self.explicit_loudness.as_ref().map_or_else(
            || self.ebur128.loudness_momentary(),
            |explicit| explicit.momentary(),
        )
    }

    fn query_shortterm(&self) -> Result<f64, String> {
        self.explicit_loudness.as_ref().map_or_else(
            || self.ebur128.loudness_shortterm(),
            |explicit| explicit.shortterm(),
        )
    }

    fn query_integrated_momentary(&self) -> Result<f64, String> {
        self.integrated_explicit_loudness.as_ref().map_or_else(
            || {
                self.integrated_ebur128
                    .as_ref()
                    .expect("prepared integrated EBU R128 meter")
                    .loudness_momentary()
            },
            ExplicitLoudnessMeters::momentary,
        )
    }

    fn query_integrated_shortterm(&self) -> Result<f64, String> {
        self.integrated_explicit_loudness.as_ref().map_or_else(
            || {
                self.integrated_ebur128
                    .as_ref()
                    .expect("prepared integrated EBU R128 meter")
                    .loudness_shortterm()
            },
            ExplicitLoudnessMeters::shortterm,
        )
    }

    /// Update LoudnessData in-place to avoid allocations
    pub fn update_loudness_data(&mut self, d: &mut LoudnessData) {
        let is_stable = self.loudness_range_is_stable();
        d.loudness_range = self.loudness_range.as_mut().map(|history| {
            let mut data = history.query();
            data.is_stable = is_stable;
            data
        });
        let momentary = self.query_momentary();
        let shortterm = self.query_shortterm();
        let integrated = match &self.whole_program_integrated {
            Some(exact) if exact.capacity_exceeded => Ok(f64::NEG_INFINITY),
            Some(exact) => Ok(exact.cached_loudness),
            None => match &self.integrated_explicit_loudness {
                Some(explicit) => Ok(explicit.integrated().unwrap_or(f64::NEG_INFINITY)),
                None => self
                    .integrated_ebur128
                    .as_ref()
                    .expect("prepared integrated EBU R128 meter")
                    .loudness_global(),
            },
        };
        let momentary_ok = momentary.is_ok();
        let shortterm_ok = shortterm.is_ok();
        let integrated_ok = integrated.is_ok();
        let mut meter_query_failed = !momentary_ok || !shortterm_ok || !integrated_ok;
        d.momentary_lufs = momentary.unwrap_or(f64::NEG_INFINITY);
        d.shortterm_lufs = shortterm.unwrap_or(f64::NEG_INFINITY);
        d.integrated_lufs = integrated.unwrap_or(f64::NEG_INFINITY);
        d.integrated_measurement_running = self.integrated_measurement_running;

        // Use the pre-allocated per-channel buffers (no stack-array channel
        // limit, so 22.2 / Atmos beds are not silently truncated).
        let nc = self.channels as usize;
        if self.peaks_buf.len() < nc {
            self.peaks_buf.resize(nc, 0.0);
            self.true_peaks_buf.resize(nc, 0.0);
        }
        let peaks = &mut self.peaks_buf[..nc];
        let tps = &mut self.true_peaks_buf[..nc];
        let mut sample_peak_failed = false;
        let mut maximum_true_peak_dbtp = self.maximum_true_peak_dbtp;

        for ch in 0..nc {
            let peak_meter = self.peak_meter.as_mut().unwrap_or(&mut self.ebur128);
            let sample_peak = peak_meter.prev_sample_peak(ch as u32);
            let true_peak = self.true_peak_meter.take_interval_peak(ch);
            sample_peak_failed |= sample_peak.is_err();
            peaks[ch] = sample_peak.unwrap_or(0.0);
            let tp_linear = true_peak.unwrap_or(0.0);
            tps[ch] = if tp_linear > 0.0 {
                20.0 * tp_linear.log10()
            } else {
                f64::NEG_INFINITY
            };
            if tps[ch].is_finite() {
                maximum_true_peak_dbtp =
                    Some(maximum_true_peak_dbtp.map_or(tps[ch], |maximum| maximum.max(tps[ch])));
            }
        }
        self.maximum_true_peak_dbtp = maximum_true_peak_dbtp;
        meter_query_failed |= sample_peak_failed;

        d.update_peaks(peaks);
        d.update_true_peaks(tps);
        d.maximum_true_peak_dbtp = maximum_true_peak_dbtp;
        d.maximum_momentary_lufs = self.maximum_momentary_lufs;
        d.maximum_shortterm_lufs = self.maximum_shortterm_lufs;

        d.peak = d.channel_peaks.iter().copied().fold(0.0, f64::max);
        let capacity_exceeded = self
            .whole_program_integrated
            .as_ref()
            .is_some_and(|exact| exact.capacity_exceeded);
        let newly_exceeded = self.whole_program_integrated.as_mut().is_some_and(|exact| {
            if exact.capacity_exceeded && !exact.capacity_error_published {
                exact.capacity_error_published = true;
                true
            } else {
                false
            }
        });
        if meter_query_failed || newly_exceeded {
            self.query_error_generation = self.query_error_generation.saturating_add(1);
        }
        d.query_error = if capacity_exceeded {
            Some(LoudnessQueryError::IntegratedProgramCapacityExceeded)
        } else if meter_query_failed {
            Some(LoudnessQueryError::MeterQueryFailed)
        } else {
            None
        };
        d.momentary_valid =
            momentary_ok && self.frames_seen >= required_momentary_frames(self.sample_rate);
        d.shortterm_valid =
            shortterm_ok && self.frames_seen >= required_shortterm_frames(self.sample_rate);
        d.integrated_valid =
            integrated_ok && !capacity_exceeded && self.integrated_completed_sub_blocks >= 4;
        d.sample_peak_valid = !sample_peak_failed;
        d.true_peak_valid = self.true_peak_meter.compliant;
        d.measurement_valid =
            d.momentary_valid && d.shortterm_valid && d.integrated_valid && d.sample_peak_valid;
        d.measurement_enabled = true;
        d.channel_layout_is_compliant = self.channel_layout.is_some() || self.channels <= 2;
        d.query_error_generation = self.query_error_generation;
        d.true_peak_is_compliant = self.true_peak_meter.compliant;
        d.integrated_mode = self.integrated_mode;
        d.integrated_window_seconds = INTEGRATED_HISTORY_SECONDS;
        if self.channels == 2 {
            self.correlation_matrix
                .update_correlation_data(&mut self.matrix_scratch);
            d.correlation_lr = (self.matrix_scratch.samples_seen >= 2)
                .then(|| self.matrix_scratch.matrix[1] as f64);
        } else {
            d.correlation_lr = None;
        }

        if self.spatial_enabled {
            // Refresh the inter-channel correlation matrix. We write into a
            // re-used scratch CorrelationData so the matrix Vec is allocated
            // exactly once per LoudnessMonitor instance, then copy the slice
            // into LoudnessData.
            self.correlation_matrix
                .update_correlation_data(&mut self.matrix_scratch);
            d.update_correlation_matrix(&self.matrix_scratch.matrix);
            d.correlation_samples_seen = self.correlation_matrix.samples_seen();
        } else {
            // Spatial off → emit an empty matrix so downstream consumers can
            // unambiguously detect "feature disabled" via `is_empty()`.
            d.update_correlation_matrix(&[]);
            d.correlation_samples_seen = 0;
        }
    }

    pub fn get_loudness(&mut self) -> LoudnessData {
        let mut d = LoudnessData::new(self.channels as usize);
        self.update_loudness_data(&mut d);
        d
    }

    pub fn reset(&mut self) -> Result<(), String> {
        self.ebur128.reset();
        if let Some(explicit) = &mut self.explicit_loudness {
            explicit.reset();
        }
        if let Some(peak_meter) = &mut self.peak_meter {
            peak_meter.reset();
        }
        if let Some(integrated_ebur128) = &mut self.integrated_ebur128 {
            integrated_ebur128.reset();
        }
        if let Some(explicit) = &mut self.integrated_explicit_loudness {
            explicit.reset();
        }
        self.correlation_matrix.reset();
        self.true_peak_meter.reset();
        self.maximum_true_peak_dbtp = None;
        self.maximum_momentary_lufs = None;
        self.maximum_shortterm_lufs = None;
        self.query_error_generation = 0;
        self.frames_seen = 0;
        self.frames_into_sub_block = 0;
        self.integrated_frames_seen = 0;
        self.integrated_frames_into_sub_block = 0;
        self.integrated_completed_sub_blocks = 0;
        if let Some(history) = &mut self.loudness_range {
            history.reset();
        }
        if let Some(exact) = &mut self.whole_program_integrated {
            exact.reset();
        }
        Ok(())
    }
}

/// Measure passthrough audio and publish prepared, immutable loudness snapshots.
///
/// Publication is best effort when readers retain snapshots or their nested
/// arrays. A retained candidate is skipped without changing any of its fields;
/// accumulation continues while enabled. Reset and enable transitions may leave
/// the previous complete snapshot visible until a prepared candidate becomes
/// writable. A subsequent disabled callback retries a pending cleared snapshot.
/// True-peak query intervals extend across skipped publications, as they do
/// under outer-cache contention.
/// Drain finishes only the true-peak interpolation response and extends the
/// last published peak interval without emitting audio. Publication remains
/// best effort; a direct repeated drain retries if retained readers prevented
/// publication. Once published, the final snapshot is stable until new input.
///
/// Loudness Range defaults to 36,000 rolling observations with 576,000 bytes of
/// prepared history/scratch. [`Self::with_loudness_range`] selects a different
/// bounded policy or disables the statistic. Dirty percentile queries perform
/// linear work in retained history; cached queries reuse their scalar result.
pub struct LoudnessMonitorPlugin {
    num_channels: usize,
    sample_rate: f64,
    integrated_control_instance_id: u64,
    last_integrated_control_request_id: u64,
    last_integrated_control_operation: Option<IntegratedControlOperation>,
    initialized: bool,
    enabled: bool,
    // A reset/enable transition may wait for an unretained complete snapshot.
    clear_publication_pending: bool,
    drain_finished: bool,
    drain_publication_pending: bool,
    cache: RealTimeCache<LoudnessData>,
    monitor: LoudnessMonitor,
    channel_layout: Option<ChannelLayout>,
    cached_parameters: Vec<Parameter>,
}

impl LoudnessMonitorPlugin {
    pub fn new(num_channels: usize) -> Result<Self, String> {
        Self::new_inner(num_channels, None)
    }

    pub fn with_channel_layout(channel_layout: ChannelLayout) -> Result<Self, String> {
        Self::new_inner(channel_layout.channels.len(), Some(channel_layout))
    }

    pub fn new_with_layout(
        num_channels: usize,
        channel_layout: ChannelLayout,
    ) -> Result<Self, String> {
        Self::new_inner(num_channels, Some(channel_layout))
    }

    fn new_inner(
        num_channels: usize,
        channel_layout: Option<ChannelLayout>,
    ) -> Result<Self, String> {
        if num_channels == 0 {
            return Err("loudness monitor requires at least one channel".to_string());
        }
        if let Some(layout) = &channel_layout {
            layout.validate_for_width(num_channels)?;
        }
        let sr = 48000.0;
        let integrated_control_instance_id =
            allocate_loudness_control_instance_id(&NEXT_LOUDNESS_CONTROL_INSTANCE_ID)?;
        let monitor = if let Some(layout) = &channel_layout {
            LoudnessMonitor::new_with_layout(num_channels as u32, sr, layout.clone())?
        } else {
            LoudnessMonitor::new(num_channels as u32, sr)?
        }
        .with_loudness_range(Some(LoudnessRangeConfig::default()))?;
        let layout_compliant = channel_layout.is_some() || num_channels <= 2;
        let cache = new_loudness_cache(
            num_channels,
            false,
            layout_compliant,
            IntegratedLoudnessMode::Rolling,
            true,
            LoudnessMaximums::from_monitor(&monitor, integrated_control_instance_id, 0),
            monitor.empty_loudness_range_data(),
        );
        let mut p = Self {
            num_channels,
            sample_rate: sr,
            integrated_control_instance_id,
            last_integrated_control_request_id: 0,
            last_integrated_control_operation: None,
            initialized: false,
            enabled: true,
            clear_publication_pending: false,
            drain_finished: false,
            drain_publication_pending: false,
            cache,
            monitor,
            channel_layout,
            cached_parameters: Vec::new(),
        };
        p.rebuild_cached_parameters();
        Ok(p)
    }

    pub fn channel_layout(&self) -> Option<&ChannelLayout> {
        self.channel_layout.as_ref()
    }

    fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = vec![
            Parameter::new_bool("enabled", "Enabled", self.enabled),
            Parameter::new_bool(
                "integrated_running",
                "Integrated Measurement Running",
                self.monitor.integrated_measurement_running(),
            ),
            Parameter::new_string(
                "integrated_control_command",
                "Integrated Measurement Command",
                String::new(),
            ),
        ];
    }

    fn set_cached_bool(&mut self, parameter_id: &str, value: bool) {
        if let Some(parameter) = self
            .cached_parameters
            .iter_mut()
            .find(|parameter| parameter.id.as_str() == parameter_id)
        {
            parameter.default_value = ParameterValue::Bool(value);
        }
    }

    /// Return the live control-thread Integrated/LRA state.
    pub fn integrated_measurement_running(&self) -> bool {
        self.monitor.integrated_measurement_running()
    }

    /// Runtime identity used to target transient lifecycle commands safely.
    pub fn integrated_control_instance_id(&self) -> u64 {
        self.integrated_control_instance_id
    }

    /// Latest command accepted by this runtime instance, independent of whether
    /// a retained snapshot currently permits its receipt to be published.
    pub fn integrated_control_request_id(&self) -> u64 {
        self.last_integrated_control_request_id
    }

    fn apply_integrated_control_command(&mut self, value: &str) -> PluginResult<()> {
        // The empty metadata default is deliberately inert so generic state
        // restoration can enumerate parameters without replaying a command.
        if value.is_empty() {
            return Ok(());
        }
        let command = parse_integrated_control_command(value).map_err(str::to_string)?;
        if command.instance_id != self.integrated_control_instance_id {
            return Err("integrated control command targets a replaced monitor".to_string());
        }
        if command.request_id < self.last_integrated_control_request_id {
            return Err("integrated control request is older than the applied request".to_string());
        }
        if command.request_id == self.last_integrated_control_request_id {
            if self.last_integrated_control_operation != Some(command.operation) {
                return Err(
                    "integrated control request ID was reused for another operation".to_string(),
                );
            }
            self.publish_integrated_control_state();
            return Ok(());
        }

        let previous_request_id = self.last_integrated_control_request_id;
        let previous_operation = self.last_integrated_control_operation;
        self.last_integrated_control_request_id = command.request_id;
        self.last_integrated_control_operation = Some(command.operation);

        let result = match command.operation {
            IntegratedControlOperation::Start => self.start_integrated_measurement(),
            IntegratedControlOperation::Pause => {
                self.monitor.pause_integrated_measurement();
                self.set_cached_bool("integrated_running", false);
                Ok(())
            }
            IntegratedControlOperation::Continue => {
                self.monitor.continue_integrated_measurement();
                self.set_cached_bool("integrated_running", true);
                Ok(())
            }
            IntegratedControlOperation::Reset => self.clear_measurement(),
        };
        if let Err(error) = result {
            self.last_integrated_control_request_id = previous_request_id;
            self.last_integrated_control_operation = previous_operation;
            return Err(error);
        }

        self.publish_integrated_control_state();
        Ok(())
    }

    /// Pause only Integrated Loudness and Loudness Range accumulation.
    pub fn pause_integrated_measurement(&mut self) {
        if !self.monitor.integrated_measurement_running() {
            return;
        }
        self.monitor.pause_integrated_measurement();
        self.set_cached_bool("integrated_running", false);
        self.publish_integrated_control_state();
    }

    /// Continue Integrated Loudness and Loudness Range without clearing history.
    pub fn continue_integrated_measurement(&mut self) {
        if self.monitor.integrated_measurement_running() {
            return;
        }
        self.monitor.continue_integrated_measurement();
        self.set_cached_bool("integrated_running", true);
        self.publish_integrated_control_state();
    }

    /// Start a fresh full measurement epoch and enable Integrated/LRA accumulation.
    pub fn start_integrated_measurement(&mut self) -> Result<(), String> {
        self.monitor.start_integrated_measurement()?;
        self.set_cached_bool("integrated_running", true);
        self.clear_cached_measurement();
        Ok(())
    }

    fn clear_measurement(&mut self) -> Result<(), String> {
        self.monitor.reset()?;
        self.clear_cached_measurement();
        Ok(())
    }

    fn clear_cached_measurement(&mut self) {
        self.drain_finished = false;
        self.drain_publication_pending = false;
        self.clear_publication_pending = true;
        // Retain the existing bounded control/reset attempts. Every later
        // successful writer fills an entire candidate, including dirty spares.
        for _ in 0..3 {
            self.publish_cleared_data();
        }
    }

    fn publish_cleared_data(&mut self) {
        let channels = self.num_channels;
        let spatial = self.monitor.spatial_enabled();
        let layout_compliant = self.channel_layout.is_some() || channels <= 2;
        let integrated_mode = self.monitor.integrated_mode();
        let enabled = self.enabled;
        let integrated_measurement_running = self.monitor.integrated_measurement_running();
        let loudness_range = self.monitor.empty_loudness_range_data();
        let integrated_control_instance_id = self.integrated_control_instance_id;
        let integrated_control_request_id = self.last_integrated_control_request_id;
        if self.cache.update_if(
            |data| loudness_data_is_writable(data, channels, spatial),
            |data| {
                reset_loudness_data(
                    data,
                    LoudnessDataResetConfig {
                        channels,
                        spatial,
                        layout_compliant,
                        integrated_mode,
                        enabled,
                        integrated_measurement_running,
                        loudness_range,
                        integrated_control_instance_id,
                        integrated_control_request_id,
                    },
                )
            },
        ) {
            self.clear_publication_pending = false;
        }
    }

    fn publish_integrated_control_state(&mut self) {
        if self.clear_publication_pending {
            self.publish_cleared_data();
            return;
        }
        let channels = self.num_channels;
        let spatial = self.monitor.spatial_enabled();
        let running = self.monitor.integrated_measurement_running();
        let previous = self.cache.load();
        let integrated_control_instance_id = self.integrated_control_instance_id;
        let integrated_control_request_id = self.last_integrated_control_request_id;
        self.cache.update_if(
            |data| loudness_data_is_writable(data, channels, spatial),
            |data| {
                data.update_from(&previous);
                data.integrated_measurement_running = running;
                data.integrated_control_instance_id = integrated_control_instance_id;
                data.integrated_control_request_id = integrated_control_request_id;
            },
        );
    }

    fn publish_final_data(&mut self) {
        let previous = self.cache.load();
        let merge_previous = !self.clear_publication_pending;
        let channels = self.num_channels;
        let spatial = self.monitor.spatial_enabled();
        let integrated_control_instance_id = self.integrated_control_instance_id;
        let integrated_control_request_id = self.last_integrated_control_request_id;
        let monitor = &mut self.monitor;
        if self.cache.update_if(
            |data| loudness_data_is_writable(data, channels, spatial),
            |data| {
                monitor.update_loudness_data(data);
                data.integrated_control_instance_id = integrated_control_instance_id;
                data.integrated_control_request_id = integrated_control_request_id;
                if merge_previous {
                    // Readiness above proves unique ownership of every nested
                    // array before any query consumes the pending peak interval.
                    for (peak, old) in Arc::get_mut(&mut data.channel_peaks)
                        .expect("prepared sample peaks")
                        .iter_mut()
                        .zip(previous.channel_peaks.iter())
                    {
                        *peak = peak.max(*old);
                    }
                    data.peak = data.channel_peaks.iter().copied().fold(0.0, f64::max);
                    for (peak, old) in Arc::get_mut(&mut data.true_peaks_dbtp)
                        .expect("prepared true peaks")
                        .iter_mut()
                        .zip(previous.true_peaks_dbtp.iter())
                    {
                        *peak = peak.max(*old);
                    }
                }
            },
        ) {
            self.clear_publication_pending = false;
            self.drain_publication_pending = false;
        }
    }

    /// Toggle the inter-channel correlation matrix on the embedded monitor.
    ///
    /// Off by default. The audio engine flips this on for the output-side
    /// LoudnessMonitor so the spatial-spider widget has data to display; all
    /// other LoudnessMonitor instances (input-side, per-meter, CLI, ad-hoc)
    /// stay off and pay zero overhead.
    /// This prepares snapshot storage and must run on the control thread.
    pub fn set_spatial_enabled(&mut self, enabled: bool) {
        self.monitor.set_spatial_enabled(enabled);
        // Enabling spatial data is a control-thread structural operation.
        // Rebuild all three cache slots now so the first audio callback only copies.
        self.cache = new_loudness_cache(
            self.num_channels,
            enabled,
            self.channel_layout.is_some() || self.num_channels <= 2,
            self.monitor.integrated_mode(),
            self.enabled,
            LoudnessMaximums::from_monitor(
                &self.monitor,
                self.integrated_control_instance_id,
                self.last_integrated_control_request_id,
            ),
            self.monitor.empty_loudness_range_data(),
        );
        self.clear_publication_pending = false;
        self.drain_finished = false;
        self.drain_publication_pending = false;
    }

    /// Prepare spatial snapshots on the control thread, including after initialization.
    pub fn with_spatial(mut self) -> Self {
        self.set_spatial_enabled(true);
        self
    }

    /// Prepare optional loudness-range storage before accepting audio.
    ///
    /// The analyzer defaults to rolling history; internal low-level meters
    /// default to off. This builder allocates only on the control thread.
    ///
    /// # Errors
    /// Returns an error for invalid capacity, preparation failure, or a changed
    /// configuration after this measurement epoch has accepted audio.
    pub fn with_loudness_range(
        mut self,
        config: Option<LoudnessRangeConfig>,
    ) -> Result<Self, String> {
        if self.monitor.loudness_range_config() == config {
            return Ok(self);
        }
        self.monitor.configure_loudness_range(config)?;
        self.cache = new_loudness_cache(
            self.num_channels,
            self.monitor.spatial_enabled(),
            self.channel_layout.is_some() || self.num_channels <= 2,
            self.monitor.integrated_mode(),
            self.enabled,
            LoudnessMaximums::from_monitor(
                &self.monitor,
                self.integrated_control_instance_id,
                self.last_integrated_control_request_id,
            ),
            self.monitor.empty_loudness_range_data(),
        );
        self.clear_publication_pending = false;
        self.drain_finished = false;
        self.drain_publication_pending = false;
        Ok(self)
    }

    /// Return the prepared loudness-range policy, or `None` when disabled.
    pub fn loudness_range_config(&self) -> Option<LoudnessRangeConfig> {
        self.monitor.loudness_range_config()
    }

    /// Select integrated-history policy before realtime processing begins.
    /// Whole-program mode prepares its complete bounded store here, never in
    /// `process`.
    pub fn with_integrated_mode(mut self, mode: IntegratedLoudnessMode) -> Result<Self, String> {
        let spatial = self.monitor.spatial_enabled();
        let integrated_measurement_running = self.monitor.integrated_measurement_running();
        let range_config = self.monitor.loudness_range_config();
        let mut monitor = if let Some(layout) = &self.channel_layout {
            LoudnessMonitor::new_with_layout_and_integrated_mode(
                self.num_channels as u32,
                self.sample_rate,
                layout.clone(),
                mode,
            )?
        } else {
            LoudnessMonitor::new_with_integrated_mode(
                self.num_channels as u32,
                self.sample_rate,
                mode,
            )?
        }
        .with_loudness_range(range_config)?;
        monitor.set_spatial_enabled(spatial);
        monitor.set_integrated_measurement_running(integrated_measurement_running);
        self.cache = new_loudness_cache(
            self.num_channels,
            spatial,
            self.channel_layout.is_some() || self.num_channels <= 2,
            mode,
            self.enabled,
            LoudnessMaximums::from_monitor(
                &monitor,
                self.integrated_control_instance_id,
                self.last_integrated_control_request_id,
            ),
            monitor.empty_loudness_range_data(),
        );
        self.monitor = monitor;
        self.clear_publication_pending = false;
        self.drain_finished = false;
        self.drain_publication_pending = false;
        Ok(self)
    }
}

impl Plugin for LoudnessMonitorPlugin {
    fn guarantees_identity_frame_geometry(&self) -> bool {
        // Analysis observes the input without changing the audio frame clock.
        true
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        // Retained measurement state never emits audio.
        crate::plugin::TailLength::Finite(0)
    }

    fn info(&self) -> PluginInfo {
        PluginInfo::new("Loudness Monitor", "1.2.0", "Sotf")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Analyzer
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::analyzer(Some(PluginCompiledOp::AnalyzerTap))
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }
    fn output_channels(&self) -> usize {
        self.num_channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        let parameter = self
            .cached_parameters
            .iter()
            .find(|parameter| parameter.id == id)
            .ok_or_else(|| format!("Unknown parameter: {id}"))?;
        parameter
            .validate(&value)
            .map_err(|error| format!("{id}: {error}"))?;
        if id.as_str() == "integrated_control_command" {
            return self.apply_integrated_control_command(
                value
                    .as_string()
                    .expect("string parameter validation ensures a string"),
            );
        } else if id.as_str() == "enabled" {
            let enabled = value.as_bool().unwrap_or(true);
            if self.enabled != enabled {
                self.enabled = enabled;
                if !enabled {
                    // An enabled transition starts a fresh running epoch even
                    // when the control was paused immediately beforehand.
                    self.monitor.continue_integrated_measurement();
                    self.set_cached_bool("integrated_running", true);
                    self.clear_measurement()?;
                } else {
                    // Re-enabling creates a fresh running programme epoch.
                    self.monitor.continue_integrated_measurement();
                    self.set_cached_bool("integrated_running", true);
                    // Disable already reset the meter. Fill the complete
                    // candidate so a previously retained spare cannot return
                    // pre-disable arrays with a newly enabled scalar flag.
                    self.clear_cached_measurement();
                }
                self.set_cached_bool("enabled", enabled);
            }
        } else if id.as_str() == "integrated_running" {
            let running = value.as_bool().unwrap_or(true);
            if self.monitor.integrated_measurement_running() != running {
                self.monitor.set_integrated_measurement_running(running);
                self.set_cached_bool("integrated_running", running);
                self.publish_integrated_control_state();
            }
        }
        Ok(())
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "enabled" => Some(ParameterValue::Bool(self.enabled)),
            "integrated_running" => Some(ParameterValue::Bool(
                self.monitor.integrated_measurement_running(),
            )),
            "integrated_control_command" => Some(ParameterValue::String(String::new())),
            _ => None,
        }
    }
    fn initialize(&mut self, sr: f64) -> PluginResult<()> {
        if !sr.is_finite() || sr < 10.0 {
            return Err("loudness monitor sample rate must be at least 10 Hz".to_string());
        }
        // Preserve the spatial-enable bit across reinitialisation so callers
        // that opted in once don't silently lose the matrix after a sample-
        // rate or channel-count change.
        let spatial = self.monitor.spatial_enabled();
        let integrated_mode = self.monitor.integrated_mode();
        let range_config = self.monitor.loudness_range_config();
        let mut monitor = if let Some(layout) = &self.channel_layout {
            LoudnessMonitor::new_with_layout_and_integrated_mode(
                self.num_channels as u32,
                sr,
                layout.clone(),
                integrated_mode,
            )?
        } else {
            LoudnessMonitor::new_with_integrated_mode(
                self.num_channels as u32,
                sr,
                integrated_mode,
            )?
        }
        .with_loudness_range(range_config)?;
        monitor.set_spatial_enabled(spatial);
        let cache = new_loudness_cache(
            self.num_channels,
            spatial,
            self.channel_layout.is_some() || self.num_channels <= 2,
            integrated_mode,
            self.enabled,
            LoudnessMaximums::from_monitor(
                &monitor,
                self.integrated_control_instance_id,
                self.last_integrated_control_request_id,
            ),
            monitor.empty_loudness_range_data(),
        );
        self.sample_rate = sr;
        self.monitor = monitor;
        self.set_cached_bool("integrated_running", true);
        self.cache = cache;
        self.clear_publication_pending = false;
        self.drain_finished = false;
        self.drain_publication_pending = false;
        self.initialized = true;
        Ok(())
    }
    fn reset(&mut self) {
        if let Err(e) = self.monitor.reset() {
            crate::rate_limited_log!(warn, 5, "loudness monitor reset failed: {e}");
        }
        self.clear_cached_measurement();
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let expected_samples = self.validate_analyzer_input(input, context)?;
        if output.len() != expected_samples {
            return Err(format!(
                "loudness monitor expected {expected_samples} output samples for {} frames x {} channels, got output={}",
                context.num_frames,
                self.num_channels,
                output.len()
            ));
        }
        output.copy_from_slice(input);
        self.process_analyzer_input(input, context)
    }
    fn process_analyzer_tap_f32(
        &mut self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        Some(
            self.validate_analyzer_input(input, context)
                .and_then(|_| self.process_analyzer_input(input, context)),
        )
    }
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        std::num::NonZeroU64::new(1)
    }

    fn drain(
        &mut self,
        _output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        // An empty input validates initialization, rate and zero source frames
        // before changing the meter or consuming a publication interval.
        self.validate_analyzer_input(&[], context)?;
        if !self.enabled {
            if self.clear_publication_pending {
                self.publish_cleared_data();
            }
            return Ok(PluginDrainResult::COMPLETE);
        }
        if !self.drain_finished {
            self.monitor.finish_true_peak();
            self.drain_finished = true;
            self.drain_publication_pending = true;
        }
        if self.drain_publication_pending {
            self.publish_final_data();
        }
        Ok(PluginDrainResult::COMPLETE)
    }
    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        if op != PluginCompiledOp::AnalyzerTap {
            return None;
        }
        Some(self.process(input, output, context))
    }
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }
    fn take_cache_contention_stats(&mut self) -> (u64, u64) {
        self.cache.take_contention_stats()
    }
}

impl LoudnessMonitorPlugin {
    fn validate_analyzer_input(
        &self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if !self.initialized {
            return Err("loudness monitor must be initialized before processing".to_string());
        }
        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "loudness monitor initialized at {} Hz but received {} Hz context",
                self.sample_rate, context.sample_rate
            ));
        }
        let expected_samples = context
            .num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "loudness monitor frame/channel count overflow".to_string())?;
        if input.len() != expected_samples {
            return Err(format!(
                "loudness monitor expected {expected_samples} input samples for {} frames x {} channels, got input={}",
                context.num_frames,
                self.num_channels,
                input.len()
            ));
        }
        Ok(expected_samples)
    }

    fn process_analyzer_input(
        &mut self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if !self.enabled {
            if self.clear_publication_pending {
                self.publish_cleared_data();
            } else {
                self.publish_integrated_control_state();
            }
            return Ok(context.num_frames);
        }
        if input.is_empty() && self.drain_finished {
            if self.drain_publication_pending {
                self.publish_final_data();
            }
            return Ok(0);
        }
        self.monitor.add_frames(input)?;
        if !input.is_empty() {
            self.drain_finished = false;
            self.drain_publication_pending = false;
        }

        // Nested readers may retain any array after releasing their outer
        // snapshot. Reject the whole candidate before changing scalar fields.
        // A skipped query retains the existing true-peak interval accumulator
        // until the next publication, as with outer-cache contention.
        let channels = self.num_channels;
        let spatial = self.monitor.spatial_enabled();
        let integrated_control_instance_id = self.integrated_control_instance_id;
        let integrated_control_request_id = self.last_integrated_control_request_id;
        let monitor = &mut self.monitor;
        if self.cache.update_if(
            |data| loudness_data_is_writable(data, channels, spatial),
            |data| {
                monitor.update_loudness_data(data);
                data.integrated_control_instance_id = integrated_control_instance_id;
                data.integrated_control_request_id = integrated_control_request_id;
            },
        ) {
            self.clear_publication_pending = false;
        }
        Ok(context.num_frames)
    }
}

fn loudness_data_is_writable(data: &mut LoudnessData, channels: usize, spatial: bool) -> bool {
    let matrix_len = if spatial {
        let Some(len) = channels.checked_mul(channels) else {
            return false;
        };
        len
    } else {
        0
    };
    // Authoritative mutable access rejects both strong and Weak readers. Do
    // not create/export nested owners between readiness and the complete write.
    Arc::get_mut(&mut data.channel_peaks).is_some_and(|values| values.len() == channels)
        && Arc::get_mut(&mut data.true_peaks_dbtp).is_some_and(|values| values.len() == channels)
        && Arc::get_mut(&mut data.correlation_matrix)
            .is_some_and(|values| values.len() == matrix_len)
}

#[derive(Clone, Copy)]
struct LoudnessMaximums {
    true_peak_dbtp: Option<f64>,
    momentary_lufs: Option<f64>,
    shortterm_lufs: Option<f64>,
    integrated_measurement_running: bool,
    integrated_control_instance_id: u64,
    integrated_control_request_id: u64,
}

impl LoudnessMaximums {
    fn from_monitor(
        monitor: &LoudnessMonitor,
        integrated_control_instance_id: u64,
        integrated_control_request_id: u64,
    ) -> Self {
        Self {
            true_peak_dbtp: monitor.maximum_true_peak_dbtp,
            momentary_lufs: monitor.maximum_momentary_lufs,
            shortterm_lufs: monitor.maximum_shortterm_lufs,
            integrated_measurement_running: monitor.integrated_measurement_running(),
            integrated_control_instance_id,
            integrated_control_request_id,
        }
    }
}

fn new_loudness_data(
    channels: usize,
    spatial: bool,
    layout_compliant: bool,
    integrated_mode: IntegratedLoudnessMode,
    enabled: bool,
    maxima: LoudnessMaximums,
    loudness_range: Option<LoudnessRangeData>,
) -> LoudnessData {
    let mut data = LoudnessData::new(channels);
    data.channel_layout_is_compliant = layout_compliant;
    data.integrated_mode = integrated_mode;
    data.measurement_enabled = enabled;
    data.integrated_measurement_running = maxima.integrated_measurement_running;
    data.integrated_control_instance_id = maxima.integrated_control_instance_id;
    data.integrated_control_request_id = maxima.integrated_control_request_id;
    data.maximum_true_peak_dbtp = maxima.true_peak_dbtp;
    data.maximum_momentary_lufs = maxima.momentary_lufs;
    data.maximum_shortterm_lufs = maxima.shortterm_lufs;
    data.loudness_range = loudness_range;
    if spatial {
        data.update_correlation_matrix(&vec![0.0; channels.saturating_mul(channels)]);
    }
    data
}

fn new_loudness_cache(
    channels: usize,
    spatial: bool,
    layout_compliant: bool,
    integrated_mode: IntegratedLoudnessMode,
    enabled: bool,
    maxima: LoudnessMaximums,
    loudness_range: Option<LoudnessRangeData>,
) -> RealTimeCache<LoudnessData> {
    RealTimeCache::new_triplet(
        new_loudness_data(
            channels,
            spatial,
            layout_compliant,
            integrated_mode,
            enabled,
            maxima,
            loudness_range,
        ),
        new_loudness_data(
            channels,
            spatial,
            layout_compliant,
            integrated_mode,
            enabled,
            maxima,
            loudness_range,
        ),
        new_loudness_data(
            channels,
            spatial,
            layout_compliant,
            integrated_mode,
            enabled,
            maxima,
            loudness_range,
        ),
    )
}

struct LoudnessDataResetConfig {
    channels: usize,
    spatial: bool,
    layout_compliant: bool,
    integrated_mode: IntegratedLoudnessMode,
    enabled: bool,
    integrated_measurement_running: bool,
    loudness_range: Option<LoudnessRangeData>,
    integrated_control_instance_id: u64,
    integrated_control_request_id: u64,
}

fn reset_loudness_data(data: &mut LoudnessData, config: LoudnessDataResetConfig) {
    data.measurement_valid = false;
    data.query_error_generation = 0;
    data.query_error = None;
    data.measurement_enabled = config.enabled;
    data.integrated_measurement_running = config.integrated_measurement_running;
    data.integrated_control_instance_id = config.integrated_control_instance_id;
    data.integrated_control_request_id = config.integrated_control_request_id;
    data.momentary_valid = false;
    data.shortterm_valid = false;
    data.integrated_valid = false;
    data.sample_peak_valid = false;
    data.true_peak_valid = false;
    data.channel_layout_is_compliant = config.layout_compliant;
    data.momentary_lufs = f64::NEG_INFINITY;
    data.shortterm_lufs = f64::NEG_INFINITY;
    data.integrated_lufs = f64::NEG_INFINITY;
    data.integrated_mode = config.integrated_mode;
    data.loudness_range = config.loudness_range;
    data.maximum_true_peak_dbtp = None;
    data.maximum_momentary_lufs = None;
    data.maximum_shortterm_lufs = None;
    data.peak = 0.0;
    data.correlation_lr = None;
    data.correlation_samples_seen = 0;
    data.true_peak_is_compliant = false;
    data.integrated_window_seconds = INTEGRATED_HISTORY_SECONDS;

    if let Some(peaks) = Arc::get_mut(&mut data.channel_peaks) {
        peaks.fill(0.0);
    }
    if let Some(true_peaks) = Arc::get_mut(&mut data.true_peaks_dbtp) {
        true_peaks.fill(f64::NEG_INFINITY);
    }
    if let Some(matrix) = Arc::get_mut(&mut data.correlation_matrix) {
        if config.spatial && matrix.len() == config.channels.saturating_mul(config.channels) {
            matrix.fill(0.0);
        } else if !config.spatial {
            matrix.clear();
        }
    }
}

#[cfg(test)]
mod integrated_control_command_tests {
    use super::*;

    #[test]
    fn parser_accepts_the_full_canonical_u64_command_and_rejects_oversize() {
        let maximum = u64::MAX;
        let command = format!("{maximum}:{maximum}:continue");
        assert_eq!(command.len(), INTEGRATED_CONTROL_COMMAND_MAX_BYTES);
        assert_eq!(
            parse_integrated_control_command(&command),
            Ok(IntegratedControlCommand {
                instance_id: maximum,
                request_id: maximum,
                operation: IntegratedControlOperation::Continue,
            })
        );
        assert!(parse_integrated_control_command(&format!("{command}x")).is_err());
    }

    #[test]
    fn runtime_instance_ids_are_nonzero_and_fail_at_checked_exhaustion() {
        let counter = AtomicU64::new(1);
        assert_eq!(allocate_loudness_control_instance_id(&counter), Ok(1));
        assert_eq!(allocate_loudness_control_instance_id(&counter), Ok(2));

        let exhausted = AtomicU64::new(u64::MAX);
        assert!(allocate_loudness_control_instance_id(&exhausted).is_err());
        assert_eq!(exhausted.load(Ordering::Relaxed), u64::MAX);
    }
}

#[cfg(test)]
mod true_peak_tests {
    use super::*;

    mod reference {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/common/true_peak_reference.rs"
        ));
    }

    fn oracle_peak(signal: &[f32], phases: &[usize]) -> f64 {
        reference::frame_peaks(signal, phases.len())
            .into_iter()
            .fold(0.0, f64::max)
    }

    fn direct_windowed_sinc_reference(input: &[f32], factor: usize) -> Vec<f64> {
        const TAPS: usize = 64;
        let phase_coefficients: Vec<Vec<f64>> = (0..factor)
            .map(|phase| {
                let fraction = phase as f64 / factor as f64;
                let mut coefficients = Vec::with_capacity(TAPS);
                let mut normalization = 0.0;
                for tap in 0..TAPS {
                    let position = tap as f64 / (TAPS - 1) as f64;
                    let window = 0.42 - 0.5 * (std::f64::consts::TAU * position).cos()
                        + 0.08 * (2.0 * std::f64::consts::TAU * position).cos();
                    let distance = tap as f64 - (TAPS - 1) as f64 / 2.0 - fraction;
                    let sinc = if distance == 0.0 {
                        1.0
                    } else {
                        (std::f64::consts::PI * distance).sin() / (std::f64::consts::PI * distance)
                    };
                    let coefficient = window * sinc;
                    coefficients.push(coefficient);
                    normalization += coefficient;
                }
                for coefficient in &mut coefficients {
                    *coefficient /= normalization;
                }
                coefficients
            })
            .collect();

        let emitted_samples = input.len() + TAPS - 1;
        (0..emitted_samples)
            .map(|output_index| {
                phase_coefficients
                    .iter()
                    .map(|phase| {
                        phase
                            .iter()
                            .enumerate()
                            .filter_map(|(tap, &coefficient)| {
                                let source_index =
                                    output_index as isize + tap as isize - (TAPS - 1) as isize;
                                usize::try_from(source_index)
                                    .ok()
                                    .and_then(|index| input.get(index))
                                    .map(|&sample| coefficient * f64::from(sample))
                            })
                            .sum::<f64>()
                            .abs()
                    })
                    .fold(0.0, f64::max)
            })
            .collect()
    }

    fn assert_custom_meter_matches_direct_reference(sample_rate: u32, input: &[f32]) {
        let factor = true_peak_oversampling_factor(sample_rate).unwrap();
        assert!(matches!(factor, 8 | 16 | 32));
        let expected = direct_windowed_sinc_reference(input, factor);
        let mut meter = Bs1770TruePeakMeter::new(1, sample_rate);
        let mut offset = 0;
        for requested in [1, 7, 13, 2, 31, 19, usize::MAX] {
            if offset == input.len() {
                break;
            }
            let end = offset.saturating_add(requested).min(input.len());
            meter.add_frames(&input[offset..end], 1);
            let measured = meter.take_interval_peak(0).unwrap();
            let expected_interval = expected[offset..end].iter().copied().fold(0.0, f64::max);
            assert!(
                (measured - expected_interval).abs() < 2.0e-14,
                "rate={sample_rate}, input interval={offset}..{end}, measured={measured}, expected={expected_interval}"
            );
            offset = end;
        }
        assert_eq!(offset, input.len());

        meter.finish();
        let measured_tail = meter.take_interval_peak(0).unwrap();
        let expected_tail = expected[input.len()..].iter().copied().fold(0.0, f64::max);
        assert!(
            (measured_tail - expected_tail).abs() < 2.0e-14,
            "rate={sample_rate}, 63-sample finish tail: measured={measured_tail}, expected={expected_tail}"
        );
        meter.finish();
        assert_eq!(meter.take_interval_peak(0), Some(0.0));
    }

    struct PreAud123PublishedMeter {
        history: Vec<[f64; TRUE_PEAK_TAPS]>,
        interval_peaks: Vec<f64>,
        phase_indices: &'static [usize],
    }

    impl PreAud123PublishedMeter {
        fn new(channels: usize, sample_rate: u32) -> Self {
            let phase_indices = if sample_rate == 96_000 {
                &[0, 2][..]
            } else {
                &[0, 1, 2, 3][..]
            };
            Self {
                history: vec![[0.0; TRUE_PEAK_TAPS]; channels],
                interval_peaks: vec![0.0; channels],
                phase_indices,
            }
        }

        fn add_frames(&mut self, samples: &[f32], channels: usize) {
            for frame in samples.chunks_exact(channels) {
                for (channel, &sample) in frame.iter().enumerate() {
                    let history = &mut self.history[channel];
                    history.copy_within(1.., 0);
                    history[TRUE_PEAK_TAPS - 1] = f64::from(sample);
                    let peak = self
                        .phase_indices
                        .iter()
                        .map(|&phase| {
                            TRUE_PEAK_PHASES[phase]
                                .iter()
                                .zip(history.iter())
                                .map(|(coefficient, value)| coefficient * value)
                                .sum::<f64>()
                                .abs()
                        })
                        .fold(0.0, f64::max);
                    self.interval_peaks[channel] = self.interval_peaks[channel].max(peak);
                }
            }
        }

        fn finish(&mut self) {
            for channel in 0..self.history.len() {
                for _ in 1..TRUE_PEAK_TAPS {
                    let history = &mut self.history[channel];
                    history.copy_within(1.., 0);
                    history[TRUE_PEAK_TAPS - 1] = 0.0;
                    let peak = self
                        .phase_indices
                        .iter()
                        .map(|&phase| {
                            TRUE_PEAK_PHASES[phase]
                                .iter()
                                .zip(history.iter())
                                .map(|(coefficient, value)| coefficient * value)
                                .sum::<f64>()
                                .abs()
                        })
                        .fold(0.0, f64::max);
                    self.interval_peaks[channel] = self.interval_peaks[channel].max(peak);
                }
                self.history[channel].fill(0.0);
            }
        }

        fn take_interval_peak(&mut self, channel: usize) -> f64 {
            std::mem::take(&mut self.interval_peaks[channel])
        }

        fn reset(&mut self) {
            self.history.fill([0.0; TRUE_PEAK_TAPS]);
            self.interval_peaks.fill(0.0);
        }
    }

    fn fixture() -> Vec<f32> {
        let mut signal: Vec<f32> = (0..513)
            .map(|index| (std::f64::consts::TAU * 0.459 * index as f64).sin() as f32 * 0.91)
            .collect();
        // An impulse immediately on a likely callback boundary exercises FIR
        // history continuity rather than merely steady-state tone behavior.
        signal[64] = -0.97;
        signal
    }

    #[test]
    fn annex_2_meter_matches_direct_convolution_at_supported_rates() {
        let signal = fixture();
        for (sample_rate, phases) in [
            (48_000, &[0, 1, 2, 3][..]),
            (88_200, &[0, 1, 2, 3][..]),
            (96_000, &[0, 2][..]),
        ] {
            let mut meter = Bs1770TruePeakMeter::new(1, sample_rate);
            meter.add_frames(&signal, 1);
            let measured = meter.take_interval_peak(0).unwrap();
            let expected = oracle_peak(&signal, phases);
            assert!((measured - expected).abs() < 1.0e-14, "{sample_rate} Hz");
        }
    }

    #[test]
    fn annex_2_meter_preserves_history_across_callback_boundaries() {
        let signal = fixture();
        for sample_rate in [8_000, 44_100, 48_000, 88_200, 96_000] {
            let mut whole = Bs1770TruePeakMeter::new(1, sample_rate);
            whole.add_frames(&signal, 1);
            let expected = whole.take_interval_peak(0).unwrap();

            let mut split = Bs1770TruePeakMeter::new(1, sample_rate);
            let mut measured = 0.0_f64;
            let mut offset = 0;
            for length in [1, 7, 56, 1, 113, 257, usize::MAX] {
                if offset == signal.len() {
                    break;
                }
                let end = offset.saturating_add(length).min(signal.len());
                split.add_frames(&signal[offset..end], 1);
                measured = measured.max(split.take_interval_peak(0).unwrap());
                offset = end;
            }
            assert_eq!(offset, signal.len());
            assert!((measured - expected).abs() < 1.0e-14, "{sample_rate} Hz");
        }
    }

    #[test]
    fn prepared_kernels_drain_the_full_support_and_reset_all_channels() {
        for sample_rate in [8_000, 44_100, 48_000, 88_200, 96_000, 384_000] {
            for channels in [1, 2, 6, 24] {
                let mut meter = Bs1770TruePeakMeter::new(channels, sample_rate);
                let mut signal = vec![0.0_f32; 64 * channels];
                for channel in 0..channels {
                    signal[63 * channels + channel] = 0.5;
                }

                meter.add_frames(&signal, channels);
                meter.finish();
                for channel in 0..channels {
                    let peak = meter.take_interval_peak(channel).unwrap();
                    assert!(
                        (0.45..=0.51).contains(&peak),
                        "rate={sample_rate}, channels={channels}, channel={channel}, peak={peak}"
                    );
                }

                meter.finish();
                for channel in 0..channels {
                    assert_eq!(meter.take_interval_peak(channel), Some(0.0));
                }

                meter.reset();
                meter.add_frames(&vec![0.0; channels], channels);
                meter.finish();
                for channel in 0..channels {
                    assert_eq!(meter.take_interval_peak(channel), Some(0.0));
                }
            }
        }
    }

    #[test]
    fn high_ratio_kernels_match_direct_convolution_for_all_impulse_offsets_and_queries() {
        for sample_rate in [8_000, 12_000, 24_000] {
            for offset in 0..64 {
                let mut impulse = vec![0.0_f32; 64];
                impulse[offset] = -0.73;
                assert_custom_meter_matches_direct_reference(sample_rate, &impulse);
            }

            let dense: Vec<f32> = (0..257)
                .map(|index| {
                    let time = index as f64;
                    (0.31 * (std::f64::consts::TAU * 0.173 * time).sin()
                        + 0.27 * (std::f64::consts::TAU * 0.311 * time + 0.2).cos()
                        - 0.19 * (std::f64::consts::TAU * 0.427 * time + 0.7).sin())
                        as f32
                })
                .collect();
            assert_custom_meter_matches_direct_reference(sample_rate, &dense);
        }
    }

    #[test]
    #[ignore = "manual matched pre-AUD123 kernel-only CPU control; run with --ignored --nocapture"]
    fn pre_aud123_published_kernel_cpu_control() {
        use std::time::{Duration, Instant};

        const CALLBACKS: usize = 128;
        const TRIALS: usize = 7;
        let samples_for = |channels: usize| {
            (0..128 * channels)
                .map(|index| {
                    let time = (index / channels) as f64;
                    (0.31 * (std::f64::consts::TAU * 0.173 * time).sin()
                        + 0.27 * (std::f64::consts::TAU * 0.311 * time + 0.2).cos())
                        as f32
                })
                .collect::<Vec<_>>()
        };

        for sample_rate in [48_000, 96_000] {
            for channels in [1, 24] {
                let input = samples_for(channels);
                let mut legacy = PreAud123PublishedMeter::new(channels, sample_rate);
                let mut candidate = Bs1770TruePeakMeter::new(channels, sample_rate);

                legacy.add_frames(&input, channels);
                candidate.add_frames(&input, channels);
                for channel in 0..channels {
                    assert_eq!(
                        legacy.take_interval_peak(channel),
                        candidate.take_interval_peak(channel).unwrap(),
                        "process output rate={sample_rate}, channel={channel}"
                    );
                }

                legacy.add_frames(&input, channels);
                candidate.add_frames(&input, channels);
                for channel in 0..channels {
                    assert_eq!(
                        legacy.take_interval_peak(channel),
                        candidate.take_interval_peak(channel).unwrap(),
                        "second process interval rate={sample_rate}, channel={channel}"
                    );
                }
                legacy.finish();
                candidate.finish();
                for channel in 0..channels {
                    assert_eq!(
                        legacy.take_interval_peak(channel),
                        candidate.take_interval_peak(channel).unwrap(),
                        "finish tail rate={sample_rate}, channel={channel}"
                    );
                }

                let mut legacy_process = Vec::with_capacity(TRIALS);
                let mut candidate_process = Vec::with_capacity(TRIALS);
                let mut legacy_drain = Vec::with_capacity(TRIALS);
                let mut candidate_drain = Vec::with_capacity(TRIALS);
                for trial in 0..TRIALS {
                    let measure_process =
                        |legacy: &mut PreAud123PublishedMeter,
                         candidate: &mut Bs1770TruePeakMeter,
                         use_candidate: bool| {
                            let start = Instant::now();
                            for _ in 0..CALLBACKS {
                                if use_candidate {
                                    candidate.add_frames(&input, channels);
                                    for channel in 0..channels {
                                        std::hint::black_box(
                                            candidate.take_interval_peak(channel).unwrap(),
                                        );
                                    }
                                } else {
                                    legacy.add_frames(&input, channels);
                                    for channel in 0..channels {
                                        std::hint::black_box(legacy.take_interval_peak(channel));
                                    }
                                }
                            }
                            start.elapsed()
                        };

                    let (legacy_time, candidate_time) = if trial % 2 == 0 {
                        (
                            measure_process(&mut legacy, &mut candidate, false),
                            measure_process(&mut legacy, &mut candidate, true),
                        )
                    } else {
                        let candidate_time = measure_process(&mut legacy, &mut candidate, true);
                        let legacy_time = measure_process(&mut legacy, &mut candidate, false);
                        (legacy_time, candidate_time)
                    };
                    legacy_process.push(legacy_time);
                    candidate_process.push(candidate_time);

                    let measure_legacy_drain = |meter: &mut PreAud123PublishedMeter| {
                        meter.reset();
                        meter.add_frames(&input, channels);
                        let start = Instant::now();
                        meter.finish();
                        let elapsed = start.elapsed();
                        for channel in 0..channels {
                            std::hint::black_box(meter.take_interval_peak(channel));
                        }
                        elapsed
                    };
                    let measure_candidate_drain = |meter: &mut Bs1770TruePeakMeter| {
                        meter.reset();
                        meter.add_frames(&input, channels);
                        let start = Instant::now();
                        meter.finish();
                        let elapsed = start.elapsed();
                        for channel in 0..channels {
                            std::hint::black_box(meter.take_interval_peak(channel).unwrap());
                        }
                        elapsed
                    };

                    let mut legacy_time = Duration::ZERO;
                    let mut candidate_time = Duration::ZERO;
                    for _ in 0..CALLBACKS {
                        if trial % 2 == 0 {
                            legacy_time += measure_legacy_drain(&mut legacy);
                            candidate_time += measure_candidate_drain(&mut candidate);
                        } else {
                            candidate_time += measure_candidate_drain(&mut candidate);
                            legacy_time += measure_legacy_drain(&mut legacy);
                        }
                    }
                    legacy_drain.push(legacy_time);
                    candidate_drain.push(candidate_time);
                }

                let timing_stats_us = |times: &mut [Duration]| {
                    times.sort_unstable();
                    let to_us =
                        |duration: Duration| duration.as_secs_f64() * 1.0e6 / CALLBACKS as f64;
                    (
                        to_us(times[0]),
                        to_us(times[times.len() / 2]),
                        to_us(times[times.len() - 1]),
                    )
                };
                let (legacy_process_min, legacy_process_us, legacy_process_max) =
                    timing_stats_us(&mut legacy_process);
                let (candidate_process_min, candidate_process_us, candidate_process_max) =
                    timing_stats_us(&mut candidate_process);
                let (legacy_drain_min, legacy_drain_us, legacy_drain_max) =
                    timing_stats_us(&mut legacy_drain);
                let (candidate_drain_min, candidate_drain_us, candidate_drain_max) =
                    timing_stats_us(&mut candidate_drain);
                eprintln!(
                    "pre-AUD123 kernel-only rate={sample_rate} channels={channels}: process_us_per_128f legacy={legacy_process_us:.3} [{legacy_process_min:.3},{legacy_process_max:.3}], candidate={candidate_process_us:.3} [{candidate_process_min:.3},{candidate_process_max:.3}], ratio={:.3}; finish_us legacy={legacy_drain_us:.3} [{legacy_drain_min:.3},{legacy_drain_max:.3}], candidate={candidate_drain_us:.3} [{candidate_drain_min:.3},{candidate_drain_max:.3}], ratio={:.3}",
                    candidate_process_us / legacy_process_us,
                    candidate_drain_us / legacy_drain_us
                );
            }
        }
    }

    #[test]
    fn unsupported_rate_publishes_no_true_peak() {
        let mut meter = Bs1770TruePeakMeter::new(1, 7_999);
        meter.add_frames(&fixture(), 1);
        assert!(!meter.compliant);
        assert_eq!(meter.take_interval_peak(0), None);
    }

    #[test]
    fn nonfinite_samples_do_not_turn_true_peak_state_into_nan() {
        for sample_rate in [8_000, 12_000, 44_100, 48_000, 96_000] {
            let mut meter = Bs1770TruePeakMeter::new(1, sample_rate);
            meter.add_frames(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY], 1);
            meter.finish();
            let peak = meter.take_interval_peak(0).unwrap();
            assert!(!peak.is_nan(), "rate={sample_rate} Hz");
            assert!(peak >= 0.0, "rate={sample_rate} Hz, peak={peak}");

            meter.reset();
            meter.add_frames(&[f32::NAN; 65], 1);
            meter.finish();
            assert_eq!(meter.take_interval_peak(0), Some(0.0));
        }
    }

    #[test]
    fn oversampling_factor_reaches_192_khz_at_every_rate_boundary() {
        for (sample_rate, factor) in [
            (5_999, None),
            (7_999, None),
            (8_000, Some(32)),
            (11_999, Some(32)),
            (12_000, Some(16)),
            (23_999, Some(16)),
            (24_000, Some(8)),
            (47_999, Some(8)),
            (48_000, Some(4)),
            (95_999, Some(4)),
            (96_000, Some(2)),
            (2_822_400, Some(2)),
            (2_822_401, None),
        ] {
            assert_eq!(
                true_peak_oversampling_factor(sample_rate),
                factor,
                "sample_rate={sample_rate}"
            );
        }
    }

    #[test]
    fn prepared_blackman_sinc_phases_have_unit_dc_gain() {
        for factor in [8, 16, 32] {
            for phase in 0..factor {
                let coefficients = blackman_sinc_phase(factor, phase);
                let dc_gain: f64 = coefficients.iter().sum();
                assert!(
                    (dc_gain - 1.0).abs() < 2.0e-15,
                    "factor={factor}, phase={phase}, dc_gain={dc_gain}"
                );
            }
        }
    }

    #[test]
    fn whole_program_gate_matches_independent_two_level_reference() {
        let quiet = loudness_to_energy(-60.0);
        let loud = loudness_to_energy(-20.0);
        let measured = gated_loudness(&[quiet, loud, loud]);
        assert!((measured - -20.0).abs() < 1.0e-12);
    }

    #[test]
    fn whole_program_capacity_failure_is_explicit_and_generation_is_stable() {
        let mut monitor = LoudnessMonitor::new_with_integrated_mode(
            1,
            48_000,
            IntegratedLoudnessMode::WholeProgram,
        )
        .unwrap();
        monitor.whole_program_integrated = Some(WholeProgramIntegrated::new(2));
        monitor.add_frames(&vec![0.1; 48_000 * 6 / 10]).unwrap();

        let mut data = LoudnessData::new(1);
        monitor.update_loudness_data(&mut data);
        assert_eq!(
            data.query_error,
            Some(LoudnessQueryError::IntegratedProgramCapacityExceeded)
        );
        assert_eq!(data.query_error_generation, 1);
        assert!(!data.integrated_valid);
        assert!(data.integrated_lufs.is_infinite() && data.integrated_lufs.is_sign_negative());

        monitor.update_loudness_data(&mut data);
        assert_eq!(data.query_error_generation, 1);
        monitor.reset().unwrap();
        monitor.update_loudness_data(&mut data);
        assert!(data.query_error.is_none());
        assert_eq!(data.query_error_generation, 0);
    }
}
