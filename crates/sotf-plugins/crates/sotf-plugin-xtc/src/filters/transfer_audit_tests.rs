//! Independent geometric plants, regularized solves, and mismatch measurements.

// Rust guideline compliant 2026-02-21
use super::compute_full_2x2_inverse;
use rustfft::num_complex::Complex;

type C = Complex<f64>;
type Matrix = [[C; 2]; 2];

fn plant(frequency: f64, head_x: f64, yaw: f64, reflection: f64) -> Matrix {
    // Independently defined point-source geometry: x right, y forward, 343 m/s.
    // This is a controlled free-field/one-echo test, not a measured human HRTF.
    std::array::from_fn(|ear| {
        std::array::from_fn(|speaker| {
            let side = if ear == 0 { -0.0875 } else { 0.0875 };
            let x = head_x + side * yaw.cos();
            let y = side * yaw.sin();
            let sx: f64 = if speaker == 0 { -1.0 } else { 1.0 };
            let sy = 3.0_f64.sqrt();
            let distance = ((sx - x).powi(2) + (sy - y).powi(2)).sqrt();
            let image_distance = ((sx - x).powi(2) + (sy + 2.0 + y).powi(2)).sqrt();
            C::from_polar(
                2.0 / distance,
                -std::f64::consts::TAU * frequency * distance / 343.0,
            ) + C::from_polar(
                reflection * 2.0 / image_distance,
                -std::f64::consts::TAU * frequency * image_distance / 343.0,
            )
        })
    })
}

fn reference_inverse(c: Matrix, beta: f64) -> Matrix {
    // Cholesky factorization and triangular solves, independent of production's
    // explicit determinant inverse and entirely in f64.
    let a: Matrix = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            (0..2).map(|ear| c[ear][i].conj() * c[ear][j]).sum::<C>()
                + C::new(if i == j { beta } else { 0.0 }, 0.0)
        })
    });
    let l00 = a[0][0].re.sqrt();
    let l10 = a[1][0] / l00;
    let l11 = (a[1][1].re - l10.norm_sqr()).sqrt();
    let columns: [[C; 2]; 2] = std::array::from_fn(|ear| {
        let y0 = c[ear][0].conj() / l00;
        let y1 = (c[ear][1].conj() - l10 * y0) / l11;
        let x1 = y1 / l11;
        let x0 = (y0 - l10.conj() * x1) / l00;
        [x0, x1]
    });
    [
        [columns[0][0], columns[1][0]],
        [columns[0][1], columns[1][1]],
    ]
}

fn production_inverse(c: Matrix, beta: f32, limit: f32) -> Matrix {
    let f = |x: C| Complex::<f32>::new(x.re as f32, x.im as f32);
    let (a, b, c, d) = compute_full_2x2_inverse(
        f(c[0][0]),
        f(c[0][1]),
        f(c[1][0]),
        f(c[1][1]),
        beta,
        limit,
        true,
    );
    let d64 = |x: Complex<f32>| C::new(f64::from(x.re), f64::from(x.im));
    [[d64(a), d64(b)], [d64(c), d64(d)]]
}

fn multiply(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..2).map(|k| a[i][k] * b[k][j]).sum()))
}

