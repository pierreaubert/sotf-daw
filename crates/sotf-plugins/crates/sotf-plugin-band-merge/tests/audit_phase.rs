//! Full-band complex and group-delay references for the actual split/merge pair.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_band_merge::BandMergePlugin;
use sotf_plugin_band_split::{BandSplitPlugin, BandSplitRecombinationMode};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

#[derive(Clone, Copy, Debug)]
struct C {
    re: f64,
    im: f64,
}
impl C {
    const ONE: Self = Self { re: 1.0, im: 0.0 };
    const ZERO: Self = Self { re: 0.0, im: 0.0 };
    fn exp(phase: f64) -> Self {
        Self {
            re: phase.cos(),
            im: phase.sin(),
        }
    }
    fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn phase(self) -> f64 {
        self.im.atan2(self.re)
    }
    fn conjugate(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }
    fn inverse(self) -> Self {
        let n = self.re * self.re + self.im * self.im;
        Self {
            re: self.re / n,
            im: -self.im / n,
        }
    }
    fn scale(self, k: f64) -> Self {
        Self {
            re: self.re * k,
            im: self.im * k,
        }
    }
    fn pow(self, n: usize) -> Self {
        (0..n).fold(Self::ONE, |a, _| a * self)
    }
}
impl std::ops::Add for C {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self {
            re: self.re + b.re,
            im: self.im + b.im,
        }
    }
}
impl std::ops::Sub for C {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self {
            re: self.re - b.re,
            im: self.im - b.im,
        }
    }
}
impl std::ops::Mul for C {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self {
            re: self.re * b.re - self.im * b.im,
            im: self.re * b.im + self.im * b.re,
        }
    }
}

// Bilinear-transform analog Butterworth prototypes, squared to form LR4/LR8.
// This uses no production filter/coefficient or spectral-analysis helpers.
fn branches(kind: &str, cutoff: f64, omega: f64, rate: u32) -> (C, C) {
    let r = (omega * 0.5).tan() / (std::f64::consts::PI * cutoff / f64::from(rate)).tan();
    let s = C { re: 0.0, im: r };
    let (butterworth, order) = if kind == "LR24" {
        (
            C {
                re: 1.0 - r * r,
                im: std::f64::consts::SQRT_2 * r,
            },
            4,
        )
    } else {
        let q1 = 1.0 / (2.0 * (std::f64::consts::PI / 8.0).sin());
        let q2 = 1.0 / (2.0 * (3.0 * std::f64::consts::PI / 8.0).sin());
        (
            (s * s + s.scale(1.0 / q1) + C::ONE) * (s * s + s.scale(1.0 / q2) + C::ONE),
            8,
        )
    };
    let low = (butterworth * butterworth).inverse();
    (low, s.pow(order) * low)
}

fn response(kind: &str, cutoffs: &[f64], compensated: bool, omega: f64, rate: u32) -> C {
    let stages: Vec<_> = cutoffs
        .iter()
        .map(|&f| branches(kind, f, omega, rate))
        .collect();
    if compensated {
        stages
            .into_iter()
            .fold(C::ONE, |h, (low, high)| h * (low + high))
    } else {
        let mut carry = C::ONE;
        let mut sum = C::ZERO;
        for (low, high) in stages {
            sum = sum + carry * low;
            carry = carry * high;
        }
        sum + carry
    }
}

fn spectrum(impulse: &[f32], omega: f64) -> C {
    let step = C::exp(-omega);
    let mut oscillator = C::ONE;
    let mut sum = C::ZERO;
    for &sample in impulse {
        sum = sum + oscillator.scale(f64::from(sample));
        oscillator = oscillator * step;
    }
    sum
}

