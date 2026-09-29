//! Denoiser constructor controls survive the public facade.

// Rust guideline compliant 2026-02-21
use serde_json::json;
use sotf_plugins::{ParameterId, ParameterValue, create_plugin};

#[test]
fn facade_retains_denoiser_persistent_controls() {
    let config = json!({"harmonic_percussive":true,"spatial_denoise":true,"spatial_strength":0.9});
    let typed: sotf_plugins::DenoiserPluginParams = serde_json::from_value(config).unwrap();
    let config = serde_json::to_value(typed).unwrap();
    let mut plugin = create_plugin("denoiser", &config, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    for (id, expected) in [
        ("harmonic_percussive", ParameterValue::Bool(true)),
        ("spatial_denoise", ParameterValue::Bool(true)),
        ("spatial_strength", ParameterValue::Float(0.9)),
    ] {
        assert_eq!(plugin.get_parameter(&ParameterId::from(id)), Some(expected));
    }
}

#[test]
fn facade_rejects_invalid_denoiser_constructor_controls() {
    for config in [
        json!({"spatial_strength":-0.01}),
        json!({"spatial_strength":1.01}),
        json!({"harmonic_percussive":"true"}),
        json!({"spatial_denoise":1}),
    ] {
        assert!(create_plugin("denoiser", &config, 2, 48_000).is_err());
    }
}
