//! Custom-layout decoder tests (R1/A2/A3).
//!
//! Tolerances fixed a priori: matrix equivalence uses the AUD133 f32-storage
//! bound (2e-6 abs); permutation uses 1e-6 abs; FOA linearity uses 1e-5 abs
//! and order-7 linearity 5e-5 abs from f32 accumulation over ≤64 taps;
//! LFE silence and save/reload equality are exact.

// Rust guideline compliant 2026-02-21

use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_host::speaker_config::get_speaker_config;
use sotf_plugin_ambisonics::custom_layout::{
    CUSTOM_LAYOUT_KEY, CustomDecoderConfig, CustomLayout, CustomSpeaker,
};
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

fn speaker(label: &str, azimuth_deg: f32, elevation_deg: f32, is_lfe: bool) -> CustomSpeaker {
    CustomSpeaker {
        label: label.to_owned(),
        azimuth_deg,
        elevation_deg,
        is_lfe,
    }
}

/// Custom replica of the SOTF 5.1 geometry (ITU-R BS.775 positions).
fn replica_5_1() -> CustomLayout {
    CustomLayout {
        name: "replica-5.1".to_owned(),
        speakers: vec![
            speaker("FL", 30.0, 0.0, false),
            speaker("FR", -30.0, 0.0, false),
            speaker("C", 0.0, 0.0, false),
            speaker("LFE", 0.0, 0.0, true),
            speaker("SL", 110.0, 0.0, false),
            speaker("SR", -110.0, 0.0, false),
        ],
    }
}

/// Custom replica of the SOTF 9.1.6 geometry (16 channels).
fn replica_9_1_6() -> CustomLayout {
    let rows = [
        ("FL", 30.0, 0.0, false),
        ("FR", -30.0, 0.0, false),
        ("C", 0.0, 0.0, false),
        ("LFE", 0.0, 0.0, true),
        ("SL", 90.0, 0.0, false),
        ("SR", -90.0, 0.0, false),
        ("BL", 150.0, 0.0, false),
        ("BR", -150.0, 0.0, false),
        ("WL", 60.0, 0.0, false),
        ("WR", -60.0, 0.0, false),
        ("TFL", 30.0, 45.0, false),
        ("TFR", -30.0, 45.0, false),
        ("TBL", 150.0, 45.0, false),
        ("TBR", -150.0, 45.0, false),
        ("TMiL", 90.0, 45.0, false),
        ("TMiR", -90.0, 45.0, false),
    ];
    CustomLayout {
        name: "replica-9.1.6".to_owned(),
        speakers: rows
            .iter()
            .map(|(label, azimuth, elevation, is_lfe)| {
                speaker(label, *azimuth, *elevation, *is_lfe)
            })
            .collect(),
    }
}

fn custom_config(
    order: usize,
    layout: CustomLayout,
    max_re: bool,
    dual_band: bool,
    algorithm: &str,
) -> CustomDecoderConfig {
    CustomDecoderConfig {
        params: AmbisonicsDecoderConfig {
            order,
            target_layout: CUSTOM_LAYOUT_KEY.to_owned(),
            max_re_weighting: max_re,
            dual_band,
            algorithm: algorithm.to_owned(),
        },
        custom_layout: layout,
    }
}

fn max_abs_diff(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max)
}

#[test]
fn custom_replica_matches_named_5_1_matrix() {
    // Full order sweep 1-7: the custom builders share the named f32 angles
    // and f64 SVD/VBAP helpers exactly, so every named build that satisfies
    // the shipped peak bound has a bit-near replica (bound kept at 2e-6;
    // the coordinator records the measured worst case).
    let named_config = get_speaker_config("5.1").unwrap();
    let replica = replica_5_1();
    for order in 1..=7 {
        for max_re in [false, true] {
            let named =
                DecodeMatrix::build(order, named_config, max_re).unwrap();
            let custom =
                DecodeMatrix::build_for_custom(order, &replica, max_re).unwrap();
            assert_eq!(custom.ambi_channels, named.ambi_channels);
            assert_eq!(custom.speaker_count, named.speaker_count);
            assert_eq!(custom.quality().rank, named.quality().rank);
            let error = max_abs_diff(&custom.matrix, &named.matrix);
            assert!(
                error <= 2.0e-6,
                "order={order}, max_re={max_re}: matrix error {error}"
            );

            let named_allrad =
                DecodeMatrix::build_allrad(order, named_config, max_re).unwrap();
            let custom_allrad =
                DecodeMatrix::build_allrad_for_custom(order, &replica, max_re).unwrap();
            assert_eq!(
                custom_allrad.virtual_speaker_count,
                named_allrad.virtual_speaker_count
            );
            let error = max_abs_diff(&custom_allrad.matrix, &named_allrad.matrix);
            assert!(
                error <= 2.0e-6,
                "allrad order={order}, max_re={max_re}: matrix error {error}"
            );
        }
    }
}