#[test]
fn coherent_full_band_sweep_matches_independent_phase_and_group_delay() {
    const FRAMES: usize = 32768;
    // Bounds fixed before execution: existing f32 LR bank accuracy requires a
    // 0.005 complex budget; 0.01 rad phase and 50 us group-delay budgets bound
    // the added phase measurements. No fitted phase or delay alignment is used.
    let (mut worst_complex, mut worst_phase, mut worst_delay) = (0.0_f64, 0.0_f64, 0.0_f64);
    for rate in [32_000, 48_000, 96_000, 192_000] {
        for cutoffs in [
            &[1000.0][..],
            &[400.0, 4000.0][..],
            &[200.0, 1200.0, 6000.0][..],
        ] {
            for kind in ["LR24", "LR48"] {
                for compensated in [false, true] {
                    let mode = if compensated {
                        BandSplitRecombinationMode::PhaseCompensated
                    } else {
                        BandSplitRecombinationMode::LegacyCascade
                    };
                    let mut split =
                        BandSplitPlugin::new_multiband_with_mode(1, cutoffs, kind, mode).unwrap();
                    let mut merge = BandMergePlugin::new(1, cutoffs.len() + 1).unwrap();
                    split.initialize(f64::from(rate)).unwrap();
                    merge.initialize(f64::from(rate)).unwrap();
                    let mut impulse = vec![0.0; FRAMES];
                    impulse[0] = 1.0;
                    let mut output = vec![0.0; FRAMES];
                    let order: Vec<_> = (0..cutoffs.len() + 1).collect();
                    let mut offset = 0;
                    let mut block = 0;
                    while offset < FRAMES {
                        let n = [1, 31, 257, 1024][block % 4].min(FRAMES - offset);
                        let mut bands = vec![0.0; n * order.len()];
                        let context = ProcessContext::new(rate, n);
                        sotf_host::assert_no_allocs("paired split/merge precision", || {
                            split
                                .process(&impulse[offset..offset + n], &mut bands, &context)
                                .unwrap();
                            merge
                                .process_with_band_order(
                                    &bands,
                                    &mut output[offset..offset + n],
                                    &context,
                                    &order,
                                )
                                .unwrap();
                        });
                        offset += n;
                        block += 1;
                    }
                    sotf_host::assert_no_allocs("populated precision reset", || split.reset());
                    let mut probes: Vec<f64> = (0..32)
                        .map(|i| 20.0 * (f64::from(rate) * 0.45 / 20.0).powf(f64::from(i) / 31.0))
                        .collect();
                    for &f in cutoffs {
                        probes.extend([f * 0.5, f, f * 2.0]);
                    }
                    for target in probes {
                        // Bin-center the sweep so the sampled impulse spectrum
                        // has an exact coherent frequency at every rate.
                        let bin = (target * FRAMES as f64 / f64::from(rate))
                            .round()
                            .clamp(1.0, FRAMES as f64 * 0.45);
                        let omega = std::f64::consts::TAU * bin / FRAMES as f64;
                        let expected = response(kind, cutoffs, compensated, omega, rate);
                        let measured = spectrum(&output, omega);
                        let error = (measured - expected).norm();
                        worst_complex = worst_complex.max(error);
                        assert!(
                            error < 0.005,
                            "{rate} {kind} {cutoffs:?} compensated={compensated} bin={bin}: complex error {error}"
                        );
                        let phase = (measured * expected.conjugate()).phase().abs();
                        worst_phase = worst_phase.max(phase);
                        assert!(
                            phase < 0.01,
                            "{rate} {kind} {cutoffs:?}: phase error {phase}"
                        );
                        let dw = 1e-5;
                        let measured_delta = spectrum(&output, omega + dw)
                            * spectrum(&output, omega - dw).conjugate();
                        let expected_delta = response(kind, cutoffs, compensated, omega + dw, rate)
                            * response(kind, cutoffs, compensated, omega - dw, rate).conjugate();
                        let delay_error = (measured_delta.phase() - expected_delta.phase()).abs()
                            / (2.0 * dw * f64::from(rate));
                        worst_delay = worst_delay.max(delay_error);
                        assert!(
                            delay_error < 0.00005,
                            "{rate} {kind} {cutoffs:?} bin={bin}: group-delay error {delay_error} s"
                        );
                    }
                }
            }
        }
    }
    eprintln!(
        "independent split/merge: complex {worst_complex}, phase {worst_phase} rad, group delay {worst_delay} s"
    );
}
