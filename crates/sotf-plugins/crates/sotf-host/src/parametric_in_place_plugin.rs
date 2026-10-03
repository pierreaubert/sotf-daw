//! Parametric in-place plugin trait and adapter.
//!
//! `ParametricInPlacePlugin` is the in-place analogue of [`ParametricPlugin`].
//! A plugin implements `parameter_schema`, `current_values`, `apply_values` and
//! a small handful of lifecycle/processing hooks; the
//! [`ParametricInPlacePluginAdapter`] then turns it into a regular
//! [`InPlacePlugin`] whose `parameters`, `set_parameter` and `get_parameter`
//! methods are derived automatically from the schema.

use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::parametric_plugin::{ParameterSchema, ParameterSet};
use crate::plugin::bounded_in_place::{self, BoundedInPlace};
use crate::plugin::{
    InPlacePlugin, Plugin, PluginCompileMetadata, PluginCompiledOp, PluginCostClass,
    PluginDrainResult, PluginInfo, PluginResult, ProcessContext, validate_process_block_f32,
    validate_process_block_f64,
};
use std::any::Any;
use std::sync::Arc;

/// Trait for in-place plugins whose parameters can be described by a declarative schema.
///
/// Implementors only need to provide:
/// - static/dynamic metadata via [`parameter_schema`](Self::parameter_schema)
/// - current values via [`current_values`](Self::current_values)
/// - value application via [`apply_values`](Self::apply_values)
/// - audio processing via [`process_in_place`](Self::process_in_place)
///
/// The repetitive `parameters` / `set_parameter` / `get_parameter` wiring is
/// handled by [`ParametricInPlacePluginAdapter`].
pub trait ParametricInPlacePlugin: Send {
    /// Plugin metadata.
    fn info(&self) -> PluginInfo;

    /// Number of channels.
    fn channels(&self) -> usize;

    /// Number of input channels this plugin expects.
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

    /// Guarantees identity frame geometry for every supported process block.
    ///
    /// This is separate from bounded-subdivision support: an implementation
    /// may preserve frame count without promising partition-invariant state.
    /// Override only when every valid call returns exactly `context.num_frames`.
    fn guarantees_identity_frame_geometry(&self) -> bool {
        false
    }

    /// Parameter metadata. May be dynamic (e.g. per-channel gains).
    fn parameter_schema(&self) -> ParameterSchema;

    /// Current values for every parameter declared in the schema.
    fn current_values(&self) -> ParameterSet;

    /// Apply a new set of values to the plugin state.
    ///
    /// Values have already been validated against the schema. Implementations
    /// should ignore missing keys and treat unknown keys as an error.
    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()>;

