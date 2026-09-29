// ============================================================================
// High-Resolution Processing
// ============================================================================

use super::UpmixerPlugin;
use rustfft::num_complex::Complex;

impl UpmixerPlugin {
    /// Above and below the fixed HR FFT size, mix HR frames by accepted-input
    /// source tag. The equal-size route retains its historical drain behavior.
    pub(super) fn uses_hr_source_tags(&self) -> bool {
        let uses_source_tags = self.core.fft_size != self.fft.hr_fft_size;
        #[cfg(test)]
        {
            uses_source_tags && !self.output.hr_use_pre_correction_mixer_for_test
        }
        #[cfg(not(test))]
        {
            uses_source_tags
        }
    }

    /// Number of leading HR frames whose tags refer to negative source time.
    /// The HR input is delayed by D frames, so its first transform has H + D
    /// frames of negative source history, where H is the 50% overlap hop.
    pub(super) fn hr_source_startup_discard_frames(&self) -> usize {
        self.fft.hr_fft_size / 2 + self.hr_buffers.hr_delay_buffer.len() / 2
    }

    /// Schedule one hop's gain using the matching main analysis result. Output
    /// callbacks may read any fraction of this hop without changing its ramp.
    pub(super) fn prepare_hr_output_gains(&mut self) {
        #[cfg(test)]
        let target = {
            let position = self.output.hr_test_gain_sequence_position;
            if let Some(target) = self.output.hr_test_gain_sequence.get(position).copied() {
                self.output.hr_test_gain_sequence_position = position + 1;
                target
            } else {
                self.hr_target_scale()
            }
        };
        #[cfg(not(test))]
        let target = self.hr_target_scale();
        let start = self.hr_state.prev_hr_scale;
        let hop = self.core.hop_size;
        if self.uses_hr_source_tags() {
            for i in 0..hop {
                let position =
                    (self.output.next_add_position + i) & self.output.output_accumulator_mask;
                let source_frame = self.output.main_next_add_source_frame + i as i64;
                let gain = start + (target - start) * ((i + 1) as f32 / hop as f32);
                self.output.hr_mix_gains[position] = if source_frame >= 0
                    && source_frame as u64 >= self.output.hr_gain_resume_source_frame
                {
                    gain
                } else {
                    0.0
                };
            }
        } else {
            for i in 0..hop {
                let position =
                    (self.output.next_add_position + i) & self.output.output_accumulator_mask;
                self.output.hr_mix_gains[position] =
                    start + (target - start) * ((i + 1) as f32 / hop as f32);
            }
        }
        self.hr_state.prev_hr_scale = target;
    }

    fn hr_target_scale(&self) -> f32 {
        let hr_mix = (self.hr_state.hr_transient_env
            * self.gains.hr_sharpen.current()
            * self.hr_state.hr_direct_envelope)
            .clamp(0.0, 1.0);

        if hr_mix < 0.01 || self.gains.gain_front_direct.current() <= 0.0 {
            0.0
        } else {
            // Scale HR path relative to main: sqrt ratio avoids overpowering the
            // main path while still providing transient detail enhancement.
            // Also apply the 1/N overlap-add scaling factor for the HR path itself.
            // Multiply by sqrt(2) to compensate for the -3 dB headroom scale.
            let hr_ola_scale = std::f32::consts::SQRT_2 / self.fft.hr_fft_size as f32;
            (self.core.fft_size as f32 / self.fft.hr_fft_size as f32).sqrt() * hr_mix * hr_ola_scale
        }
    }

