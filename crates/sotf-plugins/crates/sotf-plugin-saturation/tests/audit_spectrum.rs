//! Independent Fourier references for saturation coloration and folded harmonics.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_saturation::{SaturationPlugin, SaturationPluginParams};
use std::f64::consts::{PI, TAU};
use std::ops::{Add, Div, Mul, Sub};

#[derive(Clone, Copy, Default, Debug)]
struct C(f64, f64);
impl C {
    fn power(self) -> f64 {
        self.0 * self.0 + self.1 * self.1
    }
    fn polar(angle: f64) -> Self {
        Self(angle.cos(), angle.sin())
    }
}
impl Add for C {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1)
    }
}
impl Sub for C {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self(self.0 - b.0, self.1 - b.1)
    }
}
impl Mul for C {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self(self.0 * b.0 - self.1 * b.1, self.0 * b.1 + self.1 * b.0)
    }
}
impl Div for C {
    type Output = Self;
    fn div(self, b: Self) -> Self {
        let q = b.power();
        Self(
            (self.0 * b.0 + self.1 * b.1) / q,
            (self.1 * b.0 - self.0 * b.1) / q,
        )
    }
}

// Test-only radix-2 transform, independent of the DSP's math library.
fn spectrum(mut x: Vec<C>) -> Vec<C> {
    let n = x.len();
    assert!(n.is_power_of_two());
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - n.trailing_zeros());
        if j > i {
            x.swap(i, j);
        }
    }
    let mut width = 2;
    while width <= n {
        for start in (0..n).step_by(width) {
            for j in 0..width / 2 {
                let a = x[start + j];
                let b = x[start + j + width / 2] * C::polar(-TAU * j as f64 / width as f64);
                x[start + j] = a + b;
                x[start + j + width / 2] = a - b;
            }
        }
        width *= 2;
    }
    for v in &mut x {
        *v = *v / C(n as f64, 0.0);
    }
    x
}

fn curve(mode: &str, x: f64, drive: f64) -> f64 {
    let u = x * drive;
    match mode {
        "Soft Clip" | "Exciter" => u.tanh() / drive.tanh(),
        "Tube" => u / (1.0 + u * u), // Tone = 2, independent rational reference.
        "Tape" => u.signum() * (-(-2.0 * u.abs()).exp_m1()) * 0.5,
        "Asymmetric" => {
            let bias = 0.24_f64.tanh();
            let y = (u + 0.24).tanh() - bias;
            y / if y >= 0.0 { 1.0 - bias } else { 1.0 + bias }
        }
        _ => unreachable!(),
    }
}

fn render(
    mode: &str,
    rate: u32,
    bin: usize,
    drive: f32,
    adaa: bool,
    mix: f32,
    amplitude: f64,
) -> Vec<C> {
    const N: usize = 4096;
    let mut p = SaturationPlugin::from_validated_params(
        1,
        SaturationPluginParams {
            mode: mode.into(),
            drive,
            tone: 2.0,
            exciter_freq: 3000.0,
            mix,
            oversampling: "Off".into(),
            use_adaa: adaa,
            dc_blocker_enabled: false,
            dynamic_amount: 0.0,
            ..Default::default()
        },
    );
    p.initialize(f64::from(rate)).unwrap();
    let mut tail = Vec::new();
    for block in 0..3 {
        let mut x: Vec<f32> = (0..N)
            .map(|n| {
                (amplitude * (TAU * bin as f64 * (block * N + n) as f64 / N as f64).sin()) as f32
            })
            .collect();
        p.process_in_place(&mut x, &ProcessContext::new(rate, N))
            .unwrap();
        tail = x;
    }
    spectrum(tail.into_iter().map(|x| C(f64::from(x), 0.0)).collect())
}

fn reference(mode: &str, rate: u32, bin: usize, drive: f64, points: usize) -> Vec<C> {
    // Dense sampling of the continuous waveshaper produces its harmonic series.
    // Exciter's low/high sine amplitudes and phases come from an independent
    // bilinear Butterworth transfer, squared for the fourth-order crossover.
    let s = C(
        0.0,
        (PI * bin as f64 / 4096.0).tan() / (PI * 3000.0 / f64::from(rate)).tan(),
    );
    let denominator = C(1.0, 0.0) + C(2.0_f64.sqrt(), 0.0) * s + s * s;
    let low = C(1.0, 0.0) / (denominator * denominator);
    let high = s * s * s * s * low;
    spectrum(
        (0..points)
            .map(|n| {
                let phase = C::polar(TAU * n as f64 / points as f64);
                let y = if mode == "Exciter" {
                    0.7 * (low * phase).1 + curve(mode, 0.7 * (high * phase).1, drive)
                } else {
                    curve(mode, 0.7 * phase.1, drive)
                };
                C(y, 0.0)
            })
            .collect(),
    )
}

