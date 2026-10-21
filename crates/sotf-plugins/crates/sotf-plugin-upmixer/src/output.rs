// ============================================================================
// Output Processing
// ============================================================================

use super::UpmixerPlugin;

// Physical release anchor: retain the former 0.02 release step over the
// representative 1024-frame / 48 kHz callback, independent of actual callbacks.
pub(super) fn safety_clock_coefficients(sample_rate: f64) -> (f64, f64) {
    let control_retention = (-1.0 / (0.05 * sample_rate)).exp();
    let release_alpha = -(0.98_f64.ln() * 48_000.0 / (1024.0 * sample_rate)).exp_m1();
    (control_retention, release_alpha)
}

impl UpmixerPlugin {
    pub(super) fn flush_safety_clock(&mut self) {
        let frames = std::mem::take(&mut self.param_smoothers.safety_pending_frames);
        if frames == 0 {
            return;
        }
        let value = self.param_smoothers.safety_cap_db_smoother.next_n(frames);
        // Preserve the per-analysis-stage cache threshold and conversion.
        if (value - self.safety.safety_cap_db).abs() > 0.001 {
            self.safety.safety_cap_db = value;
            self.update_safety_cap_cache();
        }
    }

    #[inline]
    pub(super) fn advance_final_safety_control(&mut self) {
        let target = f64::from(self.param_smoothers.safety_cap_db_smoother.target());
        let current = self.safety.final_cap_db;
        // The existing 50 ms one-pole and near-target snap, evaluated on each
        // generated output frame. f64 state avoids high-rate f32 convergence stalls.
        let value = if (current - target).abs() < 1e-5 {
            target
        } else {
            target + self.safety.final_cap_retention * (current - target)
        };
        self.safety.final_cap_db = value;
        if value != self.safety.final_cap_cached_db
            && ((value - self.safety.final_cap_cached_db).abs() > 0.001 || value == target)
        {
            self.safety.final_cap_cached_db = value;
            self.safety.final_cap_linear = if value >= 0.0 {
                math_audio_dsp::fast_math::fast_pow10((value / 20.0) as f32)
            } else {
                1.0
            };
        }
    }

    /// Cap each emitted frame after AutoGain, overlap-add and HR/binaural mixing.
    ///
    /// All channels share instantaneous attack and a physical-time release.
    /// No future frame can attenuate the current frame. The earlier per-FFT
    /// bed-channel stage remains separate and keeps its existing coefficients.
    #[inline]
    pub(super) fn apply_final_safety_cap(&mut self, output: &mut [f32], num_frames: usize) {
        let channels = self.effective_output_channels();
        if channels == 0 {
            return;
        }
        let count = (num_frames * channels).min(output.len());
        for frame in output[..count].chunks_exact_mut(channels) {
            self.advance_final_safety_control();
            if self.safety.final_cap_db < 0.0 {
                continue;
            }
            let peak = frame.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
            let cap = self.safety.final_cap_linear;
            let target = if peak.is_finite() && peak > cap && peak > 0.0 {
                f64::from(cap) / f64::from(peak)
            } else {
                1.0
            };
            let previous = self.safety.final_safety_scale;
            let gain = if target < previous {
                target
            } else {
                previous + self.safety.final_release_alpha * (target - previous)
            }
            .clamp(0.0, 1.0);
            self.safety.final_safety_scale = gain;
            if gain < 1.0 {
                for sample in frame {
                    *sample = (f64::from(*sample) * gain) as f32;
                }
            }
        }
    }

