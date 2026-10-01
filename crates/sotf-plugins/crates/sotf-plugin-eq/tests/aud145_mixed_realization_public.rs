//! AUD145 public mixed-realization and SVF-rate-boundary route checks.

// Rust guideline compliant 2026-02-21

use serde::Deserialize;
use sotf_host::{
    AutoGainParams, ParameterId, ParameterSet, ParameterValue, ParametricPlugin, ProcessContext,
};
use sotf_plugin_eq::{BiquadFilterConfig, EqPlugin, EqPluginParams};
use std::fs;
use std::path::{Path, PathBuf};

const SVF_SAMPLE_RATE: u32 = 48_000;
const SVF_CHANNELS: usize = 5;
const SVF_PAIRS: [[usize; 2]; 2] = [[0, 1], [3, 2]];
const SVF_FRAMES: usize = 2_048;
const CALLBACK_FRAMES: usize = 193;
const PEAK_LIMIT: f64 = 2.0e-5;
const RMS_LIMIT: f64 = 2.0e-6;

#[derive(Deserialize)]
struct MixedCaseSet {
    cases: Vec<MixedCase>,
}

#[derive(Deserialize)]
struct MixedCase {
    id: String,
    sample_rate: u32,
    factor: u32,
    channels: usize,
    frames: usize,
    pairs: Vec<[usize; 2]>,
    filters: Vec<BiquadFilterConfig>,
    input: String,
    latency: usize,
}

fn f32le_samples(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        bytes.len() % 4,
        0,
        "{} is not f32le aligned",
        path.display()
    );
    let (samples, remainder) = bytes.as_chunks::<4>();
    assert!(
        remainder.is_empty(),
        "input fixture has a partial f32 sample"
    );
    samples
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

fn write_f32le(path: PathBuf, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes).expect("write mixed public output");
}

fn render(plugin: &mut EqPlugin, input: &[f32], channels: usize, sample_rate: u32) -> Vec<f32> {
    assert_eq!(
        input.len() % channels,
        0,
        "input must contain complete frames"
    );
    let mut output = vec![0.0; input.len()];
    let block_samples = CALLBACK_FRAMES * channels;
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
            .expect("process public EQ callback");
        assert_eq!(processed, frames, "public EQ returned a short block");
    }
    assert_eq!(output.len(), input.len());
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

/// Capture the frozen 18-case mixed Biquad/Warped/Kautz matrix.
#[test]
#[ignore = "explicit AUD145 mixed-realization public capture"]
fn capture_mixed_realization_multirate_matrix() {
    let reference_dir = PathBuf::from(
        std::env::var_os("AUD145_MIXED_REFERENCE_DIR")
            .expect("set AUD145_MIXED_REFERENCE_DIR to the frozen case packet"),
    );
    let capture_dir = PathBuf::from(
        std::env::var_os("AUD145_MIXED_CAPTURE_DIR")
            .expect("set AUD145_MIXED_CAPTURE_DIR to a new absolute directory"),
    );
    assert!(
        reference_dir.is_absolute(),
        "reference path must be absolute"
    );
    assert!(capture_dir.is_absolute(), "capture path must be absolute");
    fs::create_dir_all(&capture_dir).expect("create mixed capture directory");

    let cases: MixedCaseSet = serde_json::from_slice(
        &fs::read(reference_dir.join("cases.json")).expect("read frozen case definitions"),
    )
    .expect("parse frozen mixed case definitions");
    assert_eq!(
        cases.cases.len(),
        18,
        "case count must match independent matrix"
    );

    for case in &cases.cases {
        assert_eq!(case.channels, 5, "{}: unexpected input width", case.id);
        let input_path = reference_dir
            .parent()
            .expect("case packet must have an artifacts parent")
            .join(&case.input);
        let input = f32le_samples(&input_path);
        assert_eq!(
            input.len(),
            case.frames * case.channels,
            "{}: input length",
            case.id
        );

        let mut plugin = EqPlugin::from_params(
            case.channels,
            case.sample_rate,
            EqPluginParams {
                filters: case.filters.clone(),
                channel_filters: None,
                stereo_pairs: Some(case.pairs.clone()),
                auto_gain: AutoGainParams::default(),
            },
        )
        .unwrap_or_else(|error| panic!("{}: construct public EQ: {error}", case.id));
        plugin
            .parametric_set_parameter(
                ParameterId::from("auto_gain_enabled"),
                ParameterValue::Bool(false),
            )
            .unwrap_or_else(|error| panic!("{}: disable AutoGain: {error}", case.id));
        plugin
            .parametric_set_parameter(
                ParameterId::from("oversampling"),
                ParameterValue::Int(i32::try_from(case.factor).expect("factor fits i32")),
            )
            .unwrap_or_else(|error| panic!("{}: select factor: {error}", case.id));
        plugin
            .plugin_initialize(case.sample_rate)
            .unwrap_or_else(|error| panic!("{}: initialize: {error}", case.id));
        assert_eq!(
            plugin.parametric_get_parameter(&ParameterId::from("oversampling")),
            Some(ParameterValue::Int(
                i32::try_from(case.factor).expect("factor fits i32")
            )),
            "{}: selected factor readback",
            case.id
        );
        assert_eq!(
            plugin.latency_samples(),
            case.latency,
            "{}: latency",
            case.id
        );

        let output = render(&mut plugin, &input, case.channels, case.sample_rate);
        assert!(output.iter().any(|sample| sample.abs() > 1.0e-7));
        write_f32le(capture_dir.join(format!("{}.f32le", case.id)), &output);
    }
}

