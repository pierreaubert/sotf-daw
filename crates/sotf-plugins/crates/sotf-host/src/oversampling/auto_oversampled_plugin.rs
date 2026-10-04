use super::misc::interleaved_to_planar;
use super::misc::planar_to_interleaved;
use super::oversampler::Oversampler;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginCompileMetadata, PluginDrainResult, PluginInfo, PluginResult, ProcessContext,
};
use std::any::Any;
use std::sync::Arc;

/// Runtime wrapper used by `DawHost` for `Plugin::preferred_oversampling()`.
///
/// The generic `OversampledPlugin<P>` remains the zero-cost wrapper for
/// concrete `InPlacePlugin` types. This dyn wrapper lets the host honor
/// oversampling preferences for already-erased `Box<dyn Plugin>` values.
///
/// End-of-stream drain preserves partial input, both FFT overlaps, and the
/// inner plugin's declared tail, padding to internal chunk boundaries. Drain
/// accepts any nonempty frame-aligned destination and emits at most 256 frames.
/// Reset is required before processing after a nonempty stream has begun drain.
/// Draining an empty stream is a no-op. Inner drain errors require reset.
pub struct AutoOversampledPlugin {
    pub(super) inner: Box<dyn Plugin>,
    pub(super) oversampler: Oversampler,
    pub(super) factor: u32,
    pub(super) channels: usize,
    pub(super) os_input_interleaved: Vec<f32>,
    pub(super) os_interleaved: Vec<f32>,
    next_os_context: ProcessContext<'static>,
    initialized: bool,
    drain_prepared: bool,
}

impl AutoOversampledPlugin {
    /// Default maximum host block size the oversampler pre-allocates for.
    pub const DEFAULT_MAX_BLOCK_FRAMES: usize = 16384;

    pub fn new(inner: Box<dyn Plugin>, factor: u32) -> Result<Self, String> {
        Self::new_with_max_frames(inner, factor, Self::DEFAULT_MAX_BLOCK_FRAMES)
    }

    pub fn new_with_max_frames(
        inner: Box<dyn Plugin>,
        factor: u32,
        max_block_frames: usize,
    ) -> Result<Self, String> {
        let channels = inner.input_channels();
        if channels != inner.output_channels() {
            return Err(format!(
                "Cannot auto-oversample plugin '{}' with mismatched I/O channels ({} -> {})",
                inner.info().name,
                inner.input_channels(),
                inner.output_channels()
            ));
        }
        let mut oversampler = Oversampler::new(factor, channels)?;
        oversampler.reserve_for_max_frames(max_block_frames)?;
        // The DSP callback always receives a full internal chunk, even when
        // the host negotiates single-frame callbacks.
        let os_buf_size = max_block_frames
            .max(super::misc::OS_CHUNK_SIZE)
            .checked_mul(factor as usize)
            .and_then(|frames| frames.checked_mul(channels))
            .filter(|samples| *samples <= isize::MAX as usize / std::mem::size_of::<f32>())
            .ok_or_else(|| "Oversampling scratch capacity overflow".to_string())?;
        Ok(Self {
            inner,
            oversampler,
            factor,
            channels,
            os_input_interleaved: vec![0.0; os_buf_size],
            os_interleaved: vec![0.0; os_buf_size],
            next_os_context: ProcessContext::new(f64::from(48_000 * factor), 0),
            initialized: false,
            drain_prepared: false,
        })
    }
}

