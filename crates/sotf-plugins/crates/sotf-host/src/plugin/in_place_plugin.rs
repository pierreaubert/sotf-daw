use super::plugin_info::PluginInfo;
use super::process_context::ProcessContext;
use super::types::{PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginResult};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use std::any::Any;
use std::sync::Arc;

/// Helper trait for plugins that process audio in-place (input channels == output channels)
pub trait InPlacePlugin: Send {
    /// Get plugin information
    fn info(&self) -> PluginInfo;

    /// Get the number of output channels (same as input by default)
    fn channels(&self) -> usize;

    /// Get the number of input channels this plugin expects.
    /// Override this when the plugin needs more input channels than output channels,
    /// e.g. for external sidechain support where extra channels carry the sidechain signal.
    /// Default: same as `channels()`.
    fn input_channels(&self) -> usize {
        self.channels()
    }

    /// Allow bounded subdivision with exact audio and signal-state equivalence.
    ///
    /// Opting in guarantees full consumption of every valid positive block,
    /// with identical audio and signal state under arbitrary ordered partitions.
    /// Processing uses only `sample_rate` and `num_frames` from the context;
    /// transport and event slices must be ignored. Native f64 obeys the same
    /// contract when supported. Diagnostic publication may follow subcalls.
    ///
    /// After initialization, finite correctly shaped input at the initialized
    /// rate must succeed and return exactly `num_frames`. Lifecycle/rate errors
    /// must precede mutation; no later subcall may fail for that same context.
    /// Standard adapters prepare bounded scratch during initialization, without
    /// imposing a maximum public callback size. Asymmetric adapter layouts
    /// require this opt-in; direct in-place calls retain their full input stride.
    fn supports_bounded_subdivision(&self) -> bool {
        false
    }

    /// Get the list of parameters this plugin supports
    fn parameters(&self) -> Vec<Parameter>;

    /// Set a parameter value
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()>;

    /// Helper to validate a parameter value against its definition
    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        let params = self.parameters();
        if let Some(param) = params.iter().find(|p| p.id == *id) {
            param.validate(value).map_err(|e| format!("{}: {}", id, e))
        } else {
            Err(format!("Unknown parameter: {}", id))
        }
    }

    /// Get a parameter value
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue>;

    /// Initialize the plugin with the given sample rate
    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        let _ = sample_rate;
        Ok(())
    }

    /// Reset the plugin state
    fn reset(&mut self) {
        // Default: no-op
    }

    /// Process audio samples in-place
    ///
    /// # Arguments
    /// * `buffer` - Interleaved audio samples [C0_F0, C1_F0, ..., C0_F1, C1_F1, ...]
    ///   Length is num_frames * channels()
    /// * `context` - Processing context
    ///
    /// Standard adapters reject non-finite samples and malformed lengths before
    /// calling this method, so rejected public `Plugin::process` blocks do not
    /// advance the wrapped plugin's state. Direct callers of this lower-level
    /// trait are responsible for providing the same validated contract.
    ///
    /// # Returns
    /// Actual number of frames processed, or error message
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize>;

    /// Maximum output-rate frames returned by one end-of-stream drain step.
    /// See [`super::Plugin::drain_output_frames_max`].
    fn drain_output_frames_max(&self) -> usize {
        0
    }

    /// Stream-independent upper bound on one drain step's emission.
    /// See [`super::Plugin::drain_frames_envelope`]. `None` (the default)
    /// keeps the host's live sizing for this plugin.
    fn drain_frames_envelope(&self) -> Option<usize> {
        None
    }

    /// Stream-independent upper bound on in-place process production.
    /// See [`super::Plugin::output_frames_envelope`]. `None` (the default)
    /// keeps the host's live sizing for this plugin. Only plugins whose
    /// every success path returns exactly `context.num_frames` may return
    /// `Some(input_frames)`.
    fn output_frames_envelope(&self, _input_frames: usize) -> Option<usize> {
        None
    }

    /// Finish already accepted asynchronous work before querying EOS metadata.
    /// See [`super::Plugin::prepare_drain_metadata`].
    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        Ok(())
    }

    /// Refresh native metadata on the plugin's serialized control thread.
    /// See [`super::Plugin::refresh_control_thread_metadata`].
    fn refresh_control_thread_metadata(&mut self) {}

    /// Prepare bounded, idempotent EOS work; see [`super::Plugin::begin_drain`].
    fn begin_drain(&mut self, _context: &ProcessContext) -> PluginResult<()> {
        Ok(())
    }

    /// Bound full-capacity EOS calls; see [`super::Plugin::drain_call_bound`].
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        None
    }

    /// Flush buffered samples without accepting more programme input.
    /// The allocation, completion and capacity contracts match [`super::Plugin::drain`].
    fn drain(
        &mut self,
        _output: &mut [f32],
        _context: &ProcessContext,
    ) -> PluginResult<super::PluginDrainResult> {
        Ok(super::PluginDrainResult::COMPLETE)
    }

    /// Optional specialized operation used by host compiled render plans.
    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        let _ = (op, input, output, context);
        None
    }

    /// Stable scalar gain that a host compiled plan may fuse with adjacent ops.
    fn compiled_static_gain(&self) -> Option<f32> {
        None
    }

    /// Parameter-sensitive compile/fusion metadata for this plugin state.
    fn compile_metadata(&self) -> PluginCompileMetadata {
        let mut metadata =
            PluginCompileMetadata::boundary(self.cost_class(), self.latency_samples());
        metadata.static_gain = self.compiled_static_gain();
        metadata
    }

    /// Process f64 audio samples in-place. Override together with
    /// `supports_f64()` for true double-precision DSP.
    fn process_in_place_f64(
        &mut self,
        buffer: &mut [f64],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let mut buffer_f32 = vec![0.0; buffer.len()];
        for (dst, &src) in buffer_f32.iter_mut().zip(buffer.iter()) {
            *dst = src as f32;
        }
        let frames = self.process_in_place(&mut buffer_f32, context)?;
        for (dst, &src) in buffer.iter_mut().zip(buffer_f32.iter()) {
            *dst = src as f64;
        }
        Ok(frames)
    }

    /// Get the processing latency in samples (if any)
    fn latency_samples(&self) -> usize {
        0
    }

    /// Allocation-free zero-input response bound, as defined by [`super::TailLength`].
    fn tail_length(&self) -> super::TailLength {
        super::TailLength::Unknown
    }

    /// State-independent zero-input response bound; see [`super::Plugin::tail_support`].
    fn tail_support(&self) -> Option<u64> {
        None
    }

    /// Minimum input-rate scheduling budget for worst-case realtime work.
    fn realtime_quantum_frames(&self) -> usize {
        1
    }

    /// Coarse cost category for host scheduling.
    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Scalar
    }

    /// Get data from the plugin (if it's an analyzer or exposes internal state)
    /// Returns `None` by default for plugins that don't expose data.
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    /// Returns the plugin's preferred oversampling factor, if any.
    /// When `Some(n)`, the host may insert oversampling before/after this plugin.
    /// `n` must be 2 or 4.
    fn preferred_oversampling(&self) -> Option<u32> {
        None
    }

    /// Whether this plugin can process in f64 precision.
    /// When true, the host may provide f64 buffers via a future `process_f64()` method.
    fn supports_f64(&self) -> bool {
        false
    }
}
