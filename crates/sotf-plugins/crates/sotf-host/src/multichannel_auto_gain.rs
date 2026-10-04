// ============================================================================
// MultichannelAutoGain
// ============================================================================
//
// Wraps a stereo `AutoGain` with the fold-down meter buffer used by plugins
// that emit multichannel output from stereo input (Upmixer, AAE, etc.).
//
// The folded meter buffer is a stereo (L, R) sum of the multichannel output:
// each non-LFE speaker contributes to L when azimuth > +10°, to R when
// azimuth < -10°, and is split with -3 dB to both when |azimuth| <= 10°
// (front/back center). The stereo gain produced by the inner `AutoGain` is
// then applied uniformly to every output channel.
//
// When the output has 2 channels, the meter buffer is the output itself.
// For mono (1 channel), the single channel is duplicated to both meter
// channels. For 0 channels (degenerate), the call is a no-op.

use crate::auto_gain::{AutoGain, AutoGainData, AutoGainParams};
use crate::speaker_config::SpeakerConfig;

/// Stereo `AutoGain` with multichannel output support via stereo fold-down.
#[derive(Debug)]
pub struct MultichannelAutoGain {
    inner: AutoGain,
    meter_buf: Vec<f32>,
    measurement_interval: usize,
    measurement_phase: usize,
}

impl MultichannelAutoGain {
    /// Create with given sample rate and parameters. The inner `AutoGain` is
    /// always 2-channel (stereo) — we fold multichannel output down to stereo
    /// for measurement.
    /// Maximum number of frames the meter buffer is pre-sized for. This covers
    /// all current SOTF host block sizes (128–8192) without reallocation.
    const MAX_METER_FRAMES: usize = 8192;

