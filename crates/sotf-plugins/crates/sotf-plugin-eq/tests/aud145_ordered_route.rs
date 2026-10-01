//! AUD145 ordered per-band route captures and public configuration rejection.
//!
//! Run the independent base-rate capture explicitly with
//! `AUD145_PLACEMENT_CAPTURE_DIR=... cargo test -p sotf-plugin-eq --test
//! aud145_ordered_route capture_base_rate_placement_matrix -- --ignored` and
//! compare it with `audit/artifacts/aud145-placement-reference-r1/compare.py`.

use sotf_host::{AutoGainParams, ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
    KautzSectionConfig,
};
use std::fs;
use std::path::{Path, PathBuf};

const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
const FRAMES_PER_BLOCK: usize = 193;

fn filter(placement: Option<EqBandPlacement>) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: "peak".into(),
        freq: 1_379.0,
        q: 0.83,
        db_gain: 7.0,
        order: 2,
        topology: EqFilterTopology::Biquad,
        placement,
        lambda: None,
        kautz_sections: Vec::new(),
    }
}

fn legacy_mixed_advanced_params() -> EqPluginParams {
    let mut warped = filter(None);
    warped.freq = 3_100.0;
    warped.q = 1.1;
    warped.db_gain = 4.5;
    warped.topology = EqFilterTopology::WarpedBiquad;
    warped.lambda = None;

    let mut kautz = filter(None);
    kautz.freq = 10_000.0;
    kautz.q = 2.0;
    kautz.db_gain = 4.0;
    kautz.topology = EqFilterTopology::KautzFilter;
    kautz.kautz_sections = vec![
        KautzSectionConfig {
            pole_freq: 4_700.0,
            q: 1.7,
            gain: -2.0,
        },
        KautzSectionConfig {
            pole_freq: 10_000.0,
            q: 2.0,
            gain: 4.0,
        },
    ];

    params(vec![filter(None), warped, kautz], None)
}

fn params(
    filters: Vec<BiquadFilterConfig>,
    stereo_pairs: Option<Vec<[usize; 2]>>,
) -> EqPluginParams {
    EqPluginParams {
        filters,
        channel_filters: None,
        stereo_pairs,
        auto_gain: AutoGainParams::default(),
    }
}

fn decode_f32le(bytes: &[u8]) -> Vec<f32> {
    assert_eq!(bytes.len() % 4, 0, "input fixture has partial f32 sample");
    let (samples, remainder) = bytes.as_chunks::<4>();
    assert!(
        remainder.is_empty(),
        "input fixture has a partial f32 sample"
    );
    samples
        .iter()
        .map(|sample| f32::from_le_bytes(*sample))
        .collect()
}

fn encode_f32le(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn decode_f64le(bytes: &[u8]) -> Vec<f64> {
    assert_eq!(bytes.len() % 8, 0, "reference has a partial f64 sample");
    let (samples, remainder) = bytes.as_chunks::<8>();
    assert!(remainder.is_empty(), "reference has a partial f64 sample");
    samples
        .iter()
        .map(|sample| f64::from_le_bytes(*sample))
        .collect()
}

fn render(
    plugin: &mut EqPlugin,
    input: &[f32],
    channels: usize,
    sample_rate: u32,
    block_frames: usize,
) -> Vec<f32> {
    assert_eq!(input.len() % channels, 0);
    let mut output = vec![0.0; input.len()];
    let block_samples = block_frames * channels;
    for (input_block, output_block) in input
        .chunks(block_samples)
        .zip(output.chunks_mut(block_samples))
    {
        let frames = input_block.len() / channels;
        let processed = plugin
            .process(
                input_block,
                output_block,
                &ProcessContext::new(sample_rate, frames),
            )
            .expect("process ordered EQ route");
        assert_eq!(processed, frames, "ordered route returned a short block");
    }
    assert_eq!(
        output.len(),
        input.len(),
        "ordered route changed frame count"
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));
    output
}

fn placement_sequences() -> [(&'static str, &'static [EqBandPlacement]); 7] {
    use EqBandPlacement::{Left, Mid, Right, Side, Stereo};
    [
        ("stereo", &[Stereo]),
        ("left", &[Left]),
        ("right", &[Right]),
        ("mid", &[Mid]),
        ("side", &[Side]),
        ("left-mid", &[Left, Mid]),
        ("mid-left", &[Mid, Left]),
    ]
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../")
}

