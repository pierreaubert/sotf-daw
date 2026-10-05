//! Public-API coverage for tilt shape and per-band routing.
//!
//! Exercises only exported items: JSON state, parameter metadata, getters,
//! setters, and emitted audio.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_dynamic_eq::{DynamicEqPlugin, DynamicEqPluginParams};

fn make_tone(frequency: f64, sample_rate: u32, frames: usize, amplitude: f64) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let phase = 2.0 * std::f64::consts::PI * frequency * frame as f64 / sample_rate as f64;
            (amplitude * phase.sin()) as f32
        })
        .collect()
}

#[test]
fn schema_exposes_placement_and_tilt_shape_range() {
    let plugin = DynamicEqPlugin::new(2);
    let ids: Vec<String> = plugin
        .parameters()
        .iter()
        .map(|parameter| parameter.id.as_str().to_string())
        .collect();
    assert!(ids.contains(&"band_0_placement".to_string()));
    assert!(ids.contains(&"band_7_placement".to_string()));
    assert!(ids.contains(&"band_0_shape".to_string()));

    // Placement defaults to stereo (choice 0) on every stored slot.
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_3_placement")),
        Some(ParameterValue::Int(0))
    );
}

#[test]
fn json_state_carries_tilt_placement_and_pairs() {
    let json = r#"{
        "num_bands": 2,
        "threshold": -30.0,
        "ratio": 4.0,
        "attack_ms": 2.0,
        "release_ms": 60.0,
        "knee": 0.0,
        "link_channels": false,
        "mix": 1.0,
        "stereo_pairs": [[0, 1]],
        "bands": [
            {
                "shape": "tilt",
                "placement": "left",
                "frequency": 1000.0,
                "q": 1.0,
                "gain": 9.0,
                "shelf_slope": 1.0,
                "band_threshold": -30.0,
                "band_ratio": 4.0,
                "active": true,
                "solo": false
            },
            {
                "shape": "peak",
                "placement": "side",
                "frequency": 3000.0,
                "q": 1.4,
                "gain": -6.0,
                "shelf_slope": 1.0,
                "band_threshold": -30.0,
                "band_ratio": 4.0,
                "active": true,
                "solo": false
            }
        ]
    }"#;
    let params: DynamicEqPluginParams = serde_json::from_str(json).unwrap();
    let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(2, params, 48_000).unwrap();

    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_shape")),
        Some(ParameterValue::Int(3))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_placement")),
        Some(ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_placement")),
        Some(ParameterValue::Int(4))
    );

    // Unknown spellings are rejected instead of silently defaulting.
    assert!(
        serde_json::from_str::<DynamicEqPluginParams>(r#"{"bands": [{"shape": "comb"}]}"#,)
            .is_err()
    );
    assert!(
        serde_json::from_str::<DynamicEqPluginParams>(r#"{"bands": [{"placement": "diagonal"}]}"#,)
            .is_err()
    );

    // The tilt/left band audibly moves the left leg on real audio.
    let sample_rate = 48_000_u32;
    let frames = 8_192;
    let tone = make_tone(200.0, sample_rate, frames, 0.5);
    let mut input = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        input[frame * 2] = tone[frame];
        input[frame * 2 + 1] = tone[frame];
    }
    let mut audio = input.clone();
    plugin
        .process_in_place(&mut audio, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(audio.iter().all(|sample| sample.is_finite()));
    let moved = audio
        .iter()
        .zip(input.iter())
        .any(|(out, dry)| (out - dry).abs() > 1.0e-3);
    assert!(moved, "tilt/left band had no audible effect");
}

#[test]
fn placement_and_tilt_shape_are_structural_through_public_setters() {
    let mut plugin = DynamicEqPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();

    let error = plugin
        .set_parameter(
            ParameterId::from("band_0_placement"),
            ParameterValue::Int(2),
        )
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected error: {error}");

    let error = plugin
        .set_parameter(ParameterId::from("band_0_shape"), ParameterValue::Int(3))
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected error: {error}");
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_shape")),
        Some(ParameterValue::Int(0))
    );
}
