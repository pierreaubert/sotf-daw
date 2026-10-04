//! RNNoise speech denoiser host adapter.
//!
//! Wraps the shared [`RnnoiseBackend`](plugins_denoiser::rnnoise::RnnoiseBackend)
//! with SOTF host traits, a suppression-strength blend, and a validated model
//! registry. RNNoise runs at 48 kHz; mono/stereo host streams at other finite
//! positive rates use a prepared streaming converter. Wider layouts are rejected.
//! The dry path stays at the full host bandwidth. At host rates above 48 kHz,
//! the 100%-wet path is limited by RNNoise's 24 kHz Nyquist frequency.

pub mod model;
pub mod params;
mod rate_adapter;

pub use crate::model::SpeechDenoiserModel;
use crate::params::PARAMS as SP;
use crate::rate_adapter::RateAdapter;
use plugins_denoiser::rnnoise::RnnoiseBackend;
pub use plugins_denoiser::rnnoise::{
    RNNOISE_BAND_COUNT, RnnoiseAnalyzerData as SpeechDenoiserData,
};
use serde::{Deserialize, Serialize};
use sotf_host::analyzer::RealTimeCache;
use sotf_host::param_bridge;
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use std::any::Any;
use std::sync::Arc;

/// RNNoise processes fixed 480-sample frames at 48 kHz.
pub const SPEECH_DENOISER_FRAME_SIZE: usize = 480;

/// Total signal delay in frames: 480 model frames plus the 480-frame queue.
pub const SPEECH_DENOISER_LATENCY_FRAMES: usize = 960;

/// Full-scale strength smoothing length in frames (10 ms at 48 kHz).
///
/// Matches the 480-sample bypass crossfade convention so suppression changes
/// are click-free without adding latency.
const STRENGTH_SMOOTHING_FRAMES: f32 = SPEECH_DENOISER_FRAME_SIZE as f32;

/// Sanitizes one input sample exactly like the backend model path.
fn sanitize_dry_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Rejects non-finite or out-of-range suppression strength.
fn validate_strength(strength: f32) -> PluginResult<f32> {
    if !strength.is_finite() {
        return Err("strength must be finite".to_string());
    }
    if !(0.0..=1.0).contains(&strength) {
        return Err(format!("strength {strength} is outside 0..=1"));
    }
    Ok(strength)
}

