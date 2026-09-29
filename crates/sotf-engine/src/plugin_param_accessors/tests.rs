use crate::plugins::{PluginSettings, PluginType};
use sotf_plugins::param_specs;

/// Validate that every plugin's LAYOUT indices are within bounds of its PARAMS.
///
/// Iterates all plugin types dynamically so new plugins are automatically covered.
/// This prevents the class of bug where a layout built for one param set
/// (e.g., multiband GLOBAL_PARAMS with crossover entries at 0-5) is accidentally
/// paired with a different param set (e.g., single-band PARAMS starting at threshold=0).
#[test]
fn validate_all_plugin_layout_indices() {
    let mut all_errors = Vec::new();
    for pt in PluginType::all() {
        let name = pt.name();
        let settings = PluginSettings::default_for(&pt).unwrap();
        let params = settings.param_specs();
        if let Some(layout) = settings.layout() {
            let errors = layout.validate(params.len(), name);
            all_errors.extend(errors);
        }
    }

    if !all_errors.is_empty() {
        panic!(
            "LAYOUT/PARAMS index mismatches found:\n  {}",
            all_errors.join("\n  ")
        );
    }
}

/// Validate that every PARAMS index appears somewhere in the LAYOUT.
/// Catches the bug where new params are added to PARAMS but not to LAYOUT,
/// making them invisible in the UI view (while still showing in table view).
///
/// Iterates all plugin types dynamically so new plugins are automatically covered.
#[test]
fn validate_all_params_have_layout_coverage() {
    let mut all_errors = Vec::new();
    for pt in PluginType::all() {
        let name = pt.name();
        let settings = PluginSettings::default_for(&pt).unwrap();
        let params = settings.param_specs();
        if let Some(layout) = settings.layout() {
            let errors = layout.validate_coverage(params, name);
            all_errors.extend(errors);
        }
    }

    if !all_errors.is_empty() {
        panic!(
            "PARAMS entries missing from LAYOUT ({} total):\n  {}",
            all_errors.len(),
            all_errors.join("\n  ")
        );
    }
}

/// Validate that every non-structural PARAMS engine_key that `engine_param_at()`
/// can emit actually exists in the DSP plugin's parameter list.
///
/// This catches the bug where a parameter is declared in PARAMS (so the UI
/// shows it and `engine_param_at()` can send it) but the DSP plugin never
/// registered it in `parameters()` / `set_parameter()`, causing silent drops.
#[test]
fn validate_engine_keys_exist_in_dsp_plugin() {
    let mut all_errors = Vec::new();
    for pt in PluginType::all() {
        let name = pt.name();
        let settings = PluginSettings::default_for(&pt).unwrap();
        let specs = settings.param_specs();

        if specs.is_empty() {
            continue;
        }

        // Create the DSP plugin via the factory
        let config = settings.to_plugin_config(44100.0);
        let input_channels = settings.required_input_channels().unwrap_or(2);
        let plugin = match sotf_plugins::create_plugin(
            &config.plugin_type,
            &config.parameters,
            input_channels,
            44100,
        ) {
            Ok(p) => p,
            Err(e) => {
                all_errors.push(format!("{}: failed to create DSP plugin: {}", name, e));
                continue;
            }
        };

        let dsp_params = plugin.parameters();
        let dsp_keys: std::collections::HashSet<&str> =
            dsp_params.iter().map(|p| p.id.as_str()).collect();

        for (i, spec) in specs.iter().enumerate() {
            let Some((engine_key, _)) = settings.engine_param_at(i) else {
                continue;
            };
            if spec.update_mode == param_specs::UpdateMode::Structural {
                continue;
            }
            if !dsp_keys.contains(engine_key.as_str()) {
                all_errors.push(format!(
                    "{} param {} ({}): engine_key '{}' not found in DSP plugin parameters",
                    name, i, spec.name, engine_key
                ));
            }
        }
    }

    if !all_errors.is_empty() {
        panic!(
            "Engine keys missing from DSP plugin ({} total):\n  {}",
            all_errors.len(),
            all_errors.join("\n  ")
        );
    }
}

#[test]
fn spectrum_tilt_params_do_not_emit_engine_updates() {
    let settings = PluginSettings::default_for(&PluginType::SpectrumAnalyzer).unwrap();

    let tilt_correction_idx =
        param_specs::index_of(param_specs::spectrum::PARAMS, "tilt_correction");
    let tilt_reference_idx = param_specs::index_of(param_specs::spectrum::PARAMS, "tilt_reference");

    assert_eq!(settings.engine_param_at(tilt_correction_idx), None);
    assert_eq!(settings.engine_param_at(tilt_reference_idx), None);
}