fn fold(harmonics: &[C], bin: usize) -> Vec<C> {
    let mut folded = vec![C::default(); 4096];
    for (i, &value) in harmonics.iter().enumerate() {
        let harmonic = if i < harmonics.len() / 2 {
            i as isize
        } else {
            i as isize - harmonics.len() as isize
        };
        let j = (harmonic * bin as isize).rem_euclid(4096) as usize;
        folded[j] = folded[j] + value;
    }
    folded
}
fn error(a: &[C], b: &[C]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(&a, &b)| (a - b).power())
        .sum::<f64>()
        .sqrt()
}
fn alias_power(x: &[C], bin: usize) -> f64 {
    let mut legitimate = vec![false; x.len()];
    legitimate[0] = true;
    for h in 1..=x.len() / 2 / bin {
        legitimate[h * bin] = true;
        legitimate[x.len() - h * bin] = true;
    }
    x.iter()
        .zip(legitimate)
        .filter(|(_, keep)| !keep)
        .map(|(c, _)| c.power())
        .sum()
}

#[test]
fn all_modes_match_converged_oversampled_harmonic_references() {
    let mut largest = 0.0_f64;
    let mut strongest_alias = 0.0_f64;
    for mode in ["Soft Clip", "Tube", "Tape", "Exciter", "Asymmetric"] {
        for rate in [44_100, 48_000, 96_000] {
            for bin in [37, 311, 901, 1601] {
                for drive in [1.0, 4.0, 12.0] {
                    let expected = fold(&reference(mode, rate, bin, drive, 32768), bin);
                    let coarser = fold(&reference(mode, rate, bin, drive, 16384), bin);
                    assert!(
                        error(&expected, &coarser) < 2e-5,
                        "reference failed to converge: {mode}, bin={bin}, drive={drive}"
                    );
                    let actual = render(mode, rate, bin, drive as f32, false, 1.0, 0.7);
                    let residual = error(&actual, &expected);
                    largest = largest.max(residual);
                    assert!(
                        residual < 1e-5,
                        "{mode} {rate} bin={bin} drive={drive}: spectral error {residual}"
                    );
                    strongest_alias = strongest_alias.max(alias_power(&actual, bin));
                    if mode != "Soft Clip" {
                        let flagged = render(mode, rate, bin, drive as f32, true, 1.0, 0.7);
                        assert_eq!(
                            error(&actual, &flagged),
                            0.0,
                            "ADAA must only affect Soft Clip"
                        );
                    }
                }
            }
        }
    }
    assert!(
        strongest_alias > 0.01,
        "the sweep must include substantial folded energy"
    );
    println!(
        "maximum spectral RMS error={largest:e}; largest native folded power={strongest_alias:e}"
    );
}

#[test]
fn soft_clip_adaa_reduces_folded_energy_at_high_drive() {
    for rate in [44_100, 48_000, 96_000] {
        for bin in [901, 1601] {
            let direct = alias_power(&render("Soft Clip", rate, bin, 12.0, false, 1.0, 0.7), bin);
            let adaa = alias_power(&render("Soft Clip", rate, bin, 12.0, true, 1.0, 0.7), bin);
            assert!(
                adaa < direct * 0.5,
                "{rate}, bin={bin}: direct={direct:e}, ADAA={adaa:e}"
            );
        }
    }
}

#[test]
fn small_signal_adaa_and_parallel_mix_include_the_fractional_sample_response() {
    // ADAA averages adjacent inputs in the linear limit: H=(1+z^-1)/2.
    // The dry path is current-sample audio; no integer latency can align this
    // frequency-dependent response. Assert the actual complex parallel sum.
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for bin in [511, 1023, 1535] {
            for mix in [0.0, 0.5, 1.0] {
                let x = render("Soft Clip", rate, bin, 1.0, true, mix, 1e-4);
                let measured = x[bin] / C(0.0, -0.5e-4);
                let wet = (C(1.0, 0.0) + C::polar(-TAU * bin as f64 / 4096.0))
                    * C(0.5 / 1.0_f64.tanh(), 0.0);
                let expected = C(1.0 - f64::from(mix), 0.0) + wet * C(f64::from(mix), 0.0);
                assert!(
                    (measured - expected).power().sqrt() < 3e-4,
                    "{rate} bin={bin} mix={mix}: measured {measured:?}, expected {expected:?}"
                );
            }
        }
    }
}
