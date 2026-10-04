pub mod params;
pub mod repair;

use crate::params::PARAMS as DC;
use crate::params::{BANDS_OPTIONS, MODE_OPTIONS};
use crate::repair::{DelayLine, MAX_LATENCY_SAMPLES, MAX_REPAIR_WIDTH, OwnedEngine};
use plugins_denoiser::transient::TransientSuppressor;
use serde::{Deserialize, Serialize};
use sotf_host::param_bridge;
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};

sotf_host::define_choice_index_deserializer!(deserialize_mode_param, MODE_OPTIONS);
sotf_host::define_choice_index_deserializer!(deserialize_bands_param, BANDS_OPTIONS);

/// Serializable declick configuration.
///
/// Fields `mode` through `audition_residual` were appended after the frozen
/// legacy triple; missing keys deserialize to neutral defaults so old saved
/// state keeps producing the legacy fullband random behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclickPluginParams {
    #[serde(default = "d_enabled")]
    pub enabled: bool,
    #[serde(default = "d_sensitivity")]
    pub sensitivity: f32,
    #[serde(default = "d_link_channels")]
    pub link_channels: bool,
    #[serde(default = "d_mode", deserialize_with = "deserialize_mode_param")]
    pub mode: usize,
    #[serde(default = "d_bands", deserialize_with = "deserialize_bands_param")]
    pub bands: usize,
    #[serde(default = "d_crossover_hz")]
    pub crossover_hz: f32,
    #[serde(default = "d_frequency_skew")]
    pub frequency_skew: f32,
    #[serde(default = "d_repair_width")]
    pub repair_width: usize,
    #[serde(default = "d_audition_residual")]
    pub audition_residual: bool,
}

fn d_enabled() -> bool {
    pk(DC, "enabled").default_bool()
}
fn d_sensitivity() -> f32 {
    pk(DC, "sensitivity").default_f32()
}
fn d_link_channels() -> bool {
    pk(DC, "link_channels").default_bool()
}
fn d_mode() -> usize {
    pk(DC, "mode").default_usize()
}
fn d_bands() -> usize {
    pk(DC, "bands").default_usize()
}
fn d_crossover_hz() -> f32 {
    pk(DC, "crossover_hz").default_f32()
}
fn d_frequency_skew() -> f32 {
    pk(DC, "frequency_skew").default_f32()
}
fn d_repair_width() -> usize {
    pk(DC, "repair_width").default_usize()
}
fn d_audition_residual() -> bool {
    pk(DC, "audition_residual").default_bool()
}

impl Default for DeclickPluginParams {
    fn default() -> Self {
        Self {
            enabled: d_enabled(),
            sensitivity: d_sensitivity(),
            link_channels: d_link_channels(),
            mode: d_mode(),
            bands: d_bands(),
            crossover_hz: d_crossover_hz(),
            frequency_skew: d_frequency_skew(),
            repair_width: d_repair_width(),
            audition_residual: d_audition_residual(),
        }
    }
}

/// Legacy fullband random path with its aligned dry tap.
///
/// Grouped so drain and process can borrow the path and the drain scratch
/// buffer as disjoint fields.
struct LegacyPath {
    channels: usize,
    suppressor: TransientSuppressor,
    dry: DelayLine,
    work: Vec<f32>,
    dry_frame: Vec<f32>,
    mix_current: f32,
    mix_target: f32,
    mix_decay: f32,
}

impl LegacyPath {
    fn new(channels: usize, sample_rate: f64) -> Result<Self, String> {
        Ok(Self {
            channels,
            suppressor: TransientSuppressor::new(channels, sample_rate)?,
            dry: DelayLine::new(channels, plugins_denoiser::transient::LOOKAHEAD_SAMPLES)?,
            work: vec![0.0; channels],
            dry_frame: vec![0.0; channels],
            mix_current: 0.0,
            mix_target: 0.0,
            mix_decay: audition_decay(sample_rate),
        })
    }

    fn reset(&mut self) {
        self.suppressor.reset();
        self.dry.reset();
        self.work.fill(0.0);
        self.dry_frame.fill(0.0);
        self.mix_current = self.mix_target;
    }

