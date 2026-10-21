//! Lateral/yaw geometry and low-frequency plant-mode regressions.

// Rust guideline compliant 2026-02-21
use super::*;

#[test]
fn lateral_only_pose_requires_distinct_left_and_right_filters() {
    for x in [-0.1, 0.1] {
        let params = XtcPluginParams {
            head_offset_x: x,
            ..Default::default()
        };
        let cache = compute_geometry_cache(&params, 48_000, 1025);
        let filters = compute_xtc_filters_full(&params, 48_000, 1025);
        println!(
            "lateral_pose x={x} asymmetric={} symmetric_filters={}",
            cache.asymmetric.is_some(),
            filters.is_symmetric
        );
        assert!(
            cache.asymmetric.is_some(),
            "lateral displacement is not a symmetric plant"
        );
        assert!(!filters.is_symmetric);
        assert!(filters.filter_rl.is_some() && filters.filter_rr.is_some());
    }
}

#[test]
fn positive_x_moves_toward_the_right_speaker() {
    let params = XtcPluginParams {
        head_offset_x: 0.1,
        head_yaw_deg: 15.0,
        ..Default::default()
    };
    let cache = compute_geometry_cache(&params, 48_000, 1025);
    let geometry = cache.asymmetric.unwrap();
    println!(
        "positive_x left_distance={} right_distance={}",
        geometry.delay_left_ipsi * 343.0,
        geometry.delay_right_ipsi * 343.0
    );
    assert!(
        geometry.delay_left_ipsi > geometry.delay_right_ipsi,
        "positive x is right in the reflection model; left speaker distance must increase"
    );
}

#[test]
fn asymmetric_pose_honors_explicit_lf_delay_mode() {
    let phase = XtcPluginParams {
        head_yaw_deg: 15.0,
        ..Default::default()
    };
    let explicit = XtcPluginParams {
        itd_modeling: "explicit_delay".into(),
        ..phase.clone()
    };
    let a = compute_xtc_filters_full(&phase, 48_000, 1025);
    let b = compute_xtc_filters_full(&explicit, 48_000, 1025);
    let difference = (a.filter_lr[8] - b.filter_lr[8]).norm();
    println!("asymmetric_lf_mode difference={difference}");
    assert!(
        difference > 1e-5,
        "the advertised LF mode must affect asymmetric filters"
    );
}

#[test]
fn tiny_yaw_has_no_artificial_geometry_deadband() {
    let params = XtcPluginParams {
        head_yaw_deg: 0.01,
        ..Default::default()
    };
    assert!(
        compute_geometry_cache(&params, 48_000, 1025)
            .asymmetric
            .is_some()
    );
}

#[test]
fn centered_default_coefficients_match_baseline() {
    let filters = compute_xtc_filters_full(&XtcPluginParams::default(), 48_000, 1025);
    let bits: Vec<_> = filters
        .filter_ll
        .iter()
        .chain(&filters.filter_lr)
        .flat_map(|c| [c.re.to_bits(), c.im.to_bits()])
        .collect();
    let expected: Vec<u32> = include_str!("centered_default_bits.txt")
        .lines()
        .map(|line| line.parse().unwrap())
        .collect();
    assert_eq!(bits.len(), expected.len());
    let mut bit_differences = 0;
    for (&actual, &expected) in bits.iter().zip(&expected) {
        bit_differences += usize::from(actual != expected);
        // The fixture was captured on macOS/aarch64. Allow libm rounding
        // differences on other platforms, while reporting local bit identity.
        assert!((f32::from_bits(actual) - f32::from_bits(expected)).abs() < 2e-6);
    }
    println!(
        "centered_baseline coefficient_words={} bit_differences={bit_differences}",
        bits.len()
    );
}

fn matrix(filters: &XtcFilters, bin: usize) -> [[Complex<f32>; 2]; 2] {
    [
        [filters.filter_ll[bin], filters.filter_lr[bin]],
        [
            filters.filter_rl.as_ref().unwrap_or(&filters.filter_lr)[bin],
            filters.filter_rr.as_ref().unwrap_or(&filters.filter_ll)[bin],
        ],
    ]
}

#[test]
fn pose_paths_match_independent_f64_geometry() {
    let mut cases = 0;
    let mut maximum_error = 0.0_f64;
    for x in [-0.5, -0.1, 0.0, 0.1, 0.5] {
        for yaw in [-60.0, -15.0, 0.0, 15.0, 60.0] {
            if x == 0.0 && yaw == 0.0 {
                continue;
            }
            let params = XtcPluginParams {
                head_offset_x: x,
                head_yaw_deg: yaw,
                ..Default::default()
            };
            let cache = compute_geometry_cache(&params, 48_000, 1025);
            let g = cache.asymmetric.unwrap();
            // Independent f64 evaluation of the documented analytical approximation.
            // This checks coordinate/routing arithmetic, not human HRTF accuracy.
            let theta = f64::from(params.speaker_angle_deg).to_radians();
            let yaw = f64::from(yaw).to_radians();
            let d = f64::from(params.distance_m + params.head_offset_z);
            let distance = |angle: f64, offset: f64| {
                ((d * angle.sin() + offset).powi(2) + (d * angle.cos()).powi(2)).sqrt()
            };
            let left = distance(theta + yaw, f64::from(x));
            let right = distance(theta - yaw, -f64::from(x));
            let diffraction =
                |angle: f64| f64::from(params.head_radius_m) * (angle.abs() + angle.abs().sin());
            let left_cross = right + diffraction(theta - yaw);
            let right_cross = left + diffraction(theta + yaw);
            for (actual, expected) in [
                (g.delay_left_ipsi, left / 343.0),
                (g.delay_right_ipsi, right / 343.0),
                (g.delay_left_contra, left_cross / 343.0),
                (g.delay_right_contra, right_cross / 343.0),
                (g.amplitude_ratio_left, left / left_cross),
                (g.amplitude_ratio_right, right / right_cross),
            ] {
                let error = (f64::from(actual) - expected).abs();
                maximum_error = maximum_error.max(error);
                assert!(error < 3e-7, "x={x} yaw={yaw} error={error}");
            }
            cases += 1;
        }
    }
    println!("pose_geometry cases={cases} maximum_error={maximum_error}");
}

