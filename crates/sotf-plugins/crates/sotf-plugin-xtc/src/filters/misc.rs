use rustfft::num_complex::Complex;
#[cfg(test)]
use std::f32::consts::PI;

/// Speed of sound at 20°C in m/s
pub(crate) const SPEED_OF_SOUND: f32 = 343.0;

/// Replace any NaN or Inf values in filter coefficients with zero.
/// Prevents corrupted filter bins from producing distorted output.
pub(crate) fn sanitize_filter(filter: &mut [Complex<f32>]) {
    for c in filter.iter_mut() {
        if !c.re.is_finite() {
            c.re = 0.0;
        }
        if !c.im.is_finite() {
            c.im = 0.0;
        }
    }
}

/// Soft-limit a complex number's magnitude using tanh saturation.
///
/// Below 50% of max_mag: passthrough (no change).
/// Above 50%: smooth tanh curve approaching max_mag asymptotically.
/// Phase is always preserved — only magnitude is affected.
#[cfg(test)]
#[inline]
pub(crate) fn soft_limit_complex_magnitude(c: Complex<f32>, max_mag: f32) -> Complex<f32> {
    let mag = c.norm();
    let knee_start = max_mag * 0.5;

    if mag <= knee_start {
        return c;
    }

    let headroom = max_mag - knee_start;
    let excess = mag - knee_start;
    let new_mag = knee_start + headroom * (excess / headroom).tanh();

    c * (new_mag / mag)
}

/// Apply one soft gain to a two-speaker input column.
///
/// The largest coefficient sets the existing half-ceiling tanh knee. Sharing
/// its real gain preserves the complex speaker-drive ratio and therefore the
/// normalized ear response for any fixed linear plant. Preparation uses f64
/// so finite f32 extremes do not overflow magnitude calculations.
#[inline]
pub(super) fn soft_limit_column_f64(column: [Complex<f64>; 2], ceiling: f64) -> [Complex<f64>; 2] {
    let peak = column[0].norm().max(column[1].norm());
    let knee = ceiling * 0.5;
    if peak <= knee {
        return column;
    }
    let limited = knee + knee * ((peak - knee) / knee).tanh();
    let gain = limited / peak;
    column.map(|value| value * gain)
}

/// Limit a stored f32 filter column using widened preparation arithmetic.
#[inline]
pub(super) fn soft_limit_filter_column(
    column: [Complex<f32>; 2],
    ceiling: f32,
) -> [Complex<f32>; 2] {
    let widened = column.map(|value| Complex::new(f64::from(value.re), f64::from(value.im)));
    soft_limit_column_f64(widened, f64::from(ceiling))
        .map(|value| Complex::new(value.re as f32, value.im as f32))
}

/// Compute the Woodworth diffraction path around the head for a given incidence angle.
///
/// The sound reaching the far ear must diffract around the spherical head.
/// The extra path length depends on the angle of incidence (azimuth from median plane).
///
/// For angle <= PI/2: extra_path = a * (angle + sin(angle))
/// For angle > PI/2:  extra_path = a * (PI - angle + sin(angle))
#[cfg(test)]
#[inline]
pub(crate) fn woodworth_diffraction_path(angle_rad: f32, head_radius: f32) -> f32 {
    let theta = angle_rad.abs().min(PI);
    // Antipodal spherical ears have front/back symmetry. Fold the rear
    // hemisphere before evaluating the shorter arc, so rear median incidence
    // has zero ITD. Aaronson & Hartmann (2014), equations 1a/1b:
    // https://pmc.ncbi.nlm.nih.gov/articles/PMC3985894/
    let folded = if theta <= PI / 2.0 { theta } else { PI - theta };
    head_radius * (folded + folded.sin())
}

/// Compute the angular separation between a sound source and the contralateral ear.
///
/// For a source at azimuth `speaker_angle` from the median plane, the ipsilateral ear
/// (same side) is at 90° from center, and the contralateral ear (opposite side) is at
/// -90°. The angular separation from source to contralateral ear, measured around the
/// head surface, is approximately PI/2 + speaker_angle.
#[cfg(test)]
#[inline]
pub(crate) fn contralateral_shadow_angle(speaker_angle_rad: f32) -> f32 {
    (PI / 2.0 + speaker_angle_rad).min(PI)
}

/// Compute condition number of the 2x2 plant matrix C at a frequency bin.
///
/// For a 2x2 matrix, the condition number is σ_max / σ_min where σ are the
/// singular values. For [[a, b], [c, d]], the singular values can be computed
/// cheaply from the Frobenius norm and determinant.
#[inline]
pub(super) fn condition_number_2x2(
    h00: Complex<f32>,
    h01: Complex<f32>,
    h10: Complex<f32>,
    h11: Complex<f32>,
) -> f32 {
    // Use widened, scale-independent arithmetic. Subtracting the two close
    // eigenvalue roots in f32 made mirrored near-singular plants choose
    // different regularization strengths.
    let widen = |c: Complex<f32>| Complex::new(f64::from(c.re), f64::from(c.im));
    let [a, b, c, d] = [h00, h01, h10, h11].map(widen);
    let frobenius = a.norm_sqr() + b.norm_sqr() + c.norm_sqr() + d.norm_sqr();
    let determinant = (a * d - b * c).norm();
    if determinant == 0.0 || frobenius == 0.0 {
        return 1e6;
    }
    let discriminant = (frobenius * frobenius - 4.0 * determinant * determinant).max(0.0);
    let largest_squared = (frobenius + discriminant.sqrt()) * 0.5;
    // sigma_min = |det(C)| / sigma_max avoids cancellation in the small root.
    (largest_squared / determinant) as f32
}