#[test]
fn regularized_inverse_matches_independent_cholesky_plants() {
    let mut maximum_error = 0.0_f64;
    let mut cases = 0;
    for frequency in [100.0, 250.0, 500.0, 1000.0, 4000.0, 8000.0] {
        for x in [-0.1, 0.0, 0.1] {
            for yaw in [-0.3, 0.0, 0.3] {
                for reflection in [0.0, 0.4] {
                    for beta in [0.01_f32, 0.1, 1.0] {
                        let c = plant(frequency, x, yaw, reflection);
                        let expected = reference_inverse(c, f64::from(beta));
                        // A high limit keeps this comparison below soft limiting.
                        let actual = production_inverse(c, beta, 1000.0);
                        for (actual, expected) in actual
                            .into_iter()
                            .flatten()
                            .zip(expected.into_iter().flatten())
                        {
                            let error = (actual - expected).norm();
                            maximum_error = maximum_error.max(error);
                            assert!(
                                error < 2e-4,
                                "inverse error={error} frequency={frequency} x={x} yaw={yaw} beta={beta}"
                            );
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("independent_inverse cases={cases} maximum_complex_error={maximum_error:e}");
}

#[test]
fn signed_displacement_and_room_mismatch_are_not_hidden() {
    let mut worst_depth = f64::INFINITY;
    let mut best_depth = f64::NEG_INFINITY;
    let mut maximum_gain = 0.0_f64;
    let mut degraded = 0;
    let mut cases = 0;
    for frequency in [100.0, 250.0, 500.0, 1000.0, 4000.0, 8000.0] {
        let design = plant(frequency, 0.0, 0.0, 0.0);
        let inverse = production_inverse(design, 0.01, 4.0);
        for coefficient in inverse.into_iter().flatten() {
            maximum_gain = maximum_gain.max(coefficient.norm());
            assert!(coefficient.norm() <= 4.000001);
        }
        for x in [-0.1, -0.02, 0.0, 0.02, 0.1] {
            for yaw in [-0.3, 0.0, 0.3] {
                for reflection in [0.0, 0.4] {
                    let actual = plant(frequency, x, yaw, reflection);
                    let corrected = multiply(actual, inverse);
                    for (ear, (actual_row, corrected_row)) in
                        actual.iter().zip(&corrected).enumerate()
                    {
                        let target = 1 - ear;
                        let depth = 20.0
                            * (actual_row[target].norm() / corrected_row[target].norm().max(1e-30))
                                .log10();
                        println!(
                            "{}",
                            serde_json::json!({
                                "measurement":"independent_xtc_mismatch", "frequency_hz":frequency,
                                "head_x_m":x, "yaw_rad":yaw, "reflection_gain":reflection,
                                "ear":ear, "signed_depth_db":depth,
                                "baseline_crosstalk":actual_row[target].norm(),
                                "residual_crosstalk":corrected_row[target].norm(),
                                "intended_gain":corrected_row[ear].norm()
                            })
                        );
                        assert!(depth.is_finite());
                        if depth < 0.0 {
                            degraded += 1;
                        }
                        worst_depth = worst_depth.min(depth);
                        best_depth = best_depth.max(depth);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert!(
        degraded > 0,
        "mismatch must not be reported as uniformly successful cancellation"
    );
    println!(
        "independent_mismatch ear_cases={cases} degraded={degraded} min_signed_depth_db={worst_depth} max_signed_depth_db={best_depth} max_filter_gain={maximum_gain}"
    );
}

#[test]
fn inverse_gain_limiting_preserves_each_input_steering_ray() {
    let mut columns = 0;
    let mut failures = 0;
    let mut limited = 0;
    let mut maximum_error = 0.0_f64;
    for frequency in [100.0, 250.0, 500.0, 1000.0, 4000.0, 8000.0] {
        for x in [-0.1, 0.0, 0.1] {
            for yaw in [-0.3, 0.0, 0.3] {
                for reflection in [0.0, 0.4] {
                    for beta in [0.00001_f32, 0.001, 0.1] {
                        let c = plant(frequency, x, yaw, reflection);
                        let expected = reference_inverse(c, f64::from(beta));
                        for ceiling in [0.5_f32, 1.0, 4.0] {
                            let actual = production_inverse(c, beta, ceiling);
                            for column in 0..2 {
                                let a = [expected[0][column], expected[1][column]];
                                let b = [actual[0][column], actual[1][column]];
                                let norm = (a[0].norm_sqr() + a[1].norm_sqr()).sqrt()
                                    * (b[0].norm_sqr() + b[1].norm_sqr()).sqrt();
                                assert!(norm > 0.0);
                                // A common real gain must preserve the complex
                                // two-speaker ray, hence every plant's normalized
                                // desired/cross-ear ratio for that input cue.
                                let error = (a[0] * b[1] - a[1] * b[0]).norm() / norm;
                                maximum_error = maximum_error.max(error);
                                failures += usize::from(error > 2e-5);
                                limited += usize::from(
                                    a[0].norm().max(a[1].norm()) > f64::from(ceiling) * 0.5,
                                );
                                assert!(
                                    b.iter().all(|v| v.norm() <= f64::from(ceiling) * 1.000001)
                                );
                                columns += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "inverse_steering columns={columns} limited_columns={limited} failed_columns={failures} maximum_ray_error={maximum_error}"
    );
    assert!(limited > 0);
    assert_eq!(failures, 0);
}