#[test]
fn custom_cube_decodes_and_processes() {
    // Cube vertices: azimuth +-45/+-135, elevation +-35.264 degrees.
    let elevation = (1.0_f64 / 3.0_f64.sqrt()).asin().to_degrees() as f32;
    let labels = ["FLT", "FRT", "BLT", "BRT", "FLB", "FRB", "BLB", "BRB"];
    let azimuths = [45.0, -45.0, 135.0, -135.0, 45.0, -45.0, 135.0, -135.0];
    let layout = CustomLayout {
        name: "cube".to_owned(),
        speakers: labels
            .iter()
            .zip(azimuths.iter())
            .enumerate()
            .map(|(index, (label, azimuth))| {
                speaker(
                    label,
                    *azimuth,
                    if index < 4 { elevation } else { -elevation },
                    false,
                )
            })
            .collect(),
    };
    for algorithm in ["mode_matching", "allrad"] {
        let matrix = if algorithm == "allrad" {
            DecodeMatrix::build_allrad_for_custom(1, &layout, false).unwrap()
        } else {
            DecodeMatrix::build_for_custom(1, &layout, false).unwrap()
        };
        assert!(matrix.matrix.iter().all(|value| value.is_finite()));
        assert!(matrix.quality().peak_coefficient <= 8.0);

        let mut plugin = AmbisonicsDecoderPlugin::new_custom(&custom_config(
            1,
            layout.clone(),
            false,
            false,
            algorithm,
        ))
        .unwrap();
        assert_eq!(plugin.output_channels(), 8);
        plugin.initialize(48_000).unwrap();
        let input = [1.0_f32, 0.0, 0.0, 0.0];
        let mut output = [0.0_f32; 8];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 1))
            .unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 1e-6));
    }
}

#[test]
fn degenerate_custom_layout_is_bounded_or_rejected() {
    // Four coincident non-LFE speakers: rank-deficient by construction.
    let layout = CustomLayout {
        name: "coincident".to_owned(),
        speakers: ["A", "B", "C", "D"]
            .iter()
            .map(|label| speaker(label, 0.0, 0.0, false))
            .collect(),
    };
    match DecodeMatrix::build_for_custom(1, &layout, false) {
        Ok(matrix) => {
            assert!(matrix.quality().rank < matrix.ambi_channels);
            assert!(matrix.matrix.iter().all(|value| value.is_finite()));
            assert!(matrix.quality().peak_coefficient <= 8.0);
        }
        Err(error) => assert!(
            error.contains("ill-conditioned") || error.contains("rank"),
            "{error}"
        ),
    }
    match DecodeMatrix::build_allrad_for_custom(1, &layout, false) {
        Ok(matrix) => {
            assert!(matrix.matrix.iter().all(|value| value.is_finite()));
            assert!(matrix.quality().peak_coefficient <= 8.0);
        }
        Err(error) => assert!(error.contains("ill-conditioned"), "{error}"),
    }
}