    pub fn new(sample_rate: impl Into<f64>, params: AutoGainParams) -> Result<Self, String> {
        let sample_rate = sample_rate.into();
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("auto-gain sample rate must be finite and positive".into());
        }
        Ok(Self {
            inner: AutoGain::new(2, sample_rate, params)?,
            meter_buf: vec![0.0; Self::MAX_METER_FRAMES * 2],
            measurement_interval: (sample_rate / 10.0).floor().max(1.0) as usize,
            measurement_phase: 0,
        })
    }

    pub fn set_sample_rate(&mut self, sr: impl Into<f64>) -> Result<(), String> {
        let sr = sr.into();
        if !sr.is_finite() || sr <= 0.0 {
            return Err("auto-gain sample rate must be finite and positive".into());
        }
        self.inner.set_sample_rate(sr)?;
        self.measurement_interval = (sr / 10.0).floor().max(1.0) as usize;
        self.measurement_phase = 0;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.measurement_phase = 0;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.inner.set_enabled(enabled);
    }
    pub fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }
    pub fn set_max_gain_db(&mut self, max_db: f32) {
        self.inner.set_max_gain_db(max_db);
    }
    pub fn set_smoothing_ms(&mut self, ms: f32) {
        self.inner.set_smoothing_ms(ms);
    }

    /// Measure the stereo input loudness. `input` is interleaved 2-ch.
    pub fn measure_input(&mut self, input: &[f32]) -> Result<(), String> {
        self.inner.measure_input(input)
    }

    /// Fold multichannel output to stereo, measure output loudness, and apply
    /// the resulting gain uniformly to all output channels.
    ///
    /// `output` is interleaved with `out_ch` channels per frame. `out_ch` may
    /// be less than `speaker_config.total_channels` (e.g. upmixer's binaural
    /// preview emits 2-ch even when configured for 5.1+); only speakers with
    /// `sp.channel < out_ch` contribute to the meter.
    pub fn measure_and_apply(
        &mut self,
        output: &mut [f32],
        num_frames: usize,
        out_ch: usize,
        speaker_config: &SpeakerConfig,
    ) -> Result<(), String> {
        if !self.inner.is_enabled() || num_frames == 0 || out_ch == 0 {
            return Ok(());
        }
        let expected_len = num_frames
            .checked_mul(out_ch)
            .ok_or_else(|| "output buffer size overflow".to_string())?;
        if output.len() != expected_len {
            return Err(format!(
                "output buffer length mismatch: expected {expected_len} samples for \
                 {num_frames} frames x {out_ch} channels, got {}",
                output.len()
            ));
        }

        // Preserve the legacy whole-callback target refresh, while bounding
        // scratch storage even for offline callbacks larger than 8192 frames.
        let mut position = 0;
        while position < num_frames {
            let frames = (num_frames - position).min(Self::MAX_METER_FRAMES);
            self.fill_meter_buffer(
                &output[position * out_ch..(position + frames) * out_ch],
                frames,
                out_ch,
                speaker_config,
            );
            let measured = &self.meter_buf[..frames * 2];
            self.inner.ingest_output(measured)?;
            position += frames;
        }
        self.inner.refresh_output_measurement();
        self.apply_gains(output, out_ch);
        Ok(())
    }

    /// Measure aligned input/output pairs and apply gain on a fixed sample clock.
    ///
    /// `input` is stereo and must already include the renderer's transport delay.
    /// Output folding follows [`Self::measure_and_apply`]. Measurements refresh
    /// every `max(1, sample_rate / 10)` enabled frames; a new target affects only
    /// the following frame. Callback boundaries do not refresh measurements.
    /// Disabled and zero-frame calls are no-ops, retaining the active-frame phase
    /// and existing gain history. Reset and sample-rate changes restart the phase.
    ///
    /// Scratch storage is bounded independently of the caller's block size. Use
    /// this paired API throughout a metering epoch to obtain causal timing; the
    /// older separate measurement methods retain their original block timing.
    /// Published sample peaks cover the complete paired measurement interval,
    /// including all callback, ring, and bounded scratch fragments.
    ///
    /// # Errors
    /// Returns an error for overflowing or mismatched active buffer dimensions,
    /// before modifying output or state, or if loudness ingestion fails.
    pub fn measure_aligned_and_apply(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_frames: usize,
        out_ch: usize,
        speaker_config: &SpeakerConfig,
    ) -> Result<(), String> {
        if !self.inner.is_enabled() || num_frames == 0 || out_ch == 0 {
            return Ok(());
        }
        let input_len = num_frames
            .checked_mul(2)
            .ok_or_else(|| "input buffer size overflow".to_string())?;
        let output_len = num_frames
            .checked_mul(out_ch)
            .ok_or_else(|| "output buffer size overflow".to_string())?;
        if input.len() != input_len || output.len() != output_len {
            return Err(format!(
                "aligned AutoGain buffer mismatch: expected {input_len} input and \
                 {output_len} output samples, got {} and {}",
                input.len(),
                output.len()
            ));
        }

        let mut position = 0;
        while position < num_frames {
            let frames = (num_frames - position)
                .min(self.measurement_interval - self.measurement_phase)
                .min(Self::MAX_METER_FRAMES);
            let end = position + frames;
            let output_span = &mut output[position * out_ch..end * out_ch];
            let input_span = &input[position * 2..end * 2];
            self.inner.ingest_input(input_span)?;
            self.fill_meter_buffer(output_span, frames, out_ch, speaker_config);
            let measured = &self.meter_buf[..frames * 2];
            self.inner.ingest_output(measured)?;
            // Ingesting cannot change gain until the explicit boundary refresh.
            self.apply_gains(output_span, out_ch);
            self.measurement_phase += frames;
            if self.measurement_phase == self.measurement_interval {
                self.inner.refresh_input_measurement();
                self.inner.refresh_output_measurement();
                self.measurement_phase = 0;
            }
            position = end;
        }
        Ok(())
    }

    fn apply_gains(&mut self, output: &mut [f32], out_ch: usize) {
        for frame in output.chunks_exact_mut(out_ch) {
            let gain = self.inner.next_gain_linear();
            for sample in frame {
                *sample *= gain;
            }
        }
    }

    /// Snapshot of the current AutoGain state (for `get_data()` UI exposure).
    pub fn data(&self) -> AutoGainData {
        self.inner.get_data()
    }

    fn fill_meter_buffer(
        &mut self,
        output: &[f32],
        num_frames: usize,
        out_ch: usize,
        speaker_config: &SpeakerConfig,
    ) {
        let needed = num_frames * 2;
        debug_assert!(num_frames <= Self::MAX_METER_FRAMES);
        let buf = &mut self.meter_buf[..needed];

        // Stereo or mono passthrough: copy directly.
        if out_ch <= 2 {
            for frame in 0..num_frames {
                let out_base = frame * out_ch;
                let meter_base = frame * 2;
                buf[meter_base] = output[out_base];
                buf[meter_base + 1] = if out_ch == 2 {
                    output[out_base + 1]
                } else {
                    output[out_base]
                };
            }
            return;
        }

        buf.fill(0.0);

        for frame in 0..num_frames {
            let out_base = frame * out_ch;
            let meter_base = frame * 2;
            for sp in speaker_config.speakers {
                if sp.is_lfe || sp.channel >= out_ch {
                    continue;
                }
                let sample = output[out_base + sp.channel];
                if sp.azimuth > 10.0 {
                    buf[meter_base] += sample;
                } else if sp.azimuth < -10.0 {
                    buf[meter_base + 1] += sample;
                } else {
                    let split = sample * std::f32::consts::FRAC_1_SQRT_2;
                    buf[meter_base] += split;
                    buf[meter_base + 1] += split;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_gain::AutoGainLoudnessType;
    use crate::speaker_config::get_speaker_config;

    fn enabled_params() -> AutoGainParams {
        AutoGainParams {
            enabled: true,
            loudness_type: AutoGainLoudnessType::Momentary,
            max_gain_db: 12.0,
            smoothing_ms: 50.0,
        }
    }

    #[test]
    fn disabled_is_noop() {
        let mut mag = MultichannelAutoGain::new(48000, AutoGainParams::default()).unwrap();
        let cfg = get_speaker_config("5.1").unwrap();
        let mut output = vec![0.5_f32; 1024 * cfg.total_channels];
        let snapshot = output.clone();
        mag.measure_and_apply(&mut output, 1024, cfg.total_channels, cfg)
            .unwrap();
        assert_eq!(
            output, snapshot,
            "disabled MultichannelAutoGain must not modify output"
        );
    }

    #[test]
    fn passes_through_stereo() {
        let mut mag = MultichannelAutoGain::new(48000, enabled_params()).unwrap();
        let cfg = get_speaker_config("2.0").unwrap();
        let frames = 1024;
        let input: Vec<f32> = (0..frames * 2)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        let mut output = input.clone();
        mag.measure_input(&input).unwrap();
        mag.measure_and_apply(&mut output, frames, cfg.total_channels, cfg)
            .unwrap();
        assert!(output.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn folds_5_1_output_for_metering() {
        let mut mag = MultichannelAutoGain::new(48000, enabled_params()).unwrap();
        let cfg = get_speaker_config("5.1").unwrap();
        let frames = 4096;
        // Build a 5.1 buffer with energy on FL/FR only, varying over time.
        let mut output = vec![0.0_f32; frames * cfg.total_channels];
        for frame in 0..frames {
            let s = (frame as f32 * 0.01).sin() * 0.4;
            // Find FL (azimuth +30) and FR (azimuth -30) channels.
            for sp in cfg.speakers {
                if sp.is_lfe {
                    continue;
                }
                if (sp.azimuth - 30.0).abs() < 1.0 {
                    output[frame * cfg.total_channels + sp.channel] = s;
                } else if (sp.azimuth + 30.0).abs() < 1.0 {
                    output[frame * cfg.total_channels + sp.channel] = -s;
                }
            }
        }
        // Stereo input that matches FL/FR content so AutoGain converges to ~0 dB.
        let input: Vec<f32> = (0..frames * 2)
            .flat_map(|f| {
                let s = (f as f32 * 0.01).sin() * 0.4;
                [s, -s]
            })
            .collect();
        mag.measure_input(&input).unwrap();
        mag.measure_and_apply(&mut output, frames, cfg.total_channels, cfg)
            .unwrap();
        assert!(output.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn out_ch_zero_does_not_panic() {
        // Defensive check: out_ch == 0 is unreachable from current callers,
        // but the helper is a public sotf-host API. Must not panic.
        let mut mag = MultichannelAutoGain::new(48000, enabled_params()).unwrap();
        let cfg = get_speaker_config("5.1").unwrap();
        let mut output: Vec<f32> = Vec::new();
        let res = mag.measure_and_apply(&mut output, 8, 0, cfg);
        assert!(
            res.is_ok(),
            "out_ch == 0 should be a graceful no-op, got {:?}",
            res
        );
    }

    #[test]
    fn out_ch_one_mono_passthrough() {
        // out_ch == 1: the single channel should map to both L and R of meter.
        let mut mag = MultichannelAutoGain::new(48000, enabled_params()).unwrap();
        let cfg = get_speaker_config("1.0").unwrap();
        let frames = 1024;
        let input: Vec<f32> = (0..frames * 2)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        let mut output: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        mag.measure_input(&input).unwrap();
        let res = mag.measure_and_apply(&mut output, frames, 1, cfg);
        assert!(res.is_ok());
        assert!(output.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn binaural_preview_uses_actual_out_ch() {
        // 5.1 speaker_config but out_ch=2 (upmixer binaural_preview): the
        // helper must treat it as stereo passthrough, ignoring channels 2-5.
        let mut mag = MultichannelAutoGain::new(48000, enabled_params()).unwrap();
        let cfg = get_speaker_config("5.1").unwrap();
        let frames = 1024;
        let input: Vec<f32> = (0..frames * 2)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        let mut output = input.clone();
        mag.measure_input(&input).unwrap();
        mag.measure_and_apply(&mut output, frames, 2, cfg).unwrap();
        assert!(output.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn invalid_output_length_returns_error() {
        let mut mag = MultichannelAutoGain::new(48000, enabled_params()).unwrap();
        let cfg = get_speaker_config("5.1").unwrap();
        let mut output = vec![0.0_f32; 15];

        let err = mag.measure_and_apply(&mut output, 4, cfg.total_channels, cfg);
        assert!(err.is_err(), "mismatched buffer length should be rejected");
    }
}