/// Smooth sigmoid function for gradual transitions
#[inline]
pub(super) fn sigmoid_smooth(x: f32, width: f32) -> f32 {
    1.0 / (1.0 + (-x / width).exp())
}

#[cfg(test)]
mod rear_diffraction_tests {
    use super::*;

    #[test]
    fn antipodal_diffraction_matches_independent_interaural_projection() {
        let mut cases = 0;
        let mut failures = 0;
        let mut maximum_error = 0.0_f64;
        for radius in [0.07_f32, 0.0875, 0.11] {
            for quarter_degrees in 0..=720 {
                for sign in [-1.0_f32, 1.0] {
                    let angle = sign * (quarter_degrees as f32 * 0.25).to_radians();
                    // The source projection on the interaural axis determines
                    // the shorter spherical arc. This f64 oracle does not use
                    // the production front/rear branch.
                    let projection = f64::from(angle).sin().abs();
                    let expected = f64::from(radius) * (projection.asin() + projection);
                    let actual = woodworth_diffraction_path(angle, radius);
                    let error = (f64::from(actual) - expected).abs();
                    maximum_error = maximum_error.max(error);
                    failures += usize::from(error > 5e-8);
                    cases += 1;
                }
            }
            assert_eq!(woodworth_diffraction_path(0.0, radius), 0.0);
            let rear = woodworth_diffraction_path(PI, radius);
            println!(
                "rear_median radius={radius} extra_path_m={rear} delay_us={}",
                rear / 343.0 * 1e6
            );
        }
        println!(
            "rear_diffraction cases={cases} maximum_error_m={maximum_error} failed_values={failures}"
        );
        assert_eq!(failures, 0);
    }
}

#[cfg(test)]
mod condition_number_tests {
    use super::*;

    #[test]
    fn small_singular_value_and_scale_do_not_change_condition_number() {
        for epsilon in [0.01_f32, 0.0001, 0.000001] {
            let cross = 1.0 - epsilon;
            let expected = (1.0 + f64::from(cross)) / (1.0 - f64::from(cross));
            for exponent in [-40, 0, 40] {
                let scale = 2.0_f32.powi(exponent);
                let diagonal = Complex::new(scale, 0.0);
                let off_diagonal = Complex::new(cross * scale, 0.0);
                let actual = condition_number_2x2(diagonal, off_diagonal, off_diagonal, diagonal);
                let relative_error = (f64::from(actual) / expected - 1.0).abs();
                println!(
                    "condition epsilon={epsilon} scale_exponent={exponent} actual={actual} expected={expected} relative_error={relative_error}"
                );
                assert!(relative_error < 1e-5);
            }
        }
        assert_eq!(
            condition_number_2x2(
                Complex::new(1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(1.0, 0.0)
            ),
            1e6
        );
    }
}

#[cfg(test)]
mod coordinated_limit_tests {
    use super::*;

    #[test]
    fn column_limit_retains_legacy_peak_curve_and_quiet_identity() {
        let quiet = [Complex::new(0.125, -0.125), Complex::new(-0.25, 0.0)];
        assert_eq!(soft_limit_filter_column(quiet, 1.0), quiet);
        assert_eq!(
            soft_limit_filter_column([Complex::new(0.0, 0.0); 2], 1.0),
            [Complex::new(0.0, 0.0); 2]
        );
        let mut cases = 0;
        let mut maximum_curve_error = 0.0_f32;
        for ceiling in [0.125, 1.0, 4.0] {
            for phase in [-2.0_f32, -0.5, 0.0, 0.5, 2.0] {
                for step in 0..=2048 {
                    let peak = Complex::from_polar(ceiling * step as f32 / 256.0, phase);
                    let result =
                        soft_limit_filter_column([peak, peak * Complex::new(0.2, 0.1)], ceiling);
                    let legacy = soft_limit_complex_magnitude(peak, ceiling);
                    let error = (result[0] - legacy).norm();
                    maximum_curve_error = maximum_curve_error.max(error);
                    assert!(error <= ceiling * 5e-7);
                    assert!(result.iter().all(|c| c.norm() <= ceiling * 1.000001));
                    cases += 1;
                }
            }
        }
        for ceiling in [0.125, 1.0, 4.0] {
            let result = soft_limit_filter_column(
                [
                    Complex::new(f32::MAX, f32::MAX),
                    Complex::new(-f32::MAX, f32::MAX),
                ],
                ceiling,
            );
            assert!(
                result.iter().all(|c| c.re.is_finite()
                    && c.im.is_finite()
                    && c.norm() <= ceiling * 1.000001)
            );
        }
        println!("column_limit_curve cases={cases} maximum_curve_error={maximum_curve_error}");
    }
}