    /// Verify that every main frame with a nonzero prepared HR gain has its
    /// matching source frame ready before the main ring is consumed.
    pub(super) fn validate_hr_source_alignment(
        &self,
        main_ring_start: usize,
        source_start: u64,
        num_frames: usize,
    ) -> Result<(), String> {
        let main_mask = self.output.output_accumulator_mask;
        let hr_mask = self.hr_buffers.hr_output_accumulator_mask;
        let mut hr_read = self.hr_buffers.hr_output_read_position;
        let mut hr_fill = self.hr_buffers.hr_output_accumulator_fill;

        for offset in 0..num_frames {
            let source_frame = source_start + offset as u64;
            let gain = self.output.hr_mix_gains[(main_ring_start + offset) & main_mask];

            while hr_fill > 0 {
                let tag = self.hr_buffers.hr_output_source_tags[hr_read];
                if tag == u64::MAX {
                    return Err(format!(
                        "Upmixer HR ready queue has an untagged frame at source {source_frame}"
                    ));
                }
                if tag >= source_frame {
                    break;
                }
                hr_read = (hr_read + 1) & hr_mask;
                hr_fill -= 1;
            }

            let matching_tag = if hr_fill > 0 {
                self.hr_buffers.hr_output_source_tags[hr_read] == source_frame
            } else {
                false
            };
            if gain > 0.0 && !matching_tag {
                let available = if hr_fill > 0 {
                    self.hr_buffers.hr_output_source_tags[hr_read].to_string()
                } else {
                    "none".to_string()
                };
                return Err(format!(
                    "Upmixer HR source frame unavailable: main={source_frame}, main_ring={}, HR head={available}, gain={gain}, fill={hr_fill}, read={hr_read}, input_fill={}, discard={}, accepted={}, pad={}",
                    (main_ring_start + offset) & main_mask,
                    self.hr_buffers.hr_input_buffer_fill,
                    self.hr_buffers.hr_startup_discard_remaining,
                    self.output.accepted_input_frames,
                    self.core.startup_padding_remaining,
                ));
            }
            if matching_tag {
                hr_read = (hr_read + 1) & hr_mask;
                hr_fill -= 1;
            }
        }
        Ok(())
    }

    /// Mix ready HR samples only into the main output frame with the same
    /// accepted-input source-frame tag. Unlike the N>=512 legacy route, a
    /// future HR frame stays queued while the main source clock catches up.
    pub(super) fn mix_hr_output_source_aligned(
        &mut self,
        output: &mut [f32],
        main_ring_start: usize,
        source_start: u64,
        num_frames: usize,
        output_channels: usize,
        binaural: bool,
    ) {
        let main_mask = self.output.output_accumulator_mask;
        let hr_mask = self.hr_buffers.hr_output_accumulator_mask;
        let nch = self.core.num_output_channels;

        for offset in 0..num_frames {
            let source_frame = source_start + offset as u64;
            let main_position = (main_ring_start + offset) & main_mask;
            let gain = self.output.hr_mix_gains[main_position];

            while self.hr_buffers.hr_output_accumulator_fill > 0 {
                let hr_position = self.hr_buffers.hr_output_read_position;
                let tag = self.hr_buffers.hr_output_source_tags[hr_position];
                if tag >= source_frame {
                    break;
                }
                let acc_base = hr_position * nch;
                self.hr_buffers.hr_output_accumulator[acc_base..acc_base + nch].fill(0.0);
                self.hr_buffers.hr_output_source_tags[hr_position] = u64::MAX;
                self.hr_buffers.hr_output_read_position = (hr_position + 1) & hr_mask;
                self.hr_buffers.hr_output_accumulator_fill -= 1;
            }

            if self.hr_buffers.hr_output_accumulator_fill == 0 {
                continue;
            }
            let hr_position = self.hr_buffers.hr_output_read_position;
            if self.hr_buffers.hr_output_source_tags[hr_position] != source_frame {
                continue;
            }

            let acc_base = hr_position * nch;
            let out_base = offset * output_channels;
            if binaural {
                let mut left = 0.0;
                let mut right = 0.0;
                for &ch in &self.panning.cached_hr_active_channels {
                    let sample = self.hr_buffers.hr_output_accumulator[acc_base + ch];
                    let (left_gain, right_gain) = self.binaural_preview_gains_for_channel(ch);
                    left += sample * left_gain;
                    right += sample * right_gain;
                }
                output[out_base] += left * gain;
                output[out_base + 1] += right * gain;
            } else {
                for &ch in &self.panning.cached_hr_active_channels {
                    output[out_base + ch] +=
                        self.hr_buffers.hr_output_accumulator[acc_base + ch] * gain;
                }
            }

            self.hr_buffers.hr_output_accumulator[acc_base..acc_base + nch].fill(0.0);
            self.hr_buffers.hr_output_source_tags[hr_position] = u64::MAX;
            self.hr_buffers.hr_output_read_position = (hr_position + 1) & hr_mask;
            self.hr_buffers.hr_output_accumulator_fill -= 1;
        }
    }

