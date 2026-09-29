//! Persisted engine Denoiser settings reach the public DSP constructor.

// Rust guideline compliant 2026-02-21
use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::{ParameterId, ParameterValue, create_plugin};

#[test]
fn engine_denoiser_settings_roundtrip_reaches_dsp() {
    for strength in [0.0, 0.9, 1.0, 1.01] {
        let mut settings = PluginSettings::default_for(&PluginType::Denoiser).unwrap();
        let PluginSettings::Denoiser {
            harmonic_percussive,
            spatial_denoise,
            spatial_strength,
            ..
        } = &mut settings
        else {
            panic!("Denoiser default must have Denoiser settings")
        };
        *harmonic_percussive = true;
        *spatial_denoise = true;
        *spatial_strength = strength;
        let encoded = serde_json::to_vec(&settings).unwrap();
        let restored: PluginSettings = serde_json::from_slice(&encoded).unwrap();
        let config = restored.to_plugin_config(48_000.0);
        let result = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000);
        if strength > 1.0 {
            assert!(result.is_err());
            continue;
        }
        let mut plugin = result.unwrap();
        plugin.initialize(48_000).unwrap();
        for (id, expected) in [
            ("harmonic_percussive", ParameterValue::Bool(true)),
            ("spatial_denoise", ParameterValue::Bool(true)),
            ("spatial_strength", ParameterValue::Float(strength as f32)),
        ] {
            assert_eq!(plugin.get_parameter(&ParameterId::from(id)), Some(expected));
        }
    }
}
