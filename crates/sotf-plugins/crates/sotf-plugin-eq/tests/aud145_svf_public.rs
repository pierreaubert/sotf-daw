//! AUD145 public SVF captures for the frozen independent analog reference.

// Rust guideline compliant 2026-02-21

use sotf_host::{AutoGainParams, ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
};
use std::fs;
use std::path::PathBuf;

const FRAMES: usize = 16_384;
const BLOCK_FRAMES: usize = 193;
const CENTER_HZ: f64 = 1_379.0;

struct Case {
    id: String,
    sample_rate: u32,
    filter_type: &'static str,
    q: f64,
    gain_db: f64,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::with_capacity(48);
    for sample_rate in [44_100, 48_000, 96_000] {
        for filter_type in ["lowshelf", "highshelf"] {
            for (q_label, q) in [("0.5", 0.5), ("0.83", 0.83), ("2", 2.0)] {
                for gain_db in [-7.0, 7.0] {
                    let gain_label = if gain_db < 0.0 { "gain-7" } else { "gain7" };
                    cases.push(Case {
                        id: format!("svf-{sample_rate}-{filter_type}-q{q_label}-{gain_label}"),
                        sample_rate,
                        filter_type,
                        q,
                        gain_db,
                    });
                }
            }
        }
        for (filter_type, q, gain_db, q_label, gain_label) in [
            ("peak", 0.83, 0.0, "0.83", "gain0"),
            ("peak", 0.83, 7.0, "0.83", "gain7"),
            ("allpass", 0.83, 0.0, "0.83", "gain0"),
            ("highpass", 0.83, 0.0, "0.83", "gain0"),
        ] {
            cases.push(Case {
                id: format!("svf-{sample_rate}-{filter_type}-q{q_label}-{gain_label}"),
                sample_rate,
                filter_type,
                q,
                gain_db,
            });
        }
    }
    assert_eq!(
        cases.len(),
        48,
        "capture matrix must match frozen cases.json"
    );
    cases
}

fn render(case: &Case) -> Vec<f32> {
    let mut plugin = EqPlugin::from_params(
        1,
        case.sample_rate,
        EqPluginParams {
            filters: vec![BiquadFilterConfig {
                filter_type: case.filter_type.to_string(),
                freq: CENTER_HZ,
                q: case.q,
                db_gain: case.gain_db,
                order: 2,
                topology: EqFilterTopology::Biquad,
                placement: Some(EqBandPlacement::Stereo),
                lambda: None,
                kautz_sections: Vec::new(),
            }],
            channel_filters: None,
            stereo_pairs: None,
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
        .parametric_set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(1))
        .unwrap_or_else(|error| panic!("{}: select 1x processing: {error}", case.id));
    plugin
        .plugin_initialize(f64::from(case.sample_rate))
        .unwrap_or_else(|error| panic!("{}: initialize EQ: {error}", case.id));
    plugin
        .parametric_set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
        .unwrap_or_else(|error| panic!("{}: select SVF topology: {error}", case.id));

    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("oversampling")),
        Some(ParameterValue::Int(1)),
        "{}: oversampling readback",
        case.id
    );
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("topology")),
        Some(ParameterValue::Int(1)),
        "{}: topology readback",
        case.id
    );

    let mut input = vec![0.0_f32; FRAMES];
    input[0] = 1.0;
    let mut output = vec![0.0_f32; FRAMES];
    for start in (0..FRAMES).step_by(BLOCK_FRAMES) {
        let frames = (FRAMES - start).min(BLOCK_FRAMES);
        let processed = plugin
            .process(
                &input[start..start + frames],
                &mut output[start..start + frames],
                &ProcessContext::new(case.sample_rate, frames),
            )
            .unwrap_or_else(|error| panic!("{}: process public EQ: {error}", case.id));
        assert_eq!(processed, frames, "{}: short callback", case.id);
    }
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{}: non-finite output",
        case.id
    );
    assert!(
        output.iter().any(|sample| sample.abs() > 1.0e-6),
        "{}: trivial output",
        case.id
    );
    output
}

/// Capture all production SVF impulse vectors for the frozen independent oracle.
#[test]
#[ignore = "explicit 48-case SVF accuracy capture; set AUD145_SVF_CAPTURE_DIR"]
fn capture_public_svf_accuracy_matrix() {
    let capture_dir = PathBuf::from(
        std::env::var_os("AUD145_SVF_CAPTURE_DIR")
            .expect("set AUD145_SVF_CAPTURE_DIR to a new output directory"),
    );
    fs::create_dir_all(&capture_dir).expect("create capture directory");

    for case in cases() {
        let output = render(&case);
        let mut bytes = Vec::with_capacity(output.len() * std::mem::size_of::<f32>());
        for sample in output {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(capture_dir.join(format!("{}.f32", case.id)), bytes)
            .unwrap_or_else(|error| panic!("{}: write vector: {error}", case.id));
    }
}
