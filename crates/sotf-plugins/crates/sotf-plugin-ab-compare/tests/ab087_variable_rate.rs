//! AB087 variable-rate composition: owned SRC roundtrips, burst staging, DAG drain,
//! and the proven band-mask residual flush.

// Rust guideline compliant 2026-02-21
use math_audio_iir_fir::{Biquad, BiquadFilterType};
use serde_json::{Value, json};
use sotf_host::CountingAlloc;
use sotf_host::host::{DawHost, GraphEdge};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_plugin::ParametricPluginAdapter;
use sotf_host::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use sotf_host::test_utils::{assert_no_allocs_or_deallocs, measure_heap_activity};
use sotf_plugin_ab_compare::{
    ABComparePlugin, ABComparePluginParams, GraphEdgeConfig, GraphNodeConfig, PathConfig,
    PluginInRack, build_path_from_config_with_factory,
};
use sotf_plugin_eq::EqPlugin;
use sotf_plugin_gain::GainPlugin;
use sotf_plugin_resampler::ResamplerPlugin;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::num::NonZeroU64;

#[global_allocator]
static AB087_ALLOCATOR: CountingAlloc = CountingAlloc;

const SAMPLE_RATE: u32 = 48_000;
const HALF_RATE: u32 = 24_000;
const DOUBLE_RATE: u32 = 96_000;
const CHANNELS: usize = 2;
/// Legacy hang guard for small-tail drains that complete far below it
/// (rings, converter tails, empty drains): exceeding it panics loudly,
/// never truncates. Tests with analytic tails (band-mask settling) use
/// `drain_all_collected_rate_bounded` with a derived budget instead.
const DRAIN_CALL_LIMIT: usize = 4096;
const BURST_CHUNK: usize = 64;
/// Nested production-resampler chunk: the facade default (`chunk_size: 1024` in
/// `sotf-plugins/src/factory/create.rs`), so nested stages run exactly as the
/// authoritative facade builds them.
const NESTED_SRC_CHUNK: usize = 1024;
/// Owned converter chunk mirror of the factory's `CONVERTER_CHUNK_FRAMES`.
/// Pinned deliberately: the bitwise references below construct the identical
/// converter, so any factory chunk change fails loudly here instead of
/// silently comparing against a differently-chunked reference.
const CONVERTER_CHUNK_PIN: usize = 256;
/// Proven band-mask residual ratio mirror of the plugin's
/// `MASK_RESIDUAL_RATIO`: program peak x 2^-24.
const MASK_RESIDUAL_RATIO: f64 = 1.0 / 16_777_216.0;

#[test]
fn fractional_outer_clock_runs_two_sink_free_paths_without_truncation() {
    let rate = 1234.5678_f64;
    let params = ABComparePluginParams {
        path_a: PathConfig::None,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        rate,
        params,
        ab087_factory,
    )
    .unwrap();
    plugin.initialize(rate).unwrap();
    let input = vec![0.25_f32; CHANNELS * 17];
    let mut output = vec![0.0_f32; input.len()];
    let frames = plugin
        .process(&input, &mut output, &ProcessContext::new(rate, 17))
        .unwrap();
    assert_eq!(frames, 17);
    assert_eq!(output, input);
}

#[test]
fn fractional_outer_clock_runs_nested_gain_path_without_rounding() {
    let rate = 12_345.678_f64;
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "gain".to_owned(),
            parameters: json!({"gain_db": 0.0, "smoothing_ms": 0.0}),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin = ABComparePlugin::from_params(CHANNELS, params).unwrap();
    plugin.initialize(rate).unwrap();
    let input = vec![0.25_f32; CHANNELS * 17];
    let mut output = vec![0.0_f32; input.len()];
    assert_eq!(
        plugin
            .process(&input, &mut output, &ProcessContext::new(rate, 17))
            .unwrap(),
        17
    );
    assert_eq!(output, input);
}

thread_local! {
    static DEC_PROCESS_CALLS: Cell<usize> = const { Cell::new(0) };
    static DEC_BEGIN_CALLS: Cell<usize> = const { Cell::new(0) };
    static DEC_DRAIN_CALLS: Cell<usize> = const { Cell::new(0) };
    static BURST_PROCESS_CALLS: Cell<usize> = const { Cell::new(0) };
    static BURST_BEGIN_CALLS: Cell<usize> = const { Cell::new(0) };
    static BURST_DRAIN_CALLS: Cell<usize> = const { Cell::new(0) };
    static CONSTRUCTION_RATES: RefCell<Vec<(String, f64)>> = const { RefCell::new(Vec::new()) };
}

fn reset_ab087_counters() {
    DEC_PROCESS_CALLS.set(0);
    DEC_BEGIN_CALLS.set(0);
    DEC_DRAIN_CALLS.set(0);
    BURST_PROCESS_CALLS.set(0);
    BURST_BEGIN_CALLS.set(0);
    BURST_DRAIN_CALLS.set(0);
}

/// Deterministic 48→24 kHz decimator model: `y[n] = (x[2n] + x[2n+1]) / 2`
/// per channel, with a one-frame carry across calls. Drain emits a carried
/// odd frame averaged with implicit zeros, then completes.
struct DecimatorTwoFixture {
    channels: usize,
    carry: Vec<f32>,
    has_carry: bool,
    last_output: usize,
    draining: bool,
}

impl DecimatorTwoFixture {
    fn new(channels: usize) -> Result<Self, String> {
        if channels == 0 {
            return Err("AB087 decimator requires channels".into());
        }
        Ok(Self {
            channels,
            carry: vec![0.0; channels],
            has_carry: false,
            last_output: 0,
            draining: false,
        })
    }

    fn carry_frames(&self) -> usize {
        usize::from(self.has_carry)
    }

    fn sample_at(&self, frame: usize, channel: usize, input: &[f32]) -> f32 {
        if self.has_carry {
            if frame == 0 {
                return self.carry[channel];
            }
            input[(frame - 1) * self.channels + channel]
        } else {
            input[frame * self.channels + channel]
        }
    }
}

impl Plugin for DecimatorTwoFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AB087 decimate-2 fixture", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AB087 decimator has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(SAMPLE_RATE) {
            return Err(format!(
                "AB087 decimator expected {SAMPLE_RATE} Hz, got {sample_rate} Hz"
            ));
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.carry.fill(0.0);
        self.has_carry = false;
        self.last_output = 0;
        self.draining = false;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        DEC_PROCESS_CALLS.with(|count| count.set(count.get() + 1));
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "AB087 decimator block size overflow".to_string())?;
        if input.len() != expected {
            return Err("AB087 decimator received invalid process geometry".into());
        }
        if self.draining {
            return Err("AB087 decimator requires reset after drain begins".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("AB087 decimator input must be finite".into());
        }
        let total = self.carry_frames() + context.num_frames;
        let out_frames = total / 2;
        if output.len() < out_frames * self.channels {
            return Err("AB087 decimator output staging too small".into());
        }
        for (frame, frame_out) in output[..out_frames * self.channels]
            .chunks_mut(self.channels)
            .enumerate()
        {
            for (channel, slot) in frame_out.iter_mut().enumerate() {
                let first = self.sample_at(2 * frame, channel, input);
                let second = self.sample_at(2 * frame + 1, channel, input);
                *slot = (first + second) * 0.5;
            }
        }
        if total % 2 == 1 {
            let last = total - 1;
            for channel in 0..self.channels {
                self.carry[channel] = self.sample_at(last, channel, input);
            }
            self.has_carry = true;
        } else {
            self.has_carry = false;
        }
        self.last_output = out_frames;
        Ok(out_frames)
    }

    fn latency_samples(&self) -> usize {
        0
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(1)
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        (self.carry_frames() + input_frames) / 2
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        // Carry-maximized (R7-F2): live `(carry + n) / 2` with carry in
        // {0, 1} undercounts arrivals at evolved carry (queried at carry 0,
        // arriving at carry 1 emits one more), so the envelope maximizes
        // the carry. Non-decreasing, dominates live in every state.
        Some(input_frames.saturating_add(1) / 2)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        false
    }

    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        input_rate / 2.0
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output)
    }

    fn drain_output_frames_max(&self) -> usize {
        1
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        DEC_BEGIN_CALLS.with(|count| count.set(count.get() + 1));
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 decimator drain requires a zero-frame 48 kHz context".into());
        }
        self.draining = true;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(1)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        DEC_DRAIN_CALLS.with(|count| count.set(count.get() + 1));
        if !self.draining || context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 decimator was not prepared for drain".into());
        }
        if output.len() != self.channels {
            return Err("AB087 decimator received the wrong drain capacity".into());
        }
        if self.has_carry {
            for (channel, sample) in output.iter_mut().enumerate() {
                *sample = self.carry[channel] * 0.5;
            }
            self.has_carry = false;
            self.last_output = 1;
            return Ok(PluginDrainResult {
                frames: 1,
                complete: true,
            });
        }
        self.last_output = 0;
        Ok(PluginDrainResult::COMPLETE)
    }
}

/// Same-clock chunking fixture: buffers input in `chunk`-frame units and
/// emits only complete chunks, preserving every sample in order. Models the
/// bursty production of compensating resampler pairs without changing clock.
/// With `report: false` the fixture stays silent (`last_output_frames` is
/// `None`); unpadded host returns keep it observable, so silent and reporting
/// twins must agree exactly on every count and every sample, process and
/// drain alike.
struct BurstFixture {
    channels: usize,
    chunk: usize,
    residual: Vec<f32>,
    last_output: usize,
    draining: bool,
    report: bool,
}

impl BurstFixture {
    fn new(channels: usize, chunk: usize, report: bool) -> Result<Self, String> {
        if channels == 0 || chunk == 0 {
            return Err("AB087 burst fixture requires channels and chunk".into());
        }
        let capacity = chunk
            .checked_mul(channels)
            .ok_or_else(|| "AB087 burst fixture capacity overflow".to_string())?;
        let mut residual = Vec::new();
        residual
            .try_reserve_exact(capacity)
            .map_err(|error| format!("AB087 burst fixture allocation failed: {error}"))?;
        Ok(Self {
            channels,
            chunk,
            residual,
            last_output: 0,
            draining: false,
            report,
        })
    }

    fn residual_frames(&self) -> usize {
        debug_assert!(self.residual.len().is_multiple_of(self.channels));
        self.residual.len() / self.channels
    }
}

impl Plugin for BurstFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AB087 burst fixture", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AB087 burst fixture has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(SAMPLE_RATE) {
            return Err(format!(
                "AB087 burst fixture expected {SAMPLE_RATE} Hz, got {sample_rate} Hz"
            ));
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.residual.clear();
        self.last_output = 0;
        self.draining = false;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        BURST_PROCESS_CALLS.with(|count| count.set(count.get() + 1));
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "AB087 burst fixture block size overflow".to_string())?;
        if input.len() != expected {
            return Err("AB087 burst fixture received invalid process geometry".into());
        }
        if self.draining {
            return Err("AB087 burst fixture requires reset after drain begins".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("AB087 burst fixture input must be finite".into());
        }
        let out_frames = (self.residual_frames() + context.num_frames) / self.chunk * self.chunk;
        if output.len() < out_frames * self.channels {
            return Err("AB087 burst fixture output staging too small".into());
        }
        let from_residual = self.residual_frames().min(out_frames);
        output[..from_residual * self.channels]
            .copy_from_slice(&self.residual[..from_residual * self.channels]);
        self.residual
            .copy_within(from_residual * self.channels.., 0);
        self.residual
            .truncate((self.residual_frames() - from_residual) * self.channels);
        let from_input = out_frames - from_residual;
        output[from_residual * self.channels..out_frames * self.channels]
            .copy_from_slice(&input[..from_input * self.channels]);
        self.residual
            .extend_from_slice(&input[from_input * self.channels..]);
        debug_assert!(self.residual_frames() < self.chunk);
        self.last_output = out_frames;
        Ok(out_frames)
    }

    fn latency_samples(&self) -> usize {
        0
    }

    fn tail_length(&self) -> TailLength {
        // Drain emits exactly the buffered residual in one call and clears
        // it, so the live residual count is the exact remainder in every
        // phase (fresh, mid-stream, post-drain zero) — stronger than the
        // old fixed chunk-1 bound, and nothing queried the old value.
        TailLength::Finite(self.residual_frames() as u64)
    }

    fn tail_support(&self) -> Option<u64> {
        // Residual always stays below one chunk (complete chunks emit).
        Some(self.chunk.saturating_sub(1) as u64)
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        (self.residual_frames() + input_frames) / self.chunk * self.chunk
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        // Residual stays in [0, chunk), so completed chunks are at most
        // (chunk - 1 + n) / chunk; non-decreasing in n as required.
        let chunks = (self.chunk.checked_sub(1)?).checked_add(input_frames)? / self.chunk;
        chunks.checked_mul(self.chunk)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        false
    }

    fn last_output_frames(&self) -> Option<usize> {
        self.report.then_some(self.last_output)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.chunk
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        // Drain emits the residual, always below one chunk.
        Some(self.chunk)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        BURST_BEGIN_CALLS.with(|count| count.set(count.get() + 1));
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 burst fixture drain requires a zero-frame 48 kHz context".into());
        }
        self.draining = true;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(1)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        BURST_DRAIN_CALLS.with(|count| count.set(count.get() + 1));
        if !self.draining || context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 burst fixture was not prepared for drain".into());
        }
        let required = self.chunk * self.channels;
        if output.len() != required {
            return Err("AB087 burst fixture received the wrong drain capacity".into());
        }
        let frames = self.residual_frames();
        output[..frames * self.channels].copy_from_slice(&self.residual);
        self.residual.clear();
        self.last_output = frames;
        Ok(PluginDrainResult {
            frames,
            complete: true,
        })
    }
}

/// Zero-latency scalar gain with declared identity geometry, for branched
/// same-rate graphs and steady-identity pairing controls.
struct ZGainFixture {
    channels: usize,
    gain: f32,
    last_output: usize,
}

impl ZGainFixture {
    fn new(channels: usize, gain: f32) -> Result<Self, String> {
        if channels == 0 || !gain.is_finite() {
            return Err("AB087 zgain fixture requires channels and finite gain".into());
        }
        Ok(Self {
            channels,
            gain,
            last_output: 0,
        })
    }
}

impl Plugin for ZGainFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AB087 zgain fixture", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AB087 zgain fixture has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(SAMPLE_RATE) {
            return Err(format!(
                "AB087 zgain fixture expected {SAMPLE_RATE} Hz, got {sample_rate} Hz"
            ));
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.last_output = 0;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "AB087 zgain fixture block size overflow".to_string())?;
        if input.len() != expected || output.len() != expected {
            return Err("AB087 zgain fixture received invalid process geometry".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("AB087 zgain fixture input must be finite".into());
        }
        for (out, sample) in output.iter_mut().zip(input.iter()) {
            *out = sample * self.gain;
        }
        self.last_output = context.num_frames;
        Ok(context.num_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        // Every success path writes exactly `num_frames`.
        Some(input_frames)
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        // Drain always completes with zero frames.
        Some(0)
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 zgain fixture drain requires a zero-frame 48 kHz context".into());
        }
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(1)
    }

    fn drain(
        &mut self,
        _output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 zgain fixture was not prepared for drain".into());
        }
        self.last_output = 0;
        Ok(PluginDrainResult::COMPLETE)
    }
}

/// Declared-capacity liar (process): under-declares its live bound by one
/// frame, writes honestly, then over-reports past staging. The host names
/// the node with the measured return-vs-declared evidence; nothing emits.
/// Reports its lie (consistent liar), proving the capacity guard — not any
/// silence gate — catches adversarial geometry.
struct ProcessLyingFixture {
    channels: usize,
    last_output: Option<usize>,
}

impl ProcessLyingFixture {
    fn new(channels: usize) -> Result<Self, String> {
        if channels == 0 {
            return Err("AB087 process-lying fixture requires channels".into());
        }
        Ok(Self {
            channels,
            last_output: None,
        })
    }
}

impl Plugin for ProcessLyingFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AB087 process-lying fixture", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AB087 process-lying fixture has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 process-lying fixture expects the outer rate".into());
        }
        Ok(())
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames.saturating_sub(1)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        false
    }

    fn last_output_frames(&self) -> Option<usize> {
        self.last_output
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != context.num_frames * self.channels {
            return Err("AB087 process-lying fixture got a short input".into());
        }
        let honest = output.len().min(input.len());
        output[..honest].copy_from_slice(&input[..honest]);
        self.last_output = Some(context.num_frames);
        Ok(context.num_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }

    fn drain_output_frames_max(&self) -> usize {
        0
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(1)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 process-lying fixture was not prepared for drain".into());
        }
        Ok(())
    }

    fn drain(
        &mut self,
        _output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 process-lying fixture was not prepared for drain".into());
        }
        self.last_output = Some(0);
        Ok(PluginDrainResult::COMPLETE)
    }
}

/// Declared-capacity liar (drain): honest in process, then over-reports its
/// drain return past its declared drain capacity. The host drain guard
/// names the violation with measured evidence; nothing emits.
struct DrainLyingFixture {
    channels: usize,
    last_output: Option<usize>,
    draining: bool,
}

impl DrainLyingFixture {
    fn new(channels: usize) -> Result<Self, String> {
        if channels == 0 {
            return Err("AB087 drain-lying fixture requires channels".into());
        }
        Ok(Self {
            channels,
            last_output: None,
            draining: false,
        })
    }
}

impl Plugin for DrainLyingFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AB087 drain-lying fixture", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AB087 drain-lying fixture has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 drain-lying fixture expects the outer rate".into());
        }
        Ok(())
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn last_output_frames(&self) -> Option<usize> {
        self.last_output
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != context.num_frames * self.channels
            || output.len() != context.num_frames * self.channels
        {
            return Err("AB087 drain-lying fixture got ragged process buffers".into());
        }
        output.copy_from_slice(input);
        self.last_output = Some(context.num_frames);
        Ok(context.num_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(1)
    }

    fn drain_output_frames_max(&self) -> usize {
        2
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(1)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 drain-lying fixture was not prepared for drain".into());
        }
        self.draining = true;
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !self.draining || context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 drain-lying fixture was not prepared for drain".into());
        }
        if output.len() != 2 * self.channels {
            return Err("AB087 drain-lying fixture got a wrong drain capacity".into());
        }
        output.fill(0.25);
        self.last_output = Some(3);
        self.draining = false;
        Ok(PluginDrainResult {
            frames: 3,
            complete: true,
        })
    }
}

/// Transparent process with a long structural tail: copies input to output
/// exactly (genuine identity geometry, zero latency) while retaining the
/// last input frames; drain re-emits the retained echo one frame per call,
/// then zero-pads to the full structural length. Models a deeply-driven
/// low-capacity child for quota-refresh proofs: the tail is exact in every
/// state, capacity stays 1, and no probe size misleads the host identity
/// cache (the geometry IS identity, honestly declared).
struct EchoTailFixture {
    channels: usize,
    history: Vec<f32>,
    draining: bool,
    emit_index: usize,
}

impl EchoTailFixture {
    /// Structural tail: drain always emits exactly this many frames. 2500
    /// content frames at 1 frame/call plus the ~13k-frame flush total near
    /// 16k calls — past the 4096 fallback and the ~5003 partial bound
    /// while assuming only F > ~2500 (5x below estimate): the partial
    /// bound double-counts pumps plus re-emission (~2x content), so the
    /// tail balances both assertions instead of maximizing either.
    /// Scales the test linearly.
    const TAIL_FRAMES: usize = 2500;

    fn new(channels: usize) -> Result<Self, String> {
        if channels == 0 {
            return Err("AB087 echo-tail fixture requires channels".into());
        }
        Ok(Self {
            channels,
            history: Vec::new(),
            draining: false,
            emit_index: 0,
        })
    }

    fn history_frames(&self) -> usize {
        debug_assert!(self.history.len().is_multiple_of(self.channels));
        self.history.len() / self.channels
    }
}

impl Plugin for EchoTailFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AB087 echo-tail fixture", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AB087 echo-tail fixture has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(SAMPLE_RATE) {
            return Err(format!(
                "AB087 echo-tail fixture expected {SAMPLE_RATE} Hz, got {sample_rate} Hz"
            ));
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.history.clear();
        self.draining = false;
        self.emit_index = 0;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "AB087 echo-tail fixture block size overflow".to_string())?;
        if input.len() != expected || output.len() < expected {
            return Err("AB087 echo-tail fixture received invalid process geometry".into());
        }
        if self.draining {
            return Err("AB087 echo-tail fixture requires reset after drain begins".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("AB087 echo-tail fixture input must be finite".into());
        }
        output[..expected].copy_from_slice(input);
        self.history.extend_from_slice(input);
        let capacity = Self::TAIL_FRAMES * self.channels;
        if self.history.len() > capacity {
            let excess = self.history.len() - capacity;
            self.history.drain(..excess);
        }
        Ok(context.num_frames)
    }

    fn latency_samples(&self) -> usize {
        0
    }

    fn tail_length(&self) -> TailLength {
        // Structural and exact: drain emits exactly TAIL_FRAMES (retained
        // echo first, then zeros) in every state, fresh or driven.
        TailLength::Finite(Self::TAIL_FRAMES as u64)
    }

    fn tail_support(&self) -> Option<u64> {
        Some(Self::TAIL_FRAMES as u64)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn drain_output_frames_max(&self) -> usize {
        1
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(1)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err(
                "AB087 echo-tail fixture drain requires a zero-frame 48 kHz context".into(),
            );
        }
        self.draining = true;
        self.emit_index = 0;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(Self::TAIL_FRAMES as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !self.draining || context.num_frames != 0 || context.sample_rate != f64::from(SAMPLE_RATE) {
            return Err("AB087 echo-tail fixture was not prepared for drain".into());
        }
        if output.len() != self.channels {
            return Err("AB087 echo-tail fixture received the wrong drain capacity".into());
        }
        if self.emit_index < self.history_frames() {
            let start = self.emit_index * self.channels;
            output.copy_from_slice(&self.history[start..start + self.channels]);
        } else {
            output.fill(0.0);
        }
        self.emit_index += 1;
        Ok(PluginDrainResult {
            frames: 1,
            complete: self.emit_index == Self::TAIL_FRAMES,
        })
    }
}

fn ab087_factory(
    plugin_type: &str,
    parameters: &Value,
    channels: usize,
    sample_rate: f64,
) -> Result<Box<dyn Plugin>, String> {
    // Part B passes topologically resolved input clocks (24 kHz mid-graph
    // nodes, 96/192 kHz upsampled stages); only a zero rate is illegitimate.
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err("AB087 factory requires a nonzero sample rate".to_owned());
    }
    match plugin_type {
        "decimate-2" => Ok(Box::new(DecimatorTwoFixture::new(channels)?)),
        "lie-process" => Ok(Box::new(ProcessLyingFixture::new(channels)?)),
        "lie-drain" => Ok(Box::new(DrainLyingFixture::new(channels)?)),
        "burst" => {
            let chunk = parameters
                .get("chunk")
                .and_then(Value::as_u64)
                .unwrap_or(BURST_CHUNK as u64) as usize;
            let report = parameters
                .get("report")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(Box::new(BurstFixture::new(channels, chunk, report)?))
        }
        "echotail" => Ok(Box::new(EchoTailFixture::new(channels)?)),
        "zgain" => {
            let gain = parameters
                .get("gain")
                .and_then(Value::as_f64)
                .unwrap_or(1.0) as f32;
            Ok(Box::new(ZGainFixture::new(channels, gain)?))
        }
        // Production resampler stage with facade parameter keys
        // (`input_sample_rate` / `output_sample_rate` / `chunk_size`, default
        // chunk 1024). Rates come from parameters — like the facade — because
        // a mid-chain stage's input rate is the running chain rate, which the
        // factory's outer `sample_rate` argument cannot know.
        "resampler" => {
            let input_rate = parameters
                .get("input_sample_rate")
                .and_then(Value::as_f64)
                .unwrap_or(sample_rate);
            let output_rate = parameters
                .get("output_sample_rate")
                .and_then(Value::as_f64)
                .unwrap_or(sample_rate);
            let chunk = parameters
                .get("chunk_size")
                .and_then(Value::as_u64)
                .unwrap_or(NESTED_SRC_CHUNK as u64) as usize;
            Ok(Box::new(ResamplerPlugin::new(
                channels,
                input_rate,
                output_rate,
                chunk,
            )?))
        }
        _ => Err(format!("AB087 factory has no plugin type {plugin_type:?}")),
    }
}

// ---------------------------------------------------------------------------
// Independent oracles (separate reference code, never the fixtures above).
// ---------------------------------------------------------------------------