fn frozen_legacy_case_names() -> Vec<String> {
    let baseline_root = repository_root().join("audit/artifacts/aud145-preedit-baseline-r1");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(baseline_root.join("manifest.json")).expect("read frozen legacy manifest"),
    )
    .expect("parse frozen legacy manifest");
    let cases = manifest["cases"].as_array().expect("legacy case list");
    assert_eq!(cases.len(), 4, "unexpected frozen baseline case count");
    cases
        .iter()
        .map(|case| case.as_str().expect("case name").to_string())
        .collect()
}

fn render_frozen_legacy_case(case_name: &str) -> (Vec<f32>, Vec<u8>, usize) {
    let case_dir = repository_root()
        .join("audit/artifacts/aud145-preedit-baseline-r1")
        .join(case_name);
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(case_dir.join("settings.json")).expect("read frozen legacy settings"),
    )
    .expect("parse frozen legacy settings");
    let sample_rate = settings["sample_rate"].as_u64().expect("sample rate") as u32;
    let channels = settings["channels"].as_u64().expect("channel count") as usize;
    let block_size = settings["block_size"].as_u64().expect("block size") as usize;
    let oversampling = settings["oversampling"].as_i64().expect("oversampling") as i32;
    let params: EqPluginParams = serde_json::from_value(settings["params"].clone())
        .expect("deserialize frozen legacy EQ settings");
    let input = decode_f32le(&fs::read(case_dir.join("input.f32le")).expect("read frozen input"));
    let mut plugin = EqPlugin::from_params(channels, sample_rate, params)
        .expect("construct legacy settings after schema extension");
    plugin
        .parametric_set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(oversampling),
        )
        .expect("restore frozen oversampling value");
    plugin
        .plugin_initialize(sample_rate)
        .expect("initialize legacy settings replay");
    let output = render(&mut plugin, &input, channels, sample_rate, block_size);
    let actual_bytes = encode_f32le(&output);

    if let Some(capture_root) = std::env::var_os("AUD145_LEGACY_CORRECTED_CAPTURE_DIR") {
        let capture_root = PathBuf::from(capture_root);
        let capture_dir = if capture_root.is_absolute() {
            capture_root
        } else {
            repository_root().join(capture_root)
        };
        fs::create_dir_all(&capture_dir).expect("create corrected legacy capture directory");
        fs::write(
            capture_dir.join(format!("{case_name}.f32le")),
            &actual_bytes,
        )
        .expect("write corrected legacy replay");
    }

    (output, actual_bytes, channels)
}

fn assert_matches_corrected_legacy_reference(case_name: &str, actual: &[f32]) {
    let reference_path = repository_root()
        .join("audit/artifacts/aud145-legacy-corrected-reference-r1")
        .join(format!("{case_name}.f64le"));
    let expected = decode_f64le(
        &fs::read(reference_path).expect("read independent corrected legacy reference"),
    );
    assert_eq!(
        actual.len(),
        expected.len(),
        "reference length for {case_name}"
    );
    assert!(expected.iter().all(|sample| sample.is_finite()));
    assert!(actual.iter().all(|sample| sample.is_finite()));

    let mut peak_error = 0.0_f64;
    let mut squared_error = 0.0_f64;
    let mut expected_peak = 0.0_f64;
    let mut expected_squared = 0.0_f64;
    for (&actual, &expected) in actual.iter().zip(&expected) {
        let expected_actual = f64::from(actual);
        let error = expected_actual - expected;
        peak_error = peak_error.max(error.abs());
        squared_error += error * error;
        expected_peak = expected_peak.max(expected.abs());
        expected_squared += expected * expected;
    }
    let length = expected.len() as f64;
    let rms_error = (squared_error / length).sqrt();
    let expected_rms = (expected_squared / length).sqrt();
    let peak_limit = 2.0e-5 * expected_peak.max(1.0);
    let rms_limit = 2.0e-6 * expected_rms.max(1.0);
    assert!(
        peak_error <= peak_limit,
        "{case_name}: peak error {peak_error} exceeds {peak_limit}"
    );
    assert!(
        rms_error <= rms_limit,
        "{case_name}: RMS error {rms_error} exceeds {rms_limit}"
    );
}

fn channel_bytes(samples: &[f32], channels: usize, channel: usize) -> Vec<u8> {
    assert!(channel < channels);
    assert_eq!(samples.len() % channels, 0);
    let mut bytes = Vec::with_capacity(samples.len() / channels * 4);
    for frame in samples.chunks_exact(channels) {
        bytes.extend_from_slice(&frame[channel].to_le_bytes());
    }
    bytes
}