#[test]
fn lfe_channel_is_silent_and_ignored() {
    // LFE exclusion must follow the flag at any channel position: first,
    // middle and last are all exercised.
    for lfe_index in [0_usize, 2, 5] {
        let mut layout = replica_5_1();
        let lfe = layout.speakers.remove(3);
        layout.speakers.insert(lfe_index, lfe);
        layout.name = format!("lfe-at-{lfe_index}-5.1");
        for algorithm in ["mode_matching", "allrad"] {
            let matrix = if algorithm == "allrad" {
                DecodeMatrix::build_allrad_for_custom(1, &layout, false).unwrap()
            } else {
                DecodeMatrix::build_for_custom(1, &layout, false).unwrap()
            };
            let row = &matrix.matrix[lfe_index * 4..lfe_index * 4 + 4];
            assert!(
                row.iter().all(|value| *value == 0.0),
                "algorithm={algorithm}, lfe_index={lfe_index}"
            );

            let mut moved_lfe = layout.clone();
            moved_lfe.speakers[lfe_index].azimuth_deg = 123.0;
            moved_lfe.speakers[lfe_index].elevation_deg = 45.0;
            let again = if algorithm == "allrad" {
                DecodeMatrix::build_allrad_for_custom(1, &moved_lfe, false).unwrap()
            } else {
                DecodeMatrix::build_for_custom(1, &moved_lfe, false).unwrap()
            };
            assert_eq!(again.matrix, matrix.matrix);

            let mut plugin = AmbisonicsDecoderPlugin::new_custom(&custom_config(
                1,
                layout.clone(),
                false,
                false,
                algorithm,
            ))
            .unwrap();
            plugin.initialize(48_000).unwrap();
            let mut output = [0.0_f32; 6];
            plugin
                .process(
                    &[1.0_f32, 0.25, -0.5, 0.75],
                    &mut output,
                    &ProcessContext::new(48_000, 1),
                )
                .unwrap();
            assert_eq!(output[lfe_index], 0.0);
            assert!(
                output
                    .iter()
                    .enumerate()
                    .any(|(index, sample)| index != lfe_index && sample.abs() > 1e-6),
                "algorithm={algorithm}, lfe_index={lfe_index}"
            );
        }
    }
}

#[test]
fn speaker_permutation_permutes_custom_outputs() {
    let base = replica_5_1();
    let mut reversed = base.clone();
    reversed.speakers.reverse();
    reversed.name = "reversed-5.1".to_owned();
    for algorithm in ["mode_matching", "allrad"] {
        let base_matrix = if algorithm == "allrad" {
            DecodeMatrix::build_allrad_for_custom(1, &base, false).unwrap()
        } else {
            DecodeMatrix::build_for_custom(1, &base, false).unwrap()
        };
        let reversed_matrix = if algorithm == "allrad" {
            DecodeMatrix::build_allrad_for_custom(1, &reversed, false).unwrap()
        } else {
            DecodeMatrix::build_for_custom(1, &reversed, false).unwrap()
        };
        for channel in 0..6 {
            let expected = &base_matrix.matrix[(5 - channel) * 4..(5 - channel) * 4 + 4];
            let actual = &reversed_matrix.matrix[channel * 4..channel * 4 + 4];
            let error = max_abs_diff(expected, actual);
            assert!(
                error <= 1.0e-6,
                "algorithm={algorithm}, channel={channel}: permutation error {error}"
            );
        }
    }
}

#[test]
fn failed_custom_preparation_preserves_running_decoder() {
    let config = custom_config(1, replica_5_1(), true, false, "mode_matching");
    let mut plugin = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
    plugin.initialize(48_000).unwrap();
    let input = [0.5_f32, -0.25, 0.125, 0.75];
    let mut before = [0.0_f32; 6];
    plugin
        .process(&input, &mut before, &ProcessContext::new(48_000, 1))
        .unwrap();

    let mut empty = replica_5_1();
    empty.speakers.clear();
    let mut bad_angle = replica_5_1();
    bad_angle.speakers[0].azimuth_deg = 999.0;
    for candidate in [
        custom_config(1, empty, true, false, "mode_matching"),
        custom_config(1, bad_angle, true, false, "mode_matching"),
        custom_config(8, replica_5_1(), true, false, "mode_matching"),
        custom_config(1, replica_5_1(), true, false, "no_such_algorithm"),
    ] {
        assert!(AmbisonicsDecoderPlugin::new_custom(&candidate).is_err());
    }

    let mut after = [0.0_f32; 6];
    plugin
        .process(&input, &mut after, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(plugin.output_channels(), 6);
}

#[test]
fn custom_config_persists_through_json_roundtrip() {
    let config = custom_config(2, replica_9_1_6(), true, false, "allrad");
    let json = serde_json::to_string(&config).unwrap();
    let restored: CustomDecoderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, config);

    let input: Vec<f32> = (0..37 * 9)
        .map(|index| (index as f32 * 0.03125).sin() * 0.25)
        .collect();
    let mut first = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
    first.initialize(48_000).unwrap();
    let mut first_out = vec![0.0_f32; 37 * 16];
    first
        .process(&input, &mut first_out, &ProcessContext::new(48_000, 37))
        .unwrap();

    let mut second = AmbisonicsDecoderPlugin::new_custom(&restored).unwrap();
    second.initialize(48_000).unwrap();
    let mut second_out = vec![0.0_f32; 37 * 16];
    second
        .process(&input, &mut second_out, &ProcessContext::new(48_000, 37))
        .unwrap();
    assert_eq!(second_out, first_out);
}

