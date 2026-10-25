//! Reconstruct the actual offline preference export through the production factory.

// Rust guideline compliant 2026-02-21
use super::create_plugin;
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

#[test]
fn ambisonics_preference_export_preserves_decoder_and_applies_only_diagonal_correction() {
    let exported: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/ambisonics-layout-preferences/preset.synthetic.json"
    ))
    .unwrap();
    // An actual emitted preset, serialized and reopened before factory construction.
    let reopened: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&exported).unwrap()).unwrap();
    let nodes = reopened["plugins"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["plugin_type"], "ambisonics_decoder");
    assert_eq!(nodes[1]["plugin_type"], "matrix");
    let config: AmbisonicsDecoderConfig =
        serde_json::from_value(nodes[0]["parameters"].clone()).unwrap();
    assert_eq!(config.order, 1);
    assert_eq!(config.target_layout, "5.1");
    assert!(config.dual_band);
    let matrix = nodes[1]["parameters"]["matrix"].as_array().unwrap();
    assert_eq!(matrix.len(), 36);
    for (index, value) in matrix.iter().enumerate() {
        let value = value.as_f64().unwrap();
        if index / 6 != index % 6 {
            assert_eq!(value, 0.0);
        } else {
            assert!((0.8..=1.25).contains(&value));
        }
    }
    assert_eq!(matrix[3 * 6 + 3], 1.0);
    let mut decoder =
        create_plugin("ambisonics_decoder", &nodes[0]["parameters"], 4, 48_000).unwrap();
    let mut correction = create_plugin("matrix", &nodes[1]["parameters"], 6, 48_000).unwrap();
    assert_eq!(
        (decoder.input_channels(), decoder.output_channels()),
        (4, 6)
    );
    assert_eq!(
        (correction.input_channels(), correction.output_channels()),
        (6, 6)
    );
    decoder.initialize(48_000.0).unwrap();
    correction.initialize(48_000.0).unwrap();
    assert_eq!(decoder.latency_samples(), 0);
    assert_eq!(correction.latency_samples(), 0);
    let frames = 8193;
    let mut input = vec![0.0_f32; frames * 4];
    for (index, value) in input[..4097 * 4].iter_mut().enumerate() {
        *value = ((index * 13 % 79) as f32 - 39.0) / 512.0;
    }
    let mut actual = vec![f32::NAN; frames * 6];
    for start in (0..frames).step_by(193) {
        let count = (frames - start).min(193);
        let context = ProcessContext::new(48_000, count).with_sample_position(start as u64);
        let mut decoded = vec![f32::NAN; count * 6];
        assert_eq!(
            decoder
                .process(
                    &input[start * 4..(start + count) * 4],
                    &mut decoded,
                    &context
                )
                .unwrap(),
            count
        );
        assert_eq!(
            correction
                .process(
                    &decoded,
                    &mut actual[start * 6..(start + count) * 6],
                    &context
                )
                .unwrap(),
            count
        );
    }
    let mut reference = AmbisonicsDecoderPlugin::new(&config).unwrap();
    reference.initialize(48_000.0).unwrap();
    let mut expected = vec![f32::NAN; frames * 6];
    reference
        .process(&input, &mut expected, &ProcessContext::new(48_000, frames))
        .unwrap();
    let gains: Vec<_> = (0..6)
        .map(|channel| matrix[channel * 6 + channel].as_f64().unwrap() as f32)
        .collect();
    let mut signal_change = false;
    for (index, (&actual, &dry)) in actual.iter().zip(&expected).enumerate() {
        let corrected = dry * gains[index % 6];
        assert!(actual.is_finite() && (actual - corrected).abs() <= 1e-7);
        signal_change |= (actual - dry).abs() > 1e-5;
        if index % 6 == 3 {
            assert_eq!(actual, 0.0);
        }
    }
    assert!(signal_change, "fixture must exercise a nonunity correction");
}