    /// Phase 5: Extract real parts from time domain and apply final scaling
    #[inline]
    pub(super) fn extract_output_and_scale(&mut self, output: &mut [f32], combined_scale: f32) {
        // ... (skipping some comments)
        let taper_len = self.main_buffers.edge_taper_table.len();
        for ch in 0..self.core.num_output_channels {
            // Synthesis window for WOLA. Frequency-domain routing can change phase
            // and magnitude enough that the analysis window alone no longer
            // guarantees tapered IFFT block edges.
            sotf_host::simd::window_mul_simd_inplace(
                &mut self.main_buffers.time_out_channels[ch],
                &self.main_buffers.window,
            );

            let speaker = &self.core.speaker_config.speakers[ch];
            if speaker.elevation > 10.0 {
                let taper_end = taper_len.min(self.core.fft_size / 2);
                for i in 0..taper_end {
                    let fade = self.main_buffers.edge_taper_table[i];
                    self.main_buffers.time_out_channels[ch][i] *= fade;
                    self.main_buffers.time_out_channels[ch][self.core.fft_size - 1 - i] *= fade;
                }
            }
        }

        // Safety cap: optionally reduce overall gain so that the block peak does not
        // exceed safety_cap_db (in dB) above unit amplitude.
        // Exclude height channels from peak detection: the height_band_gains spectral
        // mask (values down to 0.02) creates time-domain ringing after IFFT, producing
        // erratic peaks that would modulate the safety cap and cause scratchiness in
        // the bed channels (L/R/C/surround).
        let mut target_safety_scale = 1.0_f32;
        if self.safety.safety_cap_db >= 0.0 {
            let mut max_abs = 0.0_f32;
            for ch in 0..self.core.num_output_channels {
                if self.panning.cached_is_height[ch] {
                    continue;
                }
                let ch_max =
                    sotf_host::simd::find_max_abs_simd(&self.main_buffers.time_out_channels[ch]);
                if ch_max > max_abs {
                    max_abs = ch_max;
                }
            }

            if max_abs > 0.0 {
                let cap_linear = self.safety.safety_cap_linear;
                let effective_peak = max_abs * combined_scale;
                // Guard against division by zero or negative values (defensive)
                if effective_peak > 0.0 && effective_peak.is_finite() && effective_peak > cap_linear
                {
                    target_safety_scale = cap_linear / effective_peak;
                }
            }
        }

        // Smooth safety scale changes to avoid clicks and pumping artifacts.
        // Use asymmetric smoothing: fast attack (protect against clipping) but slow release.
        let attack_coeff = 0.5_f32; // Fast attack to catch peaks
        let release_coeff = 0.05_f32; // Slow release to avoid pumping
        let smoothing = if target_safety_scale < self.safety.prev_safety_scale {
            attack_coeff // Going down (reducing gain) - fast
        } else {
            release_coeff // Going up (restoring gain) - slow
        };

        // Calculate the target scale for the end of this block
        let end_scale = self.safety.prev_safety_scale
            + smoothing * (target_safety_scale - self.safety.prev_safety_scale);

        let start_scale = self.safety.prev_safety_scale;
        self.safety.prev_safety_scale = end_scale;

        for i in 0..self.core.fft_size {
            let idx = i * self.core.num_output_channels;
            let t = i as f32 / self.core.fft_size as f32;
            let block_safety_scale = start_scale + t * (end_scale - start_scale);
            let final_scale = combined_scale * block_safety_scale;
            for ch in 0..self.core.num_output_channels {
                output[idx + ch] = self.main_buffers.time_out_channels[ch][i] * final_scale;
            }
        }

        // Note: FTZ/DAZ CPU flags are set by the processing thread (enable_ftz_daz()),
        // so denormals are flushed to zero by the CPU automatically. No manual flush needed.
    }
}

#[cfg(test)]
mod audit_safety_tests {
    use crate::UpmixerPlugin;
    use sotf_host::{Plugin, ProcessContext};

    fn processor(rate: f64) -> UpmixerPlugin {
        let params = serde_json::from_value(serde_json::json!({"speaker_config":"2.0"})).unwrap();
        let mut p = UpmixerPlugin::from_params(params);
        p.initialize(rate).unwrap();
        p
    }

    #[test]
    fn final_cap_does_not_use_future_frame_peaks() {
        let mut whole = processor(48_000.0);
        let mut prefix_only = processor(48_000.0);
        let mut samples = [0.2, 0.1, 4.0, -1.0];
        let mut prefix = [0.2, 0.1];
        whole.apply_final_safety_cap(&mut samples, 2);
        prefix_only.apply_final_safety_cap(&mut prefix, 1);
        println!(
            "safety_prefix whole={:?} isolated={prefix:?}",
            &samples[..2]
        );
        assert_eq!(&samples[..2], &prefix);
    }

    #[test]
    fn final_cap_release_has_physical_time_and_linked_channel_ratios() {
        for rate in [44_100, 48_000, 192_000, 384_000] {
            let mut p = processor(f64::from(rate));
            let cap = p.safety.safety_cap_linear;
            let mut peak = [4.0, -1.0];
            p.apply_final_safety_cap(&mut peak, 1);
            assert!(peak[0] <= cap);
            assert_eq!(peak[1] / peak[0], -0.25);
            let frames = rate as usize / 10;
            let mut quiet = vec![0.1; frames * 2];
            p.apply_final_safety_cap(&mut quiet, frames);
            let expected = 1.0 - (1.0 - f64::from(cap) / 4.0) * 0.98_f64.powf(4800.0 / 1024.0);
            let error = (p.safety.final_safety_scale - expected).abs();
            println!(
                "safety_release rate={rate} scale={} expected={expected} error={error}",
                p.safety.final_safety_scale
            );
            assert!(error < 1e-11);
        }
    }

