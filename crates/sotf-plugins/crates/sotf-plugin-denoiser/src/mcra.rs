// ============================================================================
// IMCRA (Improved Minimum Controlled Recursive Averaging) Noise Estimation
// ============================================================================
//
// Implements IMCRA with dual-window minimum tracking for robust noise power
// spectral density estimation from noisy signals.
//
// Improvements over basic MCRA:
// - Dual staggered minimum windows eliminate periodic blind spots
// - Multi-frame bootstrap for stable initialization
//
// Reference: Israel Cohen, "Noise Spectrum Estimation in Adverse Environments:
//            Improved Minima Controlled Recursive Averaging", 2003

use super::DenoiserPlugin;

/// Small constant to prevent division by zero
const EPSILON: f32 = 1e-10;

/// Preserve the historical quality-mode observation span: five 1024-frame hops
/// at 48 kHz. This is 106.6667 ms, not the former comment's 53 ms. Preparation
/// rounds this span up to complete hops at the current clock. It controls noise
/// estimation warm-up, not signal latency or proof of initial speech absence.
const BOOTSTRAP_SECONDS: f64 = 5120.0 / 48_000.0;

impl DenoiserPlugin {
    pub(super) fn bootstrap_frame_count(sample_rate: f64, hop_size: usize) -> usize {
        (sample_rate * BOOTSTRAP_SECONDS / hop_size as f64)
            .ceil()
            .max(1.0) as usize
    }

    /// Orchestrate noise estimation: bootstrap phase then IMCRA.
    /// Returns `true` while any channel is still bootstrapping.
    pub(super) fn update_noise_estimation(&mut self) -> bool {
        let mut any_bootstrapping = false;

        for ch in 0..self.config.channels {
            if self.mcra.frame_counter[ch] < self.mcra.bootstrap_frames {
                // Bootstrap: accumulate power into noise_psd
                for k in 0..self.config.spectrum_size {
                    let power = self.get_power_at_bin(ch, k);
                    self.mcra.noise_psd[ch][k] += power;
                }
                self.mcra.frame_counter[ch] += 1;

                if self.mcra.frame_counter[ch] >= self.mcra.bootstrap_frames {
                    self.finalize_bootstrap(ch);
                } else {
                    any_bootstrapping = true;
                }
            } else {
                self.update_mcra(ch);
            }
        }

        // UI telemetry is an aggregate; it must not feed the next channel's DSP.
        self.ui.learning_active = self.mcra.learning_active.iter().any(|active| *active);
        any_bootstrapping
    }

    /// Finalize bootstrap by averaging accumulated power and initializing
    /// all MCRA state from the averaged noise estimate.
    fn finalize_bootstrap(&mut self, channel: usize) {
        let n = self.mcra.frame_counter[channel] as f32;
        // Minimum noise floor: -60 dB power. Prevents division-by-near-zero
        // in Wiener gain when bootstrap captured mostly silence.
        let min_noise_power = 1e-6_f32;
        for k in 0..self.config.spectrum_size {
            let avg = (self.mcra.noise_psd[channel][k] / n).max(min_noise_power);
            self.mcra.noise_psd[channel][k] = avg;
            self.mcra.smoothed_psd[channel][k] = avg;
            self.mcra.min_psd[channel][k] = avg;
            self.mcra.min_psd_b[channel][k] = avg;
            self.mcra.speech_presence[channel][k] = 0.0;
        }
    }

