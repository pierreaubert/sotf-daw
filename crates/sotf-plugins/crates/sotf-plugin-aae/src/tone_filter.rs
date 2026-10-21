//! First-order shelving filter for frequency-dependent decay in the FDN.
//!
//! Each FDN delay line needs a different RT60 at bass vs treble frequencies.
//! This filter sits inside the feedback loop and applies per-sample gain that
//! varies with frequency, creating the desired frequency-dependent decay.
//!
//! Design: given target gains at DC (g_dc) and Nyquist (g_ny), compute
//! first-order IIR coefficients that interpolate between them.

/// Prepared recursive coefficients; copying them does not perform design work.
///
/// Double precision preserves nearly cancelling fast-endpoint numerators.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ToneCoefficients {
    b0: f64,
    b1: f64,
    a1: f64,
}
impl ToneCoefficients {
    pub(crate) const SILENCE: Self = Self {
        b0: 0.0,
        b1: 0.0,
        a1: 0.0,
    };
}

/// First-order IIR tone correction filter.
///
/// Transfer function: `H(z) = (b0 + b1*z^-1) / (1 + a1*z^-1)`.
///
/// Public gain constructors constrain `H(1)` and `H(-1)`. FDN-specific
/// preparation instead constrains the requested decaying pole radii.
pub struct ToneFilter {
    b0: f64,
    b1: f64,
    a1: f64,
    target_b0: f64,
    target_b1: f64,
    target_a1: f64,
    transition_remaining: usize,
    /// Filter state
    x1: f64,
    y1: f64,
}

