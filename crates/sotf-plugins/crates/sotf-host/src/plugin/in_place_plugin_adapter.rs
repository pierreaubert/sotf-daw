use super::in_place_plugin::InPlacePlugin;
use super::plugin_info::PluginInfo;
use super::process_context::ProcessContext;
use super::types::{PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginResult};
use super::{Plugin, PluginDrainResult, validate_process_block_f32, validate_process_block_f64};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::bounded_in_place::{self, BoundedInPlace};
use std::any::Any;
use std::sync::Arc;

/// Adapter to convert InPlacePlugin to Plugin
pub struct InPlacePluginAdapter<T: InPlacePlugin> {
    pub(super) plugin: T,
    /// Legacy f64 fallback storage for processors without subdivision support.
    scratch: Vec<f32>,
    bounded: BoundedInPlace,
}

impl<T: InPlacePlugin> InPlacePluginAdapter<T> {
    pub fn new(plugin: T) -> Self {
        Self {
            plugin,
            scratch: Vec::new(),
            bounded: BoundedInPlace::default(),
        }
    }

    fn ensure_scratch(&mut self, len: usize) {
        if self.scratch.len() < len {
            self.scratch.resize(len, 0.0);
        }
    }

    fn process_in_place_f64(
        &mut self,
        buffer: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
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
}

impl<T: InPlacePlugin> Plugin for InPlacePluginAdapter<T> {
    fn info(&self) -> PluginInfo {
        self.plugin.info()
    }

    fn input_channels(&self) -> usize {
        self.plugin.input_channels()
    }

    fn output_channels(&self) -> usize {
        self.plugin.channels()
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.plugin.parameters()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.plugin.set_parameter(id, value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.plugin.get_parameter(id)
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
            self.process_in_place_f64(output, context)
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

    fn tail_length(&self) -> super::TailLength {
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
