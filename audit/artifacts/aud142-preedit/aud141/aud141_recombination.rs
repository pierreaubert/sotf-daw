//! Public regression for the LR24 multiway all-pass decomposition.

use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_crossover::CrossoverPlugin;

const SAMPLE_RATE: u32 = 48_000;
const PROBE_HZ: f64 = 1_100.0;
const WARMUP_FRAMES: usize = 48_000;
const MEASURE_FRAMES: usize = 9_600;

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn reciprocal(self) -> Self {
        let norm_squared = self.re * self.re + self.im * self.im;
        Self::new(self.re / norm_squared, -self.im / norm_squared)
    }
}

impl std::ops::Add for Complex {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl std::ops::Mul for Complex {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

fn lr24_low_high(cutoff_hz: f64) -> (Complex, Complex) {
    // Independent analog LR4 prototypes evaluated with the prewarped
    // bilinear substitution. For s = j*t, both fourth-order numerators are
    // real, while the shared denominator is (1 - t^2 + j*sqrt(2)*t)^2.
    let t = (std::f64::consts::PI * PROBE_HZ / f64::from(SAMPLE_RATE)).tan()
        / (std::f64::consts::PI * cutoff_hz / f64::from(SAMPLE_RATE)).tan();
    let denominator = Complex::new(1.0 - t * t, std::f64::consts::SQRT_2 * t);
    let common = (denominator * denominator).reciprocal();
    let low = common;
    let high = Complex::new(t.powi(4), 0.0) * common;
    (low, high)
}

fn project(signal: &[f32], start: usize, frames: usize) -> Complex {
    let mut re = 0.0;
    let mut im = 0.0;
    let omega = std::f64::consts::TAU * PROBE_HZ / f64::from(SAMPLE_RATE);
    for (offset, &sample) in signal[start..start + frames].iter().enumerate() {
        let phase = omega * (start + offset) as f64;
        let sample = f64::from(sample);
        re += sample * phase.cos();
        im -= sample * phase.sin();
    }
    let scale = 2.0 / frames as f64;
    Complex::new(re * scale, im * scale)
}

#[test]
fn overlapping_multiway_lr24_bands_match_allpass_decomposition() {
    let (low0, high0) = lr24_low_high(1_000.0);
    let (low1, high1) = lr24_low_high(1_200.0);
    let expected_bands = [low0 * (low1 + high1), high0 * low1, high0 * high1];
    let expected_sum = expected_bands
        .iter()
        .copied()
        .fold(Complex::new(0.0, 0.0), |sum, band| sum + band);
    let expected_allpass = (low0 + high0) * (low1 + high1);
    assert!((expected_sum.re - expected_allpass.re).abs() < 1.0e-12);
    assert!((expected_sum.im - expected_allpass.im).abs() < 1.0e-12);
    assert!((expected_allpass.norm() - 1.0).abs() < 1.0e-12);

    let total_frames = WARMUP_FRAMES + MEASURE_FRAMES;
    let input: Vec<f32> = (0..total_frames)
        .map(|frame| {
            (std::f64::consts::TAU * PROBE_HZ * frame as f64 / f64::from(SAMPLE_RATE)).sin() as f32
                * 0.25
        })
        .collect();
    let mut plugin = CrossoverPlugin::new_multiway(1, "LR24", 1_000.0, "both", &[1_200.0]).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    let mut output = vec![0.0_f32; total_frames * 3];
    plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, total_frames),
        )
        .unwrap();

    let input_response = project(&input, WARMUP_FRAMES, MEASURE_FRAMES);
    assert!(
        input_response.norm() > 0.1,
        "coherent input tone projection is too small to normalize residuals: {input_response:?}"
    );
    let actual_bands: [Complex; 3] = std::array::from_fn(|band| {
        let channel: Vec<f32> = output[band..].iter().step_by(3).copied().collect();
        project(&channel, WARMUP_FRAMES, MEASURE_FRAMES)
    });
    let actual_transfer: [Complex; 3] = std::array::from_fn(|band| {
        let re =
            actual_bands[band].re * input_response.re + actual_bands[band].im * input_response.im;
        let im =
            actual_bands[band].im * input_response.re - actual_bands[band].re * input_response.im;
        let denom = input_response.re * input_response.re + input_response.im * input_response.im;
        Complex::new(re / denom, im / denom)
    });
    let actual_sum = actual_transfer
        .iter()
        .copied()
        .fold(Complex::new(0.0, 0.0), |sum, band| sum + band);
    let normalized_residual = actual_transfer
        .iter()
        .zip(expected_bands)
        .map(|(actual, expected)| {
            (actual.re - expected.re).powi(2) + (actual.im - expected.im).powi(2)
        })
        .sum::<f64>()
        .sqrt();

    println!("input projection: {input_response:?}");
    println!("actual band transfers: {actual_transfer:?}");
    println!("expected band transfers: {expected_bands:?}");
    println!("input-normalized branch residual: {normalized_residual}");
    println!(
        "actual summed transfer: {actual_sum:?}, magnitude={}",
        actual_sum.norm()
    );
    println!("expected all-pass transfer: {expected_allpass:?}");

    for (band, (actual, expected)) in actual_transfer.iter().zip(expected_bands).enumerate() {
        let error = (actual.re - expected.re).hypot(actual.im - expected.im);
        assert!(
            error < 2.0e-3,
            "band {band} complex error {error}: actual={actual:?}, expected={expected:?}"
        );
    }
    assert!(
        normalized_residual < 2.0e-3,
        "input-normalized branch residual {normalized_residual} exceeds 0.002"
    );
    let sum_error =
        (actual_sum.re - expected_allpass.re).hypot(actual_sum.im - expected_allpass.im);
    assert!(
        sum_error < 2.0e-3,
        "summed complex error {sum_error}: actual={actual_sum:?}, expected={expected_allpass:?}"
    );
}
