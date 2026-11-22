//! Processes a surround bed and generates height signals.

// Rust guideline compliant 2026-10-09

use crate::surround_bass::{BassMode, BassSettings, SurroundBass};
use crate::surround_routing::SurroundRouting;
use crate::{SURROUND_BASS_SOURCE_ROLES, SurroundBassMode, UpmixerPlugin, UpmixerPluginParams};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, PluginDrainResult, ProcessContext, TailLength};
use sotf_host::smoothing::Smoother;
use sotf_host::speaker_config::{ChannelLayout, ChannelRole};

const CHUNK_FRAMES: usize = 256;
const PAIR_OUTPUT_CHANNELS: usize = 8;
const HEIGHT_REFERENCE_GAIN: f32 = 0.251_188_64;
const HEIGHT_DELAY_SECONDS: [f64; 6] = [0.00013, 0.00017, 0.00019, 0.00023, 0.00027, 0.00031];

/// A short, unity-magnitude delay with independent state for one height output.
struct HeightDelay {
    samples: Vec<f32>,
    cursor: usize,
}

impl HeightDelay {
    fn new(sample_rate: f64, index: usize) -> Self {
        let frames = (sample_rate * HEIGHT_DELAY_SECONDS[index]).round().max(1.0) as usize;
        Self {
            samples: vec![0.0; frames],
            cursor: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let output = self.samples[self.cursor];
        self.samples[self.cursor] = input;
        self.cursor += 1;
        if self.cursor == self.samples.len() {
            self.cursor = 0;
        }
        output
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
    }
}

#[derive(Clone, Copy)]
struct SurroundControlFrame {
    gain: f32,
    limit: f32,
    decorrelation: f32,
    bypass: f32,
}

/// A surround bed router with independent front and rear height analysis.
pub(super) struct SurroundProcessor {
    routing: SurroundRouting,
    bass: SurroundBass,
    front: Option<Box<UpmixerPlugin>>,
    rear: Option<Box<UpmixerPlugin>>,
    front_input: Vec<f32>,
    rear_input: Vec<f32>,
    front_output: Vec<f32>,
    rear_output: Vec<f32>,
    delayed_bed: Vec<f32>,
    delay_cursor: usize,
    latency: usize,
    height_gain: Smoother,
    height_limit: Smoother,
    height_delays: Vec<HeightDelay>,
    decorrelation_mix: Smoother,
    requested_height_gain: f32,
    bypass_heights: bool,
    bypass: Smoother,
    delayed_controls: Vec<SurroundControlFrame>,
    control_cursor: usize,
    sample_rate: f64,
    has_input: bool,
    drain_remaining: Option<usize>,
}

impl SurroundProcessor {
    /// Validates user configuration and prepares the surround processor.
    pub(super) fn from_params(
        output: &ChannelLayout,
        params: UpmixerPluginParams,
        sample_rate: f64,
    ) -> Result<Self, String> {
        let input = params
            .multichannel
            .input_layout
            .clone()
            .ok_or("surround input requires an explicit channel layout")?;
        input.validate_for_width(input.channels.len())?;
        let mut selected = vec![false; input.channels.len()];
        if params.multichannel.bass_sources.is_some()
            && params.multichannel.bass_source_mask.is_some()
        {
            return Err("select bass sources using roles or a mask, not both".into());
        }
        if let Some(mask) = params.multichannel.bass_source_mask {
            if mask & !0x7f != 0 {
                return Err("bass source mask contains an unsupported role".into());
            }
            for assignment in &input.channels {
                if let Some(index) = SURROUND_BASS_SOURCE_ROLES
                    .iter()
                    .position(|role| *role == assignment.role)
                {
                    selected[assignment.index] = mask & (1 << index) != 0;
                }
            }
        } else if let Some(roles) = &params.multichannel.bass_sources {
            for role in roles {
                let assignment = input
                    .channels
                    .iter()
                    .find(|assignment| assignment.role == *role)
                    .ok_or_else(|| format!("bass source {role:?} is absent from input"))?;
                if *role == ChannelRole::Lfe || selected[assignment.index] {
                    return Err("bass sources must be unique non-LFE input roles".into());
                }
                selected[assignment.index] = true;
            }
        } else {
            for assignment in &input.channels {
                selected[assignment.index] = assignment.role != ChannelRole::Lfe;
            }
        }
        let bass_mode = match params.multichannel.bass_mode {
            SurroundBassMode::Off => BassMode::Off,
            SurroundBassMode::Derived => BassMode::Derived,
            SurroundBassMode::Managed => BassMode::Managed,
        };
        let gain_db = params.multichannel.bass_gain_db;
        let lfe_gain_db = params.multichannel.lfe_playback_gain_db;
        if !gain_db.is_finite()
            || !lfe_gain_db.is_finite()
            || !(-60.0..=12.0).contains(&gain_db)
            || !(0.0..=10.0).contains(&lfe_gain_db)
        {
            return Err("bass and LFE gains are outside their supported ranges".into());
        }
        let settings = BassSettings {
            mode: bass_mode,
            cutoff_hz: f64::from(params.multichannel.bass_cutoff_hz),
            selected,
            source_gains: vec![1.0; input.channels.len()],
            sub_gain: 10.0_f32.powf((gain_db - lfe_gain_db) / 20.0),
            combine_input_lfe: params.multichannel.combine_input_lfe,
        };
        Self::new(&input, output, params, sample_rate, settings)
    }

