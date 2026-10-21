//! Regression gates for signed, bin-aligned XTC diagnostics.

// Rust guideline compliant 2026-02-21
use super::measure::{measure_cancellation_depth_db, measure_cancellation_depth_db_with_filters};
use crate::XtcPluginParams;
use crate::filters::XtcFilters;
use rustfft::num_complex::Complex;

#[test]
fn worsening_crosstalk_keeps_its_negative_sign() {
    let n = 1025;
    let filters = XtcFilters {
        filter_ll: vec![Complex::new(1.0, 0.0); n],
        filter_lr: vec![Complex::new(2.0, 0.0); n],
        filter_rl: None,
        filter_rr: None,
        is_symmetric: true,
        speaker_filters: None,
    };
    let actual = measure_cancellation_depth_db_with_filters(
        &filters,
        &XtcPluginParams::default(),
        48_000,
        937.5,
        n,
    );
    println!("signed_diagnostic depth_db={actual}");
    assert!(
        actual < 0.0,
        "a worsening filter must not be reported as zero improvement"
    );
}

#[test]
fn one_filter_bin_uses_one_acoustic_frequency() {
    let params = XtcPluginParams::default();
    let a = measure_cancellation_depth_db(&params, 48_000, 1000.25);
    let b = measure_cancellation_depth_db(&params, 48_000, 1003.0);
    println!("same_bin a={a} b={b}");
    assert_eq!(
        a, b,
        "the same selected filter bin must use the same plant frequency"
    );
}

#[test]
fn invalid_frequency_is_not_reported_as_cancellation() {
    let params = XtcPluginParams::default();
    for frequency in [f32::NAN, -1.0, f32::INFINITY, 24_001.0] {
        let value = measure_cancellation_depth_db(&params, 48_000, frequency);
        assert!(
            value.is_nan(),
            "invalid frequency {frequency} produced {value}"
        );
    }
}

use super::{ComplexTransferMatrix, evaluate_cancellation, predict_analytic_cancellation};

#[test]
fn supplied_complex_matrix_matches_closed_form_and_ear_permutation() {
    let h = [[[1.0, 0.0], [0.2, 0.1]], [[0.3, -0.4], [1.5, 0.0]]];
    let w = [[[0.8, 0.0], [-0.2, 0.0]], [[-0.1, 0.0], [0.9, 0.0]]];
    let m = evaluate_cancellation(h, w);
    // Explicit scalar products: T=[.78-.01i, -.02+.09i; .09-.32i, 1.29+.08i].
    let raw = [
        20.0 * (0.5 / 0.09_f64.hypot(0.32)).log10(),
        20.0 * (0.2_f64.hypot(0.1) / 0.02_f64.hypot(0.09)).log10(),
    ];
    let desired = [
        20.0 * 0.78_f64.hypot(0.01).log10(),
        20.0 * (1.29_f64.hypot(0.08) / 1.5).log10(),
    ];
    for i in 0..2 {
        assert!((m.raw_depth_db[i] - raw[i]).abs() < 1e-12);
        assert!((m.desired_gain_db[i] - desired[i]).abs() < 1e-12);
        assert!((m.separation_improvement_db[i] - raw[i] - desired[i]).abs() < 1e-12);
    }
    let swap = |matrix: ComplexTransferMatrix| {
        [[matrix[1][1], matrix[1][0]], [matrix[0][1], matrix[0][0]]]
    };
    let mirrored = evaluate_cancellation(swap(h), swap(w));
    assert_eq!(
        mirrored.raw_depth_db,
        [m.raw_depth_db[1], m.raw_depth_db[0]]
    );
}

#[test]
fn attenuation_and_muting_are_not_credited_as_separation() {
    let plant = [[[1.0, 0.0], [0.2, 0.0]], [[0.4, 0.0], [1.0, 0.0]]];
    let identity = [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]];
    assert_eq!(
        evaluate_cancellation(plant, identity).raw_depth_db,
        [0.0; 2]
    );
    let attenuated = identity.map(|r| r.map(|[a, b]| [a * 1e-4, b * 1e-4]));
    let m = evaluate_cancellation(plant, attenuated);
    for i in 0..2 {
        assert!((m.raw_depth_db[i] - 80.0).abs() < 1e-12);
        assert!((m.desired_gain_db[i] + 80.0).abs() < 1e-12);
        assert!(m.separation_improvement_db[i].abs() < 1e-12);
    }
    let mute = evaluate_cancellation(plant, [[[0.0; 2]; 2]; 2]);
    assert_eq!(mute.raw_depth_db, [f64::INFINITY; 2]);
    assert_eq!(mute.desired_gain_db, [f64::NEG_INFINITY; 2]);
    assert!(mute.separation_improvement_db.iter().all(|v| v.is_nan()));
}