fn reference_root() -> PathBuf {
    repository_root().join("audit/artifacts/aud145-placement-reference-r1")
}

/// Produces all 42 exact public-API vectors consumed by the independent f64 oracle.
#[test]
#[ignore = "explicit independent-reference capture; set AUD145_PLACEMENT_CAPTURE_DIR"]
fn capture_base_rate_placement_matrix() {
    let capture_path = PathBuf::from(
        std::env::var_os("AUD145_PLACEMENT_CAPTURE_DIR")
            .expect("set AUD145_PLACEMENT_CAPTURE_DIR to a new capture directory"),
    );
    let capture_dir = if capture_path.is_absolute() {
        capture_path
    } else {
        repository_root().join(capture_path)
    };
    fs::create_dir_all(&capture_dir).expect("create placement capture directory");
    let reference_root = reference_root();

    for sample_rate in SAMPLE_RATES {
        for (channels, stereo_pairs) in [
            (2usize, vec![[0usize, 1usize]]),
            (5usize, vec![[0usize, 1usize], [3usize, 2usize]]),
        ] {
            let input_name = format!("{sample_rate}-{channels}ch-input.f32le");
            let input = decode_f32le(
                &fs::read(reference_root.join(&input_name)).expect("read frozen input vector"),
            );
            assert_eq!(input.len() % channels, 0);
            assert_eq!(
                input.len() / channels,
                4_096,
                "unexpected input frame count"
            );

            for (label, placements) in placement_sequences() {
                let configs = placements
                    .iter()
                    .copied()
                    .map(|placement| filter(Some(placement)))
                    .collect();
                let mut plugin = EqPlugin::from_params(
                    channels,
                    sample_rate,
                    params(configs, Some(stereo_pairs.clone())),
                )
                .expect("construct explicit ordered placement route");
                plugin
                    .plugin_initialize(sample_rate)
                    .expect("initialize explicit ordered placement route");
                let output = render(&mut plugin, &input, channels, sample_rate, FRAMES_PER_BLOCK);
                let output_name = format!("{sample_rate}-{channels}ch-{label}.f32le");
                fs::write(capture_dir.join(output_name), encode_f32le(&output))
                    .expect("write complete public-API output vector");
            }
        }
    }
}

/// Produces the 84 cold public-API vectors consumed by the independent
/// multirate transport reference. Capture is explicit because it writes files.
#[test]
#[ignore = "explicit multirate-reference capture; set AUD145_MULTIRATE_CAPTURE_DIR"]
fn capture_multirate_prefix_placement_matrix() {
    let capture_path = PathBuf::from(
        std::env::var_os("AUD145_MULTIRATE_CAPTURE_DIR")
            .expect("set AUD145_MULTIRATE_CAPTURE_DIR to a new capture directory"),
    );
    let capture_dir = if capture_path.is_absolute() {
        capture_path
    } else {
        repository_root().join(capture_path)
    };
    fs::create_dir_all(&capture_dir).expect("create multirate capture directory");

    let reference_root = repository_root().join("audit/artifacts/aud145-placement-reference-r1");
    for factor in [2u32, 4] {
        for sample_rate in SAMPLE_RATES {
            for (channels, stereo_pairs) in [
                (2usize, vec![[0usize, 1usize]]),
                (5usize, vec![[0usize, 1usize], [3usize, 2usize]]),
            ] {
                let input_name = format!("{sample_rate}-{channels}ch-input.f32le");
                let input = decode_f32le(
                    &fs::read(reference_root.join(input_name)).expect("read frozen input vector"),
                );
                assert_eq!(
                    input.len() / channels,
                    4_096,
                    "unexpected input frame count"
                );

                for (label, placements) in placement_sequences() {
                    let configs = placements
                        .iter()
                        .copied()
                        .map(|placement| filter(Some(placement)))
                        .collect();
                    let mut plugin = EqPlugin::from_params(
                        channels,
                        sample_rate,
                        params(configs, Some(stereo_pairs.clone())),
                    )
                    .expect("construct explicit multirate placement route");
                    plugin
                        .parametric_set_parameter(
                            ParameterId::from("auto_gain_enabled"),
                            ParameterValue::Bool(false),
                        )
                        .expect("disable full-plugin AutoGain for transport reference");
                    plugin
                        .plugin_initialize(sample_rate)
                        .expect("initialize cold multirate route");
                    plugin
                        .parametric_set_parameter(
                            ParameterId::from("oversampling"),
                            ParameterValue::Int(factor as i32),
                        )
                        .expect("select requested multirate factor");
                    assert_eq!(
                        plugin.parametric_get_parameter(&ParameterId::from("oversampling")),
                        Some(ParameterValue::Int(factor as i32)),
                        "requested oversampling factor was not retained"
                    );

                    let output =
                        render(&mut plugin, &input, channels, sample_rate, FRAMES_PER_BLOCK);
                    let output_name = format!("{factor}x-{sample_rate}-{channels}ch-{label}.f32le");
                    fs::write(capture_dir.join(output_name), encode_f32le(&output))
                        .expect("write complete public-API multirate vector");
                }
            }
        }
    }
}