impl ToneFilter {
    /// Design a tone correction filter from target gains at DC and Nyquist.
    ///
    /// - `g_dc`: desired gain at 0 Hz (controls bass RT60)
    /// - `g_ny`: desired gain at Nyquist (controls treble RT60)
    ///
    /// Both must be in (0, 1) for a stable decaying FDN.
    pub fn new(g_dc: f32, g_ny: f32) -> Self {
        let g_dc = f64::from(g_dc);
        let g_ny = f64::from(g_ny);
        // First-order filter design from two magnitude constraints:
        //   |H(z=1)| = g_dc  → (b0 + b1) / (1 + a1) = g_dc
        //   |H(z=-1)| = g_ny → (b0 - b1) / (1 - a1) = g_ny
        //
        // Solving with a1 as free parameter using the Jot (1991) approach:
        // Set the pole to create a smooth transition between g_dc and g_ny.
        //
        // We use the formulation from Jot's FDN reverberator:
        //   a1 = (g_dc - g_ny) / (g_dc + g_ny)  (approximate, works well for |g_dc - g_ny| small)
        //   Then solve for b0, b1 from the two constraints.

        let sum = g_dc + g_ny;
        let diff = g_dc - g_ny;

        if sum.abs() < 1e-10 {
            // Both gains essentially zero — pass nothing
            return Self {
                b0: 0.0,
                b1: 0.0,
                a1: 0.0,
                target_b0: 0.0,
                target_b1: 0.0,
                target_a1: 0.0,
                transition_remaining: 0,
                x1: 0.0,
                y1: 0.0,
            };
        }

        // Pole position — clamped to avoid placing the pole on or outside the unit
        // circle when the gain ratio is extreme (e.g. g_ny ≈ 0 with g_dc finite).
        // Without the clamp, |a1| → 1, making the time constant of the filter
        // approach infinity (non-decaying FDN feedback loop).
        // The limit ±0.999 keeps the pole at |z| ≤ 0.999, giving a worst-case
        // time constant of -1 / ln(0.999) ≈ 999.5 samples, well-behaved even
        // at extreme RT60 / treble-ratio combinations.
        let a1 = (-diff / sum).clamp(-0.999, 0.999);

        // From H(1) = g_dc: (b0 + b1) = g_dc * (1 + a1)
        // From H(-1) = g_ny: (b0 - b1) = g_ny * (1 - a1)
        let h_dc = g_dc * (1.0 + a1);
        let h_ny = g_ny * (1.0 - a1);

        let b0 = (h_dc + h_ny) * 0.5;
        let b1 = (h_dc - h_ny) * 0.5;

        Self {
            b0,
            b1,
            a1,
            target_b0: b0,
            target_b1: b1,
            target_a1: a1,
            transition_remaining: 0,
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Design attenuation at decaying loop poles rather than the unit circle.
    pub(crate) fn coefficients_for_loop(
        g_dc: f32,
        g_ny: f32,
        loop_frames: f32,
    ) -> ToneCoefficients {
        let base = Self::new(g_dc, g_ny);
        let original = ToneCoefficients {
            b0: base.b0,
            b1: base.b1,
            a1: base.a1,
        };
        if g_dc == g_ny || g_dc <= 0.0 || g_ny <= 0.0 || loop_frames <= 0.0 {
            return original;
        }
        let r_dc = f64::from(g_dc).powf(1.0 / f64::from(loop_frames));
        let r_ny = f64::from(g_ny).powf(1.0 / f64::from(loop_frames));
        // Keep the filter pole strictly inside its corresponding target radial
        // point. The normal FDN parameter range retains the existing pole clamp.
        let a1 = if base.a1 < 0.0 {
            base.a1.max(-r_dc * (1.0 - 1e-6))
        } else {
            base.a1.min(r_ny * (1.0 - 1e-6))
        };
        // H(r_dc)=g_dc and H(-r_ny)=g_ny. Solving these real constraints
        // includes the IIR memory in the requested loop decay. Solving only
        // H(+/-1) leaves an extra decay-time contribution from the filter pole.
        let dc = f64::from(g_dc) * (1.0 + a1 / r_dc);
        let ny = f64::from(g_ny) * (1.0 - a1 / r_ny);
        ToneCoefficients {
            b0: (r_dc * dc + r_ny * ny) / (r_dc + r_ny),
            b1: r_dc * r_ny * (dc - ny) / (r_dc + r_ny),
            a1,
        }
    }

    pub(crate) fn new_for_loop(g_dc: f32, g_ny: f32, loop_frames: f32) -> Self {
        let mut filter = Self::unity();
        filter.set_coefficients_smooth(Self::coefficients_for_loop(g_dc, g_ny, loop_frames), 0);
        filter
    }

    /// Install prepared targets without powers, allocation, or filter-state loss.
    pub(crate) fn set_coefficients_smooth(
        &mut self,
        coefficients: ToneCoefficients,
        samples: usize,
    ) {
        self.target_b0 = coefficients.b0;
        self.target_b1 = coefficients.b1;
        self.target_a1 = coefficients.a1;
        self.transition_remaining = samples;
        if samples == 0 {
            self.b0 = coefficients.b0;
            self.b1 = coefficients.b1;
            self.a1 = coefficients.a1;
        }
    }

    /// Create a unity (bypass) filter.
    pub fn unity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            a1: 0.0,
            target_b0: 1.0,
            target_b1: 0.0,
            target_a1: 0.0,
            transition_remaining: 0,
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Process one sample.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let input = f64::from(input);
        if self.transition_remaining > 0 {
            let remaining = self.transition_remaining as f64;
            self.b0 += (self.target_b0 - self.b0) / remaining;
            self.b1 += (self.target_b1 - self.b1) / remaining;
            self.a1 += (self.target_a1 - self.a1) / remaining;
            self.transition_remaining -= 1;
        }
        let output = self.b0 * input + self.b1 * self.x1 - self.a1 * self.y1;
        self.x1 = input;
        self.y1 = output;
        output as f32
    }

    /// Update filter coefficients (e.g., when RT60 parameters change).
    pub fn set_gains(&mut self, g_dc: f32, g_ny: f32) {
        self.set_gains_smooth(g_dc, g_ny, 0);
    }

    pub fn set_gains_smooth(&mut self, g_dc: f32, g_ny: f32, samples: usize) {
        let g_dc = f64::from(g_dc);
        let g_ny = f64::from(g_ny);
        let sum = g_dc + g_ny;
        let diff = g_dc - g_ny;
        if sum.abs() < 1e-10 {
            self.target_b0 = 0.0;
            self.target_b1 = 0.0;
            self.target_a1 = 0.0;
        } else {
            self.target_a1 = (-diff / sum).clamp(-0.999, 0.999);
            let h_dc = g_dc * (1.0 + self.target_a1);
            let h_ny = g_ny * (1.0 - self.target_a1);
            self.target_b0 = (h_dc + h_ny) * 0.5;
            self.target_b1 = (h_dc - h_ny) * 0.5;
        }
        self.transition_remaining = samples;
        if samples == 0 {
            self.b0 = self.target_b0;
            self.b1 = self.target_b1;
            self.a1 = self.target_a1;
        }
    }

    /// Reset filter state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unity_filter() {
        let mut f = ToneFilter::unity();
        for i in 0..100 {
            let input = (i as f32 * 0.1).sin();
            let output = f.process(input);
            assert!(
                (output - input).abs() < 1e-5,
                "Unity filter should pass through, got {output} vs {input}"
            );
        }
    }

