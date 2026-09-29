//! Bounded, allocation-free zero-input continuation at end of stream.

use crate::UpmixerPlugin;
use sotf_host::plugin::{PluginDrainResult, PluginResult, ProcessContext};

pub(super) struct UpmixerDrain {
    silence: Vec<f32>,
    pub(super) output: Vec<f32>,
    pub(super) has_input: bool,
    pub(super) input_phase: usize,
    pub(super) remaining: Option<usize>,
    cached_frames: usize,
    read_frame: usize,
}

impl UpmixerDrain {
    pub(super) fn new(hop: usize, channels: usize) -> Self {
        Self {
            silence: vec![0.0; hop * 2],
            output: vec![0.0; hop * channels],
            has_input: false,
            input_phase: 0,
            remaining: None,
            cached_frames: 0,
            read_frame: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.has_input = false;
        self.input_phase = 0;
        self.remaining = None;
        self.cached_frames = 0;
        self.read_frame = 0;
    }
}

impl UpmixerPlugin {
    pub(super) fn drain_stream_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.drain.has_input
            || self.drain.remaining == Some(0)
            || self.params.bypass_all_processing
        {
            return std::num::NonZeroU64::new(1);
        }
        let hop = self.core.hop_size;
        if hop == 0 {
            return None;
        }
        let remaining = self
            .drain
            .remaining
            .unwrap_or_else(|| self.drain_tail_frames());
        let unread = self
            .drain
            .cached_frames
            .checked_sub(self.drain.read_frame)?;
        // A partially served cache takes its own call, even when the caller
        // now supplies a full hop. Later calls each refill and emit one hop.
        let calls = if unread > 0 {
            remaining
                .checked_sub(unread)?
                .div_ceil(hop)
                .checked_add(1)?
        } else {
            remaining.div_ceil(hop)
        };
        std::num::NonZeroU64::new(u64::try_from(calls.max(1)).ok()?)
    }

    fn drain_tail_frames(&self) -> usize {
        let n = self.core.fft_size;
        let hop = self.core.hop_size;
        let latency = self.output_latency_frames();
        let padding = (hop - self.drain.input_phase % hop) % hop;
        // Analysis starts at -H, but the final input-containing window still
        // starts at floor((T-1)/H)*H and ends N frames later. Include startup N.
        let mut finite = latency + n - hop + padding;
        if self.params.enable_hr_direct || self.hr_state.hr_direct_envelope > 0.0 {
            let hr_n = self.fft.hr_fft_size;
            let hr_hop = hr_n / 2;
            let delay = self.hr_buffers.hr_delay_buffer.len() / 2;
            let hr_padding = (hr_hop - (self.drain.input_phase + delay) % hr_hop) % hr_hop;
            finite = finite.max(latency + delay + hr_n - hr_hop + hr_padding);
        }

        // Crossover and decorrelation coefficients are applied to finite FFT
        // blocks; they are not running IIR filters. The optional oscillator's
        // recursive amplitude/release state is the audio-generating exception.
        if self.subharmonic.enable_subharmonic_synth
            || self.subharmonic.subharmonic_envelope > 0.0
            || self.subharmonic.subharmonic_amp_envelope > 0.0
        {
            let alpha = f64::from(self.subharmonic.cached_subharmonic_release_coeff);
            let tau_samples = if alpha > 0.0 && alpha < 1.0 {
                -1.0 / (-alpha).ln_1p()
            } else if alpha >= 1.0 {
                0.0
            } else {
                // If f32 coefficient quantization collapses the decay to zero
                // at an extreme rate, retain a bounded configured-time policy.
                f64::from(self.subharmonic.subharmonic_release_ms)
                    * f64::from(self.core.sample_rate)
                    / 1000.0
            };
            // Fourteen effective time constants plus one synthesis window.
            // This is a render cap, not a claim of exact recursive silence.
            finite = finite
                .saturating_add((14.0 * tau_samples).ceil() as usize)
                .saturating_add(n);
        }
        finite
    }

    pub(super) fn drain_stream(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.drain.has_input || self.drain.remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if self.params.bypass_all_processing {
            self.drain.remaining = Some(0);
            return Ok(PluginDrainResult::COMPLETE);
        }
        let channels = self.effective_output_channels();
        if output.is_empty() || !output.len().is_multiple_of(channels) {
            return Err("Upmixer drain requires complete output frames".into());
        }
        // Freeze the bound only after validating capacity; subsequent errors
        // cannot consume input or change the lifecycle of a pending stream.
        let remaining = if let Some(remaining) = self.drain.remaining {
            remaining
        } else {
            let remaining = self.drain_tail_frames();
            self.drain.remaining = Some(remaining);
            remaining
        };

        if self.drain.read_frame == self.drain.cached_frames {
            let frames = remaining.min(self.core.hop_size);
            let silence = std::mem::take(&mut self.drain.silence);
            let mut cache = std::mem::take(&mut self.drain.output);
            let mut zero_context = *context;
            zero_context.num_frames = frames;
            let result = self.process_stream(
                &silence[..frames * 2],
                &mut cache[..frames * channels],
                &zero_context,
            );
            self.drain.silence = silence;
            self.drain.output = cache;
            let written = result?;
            debug_assert_eq!(written, frames);
            self.drain.read_frame = 0;
            self.drain.cached_frames = written;
        }
        let frames =
            (self.drain.cached_frames - self.drain.read_frame).min(output.len() / channels);
        let start = self.drain.read_frame * channels;
        output[..frames * channels]
            .copy_from_slice(&self.drain.output[start..start + frames * channels]);
        self.drain.read_frame += frames;
        let remaining = remaining - frames;
        self.drain.remaining = Some(remaining);
        if remaining == 0 {
            self.subharmonic.subharmonic_envelope = 0.0;
            self.subharmonic.subharmonic_amp_envelope = 0.0;
            self.subharmonic.subharmonic_phase = 0.0;
        }
        Ok(PluginDrainResult {
            frames,
            complete: remaining == 0,
        })
    }
}
