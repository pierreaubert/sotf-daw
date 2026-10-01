use super::crossover_kind::CrossoverKind;
use super::crossover_mode::CrossoverMode;
use super::iir_family::IirSplit;
use super::parse::parse_channel_freq_id;
use super::parse::parse_channel_mode_id;
use super::per_channel_op_mode::PerChannelOpMode;
use super::precise_lr4::PreciseLr4 as Lr4Crossover;
use super::types::{CrossoverPluginParams, CrossoverTopology};
use sotf_host::fir_crossover::{DEFAULT_FIR_CROSSOVER_TAPS, FirCrossover, MultibandFirCrossover};
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::simd::{enable_ftz_daz, flush_denormals_inplace};
use sotf_host::smoothing::LogSmoother;

// Bound one drain callback independently of FIR length or destination capacity.
const FIR_DRAIN_FRAMES: usize = 256;
const NEW_IIR_MIN_CUTOFF_HZ: f32 = 20.0;
const NEW_IIR_MAX_CUTOFF_HZ: f32 = 20_000.0;
// Keep new IIR poles inside the bilinear-transform region used by legacy LR24.
const NEW_IIR_MAX_SAMPLE_RATE_RATIO: f32 = 0.495;
const COEFFICIENT_UPDATE_SAMPLES: usize = 16;

fn validate_new_iir_cutoff(frequency: f32, sample_rate: u32) -> Result<(), String> {
    let sample_rate_limit = sample_rate as f32 * NEW_IIR_MAX_SAMPLE_RATE_RATIO;
    if !frequency.is_finite()
        || frequency < NEW_IIR_MIN_CUTOFF_HZ
        || frequency > NEW_IIR_MAX_CUTOFF_HZ
        || frequency >= sample_rate_limit
    {
        return Err(format!(
            "new IIR crossover frequency must be finite, in [{NEW_IIR_MIN_CUTOFF_HZ}, {NEW_IIR_MAX_CUTOFF_HZ}] Hz, and below {sample_rate_limit} Hz"
        ));
    }
    Ok(())
}

fn validate_new_iir_frequencies(frequencies: &[f32], sample_rate: u32) -> Result<(), String> {
    for &frequency in frequencies {
        validate_new_iir_cutoff(frequency, sample_rate)?;
    }
    if frequencies.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("new IIR crossover frequencies must be strictly increasing".into());
    }
    Ok(())
}

/// Estimated persistent FIR DSP payload owned by a compiled crossover.
///
/// The report counts coefficient arrays, convolution histories/write
/// positions, alignment delay samples, and steady-processing scratch. It does
/// not include allocator bookkeeping or the `Vec` headers stored inline in the
/// plugin, so hosts can use it as a stable pre-admission lower bound.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FirMemoryReport {
    pub coefficient_bytes: usize,
    pub history_bytes: usize,
    pub alignment_bytes: usize,
    pub scratch_bytes: usize,
    pub total_bytes: usize,
}

/// Delays the early branches of a cascaded FIR crossover so every emitted
/// band has the same cumulative group delay as the final branch.
struct FirBandAlignment {
    delay_lines: Vec<Vec<f32>>,
    write_positions: Vec<usize>,
    delay_frames: Vec<usize>,
    channels: usize,
}

impl FirBandAlignment {
    fn new(num_bands: usize, channels: usize, split_latency: usize) -> Self {
        let num_splits = num_bands.saturating_sub(1);
        let delay_frames: Vec<usize> = (0..num_bands)
            .map(|band| {
                let intrinsic_splits = (band + 1).min(num_splits);
                (num_splits - intrinsic_splits) * split_latency
            })
            .collect();
        let delay_lines = delay_frames
            .iter()
            .map(|&frames| vec![0.0; frames * channels])
            .collect();
        Self {
            delay_lines,
            write_positions: vec![0; num_bands],
            delay_frames,
            channels,
        }
    }

    fn process_frame(&mut self, bands: &mut [f32]) {
        for band in 0..self.delay_frames.len() {
            let frames = self.delay_frames[band];
            if frames == 0 {
                continue;
            }
            let band_offset = band * self.channels;
            let delay_offset = self.write_positions[band] * self.channels;
            for channel in 0..self.channels {
                std::mem::swap(
                    &mut self.delay_lines[band][delay_offset + channel],
                    &mut bands[band_offset + channel],
                );
            }
            self.write_positions[band] = (self.write_positions[band] + 1) % frames;
        }
    }

    fn reset(&mut self) {
        for delay in &mut self.delay_lines {
            delay.fill(0.0);
        }
        self.write_positions.fill(0);
    }
}

pub struct CrossoverPlugin {
    pub(super) num_channels: usize,
    pub(super) sample_rate: u32,
    /// Set once the host has compiled/initialized this instance. Structural
    /// FIR parameters are configuration-only after this point.
    initialized: bool,
    fir_has_input: bool,
    fir_drain_remaining: Option<usize>,
    pub(super) mode: CrossoverMode,
    pub(super) kind: CrossoverKind,
    pub(super) fir_taps: usize,
    pub(super) cached_parameters: Vec<Parameter>,

    /// Single crossover for 2-way operation
    pub(super) crossover_2way: Lr4Crossover,
    pub(super) fir_crossover_2way: Option<FirCrossover<f32>>,
    pub(super) freq_smoother: LogSmoother,

    /// Multi-band crossover for 3-way and 4-way operation.
    /// None when in 2-way mode.
    /// Phase-coherent multiway bank. Every band traverses every split as
    /// either LP or HP, so recombination is the product of LR all-pass sums.
    pub(super) multiband: Option<Vec<Vec<Lr4Crossover>>>,
    pub(super) fir_multiband: Option<MultibandFirCrossover<f32>>,
    fir_band_alignment: Option<FirBandAlignment>,
    pub(super) extra_freq_smoothers: Vec<LogSmoother>,
    /// Position in the persistent 16-sample coefficient update cadence.
    /// This must not restart at callback boundaries.
    smoother_subblock_phase: usize,

    /// Sorted crossover frequencies for multi-way mode (including primary).
    pub(super) all_frequencies: Vec<f32>,

    /// Pre-allocated scratch buffers
    pub(super) low_buf: Vec<f32>,
    pub(super) high_buf: Vec<f32>,
    /// Flat buffer for multi-way band outputs: [band0_ch0..band0_chN, band1_ch0..band1_chN, ...]
    pub(super) band_flat: Vec<f32>,

    /// When non-empty, the plugin runs in per-channel mode: each channel is
    /// processed by its own single-channel LR24 crossover and the per-channel
    /// `op_modes` array decides what each channel outputs.
    pub(super) channel_frequencies_hz: Vec<f32>,
    pub(super) op_modes: Vec<PerChannelOpMode>,
    pub(super) per_channel_lr4: Vec<Lr4Crossover>,
    /// New-family per-channel split state, one independently owned split per input channel.
    pub(super) per_channel_iir: Vec<IirSplit>,
    /// Per-channel scratch buffers for the 1-sample-wide low/high outputs.
    pub(super) per_channel_low: Vec<f32>,
    pub(super) per_channel_high: Vec<f32>,

    /// New-family two-way split, absent for the preserved LR24/FIR paths.
    family_two_way: Option<IirSplit>,
    /// Phase-coherent LR12/LR48 multiway banks, indexed by output band then split.
    family_multiband_lr: Option<Vec<Vec<IirSplit>>>,
    /// Conventional cascaded Butterworth/Bessel splits, indexed by split.
    family_multiband_serial: Option<Vec<IirSplit>>,
}

impl CrossoverPlugin {
    pub fn new(
        num_channels: usize,
        crossover_type: &str,
        frequency: f64,
        output: &str,
    ) -> Result<Self, String> {
        Self::new_multiway(num_channels, crossover_type, frequency, output, &[])
    }

