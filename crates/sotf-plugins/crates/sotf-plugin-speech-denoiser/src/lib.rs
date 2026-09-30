pub mod params;

use crate::params::PARAMS as SP;
use plugins_denoiser::rnnoise::RnnoiseBackend;
pub use plugins_denoiser::rnnoise::{
    RNNOISE_BAND_COUNT, RnnoiseAnalyzerData as SpeechDenoiserData,
};
use serde::{Deserialize, Serialize};
use sotf_host::analyzer::RealTimeCache;
use sotf_host::param_bridge;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechDenoiserPluginParams {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for SpeechDenoiserPluginParams {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
        }
    }
}

/// RNNoise speech denoiser with a fixed 960-frame processing latency.
///
/// Enabled end-of-stream drain emits one 960-frame zero-continuation window to
/// release accepted programme audio, then resets the backend. The model and
/// high-pass response remain `Unknown`; this declared render cutoff does not
/// claim that their natural recursive response is finite.
pub struct SpeechDenoiserPlugin {
    channels: usize,
    enabled: bool,
    inner: RnnoiseBackend,
    cached_parameters: Vec<Parameter>,
    initialized_sample_rate: Option<u32>,
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
        let mut plugin = Self {
            channels,
            enabled: params.enabled,
            inner: RnnoiseBackend::new(),
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
        Ok(Self::from_params(channels, params))
    }

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.enabled { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = param_bridge::build_parameters(SP, |i| self.param_value(i));
    }

    /// Apply already-owned parameter storage without taking ownership of it.
    /// Hosts that automate on the realtime thread should use this borrowed
    /// path so destruction of the caller's parameter map remains off-callback.
    pub fn apply_values_realtime(&mut self, values: &ParameterSet) -> PluginResult<()> {
        for (id, value) in values {
            self.parametric_validate_parameter(id, value)?;
        }
        for (id, value) in values {
            self.apply_value_ref(id, value)?;
        }
        Ok(())
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
                if let Some(parameter) = self
                    .cached_parameters
                    .iter_mut()
                    .find(|parameter| parameter.id == *id)
                {
                    parameter.default_value = ParameterValue::Bool(self.enabled);
                }
                Ok(())
            }
            _ => Err(format!("Unknown parameter: {id}")),
        }
    }

    fn process_backend(&mut self, buffer: &mut [f32], frames: usize) -> PluginResult<usize> {
        let written = self
            .inner
            .process(buffer, frames, self.channels, !self.enabled);
        if written != frames {
            return Err(format!(
                "RNNoise processed {written} of {frames} requested frames"
            ));
        }
        let analyzer_data = self.inner.analyzer_data();
        if analyzer_data.model_frames != self.published_model_frames {
            self.analyzer_cache.update(|data| *data = analyzer_data);
            self.published_model_frames = analyzer_data.model_frames;
        }
        Ok(written)
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
            _ => None,
        }
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        values.insert(
            ParameterId::from("enabled"),
            ParameterValue::Bool(self.enabled),
        );
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        self.apply_values_realtime(&values)
    }

    /// Initialize the plugin at the given sample rate.
    ///
    /// Returns `Err` if `sample_rate != 48000`; RNNoise is hard-coded for
    /// 48 kHz and will silently corrupt the frequency response at any other
    /// rate.
    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        self.inner.initialize(sample_rate, self.channels)?;
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
        let written = self.process_backend(buffer, context.num_frames)?;
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
        SPEECH_DENOISER_FRAME_SIZE
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.initialized_sample_rate?;
        let remaining = if !self.has_input {
            0
        } else {
            self.drain_remaining.unwrap_or(self.latency_samples())
        };
        std::num::NonZeroU64::new(remaining.div_ceil(SPEECH_DENOISER_FRAME_SIZE).max(1) as u64)
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
        }
        Ok(PluginDrainResult {
            frames,
            complete: next_remaining == 0,
        })
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.analyzer_cache.load() as Arc<dyn Any + Send + Sync>)
    }

    /// Returns a fixed latency of 960 samples regardless of the `enabled`
    /// flag.
    ///
    /// Plugin hosts require latency to remain constant after initialisation.
    /// RNNoise contributes 480 frames and arbitrary callback framing adds 480.
    /// Returning 0 when disabled would cause phase cancellation in parallel
    /// processing chains and misalignment with other latency-compensated
    /// tracks.
    fn latency_samples(&self) -> usize {
        self.inner.latency_samples()
    }
}
