// ============================================================================
// GSC — Generalized Sidelobe Canceller
// ============================================================================
//
// Adaptive beamformer architecture:
// 1. Fixed Beamformer (FBF): delay-and-sum toward look direction
// 2. Blocking Matrix: B = I - dd^H/||d||² — projects onto null space
// 3. NLMS Adaptive Noise Canceller: cancels residual noise from FBF output
//
// Works sample-by-sample (no FFT) — lowest latency of all beamformers.

/// Generalized Sidelobe Canceller.
#[derive(Debug)]
pub struct GscBeamformer {
    num_mics: usize,
    /// Fixed beamformer weights (real-valued delay-and-sum)
    fbf_weights: Vec<f32>,
    /// Blocking matrix columns: [(num_mics-1) x num_mics] in flattened form
    blocking_matrix: Vec<Vec<f32>>,
    /// NLMS weights and reference history use f64 so a finite f32 input cannot
    /// overflow the blocking projection or its squared reference power.
    adaptive_weights: Vec<Vec<f64>>,
    /// Reference signal delay lines [(num_mics-1) x filter_length].
    reference_buffers: Vec<Vec<f64>>,
    ref_write_pos: usize,
    filter_length: usize,
    /// NLMS step size
    mu: f32,
    /// Regularization
    delta: f32,
    /// Pre-allocated scratch for reference signals (avoids per-sample allocation)
    reference_scratch: Vec<f64>,
    /// Delay-aligned microphone samples shared by the fixed and blocking paths.
    aligned_samples: Vec<f64>,
    /// Per-mic delay lines for fractional delay compensation
    delay_lines: Vec<Vec<f32>>,
    delay_write_pos: usize,
    max_delay_samples: usize,
    steering_delays: Vec<f32>,
    detector_alpha: f64,
    coherent_power: f64,
    array_power: f64,
    mix_alpha: f64,
    cancellation_mix: f64,
}

impl GscBeamformer {
    /// Create a new GSC beamformer.
    ///
    /// # Arguments
    /// * `num_mics` - Number of microphones
    /// * `steering_delays` - Time delay per mic in samples (from look direction)
    /// * `filter_length` - NLMS filter length (default: 64)
    /// * `mu` - NLMS step size (default: 0.01)
    pub fn new(num_mics: usize, steering_delays: &[f32], filter_length: usize, mu: f32) -> Self {
        Self::new_with_sample_rate(num_mics, steering_delays, filter_length, mu, 48_000.0)
    }

    /// Creates a GSC with sample-rate-aware spatial-coherence detection.
    ///
    /// # Panics
    ///
    /// Panics if fewer than two microphones are requested.
    pub fn new_with_sample_rate(
        num_mics: usize,
        steering_delays: &[f32],
        filter_length: usize,
        mu: f32,
        sample_rate: f64,
    ) -> Self {
        assert!(num_mics >= 2, "GSC requires at least 2 microphones");

        // Fixed beamformer: uniform real-valued weights for delay-and-sum.
        let fbf_weights = vec![1.0f32 / num_mics as f32; num_mics];

        // Blocking matrix: orthogonal complement of steering vector
        // B = I - d*d^H / (d^H * d)
        let num_refs = num_mics - 1;
        let d_norm_sq = fbf_weights.iter().map(|w| w * w).sum::<f32>();
        let mut blocking_matrix = Vec::with_capacity(num_refs);
        for r in 0..num_mics {
            let mut row = vec![0.0f32; num_mics];
            for c in 0..num_mics {
                row[c] = if r == c { 1.0 } else { 0.0 };
                row[c] -= fbf_weights[r] * fbf_weights[c] / d_norm_sq;
            }
            let norm_sq = row.iter().map(|x| x * x).sum::<f32>();
            if norm_sq > 1e-10 && blocking_matrix.len() < num_refs {
                blocking_matrix.push(row);
            }
        }
        // Fallback: should never happen for num_mics >= 2, but keep safe
        while blocking_matrix.len() < num_refs {
            let mut row = vec![0.0f32; num_mics];
            row[blocking_matrix.len()] = 1.0;
            row[blocking_matrix.len() + 1] = -1.0;
            blocking_matrix.push(row);
        }

        // Delay lines
        let max_delay = steering_delays
            .iter()
            .map(|d| d.ceil() as usize + 1)
            .max()
            .unwrap_or(1)
            .max(1);
        let delay_lines = vec![vec![0.0f32; max_delay]; num_mics];

        Self {
            num_mics,
            fbf_weights,
            blocking_matrix,
            adaptive_weights: vec![vec![0.0; filter_length]; num_refs],
            reference_buffers: vec![vec![0.0; filter_length]; num_refs],
            ref_write_pos: 0,
            filter_length,
            mu,
            delta: 1e-6,
            reference_scratch: vec![0.0; num_refs],
            aligned_samples: vec![0.0; num_mics],
            delay_lines,
            delay_write_pos: 0,
            max_delay_samples: max_delay,
            steering_delays: steering_delays.to_vec(),
            // Smooth spatial powers over 10 ms so target zero crossings do
            // not masquerade as uncorrelated interference and enable learning.
            detector_alpha: (-1.0 / (sample_rate.max(1.0) * 0.010)).exp(),
            coherent_power: 0.0,
            array_power: 0.0,
            // Fade learned cancellation over 5 ms when a coherent target
            // appears, protecting it from mismatch in retained noise weights.
            mix_alpha: (-1.0 / (sample_rate.max(1.0) * 0.005)).exp(),
            cancellation_mix: 1.0,
        }
    }

