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
            !cache.is_symmetric, filters.is_symmetric
        );
        assert!(
            !cache.is_symmetric,
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
    let geometry = &cache.sources;
    println!(
        "positive_x left_distance={} right_distance={}",
        geometry[0].amplitudes.into_iter().fold(0.0_f32, f32::max),
        geometry[1].amplitudes.into_iter().fold(0.0_f32, f32::max)
    );
    assert!(
        geometry[0].amplitudes.into_iter().fold(0.0_f32, f32::max)
            < geometry[1].amplitudes.into_iter().fold(0.0_f32, f32::max),
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
    assert!(!compute_geometry_cache(&params, 48_000, 1025).is_symmetric);
}

#[test]
fn centered_continuous_path_coefficients_match_reference() {
    let filters = compute_xtc_filters_full(&XtcPluginParams::default(), 48_000, 1025);
    let bits: Vec<_> = filters
        .filter_ll
        .iter()
        .chain(&filters.filter_lr)
        .flat_map(|c| [c.re.to_bits(), c.im.to_bits()])
        .collect();
    let expected: Vec<u32> = include_str!("centered_continuous_paths_bits.txt")
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
        "centered_continuous_reference coefficient_words={} bit_differences={bit_differences}",
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
    let mut failures = 0;
    for x in [-0.5, -0.1, 0.0, 0.1, 0.5] {
        for z in [-0.5, 0.0, 0.5] {
            for yaw in [-90.0, -60.0, -15.0, 0.0, 15.0, 60.0, 90.0] {
                if x == 0.0 && yaw == 0.0 {
                    continue;
                }
                let params = XtcPluginParams {
                    head_offset_x: x,
                    head_offset_z: z,
                    head_yaw_deg: yaw,
                    ..Default::default()
                };
                let cache = compute_geometry_cache(&params, 48_000, 1025);
                // Independent world-coordinate subtraction followed by a passive
                // rotation into the head frame. Yaw cannot move the head center.
                let theta = f64::from(params.speaker_angle_deg).to_radians();
                let yaw = f64::from(yaw).to_radians();
                let d = f64::from(params.distance_m);
                let forward = d * theta.cos() - f64::from(z);
                let left_x = -d * theta.sin() - f64::from(x);
                let right_x = d * theta.sin() - f64::from(x);
                let ranges = [left_x.hypot(forward), right_x.hypot(forward)];
                let reference = (ranges[0] + ranges[1]) * 0.5;
                for (speaker, sx) in [left_x, right_x].into_iter().enumerate() {
                    let projection =
                        ((sx * yaw.cos() - forward * yaw.sin()) / ranges[speaker]).clamp(-1.0, 1.0);
                    let near_angle = projection.abs().acos();
                    let extra = f64::from(params.head_radius_m)
                        * (projection.abs().asin() + projection.abs());
                    let g = &cache.sources[speaker];
                    let near_error = (f64::from(g.near_angle) - near_angle).abs();
                    maximum_error = maximum_error.max(near_error);
                    failures += usize::from(near_error >= 3e-7);
                    for (ear, side) in [-1.0, 1.0].into_iter().enumerate() {
                        let path_extra = if side * projection < 0.0 { extra } else { 0.0 };
                        for (actual, expected) in [
                            (g.ear_angles[ear], (side * projection).acos()),
                            (
                                g.amplitudes[ear],
                                reference / (ranges[speaker] + path_extra),
                            ),
                            (
                                g.delays[ear],
                                (ranges[speaker] - reference + path_extra) / 343.0,
                            ),
                        ] {
                            let error = (f64::from(actual) - expected).abs();
                            maximum_error = maximum_error.max(error);
                            failures += usize::from(error >= 3e-7);
                        }
                    }
                }
                cases += 1;
            }
        }
    }
    println!("pose_geometry cases={cases} maximum_error={maximum_error} failed_values={failures}");
    assert_eq!(failures, 0);
}

#[test]
fn mirrored_pose_swaps_both_filter_rows_and_columns() {
    let mut cases = 0;
    let mut maximum_error = 0.0_f32;
    for rate in [44_100, 48_000, 96_000] {
        for model in [0, 1] {
            for pinna in [false, true] {
                for mode in ["phase_only", "explicit_delay"] {
                    for (x, yaw, z) in [
                        (0.1, 0.0, 0.0),
                        (0.0, 15.0, 0.0),
                        (0.04, -20.0, 0.0),
                        (0.1, 0.0, -0.5),
                        (0.0, 15.0, 0.5),
                        (0.04, -20.0, 0.5),
                        (0.0, 90.0, 0.0),
                        (0.0, -90.0, 0.0),
                        (0.1, 75.0, 0.5),
                    ] {
                        let p = XtcPluginParams {
                            head_offset_x: x,
                            head_offset_z: z,
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
                    for z in [-0.5, 0.0, 0.5] {
                        let p = XtcPluginParams {
                            head_model: model,
                            head_offset_z: z,
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
                                        let midpoint_error = ((b[row][col] + c[row][col]) * 0.5
                                            - a[row][col])
                                            .norm();
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
    }
    println!("pose_center_continuity cases={cases} maximum_error={maximum_error}");
    println!("pose_center_midpoint maximum_error={maximum_midpoint_error}");
}

#[test]
fn depth_translation_moves_toward_fixed_speakers_and_yaw_preserves_center_ranges() {
    let center = compute_geometry_cache(&XtcPluginParams::default(), 48_000, 1025);
    for z in [-0.5, 0.5] {
        let params = XtcPluginParams {
            head_offset_z: z,
            ..Default::default()
        };
        let cache = compute_geometry_cache(&params, 48_000, 1025);
        // Approaching the fixed sources increases their lateral incidence and
        // the extra far-ear delay. Absolute common propagation is normalized out.
        assert_eq!(
            cache.sources[0].near_angle < center.sources[0].near_angle,
            z > 0.0
        );
        assert_eq!(
            cache.sources[0].delays[1] > center.sources[0].delays[1],
            z > 0.0
        );
        for x in [-0.5, 0.0, 0.5] {
            let mut ranges = None;
            for yaw in [-90.0, -60.0, -15.0, 15.0, 60.0, 90.0] {
                let p = XtcPluginParams {
                    head_offset_x: x,
                    head_yaw_deg: yaw,
                    ..params.clone()
                };
                let g = compute_geometry_cache(&p, 48_000, 1025);
                let current = g.sources.map(|source| {
                    (
                        source
                            .amplitudes
                            .into_iter()
                            .fold(0.0_f32, f32::max)
                            .to_bits(),
                        source
                            .delays
                            .into_iter()
                            .fold(f32::INFINITY, f32::min)
                            .to_bits(),
                    )
                });
                assert_eq!(*ranges.get_or_insert(current), current);
            }
        }
    }
}
