//! Independent array response, diffuse directivity, and white-noise gain checks.

use nalgebra::{Complex, DMatrix, DVector};
use sotf_plugin_beamformer::mvdr::MvdrBeamformer;
use sotf_plugin_beamformer::steering::ArrayGeometry;
use sotf_plugin_beamformer::superdirective::SuperdirectiveBeamformer;

fn direction(mics: usize, spacing: f64, frequency: f64, angle: f64) -> Vec<Complex<f64>> {
    (0..mics)
        .map(|mic| {
            let delay = mic as f64 * spacing * angle.to_radians().sin() / 343.0;
            Complex::from_polar(1.0, -std::f64::consts::TAU * frequency * delay)
        })
        .collect()
}

#[test]
fn mvdr_learned_close_interference_preserves_target_without_white_noise_boost() {
    let bins = [1, 5, 11, 43];
    for mics in [2, 4, 8] {
        for spacing in [0.01, 0.05] {
            for angle in [-45.0, 0.0, 45.0] {
                let d: Vec<_> = bins
                    .iter()
                    .map(|&bin| direction(mics, spacing, bin as f64 * 93.75, angle))
                    .collect();
                let interference: Vec<_> = bins
                    .iter()
                    .map(|&bin| direction(mics, spacing, bin as f64 * 93.75, angle + 5.0))
                    .collect();
                let steering: Vec<Vec<_>> = d
                    .iter()
                    .map(|v| {
                        v.iter()
                            .map(|c| Complex::new(c.re as f32, c.im as f32))
                            .collect()
                    })
                    .collect();
                let spectra: Vec<Vec<_>> = (0..mics)
                    .map(|mic| {
                        interference
                            .iter()
                            .map(|v| Complex::new(v[mic].re as f32, v[mic].im as f32))
                            .collect()
                    })
                    .collect();
                let mut core = MvdrBeamformer::new(mics, bins.len());
                // Deliberately supply known interference covariance even when
                // close to the look direction. This isolates weight robustness
                // from the production target-presence classifier's decisions.
                core.target_presence_threshold = 1.1;
                for _ in 0..300 {
                    assert!(core.update_noise_covariance(&spectra, &steering));
                }
                let weights = core.compute_weights(&steering);
                for (k, w) in weights.iter().enumerate() {
                    let w: Vec<_> = w
                        .iter()
                        .map(|c| Complex::new(f64::from(c.re), f64::from(c.im)))
                        .collect();
                    let energy = w.iter().map(|c| c.norm_sqr()).sum::<f64>();
                    assert!(energy <= 1.0 + 1e-6, "MVDR white-noise boost: {energy}");
                    let target: Complex<f64> = w.iter().zip(&d[k]).map(|(w, d)| w.conj() * d).sum();
                    assert!((target - Complex::new(1.0, 0.0)).norm() < 2e-5);
                    let residual: Complex<f64> = w
                        .iter()
                        .zip(&interference[k])
                        .map(|(w, n)| w.conj() * n)
                        .sum();
                    let uniform: Complex<f64> = d[k]
                        .iter()
                        .zip(&interference[k])
                        .map(|(d, n)| d.conj() * n / mics as f64)
                        .sum();
                    assert!(residual.norm_sqr() <= uniform.norm_sqr() + 2e-5);
                    eprintln!(
                        "MVDR_ROBUST mics={mics} spacing={spacing} angle={angle} bin={} wng_db={:.6} interference_power={:.9}",
                        bins[k],
                        -10.0 * energy.log10(),
                        residual.norm_sqr()
                    );
                }
            }
        }
    }
}

