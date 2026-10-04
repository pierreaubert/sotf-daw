//! AUD145 public Warped-family and Kautz impulse captures.
//!
//! These invoke EqPlugin directly and write complete single-channel output
//! vectors for the independent analog/polynomial references.

// Rust guideline compliant 2026-02-21

use sotf_host::{AutoGainParams, ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
    KautzSectionConfig,
};
use std::fs;
use std::path::PathBuf;

const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
const FRAMES: usize = 16_384;
const BLOCK_FRAMES: usize = 193;
const CENTER_HZ: f64 = 1_379.0;

fn public_plugin(sample_rate: u32, filter: BiquadFilterConfig) -> EqPlugin {
    let mut plugin = EqPlugin::from_params(
        1,
        sample_rate,
        EqPluginParams {
            filters: vec![filter],
            channel_filters: None,
            stereo_pairs: None,
            auto_gain: AutoGainParams::default(),
        },
    )
    .expect("construct public one-channel EQ");
    plugin
        .parametric_set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .expect("disable AutoGain for the filter response");
    plugin
        .parametric_set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(1))
        .expect("select 1x processing");
    plugin
        .plugin_initialize(f64::from(sample_rate))
        .expect("initialize public EQ");
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("oversampling")),
        Some(ParameterValue::Int(1))
    );
    plugin
}

fn render(plugin: &mut EqPlugin, input: &[f32], sample_rate: u32) -> Vec<f32> {
    let mut output = vec![0.0_f32; input.len()];
    for start in (0..input.len()).step_by(BLOCK_FRAMES) {
        let frames = (input.len() - start).min(BLOCK_FRAMES);
        let processed = plugin
            .process(
                &input[start..start + frames],
                &mut output[start..start + frames],
                &ProcessContext::new(sample_rate, frames),
            )
            .expect("process public EQ block");
        assert_eq!(processed, frames, "public EQ returned a short callback");
    }
    assert_eq!(output.len(), input.len());
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));
    output
}

fn impulse() -> Vec<f32> {
    let mut input = vec![0.0_f32; FRAMES];
    input[0] = 1.0;
    input
}

fn write_f32(path: PathBuf, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes).expect("write public EQ capture");
}

fn warped_filter(filter_type: &str, lambda: Option<f64>) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: filter_type.into(),
        freq: CENTER_HZ,
        q: 0.83,
        db_gain: 0.0,
        order: 2,
        topology: EqFilterTopology::WarpedBiquad,
        placement: Some(EqBandPlacement::Stereo),
        lambda,
        kautz_sections: Vec::new(),
    }
}

/// Capture the 48 non-Peak Warped impulse cases from the frozen reference.
#[test]
#[ignore = "explicit AUD145 non-Peak Warped accuracy capture"]
fn capture_public_warped_family_matrix() {
    let capture_dir = PathBuf::from(
        std::env::var_os("AUD145_WARPED_FAMILIES_CAPTURE_DIR")
            .expect("set AUD145_WARPED_FAMILIES_CAPTURE_DIR to a new directory"),
    );
    fs::create_dir_all(&capture_dir).expect("create Warped family capture directory");
    let filter_types = ["lowpass", "highpass", "notch", "allpass"];
    let lambda_modes = [
        ("zero", Some(0.0)),
        ("positive", Some(0.37)),
        ("negative", Some(-0.37)),
        ("auto_bark", None),
    ];
    let mut count = 0;

    for sample_rate in SAMPLE_RATES {
        for (lambda_name, lambda) in lambda_modes {
            for filter_type in filter_types {
                let mut plugin = public_plugin(sample_rate, warped_filter(filter_type, lambda));
                let output = render(&mut plugin, &impulse(), sample_rate);
                let path = capture_dir.join(format!(
                    "warped-{sample_rate}-{lambda_name}-{filter_type}.f32"
                ));
                write_f32(path, &output);
                count += 1;
            }
        }
    }
    assert_eq!(count, 48, "capture matrix must match cases.json");
}

