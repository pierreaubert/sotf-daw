//! Prepared audio-rate oversampling, output protection, and finite EOS.

// Rust guideline compliant 2026-02-21
use super::native_kernel::{Bs1770TruePeakDetector, NativeKernel};
use super::oversampled_core::{
    CHUNK, Controls, MAX_CHANNELS, WetCore, applied_gain, prepare_kernel, subcontext,
};
use super::types::LimiterData;
use sotf_host::InPlacePlugin;
use sotf_host::analyzer::RealTimeCache;
use sotf_host::oversampling::OversampledPlugin;
use sotf_host::plugin::{PluginDrainResult, PluginResult, ProcessContext, TailLength};
use sotf_host::smoothing::Smoother;
use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Open,
    Wet,
    Guard,
    Dry,
    Complete,
    Failed,
}

/// A native-clock channel ring; gain rings start at unity, audio rings at zero.
struct Delay {
    samples: Vec<f32>,
    frames: usize,
    position: usize,
    initial: f32,
}
impl Delay {
    fn new(frames: usize, channels: usize, initial: f32) -> Self {
        Self {
            samples: vec![initial; frames * channels],
            frames,
            position: 0,
            initial,
        }
    }
    fn sample(&mut self, channel: usize, channels: usize, input: f32) -> f32 {
        if self.frames == 0 {
            return input;
        }
        let index = self.position * channels + channel;
        let output = self.samples[index];
        self.samples[index] = input;
        output
    }
    fn advance(&mut self) {
        if self.frames != 0 {
            self.position = (self.position + 1) % self.frames;
        }
    }
    fn reset(&mut self) {
        self.samples.fill(self.initial);
        self.position = 0;
    }
}

pub(super) struct OversampledPath {
    channels: usize,
    rate: u32,
    wet: OversampledPlugin<WetCore>,
    guard: NativeKernel,
    guard_controls: Controls,
    isp: bool,
    mix: Smoother,
    dry: Delay,
    contribution_delay: Delay,
    first_gain_delay: Vec<f32>,
    guard_gains: Vec<f32>,
    dry_scratch: Vec<f32>,
    input_peaks: [f32; CHUNK],
    output_detectors: Vec<Bs1770TruePeakDetector>,
    peak_accumulator: f32,
    output_peak_accumulator: f32,
    gain_accumulator: f32,
    isp_accumulator: Vec<f32>,
    meter_frames: usize,
    accepted_phase: usize,
    output_frames: u64,
    received: bool,
    phase: Phase,
    guard_delay: usize,
    guard_remaining: usize,
    dry_remaining: usize,
    latency: usize,
    tail: u64,
}