/// Pairwise-average decimation over whole frames: `y[n] = (x[2n]+x[2n+1])/2`.
/// Drops a trailing odd frame; the drain oracle below accounts for it.
fn decimate_process_reference(input: &[f32], channels: usize) -> Vec<f32> {
    assert!(input.len().is_multiple_of(channels));
    let input_frames = input.len() / channels;
    let mut output = Vec::with_capacity(input_frames / 2 * channels);
    for frame in 0..input_frames / 2 {
        for channel in 0..channels {
            let first = f64::from(input[(2 * frame) * channels + channel]);
            let second = f64::from(input[(2 * frame + 1) * channels + channel]);
            output.push(((first + second) * 0.5) as f32);
        }
    }
    output
}

/// Complete decimator stream: process pairs plus the averaged odd carry.
/// Exact rational behavior; compared with `assert_eq` (NaN fails, -0.0 tolerant).
fn decimate_whole_stream_reference(input: &[f32], channels: usize) -> Vec<f32> {
    let mut whole = decimate_process_reference(input, channels);
    let input_frames = input.len() / channels;
    if input_frames % 2 == 1 {
        for channel in 0..channels {
            let carried = f64::from(input[(input_frames - 1) * channels + channel]);
            whole.push((carried * 0.5) as f32);
        }
    }
    whole
}

/// Diamond graph reference: `S -> {A, B} -> T` with host summation at the
/// join, all zero-latency so no compensation applies. f64 accumulation with
/// the AUD137 1e-6 per-sample bound.
fn diamond_reference(
    input: &[f32],
    source_gain: f64,
    branch_a_gain: f64,
    branch_b_gain: f64,
    sink_gain: f64,
) -> Vec<f32> {
    input
        .iter()
        .map(|sample| {
            let staged = f64::from(*sample) * source_gain;
            let joined = staged * branch_a_gain + staged * branch_b_gain;
            (joined * sink_gain) as f32
        })
        .collect()
}

/// Drive one raw stage (production resampler or decimator fixture) through a
/// single whole-input process call plus full EOF, collecting actual emitted
/// frames via the diligent `last_output_frames` contract. Chaining this
/// helper stage by stage builds the conversion-content reference that nested
/// ABCompare paths must reproduce as an exact prefix (ahead of their own
/// exact-zero latency-ring flush) under any callback partitioning (stages
/// chunk stream-positionally, so references are partition-invariant by
/// construction).
fn run_raw_plugin_to_end(plugin: &mut dyn Plugin, input_rate: u32, input: &[f32]) -> Vec<f32> {
    assert!(input.len().is_multiple_of(CHANNELS));
    plugin.initialize(f64::from(input_rate)).unwrap();
    let input_frames = input.len() / CHANNELS;
    let capacity = plugin.output_frames_for_input(input_frames);
    let mut block = vec![f32::NAN; capacity * CHANNELS];
    let returned = plugin
        .process(
            input,
            &mut block,
            &ProcessContext::new(input_rate, input_frames),
        )
        .unwrap();
    let actual = plugin.last_output_frames().unwrap_or(returned);
    assert!(
        actual <= capacity,
        "reference stage overproduced its bound: {actual} > {capacity}"
    );
    let mut output = block[..actual * CHANNELS].to_vec();
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "reference stage must stay finite"
    );
    plugin
        .begin_drain(&ProcessContext::new(input_rate, 0))
        .unwrap();
    let drain_capacity = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; drain_capacity * CHANNELS];
    for _ in 0..DRAIN_CALL_LIMIT {
        drain_block.fill(f32::NAN);
        let result = plugin
            .drain(&mut drain_block, &ProcessContext::new(input_rate, 0))
            .unwrap();
        assert!(result.frames <= drain_capacity);
        output.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
        if result.complete {
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "reference tail must stay finite"
            );
            return output;
        }
    }
    panic!("AB087 reference drain exceeded the test's bounded call allowance");
}

/// Stateful fresh-`Biquad` HP->LP cascade, mirroring the production mixer's
/// per-sample operations exactly (f64 stage chain, `as f32` per sample).
/// State carries across `feed` calls (retune/reactivation oracles); a fresh
/// oracle starts zeroed like the plugin's post-initialize state.
struct MaskOracle {
    highpass: Vec<Biquad>,
    lowpass: Vec<Biquad>,
    channels: usize,
}

impl MaskOracle {
    fn new(channels: usize, low_hz: f64, high_hz: f64, sample_rate: f64) -> Self {
        let q = 1.0 / std::f64::consts::SQRT_2;
        let highpass = (0..channels)
            .map(|_| Biquad::new(BiquadFilterType::Highpass, low_hz, sample_rate, q, 0.0))
            .collect();
        let lowpass = (0..channels)
            .map(|_| Biquad::new(BiquadFilterType::Lowpass, high_hz, sample_rate, q, 0.0))
            .collect();
        Self {
            highpass,
            lowpass,
            channels,
        }
    }

    fn feed(&mut self, wet: &[f32]) -> Vec<f32> {
        assert!(wet.len().is_multiple_of(self.channels));
        let mut output = Vec::with_capacity(wet.len());
        for frame in wet.chunks_exact(self.channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                let hp_out = self.highpass[channel].process(f64::from(sample));
                output.push(self.lowpass[channel].process(hp_out) as f32);
            }
        }
        output
    }
}

/// Fresh-`Biquad` HP->LP cascade over a known wet sequence: identical wet
/// input reproduces plugin output bit-for-bit.
fn mask_cascade_reference(
    wet: &[f32],
    channels: usize,
    low_hz: f64,
    high_hz: f64,
    sample_rate: f64,
) -> Vec<f32> {
    MaskOracle::new(channels, low_hz, high_hz, sample_rate).feed(wet)
}

/// Hand-rolled RBJ cookbook biquad (Direct Form I): independent of the
/// `math-audio` implementation, cross-checking the mask cascade design.
struct RbjBiquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl RbjBiquad {
    fn new(highpass: bool, freq: f64, rate: f64) -> Self {
        let q = 1.0 / std::f64::consts::SQRT_2;
        let omega = 2.0 * std::f64::consts::PI * freq / rate;
        let (sn, cs) = omega.sin_cos();
        let alpha = sn / (2.0 * q);
        let (b0, b1, b2) = if highpass {
            ((1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0)
        } else {
            ((1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0)
        };
        let a0 = 1.0 + alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cs / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Independent cookbook HP->LP cascade over a known wet sequence (compare at
/// the AUD137 1e-6 bound: coefficient rounding differs from `math-audio` in
/// the last ulp, while the `Biquad` oracle above is bit-exact).
fn mask_cascade_rbj_reference(
    wet: &[f32],
    channels: usize,
    low_hz: f64,
    high_hz: f64,
    sample_rate: f64,
) -> Vec<f32> {
    assert!(wet.len().is_multiple_of(channels));
    let mut highpass: Vec<RbjBiquad> = (0..channels)
        .map(|_| RbjBiquad::new(true, low_hz, sample_rate))
        .collect();
    let mut lowpass: Vec<RbjBiquad> = (0..channels)
        .map(|_| RbjBiquad::new(false, high_hz, sample_rate))
        .collect();
    let mut output = Vec::with_capacity(wet.len());
    for frame in wet.chunks_exact(channels) {
        for (channel, &sample) in frame.iter().enumerate() {
            let hp_out = highpass[channel].process(f64::from(sample));
            output.push(lowpass[channel].process(hp_out) as f32);
        }
    }
    output
}

/// Maximum absolute cascade output over `extra_frames` zero-fed frames after
/// `wet`: the soundness probe for the proven flush. The plugin's emitted
/// flush must leave a remainder below `wet_peak x MASK_RESIDUAL_RATIO`, so
/// extending the oracle far beyond the observed flush must stay under it.
fn mask_tail_max_beyond(
    wet: &[f32],
    channels: usize,
    low_hz: f64,
    high_hz: f64,
    sample_rate: f64,
    extra_frames: usize,
) -> f64 {
    let q = 1.0 / std::f64::consts::SQRT_2;
    let mut highpass: Vec<Biquad> = (0..channels)
        .map(|_| Biquad::new(BiquadFilterType::Highpass, low_hz, sample_rate, q, 0.0))
        .collect();
    let mut lowpass: Vec<Biquad> = (0..channels)
        .map(|_| Biquad::new(BiquadFilterType::Lowpass, high_hz, sample_rate, q, 0.0))
        .collect();
    for frame in wet.chunks_exact(channels) {
        for (channel, &sample) in frame.iter().enumerate() {
            let hp_out = highpass[channel].process(f64::from(sample));
            let _ = lowpass[channel].process(hp_out);
        }
    }
    let mut peak = 0.0_f64;
    for _ in 0..extra_frames {
        for (highpass, lowpass) in highpass.iter_mut().zip(lowpass.iter_mut()) {
            let hp_out = highpass.process(0.0);
            let tail = lowpass.process(hp_out);
            peak = peak.max(tail.abs());
        }
    }
    peak
}

fn dense_input(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|index| {
            let frame = index / CHANNELS;
            let channel = index % CHANNELS;
            let value = ((frame * (7 + 4 * channel) + 3 * channel) % 23) as f32 - 11.0;
            value / 16.0
        })
        .collect()
}

fn constant_input(frames: usize, value: f32) -> Vec<f32> {
    vec![value; frames * CHANNELS]
}

fn impulse_input(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * CHANNELS];
    input[0] = 1.0;
    input
}

/// Render through `process`, honoring each call's actual returned count.
/// Asserts the untouched output suffix stays NaN (honest variable output).
fn render_collected(plugin: &mut ABComparePlugin, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    assert_eq!(chunks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = Vec::new();
    let mut frame_offset = 0;
    for &frames in chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let produced = plugin
            .process(&input[start..end], &mut block, &context)
            .unwrap();
        assert!(
            produced <= frames,
            "plugin emitted {produced} frames for a {frames}-frame block"
        );
        output.extend_from_slice(&block[..produced * CHANNELS]);
        assert!(
            block[produced * CHANNELS..]
                .iter()
                .all(|sample| sample.is_nan()),
            "output beyond the returned {produced} frames must stay untouched"
        );
        frame_offset += frames;
    }
    output
}

/// Render through `process` like `render_collected`, additionally capturing
/// each call's returned count for per-call count oracles (every return at or
/// below its block bound; process plus drain returns conserve the whole).
fn render_collected_captured(
    plugin: &mut ABComparePlugin,
    input: &[f32],
    chunks: &[usize],
) -> (Vec<f32>, Vec<usize>) {
    assert_eq!(chunks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = Vec::new();
    let mut returns = Vec::with_capacity(chunks.len());
    let mut frame_offset = 0;
    for &frames in chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let produced = plugin
            .process(&input[start..end], &mut block, &context)
            .unwrap();
        assert!(
            produced <= frames,
            "plugin emitted {produced} frames for a {frames}-frame block"
        );
        returns.push(produced);
        output.extend_from_slice(&block[..produced * CHANNELS]);
        assert!(
            block[produced * CHANNELS..]
                .iter()
                .all(|sample| sample.is_nan()),
            "output beyond the returned {produced} frames must stay untouched"
        );
        frame_offset += frames;
    }
    (output, returns)
}

fn drain_all_collected(plugin: &mut ABComparePlugin) -> (Vec<f32>, Vec<usize>) {
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();
    let capacity_frames = plugin.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut output = Vec::new();
    let mut partition = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, &context).unwrap();
        assert!(result.frames <= capacity_frames);
        partition.push(result.frames);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return (output, partition);
        }
    }
    panic!("AB087 drain exceeded the test's bounded call allowance");
}

/// Drain with a derived budget instead of the legacy hang guard: calls may
/// not exceed twice the frames emitted plus the completion handshake (rate
/// liveness — sustained progress of at least one frame per two calls, so
/// every-other-call alignment pauses pass but stalls fail fast), with an
/// absolute backstop of the test's own probe horizon (`stream_frames` plus
/// `extra_horizon_frames` at one frame per call plus the handshake).
/// Completion is the `complete` flag only; exceeding either bound panics
/// (production no-progress), never truncates.
fn drain_all_collected_rate_bounded(
    plugin: &mut ABComparePlugin,
    stream_frames: usize,
    extra_horizon_frames: usize,
) -> (Vec<f32>, Vec<usize>) {
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();
    let capacity_frames = plugin.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut output = Vec::new();
    let mut partition = Vec::new();
    let backstop = stream_frames
        .checked_add(extra_horizon_frames)
        .and_then(|total| total.checked_add(2))
        .expect("drain probe backstop fits address space");
    let mut calls = 0;
    loop {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, &context).unwrap();
        assert!(result.frames <= capacity_frames);
        partition.push(result.frames);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        calls += 1;
        if result.complete {
            println!(
                "rate-bounded drain: {calls} calls, {} frames",
                output.len() / CHANNELS,
            );
            return (output, partition);
        }
        let emitted = output.len() / CHANNELS;
        let rate_bound = emitted
            .checked_mul(2)
            .and_then(|doubled| doubled.checked_add(2))
            .expect("drain rate bound fits address space");
        assert!(
            calls <= rate_bound,
            "drain made no progress: {calls} calls for {emitted} frames"
        );
        assert!(
            calls < backstop,
            "drain exceeded its probe backstop ({backstop} calls for {stream_frames}+{extra_horizon_frames} horizon frames)"
        );
    }
}

fn assert_vectors_close(actual: &[f32], expected: &[f32], name: &str) {
    assert_eq!(actual.len(), expected.len(), "{name} full-stream length");
    for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "{name} sample {index}: expected {expected}, got {actual}"
        );
    }
}

/// Latency-compensated whole-stream assertion for converted paths: the
/// emitted stream is the conversion content (`expected`) followed by
/// exact-zero ring-flush frames. Rings always flush fully — even at zero mix
/// gain, whose gain could reactivate mid-drain — so total length is input
/// frames plus reported latency, and the suffix is exactly +0.0.
fn assert_compensated_whole_stream(
    whole: &[f32],
    expected: &[f32],
    input_frames: usize,
    latency: usize,
    name: &str,
) {
    assert_eq!(
        whole.len() / CHANNELS,
        input_frames + latency,
        "{name} compensated length"
    );
    assert!(
        whole.len() >= expected.len(),
        "{name}: compensated stream must cover the conversion content"
    );
    assert_eq!(&whole[..expected.len()], expected, "{name} content prefix");
    assert!(
        whole[expected.len()..].iter().all(|sample| *sample == 0.0),
        "{name} ring-flush suffix is exact zeros"
    );
}

fn burst_params(chunk: usize) -> Value {
    json!({ "chunk": chunk })
}

fn burst_params_silent(chunk: usize) -> Value {
    json!({ "chunk": chunk, "report": false })
}

fn resampler_params(input_rate: u32, output_rate: u32, chunk: usize) -> Value {
    json!({
        "input_sample_rate": input_rate,
        "output_sample_rate": output_rate,
        "chunk_size": chunk,
    })
}

fn make_silent_burst_plugin() -> ABComparePlugin {
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params_silent(BURST_CHUNK),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin
}

fn make_burst_plugin(mix: f32) -> ABComparePlugin {
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params(BURST_CHUNK),
        },
        path_b: PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params(BURST_CHUNK),
        },
        mix,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin
}

fn make_reporting_burst_plugin() -> ABComparePlugin {
    // Structural twin of make_silent_burst_plugin: reporting burst on A,
    // None on B. The shared make_burst_plugin pairs burst with burst,
    // which retains nothing on B at drain and would complete one pump
    // earlier for structural reasons unrelated to silence.
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params(BURST_CHUNK),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin
}

#[test]
fn mixed_rate_chain_composes_back_to_outer_clock() {
    // Supersedes the R1 same-rate refusal endpoint: a 48→24 kHz chain path
    // now gains the ABCompare-owned production converter back to 48 kHz and
    // must reproduce the raw stage-by-stage reference as an exact content
    // prefix (plus exact-zero latency-ring flush) under irregular
    // partitioning, with full EOF and honest counts throughout.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 1669, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 2048);
    for (name, input) in [
        ("constant", constant_input(2048, 0.25)),
        ("impulse", impulse_input(2048)),
        ("dense", dense_input(2048)),
    ] {
        // Reference: raw decimator fixture, then the raw production
        // converter with the pinned chunk, each single-block plus full EOF.
        let mut decimator = DecimatorTwoFixture::new(CHANNELS).unwrap();
        let mid = run_raw_plugin_to_end(&mut decimator, SAMPLE_RATE, &input);
        assert_eq!(mid.len() / CHANNELS, 1024, "{name} decimated length");
        let mut converter =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let expected = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "decimate-2".to_owned(),
                parameters: Value::Null,
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: converter latency must be observed"
        );
        let before = DEC_PROCESS_CALLS.get();
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert!(
            DEC_PROCESS_CALLS.get() > before,
            "{name}: the decimator child must actually advance"
        );
        assert_compensated_whole_stream(&whole, &expected, 2048, plugin.latency_samples(), name);
        if name == "constant" {
            // Converter interior of exact-0.25 DC: the middle half avoids
            // both the startup transient and the flush tail.
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }
}

fn graph_node(id: &str, plugin_type: &str, parameters: Value) -> GraphNodeConfig {
    GraphNodeConfig {
        id: id.to_owned(),
        plugin_type: plugin_type.to_owned(),
        parameters,
    }
}

fn graph_edge(from: &str, to: &str) -> GraphEdgeConfig {
    GraphEdgeConfig {
        from: from.to_owned(),
        to: to.to_owned(),
        channel_map: None,
        destination_offset: 0,
    }
}

#[test]
fn graph_48_to_24_composes_back_to_outer_clock() {
    // D5 supersedes the R3 graph refusal endpoint: a single-node 48→24 kHz
    // graph path gains the ABCompare-owned production converter back to
    // 48 kHz and must reproduce the raw stage-by-stage reference as an exact
    // content prefix (plus exact-zero latency-ring flush) under irregular
    // partitioning, with full EOF and honest counts throughout.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let expected = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Graph {
                nodes: vec![graph_node(
                    "down",
                    "resampler",
                    resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                )],
                edges: Vec::new(),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: nested plus converter latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }
}

#[test]
fn graph_48_to_96_composes_back_to_outer_clock() {
    // D5, upsampling direction: a single-node 48→96 kHz graph path plus the
    // owned 96→48 converter, content-prefix bit-exact against the raw
    // reference (plus exact-zero latency-ring flush).
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let expected = run_raw_plugin_to_end(&mut converter, DOUBLE_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Graph {
                nodes: vec![graph_node(
                    "up",
                    "resampler",
                    resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                )],
                edges: Vec::new(),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: nested plus converter latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }
}

#[test]
fn graph_48_to_24_to_48_composes_mid_graph() {
    // D5 mid-graph composition: a 48→24→48 production pair inside one graph
    // already ends at the outer clock, so no converter is appended. The
    // downstream 24 kHz stage initializes at its resolved input clock through
    // the explicit-rate entry point; content-prefix bit-exactness against the
    // two-stage raw reference proves exactly two stages ran.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut down =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut down, SAMPLE_RATE, &input);
        let mut up =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let expected = run_raw_plugin_to_end(&mut up, HALF_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Graph {
                nodes: vec![
                    graph_node(
                        "down",
                        "resampler",
                        resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                    ),
                    graph_node(
                        "up",
                        "resampler",
                        resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
                    ),
                ],
                edges: vec![graph_edge("down", "up")],
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: mid-graph pair latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }
}

#[test]
fn graph_48_to_96_to_48_composes_mid_graph() {
    // D5 mid-graph composition, upsampling first: a 48→96→48 production pair
    // inside one graph ends at the outer clock with no appended converter.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut up =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut up, SAMPLE_RATE, &input);
        let mut down =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let expected = run_raw_plugin_to_end(&mut down, DOUBLE_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Graph {
                nodes: vec![
                    graph_node(
                        "up",
                        "resampler",
                        resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                    ),
                    graph_node(
                        "down",
                        "resampler",
                        resampler_params(DOUBLE_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
                    ),
                ],
                edges: vec![graph_edge("up", "down")],
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: mid-graph pair latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }
}

#[test]
fn graph_path_host_tracks_rate_eof_and_repeated_build() {
    // D5 construction-level proof on the factory-built path host itself: the
    // composed graph reports the outer clock, tracks actual production every
    // block, survives a mid-stream rebuild, and drains to the raw two-stage
    // reference bitwise with no latency-ring layer in between.
    reset_ab087_counters();
    let config = PathConfig::Graph {
        nodes: vec![graph_node(
            "down",
            "resampler",
            resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
        )],
        edges: Vec::new(),
    };
    let mut host =
        build_path_from_config_with_factory(&config, CHANNELS, SAMPLE_RATE, Some(ab087_factory))
            .unwrap();
    assert_eq!(host.output_sample_rate(SAMPLE_RATE).unwrap(), f64::from(SAMPLE_RATE));
    assert!(
        host.total_latency_samples() > 0,
        "nested plus converter latency must be observed"
    );

    let input = dense_input(4096);
    let mut nested =
        ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
    let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
    let mut converter =
        ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
    let expected = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);

    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    let mut whole = Vec::new();
    let mut frame_offset = 0;
    let mut saw_production = false;
    for (block_index, &frames) in chunks.iter().enumerate() {
        if block_index == 4 {
            // Repeated mid-stream rebuild: the composed clock and negotiated
            // state must be untouched.
            host.build().unwrap();
            assert_eq!(host.output_sample_rate(SAMPLE_RATE).unwrap(), f64::from(SAMPLE_RATE));
        }
        let capacity = host.output_frames_for_input(frames);
        let mut block = vec![f32::NAN; capacity * CHANNELS];
        host.process(
            &input[frame_offset * CHANNELS..(frame_offset + frames) * CHANNELS],
            &mut block,
        )
        .unwrap();
        let actual = host
            .last_output_frames()
            .expect("composed graph must track actual production");
        assert!(actual <= capacity, "host overproduced its bound");
        saw_production |= actual > 0;
        whole.extend_from_slice(&block[..actual * CHANNELS]);
        frame_offset += frames;
    }
    assert!(saw_production, "the composed graph must actually produce");

    let drain_capacity = host.drain_output_frames_max();
    let mut block = vec![f32::NAN; drain_capacity.max(1) * CHANNELS];
    let mut drain_frames = 0;
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = host.drain(&mut block).unwrap();
        whole.extend_from_slice(&block[..result.frames * CHANNELS]);
        drain_frames += result.frames;
        if result.complete {
            break;
        }
    }
    assert_eq!(
        whole.len(),
        expected.len(),
        "host-level whole-stream length must match the raw reference"
    );
    assert_eq!(
        whole, expected,
        "host-level whole stream must match the raw reference bitwise"
    );
    println!(
        "graph 48→24→48 host: {} process frames, {} drain frames, latency {}",
        whole.len() / CHANNELS - drain_frames,
        drain_frames,
        host.total_latency_samples(),
    );
}

#[test]
fn graph_twin_multi_sink_converts_back_to_outer_clock() {
    // Twin multi-sink through the factory join: identical sinks gain one
    // owned converter each, then feed the transparent output join; the
    // merge sums the twin-identical per-call counts with no retention
    // buildup. The whole stream is exactly twice the single-sink reference
    // (one f32 add per sample commutes, so the sum is bitwise regardless
    // of edge order), plus the exact-zero latency-ring flush.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        // Single-sink reference: raw nested stage, then the raw production
        // converter with the pinned chunk, each single-block plus full EOF.
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let nested_latency = nested.latency_samples();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let converter_latency = converter.latency_samples();
        let single = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);
        // Twin sum oracle: (0 + s) + s per sample, matching the host's
        // zero-fill plus sequential per-output accumulation.
        let expected: Vec<f32> = single.iter().map(|sample| sample + sample).collect();

        let params = ABComparePluginParams {
            path_a: PathConfig::Graph {
                nodes: vec![
                    graph_node(
                        "down-a",
                        "resampler",
                        resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                    ),
                    graph_node(
                        "down-b",
                        "resampler",
                        resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                    ),
                ],
                edges: Vec::new(),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        // Exact latency pin: twin branches share one path latency, derived
        // from the raw stage latencies on the exact 48 kHz lattice (nested
        // latency at 24 kHz doubles, converter latency adds directly).
        let derived_latency = 2 * nested_latency + converter_latency;
        assert_eq!(
            plugin.latency_samples(),
            derived_latency,
            "{name}: twin plus converter latency must match the derived value",
        );
        let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
        assert!(
            returns.iter().any(|&produced| produced > 0),
            "{name}: the twin path must actually produce",
        );
        let (tail, partition) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        // Count conservation: process plus drain returns cover the whole
        // stream exactly (no drop, no duplication).
        assert_eq!(
            returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
            whole.len() / CHANNELS,
            "{name} count conservation",
        );
        assert_compensated_whole_stream(&whole, &expected, 4096, derived_latency, name);
        println!(
            "twin multi-sink {name}: latency {derived_latency}, drain partition {partition:?}",
        );
        // Post-complete drain stays complete with zero frames.
        let context = ProcessContext::new(SAMPLE_RATE, 0);
        let terminal = plugin.drain(&mut [], &context).unwrap();
        assert!(
            terminal.complete && terminal.frames == 0,
            "{name}: post-complete drain must stay complete with zero frames",
        );
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(
                middle.iter().all(|sample| (sample - 0.5).abs() <= 2.0e-3),
                "twin constant interior must hold doubled DC",
            );
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.1, "impulse energy must survive both twins");
        }
    }
}