/// Resolves a model parameter value to a registry entry.
///
/// Accepts an Int choice index or a String label; both name the same entry.
/// Shared by the transactional pre-check and the commit path so the two can
/// never disagree about which identities are valid.
fn resolve_model(value: &ParameterValue) -> PluginResult<SpeechDenoiserModel> {
    match value {
        ParameterValue::Int(index) => usize::try_from(*index)
            .ok()
            .and_then(SpeechDenoiserModel::from_index)
            .ok_or_else(|| format!("unknown speech denoiser model index: {index}")),
        ParameterValue::String(label) => SpeechDenoiserModel::from_label(label)
            .ok_or_else(|| format!("unknown speech denoiser model: {label:?}")),
        _ => Err("model must be an Int index or String label".to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechDenoiserPluginParams {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_strength")]
    pub strength: f32,
    #[serde(default)]
    pub model: SpeechDenoiserModel,
}

fn default_enabled() -> bool {
    true
}

fn default_strength() -> f32 {
    pk(SP, "strength").default_f32()
}

impl Default for SpeechDenoiserPluginParams {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            strength: default_strength(),
            model: SpeechDenoiserModel::default(),
        }
    }
}

/// RNNoise speech denoiser with a fixed 960-frame processing latency.
///
/// Suppression strength blends latency-aligned wet and dry audio per sample:
/// `out = dry + s * (wet - dry)`, where `dry` is the sanitized input delayed
/// by exactly the 960-frame signal latency and `wet` is the RNNoise output.
/// Strength slews toward its target over 480 frames; the 0.0 and 1.0 endpoints
/// emit dry and wet bit-exactly, so default and disabled audio are unchanged.
///
/// Enabled end-of-stream drain emits one 960-frame zero-continuation window to
/// release accepted programme audio, then resets the backend. The model and
/// high-pass response remain `Unknown`; this declared render cutoff does not
/// claim that their natural recursive response is finite.
pub struct SpeechDenoiserPlugin {
    channels: usize,
    enabled: bool,
    strength_target: f32,
    strength_current: f32,
    model: SpeechDenoiserModel,
    inner: RnnoiseBackend,
    dry_delay: Vec<Vec<f32>>,
    dry_pos: usize,
    rate_adapter: Option<RateAdapter>,
    strength_smoothing_frames: f32,
    cached_parameters: Vec<Parameter>,
    initialized_sample_rate: Option<f64>,
    analyzer_cache: RealTimeCache<SpeechDenoiserData>,
    published_model_frames: u64,
    has_input: bool,
    drain_remaining: Option<usize>,
}

impl SpeechDenoiserPlugin {
    pub fn new(channels: usize) -> Self {
        Self::from_params(channels, SpeechDenoiserPluginParams::default())
    }

    pub fn from_params(channels: usize, params: SpeechDenoiserPluginParams) -> Self {
        // The infallible programmatic constructor clamps defensively; the
        // fallible factory path (`try_from_params`) rejects bad values so a
        // malformed saved state can never silently change suppression depth.
        let strength = if params.strength.is_finite() {
            params.strength.clamp(0.0, 1.0)
        } else {
            default_strength()
        };
        let mut plugin = Self {
            channels,
            enabled: params.enabled,
            strength_target: strength,
            strength_current: strength,
            model: params.model,
            inner: RnnoiseBackend::new(),
            dry_delay: Vec::new(),
            dry_pos: 0,
            rate_adapter: None,
            strength_smoothing_frames: STRENGTH_SMOOTHING_FRAMES,
            cached_parameters: Vec::new(),
            initialized_sample_rate: None,
            analyzer_cache: RealTimeCache::new_triplet(
                SpeechDenoiserData::default(),
                SpeechDenoiserData::default(),
                SpeechDenoiserData::default(),
            ),
            published_model_frames: 0,
            has_input: false,
            drain_remaining: None,
        };
        plugin.rebuild_cached_parameters();
        plugin
    }

    pub fn try_from_params(
        channels: usize,
        params: SpeechDenoiserPluginParams,
    ) -> PluginResult<Self> {
        if !(1..=2).contains(&channels) {
            return Err(format!(
                "Speech Denoiser supports mono or stereo only; got {channels} channels"
            ));
        }
        validate_strength(params.strength)?;
        Ok(Self::from_params(channels, params))
    }

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.enabled { 1.0 } else { 0.0 }),
            1 => Some(f64::from(self.strength_target)),
            2 => Some(self.model.index() as f64),
            _ => None,
        }
    }

    fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = param_bridge::build_parameters(SP, |i| self.param_value(i));
    }

    fn sync_cached_default(&mut self, id: &ParameterId, value: ParameterValue) {
        if let Some(parameter) = self
            .cached_parameters
            .iter_mut()
            .find(|parameter| parameter.id == *id)
        {
            parameter.default_value = value;
        }
    }

    /// Apply already-owned parameter storage without taking ownership of it.
    /// Hosts that automate on the realtime thread should use this borrowed
    /// path so destruction of the caller's parameter map remains off-callback.
    pub fn apply_values_realtime(&mut self, values: &ParameterSet) -> PluginResult<()> {
        for (id, value) in values {
            self.parametric_validate_parameter(id, value)?;
        }
        // Transactionality (COMMON §2): pre-check the state-dependent
        // preconditions of every entry before committing any, so a mixed
        // batch carrying one rejected entry leaves the accepted configuration
        // and populated history untouched. Each commit below only touches its
        // own field plus its cached default, and neither drain state nor the
        // initialized flag changes between the passes, so the commit pass
        // cannot fail once the pre-check pass succeeds.
        for (id, value) in values {
            self.precheck_value_ref(id, value)?;
        }
        for (id, value) in values {
            self.apply_value_ref(id, value)?;
        }
        Ok(())
    }

    /// Checks state-dependent preconditions without mutating anything.
    ///
    /// Must mirror [`apply_value_ref`](Self::apply_value_ref)'s rejection
    /// conditions exactly: the transactional batch path runs this for every
    /// entry before committing any. Allocation-free on success (reads and
    /// comparisons only); error strings allocate on the rejection path, which
    /// the host handles off the sample loop.
    fn precheck_value_ref(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        match id.as_str() {
            "enabled" => {
                let enabled = value
                    .as_bool()
                    .ok_or_else(|| "enabled must be a boolean".to_string())?;
                if enabled != self.enabled && self.drain_remaining.is_some() {
                    return Err(
                        "Speech Denoiser must be reset after drain before changing enabled".into(),
                    );
                }
                Ok(())
            }
            "strength" => {
                let strength = value
                    .as_float()
                    .ok_or_else(|| "strength must be a float".to_string())?;
                validate_strength(strength)?;
                if strength != self.strength_target && self.drain_remaining.is_some() {
                    return Err(
                        "Speech Denoiser must be reset after drain before changing strength".into(),
                    );
                }
                Ok(())
            }
            "model" => {
                let model = resolve_model(value)?;
                if model != self.model {
                    if self.drain_remaining.is_some() {
                        return Err(
                            "Speech Denoiser must be reset after drain before changing model"
                                .into(),
                        );
                    }
                    if self.initialized_sample_rate.is_some() {
                        return Err(
                            "Speech Denoiser model changes require graph rebuild".to_string()
                        );
                    }
                }
                Ok(())
            }
            _ => Err(format!("Unknown parameter: {id}")),
        }
    }

    fn apply_value_ref(&mut self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        match id.as_str() {
            "enabled" => {
                let enabled = value
                    .as_bool()
                    .ok_or_else(|| "enabled must be a boolean".to_string())?;
                if enabled == self.enabled {
                    return Ok(());
                }
                if self.drain_remaining.is_some() {
                    return Err(
                        "Speech Denoiser must be reset after drain before changing enabled".into(),
                    );
                }
                self.enabled = enabled;
                self.sync_cached_default(id, ParameterValue::Bool(self.enabled));
                Ok(())
            }
            "strength" => {
                let strength = value
                    .as_float()
                    .ok_or_else(|| "strength must be a float".to_string())?;
                validate_strength(strength)?;
                if strength == self.strength_target {
                    return Ok(());
                }
                if self.drain_remaining.is_some() {
                    return Err(
                        "Speech Denoiser must be reset after drain before changing strength".into(),
                    );
                }
                self.strength_target = strength;
                self.sync_cached_default(id, ParameterValue::Float(self.strength_target));
                Ok(())
            }
            "model" => {
                let model = resolve_model(value)?;
                if model == self.model {
                    return Ok(());
                }
                if self.drain_remaining.is_some() {
                    return Err(
                        "Speech Denoiser must be reset after drain before changing model".into(),
                    );
                }
                // Model weights are prepared off the audio callback: changing
                // the identity of a live instance requires a host graph
                // rebuild from serialized configuration. The running model
                // keeps processing until that rebuild succeeds.
                if self.initialized_sample_rate.is_some() {
                    return Err("Speech Denoiser model changes require graph rebuild".to_string());
                }
                self.model = model;
                self.sync_cached_default(id, ParameterValue::Int(self.model.index() as i32));
                Ok(())
            }
            _ => Err(format!("Unknown parameter: {id}")),
        }
    }

    fn advance_strength(&mut self) {
        let step = 1.0 / self.strength_smoothing_frames;
        if self.strength_current < self.strength_target {
            self.strength_current = (self.strength_current + step).min(self.strength_target);
        } else if self.strength_current > self.strength_target {
            self.strength_current = (self.strength_current - step).max(self.strength_target);
        }
    }

    fn blend_chunk(&mut self, chunk: &mut [f32], dry: &[f32], chunk_frames: usize) {
        let channels = self.channels;
        for frame in 0..chunk_frames {
            self.advance_strength();
            let strength = self.strength_current;
            if strength >= 1.0 {
                // Bit-exact wet passthrough at full suppression.
                continue;
            }
            if strength <= 0.0 {
                for ch in 0..channels {
                    chunk[frame * channels + ch] = dry[frame * channels + ch];
                }
                continue;
            }
            for ch in 0..channels {
                let wet = chunk[frame * channels + ch];
                let dry_sample = dry[frame * channels + ch];
                chunk[frame * channels + ch] = dry_sample + strength * (wet - dry_sample);
            }
        }
    }

    /// Captures dry history, runs one backend chunk, and blends the result.
    ///
    /// The backend already subdivides calls into 480-frame pieces internally,
    /// so per-chunk invocation produces its identical sample sequence while
    /// bounding the stack scratch used for the aligned dry signal.
    fn process_chunk(&mut self, chunk: &mut [f32], chunk_frames: usize) -> PluginResult<()> {
        let channels = self.channels;
        if channels == 0 || self.dry_delay.len() != channels {
            return Err("Speech Denoiser dry delay is not initialized".to_string());
        }
        debug_assert_eq!(chunk.len(), chunk_frames * channels);
        debug_assert!(chunk_frames <= SPEECH_DENOISER_FRAME_SIZE);
        let mut dry_scratch = [0.0f32; SPEECH_DENOISER_FRAME_SIZE * 2];
        let ring_len = self.dry_delay[0].len();
        for frame in 0..chunk_frames {
            for ch in 0..channels {
                let sanitized = sanitize_dry_sample(chunk[frame * channels + ch]);
                let pos = self.dry_pos;
                dry_scratch[frame * channels + ch] = self.dry_delay[ch][pos];
                self.dry_delay[ch][pos] = sanitized;
            }
            self.dry_pos += 1;
            if self.dry_pos >= ring_len {
                self.dry_pos = 0;
            }
        }
        let written = self
            .inner
            .process(chunk, chunk_frames, channels, !self.enabled);
        if written != chunk_frames {
            return Err(format!(
                "RNNoise processed {written} of {chunk_frames} requested frames"
            ));
        }
        if self.enabled {
            let dry_len = chunk_frames * channels;
            self.blend_chunk(chunk, &dry_scratch[..dry_len], chunk_frames);
        } else {
            // The bypass path replays backend audio unchanged, but smoothing
            // state keeps tracking the target so re-enabling starts clean.
            for _ in 0..chunk_frames {
                self.advance_strength();
            }
        }
        Ok(())
    }

    fn process_backend(&mut self, buffer: &mut [f32], frames: usize) -> PluginResult<usize> {
        let channels = self.channels;
        let mut processed = 0;
        while processed < frames {
            let chunk_frames = (frames - processed).min(SPEECH_DENOISER_FRAME_SIZE);
            let start = processed * channels;
            let end = start + chunk_frames * channels;
            self.process_chunk(&mut buffer[start..end], chunk_frames)?;
            processed += chunk_frames;
        }
        let analyzer_data = self.inner.analyzer_data();
        if analyzer_data.model_frames != self.published_model_frames {
            self.analyzer_cache.update(|data| *data = analyzer_data);
            self.published_model_frames = analyzer_data.model_frames;
        }
        Ok(frames)
    }

    fn process_at_host_rate(&mut self, buffer: &mut [f32], frames: usize) -> PluginResult<usize> {
        let mut processed = 0;
        while processed < frames {
            let chunk_frames = (frames - processed).min(64);
            let start = processed * self.channels;
            let end = start + chunk_frames * self.channels;
            let mut dry = [0.0_f32; 128];
            {
                let adapter = self.rate_adapter.as_mut().ok_or("speech adapter is missing")?;
                adapter.process_chunk(
                    &mut buffer[start..end],
                    &mut dry[..chunk_frames * self.channels],
                    &mut self.inner,
                    !self.enabled,
                )?;
            }
            if self.enabled {
                self.blend_chunk(&mut buffer[start..end], &dry, chunk_frames);
            } else {
                buffer[start..end].copy_from_slice(&dry[..chunk_frames * self.channels]);
                for _ in 0..chunk_frames {
                    self.advance_strength();
                }
            }
            processed += chunk_frames;
        }
        let analyzer_data = self.inner.analyzer_data();
        if analyzer_data.model_frames != self.published_model_frames {
            self.analyzer_cache.update(|data| *data = analyzer_data);
            self.published_model_frames = analyzer_data.model_frames;
        }
        Ok(frames)
    }
}

