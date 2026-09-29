//! Private wet-core clock and finite contributor descriptors.

// Rust guideline compliant 2026-02-21
use super::native_kernel::{KernelControls, NativeKernel};
use sotf_host::InPlacePlugin;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{PluginDrainResult, PluginInfo, PluginResult, ProcessContext, TailLength};
use std::num::NonZeroU64;

pub(super) const CHUNK: usize = 256;
pub(super) const MAX_CHANNELS: usize = 32;
// A normal callback can create one chunk; EOS setup can create two. Including
// startup, a partial unit, and the previous overlap contributor needs <5 chunks.
// Eight tagged entries provide bounded slack, checked by contributor tests.
const DESCRIPTORS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Controls {
    pub threshold: f32,
    pub release: f32,
    pub soft: bool,
    pub true_peak: bool,
    pub dual_release: bool,
    pub link: f32,
}

impl Controls {
    pub fn kernel(self, channels: usize, isp_mode: bool) -> KernelControls {
        KernelControls {
            channels,
            soft: self.soft,
            true_peak: self.true_peak,
            isp_mode,
            dual_release: self.dual_release,
            link_amount: self.link,
        }
    }

    pub fn install(self, old: Self, kernel: &mut NativeKernel, rate: u32) {
        if self.threshold != old.threshold {
            kernel.threshold_db_smoother.set_target(self.threshold);
        }
        if self.release != old.release {
            kernel.release_coeff = (-1.0 / (self.release * 0.001 * rate as f32)).exp();
            kernel
                .dual_release_env
                .set_times(self.release, self.release * 5.0, rate);
            for envelope in &mut kernel.channel_dual_release {
                envelope.set_times(self.release, self.release * 5.0, rate);
            }
        }
    }
}

/// Offset a copied transport snapshot without rebuilding the host PPQ origin.
pub(super) fn subcontext(
    context: &ProcessContext<'_>,
    offset: usize,
    frames: usize,
) -> ProcessContext<'static> {
    let mut transport = context.transport;
    transport.sample_position = transport.sample_position.saturating_add(offset as u64);
    transport.ppq_position += offset as f64 / f64::from(context.sample_rate) * transport.bpm / 60.0;
    ProcessContext::new(context.sample_rate, frames).with_transport(transport)
}

/// Build at the actual processing clock with an exact integer audio delay.
pub(super) fn prepare_kernel(
    channels: usize,
    rate: u32,
    delay: usize,
    controls: Controls,
) -> NativeKernel {
    let capacity = delay.max(1);
    let mut kernel = NativeKernel::new(
        channels,
        controls.threshold,
        controls.release,
        0.0,
        capacity,
    );
    kernel.update_coefficients(channels, rate, controls.release, 0.0, capacity, false);
    kernel.initialize(channels, rate, controls.release, capacity);
    kernel.lookahead_len = delay;
    kernel.threshold_db_smoother.reset(controls.threshold);
    kernel.mix_smoother.reset(1.0);
    kernel.reset();
    kernel
}

/// An actual finite nonzero sample's attenuation, independent of filter loss.
pub(super) fn applied_gain(before: f32, after: f32) -> f32 {
    if before == 0.0 || !before.is_finite() || !after.is_finite() {
        1.0
    } else {
        ((after as f64 / before as f64).abs().min(1.0)) as f32
    }
}

#[derive(Clone, Copy)]
struct Descriptor {
    block: Option<u64>,
    gains: [f32; MAX_CHANNELS],
}

const EMPTY_DESCRIPTOR: Descriptor = Descriptor {
    block: None,
    gains: [1.0; MAX_CHANNELS],
};

pub(super) struct WetCore {
    channels: usize,
    factor: usize,
    rate: u32,
    delay: usize,
    controls: Controls,
    reset_controls: Controls,
    pub kernel: NativeKernel,
    timeline: [Controls; CHUNK],
    read: usize,
    queued: usize,
    frozen: Option<Controls>,
    output_frames: u64,
    descriptors: [Descriptor; DESCRIPTORS],
    has_input: bool,
    remaining: Option<usize>,
}