#[test]
fn graph_mixed_clock_hetero_sinks_compose_through_join() {
    // Stage B hetero composition (R20 disposition): a 48→24 leg and a
    // 48→96 leg gain one owned converter each, then feed the transparent
    // output join; the host's fan-in merge (per-edge retention plus
    // minimum-depth summation) composes the divergent per-call production
    // losslessly. The oracle sums the raw per-branch references with the
    // merge compensation delay (frame difference of the derived branch
    // latencies); warm retention allocates nothing.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        // Raw per-branch references with exact derived branch latencies.
        let mut down =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let down_latency = down.latency_samples();
        let mid_a = run_raw_plugin_to_end(&mut down, SAMPLE_RATE, &input);
        let mut conv_a =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let conv_a_latency = conv_a.latency_samples();
        let ref_a = run_raw_plugin_to_end(&mut conv_a, HALF_RATE, &mid_a);
        let latency_a = 2 * down_latency + conv_a_latency;

        let mut up =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let up_latency = up.latency_samples();
        let mid_b = run_raw_plugin_to_end(&mut up, SAMPLE_RATE, &input);
        let mut conv_b =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let conv_b_latency = conv_b.latency_samples();
        let ref_b = run_raw_plugin_to_end(&mut conv_b, DOUBLE_RATE, &mid_b);
        let latency_b = (up_latency + 2 * conv_b_latency).div_ceil(2);

        // Hetero guard: identical latencies would make the compensation
        // oracle vacuous.
        assert_ne!(
            latency_a, latency_b,
            "hetero legs must have unequal latencies",
        );
        let max_latency = latency_a.max(latency_b);
        let (comp_a, comp_b) = (max_latency - latency_a, max_latency - latency_b);

        let params = ABComparePluginParams {
            path_a: PathConfig::Graph {
                nodes: vec![
                    graph_node(
                        "down",
                        "resampler",
                        resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                    ),
                    graph_node(
                        "up",
                        "resampler",
                        resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                    ),
                ],
                edges: Vec::new(),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert_eq!(
            plugin.latency_samples(),
            max_latency,
            "{name}: joined latency must be the maximum branch latency",
        );
        let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
        assert!(
            returns.iter().any(|&produced| produced > 0),
            "{name}: the joined path must actually produce",
        );
        let (tail, partition) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_eq!(
            returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
            whole.len() / CHANNELS,
            "{name} count conservation",
        );

        // Delay-compensated sum oracle: each branch reference shifts by its
        // merge compensation and zero-extends, summed in config edge order
        // (one f32 add per sample, bitwise regardless of order).
        let total_frames = 4096 + max_latency;
        assert_eq!(whole.len() / CHANNELS, total_frames, "{name} joined length",);
        let frames_a = ref_a.len() / CHANNELS;
        let frames_b = ref_b.len() / CHANNELS;
        let mut expected = Vec::with_capacity(total_frames * CHANNELS);
        for frame in 0..total_frames {
            for channel in 0..CHANNELS {
                let sample_a = if frame >= comp_a && frame - comp_a < frames_a {
                    ref_a[(frame - comp_a) * CHANNELS + channel]
                } else {
                    0.0
                };
                let sample_b = if frame >= comp_b && frame - comp_b < frames_b {
                    ref_b[(frame - comp_b) * CHANNELS + channel]
                } else {
                    0.0
                };
                expected.push(sample_a + sample_b);
            }
        }
        assert_eq!(whole, expected, "{name} joined content must match bitwise");
        println!(
            "mixed-clock hetero {name}: latencies {latency_a}/{latency_b}, drain partition {partition:?}",
        );
        let context = ProcessContext::new(SAMPLE_RATE, 0);
        let terminal = plugin.drain(&mut [], &context).unwrap();
        assert!(
            terminal.complete && terminal.frames == 0,
            "{name}: post-complete drain must stay complete with zero frames",
        );
        if name == "constant" {
            // Each leg passes DC near unity, so the summed interior holds
            // 0.5: a dead leg would read ~0.25 instead.
            let start_frame = max_latency + 512;
            let end_frame = 4096_usize.saturating_sub(512);
            assert!(
                start_frame < end_frame,
                "constant interior needs headroom past latency {max_latency}",
            );
            let middle = &whole[start_frame * CHANNELS..end_frame * CHANNELS];
            assert!(
                middle.iter().all(|sample| (sample - 0.5).abs() <= 2.0e-3),
                "summed constant interior must hold 0.5 from both legs",
            );
        }
        if name == "impulse" {
            let peak = whole
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the joined legs");
        }
    }

    // Warm retention allocates nothing: merge queues are prepared-scale
    // pre-sized, so a post-reset process/drain cycle stays silent.
    let params = ABComparePluginParams {
        path_a: PathConfig::Graph {
            nodes: vec![
                graph_node(
                    "down",
                    "resampler",
                    resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                ),
                graph_node(
                    "up",
                    "resampler",
                    resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                ),
            ],
            edges: Vec::new(),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = dense_input(4096);
    let (mut whole, _) = render_collected_captured(&mut plugin, &input, &chunks);
    let (tail, _) = drain_all_collected(&mut plugin);
    whole.extend_from_slice(&tail);
    plugin.reset();
    let mut block = vec![f32::NAN; 4096 * CHANNELS];
    let mut frame_offset = 0;
    for &frames in &chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let mut produced = 0;
        assert_no_allocs_or_deallocs("warm hetero process", || {
            produced = plugin
                .process(
                    &input[start..end],
                    &mut block[..frames * CHANNELS],
                    &context,
                )
                .unwrap();
        });
        assert!(produced <= frames);
        frame_offset += frames;
    }
    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    assert_no_allocs_or_deallocs("warm hetero begin drain", || {
        plugin.begin_drain(&drain_context).unwrap();
    });
    let live_bound = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; live_bound * CHANNELS];
    let mut warm_complete = false;
    for _ in 0..DRAIN_CALL_LIMIT {
        let mut result = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("warm hetero drain", || {
            drain_block.fill(f32::NAN);
            result = plugin.drain(&mut drain_block, &drain_context).unwrap();
        });
        assert!(result.frames <= live_bound);
        if result.complete {
            warm_complete = true;
            break;
        }
    }
    assert!(
        warm_complete,
        "warm hetero drain must complete within its call bound",
    );
}

#[test]
fn cold_converting_drain_allocates_nothing_and_completes_exactly() {
    // First-drain (cold) coverage on a converting path: the warm legs
    // above prove steady-state silence, but the first drain touches cold
    // EOS, retention, and converter state. Fresh mixed-clock hetero join,
    // one unmeasured render, then measured begin/drain: silence, exact
    // completion, conservation, and a nonempty tail (no vacuous pass).
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    let input = dense_input(4096);
    let params = ABComparePluginParams {
        path_a: PathConfig::Graph {
            nodes: vec![
                graph_node(
                    "down",
                    "resampler",
                    resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                ),
                graph_node(
                    "up",
                    "resampler",
                    resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                ),
            ],
            edges: Vec::new(),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    assert_no_allocs_or_deallocs("cold hetero begin drain", || {
        plugin.begin_drain(&drain_context).unwrap();
    });
    let live_bound = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; live_bound * CHANNELS];
    let mut partition = Vec::new();
    let mut cold_complete = false;
    for _ in 0..DRAIN_CALL_LIMIT {
        let mut result = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("cold hetero drain", || {
            drain_block.fill(f32::NAN);
            result = plugin.drain(&mut drain_block, &drain_context).unwrap();
        });
        assert!(result.frames <= live_bound);
        partition.push(result.frames);
        whole.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
        if result.complete {
            cold_complete = true;
            break;
        }
    }
    assert!(
        cold_complete,
        "cold hetero drain must complete within its call bound"
    );
    assert!(
        partition.iter().sum::<usize>() > 0,
        "cold hetero drain must emit its converting tail"
    );
    assert_eq!(
        returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
        whole.len() / CHANNELS,
        "cold drain conservation"
    );
    println!("cold hetero drain: partition {partition:?}");
    let terminal = plugin.drain(&mut [], &drain_context).unwrap();
    assert!(
        terminal.complete && terminal.frames == 0,
        "post-complete drain must stay complete with zero frames"
    );
}

#[test]
fn graph_same_clock_hetero_sinks_compose_through_join() {
    // Same-clock hetero composition: burst-64 and burst-128 branches feed
    // the transparent join; the merge retains the longer branch's surplus
    // across call boundaries instead of dropping it, and AB emits the
    // paced minimum per call. The [0, 64, ...] lead proves the join
    // gates on min-depth (call 1) while the block cap paces call 2;
    // call 4's full block with B producing nothing proves retained
    // backlog covers the gap — while content doubles the input
    // bit-exactly.
    reset_ab087_counters();
    let chunks = [64_usize, 64, 128, 64, 200, 7, 1];
    assert_eq!(chunks.iter().sum::<usize>(), 528);
    let input = dense_input(528);
    let params = ABComparePluginParams {
        path_a: PathConfig::Graph {
            nodes: vec![
                graph_node("burst-a", "burst", burst_params(BURST_CHUNK)),
                graph_node("burst-b", "burst", burst_params(2 * BURST_CHUNK)),
            ],
            edges: Vec::new(),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(plugin.latency_samples(), 0);
    let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
    // Two-layer oracle. Host layer: the merge consumes the full minimum
    // queued depth every call, so the join produces the
    // minimum-cumulative increments (walk both chunk residuals). Parent
    // layer: AB stages the residual-aware live bound — need covers the
    // backlog, so the full join production lands in the process queue —
    // but emits min(queuedA, queuedB, queuedDry, num_frames) per call.
    // The None path and dry pace the input, so each call emits
    // min(join_backlog, paced_backlog, block): call 1 emits 0 (B
    // starves the join — no fabrication), call 4 emits a full block
    // while B produces nothing (retained backlog covers the gap — the
    // retention proof), and calls 2/5 hold 64/56 frames back under the
    // per-call cap for later blocks. Emitting the raw 128-frame join
    // backlog on a 64-frame call would overrun the caller's staging.
    let mut expected_returns = Vec::with_capacity(chunks.len());
    let (mut res_a, mut res_b, mut tot_a, mut tot_b, mut emitted) = (0, 0, 0, 0, 0);
    let mut cum_input = 0;
    for &frames in &chunks {
        let prod_a = (res_a + frames) / BURST_CHUNK * BURST_CHUNK;
        res_a = (res_a + frames) % BURST_CHUNK;
        let prod_b = (res_b + frames) / (2 * BURST_CHUNK) * (2 * BURST_CHUNK);
        res_b = (res_b + frames) % (2 * BURST_CHUNK);
        tot_a += prod_a;
        tot_b += prod_b;
        cum_input += frames;
        let join_backlog = tot_a.min(tot_b) - emitted;
        let paced_backlog = cum_input - emitted;
        let emit = join_backlog.min(paced_backlog).min(frames);
        emitted += emit;
        expected_returns.push(emit);
    }
    assert_eq!(
        returns, expected_returns,
        "AB must emit the paced minimum of the joined backlog",
    );
    assert_eq!(
        returns,
        [0_usize, 64, 128, 64, 200, 7, 1],
        "retention shape: starvation gates call 1, the block cap holds calls 2/5, backlog covers call 4",
    );
    let (tail, partition) = drain_all_collected(&mut plugin);
    whole.extend_from_slice(&tail);
    assert_eq!(
        returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
        whole.len() / CHANNELS,
        "count conservation",
    );
    let doubled: Vec<f32> = input.iter().map(|sample| sample + sample).collect();
    assert_eq!(whole, doubled, "hetero content must double the input");
    println!("same-clock hetero: drain partition {partition:?}");
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    let terminal = plugin.drain(&mut [], &context).unwrap();
    assert!(
        terminal.complete && terminal.frames == 0,
        "post-complete drain must stay complete with zero frames",
    );
}

#[test]
fn graph_fan_in_rate_disagreement_refuses_at_construction() {
    // Narrowed Stage B refusal: a fan-in node with disagreeing upstream
    // clocks has no honest input clock — converting either branch would pick
    // the fan-in design rate without config authority, and no config field
    // disambiguates it. Construction fails with the exact disagreement,
    // never reaching the audio thread.
    reset_ab087_counters();
    let params = ABComparePluginParams {
        path_a: PathConfig::Graph {
            nodes: vec![
                graph_node(
                    "down",
                    "resampler",
                    resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                ),
                graph_node(
                    "up",
                    "resampler",
                    resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                ),
                graph_node("join", "zgain", json!({ "gain": 1.0 })),
            ],
            edges: vec![graph_edge("down", "join"), graph_edge("up", "join")],
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let error =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .err()
            .expect("disagreeing fan-in must fail construction");
    assert!(
        error.contains("disagreeing upstream clocks"),
        "unexpected construction error: {error}"
    );
}

fn recording_factory(
    plugin_type: &str,
    parameters: &Value,
    num_channels: usize,
    sample_rate: f64,
) -> Result<Box<dyn Plugin>, String> {
    CONSTRUCTION_RATES.with(|rates| {
        rates
            .borrow_mut()
            .push((plugin_type.to_owned(), sample_rate));
    });
    match plugin_type {
        "down" => Ok(Box::new(
            ResamplerPlugin::new(num_channels, sample_rate, HALF_RATE, NESTED_SRC_CHUNK).unwrap(),
        )),
        // Coefficient-bearing mid-graph stage: designs its biquad at the
        // resolved construction clock, so a 48 kHz mis-construction would
        // mistune the response the audio reference below pins at 24 kHz.
        "mid" => {
            let freq = parameters
                .get("freq")
                .and_then(Value::as_f64)
                .ok_or("recording factory mid node requires freq")?;
            let q = parameters
                .get("q")
                .and_then(Value::as_f64)
                .ok_or("recording factory mid node requires q")?;
            let band = Biquad::new(
                BiquadFilterType::Lowpass,
                freq,
                f64::from(sample_rate),
                q,
                0.0,
            );
            Ok(EqPlugin::new(num_channels, vec![band]).into_boxed_plugin())
        }
        other => Err(format!("recording factory has no plugin type {other}")),
    }
}

#[test]
fn graph_builtin_construction_uses_resolved_input_rate() {
    // ACCEPTANCE: build_graph constructs each node at its resolved input
    // clock so coefficient-bearing builtins design for the rate they run
    // at — not the outer rate (part B of `graph-lossless-proposal.md`).
    // The recording factory observes the construction rate per node: the
    // 48 kHz input node stays outer, while the mid-graph node constructs
    // at 24 kHz. A real EQ lowpass (not a passthrough marker) proves the
    // coefficients are 24 kHz-designed: the audio reference below runs
    // 24 kHz audio through it and pins the response against an independent
    // RBJ cookbook biquad at 1e-6.
    const MID_FREQ_HZ: f64 = 6000.0;
    const MID_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
    CONSTRUCTION_RATES.with(|rates| rates.borrow_mut().clear());
    let config = PathConfig::Graph {
        nodes: vec![
            graph_node("down", "down", json!({})),
            graph_node("mid", "mid", json!({ "freq": MID_FREQ_HZ, "q": MID_Q })),
        ],
        edges: vec![graph_edge("down", "mid")],
    };
    let host = build_path_from_config_with_factory(
        &config,
        CHANNELS,
        SAMPLE_RATE,
        Some(recording_factory),
    )
    .unwrap();
    assert_eq!(host.output_sample_rate(SAMPLE_RATE).unwrap(), f64::from(SAMPLE_RATE));
    let rates = CONSTRUCTION_RATES.with(|rates| rates.borrow().clone());
    assert_eq!(
        rates,
        vec![
            ("down".to_owned(), f64::from(SAMPLE_RATE)),
            ("mid".to_owned(), f64::from(HALF_RATE)),
        ],
        "mid-graph nodes must construct at their resolved input clock"
    );
    // Audio reference: the mid-graph EQ actually filters 24 kHz audio with
    // 24 kHz-designed coefficients. The raw-stage oracle (down-converter,
    // fresh EQ designed identically at 24 kHz, owned-clock converter
    // replica) must match the host bit-for-bit; an independent RBJ cookbook
    // biquad must match the EQ stage at 1e-6 (a 48 kHz mis-design would
    // mistune the normalized cutoff 0.125 vs 0.25, far outside 1e-6).
    const INPUT_FRAMES: usize = 1025;
    const CHUNKS: [usize; 6] = [1, 7, 64, 56, 137, 760];
    assert_eq!(CHUNKS.iter().sum::<usize>(), INPUT_FRAMES);
    for (name, input) in [
        ("constant", constant_input(INPUT_FRAMES, 0.25)),
        ("impulse", impulse_input(INPUT_FRAMES)),
        ("dense", dense_input(INPUT_FRAMES)),
    ] {
        let mut raw_down =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let raw_eq = EqPlugin::new(
            CHANNELS,
            vec![Biquad::new(
                BiquadFilterType::Lowpass,
                MID_FREQ_HZ,
                f64::from(HALF_RATE),
                MID_Q,
                0.0,
            )],
        );
        let mut raw_conv =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let mut raw_eq = raw_eq.into_boxed_plugin();
        raw_down.initialize(f64::from(SAMPLE_RATE)).unwrap();
        raw_eq.initialize(f64::from(HALF_RATE)).unwrap();
        raw_conv.initialize(f64::from(HALF_RATE)).unwrap();
        let mut expected = Vec::new();
        let mut eq_input_full = Vec::new();
        let mut eq_output_full = Vec::new();
        let mut offset = 0;
        for &frames in &CHUNKS {
            let block = &input[offset * CHANNELS..(offset + frames) * CHANNELS];
            offset += frames;
            let down_out = oracle_process(&mut raw_down, SAMPLE_RATE, block);
            let eq_out = oracle_process(raw_eq.as_mut(), HALF_RATE, &down_out);
            let conv_out = oracle_process(&mut raw_conv, HALF_RATE, &eq_out);
            eq_input_full.extend_from_slice(&down_out);
            eq_output_full.extend_from_slice(&eq_out);
            expected.extend_from_slice(&conv_out);
        }
        let down_tail = oracle_drain(&mut raw_down, SAMPLE_RATE);
        eq_input_full.extend_from_slice(&down_tail);
        if !down_tail.is_empty() {
            let eq_tail_in = oracle_process(raw_eq.as_mut(), HALF_RATE, &down_tail);
            let conv_tail_in = oracle_process(&mut raw_conv, HALF_RATE, &eq_tail_in);
            eq_output_full.extend_from_slice(&eq_tail_in);
            expected.extend_from_slice(&conv_tail_in);
        }
        let eq_tail = oracle_drain(raw_eq.as_mut(), HALF_RATE);
        eq_output_full.extend_from_slice(&eq_tail);
        if !eq_tail.is_empty() {
            let conv_eq_tail = oracle_process(&mut raw_conv, HALF_RATE, &eq_tail);
            expected.extend_from_slice(&conv_eq_tail);
        }
        expected.extend_from_slice(&oracle_drain(&mut raw_conv, HALF_RATE));
        // Independent cookbook check on the EQ stage alone.
        let mut cookbook: Vec<RbjBiquad> = (0..CHANNELS)
            .map(|_| RbjBiquad::new(false, MID_FREQ_HZ, f64::from(HALF_RATE)))
            .collect();
        let mut rbj_out = Vec::with_capacity(eq_input_full.len());
        for frame in eq_input_full.as_chunks::<CHANNELS>().0 {
            for (channel, &sample) in frame.iter().enumerate() {
                rbj_out.push(cookbook[channel].process(f64::from(sample)) as f32);
            }
        }
        assert_vectors_close(&eq_output_full, &rbj_out, name);
        let mut host = build_path_from_config_with_factory(
            &config,
            CHANNELS,
            SAMPLE_RATE,
            Some(recording_factory),
        )
        .unwrap();
        let mut produced = Vec::new();
        let mut frame_offset = 0;
        for &frames in &CHUNKS {
            let capacity = host.output_frames_for_input(frames);
            let mut block = vec![f32::NAN; capacity * CHANNELS];
            let actual = host
                .process(
                    &input[frame_offset * CHANNELS..(frame_offset + frames) * CHANNELS],
                    &mut block,
                )
                .unwrap();
            assert!(actual <= capacity, "{name}: host overproduced its bound");
            produced.extend_from_slice(&block[..actual * CHANNELS]);
            frame_offset += frames;
        }
        let predicted_drain = expected.len() / CHANNELS - produced.len() / CHANNELS;
        let drain_backstop = INPUT_FRAMES + predicted_drain + 2;
        let drain_capacity = host.drain_output_frames_max();
        let mut drain_block = vec![f32::NAN; drain_capacity.max(1) * CHANNELS];
        let mut drain_calls = 0;
        loop {
            drain_block.fill(f32::NAN);
            let result = host.drain(&mut drain_block).unwrap();
            assert!(
                result.frames <= drain_capacity,
                "{name}: drain overproduced"
            );
            produced.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
            drain_calls += 1;
            if result.complete {
                break;
            }
            assert!(
                drain_calls < drain_backstop,
                "{name}: drain exceeded its derived backstop ({drain_backstop})"
            );
        }
        assert_eq!(produced.len(), expected.len(), "{name} whole-stream length");
        assert_eq!(produced, expected, "{name} whole-stream samples");
        println!(
            "mid-graph EQ@24k {name}: {} frames bitwise, drain {drain_calls} calls",
            produced.len() / CHANNELS,
        );
    }
}

#[test]
fn decimator_child_drains_independently_with_exact_counts() {
    reset_ab087_counters();
    // Drive the decimator child host directly (irregular blocks, then EOF)
    // and compare the whole produced stream against the independent oracle.
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse-even", impulse_input(4096)),
        ("impulse-odd", impulse_input(1025)),
        ("dense-odd", dense_input(1025)),
    ] {
        let mut host = DawHost::new(CHANNELS, SAMPLE_RATE);
        host.add_plugin(Box::new(DecimatorTwoFixture::new(CHANNELS).unwrap()))
            .unwrap();
        host.build().unwrap();
        assert_eq!(host.output_sample_rate(SAMPLE_RATE).unwrap(), f64::from(HALF_RATE));

        let input_frames = input.len() / CHANNELS;
        let mut chunks = vec![1_usize, 64, 137, 7];
        let chunked: usize = chunks.iter().sum();
        assert!(chunked < input_frames);
        chunks.push(input_frames - chunked);

        let mut produced = Vec::new();
        let mut frame_offset = 0;
        for &frames in &chunks {
            let capacity = host.output_frames_for_input(frames);
            let mut block = vec![f32::NAN; capacity * CHANNELS];
            let actual = host
                .process(
                    &input[frame_offset * CHANNELS..(frame_offset + frames) * CHANNELS],
                    &mut block,
                )
                .unwrap();
            assert!(actual <= capacity, "{name}: host overproduced its bound");
            produced.extend_from_slice(&block[..actual * CHANNELS]);
            frame_offset += frames;
        }
        let mut tail = Vec::new();
        let drain_capacity = host.drain_output_frames_max();
        assert_eq!(drain_capacity, 1, "{name}: decimator drain bound");
        let mut block = vec![f32::NAN; drain_capacity * CHANNELS];
        for _ in 0..DRAIN_CALL_LIMIT {
            block.fill(f32::NAN);
            let result = host.drain(&mut block).unwrap();
            tail.extend_from_slice(&block[..result.frames * CHANNELS]);
            if result.complete {
                break;
            }
        }
        let mut whole = produced;
        whole.extend_from_slice(&tail);
        let expected = decimate_whole_stream_reference(&input, CHANNELS);
        assert_eq!(whole.len(), expected.len(), "{name} whole-stream length");
        assert_eq!(whole, expected, "{name} whole-stream samples");

        // Exact count accounting: floor pairs plus the odd carry flush.
        let expected_frames = input_frames / 2 + input_frames % 2;
        assert_eq!(whole.len() / CHANNELS, expected_frames, "{name} count");
        if name == "constant" {
            assert!(whole.iter().all(|sample| *sample == 0.25));
            assert_eq!(whole.len() / CHANNELS, 2048);
        }
        if name == "impulse-even" {
            assert_eq!(whole[0], 0.5);
            assert!(whole[CHANNELS..].iter().all(|sample| *sample == 0.0));
        }
    }
}

#[test]
fn decimator_live_query_undercounts_evolved_carry_arrival() {
    // R7-F2 arrival-pattern pin: queried at carry 0, live(1) is 0, but after
    // one frame evolves the state to carry 1 the same query answers 1 and an
    // arrival there produces 1 — fixed-state monotonicity never proves
    // varying-state arrival. The carry-maximized envelope covers both
    // states at every quantum.
    reset_ab087_counters();
    let mut decimator = DecimatorTwoFixture::new(CHANNELS).unwrap();
    assert_eq!(decimator.output_frames_for_input(1), 0);
    assert_eq!(decimator.output_frames_envelope(1), Some(1));
    for quantum in 0..8_usize {
        let live = decimator.output_frames_for_input(quantum);
        let envelope = decimator.output_frames_envelope(quantum).unwrap();
        assert_eq!(
            envelope,
            quantum.div_ceil(2),
            "envelope formula at {quantum}"
        );
        assert!(live <= envelope, "carry-0 live {live} under {envelope}");
    }
    // One frame at carry 0: retained, carry becomes 1, nothing emitted.
    let input = vec![0.5_f32; CHANNELS];
    let mut output = vec![f32::NAN; 2 * CHANNELS];
    let context = ProcessContext::new(SAMPLE_RATE, 1);
    assert_eq!(decimator.process(&input, &mut output, &context).unwrap(), 0);
    // Same query, evolved state: the answer changed under the query.
    assert_eq!(decimator.output_frames_for_input(1), 1);
    for quantum in 0..8_usize {
        let live = decimator.output_frames_for_input(quantum);
        let envelope = decimator.output_frames_envelope(quantum).unwrap();
        assert!(live <= envelope, "carry-1 live {live} under {envelope}");
    }
    // Arrival at carry 1 emits the pair average the carry-0 query missed.
    assert_eq!(decimator.process(&input, &mut output, &context).unwrap(), 1);
    assert!(output[..CHANNELS].iter().all(|sample| *sample == 0.5));
    println!("decimator arrival: live(1) 0->1 across carry, envelope 1 covers");
}

#[test]
fn same_clock_bursts_compose_across_irregular_blocks() {
    reset_ab087_counters();
    let input = dense_input(1000);
    let irregular = [1_usize, 7, 64, 63, 65, 137, 2, 621, 40];
    assert_eq!(irregular.iter().sum::<usize>(), 1000);

    // Single-block reference: bursts preserve every sample, so the composed
    // whole stream (process + drain) equals the input bit-for-bit.
    let mut single = make_burst_plugin(0.0);
    assert_eq!(single.drain_output_frames_max(), BURST_CHUNK);
    let mut single_process = render_collected(&mut single, &input, &[1000]);
    let (single_tail, single_partition) = drain_all_collected(&mut single);
    assert!(single_partition.iter().all(|&frames| frames <= BURST_CHUNK));
    single_process.extend_from_slice(&single_tail);
    assert_eq!(single_process.len(), input.len(), "single-block length");
    assert_eq!(single_process, input, "single-block samples");

    // Irregular partitioning must produce the identical concatenated stream.
    let mut split = make_burst_plugin(0.0);
    let mut split_process = render_collected(&mut split, &input, &irregular);
    let (split_tail, _) = drain_all_collected(&mut split);
    split_process.extend_from_slice(&split_tail);
    assert_eq!(split_process, single_process, "partition invariance");
    assert_eq!(split_process, input, "irregular-block samples");
    assert!(BURST_PROCESS_CALLS.get() > 0);
    assert!(BURST_DRAIN_CALLS.get() > 0);

    // The same burst composition delivered through an outer host EOF route.
    // The host pads same-clock variable returns up to the input length and
    // reports actual production via `last_output_frames` (DawHost::process);
    // the diligent outer caller below is the contract this suite pins.
    let nested = make_burst_plugin(0.0);
    let mut outer = DawHost::new(CHANNELS, SAMPLE_RATE);
    outer.add_plugin(Box::new(nested)).unwrap();
    outer.build().unwrap();
    let mut outer_process = vec![0.0; input.len()];
    let outer_returned = outer.process(&input, &mut outer_process).unwrap();
    let outer_actual = outer.last_output_frames().unwrap_or(outer_returned);
    assert!(
        outer_actual <= 1000,
        "outer host must observe the nested variable emit"
    );
    let mut outer_whole = outer_process[..outer_actual * CHANNELS].to_vec();
    let outer_capacity = outer.drain_output_frames_max();
    let mut block = vec![f32::NAN; outer_capacity * CHANNELS];
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = outer.drain(&mut block).unwrap();
        outer_whole.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            break;
        }
    }
    assert_eq!(outer_whole.len(), input.len(), "outer-host length");
    assert_eq!(outer_whole, input, "outer-host whole stream");
}

#[test]
fn burst_against_steady_identity_pairs_without_loss() {
    reset_ab087_counters();
    let input = dense_input(500);
    let chunks = [3_usize, 64, 61, 137, 235];
    assert_eq!(chunks.iter().sum::<usize>(), 500);

    // Bursty path A against steady identity path B; every mix position must
    // reproduce the input exactly because both paths carry it unchanged.
    for (name, mix) in [("pure A", -1.0), ("center", 0.0), ("pure B", 1.0)] {
        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "burst".to_owned(),
                parameters: burst_params(BURST_CHUNK),
            },
            path_b: PathConfig::Plugin {
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 1.0 }),
            },
            mix,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let mut process_output = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        process_output.extend_from_slice(&tail);
        assert_eq!(process_output.len(), input.len(), "{name} length");
        assert_eq!(process_output, input, "{name} samples");
    }
}