#[test]
fn legacy_named_json_without_custom_keys_still_builds() {
    let params: AmbisonicsDecoderConfig = serde_json::from_str(
        r#"{"order":1,"target_layout":"5.1","max_re_weighting":true,"dual_band":false,"algorithm":"mode_matching"}"#,
    )
    .unwrap();
    let plugin = AmbisonicsDecoderPlugin::new(&params).unwrap();
    assert_eq!(plugin.output_channels(), 6);
    assert!(plugin.custom_layout().is_none());
}

#[test]
fn order7_custom_sixteen_channel_processes_all_basis() {
    // A3: 64-channel HOA through a persisted custom 16-channel decoder.
    for algorithm in ["mode_matching", "allrad"] {
        let config = custom_config(7, replica_9_1_6(), false, false, algorithm);
        let json = serde_json::to_string(&config).unwrap();
        let restored: CustomDecoderConfig = serde_json::from_str(&json).unwrap();
        let mut plugin = AmbisonicsDecoderPlugin::new_custom(&restored).unwrap();
        assert_eq!(plugin.input_channels(), 64);
        assert_eq!(plugin.output_channels(), 16);
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));
        plugin.initialize(48_000).unwrap();

        let expected = if algorithm == "allrad" {
            DecodeMatrix::build_allrad_for_custom(7, &replica_9_1_6(), false).unwrap()
        } else {
            DecodeMatrix::build_for_custom(7, &replica_9_1_6(), false).unwrap()
        };
        for acn in 0..64 {
            let mut input = [0.0_f32; 64];
            input[acn] = 1.0;
            let mut output = [0.0_f32; 16];
            assert_eq!(
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 1))
                    .unwrap(),
                1
            );
            for (speaker, actual) in output.iter().enumerate() {
                let reference = expected.matrix[speaker * 64 + acn];
                assert!(
                    (*actual - reference).abs() <= 1.0e-6,
                    "algorithm={algorithm}, speaker={speaker}, ACN={acn}"
                );
            }
        }

        let frames = 257;
        let mut dense = vec![0.0_f32; frames * 64];
        for (index, sample) in dense.iter_mut().enumerate() {
            *sample = ((index * 73 % 509) as f32 - 254.0) / 4096.0;
        }
        let mut dense_out = vec![0.0_f32; frames * 16];
        assert_eq!(
            plugin
                .process(&dense, &mut dense_out, &ProcessContext::new(48_000, frames))
                .unwrap(),
            frames
        );
        assert!(dense_out.iter().all(|sample| sample.is_finite()));

        // Single-band custom path stops exactly at end of program (A3 tail).
        let zeros = vec![0.0_f32; 64 * 64];
        let mut tail = vec![1.0_f32; 64 * 16];
        plugin
            .process(&zeros, &mut tail, &ProcessContext::new(48_000, 64))
            .unwrap();
        assert!(tail.iter().all(|value| *value == 0.0));
    }
}

