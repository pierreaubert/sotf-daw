//! Prepared unit-circle oscillators for slow FDN delay modulation.

// Rust guideline compliant 2026-02-21
/// Bound accumulated roundoff without evaluating trigonometry per sample.
const NORMALIZE_PERIOD: usize = 4096;

pub(super) struct Modulator {
    sine: f64,
    cosine: f64,
    step_sine: f64,
    step_cosine: f64,
    remaining: usize,
}

impl Modulator {
    pub(super) fn new(frequency: f64, sample_rate: f64, phase: f64) -> Self {
        let (step_sine, step_cosine) = (std::f64::consts::TAU * frequency / sample_rate).sin_cos();
        let mut oscillator = Self {
            sine: 0.0,
            cosine: 1.0,
            step_sine,
            step_cosine,
            remaining: NORMALIZE_PERIOD,
        };
        oscillator.reset(phase);
        oscillator
    }

    pub(super) fn reset(&mut self, phase: f64) {
        (self.sine, self.cosine) = (std::f64::consts::TAU * phase).sin_cos();
        self.remaining = NORMALIZE_PERIOD;
    }

    #[inline]
    pub(super) fn next(&mut self) -> f32 {
        let output = self.sine as f32;
        let sine = self.sine * self.step_cosine + self.cosine * self.step_sine;
        self.cosine = self.cosine * self.step_cosine - self.sine * self.step_sine;
        self.sine = sine;
        self.remaining -= 1;
        if self.remaining == 0 {
            let scale = (self.sine * self.sine + self.cosine * self.cosine)
                .sqrt()
                .recip();
            self.sine *= scale;
            self.cosine *= scale;
            self.remaining = NORMALIZE_PERIOD;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::Modulator;

    #[test]
    fn prepared_oscillators_match_analytic_sample_clock_and_exact_reset() {
        let mut maximum_error = 0.0_f64;
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for (index, frequency) in [0.07, 0.11, 0.13, 0.17, 0.19, 0.23, 0.29, 0.31]
                .into_iter()
                .enumerate()
            {
                let phase = index as f64 / 8.0;
                let mut oscillator = Modulator::new(frequency, rate, phase);
                let mut prefix = Vec::new();
                for frame in 0..1_000_000 {
                    let actual = oscillator.next();
                    let expected =
                        (std::f64::consts::TAU * (phase + frame as f64 * frequency / rate)).sin();
                    maximum_error = maximum_error.max((f64::from(actual) - expected).abs());
                    assert!((f64::from(actual) - expected).abs() < 6e-8);
                    if frame < 8193 {
                        prefix.push(actual);
                    }
                }
                oscillator.reset(phase);
                for expected in prefix {
                    assert_eq!(oscillator.next(), expected);
                }
            }
        }
        println!(
            "fdn_lfo_reference cases=32 samples=32000000 maximum_error={maximum_error:e} exact_reset=true"
        );
    }
}