    /// Builds the routing, filters, analysis engines, and callback buffers.
    pub(super) fn new(
        input: &ChannelLayout,
        output: &ChannelLayout,
        mut params: UpmixerPluginParams,
        sample_rate: f64,
        settings: BassSettings,
    ) -> Result<Self, String> {
        let routing = SurroundRouting::new(input, output)?;
        let bass = SurroundBass::new(&routing, sample_rate, settings)?;
        if !params.height.height_gain.is_finite() || !params.core.safety_cap_db.is_finite() {
            return Err("height gain and safety cap must be finite".into());
        }
        let requested_height_gain = params.height.height_gain.clamp(0.0, 2.0);
        let height_limit = 10.0_f32.powf(params.core.safety_cap_db.clamp(0.0, 3.0) / 20.0);
        let bypass_heights = params.multichannel.bypass_heights;
        let bypass_decorrelation = params.bypass.bypass_decorrelation;
        let gain = if bypass_heights {
            0.0
        } else {
            requested_height_gain * HEIGHT_REFERENCE_GAIN
        };
        let bypass = params.bypass.bypass_all_processing || params.multichannel.bypass_surround;
        let has_heights = !routing.heights.is_empty();
        let (front, rear, latency) = if has_heights {
            params.multichannel.input_layout = None;
            params.multichannel.bass_mode = SurroundBassMode::Off;
            params.multichannel.bypass_heights = false;
            params.multichannel.bypass_surround = false;
            params.core.speaker_config = "5.1.2".into();
            params.core.enable_hr_direct = false;
            params.core.binaural_preview = false;
            params.core.safety_cap_db = 60.0;
            params.height.height_gain = 1.0;
            params.subharmonic.enable_subharmonic_synth = false;
            params.output.auto_gain_enabled = false;
            params.bypass.bypass_all_processing = false;
            let mut front = Box::new(UpmixerPlugin::from_params(params.clone()));
            let mut rear = Box::new(UpmixerPlugin::from_params(params));
            front.core.defer_surround_analysis = true;
            rear.core.defer_surround_analysis = true;
            front.initialize(sample_rate)?;
            rear.initialize(sample_rate)?;
            let latency = front.latency_samples();
            if rear.latency_samples() != latency {
                return Err("height analysis paths have different latencies".into());
            }
            (Some(front), Some(rear), latency)
        } else {
            (None, None, 0)
        };
        let delayed_bed = vec![0.0; latency * routing.input_channels];
        let height_delays = (0..routing.heights.len())
            .map(|index| HeightDelay::new(sample_rate, index))
            .collect();
        let extra_latency = front.as_ref().map_or(0, |plugin| {
            plugin.output_latency_frames() - plugin.core.fft_size.max(plugin.fft.hr_fft_size)
        });
        let initial_control = SurroundControlFrame {
            gain,
            limit: height_limit,
            decorrelation: if bypass_decorrelation { 0.0 } else { 1.0 },
            bypass: if bypass { 1.0 } else { 0.0 },
        };
        Ok(Self {
            routing,
            bass,
            front,
            rear,
            front_input: vec![0.0; CHUNK_FRAMES * 2],
            rear_input: vec![0.0; CHUNK_FRAMES * 2],
            front_output: vec![0.0; CHUNK_FRAMES * PAIR_OUTPUT_CHANNELS],
            rear_output: vec![0.0; CHUNK_FRAMES * PAIR_OUTPUT_CHANNELS],
            delayed_bed,
            delay_cursor: 0,
            latency,
            height_gain: Smoother::new(gain, 5.0, sample_rate),
            height_limit: Smoother::new(height_limit, 5.0, sample_rate),
            height_delays,
            decorrelation_mix: Smoother::new(
                if bypass_decorrelation { 0.0 } else { 1.0 },
                5.0,
                sample_rate,
            ),
            requested_height_gain,
            bypass_heights,
            bypass: Smoother::new(if bypass { 1.0 } else { 0.0 }, 5.0, sample_rate),
            delayed_controls: vec![initial_control; extra_latency],
            control_cursor: 0,
            sample_rate,
            has_input: false,
            drain_remaining: None,
        })
    }

    /// Reports the common bed and height delay in frames.
    pub(super) fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Reports the interleaved input width.
    pub(super) fn input_channels(&self) -> usize {
        self.routing.input_channels
    }

    /// Reports the interleaved output width.
    pub(super) fn output_channels(&self) -> usize {
        self.routing.output_channels
    }