    pub fn new_multiway(
        num_channels: usize,
        crossover_type: &str,
        frequency: f64,
        output: &str,
        extra_frequencies: &[f64],
    ) -> Result<Self, String> {
        Self::new_multiway_with_fir_taps(
            num_channels,
            crossover_type,
            frequency,
            output,
            extra_frequencies,
            DEFAULT_FIR_CROSSOVER_TAPS,
        )
    }

    pub(super) fn new_multiway_with_fir_taps(
        num_channels: usize,
        crossover_type: &str,
        frequency: f64,
        output: &str,
        extra_frequencies: &[f64],
        fir_taps: usize,
    ) -> Result<Self, String> {
        if num_channels == 0 {
            return Err("crossover requires at least one channel".into());
        }
        if extra_frequencies.len() > 2 {
            return Err("crossover supports at most four bands (three crossover points)".into());
        }
        if !(31..=16385).contains(&fir_taps) {
            return Err(format!("fir_taps must be in [31, 16385], got {fir_taps}"));
        }
        let kind = CrossoverKind::parse(crossover_type)?;
        let mode = CrossoverMode::from_str(output)?;
        let sr = 48000;
        let fir_taps = if fir_taps.is_multiple_of(2) {
            fir_taps
                .checked_add(1)
                .ok_or_else(|| "fir_taps overflow".to_string())?
        } else {
            fir_taps
        };

        let mut all_freqs: Vec<f32> = vec![frequency as f32];
        for &f in extra_frequencies {
            all_freqs.push(f as f32);
        }
        if kind.is_new_iir() {
            validate_new_iir_frequencies(&all_freqs, sr)?;
        } else {
            let nyquist_limit = sr as f32 * 0.5 * 0.99;
            if all_freqs
                .iter()
                .any(|f| !f.is_finite() || *f <= 0.0 || *f >= nyquist_limit)
            {
                return Err(format!(
                    "crossover frequencies must be finite and in (0, {nyquist_limit}) Hz"
                ));
            }
            all_freqs.sort_by(|a, b| a.total_cmp(b));
            if all_freqs.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err("crossover frequencies must be unique".into());
            }
        }

        let num_bands = all_freqs.len() + 1;