impl ParametricInPlacePlugin for SpeechDenoiserPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Speech Denoiser", env!("CARGO_PKG_VERSION"), "SotF")
            .with_description("RNNoise speech denoiser")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Fft
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(PluginCostClass::Fft, None, self.latency_samples(), false)
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn parametric_validate_parameter(
        &self,
        id: &ParameterId,
        value: &ParameterValue,
    ) -> PluginResult<()> {
        // Choice models travel as an Int index or a String label; both name
        // the same registry entry and reject unknown identities. Value
        // resolution is canonical in `resolve_model` so validation and the
        // commit path can never disagree about valid identities.
        if id.as_str() == "model" {
            return resolve_model(value)
                .map(|_| ())
                .map_err(|error| format!("model: {error}"));
        }
        let parameter = self
            .cached_parameters
            .iter()
            .find(|parameter| &parameter.id == id)
            .ok_or_else(|| format!("Unknown parameter: {id}"))?;
        parameter
            .validate(value)
            .map_err(|error| format!("{id}: {error}"))
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        self.parametric_validate_parameter(&id, &value)?;
        self.apply_value_ref(&id, &value)
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "enabled" => Some(ParameterValue::Bool(self.enabled)),
            "strength" => Some(ParameterValue::Float(self.strength_target)),
            "model" => Some(ParameterValue::Int(self.model.index() as i32)),
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
            ParameterId::from("strength"),
            ParameterValue::Float(self.strength_target),
        );
        values.insert(
            ParameterId::from("model"),
            ParameterValue::Int(self.model.index() as i32),
        );
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        self.apply_values_realtime(&values)
    }

    /// Initialize the plugin at the given sample rate.
    ///
    /// RNNoise itself stays at 48 kHz. Other finite positive host rates use a
    /// prepared streaming adapter with a fixed host-frame delay. The selected
    /// model is parsed, transposed, and built here, off
    /// the audio callback; a failed load retains the previous backend with
    /// its accepted model and populated history.
    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("Speech denoiser sample rate must be finite and positive".into());
        }
        let adapter = if sample_rate == 48_000.0 {
            None
        } else {
            Some(RateAdapter::new(sample_rate, self.channels)?)
        };
        let strength_frames = (sample_rate * 0.01).round();
        if !strength_frames.is_finite() || strength_frames < 1.0
            || strength_frames >= f32::MAX as f64
        {
            return Err("Speech denoiser strength horizon is unsupported".into());
        }
        self.inner
            .initialize_with_model(48_000, self.channels, self.model.backend_id())?;
        let latency = self.inner.latency_samples();
        debug_assert_eq!(latency, SPEECH_DENOISER_LATENCY_FRAMES);
        self.dry_delay = if adapter.is_none() {
            vec![vec![0.0; latency]; self.channels]
        } else {
            Vec::new()
        };
        self.dry_pos = 0;
        self.rate_adapter = adapter;
        self.strength_smoothing_frames = strength_frames as f32;
        self.strength_current = self.strength_target;
        self.initialized_sample_rate = Some(sample_rate);
        self.has_input = false;
        self.drain_remaining = None;
        self.published_model_frames = 0;
        self.analyzer_cache
            .update(|data| *data = SpeechDenoiserData::default());
        Ok(())
    }

    fn reset(&mut self) {
        self.inner.reset();
        if let Some(adapter) = self.rate_adapter.as_mut() {
            adapter.reset();
        }
        for ring in &mut self.dry_delay {
            ring.fill(0.0);
        }
        self.dry_pos = 0;
        self.strength_current = self.strength_target;
        self.has_input = false;
        self.drain_remaining = None;
        self.published_model_frames = 0;
        self.analyzer_cache
            .update(|data| *data = SpeechDenoiserData::default());
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let Some(sample_rate) = self.initialized_sample_rate else {
            return Err("Speech Denoiser must be initialized before processing".to_string());
        };
        if context.sample_rate != sample_rate {
            return Err(format!(
                "Speech Denoiser context rate {} Hz differs from initialized rate {sample_rate} Hz",
                context.sample_rate
            ));
        }
        // Validate buffer length before touching any index.
        let expected_len = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| {
                format!(
                    "Buffer size overflow: num_frames={} * channels={}",
                    context.num_frames, self.channels
                )
            })?;
        if buffer.len() < expected_len {
            return Err(format!(
                "Buffer too small: {} < {} (num_frames={} * channels={})",
                buffer.len(),
                expected_len,
                context.num_frames,
                self.channels
            ));
        }

        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("Speech Denoiser must be reset after drain before processing input".into());
        }
        // A zero-frame callback is a state-neutral query. In particular, the
        // final enabled drain resets the backend while retaining its last
        // published telemetry; forwarding this no-op to the backend would
        // publish the reset analyzer frame over that final snapshot.
        if context.num_frames == 0 {
            return Ok(0);
        }
        let written = if self.rate_adapter.is_some() {
            self.process_at_host_rate(buffer, context.num_frames)?
        } else {
            self.process_backend(buffer, context.num_frames)?
        };
        self.has_input |= written > 0;
        Ok(written)
    }

    fn tail_length(&self) -> TailLength {
        if self.initialized_sample_rate.is_some() && !self.enabled {
            TailLength::Finite(self.latency_samples() as u64)
        } else {
            TailLength::Unknown
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        // Structural capacity must be available before wrapper preparation feeds input.
        if self.rate_adapter.is_some() { 64 } else { SPEECH_DENOISER_FRAME_SIZE }
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.initialized_sample_rate?;
        let remaining = if !self.has_input {
            0
        } else {
            self.drain_remaining.unwrap_or_else(|| {
                self.rate_adapter.as_ref().map_or_else(
                    || self.latency_samples(),
                    |adapter| if self.enabled { adapter.drain_frames() } else { adapter.latency() },
                )
            })
        };
        std::num::NonZeroU64::new(remaining.div_ceil(self.drain_output_frames_max()).max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        let rate = self
            .initialized_sample_rate
            .ok_or("Speech Denoiser must be initialized before drain")?;
        if context.sample_rate != rate {
            return Err("Speech Denoiser drain sample-rate mismatch".into());
        }
        if !output.len().is_multiple_of(self.channels) {
            return Err("Speech Denoiser drain requires whole output frames".into());
        }
        // Empty streams do not freeze, regardless of enabled state.
        if !self.has_input {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if self.rate_adapter.is_some() {
            let total = self.rate_adapter.as_ref().unwrap();
            let total = if self.enabled { total.drain_frames() } else { total.latency() };
            let remaining = self.drain_remaining.unwrap_or(total);
            if remaining == 0 {
                return Ok(PluginDrainResult::COMPLETE);
            }
            let frames = (output.len() / self.channels).min(64).min(remaining);
            if frames == 0 {
                return Err("Speech Denoiser drain requires positive output capacity".into());
            }
            self.drain_remaining = Some(remaining);
            output[..frames * self.channels].fill(0.0);
            self.process_at_host_rate(&mut output[..frames * self.channels], frames)?;
            let next = remaining - frames;
            self.drain_remaining = Some(next);
            if next == 0 && self.enabled {
                self.inner.reset();
                if let Some(adapter) = self.rate_adapter.as_mut() {
                    adapter.reset();
                }
                self.strength_current = self.strength_target;
            }
            return Ok(PluginDrainResult { frames, complete: next == 0 });
        }
        let remaining = self.drain_remaining.unwrap_or(self.latency_samples());
        if remaining == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let frames = (output.len() / self.channels)
            .min(SPEECH_DENOISER_FRAME_SIZE)
            .min(remaining);
        if frames == 0 {
            return Err("Speech Denoiser drain requires positive output capacity".into());
        }
        // Freeze changed input/control state as soon as valid drain work begins.
        self.drain_remaining = Some(remaining);
        let samples = frames * self.channels;
        output[..samples].fill(0.0);
        self.process_backend(&mut output[..samples], frames)?;
        let next_remaining = remaining - frames;
        self.drain_remaining = Some(next_remaining);
        if next_remaining == 0 && self.enabled {
            // The enabled model/high-pass response is Unknown. Emit the fixed
            // accepted-program window, then discard any residual recursive
            // response so COMPLETE is terminal without claiming finite support.
            // Keep analyzer_cache intact: it holds the last published frame.
            self.inner.reset();
            for ring in &mut self.dry_delay {
                ring.fill(0.0);
            }
            self.dry_pos = 0;
            self.strength_current = self.strength_target;
        }
        Ok(PluginDrainResult {
            frames,
            complete: next_remaining == 0,
        })
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.analyzer_cache.load() as Arc<dyn Any + Send + Sync>)
    }

    /// Returns the prepared host-frame latency regardless of the `enabled` flag.
    ///
    /// Plugin hosts require latency to remain constant after initialisation.
    /// At 48 kHz this is the original 960 frames. At other rates it includes
    /// the prepared conversion, model, chunking, and wet-alignment delays.
    /// Returning 0 when disabled would cause phase cancellation in parallel
    /// processing chains and misalignment with other latency-compensated
    /// tracks.
    fn latency_samples(&self) -> usize {
        self.rate_adapter.as_ref().map_or_else(
            || self.inner.latency_samples(),
            RateAdapter::latency,
        )
    }
}