fn kautz_sections(case: &str) -> Vec<KautzSectionConfig> {
    let first = KautzSectionConfig {
        pole_freq: 4_700.0,
        q: 1.7,
        gain: -2.0,
    };
    let second = KautzSectionConfig {
        pole_freq: 10_000.0,
        q: 2.0,
        gain: 4.0,
    };
    match case {
        "single" => vec![first],
        "two" => vec![first, second],
        "reversed" => vec![second, first],
        other => panic!("unknown Kautz capture case {other}"),
    }
}

fn kautz_filter(sections: Vec<KautzSectionConfig>, db_gain: f64) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: "peak".into(),
        freq: CENTER_HZ,
        q: 0.83,
        db_gain,
        order: 2,
        topology: EqFilterTopology::KautzFilter,
        placement: Some(EqBandPlacement::Stereo),
        lambda: None,
        kautz_sections: sections,
    }
}

/// Capture the nine explicit-section public Kautz impulse cases.
#[test]
#[ignore = "explicit AUD145 Kautz accuracy capture"]
fn capture_public_kautz_matrix() {
    let capture_dir = PathBuf::from(
        std::env::var_os("AUD145_KAUTZ_CAPTURE_DIR")
            .expect("set AUD145_KAUTZ_CAPTURE_DIR to a new directory"),
    );
    fs::create_dir_all(&capture_dir).expect("create Kautz capture directory");
    let mut count = 0;

    for sample_rate in SAMPLE_RATES {
        for case in ["single", "two", "reversed"] {
            let filter = kautz_filter(kautz_sections(case), 0.0);
            let mut plugin = public_plugin(sample_rate, filter);
            let output = render(&mut plugin, &impulse(), sample_rate);
            write_f32(
                capture_dir.join(format!("kautz-{sample_rate}-{case}.f32")),
                &output,
            );
            count += 1;
        }
    }
    assert_eq!(count, 9, "capture matrix must match cases.json");
}

fn process_pattern(plugin: &mut EqPlugin, sample_rate: u32) -> Vec<f32> {
    let input: Vec<f32> = (0..FRAMES)
        .map(|frame| {
            let time = frame as f64 / f64::from(sample_rate);
            let tone = 0.23 * (std::f64::consts::TAU * 733.0 * time).sin()
                + 0.11 * (std::f64::consts::TAU * 1_681.0 * time).cos();
            (tone + if frame == 0 { 0.5 } else { 0.0 }) as f32
        })
        .collect();
    render(plugin, &input, sample_rate)
}

fn max_difference(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| f64::from((*left - *right).abs()))
        .fold(0.0_f64, f64::max)
}

#[test]
fn omitted_kautz_sections_preserve_legacy_linear_weight_across_json_restore() {
    let sample_rate = 48_000;
    let legacy_filter = kautz_filter(Vec::new(), -2.0);
    let legacy_params = EqPluginParams {
        filters: vec![legacy_filter],
        channel_filters: None,
        stereo_pairs: None,
        auto_gain: AutoGainParams::default(),
    };
    let saved = serde_json::to_string(&legacy_params).expect("serialize legacy Kautz settings");
    let restored: EqPluginParams = serde_json::from_str(&saved).expect("restore legacy settings");
    assert_eq!(restored.filters[0].db_gain, -2.0);
    assert!(restored.filters[0].kautz_sections.is_empty());

    let mut fallback = public_plugin(sample_rate, restored.filters[0].clone());
    let mut explicit_weight = public_plugin(
        sample_rate,
        kautz_filter(
            vec![KautzSectionConfig {
                pole_freq: CENTER_HZ,
                q: 0.83,
                gain: -2.0,
            }],
            0.0,
        ),
    );
    let mut db_converted_weight = public_plugin(
        sample_rate,
        kautz_filter(
            vec![KautzSectionConfig {
                pole_freq: CENTER_HZ,
                q: 0.83,
                gain: 10.0_f64.powf(-2.0 / 20.0),
            }],
            0.0,
        ),
    );

    let fallback_output = process_pattern(&mut fallback, sample_rate);
    let explicit_output = process_pattern(&mut explicit_weight, sample_rate);
    let converted_output = process_pattern(&mut db_converted_weight, sample_rate);
    assert_eq!(fallback_output, explicit_output);
    assert!(
        max_difference(&fallback_output, &converted_output) > 1.0e-3,
        "legacy -2 value is a linear modal weight and must not be converted from dB"
    );
}