    /// Update IMCRA noise estimation for one channel.
    ///
    /// Algorithm:
    /// 1. Smooth power spectrum: S_tmp = α_s * S_tmp + (1-α_s) * |X|²
    /// 2. Dual-window minimum tracking (eliminates periodic blind spots)
    /// 3. Detect speech presence: I = 1 if S_tmp/S_min > δ else 0
    /// 4. Smooth speech probability: p = α_p * p + (1-α_p) * I
    /// 5. Update noise: σ_n² adaptive based on speech probability
    fn update_mcra(&mut self, channel: usize) {
        let alpha_s = self.mcra.mcra_alpha_s;
        let alpha_p = self.mcra.mcra_alpha_p;
        let delta = self.mcra.mcra_delta;
        let l = self.mcra.mcra_l;
        let half_l = l / 2;
        let frame = self.mcra.frame_counter[channel];

        // Hoist loop-invariant frame boundary checks out of the per-bin loop
        let reset_window_a = frame.is_multiple_of(l);
        let reset_window_b = frame % l == half_l;

        // Use this channel's previous-frame learning state as fast-adapt trigger
        // (avoids chicken-and-egg: we need quiet_ratio before the loop)
        let use_fast_adapt = self.mcra.learning_active[channel];
        // Track whether we're learning (quiet moment detected)
        let mut quiet_bins = 0;

        for k in 0..self.config.spectrum_size {
            let power = self.get_power_at_bin(channel, k);

            // Step 1: Update smoothed power spectral density
            let s_tmp = alpha_s * self.mcra.smoothed_psd[channel][k] + (1.0 - alpha_s) * power;
            self.mcra.smoothed_psd[channel][k] = s_tmp;

            // Step 2: IMCRA dual-window minimum tracking
            // Window A: resets at frames 0, L, 2L, ...
            if reset_window_a {
                self.mcra.min_psd[channel][k] = s_tmp;
            } else {
                self.mcra.min_psd[channel][k] = self.mcra.min_psd[channel][k].min(s_tmp);
            }

            // Window B: resets at frames L/2, 3L/2, 5L/2, ...
            if reset_window_b {
                self.mcra.min_psd_b[channel][k] = s_tmp;
            } else {
                self.mcra.min_psd_b[channel][k] = self.mcra.min_psd_b[channel][k].min(s_tmp);
            }

            // Use minimum of both windows (ensures no blind spot at reset)
            let s_min = self.mcra.min_psd[channel][k]
                .min(self.mcra.min_psd_b[channel][k])
                .max(EPSILON);

            // Step 3: Compute speech presence indicator
            let s_r = s_tmp / s_min;
            let indicator = if s_r > delta { 1.0 } else { 0.0 };

            // Step 4: Smooth speech presence probability
            let p = alpha_p * self.mcra.speech_presence[channel][k] + (1.0 - alpha_p) * indicator;
            self.mcra.speech_presence[channel][k] = p;

            // Step 5: Update noise estimate with adaptive smoothing
            // When p is high (speech present), alpha_d → 1 → slow/no update
            // When p is low (speech absent), alpha_d → alpha_s → normal update
            // Two ordinary updates with a frozen observation/probability retain
            // alpha_d^2 of the previous estimate. This doubles the exponential
            // tracking rate; halving alpha_d instead can accelerate it much more.
            let mut alpha_d = alpha_s + (1.0 - alpha_s) * p;
            if use_fast_adapt && p < 0.3 {
                alpha_d *= alpha_d;
            }
            let noise_est = alpha_d * self.mcra.noise_psd[channel][k] + (1.0 - alpha_d) * power;
            self.mcra.noise_psd[channel][k] = noise_est;

            // Count quiet bins for learning indicator
            if p < 0.5 {
                quiet_bins += 1;
            }
        }

        // Update learning indicator (more than half the bins are quiet)
        self.mcra.learning_active[channel] = quiet_bins > self.config.spectrum_size / 2;

        // Increment frame counter
        self.mcra.frame_counter[channel] += 1;
    }

    /// Get estimated noise power at a specific bin
    #[inline]
    pub(super) fn get_noise_power(&self, channel: usize, bin: usize) -> f32 {
        self.mcra.noise_psd[channel][bin].max(EPSILON)
    }