#[test]
fn independent_metrics_retain_large_signed_values_and_scale_invariance() {
    let plant = [[[1.0, 0.0], [0.2, 0.0]], [[0.2, 0.0], [1.0, 0.0]]];
    for cross in [-0.2 + 1e-8, 2.0, 1e8] {
        let filter = [[[1.0, 0.0], [cross, 0.0]], [[cross, 0.0], [1.0, 0.0]]];
        let m = evaluate_cancellation(plant, filter);
        let reference = 20.0 * (0.2_f64 / (0.2 + cross).abs()).log10();
        for d in m.raw_depth_db {
            assert!((d - reference).abs() < 1e-7);
        }
        for scale in [1e-100, 1e100] {
            let scaled = plant.map(|r| r.map(|[a, b]| [a * scale, b * scale]));
            let other = evaluate_cancellation(scaled, filter);
            for i in 0..2 {
                assert!((other.raw_depth_db[i] - m.raw_depth_db[i]).abs() < 1e-6);
            }
        }
    }
    let bad = [[[1.0, 0.0], [0.0, 0.0]], [[2.0, 0.0], [1.0, 0.0]]];
    let m = evaluate_cancellation(plant, bad);
    assert!(m.raw_depth_db[0] < -10.0);
    assert_eq!(m.raw_depth_db[1], 0.0);
}

#[test]
fn invalid_matrices_and_resource_predictions_remain_explicit() {
    let identity = [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]];
    for value in [f64::NAN, f64::INFINITY] {
        let mut invalid = identity;
        invalid[0][0][0] = value;
        let m = evaluate_cancellation(invalid, identity);
        assert!(
            m.raw_depth_db
                .iter()
                .chain(&m.desired_gain_db)
                .all(|v| v.is_nan())
        );
    }
    for mode in 0..4 {
        let mut p = XtcPluginParams::default();
        match mode {
            0 => p.hrtf_file = Some("not-loaded.sofa".into()),
            1 => p.source_mode = "roomeq_recommended".into(),
            2 => p.room_reflections_enabled = true,
            _ => p.fft_size = 0,
        }
        assert!(predict_analytic_cancellation(&p, 48_000, 1000.0).is_none());
        assert!(measure_cancellation_depth_db(&p, 48_000, 1000.0).is_nan());
        assert!(!super::run_validation(&p, 48_000).all_passed());
    }
}

#[test]
fn prediction_uses_the_configured_grid_and_reports_both_inputs() {
    // A remembered, inactive RoomEQ artifact does not change synthetic mode.
    let dormant = XtcPluginParams {
        recommended_matrix_file: Some("not-loaded.json".into()),
        ..Default::default()
    };
    assert!(predict_analytic_cancellation(&dormant, 48_000, 1000.0).is_some());
    for rate in [44_100, 48_000, 96_000] {
        for fft_size in [128, 512, 2048, 4096, 16384] {
            let p = XtcPluginParams {
                fft_size,
                head_model: 1,
                head_yaw_deg: 20.0,
                beta_base: 0.05,
                ..Default::default()
            };
            let frequency = rate as f32 * 13.25 / fft_size as f32;
            let report = predict_analytic_cancellation(&p, rate, frequency).unwrap();
            assert_eq!(report.fft_bin, 13);
            assert_eq!(report.frequency_hz, 13.0 * (rate as f32 / fft_size as f32));
            assert!(report.metrics.raw_depth_db.iter().all(|v| v.is_finite()));
            assert_eq!(
                measure_cancellation_depth_db(&p, rate, frequency),
                report.metrics.raw_depth_db[0].min(report.metrics.raw_depth_db[1]) as f32
            );
        }
    }
}