impl OversampledPath {
    pub fn prepare(
        channels: usize,
        rate: u32,
        factor: u32,
        lookahead_ms: f32,
        controls: Controls,
        isp: bool,
        mix: f32,
    ) -> PluginResult<Self> {
        if rate == 0 || !(1..=MAX_CHANNELS).contains(&channels) || ![2, 4].contains(&factor) {
            return Err(
                "oversampled limiter requires a nonzero rate, 1..=32 channels, and factor 2 or 4"
                    .into(),
            );
        }
        let high_rate = rate
            .checked_mul(factor)
            .ok_or_else(|| "limiter oversampled rate overflow".to_string())?;
        // Quantize once at the native clock, then use an exact integer multiple.
        let delay = (lookahead_ms.max(0.0) * 0.001 * rate as f32) as usize;
        let high_delay = delay
            .checked_mul(factor as usize)
            .ok_or_else(|| "limiter lookahead overflow".to_string())?;
        high_delay
            .max(1)
            .checked_mul(channels)
            .filter(|n| *n <= isize::MAX as usize / size_of::<f32>())
            .ok_or_else(|| "limiter lookahead capacity overflow".to_string())?;
        let detector_delay = Bs1770TruePeakDetector::detector_delay_samples(rate);
        if isp && (mix < 1.0 || controls.soft || delay < detector_delay) {
            return Err(format!(
                "ISP mode requires 100% wet, hard limiting, and at least {detector_delay} lookahead samples at {rate} Hz"
            ));
        }
        let guard_lookahead = if isp { detector_delay } else { 0 };
        let guard_delay = if isp { 4 * detector_delay } else { 0 };
        let core_controls = Controls {
            true_peak: controls.true_peak || isp,
            ..controls
        };
        let core = WetCore::new(
            channels,
            factor as usize,
            high_rate,
            high_delay,
            core_controls,
        );
        let mut wet = OversampledPlugin::new_with_max_frames(core, factor, channels, CHUNK)?;
        wet.initialize(rate)?;
        let guard_controls = Self::guard_controls(controls);
        let guard = prepare_kernel(channels, rate, guard_lookahead, guard_controls);
        let latency = wet
            .latency_samples()
            .checked_add(guard_delay)
            .ok_or_else(|| "limiter latency overflow".to_string())?;
        let TailLength::Finite(wet_tail) = wet.tail_length() else {
            return Err("prepared limiter wet path must have finite support".into());
        };
        let tail = wet_tail
            .checked_add(guard_delay as u64)
            .ok_or_else(|| "limiter tail overflow".to_string())?
            .max(latency as u64);
        Ok(Self {
            channels,
            rate,
            wet,
            guard,
            guard_controls,
            isp,
            mix: Smoother::new(mix, 5.0, rate),
            dry: Delay::new(latency, channels, 0.0),
            contribution_delay: Delay::new(guard_delay, channels, 1.0),
            first_gain_delay: vec![1.0; 3 * detector_delay * channels],
            guard_gains: vec![1.0; CHUNK * channels],
            dry_scratch: vec![0.0; CHUNK * channels],
            input_peaks: [0.0; CHUNK],
            output_detectors: (0..channels)
                .map(|_| Bs1770TruePeakDetector::new(rate))
                .collect(),
            peak_accumulator: 0.0,
            output_peak_accumulator: 0.0,
            gain_accumulator: 1.0,
            isp_accumulator: vec![0.0; channels],
            meter_frames: 0,
            accepted_phase: 0,
            output_frames: 0,
            received: false,
            phase: Phase::Open,
            guard_delay,
            guard_remaining: 0,
            dry_remaining: 0,
            latency,
            tail,
        })
    }
    fn guard_controls(controls: Controls) -> Controls {
        Controls {
            soft: false,
            true_peak: false,
            dual_release: false,
            ..controls
        }
    }
    pub fn is_draining(&self) -> bool {
        self.phase != Phase::Open
    }
    pub fn latency(&self) -> usize {
        self.latency
    }
    pub fn tail(&self) -> TailLength {
        TailLength::Finite(self.tail)
    }

    pub fn reset(&mut self, controls: Controls, isp: bool, mix: f32) {
        self.wet.inner_mut().set_reset_controls(Controls {
            true_peak: controls.true_peak || isp,
            ..controls
        });
        self.wet.reset();
        let guard_controls = Self::guard_controls(controls);
        guard_controls.install(self.guard_controls, &mut self.guard, self.rate);
        self.guard_controls = guard_controls;
        self.guard.reset();
        self.isp = isp;
        self.mix.reset(mix);
        self.dry.reset();
        self.contribution_delay.reset();
        self.first_gain_delay.fill(1.0);
        self.guard_gains.fill(1.0);
        self.dry_scratch.fill(0.0);
        self.input_peaks.fill(0.0);
        for detector in &mut self.output_detectors {
            detector.reset();
        }
        self.peak_accumulator = 0.0;
        self.output_peak_accumulator = 0.0;
        self.gain_accumulator = 1.0;
        self.isp_accumulator.fill(0.0);
        self.meter_frames = 0;
        self.accepted_phase = 0;
        self.output_frames = 0;
        self.received = false;
        self.phase = Phase::Open;
        self.guard_remaining = 0;
        self.dry_remaining = 0;
    }

    fn adopt_output_controls(&mut self, controls: Controls, isp: bool, mix: f32) {
        let guard = Self::guard_controls(controls);
        guard.install(self.guard_controls, &mut self.guard, self.rate);
        self.guard_controls = guard;
        self.isp = isp;
        if self.mix.target() != mix {
            self.mix.set_target(mix);
        }
    }