fn svf_mixed_configs(reverse: bool) -> Vec<BiquadFilterConfig> {
    let ordinary = BiquadFilterConfig {
        filter_type: "peak".into(),
        freq: 2_200.0,
        q: 0.83,
        db_gain: 5.0,
        order: 2,
        topology: Default::default(),
        placement: Some(sotf_plugin_eq::EqBandPlacement::Left),
        lambda: None,
        kautz_sections: Vec::new(),
    };
    let warped = BiquadFilterConfig {
        filter_type: "lowpass".into(),
        freq: 6_100.0,
        q: 0.9,
        db_gain: 0.0,
        order: 2,
        topology: sotf_plugin_eq::EqFilterTopology::WarpedBiquad,
        placement: Some(sotf_plugin_eq::EqBandPlacement::Mid),
        lambda: None,
        kautz_sections: Vec::new(),
    };
    let kautz = BiquadFilterConfig {
        filter_type: "peak".into(),
        freq: 4_700.0,
        q: 1.7,
        db_gain: 0.0,
        order: 2,
        topology: sotf_plugin_eq::EqFilterTopology::KautzFilter,
        placement: Some(sotf_plugin_eq::EqBandPlacement::Right),
        lambda: None,
        kautz_sections: vec![
            sotf_plugin_eq::KautzSectionConfig {
                pole_freq: 4_700.0,
                q: 1.7,
                gain: -0.4,
            },
            sotf_plugin_eq::KautzSectionConfig {
                pole_freq: 10_000.0,
                q: 2.0,
                gain: 0.7,
            },
        ],
    };
    if reverse {
        vec![kautz, warped, ordinary]
    } else {
        vec![ordinary, warped, kautz]
    }
}

fn mixed_svf_input() -> Vec<f32> {
    let mut input = Vec::with_capacity(SVF_FRAMES * SVF_CHANNELS);
    for frame in 0..SVF_FRAMES {
        let time = frame as f64 / f64::from(SVF_SAMPLE_RATE);
        let common = 0.31 * (std::f64::consts::TAU * 733.0 * time).sin();
        let side = 0.19 * (std::f64::consts::TAU * 1_681.0 * time).cos();
        let second_left = 0.17 * (std::f64::consts::TAU * 419.0 * time).sin();
        let second_right = 0.12 * (std::f64::consts::TAU * 2_203.0 * time).cos();
        let unpaired = 0.09 * (std::f64::consts::TAU * 3_117.0 * time).sin();
        input.extend_from_slice(&[
            (common + side) as f32,
            (common - side) as f32,
            second_right as f32,
            second_left as f32,
            unpaired as f32,
        ]);
    }
    input
}

