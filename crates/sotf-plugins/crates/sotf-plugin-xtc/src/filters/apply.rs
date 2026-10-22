use super::super::config::XtcPluginParams;
use rustfft::num_complex::Complex;

/// Apply frequency-dependent beta boosts (low/high) to the base beta value.
#[inline]
pub(super) fn apply_beta_freq_boosts(beta: f32, freq: f32, params: &XtcPluginParams) -> f32 {
    let low_factor =
        1.0 + params.beta_low_freq_boost * (1.0 / (1.0 + (-(100.0 - freq) / 30.0).exp()));
    let high_factor =
        1.0 + params.beta_high_freq_boost * (1.0 / (1.0 + (-(freq - 12000.0) / 1500.0).exp()));
    beta * low_factor * high_factor
}

/// Normalize the desired-ear magnitudes of a prepared two-input filter.
///
/// Apply the existing 90% correction with its 0.5..4 target clamp and tiny
/// response skip. A single real gain preserves each column's steering. The
/// coefficient ceiling caps that gain without reapplying the inverse soft knee;
/// the final common matrix effort cap still owns the loudspeaker-row budget.
pub(super) fn normalize_filter_columns(
    plant: [[Complex<f32>; 2]; 2],
    mut filters: [[Complex<f32>; 2]; 2],
    ceiling: f32,
) -> [[Complex<f32>; 2]; 2] {
    let wide = |value: Complex<f32>| Complex::new(f64::from(value.re), f64::from(value.im));
    for column in 0..2 {
        let input = [wide(filters[0][column]), wide(filters[1][column])];
        let response = input[0] * wide(plant[column][0]) + input[1] * wide(plant[column][1]);
        let magnitude = response.norm();
        let requested_gain = if magnitude > f64::from(0.01_f32) {
            1.0 + 0.9 * ((1.0 / magnitude).clamp(0.5, 4.0) - 1.0)
        } else {
            1.0
        };
        let peak = input[0].norm().max(input[1].norm());
        // The inverse already applied its soft knee. Cap only the correction
        // gain here; compressing again would move a unity response away from
        // unity, even when the requested correction was exactly one.
        let gain = if peak > 0.0 {
            requested_gain.min(f64::from(ceiling) / peak)
        } else {
            requested_gain
        };
        let corrected = input.map(|value| value * gain);
        filters[0][column] = Complex::new(corrected[0].re as f32, corrected[0].im as f32);
        filters[1][column] = Complex::new(corrected[1].re as f32, corrected[1].im as f32);
    }
    filters
}