    /// Initialize the plugin with the given sample rate.
    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        let _ = sample_rate;
        Ok(())
    }

    /// Reset plugin state.
    fn reset(&mut self) {}

    /// Process audio samples in-place.
    ///
    /// The public adapter validates exact block shape and finite input before
    /// entering this method. Direct lower-level callers must uphold that same
    /// contract themselves.
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize>;

    /// Maximum output-rate frames written by one [`Self::drain`] call.
    fn drain_output_frames_max(&self) -> usize {
        0
    }

    /// Stream-independent upper bound on one [`Self::drain`] call's
    /// emission; see [`Plugin::drain_frames_envelope`]. `None` (the
    /// default) keeps the host's live sizing for this plugin.
    fn drain_frames_envelope(&self) -> Option<usize> {
        None
    }

    /// Stream-independent upper bound on [`Self::process_in_place`]
    /// production; see [`Plugin::output_frames_envelope`]. `None` (the
    /// default) keeps the host's live sizing for this plugin. In-place
    /// processing still reports its own count, so only plugins whose every
    /// success path returns exactly `context.num_frames` may return
    /// `Some(input_frames)`.
    fn output_frames_envelope(&self, _input_frames: usize) -> Option<usize> {
        None
    }

    /// Finish already accepted asynchronous work before querying EOS metadata.
    /// See [`Plugin::prepare_drain_metadata`].
    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        Ok(())
    }

    /// Refresh native metadata on the plugin's serialized control thread.
    /// See [`Plugin::refresh_control_thread_metadata`].
    fn refresh_control_thread_metadata(&mut self) {}

    /// Prepare bounded, idempotent EOS work; see [`Plugin::begin_drain`].
    fn begin_drain(&mut self, _context: &ProcessContext) -> PluginResult<()> {
        Ok(())
    }

    /// Bound full-capacity EOS calls; see [`Plugin::drain_call_bound`].
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        None
    }

    /// Emit retained audio without accepting new input; see [`Plugin::drain`].
    ///
    /// Output uses the plugin's output-channel layout. The default has no tail.
    fn drain(
        &mut self,
        _output: &mut [f32],
        _context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        Ok(PluginDrainResult::COMPLETE)
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

    /// Process f64 audio samples in-place.
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

    /// Processing latency in samples.
    fn latency_samples(&self) -> usize {
        0
    }

    /// Allocation-free zero-input response bound; see [`crate::plugin::TailLength`].
    fn tail_length(&self) -> crate::plugin::TailLength {
        crate::plugin::TailLength::Unknown
    }

    /// State-independent zero-input response bound; see [`crate::plugin::Plugin::tail_support`].
    fn tail_support(&self) -> Option<u64> {
        None
    }

    /// Minimum input-rate scheduling budget for worst-case realtime work.
    ///
    /// This is forwarded through [`ParametricInPlacePluginAdapter`] to the
    /// object-safe [`Plugin`] contract. Smaller and irregular blocks remain
    /// valid. Queued/offline hosts use this value to keep at least this much
    /// upstream work ahead of a downstream consumer. It is not permission for
    /// a direct callback host to exceed that callback's physical deadline.
    fn realtime_quantum_frames(&self) -> usize {
        1
    }

    /// Coarse cost category for host scheduling.
    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Scalar
    }

    /// Get data from the plugin (if it's an analyzer or exposes internal state).
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    /// Preferred oversampling factor, if any.
    fn preferred_oversampling(&self) -> Option<u32> {
        None
    }

    /// Whether this plugin can process in f64 precision.
    fn supports_f64(&self) -> bool {
        false
    }

    /// Reports immediate-only momentary controls.
    ///
    /// The blanket adapter forwards this to the object-safe plugin
    /// contract. The host consults it between blocks on the engine
    /// control path only, never from the audio callback. A `true`
    /// return admits that structural id through the host immediate gate
    /// only; automation keeps rejecting it. Defaults false.
    fn supports_immediate_momentary_control(&self, id: &ParameterId) -> bool {
        let _ = id;
        false
    }

    /// Validate a value against the parameter schema.
    fn parametric_validate_parameter(
        &self,
        id: &ParameterId,
        value: &ParameterValue,
    ) -> PluginResult<()> {
        if let Some(param) = self.parameter_schema().iter().find(|p| &p.id == id) {
            param.validate(value).map_err(|e| format!("{}: {}", id, e))
        } else {
            Err(format!("Unknown parameter: {}", id))
        }
    }

    /// Build the parameter list from the schema and current values.
    fn parametric_parameters(&self) -> Vec<Parameter> {
        let values = self.current_values();
        self.parameter_schema()
            .iter()
            .map(|param| {
                let mut param = param.clone();
                if let Some(value) = values.get(&param.id) {
                    param.default_value = value.clone();
                }
                param
            })
            .collect()
    }

    /// Set a single parameter value after validating it against the schema.
    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        self.parametric_validate_parameter(&id, &value)?;
        let mut values = ParameterSet::new();
        values.insert(id, value);
        self.apply_values(values)
    }

    /// Read a single parameter value.
    ///
    /// The default builds the full allocating snapshot. Plugins queried on an
    /// audio callback must override this method with direct scalar access.
    /// Owned string values may still require control-thread queries.
    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.current_values().get(id).cloned()
    }

    /// Convenience alias for [`parametric_validate_parameter`](Self::parametric_validate_parameter).
    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        self.parametric_validate_parameter(id, value)
    }

    /// Convenience alias for [`parametric_parameters`](Self::parametric_parameters).
    fn parameters(&self) -> Vec<Parameter> {
        self.parametric_parameters()
    }

    /// Convenience alias for [`parametric_set_parameter`](Self::parametric_set_parameter).
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.parametric_set_parameter(id, value)
    }

    /// Convenience alias for [`parametric_get_parameter`](Self::parametric_get_parameter).
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.parametric_get_parameter(id)
    }
}

/// Adapter that turns a [`ParametricInPlacePlugin`] into a standard [`InPlacePlugin`].
#[derive(Debug)]
pub struct ParametricInPlacePluginAdapter<T: ParametricInPlacePlugin> {
    plugin: T,
    /// Legacy f64 fallback storage for processors without subdivision support.
    scratch: Vec<f32>,
    bounded: BoundedInPlace,
}

impl<T: ParametricInPlacePlugin> ParametricInPlacePluginAdapter<T> {
    /// Wrap a parametric in-place plugin for use in the host graph.
    pub fn new(plugin: T) -> Self {
        Self {
            plugin,
            scratch: Vec::new(),
            bounded: BoundedInPlace::default(),
        }
    }

    /// Consume the adapter and return the inner plugin.
    pub fn into_inner(self) -> T {
        self.plugin
    }

    fn ensure_scratch(&mut self, len: usize) {
        if self.scratch.len() < len {
            self.scratch.resize(len, 0.0);
        }
    }
}

