//! Offline preference exports preserve auditioned native settings and streaming behavior.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};

const PRESET: &str = include_str!("data/ml-preferences-preset.json");
const SESSION: &str = include_str!("data/ml-preferences-session.json");
const REPORT: &str = include_str!("data/ml-preferences-report.json");

#[test]
fn preference_export_is_exactly_an_auditioned_native_setting() {
    let exported: CrossfeedPluginParams = serde_json::from_str(PRESET).unwrap();
    let session: serde_json::Value = serde_json::from_str(SESSION).unwrap();
    let report: serde_json::Value = serde_json::from_str(REPORT).unwrap();
    assert_eq!(report["accepted"], true);
    assert_eq!(report["data_origin"], "synthetic_validation");
    let selected = session["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == report["selected_id"])
        .unwrap();
    let auditioned: CrossfeedPluginParams =
        serde_json::from_value(selected["parameters"].clone()).unwrap();
    assert_eq!(exported, auditioned);
    assert_eq!(exported.mode, CrossfeedMode::Bauer);
    assert!(!exported.autogain_enabled);
    let serialized = serde_json::to_string(&exported).unwrap();
    let reopened: CrossfeedPluginParams = serde_json::from_str(&serialized).unwrap();
    assert_eq!(exported, reopened);
}

fn render(rate: u32, block_frames: usize) -> Vec<f32> {
    let params: CrossfeedPluginParams = serde_json::from_str(PRESET).unwrap();
    let mut plugin = CrossfeedPlugin::new(params).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    // A left-only impulse and later anti-phase impulse exercise stereo memory.
    let mut output = vec![0.0; 16_386];
    output[0] = 0.5;
    output[2_000] = 0.25;
    output[2_001] = -0.25;
    for chunk in output.chunks_mut(block_frames * 2) {
        let frames = chunk.len() / 2;
        let processed = plugin
            .process_in_place(chunk, &ProcessContext::new(rate, frames))
            .unwrap();
        assert_eq!(processed, frames);
    }
    assert!(output.iter().all(|value| value.is_finite()));
    assert!(output[0].abs() > 0.01, "undelayed dry path");
    assert!(
        output
            .iter()
            .skip(1)
            .step_by(2)
            .any(|value| value.abs() > 1e-5)
    );
    output
}

#[test]
fn exported_setting_preserves_partial_callback_and_rate_contracts() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let expected = render(rate, 1_024);
        for frames in [1, 31, 257] {
            let actual = render(rate, frames);
            let maximum_error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                maximum_error <= 1e-6,
                "rate={rate}, block={frames}, error={maximum_error}"
            );
        }
    }
}