    /// Process one sample from all microphones.
    ///
    /// # Arguments
    /// * `mic_samples` - One sample per microphone
    ///
    /// # Returns
    /// Single beamformed output sample
    #[allow(clippy::needless_range_loop)]
    pub fn process_sample(&mut self, mic_samples: &[f32]) -> f32 {
        self.process_sample_with_adaptation(mic_samples, true)
    }

    pub(crate) fn process_sample_frozen(&mut self, mic_samples: &[f32]) -> f32 {
        self.process_sample_with_adaptation(mic_samples, false)
    }

    pub(crate) fn finite_support_frames(&self) -> usize {
        self.latency_samples() + self.filter_length.saturating_sub(1)
    }

    #[cfg(test)]
    pub(crate) fn install_test_weights(&mut self, weights: &[f32]) {
        assert_eq!(self.adaptive_weights.len(), 1);
        assert_eq!(self.adaptive_weights[0].len(), weights.len());
        for (destination, &source) in self.adaptive_weights[0].iter_mut().zip(weights) {
            *destination = f64::from(source);
        }
    }

    #[cfg(test)]
    pub(crate) fn learned_snapshot(&self) -> Vec<Vec<f64>> {
        self.adaptive_weights.clone()
    }

    #[cfg(test)]
    pub(crate) fn detector_snapshot(&self) -> [f64; 3] {
        [self.coherent_power, self.array_power, self.cancellation_mix]
    }