    /// Run HR FFT processing: window, forward FFT, HF filtering, IFFT per channel.
    /// Populates `hr_time_out_channels[ch]` with the per-channel time-domain results.
    /// Does NOT scale or mix — the caller handles that.
    fn process_hr_fft(&mut self, input: &[f32]) {
        // 1. Copy input to HR time-domain buffers and apply HR analysis window
        // Apply the same -3 dB headroom scale (1/sqrt(2)) as the main path (fft.rs)
        let headroom_scale = std::f32::consts::FRAC_1_SQRT_2;
        for i in 0..self.fft.hr_fft_size {
            let idx = i * 2;
            let window_val = self.hr_buffers.hr_window[i] * headroom_scale;
            self.hr_buffers.hr_time_domain_left[i] = input[idx] * window_val;
            self.hr_buffers.hr_time_domain_right[i] = input[idx + 1] * window_val;
        }

        // 2. Forward FFT (Real->Complex)
        self.fft
            .hr_fft_forward
            .process(
                &mut self.hr_buffers.hr_time_domain_left,
                &mut self.hr_buffers.hr_freq_domain_left,
            )
            .unwrap();
        self.fft
            .hr_fft_forward
            .process(
                &mut self.hr_buffers.hr_time_domain_right,
                &mut self.hr_buffers.hr_freq_domain_right,
            )
            .unwrap();

        // 3. Frequency-dependent processing for HF direct path only
        let freq_per_bin = self.core.sample_rate as f32 / self.fft.hr_fft_size as f32;
        let hf_cut = self.params.bandpass_hz.max(1000.0);
        let hr_spectrum_size = self.fft.hr_fft_size / 2 + 1;

        let gain_front_direct = self.gains.gain_front_direct.current();

        // Only process front, non-LFE, non-height channels (cached during build)
        for &ch_idx in &self.panning.cached_hr_active_channels {
            let is_center = self.panning.cached_is_center[ch_idx];
            let panning_gain_left = self.panning.panning_gains_left[ch_idx];
            let panning_gain_right = self.panning.panning_gains_right[ch_idx];

            let mut gain_scale = gain_front_direct;
            if is_center {
                let spread = self.gains.center_spread.current();
                gain_scale *= 1.0 - spread;
            }

            if gain_scale == 0.0 {
                // Zero out this channel's HR output so stale data isn't mixed in
                self.hr_buffers.hr_time_out_channels[ch_idx].fill(0.0);
                continue;
            }

            // Process bins with a raised-cosine transition band to avoid brick-wall
            // Gibbs ringing. A hard cutoff at hf_cut creates pre/post echoes around
            // transients — precisely when the HR path is most active.
            // Transition region: [hf_cut - transition_bw, hf_cut], width = 8 bins.
            let transition_bw = 8.0 * freq_per_bin;
            self.hr_buffers
                .hr_temp_freq_out
                .fill(Complex::new(0.0, 0.0));

            for i in 0..hr_spectrum_size {
                let freq = i as f32 * freq_per_bin;
                let gain = if freq <= hf_cut - transition_bw {
                    0.0
                } else if freq >= hf_cut {
                    1.0
                } else {
                    // Raised cosine: smoothly ramps from 0 to 1 over transition_bw
                    let t = (freq - (hf_cut - transition_bw)) / transition_bw;
                    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
                };
                if gain > 0.0 {
                    let l = self.hr_buffers.hr_freq_domain_left[i];
                    let r = self.hr_buffers.hr_freq_domain_right[i];
                    self.hr_buffers.hr_temp_freq_out[i] =
                        (l * panning_gain_left + r * panning_gain_right) * gain_scale * gain;
                }
            }

            if hr_spectrum_size > 0 {
                self.hr_buffers.hr_temp_freq_out[0].im = 0.0;
                self.hr_buffers.hr_temp_freq_out[hr_spectrum_size - 1].im = 0.0;
            }

            self.fft
                .hr_fft_inverse
                .process(
                    &mut self.hr_buffers.hr_temp_freq_out,
                    &mut self.hr_buffers.hr_time_out_channels[ch_idx],
                )
                .unwrap();

            // Matching sqrt-Hann synthesis window. The 1/N OLA normalization is
            // applied in mix_hr_output via hr_ola_scale.
            sotf_host::simd::window_mul_simd_inplace(
                &mut self.hr_buffers.hr_time_out_channels[ch_idx],
                &self.hr_buffers.hr_window,
            );
        }
    }