#[test]
fn custom_dual_band_matches_linearity_within_f32_accumulation() {
    // Matrix decode is linear; dual-band sums two linear paths.
    for dual_band in [false, true] {
        let mut plugin = AmbisonicsDecoderPlugin::new_custom(&custom_config(
            1,
            replica_5_1(),
            true,
            dual_band,
            "mode_matching",
        ))
        .unwrap();
        plugin.initialize(48_000).unwrap();
        // Dual-band state must settle before the linearity comparison.
        let settle = vec![0.0_f32; 2048 * 4];
        let mut settle_out = vec![0.0_f32; 2048 * 6];
        plugin
            .process(&settle, &mut settle_out, &ProcessContext::new(48_000, 2048))
            .unwrap();
        let first = [0.2_f32, -0.1, 0.05, 0.15];
        let second = [-0.05_f32, 0.3, -0.2, 0.1];
        let mut out_first = [0.0_f32; 6];
        let mut out_second = [0.0_f32; 6];
        let mut out_sum = [0.0_f32; 6];
        // Each probe runs on a freshly settled instance for state equality.
        for (probe, out) in [
            (first, &mut out_first),
            (second, &mut out_second),
            (
                [
                    first[0] + second[0],
                    first[1] + second[1],
                    first[2] + second[2],
                    first[3] + second[3],
                ],
                &mut out_sum,
            ),
        ] {
            plugin.reset();
            plugin
                .process(&settle, &mut settle_out, &ProcessContext::new(48_000, 2048))
                .unwrap();
            plugin
                .process(&probe, &mut *out, &ProcessContext::new(48_000, 1))
                .unwrap();
        }
        for channel in 0..6 {
            let error = (out_first[channel] + out_second[channel] - out_sum[channel]).abs();
            assert!(
                error <= 1.0e-5,
                "dual_band={dual_band}, channel={channel}: linearity error {error}"
            );
        }
    }
}

#[test]
fn decoder_export_snapshot_roundtrips() {
    let matrix = DecodeMatrix::build_for_custom(2, &replica_9_1_6(), true).unwrap();
    let export = matrix.export();
    assert_eq!(export.algorithm, "mode_matching");
    assert_eq!(export.ambi_channels, 9);
    assert_eq!(export.speaker_count, 16);
    assert_eq!(export.virtual_speaker_count, 0);
    // The snapshot pins the live coefficients, weights and diagnostics.
    assert_eq!(export.matrix, matrix.matrix);
    assert_eq!(export.max_re_weights, matrix.max_re_weights);
    let quality = matrix.quality();
    assert_eq!(export.rank, quality.rank);
    assert_eq!(export.condition_number, quality.condition_number);
    assert_eq!(export.reconstruction_error, quality.reconstruction_error);
    assert_eq!(export.peak_coefficient, quality.peak_coefficient);
    let json = serde_json::to_string(&export).unwrap();
    let restored: sotf_plugin_ambisonics::decode_matrix::DecoderExport =
        serde_json::from_str(&json).unwrap();
    assert_eq!(restored, export);

    let allrad = DecodeMatrix::build_allrad_for_custom(2, &replica_9_1_6(), true).unwrap();
    let allrad_export = allrad.export();
    assert_eq!(allrad_export.algorithm, "allrad");
    assert!(allrad_export.virtual_speaker_count > 0);
    assert_eq!(allrad_export.matrix, allrad.matrix);
    // Archival-only: no API rebuilds a live matrix from a snapshot, so the
    // only reload path is rebuilding from geometry (covered by the
    // persistence tests above).
}

