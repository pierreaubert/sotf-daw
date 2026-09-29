// Rust guideline compliant 2026-02-21
use super::*;

fn steering(mics: usize, bins: usize) -> Vec<Vec<Complex<f32>>> {
    vec![vec![Complex::new(1.0, 0.0); mics]; bins]
}

#[test]
fn unrepresentable_bin_preserves_complete_history_and_updates_valid_neighbors() {
    let directions = steering(2, 3);
    let mut core = MvdrBeamformer::new(2, 3);
    let ordinary = vec![
        vec![Complex::new(0.25, 0.125); 3],
        vec![Complex::new(-0.25, -0.125); 3],
    ];
    core.update_noise_covariance(&ordinary, &directions);
    core.compute_weights(&directions);
    let before = core.noise_cov.clone();
    let mut hot = ordinary.clone();
    hot[0][1] = Complex::new(1e20, 0.0);
    hot[1][1] = Complex::new(-1e20, 0.0);
    assert!(core.update_noise_covariance(&hot, &directions));
    assert_eq!(&core.noise_cov[4..8], &before[4..8]);
    assert_ne!(&core.noise_cov[..4], &before[..4]);
    assert_ne!(&core.noise_cov[8..], &before[8..]);
    assert!(
        core.noise_cov
            .iter()
            .all(|v| v.re.is_finite() && v.im.is_finite())
    );
    assert!(core.weights_dirty());
    core.compute_weights(&directions);
    assert!(
        core.weights_buf
            .iter()
            .flatten()
            .all(|v| v.re.is_finite() && v.im.is_finite())
    );

    for channel in &mut hot {
        let sample = channel[1];
        channel.fill(sample);
    }
    let before = core.noise_cov.clone();
    assert!(!core.update_noise_covariance(&hot, &directions));
    assert!(!core.weights_dirty());
    assert_eq!(core.noise_cov, before);
}

#[test]
fn wide_retry_matches_independent_complex_covariance_recurrence() {
    for mics in [2, 4, 8] {
        let mut core = MvdrBeamformer::new(mics, 1);
        let directions = steering(mics, 1);
        let samples: Vec<_> = (0..mics)
            .map(|i| {
                vec![Complex::new(
                    if i % 2 == 0 { 2e19 } else { -2e19 },
                    if i % 2 == 0 { 1e19 } else { -1e19 },
                )]
            })
            .collect();
        let before = core.noise_cov.clone();
        assert!(core.update_noise_covariance(&samples, &directions));
        for i in 0..mics {
            for j in 0..mics {
                let x = samples[i][0];
                let y = samples[j][0];
                let previous = before[i * mics + j];
                // Direct real arithmetic, independent of Complex multiplication.
                let a = f64::from(core.alpha);
                let b = f64::from(1.0_f32 - core.alpha);
                let expected = [
                    (a * f64::from(previous.re)
                        + b * (f64::from(x.re) * f64::from(y.re)
                            + f64::from(x.im) * f64::from(y.im))) as f32,
                    (a * f64::from(previous.im)
                        + b * (f64::from(x.im) * f64::from(y.re)
                            - f64::from(x.re) * f64::from(y.im))) as f32,
                ];
                let actual = core.noise_cov[i * mics + j];
                assert_eq!([actual.re, actual.im], expected);
                assert!(actual.re.is_finite() && actual.im.is_finite());
            }
        }
    }
}

#[test]
fn detector_classification_matches_wide_power_ratio_across_scales() {
    for amplitude in [0.001_f32, 1.0, 1e10, 2e19] {
        for second in [-1.0_f32, 0.0, 0.1, 0.2, 1.0] {
            let x = amplitude;
            let y = amplitude * second;
            let coherent = (f64::from(x) + f64::from(y)).powi(2)
                / (2.0 * (f64::from(x).powi(2) + f64::from(y).powi(2)));
            let expected = coherent < f64::from(0.65_f32);
            let mut core = MvdrBeamformer::new(2, 3);
            let input = vec![vec![Complex::new(x, 0.0); 3], vec![Complex::new(y, 0.0); 3]];
            assert_eq!(
                core.update_noise_covariance(&input, &steering(2, 3)),
                expected,
                "amplitude={amplitude},second={second},fraction={coherent}"
            );
        }
    }
}