    fn prepare_dry(&mut self, input: Option<&mut [f32]>, frames: usize) {
        match input {
            Some(input) => {
                for frame in 0..frames {
                    let mut peak = 0.0_f32;
                    for channel in 0..self.channels {
                        let index = frame * self.channels + channel;
                        if !input[index].is_finite() {
                            input[index] = 0.0;
                        }
                        let value = input[index];
                        peak = peak.max(value.abs());
                        self.dry_scratch[index] = self.dry.sample(channel, self.channels, value);
                    }
                    self.input_peaks[frame] = peak;
                    self.dry.advance();
                }
            }
            None => {
                for frame in 0..frames {
                    for channel in 0..self.channels {
                        self.dry_scratch[frame * self.channels + channel] =
                            self.dry.sample(channel, self.channels, 0.0);
                    }
                    self.input_peaks[frame] = 0.0;
                    self.dry.advance();
                }
            }
        }
    }

    /// Protect and meter only actual delivered native-clock frames.
    fn finish(
        &mut self,
        audio: &mut [f32],
        context: &ProcessContext,
        wet_contributors: bool,
        measure_isp: bool,
        cache: &mut RealTimeCache<LimiterData>,
    ) -> PluginResult<()> {
        let channels = self.channels;
        let gains = &mut self.guard_gains;
        let first_delay = &mut self.first_gain_delay;
        let isp = self.isp;
        let isp_delay = self.guard.isp_delay_len;
        self.guard.process_observed(
            audio,
            context,
            self.guard_controls.kernel(channels, isp),
            None,
            |frame, channel, output_stage, position, before, after| {
                let gain = applied_gain(before, after);
                let index = frame * channels + channel;
                if !output_stage {
                    if isp && isp_delay > 0 {
                        let ring = position * channels + channel;
                        gains[index] = first_delay[ring];
                        first_delay[ring] = gain;
                    } else {
                        gains[index] = gain;
                    }
                } else {
                    gains[index] *= gain;
                }
            },
        )?;
        let interval = (self.rate as usize / 10).max(1);
        for frame in 0..context.num_frames {
            let mix = self.mix.advance();
            self.peak_accumulator = self.peak_accumulator.max(self.input_peaks[frame]);
            for channel in 0..channels {
                let index = frame * channels + channel;
                let contribution = if wet_contributors {
                    self.wet
                        .inner()
                        .contributing_gain(self.output_frames, channel)
                } else {
                    1.0
                };
                let contribution = self
                    .contribution_delay
                    .sample(channel, channels, contribution);
                let wet_gain = (contribution * gains[index]).clamp(0.0, 1.0);
                let mixed_gain = ((1.0 - mix) + mix * wet_gain).clamp(0.0, 1.0);
                self.gain_accumulator = self.gain_accumulator.min(mixed_gain);
                audio[index] = (1.0 - mix) * self.dry_scratch[index] + mix * audio[index];
                self.output_peak_accumulator =
                    self.output_peak_accumulator.max(audio[index].abs());
                let peak = self.output_detectors[channel].process_linear(audio[index]);
                if measure_isp {
                    self.isp_accumulator[channel] = self.isp_accumulator[channel].max(peak);
                }
            }
            self.contribution_delay.advance();
            self.output_frames += 1;
            self.meter_frames += 1;
            if self.meter_frames >= interval {
                let reduction = if self.gain_accumulator == 1.0 {
                    0.0
                } else {
                    -20.0 * self.gain_accumulator.max(1.0e-6).log10()
                };
                cache.update(|data| {
                    data.gain_reduction_db = reduction;
                    data.is_limiting = reduction > 0.01;
                    data.peak_db = 20.0 * self.peak_accumulator.max(1.0e-5).log10();
                    data.output_peak_db =
                        20.0 * self.output_peak_accumulator.max(1.0e-5).log10();
                    for (channel, peak) in data.isp_dbtp.iter_mut().enumerate() {
                        *peak = if measure_isp && self.isp_accumulator[channel] >= 1.0e-12 {
                            20.0 * self.isp_accumulator[channel].log10()
                        } else {
                            -120.0
                        };
                    }
                    for (channel, peak) in data.output_isp_dbtp.iter_mut().enumerate() {
                        *peak = if measure_isp && self.isp_accumulator[channel] >= 1.0e-12 {
                            20.0 * self.isp_accumulator[channel].log10()
                        } else {
                            -120.0
                        };
                    }
                });
                self.peak_accumulator = 0.0;
                self.output_peak_accumulator = 0.0;
                self.gain_accumulator = 1.0;
                self.isp_accumulator.fill(0.0);
                self.meter_frames = 0;
            }
        }
        Ok(())
    }