#[test]
fn connected_dag_process_and_drain_match_branch_oracle() {
    reset_ab087_counters();
    // Diamond: S(0.5) -> {A(2.0), B(0.25)} -> T(1.0); the host sums the join
    // and every node is zero-latency, so the oracle is input * 1.125.
    let diamond = PathConfig::Graph {
        nodes: vec![
            GraphNodeConfig {
                id: "source".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 0.5 }),
            },
            GraphNodeConfig {
                id: "branch-a".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 2.0 }),
            },
            GraphNodeConfig {
                id: "branch-b".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 0.25 }),
            },
            GraphNodeConfig {
                id: "sink".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 1.0 }),
            },
        ],
        edges: vec![
            GraphEdgeConfig {
                from: "source".to_owned(),
                to: "branch-a".to_owned(),
                channel_map: None,
                destination_offset: 0,
            },
            GraphEdgeConfig {
                from: "source".to_owned(),
                to: "branch-b".to_owned(),
                channel_map: None,
                destination_offset: 0,
            },
            GraphEdgeConfig {
                from: "branch-a".to_owned(),
                to: "sink".to_owned(),
                channel_map: None,
                destination_offset: 0,
            },
            GraphEdgeConfig {
                from: "branch-b".to_owned(),
                to: "sink".to_owned(),
                channel_map: None,
                destination_offset: 0,
            },
        ],
    };
    let params = ABComparePluginParams {
        path_a: diamond,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();

    let input = dense_input(83);
    let expected = diamond_reference(&input, 0.5, 2.0, 0.25, 1.0);
    assert!(
        expected.iter().any(|sample| sample.abs() > 0.5),
        "oracle must exercise both branches"
    );
    let mut process_output = render_collected(&mut plugin, &input, &[1, 7, 19, 3, 53]);
    let (tail, partition) = drain_all_collected(&mut plugin);
    assert!(
        partition.iter().all(|&frames| frames == 0),
        "tail-less diamond emits no drain frames"
    );
    process_output.extend_from_slice(&tail);
    assert_vectors_close(&process_output, &expected, "diamond");
}

/// One raw stage process call with diligent actual-count readback, mirroring
/// the reference helper's per-call mechanics while keeping caller-owned
/// state across blocks.
fn oracle_process(stage: &mut dyn Plugin, rate: u32, input: &[f32]) -> Vec<f32> {
    assert!(input.len().is_multiple_of(CHANNELS));
    let frames = input.len() / CHANNELS;
    let capacity = stage.output_frames_for_input(frames);
    let mut block = vec![f32::NAN; capacity * CHANNELS];
    let returned = stage
        .process(input, &mut block, &ProcessContext::new(rate, frames))
        .unwrap();
    let actual = stage.last_output_frames().unwrap_or(returned);
    assert!(
        actual <= capacity,
        "oracle stage overproduced its bound: {actual} > {capacity}"
    );
    block[..actual * CHANNELS].to_vec()
}

/// One raw stage full drain with the reference helper's loop shape.
fn oracle_drain(stage: &mut dyn Plugin, rate: u32) -> Vec<f32> {
    stage.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
    let capacity = stage.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity * CHANNELS];
    let mut output = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = stage
            .drain(&mut block, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(result.frames <= capacity);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return output;
        }
    }
    panic!("oracle drain exceeded the test's bounded call allowance");
}

/// ACCEPTANCE: lossless variable-diamond oracle. Drives raw branch stages
/// block by block with the test's partition, pushes converter output and
/// compensation-delayed burst output into per-edge retention queues, joins
/// aligned-prefix sums every block, then drains each branch raw and joins
/// retained queues plus tails with zero-padding — exactly the host's
/// retained merge/drain semantics (per-edge FIFOs, `DelayBuffer` FIFO, EOF
/// transfer with silence-padded finished edges). Nothing drops: every
/// emitted frame joins exactly once. Sample positions and drain-feed
/// partitioning are proven irrelevant by the passing chain roundtrips,
/// which match single-block position-zero references bitwise under
/// irregular scheduler partitioning.
struct RetainedDiamondOracle {
    down: ResamplerPlugin,
    up: ResamplerPlugin,
    burst: BurstFixture,
    queue_a: VecDeque<f32>,
    queue_b: VecDeque<f32>,
    delay: VecDeque<f32>,
    comp_frames: usize,
    s_gain: f32,
    t_gain: f32,
    a_counts: Vec<usize>,
    b_counts: Vec<usize>,
    join_counts: Vec<usize>,
    a_joined_nonzero: bool,
    b_joined_nonzero: bool,
}

impl RetainedDiamondOracle {
    fn new(s_gain: f32, t_gain: f32) -> Self {
        let mut down =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let mut up =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let mut burst = BurstFixture::new(CHANNELS, BURST_CHUNK, true).unwrap();
        down.initialize(f64::from(SAMPLE_RATE)).unwrap();
        up.initialize(f64::from(HALF_RATE)).unwrap();
        burst.initialize(f64::from(SAMPLE_RATE)).unwrap();
        // Merge compensation on the short-latency (burst) edge: the A-branch
        // cumulative latency in the 48 kHz join clock. Exact for the 2:1 step
        // (no tick rounding); the bitwise whole-stream match below verifies it.
        let comp_frames = up.latency_samples() + 2 * down.latency_samples();
        assert!(comp_frames > 0, "oracle needs a nonzero merge compensation");
        Self {
            down,
            up,
            burst,
            queue_a: VecDeque::new(),
            queue_b: VecDeque::new(),
            delay: VecDeque::from(vec![0.0; comp_frames * CHANNELS]),
            comp_frames,
            s_gain,
            t_gain,
            a_counts: Vec::new(),
            b_counts: Vec::new(),
            join_counts: Vec::new(),
            a_joined_nonzero: false,
            b_joined_nonzero: false,
        }
    }

    /// Feed samples through the merge compensation delay FIFO.
    fn delay_feed(&mut self, input: &[f32]) -> Vec<f32> {
        assert!(input.len().is_multiple_of(CHANNELS));
        let mut output = Vec::with_capacity(input.len());
        for &sample in input {
            self.delay.push_back(sample);
            output.push(self.delay.pop_front().unwrap());
        }
        output
    }

    /// Flush the merge delay with compensation many zero frames, exactly the
    /// scheduler's EOF flush at the short-latency branch's completion.
    fn delay_flush(&mut self) -> Vec<f32> {
        let mut output = Vec::with_capacity(self.comp_frames * CHANNELS);
        for _ in 0..self.comp_frames * CHANNELS {
            self.delay.push_back(0.0);
            output.push(self.delay.pop_front().unwrap());
        }
        output
    }

    /// Feed one input block; returns the joined T output for the block.
    fn feed_block(&mut self, block: &[f32]) -> Vec<f32> {
        let staged: Vec<f32> = block.iter().map(|sample| sample * self.s_gain).collect();
        let a1 = oracle_process(&mut self.down, SAMPLE_RATE, &staged);
        let a2 = oracle_process(&mut self.up, HALF_RATE, &a1);
        let b = oracle_process(&mut self.burst, SAMPLE_RATE, &staged);
        let delayed = self.delay_feed(&b);
        // Retained host join: both edges push their full emission through
        // compensation into per-edge FIFOs; the join consumes the aligned
        // prefix and T scales the sum. Unmatched suffixes stay queued for
        // later blocks — nothing drops.
        self.queue_a.extend(a2.iter().copied());
        self.queue_b.extend(delayed.iter().copied());
        let joined_frames = (self.queue_a.len() / CHANNELS).min(self.queue_b.len() / CHANNELS);
        let mut joined = Vec::with_capacity(joined_frames * CHANNELS);
        for _ in 0..joined_frames * CHANNELS {
            let a = self.queue_a.pop_front().unwrap();
            let d = self.queue_b.pop_front().unwrap();
            if a != 0.0 {
                self.a_joined_nonzero = true;
            }
            if d != 0.0 {
                self.b_joined_nonzero = true;
            }
            joined.push((a + d) * self.t_gain);
        }
        self.a_counts.push(a2.len() / CHANNELS);
        self.b_counts.push(b.len() / CHANNELS);
        self.join_counts.push(joined_frames);
        joined
    }

    /// Drain every branch raw, extend the process-retained queues, and join
    /// with zero-padding — exactly the drain scheduler's queue semantics
    /// (lossless: every queued frame pairs, finished edges pad silence),
    /// including the compensation-delay flush.
    fn finish(&mut self) -> Vec<f32> {
        let a1_tail = oracle_drain(&mut self.down, SAMPLE_RATE);
        let mut a_drain = if a1_tail.is_empty() {
            Vec::new()
        } else {
            oracle_process(&mut self.up, HALF_RATE, &a1_tail)
        };
        a_drain.extend_from_slice(&oracle_drain(&mut self.up, HALF_RATE));
        let b_tail = oracle_drain(&mut self.burst, SAMPLE_RATE);
        // Drain continues through the merge delay, then the scheduler flushes
        // the delay with compensation many zero frames at B's completion.
        let mut delayed = self.delay_feed(&b_tail);
        delayed.extend(self.delay_flush());
        // Process-retained suffixes stay queued across the EOF boundary (the
        // host transfers them into drain); tails extend the same queues, and
        // the exhausted side pads silence.
        self.queue_a.extend(a_drain.iter().copied());
        self.queue_b.extend(delayed.iter().copied());
        let joined_frames = (self.queue_a.len() / CHANNELS).max(self.queue_b.len() / CHANNELS);
        self.queue_a.resize(joined_frames * CHANNELS, 0.0);
        self.queue_b.resize(joined_frames * CHANNELS, 0.0);
        let mut joined = Vec::with_capacity(joined_frames * CHANNELS);
        for _ in 0..joined_frames * CHANNELS {
            let a = self.queue_a.pop_front().unwrap();
            let d = self.queue_b.pop_front().unwrap();
            joined.push((a + d) * self.t_gain);
        }
        joined
    }
}

#[test]
fn variable_diamond_lossless_join_matches_retention_oracle() {
    reset_ab087_counters();
    // ACCEPTANCE (D6): plugin-level lossless diamond proof. S(0.5) ->
    // { A1 down 48→24 -> A2 up 24→48, B burst-64 } -> T(2.0) with genuine
    // unequal burst/rate-converter branches. The retention oracle reproduces
    // the host scheduler bit-exactly: per-edge process queues with the merge
    // compensation delay on the burst edge, aligned-prefix joins every
    // block, EOF transfer of retained queues plus native tails with
    // zero-padding and the delay flush during drain. Both branches provably
    // contribute nonzero joined content (activation intent preserved from
    // the retired min-prefix diagnostic, which could never go green). The
    // whole-stream asserts below pin lossless length and content; the surplus
    // past the oracle is exactly the zero ring flush within reported latency.
    // N=8229 (residue 37): the aligned process join must clear the 3262-frame
    // comp prefill for burst content to enter (precondition below derives it).
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 7813, 77];
    assert_eq!(chunks.iter().sum::<usize>(), 8229);
    let diamond = PathConfig::Graph {
        nodes: vec![
            graph_node("source", "zgain", json!({ "gain": 0.5 })),
            graph_node(
                "branch-a-down",
                "resampler",
                resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node(
                "branch-a-up",
                "resampler",
                resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node("branch-b", "burst", burst_params(BURST_CHUNK)),
            graph_node("sink", "zgain", json!({ "gain": 2.0 })),
        ],
        edges: vec![
            graph_edge("source", "branch-a-down"),
            graph_edge("branch-a-down", "branch-a-up"),
            graph_edge("source", "branch-b"),
            graph_edge("branch-a-up", "sink"),
            graph_edge("branch-b", "sink"),
        ],
    };
    for (name, input) in [
        ("constant", constant_input(8229, 0.25)),
        ("impulse", impulse_input(8229)),
        ("dense", dense_input(8229)),
    ] {
        // Independent oracle over raw stages first.
        let mut oracle = RetainedDiamondOracle::new(0.5, 2.0);
        let mut expected = Vec::new();
        let mut offset = 0;
        for &frames in &chunks {
            expected.extend_from_slice(
                &oracle.feed_block(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
            );
            offset += frames;
        }
        let process_frames = expected.len() / CHANNELS;
        expected.extend_from_slice(&oracle.finish());
        let drain_frames = expected.len() / CHANNELS - process_frames;

        // Stream-duration precondition (derived): the aligned join consumes
        // min(A,B) pushed frames from each queue front, and queue B opens
        // with `comp_frames` prefill zeros, so burst content reaches the
        // process join iff joined_total > comp_frames. At N=4133 the up-1024
        // stage saw < 2048 frames (down-1024 priming shortfall p1 in (0,256],
        // measured via the 3580-frame graph-path print), completing 1 chunk
        // for A_push ~= 2000 < comp = 3262, so the join never left the
        // prefill. At N=8229 down completes 8 chunks and up 3, so
        // A_push ~= 6000 clears 3262 with wide margin (B_push = 8192
        // exactly, residue 37). For these inputs delayed[comp] = staged[0]
        // is nonzero on channel 0 (0.125 / 0.5 / -0.34375), so this
        // precondition is exactly equivalent to the burst flag firing.
        let joined_total: usize = oracle.join_counts.iter().sum();
        let comp = oracle.comp_frames;
        assert!(
            joined_total > comp,
            "{name}: stream too short: joined {joined_total} within {comp} prefill frames"
        );

        // Activation proofs: both branches genuinely variable and nonzero in
        // the join (no vacuous branch coverage).
        assert!(
            oracle.b_counts.contains(&0),
            "{name}: burst branch must idle some blocks"
        );
        assert!(
            oracle.b_counts.iter().any(|&count| count > 0),
            "{name}: burst branch must fire some blocks"
        );
        let a_min = *oracle.a_counts.iter().min().unwrap();
        let a_max = *oracle.a_counts.iter().max().unwrap();
        assert!(
            a_max > a_min,
            "{name}: converter branch counts must vary ({a_min}..={a_max})"
        );
        assert!(a_max > 0, "{name}: converter branch must produce");
        assert!(
            oracle.a_joined_nonzero,
            "{name}: converter content must enter the join"
        );
        assert!(
            oracle.b_joined_nonzero,
            "{name}: burst content must enter the join"
        );
        assert!(drain_frames > 0, "{name}: drain must carry tail content");
        if name == "dense" {
            assert!(
                expected[process_frames * CHANNELS..]
                    .iter()
                    .any(|sample| *sample != 0.0),
                "dense drain tail must be nonzero somewhere"
            );
        }

        let params = ABComparePluginParams {
            path_a: diamond.clone(),
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let burst_before = BURST_PROCESS_CALLS.get();
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: diamond latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert!(
            BURST_PROCESS_CALLS.get() > burst_before,
            "{name}: the burst branch must actually advance"
        );
        // Whole-stream lossless proof: every oracle frame reproduced in
        // order, bit-for-bit; any surplus is exactly the zero ring flush
        // within reported latency. No length equation is assumed — drops,
        // insertions, or unaccounted audio fail loudly below.
        assert!(
            whole.len() >= expected.len(),
            "{name}: plugin emitted {} frames, oracle expects {}",
            whole.len() / CHANNELS,
            expected.len() / CHANNELS,
        );
        assert_eq!(
            &whole[..expected.len()],
            &expected[..],
            "{name} lossless whole-stream content"
        );
        let ring_suffix = &whole[expected.len()..];
        assert!(
            ring_suffix.iter().all(|sample| *sample == 0.0),
            "{name}: surplus past the oracle must be the zero ring flush"
        );
        assert!(
            ring_suffix.len() / CHANNELS <= plugin.latency_samples(),
            "{name}: {} surplus frames exceed reported latency {}",
            ring_suffix.len() / CHANNELS,
            plugin.latency_samples(),
        );
        println!(
            "lossless diamond {name}: process {process_frames} frames (join {:?}), \
             drain {drain_frames} frames, comp {}, ring suffix {}",
            oracle.join_counts,
            oracle.comp_frames,
            ring_suffix.len() / CHANNELS,
        );
    }
}

#[test]
fn twin_branching_composes_process_and_drain() {
    // The factory join supersedes the branching-drain refusal: identical
    // parallel branches sum in process and drain losslessly, and divergent
    // branches join transparently with the same exactness. All three
    // behaviors are pinned deliberately.
    reset_ab087_counters();
    let parallel = PathConfig::Graph {
        nodes: vec![
            GraphNodeConfig {
                id: "branch-a".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 0.5 }),
            },
            GraphNodeConfig {
                id: "branch-b".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 0.5 }),
            },
        ],
        edges: Vec::new(),
    };
    let params = ABComparePluginParams {
        path_a: parallel,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();

    let input = dense_input(64);
    let process_output = render_collected(&mut plugin, &input, &[64]);
    assert_eq!(process_output, input, "parallel 0.5 + 0.5 sums to unity");
    let (tail, _) = drain_all_collected(&mut plugin);
    assert!(tail.is_empty(), "twin unity path drains empty");

    // Twin bursty nodes: per-call counts match a solo reporting burst
    // exactly (lockstep proof), while content doubles (both twins live).
    let parallel_burst = PathConfig::Graph {
        nodes: vec![
            GraphNodeConfig {
                id: "branch-a".to_owned(),
                plugin_type: "burst".to_owned(),
                parameters: burst_params(BURST_CHUNK),
            },
            GraphNodeConfig {
                id: "branch-b".to_owned(),
                plugin_type: "burst".to_owned(),
                parameters: burst_params(BURST_CHUNK),
            },
        ],
        edges: Vec::new(),
    };
    let params = ABComparePluginParams {
        path_a: parallel_burst,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut twin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    twin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let solo_params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params(BURST_CHUNK),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut solo = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        solo_params,
        ab087_factory,
    )
    .unwrap();
    solo.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let chunks = [10_usize, 64, 7, 129, 64];
    assert_eq!(chunks.iter().sum::<usize>(), 274);
    let input = dense_input(274);
    let (mut twin_whole, twin_returns) = render_collected_captured(&mut twin, &input, &chunks);
    let (mut solo_whole, solo_returns) = render_collected_captured(&mut solo, &input, &chunks);
    assert_eq!(
        twin_returns, solo_returns,
        "twin per-call counts must match the solo burst exactly",
    );
    let (twin_tail, _) = drain_all_collected(&mut twin);
    let (solo_tail, _) = drain_all_collected(&mut solo);
    twin_whole.extend_from_slice(&twin_tail);
    solo_whole.extend_from_slice(&solo_tail);
    assert_eq!(solo_whole, input, "solo burst preserves every sample");
    let doubled: Vec<f32> = solo_whole.iter().map(|sample| sample + sample).collect();
    assert_eq!(
        twin_whole, doubled,
        "twin burst content must double the solo stream",
    );

    // Divergent identity branches (different gains) join transparently:
    // process sums exactly and drain completes (empty tails), leaving the
    // parent accepting input.
    let divergent = PathConfig::Graph {
        nodes: vec![
            GraphNodeConfig {
                id: "branch-a".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 0.5 }),
            },
            GraphNodeConfig {
                id: "branch-b".to_owned(),
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 0.25 }),
            },
        ],
        edges: Vec::new(),
    };
    let params = ABComparePluginParams {
        path_a: divergent,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut admitted =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    admitted.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = dense_input(64);
    let output = render_collected(&mut admitted, &input, &[64]);
    let summed: Vec<f32> = input
        .iter()
        .map(|sample| sample * 0.5 + sample * 0.25)
        .collect();
    assert_eq!(
        output, summed,
        "divergent identity branches must sum exactly in process",
    );
    let (tail, _) = drain_all_collected(&mut admitted);
    assert!(tail.is_empty(), "joined unity path drains empty");
    // Composed drain freezes parameters until reset (existing lifecycle):
    // the drained parent refuses mutation here, then reset restores a
    // clean accepting parent that re-renders the summed stream exactly.
    let frozen = admitted
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
        .unwrap_err();
    assert!(
        frozen.contains("frozen until reset"),
        "drained parent must pin the frozen lifecycle: {frozen}",
    );
    admitted.reset();
    admitted
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
        .expect("reset must leave the parent accepting input");
    admitted
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(-1.0))
        .unwrap();
    let rerun = render_collected(&mut admitted, &input, &[64]);
    assert_eq!(
        rerun, summed,
        "reset parent must re-render the summed stream",
    );
}