        let (multiband, extra_smoothers) = if all_freqs.len() > 1 {
            // FIR and LR banks are mutually exclusive runtime topologies. Do
            // not retain an unused phase-coherent IIR bank in FIR instances.
            let mb = (kind == CrossoverKind::Lr24).then(|| {
                (0..num_bands)
                    .map(|_| {
                        all_freqs
                            .iter()
                            .map(|&frequency| Lr4Crossover::new(frequency, sr as f32, num_channels))
                            .collect()
                    })
                    .collect()
            });
            let smoothers: Vec<LogSmoother> = all_freqs
                .iter()
                .skip(1) // first freq uses the primary smoother
                .map(|&f| LogSmoother::new(f, 20.0, sr))
                .collect();
            (mb, smoothers)
        } else {
            (None, Vec::new())
        };
        let fir_crossover_2way = (kind == CrossoverKind::LinearPhase && all_freqs.len() == 1)
            .then(|| FirCrossover::new(frequency as f32, sr as f32, num_channels, fir_taps));
        let fir_multiband = (kind == CrossoverKind::LinearPhase && all_freqs.len() > 1)
            .then(|| MultibandFirCrossover::new(&all_freqs, sr as f32, num_channels, fir_taps));
        let fir_band_alignment = (kind == CrossoverKind::LinearPhase && all_freqs.len() > 1)
            .then(|| FirBandAlignment::new(num_bands, num_channels, (fir_taps - 1) / 2));
        let family_two_way = (kind.is_new_iir() && all_freqs.len() == 1)
            .then(|| IirSplit::new(kind, all_freqs[0], sr, num_channels))
            .transpose()?;
        let family_multiband_lr = (kind.is_new_iir() && kind.is_lr() && all_freqs.len() > 1)
            .then(|| {
                (0..num_bands)
                    .map(|_| {
                        all_freqs
                            .iter()
                            .map(|&frequency| IirSplit::new(kind, frequency, sr, num_channels))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let family_multiband_serial = (kind.is_new_iir() && !kind.is_lr() && all_freqs.len() > 1)
            .then(|| {
                all_freqs
                    .iter()
                    .map(|&frequency| IirSplit::new(kind, frequency, sr, num_channels))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let mut p = Self {
            num_channels,
            sample_rate: sr,
            initialized: false,
            fir_has_input: false,
            fir_drain_remaining: None,
            mode,
            kind,
            fir_taps,
            crossover_2way: Lr4Crossover::new(frequency as f32, sr as f32, num_channels),
            fir_crossover_2way,
            freq_smoother: LogSmoother::new(frequency as f32, 20.0, sr),
            multiband,
            fir_multiband,
            fir_band_alignment,
            extra_freq_smoothers: extra_smoothers,
            smoother_subblock_phase: 0,
            all_frequencies: all_freqs,
            cached_parameters: Vec::new(),
            low_buf: vec![0.0; num_channels],
            high_buf: vec![0.0; num_channels],
            band_flat: vec![0.0; num_bands * num_channels],
            channel_frequencies_hz: Vec::new(),
            op_modes: Vec::new(),
            per_channel_lr4: Vec::new(),
            per_channel_iir: Vec::new(),
            per_channel_low: Vec::new(),
            per_channel_high: Vec::new(),
            family_two_way,
            family_multiband_lr,
            family_multiband_serial,
        };
        p.rebuild_cached_parameters();
        Ok(p)
    }

    /// Build a crossover plugin in per-channel mode: each channel gets its
    /// own IIR crossover with its own cutoff frequency and operation mode
    /// (lowpass / highpass / mute). The plugin remains 1-input / 1-output
    /// per channel; output channel count equals input channel count.
    ///
    /// Used by the RoomEQ factored graph to encode all per-channel HP or LP
    /// route filters in a single multichannel node.
    pub fn new_per_channel(
        crossover_type: &str,
        channel_frequencies_hz: Vec<f32>,
        channel_modes: Vec<PerChannelOpMode>,
    ) -> Result<Self, String> {
        if channel_frequencies_hz.is_empty() {
            return Err("channel_frequencies_hz must not be empty".into());
        }
        if channel_frequencies_hz.len() != channel_modes.len() {
            return Err(format!(
                "channel_frequencies_hz.len()={} but channel_modes.len()={}",
                channel_frequencies_hz.len(),
                channel_modes.len()
            ));
        }
        let kind = CrossoverKind::parse(crossover_type)?;
        if kind == CrossoverKind::LinearPhase {
            return Err(format!(
                "per-channel crossover does not support LinearPhase, got {crossover_type}"
            ));
        }
        if kind.is_new_iir() {
            for &frequency in &channel_frequencies_hz {
                validate_new_iir_cutoff(frequency, 48_000)?;
            }
        } else {
            let nyquist_limit = 48_000.0 * 0.5 * 0.99;
            if channel_frequencies_hz
                .iter()
                .any(|f| !f.is_finite() || *f <= 0.0 || *f >= nyquist_limit)
            {
                return Err(format!(
                    "channel crossover frequencies must be finite and in (0, {nyquist_limit}) Hz"
                ));
            }
        }
        let num_channels = channel_frequencies_hz.len();
        let sr = 48000u32;
        let per_channel_lr4: Vec<Lr4Crossover> = if kind == CrossoverKind::Lr24 {
            channel_frequencies_hz
                .iter()
                .map(|&f| Lr4Crossover::new(f, sr as f32, 1))
                .collect()
        } else {
            Vec::new()
        };
        let per_channel_iir: Vec<IirSplit> = if kind.is_new_iir() {
            channel_frequencies_hz
                .iter()
                .map(|&frequency| IirSplit::new(kind, frequency, sr, 1))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        // The shared/global crossover and smoothers remain populated but
        // unused in per-channel mode; sized minimally to avoid surprises.
        let primary_freq = channel_frequencies_hz[0];
        let mut p = Self {
            num_channels,
            sample_rate: sr,
            initialized: false,
            fir_has_input: false,
            fir_drain_remaining: None,
            mode: CrossoverMode::Lowpass,
            kind,
            fir_taps: DEFAULT_FIR_CROSSOVER_TAPS,
            crossover_2way: Lr4Crossover::new(primary_freq, sr as f32, num_channels),
            fir_crossover_2way: None,
            freq_smoother: LogSmoother::new(primary_freq, 20.0, sr),
            multiband: None,
            fir_multiband: None,
            fir_band_alignment: None,
            extra_freq_smoothers: Vec::new(),
            smoother_subblock_phase: 0,
            all_frequencies: vec![primary_freq],
            cached_parameters: Vec::new(),
            low_buf: vec![0.0; num_channels],
            high_buf: vec![0.0; num_channels],
            band_flat: vec![0.0; num_channels],
            channel_frequencies_hz,
            op_modes: channel_modes,
            per_channel_lr4,
            per_channel_iir,
            per_channel_low: vec![0.0; 1],
            per_channel_high: vec![0.0; 1],
            family_two_way: None,
            family_multiband_lr: None,
            family_multiband_serial: None,
        };
        p.rebuild_cached_parameters();
        Ok(p)
    }

    /// True when the plugin is configured with independent per-channel cutoffs.
    pub fn is_per_channel(&self) -> bool {
        !self.channel_frequencies_hz.is_empty()
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let mut params = vec![
            Parameter::new_string("type", "Type", self.kind.as_str().to_string())
                .with_update_mode(UpdateMode::Structural),
        ];
        if !self.is_per_channel() {
            params.extend([
                Parameter::new_float(
                    "frequency",
                    "Frequency",
                    self.freq_smoother.target(),
                    20.0,
                    20000.0,
                ),
                Parameter::new_string("mode", "Mode", self.mode.as_str().to_string())
                    .with_update_mode(UpdateMode::Structural),
            ]);
        }

        // Add extra frequency parameters for multi-way
        if !self.is_per_channel() {
            for (i, smoother) in self.extra_freq_smoothers.iter().enumerate() {
                params.push(Parameter::new_float(
                    &format!("frequency_{}", i + 2),
                    &format!("Frequency {}", i + 2),
                    smoother.target(),
                    20.0,
                    20000.0,
                ));
            }
            if self.kind == CrossoverKind::LinearPhase {
                params.push(Parameter::new_int(
                    "fir_taps",
                    "FIR Taps",
                    self.fir_taps as i32,
                    31,
                    16385,
                ));
            }
        }

        if self.is_per_channel() {
            for (ch, &freq) in self.channel_frequencies_hz.iter().enumerate() {
                let id = format!("channel_frequency_{ch}");
                let name = format!("Frequency Ch{ch}");
                params.push(Parameter::new_float(&id, &name, freq, 20.0, 20000.0).with_unit("Hz"));
                let mode_id = format!("channel_mode_{ch}");
                let mode_name = format!("Mode Ch{ch}");
                let mode_str = match self
                    .op_modes
                    .get(ch)
                    .copied()
                    .unwrap_or(PerChannelOpMode::Mute)
                {
                    PerChannelOpMode::Lowpass => "lowpass",
                    PerChannelOpMode::Highpass => "highpass",
                    PerChannelOpMode::Mute => "mute",
                    PerChannelOpMode::Passthrough => "passthrough",
                };
                params.push(Parameter::new_string(
                    &mode_id,
                    &mode_name,
                    mode_str.to_string(),
                ));
            }
        }

        self.cached_parameters = params;
    }

    /// Update an existing float parameter's value without rebuilding its
    /// metadata. Realtime IIR cutoff automation must not allocate or release
    /// the cached parameter strings and ranges.
    fn update_cached_float_parameter(&mut self, id: &str, value: f32) {
        if let Some(parameter) = self
            .cached_parameters
            .iter_mut()
            .find(|parameter| parameter.id.as_str() == id)
        {
            parameter.default_value = ParameterValue::Float(value);
        }
    }

    pub(super) fn rebuild_fir_crossovers(&mut self) {
        if self.kind != CrossoverKind::LinearPhase {
            return;
        }
        let sr = self.sample_rate as f32;
        let primary = self
            .all_frequencies
            .first()
            .copied()
            .unwrap_or_else(|| self.freq_smoother.target());
        self.fir_crossover_2way = (self.all_frequencies.len() == 1)
            .then(|| FirCrossover::new(primary, sr, self.num_channels, self.fir_taps));
        self.fir_multiband = (self.all_frequencies.len() > 1).then(|| {
            MultibandFirCrossover::new(&self.all_frequencies, sr, self.num_channels, self.fir_taps)
        });
        self.fir_band_alignment = (self.all_frequencies.len() > 1).then(|| {
            FirBandAlignment::new(
                self.all_frequencies.len() + 1,
                self.num_channels,
                (self.fir_taps - 1) / 2,
            )
        });
    }

    pub fn from_params(
        num_channels: usize,
        params: &CrossoverPluginParams,
    ) -> Result<Self, String> {
        if params
            .extra_frequencies
            .iter()
            .any(|frequency| !frequency.is_finite())
        {
            return Err("extra crossover frequencies must be finite".into());
        }
        if params
            .channel_frequencies_hz
            .iter()
            .any(|frequency| !frequency.is_finite())
        {
            return Err("per-channel crossover frequencies must be finite".into());
        }
        let channel_modes = params.channel_modes.as_deref().unwrap_or_default();
        let parsed_channel_modes = channel_modes
            .iter()
            .map(|mode| PerChannelOpMode::from_str(mode))
            .collect::<Result<Vec<_>, _>>()?;

        let topology = match params.topology {
            Some(topology) => topology,
            None if !params.channel_frequencies_hz.is_empty()
                && !params.extra_frequencies.is_empty() =>
            {
                return Err(
                    "crossover topology is ambiguous when channel and extra frequencies are both present"
                        .into(),
                );
            }
            None if !params.channel_frequencies_hz.is_empty() => CrossoverTopology::PerChannel,
            None => CrossoverTopology::Bands,
        };

        if topology == CrossoverTopology::PerChannel {
            // Per-channel mode: a complete explicit mode list selects the
            // active topology, so the global `output` is dormant and may be
            // any valid band-mode value (including `both`). Keep the scalar
            // fallback for legacy callers with empty or short mode lists.
            let expected = params.channel_frequencies_hz.len();
            if expected != num_channels {
                return Err(format!(
                    "CrossoverPlugin::from_params: channels arg ({num_channels}) does not match channel_frequencies_hz.len() ({expected})"
                ));
            }
            if params.topology == Some(CrossoverTopology::PerChannel)
                && params.channel_modes.is_some()
                && channel_modes.len() != expected
            {
                return Err(format!(
                    "explicit per-channel channel_modes must contain {expected} modes, got {}",
                    channel_modes.len()
                ));
            }
            if channel_modes.len() > expected {
                return Err(format!(
                    "channel_modes.len()={} exceeds channel_frequencies_hz.len() ({expected})",
                    channel_modes.len()
                ));
            }
            let default_mode = if channel_modes.len() == expected {
                None
            } else {
                Some(PerChannelOpMode::from_str(&params.output)?)
            };
            let mut modes = Vec::with_capacity(expected);
            for i in 0..expected {
                if let Some(mode) = parsed_channel_modes.get(i) {
                    modes.push(*mode);
                } else {
                    modes.push(default_mode.expect("incomplete modes require scalar fallback"));
                }
            }
            return Self::new_per_channel(
                &params.crossover_type,
                params.channel_frequencies_hz.clone(),
                modes,
            );
        }
        let band_count = params
            .band_count
            .unwrap_or_else(|| params.extra_frequencies.len().saturating_add(2));
        if !(2..=4).contains(&band_count) {
            return Err(format!(
                "crossover band_count must be in [2, 4], got {band_count}"
            ));
        }
        let active_extra_count = band_count - 2;
        if params.extra_frequencies.len() < active_extra_count {
            return Err(format!(
                "crossover band_count {band_count} requires {active_extra_count} active extra frequencies, got {}",
                params.extra_frequencies.len()
            ));
        }
        let active_extra_frequencies = &params.extra_frequencies[..active_extra_count];
        if active_extra_frequencies
            .iter()
            .any(|frequency| !frequency.is_finite())
        {
            return Err("active extra crossover frequencies must be finite".into());
        }
        Self::new_multiway_with_fir_taps(
            num_channels,
            &params.crossover_type,
            params.frequency,
            &params.output,
            active_extra_frequencies,
            params.fir_taps.unwrap_or(DEFAULT_FIR_CROSSOVER_TAPS),
        )
    }

    /// Number of output bands based on current configuration.
    pub(super) fn num_bands(&self) -> usize {
        self.all_frequencies.len() + 1
    }

    /// Calculate output channels based on mode and band count.
    pub(super) fn calc_output_channels(&self) -> usize {
        if self.is_per_channel() {
            return self.num_channels;
        }
        match self.mode {
            CrossoverMode::Lowpass | CrossoverMode::Highpass => self.num_channels,
            CrossoverMode::Both => self.num_channels * self.num_bands(),
        }
    }

    /// Returns true if operating in multi-way (3+ bands) mode.
    pub(super) fn is_multiway(&self) -> bool {
        self.all_frequencies.len() > 1
    }

    /// Report the persistent FIR DSP payload before the graph is admitted.
    /// Returns `None` for LR/per-channel instances.
    pub fn fir_memory_report(&self) -> Option<FirMemoryReport> {
        if self.kind != CrossoverKind::LinearPhase || self.is_per_channel() {
            return None;
        }
        let sample_bytes = std::mem::size_of::<f32>();
        let index_bytes = std::mem::size_of::<usize>();
        let splits = self.all_frequencies.len();
        let bands = splits + 1;

        let coefficient_bytes = splits
            .saturating_mul(self.fir_taps)
            .saturating_mul(sample_bytes);
        let history_bytes = splits.saturating_mul(self.num_channels).saturating_mul(
            self.fir_taps
                .saturating_mul(sample_bytes)
                .saturating_add(index_bytes),
        );
        let split_latency = (self.fir_taps - 1) / 2;
        let alignment_frames = (0..bands)
            .map(|band| {
                let intrinsic_splits = (band + 1).min(splits);
                (splits - intrinsic_splits).saturating_mul(split_latency)
            })
            .sum::<usize>();
        let alignment_bytes = alignment_frames
            .saturating_mul(self.num_channels)
            .saturating_mul(sample_bytes);
        // Plugin low/high/band-flat storage plus the multiband FIR's three
        // channel-sized scratch buffers (the latter are absent for two-way).
        let scratch_channels = if splits == 1 { 2 } else { bands + 5 };
        let scratch_bytes = scratch_channels
            .saturating_mul(self.num_channels)
            .saturating_mul(sample_bytes);
        let total_bytes = coefficient_bytes
            .saturating_add(history_bytes)
            .saturating_add(alignment_bytes)
            .saturating_add(scratch_bytes);
        Some(FirMemoryReport {
            coefficient_bytes,
            history_bytes,
            alignment_bytes,
            scratch_bytes,
            total_bytes,
        })
    }

    /// Parse "frequency_N" into an extra smoother index (0-based).
    /// "frequency_2" -> Some(0), "frequency_3" -> Some(1), etc.
    /// Returns None for indices < 2 to prevent aliasing "frequency_1" onto index 0.
    pub(super) fn parse_extra_freq_index(s: &str) -> Option<usize> {
        s.strip_prefix("frequency_")
            .and_then(|idx_str| idx_str.parse::<usize>().ok())
            .and_then(|idx| if idx >= 2 { Some(idx - 2) } else { None })
    }

    /// Run the prepared FIR bank; absent input supplies exact zero continuation.
    fn process_fir(
        &mut self,
        input: Option<&[f32]>,
        output: &mut [f32],
        num_frames: usize,
    ) -> PluginResult<()> {
        let in_ch = self.num_channels;
        let out_ch = self.calc_output_channels();
        if self.is_multiway() {
            // This scratch is unused by the multiway FIR kernel. Reuse it as
            // one immutable zero input frame while retaining the normal path.
            if input.is_none() {
                self.low_buf.fill(0.0);
            }
            let num_bands = self.num_bands();
            let mb = self.fir_multiband.as_mut().ok_or_else(|| {
                "crossover multiband FIR bank missing for multi-way linear-phase mode; rebuild the graph".to_string()
            })?;

            for frame in 0..num_frames {
                let in_off = frame * in_ch;
                let out_off = frame * out_ch;
                let frame_slice = input.map_or(self.low_buf.as_slice(), |input| {
                    &input[in_off..in_off + in_ch]
                });

                {
                    let flat = &mut self.band_flat[..num_bands * in_ch];
                    let mut band_slices: [&mut [f32]; 4] = [&mut [], &mut [], &mut [], &mut []];
                    let mut remaining = flat;
                    for slot in band_slices.iter_mut().take(num_bands) {
                        let (chunk, rest) = remaining.split_at_mut(in_ch);
                        *slot = chunk;
                        remaining = rest;
                    }
                    mb.process_frame(frame_slice, &mut band_slices[..num_bands]);
                }
                if let Some(alignment) = &mut self.fir_band_alignment {
                    alignment.process_frame(&mut self.band_flat[..num_bands * in_ch]);
                }

                match self.mode {
                    CrossoverMode::Lowpass => {
                        output[out_off..out_off + in_ch].copy_from_slice(&self.band_flat[..in_ch]);
                    }
                    CrossoverMode::Highpass => {
                        let hi_off = (num_bands - 1) * in_ch;
                        output[out_off..out_off + in_ch]
                            .copy_from_slice(&self.band_flat[hi_off..hi_off + in_ch]);
                    }
                    CrossoverMode::Both => {
                        output[out_off..out_off + out_ch]
                            .copy_from_slice(&self.band_flat[..out_ch]);
                    }
                }
            }
        } else {
            let xover = self.fir_crossover_2way.as_mut().ok_or_else(|| {
                "crossover two-way FIR bank missing for linear-phase mode; rebuild the graph"
                    .to_string()
            })?;
            for frame in 0..num_frames {
                let in_off = frame * in_ch;
                let out_off = frame * out_ch;
                for channel in 0..in_ch {
                    let sample = input.map_or(0.0, |input| input[in_off + channel]);
                    let (low, high) = xover.process_sample(sample, channel);
                    self.low_buf[channel] = low;
                    self.high_buf[channel] = high;
                }

                match self.mode {
                    CrossoverMode::Lowpass => {
                        output[out_off..out_off + in_ch].copy_from_slice(&self.low_buf);
                    }
                    CrossoverMode::Highpass => {
                        output[out_off..out_off + in_ch].copy_from_slice(&self.high_buf);
                    }
                    CrossoverMode::Both => {
                        output[out_off..out_off + in_ch].copy_from_slice(&self.low_buf);
                        output[out_off + in_ch..out_off + 2 * in_ch]
                            .copy_from_slice(&self.high_buf);
                    }
                }
            }
        }
        Ok(())
    }

    fn process_new_iir(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_frames: usize,
    ) -> Result<(), String> {
        let channels = self.num_channels;
        let output_channels = self.output_channels();
        let num_bands = self.num_bands();

        if let Some(banks) = self.family_multiband_lr.as_mut() {
            for frame in 0..num_frames {
                if self.smoother_subblock_phase == 0 {
                    let frequency = self.freq_smoother.next_n(COEFFICIENT_UPDATE_SAMPLES);
                    for bank in banks.iter_mut() {
                        bank[0].set_frequency(frequency);
                    }
                    for (index, smoother) in self.extra_freq_smoothers.iter_mut().enumerate() {
                        let frequency = smoother.next_n(COEFFICIENT_UPDATE_SAMPLES);
                        for bank in banks.iter_mut() {
                            bank[index + 1].set_frequency(frequency);
                        }
                    }
                }
                self.smoother_subblock_phase =
                    (self.smoother_subblock_phase + 1) % COEFFICIENT_UPDATE_SAMPLES;

                let input_offset = frame * channels;
                let output_offset = frame * output_channels;
                let frame_input = &input[input_offset..input_offset + channels];
                for (band, bank) in banks.iter_mut().enumerate() {
                    let band_offset = band * channels;
                    self.band_flat[band_offset..band_offset + channels]
                        .copy_from_slice(frame_input);
                    for (split, crossover) in bank.iter_mut().enumerate() {
                        crossover.process_frame(
                            &self.band_flat[band_offset..band_offset + channels],
                            &mut self.low_buf,
                            &mut self.high_buf,
                        );
                        if band < split {
                            for (low, high) in self.low_buf.iter_mut().zip(&self.high_buf) {
                                *low += *high;
                            }
                            self.band_flat[band_offset..band_offset + channels]
                                .copy_from_slice(&self.low_buf);
                        } else if band == split {
                            self.band_flat[band_offset..band_offset + channels]
                                .copy_from_slice(&self.low_buf);
                        } else {
                            self.band_flat[band_offset..band_offset + channels]
                                .copy_from_slice(&self.high_buf);
                        }
                    }
                }
                match self.mode {
                    CrossoverMode::Lowpass => output[output_offset..output_offset + channels]
                        .copy_from_slice(&self.band_flat[..channels]),
                    CrossoverMode::Highpass => {
                        let high_offset = (num_bands - 1) * channels;
                        output[output_offset..output_offset + channels]
                            .copy_from_slice(&self.band_flat[high_offset..high_offset + channels]);
                    }
                    CrossoverMode::Both => output[output_offset..output_offset + output_channels]
                        .copy_from_slice(&self.band_flat[..output_channels]),
                }
            }
            return Ok(());
        }

        if let Some(splits) = self.family_multiband_serial.as_mut() {
            let bands = splits.len() + 1;
            let residual_offset = (bands - 1) * channels;
            for frame in 0..num_frames {
                if self.smoother_subblock_phase == 0 {
                    let frequency = self.freq_smoother.next_n(COEFFICIENT_UPDATE_SAMPLES);
                    splits[0].set_frequency(frequency);
                    for (index, smoother) in self.extra_freq_smoothers.iter_mut().enumerate() {
                        let frequency = smoother.next_n(COEFFICIENT_UPDATE_SAMPLES);
                        splits[index + 1].set_frequency(frequency);
                    }
                }
                self.smoother_subblock_phase =
                    (self.smoother_subblock_phase + 1) % COEFFICIENT_UPDATE_SAMPLES;

                let input_offset = frame * channels;
                let output_offset = frame * output_channels;
                let frame_input = &input[input_offset..input_offset + channels];
                self.band_flat[residual_offset..residual_offset + channels]
                    .copy_from_slice(frame_input);
                for (split_index, crossover) in splits.iter_mut().enumerate() {
                    crossover.process_frame(
                        &self.band_flat[residual_offset..residual_offset + channels],
                        &mut self.low_buf,
                        &mut self.high_buf,
                    );
                    let low_offset = split_index * channels;
                    self.band_flat[low_offset..low_offset + channels]
                        .copy_from_slice(&self.low_buf);
                    self.band_flat[residual_offset..residual_offset + channels]
                        .copy_from_slice(&self.high_buf);
                }
                match self.mode {
                    CrossoverMode::Lowpass => output[output_offset..output_offset + channels]
                        .copy_from_slice(&self.band_flat[..channels]),
                    CrossoverMode::Highpass => output[output_offset..output_offset + channels]
                        .copy_from_slice(
                            &self.band_flat[residual_offset..residual_offset + channels],
                        ),
                    CrossoverMode::Both => output[output_offset..output_offset + output_channels]
                        .copy_from_slice(&self.band_flat[..output_channels]),
                }
            }
            return Ok(());
        }

        let crossover = self
            .family_two_way
            .as_mut()
            .ok_or_else(|| "new IIR crossover state is missing; rebuild the graph".to_string())?;
        let mut frame = 0;
        while frame < num_frames {
            if self.smoother_subblock_phase == 0 {
                let frequency = self.freq_smoother.next_n(COEFFICIENT_UPDATE_SAMPLES);
                crossover.set_frequency(frequency);
            }
            let segment =
                (COEFFICIENT_UPDATE_SAMPLES - self.smoother_subblock_phase).min(num_frames - frame);
            for segment_frame in frame..frame + segment {
                let input_offset = segment_frame * channels;
                let output_offset = segment_frame * output_channels;
                crossover.process_frame(
                    &input[input_offset..input_offset + channels],
                    &mut self.low_buf,
                    &mut self.high_buf,
                );
                match self.mode {
                    CrossoverMode::Lowpass => output[output_offset..output_offset + channels]
                        .copy_from_slice(&self.low_buf),
                    CrossoverMode::Highpass => output[output_offset..output_offset + channels]
                        .copy_from_slice(&self.high_buf),
                    CrossoverMode::Both => {
                        output[output_offset..output_offset + channels]
                            .copy_from_slice(&self.low_buf);
                        output[output_offset + channels..output_offset + output_channels]
                            .copy_from_slice(&self.high_buf);
                    }
                }
            }
            frame += segment;
            self.smoother_subblock_phase =
                (self.smoother_subblock_phase + segment) % COEFFICIENT_UPDATE_SAMPLES;
        }
        Ok(())
    }

    fn fir_support_frames(&self) -> usize {
        // A cascade of S length-L FIR splits has support S*(L-1). An
        // earlier band's alignment adds only half the missing split support,
        // so this also bounds every aligned low/high output selection.
        self.all_frequencies.len() * (self.fir_taps - 1)
    }

    /// Process a coefficient-stable LR24 two-way segment. Output mode is
    /// selected once for the segment and the external frame-slice adapter is
    /// bypassed in favor of the crossover's scalar channel primitive.
    fn process_lr_two_way_segment(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        first_frame: usize,
        frames: usize,
    ) {
        let channels = self.num_channels;
        let output_channels = self.calc_output_channels();
        let end_frame = first_frame + frames;
        match self.mode {
            CrossoverMode::Lowpass => {
                for frame in first_frame..end_frame {
                    for channel in 0..channels {
                        let (low, _) = self
                            .crossover_2way
                            .process(input[frame * channels + channel], channel);
                        output[frame * output_channels + channel] = low;
                    }
                }
            }
            CrossoverMode::Highpass => {
                for frame in first_frame..end_frame {
                    for channel in 0..channels {
                        let (_, high) = self
                            .crossover_2way
                            .process(input[frame * channels + channel], channel);
                        output[frame * output_channels + channel] = high;
                    }
                }
            }
            CrossoverMode::Both => {
                for frame in first_frame..end_frame {
                    let output_offset = frame * output_channels;
                    for channel in 0..channels {
                        let (low, high) = self
                            .crossover_2way
                            .process(input[frame * channels + channel], channel);
                        output[output_offset + channel] = low;
                        output[output_offset + channels + channel] = high;
                    }
                }
            }
        }
    }
}

impl Plugin for CrossoverPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Crossover", env!("CARGO_PKG_VERSION"), "SotF").with_description(
            "Linkwitz-Riley and linear-phase FIR crossover with multi-way and dual-output support",
        )
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }

    fn output_channels(&self) -> usize {
        self.calc_output_channels()
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        let latency_samples = self.latency_samples();
        let cost = if self.kind == CrossoverKind::LinearPhase {
            PluginCostClass::Convolution
        } else {
            PluginCostClass::Iir
        };
        let mut metadata = PluginCompileMetadata::linear_transform(
            cost,
            None,
            latency_samples,
            false,
            true,
            false,
        );
        // FIR instances carry explicit latency and large convolution state;
        // keep them as scheduling boundaries. LR instances are ordinary
        // zero-latency linear transforms and can participate in fusion.
        metadata.boundary = self.kind == CrossoverKind::LinearPhase;
        metadata
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.fir_drain_remaining.is_some() {
            return Err("reset the FIR crossover before changing controls after drain".into());
        }
        // FIR coefficients (and, for multi-way FIR, the complete set of
        // convolution histories) are compile-time state. Rebuilding them from
        // the control path after initialization would allocate, reset audio
        // history, and potentially change the graph's declared latency.
        if self.initialized
            && self.kind == CrossoverKind::LinearPhase
            && (id.as_str() == "frequency"
                || id.as_str() == "fir_taps"
                || Self::parse_extra_freq_index(&id.0).is_some())
        {
            return Err(format!(
                "crossover '{}' is a structural FIR parameter; rebuild the graph to change it",
                id.0
            ));
        }

        // In per-channel mode, the global `frequency` and `mode` parameters
        // don't apply — every channel has its own. Reject these writes so
        // they don't silently mutate unused global state.
        if self.is_per_channel() && (id.as_str() == "frequency" || id.as_str() == "mode") {
            return Err(format!(
                "crossover '{}' is in per-channel mode; use 'channel_frequency_N' / 'channel_mode_N' instead",
                id.0
            ));
        }

        if self.initialized
            && (parse_channel_freq_id(&id.0).is_some() || parse_channel_mode_id(&id.0).is_some())
        {
            return Err(format!(
                "crossover '{}' is a structural per-channel parameter; rebuild the graph to change it",
                id.0
            ));
        }

        // `Plugin::validate_parameter()` clones the complete parameter list,
        // which allocates even for valid scalar automation. Validate against
        // this instance's already cached metadata instead.
        self.cached_parameters
            .iter()
            .find(|parameter| parameter.id.as_str() == id.as_str())
            .ok_or_else(|| format!("Unknown parameter: {id}"))?
            .validate(&value)
            .map_err(|error| format!("{id}: {error}"))?;

        if id.as_str() == "type" {
            Err("crossover type is structural; rebuild the graph to change it".into())
        } else if id.as_str() == "frequency" {
            let val = value.as_float().unwrap_or(1000.0);
            if val.is_finite() {
                let frequency = val;
                if self.kind.is_new_iir() {
                    validate_new_iir_cutoff(frequency, self.sample_rate)?;
                }
                if self
                    .all_frequencies
                    .get(1)
                    .is_some_and(|next| frequency >= *next)
                {
                    return Err("frequency must remain below frequency_2".into());
                }
                self.freq_smoother.set_target(frequency);
                // Update first frequency in multi-way list and re-sort to maintain
                // sorted order. MultibandLr4Crossover requires sorted frequencies.
                if !self.all_frequencies.is_empty() {
                    self.all_frequencies[0] = frequency;
                    self.rebuild_fir_crossovers();
                }
                self.update_cached_float_parameter("frequency", frequency);
            }
            Ok(())
        } else if id.as_str() == "mode" {
            if let Some(s) = value.as_string() {
                let new_mode = CrossoverMode::from_str(s)?;
                if new_mode != self.mode {
                    return Err(
                        "crossover mode is structural; rebuild the graph to change it".into(),
                    );
                }
            }
            Ok(())
        } else if let Some(smoother_idx) = Self::parse_extra_freq_index(&id.0) {
            let val = value.as_float().unwrap_or(1000.0);
            if val.is_finite() && smoother_idx < self.extra_freq_smoothers.len() {
                let freq_idx = smoother_idx + 1; // offset: extra smoothers start at freq index 1
                if freq_idx < self.all_frequencies.len() {
                    let frequency = val;
                    if self.kind.is_new_iir() {
                        validate_new_iir_cutoff(frequency, self.sample_rate)?;
                    }
                    let lower = self.all_frequencies[freq_idx - 1];
                    let upper = self.all_frequencies.get(freq_idx + 1).copied();
                    if frequency <= lower || upper.is_some_and(|upper| frequency >= upper) {
                        return Err(format!(
                            "frequency_{} must remain between its neighboring crossover points",
                            smoother_idx + 2
                        ));
                    }
                    self.extra_freq_smoothers[smoother_idx].set_target(frequency);
                    self.all_frequencies[freq_idx] = frequency;
                    self.rebuild_fir_crossovers();
                    self.update_cached_float_parameter(&id.0, frequency);
                }
            }
            Ok(())
        } else if id.as_str() == "fir_taps" {
            let taps = value.as_int().unwrap_or(DEFAULT_FIR_CROSSOVER_TAPS as i32);
            self.fir_taps = (taps.max(31) as usize).min(16385);
            if self.fir_taps.is_multiple_of(2) {
                self.fir_taps += 1;
            }
            self.rebuild_fir_crossovers();
            self.rebuild_cached_parameters();
            Ok(())
        } else if let Some(ch) = parse_channel_freq_id(&id.0) {
            if !self.is_per_channel() || ch >= self.channel_frequencies_hz.len() {
                return Err(format!("invalid per-channel frequency id: {}", id.0));
            }
            let val = value
                .as_float()
                .ok_or_else(|| "channel frequency must be a float".to_string())?;
            if self.kind.is_new_iir() {
                validate_new_iir_cutoff(val, self.sample_rate)?;
            } else {
                let nyquist_limit = self.sample_rate as f32 * 0.5 * 0.99;
                if !val.is_finite() || val <= 0.0 || val >= nyquist_limit {
                    return Err(format!(
                        "channel frequency must be finite and in (0, {nyquist_limit}) Hz"
                    ));
                }
            }
            self.channel_frequencies_hz[ch] = val;
            if self.kind.is_new_iir() {
                self.per_channel_iir[ch].set_frequency(val);
            } else {
                self.per_channel_lr4[ch] = Lr4Crossover::new(val, self.sample_rate as f32, 1);
            }
            self.rebuild_cached_parameters();
            Ok(())
        } else if let Some(ch) = parse_channel_mode_id(&id.0) {
            if !self.is_per_channel() || ch >= self.op_modes.len() {
                return Err(format!("invalid per-channel mode id: {}", id.0));
            }
            let s = value
                .as_string()
                .ok_or_else(|| "channel mode must be a string".to_string())?;
            self.op_modes[ch] = PerChannelOpMode::from_str(s)?;
            self.rebuild_cached_parameters();
            Ok(())
        } else {
            Err(format!("Unknown parameter: {}", id))
        }
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        if id.as_str() == "type" {
            Some(ParameterValue::String(self.kind.as_str().to_string()))
        } else if id.as_str() == "frequency" {
            Some(ParameterValue::Float(self.freq_smoother.target()))
        } else if id.as_str() == "mode" {
            Some(ParameterValue::String(self.mode.as_str().to_string()))
        } else if let Some(smoother_idx) = Self::parse_extra_freq_index(&id.0) {
            self.extra_freq_smoothers
                .get(smoother_idx)
                .map(|s| ParameterValue::Float(s.target()))
        } else if id.as_str() == "fir_taps" && self.kind == CrossoverKind::LinearPhase {
            Some(ParameterValue::Int(self.fir_taps as i32))
        } else if let Some(ch) = parse_channel_freq_id(&id.0) {
            if self.is_per_channel() {
                self.channel_frequencies_hz
                    .get(ch)
                    .copied()
                    .map(ParameterValue::Float)
            } else {
                None
            }
        } else if let Some(ch) = parse_channel_mode_id(&id.0) {
            if self.is_per_channel() {
                self.op_modes.get(ch).map(|m| {
                    ParameterValue::String(
                        match m {
                            PerChannelOpMode::Lowpass => "lowpass",
                            PerChannelOpMode::Highpass => "highpass",
                            PerChannelOpMode::Mute => "mute",
                            PerChannelOpMode::Passthrough => "passthrough",
                        }
                        .to_string(),
                    )
                })
            } else {
                None
            }
        } else {
            None
        }
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("crossover sample rate must be greater than zero".into());
        }
        if self.kind.is_new_iir() {
            validate_new_iir_frequencies(&self.all_frequencies, sample_rate)?;
            for &frequency in &self.channel_frequencies_hz {
                validate_new_iir_cutoff(frequency, sample_rate)?;
            }
        } else {
            let nyquist_limit = sample_rate as f32 * 0.5 * 0.99;
            if self
                .all_frequencies
                .iter()
                .chain(self.channel_frequencies_hz.iter())
                .any(|f| !f.is_finite() || *f <= 0.0 || *f >= nyquist_limit)
            {
                return Err(format!(
                    "crossover frequency exceeds sample-rate limit {nyquist_limit} Hz"
                ));
            }
        }
        if self.kind.is_new_iir() {
            self.smoother_subblock_phase = 0;
        }
        self.sample_rate = sample_rate;
        // Clamp all frequencies to just below Nyquist to prevent nonsense biquad
        // coefficients at low sample rates (e.g. 32 kHz with a 20 kHz crossover).
        let nyquist_limit = sample_rate as f32 * 0.5 * 0.99;

        if self.is_per_channel() {
            if self.kind.is_new_iir() {
                for (split, &frequency) in self
                    .per_channel_iir
                    .iter_mut()
                    .zip(&self.channel_frequencies_hz)
                {
                    split.reconfigure(frequency, sample_rate);
                    split.reset();
                }
            } else {
                // Mutate the stored values so `get_parameter` and serialization
                // reflect the clamped legacy LR24 realization.
                for freq in self.channel_frequencies_hz.iter_mut() {
                    *freq = freq.min(nyquist_limit);
                }
                self.per_channel_lr4 = self
                    .channel_frequencies_hz
                    .iter()
                    .map(|&f| {
                        let clamped = f.min(nyquist_limit);
                        Lr4Crossover::new(clamped, sample_rate as f32, 1)
                    })
                    .collect();
            }
            self.per_channel_low.resize(1, 0.0);
            self.per_channel_high.resize(1, 0.0);
            self.initialized = true;
            return Ok(());
        }

        let clamped_primary = if self.kind.is_new_iir() {
            self.freq_smoother.target()
        } else {
            self.freq_smoother.target().min(nyquist_limit)
        };
        self.freq_smoother = LogSmoother::new(clamped_primary, 20.0, sample_rate);
        self.crossover_2way
            .reinit(clamped_primary, sample_rate as f32, self.num_channels);
        if let Some(crossover) = self.family_two_way.as_mut() {
            crossover.reconfigure(clamped_primary, sample_rate);
            crossover.reset();
        }
        self.low_buf.resize(self.num_channels, 0.0);
        self.high_buf.resize(self.num_channels, 0.0);

        for (index, smoother) in self.extra_freq_smoothers.iter_mut().enumerate() {
            let frequency = if self.kind.is_new_iir() {
                smoother.target()
            } else {
                smoother.target().min(nyquist_limit)
            };
            self.all_frequencies[index + 1] = frequency;
            *smoother = LogSmoother::new(frequency, 20.0, sample_rate);
        }
        if !self.all_frequencies.is_empty() {
            self.all_frequencies[0] = clamped_primary;
        }

        if let Some(ref mut banks) = self.multiband {
            for bank in banks {
                for (split, crossover) in bank.iter_mut().enumerate() {
                    crossover.reinit(
                        self.all_frequencies[split],
                        sample_rate as f32,
                        self.num_channels,
                    );
                }
            }
        }
        if let Some(banks) = self.family_multiband_lr.as_mut() {
            for bank in banks {
                for (split, crossover) in bank.iter_mut().enumerate() {
                    crossover.reconfigure(self.all_frequencies[split], sample_rate);
                    crossover.reset();
                }
            }
        }
        if let Some(splits) = self.family_multiband_serial.as_mut() {
            for (split, crossover) in splits.iter_mut().enumerate() {
                crossover.reconfigure(self.all_frequencies[split], sample_rate);
                crossover.reset();
            }
        }
        self.rebuild_fir_crossovers();

        // Resize band flat buffer
        let nb = self.num_bands();
        self.band_flat.resize(nb * self.num_channels, 0.0);

        self.initialized = true;
        if self.kind == CrossoverKind::LinearPhase {
            self.reset();
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.fir_has_input = false;
        self.fir_drain_remaining = None;
        if self.is_per_channel() {
            for xo in &mut self.per_channel_lr4 {
                xo.reset();
            }
            for split in &mut self.per_channel_iir {
                split.reset();
            }
            return;
        }
        self.crossover_2way.reset();
        if let Some(ref mut xover) = self.fir_crossover_2way {
            xover.reset();
        }
        // Reset smoothers to their targets so that a mid-transition reset does not
        // cause a click from the remaining interpolation step on the next block.
        self.freq_smoother.reset(self.freq_smoother.target());
        self.smoother_subblock_phase = 0;
        for s in &mut self.extra_freq_smoothers {
            s.reset(s.target());
        }
        if let Some(ref mut banks) = self.multiband {
            for bank in banks {
                for crossover in bank {
                    crossover.reset();
                }
            }
        }
        if let Some(crossover) = self.family_two_way.as_mut() {
            crossover.reset();
        }
        if let Some(banks) = self.family_multiband_lr.as_mut() {
            for bank in banks {
                for crossover in bank {
                    crossover.reset();
                }
            }
        }
        if let Some(splits) = self.family_multiband_serial.as_mut() {
            for crossover in splits {
                crossover.reset();
            }
        }
        if let Some(ref mut mb) = self.fir_multiband {
            mb.reset();
        }
        if let Some(alignment) = &mut self.fir_band_alignment {
            alignment.reset();
        }
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        enable_ftz_daz();
        let num_frames = context.num_frames;
        let in_ch = self.num_channels;
        let out_ch = self.output_channels();
        let expected_input = num_frames
            .checked_mul(in_ch)
            .ok_or_else(|| "crossover input size overflow".to_string())?;
        let expected_output = num_frames
            .checked_mul(out_ch)
            .ok_or_else(|| "crossover output size overflow".to_string())?;
        if input.len() != expected_input || output.len() != expected_output {
            return Err(format!(
                "crossover buffer mismatch: input {} (expected {expected_input}), output {} (expected {expected_output})",
                input.len(),
                output.len()
            ));
        }

        if self.kind == CrossoverKind::LinearPhase {
            if !self.initialized || context.sample_rate != self.sample_rate {
                return Err(
                    "FIR crossover requires initialization at the process sample rate".into(),
                );
            }
            if num_frames == 0 {
                return Ok(0);
            }
            if self.fir_drain_remaining.is_some() {
                return Err("reset the FIR crossover before processing after drain".into());
            }
        }

        if self.is_per_channel() {
            // Per-channel mode: each channel is processed independently by
            // its own single-channel LR24 crossover. Output channel count
            // equals input channel count.
            let mut low_scratch = [0.0f32];
            let mut high_scratch = [0.0f32];
            for frame in 0..num_frames {
                let in_off = frame * in_ch;
                let out_off = frame * in_ch;
                for ch in 0..in_ch {
                    let sample = input[in_off + ch];
                    match self.op_modes[ch] {
                        PerChannelOpMode::Mute => {
                            output[out_off + ch] = 0.0;
                        }
                        PerChannelOpMode::Passthrough => {
                            output[out_off + ch] = sample;
                        }
                        mode => {
                            let sample_arr = [sample];
                            if self.kind.is_new_iir() {
                                self.per_channel_iir[ch].process_frame(
                                    &sample_arr,
                                    &mut low_scratch,
                                    &mut high_scratch,
                                );
                            } else {
                                self.per_channel_lr4[ch].process_frame(
                                    &sample_arr,
                                    &mut low_scratch,
                                    &mut high_scratch,
                                );
                            }
                            output[out_off + ch] = match mode {
                                PerChannelOpMode::Lowpass => low_scratch[0],
                                PerChannelOpMode::Highpass => high_scratch[0],
                                // Mute and Passthrough handled above; matching
                                // here keeps the match exhaustive.
                                PerChannelOpMode::Mute => 0.0,
                                PerChannelOpMode::Passthrough => sample,
                            };
                        }
                    }
                }
            }
            flush_denormals_inplace(output);
            return Ok(num_frames);
        }

        if self.kind == CrossoverKind::LinearPhase {
            self.process_fir(Some(input), output, num_frames)?;
            self.fir_has_input = true;
        } else if self.kind.is_new_iir() {
            self.process_new_iir(input, output, num_frames)?;
        } else if self.is_multiway() {
            // Multi-way processing
            let num_bands = self.num_bands();
            let banks = self.multiband.as_mut().ok_or_else(|| {
                "crossover LR multiband bank missing for multi-way mode; rebuild the graph"
                    .to_string()
            })?;

            // Sub-block size for frequency updates: every 16 samples to avoid
            // zipper noise while keeping CPU cost reasonable.
            const SUBBLOCK: usize = 16;

            for frame in 0..num_frames {
                if self.smoother_subblock_phase == 0 {
                    let new_freq0 = self.freq_smoother.next_n(SUBBLOCK);
                    for bank in banks.iter_mut() {
                        bank[0].set_frequency(new_freq0);
                    }
                    for (i, smoother) in self.extra_freq_smoothers.iter_mut().enumerate() {
                        let f = smoother.next_n(SUBBLOCK);
                        for bank in banks.iter_mut() {
                            bank[i + 1].set_frequency(f);
                        }
                    }
                }
                self.smoother_subblock_phase = (self.smoother_subblock_phase + 1) % SUBBLOCK;
                let in_off = frame * in_ch;
                let out_off = frame * out_ch;
                let frame_slice = &input[in_off..in_off + in_ch];

                for (band, bank) in banks.iter_mut().enumerate() {
                    let band_offset = band * in_ch;
                    self.band_flat[band_offset..band_offset + in_ch].copy_from_slice(frame_slice);
                    for (split, crossover) in bank.iter_mut().enumerate() {
                        crossover.process_frame(
                            &self.band_flat[band_offset..band_offset + in_ch],
                            &mut self.low_buf,
                            &mut self.high_buf,
                        );
                        let filtered = if band < split {
                            // Earlier branches must stay all-pass through later
                            // splits so their sum telescopes to the product of
                            // each LR low/high all-pass pair.
                            for (low, high) in self.low_buf.iter_mut().zip(&self.high_buf) {
                                *low += *high;
                            }
                            &self.low_buf
                        } else if band == split {
                            &self.low_buf
                        } else {
                            &self.high_buf
                        };
                        self.band_flat[band_offset..band_offset + in_ch].copy_from_slice(filtered);
                    }
                }

                match self.mode {
                    CrossoverMode::Lowpass => {
                        output[out_off..out_off + in_ch].copy_from_slice(&self.band_flat[..in_ch]);
                    }
                    CrossoverMode::Highpass => {
                        let hi_off = (num_bands - 1) * in_ch;
                        output[out_off..out_off + in_ch]
                            .copy_from_slice(&self.band_flat[hi_off..hi_off + in_ch]);
                    }
                    CrossoverMode::Both => {
                        output[out_off..out_off + out_ch]
                            .copy_from_slice(&self.band_flat[..out_ch]);
                    }
                }
            }
        } else {
            // 2-way processing
            const SUBBLOCK: usize = 16;

            let mut frame = 0;
            while frame < num_frames {
                if self.smoother_subblock_phase == 0 {
                    let new_freq = self.freq_smoother.next_n(SUBBLOCK);
                    self.crossover_2way.set_frequency(new_freq);
                }
                let segment = (SUBBLOCK - self.smoother_subblock_phase).min(num_frames - frame);
                self.process_lr_two_way_segment(input, output, frame, segment);
                frame += segment;
                self.smoother_subblock_phase = (self.smoother_subblock_phase + segment) % SUBBLOCK;
            }
        }

        flush_denormals_inplace(output);
        Ok(num_frames)
    }

    fn tail_length(&self) -> TailLength {
        if self.kind == CrossoverKind::LinearPhase && self.initialized {
            TailLength::Finite(self.fir_support_frames() as u64)
        } else {
            // LR recurrence needs a separate truncation/settled-state policy.
            TailLength::Unknown
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        if self.kind == CrossoverKind::LinearPhase {
            FIR_DRAIN_FRAMES.min(self.fir_support_frames())
        } else {
            0
        }
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        let remaining = if self.kind == CrossoverKind::LinearPhase && self.fir_has_input {
            self.fir_drain_remaining
                .unwrap_or_else(|| self.fir_support_frames())
        } else {
            0
        };
        // A short FIR advertises its entire support; longer FIRs advance one block.
        std::num::NonZeroU64::new(remaining.div_ceil(FIR_DRAIN_FRAMES).max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if self.kind != CrossoverKind::LinearPhase
            || !self.fir_has_input
            || self.fir_drain_remaining == Some(0)
        {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let channels = self.calc_output_channels();
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("FIR crossover drain requires the initialized sample rate".into());
        }
        if output.len() < channels || !output.len().is_multiple_of(channels) {
            return Err("FIR crossover drain needs nonempty whole output frames".into());
        }
        let remaining = self
            .fir_drain_remaining
            .unwrap_or_else(|| self.fir_support_frames());
        let frames = remaining.min(FIR_DRAIN_FRAMES).min(output.len() / channels);
        enable_ftz_daz();
        self.process_fir(None, &mut output[..frames * channels], frames)?;
        flush_denormals_inplace(&mut output[..frames * channels]);
        self.fir_drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == frames,
        })
    }

    fn latency_samples(&self) -> usize {
        if self.kind != CrossoverKind::LinearPhase {
            return 0;
        }
        self.fir_multiband
            .as_ref()
            .map(|mb| mb.latency_samples())
            .or_else(|| {
                self.fir_crossover_2way
                    .as_ref()
                    .map(|xo| xo.latency_samples())
            })
            .unwrap_or(0)
    }
}