    #[allow(clippy::needless_range_loop)]
    fn process_sample_with_adaptation(&mut self, mic_samples: &[f32], adapt: bool) -> f32 {
        let m = self.num_mics.min(mic_samples.len());
        let num_refs = self.blocking_matrix.len();

        // Write current samples to delay lines
        for i in 0..m {
            self.delay_lines[i][self.delay_write_pos] = mic_samples[i];
        }

        // 1. Fixed beamformer output: delay-compensated sum
        let mut fbf_output = 0.0_f64;
        for i in 0..m {
            let delay = self.steering_delays[i];
            let int_delay = delay.floor() as usize;
            let frac = f64::from(delay - delay.floor());
            let buf_len = self.delay_lines[i].len();
            let idx0 = (self.delay_write_pos + buf_len - int_delay) % buf_len;
            // Fractional delay lies between the integer-delayed sample and
            // the next older sample in ring time.
            let idx1 = (idx0 + buf_len - 1) % buf_len;
            let delayed_sample = f64::from(self.delay_lines[i][idx0]) * (1.0 - frac)
                + f64::from(self.delay_lines[i][idx1]) * frac;
            self.aligned_samples[i] = delayed_sample;
            fbf_output += delayed_sample * f64::from(self.fbf_weights[i]);
        }

        // Advance delay write position
        self.delay_write_pos = (self.delay_write_pos + 1) % self.max_delay_samples;

        // 2. Blocking matrix: compute reference signals
        // u = B * x (num_refs reference signals)
        self.reference_scratch[..num_refs].fill(0.0);
        for (r, row) in self.blocking_matrix.iter().enumerate() {
            for i in 0..m {
                self.reference_scratch[r] += f64::from(row[i]) * self.aligned_samples[i];
            }
        }
        let references = &self.reference_scratch[..num_refs];

        // Store references in delay lines
        for (buf, &ref_val) in self.reference_buffers[..num_refs]
            .iter_mut()
            .zip(references)
        {
            buf[self.ref_write_pos] = ref_val;
        }

        // 3. NLMS adaptive noise canceller
        // Compute noise estimate: y = Σ_r w_r^T * u_r
        let mut noise_estimate = 0.0_f64;
        let mut total_ref_power = f64::from(self.delta);

        for r in 0..num_refs {
            let mut buf_idx = self.ref_write_pos;
            for j in 0..self.filter_length {
                noise_estimate += self.adaptive_weights[r][j] * self.reference_buffers[r][buf_idx];
                total_ref_power += self.reference_buffers[r][buf_idx].powi(2);
                buf_idx = if buf_idx == 0 {
                    self.filter_length - 1
                } else {
                    buf_idx - 1
                };
            }
        }

        // Error signal (beamformed output minus noise estimate)
        let error = fbf_output - noise_estimate;

        // NLMS weight update: w += μ * e * u / (||u||² + δ)
        let mut valid_evidence = false;
        if adapt {
            let coherent = fbf_output * fbf_output * m as f64;
            let total = self.aligned_samples[..m].iter().map(|x| x * x).sum::<f64>();
            // Unusable samples must not leave recursive detector state NaN
            // after their finite delay/reference history has cleared.
            valid_evidence = coherent.is_finite() && total.is_finite();
            if valid_evidence {
                self.coherent_power = self.detector_alpha * self.coherent_power
                    + (1.0 - self.detector_alpha) * coherent;
                self.array_power =
                    self.detector_alpha * self.array_power + (1.0 - self.detector_alpha) * total;
                // Long silence must not retain slow subnormal detector tails.
                // Every squared nonzero finite f32 sample is normal in f64.
                if self.coherent_power.is_subnormal() {
                    self.coherent_power = 0.0;
                }
                if self.array_power.is_subnormal() {
                    self.array_power = 0.0;
                }
            }
        }
        // Use the same coherent fraction as spectral MVDR target protection.
        // The ratio is independent of signal scale and blocks adaptation on
        // modest steering/calibration mismatch, not just perfectly equal mics.
        let target_dominant = self.coherent_power >= 0.65 * self.array_power;
        if valid_evidence {
            let target_mix = if target_dominant { 0.0 } else { 1.0 };
            self.cancellation_mix =
                self.mix_alpha * self.cancellation_mix + (1.0 - self.mix_alpha) * target_mix;
            if self.cancellation_mix.is_subnormal() {
                self.cancellation_mix = 0.0;
            }
        }
        if valid_evidence && !target_dominant {
            let step = f64::from(self.mu) * error / total_ref_power;
            for r in 0..num_refs {
                let mut buf_idx = self.ref_write_pos;
                for j in 0..self.filter_length {
                    let candidate =
                        self.adaptive_weights[r][j] + step * self.reference_buffers[r][buf_idx];
                    // Invalid input or arithmetic must not poison retained
                    // coefficients after the finite reference history clears.
                    if candidate.is_finite() {
                        self.adaptive_weights[r][j] = candidate;
                    }
                    buf_idx = if buf_idx == 0 {
                        self.filter_length - 1
                    } else {
                        buf_idx - 1
                    };
                }
            }
            self.bound_adaptive_energy();
        }

        // Advance write position
        self.ref_write_pos = (self.ref_write_pos + 1) % self.filter_length;

        // Cancellation can exceed the f32 range even when all inputs are
        // finite. Saturate only at the public sample boundary, preserving
        // the wider signal and adaptation internally.
        let output = fbf_output - self.cancellation_mix * noise_estimate;
        if output.is_finite() {
            output.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
        } else {
            0.0
        }
    }

