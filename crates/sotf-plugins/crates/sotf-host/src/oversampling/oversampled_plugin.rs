use super::misc::interleaved_to_planar;
use super::misc::planar_to_interleaved;
use super::oversampler::Oversampler;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{InPlacePlugin, PluginDrainResult, PluginInfo, PluginResult, ProcessContext};
use std::any::Any;
use std::sync::Arc;

/// Wraps any `InPlacePlugin` with transparent oversampling.
///
/// The inner plugin processes audio at `factor × sample_rate`. The wrapper
/// handles upsampling before and downsampling after `process_in_place()`.
/// End-of-stream drain retains both FFT overlaps and the inner plugin's
/// declared tail, with padding to complete internal chunks. It emits at most
/// 256 frames into any nonempty frame-aligned destination. Reset is required
/// after draining a nonempty stream or an inner drain error; empty drain is a no-op.
///
/// This enables any plugin to be oversampled without modifying its internals:
/// ```ignore
/// let saturator = SaturationPlugin::new(2);
/// let oversampled = OversampledPlugin::new(saturator, 4, 2)?; // 4x, stereo
/// ```
pub struct OversampledPlugin<P: InPlacePlugin> {
    pub(super) inner: P,
    pub(super) oversampler: Oversampler,
    pub(super) factor: u32,
    pub(super) channels: usize,
    pub(super) sample_rate: u32,
    /// Pre-allocated interleaved buffer for oversampled processing
    pub(super) os_interleaved: Vec<f32>,
    next_os_context: ProcessContext<'static>,
    initialized: bool,
    drain_prepared: bool,
}

impl<P: InPlacePlugin> OversampledPlugin<P> {
    /// Default maximum host block size the oversampler pre-allocates for.
    /// Doubled from the previous 8192 to cover large offline-render blocks
    /// without reallocation.
    pub const DEFAULT_MAX_BLOCK_FRAMES: usize = 16384;

    /// Create a new oversampled plugin wrapper.
    ///
    /// `factor` must be 2 or 4. The inner plugin will be initialized at
    /// `sample_rate * factor` when `initialize()` is called.
    /// Pre-allocates for [`Self::DEFAULT_MAX_BLOCK_FRAMES`] frames; use
    /// [`Self::new_with_max_frames`] if the host requires a different limit.
    pub fn new(inner: P, factor: u32, channels: usize) -> Result<Self, String> {
        Self::new_with_max_frames(inner, factor, channels, Self::DEFAULT_MAX_BLOCK_FRAMES)
    }

    /// Create a new oversampled plugin wrapper with a custom maximum host
    /// block size. The oversampled buffer is pre-sized to
    /// `max_block_frames * factor * channels`.
    pub fn new_with_max_frames(
        inner: P,
        factor: u32,
        channels: usize,
        max_block_frames: usize,
    ) -> Result<Self, String> {
        let mut oversampler = Oversampler::new(factor, channels)?;
        oversampler.reserve_for_max_frames(max_block_frames)?;
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
            sample_rate: 48000,
            os_interleaved: vec![0.0; os_buf_size],
            next_os_context: ProcessContext::new(48_000 * factor, 0),
            initialized: false,
            drain_prepared: false,
        })
    }

    /// Access the inner plugin.
    pub fn inner(&self) -> &P {
        &self.inner
    }

    /// Mutably access the inner plugin.
    pub fn inner_mut(&mut self) -> &mut P {
        &mut self.inner
    }
}

impl<P: InPlacePlugin> InPlacePlugin for OversampledPlugin<P> {
    fn info(&self) -> PluginInfo {
        let mut info = self.inner.info();
        info.name = format!("{}({}x)", info.name, self.factor);
        info
    }