#[test]
fn superdirective_weights_bound_white_noise_and_steering_error() {
    let mut failures = Vec::new();
    let mut minimum_wng = f64::INFINITY;
    let mut maximum_target_error = 0.0_f64;
    let mut maximum_directivity_loss = 0.0_f64;
    for mics in [2, 4, 8] {
        for spacing in [0.01, 0.05, 0.20] {
            for angle in [0.0_f64, 30.0, 80.0] {
                let mut core = SuperdirectiveBeamformer::new(
                    &ArrayGeometry::Linear {
                        num_mics: mics,
                        spacing_m: spacing as f32,
                    },
                    angle as f32,
                    512,
                    48_000.0,
                    0.01,
                );
                // Probe the linear public operator using independent microphone
                // basis vectors, rather than reading its private weights.
                let mut basis = vec![vec![Complex::new(0.0, 0.0); 257]; mics];
                let mut weights = vec![vec![Complex::new(0.0_f64, 0.0); mics]; 257];
                for mic in 0..mics {
                    basis[mic].fill(Complex::new(1.0, 0.0));
                    for (row, value) in weights.iter_mut().zip(core.apply(&basis)) {
                        row[mic] = Complex::new(f64::from(value.re), -f64::from(value.im));
                    }
                    basis[mic].fill(Complex::new(0.0, 0.0));
                }
                for bin in [1, 2, 5, 11, 21, 43, 85] {
                    let frequency = bin as f64 * 48_000.0 / 512.0;
                    let d = direction(mics, spacing, frequency, angle);
                    let w = &weights[bin];
                    let response: Complex<f64> = w.iter().zip(&d).map(|(w, d)| w.conj() * d).sum();
                    assert!((response - Complex::new(1.0, 0.0)).norm() < 2e-4);
                    let energy = w.iter().map(|w| w.norm_sqr()).sum::<f64>();
                    let wng = -10.0 * energy.log10();
                    minimum_wng = minimum_wng.min(wng);
                    if wng < -1e-4 {
                        failures.push(format!(
                            "{mics}/{spacing}/{angle}/{frequency}: WNG {wng:.3} dB"
                        ));
                    }
                    // Independent isotropic diffuse covariance sinc(k*r).
                    let mut diffuse = Complex::new(0.0, 0.0);
                    let mut uniform_diffuse = 0.0;
                    for i in 0..mics {
                        for j in 0..mics {
                            let argument =
                                std::f64::consts::TAU * frequency * (i as f64 - j as f64) * spacing
                                    / 343.0;
                            let coherence = if argument.abs() < 1e-12 {
                                1.0
                            } else {
                                argument.sin() / argument
                            };
                            diffuse += w[i].conj() * w[j] * coherence;
                            uniform_diffuse +=
                                (d[i].conj() * d[j]).re * coherence / (mics * mics) as f64;
                        }
                    }
                    assert!(diffuse.re > 0.0 && diffuse.im.abs() < 1e-8);
                    assert!(
                        diffuse.re <= uniform_diffuse + 2e-4,
                        "diffuse noise worse than delay-and-sum"
                    );
                    // Independent f64 dense inverse of the original loaded
                    // diffuse covariance quantifies the robustness tradeoff.
                    let gamma = DMatrix::from_fn(mics, mics, |i, j| {
                        let x = std::f64::consts::TAU * frequency * (i as f64 - j as f64) * spacing
                            / 343.0;
                        Complex::new(if x.abs() < 1e-12 { 1.0 } else { x.sin() / x }, 0.0)
                    });
                    let loaded = &gamma * Complex::new(0.99, 0.0)
                        + DMatrix::identity(mics, mics) * Complex::new(0.01, 0.0);
                    let direction = DVector::from_column_slice(&d);
                    let mut unbounded = loaded.try_inverse().unwrap() * &direction;
                    let denominator = (direction.adjoint() * &unbounded)[0];
                    unbounded /= denominator;
                    let unbounded_diffuse = (unbounded.adjoint() * gamma * &unbounded)[0].re;
                    let directivity_loss = 10.0 * (diffuse.re / unbounded_diffuse).log10();
                    maximum_directivity_loss = maximum_directivity_loss.max(directivity_loss);
                    for error_kind in 0..3 {
                        let perturbed: Vec<Complex<f64>> = d
                            .iter()
                            .enumerate()
                            .map(|(mic, &value)| {
                                let sign = if mic % 2 == 0 { -1.0 } else { 1.0 };
                                match error_kind {
                                    // Alternating gain error of +/-0.5 dB.
                                    0 => value * 10.0_f64.powf(sign * 0.5 / 20.0),
                                    // Independent per-mic clock skew of +/-0.05 sample.
                                    1 => {
                                        value
                                            * Complex::from_polar(
                                                1.0,
                                                -std::f64::consts::TAU * frequency * sign * 0.05
                                                    / 48_000.0,
                                            )
                                    }
                                    // Alternating position error of +/-1 mm along x.
                                    _ => {
                                        value
                                            * Complex::from_polar(
                                                1.0,
                                                -std::f64::consts::TAU
                                                    * frequency
                                                    * sign
                                                    * 0.001
                                                    * angle.to_radians().sin()
                                                    / 343.0,
                                            )
                                    }
                                }
                            })
                            .collect();
                        let actual: Complex<f64> =
                            w.iter().zip(&perturbed).map(|(w, d)| w.conj() * d).sum();
                        let error = (actual - Complex::new(1.0, 0.0)).norm();
                        maximum_target_error = maximum_target_error.max(error);
                        let uncertainty = perturbed
                            .iter()
                            .zip(&d)
                            .map(|(a, b)| (*a - *b).norm_sqr())
                            .sum::<f64>()
                            .sqrt();
                        assert!(
                            error <= uncertainty + 2e-4,
                            "uncertainty amplification {mics}/{spacing}/{angle}/{frequency}: {error} > {uncertainty}"
                        );
                    }
                    eprintln!(
                        "BEAMFORMER mics={mics} spacing={spacing} angle={angle} hz={frequency} wng_db={wng:.6} diffuse_di_db={:.6} directivity_loss_db={directivity_loss:.6}",
                        -10.0 * diffuse.re.log10()
                    );
                }
            }
        }
    }
    eprintln!(
        "BEAMFORMER_SUMMARY minimum_wng_db={minimum_wng:.6} maximum_target_error={maximum_target_error:.9} maximum_directivity_loss_db={maximum_directivity_loss:.6}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