    /// Reinitializes all sample-rate-dependent state.
    pub(super) fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        self.bass.initialize(sample_rate)?;
        if let Some(front) = &mut self.front {
            front.initialize(sample_rate)?;
        }
        if let Some(rear) = &mut self.rear {
            rear.initialize(sample_rate)?;
        }
        if let (Some(front), Some(rear)) = (&self.front, &self.rear) {
            let latency = front.latency_samples();
            if rear.latency_samples() != latency {
                return Err("height analysis paths have different latencies".into());
            }
            self.latency = latency;
            self.delayed_bed
                .resize(latency * self.routing.input_channels, 0.0);
            let extra_latency =
                front.output_latency_frames() - front.core.fft_size.max(front.fft.hr_fft_size);
            self.delayed_controls.resize(
                extra_latency,
                SurroundControlFrame {
                    gain: self.height_gain.target(),
                    limit: self.height_limit.target(),
                    decorrelation: self.decorrelation_mix.target(),
                    bypass: self.bypass.target(),
                },
            );
        }
        self.height_gain.set_time(5.0, sample_rate);
        self.height_limit.set_time(5.0, sample_rate);
        self.decorrelation_mix.set_time(5.0, sample_rate);
        for (index, delay) in self.height_delays.iter_mut().enumerate() {
            *delay = HeightDelay::new(sample_rate, index);
        }
        self.bypass.set_time(5.0, sample_rate);
        self.sample_rate = sample_rate;
        self.reset();
        Ok(())
    }