#[test]
fn crossover_is_exposed_with_editable_layout_and_dsp_config() {
    assert!(
        PluginType::all().contains(&PluginType::Crossover),
        "Crossover must stay in the app-facing plugin inventory"
    );

    let settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    let layout = settings
        .layout()
        .expect("Crossover must have an editable declarative layout");
    assert!(
        layout
            .validate(settings.param_specs().len(), "Crossover")
            .is_empty()
    );
    assert!(
        layout
            .validate_coverage(settings.param_specs(), "Crossover")
            .is_empty(),
        "Crossover layout must expose every declared parameter"
    );

    let config = settings.to_plugin_config(48_000.0);
    assert_eq!(config.plugin_type, "crossover");
    sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("Crossover settings must create the DSP plugin");
}

#[test]
fn eq_global_parameters_round_trip_through_engine_accessors() {
    let mut settings = PluginSettings::default_for(&PluginType::EQ).unwrap();
    let params = settings.param_specs();

    assert_eq!(params.len(), param_specs::eq::GLOBAL_PARAMS.len());
    let auto_gain = param_specs::index_of(params, "auto_gain_enabled");
    let oversampling = param_specs::index_of(params, "oversampling");

    assert_eq!(settings.param_value(auto_gain), Some(0.0));
    assert_eq!(settings.param_value(oversampling), Some(1.0));

    settings.set_param_value(auto_gain, 1.0);
    settings.set_param_value(oversampling, 4.0);

    assert_eq!(settings.param_value(auto_gain), Some(1.0));
    assert_eq!(settings.param_value(oversampling), Some(4.0));
    assert_eq!(
        settings.engine_param_at(auto_gain),
        Some(("auto_gain_enabled".to_string(), "true".to_string()))
    );
    assert_eq!(
        settings.engine_param_at(oversampling),
        None,
        "oversampling remains structural and is not sent as a runtime update"
    );
}

#[test]
fn de_esser_range_and_link_survive_engine_presets_and_factory_conversion() {
    use sotf_plugins::{ParameterId, ParameterValue};

    // Presets saved before these controls existed retain independent channels
    // and the original effectively unrestricted reduction range.
    let mut settings: PluginSettings =
        serde_json::from_value(serde_json::json!({ "DeEsser": {} })).unwrap();
    assert_eq!(settings.param_value(8), Some(60.0));
    assert_eq!(settings.param_value(9), Some(0.0));
    assert_eq!(settings.param_specs()[8].engine_key, "range_db");
    assert_eq!(settings.param_specs()[9].engine_key, "stereo_link");

    settings.set_param_value(8, 6.0);
    settings.set_param_value(9, 0.75);
    let encoded = serde_json::to_value(&settings).unwrap();
    let restored: PluginSettings = serde_json::from_value(encoded).unwrap();
    for (index, key, expected) in [(8, "range_db", 6.0), (9, "stereo_link", 0.75)] {
        assert_eq!(restored.param_value(index), Some(expected));
        let (engine_key, engine_value) = restored.engine_param_at(index).unwrap();
        assert_eq!(engine_key, key);
        assert_eq!(engine_value.parse::<f64>().unwrap(), expected);
    }

    let config = restored.to_plugin_config(48_000.0);
    let plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("range_db")),
        Some(ParameterValue::Float(6.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("stereo_link")),
        Some(ParameterValue::Float(0.75))
    );
}

#[test]
fn compressor_range_and_hold_survive_presets_accessors_and_factory_conversion() {
    use sotf_plugins::{ParameterId, ParameterValue};

    for (kind, range_index, hold_index) in [
        (PluginType::Compressor, 16, 17),
        (PluginType::MultibandCompressor, 17, 18),
        (PluginType::AnalogCompressor, 13, 14),
    ] {
        let defaults = PluginSettings::default_for(&kind).unwrap();
        let mut old_preset = serde_json::to_value(&defaults).unwrap();
        let fields = old_preset
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
            .as_object_mut()
            .unwrap();
        fields.remove("range_db");
        fields.remove("hold_ms");
        let mut settings: PluginSettings = serde_json::from_value(old_preset).unwrap();
        assert_eq!(settings.param_value(range_index), Some(120.0), "{kind:?}");
        assert_eq!(settings.param_value(hold_index), Some(0.0), "{kind:?}");
        for (index, key, value) in [
            (range_index, "range_db", 6.0),
            (hold_index, "hold_ms", 25.0),
        ] {
            assert_eq!(settings.param_specs()[index].engine_key, key);
            settings.set_param_value(index, value);
            let (engine_key, engine_value) = settings.engine_param_at(index).unwrap();
            assert_eq!(engine_key, key);
            assert_eq!(engine_value.parse::<f64>().unwrap(), value);
        }
        let restored: PluginSettings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        let config = restored.to_plugin_config(48_000.0);
        let plugin =
            sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
                .unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("range_db")),
            Some(ParameterValue::Float(6.0)),
            "{kind:?}"
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("hold_ms")),
            Some(ParameterValue::Float(25.0)),
            "{kind:?}"
        );
    }
}