    fn bound_adaptive_energy(&mut self) {
        // Unit total microphone FIR energy gives a 0 dB sensor-white-noise
        // gain floor. Only scale blocked cancellation; preserve fixed summation.
        // Every blocking row is diag*e_r + off*ones. Its Gram matrix is
        // diag^2*I + (2*diag*off + M*off^2)*ones. Use that identity to bound
        // microphone-space FIR energy in O(taps*references), without forming
        // a microphone-by-tap matrix or allocating on the audio thread.
        let m = self.num_mics as f64;
        let off = f64::from(self.blocking_matrix[0][1]);
        let diag = f64::from(self.blocking_matrix[0][0]) - off;
        let cross = (2.0 * diag + m * off) * off;
        let mut adaptive_energy = 0.0;
        let mut first_sum = 0.0;
        for tap in 0..self.filter_length {
            let mut sum = 0.0;
            let mut squares = 0.0;
            for row in &self.adaptive_weights {
                sum += row[tap];
                squares += row[tap] * row[tap];
            }
            adaptive_energy += diag * diag * squares + cross * sum * sum;
            if tap == 0 {
                first_sum = sum;
            }
        }
        let fixed = f64::from(self.fbf_weights[0]);
        let fixed_energy = m * fixed * fixed;
        // Include the tiny fixed/blocking cross term from f32 construction.
        let linear = -2.0 * fixed * (diag + m * off) * first_sum;
        if fixed_energy + linear + adaptive_energy > 1.0 {
            let capacity = 1.0 - fixed_energy;
            let scale = ((linear * linear + 4.0 * adaptive_energy * capacity).sqrt() - linear)
                / (2.0 * adaptive_energy);
            for row in &mut self.adaptive_weights {
                for weight in row {
                    *weight *= scale;
                }
            }
        }
    }

    /// Reset adaptive weights.
    pub fn reset(&mut self) {
        for w in &mut self.adaptive_weights {
            w.fill(0.0);
        }
        for b in &mut self.reference_buffers {
            b.fill(0.0);
        }
        for d in &mut self.delay_lines {
            d.fill(0.0);
        }
        self.ref_write_pos = 0;
        self.delay_write_pos = 0;
        self.coherent_power = 0.0;
        self.array_power = 0.0;
        self.cancellation_mix = 1.0;
    }

    /// Common steering-compensation latency in whole samples.
    pub fn latency_samples(&self) -> usize {
        self.steering_delays
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .ceil() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decaying_states_do_not_retain_subnormal_values() {
        let mut core = GscBeamformer::new(2, &[0.0, 0.0], 32, 0.01);
        core.coherent_power = f64::MIN_POSITIVE;
        core.array_power = f64::MIN_POSITIVE;
        core.cancellation_mix = f64::MIN_POSITIVE;
        assert_eq!(core.process_sample(&[0.0, 0.0]), 0.0);
        assert!(
            core.detector_snapshot()
                .iter()
                .all(|x| *x == 0.0 || x.is_normal())
        );
    }

    #[test]
    fn invalid_evidence_preserves_detectors_and_recovers_without_reset() {
        for mics in [2, 8] {
            for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut core = GscBeamformer::new(mics, &vec![0.0; mics], 32, 0.01);
                for n in 0..4096 {
                    let input: Vec<f32> = (0..mics)
                        .map(|mic| (n as f32 * 0.11 + mic as f32 * 1.7).sin() * 0.25)
                        .collect();
                    core.process_sample(&input);
                }
                let weights = core.learned_snapshot();
                let detector = core.detector_snapshot();
                let mut input = vec![0.25; mics];
                input[0] = bad;
                assert!(core.process_sample(&input).is_finite());
                assert_eq!(core.learned_snapshot(), weights);
                assert_eq!(core.detector_snapshot(), detector);
                for n in 0..9600 {
                    let target = (n as f32 * 0.07).sin() * 0.25;
                    let output = core.process_sample(&vec![target; mics]);
                    assert!(output.is_finite());
                    if n >= 4800 {
                        assert!((output - target).abs() < 1e-6);
                    }
                }
                assert!(core.detector_snapshot().iter().all(|x| x.is_finite()));
            }
        }
    }