impl<T: ParametricInPlacePlugin> InPlacePlugin for ParametricInPlacePluginAdapter<T> {
    fn info(&self) -> PluginInfo {
        self.plugin.info()
    }

    fn channels(&self) -> usize {
        self.plugin.channels()
    }

    fn input_channels(&self) -> usize {
        self.plugin.input_channels()
    }

    fn supports_bounded_subdivision(&self) -> bool {
        self.plugin.supports_bounded_subdivision()
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.plugin.parametric_parameters()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.plugin.parametric_set_parameter(id, value)
    }

    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        self.plugin.parametric_validate_parameter(id, value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.plugin.parametric_get_parameter(id)
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        self.bounded.invalidate();
        self.plugin.initialize(sample_rate)?;
        self.bounded.prepare(
            self.plugin.supports_bounded_subdivision(),
            [self.plugin.input_channels(), self.plugin.channels()],
            self.plugin.supports_f64(),
            sample_rate,
        )
    }

    fn reset(&mut self) {
        self.plugin.reset()
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        self.plugin.process_in_place(buffer, context)
    }

    fn process_in_place_f64(
        &mut self,
        buffer: &mut [f64],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        if self.plugin.supports_f64() {
            return self.plugin.process_in_place_f64(buffer, context);
        }
        if self.plugin.supports_bounded_subdivision() {
            let channels = [self.plugin.input_channels(), self.plugin.channels()];
            validate_process_block_f64(buffer, buffer, context, channels[0], channels[0])?;
            self.bounded.validate(channels, context)?;
            let plugin = &mut self.plugin;
            return bounded_in_place::fallback_in_place(
                &mut self.bounded.f32_samples,
                buffer,
                context,
                channels[0],
                |work, context| plugin.process_in_place(work, context),
            );
        }
        self.ensure_scratch(buffer.len());
        let scratch = &mut self.scratch[..buffer.len()];
        for (dst, &src) in scratch.iter_mut().zip(buffer.iter()) {
            *dst = src as f32;
        }
        let frames = self.plugin.process_in_place(scratch, context)?;
        for (dst, &src) in buffer.iter_mut().zip(scratch.iter()) {
            *dst = src as f64;
        }
        Ok(frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.plugin.drain_output_frames_max()
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        self.plugin.drain_frames_envelope()
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        self.plugin.output_frames_envelope(input_frames)
    }

    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        self.plugin.prepare_drain_metadata()
    }

    fn refresh_control_thread_metadata(&mut self) {
        self.plugin.refresh_control_thread_metadata()
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        self.plugin.begin_drain(context)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.plugin.drain_call_bound()
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        self.plugin.drain(output, context)
    }

    fn latency_samples(&self) -> usize {
        self.plugin.latency_samples()
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        self.plugin.tail_length()
    }

    fn tail_support(&self) -> Option<u64> {
        self.plugin.tail_support()
    }

    fn realtime_quantum_frames(&self) -> usize {
        self.plugin.realtime_quantum_frames()
    }

    fn cost_class(&self) -> PluginCostClass {
        self.plugin.cost_class()
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.plugin.get_data()
    }

    fn preferred_oversampling(&self) -> Option<u32> {
        self.plugin.preferred_oversampling()
    }

    fn supports_f64(&self) -> bool {
        self.plugin.supports_f64()
    }
}

impl<T: ParametricInPlacePlugin> Plugin for ParametricInPlacePluginAdapter<T> {
    fn info(&self) -> PluginInfo {
        self.plugin.info()
    }

    fn input_channels(&self) -> usize {
        self.plugin.input_channels()
    }

    fn output_channels(&self) -> usize {
        self.plugin.channels()
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        self.plugin.guarantees_identity_frame_geometry()
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.plugin.parametric_parameters()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.plugin.parametric_set_parameter(id, value)
    }

    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        self.plugin.parametric_validate_parameter(id, value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.plugin.parametric_get_parameter(id)
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        self.bounded.invalidate();
        self.plugin.initialize(sample_rate)?;
        self.bounded.prepare(
            self.plugin.supports_bounded_subdivision(),
            [self.plugin.input_channels(), self.plugin.channels()],
            self.plugin.supports_f64(),
            sample_rate,
        )
    }