fn make_svf_plugin(configs: &[BiquadFilterConfig], factor: i32, factor_first: bool) -> EqPlugin {
    let mut plugin = EqPlugin::from_params(
        SVF_CHANNELS,
        SVF_SAMPLE_RATE,
        EqPluginParams {
            filters: configs.to_vec(),
            channel_filters: None,
            stereo_pairs: Some(SVF_PAIRS.to_vec()),
            auto_gain: AutoGainParams::default(),
        },
    )
    .expect("construct public mixed EQ");
    plugin
        .parametric_set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .expect("disable AutoGain for route comparison");
    let set_factor = |plugin: &mut EqPlugin| {
        plugin
            .parametric_set_parameter(
                ParameterId::from("oversampling"),
                ParameterValue::Int(factor),
            )
            .expect("retain selected oversampling factor");
    };
    let set_svf = |plugin: &mut EqPlugin| {
        plugin
            .parametric_set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
            .expect("select global SVF topology");
    };
    if factor_first {
        set_factor(&mut plugin);
        set_svf(&mut plugin);
    } else {
        set_svf(&mut plugin);
        set_factor(&mut plugin);
    }
    plugin
        .plugin_initialize(SVF_SAMPLE_RATE)
        .expect("initialize selected base-rate SVF route");
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("oversampling")),
        Some(ParameterValue::Int(factor))
    );
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("topology")),
        Some(ParameterValue::Int(1))
    );
    assert_eq!(plugin.latency_samples(), 0, "SVF bypasses FFT transport");
    plugin
}

fn make_biquad_plugin(configs: &[BiquadFilterConfig], factor: i32) -> EqPlugin {
    let mut plugin = EqPlugin::from_params(
        SVF_CHANNELS,
        SVF_SAMPLE_RATE,
        EqPluginParams {
            filters: configs.to_vec(),
            channel_filters: None,
            stereo_pairs: Some(SVF_PAIRS.to_vec()),
            auto_gain: AutoGainParams::default(),
        },
    )
    .expect("construct public Biquad EQ");
    plugin
        .parametric_set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .expect("disable AutoGain for route comparison");
    plugin
        .parametric_set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(factor),
        )
        .expect("select Biquad oversampling factor");
    plugin
        .plugin_initialize(SVF_SAMPLE_RATE)
        .expect("initialize Biquad route");
    plugin
}

fn compare_full_vectors(actual: &[f32], expected: &[f32], case: &str) {
    assert_eq!(actual.len(), expected.len(), "{case}: complete frame count");
    let mut peak = 0.0_f64;
    let mut energy = 0.0_f64;
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            actual.is_finite() && expected.is_finite(),
            "{case}: finite vectors"
        );
        let error = f64::from((*actual - *expected).abs());
        peak = peak.max(error);
        energy += error * error;
    }
    let rms = (energy / actual.len() as f64).sqrt();
    assert!(peak <= PEAK_LIMIT, "{case}: peak error {peak:e}");
    assert!(rms <= RMS_LIMIT, "{case}: RMS error {rms:e}");
}

fn vector_error_metrics(actual: &[f32], expected: &[f32]) -> (f64, f64) {
    assert_eq!(actual.len(), expected.len(), "complete vectors must align");
    let mut peak = 0.0_f64;
    let mut energy = 0.0_f64;
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(actual.is_finite() && expected.is_finite());
        let error = f64::from((*actual - *expected).abs());
        peak = peak.max(error);
        energy += error * error;
    }
    (peak, (energy / actual.len() as f64).sqrt())
}

