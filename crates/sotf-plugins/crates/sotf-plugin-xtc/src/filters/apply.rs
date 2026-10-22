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
}