    /// Drain `num_frames` from the HR output ring buffer and mix into `output`.
    /// Operates in synchronized lockstep with the main path via the delay buffer.
    pub(super) fn mix_hr_output(&mut self, output: &mut [f32], num_frames: usize) {
        #[cfg(test)]
        if self.output.hr_use_pre_correction_mixer_for_test {
            self.mix_hr_output_pre_correction_for_test(output, num_frames);
            return;
        }

        let drain = num_frames.min(self.hr_buffers.hr_output_accumulator_fill);

        let nch = self.core.num_output_channels;
        let mask = self.hr_buffers.hr_output_accumulator_mask;
        let main_start = self.output.output_read_position.wrapping_sub(num_frames)
            & self.output.output_accumulator_mask;

        // Drain HR output and mix directly into main output buffer
        for i in 0..drain {
            let scale =
                self.output.hr_mix_gains[(main_start + i) & self.output.output_accumulator_mask];
            let read_idx = (self.hr_buffers.hr_output_read_position + i) & mask;
            let acc_base = read_idx * nch;
            let out_base = i * nch;

            for &ch in &self.panning.cached_hr_active_channels {
                output[out_base + ch] +=
                    self.hr_buffers.hr_output_accumulator[acc_base + ch] * scale;
                self.hr_buffers.hr_output_accumulator[acc_base + ch] = 0.0;
            }
        }

        self.hr_buffers.hr_output_read_position =
            (self.hr_buffers.hr_output_read_position + drain) & mask;
        self.hr_buffers.hr_output_accumulator_fill -= drain;
    }

    /// Isolated copy of the pre-AUD130 prepared-gain mixer for N>=512
    /// equivalence tests.
    #[cfg(test)]
    fn mix_hr_output_pre_correction_for_test(&mut self, output: &mut [f32], num_frames: usize) {
        let drain = num_frames.min(self.hr_buffers.hr_output_accumulator_fill);
        let nch = self.core.num_output_channels;
        let mask = self.hr_buffers.hr_output_accumulator_mask;
        let main_start = self.output.output_read_position.wrapping_sub(num_frames)
            & self.output.output_accumulator_mask;

        for i in 0..drain {
            let scale =
                self.output.hr_mix_gains[(main_start + i) & self.output.output_accumulator_mask];
            let read_idx = (self.hr_buffers.hr_output_read_position + i) & mask;
            let acc_base = read_idx * nch;
            let out_base = i * nch;
            for &ch in &self.panning.cached_hr_active_channels {
                output[out_base + ch] +=
                    self.hr_buffers.hr_output_accumulator[acc_base + ch] * scale;
                self.hr_buffers.hr_output_accumulator[acc_base + ch] = 0.0;
            }
        }

        self.hr_buffers.hr_output_read_position =
            (self.hr_buffers.hr_output_read_position + drain) & mask;
        self.hr_buffers.hr_output_accumulator_fill -= drain;
    }