#[test]
fn frozen_legacy_warped_mixed_case_matches_independent_corrected_reference() {
    assert!(
        frozen_legacy_case_names().contains(&"mixed_biquad_warped_kautz_stored_order".to_string())
    );
    let (actual, _, _) = render_frozen_legacy_case("mixed_biquad_warped_kautz_stored_order");
    assert_matches_corrected_legacy_reference("mixed_biquad_warped_kautz_stored_order", &actual);
}

#[test]
fn frozen_legacy_channel_filter_warped_case_matches_reference_and_preserves_right_channel() {
    assert!(frozen_legacy_case_names().contains(&"legacy_channel_filters_precedence".to_string()));
    let case_dir = repository_root()
        .join("audit/artifacts/aud145-preedit-baseline-r1/legacy_channel_filters_precedence");
    let (actual, actual_bytes, channels) =
        render_frozen_legacy_case("legacy_channel_filters_precedence");
    assert_eq!(channels, 2, "frozen precedence case is stereo");
    assert_matches_corrected_legacy_reference("legacy_channel_filters_precedence", &actual);

    let historical =
        decode_f32le(&fs::read(case_dir.join("output.f32le")).expect("read frozen output"));
    assert_eq!(
        channel_bytes(&actual, channels, 1),
        channel_bytes(&historical, channels, 1),
        "the unaffected right channel remains byte-exact"
    );
    assert_eq!(actual_bytes, encode_f32le(&actual));
}

#[test]
fn frozen_legacy_unaffected_cases_remain_bit_exact() {
    let case_names = frozen_legacy_case_names();
    for case_name in [
        "legacy_shared_oversampling_4x",
        "legacy_autogain_enabled_whole_plugin",
    ] {
        assert!(case_names.iter().any(|name| name == case_name));
        let case_dir = repository_root()
            .join("audit/artifacts/aud145-preedit-baseline-r1")
            .join(case_name);
        let (_, actual, _) = render_frozen_legacy_case(case_name);
        let historical = fs::read(case_dir.join("output.f32le")).expect("read frozen output");
        assert_eq!(
            actual, historical,
            "legacy sample stream changed for {case_name}"
        );
    }
}

#[test]
fn explicit_placements_reject_legacy_per_channel_filter_banks() {
    let mut nested = filter(Some(EqBandPlacement::Left));
    let nested_params = EqPluginParams {
        filters: Vec::new(),
        channel_filters: Some(vec![vec![nested.clone()], vec![nested.clone()]]),
        stereo_pairs: None,
        auto_gain: AutoGainParams::default(),
    };
    assert!(
        EqPlugin::from_params(2, 48_000, nested_params).is_err(),
        "nested channel_filters placement must be refused"
    );

    nested.placement = None;
    let mixed_params = EqPluginParams {
        filters: vec![filter(Some(EqBandPlacement::Mid))],
        channel_filters: Some(vec![vec![nested.clone()], vec![nested]]),
        stereo_pairs: None,
        auto_gain: AutoGainParams::default(),
    };
    assert!(
        EqPlugin::from_params(2, 48_000, mixed_params).is_err(),
        "global ordered placement must not be combined with channel_filters"
    );
}