    pub fn process(
        &mut self,
        audio: &mut [f32],
        context: &ProcessContext,
        controls: Controls,
        isp: bool,
        mix: f32,
        cache: &mut RealTimeCache<LimiterData>,
    ) -> PluginResult<usize> {
        self.validate(context)?;
        if context.num_frames.checked_mul(self.channels) != Some(audio.len()) {
            return Err("limiter input must contain exactly the requested channel frames".into());
        }
        if context.num_frames > 0 && self.phase != Phase::Open {
            return Err("limiter requires reset before processing input after drain".into());
        }
        self.adopt_output_controls(controls, isp, mix);
        let core_controls = Controls {
            true_peak: controls.true_peak || isp,
            ..controls
        };
        let mut offset = 0;
        while offset < context.num_frames {
            let frames = (CHUNK - self.accepted_phase).min(context.num_frames - offset);
            let segment = &mut audio[offset * self.channels..(offset + frames) * self.channels];
            self.prepare_dry(Some(segment), frames);
            self.wet.inner_mut().append(frames, core_controls);
            let ctx = subcontext(context, offset, frames);
            if let Err(error) = self
                .wet
                .process_in_place(segment, &ctx)
                .and_then(|_| self.finish(segment, &ctx, true, controls.true_peak || isp, cache))
            {
                self.phase = Phase::Failed;
                return Err(error);
            }
            self.accepted_phase = (self.accepted_phase + frames) % CHUNK;
            self.received = true;
            offset += frames;
        }
        Ok(context.num_frames)
    }

