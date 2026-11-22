//! Exercises surround settings through engine serialization and the plugin factory.

// Rust guideline compliant 2026-10-09

use sotf_audio::plugins::{PluginSettings, PluginType};
use sotf_plugins::ProcessContext;
use sotf_plugins::factory::create_plugin;
use sotf_plugins::plugin_upmixer::SurroundBassMode;
use sotf_plugins::speaker_config::{ChannelLayout, get_speaker_config};

#[test]
fn five_one_six_output_survives_engine_selection_and_render() {
    use sotf_audio::plugins::{PluginChain, plugin_output_channels};
    use sotf_plugins::param_specs::{index_of, upmixer::PARAMS};

    let mut settings = PluginSettings::default_for(&PluginType::Upmixer).unwrap();
    let output = index_of(PARAMS, "speaker_config");
    settings.set_param_value(output, 10.0);
    settings.set_param_value(index_of(PARAMS, "input_format"), 1.0);
    assert_eq!(settings.param_value(output), Some(10.0));
    let saved = serde_json::to_string(&settings).unwrap();
    let restored: PluginSettings = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.param_value(output), Some(10.0));
    let mut legacy: serde_json::Value = serde_json::from_str(&saved).unwrap();
    legacy["Upmixer"]["speaker_config"] = serde_json::json!(10);
    let numeric: PluginSettings = serde_json::from_value(legacy).unwrap();
    assert_eq!(numeric.param_value(output), Some(10.0));
    assert_eq!(plugin_output_channels(&restored, 5), 12);
    let mut chain = PluginChain::new();
    chain.add_plugin(&PluginType::Upmixer).unwrap();
    chain.get_plugin_mut(0).unwrap().settings = restored.clone();
    assert_eq!(chain.output_channels_for_input(5), 12);
    assert_eq!(chain.output_speaker_config(), Some("5.1.6"));
    chain.add_plugin(&PluginType::LoudnessMonitor).unwrap();
    let configs = chain.to_plugin_configs(48_000.0);
    let monitor = configs
        .iter()
        .find(|config| config.plugin_type == "loudness_monitor")
        .unwrap();
    assert_eq!(monitor.parameters["speaker_config"], "5.1.6");
    let config = restored.to_plugin_config(48_000.0);
    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 5, 48_000).unwrap();
    plugin.initialize(48_000.0).unwrap();
    assert_eq!(plugin.output_channels(), 12);
    let frames = plugin.latency_samples() + 1;
    let mut input = vec![0.0; frames * 5];
    input[2] = 0.25;
    let mut output = vec![0.0; frames * 12];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, frames))
        .unwrap();
    assert_eq!(output[plugin.latency_samples() * 12 + 2], 0.25);
}

#[test]
fn engine_preserves_surround_layout_and_bass_configuration() {
    let mut settings = PluginSettings::default_for(&PluginType::Upmixer).unwrap();
    if let PluginSettings::Upmixer {
        speaker_config,
        multichannel,
        ..
    } = &mut settings
    {
        *speaker_config = "7.1.4".into();
        multichannel.input_layout =
            Some(ChannelLayout::from_speaker_config(get_speaker_config("5.0").unwrap()).unwrap());
        multichannel.bass_mode = SurroundBassMode::Derived;
        multichannel.bass_cutoff_hz = 80.0;
    } else {
        panic!("expected Upmixer settings");
    }
    let saved = serde_json::to_value(&settings).unwrap();
    let restored: PluginSettings = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.required_input_channels(), Some(5));
    let config = restored.to_plugin_config(48_000.0);
    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 5, 48_000).unwrap();
    plugin.initialize(48_000.0).unwrap();
    assert_eq!(plugin.input_channels(), 5);
    assert_eq!(plugin.output_channels(), 12);
    let frames = plugin.latency_samples() + 512;
    let mut input = vec![0.0; frames * 5];
    input[2] = 0.5;
    let mut output = vec![0.0; frames * 12];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, frames))
        .unwrap();
    assert_eq!(output[plugin.latency_samples() * 12 + 2], 0.5);
}