impl Plugin for AutoOversampledPlugin {
    fn info(&self) -> PluginInfo {
        let mut info = self.inner.info();
        info.name = format!("{}({}x)", info.name, self.factor);
        info
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.inner.parameters()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.inner.set_parameter(id, value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.inner.get_parameter(id)
    }

    fn supports_immediate_momentary_control(&self, id: &ParameterId) -> bool {
        self.inner.supports_immediate_momentary_control(id)
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        let inner_rate = sample_rate * f64::from(self.factor);
        if !sample_rate.is_finite() || sample_rate <= 0.0 || !inner_rate.is_finite() {
            return Err("Oversampled sample rate must be finite and positive".into());
        }
        self.inner.initialize(inner_rate)?;
        let drain_frames = self.inner.drain_output_frames_max();
        let drain_samples = drain_frames
            .checked_mul(self.channels)
            .filter(|samples| *samples <= isize::MAX as usize / std::mem::size_of::<f32>())
            .ok_or_else(|| "Oversampled inner drain capacity overflow".to_string())?;
        self.oversampler.reserve_for_drain_frames(drain_frames)?;
        if self.os_interleaved.len() < drain_samples {
            self.os_interleaved.resize(drain_samples, 0.0);
        }
        self.oversampler.reset();
        self.next_os_context = ProcessContext::new(inner_rate, 0);
        self.initialized = true;
        self.drain_prepared = false;
        Ok(())
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.oversampler.reset();
        self.next_os_context = ProcessContext::new(self.next_os_context.sample_rate, 0);
        self.drain_prepared = false;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        crate::plugin::validate_process_block_f32(
            input,
            output,
            context,
            self.channels,
            self.channels,
        )?;
        output[..input.len()].copy_from_slice(input);
        let nf = context.num_frames;
        let nc = self.channels;
        let factor = self.factor;
        let buffered_frames = self.oversampler.residual_frames;
        let mut processed_frames = 0;
        let inner = &mut self.inner;
        let os_input_interleaved = &mut self.os_input_interleaved;
        let os_interleaved = &mut self.os_interleaved;
        let mut inner_error = None;

        self.oversampler
            .process(&mut output[..input.len()], nf, |planar, os_frames| {
                if inner_error.is_some() {
                    return;
                }
                let total_os = os_frames * nc;
                if os_interleaved.capacity() < total_os
                    || os_input_interleaved.capacity() < total_os
                {
                    crate::rate_limited_log!(
                        warn,
                        5,
                        "auto-oversampling: os_interleaved grew from {} to {} on hot path",
                        os_interleaved.capacity(),
                        total_os
                    );
                }
                if os_interleaved.len() < total_os {
                    os_interleaved.resize(total_os, 0.0);
                }
                if os_input_interleaved.len() < total_os {
                    os_input_interleaved.resize(total_os, 0.0);
                }
                planar_to_interleaved(planar, &mut os_input_interleaved[..total_os], os_frames, nc);
                let ctx = super::misc::oversampled_context(
                    context,
                    factor,
                    buffered_frames,
                    processed_frames,
                    os_frames,
                );
                match inner.process(
                    &os_input_interleaved[..total_os],
                    &mut os_interleaved[..total_os],
                    &ctx,
                ) {
                    Ok(frames) if frames == os_frames => {}
                    Ok(frames) => {
                        inner_error = Some(format!(
                            "auto-oversampled inner processed {frames} frames, expected {os_frames}"
                        ));
                        return;
                    }
                    Err(err) => {
                        inner_error = Some(err);
                        return;
                    }
                }
                interleaved_to_planar(&os_interleaved[..total_os], planar, os_frames, nc);
                processed_frames += os_frames / factor as usize;
            })?;

        if let Some(err) = inner_error {
            return Err(err);
        }
        self.next_os_context =
            super::misc::oversampled_context(context, factor, buffered_frames, processed_frames, 0);
        Ok(nf)
    }

    fn drain_output_frames_max(&self) -> usize {
        super::misc::OS_CHUNK_SIZE
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        // The live drain bound is already a state-independent constant,
        // so the envelope lifts it unchanged.
        Some(super::misc::OS_CHUNK_SIZE)
    }

    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        self.inner.prepare_drain_metadata()
    }