#[test]
fn band_mask_drain_emits_proven_residual_and_completes() {
    // Supersedes the R1 mask-drain refusal: with unity paths the wet sequence
    // is exactly the input, so the whole emitted stream (process + proven
    // flush) must match the fresh-cascade oracle bit-for-bit, agree with the
    // independent cookbook cascade at 1e-6, and leave a remainder below
    // program peak x 2^-24 (verified far beyond the observed flush).
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 56];
    assert_eq!(chunks.iter().sum::<usize>(), 128);
    for (name, input) in [("impulse", impulse_input(128)), ("dense", dense_input(128))] {
        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 1.0 }),
            },
            path_b: PathConfig::Plugin {
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 1.0 }),
            },
            mix: -1.0,
            band_mask_low_hz: 500.0,
            band_mask_high_hz: 8_000.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let mut process_output = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        process_output.extend_from_slice(&tail);
        let emitted_frames = process_output.len() / CHANNELS;
        let flush_frames = emitted_frames - 128;
        assert!(
            flush_frames >= 64,
            "{name}: the residual flush must engage nontrivially, got {flush_frames}"
        );
        // Unity paths make wet exactly the input during process and zeros
        // during drain (tail-less children, no latency rings, empty dry).
        let mut wet = input.clone();
        wet.extend(std::iter::repeat_n(0.0, flush_frames * CHANNELS));
        let cascade =
            mask_cascade_reference(&wet, CHANNELS, 500.0, 8_000.0, f64::from(SAMPLE_RATE));
        assert_eq!(cascade.len(), process_output.len(), "{name} length");
        assert_eq!(process_output, cascade, "{name} bit-exact cascade");
        let cookbook =
            mask_cascade_rbj_reference(&wet, CHANNELS, 500.0, 8_000.0, f64::from(SAMPLE_RATE));
        assert_vectors_close(&process_output, &cookbook, name);
        // Soundness: the unemitted remainder stays below peak x 2^-24.
        let wet_peak = input
            .iter()
            .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
        assert!(wet_peak > 0.0, "{name}: programs must excite the mask");
        let threshold = wet_peak * MASK_RESIDUAL_RATIO;
        let tail_max = mask_tail_max_beyond(
            &wet,
            CHANNELS,
            500.0,
            8_000.0,
            f64::from(SAMPLE_RATE),
            20_000,
        );
        assert!(
            tail_max < threshold,
            "{name}: remainder {tail_max} must stay below {threshold}"
        );
    }

    // Immediate drain with no program: zero excitation means zero flush, and
    // tail-less unity paths complete with no frames at all.
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "zgain".to_owned(),
            parameters: json!({ "gain": 1.0 }),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        band_mask_low_hz: 500.0,
        band_mask_high_hz: 8_000.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let (tail, partition) = drain_all_collected(&mut plugin);
    assert!(tail.is_empty(), "unexcited mask emits no flush");
    assert_eq!(partition, vec![0]);
}

fn unity_mask_plugin(low_hz: f32, high_hz: f32) -> ABComparePlugin {
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "zgain".to_owned(),
            parameters: json!({ "gain": 1.0 }),
        },
        path_b: PathConfig::Plugin {
            plugin_type: "zgain".to_owned(),
            parameters: json!({ "gain": 1.0 }),
        },
        mix: -1.0,
        band_mask_low_hz: low_hz,
        band_mask_high_hz: high_hz,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory).unwrap()
}

fn none_mask_params(low_hz: f32, high_hz: f32) -> ABComparePluginParams {
    ABComparePluginParams {
        path_a: PathConfig::None,
        path_b: PathConfig::None,
        mix: -1.0,
        band_mask_low_hz: low_hz,
        band_mask_high_hz: high_hz,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    }
}

/// Render through `process` at an explicit outer rate, honoring each call's
/// actual returned count.
fn render_collected_at_rate(
    plugin: &mut ABComparePlugin,
    input: &[f32],
    chunks: &[usize],
    rate: u32,
) -> Vec<f32> {
    assert_eq!(chunks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = Vec::new();
    let mut frame_offset = 0;
    for &frames in chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(rate, frames);
        let produced = plugin
            .process(&input[start..end], &mut block, &context)
            .unwrap();
        assert!(
            produced <= frames,
            "plugin emitted {produced} frames for a {frames}-frame block"
        );
        output.extend_from_slice(&block[..produced * CHANNELS]);
        assert!(
            block[produced * CHANNELS..]
                .iter()
                .all(|sample| sample.is_nan()),
            "output beyond the returned {produced} frames must stay untouched"
        );
        frame_offset += frames;
    }
    output
}

fn drain_all_collected_at_rate(plugin: &mut ABComparePlugin, rate: u32) -> (Vec<f32>, Vec<usize>) {
    let context = ProcessContext::new(rate, 0);
    plugin.begin_drain(&context).unwrap();
    let capacity_frames = plugin.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut output = Vec::new();
    let mut partition = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, &context).unwrap();
        assert!(result.frames <= capacity_frames);
        partition.push(result.frames);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return (output, partition);
        }
    }
    panic!("AB087 drain exceeded the test's bounded call allowance");
}

#[test]
fn band_mask_activation_boundary_pins_epsilon() {
    // band_mask_active is low > 20.5 || high < 19999.5: inactive pairs pass
    // through bitwise with empty drains, just-active pairs filter (bitwise
    // oracle, RBJ at 1e-6, flush engages, remainder below peak·2^-24). Proves
    // no vacuous "active" tail test (cf. reviewer 20/20000, inactive).
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 56];
    assert_eq!(chunks.iter().sum::<usize>(), 128);
    let input = dense_input(128);
    for (name, low, high) in [
        ("full-range", 20.0_f32, 20_000.0_f32),
        ("boundary", 20.5_f32, 19_999.5_f32),
    ] {
        let mut plugin = unity_mask_plugin(low, high);
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let process_output = render_collected(&mut plugin, &input, &chunks);
        assert_eq!(process_output, input, "{name} inactive passthrough");
        let (tail, _) = drain_all_collected(&mut plugin);
        assert!(tail.is_empty(), "{name} inactive drain empty");
    }
    for (name, low, high) in [
        ("low-edge", 20.6_f32, 20_000.0_f32),
        ("high-edge", 20.0_f32, 19_999.4_f32),
        ("both-edges", 20.6_f32, 19_999.4_f32),
    ] {
        let mut plugin = unity_mask_plugin(low, high);
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let mut process_output = render_collected(&mut plugin, &input, &chunks);
        assert_ne!(process_output, input, "{name} just-active must filter");
        let (tail, _) =
            drain_all_collected_rate_bounded(&mut plugin, input.len() / CHANNELS, 20_000);
        process_output.extend_from_slice(&tail);
        let flush_frames = process_output.len() / CHANNELS - 128;
        assert!(flush_frames > 0, "{name} flush must engage");
        let mut wet = input.clone();
        wet.extend(std::iter::repeat_n(0.0, flush_frames * CHANNELS));
        // Oracle cutoffs convert from the same f32 values the plugin uses.
        let (low_f64, high_f64) = (f64::from(low), f64::from(high));
        let cascade =
            mask_cascade_reference(&wet, CHANNELS, low_f64, high_f64, f64::from(SAMPLE_RATE));
        assert_eq!(process_output, cascade, "{name} bit-exact cascade");
        let cookbook =
            mask_cascade_rbj_reference(&wet, CHANNELS, low_f64, high_f64, f64::from(SAMPLE_RATE));
        assert_vectors_close(&process_output, &cookbook, name);
        let wet_peak = input
            .iter()
            .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
        let threshold = wet_peak * MASK_RESIDUAL_RATIO;
        let tail_max = mask_tail_max_beyond(
            &wet,
            CHANNELS,
            low_f64,
            high_f64,
            f64::from(SAMPLE_RATE),
            20_000,
        );
        assert!(
            tail_max < threshold,
            "{name}: remainder {tail_max} must stay below {threshold}"
        );
        println!(
            "boundary {name} ({low}/{high}): flush {flush_frames}, remainder {tail_max:.3e} < {threshold:.3e}"
        );
    }
}

#[test]
fn band_mask_worst_tail_21hz_hp_bit_exact() {
    // D7(a)-honest: the reviewer's 20 Hz HP is INACTIVE under band_mask_active
    // (low > 20.5 required), which would make the tail test vacuous. Nearest
    // active worst case: 21 Hz HP (+20000 LP) impulse at 48 kHz. Drain
    // completes, flush finite and nontrivial, bit-exact fresh cascade, RBJ at
    // 1e-6, 200k-frame extended probe below peak·2^-24.
    reset_ab087_counters();
    const LOW_HZ: f32 = 21.0;
    const HIGH_HZ: f32 = 20_000.0;
    let input = impulse_input(128);
    let mut plugin = unity_mask_plugin(LOW_HZ, HIGH_HZ);
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut process_output = render_collected(&mut plugin, &input, &[1, 7, 64, 56]);
    assert_ne!(process_output, input, "worst-tail mask must filter");
    let (tail, _) = drain_all_collected_rate_bounded(&mut plugin, input.len() / CHANNELS, 200_000);
    process_output.extend_from_slice(&tail);
    let flush_frames = process_output.len() / CHANNELS - 128;
    assert!(flush_frames > 0, "worst-tail flush must engage");
    let mut wet = input.clone();
    wet.extend(std::iter::repeat_n(0.0, flush_frames * CHANNELS));
    let (low_f64, high_f64) = (f64::from(LOW_HZ), f64::from(HIGH_HZ));
    let cascade = mask_cascade_reference(&wet, CHANNELS, low_f64, high_f64, f64::from(SAMPLE_RATE));
    assert_eq!(process_output, cascade, "worst-tail bit-exact cascade");
    let cookbook =
        mask_cascade_rbj_reference(&wet, CHANNELS, low_f64, high_f64, f64::from(SAMPLE_RATE));
    assert_vectors_close(&process_output, &cookbook, "worst-tail");
    // Impulse peak is exactly 1.0.
    let threshold = MASK_RESIDUAL_RATIO;
    let tail_max = mask_tail_max_beyond(
        &wet,
        CHANNELS,
        low_f64,
        high_f64,
        f64::from(SAMPLE_RATE),
        200_000,
    );
    assert!(
        tail_max < threshold,
        "remainder {tail_max} must stay below {threshold}"
    );
    println!(
        "worst-tail 21/20000 impulse: flush {flush_frames} frames, 200k-probe remainder {tail_max:.3e} < {threshold:.3e}"
    );
}

#[test]
fn band_mask_converter_path_matches_masked_conversion_oracle() {
    // D7(c): path A = production 48→24 nested plus the owned 24→48 converter
    // with an active 500/8000 mask. Wet is the conversion reference plus zero
    // pad (rings pad zeros after content; the mask filters the concatenation),
    // so the whole masked stream matches the fresh cascade bit-for-bit; RBJ
    // at 1e-6; remainder below wet-peak·2^-24; the mask flush extends past the
    // exact-zero ring flush the unmasked path would emit.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let converted = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "resampler".to_owned(),
                parameters: resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            band_mask_low_hz: 500.0,
            band_mask_high_hz: 8_000.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let latency = plugin.latency_samples();
        assert!(latency > 0, "{name}: converter latency must be observed");
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert!(
            whole.len() / CHANNELS > 4096 + latency,
            "{name}: mask flush must extend past the ring flush"
        );
        assert!(
            whole.len() > converted.len(),
            "{name}: masked whole must extend past conversion content"
        );
        let mut wet = converted.clone();
        wet.extend(std::iter::repeat_n(0.0, whole.len() - converted.len()));
        let cascade =
            mask_cascade_reference(&wet, CHANNELS, 500.0, 8_000.0, f64::from(SAMPLE_RATE));
        assert_eq!(whole, cascade, "{name} masked conversion bit-exact");
        let cookbook =
            mask_cascade_rbj_reference(&wet, CHANNELS, 500.0, 8_000.0, f64::from(SAMPLE_RATE));
        assert_vectors_close(&whole, &cookbook, name);
        let wet_peak = converted
            .iter()
            .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
        assert!(wet_peak > 0.0, "{name}: conversion must excite the mask");
        let threshold = wet_peak * MASK_RESIDUAL_RATIO;
        let tail_max = mask_tail_max_beyond(
            &wet,
            CHANNELS,
            500.0,
            8_000.0,
            f64::from(SAMPLE_RATE),
            20_000,
        );
        assert!(
            tail_max < threshold,
            "{name}: remainder {tail_max} must stay below {threshold}"
        );
        println!(
            "converter+mask {name}: whole {} frames (rings end {}), masked flush past conversion {}, remainder {tail_max:.3e} < {threshold:.3e}",
            whole.len() / CHANNELS,
            4096 + latency,
            whole.len() / CHANNELS - converted.len() / CHANNELS,
        );
    }
}

#[test]
fn band_mask_multirate_oracles_match_to_192k() {
    // D7(d)+: active 500/8000 mask at every admitted outer rate (None paths,
    // so no 48 kHz-only fixtures): bit-exact fresh cascade, RBJ at 1e-6,
    // constant-time-span remainder probe below peak·2^-24, flush engages.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 56];
    assert_eq!(chunks.iter().sum::<usize>(), 128);
    for rate in [24_000_u32, 44_100, 48_000, 96_000, 192_000] {
        for (name, input) in [("impulse", impulse_input(128)), ("dense", dense_input(128))] {
            let label = format!("{rate}Hz-{name}");
            let mut plugin = ABComparePlugin::from_params_with_factory(
                CHANNELS,
                rate,
                none_mask_params(500.0, 8_000.0),
                ab087_factory,
            )
            .unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
            let mut process_output = render_collected_at_rate(&mut plugin, &input, &chunks, rate);
            assert_ne!(process_output, input, "{label}: mask must filter");
            let (tail, _) = drain_all_collected_at_rate(&mut plugin, rate);
            process_output.extend_from_slice(&tail);
            let flush_frames = process_output.len() / CHANNELS - 128;
            assert!(flush_frames > 0, "{label}: flush must engage");
            let mut wet = input.clone();
            wet.extend(std::iter::repeat_n(0.0, flush_frames * CHANNELS));
            let cascade = mask_cascade_reference(&wet, CHANNELS, 500.0, 8_000.0, f64::from(rate));
            assert_eq!(process_output, cascade, "{label} bit-exact cascade");
            let cookbook =
                mask_cascade_rbj_reference(&wet, CHANNELS, 500.0, 8_000.0, f64::from(rate));
            assert_vectors_close(&process_output, &cookbook, &label);
            let wet_peak = input
                .iter()
                .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
            let threshold = wet_peak * MASK_RESIDUAL_RATIO;
            // Constant ~0.42 s probe span at every rate.
            let extra = 20_000_usize * (rate as usize) / 48_000;
            let tail_max =
                mask_tail_max_beyond(&wet, CHANNELS, 500.0, 8_000.0, f64::from(rate), extra);
            assert!(
                tail_max < threshold,
                "{label}: remainder {tail_max} must stay below {threshold}"
            );
            println!(
                "mask {label}: flush {flush_frames} frames, probe({extra}) remainder {tail_max:.3e} < {threshold:.3e}"
            );
        }
    }
}

#[test]
fn band_mask_cutoff_rate_corners_refuse_loudly() {
    // Invalid cutoff/rate corners fail construction with the exact gap: low
    // above high; active cutoffs at or beyond Nyquist. Inactive edges
    // at/beyond Nyquist stay exempt (construction succeeds, passthrough).
    reset_ab087_counters();
    let error = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        none_mask_params(5_000.0, 1_000.0),
        ab087_factory,
    )
    .err()
    .expect("low above high must fail construction");
    assert!(error.contains("below high cutoff"), "unexpected: {error}");

    let error = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        8_000,
        none_mask_params(5_000.0, 20_000.0),
        ab087_factory,
    )
    .err()
    .expect("active low at Nyquist must fail construction");
    assert!(error.contains("below Nyquist"), "unexpected: {error}");

    let error = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        8_000,
        none_mask_params(20.0, 5_000.0),
        ab087_factory,
    )
    .err()
    .expect("active high beyond Nyquist must fail construction");
    assert!(error.contains("below Nyquist"), "unexpected: {error}");

    let mut plugin = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        8_000,
        none_mask_params(20.0, 20_000.0),
        ab087_factory,
    )
    .unwrap();
    plugin.initialize(8_000).unwrap();
    let input = dense_input(64);
    let output = render_collected_at_rate(&mut plugin, &input, &[64], 8_000);
    assert_eq!(output, input, "exempt inactive mask passes through");
    let (tail, _) = drain_all_collected_at_rate(&mut plugin, 8_000);
    assert!(tail.is_empty(), "exempt inactive mask drains empty");
}

#[test]
fn silent_variable_production_composes_like_reporting() {
    // The R20 disposition supersedes the silence refusal endpoint: a
    // same-clock bursty child that reports no last-output count still pairs
    // frames by stream position through unpadded host returns in process and
    // actual host drain returns at EOF, so the silent stream composes
    // exactly like its structural reporting twin (reporting burst on A,
    // None on B — same paths, only the reporting channel differs):
    // identical per-call counts, identical whole-stream content,
    // identical drain — and reset-then-rerun reproduces the stream.
    // Adversarial (lying) geometry is refused separately through
    // truthful contract validation.
    reset_ab087_counters();
    let chunks = [10_usize, BURST_CHUNK, 7, 129, 1, 64];
    assert_eq!(chunks.iter().sum::<usize>(), 275);
    let input = dense_input(275);
    let mut silent = make_silent_burst_plugin();
    let mut reporting = make_reporting_burst_plugin();
    let (mut silent_whole, silent_returns) =
        render_collected_captured(&mut silent, &input, &chunks);
    let (mut reporting_whole, reporting_returns) =
        render_collected_captured(&mut reporting, &input, &chunks);
    assert_eq!(
        silent_returns, reporting_returns,
        "silent per-call counts must match the reporting twin exactly",
    );
    assert!(
        silent_returns.iter().any(|&produced| produced > 0),
        "the silent child must actually produce",
    );
    let (silent_tail, silent_partition) = drain_all_collected(&mut silent);
    let (reporting_tail, reporting_partition) = drain_all_collected(&mut reporting);
    silent_whole.extend_from_slice(&silent_tail);
    reporting_whole.extend_from_slice(&reporting_tail);
    assert_eq!(
        silent_partition, reporting_partition,
        "silent drain partition must match the reporting twin",
    );
    // Derived shape (both twins): process emits 256 of 275 frames, so the
    // None path holds 19 retained frames and the burst fixture 19 residual.
    // Drain call 1 pumps 19 + 19 and emits 19 (the retained transfer
    // completes the child only on observation); call 2 observes the
    // empty host's completion handshake. Total 19, exact EOF.
    assert_eq!(
        silent_partition,
        [19_usize, 0],
        "silent drain must pin the derived retained-transfer shape",
    );
    assert_eq!(
        silent_whole, reporting_whole,
        "silent whole-stream content must match the reporting twin",
    );
    assert_eq!(
        silent_whole, input,
        "the composed silent stream preserves every sample",
    );
    println!("silent composition: returns {silent_returns:?}");

    // Reset restores a clean stream that reproduces the same counts.
    silent.reset();
    let (rerun_whole, rerun_returns) = render_collected_captured(&mut silent, &input, &chunks);
    assert_eq!(
        rerun_returns, silent_returns,
        "reset-then-rerun must reproduce the silent counts",
    );
    assert_eq!(
        rerun_whole,
        silent_whole[..rerun_whole.len()],
        "reset-then-rerun must reproduce the silent content",
    );
}

#[test]
fn drain_staging_overflow_refuses_before_child_advance() {
    // Admission side of the fixed-staging contract: a child whose live drain
    // bound exceeds the prepared staging is refused loudly at begin_drain
    // before either child advances (fail closed, never silent truncation).
    reset_ab087_counters();
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params(50_000),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let error = plugin
        .begin_drain(&ProcessContext::new(SAMPLE_RATE, 0))
        .unwrap_err();
    assert!(
        error.contains("staging overflow") && error.contains("exceeds prepared"),
        "unexpected refusal: {error}"
    );
    assert_eq!(BURST_BEGIN_CALLS.get(), 0);
    assert_eq!(BURST_DRAIN_CALLS.get(), 0);
}

#[test]
fn lying_process_declaration_fails_loud_with_measured_evidence() {
    // Adversarial geometry stays refused through truthful contract
    // validation (R20 disposition): a child that under-declares its live
    // bound by one frame and over-reports past staging fails before any
    // emission, naming the node with the measured return-vs-declared
    // evidence. The liar reports (consistent liar), so the capacity guard —
    // not any silence gate — catches it.
    reset_ab087_counters();
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "lie-process".to_owned(),
            parameters: Value::Null,
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = dense_input(64);
    let mut block = vec![f32::NAN; 64 * CHANNELS];
    let error = plugin
        .process(&input, &mut block, &ProcessContext::new(SAMPLE_RATE, 64))
        .unwrap_err();
    assert!(
        error.contains("returned 64 frames") && error.contains("63-frame declared output capacity"),
        "lying process must fail with measured evidence: {error}",
    );
    assert!(
        block.iter().all(|sample| sample.is_nan()),
        "failed process must leave output untouched",
    );
    let error = plugin
        .process(&input, &mut block, &ProcessContext::new(SAMPLE_RATE, 64))
        .unwrap_err();
    assert!(
        error.contains("requires reset"),
        "partial failure must require reset, got: {error}",
    );
}

#[test]
fn lying_drain_declaration_fails_loud_with_measured_evidence() {
    // The drain-side liar is honest in process, then over-reports its drain
    // return past its declared drain capacity. The host drain guard fails
    // loudly with the measured violation; nothing emits.
    reset_ab087_counters();
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "lie-drain".to_owned(),
            parameters: Value::Null,
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = dense_input(64);
    let output = render_collected(&mut plugin, &input, &[64]);
    assert_eq!(output, input, "the drain liar must be honest in process",);
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();
    let mut block = vec![f32::NAN; plugin.drain_output_frames_max() * CHANNELS];
    let error = plugin.drain(&mut block, &context).unwrap_err();
    assert!(
        error.contains("child 0 drain failed") && error.contains("declared output capacity"),
        "lying drain must fail with measured evidence: {error}",
    );
    assert!(
        block.iter().all(|sample| sample.is_nan()),
        "failed drain must leave output untouched",
    );
    let error = plugin.drain(&mut block, &context).unwrap_err();
    assert!(
        error.contains("requires reset"),
        "partial drain failure must require reset, got: {error}",
    );
}

#[test]
fn last_output_frames_reports_actual_emits() {
    reset_ab087_counters();
    let mut plugin = make_burst_plugin(0.0);
    assert_eq!(plugin.last_output_frames(), Some(0));
    let input = dense_input(74);
    let mut block = vec![f32::NAN; 10 * CHANNELS];
    assert_eq!(
        plugin
            .process(
                &input[..10 * CHANNELS],
                &mut block,
                &ProcessContext::new(SAMPLE_RATE, 10)
            )
            .unwrap(),
        0
    );
    assert_eq!(plugin.last_output_frames(), Some(0));
    let mut block = vec![f32::NAN; 64 * CHANNELS];
    assert_eq!(
        plugin
            .process(
                &input[10 * CHANNELS..],
                &mut block,
                &ProcessContext::new(SAMPLE_RATE, 64),
            )
            .unwrap(),
        64
    );
    assert_eq!(plugin.last_output_frames(), Some(64));

    // Silent production composes through unpadded returns and tracks the
    // last successful count exactly like reporting production.
    let mut silent = make_silent_burst_plugin();
    silent.reset();
    assert_eq!(silent.last_output_frames(), Some(0));
    let input = dense_input(72);
    let mut block = vec![f32::NAN; 8 * CHANNELS];
    assert_eq!(
        silent
            .process(
                &input[..8 * CHANNELS],
                &mut block,
                &ProcessContext::new(SAMPLE_RATE, 8)
            )
            .unwrap(),
        0
    );
    assert_eq!(silent.last_output_frames(), Some(0));
    let mut block = vec![f32::NAN; 64 * CHANNELS];
    assert_eq!(
        silent
            .process(
                &input[8 * CHANNELS..],
                &mut block,
                &ProcessContext::new(SAMPLE_RATE, 64)
            )
            .unwrap(),
        64
    );
    assert_eq!(silent.last_output_frames(), Some(64));
}