#[test]
fn multiband_compressor_preserves_band_overrides_and_link_controls() {
    use sotf_plugins::{ParameterId, ParameterValue};
    let defaults = PluginSettings::default_for(&PluginType::MultibandCompressor).unwrap();
    let mut saved = serde_json::to_value(&defaults).unwrap();
    let fields = &mut saved["MultibandCompressor"];
    fields["range_db"] = serde_json::json!(12.0);
    fields["hold_ms"] = serde_json::json!(10.0);
    fields["sidechain_tilt_db"] = serde_json::json!(3.0);
    fields["link_amount"] = serde_json::json!(0.25);
    let mut band = serde_json::to_value(sotf_plugins::BandCompressorParams::default()).unwrap();
    band["range_db"] = serde_json::json!(3.0);
    band["hold_ms"] = serde_json::json!(50.0);
    fields["bands"] = serde_json::json!([band]);
    let settings: PluginSettings = serde_json::from_value(saved).unwrap();
    let config = settings.to_plugin_config(48_000.0);
    let plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    for (key, value) in [
        ("range_db", 12.0),
        ("hold_ms", 10.0),
        ("band_0_range_db", 3.0),
        ("band_0_hold_ms", 50.0),
        ("band_1_range_db", 12.0),
        ("band_1_hold_ms", 10.0),
        ("sidechain_tilt_db", 3.0),
        ("link_amount", 0.25),
    ] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(key)),
            Some(ParameterValue::Float(value)),
            "{key}"
        );
    }
}

#[test]
fn downmix_matrix_ltrt_is_exposed_and_round_trips() {
    let mut settings = PluginSettings::default_for(&PluginType::Downmix).unwrap();
    let matrix_ltrt_idx = param_specs::index_of(param_specs::downmix::PARAMS, "matrix_ltrt");

    assert_eq!(matrix_ltrt_idx, 8);
    assert_eq!(settings.param_specs().len(), 9);
    assert_eq!(settings.param_value(matrix_ltrt_idx), Some(0.0));

    settings.set_param_value(matrix_ltrt_idx, 1.0);
    assert_eq!(settings.param_value(matrix_ltrt_idx), Some(1.0));

    let config = settings.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["matrix_ltrt"], serde_json::json!(true));
}

#[test]
fn gate_modes_preserve_legacy_presets_indices_and_factory_state() {
    use sotf_plugins::{GateMode, ParameterId, ParameterValue};
    let defaults = PluginSettings::default_for(&PluginType::Gate).unwrap();
    let mut legacy = serde_json::to_value(&defaults).unwrap();
    legacy["Gate"].as_object_mut().unwrap().remove("mode");
    legacy["Gate"]
        .as_object_mut()
        .unwrap()
        .remove("max_boost_db");
    let mut settings: PluginSettings = serde_json::from_value(legacy).unwrap();
    assert_eq!(settings.param_value(15), Some(0.0));
    assert_eq!(settings.param_value(16), Some(12.0));
    assert_eq!(settings.param_specs()[14].engine_key, "lookahead_ms");
    assert_eq!(settings.param_specs()[15].engine_key, "mode");
    assert_eq!(settings.param_specs()[16].engine_key, "max_boost_db");
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        settings.set_param_value(15, mode.index() as f64);
        settings.set_param_value(16, 6.0);
        assert_eq!(settings.param_value(15), Some(mode.index() as f64));
        assert_eq!(
            settings.engine_param_at(15),
            None,
            "mode requires reconstruction"
        );
        let (key, value) = settings.engine_param_at(16).unwrap();
        assert_eq!(key, "max_boost_db");
        assert_eq!(value.parse::<f64>().unwrap(), 6.0);
        let saved = serde_json::to_value(&settings).unwrap();
        assert_eq!(saved["Gate"]["mode"], serde_json::to_value(mode).unwrap());
        let restored: PluginSettings = serde_json::from_value(saved).unwrap();
        let config = restored.to_plugin_config(48_000.0);
        let plugin =
            sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
                .unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("mode")),
            Some(ParameterValue::Int(mode.index() as i32))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("max_boost_db")),
            Some(ParameterValue::Float(6.0))
        );
    }
}