    /// Reset MCRA state for a channel
    pub(super) fn reset_mcra(&mut self, channel: usize) {
        self.mcra.noise_psd[channel].fill(0.0);
        self.mcra.smoothed_psd[channel].fill(0.0);
        self.mcra.min_psd[channel].fill(0.0);
        self.mcra.min_psd_b[channel].fill(0.0);
        self.mcra.speech_presence[channel].fill(0.0);
        self.mcra.frame_counter[channel] = 0;
        self.mcra.learning_active[channel] = true;
    }
}

#[cfg(test)]
mod channel_learning_tests {
    use super::*;
    use rustfft::num_complex::Complex;
    use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;

    #[test]
    fn bootstrap_observation_duration_follows_prepared_clock() {
        for rate in [44_100.0, 48_000.0, 48_000.25, 96_000.0, 192_000.0] {
            for low_latency in [false, true] {
                let mut plugin = DenoiserPlugin::new(2, low_latency);
                plugin.initialize(rate).unwrap();
                let expected =
                    (rate * (5120.0 / 48000.0) / plugin.config.hop_size as f64).ceil() as usize;
                for spectrum in &mut plugin.fft.freq_domain {
                    spectrum.fill(Complex::new(1.0, 0.0));
                }
                let observed = (1..=1000)
                    .find(|_| !plugin.update_noise_estimation())
                    .unwrap();
                let duration = observed as f64 * plugin.config.hop_size as f64 / rate;
                println!(
                    "bootstrap rate={rate} low_latency={low_latency} observed_hops={observed} expected_hops={expected} duration_seconds={duration}"
                );
                assert_eq!(
                    observed, expected,
                    "bootstrap changed its physical duration"
                );
                assert!(duration >= 5120.0 / 48000.0);
                assert!(duration < 5120.0 / 48000.0 + plugin.config.hop_size as f64 / rate + 1e-12);
                for channel in 0..2 {
                    assert_eq!(
                        plugin.mcra.noise_psd[channel],
                        vec![1.0; plugin.config.spectrum_size]
                    );
                }
            }
        }
    }