    fn set_sample_rate(&mut self, sample_rate: f64) -> Result<(), String> {
        self.suppressor.set_sample_rate(sample_rate)?;
        self.mix_decay = audition_decay(sample_rate);
        Ok(())
    }

    fn set_mix_immediate(&mut self, audition: bool) {
        self.mix_target = if audition { 1.0 } else { 0.0 };
        self.mix_current = self.mix_target;
    }

    fn process_frames(&mut self, buffer: &mut [f32]) -> Result<(), String> {
        if !buffer.len().is_multiple_of(self.channels) {
            return Err(format!(
                "declick buffer length {} is not divisible by {} channels",
                buffer.len(),
                self.channels
            ));
        }
        for frame in buffer.chunks_exact_mut(self.channels) {
            self.mix_current =
                self.mix_current * self.mix_decay + self.mix_target * (1.0 - self.mix_decay);
            self.work.copy_from_slice(frame);
            let (dry, work, dry_out) = (&mut self.dry, &self.work, &mut self.dry_frame);
            dry.push_frame(work, dry_out)?;
            self.suppressor.process(&mut self.work)?;
            let mix = self.mix_current;
            if mix == 0.0 {
                // Exact repaired output; also avoids `NaN * 0.0` poisoning
                // the frame if a dry tap ever goes non-finite.
                frame.copy_from_slice(&self.work);
            } else {
                for (c, slot) in frame.iter_mut().enumerate().take(self.channels) {
                    let residual = self.dry_frame[c] - self.work[c];
                    *slot = self.work[c] + (residual - self.work[c]) * mix;
                }
            }
        }
        Ok(())
    }
}

pub struct DeclickPlugin {
    channels: usize,
    enabled: bool,
    sensitivity: f32,
    link_channels: bool,
    mode: usize,
    bands: usize,
    crossover_hz: f32,
    frequency_skew: f32,
    repair_width: usize,
    audition_residual: bool,
    legacy: LegacyPath,
    owned: OwnedEngine,
    initialized_sample_rate: f64,
    cached_parameters: Vec<Parameter>,
    has_input: bool,
    drain_remaining: Option<usize>,
    drain_silence: Vec<f32>,
}

impl DeclickPlugin {
    pub fn new<S: Into<f64>>(channels: usize, sample_rate: S) -> Result<Self, String> {
        Self::from_params(channels, sample_rate, DeclickPluginParams::default())
    }