    #[test]
    fn test_dc_gain() {
        let g_dc = 0.95;
        let g_ny = 0.8;
        let mut f = ToneFilter::new(g_dc, g_ny);

        // Feed DC signal (constant 1.0) and check steady-state gain
        let mut output = 0.0;
        for _ in 0..10000 {
            output = f.process(1.0);
        }
        assert!(
            (output - g_dc).abs() < 0.01,
            "DC gain should be ~{g_dc}, got {output}"
        );
    }

    #[test]
    fn test_nyquist_gain() {
        let g_dc = 0.95;
        let g_ny = 0.7;
        let mut f = ToneFilter::new(g_dc, g_ny);

        // Feed Nyquist signal (alternating +1, -1)
        let mut output = 0.0;
        for i in 0..10000 {
            let input = if i % 2 == 0 { 1.0 } else { -1.0 };
            output = f.process(input);
        }
        assert!(
            (output.abs() - g_ny).abs() < 0.01,
            "Nyquist gain should be ~{g_ny}, got {}",
            output.abs()
        );
    }

    #[test]
    fn test_both_gains_equal() {
        let g = 0.9;
        let mut f = ToneFilter::new(g, g);

        // Should behave like a simple scalar multiply
        let mut output = 0.0;
        for _ in 0..1000 {
            output = f.process(1.0);
        }
        assert!(
            (output - g).abs() < 0.01,
            "Equal gains should give flat response at {g}, got {output}"
        );
    }

    #[test]
    fn test_zero_gains() {
        let mut f = ToneFilter::new(0.0, 0.0);
        for _ in 0..100 {
            let output = f.process(1.0);
            assert!(output.abs() < 1e-10);
        }
    }

    /// When g_ny is very small relative to g_dc (bright room with fast treble decay),
    /// a1 approaches -1.0, placing the FDN feedback pole very close to z = +1 (DC).
    /// Without clamping, the impulse response barely decays over 100k samples — making
    /// the reverb tail effectively infinite. The clamped design should produce measurable
    /// decay in 100k samples (≪ 2 second tail at 48 kHz).
    #[test]
    fn test_extreme_gain_ratio_decays_properly() {
        // treble_ratio = 0.2, rt60 = 0.3s → g_ny can be extremely small.
        // With g_ny = 1e-6, unclamped a1 ≈ -0.999998, pole ≈ +0.999998,
        // time constant ≈ 4.5M samples — effectively non-decaying.
        let g_dc = 0.95_f32;
        let g_ny = 1e-6_f32;
        let mut f = ToneFilter::new(g_dc, g_ny);

        // Feed an impulse followed by silence
        let first_out = f.process(1.0);
        assert!(
            first_out.is_finite(),
            "First output must be finite, got {first_out}"
        );

        let mut output = 0.0_f32;
        for _ in 0..100_000 {
            output = f.process(0.0);
            assert!(output.is_finite(), "Output diverged: {output}");
        }
        // After 100k samples (> 2s at 48 kHz) signal must have decayed to at least
        // 10% of the initial peak — i.e., at least 20 dB attenuation.
        // Without the clamp this fails because the pole is at ~0.999998.
        assert!(
            output.abs() < first_out.abs() * 0.1,
            "Filter should decay >20 dB in 100k samples. first_out={first_out}, \
             output_at_100k={output} (ratio {})",
            output.abs() / first_out.abs().max(1e-30)
        );
    }

    /// Mirror test: g_dc << g_ny — a1 approaches +1, pole approaches z = -1 (Nyquist).
    #[test]
    fn test_extreme_inverse_ratio_decays_properly() {
        let g_dc = 1e-6_f32;
        let g_ny = 0.95_f32;
        let mut f = ToneFilter::new(g_dc, g_ny);

        let first_out = f.process(1.0);
        assert!(first_out.is_finite());

        let mut output = 0.0_f32;
        for _ in 0..100_000 {
            output = f.process(0.0);
            assert!(
                output.is_finite(),
                "Output diverged (pole at Nyquist): {output}"
            );
        }
        assert!(
            output.abs() < first_out.abs() * 0.1,
            "Filter should decay >20 dB in 100k samples. first_out={first_out}, \
             output_at_100k={output}"
        );
    }

