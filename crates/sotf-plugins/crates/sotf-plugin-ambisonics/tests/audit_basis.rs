//! Independent SN3D polynomial, orthogonality and polar-direction checks.

// Rust guideline compliant 2026-02-21
use sotf_plugin_ambisonics::spherical_harmonics::{
    acn_to_degree_index, spherical_harmonics_vector,
};

fn factorial(n: usize) -> f64 {
    (1..=n).map(|x| x as f64).product()
}

// Explicit differentiated Rodrigues polynomial, rather than the production
// associated-Legendre recurrence. cos(elevation)^|m| preserves polar precision.
fn basis(degree: usize, index: i32, az: f64, el: f64) -> f64 {
    let m = index.unsigned_abs() as usize;
    let mut derivative = 0.0;
    for k in 0..=degree / 2 {
        let power = degree - 2 * k;
        if power < m {
            continue;
        }
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        derivative += sign * factorial(2 * degree - 2 * k)
            / (2.0_f64.powi(degree as i32)
                * factorial(k)
                * factorial(degree - k)
                * factorial(power - m))
            * el.sin().powi((power - m) as i32);
    }
    let normalization =
        ((if m == 0 { 1.0 } else { 2.0 }) * factorial(degree - m) / factorial(degree + m)).sqrt();
    let azimuth = if index < 0 {
        (m as f64 * az).sin()
    } else {
        (m as f64 * az).cos()
    };
    normalization * derivative * el.cos().abs().powi(m as i32) * azimuth
}

#[test]
fn all_acn_sn3d_basis_values_match_independent_polynomials_including_near_poles() {
    let mut worst = 0.0_f64;
    for az in [-3.1, -1.7, 0.0, 0.3, 2.8] {
        for el in [
            -std::f64::consts::FRAC_PI_2 + 1e-9,
            -1.2,
            -0.4,
            0.0,
            0.7,
            1.3,
            std::f64::consts::FRAC_PI_2 - 1e-9,
        ] {
            let mut actual = [0.0; 64];
            spherical_harmonics_vector(7, az, el, &mut actual);
            for degree in 0..=7 {
                let mut norm = 0.0;
                for index in -(degree as i32)..=degree as i32 {
                    let acn = (degree * degree + degree) as i32 + index;
                    assert_eq!(acn_to_degree_index(acn as usize), (degree as i32, index));
                    let expected = basis(degree, index, az, el);
                    let error = (actual[acn as usize] - expected).abs();
                    worst = worst.max(error);
                    assert!(
                        error < 2e-11,
                        "degree={degree} index={index} az={az} el={el}: actual {} expected {expected}, error {error}",
                        actual[acn as usize]
                    );
                    norm += actual[acn as usize].powi(2);
                }
                assert!(
                    (norm - 1.0).abs() < 2e-12,
                    "SN3D addition theorem degree={degree}: {norm}"
                );
            }
        }
    }
    eprintln!("independent SN3D polynomial maximum error: {worst}");
}

#[test]
fn sn3d_gram_matrix_has_the_analytic_full_sphere_normalization() {
    // Eight-point Gauss-Legendre elevation quadrature and 16 uniform azimuths
    // integrate every degree<=14 product exactly (order-7 basis inner products).
    let nodes = [
        0.1834346424956498,
        0.525532409916329,
        0.7966664774136267,
        0.9602898564975363,
    ];
    let weights = [
        0.362683783378362,
        0.3137066458778873,
        0.2223810344533745,
        0.1012285362903763,
    ];
    let mut gram = vec![0.0_f64; 64 * 64];
    for (node, weight) in nodes.into_iter().zip(weights) {
        for sign in [-1.0_f64, 1.0] {
            let el = (sign * node).asin();
            for j in 0..16 {
                let az = std::f64::consts::TAU * f64::from(j) / 16.0;
                let mut y = [0.0; 64];
                spherical_harmonics_vector(7, az, el, &mut y);
                for row in 0..64 {
                    for col in 0..64 {
                        gram[row * 64 + col] += y[row] * y[col] * weight / 32.0;
                    }
                }
            }
        }
    }
    for row in 0..64 {
        let degree = (row as f64).sqrt().floor() as usize;
        for col in 0..64 {
            let expected = if row == col {
                1.0 / (2 * degree + 1) as f64
            } else {
                0.0
            };
            assert!(
                (gram[row * 64 + col] - expected).abs() < 2e-12,
                "Gram[{row},{col}]={}",
                gram[row * 64 + col]
            );
        }
    }
}