#[test]
fn invalid_fft_frame_preserves_history_and_next_finite_frame_learns() {
    let directions = steering(2, 3);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut core = MvdrBeamformer::new(2, 3);
        core.compute_weights(&directions);
        let before = core.noise_cov.clone();
        let mut samples = vec![
            vec![Complex::new(0.25, 0.0); 3],
            vec![Complex::new(-0.25, 0.0); 3],
        ];
        samples[1][2].im = invalid;
        assert!(!core.update_noise_covariance(&samples, &directions));
        assert_eq!(core.noise_cov, before);
        assert!(!core.weights_dirty());
        samples[1][2].im = 0.0;
        assert!(core.update_noise_covariance(&samples, &directions));
        assert_ne!(core.noise_cov, before);
    }
}

#[test]
fn finite_overload_does_not_permanently_force_delay_and_sum() {
    let directions = steering(2, 2);
    let mut hot = MvdrBeamformer::new(2, 2);
    let mut reference = MvdrBeamformer::new(2, 2);
    let overload = vec![
        vec![Complex::new(1e20, 0.0); 2],
        vec![Complex::new(-1e20, 0.0); 2],
    ];
    hot.update_noise_covariance(&overload, &directions);
    let ordinary = vec![
        vec![Complex::new(0.25, 0.0); 2],
        vec![Complex::new(0.0, 0.0); 2],
    ];
    for _ in 0..4096 {
        assert!(hot.update_noise_covariance(&ordinary, &directions));
        reference.update_noise_covariance(&ordinary, &directions);
    }
    assert_eq!(
        hot.compute_weights(&directions),
        reference.compute_weights(&directions)
    );
    assert!(hot.weights_buf[0][0].re < 0.005);
    assert!(hot.weights_buf[0][1].re > 0.99);
}

#[test]
fn ordinary_covariance_matches_original_f32_recurrence_exactly() {
    for mics in 2..=8 {
        let bins = 5;
        let mut core = MvdrBeamformer::new(mics, bins);
        let directions = steering(mics, bins);
        let mut expected = core.noise_cov.clone();
        let mut samples = vec![vec![Complex::new(0.0, 0.0); bins]; mics];
        for frame in 0..400 {
            for mic in 0..(mics / 2) * 2 {
                for bin in 0..bins {
                    let sign = if mic % 2 == 0 { 1.0 } else { -1.0 };
                    samples[mic][bin] = Complex::new(
                        sign * ((frame * 17 + bin * 3 + mic / 2) % 113) as f32 / 64.0,
                        sign * ((frame * 11 + bin * 7 + mic / 2) % 97) as f32 / 128.0,
                    );
                }
            }
            for bin in 0..bins {
                for i in 0..mics {
                    for j in 0..mics {
                        let outer = samples[i][bin] * samples[j][bin].conj();
                        let previous = &mut expected[bin * mics * mics + i * mics + j];
                        *previous = Complex::new(
                            previous.re * 0.95 + outer.re * (1.0_f32 - 0.95),
                            previous.im * 0.95 + outer.im * (1.0_f32 - 0.95),
                        );
                    }
                }
            }
            assert!(core.update_noise_covariance(&samples, &directions));
            assert_eq!(core.noise_cov, expected, "mics={mics},frame={frame}");
        }
    }
}

#[test]
fn representable_overload_keeps_the_existing_smoothing_decay() {
    let mut core = MvdrBeamformer::new(2, 1);
    let directions = steering(2, 1);
    let overload = vec![
        vec![Complex::new(2e19, 0.0)],
        vec![Complex::new(-2e19, 0.0)],
    ];
    assert!(core.update_noise_covariance(&overload, &directions));
    let initial = f64::from(core.noise_cov[0].re);
    assert!(initial > 1e37 && initial < f64::from(f32::MAX));
    let zero = vec![vec![Complex::new(0.0, 0.0)]; 2];
    for hop in 1..=2048 {
        assert!(core.update_noise_covariance(&zero, &directions));
        let expected = initial * f64::from(0.95_f32).powi(hop);
        let actual = f64::from(core.noise_cov[0].re);
        assert!(
            (actual / expected - 1.0).abs() < 5e-6,
            "hop={hop}: {actual} != {expected}"
        );
        assert!(
            core.noise_cov
                .iter()
                .all(|v| v.re.is_finite() && v.im.is_finite())
        );
    }
    assert!(core.noise_cov[0].re < 1e-8);
}