    fn refresh_control_thread_metadata(&mut self) {
        self.inner.refresh_control_thread_metadata()
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if self.oversampler.drain_failed() {
            return Err("Oversampler must be reset after a failed drain".into());
        }
        if !self.initialized
            || context.sample_rate * f64::from(self.factor) != self.next_os_context.sample_rate
        {
            return Err("Oversampled drain requires its initialized sample rate".into());
        }
        if self.drain_prepared || !self.oversampler.received_input() {
            return Ok(());
        }
        let nc = self.channels;
        let needed = super::misc::OS_CHUNK_SIZE * self.factor as usize * nc;
        if self.os_input_interleaved.len() < needed || self.os_interleaved.len() < needed {
            return Err("Oversampled EOS setup exceeds prepared scratch".into());
        }
        if self.inner.drain_output_frames_max() > self.os_interleaved.len() / nc {
            return Err("Oversampled inner drain capacity changed; reinitialize first".into());
        }
        let inner = &mut self.inner;
        let input = &mut self.os_input_interleaved;
        let scratch = &mut self.os_interleaved;
        let next_context = &mut self.next_os_context;
        self.oversampler.begin_drain_with(|planar, frames| {
            let samples = frames * nc;
            planar_to_interleaved(planar, &mut input[..samples], frames, nc);
            let context = ProcessContext {
                num_frames: frames,
                ..*next_context
            };
            let written = inner.process(&input[..samples], &mut scratch[..samples], &context)?;
            if written != frames {
                return Err("Oversampled inner returned an incorrect setup frame count".into());
            }
            interleaved_to_planar(&scratch[..samples], planar, frames, nc);
            super::misc::advance_context(next_context, frames);
            Ok(())
        })?;
        inner
            .begin_drain(next_context)
            .map_err(|error| self.oversampler.fail_drain(error))?;
        self.drain_prepared = true;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.drain_prepared && self.oversampler.received_input() {
            return None;
        }
        self.oversampler.drain_call_bound(
            self.inner.drain_call_bound(),
            self.inner.drain_output_frames_max(),
        )
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        self.oversampler.validate_drain_output(output)?;
        self.begin_drain(context)?;
        let nc = self.channels;
        let inner = &mut self.inner;
        let input = &mut self.os_input_interleaved;
        let scratch = &mut self.os_interleaved;
        let next_context = &mut self.next_os_context;
        self.oversampler
            .drain_with(output, |planar, process_frames| {
                let result = if let Some(frames) = process_frames {
                    let samples = frames * nc;
                    planar_to_interleaved(planar, &mut input[..samples], frames, nc);
                    let context = ProcessContext {
                        num_frames: frames,
                        ..*next_context
                    };
                    let written =
                        inner.process(&input[..samples], &mut scratch[..samples], &context)?;
                    if written != frames {
                        return Err(
                            "Oversampled inner returned an incorrect frame count during drain"
                                .to_string(),
                        );
                    }
                    PluginDrainResult {
                        frames,
                        complete: false,
                    }
                } else {
                    let frames = inner.drain_output_frames_max();
                    if frames > scratch.len() / nc || frames > planar[0].len() {
                        return Err(
                            "Oversampled inner drain capacity changed; reinitialize first"
                                .to_string(),
                        );
                    }
                    let result = inner.drain(&mut scratch[..frames * nc], next_context)?;
                    if result.frames > frames {
                        return Err(
                            "Oversampled inner drain exceeded its declared capacity".to_string()
                        );
                    }
                    result
                };
                interleaved_to_planar(&scratch[..result.frames * nc], planar, result.frames, nc);
                if process_frames.is_some() {
                    super::misc::advance_context(next_context, result.frames);
                }
                Ok(result)
            })
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        let mut metadata = self.inner.compile_metadata();
        metadata.compiled_op = None;
        metadata.static_gain = None;
        metadata.latency_samples = self.latency_samples();
        metadata.boundary = true;
        metadata
    }

    fn latency_samples(&self) -> usize {
        // Public latency is integral in the host clock. Round up so an inner
        // fractional-frame delay is never advertised as already available.
        self.oversampler.latency_samples()
            + self.inner.latency_samples().div_ceil(self.factor as usize)
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        let inner_rate = self.next_os_context.sample_rate;
        if self.inner.output_sample_rate(inner_rate) != inner_rate {
            return crate::plugin::TailLength::Unknown;
        }
        self.oversampler.tail_length(self.inner.tail_length())
    }

    fn realtime_quantum_frames(&self) -> usize {
        self.inner
            .realtime_quantum_frames()
            .div_ceil(self.factor as usize)
            .max(1)
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.inner.get_data()
    }

    fn take_cache_contention_stats(&mut self) -> (u64, u64) {
        self.inner.take_cache_contention_stats()
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        // Every success path returns exactly `context.num_frames` (the
        // inner call is enforced frame-exact), so identity is proven.
        Some(input_frames)
    }

    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        input_rate
    }

    fn preferred_oversampling(&self) -> Option<u32> {
        None
    }

    fn supports_f64(&self) -> bool {
        // The resamplers and scratch buffers use f32 regardless of the inner
        // plugin's precision. Let the host use its preallocated conversion
        // buffers instead of the allocating default Plugin::process_f64.
        false
    }
}