#[test]
fn variable_paths_are_free_of_callback_heap_activity() {
    reset_ab087_counters();
    // Warm up staging capacities outside the guard, then verify the prepared
    // queued-process, drain, terminal-drain, and reset paths stay silent.
    let mut plugin = make_burst_plugin(0.0);
    let warm = dense_input(129);
    let _ = render_collected(&mut plugin, &warm, &[129]);
    let _ = drain_all_collected(&mut plugin);
    plugin.reset();

    let input = dense_input(200);
    let mut scratch = vec![f32::NAN; 129 * CHANNELS];
    let mut collected = vec![0.0; 200 * CHANNELS];
    let mut cursor = 0_usize;
    let mut drained = 0_usize;
    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    let capacity_frames = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; capacity_frames * CHANNELS];
    assert_no_allocs_or_deallocs("burst queued process, drain, and reset", || {
        let mut offset = 0;
        for &frames in &[7_usize, 64, 129] {
            let produced = plugin
                .process(
                    &input[offset..offset + frames * CHANNELS],
                    &mut scratch[..frames * CHANNELS],
                    &ProcessContext::new(SAMPLE_RATE, frames),
                )
                .unwrap();
            collected[cursor..cursor + produced * CHANNELS]
                .copy_from_slice(&scratch[..produced * CHANNELS]);
            cursor += produced * CHANNELS;
            offset += frames * CHANNELS;
        }
        plugin.begin_drain(&drain_context).unwrap();
        for _ in 0..DRAIN_CALL_LIMIT {
            drain_block.fill(f32::NAN);
            let result = plugin.drain(&mut drain_block, &drain_context).unwrap();
            drained += result.frames;
            if result.complete {
                break;
            }
        }
        let terminal = plugin.drain(&mut [], &drain_context).unwrap();
        assert!(terminal.complete);
        plugin.reset();
    });
    assert_eq!(cursor / CHANNELS + drained, 200);
}

#[test]
fn nested_48_to_24_roundtrip_matches_production_reference() {
    // Original R1: a production 48→24 kHz nested stage (facade-default chunk)
    // plus the owned 24→48 converter reproduces the raw stage-by-stage
    // reference as an exact content prefix (plus exact-zero latency-ring
    // flush) under irregular partitioning, with full EOF.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let expected = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "resampler".to_owned(),
                parameters: resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: nested plus converter latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }

    // The same roundtrip nested in an outer host EOF route: the diligent
    // outer caller pairs actual production with the drain tail exactly.
    let input = dense_input(4096);
    let mut nested =
        ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
    let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
    let mut converter =
        ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
    let expected = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "resampler".to_owned(),
            parameters: resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut nested_plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    nested_plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let nested_latency = nested_plugin.latency_samples();
    let mut outer = DawHost::new(CHANNELS, SAMPLE_RATE);
    outer.add_plugin(Box::new(nested_plugin)).unwrap();
    outer.build().unwrap();
    let mut outer_process = vec![0.0; input.len()];
    let returned = outer.process(&input, &mut outer_process).unwrap();
    let actual = outer.last_output_frames().unwrap_or(returned);
    let mut whole = outer_process[..actual * CHANNELS].to_vec();
    let outer_capacity = outer.drain_output_frames_max();
    let mut block = vec![f32::NAN; outer_capacity * CHANNELS];
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = outer.drain(&mut block).unwrap();
        whole.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            break;
        }
    }
    assert_compensated_whole_stream(&whole, &expected, 4096, nested_latency, "outer-host");
}

#[test]
fn nested_48_to_96_roundtrip_matches_production_reference() {
    // Original R1, upsampling direction: production 48→96 kHz nested plus
    // the owned 96→48 converter, content-prefix bit-exact against the raw
    // reference (plus exact-zero latency-ring flush).
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let expected = run_raw_plugin_to_end(&mut converter, DOUBLE_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "resampler".to_owned(),
                parameters: resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert!(
            plugin.latency_samples() > 0,
            "{name}: nested plus converter latency must be observed"
        );
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the roundtrip");
        }
    }
}

#[test]
fn compensating_resampler_pair_composes_without_converter() {
    // A 48→96→48 production pair already ends at the outer clock, so no
    // converter is appended: content-prefix bit-exactness against the
    // two-stage raw reference proves exactly two stages ran (any third stage
    // would alter bytes). Silent chains compose through the same unpadded
    // returns (see the silent/reporting twin test); adversarial geometry is
    // refused separately through truthful contract validation.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        let mut up =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let mid = run_raw_plugin_to_end(&mut up, SAMPLE_RATE, &input);
        let mut down =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let expected = run_raw_plugin_to_end(&mut down, DOUBLE_RATE, &mid);

        let params = ABComparePluginParams {
            path_a: PathConfig::Rack {
                plugins: vec![
                    PluginInRack {
                        plugin_type: "resampler".to_owned(),
                        parameters: resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
                    },
                    PluginInRack {
                        plugin_type: "resampler".to_owned(),
                        parameters: resampler_params(DOUBLE_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
                    },
                ],
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let mut whole = render_collected(&mut plugin, &input, &chunks);
        let (tail, _) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_compensated_whole_stream(&whole, &expected, 4096, plugin.latency_samples(), name);
        if name == "constant" {
            let frames = expected.len() / CHANNELS;
            let middle = &expected[frames / 4 * CHANNELS..3 * frames / 4 * CHANNELS];
            assert!(middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3));
        }
        if name == "impulse" {
            let peak = expected
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.05, "impulse energy must survive the pair");
        }
    }
}

#[test]
fn converted_paths_pin_latency_counts_and_exact_eof() {
    // Stage A converter oracles: converted paths pin their exact AB latency
    // derived from raw stage latencies (no `> 0` vagueness), conserve counts
    // across process plus drain, and end EOF with a post-complete drain that
    // stays complete with zero frames. Both rate directions, irregular
    // chunks, constant/impulse/dense audio.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 1669, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 2048);
    for (name, input) in [
        ("constant", constant_input(2048, 0.25)),
        ("impulse", impulse_input(2048)),
        ("dense", dense_input(2048)),
    ] {
        // Down direction: zero-latency decimator fixture plus the owned
        // converter. Expected latency is the converter latency alone on the
        // exact 48 kHz lattice.
        let mut decimator = DecimatorTwoFixture::new(CHANNELS).unwrap();
        assert_eq!(decimator.latency_samples(), 0);
        let mid = run_raw_plugin_to_end(&mut decimator, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let down_latency = converter.latency_samples();
        let expected = run_raw_plugin_to_end(&mut converter, HALF_RATE, &mid);
        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "decimate-2".to_owned(),
                parameters: Value::Null,
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert_eq!(
            plugin.latency_samples(),
            down_latency,
            "{name}: down-converted latency must equal the converter latency",
        );
        let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
        let (tail, partition) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_eq!(
            returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
            whole.len() / CHANNELS,
            "{name} down count conservation",
        );
        assert_compensated_whole_stream(&whole, &expected, 2048, down_latency, name);
        let context = ProcessContext::new(SAMPLE_RATE, 0);
        let terminal = plugin.drain(&mut [], &context).unwrap();
        assert!(
            terminal.complete && terminal.frames == 0,
            "{name}: down post-complete drain must stay complete with zero frames",
        );

        // Up direction: production nested upsampler plus the owned converter.
        // Expected latency converts both stage latencies to the 48 kHz
        // lattice with the host's exact ceiling division.
        let mut nested =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let nested_latency = nested.latency_samples();
        let mid = run_raw_plugin_to_end(&mut nested, SAMPLE_RATE, &input);
        let mut converter =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let converter_latency = converter.latency_samples();
        let expected = run_raw_plugin_to_end(&mut converter, DOUBLE_RATE, &mid);
        let up_latency = (nested_latency + 2 * converter_latency).div_ceil(2);
        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "resampler".to_owned(),
                parameters: resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
            },
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert_eq!(
            plugin.latency_samples(),
            up_latency,
            "{name}: up-converted latency must match the derived ceiling value",
        );
        let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
        let (tail, partition) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_eq!(
            returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
            whole.len() / CHANNELS,
            "{name} up count conservation",
        );
        assert_compensated_whole_stream(&whole, &expected, 2048, up_latency, name);
        let terminal = plugin.drain(&mut [], &context).unwrap();
        assert!(
            terminal.complete && terminal.frames == 0,
            "{name}: up post-complete drain must stay complete with zero frames",
        );
        println!("converted pins {name}: down {down_latency}, up {up_latency}");
    }
}

#[test]
fn asymmetric_converted_paths_pair_by_stream_position() {
    // Stage B unequal-length composition: a 48→24→48 path and a 48→96→48
    // path (different latencies, different chunk bursts) pair by stream
    // position under irregular callbacks, and EOF accounts each path
    // separately — the shorter path contributes exact zeros after its
    // exhaustion while the longer path's tail still emits, so the whole
    // stream reaches input plus the MAXIMUM path latency. Both paths stay
    // audible (mix 0.0) so pairing is content-proven, not just counted.
    reset_ab087_counters();
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 3717, 40];
    assert_eq!(chunks.iter().sum::<usize>(), 4096);
    for (name, input) in [
        ("constant", constant_input(4096, 0.25)),
        ("impulse", impulse_input(4096)),
        ("dense", dense_input(4096)),
    ] {
        // Raw per-path references with exact derived path latencies.
        let mut down =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK).unwrap();
        let down_latency = down.latency_samples();
        let mid_a = run_raw_plugin_to_end(&mut down, SAMPLE_RATE, &input);
        let mut conv_a =
            ResamplerPlugin::new(CHANNELS, HALF_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let conv_a_latency = conv_a.latency_samples();
        let ref_a = run_raw_plugin_to_end(&mut conv_a, HALF_RATE, &mid_a);
        let latency_a = 2 * down_latency + conv_a_latency;

        let mut up =
            ResamplerPlugin::new(CHANNELS, SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK).unwrap();
        let up_latency = up.latency_samples();
        let mid_b = run_raw_plugin_to_end(&mut up, SAMPLE_RATE, &input);
        let mut conv_b =
            ResamplerPlugin::new(CHANNELS, DOUBLE_RATE, SAMPLE_RATE, CONVERTER_CHUNK_PIN).unwrap();
        let conv_b_latency = conv_b.latency_samples();
        let ref_b = run_raw_plugin_to_end(&mut conv_b, DOUBLE_RATE, &mid_b);
        let latency_b = (up_latency + 2 * conv_b_latency).div_ceil(2);

        // Asymmetry guard: identical latencies would make this test vacuous.
        assert_ne!(
            latency_a, latency_b,
            "converted legs must have unequal latencies",
        );
        let max_latency = latency_a.max(latency_b);
        let (delay_a, delay_b) = (max_latency - latency_a, max_latency - latency_b);

        let params = ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "resampler".to_owned(),
                parameters: resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            },
            path_b: PathConfig::Plugin {
                plugin_type: "resampler".to_owned(),
                parameters: resampler_params(SAMPLE_RATE, DOUBLE_RATE, NESTED_SRC_CHUNK),
            },
            mix: 0.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert_eq!(
            plugin.latency_samples(),
            max_latency,
            "{name}: AB latency must be the maximum path latency",
        );
        let (mut whole, returns) = render_collected_captured(&mut plugin, &input, &chunks);
        let (tail, partition) = drain_all_collected(&mut plugin);
        whole.extend_from_slice(&tail);
        assert_eq!(
            returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
            whole.len() / CHANNELS,
            "{name} count conservation",
        );
        // Exact outer EOF at input plus the MAXIMUM latency: emission must
        // not stop at the shorter path's exhaustion.
        assert_eq!(
            whole.len() / CHANNELS,
            4096 + max_latency,
            "{name} asymmetric compensated length",
        );

        // Delay-compensated mix oracle: each path reference shifts by its
        // alignment delay and zero-extends past exhaustion (the shorter
        // path contributes exact zeros while the longer tail still emits),
        // mixed 0.5/0.5 in the mixer's exact f32 operation order.
        let total_frames = 4096 + max_latency;
        let frames_a = ref_a.len() / CHANNELS;
        let frames_b = ref_b.len() / CHANNELS;
        let mut expected = Vec::with_capacity(total_frames * CHANNELS);
        for frame in 0..total_frames {
            for channel in 0..CHANNELS {
                let sample_a = if frame >= delay_a && frame - delay_a < frames_a {
                    ref_a[(frame - delay_a) * CHANNELS + channel]
                } else {
                    0.0
                };
                let sample_b = if frame >= delay_b && frame - delay_b < frames_b {
                    ref_b[(frame - delay_b) * CHANNELS + channel]
                } else {
                    0.0
                };
                expected.push(sample_a * 0.5 + sample_b * 0.5);
            }
        }
        assert_vectors_close(&whole, &expected, name);
        println!(
            "asymmetric {name}: latencies {latency_a}/{latency_b}, drain partition {partition:?}",
        );
        let context = ProcessContext::new(SAMPLE_RATE, 0);
        let terminal = plugin.drain(&mut [], &context).unwrap();
        assert!(
            terminal.complete && terminal.frames == 0,
            "{name}: post-complete drain must stay complete with zero frames",
        );
        if name == "constant" {
            // Both legs pass DC near unity, so the mixed interior holds
            // 0.25: a dead leg would read ~0.125 instead. The interior
            // starts past alignment plus settling and ends inside the fed
            // input span, both derived from the maximum path latency.
            let start_frame = max_latency + 512;
            let end_frame = 4096_usize.saturating_sub(512);
            assert!(
                start_frame < end_frame,
                "constant interior needs headroom past latency {max_latency}",
            );
            let middle = &whole[start_frame * CHANNELS..end_frame * CHANNELS];
            assert!(
                middle.iter().all(|sample| (sample - 0.25).abs() <= 1.0e-3),
                "mixed constant interior must hold 0.25 from both legs",
            );
        }
        if name == "impulse" {
            let peak = whole
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            assert!(peak > 0.025, "impulse energy must survive the mixed legs");
        }
    }
}