/// Limit loudspeaker effort independently at each frequency.
///
/// A coefficient-wise limit still permits a two-input output row to carry
/// twice the configured power. This enforces `Σ_input |W[out,input](f)|² <=
/// max_gain_linear²` for each loudspeaker row, which has a physical source-power
/// interpretation and remains effective after column limiting. One common
/// matrix gain is set by the larger row power, preserving both input columns'
/// speaker ratios. A quieter row may retain unused power headroom.
pub(crate) fn apply_effort_constraint(
    filter_ll: &mut [Complex<f32>],
    filter_lr: &mut [Complex<f32>],
    filter_rl: Option<&mut [Complex<f32>]>,
    filter_rr: Option<&mut [Complex<f32>]>,
    max_gain_linear: f32,
) {
    let max_power = f64::from(max_gain_linear).powi(2);
    let power = |value: Complex<f32>| f64::from(value.re).powi(2) + f64::from(value.im).powi(2);
    let scaled = |value: Complex<f32>, gain: f64| {
        Complex::new(
            (f64::from(value.re) * gain) as f32,
            (f64::from(value.im) * gain) as f32,
        )
    };
    if let (Some(rl), Some(rr)) = (filter_rl, filter_rr) {
        for (((ll, lr), rl), rr) in filter_ll.iter_mut().zip(filter_lr).zip(rl).zip(rr) {
            let largest_power = (power(*ll) + power(*lr)).max(power(*rl) + power(*rr));
            if largest_power > max_power && largest_power.is_finite() {
                let gain = (max_power / largest_power).sqrt();
                *ll = scaled(*ll, gain);
                *lr = scaled(*lr, gain);
                *rl = scaled(*rl, gain);
                *rr = scaled(*rr, gain);
            }
        }
    } else {
        // The implicit second row in a symmetric matrix has the same power.
        for (ll, lr) in filter_ll.iter_mut().zip(filter_lr) {
            let row_power = power(*ll) + power(*lr);
            if row_power > max_power && row_power.is_finite() {
                let gain = (max_power / row_power).sqrt();
                *ll = scaled(*ll, gain);
                *lr = scaled(*lr, gain);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex;

    #[test]
    fn test_apply_effort_constraint_scales() {
        let mut ll = vec![Complex::new(10.0, 0.0); 10];
        let mut lr = vec![Complex::new(10.0, 0.0); 10];
        apply_effort_constraint(&mut ll, &mut lr, None, None, 1.0);
        assert!(ll[0].norm() < 10.0);
        assert!(lr[0].norm() < 10.0);
        assert!((ll[0].norm_sqr() + lr[0].norm_sqr() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_apply_effort_constraint_no_scale() {
        let mut ll = vec![Complex::new(0.1, 0.0); 10];
        let mut lr = vec![Complex::new(0.1, 0.0); 10];
        let original = ll[0].norm();
        apply_effort_constraint(&mut ll, &mut lr, None, None, 10.0);
        assert!((ll[0].norm() - original).abs() < 0.001);
    }

    #[test]
    fn test_apply_beta_freq_boosts() {
        let params = crate::config::XtcPluginParams::default();
        let beta = 0.01;
        let low = apply_beta_freq_boosts(beta, 50.0, &params);
        let mid = apply_beta_freq_boosts(beta, 1000.0, &params);
        let high = apply_beta_freq_boosts(beta, 20000.0, &params);
        assert!(low >= beta);
        assert!(high >= beta);
        assert!((mid - beta).abs() < 0.001);
    }
    #[test]
    fn effort_cap_preserves_both_steering_rays_and_handles_finite_extremes() {
        let original = [
            [Complex::new(8.0_f32, 1.0), Complex::new(0.4, -0.3)],
            [Complex::new(-2.0, 0.5), Complex::new(1.0, -0.1)],
        ];
        let wide = |c: Complex<f32>| Complex::new(f64::from(c.re), f64::from(c.im));
        let mut columns = 0;
        let mut failures = 0;
        let mut budget_failures = 0;
        let mut maximum_error = 0.0_f64;
        for scale in [1.0, 64.0, f32::MAX / 16.0] {
            for ceiling in [0.125_f32, 1.0, 4.0] {
                for swap_rows in [false, true] {
                    for swap_columns in [false, true] {
                        let matrix: [[Complex<f32>; 2]; 2] = std::array::from_fn(|row| {
                            std::array::from_fn(|col| {
                                original[row ^ usize::from(swap_rows)]
                                    [col ^ usize::from(swap_columns)]
                                    * scale
                            })
                        });
                        let mut ll = [matrix[0][0]];
                        let mut lr = [matrix[0][1]];
                        let mut rl = [matrix[1][0]];
                        let mut rr = [matrix[1][1]];
                        apply_effort_constraint(
                            &mut ll,
                            &mut lr,
                            Some(&mut rl),
                            Some(&mut rr),
                            ceiling,
                        );
                        let result = [[ll[0], lr[0]], [rl[0], rr[0]]];
                        for row in result {
                            let power: f64 = row.into_iter().map(|v| wide(v).norm_sqr()).sum();
                            budget_failures += usize::from(
                                !power.is_finite() || power > f64::from(ceiling).powi(2) * 1.000002,
                            );
                        }
                        for col in 0..2 {
                            let a = [wide(matrix[0][col]), wide(matrix[1][col])];
                            let b = [wide(result[0][col]), wide(result[1][col])];
                            let norm = (a[0].norm_sqr() + a[1].norm_sqr()).sqrt()
                                * (b[0].norm_sqr() + b[1].norm_sqr()).sqrt();
                            assert!(norm > 0.0);
                            let error = (a[0] * b[1] - a[1] * b[0]).norm() / norm;
                            maximum_error = maximum_error.max(error);
                            failures += usize::from(error > 2e-6);
                            columns += 1;
                        }
                    }
                }
            }
        }
        println!(
            "effort_steering columns={columns} failed_columns={failures} budget_failures={budget_failures} maximum_ray_error={maximum_error}"
        );
        assert_eq!(failures, 0);
        assert_eq!(budget_failures, 0);
    }
    #[test]
    fn normalization_keeps_unity_fixed_points_and_tiny_response_skips() {
        let zero = Complex::new(0.0, 0.0);
        let matrices = [
            (
                [
                    [Complex::new(0.25, 0.0), zero],
                    [zero, Complex::new(0.5, 0.0)],
                ],
                [
                    [Complex::new(4.0, 0.0), zero],
                    [zero, Complex::new(2.0, 0.0)],
                ],
            ),
            (
                [
                    [Complex::new(2.0 / 7.0, 0.0), Complex::new(1.0 / 7.0, 0.0)],
                    [Complex::new(-1.0 / 7.0, 0.0), Complex::new(3.0 / 7.0, 0.0)],
                ],
                [
                    [Complex::new(3.0, 0.0), Complex::new(-1.0, 0.0)],
                    [Complex::new(1.0, 0.0), Complex::new(2.0, 0.0)],
                ],
            ),
        ];
        let mut cases = 0;
        let mut failures = 0;
        let mut maximum_error = 0.0_f32;
        for (plant, filters) in matrices {
            for phase in [-1.0_f32, 0.0, 1.0] {
                let phases = [
                    Complex::from_polar(1.0, phase),
                    Complex::from_polar(1.0, -phase),
                ];
                let plant =
                    std::array::from_fn(|row| plant[row].map(|value| value * phases[row].conj()));
                let filters = filters.map(|row| [row[0] * phases[0], row[1] * phases[1]]);
                let result = normalize_filter_columns(plant, filters, 4.0);
                for (actual, expected) in result
                    .into_iter()
                    .flatten()
                    .zip(filters.into_iter().flatten())
                {
                    let error = (actual - expected).norm();
                    maximum_error = maximum_error.max(error);
                    failures += usize::from(error > 2e-6);
                    cases += 1;
                }
            }
        }
        let skipped = matrices[1].1;
        let tiny = [[zero; 2]; 2];
        let result = normalize_filter_columns(tiny, skipped, 4.0);
        for (actual, expected) in result
            .into_iter()
            .flatten()
            .zip(skipped.into_iter().flatten())
        {
            let error = (actual - expected).norm();
            maximum_error = maximum_error.max(error);
            failures += usize::from(error > 2e-6);
            cases += 1;
        }
        assert_eq!(normalize_filter_columns(tiny, tiny, 4.0), tiny);
        println!(
            "normalization_fixed_points coefficients={cases} failures={failures} maximum_error={maximum_error}"
        );
        assert_eq!(failures, 0);
    }

    #[test]
    fn normalization_moves_bounded_columns_toward_unity_without_turning_them() {
        let base = [
            [
                Complex::new(2.0_f32 / 7.0, 0.0),
                Complex::new(1.0 / 7.0, 0.0),
            ],
            [Complex::new(-1.0 / 7.0, 0.0), Complex::new(3.0 / 7.0, 0.0)],
        ];
        let filters = [
            [Complex::new(3.0, 0.0), Complex::new(-1.0, 0.0)],
            [Complex::new(1.0, 0.0), Complex::new(2.0, 0.0)],
        ];
        let wide = |c: Complex<f32>| Complex::new(f64::from(c.re), f64::from(c.im));
        let mut failures = 0;
        let mut cases = 0;
        let mut maximum_worsening = 0.0_f64;
        for level in [0.05_f32, 0.2, 0.6, 0.98, 0.99, 1.0, 1.01, 1.02, 1.8, 4.0] {
            let plant = base.map(|row| row.map(|c| c * level));
            let result = normalize_filter_columns(plant, filters, 4.0);
            for column in 0..2 {
                let response = |w: [[Complex<f32>; 2]; 2]| {
                    (wide(plant[column][0]) * wide(w[0][column])
                        + wide(plant[column][1]) * wide(w[1][column]))
                    .norm()
                };
                let before = response(filters);
                let after = response(result);
                let worsening = (after - 1.0).abs() - (before - 1.0).abs();
                maximum_worsening = maximum_worsening.max(worsening);
                failures += usize::from(worsening > 2e-6);
                let a = [wide(filters[0][column]), wide(filters[1][column])];
                let b = [wide(result[0][column]), wide(result[1][column])];
                let denominator = (a[0].norm_sqr() + a[1].norm_sqr()).sqrt()
                    * (b[0].norm_sqr() + b[1].norm_sqr()).sqrt();
                assert!((a[0] * b[1] - a[1] * b[0]).norm() / denominator < 2e-6);
                assert!(b.iter().all(|v| v.norm() <= 4.000004));
                println!(
                    "normalization_gain level={level} column={column} before={before} after={after} worsening={worsening}"
                );
                cases += 1;
            }
        }
        println!(
            "normalization_progress columns={cases} failures={failures} maximum_worsening={maximum_worsening}"
        );
        assert_eq!(failures, 0);
    }
}