#[test]
fn ill_conditioned_custom_new_custom_rejection_preserves_running_decoder() {
    // Near-coincident speakers produce a difference mode with a small
    // retained singular value sigma; the Tikhonov gain
    // sigma/(sigma^2+lambda^2) with lambda = 1e-6*sigma_max amplifies it
    // past the 8.0 peak bound. The gain curve peaks at sigma = lambda, so
    // a deterministic log-spaced separation scan must find a rejection.
    fn pair(separation_deg: f32) -> CustomLayout {
        CustomLayout {
            name: format!("near-coincident-{separation_deg}"),
            speakers: vec![
                speaker("A", 0.0, 0.0, false),
                speaker("B", separation_deg, 0.0, false),
            ],
        }
    }
    let mut rejected: Option<(usize, CustomLayout)> = None;
    for order in [3_usize, 7] {
        for separation in [4.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01] {
            let layout = pair(separation);
            assert!(layout.validate().is_ok());
            if let Err(error) = DecodeMatrix::build_for_custom(order, &layout, false) {
                assert!(
                    error.contains("ill-conditioned"),
                    "order={order}, separation={separation}: {error}"
                );
                rejected.get_or_insert((order, layout));
            }
        }
    }
    let (order, layout) = rejected.expect(
        "the separation scan must find at least one ill-conditioned geometry",
    );

    let running_config = custom_config(1, replica_5_1(), true, false, "mode_matching");
    let mut plugin = AmbisonicsDecoderPlugin::new_custom(&running_config).unwrap();
    plugin.initialize(48_000).unwrap();
    let input = [0.5_f32, -0.25, 0.125, 0.75];
    let mut before = [0.0_f32; 6];
    plugin
        .process(&input, &mut before, &ProcessContext::new(48_000, 1))
        .unwrap();

    let bad = custom_config(order, layout, false, false, "mode_matching");
    let error = AmbisonicsDecoderPlugin::new_custom(&bad).unwrap_err();
    assert!(error.contains("ill-conditioned"), "{error}");

    let mut after = [0.0_f32; 6];
    plugin
        .process(&input, &mut after, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(plugin.output_channels(), 6);
}

#[test]
fn replica_9_1_6_conditioning_orders_3_through_7_is_bounded_with_expected_rank_loss() {
    // A1 conditioning for orders 3-7 on a shipped-geometry replica (exact
    // angles verified against CONFIG_9_1_6): 15 non-LFE rows cannot span
    // 16+ harmonics, so mode-matching rank loss is structural, while the
    // AllRAD virtual-grid solve stays full rank. Both stay bounded.
    let named_config = get_speaker_config("9.1.6").unwrap();
    let replica = replica_9_1_6();
    for order in 3..=7 {
        let ambi_channels = (order + 1) * (order + 1);
        for max_re in [false, true] {
            let custom = DecodeMatrix::build_for_custom(order, &replica, max_re).unwrap();
            let named = DecodeMatrix::build(order, named_config, max_re).unwrap();
            assert_eq!(custom.quality().rank, named.quality().rank);
            assert!(
                custom.quality().rank < ambi_channels,
                "order={order}, max_re={max_re}: rank {} must be below {ambi_channels}",
                custom.quality().rank
            );
            assert!(custom.matrix.iter().all(|value| value.is_finite()));
            assert!(custom.quality().peak_coefficient <= 8.0);
            let error = max_abs_diff(&custom.matrix, &named.matrix);
            assert!(
                error <= 2.0e-6,
                "order={order}, max_re={max_re}: replica error {error}"
            );

            let custom_allrad =
                DecodeMatrix::build_allrad_for_custom(order, &replica, max_re).unwrap();
            let named_allrad = DecodeMatrix::build_allrad(order, named_config, max_re).unwrap();
            assert_eq!(
                custom_allrad.quality().rank,
                named_allrad.quality().rank
            );
            assert_eq!(
                custom_allrad.quality().rank,
                ambi_channels,
                "order={order}, max_re={max_re}: virtual-grid solve must stay full rank"
            );
            assert_eq!(
                custom_allrad.virtual_speaker_count,
                named_allrad.virtual_speaker_count
            );
            let error = max_abs_diff(&custom_allrad.matrix, &named_allrad.matrix);
            assert!(
                error <= 2.0e-6,
                "allrad order={order}, max_re={max_re}: replica error {error}"
            );
        }
    }
}

#[test]
fn custom_dual_band_tail_drain_reset_and_partition() {
    for algorithm in ["mode_matching", "allrad"] {
        let config = custom_config(1, replica_5_1(), true, true, algorithm);
        let mut plugin = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Unknown);
        assert_eq!(plugin.drain_call_bound().map(|bound| bound.get()), Some(1));
        assert_eq!(plugin.latency_samples(), 0);
        plugin.initialize(48_000).unwrap();

        let frames = 512;
        let input: Vec<f32> = (0..frames * 4)
            .map(|index| ((index * 37 % 251) as f32 - 125.0) / 1024.0)
            .collect();
        let mut fresh = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
        fresh.initialize(48_000).unwrap();
        let mut fresh_out = vec![0.0_f32; frames * 6];
        fresh
            .process(&input, &mut fresh_out, &ProcessContext::new(48_000, frames))
            .unwrap();

        // Drive with unrelated audio, reset, then match the fresh instance.
        let noise = vec![0.3_f32; frames * 4];
        let mut tmp = vec![0.0_f32; frames * 6];
        plugin
            .process(&noise, &mut tmp, &ProcessContext::new(48_000, frames))
            .unwrap();
        plugin.reset();
        let mut reset_out = vec![0.0_f32; frames * 6];
        plugin
            .process(&input, &mut reset_out, &ProcessContext::new(48_000, frames))
            .unwrap();
        assert_eq!(reset_out, fresh_out, "algorithm={algorithm}");

        // Partition equivalence: eight sequential 64-frame calls equal one
        // 512-frame call from identical state.
        let mut chunked = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
        chunked.initialize(48_000).unwrap();
        let mut chunked_out = vec![0.0_f32; frames * 6];
        for (piece, out) in input
            .chunks(64 * 4)
            .zip(chunked_out.chunks_mut(64 * 6))
        {
            chunked
                .process(piece, out, &ProcessContext::new(48_000, 64))
                .unwrap();
        }
        assert_eq!(chunked_out, fresh_out, "algorithm={algorithm}");
    }
}