#[test]
fn variable_diamond_envelope_covers_cold_and_warm_real_drain() {
    reset_ab087_counters();
    // R11 real-backend envelope proof on the acceptance diamond geometry
    // (mirrors `variable_diamond_lossless_join_matches_retention_oracle`):
    // every node publishes honest envelopes (production resamplers,
    // chunked burst, identity gains), so the nested hosts prepare once
    // and the whole cold drain — retention transfer, session freeze, and
    // every round — allocates nothing. Warm (post-reset) repeats the
    // proof with an unchanged envelope, and the whole stream still
    // matches the retention oracle bit-for-bit (envelope scheduling
    // changes round chunking, never content).
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 7813, 77];
    assert_eq!(chunks.iter().sum::<usize>(), 8229);
    let diamond = PathConfig::Graph {
        nodes: vec![
            graph_node("source", "zgain", json!({ "gain": 0.5 })),
            graph_node(
                "branch-a-down",
                "resampler",
                resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node(
                "branch-a-up",
                "resampler",
                resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node("branch-b", "burst", burst_params(BURST_CHUNK)),
            graph_node("sink", "zgain", json!({ "gain": 2.0 })),
        ],
        edges: vec![
            graph_edge("source", "branch-a-down"),
            graph_edge("branch-a-down", "branch-a-up"),
            graph_edge("source", "branch-b"),
            graph_edge("branch-a-up", "sink"),
            graph_edge("branch-b", "sink"),
        ],
    };
    let input = constant_input(8229, 0.25);

    // Independent oracle whole-stream reference (activation intent stays
    // with the acceptance test; here the oracle guards content only).
    let mut oracle = RetainedDiamondOracle::new(0.5, 2.0);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &chunks {
        expected.extend_from_slice(
            &oracle.feed_block(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
        );
        offset += frames;
    }
    let process_frames = expected.len() / CHANNELS;
    expected.extend_from_slice(&oracle.finish());
    let drain_frames = expected.len() / CHANNELS - process_frames;
    assert!(drain_frames > 0, "drain must carry tail content");

    let params = ABComparePluginParams {
        path_a: diamond,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();

    // Process envelope: AB emits at most one frame per input frame on
    // every path, fresh and mid-stream alike.
    for &size in &[1usize, 64, 7813, 8192] {
        assert_eq!(
            plugin.output_frames_envelope(size),
            Some(size),
            "AB process envelope must bound every path by the input count"
        );
    }

    // Cold stream: render outside the counter (first-touch block sizing
    // is legitimate), then the whole drain inside it.
    let mut whole = render_collected(&mut plugin, &input, &chunks);
    let cold_envelope = plugin
        .drain_frames_envelope()
        .expect("all-envelope diamond must publish a drain envelope");
    let live_bound = plugin.drain_output_frames_max();
    assert!(
        live_bound <= cold_envelope,
        "live drain bound {live_bound} exceeds envelope {cold_envelope}"
    );
    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    // Staging sized once from the live bound outside the counter; the
    // envelope only prepares host internals (no caller behavior change).
    let mut drain_block = vec![f32::NAN; live_bound * CHANNELS];
    let mut cold_calls = 0usize;
    let mut cold_drained = 0usize;
    let mut tail = Vec::new();
    // Collection grows the harness Vec outside the counter; every plugin
    // call (begin, each drain step, terminal) is counter-proven alone.
    assert_no_allocs_or_deallocs("cold begin drain", || {
        plugin.begin_drain(&drain_context).unwrap();
    });
    let mut cold_complete = false;
    for _ in 0..DRAIN_CALL_LIMIT {
        let mut result = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("cold real diamond drain", || {
            drain_block.fill(f32::NAN);
            result = plugin.drain(&mut drain_block, &drain_context).unwrap();
        });
        assert!(
            result.frames <= live_bound,
            "drain emitted {} past its {live_bound}-frame bound",
            result.frames
        );
        assert!(
            drain_block[result.frames * CHANNELS..]
                .iter()
                .all(|sample| sample.is_nan()),
            "drain output past {} frames must stay untouched",
            result.frames
        );
        tail.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
        cold_calls += 1;
        cold_drained += result.frames;
        if result.complete {
            cold_complete = true;
            break;
        }
    }
    assert!(
        cold_complete,
        "cold drain must complete within its call bound"
    );
    assert_no_allocs_or_deallocs("cold terminal drain", || {
        let terminal = plugin.drain(&mut [], &drain_context).unwrap();
        assert!(terminal.complete, "terminal drain must stay complete");
    });
    assert!(cold_drained > 0, "cold drain must carry tail content");
    whole.extend_from_slice(&tail);
    assert!(
        whole.len() >= expected.len(),
        "cold: plugin emitted {} frames, oracle expects {}",
        whole.len() / CHANNELS,
        expected.len() / CHANNELS,
    );
    assert_eq!(
        &whole[..expected.len()],
        &expected[..],
        "cold lossless whole-stream content"
    );
    assert!(
        whole[expected.len()..].iter().all(|sample| *sample == 0.0),
        "cold: surplus past the oracle must be the zero ring flush"
    );

    // Warm stream: reset, re-render, and drain under the counter again
    // with an unchanged envelope.
    plugin.reset();
    let mut whole = render_collected(&mut plugin, &input, &chunks);
    let warm_envelope = plugin
        .drain_frames_envelope()
        .expect("envelope must survive reset");
    assert_eq!(
        warm_envelope, cold_envelope,
        "drain envelope must be stream-independent across reset"
    );
    let live_bound = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; live_bound * CHANNELS];
    let mut warm_calls = 0usize;
    let mut warm_drained = 0usize;
    let mut tail = Vec::new();
    assert_no_allocs_or_deallocs("warm begin drain", || {
        plugin.begin_drain(&drain_context).unwrap();
    });
    let mut warm_complete = false;
    for _ in 0..DRAIN_CALL_LIMIT {
        let mut result = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("warm real diamond drain", || {
            drain_block.fill(f32::NAN);
            result = plugin.drain(&mut drain_block, &drain_context).unwrap();
        });
        assert!(result.frames <= live_bound);
        tail.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
        warm_calls += 1;
        warm_drained += result.frames;
        if result.complete {
            warm_complete = true;
            break;
        }
    }
    assert!(
        warm_complete,
        "warm drain must complete within its call bound"
    );
    assert!(warm_drained > 0, "warm drain must carry tail content");
    whole.extend_from_slice(&tail);
    assert_eq!(
        &whole[..expected.len()],
        &expected[..],
        "warm lossless whole-stream content"
    );
    println!(
        "envelope diamond: cold drain {cold_drained} frames in {cold_calls} calls, \
         warm drain {warm_drained} frames in {warm_calls} calls, envelope {cold_envelope}, \
         oracle {drain_frames} drain frames, bitwise match",
    );
}

#[test]
fn variable_diamond_cold_process_allocates_nothing() {
    reset_ab087_counters();
    // R12: the cold FIRST process call after completed construction and
    // initialization must allocate zero (build-time MAX_BLOCK
    // preparation; no callback prewarming, no counter moved past first
    // touch). The counter starts before the first process call and covers
    // the whole cold render (including the near-max 7813 block),
    // capacity queries, and the begin/drain lifecycle; reset repeats the
    // full lifecycle warm. Same diamond geometry and oracle as the
    // acceptance test (mirrored, not shared, to avoid churning it).
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 7813, 77];
    assert_eq!(chunks.iter().sum::<usize>(), 8229);
    let diamond = PathConfig::Graph {
        nodes: vec![
            graph_node("source", "zgain", json!({ "gain": 0.5 })),
            graph_node(
                "branch-a-down",
                "resampler",
                resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node(
                "branch-a-up",
                "resampler",
                resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node("branch-b", "burst", burst_params(BURST_CHUNK)),
            graph_node("sink", "zgain", json!({ "gain": 2.0 })),
        ],
        edges: vec![
            graph_edge("source", "branch-a-down"),
            graph_edge("branch-a-down", "branch-a-up"),
            graph_edge("source", "branch-b"),
            graph_edge("branch-a-up", "sink"),
            graph_edge("branch-b", "sink"),
        ],
    };
    let input = constant_input(8229, 0.25);

    let mut oracle = RetainedDiamondOracle::new(0.5, 2.0);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &chunks {
        expected.extend_from_slice(
            &oracle.feed_block(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
        );
        offset += frames;
    }
    let process_frames = expected.len() / CHANNELS;
    expected.extend_from_slice(&oracle.finish());
    let drain_frames = expected.len() / CHANNELS - process_frames;
    assert!(drain_frames > 0, "drain must carry tail content");

    let params = ABComparePluginParams {
        path_a: diamond,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();

    // Capacity queries under the counter, before any process call.
    assert_no_allocs_or_deallocs("cold diamond capacity queries", || {
        for &size in &[1usize, 64, 7813, 8192] {
            assert_eq!(plugin.output_frames_envelope(size), Some(size));
            assert_eq!(plugin.output_frames_for_input(size), size);
        }
        let live = plugin.drain_output_frames_max();
        assert!(live <= plugin.drain_frames_envelope().unwrap());
    });

    // Cold render: manual loop (the collected helper grows harness Vecs).
    // Block buffers are NaN-poisoned and sized outside the counter.
    let mut whole = Vec::new();
    let mut frame_offset = 0;
    for &frames in &chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let mut produced = 0;
        assert_no_allocs_or_deallocs("cold diamond process", || {
            produced = plugin
                .process(&input[start..end], &mut block, &context)
                .unwrap();
        });
        assert!(
            produced <= frames,
            "plugin emitted {produced} frames for a {frames}-frame block"
        );
        whole.extend_from_slice(&block[..produced * CHANNELS]);
        assert!(
            block[produced * CHANNELS..]
                .iter()
                .all(|sample| sample.is_nan()),
            "output beyond the returned {produced} frames must stay untouched"
        );
        frame_offset += frames;
    }
    assert_eq!(
        whole.len() / CHANNELS,
        process_frames,
        "cold render must produce the oracle frame count"
    );

    // Cold drain lifecycle under per-call counters.
    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    let live_bound = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; live_bound * CHANNELS];
    assert_no_allocs_or_deallocs("cold diamond begin drain", || {
        plugin.begin_drain(&drain_context).unwrap();
    });
    let mut cold_complete = false;
    let mut cold_calls = 0usize;
    for _ in 0..DRAIN_CALL_LIMIT {
        let mut result = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("cold diamond drain", || {
            drain_block.fill(f32::NAN);
            result = plugin.drain(&mut drain_block, &drain_context).unwrap();
        });
        assert!(result.frames <= live_bound);
        whole.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
        cold_calls += 1;
        if result.complete {
            cold_complete = true;
            break;
        }
    }
    assert!(
        cold_complete,
        "cold drain must complete within its call bound"
    );
    assert!(
        whole.len() >= expected.len(),
        "cold: plugin emitted {} frames, oracle expects {}",
        whole.len() / CHANNELS,
        expected.len() / CHANNELS,
    );
    assert_eq!(
        &whole[..expected.len()],
        &expected[..],
        "cold lossless whole-stream content"
    );
    assert!(
        whole[expected.len()..].iter().all(|sample| *sample == 0.0),
        "cold: surplus past the oracle must be the zero ring flush"
    );

    // Warm lifecycle: reset keeps prepared capacities, so the second
    // cold start (first post-reset process) allocates nothing either.
    plugin.reset();
    let mut whole = Vec::new();
    let mut frame_offset = 0;
    for &frames in &chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let mut produced = 0;
        assert_no_allocs_or_deallocs("warm diamond process", || {
            produced = plugin
                .process(&input[start..end], &mut block, &context)
                .unwrap();
        });
        assert!(produced <= frames);
        whole.extend_from_slice(&block[..produced * CHANNELS]);
        frame_offset += frames;
    }
    let live_bound = plugin.drain_output_frames_max();
    let mut drain_block = vec![f32::NAN; live_bound * CHANNELS];
    assert_no_allocs_or_deallocs("warm diamond begin drain", || {
        plugin.begin_drain(&drain_context).unwrap();
    });
    let mut warm_complete = false;
    let mut warm_calls = 0usize;
    for _ in 0..DRAIN_CALL_LIMIT {
        let mut result = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("warm diamond drain", || {
            drain_block.fill(f32::NAN);
            result = plugin.drain(&mut drain_block, &drain_context).unwrap();
        });
        assert!(result.frames <= live_bound);
        whole.extend_from_slice(&drain_block[..result.frames * CHANNELS]);
        warm_calls += 1;
        if result.complete {
            warm_complete = true;
            break;
        }
    }
    assert!(
        warm_complete,
        "warm drain must complete within its call bound"
    );
    assert_eq!(
        &whole[..expected.len()],
        &expected[..],
        "warm lossless whole-stream content"
    );
    println!(
        "cold diamond: render {process_frames} frames plus drain in {cold_calls} calls cold / \
         {warm_calls} calls warm, zero callback allocations, bitwise match",
    );
}

#[test]
fn fork_odd_chunk_straddle_prepares_full_straddle_need() {
    reset_ab087_counters();
    // F1 falsifier (R14 red-before: production sizing is NOT yet fixed, so
    // the second block is predicted to grow host scratch; R15 turns it
    // green). Fork with no merges, so no envelope join scratch covers the
    // shared term: a unity-gain source fans out to twin 32-channel real
    // resamplers (48 to 96 kHz, chunk 1000). Chunk 1000 is legal (only
    // `!= 0` is checked) and does not divide MAX_BLOCK 8192, so block
    // 1999 walks each residual 0 -> 999 and the 8192 call declares nine
    // block-maxima against eight prepared. The x32 scratch
    // overprovisioning covers small channel counts, so 32 channels are
    // load-bearing: per-call need 9*M*32 exceeds prepared 8*M*32.
    const FORK_CHANNELS: usize = 32;
    const CHUNK: usize = 1000;
    // Fork block maximum from the vendored fork formula
    // (FixedAsync::Input: ceil((chunk + max_step) * max_ratio) + 1 with
    // max_ratio 4.0 and max_step 1.0 at nominal 2.0): 4005 by exact
    // integer arithmetic. The probe below pins it loudly on fork drift.
    const BLOCK_MAX: usize = 4005;
    // Red-before baseline (R15 measured 1025280 prepared): the R16
    // envelope sizing prepares above the straddle need instead. This
    // integration test cannot observe actual host scratch (crate
    // internals), so the print below labels this historical value
    // honestly rather than claiming it as current capacity.
    const PRE_FIX_PREPARED_SAMPLES: usize = 8 * BLOCK_MAX * FORK_CHANNELS;
    const STRADDLE_SAMPLES: usize = 9 * BLOCK_MAX * FORK_CHANNELS;
    assert_eq!(PRE_FIX_PREPARED_SAMPLES, 1_025_280);
    assert_eq!(STRADDLE_SAMPLES, 1_153_440);

    // Probe the block maximum on a sacrificial twin (same construction as
    // the graph branches): one full chunk at residual zero declares M.
    let probe = ResamplerPlugin::new(FORK_CHANNELS, SAMPLE_RATE, DOUBLE_RATE, CHUNK).unwrap();
    assert_eq!(
        probe.output_frames_for_input(CHUNK),
        BLOCK_MAX,
        "chunk-1000 48-to-96 kHz block maximum must match the fork formula"
    );

    let mut host = DawHost::new(FORK_CHANNELS, SAMPLE_RATE);
    // GainPlugin is a ParametricPlugin: the established adapter carries
    // it onto the Plugin surface (live n, envelope Some(n)); its process
    // forwards to the same buffers, so 0 dB stays bit-transparent.
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(ParametricPluginAdapter::new(GainPlugin::new(
                FORK_CHANNELS,
                0.0,
            ))),
        )
        .unwrap();
    let up_a = host
        .add_node(
            "up-a".to_string(),
            Box::new(ResamplerPlugin::new(FORK_CHANNELS, SAMPLE_RATE, DOUBLE_RATE, CHUNK).unwrap()),
        )
        .unwrap();
    let up_b = host
        .add_node(
            "up-b".to_string(),
            Box::new(ResamplerPlugin::new(FORK_CHANNELS, SAMPLE_RATE, DOUBLE_RATE, CHUNK).unwrap()),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, up_a)).unwrap();
    host.add_edge(GraphEdge::new(source, up_b)).unwrap();
    host.build().unwrap();

    // Legal geometry, by hand: both outputs resolve to 96 kHz, and the
    // build-state live declaration at MAX_BLOCK is eight full chunks.
    assert_eq!(
        host.output_sample_rate(SAMPLE_RATE).unwrap(),
        f64::from(DOUBLE_RATE),
        "fork outputs must resolve to 96 kHz"
    );
    assert_eq!(
        host.output_frames_for_input(8192),
        8 * BLOCK_MAX,
        "build-state live declaration must be eight chunks"
    );
    // The envelope already covers the straddle peak: only the shared
    // process scratch term is live-sized. That is the F1 claim, precisely.
    assert_eq!(
        host.output_frames_envelope(8192),
        Some(9 * BLOCK_MAX),
        "process envelope must cover the nine-chunk straddle"
    );

    // Standalone reference twin: identical construction and call sequence,
    // fed directly. The unity source is bit-transparent (0 dB is exactly
    // 1.0, settled from construction), so each branch sees this input.
    let mut reference =
        ResamplerPlugin::new(FORK_CHANNELS, SAMPLE_RATE, DOUBLE_RATE, CHUNK).unwrap();
    reference.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input1 = vec![0.25f32; 1999 * FORK_CHANNELS];
    let input2 = vec![0.25f32; 8192 * FORK_CHANNELS];
    // Reference-side chunk arithmetic first: one chunk completes on block
    // 1 (residual 999), nine complete on block 2. This independently proves
    // the residual walk before any host assertion uses it.
    let ref_live1 = reference.output_frames_for_input(1999);
    assert_eq!(ref_live1, BLOCK_MAX, "block 1 must declare one chunk");
    let mut ref_out1 = vec![f32::NAN; ref_live1 * FORK_CHANNELS];
    let ref_produced1 = reference
        .process(
            &input1,
            &mut ref_out1,
            &ProcessContext::new(SAMPLE_RATE, 1999),
        )
        .unwrap();
    let ref_live2 = reference.output_frames_for_input(8192);
    assert_eq!(
        ref_live2,
        9 * BLOCK_MAX,
        "residual 999 must straddle nine chunks"
    );
    let mut ref_out2 = vec![f32::NAN; ref_live2 * FORK_CHANNELS];
    let ref_produced2 = reference
        .process(
            &input2,
            &mut ref_out2,
            &ProcessContext::new(SAMPLE_RATE, 8192),
        )
        .unwrap();
    assert!(
        ref_produced1 > 0 && ref_produced2 > 0,
        "reference must genuinely convert (got {ref_produced1} + {ref_produced2} frames)"
    );

    // Block 1 fits prepared scratch: one chunk declared at residual zero.
    let live1 = host.output_frames_for_input(1999);
    assert_eq!(live1, BLOCK_MAX, "host block 1 must declare one chunk");
    let mut out1 = vec![f32::NAN; live1 * FORK_CHANNELS];
    let mut produced1 = 0;
    let (allocs1, deallocs1) = measure_heap_activity(|| {
        produced1 = host.process(&input1, &mut out1).unwrap();
    });
    println!(
        "fork straddle block 1999: live {live1}, produced {produced1}, \
         heap ({allocs1}, {deallocs1})",
    );
    assert_eq!(
        (allocs1, deallocs1),
        (0, 0),
        "first block must fit prepared scratch"
    );
    assert_eq!(
        produced1, ref_produced1,
        "host block 1 must produce the reference count"
    );
    let reference1 = &ref_out1[..ref_produced1 * FORK_CHANNELS];
    for (index, &sample) in reference1.iter().enumerate() {
        assert_eq!(
            out1[index],
            sample + sample,
            "host block 1 must sum the twin branches bit-exactly at sample {index}"
        );
    }

    // Block 2 is the armed corner: residual 999 declares nine chunks.
    let live2 = host.output_frames_for_input(8192);
    assert_eq!(
        live2,
        9 * BLOCK_MAX,
        "host block 2 must declare the nine-chunk straddle"
    );
    let mut out2 = vec![f32::NAN; live2 * FORK_CHANNELS];
    let mut produced2 = 0;
    let (allocs2, deallocs2) = measure_heap_activity(|| {
        produced2 = host.process(&input2, &mut out2).unwrap();
    });
    println!(
        "fork straddle block 8192: live {live2}, produced {produced2}, \
         heap ({allocs2}, {deallocs2}), pre-fix baseline {PRE_FIX_PREPARED_SAMPLES} samples, \
         need {STRADDLE_SAMPLES} samples",
    );
    // Red-before: pre-fix scratch holds 8*M*32 samples and this call needs
    // 9*M*32, so the counter fires here with correct audio. R16 production
    // sizing (envelope-or-live shared scratch) turns this green.
    assert_eq!(
        (allocs2, deallocs2),
        (0, 0),
        "straddle block must fit prepared scratch"
    );
    assert!(produced2 <= live2, "host must honor its live declaration");
    assert_eq!(
        produced2, ref_produced2,
        "host block 2 must produce the reference count"
    );
    let reference2 = &ref_out2[..ref_produced2 * FORK_CHANNELS];
    for (index, &sample) in reference2.iter().enumerate() {
        assert_eq!(
            out2[index],
            sample + sample,
            "host block 2 must sum the twin branches bit-exactly at sample {index}"
        );
    }
}

#[test]
fn envelopes_dominate_live_declarations_across_ab_states() {
    reset_ab087_counters();
    // F2: the envelope contract requires envelope(n) >= live(n) in every
    // stream state (hosts size graph-drain destinations from live
    // declarations against envelope-sized holdovers). AB publishes
    // process Some(n) over a default live n, and a drain mirror over the
    // state-dependent live capacity (nested host bounds plus queue/mask
    // progress floors). Pin both across fresh, fed, draining, and reset
    // states on two legs: a queued diamond (staging queues fill, nested
    // bounds walk with residuals) and a mask-active unity pair (mask
    // history engages the mask floor, proven flush drains). The 0/1
    // floors lift to the envelope's clamp floor of 1 by construction;
    // sampling the aggregate invariant across these genuine states is
    // what keeps the check behavioral rather than textual.
    let check = |plugin: &ABComparePlugin, leg: &str, state: &str| {
        for &size in &[0usize, 1, 64, 7813, 8192] {
            assert_eq!(
                plugin.output_frames_for_input(size),
                size,
                "{leg} {state}: live process declaration must stay identity"
            );
            assert_eq!(
                plugin.output_frames_envelope(size),
                Some(size),
                "{leg} {state}: process envelope must stay identity"
            );
        }
        let live = plugin.drain_output_frames_max();
        let envelope = plugin
            .drain_frames_envelope()
            .unwrap_or_else(|| panic!("{leg} {state}: leg must publish a drain envelope"));
        assert!(
            live <= envelope,
            "{leg} {state}: live drain bound {live} exceeds envelope {envelope}"
        );
        (live, envelope)
    };
    let feed = |plugin: &mut ABComparePlugin, leg: &str, input: &[f32], chunks: &[usize]| {
        let mut offset = 0;
        let mut produced_total = 0;
        for &frames in chunks {
            let mut block = vec![f32::NAN; frames * CHANNELS];
            let produced = plugin
                .process(
                    &input[offset * CHANNELS..(offset + frames) * CHANNELS],
                    &mut block,
                    &ProcessContext::new(SAMPLE_RATE, frames),
                )
                .unwrap();
            assert!(
                produced <= frames,
                "{leg}: plugin emitted {produced} frames for a {frames}-frame block"
            );
            produced_total += produced;
            check(plugin, leg, "fed");
            offset += frames;
        }
        assert!(produced_total > 0, "{leg}: fed stream must carry content");
    };
    let drain_checked = |plugin: &mut ABComparePlugin, leg: &str| {
        let context = ProcessContext::new(SAMPLE_RATE, 0);
        plugin.begin_drain(&context).unwrap();
        check(plugin, leg, "draining");
        let capacity_frames = plugin.drain_output_frames_max();
        let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
        let mut drained = 0;
        for _ in 0..DRAIN_CALL_LIMIT {
            block.fill(f32::NAN);
            let result = plugin.drain(&mut block, &context).unwrap();
            assert!(result.frames <= capacity_frames);
            check(plugin, leg, "draining");
            drained += result.frames;
            if result.complete {
                assert!(drained > 0, "{leg}: drain must carry tail content");
                return;
            }
        }
        panic!("{leg}: drain exceeded the test's bounded call allowance");
    };

    // Leg 1: queued diamond (same geometry as the acceptance test).
    let diamond = PathConfig::Graph {
        nodes: vec![
            graph_node("source", "zgain", json!({ "gain": 0.5 })),
            graph_node(
                "branch-a-down",
                "resampler",
                resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node(
                "branch-a-up",
                "resampler",
                resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
            ),
            graph_node("branch-b", "burst", burst_params(BURST_CHUNK)),
            graph_node("sink", "zgain", json!({ "gain": 2.0 })),
        ],
        edges: vec![
            graph_edge("source", "branch-a-down"),
            graph_edge("branch-a-down", "branch-a-up"),
            graph_edge("source", "branch-b"),
            graph_edge("branch-a-up", "sink"),
            graph_edge("branch-b", "sink"),
        ],
    };
    let mut plugin = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        ABComparePluginParams {
            path_a: diamond,
            path_b: PathConfig::None,
            mix: -1.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        },
        ab087_factory,
    )
    .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let (_, diamond_envelope) = check(&plugin, "diamond", "fresh");
    // Acceptance stream, not a short probe: the diamond's 1024-frame
    // converter chunks plus the 3262-frame compensation prefill retain
    // short streams entirely (correctly), so invariant sampling needs
    // the proven 8229-frame stream that clears both gates.
    let chunks = [1_usize, 7, 64, 63, 65, 137, 2, 7813, 77];
    assert_eq!(chunks.iter().sum::<usize>(), 8229);
    feed(&mut plugin, "diamond", &dense_input(8229), &chunks);
    let (diamond_fed_live, fed_envelope) = check(&plugin, "diamond", "fed");
    assert_eq!(
        fed_envelope, diamond_envelope,
        "diamond drain envelope must be stream-independent"
    );
    assert!(
        diamond_fed_live > 0,
        "fed diamond must carry live nested bounds"
    );
    drain_checked(&mut plugin, "diamond");
    plugin.reset();
    let (_, diamond_warm) = check(&plugin, "diamond", "reset");
    assert_eq!(
        diamond_warm, diamond_envelope,
        "diamond drain envelope must survive reset"
    );

    // Leg 2: mask-active unity pair (proven residual flush).
    let mut masked = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        ABComparePluginParams {
            path_a: PathConfig::Plugin {
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 1.0 }),
            },
            path_b: PathConfig::Plugin {
                plugin_type: "zgain".to_owned(),
                parameters: json!({ "gain": 1.0 }),
            },
            mix: -1.0,
            band_mask_low_hz: 500.0,
            band_mask_high_hz: 8_000.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        },
        ab087_factory,
    )
    .unwrap();
    masked.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let (_, mask_envelope) = check(&masked, "mask", "fresh");
    feed(&mut masked, "mask", &dense_input(128), &[1, 7, 64, 56]);
    let (mask_fed_live, mask_fed_envelope) = check(&masked, "mask", "fed");
    assert_eq!(
        mask_fed_envelope, mask_envelope,
        "mask drain envelope must be stream-independent"
    );
    assert!(
        mask_fed_live > 0,
        "fed mask must carry a live bound (mask history engages)"
    );
    drain_checked(&mut masked, "mask");
    masked.reset();
    let (_, mask_warm) = check(&masked, "mask", "reset");
    assert_eq!(
        mask_warm, mask_envelope,
        "mask drain envelope must survive reset"
    );
    println!(
        "ab live-domination: diamond envelope {diamond_envelope}, mask envelope {mask_envelope}, \
         fresh/fed/draining/reset all dominate",
    );
}

// ============================================================================
// R25(b) remaining-tail composition: nested-AB recursion, the 21 Hz mask EOF
// proof, and the converting-path bound. Outer tails compose through each
// child host's remaining-tail fold, so a nested AB's tail must surface
// verbatim (exact counts, honest Unknown, loose bounds alike).
// ============================================================================

/// Inner-AB params shared by the nested factory arm and the direct twin, so
/// recursion transparency compares identical configurations by construction.
fn nested_inner_params(inner_a: &str, low_hz: f32, high_hz: f32) -> ABComparePluginParams {
    let path_a = match inner_a {
        "burst" => PathConfig::Plugin {
            plugin_type: "burst".to_owned(),
            parameters: burst_params(BURST_CHUNK),
        },
        "echotail" => PathConfig::Plugin {
            plugin_type: "echotail".to_owned(),
            parameters: json!({}),
        },
        _ => PathConfig::Plugin {
            plugin_type: "zgain".to_owned(),
            parameters: json!({ "gain": 1.0 }),
        },
    };
    ABComparePluginParams {
        path_a,
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        band_mask_low_hz: low_hz,
        band_mask_high_hz: high_hz,
        ..ABComparePluginParams::default()
    }
}

/// Test factory with a nested-AB arm: `ab-nested` builds a full inner
/// `ABComparePlugin` at the factory's resolved clock. Construction bakes the
/// rate (hosts initialize children through the normal lifecycle, idempotent
/// on the fresh state).
fn ab_nested_factory(
    plugin_type: &str,
    parameters: &Value,
    channels: usize,
    sample_rate: f64,
) -> Result<Box<dyn Plugin>, String> {
    if plugin_type == "ab-nested" {
        let low_hz = parameters
            .get("low_hz")
            .and_then(Value::as_f64)
            .unwrap_or(20.0) as f32;
        let high_hz = parameters
            .get("high_hz")
            .and_then(Value::as_f64)
            .unwrap_or(20_000.0) as f32;
        let inner_a = parameters
            .get("inner_a")
            .and_then(Value::as_str)
            .unwrap_or("zgain");
        let inner = ABComparePlugin::from_params_with_factory(
            channels,
            sample_rate,
            nested_inner_params(inner_a, low_hz, high_hz),
            ab087_factory,
        )?;
        return Ok(Box::new(inner));
    }
    ab087_factory(plugin_type, parameters, channels, sample_rate)
}

/// Outer AB whose path A is a nested AB (pure-A mix, inactive outer mask).
fn outer_nested_ab(inner_a: &str, low_hz: f32, high_hz: f32) -> ABComparePlugin {
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "ab-nested".to_owned(),
            parameters: json!({ "inner_a": inner_a, "low_hz": low_hz, "high_hz": high_hz }),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        band_mask_low_hz: 20.0,
        band_mask_high_hz: 20_000.0,
        ..ABComparePluginParams::default()
    };
    let mut outer =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab_nested_factory)
            .unwrap();
    outer.initialize(f64::from(SAMPLE_RATE)).unwrap();
    outer
}

/// Direct twin of the nested inner: identical params, no outer layer.
fn twin_inner_ab(inner_a: &str, low_hz: f32, high_hz: f32) -> ABComparePlugin {
    let mut twin = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        nested_inner_params(inner_a, low_hz, high_hz),
        ab087_factory,
    )
    .unwrap();
    twin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    twin
}

/// Manual outer drain with a per-call tail probe, mirroring
/// `drain_all_collected_rate_bounded` budgeting (progress-gated rate bound
/// plus a stream+horizon backstop). Returns (samples, per-call partition,
/// pre-call tail queries).
fn drain_outer_with_tail_probe(
    outer: &mut ABComparePlugin,
    stream_frames: usize,
    extra_horizon_frames: usize,
) -> (Vec<f32>, Vec<usize>, Vec<TailLength>) {
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    outer.begin_drain(&context).unwrap();
    let capacity_frames = outer.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut output = Vec::new();
    let mut partition = Vec::new();
    let mut tails = Vec::new();
    let backstop = stream_frames
        .checked_add(extra_horizon_frames)
        .and_then(|total| total.checked_add(2))
        .expect("drain probe backstop fits address space");
    let mut calls = 0;
    loop {
        tails.push(outer.tail_length());
        block.fill(f32::NAN);
        let result = outer.drain(&mut block, &context).unwrap();
        assert!(result.frames <= capacity_frames);
        partition.push(result.frames);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        calls += 1;
        if result.complete {
            return (output, partition, tails);
        }
        let emitted = output.len() / CHANNELS;
        let rate_bound = emitted
            .checked_mul(2)
            .and_then(|doubled| doubled.checked_add(2))
            .expect("drain rate bound fits address space");
        assert!(
            calls <= rate_bound,
            "drain made no progress: {calls} calls for {emitted} frames"
        );
        assert!(
            calls < backstop,
            "drain exceeded its probe backstop ({backstop} calls for {stream_frames}+{extra_horizon_frames} horizon frames)"
        );
    }
}

fn assert_tail_finite_eq(tail: TailLength, expected: u64) {
    let TailLength::Finite(frames) = tail else {
        panic!("expected Finite({expected}) tail");
    };
    assert_eq!(frames, expected, "tail query must match exactly");
}

#[test]
fn nested_unity_tail_is_exact_zero() {
    // Recursion guard: nested unity (zgain/None, inactive masks everywhere)
    // holds no future content at any level, so the outer pre-drain query is
    // exactly zero, drain emits nothing, and the twin queries agree — the
    // fold invents no tails through nesting.
    reset_ab087_counters();
    let input = impulse_input(128);
    let chunks = [1usize, 7, 64, 56];
    let mut twin = twin_inner_ab("zgain", 20.0, 20_000.0);
    let twin_process = render_collected(&mut twin, &input, &chunks);
    let mut outer = outer_nested_ab("zgain", 20.0, 20_000.0);
    let outer_process = render_collected(&mut outer, &input, &chunks);
    assert_eq!(
        outer_process, input,
        "nested unity must pass through bitwise"
    );
    assert_eq!(
        outer_process, twin_process,
        "nested process must match the direct twin bitwise"
    );
    assert_tail_finite_eq(twin.tail_length(), 0);
    assert_tail_finite_eq(outer.tail_length(), 0);
    let (drained, _, _) = drain_outer_with_tail_probe(&mut outer, 128, 64);
    assert!(drained.is_empty(), "nested unity drain emits nothing");
    assert_tail_finite_eq(outer.tail_length(), 0);
    println!("nested unity tail: queried 0, drained 0 frames");
}