    /// Sets the generated height level without changing the bed.
    pub(super) fn set_height_gain(&mut self, gain: f32) -> Result<(), String> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err("height gain must be finite and between 0 and 2".into());
        }
        self.requested_height_gain = gain;
        self.height_gain.set_target(if self.bypass_heights {
            0.0
        } else {
            gain * HEIGHT_REFERENCE_GAIN
        });
        Ok(())
    }

    /// Mutes or restores heights while leaving bass management active.
    pub(super) fn set_height_bypass(&mut self, bypass: bool) {
        self.bypass_heights = bypass;
        self.height_gain.set_target(if bypass {
            0.0
        } else {
            self.requested_height_gain * HEIGHT_REFERENCE_GAIN
        });
    }

    /// Updates the cap for generated heights without touching bed or subwoofer samples.
    pub(super) fn set_height_safety_cap(&mut self, decibels: f32) {
        self.height_limit
            .set_target(10.0_f32.powf(decibels.clamp(0.0, 3.0) / 20.0));
    }

    /// Crossfades the processed output to the delayed input bed.
    pub(super) fn set_bypass(&mut self, bypass: bool) {
        self.bypass.set_target(if bypass { 1.0 } else { 0.0 });
    }

    /// Updates controls shared by both height analysis paths.
    pub(super) fn set_analysis_parameter(
        &mut self,
        id: &ParameterId,
        value: &ParameterValue,
    ) -> Result<(), String> {
        if let Some(front) = &mut self.front {
            front.set_parameter(id.clone(), value.clone())?;
        }
        if let Some(rear) = &mut self.rear {
            rear.set_parameter(id.clone(), value.clone())?;
        }
        if id.as_str() == "bypass_decorrelation" {
            let bypass = value
                .as_bool()
                .ok_or("bypass_decorrelation must be a boolean")?;
            self.decorrelation_mix
                .set_target(if bypass { 0.0 } else { 1.0 });
        }
        Ok(())
    }

    /// Clears audio history and leaves the current gain target in place.
    pub(super) fn reset(&mut self) {
        if let Some(front) = &mut self.front {
            front.reset();
        }
        if let Some(rear) = &mut self.rear {
            rear.reset();
        }
        self.bass.reset();
        self.delayed_bed.fill(0.0);
        self.delay_cursor = 0;
        self.height_gain.reset(self.height_gain.target());
        self.height_limit.reset(self.height_limit.target());
        self.decorrelation_mix
            .reset(self.decorrelation_mix.target());
        for delay in &mut self.height_delays {
            delay.reset();
        }
        self.bypass.reset(self.bypass.target());
        self.delayed_controls.fill(SurroundControlFrame {
            gain: self.height_gain.target(),
            limit: self.height_limit.target(),
            decorrelation: self.decorrelation_mix.target(),
            bypass: self.bypass.target(),
        });
        self.control_cursor = 0;
        self.has_input = false;
        self.drain_remaining = None;
    }

    /// Reports a finite height tail only when no recursive bass filter is active.
    pub(super) fn tail_length(&self) -> TailLength {
        if self.bass.has_recursive_tail() {
            TailLength::Unknown
        } else if let Some(front) = &self.front {
            match front.tail_length() {
                TailLength::Finite(frames) => {
                    TailLength::Finite(frames.saturating_add(self.maximum_height_delay() as u64))
                }
                tail => tail,
            }
        } else {
            TailLength::Finite(self.latency as u64)
        }
    }

    /// Reports the maximum number of frames emitted by one drain call.
    pub(super) fn drain_output_frames_max(&self) -> usize {
        CHUNK_FRAMES
    }

    /// Reports whether the stream has entered its drain lifecycle.
    pub(super) fn is_draining(&self) -> bool {
        self.drain_remaining.is_some()
    }

    /// Bounds calls when the host provides a full drain buffer.
    pub(super) fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        let remaining = self
            .drain_remaining
            .unwrap_or_else(|| self.drain_tail_frames());
        let calls = if self.has_input {
            remaining.div_ceil(CHUNK_FRAMES).max(1)
        } else {
            1
        };
        std::num::NonZeroU64::new(u64::try_from(calls).ok()?)
    }

    fn drain_tail_frames(&self) -> usize {
        let height_tail = match self.front.as_ref().map(|front| front.tail_length()) {
            Some(TailLength::Finite(frames)) => usize::try_from(frames)
                .unwrap_or(usize::MAX)
                .saturating_add(self.maximum_height_delay()),
            Some(TailLength::Infinite | TailLength::Unknown) => self.latency.saturating_mul(4),
            None => self.latency,
        };
        if self.bass.has_recursive_tail() {
            // An IIR response is not mathematically finite. The render cap is
            // separate from tail_length(), which remains Unknown.
            height_tail.saturating_add((self.sample_rate * 2.0).ceil() as usize)
        } else {
            height_tail
        }
    }

    fn maximum_height_delay(&self) -> usize {
        self.height_delays
            .iter()
            .map(|delay| delay.samples.len())
            .max()
            .unwrap_or(0)
    }

    /// Emits a bounded portion of the delayed bed and filter tails.
    pub(super) fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if output.is_empty() || !output.len().is_multiple_of(self.routing.output_channels) {
            return Err("surround drain requires complete output frames".into());
        }
        let remaining = self
            .drain_remaining
            .unwrap_or_else(|| self.drain_tail_frames());
        let frames = remaining
            .min(CHUNK_FRAMES)
            .min(output.len() / self.routing.output_channels);
        if frames == 0 {
            self.drain_remaining = Some(0);
            return Ok(PluginDrainResult::COMPLETE);
        }
        let silence = [0.0_f32; CHUNK_FRAMES * 8];
        let mut drain_context = *context;
        drain_context.num_frames = frames;
        self.render(
            &silence[..frames * self.routing.input_channels],
            &mut output[..frames * self.routing.output_channels],
            &drain_context,
        )?;
        let remaining = remaining - frames;
        self.drain_remaining = Some(remaining);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == 0,
        })
    }

    /// Processes complete interleaved frames using prepared scratch buffers.
    pub(super) fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("surround processor must be reset after drain".into());
        }
        let frames = self.render(input, output, context)?;
        if frames > 0 {
            self.has_input = true;
        }
        Ok(frames)
    }

    fn render(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let input_samples = context
            .num_frames
            .checked_mul(self.routing.input_channels)
            .ok_or("surround input length overflow")?;
        let output_samples = context
            .num_frames
            .checked_mul(self.routing.output_channels)
            .ok_or("surround output length overflow")?;
        if input.len() != input_samples || output.len() != output_samples {
            return Err("surround input or output buffer has the wrong length".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("surround input contains a non-finite sample".into());
        }

        let mut position = 0;
        while position < context.num_frames {
            let frames = (context.num_frames - position).min(CHUNK_FRAMES);
            if let (Some(front), Some(rear)) = (&mut self.front, &mut self.rear) {
                for frame in 0..frames {
                    let source = &input[(position + frame) * self.routing.input_channels..];
                    self.front_input[frame * 2] = source[self.routing.front[0]].clamp(-16.0, 16.0);
                    self.front_input[frame * 2 + 1] =
                        source[self.routing.front[1]].clamp(-16.0, 16.0);
                    for side in 0..2 {
                        let side_sample = source[self.routing.side[side]].clamp(-16.0, 16.0);
                        self.rear_input[frame * 2 + side] = if let Some(back) = self.routing.back {
                            side_sample / 3.0 + source[back[side]].clamp(-16.0, 16.0) * (2.0 / 3.0)
                        } else {
                            side_sample
                        };
                    }
                }
                let pair_context = ProcessContext::new(context.sample_rate, frames);
                front.process(
                    &self.front_input[..frames * 2],
                    &mut self.front_output[..frames * PAIR_OUTPUT_CHANNELS],
                    &pair_context,
                )?;
                rear.process(
                    &self.rear_input[..frames * 2],
                    &mut self.rear_output[..frames * PAIR_OUTPUT_CHANNELS],
                    &pair_context,
                )?;
            }

            for frame in 0..frames {
                let input_start = (position + frame) * self.routing.input_channels;
                let output_start = (position + frame) * self.routing.output_channels;
                let source = &input[input_start..input_start + self.routing.input_channels];
                let destination =
                    &mut output[output_start..output_start + self.routing.output_channels];
                destination.fill(0.0);
                let mut delayed = [0.0_f32; 8];
                if self.latency == 0 {
                    delayed[..self.routing.input_channels].copy_from_slice(source);
                } else {
                    let start = self.delay_cursor * self.routing.input_channels;
                    let slot = &mut self.delayed_bed[start..start + self.routing.input_channels];
                    delayed[..self.routing.input_channels].copy_from_slice(slot);
                    slot.copy_from_slice(source);
                    self.delay_cursor = (self.delay_cursor + 1) % self.latency;
                }
                self.bass
                    .process_frame(&self.routing, &delayed, destination);

                let mut control = SurroundControlFrame {
                    gain: self.height_gain.advance(),
                    limit: self.height_limit.advance(),
                    decorrelation: self.decorrelation_mix.advance(),
                    bypass: self.bypass.advance(),
                };
                if !self.delayed_controls.is_empty() {
                    std::mem::swap(
                        &mut control,
                        &mut self.delayed_controls[self.control_cursor],
                    );
                    self.control_cursor = (self.control_cursor + 1) % self.delayed_controls.len();
                }
                let SurroundControlFrame {
                    gain,
                    limit,
                    decorrelation,
                    bypass,
                } = control;
                let pair_start = frame * PAIR_OUTPUT_CHANNELS;
                for (height_index, &(target, role)) in self.routing.heights.iter().enumerate() {
                    let side = match role {
                        ChannelRole::TopFrontLeft
                        | ChannelRole::TopMiddleLeft
                        | ChannelRole::TopBackLeft => 0,
                        _ => 1,
                    };
                    let front = self.front_output[pair_start + 6 + side];
                    let rear = self.rear_output[pair_start + 6 + side];
                    let sample = match role {
                        ChannelRole::TopFrontLeft | ChannelRole::TopFrontRight
                            if self.routing.heights.len() > 2 =>
                        {
                            front
                        }
                        ChannelRole::TopBackLeft | ChannelRole::TopBackRight => rear,
                        _ => 0.5 * (front + rear),
                    };
                    let candidate = sample * gain;
                    let candidate = if candidate.is_finite() {
                        candidate
                    } else {
                        0.0
                    };
                    let delayed = self.height_delays[height_index].process(candidate);
                    let height = candidate + (delayed - candidate) * decorrelation;
                    destination[target] = if height.is_finite() {
                        height.clamp(-limit, limit)
                    } else {
                        0.0
                    };
                }
                if bypass > 0.0 {
                    if bypass >= 1.0 {
                        for (channel, output) in destination.iter_mut().enumerate() {
                            *output = self.routing.dry_source[channel]
                                .map_or(0.0, |index| delayed[index]);
                        }
                    } else {
                        for (channel, output) in destination.iter_mut().enumerate() {
                            let dry = self.routing.dry_source[channel]
                                .map_or(0.0, |index| delayed[index]);
                            if output.to_bits() != dry.to_bits() {
                                *output += (dry - *output) * bypass;
                            }
                        }
                    }
                }
            }
            position += frames;
        }
        Ok(context.num_frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::speaker_config::get_speaker_config;

    fn layout(id: &str) -> ChannelLayout {
        ChannelLayout::from_speaker_config(get_speaker_config(id).unwrap()).unwrap()
    }

    #[test]
    fn per_height_delays_have_unity_magnitude_and_known_phase() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut delays: Vec<_> = (0..6)
                .map(|index| HeightDelay::new(sample_rate, index))
                .collect();
            assert!(
                delays
                    .windows(2)
                    .all(|pair| pair[0].samples.len() < pair[1].samples.len())
            );
            for delay in &mut delays {
                let length = delay.samples.len();
                let impulse: Vec<_> = (0..length + 1)
                    .map(|frame| delay.process(if frame == 0 { 1.0 } else { 0.0 }))
                    .collect();
                assert_eq!(impulse.iter().filter(|sample| **sample != 0.0).count(), 1);
                assert_eq!(impulse[length], 1.0);
                for frequency in [100.0, 1_000.0, 10_000.0] {
                    let phase = -std::f64::consts::TAU * frequency * length as f64 / sample_rate;
                    let (real, imag) = impulse.iter().enumerate().fold(
                        (0.0, 0.0),
                        |(real, imag), (frame, sample)| {
                            let angle =
                                -std::f64::consts::TAU * frequency * frame as f64 / sample_rate;
                            (
                                real + f64::from(*sample) * angle.cos(),
                                imag + f64::from(*sample) * angle.sin(),
                            )
                        },
                    );
                    assert!((real.hypot(imag) - 1.0).abs() < 1e-12);
                    assert!((real - phase.cos()).abs() < 1e-12);
                    assert!((imag - phase.sin()).abs() < 1e-12);
                }
                delay.reset();
                assert_eq!(delay.process(0.0), 0.0);
            }
        }
    }

    fn processor(input: &str, output: &str) -> SurroundProcessor {
        let input = layout(input);
        let output = layout(output);
        let mut params = UpmixerPluginParams::default();
        params.core.fft_size = 512;
        SurroundProcessor::new(
            &input,
            &output,
            params,
            48_000.0,
            BassSettings {
                mode: BassMode::Off,
                cutoff_hz: 80.0,
                selected: vec![false; input.channels.len()],
                source_gains: vec![1.0; input.channels.len()],
                sub_gain: 1.0,
                combine_input_lfe: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn preserves_each_bed_sample_after_the_height_delay() {
        for input_id in ["5.0", "5.1", "7.0", "7.1"] {
            let input_layout = layout(input_id);
            let output_layout = layout("9.1.6");
            let mut plugin = processor(input_id, "9.1.6");
            let latency = plugin.latency_samples();
            let frames = latency + 1021;
            let input: Vec<f32> = (0..frames * input_layout.channels.len())
                .map(|sample| ((sample * 37 % 997) as f32 - 498.0) / 1000.0)
                .collect();
            let mut output = vec![0.0; frames * output_layout.channels.len()];
            let context = ProcessContext::new(48_000, frames);
            assert_eq!(
                plugin.process(&input, &mut output, &context).unwrap(),
                frames
            );
            for assignment in &input_layout.channels {
                let target = output_layout
                    .channels
                    .iter()
                    .find(|output| output.role == assignment.role)
                    .unwrap()
                    .index;
                for frame in latency..frames {
                    assert_eq!(
                        output[frame * output_layout.channels.len() + target].to_bits(),
                        input[(frame - latency) * input_layout.channels.len() + assignment.index]
                            .to_bits(),
                        "{input_id} {:?} frame {frame}",
                        assignment.role,
                    );
                }
            }
        }
    }

    #[test]
    fn five_one_six_preserves_bed_and_drives_six_height_speakers() {
        for input_id in ["5.0", "5.1"] {
            let input_layout = layout(input_id);
            let output_layout = layout("5.1.6");
            let mut plugin = processor(input_id, "5.1.6");
            let latency = plugin.latency_samples();
            let frames = latency + 8192;
            let width = input_layout.channels.len();
            let mut random = 0x1345_6789_u32;
            let input: Vec<f32> = (0..frames * width)
                .map(|_| {
                    random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (random as f32 / u32::MAX as f32 - 0.5) * 0.2
                })
                .collect();
            let mut output = vec![0.0; frames * 12];
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap();
            for assignment in &input_layout.channels {
                let target = output_layout
                    .channels
                    .iter()
                    .find(|target| target.role == assignment.role)
                    .unwrap()
                    .index;
                for frame in latency..frames {
                    assert_eq!(
                        output[frame * 12 + target].to_bits(),
                        input[(frame - latency) * width + assignment.index].to_bits(),
                        "{input_id} {:?} at {frame}",
                        assignment.role
                    );
                }
            }
            if input_id == "5.0" {
                assert!(output.chunks_exact(12).all(|frame| frame[3] == 0.0));
            }
            for height in 6..12 {
                let energy: f64 = output
                    .chunks_exact(12)
                    .map(|frame| f64::from(frame[height]).powi(2))
                    .sum();
                assert!(energy > 1e-8, "{input_id} height {height}: {energy}");
            }
        }
    }

    #[test]
    fn center_and_lfe_do_not_generate_height() {
        for (layout, width) in [("5.1.4", 10), ("5.1.6", 12)] {
            let mut plugin = processor("5.1", layout);
            let frames = plugin.latency_samples() * 3;
            let mut input = vec![0.0; frames * 6];
            for frame in 0..frames {
                input[frame * 6 + 2] = 0.4;
                input[frame * 6 + 3] = 0.6;
            }
            let mut output = vec![0.0; frames * width];
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap();
            for frame in output.chunks_exact(width) {
                assert!(frame[6..].iter().all(|sample| *sample == 0.0), "{layout}");
            }
            plugin.reset();
            plugin.set_height_gain(0.0).unwrap();
        }
    }

    #[test]
    fn rear_ambient_reaches_rear_heights_without_feeding_front_heights() {
        let mut plugin = processor("5.1", "5.1.4");
        let frames = 4096;
        let mut input = vec![0.0; frames * 6];
        let mut random = 0x1234_5678_u32;
        for frame in 0..frames {
            for channel in [4, 5] {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                input[frame * 6 + channel] = (random as f32 / u32::MAX as f32 - 0.5) * 0.2;
            }
        }
        let mut output = vec![0.0; frames * 10];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        let mut rear_energy = 0.0_f64;
        for frame in output.chunks_exact(10) {
            assert_eq!(&frame[6..8], &[0.0; 2]);
            rear_energy += f64::from(frame[8]).powi(2) + f64::from(frame[9]).powi(2);
        }
        assert!(
            rear_energy > 1e-8,
            "rear height signal must contain ambient energy"
        );
    }

    #[test]
    fn height_safety_does_not_change_extreme_finite_bed_samples() {
        let mut plugin = processor("5.0", "5.1.4");
        let latency = plugin.latency_samples();
        let frames = latency + 4096;
        let input: Vec<f32> = (0..frames * 5)
            .map(|index| {
                if index % 2 == 0 {
                    f32::MAX / 4.0
                } else {
                    -f32::MAX / 4.0
                }
            })
            .collect();
        let mut output = vec![0.0; frames * 10];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        for frame in latency..frames {
            for (source, target) in [(0, 0), (1, 1), (2, 2), (3, 4), (4, 5)] {
                assert_eq!(
                    output[frame * 10 + target].to_bits(),
                    input[(frame - latency) * 5 + source].to_bits()
                );
            }
            for height in &output[frame * 10 + 6..frame * 10 + 10] {
                assert!(height.is_finite() && height.abs() <= 10.0_f32.powf(3.0 / 20.0));
            }
        }
    }

    #[test]
    fn bass_mask_selects_center_and_applies_lfe_playback_compensation() {
        let mut params = UpmixerPluginParams::default();
        params.core.speaker_config = "5.1".into();
        params.multichannel.input_layout = Some(layout("5.0"));
        params.multichannel.bass_mode = SurroundBassMode::Managed;
        params.multichannel.bass_source_mask = Some(1 << 2);
        params.multichannel.lfe_playback_gain_db = 10.0;
        let mut plugin = UpmixerPlugin::try_from_params(params).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let frames = 48_000;
        let input: Vec<f32> = [0.7, 0.6, 0.5, 0.4, 0.3].repeat(frames);
        let mut output = vec![0.0; frames * 6];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        let last = &output[(frames - 1) * 6..];
        assert_eq!(last[0], 0.7);
        assert_eq!(last[1], 0.6);
        assert!(last[2].abs() < 1e-5);
        assert!((last[3] - 0.5 / 10.0_f32.sqrt()).abs() < 1e-5);
        assert_eq!(&last[4..], &[0.4, 0.3]);

        let mut invalid = UpmixerPluginParams::default();
        invalid.core.speaker_config = "5.1".into();
        invalid.multichannel.input_layout = Some(layout("5.0"));
        invalid.multichannel.bass_source_mask = Some(0xff);
        assert!(UpmixerPlugin::try_from_params(invalid).is_err());
    }

    #[test]
    fn coherent_derived_bass_exposes_the_sum_without_automatic_normalization() {
        let mut params = UpmixerPluginParams::default();
        params.core.speaker_config = "5.1".into();
        params.multichannel.input_layout = Some(layout("5.0"));
        params.multichannel.bass_mode = SurroundBassMode::Derived;
        let mut plugin = UpmixerPlugin::try_from_params(params).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let frames = 48_000;
        let input = vec![0.8; frames * 5];
        let mut output = vec![0.0; frames * 6];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        assert!((output[(frames - 1) * 6 + 3] - 4.0).abs() < 1e-4);
        assert_eq!(output[(frames - 1) * 6], 0.8);
    }

    #[test]
    fn processing_is_independent_of_host_block_partitions() {
        let frames = 4097;
        let mut input = vec![0.0; frames * 8];
        let mut random = 0x8765_4321_u32;
        for sample in &mut input {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *sample = (random as f32 / u32::MAX as f32 - 0.5) * 0.2;
        }
        let mut whole = processor("7.1", "7.1.4");
        let mut split = processor("7.1", "7.1.4");
        let mut expected = vec![0.0; frames * 12];
        let mut actual = vec![0.0; frames * 12];
        whole
            .process(&input, &mut expected, &ProcessContext::new(48_000, frames))
            .unwrap();
        let pattern = [1, 127, 257, 31, 512];
        let mut position = 0;
        let mut part = 0;
        while position < frames {
            let count = pattern[part % pattern.len()].min(frames - position);
            split
                .process(
                    &input[position * 8..(position + count) * 8],
                    &mut actual[position * 12..(position + count) * 12],
                    &ProcessContext::new(48_000, count),
                )
                .unwrap();
            position += count;
            part += 1;
        }
        for (index, (a, b)) in actual.iter().zip(&expected).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {index}: {a} != {b}");
        }
    }

    #[test]
    fn coherent_and_antiphase_sources_are_not_treated_as_diffuse() {
        let frames = 16_384;
        let mut signals = [
            vec![0.0; frames * 6],
            vec![0.0; frames * 6],
            vec![0.0; frames * 6],
        ];
        let mut random = 0x1234_9876_u32;
        for frame in 0..frames {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let left = (random as f32 / u32::MAX as f32 - 0.5) * 0.2;
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let independent = (random as f32 / u32::MAX as f32 - 0.5) * 0.2;
            for input in &mut signals {
                input[frame * 6] = left;
            }
            signals[0][frame * 6 + 1] = left;
            signals[1][frame * 6 + 1] = -left;
            signals[2][frame * 6 + 1] = independent;
        }
        let mut energy = [0.0_f64; 3];
        for (case, input) in signals.iter().enumerate() {
            let mut plugin = processor("5.1", "5.1.4");
            let mut output = vec![0.0; frames * 10];
            plugin
                .process(input, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap();
            for frame in output.chunks_exact(10).skip(2048) {
                energy[case] += f64::from(frame[6]).powi(2) + f64::from(frame[7]).powi(2);
            }
        }
        assert!(energy[2] > 1e-7, "diffuse input must produce height energy");
        assert!(energy[0] < energy[2] * 0.2, "coherent={:?}", energy);
        assert!(energy[1] < energy[2] * 0.2, "antiphase={:?}", energy);
    }

    #[test]
    fn drain_matches_zero_input_continuation_and_reset_rearms_processing() {
        let frames = 733;
        let mut input = vec![0.0; frames * 6];
        input[(frames - 1) * 6] = 0.5;
        input[(frames - 1) * 6 + 4] = -0.25;
        let mut draining = processor("5.1", "5.1.4");
        let mut reference = processor("5.1", "5.1.4");
        let mut output = vec![0.0; frames * 10];
        let context = ProcessContext::new(48_000, frames);
        draining.process(&input, &mut output, &context).unwrap();
        reference.process(&input, &mut output, &context).unwrap();
        let mut tail = Vec::new();
        let mut block = vec![0.0; 73 * 10];
        loop {
            let result = draining.drain(&mut block, &context).unwrap();
            tail.extend_from_slice(&block[..result.frames * 10]);
            if result.complete {
                break;
            }
        }
        let tail_frames = tail.len() / 10;
        assert!(tail_frames >= draining.latency_samples());
        let silence = vec![0.0; tail_frames * 6];
        let mut expected = vec![0.0; tail.len()];
        reference
            .process(
                &silence,
                &mut expected,
                &ProcessContext::new(48_000, tail_frames),
            )
            .unwrap();
        for (index, (actual, expected)) in tail.iter().zip(&expected).enumerate() {
            assert!((actual - expected).abs() < 1e-6, "tail sample {index}");
        }
        assert!(draining.process(&input, &mut output, &context).is_err());
        draining.reset();
        assert_eq!(
            draining.process(&input, &mut output, &context).unwrap(),
            frames
        );
    }

    #[test]
    fn plugin_accepts_explicit_surround_layout_and_rejects_lossy_output() {
        let mut params = UpmixerPluginParams::default();
        params.core.fft_size = 512;
        params.core.speaker_config = "7.1.4".into();
        params.multichannel.input_layout = Some(layout("5.0"));
        let mut plugin = UpmixerPlugin::try_from_params(params.clone()).unwrap();
        plugin.initialize(48_000.0).unwrap();
        assert_eq!(plugin.input_channels(), 5);
        assert_eq!(plugin.output_channels(), 12);
        assert_eq!(plugin.latency_samples(), 512);
        let frames = 600;
        let mut input = vec![0.0; frames * 5];
        input[2] = 0.5;
        let mut output = vec![0.0; frames * 12];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        assert_eq!(output[512 * 12 + 2], 0.5);
        assert_eq!(output[512 * 12 + 3], 0.0);

        params.multichannel.input_layout = Some(layout("7.1"));
        params.core.speaker_config = "5.1.4".into();
        assert!(UpmixerPlugin::try_from_params(params).is_err());
    }

    #[test]
    fn existing_stereo_json_defaults_to_the_original_input_contract() {
        let params: UpmixerPluginParams =
            serde_json::from_str(r#"{"speaker_config":"5.1"}"#).unwrap();
        let plugin = UpmixerPlugin::try_from_params(params).unwrap();
        assert_eq!(plugin.input_channels(), 2);
        assert_eq!(plugin.output_channels(), 6);
    }

    #[test]
    fn plugin_height_and_bypass_controls_keep_bed_and_latency() {
        let mut params = UpmixerPluginParams::default();
        params.core.fft_size = 512;
        params.core.speaker_config = "5.1.4".into();
        params.multichannel.input_layout = Some(layout("5.0"));
        params.multichannel.bass_mode = SurroundBassMode::Derived;
        let mut plugin = UpmixerPlugin::try_from_params(params).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let frames = 4096;
        let mut input = vec![0.0; frames * 5];
        let mut random = 42_u32;
        for sample in &mut input {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *sample = (random as f32 / u32::MAX as f32 - 0.5) * 0.2;
        }
        let mut output = vec![0.0; frames * 10];
        let context = ProcessContext::new(48_000, frames);
        plugin.process(&input, &mut output, &context).unwrap();
        let latency = plugin.latency_samples();
        plugin
            .set_parameter(ParameterId::from("height_gain"), ParameterValue::Float(0.0))
            .unwrap();
        plugin.process(&input, &mut output, &context).unwrap();
        for frame in output.chunks_exact(10).skip(frames - 128) {
            assert!(frame[6..].iter().all(|sample| sample.abs() < 1e-8));
        }
        plugin
            .set_parameter(ParameterId::from("height_gain"), ParameterValue::Float(0.7))
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from("bypass_heights"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        plugin.process(&input, &mut output, &context).unwrap();
        let tail = &output[(frames - 128) * 10..];
        assert!(
            tail.chunks_exact(10)
                .all(|frame| frame[6..].iter().all(|sample| sample.abs() < 1e-8))
        );
        assert!(tail.chunks_exact(10).any(|frame| frame[3].abs() > 1e-6));
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("height_gain")),
            Some(ParameterValue::Float(0.7))
        );
        plugin
            .set_parameter(
                ParameterId::from("bypass_heights"),
                ParameterValue::Bool(false),
            )
            .unwrap();
        plugin.process(&input, &mut output, &context).unwrap();
        assert!(
            output[(frames - 128) * 10..]
                .chunks_exact(10)
                .any(|frame| frame[6..].iter().any(|sample| sample.abs() > 1e-8))
        );
        plugin
            .set_parameter(
                ParameterId::from("bypass_surround"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        plugin.process(&input, &mut output, &context).unwrap();
        assert_eq!(plugin.latency_samples(), latency);
        for frame in frames - 128..frames {
            for (source, target) in [(0, 0), (1, 1), (2, 2), (3, 4), (4, 5)] {
                assert_eq!(
                    output[frame * 10 + target].to_bits(),
                    input[(frame - latency) * 5 + source].to_bits()
                );
            }
            assert!(output[frame * 10 + 3].abs() < 1e-8);
        }
    }
}