#[test]
fn input_and_bass_controls_roundtrip_through_engine_settings() {
    use sotf_plugins::param_specs::{index_of, upmixer::PARAMS};
    let mut settings = PluginSettings::default_for(&PluginType::Upmixer).unwrap();
    settings.set_param_value(index_of(PARAMS, "input_format"), 1.0);
    settings.set_param_value(index_of(PARAMS, "bass_mode"), 2.0);
    settings.set_param_value(index_of(PARAMS, "bypass_surround"), 1.0);
    settings.set_param_value(index_of(PARAMS, "bass_source_front_left"), 0.0);
    settings.set_param_value(index_of(PARAMS, "bass_source_front_right"), 0.0);
    settings.set_param_value(index_of(PARAMS, "bass_source_side_left"), 0.0);
    settings.set_param_value(index_of(PARAMS, "bass_source_side_right"), 0.0);
    assert_eq!(settings.required_input_channels(), Some(5));
    let encoded = serde_json::to_string(&settings).unwrap();
    let restored: PluginSettings = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        restored.param_value(index_of(PARAMS, "input_format")),
        Some(1.0)
    );
    assert_eq!(
        restored.param_value(index_of(PARAMS, "bass_mode")),
        Some(2.0)
    );
    assert_eq!(
        restored.param_value(index_of(PARAMS, "bass_source_center")),
        Some(1.0)
    );
    assert_eq!(
        restored.param_value(index_of(PARAMS, "bass_source_front_left")),
        Some(0.0)
    );
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["bass_mode"], "managed");
    assert_eq!(config.parameters["bypass_surround"], true);
    assert!(create_plugin(&config.plugin_type, &config.parameters, 5, 48_000).is_ok());
}

#[test]
fn switching_stereo_preset_to_surround_keeps_stereo_features_dormant() {
    use sotf_audio::plugins::{PluginChain, plugin_output_channels};
    use sotf_plugins::param_specs::{index_of, upmixer::PARAMS};

    for (format_index, input_channels) in [(1, 5), (2, 6), (3, 8)] {
        let mut settings = PluginSettings::default_for(&PluginType::Upmixer).unwrap();
        let PluginSettings::Upmixer {
            speaker_config,
            output,
            subharmonic,
            ..
        } = &mut settings
        else {
            unreachable!();
        };
        *speaker_config = "7.1.4".into();
        output.binaural_preview = true;
        output.auto_gain_enabled = true;
        subharmonic.enable_subharmonic_synth = true;
        settings.set_param_value(index_of(PARAMS, "input_format"), format_index as f64);

        let saved = serde_json::to_string(&settings).unwrap();
        let mut restored: PluginSettings = serde_json::from_str(&saved).unwrap();
        let config = restored.to_plugin_config(48_000.0);
        for key in [
            "binaural_preview",
            "auto_gain_enabled",
            "enable_subharmonic_synth",
        ] {
            assert_eq!(config.parameters[key], false, "{input_channels}ch: {key}");
        }
        let plugin = create_plugin(
            &config.plugin_type,
            &config.parameters,
            input_channels,
            48_000,
        )
        .expect("stereo-only settings must not prevent surround construction");
        assert_eq!(plugin.output_channels(), 12);
        assert_eq!(plugin_output_channels(&restored, input_channels), 12);

        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        chain.get_plugin_mut(0).unwrap().settings = restored.clone();
        assert_eq!(chain.output_channels_for_input(input_channels), 12);
        assert_eq!(chain.output_speaker_config(), Some("7.1.4"));
        assert_eq!(chain.speaker_config_at_index(1).as_deref(), Some("7.1.4"));
        chain.add_plugin(&PluginType::LoudnessMonitor).unwrap();
        let configs = chain.to_plugin_configs(48_000.0);
        let monitor = configs
            .iter()
            .find(|config| config.plugin_type == "loudness_monitor")
            .unwrap();
        assert_eq!(monitor.parameters["speaker_config"], "7.1.4");
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(input_channels);
        let PluginSettings::Matrix {
            input_channels: matrix_input,
            output_channels: matrix_output,
            ..
        } = &chain.get_plugin(2).unwrap().settings
        else {
            unreachable!();
        };
        assert_eq!((*matrix_input, *matrix_output), (12, 12));

        restored.set_param_value(index_of(PARAMS, "input_format"), 0.0);
        let stereo = restored.to_plugin_config(48_000.0);
        for key in [
            "binaural_preview",
            "auto_gain_enabled",
            "enable_subharmonic_synth",
        ] {
            assert_eq!(stereo.parameters[key], true, "restored stereo: {key}");
        }
        assert_eq!(plugin_output_channels(&restored, 2), 2);
        let stereo_plugin = create_plugin(&stereo.plugin_type, &stereo.parameters, 2, 48_000)
            .expect("stereo preferences must work again after switching back");
        assert_eq!(stereo_plugin.output_channels(), 2);
    }
}