    pub fn from_params<S: Into<f64>>(
        channels: usize,
        sample_rate: S,
        params: DeclickPluginParams,
    ) -> Result<Self, String> {
        let sample_rate = sample_rate.into();
        if channels == 0 {
            return Err("declick requires at least one channel".into());
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("declick sample rate must be greater than zero".into());
        }
        let sensitivity = if params.sensitivity.is_finite() {
            params.sensitivity.clamp(1.0, 100.0)
        } else {
            d_sensitivity()
        };
        let crossover_hz = if params.crossover_hz.is_finite() {
            params.crossover_hz.clamp(80.0, 12_000.0)
        } else {
            d_crossover_hz()
        };
        let frequency_skew = if params.frequency_skew.is_finite() {
            params.frequency_skew.clamp(-1.0, 1.0)
        } else {
            d_frequency_skew()
        };
        // Choice deserialization already rejects out-of-range indices; clamp
        // programmatic construction the same way sensitivity is canonicalized.
        let mode = params.mode.min(MODE_OPTIONS.len() - 1);
        let bands = params.bands.min(BANDS_OPTIONS.len() - 1);
        let repair_width = params.repair_width.min(MAX_REPAIR_WIDTH);
        let mut legacy = LegacyPath::new(channels, sample_rate)?;
        legacy.suppressor.set_sensitivity_immediate(sensitivity);
        legacy.suppressor.set_enabled_immediate(params.enabled);
        legacy.suppressor.set_link_channels(params.link_channels);
        legacy.set_mix_immediate(params.audition_residual);
        let mut owned = OwnedEngine::new(channels, sample_rate)?;
        owned.set_bands(bands + 1);
        owned.set_crossover_hz(crossover_hz, sample_rate);
        owned.set_repair_width(repair_width);
        owned.set_periodic(mode == 1);
        // Skew first: the immediate sensitivity call snapshots per-band
        // targets, so construction with nonzero skew must not start from
        // neutral and converge audibly over the first milliseconds.
        owned.set_skew(frequency_skew);
        owned.set_sensitivity_immediate(sensitivity);
        owned.set_enabled_immediate(params.enabled);
        owned.set_link_channels(params.link_channels);
        owned.set_audition_immediate(params.audition_residual);
        let mut plugin = Self {
            channels,
            enabled: params.enabled,
            sensitivity,
            link_channels: params.link_channels,
            mode,
            bands,
            crossover_hz,
            frequency_skew,
            repair_width,
            audition_residual: params.audition_residual,
            legacy,
            owned,
            initialized_sample_rate: sample_rate,
            cached_parameters: Vec::new(),
            has_input: false,
            drain_remaining: None,
            drain_silence: vec![0.0; channels * MAX_LATENCY_SAMPLES],
        };
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    /// Legacy routing: random mode, fullband, and zero width use the shared
    /// suppressor bit-exactly; anything else routes to the owned engine.
    fn use_legacy(&self) -> bool {
        self.mode == 0 && self.bands == 0 && self.repair_width == 0
    }

    fn current_params(&self) -> DeclickPluginParams {
        DeclickPluginParams {
            enabled: self.enabled,
            sensitivity: self.sensitivity,
            link_channels: self.link_channels,
            mode: self.mode,
            bands: self.bands,
            crossover_hz: self.crossover_hz,
            frequency_skew: self.frequency_skew,
            repair_width: self.repair_width,
            audition_residual: self.audition_residual,
        }
    }

    fn commit_params(&mut self, next: &DeclickPluginParams) {
        if next.enabled != self.enabled {
            self.enabled = next.enabled;
            self.legacy.suppressor.set_enabled(next.enabled);
            self.owned.set_enabled(next.enabled);
        }
        if next.sensitivity != self.sensitivity {
            self.sensitivity = next.sensitivity;
            self.legacy.suppressor.set_sensitivity(next.sensitivity);
            self.owned.set_sensitivity(next.sensitivity);
        }
        if next.link_channels != self.link_channels {
            self.link_channels = next.link_channels;
            self.legacy.suppressor.set_link_channels(next.link_channels);
            self.owned.set_link_channels(next.link_channels);
        }
        if next.frequency_skew != self.frequency_skew {
            self.frequency_skew = next.frequency_skew;
            self.owned.set_skew(next.frequency_skew);
        }
        if next.audition_residual != self.audition_residual {
            self.audition_residual = next.audition_residual;
            self.owned.set_audition(next.audition_residual);
            self.legacy.mix_target = if next.audition_residual { 1.0 } else { 0.0 };
        }
        let structural = next.mode != self.mode
            || next.bands != self.bands
            || next.crossover_hz != self.crossover_hz
            || next.repair_width != self.repair_width;
        if structural {
            self.mode = next.mode;
            self.bands = next.bands;
            self.crossover_hz = next.crossover_hz;
            self.repair_width = next.repair_width;
            self.owned.set_bands(next.bands + 1);
            self.owned
                .set_crossover_hz(next.crossover_hz, self.initialized_sample_rate);
            self.owned.set_repair_width(next.repair_width);
            self.owned.set_periodic(next.mode == 1);
            // Topology changed: clear detector/delay history so the new path
            // restarts with fresh latency. The stream stays open.
            self.reset_dsp_only();
        }
        self.update_cached_values();
    }

    /// Clear engine and delay history without closing the stream.
    fn reset_dsp_only(&mut self) {
        self.legacy.reset();
        self.owned.reset();
    }

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.enabled { 1.0 } else { 0.0 }),
            1 => Some(self.sensitivity as f64),
            2 => Some(if self.link_channels { 1.0 } else { 0.0 }),
            3 => Some(self.mode as f64),
            4 => Some(self.bands as f64),
            5 => Some(self.crossover_hz as f64),
            6 => Some(self.frequency_skew as f64),
            7 => Some(self.repair_width as f64),
            8 => Some(if self.audition_residual { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = param_bridge::build_parameters(DC, |i| self.param_value(i));
    }

    fn update_cached_values(&mut self) {
        self.cached_parameters[0].default_value = ParameterValue::Bool(self.enabled);
        self.cached_parameters[1].default_value = ParameterValue::Float(self.sensitivity);
        self.cached_parameters[2].default_value = ParameterValue::Bool(self.link_channels);
        self.cached_parameters[3].default_value = ParameterValue::Int(self.mode as i32);
        self.cached_parameters[4].default_value = ParameterValue::Int(self.bands as i32);
        self.cached_parameters[5].default_value = ParameterValue::Float(self.crossover_hz);
        self.cached_parameters[6].default_value = ParameterValue::Float(self.frequency_skew);
        self.cached_parameters[7].default_value = ParameterValue::Int(self.repair_width as i32);
        self.cached_parameters[8].default_value = ParameterValue::Bool(self.audition_residual);
    }

    /// Read back owned-engine period locks (white-box test hook).
    ///
    /// Returns `(bands, supervisor)` from the owned engine; the legacy path
    /// has no period tracker. Test-only; compiled out of production.
    #[cfg(test)]
    pub fn test_period_locks(&self) -> (Vec<Option<usize>>, Option<usize>) {
        self.owned.test_period_locks()
    }
}

fn audition_decay(sample_rate: f64) -> f32 {
    // 5 ms crossfade shared with the repair mix smoothing convention.
    let smoothing_samples = sample_rate as f32 * 5.0 * 0.001;
    (-1.0 / smoothing_samples.max(1.0)).exp()
}

impl ParametricInPlacePlugin for DeclickPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Declick", "1.1.0", "SotF")
            .with_description("Lookahead click detection and robust interpolation")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Dynamics
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(
            PluginCostClass::Dynamics,
            None,
            self.latency_samples(),
            false,
        )
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "enabled" => Some(ParameterValue::Bool(self.enabled)),
            "sensitivity" => Some(ParameterValue::Float(self.sensitivity)),
            "link_channels" => Some(ParameterValue::Bool(self.link_channels)),
            "mode" => Some(ParameterValue::Int(self.mode as i32)),
            "bands" => Some(ParameterValue::Int(self.bands as i32)),
            "crossover_hz" => Some(ParameterValue::Float(self.crossover_hz)),
            "frequency_skew" => Some(ParameterValue::Float(self.frequency_skew)),
            "repair_width" => Some(ParameterValue::Int(self.repair_width as i32)),
            "audition_residual" => Some(ParameterValue::Bool(self.audition_residual)),
            _ => None,
        }
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        values.insert(
            ParameterId::from("enabled"),
            ParameterValue::Bool(self.enabled),
        );
        values.insert(
            ParameterId::from("sensitivity"),
            ParameterValue::Float(self.sensitivity),
        );
        values.insert(
            ParameterId::from("link_channels"),
            ParameterValue::Bool(self.link_channels),
        );
        values.insert(
            ParameterId::from("mode"),
            ParameterValue::Int(self.mode as i32),
        );
        values.insert(
            ParameterId::from("bands"),
            ParameterValue::Int(self.bands as i32),
        );
        values.insert(
            ParameterId::from("crossover_hz"),
            ParameterValue::Float(self.crossover_hz),
        );
        values.insert(
            ParameterId::from("frequency_skew"),
            ParameterValue::Float(self.frequency_skew),
        );
        values.insert(
            ParameterId::from("repair_width"),
            ParameterValue::Int(self.repair_width as i32),
        );
        values.insert(
            ParameterId::from("audition_residual"),
            ParameterValue::Bool(self.audition_residual),
        );
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return Err("Reset declick before changing parameters after drain starts".into());
        }
        // Validate the complete batch before mutating DSP state. The cache is
        // updated in place, so successful automation does not rebuild a Vec.
        let mut next = self.current_params();
        for (id, value) in &values {
            param_bridge::set_parameter(DC, id, value, |i, v| match i {
                0 => next.enabled = v > 0.5,
                1 => next.sensitivity = v as f32,
                2 => next.link_channels = v > 0.5,
                3 => next.mode = v as usize,
                4 => next.bands = v as usize,
                5 => next.crossover_hz = v as f32,
                6 => next.frequency_skew = v as f32,
                7 => next.repair_width = v as usize,
                8 => next.audition_residual = v > 0.5,
                _ => {}
            })?;
        }
        next.mode = next.mode.min(MODE_OPTIONS.len() - 1);
        next.bands = next.bands.min(BANDS_OPTIONS.len() - 1);
        next.repair_width = next.repair_width.min(MAX_REPAIR_WIDTH);
        self.commit_params(&next);
        Ok(())
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return Err("Reset declick before changing parameters after drain starts".into());
        }
        let mut next = self.current_params();
        param_bridge::set_parameter(DC, &id, &value, |i, v| match i {
            0 => next.enabled = v > 0.5,
            1 => next.sensitivity = v as f32,
            2 => next.link_channels = v > 0.5,
            3 => next.mode = v as usize,
            4 => next.bands = v as usize,
            5 => next.crossover_hz = v as f32,
            6 => next.frequency_skew = v as f32,
            7 => next.repair_width = v as usize,
            8 => next.audition_residual = v > 0.5,
            _ => {}
        })?;
        next.mode = next.mode.min(MODE_OPTIONS.len() - 1);
        next.bands = next.bands.min(BANDS_OPTIONS.len() - 1);
        next.repair_width = next.repair_width.min(MAX_REPAIR_WIDTH);
        self.commit_params(&next);
        Ok(())
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("declick sample rate must be greater than zero".into());
        }
        self.legacy.set_sample_rate(sample_rate)?;
        self.owned.set_sample_rate(sample_rate)?;
        self.initialized_sample_rate = sample_rate;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.reset_dsp_only();
        self.has_input = false;
        self.drain_remaining = None;
        self.drain_silence.fill(0.0);
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "Frame/channel count overflow".to_string())?;
        if buffer.len() != expected {
            return Err(format!(
                "Buffer size mismatch: expected {}, got {}",
                expected,
                buffer.len()
            ));
        }
        if self.channels == 0 {
            return Err("declick requires at least one channel".into());
        }
        if context.sample_rate != self.initialized_sample_rate {
            return Err(format!(
                "declick sample-rate mismatch: initialized at {}, context is {}",
                self.initialized_sample_rate, context.sample_rate
            ));
        }
        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("Reset declick before processing input after drain starts".into());
        }
        if self.use_legacy() {
            self.legacy.process_frames(buffer)?;
        } else {
            self.owned.process(buffer)?;
        }
        self.has_input |= context.num_frames > 0;
        Ok(context.num_frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.latency_samples()
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        // The advertised buffer holds the entire continuation (up to sixteen
        // frames), so one call suffices when capacity covers the latency.
        std::num::NonZeroU64::new(1)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if context.sample_rate != self.initialized_sample_rate {
            return Err("Declick drain sample-rate mismatch".into());
        }
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if output.is_empty() || !output.len().is_multiple_of(self.channels) {
            return Err("Declick drain requires a positive frame-aligned destination".into());
        }
        let remaining = self.drain_remaining.unwrap_or(self.latency_samples());
        let frames = remaining.min(output.len() / self.channels);
        let samples = frames * self.channels;
        self.drain_silence[..samples].fill(0.0);
        if self.use_legacy() {
            self.legacy
                .process_frames(&mut self.drain_silence[..samples])?;
        } else {
            // Declared drain (R35): identical DSP, explicit EOF context.
            self.owned
                .process_drain(&mut self.drain_silence[..samples])?;
        }
        output[..samples].copy_from_slice(&self.drain_silence[..samples]);
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: frames == remaining,
        })
    }

    fn tail_length(&self) -> TailLength {
        // A zero candidate has eight zero future neighbors: any nonzero
        // baseline gives an excursion longer than the repair limit of six.
        // Thus no repaired audio is synthesized after the delayed input ends.
        // Widened paths extend the same argument to the reported latency:
        // neighbors join a repair only through the hysteresis gate, which
        // keeps the return and excursion shape tests unrelaxed.
        TailLength::Finite(self.latency_samples() as u64)
    }

    fn latency_samples(&self) -> usize {
        if self.use_legacy() {
            self.legacy.suppressor.latency_samples()
        } else {
            self.owned.latency_samples()
        }
    }
}