    #[test]
    fn prepared_detector_uses_physical_time_across_construction_reset_and_reinitialize() {
        use sotf_host::plugin::{Plugin, ProcessContext};
        let rates = [16_000_u32, 44_100, 48_000, 96_000];
        for (index, &initial_rate) in rates.iter().enumerate() {
            let mut plugin = crate::BeamformerPlugin::from_params(
                initial_rate,
                crate::BeamformerPluginParams {
                    beamformer_type: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut rate = initial_rate;
            for pass in 0..3 {
                if pass == 1 {
                    plugin.reset();
                }
                if pass == 2 {
                    rate = rates[(index + 1) % rates.len()];
                    plugin.initialize(f64::from(rate)).unwrap();
                }
                assert_eq!(plugin.gsc.detector_snapshot(), [0.0, 0.0, 1.0]);
                let frames = rate as usize / 50;
                let input: Vec<f32> = (0..frames).flat_map(|_| [1.0, 0.0]).collect();
                plugin
                    .process(
                        &input,
                        &mut vec![0.0; frames],
                        &ProcessContext::new(rate, frames),
                    )
                    .unwrap();
                let expected = -(-(frames as f64) / (f64::from(rate) * 0.010)).exp_m1();
                let actual = plugin.gsc.detector_snapshot();
                assert!((actual[0] - expected * 0.5).abs() < 1e-12, "rate={rate}");
                assert!((actual[1] - expected).abs() < 1e-12, "rate={rate}");
                assert_eq!(actual[2], 1.0);
            }
        }
    }

    #[test]
    fn bounded_adaptation_matches_full_microphone_fir_energy() {
        for mics in 2..=8 {
            for amplitude in [1.0, 1e30] {
                let mut core = GscBeamformer::new(mics, &vec![0.0; mics], 32, 0.01);
                for (r, row) in core.adaptive_weights.iter_mut().enumerate() {
                    for (tap, w) in row.iter_mut().enumerate() {
                        *w = amplitude * if (r + tap) % 3 == 0 { -2.0 } else { 1.0 };
                    }
                }
                core.bound_adaptive_energy();
                let mut energy = 0.0;
                for tap in 0..32 {
                    let mut sum = 0.0;
                    for mic in 0..mics {
                        let cancellation: f64 = core
                            .adaptive_weights
                            .iter()
                            .zip(&core.blocking_matrix)
                            .map(|(w, b)| w[tap] * f64::from(b[mic]))
                            .sum();
                        let coefficient = if tap == 0 {
                            f64::from(core.fbf_weights[mic])
                        } else {
                            0.0
                        } - cancellation;
                        energy += coefficient * coefficient;
                        sum += coefficient;
                    }
                    assert!((sum - if tap == 0 { 1.0 } else { 0.0 }).abs() < 1e-6);
                }
                assert!((energy - 1.0).abs() < 2e-12, "mics={mics}, energy={energy}");
            }
        }
    }

    #[test]
    fn test_gsc_creation() {
        let gsc = GscBeamformer::new(4, &[0.0, 0.001, 0.002, 0.003], 32, 0.01);
        assert_eq!(gsc.num_mics, 4);
        assert_eq!(gsc.filter_length, 32);
    }

    #[test]
    fn test_gsc_process_sample() {
        let mut gsc = GscBeamformer::new(2, &[0.0, 0.0], 16, 0.01);

        // Process some samples
        for _ in 0..100 {
            let output = gsc.process_sample(&[0.5, 0.5]);
            assert!(output.is_finite(), "Output should be finite");
        }
    }

    #[test]
    fn test_gsc_noise_cancellation() {
        let mut gsc = GscBeamformer::new(2, &[0.0, 0.0], 32, 0.05);

        // Simulate: target from broadside (same on both mics) + noise from side (opposite phase)
        let num_samples = 20_000;
        let mut error_power = 0.0f32;
        let mut raw_error_power = 0.0f32;

        for i in 0..num_samples {
            let t = i as f32 / 48000.0;
            let target = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.3;
            let noise = (2.0 * std::f32::consts::PI * 1500.0 * t).sin() * 0.5;

            // Target arrives equally, noise arrives with opposite phase
            let mic0 = target + noise;
            let mic1 = target - noise;

            let output = gsc.process_sample(&[mic0, mic1]);

            // Measure in last quarter (after convergence)
            if i >= num_samples * 3 / 4 {
                error_power += (output - target).powi(2);
                raw_error_power += noise.powi(2);
            }
        }

        // The GSC should improve the output compared to raw microphone
        assert!(
            error_power < raw_error_power * 0.2,
            "GSC must improve target-referenced error by at least 7 dB: output={error_power}, raw={raw_error_power}"
        );
    }

    #[test]
    fn test_gsc_reset() {
        let mut gsc = GscBeamformer::new(2, &[0.0, 0.0], 16, 0.01);

        // Process with signal that creates non-zero error
        for i in 0..200 {
            let t = i as f32 / 48000.0;
            let s = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            gsc.process_sample(&[s * 0.5 + 0.3, s * 0.5 - 0.3]);
        }

        // Verify weights are non-zero
        let has_nonzero = gsc
            .adaptive_weights
            .iter()
            .any(|w| w.iter().any(|&v| v != 0.0));
        assert!(has_nonzero, "Weights should be non-zero after adaptation");

        gsc.reset();

        for w in &gsc.adaptive_weights {
            for &v in w {
                assert_eq!(v, 0.0, "Weights should be zero after reset");
            }
        }
    }

    #[test]
    fn test_gsc_blocking_matrix_orthogonal() {
        let gsc = GscBeamformer::new(3, &[0.0, 0.001, 0.002], 16, 0.01);

        // The FBF weights form the steering vector for aligned signals
        let d = &gsc.fbf_weights;

        for (r, row) in gsc.blocking_matrix.iter().enumerate() {
            let dot: f32 = row.iter().zip(d).map(|(a, b)| a * b).sum();
            assert!(
                dot.abs() < 1e-5,
                "Blocking matrix row {r} not orthogonal to steering vector: dot={dot}"
            );
        }

        // Should have exactly num_mics - 1 rows
        assert_eq!(gsc.blocking_matrix.len(), 2);
    }

    #[test]
    fn test_gsc_delay_compensation() {
        // 2 mics, mic0 receives 1 sample before mic1.
        // Compensation delays [1.0, 0.0] delay mic0 by 1 to align with mic1.
        // mu = 0.0 disables adaptation so output = fbf_output.
        let mut gsc = GscBeamformer::new(2, &[1.0, 0.0], 16, 0.0);

        // Feed a ramp where mic1 is already delayed by 1 sample relative to mic0.
        for t in 10..100 {
            let mic0 = t as f32;
            let mic1 = (t - 1) as f32;
            let output = gsc.process_sample(&[mic0, mic1]);

            // After delay compensation, both mics see (t-1); fbf averages to (t-1).
            if t >= 12 {
                let expected = (t - 1) as f32;
                assert!(
                    (output - expected).abs() < 0.01,
                    "GSC delay compensation failed at t={t}: expected {expected}, got {output}"
                );
            }
        }
    }

    #[test]
    fn fractional_delay_uses_next_older_sample() {
        let mut gsc = GscBeamformer::new(2, &[1.5, 0.0], 16, 0.0);
        for t in 0..100 {
            let mic0 = t as f32;
            let mic1 = t as f32 - 1.5;
            let output = gsc.process_sample(&[mic0, mic1]);
            if t >= 4 {
                assert!((output - mic1).abs() < 1.0e-5, "t={t}: {output} != {mic1}");
                assert!(
                    gsc.reference_scratch
                        .iter()
                        .all(|value| value.abs() < 1.0e-5),
                    "aligned look source leaked into blocking references: {:?}",
                    gsc.reference_scratch
                );
            }
        }
        assert_eq!(gsc.latency_samples(), 2);
    }

    #[test]
    fn target_plane_waves_are_distortionless_across_angles() {
        use crate::steering::{ArrayGeometry, compute_steering_delays};

        let geometry = ArrayGeometry::Linear {
            num_mics: 4,
            spacing_m: 0.05,
        };
        let sample_rate = 48_000.0;
        let frequency = 700.0;
        for angle in [0.0, 30.0, 60.0, 90.0] {
            let compensation = compute_steering_delays(&geometry, angle, 0.0, sample_rate);
            let common_delay = compensation.iter().copied().fold(0.0_f32, f32::max);
            let propagation: Vec<f32> = compensation
                .iter()
                .map(|delay| common_delay - delay)
                .collect();
            let mut gsc = GscBeamformer::new(4, &compensation, 32, 0.03);
            let mut signal_energy = 0.0_f32;
            let mut error_energy = 0.0_f32;
            let mut reference_energy = 0.0_f64;
            for frame in 0..20_000 {
                let samples: Vec<f32> = propagation
                    .iter()
                    .map(|delay| {
                        (std::f32::consts::TAU * frequency * (frame as f32 - delay) / sample_rate)
                            .sin()
                    })
                    .collect();
                let output = gsc.process_sample(&samples);
                if frame > 2_000 {
                    let expected =
                        (std::f32::consts::TAU * frequency * (frame as f32 - common_delay)
                            / sample_rate)
                            .sin();
                    signal_energy += expected * expected;
                    error_energy += (output - expected).powi(2);
                    reference_energy += gsc.reference_scratch.iter().map(|x| x * x).sum::<f64>();
                }
            }
            assert!(
                error_energy / signal_energy < 1e-3,
                "angle={angle}, normalized error={}",
                error_energy / signal_energy
            );
            assert!(
                reference_energy / f64::from(signal_energy) < 1e-4,
                "angle={angle}, target leakage={}",
                reference_energy / f64::from(signal_energy)
            );
        }
    }
}
