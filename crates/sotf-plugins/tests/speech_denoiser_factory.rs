use sotf_plugins::{catalog_entry, create_plugin};

#[test]
fn speech_denoiser_factory_catalog_layout_and_state_are_consistent() {
    let entry = catalog_entry("speech_denoiser").expect("Speech Denoiser catalog entry");
    let supported = entry.metadata.channel_layout.supported_inputs;
    assert_eq!(supported.supports(1), Some(true));
    assert_eq!(supported.supports(2), Some(true));
    assert_eq!(supported.supports(3), Some(false));

    for channels in [1, 2] {
        assert!(create_plugin("speech_denoiser", &serde_json::json!({}), channels, 48_000).is_ok());
    }
    for channels in [0, 3, 6, 12] {
        assert!(
            create_plugin("speech_denoiser", &serde_json::json!({}), channels, 48_000).is_err()
        );
    }
    assert!(
        create_plugin(
            "speech_denoiser",
            &serde_json::json!({"enabled": true, "unknown": 1}),
            1,
            48_000,
        )
        .is_err()
    );
}

#[test]
fn speech_denoiser_factory_honors_non_default_strength_and_model() {
    use sotf_plugins::{ParameterId, ParameterValue};

    for channels in [1, 2] {
        let plugin = create_plugin(
            "speech_denoiser",
            &serde_json::json!({"enabled": true, "strength": 0.25, "model": "RNNoise Full"}),
            channels,
            48_000,
        )
        .unwrap_or_else(|error| panic!("{channels}ch non-default strength: {error}"));
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("strength")),
            Some(ParameterValue::Float(0.25)),
            "{channels}ch"
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("model")),
            Some(ParameterValue::Int(0)),
            "{channels}ch"
        );

        // The model also accepts its integer index.
        let indexed = create_plugin(
            "speech_denoiser",
            &serde_json::json!({"enabled": true, "strength": 0.75, "model": 0}),
            channels,
            48_000,
        )
        .unwrap_or_else(|error| panic!("{channels}ch indexed model: {error}"));
        assert_eq!(
            indexed.get_parameter(&ParameterId::from("strength")),
            Some(ParameterValue::Float(0.75)),
            "{channels}ch"
        );

        // Unknown models and out-of-range strengths are rejected.
        assert!(
            create_plugin(
                "speech_denoiser",
                &serde_json::json!({"model": "Nope"}),
                channels,
                48_000,
            )
            .is_err(),
            "{channels}ch unknown model"
        );
        assert!(
            create_plugin(
                "speech_denoiser",
                &serde_json::json!({"model": 7}),
                channels,
                48_000,
            )
            .is_err(),
            "{channels}ch out-of-range model index"
        );
        assert!(
            create_plugin(
                "speech_denoiser",
                &serde_json::json!({"strength": 1.5}),
                channels,
                48_000,
            )
            .is_err(),
            "{channels}ch out-of-range strength"
        );
    }
}