impl WetCore {
    pub fn new(
        channels: usize,
        factor: usize,
        rate: u32,
        delay: usize,
        controls: Controls,
    ) -> Self {
        Self {
            channels,
            factor,
            rate,
            delay,
            controls,
            reset_controls: controls,
            kernel: prepare_kernel(channels, rate, delay, controls),
            timeline: [controls; CHUNK],
            read: 0,
            queued: 0,
            frozen: None,
            output_frames: 0,
            descriptors: [EMPTY_DESCRIPTOR; DESCRIPTORS],
            has_input: false,
            remaining: None,
        }
    }

    pub fn append(&mut self, frames: usize, controls: Controls) {
        assert!(
            self.queued + frames <= CHUNK,
            "prepared limiter timeline capacity"
        );
        for offset in 0..frames {
            self.timeline[(self.read + self.queued + offset) % CHUNK] = controls;
        }
        self.queued += frames;
    }

    pub fn freeze(&mut self, controls: Controls) {
        self.frozen = Some(controls);
    }
    pub fn set_reset_controls(&mut self, controls: Controls) {
        self.reset_controls = controls;
    }

    /// Rubato's synchronous down unit uses exactly its current and prior unit.
    /// Startup delivery adds one CHUNK before block zero. An unfinished final
    /// unit has unity labels in the missing zero-padded positions.
    pub fn contributing_gain(&self, output_frame: u64, channel: usize) -> f32 {
        let chunk = CHUNK as u64;
        if output_frame < chunk {
            return 1.0;
        }
        let block = output_frame / chunk - 1;
        let gain = self.block_gain(block, channel);
        if block == 0 {
            gain
        } else {
            gain.min(self.block_gain(block - 1, channel))
        }
    }

    #[cfg(test)]
    pub fn live_descriptors(&self, next_output: u64) -> usize {
        let earliest = (next_output / CHUNK as u64).saturating_sub(2);
        self.output_frames
            .div_ceil((CHUNK * self.factor) as u64)
            .saturating_sub(earliest) as usize
    }

    fn block_gain(&self, block: u64, channel: usize) -> f32 {
        let unit = (CHUNK * self.factor) as u64;
        if block >= self.output_frames.div_ceil(unit) {
            return 1.0;
        }
        let descriptor = &self.descriptors[block as usize % DESCRIPTORS];
        assert_eq!(
            descriptor.block,
            Some(block),
            "live limiter gain descriptor overwritten"
        );
        descriptor.gains[channel]
    }

    fn render(&mut self, buffer: &mut [f32], context: &ProcessContext) -> PluginResult<usize> {
        let mut offset = 0;
        while offset < context.num_frames {
            let phase = self.output_frames as usize % self.factor;
            if phase == 0 {
                let next = if self.queued > 0 {
                    let value = self.timeline[self.read];
                    self.read = (self.read + 1) % CHUNK;
                    self.queued -= 1;
                    value
                } else {
                    self.frozen
                        .ok_or_else(|| "limiter wet core exhausted accepted controls".to_string())?
                };
                next.install(self.controls, &mut self.kernel, self.rate);
                self.controls = next;
            }
            let frames = (self.factor - phase).min(context.num_frames - offset);
            let start = self.output_frames;
            let unit = (CHUNK * self.factor) as u64;
            let descriptors = &mut self.descriptors;
            let ctx = subcontext(context, offset, frames);
            self.kernel.process_observed(
                &mut buffer[offset * self.channels..(offset + frames) * self.channels],
                &ctx,
                self.controls.kernel(self.channels, false),
                None,
                |frame, channel, isp, _, before, after| {
                    debug_assert!(!isp);
                    let block = (start + frame as u64) / unit;
                    let descriptor = &mut descriptors[block as usize % DESCRIPTORS];
                    if descriptor.block != Some(block) {
                        *descriptor = Descriptor {
                            block: Some(block),
                            gains: [1.0; MAX_CHANNELS],
                        };
                    }
                    descriptor.gains[channel] =
                        descriptor.gains[channel].min(applied_gain(before, after));
                },
            )?;
            self.output_frames += frames as u64;
            offset += frames;
        }
        Ok(context.num_frames)
    }
}