    fn validate(&self, context: &ProcessContext) -> PluginResult<()> {
        if context.sample_rate != self.rate {
            return Err("limiter requires its initialized sample rate".into());
        }
        if self.phase == Phase::Failed {
            return Err("limiter requires reset after failed oversampled processing".into());
        }
        Ok(())
    }
    pub fn begin(
        &mut self,
        context: &ProcessContext,
        controls: Controls,
        isp: bool,
        mix: f32,
    ) -> PluginResult<()> {
        self.validate(context)?;
        if !self.received || self.phase != Phase::Open {
            return Ok(());
        }
        self.adopt_output_controls(controls, isp, mix);
        self.wet.inner_mut().freeze(Controls {
            true_peak: controls.true_peak || isp,
            ..controls
        });
        self.phase = Phase::Wet;
        self.guard_remaining = self.guard_delay;
        self.dry_remaining = self.latency;
        if let Err(error) = self.wet.begin_drain(context) {
            self.phase = Phase::Failed;
            return Err(error);
        }
        Ok(())
    }
    pub fn bound(&self) -> Option<NonZeroU64> {
        if !self.received || self.phase == Phase::Complete {
            return NonZeroU64::new(1);
        }
        let dry = self.dry_remaining.div_ceil(CHUNK) as u64;
        let guard = self.guard_remaining.div_ceil(CHUNK) as u64;
        let calls = match self.phase {
            Phase::Open | Phase::Failed => return None,
            Phase::Wet => self
                .wet
                .drain_call_bound()?
                .get()
                .checked_add(guard)?
                .checked_add(dry)?
                .checked_add(2)?,
            Phase::Guard => guard.checked_add(dry)?.checked_add(1)?,
            Phase::Dry => dry.max(1),
            Phase::Complete => 1,
        };
        NonZeroU64::new(calls)
    }
    pub fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
        controls: Controls,
        isp: bool,
        mix: f32,
        cache: &mut RealTimeCache<LimiterData>,
    ) -> PluginResult<PluginDrainResult> {
        self.validate(context)?;
        if !output.len().is_multiple_of(self.channels) {
            return Err("limiter drain output must contain whole channel frames".into());
        }
        if !self.received || self.phase == Phase::Complete {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let capacity = (output.len() / self.channels).min(CHUNK);
        if capacity == 0 {
            return Err("limiter drain needs at least one output frame".into());
        }
        self.begin(context, controls, isp, mix)?;
        let result = self.drain_step(output, context, capacity, controls.true_peak || isp, cache);
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }
    fn drain_step(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
        capacity: usize,
        measure_isp: bool,
        cache: &mut RealTimeCache<LimiterData>,
    ) -> PluginResult<PluginDrainResult> {
        let was_wet = self.phase == Phase::Wet;
        let mut frames = 0;
        match self.phase {
            Phase::Wet => {
                let result = self
                    .wet
                    .drain(&mut output[..capacity * self.channels], context)?;
                frames = result.frames;
                if result.complete {
                    self.phase = Phase::Guard;
                }
            }
            Phase::Guard => {
                frames = capacity.min(self.guard_remaining);
                output[..frames * self.channels].fill(0.0);
                self.guard_remaining -= frames;
                if self.guard_remaining == 0 {
                    self.phase = Phase::Dry;
                }
            }
            Phase::Dry => {
                frames = capacity.min(self.dry_remaining);
                output[..frames * self.channels].fill(0.0);
            }
            Phase::Open | Phase::Complete | Phase::Failed => {}
        }
        if frames > 0 {
            self.prepare_dry(None, frames);
            let mut ctx = *context;
            ctx.num_frames = frames;
            self.finish(
                &mut output[..frames * self.channels],
                &ctx,
                was_wet,
                measure_isp,
                cache,
            )?;
            self.dry_remaining = self.dry_remaining.saturating_sub(frames);
        }
        if self.phase == Phase::Guard && self.guard_remaining == 0 {
            self.phase = Phase::Dry;
        }
        if self.phase == Phase::Dry && self.dry_remaining == 0 {
            self.phase = Phase::Complete;
        }
        Ok(PluginDrainResult {
            frames,
            complete: self.phase == Phase::Complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls() -> Controls {
        Controls {
            threshold: 0.0,
            release: 10.0,
            soft: false,
            true_peak: true,
            dual_release: false,
            link: 0.0,
        }
    }

    #[test]
    fn guard_gain_labels_follow_isp_audio_delay_and_keep_channels_separate() {
        for same_channel in [true, false] {
            let mut path =
                OversampledPath::prepare(2, 48_000, 2, 0.25, controls(), true, 1.0).unwrap();
            let mut cache = RealTimeCache::new(LimiterData {
                isp_dbtp: vec![-120.0; 2],
                output_isp_dbtp: vec![-120.0; 2],
                ..LimiterData::default()
            });
            let first_frame = 20;
            let delayed_frame = first_frame + 18; // 3 * independent D=6 at 48 kHz.
            for frame in 0..70 {
                // Inject known first-stage and output-stage gain commands into
                // the private arithmetic, observing the real gain/clamp hooks.
                path.guard.release_coeff = 1.0;
                path.guard.channel_envelopes.fill(0.0);
                if frame == first_frame {
                    path.guard.channel_envelopes[0] = 6.020_6;
                }
                path.guard.envelope = path.guard.channel_envelopes[0];
                path.guard.isp_gains.fill(1.0);
                if frame == delayed_frame {
                    path.guard.isp_gains[usize::from(!same_channel)] = 0.25;
                }
                let mut audio = [0.1, 0.1];
                path.dry_scratch[..2].fill(0.0);
                path.finish(
                    &mut audio,
                    &ProcessContext::new(48_000, 1),
                    false,
                    true,
                    &mut cache,
                )
                .unwrap();
                let expected0 = if frame == delayed_frame {
                    if same_channel { 0.125 } else { 0.5 }
                } else {
                    1.0
                };
                let expected1 = if frame == delayed_frame && !same_channel {
                    0.25
                } else {
                    1.0
                };
                assert!(
                    (path.guard_gains[0] - expected0).abs() < 0.0005,
                    "first/ISP alignment frame{frame} same{same_channel}: {}",
                    path.guard_gains[0]
                );
                assert!((path.guard_gains[1] - expected1).abs() < 0.0005);
                if frame == delayed_frame {
                    let combined = path.guard_gains[..2]
                        .iter()
                        .copied()
                        .fold(1.0_f32, f32::min);
                    let expected_db = if same_channel { 18.061_8 } else { 12.041_2 };
                    assert!((-20.0 * combined.log10() - expected_db).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn contributor_delay_and_mix_are_gain_indicators_not_waveform_ratios() {
        let mut delay = Delay::new(24, 1, 1.0); // Native D+3D at 48 kHz.
        for frame in 0..900 {
            let core = if (256..768).contains(&frame) {
                0.5
            } else {
                1.0
            };
            let gain = delay.sample(0, 1, core);
            delay.advance();
            let expected = if (280..792).contains(&frame) {
                0.5
            } else {
                1.0
            };
            assert_eq!(gain, expected);
            if gain == 0.5 {
                assert!((-20.0_f32 * gain.log10() - 6.020_6).abs() < 1e-5);
                assert!((-20.0_f32 * (0.5 + 0.5 * gain).log10() - 2.498_775).abs() < 1e-5);
            }
        }
        // A direct final-stage synthetic negative control: opposing dry/wet
        // audio cancels, but no gain stage attenuates either contributor.
        let mut path = OversampledPath::prepare(1, 48_000, 2, 0.0, controls(), false, 0.5).unwrap();
        let mut cache = RealTimeCache::new(LimiterData {
            isp_dbtp: vec![-120.0],
            output_isp_dbtp: vec![-120.0],
            ..LimiterData::default()
        });
        for _ in 0..4800 {
            path.dry_scratch[0] = 0.01;
            path.input_peaks[0] = 0.01;
            let mut wet = [-0.01];
            path.finish(
                &mut wet,
                &ProcessContext::new(48_000, 1),
                false,
                false,
                &mut cache,
            )
            .unwrap();
            assert_eq!(wet[0], 0.0);
        }
        assert_eq!(cache.load().gain_reduction_db, 0.0);
        assert!(!cache.load().is_limiting);
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn descriptor_frontier_and_full_capacity_quotas_cover_every_residual_phase() {
        let controls = Controls {
            threshold: -6.0,
            release: 10.0,
            soft: false,
            true_peak: true,
            dual_release: true,
            link: 0.0,
        };
        let mut max_live = 0;
        let mut max_bound_ratio = 1.0_f64;
        for factor in [2, 4] {
            for phase in 0..256 {
                for lookahead in [0.0, 0.137, 20.0] {
                    let mut path = OversampledPath::prepare(
                        1, 192_000, factor, lookahead, controls, false, 1.0,
                    )
                    .unwrap();
                    let mut cache = RealTimeCache::new(LimiterData {
                        isp_dbtp: vec![-120.0],
                        output_isp_dbtp: vec![-120.0],
                        ..LimiterData::default()
                    });
                    let mut input = vec![0.7; 256 + phase];
                    // Single-frame acceptance observes the frontier immediately
                    // after a new full core unit becomes ready.
                    for sample in &mut input {
                        path.process(
                            std::slice::from_mut(sample),
                            &ProcessContext::new(192_000, 1),
                            controls,
                            false,
                            1.0,
                            &mut cache,
                        )
                        .unwrap();
                        max_live =
                            max_live.max(path.wet.inner().live_descriptors(path.output_frames));
                    }
                    let context = ProcessContext::new(192_000, 0);
                    path.begin(&context, controls, false, 1.0).unwrap();
                    max_live = max_live.max(path.wet.inner().live_descriptors(path.output_frames));
                    let initial = path.bound().unwrap().get();
                    let mut calls = 0_u64;
                    let mut buffer = [0.0; 256];
                    loop {
                        let bound = path.bound().unwrap().get();
                        let result = path
                            .drain(&mut buffer, &context, controls, false, 1.0, &mut cache)
                            .unwrap();
                        calls += 1;
                        assert!(calls <= initial && bound > 0);
                        max_live =
                            max_live.max(path.wet.inner().live_descriptors(path.output_frames));
                        if result.complete {
                            break;
                        }
                    }
                    max_bound_ratio = max_bound_ratio.max(initial as f64 / calls as f64);
                }
            }
        }
        // Capacity is eight; the conservative derivation allows at most five
        // live block labels including the preceding overlap. This measurement
        // catches a future wrapper scheduler that invalidates that proof.
        assert!(max_live <= 5, "live contributor frontier {max_live}");
        println!(
            "limiter contributor high-water={max_live}/8; maximum full-capacity bound/actual={max_bound_ratio:.3}"
        );
    }
}
