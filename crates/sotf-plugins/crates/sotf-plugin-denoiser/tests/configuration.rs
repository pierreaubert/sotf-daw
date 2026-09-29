//! Persistent constructor settings must match the existing scalar controls.

// Rust guideline compliant 2026-02-21
use serde_json::json;
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::{ParameterId, ParameterValue, ProcessContext};
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};

fn values(harmonic: bool, spatial: bool, strength: f32) -> [(&'static str, ParameterValue); 3] {
    [
        ("harmonic_percussive", ParameterValue::Bool(harmonic)),
        ("spatial_denoise", ParameterValue::Bool(spatial)),
        ("spatial_strength", ParameterValue::Float(strength)),
    ]
}

#[test]
fn old_defaults_and_nondefault_json_roundtrip_preserve_persistent_controls() {
    let old: DenoiserPluginParams = serde_json::from_str("{}").unwrap();
    assert!(!old.harmonic_percussive);
    assert!(!old.spatial_denoise);
    assert_eq!(old.spatial_strength, 0.5);
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        serde_json::to_value(DenoiserPluginParams::default()).unwrap()
    );
    let config = DenoiserPluginParams {
        harmonic_percussive: true,
        spatial_denoise: true,
        spatial_strength: 0.9,
        ..Default::default()
    };
    let encoded = serde_json::to_value(config).unwrap();
    assert_eq!(encoded["harmonic_percussive"], true);
    assert_eq!(encoded["spatial_denoise"], true);
    let restored: DenoiserPluginParams = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(restored.spatial_strength, 0.9);
    assert_eq!(serde_json::to_value(restored).unwrap(), encoded);
}

#[test]
fn construction_preserves_types_indices_and_values_before_and_after_initialize() {
    for channels in [1, 2] {
        for strength in [0.0, 0.9, 1.0] {
            let mut plugin = DenoiserPlugin::try_from_params(
                channels,
                DenoiserPluginParams {
                    harmonic_percussive: true,
                    spatial_denoise: true,
                    spatial_strength: strength,
                    ..Default::default()
                },
            )
            .unwrap();
            for initialize in [false, true] {
                if initialize {
                    plugin.initialize(48_000).unwrap();
                }
                for ((id, expected), index) in values(true, true, strength).into_iter().zip(26..=28)
                {
                    assert_eq!(
                        plugin.parametric_get_parameter(&ParameterId::from(id)),
                        Some(expected)
                    );
                    assert_eq!(sotf_plugin_denoiser::params::PARAMS[index].engine_key, id);
                }
            }
        }
    }
}

#[test]
fn invalid_strength_and_json_types_are_rejected() {
    for strength in [-0.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let result = DenoiserPlugin::try_from_params(
            2,
            DenoiserPluginParams {
                spatial_strength: strength,
                ..Default::default()
            },
        );
        let error = result
            .err()
            .expect("invalid strength must fail construction");
        assert!(error.contains("spatial_strength"), "{error}");
    }
    for bad in [
        json!({"harmonic_percussive": 1}),
        json!({"spatial_denoise": "true"}),
        json!({"spatial_strength": true}),
        json!({"spatial_strength": null}),
    ] {
        assert!(serde_json::from_value::<DenoiserPluginParams>(bad).is_err());
    }
}

#[test]
fn constructor_does_not_promote_profile_actions_to_persistent_fields() {
    let config: DenoiserPluginParams = serde_json::from_value(json!({
        "learn_noise": true, "clear_profile": true, "use_captured_profile": true,
        "harmonic_percussive": true, "spatial_denoise": true, "spatial_strength": 0.9,
    }))
    .unwrap();
    let encoded = serde_json::to_value(&config).unwrap();
    assert!(encoded.get("learn_noise").is_none());
    assert!(encoded.get("clear_profile").is_none());
    let plugin = DenoiserPlugin::try_from_params(2, config).unwrap();
    for id in ["learn_noise", "clear_profile"] {
        assert_eq!(
            plugin.parametric_get_parameter(&ParameterId::from(id)),
            Some(ParameterValue::Bool(false))
        );
    }
    // An ignored clear command must not clear the persistent profile-use flag.
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(true))
    );
}

#[test]
fn configured_modes_match_named_setters_through_irregular_audio_and_reset() {
    // Distinct channel content exercises spatial coherence; sparse impulses
    // exercise the harmonic/percussive transient preservation path.
    let signal: Vec<f32> = (0..8193)
        .flat_map(|frame| {
            let tone = (frame as f32 * 0.073).sin() * 0.1;
            let noise = ((frame * 7919 % 997) as f32 / 997.0 - 0.5) * 0.04;
            let impulse = if frame % 997 == 0 { 0.4 } else { 0.0 };
            [tone + noise + impulse, -tone * 0.3 + noise * 0.7]
        })
        .collect();
    for low_latency in [false, true] {
        for multi_resolution in [false, true] {
            for (harmonic, spatial) in [(false, true), (true, false), (true, true)] {
                let base = DenoiserPluginParams {
                    low_latency,
                    multi_resolution,
                    ..Default::default()
                };
                let mut reference = DenoiserPlugin::try_from_params(2, base.clone()).unwrap();
                let mut configured = DenoiserPlugin::try_from_params(
                    2,
                    DenoiserPluginParams {
                        harmonic_percussive: harmonic,
                        spatial_denoise: spatial,
                        spatial_strength: 0.9,
                        ..base
                    },
                )
                .unwrap();
                configured.initialize(48_000).unwrap();
                reference.initialize(48_000).unwrap();
                for (id, value) in values(harmonic, spatial, 0.9) {
                    reference
                        .parametric_set_parameter(ParameterId::from(id), value)
                        .unwrap();
                }
                assert_eq!(configured.current_values(), reference.current_values());
                for after_reset in [false, true] {
                    if after_reset {
                        configured.reset();
                        reference.reset();
                    }
                    let mut output = signal.clone();
                    let mut expected = signal.clone();
                    let mut offset = 0;
                    let mut energy = 0.0;
                    for frames in [1, 17, 257, 63, 1024].into_iter().cycle() {
                        let frames = frames.min(signal.len() / 2 - offset);
                        if frames == 0 {
                            break;
                        }
                        let samples = offset * 2..(offset + frames) * 2;
                        let context = ProcessContext::new(48_000, frames);
                        assert_eq!(
                            configured
                                .process_in_place(&mut output[samples.clone()], &context)
                                .unwrap(),
                            frames
                        );
                        assert_eq!(
                            reference
                                .process_in_place(&mut expected[samples.clone()], &context)
                                .unwrap(),
                            frames
                        );
                        for sample in &output[samples] {
                            energy += sample.abs();
                        }
                        offset += frames;
                    }
                    assert!(energy > 1.0, "the oracle must exercise audible output");
                    assert_eq!(
                        output, expected,
                        "low={low_latency}, multi={multi_resolution}, harmonic={harmonic}, spatial={spatial}, reset={after_reset}"
                    );
                }
            }
        }
    }
}