/// Global SVF and its ordered advanced neighbors stay at the base sample rate.
#[test]
fn selected_oversampling_is_bypassed_for_global_svf_and_reactivated_on_return() {
    let input = mixed_svf_input();
    for reverse in [false, true] {
        let configs = svf_mixed_configs(reverse);
        let mut base_rate_reference = make_svf_plugin(&configs, 1, true);
        assert!(matches!(
            base_rate_reference.tail_length(),
            sotf_host::plugin::TailLength::Unknown
        ));
        let expected = render(
            &mut base_rate_reference,
            &input,
            SVF_CHANNELS,
            SVF_SAMPLE_RATE,
        );
        assert_ne!(expected, input, "the configured route must process audio");

        for factor in [1, 2, 4] {
            let factor_first = make_svf_plugin(&configs, factor, true);
            let factor_last = make_svf_plugin(&configs, factor, false);
            let outputs = [factor_first, factor_last]
                .into_iter()
                .map(|mut plugin| render(&mut plugin, &input, SVF_CHANNELS, SVF_SAMPLE_RATE))
                .collect::<Vec<_>>();
            let case = format!("svf-base-rate-reverse={reverse}-factor={factor}");
            compare_full_vectors(&outputs[0], &expected, &case);
            assert_eq!(outputs[0], outputs[1], "{case}: setter order changed audio");

            if factor == 4 && !reverse {
                let snapshot = make_svf_plugin(&configs, factor, true).current_values();
                let bytes = serde_json::to_vec(&snapshot).expect("serialize parameter snapshot");
                let restored_values: ParameterSet =
                    serde_json::from_slice(&bytes).expect("restore parameter snapshot");
                let mut restored = EqPlugin::from_params(
                    SVF_CHANNELS,
                    SVF_SAMPLE_RATE,
                    EqPluginParams {
                        filters: configs.clone(),
                        channel_filters: None,
                        stereo_pairs: Some(SVF_PAIRS.to_vec()),
                        auto_gain: AutoGainParams::default(),
                    },
                )
                .expect("construct restored public EQ");
                restored
                    .apply_values(restored_values)
                    .expect("restore selected SVF and factor values");
                assert_eq!(
                    restored.parametric_get_parameter(&ParameterId::from("topology")),
                    Some(ParameterValue::Int(1))
                );
                assert_eq!(
                    restored.parametric_get_parameter(&ParameterId::from("oversampling")),
                    Some(ParameterValue::Int(factor))
                );
                assert_eq!(
                    restored.parametric_get_parameter(&ParameterId::from("auto_gain_enabled")),
                    Some(ParameterValue::Bool(false))
                );
                assert_eq!(restored.current_values(), snapshot);
                restored
                    .plugin_initialize(SVF_SAMPLE_RATE)
                    .expect("initialize restored state");
                assert_eq!(restored.latency_samples(), 0);
                let restored_output = render(&mut restored, &input, SVF_CHANNELS, SVF_SAMPLE_RATE);
                let (max_error, rms_error) = vector_error_metrics(&outputs[0], &restored_output);
                println!(
                    "svf serde f32 roundtrip residual: peak={max_error:.17e}, rms={rms_error:.17e}"
                );
                assert!(
                    max_error <= 2.0e-5,
                    "saved state changed route: {max_error:e}"
                );
                assert!(
                    rms_error <= RMS_LIMIT,
                    "saved state changed route RMS: {rms_error:e}"
                );

                // Parameter serialization stores scalar UI controls as f32,
                // while the public DSP configuration starts with f64 values.
                // Rebuild a fresh plugin from precisely the restored values;
                // it must be bit-identical to the deserialized plugin.
                let mut roundtripped_configs = configs.clone();
                let restored_float = |field: &str| match restored
                    .parametric_get_parameter(&ParameterId::from(format!("band_0_{field}")))
                {
                    Some(ParameterValue::Float(value)) => f64::from(value),
                    _ => panic!("restored band_0_{field} must be a float"),
                };
                roundtripped_configs[0].freq = restored_float("freq");
                roundtripped_configs[0].q = restored_float("q");
                roundtripped_configs[0].db_gain = restored_float("gain");
                let mut fresh_roundtripped = make_svf_plugin(&roundtripped_configs, factor, true);
                let fresh_output = render(
                    &mut fresh_roundtripped,
                    &input,
                    SVF_CHANNELS,
                    SVF_SAMPLE_RATE,
                );
                assert_eq!(
                    restored_output, fresh_output,
                    "restored state must match fresh DSP from exact saved values"
                );
            }

            if factor == 4 && !reverse {
                // Fill a real 4x transport queue, switch to base-rate SVF,
                // then switch back. Each realization boundary must start a
                // fresh epoch and discard the old resampler history.
                let mut queued = make_biquad_plugin(&configs, factor);
                assert!(queued.latency_samples() > 0);
                let warm = vec![0.07_f32; 73 * SVF_CHANNELS];
                let _ = render(&mut queued, &warm, SVF_CHANNELS, SVF_SAMPLE_RATE);
                queued
                    .parametric_set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
                    .expect("switch populated transport route to SVF");
                assert_eq!(queued.latency_samples(), 0);
                let mut fresh_svf = make_svf_plugin(&configs, factor, true);
                let switched_svf = render(&mut queued, &input, SVF_CHANNELS, SVF_SAMPLE_RATE);
                let cold_svf = render(&mut fresh_svf, &input, SVF_CHANNELS, SVF_SAMPLE_RATE);
                assert_eq!(
                    switched_svf, cold_svf,
                    "SVF switch must discard old transport state"
                );

                queued
                    .parametric_set_parameter(ParameterId::from("topology"), ParameterValue::Int(0))
                    .expect("switch back to requested oversampling route");
                assert!(queued.latency_samples() > 0);
                let mut fresh = make_biquad_plugin(&configs, factor);
                let resumed = render(&mut queued, &input, SVF_CHANNELS, SVF_SAMPLE_RATE);
                let cold = render(&mut fresh, &input, SVF_CHANNELS, SVF_SAMPLE_RATE);
                assert_eq!(resumed, cold, "switch-back must reset the old FFT queue");
            }
        }
    }
}