    /// Drain HR output into a 2-channel binaural preview buffer.
    pub(super) fn mix_hr_output_binaural(&mut self, output: &mut [f32], num_frames: usize) {
        let drain = num_frames.min(self.hr_buffers.hr_output_accumulator_fill);
        let nch = self.core.num_output_channels;
        let mask = self.hr_buffers.hr_output_accumulator_mask;
        let main_start = self.output.output_read_position.wrapping_sub(num_frames)
            & self.output.output_accumulator_mask;

        for i in 0..drain {
            let scale =
                self.output.hr_mix_gains[(main_start + i) & self.output.output_accumulator_mask];
            let read_idx = (self.hr_buffers.hr_output_read_position + i) & mask;
            let acc_base = read_idx * nch;
            let out_base = i * 2;
            let mut left = 0.0;
            let mut right = 0.0;

            for &ch in &self.panning.cached_hr_active_channels {
                let sample = self.hr_buffers.hr_output_accumulator[acc_base + ch];
                let (left_gain, right_gain) = self.binaural_preview_gains_for_channel(ch);
                left += sample * left_gain;
                right += sample * right_gain;
                self.hr_buffers.hr_output_accumulator[acc_base + ch] = 0.0;
            }

            output[out_base] += left * scale;
            output[out_base + 1] += right * scale;
        }

        self.hr_buffers.hr_output_read_position =
            (self.hr_buffers.hr_output_read_position + drain) & mask;
        self.hr_buffers.hr_output_accumulator_fill -= drain;
    }

    /// Process one HR FFT block and accumulate into the HR output ring buffer.
    pub(super) fn process_hr_block(&mut self, temp_input: &[f32]) {
        self.process_hr_fft(temp_input);

        let mask = self.hr_buffers.hr_output_accumulator_mask;
        let nch = self.core.num_output_channels;
        let hr_hop = self.fft.hr_fft_size / 2;
        let hr_ring_capacity = mask + 1;

        // Guard against ring buffer overflow
        debug_assert!(
            self.hr_buffers.hr_output_accumulator_fill + hr_hop <= hr_ring_capacity,
            "HR ring buffer overflow: fill {} + hop {} > capacity {}",
            self.hr_buffers.hr_output_accumulator_fill,
            hr_hop,
            hr_ring_capacity
        );
        if self.hr_buffers.hr_output_accumulator_fill + hr_hop > hr_ring_capacity {
            return;
        }

        let source_tags_enabled = self.uses_hr_source_tags();
        for i in 0..self.fft.hr_fft_size {
            let write_idx = (self.hr_buffers.hr_next_add_position + i) & mask;
            let acc_base = write_idx * nch;

            if source_tags_enabled {
                let source_frame = self.hr_buffers.hr_next_add_source_frame + i as i64;
                if source_frame < 0 {
                    self.hr_buffers.hr_output_source_tags[write_idx] = u64::MAX;
                } else {
                    let source_frame = source_frame as u64;
                    let existing = self.hr_buffers.hr_output_source_tags[write_idx];
                    debug_assert!(
                        existing == u64::MAX || existing == source_frame,
                        "HR ring slot {write_idx} has tag {existing}, expected {source_frame}"
                    );
                    self.hr_buffers.hr_output_source_tags[write_idx] = source_frame;
                }
            }

            for &ch in &self.panning.cached_hr_active_channels {
                self.hr_buffers.hr_output_accumulator[acc_base + ch] +=
                    self.hr_buffers.hr_time_out_channels[ch][i];
            }
        }

        self.hr_buffers.hr_next_add_position =
            (self.hr_buffers.hr_next_add_position + hr_hop) & mask;
        if source_tags_enabled {
            self.hr_buffers.hr_next_add_source_frame += hr_hop as i64;
        }
        self.hr_buffers.hr_output_accumulator_fill += hr_hop;
        let discard = self
            .hr_buffers
            .hr_startup_discard_remaining
            .min(self.hr_buffers.hr_output_accumulator_fill);
        for frame in 0..discard {
            let position = (self.hr_buffers.hr_output_read_position + frame) & mask;
            let offset = position * nch;
            self.hr_buffers.hr_output_accumulator[offset..offset + nch].fill(0.0);
            if source_tags_enabled {
                self.hr_buffers.hr_output_source_tags[position] = u64::MAX;
            }
        }
        self.hr_buffers.hr_output_read_position =
            (self.hr_buffers.hr_output_read_position + discard) & mask;
        self.hr_buffers.hr_output_accumulator_fill -= discard;
        self.hr_buffers.hr_startup_discard_remaining -= discard;
    }
}