    #[test]
    fn source_and_emitted_cap_controls_have_independent_interrupted_clocks() {
        for rate in [44_100.0, 48_000.0, 192_000.0] {
            let mut p = processor(rate);
            p.set_param_value(34, 0.0);
            p.advance_analysis_clock(37);
            p.set_param_value(1, 0.2);
            p.set_param_value(6, 180.0);
            assert_eq!(p.param_smoothers.safety_cap_db_smoother.current(), 3.0);
            assert_eq!(p.param_smoothers.safety_pending_frames, 37);
            p.set_param_value(34, 1.5);
            let coefficient = f64::from((-1.0 / (rate * 0.05)).exp() as f32);
            let first = 3.0 * coefficient.powi(37);
            assert!(
                (f64::from(p.param_smoothers.safety_cap_db_smoother.current()) - first).abs()
                    < 2e-6
            );
            assert_eq!(p.safety.final_cap_db, 3.0);
            let mut quiet = [0.0; 74];
            p.apply_final_safety_cap(&mut quiet, 37);
            let output_first = 1.5 + 1.5 * (-37.0 / (rate * 0.05)).exp();
            assert!((p.safety.final_cap_db - output_first).abs() < 1e-11);
            p.advance_analysis_clock(19);
            p.set_param_value(34, 1.5); // Snapshots cannot observe the pending source control.
            assert_eq!(p.param_smoothers.safety_pending_frames, 19);
            assert!((p.safety.final_cap_db - output_first).abs() < 1e-11);
            p.process(&[], &mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            assert!(
                p.process(&[], &mut [], &ProcessContext::new(rate, 1))
                    .is_err()
            );
            assert_eq!(p.param_smoothers.safety_pending_frames, 19);
            assert!((p.safety.final_cap_db - output_first).abs() < 1e-11);
            p.advance_analysis_clock(200);
            let expected = 1.5 + coefficient.powi(219) * (first - 1.5);
            let rounding = 260.0 * f64::from(f32::EPSILON);
            let bound = rounding / (1.0 - rounding) * 4.0;
            assert!(
                (f64::from(p.param_smoothers.safety_cap_db_smoother.current()) - expected).abs()
                    < bound
            );
            assert_eq!(p.param_smoothers.safety_pending_frames, 0);
        }
    }

    #[test]
    fn safety_reset_and_reinitialization_preserve_control_time_and_reset_gain() {
        let mut p = processor(48_000.0);
        p.set_param_value(34, 0.0);
        p.advance_analysis_clock(37);
        let mut peak = [4.0, -1.0];
        p.apply_final_safety_cap(&mut peak, 1);
        let output_control = p.safety.final_cap_db;
        assert!(p.safety.final_safety_scale < 1.0);
        assert!(p.initialize(f64::NAN).is_err());
        assert_eq!(p.param_smoothers.safety_pending_frames, 37);
        p.reset();
        let coefficient = f64::from((-1.0_f64 / (48_000.0 * 0.05)).exp() as f32);
        assert!(
            (f64::from(p.param_smoothers.safety_cap_db_smoother.current())
                - 3.0 * coefficient.powi(37))
            .abs()
                < 2e-6
        );
        assert_eq!(p.param_smoothers.safety_pending_frames, 0);
        assert_eq!(p.safety.final_safety_scale, 1.0);
        assert_eq!(p.safety.final_cap_db, output_control);
        p.advance_analysis_clock(19);
        p.initialize(96_000.0).unwrap();
        assert!(
            (f64::from(p.param_smoothers.safety_cap_db_smoother.current())
                - 3.0 * coefficient.powi(56))
            .abs()
                < 2e-6
        );
        assert_eq!(p.safety.final_cap_db, output_control);
        let mut quiet = [0.0; 38];
        p.apply_final_safety_cap(&mut quiet, 19);
        assert!(
            (p.safety.final_cap_db - output_control * (-19.0_f64 / (96_000.0 * 0.05)).exp()).abs()
                < 1e-11
        );
    }

    #[test]
    fn raw_bypass_is_uncapped_but_advances_both_control_clocks() {
        let mut p = processor(48_000.0);
        p.set_parameter(
            "bypass_all_processing".into(),
            sotf_host::ParameterValue::Bool(true),
        )
        .unwrap();
        p.set_param_value(34, 0.0);
        let input = vec![4.0; 274];
        let mut output = vec![0.0; 274];
        p.process(&input, &mut output, &ProcessContext::new(48_000, 137))
            .unwrap();
        assert_eq!(input, output);
        assert_eq!(p.param_smoothers.safety_pending_frames, 137);
        let expected = 3.0 * (-137.0_f64 / (48_000.0 * 0.05)).exp();
        assert!((p.safety.final_cap_db - expected).abs() < 1e-11);
        p.set_parameter(
            "bypass_all_processing".into(),
            sotf_host::ParameterValue::Bool(false),
        )
        .unwrap();
        assert_eq!(p.param_smoothers.safety_pending_frames, 0);
        assert_eq!(p.safety.final_safety_scale, 1.0);
        assert!((p.safety.final_cap_db - expected).abs() < 1e-11);
    }
}