    fn channels(&self) -> usize {
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

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        self.sample_rate = sample_rate;
        // Initialize inner plugin at the oversampled rate
        let os_rate = sample_rate * self.factor;
        self.inner.initialize(os_rate)?;
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
        self.next_os_context = ProcessContext::new(os_rate, 0);
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

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let nf = context.num_frames;
        let nc = self.channels;

        // The oversampler's callback receives planar buffers at the OS rate.
        // We need to convert to interleaved, call inner.process_in_place, then
        // convert back to planar.
        let factor = self.factor;
        let buffered_frames = self.oversampler.residual_frames;
        let mut processed_frames = 0;

        let inner = &mut self.inner;
        let os_interleaved = &mut self.os_interleaved;
        let mut inner_error: Option<String> = None;

        self.oversampler
            .process(buffer, nf, |planar, os_frames| {
                if inner_error.is_some() {
                    return;
                }
                let total_os = os_frames * nc;
                // Ensure buffer is large enough. A grow here means the pre-
                // allocation in `build()` was too small for this block; log so
                // the offending block size is visible. Allocation on the audio
                // thread is acceptable as a one-shot fallback but not as a
                // steady state.
                if os_interleaved.capacity() < total_os {
                    crate::rate_limited_log!(
                        warn,
                        5,
                        "oversampling: os_interleaved grew from {} to {} on hot path",
                        os_interleaved.capacity(),
                        total_os
                    );
                }
                if os_interleaved.len() < total_os {
                    os_interleaved.resize(total_os, 0.0);
                }
                // Convert planar → interleaved
                planar_to_interleaved(planar, &mut os_interleaved[..total_os], os_frames, nc);
                // Process at oversampled rate
                let ctx = super::misc::oversampled_context(
                    context,
                    factor,
                    buffered_frames,
                    processed_frames,
                    os_frames,
                );
                match inner.process_in_place(&mut os_interleaved[..total_os], &ctx) {
                    Ok(frames) if frames == os_frames => {}
                    Ok(frames) => {
                        inner_error = Some(format!(
                            "oversampled inner processed {frames} frames, expected {os_frames}"
                        ));
                        return;
                    }
                    Err(err) => {
                        inner_error = Some(err);
                        return;
                    }
                }
                // Convert interleaved → planar (back)
                interleaved_to_planar(&os_interleaved[..total_os], planar, os_frames, nc);
                processed_frames += os_frames / factor as usize;
            })
            .map_err(|e| e.to_string())?;

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

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if self.oversampler.drain_failed() {
            return Err("Oversampler must be reset after a failed drain".into());
        }
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("Oversampled drain requires its initialized sample rate".into());
        }
        if self.drain_prepared || !self.oversampler.received_input() {
            return Ok(());
        }
        let nc = self.channels;
        let needed = super::misc::OS_CHUNK_SIZE * self.factor as usize * nc;
        if self.os_interleaved.len() < needed {
            return Err("Oversampled EOS setup exceeds prepared scratch".into());
        }
        if self.inner.drain_output_frames_max() > self.os_interleaved.len() / nc {
            return Err("Oversampled inner drain capacity changed; reinitialize first".into());
        }
        let inner = &mut self.inner;
        let scratch = &mut self.os_interleaved;
        let next_context = &mut self.next_os_context;
        self.oversampler.begin_drain_with(|planar, frames| {
            let samples = frames * nc;
            planar_to_interleaved(planar, &mut scratch[..samples], frames, nc);
            let context = ProcessContext {
                num_frames: frames,
                ..*next_context
            };
            let written = inner.process_in_place(&mut scratch[..samples], &context)?;
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
        let scratch = &mut self.os_interleaved;
        let next_context = &mut self.next_os_context;
        self.oversampler
            .drain_with(output, |planar, process_frames| {
                let result = if let Some(frames) = process_frames {
                    let samples = frames * nc;
                    planar_to_interleaved(planar, &mut scratch[..samples], frames, nc);
                    let context = ProcessContext {
                        num_frames: frames,
                        ..*next_context
                    };
                    let written = inner.process_in_place(&mut scratch[..samples], &context)?;
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

    fn latency_samples(&self) -> usize {
        // The integral host clock conservatively rounds fractional delay up.
        let inner_latency_1x = self.inner.latency_samples().div_ceil(self.factor as usize);
        self.oversampler.latency_samples() + inner_latency_1x
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
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

    fn preferred_oversampling(&self) -> Option<u32> {
        None // Already oversampled — don't request more
    }
}