#[test]
fn stereo_pairs_validate_even_when_no_band_uses_a_pair() {
    let invalid_pairs = [
        vec![[0usize, 0usize]],
        vec![[0usize, 5usize]],
        vec![[0usize, 1usize], [1usize, 2usize]],
    ];
    for stereo_pairs in invalid_pairs {
        assert!(
            EqPlugin::from_params(5, 48_000, params(vec![filter(None)], Some(stereo_pairs)),)
                .is_err(),
            "invalid dormant stereo-pair data must be rejected"
        );
    }

    assert!(
        EqPlugin::from_params(5, 48_000, params(vec![filter(None)], None)).is_ok(),
        "a legacy multichannel route should not require explicit pairs"
    );
    assert!(
        EqPlugin::from_params(
            2,
            48_000,
            params(vec![filter(Some(EqBandPlacement::Mid))], None),
        )
        .is_ok(),
        "stereo input should infer its one ordered pair"
    );
    assert!(
        EqPlugin::from_params(
            5,
            48_000,
            params(vec![filter(Some(EqBandPlacement::Side))], None),
        )
        .is_err(),
        "multichannel L/R/M/S route requires explicit pairs"
    );
}

#[test]
fn rejected_legacy_mixed_advanced_reinitialize_preserves_populated_epoch() {
    use sotf_host::ParametricPlugin;

    let construct = || {
        EqPlugin::from_params(2, 48_000, legacy_mixed_advanced_params())
            .expect("construct legacy mixed advanced route")
    };
    let mut actual = construct();
    let mut twin = construct();
    actual.plugin_initialize(48_000).unwrap();
    twin.plugin_initialize(48_000).unwrap();

    let prefix = (0..256 * 2)
        .map(|index| {
            if index == 0 {
                0.9
            } else {
                (index as f32 * 0.071).sin() * 0.2
            }
        })
        .collect::<Vec<_>>();
    render(&mut actual, &prefix, 2, 48_000, 37);
    render(&mut twin, &prefix, 2, 48_000, 37);

    assert!(
        actual.plugin_initialize(16_000).is_err(),
        "the 10 kHz Kautz pole is invalid below 20 kHz sample rate"
    );

    let continuation = (0..128 * 2)
        .map(|index| {
            if index == 0 {
                0.35
            } else {
                (index as f32 * 0.113).cos() * 0.07
            }
        })
        .collect::<Vec<_>>();
    let resumed = render(&mut actual, &continuation, 2, 48_000, 29);
    let expected = render(&mut twin, &continuation, 2, 48_000, 29);
    assert_eq!(
        encode_f32le(&resumed),
        encode_f32le(&expected),
        "a refused legacy reinitialize must leave every mixed-filter history unchanged"
    );

    let mut cold = construct();
    cold.plugin_initialize(48_000).unwrap();
    let cold_output = render(&mut cold, &continuation, 2, 48_000, 29);
    let maximum_cold_difference = resumed
        .iter()
        .zip(&cold_output)
        .map(|(populated, fresh)| (populated - fresh).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        maximum_cold_difference > 1.0e-5,
        "refusal continuation must prove the populated history matters; max difference={maximum_cold_difference}"
    );
}

#[test]
fn rejected_low_sample_rate_preserves_populated_biquad_and_autogain_epoch() {
    use sotf_host::ParametricPlugin;

    let construct = || {
        EqPlugin::from_params(2, 48_000, params(vec![filter(None)], None))
            .expect("construct legacy biquad route")
    };
    let mut actual = construct();
    let mut twin = construct();
    actual.plugin_initialize(48_000).unwrap();
    twin.plugin_initialize(48_000).unwrap();

    let prefix = (0..256 * 2)
        .map(|index| {
            if index == 0 {
                0.8
            } else {
                (index as f32 * 0.083).sin() * 0.17
            }
        })
        .collect::<Vec<_>>();
    render(&mut actual, &prefix, 2, 48_000, 41);
    render(&mut twin, &prefix, 2, 48_000, 41);

    assert!(
        actual.plugin_initialize(9).is_err(),
        "AutoGain's loudness meters reject sample rates below 10 Hz"
    );
    let continuation = (0..96 * 2)
        .map(|index| (index as f32 * 0.137).cos() * 0.09)
        .collect::<Vec<_>>();
    let resumed = render(&mut actual, &continuation, 2, 48_000, 31);
    let expected = render(&mut twin, &continuation, 2, 48_000, 31);
    assert_eq!(
        encode_f32le(&resumed),
        encode_f32le(&expected),
        "a rejected low sample rate must not commit the EQ sample rate or filter coefficients"
    );

    let mut cold = construct();
    cold.plugin_initialize(48_000).unwrap();
    let cold_output = render(&mut cold, &continuation, 2, 48_000, 31);
    let maximum_cold_difference = resumed
        .iter()
        .zip(&cold_output)
        .map(|(populated, fresh)| (populated - fresh).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        maximum_cold_difference > 1.0e-5,
        "refusal must prove populated history matters; max difference={maximum_cold_difference}"
    );
}