#[test]
fn nested_burst_tail_composes_exactly() {
    // Recursive exactness with live retention dynamics: the inner burst path
    // leaves a 37-frame residual (293 = 4x64 + 37) plus staging retention,
    // all counted exactly (burst tails are residual-exact), so the outer
    // pre-drain query equals the twin's verbatim and equals the drain total.
    reset_ab087_counters();
    let chunks = [1usize, 7, 64, 63, 65, 93];
    assert_eq!(chunks.iter().sum::<usize>(), 293);
    let input = dense_input(293);
    let mut twin = twin_inner_ab("burst", 20.0, 20_000.0);
    let twin_process = render_collected(&mut twin, &input, &chunks);
    let mut outer = outer_nested_ab("burst", 20.0, 20_000.0);
    let outer_process = render_collected(&mut outer, &input, &chunks);
    assert_eq!(
        outer_process, twin_process,
        "nested process must match the direct twin bitwise"
    );
    let TailLength::Finite(twin_tail) = twin.tail_length() else {
        panic!("unmasked twin tail must be Finite");
    };
    let TailLength::Finite(outer_tail) = outer.tail_length() else {
        panic!("unmasked outer tail must be Finite");
    };
    assert_eq!(
        outer_tail, twin_tail,
        "outer query must compose the inner tail verbatim"
    );
    assert!(outer_tail > 0, "burst residual must leave a live tail");
    let (outer_drain, _, _) = drain_outer_with_tail_probe(&mut outer, 293, 4096);
    let total = outer_drain.len() / CHANNELS;
    assert_eq!(
        total as u64, outer_tail,
        "outer drain must emit exactly the queried tail"
    );
    let (twin_drain, _) = drain_all_collected_rate_bounded(&mut twin, 293, 4096);
    assert_eq!(
        outer_drain, twin_drain,
        "nested drain must match the direct twin bitwise"
    );
    assert_tail_finite_eq(outer.tail_length(), 0);
    println!("nested burst tail: queried {outer_tail}, drained {total} frames");
}

#[test]
fn nested_21hz_mask_unity_tail_is_exact() {
    // Recursive 21 Hz mask EOF proof, quiet-inner leg: unity inner paths
    // hold no future content, so the inner live mask horizon is already the
    // final arm value; the outer pre-drain query is exact, every mid-drain
    // query stays exact, and the twin stream matches the independent fresh
    // cascade bitwise (RBJ at 1e-6, 200k probe below peak·2^-24).
    reset_ab087_counters();
    const LOW_HZ: f32 = 21.0;
    const HIGH_HZ: f32 = 20_000.0;
    let input = impulse_input(128);
    let chunks = [1usize, 7, 64, 56];
    let mut twin = twin_inner_ab("zgain", LOW_HZ, HIGH_HZ);
    let twin_process = render_collected(&mut twin, &input, &chunks);
    assert_ne!(twin_process, input, "21 Hz mask must filter");
    let mut outer = outer_nested_ab("zgain", LOW_HZ, HIGH_HZ);
    let outer_process = render_collected(&mut outer, &input, &chunks);
    assert_eq!(
        outer_process, twin_process,
        "nested process must match the direct twin bitwise"
    );
    let TailLength::Finite(twin_tail) = twin.tail_length() else {
        panic!("quiet-inner twin tail must be Finite");
    };
    let TailLength::Finite(outer_tail) = outer.tail_length() else {
        panic!("quiet-inner outer tail must be Finite");
    };
    assert_eq!(
        outer_tail, twin_tail,
        "outer query must compose the inner tail verbatim"
    );
    assert!(
        outer_tail > 0,
        "21 Hz impulse must excite a nontrivial flush"
    );
    let (outer_drain, partition, tails) = drain_outer_with_tail_probe(&mut outer, 128, 200_000);
    let total = outer_drain.len() / CHANNELS;
    assert_eq!(
        total as u64, outer_tail,
        "outer drain must emit exactly the queried tail"
    );
    let mut remaining = total;
    for (call, tail) in tails.iter().enumerate() {
        let TailLength::Finite(predicted) = tail else {
            panic!("quiet-phase queries must stay Finite (call {call})");
        };
        assert_eq!(
            *predicted as usize, remaining,
            "mid-drain query must equal the remainder (call {call})"
        );
        remaining -= partition[call];
    }
    let (twin_drain, _) = drain_all_collected_rate_bounded(&mut twin, 128, 200_000);
    assert_eq!(
        outer_drain, twin_drain,
        "nested drain must match the direct twin bitwise"
    );
    // Independent references on the twin stream (unity children drain
    // nothing, so the twin tail is the mask flush alone).
    let flush_frames = twin_drain.len() / CHANNELS;
    assert_eq!(flush_frames as u64, twin_tail);
    let mut wet = input.clone();
    wet.extend(std::iter::repeat_n(0.0, flush_frames * CHANNELS));
    let mut twin_stream = twin_process.clone();
    twin_stream.extend_from_slice(&twin_drain);
    let (low_f64, high_f64) = (f64::from(LOW_HZ), f64::from(HIGH_HZ));
    let cascade = mask_cascade_reference(&wet, CHANNELS, low_f64, high_f64, f64::from(SAMPLE_RATE));
    assert_eq!(
        twin_stream, cascade,
        "nested 21 Hz stream bit-exact cascade"
    );
    let cookbook =
        mask_cascade_rbj_reference(&wet, CHANNELS, low_f64, high_f64, f64::from(SAMPLE_RATE));
    assert_vectors_close(&twin_stream, &cookbook, "nested-21hz");
    let tail_max = mask_tail_max_beyond(
        &wet,
        CHANNELS,
        low_f64,
        high_f64,
        f64::from(SAMPLE_RATE),
        200_000,
    );
    assert!(
        tail_max < MASK_RESIDUAL_RATIO,
        "remainder {tail_max} must stay below {}",
        MASK_RESIDUAL_RATIO
    );
    assert_tail_finite_eq(outer.tail_length(), 0);
    println!(
        "nested 21 Hz unity tail: queried {outer_tail}, drained {total} frames, 200k-probe remainder {tail_max:.3e}"
    );
}

#[test]
fn nested_21hz_mask_burst_tail_transitions_unknown_to_exact() {
    // Recursive 21 Hz mask EOF proof, driven-inner leg: the 37-frame burst
    // residual still drives the inner mask pre-drain, so both queries
    // honestly report Unknown; once the inner mask arms, every query turns
    // Finite and equals the true remainder exactly (burst tails are
    // residual-exact, so no stale bound survives arming). Cascade/RBJ
    // conformance for the same filter is proven by the unity leg above, so
    // this leg pins recursion, transition order, exactness, and transparency.
    reset_ab087_counters();
    const LOW_HZ: f32 = 21.0;
    const HIGH_HZ: f32 = 20_000.0;
    let chunks = [1usize, 7, 64, 63, 65, 93];
    assert_eq!(chunks.iter().sum::<usize>(), 293);
    let input = dense_input(293);
    let mut twin = twin_inner_ab("burst", LOW_HZ, HIGH_HZ);
    let twin_process = render_collected(&mut twin, &input, &chunks);
    let mut outer = outer_nested_ab("burst", LOW_HZ, HIGH_HZ);
    let outer_process = render_collected(&mut outer, &input, &chunks);
    assert_eq!(
        outer_process, twin_process,
        "nested process must match the direct twin bitwise"
    );
    let TailLength::Unknown = twin.tail_length() else {
        panic!("driven twin must report Unknown while content drives the mask");
    };
    let TailLength::Unknown = outer.tail_length() else {
        panic!("outer must compose inner Unknown, never guess");
    };
    let (outer_drain, partition, tails) = drain_outer_with_tail_probe(&mut outer, 293, 200_000);
    let total = outer_drain.len() / CHANNELS;
    let mut saw_unknown = false;
    let mut saw_finite = false;
    let mut remaining = total;
    for (call, tail) in tails.iter().enumerate() {
        match tail {
            TailLength::Unknown => {
                assert!(
                    !saw_finite,
                    "Unknown after Finite would move backwards (call {call})"
                );
                saw_unknown = true;
            }
            TailLength::Finite(predicted) => {
                saw_finite = true;
                assert_eq!(
                    *predicted as usize, remaining,
                    "armed query must equal the remainder (call {call})"
                );
            }
            TailLength::Infinite => panic!("finite rig must never report Infinite"),
        }
        remaining -= partition[call];
    }
    assert!(
        saw_unknown && saw_finite,
        "must observe the Unknown->Finite arming transition"
    );
    let (twin_drain, _) = drain_all_collected_rate_bounded(&mut twin, 293, 200_000);
    assert_eq!(
        outer_drain, twin_drain,
        "nested drain must match the direct twin bitwise"
    );
    assert_tail_finite_eq(outer.tail_length(), 0);
    println!(
        "nested 21 Hz burst tail: Unknown->Finite transition over {} calls, drained {total} frames",
        tails.len()
    );
}

#[test]
fn active_21hz_mask_without_content_tails_zero() {
    // Active 21 Hz cutoffs with zero excitation: silent filter state derives
    // a zero live horizon with no future content anywhere, so the pre-drain
    // query is exactly zero and drain completes empty (mirrors the existing
    // immediate-drain shape at 500 Hz, now with tail queries at 21 Hz).
    reset_ab087_counters();
    let mut plugin = unity_mask_plugin(21.0, 20_000.0);
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_tail_finite_eq(plugin.tail_length(), 0);
    let (drained, _) = drain_all_collected(&mut plugin);
    assert!(drained.is_empty(), "unexcited mask drain emits nothing");
    assert_tail_finite_eq(plugin.tail_length(), 0);
    println!("active 21 Hz unexcited tail: queried 0, drained 0 frames");
}

#[test]
fn converting_path_tail_bound_dominates_actual() {
    // Converting-path tail contract: the nested 48->24 production resampler
    // plus the owned 24->48 converter report bound (not exact) tails
    // (call-padding over-approximates), so the outer tail is Finite and
    // dominates the true drain total without guessing it. Unmasked to
    // isolate the converting bound from mask-Unknown.
    reset_ab087_counters();
    let chunks = [1usize, 7, 64, 63, 65, 137, 2, 685];
    assert_eq!(chunks.iter().sum::<usize>(), 1024);
    let input = dense_input(1024);
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "resampler".to_owned(),
            parameters: resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        band_mask_low_hz: 20.0,
        band_mask_high_hz: 20_000.0,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab087_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let _ = render_collected(&mut plugin, &input, &chunks);
    let TailLength::Finite(bound) = plugin.tail_length() else {
        panic!("converting-path tail must be a Finite bound");
    };
    let (drained, _) = drain_all_collected_rate_bounded(&mut plugin, 1024, 20_000);
    let total = drained.len() / CHANNELS;
    assert!(
        (total as u64) <= bound,
        "converting bound {bound} must dominate the true total {total}"
    );
    assert_tail_finite_eq(plugin.tail_length(), 0);
    println!("converting-path tail: bound {bound}, actual {total} frames");
}

#[test]
fn driven_nested_21hz_small_capacity_completes_past_fallback_and_partial() {
    // Pathological driven-nested-21Hz quota proof: the inner 21 Hz mask
    // stays driven (the 2500-frame echo tail defeats eager arming) while
    // the 1-frame drain capacity stretches content plus the ~13k-frame
    // flush near 16k calls — beyond BOTH the 4096 unknown fallback AND
    // the ~5003 partial to-quiet bound — so outer completion proves the
    // child host granted the single re-queried budget when the inner tail
    // turned Finite. Twin bitwise equality, Unknown->Finite transition
    // order, and armed exactness pin the rest; absolute cascade
    // conformance for this filter rides the unity leg above (this leg
    // pins quota mechanics, like the burst leg before it). Both margins
    // assume only F > ~2500 (5x below estimate), never the estimate.
    reset_ab087_counters();
    const LOW_HZ: f32 = 21.0;
    const HIGH_HZ: f32 = 20_000.0;
    let chunks = [1usize, 7, 64, 63, 65, 93];
    assert_eq!(chunks.iter().sum::<usize>(), 293);
    let input = dense_input(293);
    let mut twin = twin_inner_ab("echotail", LOW_HZ, HIGH_HZ);
    let twin_process = render_collected(&mut twin, &input, &chunks);
    let mut outer = outer_nested_ab("echotail", LOW_HZ, HIGH_HZ);
    let outer_process = render_collected(&mut outer, &input, &chunks);
    assert_eq!(
        outer_process, twin_process,
        "nested process must match the direct twin bitwise"
    );
    let TailLength::Unknown = twin.tail_length() else {
        panic!("driven twin must report Unknown while content drives the mask");
    };
    let TailLength::Unknown = outer.tail_length() else {
        panic!("outer must compose inner Unknown, never guess");
    };
    // Grant-time answers on the twin (identical construction and state
    // to the nested inner): the partial to-quiet bound with an Unknown
    // tail is the combination the quota refresh gates on.
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    twin.begin_drain(&context).unwrap();
    let initial = twin
        .drain_call_bound()
        .expect("driven twin must publish its partial to-quiet bound")
        .get();
    assert_eq!(
        twin.drain_output_frames_max(),
        1,
        "echo 1-frame drain with zero-latency paths sets unit inner capacity"
    );
    let (twin_drain, twin_partition, twin_tails) =
        drain_outer_with_tail_probe(&mut twin, 293, 200_000);
    let twin_calls = twin_partition.len();
    assert!(
        twin_calls > 4096,
        "twin needed {twin_calls} calls: must exceed the 4096 fallback or the refresh is unproven"
    );
    assert!(
        (twin_calls as u64) > initial,
        "twin needed {twin_calls} calls past partial bound {initial}: refresh necessity"
    );
    let (outer_drain, partition, tails) = drain_outer_with_tail_probe(&mut outer, 293, 200_000);
    let total = outer_drain.len() / CHANNELS;
    assert_eq!(
        outer_drain, twin_drain,
        "nested drain must match the direct twin bitwise"
    );
    let mut saw_unknown = false;
    let mut saw_finite = false;
    let mut remaining = total;
    for (call, tail) in tails.iter().enumerate() {
        match tail {
            TailLength::Unknown => {
                assert!(
                    !saw_finite,
                    "Unknown after Finite would move backwards (call {call})"
                );
                saw_unknown = true;
            }
            TailLength::Finite(predicted) => {
                saw_finite = true;
                assert_eq!(
                    *predicted as usize, remaining,
                    "armed query must equal the remainder (call {call})"
                );
            }
            TailLength::Infinite => panic!("finite rig must never report Infinite"),
        }
        remaining -= partition[call];
    }
    assert!(
        saw_unknown && saw_finite,
        "must observe the Unknown->Finite arming transition"
    );
    // The first armed twin tail is the exact flush length (content
    // already exhausted): it must be nonzero, proving the dense echo
    // genuinely excited the mask and the refresh covered real flush work
    // past the partial budget — not a zero-length arm.
    let armed = twin_tails.iter().find_map(|tail| match tail {
        TailLength::Finite(frames) => Some(*frames as usize),
        _ => None,
    });
    let flush_frames = armed.expect("twin must arm its flush mid-drain");
    assert!(
        flush_frames > 0,
        "driven 21 Hz state must arm a nonzero flush"
    );
    assert_tail_finite_eq(outer.tail_length(), 0);
    println!(
        "pathological nested 21 Hz: partial {initial}, twin {twin_calls} calls, drained {total} frames, flush {flush_frames}"
    );
}

/// Diamond with a net-same-rate production resampler pair on branch A
/// (48->24->48, facade chunk 1024) and unity gain on branch B: the graph
/// scheduler interleaves resampling rounds, which is the R6-F1 leg (b)
/// geometry a chain can never produce.
fn resampling_diamond_host() -> DawHost {
    let mut host = DawHost::new(CHANNELS, SAMPLE_RATE);
    let unity = json!({ "gain": 1.0 });
    let source = host
        .add_node(
            "source".to_string(),
            ab087_factory("zgain", &unity, CHANNELS, SAMPLE_RATE).unwrap(),
        )
        .unwrap();
    let down = host
        .add_node(
            "down".to_string(),
            ab087_factory(
                "resampler",
                &resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                CHANNELS,
                SAMPLE_RATE,
            )
            .unwrap(),
        )
        .unwrap();
    // The up-converter initializes at its 24 kHz input clock, not the
    // host rate: plain `add_node` initializes eagerly at the host rate and
    // fails before `build` can negotiate (see `add_node_at_rate`).
    let up = host
        .add_node_at_rate(
            "up".to_string(),
            ab087_factory(
                "resampler",
                &resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
                CHANNELS,
                HALF_RATE,
            )
            .unwrap(),
            HALF_RATE,
        )
        .unwrap();
    let direct = host
        .add_node(
            "direct".to_string(),
            ab087_factory("zgain", &unity, CHANNELS, SAMPLE_RATE).unwrap(),
        )
        .unwrap();
    let join = host
        .add_node(
            "join".to_string(),
            ab087_factory("zgain", &unity, CHANNELS, SAMPLE_RATE).unwrap(),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, down)).unwrap();
    host.add_edge(GraphEdge::new(down, up)).unwrap();
    host.add_edge(GraphEdge::new(up, join)).unwrap();
    host.add_edge(GraphEdge::new(source, direct)).unwrap();
    host.add_edge(GraphEdge::new(direct, join)).unwrap();
    host.build().unwrap();
    host
}

/// Feed `input` through `host` in `blocks`; returns all output. The input
/// cursor advances by consumed frames, not rendered ones: converting
/// branches hold partial chunks back, so rendered output can lag input
/// (the remainder arrives in later blocks or at drain).
fn render_host_collected(host: &mut DawHost, input: &[f32], blocks: &[usize]) -> Vec<f32> {
    assert_eq!(blocks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = Vec::new();
    let mut consumed = 0;
    for &frames in blocks {
        let capacity = host.output_frames_for_input(frames);
        let mut block = vec![f32::NAN; capacity * CHANNELS];
        let start = consumed * CHANNELS;
        let rendered = host
            .process(&input[start..start + frames * CHANNELS], &mut block)
            .unwrap();
        assert!(rendered <= capacity);
        assert!(
            block[..rendered * CHANNELS]
                .iter()
                .all(|sample| !sample.is_nan()),
            "host left declared output unwritten"
        );
        output.extend_from_slice(&block[..rendered * CHANNELS]);
        consumed += frames;
    }
    assert_eq!(consumed, input.len() / CHANNELS);
    output
}

/// Drain `host` with one mid-drain tail query: runs until the first
/// emitted frame (guaranteed mid-drain: completion first fails loudly as
/// vacuous), queries, then finishes. Returns (quiescent, mid, pre, post).
fn drain_host_with_mid_query(host: &mut DawHost) -> (u64, u64, usize, usize) {
    let TailLength::Finite(quiescent) = host.tail_length() else {
        panic!("quiescent resampling-graph tail must be Finite");
    };
    let bound = host.drain_output_frames_max().max(1);
    let mut block = vec![f32::NAN; bound * CHANNELS];
    let mut pre = 0;
    let mut calls = 0;
    loop {
        block.fill(f32::NAN);
        let result = host.drain(&mut block).unwrap();
        assert!(result.frames <= bound);
        pre += result.frames;
        calls += 1;
        assert!(
            calls <= DRAIN_CALL_LIMIT,
            "drain exceeded its {DRAIN_CALL_LIMIT}-call hang guard"
        );
        if result.complete {
            panic!("drain completed before any mid-drain query: vacuous rig");
        }
        if pre > 0 {
            break;
        }
    }
    let TailLength::Finite(mid) = host.tail_length() else {
        panic!("mid-drain resampling-graph tail must be Finite");
    };
    let mut post = 0;
    loop {
        block.fill(f32::NAN);
        let result = host.drain(&mut block).unwrap();
        assert!(result.frames <= bound);
        post += result.frames;
        calls += 1;
        assert!(
            calls <= DRAIN_CALL_LIMIT,
            "drain exceeded its {DRAIN_CALL_LIMIT}-call hang guard"
        );
        if result.complete {
            break;
        }
    }
    (quiescent, mid, pre, post)
}

#[test]
fn resampling_graph_mid_drain_tail_dominates_executed_remainder() {
    // R6-F1 leg (b): the graph scheduler interleaves resampling rounds,
    // so the query-state residual differs from the arrival-state
    // residual by a chunk-straddle delta. The multi-chunk irregular
    // stream forces real straddle; both the quiescent and the mid-drain
    // Finite queries must dominate their executed remainders (bound,
    // never exactness — over-approximation is the honest answer here).
    reset_ab087_counters();
    let mut host = resampling_diamond_host();
    let blocks = [100usize, 777, 1023, 600];
    assert_eq!(blocks.iter().sum::<usize>(), 2500);
    let input = dense_input(2500);
    let _ = render_host_collected(&mut host, &input, &blocks);
    let (quiescent, mid, pre, post) = drain_host_with_mid_query(&mut host);
    let total = pre + post;
    assert!(
        quiescent >= total as u64,
        "quiescent {quiescent} must dominate the executed total {total}"
    );
    assert!(
        mid >= post as u64,
        "mid-drain {mid} must dominate the executed remainder {post}"
    );
    println!(
        "resampling-graph mid-drain: quiescent {quiescent}, mid {mid} over remainder {post} (total {total})"
    );
}

#[test]
fn resampling_graph_mid_drain_tail_dominates_executed_remainder_f64() {
    // f64-processed leg of the same domination proof (drain stays
    // f32-only by design; the precision step must not shrink the fold
    // below the executed remainder either).
    reset_ab087_counters();
    let mut host = resampling_diamond_host();
    let input_f32 = dense_input(2500);
    let input: Vec<f64> = input_f32.iter().map(|&sample| f64::from(sample)).collect();
    let mut consumed = 0;
    for &frames in &[100usize, 777, 1023, 600] {
        let capacity = host.output_frames_for_input(frames);
        let mut block = vec![f64::NAN; capacity * CHANNELS];
        let start = consumed * CHANNELS;
        let rendered = host
            .process_f64(&input[start..start + frames * CHANNELS], &mut block)
            .unwrap();
        assert!(rendered <= capacity);
        consumed += frames;
    }
    assert_eq!(consumed, 2500);
    let (quiescent, mid, pre, post) = drain_host_with_mid_query(&mut host);
    let total = pre + post;
    assert!(
        quiescent >= total as u64,
        "f64 quiescent {quiescent} must dominate the executed total {total}"
    );
    assert!(
        mid >= post as u64,
        "f64 mid-drain {mid} must dominate the executed remainder {post}"
    );
    println!(
        "resampling-graph f64 mid-drain: quiescent {quiescent}, mid {mid} over remainder {post} (total {total})"
    );
}

#[test]
fn converting_pair_tail_is_strict_bound_over_actual_drain() {
    // R6-F3 pin, the reviewer's counterexample verbatim: a net-same-rate
    // chain holding a 48->24->48 production pair. Post-process (partial
    // final chunk), call padding and chunk ceilings over-approximate, so
    // the quiescent fold is a STRICT bound over the executed drain —
    // Finite and dominating, never exact. Soundness (domination) and
    // strictness assert separately so an aliquot-exact rig fails with a
    // strictness-only diagnosis, never a soundness alarm.
    reset_ab087_counters();
    let mut host = DawHost::new(CHANNELS, SAMPLE_RATE);
    let down = host
        .add_node(
            "down".to_string(),
            ab087_factory(
                "resampler",
                &resampler_params(SAMPLE_RATE, HALF_RATE, NESTED_SRC_CHUNK),
                CHANNELS,
                SAMPLE_RATE,
            )
            .unwrap(),
        )
        .unwrap();
    // Explicit 24 kHz initialization: plain `add_node` would initialize
    // eagerly at the 48 kHz host rate and fail before negotiation.
    let up = host
        .add_node_at_rate(
            "up".to_string(),
            ab087_factory(
                "resampler",
                &resampler_params(HALF_RATE, SAMPLE_RATE, NESTED_SRC_CHUNK),
                CHANNELS,
                HALF_RATE,
            )
            .unwrap(),
            HALF_RATE,
        )
        .unwrap();
    host.add_edge(GraphEdge::new(down, up)).unwrap();
    host.build().unwrap();
    let blocks = [100usize, 777, 1023, 600];
    assert_eq!(blocks.iter().sum::<usize>(), 2500);
    let input = dense_input(2500);
    let _ = render_host_collected(&mut host, &input, &blocks);
    let TailLength::Finite(bound) = host.tail_length() else {
        panic!("converting-pair tail must be a Finite bound");
    };
    let capacity = host.drain_output_frames_max().max(1);
    let mut block = vec![f32::NAN; capacity * CHANNELS];
    let mut total = 0;
    let mut calls = 0;
    loop {
        block.fill(f32::NAN);
        let result = host.drain(&mut block).unwrap();
        assert!(result.frames <= capacity);
        total += result.frames;
        calls += 1;
        assert!(
            calls <= DRAIN_CALL_LIMIT,
            "drain exceeded its {DRAIN_CALL_LIMIT}-call hang guard"
        );
        if result.complete {
            break;
        }
    }
    assert!(
        (total as u64) <= bound,
        "converting bound {bound} must dominate the true total {total}"
    );
    assert!(
        (total as u64) < bound,
        "converting bound {bound} must strictly over-approximate the true total {total}"
    );
    println!("converting-pair tail: strict bound {bound} over actual {total}");
}