#[test]
fn custom_non_finite_input_is_rejected_without_poisoning() {
    for dual_band in [false, true] {
        let config = custom_config(1, replica_5_1(), true, dual_band, "mode_matching");
        let mut tested = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
        let mut clean = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
        tested.initialize(48_000).unwrap();
        clean.initialize(48_000).unwrap();

        let mut bad_input = vec![0.0_f32; 64 * 4];
        bad_input[3] = f32::NAN;
        let mut rejected_output = vec![0.0_f32; 64 * 6];
        assert!(
            tested
                .process(
                    &bad_input,
                    &mut rejected_output,
                    &ProcessContext::new(48_000, 64),
                )
                .is_err(),
            "dual_band={dual_band}"
        );

        let finite_input = vec![0.1_f32; 64 * 4];
        let mut tested_output = vec![0.0_f32; 64 * 6];
        let mut clean_output = vec![0.0_f32; 64 * 6];
        let context = ProcessContext::new(48_000, 64);
        tested
            .process(&finite_input, &mut tested_output, &context)
            .unwrap();
        clean
            .process(&finite_input, &mut clean_output, &context)
            .unwrap();
        assert_eq!(tested_output, clean_output, "dual_band={dual_band}");
    }
}

#[test]
fn custom_retained_construction_json_rebuilds_bit_identical_audio() {
    // F10 save contract through a trait object: the host retains the
    // construction JSON (geometry is NOT available via get_data, which
    // stays None so the host never treats the decoder as an analyzer).
    let config = custom_config(2, replica_9_1_6(), true, true, "mode_matching");
    let retained = serde_json::to_string(&config).unwrap();
    let mut running: Box<dyn Plugin> =
        Box::new(AmbisonicsDecoderPlugin::new_custom(&config).unwrap());
    assert!(running.get_data().is_none());
    running.initialize(48_000).unwrap();
    let frames = 128;
    let input: Vec<f32> = (0..frames * 9)
        .map(|index| (index as f32 * 0.03125).sin() * 0.25)
        .collect();
    let mut first = vec![0.0_f32; frames * 16];
    running
        .process(&input, &mut first, &ProcessContext::new(48_000, frames))
        .unwrap();

    let restored: CustomDecoderConfig = serde_json::from_str(&retained).unwrap();
    assert_eq!(restored, config);
    let mut reloaded: Box<dyn Plugin> =
        Box::new(AmbisonicsDecoderPlugin::new_custom(&restored).unwrap());
    reloaded.initialize(48_000).unwrap();
    let mut second = vec![0.0_f32; frames * 16];
    reloaded
        .process(&input, &mut second, &ProcessContext::new(48_000, frames))
        .unwrap();
    assert_eq!(second, first);
}