    #[test]
    fn bootstrap_clock_survives_reset_profile_restart_and_rejected_initialization() {
        for low_latency in [false, true] {
            let mut plugin = DenoiserPlugin::new(2, low_latency);
            plugin.initialize(96_000.0).unwrap();
            let target = plugin.mcra.bootstrap_frames;
            for spectrum in &mut plugin.fft.freq_domain {
                spectrum.fill(Complex::new(1.0, 0.0));
            }
            assert!(plugin.update_noise_estimation());
            let frames = plugin.mcra.frame_counter.clone();
            let power = plugin.mcra.noise_psd.clone();
            for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
                assert!(plugin.initialize(invalid).is_err());
                assert_eq!(plugin.mcra.bootstrap_frames, target);
                assert_eq!(plugin.mcra.frame_counter, frames);
                assert_eq!(plugin.mcra.noise_psd, power);
            }
            plugin.reset();
            assert_eq!(plugin.mcra.bootstrap_frames, target);
            assert_eq!(plugin.mcra.frame_counter, [0, 0]);
            plugin.start_learning();
            assert_eq!(plugin.mcra.bootstrap_frames, target);
            assert_eq!(plugin.mcra.frame_counter, [0, 0]);
            plugin.initialize(48_000.0).unwrap();
            assert_eq!(
                plugin.mcra.bootstrap_frames,
                if low_latency { 20 } else { 5 }
            );
            assert_eq!(plugin.mcra.frame_counter, [0, 0]);
        }
    }

    fn prepared(channels: usize) -> DenoiserPlugin {
        let mut plugin = DenoiserPlugin::new(channels, true);
        for channel in 0..channels {
            plugin.mcra.frame_counter[channel] = plugin.mcra.bootstrap_frames;
            plugin.mcra.noise_psd[channel].fill(1.0);
            plugin.mcra.smoothed_psd[channel].fill(1.0);
            plugin.mcra.min_psd[channel].fill(1.0);
            plugin.mcra.min_psd_b[channel].fill(1.0);
            plugin.fft.freq_domain[channel].fill(Complex::new(2.0_f32.sqrt(), 0.0));
        }
        plugin
    }

    #[test]
    fn fast_adaptation_matches_two_ordinary_constant_observation_updates() {
        let mut cases = 0;
        let mut failures = 0;
        let mut maximum_error = 0.0_f64;
        for alpha_s in [0.5_f32, 0.7, 0.9, 0.99] {
            for probability in [0.0_f32, 0.2, 0.299, 0.3, 0.6] {
                for previous_noise in [0.0_f32, 1.0, 10.0] {
                    for fast in [false, true] {
                        let mut plugin = prepared(1);
                        plugin.mcra.mcra_alpha_s = alpha_s;
                        // A large tracked minimum makes the observed ratio <=1,
                        // so the probability only decays by the configured pole.
                        plugin.mcra.smoothed_psd[0].fill(100.0);
                        plugin.mcra.min_psd[0].fill(100.0);
                        plugin.mcra.min_psd_b[0].fill(100.0);
                        plugin.mcra.speech_presence[0].fill(probability / plugin.mcra.mcra_alpha_p);
                        plugin.mcra.noise_psd[0].fill(previous_noise);
                        plugin.mcra.learning_active[0] = fast;
                        let power = f64::from(plugin.get_power_at_bin(0, 0));
                        plugin.update_noise_estimation();
                        let observed_probability = plugin.mcra.speech_presence[0][0];
                        assert!((observed_probability - probability).abs() < 2e-7);
                        let retention = f64::from(alpha_s)
                            + (1.0 - f64::from(alpha_s)) * f64::from(observed_probability);
                        let ordinary = |noise: f64| retention * noise + (1.0 - retention) * power;
                        let mut expected = ordinary(f64::from(previous_noise));
                        if fast && observed_probability < 0.3 {
                            expected = ordinary(expected);
                        }
                        let error = (f64::from(plugin.mcra.noise_psd[0][0]) - expected).abs();
                        maximum_error = maximum_error.max(error);
                        failures += usize::from(error > 3e-6);
                        cases += 1;
                    }
                }
            }
        }
        println!(
            "fast_adaptation_reference cases={cases} failures={failures} maximum_error={maximum_error:e}"
        );
        assert_eq!(cases, 120);
        assert_eq!(
            failures, 0,
            "fast mode must equal two ordinary updates, retaining trigger/probability boundaries"
        );
    }

    #[test]
    fn ui_learning_indicator_cannot_change_noise_estimation() {
        let mut a = prepared(1);
        let mut b = prepared(1);
        a.ui.learning_active = true;
        b.ui.learning_active = false;
        a.update_noise_estimation();
        b.update_noise_estimation();
        assert_eq!(a.mcra.noise_psd, b.mcra.noise_psd);
        assert!(a.mcra.noise_psd[0][0] > 1.0 && a.mcra.noise_psd[0][0] < 2.0);
    }

    #[test]
    fn telemetry_aggregates_channels_and_reset_and_profile_learning_restart_history() {
        let mut p = prepared(2);
        p.mcra.speech_presence[0].fill(0.9);
        p.mcra.smoothed_psd[0].fill(20.0);
        p.mcra.learning_active[0] = false;
        p.update_noise_estimation();
        assert_eq!(p.mcra.learning_active, [false, true]);
        assert!(p.ui.learning_active);
        p.mcra.learning_active.fill(false);
        p.reset();
        assert_eq!(p.mcra.learning_active, [true, true]);
        assert_eq!(p.mcra.frame_counter, [0, 0]);
        p.mcra.learning_active.fill(false);
        p.start_learning();
        assert_eq!(p.mcra.learning_active, [true, true]);
        assert_eq!(p.mcra.frame_counter, [0, 0]);
    }
}