    fn reset(&mut self) {
        self.plugin.reset()
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let channels = [self.plugin.input_channels(), self.plugin.channels()];
        validate_process_block_f32(input, output, context, channels[0], channels[1])?;
        if self.plugin.supports_bounded_subdivision() || channels[0] != channels[1] {
            self.bounded.validate(channels, context)?;
        }
        if channels[0] == channels[1] {
            output.copy_from_slice(input);
            self.plugin.process_in_place(output, context)
        } else {
            let plugin = &mut self.plugin;
            bounded_in_place::process(
                &mut self.bounded.f32_samples,
                input,
                output,
                context,
                channels,
                |work, context| plugin.process_in_place(work, context),
            )
        }
    }

    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        let validation = validate_process_block_f32(
            input,
            output,
            context,
            self.plugin.input_channels(),
            self.plugin.channels(),
        );
        if let Err(error) = validation {
            return Some(Err(error));
        }
        self.plugin.process_compiled_f32(op, input, output, context)
    }

    fn compiled_static_gain(&self) -> Option<f32> {
        self.plugin.compiled_static_gain()
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        self.plugin.compile_metadata()
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let channels = [self.plugin.input_channels(), self.plugin.channels()];
        validate_process_block_f64(input, output, context, channels[0], channels[1])?;
        let subdivide = self.plugin.supports_bounded_subdivision();
        if subdivide || channels[0] != channels[1] {
            self.bounded.validate(channels, context)?;
        }
        if channels[0] == channels[1] && (!subdivide || self.plugin.supports_f64()) {
            output.copy_from_slice(input);
            InPlacePlugin::process_in_place_f64(self, output, context)
        } else {
            let plugin = &mut self.plugin;
            if plugin.supports_f64() {
                bounded_in_place::process(
                    &mut self.bounded.f64_samples,
                    input,
                    output,
                    context,
                    channels,
                    |work, context| plugin.process_in_place_f64(work, context),
                )
            } else {
                bounded_in_place::process(
                    &mut self.bounded.f32_samples,
                    input,
                    output,
                    context,
                    channels,
                    |work, context| plugin.process_in_place(work, context),
                )
            }
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        self.plugin.drain_output_frames_max()
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        self.plugin.drain_frames_envelope()
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        self.plugin.output_frames_envelope(input_frames)
    }

    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        self.plugin.prepare_drain_metadata()
    }

    fn refresh_control_thread_metadata(&mut self) {
        self.plugin.refresh_control_thread_metadata()
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        self.plugin.begin_drain(context)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.plugin.drain_call_bound()
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        self.plugin.drain(output, context)
    }

    fn latency_samples(&self) -> usize {
        self.plugin.latency_samples()
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        self.plugin.tail_length()
    }

    fn tail_support(&self) -> Option<u64> {
        self.plugin.tail_support()
    }

    fn realtime_quantum_frames(&self) -> usize {
        self.plugin.realtime_quantum_frames()
    }

    fn cost_class(&self) -> PluginCostClass {
        self.plugin.cost_class()
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.plugin.get_data()
    }

    fn preferred_oversampling(&self) -> Option<u32> {
        self.plugin.preferred_oversampling()
    }

    fn supports_f64(&self) -> bool {
        self.plugin.supports_f64()
    }

    fn supports_immediate_momentary_control(&self, id: &ParameterId) -> bool {
        self.plugin.supports_immediate_momentary_control(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NativePrecision;

    impl ParametricInPlacePlugin for NativePrecision {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Precision", "1", "Test")
        }
        fn channels(&self) -> usize {
            2
        }
        fn parameter_schema(&self) -> ParameterSchema {
            Vec::new()
        }
        fn current_values(&self) -> ParameterSet {
            ParameterSet::new()
        }
        fn apply_values(&mut self, _: ParameterSet) -> PluginResult<()> {
            Ok(())
        }
        fn supports_f64(&self) -> bool {
            true
        }
        fn process_in_place(&mut self, _: &mut [f32], _: &ProcessContext) -> PluginResult<usize> {
            panic!("native f64 must not use the f32 path");
        }
        fn process_in_place_f64(
            &mut self,
            buffer: &mut [f64],
            context: &ProcessContext,
        ) -> PluginResult<usize> {
            for sample in buffer {
                *sample -= 1.0;
            }
            Ok(context.num_frames)
        }
    }

    #[test]
    fn native_in_place_f64_preserves_precision_without_cold_allocation() {
        for frames in [0, 1, 17, 257] {
            let mut plugin = ParametricInPlacePluginAdapter::new(NativePrecision);
            assert!(InPlacePlugin::supports_f64(&plugin));
            let mut samples: Vec<f64> = (0..frames * 2)
                .map(|index| match index % 3 {
                    0 => 1.0 + 2.0_f64.powi(-40),
                    1 => 1.0e40,
                    _ => 1.0 - 2.0_f64.powi(-40),
                })
                .collect();
            let expected: Vec<f64> = samples.iter().map(|sample| sample - 1.0).collect();
            std::thread::spawn(move || {
                crate::assert_no_allocs("parametric native f64 in-place adapter", || {
                    assert_eq!(
                        InPlacePlugin::process_in_place_f64(
                            &mut plugin,
                            &mut samples,
                            &ProcessContext::new(48_000, frames),
                        )
                        .unwrap(),
                        frames
                    );
                });
                assert_eq!(samples, expected);
            })
            .join()
            .unwrap();
        }
    }
}