#[test]
fn mirrored_pose_swaps_both_filter_rows_and_columns() {
    let mut cases = 0;
    let mut maximum_error = 0.0_f32;
    for rate in [44_100, 48_000, 96_000] {
        for model in [0, 1] {
            for pinna in [false, true] {
                for mode in ["phase_only", "explicit_delay"] {
                    for (x, yaw) in [(0.1, 0.0), (0.0, 15.0), (0.04, -20.0)] {
                        let p = XtcPluginParams {
                            head_offset_x: x,
                            head_yaw_deg: yaw,
                            head_model: model,
                            pinna_model_enabled: pinna,
                            itd_modeling: mode.into(),
                            ..Default::default()
                        };
                        let mirrored = XtcPluginParams {
                            head_offset_x: -x,
                            head_yaw_deg: -yaw,
                            ..p.clone()
                        };
                        let a = compute_xtc_filters_full(&p, rate, 1025);
                        let b = compute_xtc_filters_full(&mirrored, rate, 1025);
                        for bin in 0..1025 {
                            let a = matrix(&a, bin);
                            let b = matrix(&b, bin);
                            for row in 0..2 {
                                for col in 0..2 {
                                    let error = (a[row][col] - b[1 - row][1 - col]).norm();
                                    maximum_error = maximum_error.max(error);
                                    assert!(
                                        error < 2e-4,
                                        "rate={rate} model={model} pinna={pinna} mode={mode} x={x} yaw={yaw} bin={bin} error={error}"
                                    );
                                }
                            }
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("pose_mirror cases={cases} maximum_error={maximum_error}");
}

#[test]
fn full_matrix_converges_to_centered_path_without_a_deadband() {
    let mut maximum_error = 0.0_f32;
    let mut maximum_midpoint_error = 0.0_f32;
    let mut cases = 0;
    for rate in [44_100, 48_000, 96_000] {
        for model in [0, 1] {
            for pinna in [false, true] {
                for mode in ["phase_only", "explicit_delay"] {
                    let p = XtcPluginParams {
                        head_model: model,
                        pinna_model_enabled: pinna,
                        itd_modeling: mode.into(),
                        ..Default::default()
                    };
                    let center = compute_xtc_filters_full(&p, rate, 1025);
                    for (x, yaw) in [
                        (-1e-30, 0.0),
                        (1e-30, 0.0),
                        (-1e-6, 0.0),
                        (1e-6, 0.0),
                        (-1e-8, 0.0),
                        (1e-8, 0.0),
                        (0.0, -1e-4),
                        (0.0, 1e-4),
                        (0.0, -1e-6),
                        (0.0, 1e-6),
                    ] {
                        let pose = XtcPluginParams {
                            head_offset_x: x,
                            head_yaw_deg: yaw,
                            ..p.clone()
                        };
                        let full = compute_xtc_filters_full(&pose, rate, 1025);
                        assert!(!full.is_symmetric);
                        let opposite = XtcPluginParams {
                            head_offset_x: -x,
                            head_yaw_deg: -yaw,
                            ..p.clone()
                        };
                        let opposite = compute_xtc_filters_full(&opposite, rate, 1025);
                        let mut pose_error = 0.0_f32;
                        for bin in 0..1025 {
                            let a = matrix(&center, bin);
                            let b = matrix(&full, bin);
                            let c = matrix(&opposite, bin);
                            for row in 0..2 {
                                for col in 0..2 {
                                    let error = (a[row][col] - b[row][col]).norm();
                                    maximum_error = maximum_error.max(error);
                                    pose_error = pose_error.max(error);
                                    // A finite move changes the physical transfer to first
                                    // order. Cancel that odd term rather than mislabeling
                                    // the intended response as a branch discontinuity.
                                    let midpoint_error =
                                        ((b[row][col] + c[row][col]) * 0.5 - a[row][col]).norm();
                                    maximum_midpoint_error =
                                        maximum_midpoint_error.max(midpoint_error);
                                    assert!(
                                        midpoint_error < 2e-4,
                                        "rate={rate} model={model} pinna={pinna} mode={mode} x={x} yaw={yaw} bin={bin} midpoint_error={midpoint_error}"
                                    );
                                    if x.abs() <= 1e-8 && yaw.abs() <= 1e-6 {
                                        assert!(error < 2e-4);
                                    }
                                }
                            }
                        }
                        println!(
                            "continuity_detail rate={rate} model={model} pinna={pinna} mode={mode} x={x} yaw={yaw} maximum_error={pose_error}"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("pose_center_continuity cases={cases} maximum_error={maximum_error}");
    println!("pose_center_midpoint maximum_error={maximum_midpoint_error}");
}
