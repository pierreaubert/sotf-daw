//! Denoiser JSON and saved state retain persistent mode controls.

// Rust guideline compliant 2026-02-21
use plugins_bridge::{
    create_plugin,
    state::{load_state, save_state},
};
use sotf_host::{ParameterId, ParameterValue};

#[test]
fn bridge_constructor_and_state_roundtrip_retain_denoiser_modes() {
    let mut plugin = create_plugin(
        "Denoiser",
        2,
        48_000,
        r#"{"harmonic_percussive":true,"spatial_denoise":true,"spatial_strength":0.9,"curve_low":0.0,"curve_mid":0.5,"curve_high":1.0,"audition_residual":true}"#,
    )
    .unwrap();
    plugin.initialize(48_000).unwrap();
    for (id, expected) in [
        ("harmonic_percussive", ParameterValue::Bool(true)),
        ("spatial_denoise", ParameterValue::Bool(true)),
        ("spatial_strength", ParameterValue::Float(0.9)),
        ("curve_low", ParameterValue::Float(0.0)),
        ("curve_mid", ParameterValue::Float(0.5)),
        ("curve_high", ParameterValue::Float(1.0)),
        ("audition_residual", ParameterValue::Bool(true)),
    ] {
        assert_eq!(plugin.get_parameter(&ParameterId::from(id)), Some(expected));
    }
    let saved = save_state(plugin.as_ref());
    let mut restored = create_plugin("Denoiser", 2, 48_000, "{}").unwrap();
    restored.initialize(48_000).unwrap();
    load_state(restored.as_mut(), &saved).unwrap();
    assert_eq!(save_state(restored.as_ref()), saved);
}

#[test]
fn bridge_rejects_invalid_denoiser_constructor_controls() {
    for config in [
        r#"{"spatial_strength":-0.01}"#,
        r#"{"spatial_strength":1.01}"#,
        r#"{"harmonic_percussive":1}"#,
        r#"{"spatial_denoise":"true"}"#,
        r#"{"curve_low":-0.01}"#,
        r#"{"curve_mid":1.01}"#,
        r#"{"curve_high":"0.5"}"#,
        r#"{"audition_residual":1}"#,
    ] {
        assert!(create_plugin("Denoiser", 2, 48_000, config).is_err());
    }
}