impl InPlacePlugin for WetCore {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Limiter wet core", env!("CARGO_PKG_VERSION"), "SotF")
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
        Err("private limiter controls use the accepted-input timeline".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn initialize(&mut self, rate: u32) -> PluginResult<()> {
        if rate != self.rate {
            return Err("limiter wet core prepared at a different rate".into());
        }
        self.reset();
        Ok(())
    }
    fn reset(&mut self) {
        self.reset_controls
            .install(self.controls, &mut self.kernel, self.rate);
        self.controls = self.reset_controls;
        self.kernel.reset();
        self.read = 0;
        self.queued = 0;
        self.frozen = None;
        self.output_frames = 0;
        self.descriptors.fill(EMPTY_DESCRIPTOR);
        self.has_input = false;
        self.remaining = None;
    }
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        if context.sample_rate != self.rate || buffer.len() != context.num_frames * self.channels {
            return Err("invalid private limiter processing shape or rate".into());
        }
        if self.remaining.is_some() {
            return Err("limiter wet core is draining".into());
        }
        let frames = self.render(buffer, context)?;
        self.has_input |= frames > 0;
        Ok(frames)
    }
    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if context.sample_rate != self.rate || self.frozen.is_none() || self.queued != 0 {
            return Err("limiter wet core EOS requires the frozen completed timeline".into());
        }
        self.remaining
            .get_or_insert(if self.has_input { self.delay } else { 0 });
        Ok(())
    }
    fn drain_output_frames_max(&self) -> usize {
        CHUNK
    }
    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(self.remaining.unwrap_or(self.delay).div_ceil(CHUNK).max(1) as u64)
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        self.begin_drain(context)?;
        let remaining = self.remaining.unwrap();
        if remaining == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let frames = remaining.min(CHUNK).min(output.len() / self.channels);
        if frames == 0 {
            return Err("limiter wet core drain capacity".into());
        }
        output[..frames * self.channels].fill(0.0);
        let mut ctx = *context;
        ctx.num_frames = frames;
        self.render(&mut output[..frames * self.channels], &ctx)?;
        self.remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: frames == remaining,
        })
    }
    fn latency_samples(&self) -> usize {
        self.delay
    }
    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.delay as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_gain_labels_silence_as_unity_and_avoid_f32_ratio_overflow() {
        assert_eq!(applied_gain(0.0, 0.0), 1.0);
        assert_eq!(applied_gain(f32::MAX, f32::MAX / 2.0), 0.5);
        assert_eq!(applied_gain(-4.0, -1.0), 0.25);
        assert_eq!(applied_gain(0.01, 0.02), 1.0);
    }

    #[test]
    fn a_core_attenuation_block_labels_exactly_its_two_downsampling_contributors() {
        let controls = Controls {
            threshold: -1.0,
            release: 10.0,
            soft: false,
            true_peak: false,
            dual_release: false,
            link: 0.0,
        };
        for factor in [2, 4] {
            let mut core = WetCore::new(2, factor, 48_000 * factor as u32, 0, controls);
            core.output_frames = (4 * CHUNK * factor) as u64;
            for block in 0..4 {
                core.descriptors[block] = Descriptor {
                    block: Some(block as u64),
                    gains: [1.0; MAX_CHANNELS],
                };
            }
            core.descriptors[1].gains[0] = 0.5;
            core.descriptors[2].gains[1] = 0.25;
            for frame in 0..5 * CHUNK {
                let a = if (2 * CHUNK..4 * CHUNK).contains(&frame) {
                    0.5
                } else {
                    1.0
                };
                let b = if (3 * CHUNK..5 * CHUNK).contains(&frame) {
                    0.25
                } else {
                    1.0
                };
                assert_eq!(core.contributing_gain(frame as u64, 0), a);
                assert_eq!(core.contributing_gain(frame as u64, 1), b);
            }
        }
    }

    #[test]
    fn transport_subdivision_preserves_arbitrary_musical_origin() {
        let mut context = ProcessContext::new(48_000, 1024);
        context.transport.sample_position = 50_000;
        context.transport.ppq_position = -3.75;
        context.transport.bpm = 137.0;
        context.transport.playing = true;
        let sub = subcontext(&context, 257, 7);
        assert_eq!(sub.num_frames, 7);
        assert_eq!(sub.transport.sample_position, 50_257);
        assert_eq!(
            sub.transport.ppq_position,
            -3.75 + 257.0 / 48_000.0 * 137.0 / 60.0
        );
        assert!(sub.transport.playing);
    }
}
