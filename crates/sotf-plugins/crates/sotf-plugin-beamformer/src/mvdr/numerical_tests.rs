// Rust guideline compliant 2026-02-21
use super::{Complex, MvdrBeamformer};

#[test]
fn diagonal_covariance_weights_match_independent_wide_normalization() {
    let mut maximum_error = 0.0_f64;
    for mics in 2..=8 {
        for scale in [
            1e-30_f32, 1e-21, 1e-20, 2e-20, 1e-19, 1e-18, 1e-6, 1.0, 1e18,
        ] {
            let mut mvdr = MvdrBeamformer::new(mics, 1);
            let steering = vec![
                (0..mics)
                    .map(|mic| {
                        let angle = mic as f64 * 0.37;
                        Complex::new(angle.cos() as f32, angle.sin() as f32)
                    })
                    .collect::<Vec<_>>(),
            ];
            let diagonal: Vec<f32> = (0..mics)
                .map(|mic| scale * (1.0 + mic as f32 * 0.3))
                .collect();
            mvdr.noise_cov.fill(Complex::new(0.0, 0.0));
            for (mic, &value) in diagonal.iter().enumerate() {
                mvdr.noise_cov[mic * mics + mic] = Complex::new(value, 0.0);
            }
            let loading = diagonal.iter().map(|&x| f64::from(x)).sum::<f64>() * f64::from(0.01_f32)
                / mics as f64;
            let loaded: Vec<f64> = diagonal.iter().map(|&x| f64::from(x) + loading).collect();
            let denominator = steering[0]
                .iter()
                .zip(&loaded)
                .map(|(d, r)| (f64::from(d.re).powi(2) + f64::from(d.im).powi(2)) / r)
                .sum::<f64>();
            let fallback = loaded.iter().any(|&x| x <= f64::from(1e-20_f32))
                || denominator * denominator <= f64::from(1e-20_f32);
            let actual = &mvdr.compute_weights(&steering)[0];
            let mut target_response = Complex::new(0.0_f64, 0.0);
            for (mic, &weight) in actual.iter().enumerate() {
                assert!(
                    weight.re.is_finite() && weight.im.is_finite(),
                    "mics={mics}, scale={scale}"
                );
                let direction = Complex::new(
                    f64::from(steering[0][mic].re),
                    f64::from(steering[0][mic].im),
                );
                let expected = if fallback {
                    direction / mics as f64
                } else {
                    direction / (loaded[mic] * denominator)
                };
                let weight = Complex::new(f64::from(weight.re), f64::from(weight.im));
                let error = (weight - expected).norm();
                maximum_error = maximum_error.max(error);
                assert!(
                    error < 2e-6,
                    "mics={mics}, scale={scale}, mic={mic}, error={error}"
                );
                target_response += weight.conj() * direction;
            }
            assert!((target_response - Complex::new(1.0, 0.0)).norm() < 2e-6);
        }
    }
    eprintln!("MVDR independent diagonal normalization maximum error: {maximum_error:e}");
}

#[test]
fn invalid_covariance_replaces_the_entire_bin_with_steered_fallback() {
    let mut mvdr = MvdrBeamformer::new(4, 1);
    let steering = [vec![
        Complex::new(1.0, 0.0),
        Complex::new(0.0, 1.0),
        Complex::new(-1.0, 0.0),
        Complex::new(0.0, -1.0),
    ]];
    mvdr.weights_buf[0].fill(Complex::new(123.0, -456.0));
    mvdr.noise_cov[15].re = f32::INFINITY;
    assert_eq!(
        &mvdr.compute_weights(&steering)[0],
        &steering[0].iter().map(|d| *d * 0.25).collect::<Vec<_>>()
    );
}