#[test]
fn selected_brown_duda_model_matches_independent_complex_oracle() {
    use crate::filters::compute_xtc_filters_full;
    use rustfft::num_complex::Complex;
    let mut maximum_error = 0.0_f64;
    let mut cases = 0;
    for rate in [48_000, 96_000] {
        for yaw in [-20.0, 0.0, 20.0] {
            for x in [-0.04, 0.0, 0.04] {
                for mode in ["phase_only", "explicit_delay"] {
                    let p = XtcPluginParams {
                        head_model: 1,
                        itd_modeling: mode.into(),
                        head_yaw_deg: yaw,
                        head_offset_x: x,
                        beta_base: 0.2,
                        pinna_model_enabled: false,
                        ..Default::default()
                    };
                    let filters = compute_xtc_filters_full(&p, rate, 1025);
                    let theta = f64::from(p.speaker_angle_deg).to_radians();
                    let yaw = f64::from(p.head_yaw_deg).to_radians();
                    let radius = f64::from(p.head_radius_m);
                    let d = f64::from(p.distance_m + p.head_offset_z);
                    let distance = |angle: f64, offset: f64| {
                        ((d * angle.sin() + offset).powi(2) + (d * angle.cos()).powi(2)).sqrt()
                    };
                    let dl = distance(theta + yaw, f64::from(p.head_offset_x));
                    let dr = distance(theta - yaw, -f64::from(p.head_offset_x));
                    for bin in [8, 40, 80, 180, 340] {
                        let frequency = rate as f32 * bin as f32 / 2048.0;
                        let cross = |angle: f64, ipsi: f64, contra_ipsi: f64| {
                            let delta = radius * (angle.abs() + angle.abs().sin());
                            let contra = contra_ipsi + delta;
                            let mu = (std::f64::consts::TAU * f64::from(frequency) * radius
                                / 343.0)
                                .min(20.0);
                            let alpha = (1.0 + mu * mu / 4.0).sqrt().recip();
                            let incidence = (std::f64::consts::FRAC_PI_2 + angle.abs())
                                .min(std::f64::consts::PI);
                            let magnitude =
                                (alpha + (1.0 - alpha) * (incidence / 2.0).cos()) * ipsi / contra;
                            let path = Complex::from_polar(
                                1.0,
                                -std::f64::consts::TAU * f64::from(frequency) * (contra - ipsi)
                                    / 343.0,
                            );
                            let shadowed = path * magnitude;
                            if mode == "explicit_delay" {
                                let blend =
                                    1.0 / (1.0 + ((f64::from(frequency) - 300.0) / 50.0).exp());
                                path * blend + shadowed * (1.0 - blend)
                            } else {
                                shadowed
                            }
                        };
                        let (left, right) = if yaw == 0.0 && x == 0.0 {
                            let c = cross(theta, dl, dl);
                            (c, c)
                        } else {
                            (cross(theta - yaw, dl, dr), cross(theta + yaw, dr, dl))
                        };
                        let f64c = |c: Complex<f32>| Complex::new(f64::from(c.re), f64::from(c.im));
                        let ll = f64c(filters.filter_ll[bin]);
                        let lr = f64c(filters.filter_lr[bin]);
                        let rl =
                            f64c(filters.filter_rl.as_ref().unwrap_or(&filters.filter_lr)[bin]);
                        let rr =
                            f64c(filters.filter_rr.as_ref().unwrap_or(&filters.filter_ll)[bin]);
                        let expected = [
                            20.0 * (right.norm() / (right * ll + rl).norm()).log10(),
                            20.0 * (left.norm() / (lr + left * rr).norm()).log10(),
                        ];
                        let actual = predict_analytic_cancellation(&p, rate, frequency).unwrap();
                        for (a, e) in actual.metrics.raw_depth_db.iter().zip(expected) {
                            let error = (a - e).abs();
                            maximum_error = maximum_error.max(error);
                            assert!(
                                error < 0.02,
                                "rate={rate} yaw={yaw} x={x} bin={bin} error={error} actual={a} expected={e}"
                            );
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("independent_brown_duda_prediction cases={cases} maximum_db_error={maximum_error}");
}