    #[test]
    fn test_reset() {
        let mut f = ToneFilter::new(0.95, 0.8);
        for _ in 0..100 {
            f.process(1.0);
        }
        f.reset();
        // After reset, first sample should be b0 * input (no history)
        let out = f.process(1.0);
        assert!((f64::from(out) - f.b0).abs() < 1e-6);
    }

    #[test]
    fn test_gain_updates_interpolate_over_requested_samples() {
        let mut filter = ToneFilter::new(0.95, 0.8);
        let old_b0 = filter.b0;
        filter.set_gains_smooth(0.7, 0.3, 240);
        assert_eq!(filter.b0, old_b0, "coefficient must not jump at the setter");
        for _ in 0..240 {
            filter.process(0.0);
        }
        assert!((filter.b0 - filter.target_b0).abs() < 1e-6);
        assert!((filter.b1 - filter.target_b1).abs() < 1e-6);
        assert!((filter.a1 - filter.target_a1).abs() < 1e-6);
    }
}

#[cfg(test)]
mod radial_tests {
    use super::*;

    fn response(filter: &ToneFilter, z: f64) -> f64 {
        (filter.b0 + filter.b1 / z) / (1.0 + filter.a1 / z)
    }

    #[test]
    fn loop_design_matches_radial_constraints_with_f64_precision() {
        let mut cases = 0;
        let mut maximum_slow_relative_error = 0.0_f64;
        let mut maximum_fast_relative_error = 0.0_f64;
        let mut maximum_circle_ratio = 0.0_f64;
        let mut original_circle_ratio = 0.0_f64;
        for rate in [44_100_f32, 48_000.0, 96_000.0, 192_000.0] {
            for room in [0.2, 3.0] {
                for rt60 in [0.3, 6.0] {
                    for bass in [0.8, 2.0] {
                        for treble in [0.2, 1.0] {
                            for base in [
                                1553_f32, 1907.0, 2311.0, 2719.0, 3187.0, 3557.0, 4001.0, 4507.0,
                            ] {
                                let m = (base * (room * rate / 48_000.0)).round() + 1.0;
                                let dc = 10.0_f32.powf(-3.0 * m / (rt60 * bass * rate));
                                let ny = 10.0_f32.powf(-3.0 * m / (rt60 * treble * rate));
                                let rdc = f64::from(dc).powf(1.0 / f64::from(m));
                                let rny = f64::from(ny).powf(1.0 / f64::from(m));
                                let filter = ToneFilter::new_for_loop(dc, ny, m);
                                let old = ToneFilter::new(dc, ny);
                                for (z, target) in [(rdc, dc), (-rny, ny)] {
                                    let error = (response(&filter, z) - f64::from(target)).abs();
                                    // Keep both endpoints accurate even when numerator terms
                                    // nearly cancel at the fast endpoint.
                                    let scale = (filter.b0.abs() + (filter.b1 / z).abs())
                                        / (1.0 + filter.a1 / z).abs();
                                    assert!(
                                        error <= 16.0 * f64::EPSILON * scale.max(f64::from(target))
                                    );
                                    if target == dc.max(ny) {
                                        maximum_slow_relative_error = maximum_slow_relative_error
                                            .max(error / f64::from(target));
                                        assert!(error / f64::from(target) < 2e-10);
                                    } else {
                                        maximum_fast_relative_error = maximum_fast_relative_error
                                            .max(error / f64::from(target));
                                    }
                                }
                                let radius = rdc.max(rny);
                                assert!(filter.a1.abs() < radius);
                                let target = radius.powf(f64::from(m));
                                // |H(R exp(iw))|^2 is linear-fractional in cos(w)
                                // for a real first-order filter: extrema are at +/-R.
                                let maximum = response(&filter, radius)
                                    .abs()
                                    .max(response(&filter, -radius).abs());
                                let old_maximum = response(&old, radius)
                                    .abs()
                                    .max(response(&old, -radius).abs());
                                maximum_circle_ratio = maximum_circle_ratio.max(maximum / target);
                                original_circle_ratio =
                                    original_circle_ratio.max(old_maximum / target);
                                assert!(maximum / target < 1.0000000002);
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(maximum_fast_relative_error < 1e-5);
        assert!(
            original_circle_ratio > 2.0,
            "original pole-memory discrepancy must remain reproduced"
        );
        println!(
            "radial_constraints cases={cases} max_slow_relative_error={maximum_slow_relative_error} max_fast_relative_error={maximum_fast_relative_error} max_circle_ratio={maximum_circle_ratio} original_max_circle_ratio={original_circle_ratio}"
        );
    }
}
