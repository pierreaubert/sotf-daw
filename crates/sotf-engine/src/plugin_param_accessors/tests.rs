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

#[test]
fn band_split_explicit_cutoffs_drive_readback_and_migrate_before_edits() {
    let mut settings = PluginSettings::default_for(&PluginType::BandSplit).unwrap();
    let PluginSettings::BandSplit { frequencies, .. } = &mut settings else {
        unreachable!();
    };
    *frequencies = Some(vec![100.0, 5_000.0]);

    assert_eq!(settings.param_value(0), Some(100.0));
    assert_eq!(settings.param_value(3), Some(1.0));
    assert_eq!(settings.param_value(4), Some(5_000.0));
    assert_eq!(settings.param_value(5), Some(4_800.0));

    settings.set_param_value(4, 6_100.0);
    assert_eq!(settings.param_value(0), Some(100.0));
    assert_eq!(settings.param_value(4), Some(6_100.0));
    assert_eq!(settings.param_value(3), Some(1.0));
    let PluginSettings::BandSplit {
        frequencies,
        frequency,
        frequency_2,
        num_bands,
        ..
    } = &settings
    else {
        unreachable!();
    };
    assert!(frequencies.is_none());
    assert_eq!(*frequency, 100.0);
    assert_eq!(*frequency_2, 6_100.0);
    assert_eq!(*num_bands, 3);
}

#[test]
fn band_split_count_edit_keeps_surviving_explicit_cutoffs_and_empty_vector_stays_invalid() {
    let mut settings = PluginSettings::default_for(&PluginType::BandSplit).unwrap();
    let PluginSettings::BandSplit { frequencies, .. } = &mut settings else {
        unreachable!();
    };
    *frequencies = Some(vec![100.0, 5_000.0, 15_000.0]);

    settings.set_param_value(3, 0.0);
    assert_eq!(settings.param_value(0), Some(100.0));
    assert_eq!(settings.param_value(3), Some(0.0));
    assert_eq!(settings.param_value(4), Some(5_000.0));
    assert_eq!(settings.param_value(5), Some(15_000.0));
    let PluginSettings::BandSplit {
        frequencies,
        frequency,
        frequency_2,
        frequency_3,
        num_bands,
        ..
    } = &settings
    else {
        unreachable!();
    };
    assert!(frequencies.is_none());
    assert_eq!(
        (*frequency, *frequency_2, *frequency_3),
        (100.0, 5_000.0, 15_000.0)
    );
    assert_eq!(*num_bands, 2);

    let mut invalid = PluginSettings::default_for(&PluginType::BandSplit).unwrap();
    let PluginSettings::BandSplit {
        frequencies,
        frequency,
        frequency_2,
        ..
    } = &mut invalid
    else {
        unreachable!();
    };
    *frequencies = Some(Vec::new());
    *frequency = 700.0;
    *frequency_2 = 2_800.0;
    assert_eq!(invalid.param_value(0), None);
    assert_eq!(invalid.param_value(4), None);
}

#[test]
fn band_split_empty_explicit_vector_is_forwarded_and_rejected_by_plugin_factory() {
    let mut settings = PluginSettings::default_for(&PluginType::BandSplit).unwrap();
    let PluginSettings::BandSplit { frequencies, .. } = &mut settings else {
        unreachable!();
    };
    *frequencies = Some(Vec::new());

    let config = settings.to_plugin_config(48_000.0);
    assert_eq!(
        config.parameters["explicit_frequencies"],
        serde_json::json!([]),
        "the typed empty vector must not be replaced by scalar defaults"
    );
    assert!(
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).is_err(),
        "the plugin factory must reject an explicit empty cutoff vector"
    );
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
fn crossover_legacy_and_typed_routes_survive_engine_conversion() {
    use sotf_plugins::plugin_crossover::CrossoverTopology;

    // A saved pre-topology Crossover preset omits the optional arrays. The
    // converter must keep them omitted because the DSP's legacy Vec fields
    // accept absence (or an empty list), but not JSON null.
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "Crossover": { "frequency": 750.0 }
    }))
    .unwrap();
    let legacy_config = legacy.to_plugin_config(48_000.0);
    assert!(
        legacy_config
            .parameters
            .get("channel_frequencies_hz")
            .is_none()
    );
    assert!(legacy_config.parameters.get("channel_modes").is_none());
    sotf_plugins::create_plugin(
        &legacy_config.plugin_type,
        &legacy_config.parameters,
        2,
        48_000,
    )
    .expect("legacy frequency-only Crossover settings must still reach the factory");

    let mut settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    let PluginSettings::Crossover {
        crossover_type,
        output,
        topology,
        band_count,
        extra_frequencies,
        channel_frequencies_hz,
        channel_modes,
        ..
    } = &mut settings
    else {
        panic!("expected Crossover settings");
    };
    *crossover_type = "LR48".to_string();
    *output = "both".to_string();
    *topology = Some(CrossoverTopology::Bands);
    *band_count = Some(4);
    *extra_frequencies = vec![1_200.0, 5_000.0];
    *channel_frequencies_hz = Some(vec![500.0, 900.0]);
    *channel_modes = Some(vec!["lowpass".to_string(), "highpass".to_string()]);

    let encoded = serde_json::to_value(&settings).unwrap();
    let mut restored: PluginSettings = serde_json::from_value(encoded).unwrap();
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["topology"], "bands");
    assert_eq!(
        config.parameters["extra_frequencies"],
        serde_json::json!([1_200.0, 5_000.0])
    );
    assert_eq!(
        config.parameters["channel_frequencies_hz"],
        serde_json::json!([500.0, 900.0]),
        "inactive per-channel cutoffs remain in the full typed state"
    );
    assert_eq!(
        config.parameters["channel_modes"],
        serde_json::json!(["lowpass", "highpass"])
    );
    let plugin = sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("explicit bands topology may retain dormant per-channel data");
    assert_eq!(plugin.output_channels(), 8);

    let PluginSettings::Crossover {
        topology, output, ..
    } = &mut restored
    else {
        unreachable!();
    };
    *topology = Some(CrossoverTopology::PerChannel);
    *output = "both".to_string();
    let per_channel_config = restored.to_plugin_config(48_000.0);
    let plugin = sotf_plugins::create_plugin(
        &per_channel_config.plugin_type,
        &per_channel_config.parameters,
        2,
        48_000,
    )
    .expect("complete explicit per-channel modes make global Both dormant");
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn crossover_family_accessors_round_trip_all_choices_and_aliases() {
    let mut settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    let type_index = param_specs::index_of(settings.param_specs(), "type");
    let choices = param_specs::crossover::CROSSOVER_TYPES;

    assert_eq!(choices.len(), 13);
    for (index, choice) in choices.iter().enumerate() {
        settings.set_param_value(type_index, index as f64);
        assert_eq!(settings.param_value(type_index), Some(index as f64));
        assert_eq!(
            settings.to_plugin_config(48_000.0).parameters["type"],
            *choice
        );
        let config = settings.to_plugin_config(48_000.0);
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
            .unwrap_or_else(|error| panic!("family {choice} must instantiate: {error}"));
    }

    if let PluginSettings::Crossover { crossover_type, .. } = &mut settings {
        *crossover_type = "bessel12".to_string();
    }
    assert_eq!(settings.param_value(type_index), Some(12.0));
    let config = settings.to_plugin_config(48_000.0);
    sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("lowercase Bessel12 alias must remain accepted");
}

#[test]
fn crossover_topology_count_and_cutoff_accessors_preserve_old_indices_and_dormant_values() {
    use sotf_plugins::plugin_crossover::CrossoverTopology;

    let mut settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    let specs = settings.param_specs();
    assert_eq!(specs[0].engine_key, "type");
    assert_eq!(specs[1].engine_key, "frequency");
    assert_eq!(specs[2].engine_key, "mode");
    assert_eq!(specs[3].engine_key, "fir_taps");
    assert_eq!(
        param_specs::find_by_key(specs, "frequency").engine_key,
        "frequency"
    );
    assert_eq!(param_specs::find_by_key(specs, "mode").engine_key, "mode");
    assert_eq!(
        param_specs::find_by_key(specs, "fir_taps").engine_key,
        "fir_taps"
    );

    assert_eq!(settings.param_value(0), Some(0.0));
    assert_eq!(settings.param_value(1), Some(1_000.0));
    assert_eq!(settings.param_value(2), Some(0.0));
    assert_eq!(settings.param_value(3), Some(1_025.0));
    assert_eq!(settings.param_value(4), Some(0.0));
    assert_eq!(settings.param_value(5), Some(0.0));

    settings.set_param_value(4, 1.0);
    settings.set_param_value(5, 2.0);
    settings.set_param_value(6, 2_500.0);
    settings.set_param_value(7, 8_000.0);
    if let PluginSettings::Crossover {
        channel_frequencies_hz,
        channel_modes,
        ..
    } = &mut settings
    {
        *channel_frequencies_hz = Some(vec![500.0, 1_000.0]);
        *channel_modes = Some(vec!["lowpass".into(), "highpass".into()]);
    }
    assert_eq!(settings.param_value(4), Some(1.0));
    assert_eq!(settings.param_value(5), Some(2.0));
    assert_eq!(settings.param_value(6), Some(2_500.0));
    assert_eq!(settings.param_value(7), Some(8_000.0));
    assert_eq!(settings.engine_param_at(4), None);
    assert_eq!(settings.engine_param_at(5), None);

    let serialized = serde_json::to_value(&settings).unwrap();
    let restored: PluginSettings = serde_json::from_value(serialized).unwrap();
    let PluginSettings::Crossover {
        topology,
        band_count,
        extra_frequencies,
        ..
    } = &restored
    else {
        panic!("expected Crossover settings");
    };
    assert_eq!(*topology, Some(CrossoverTopology::PerChannel));
    assert_eq!(*band_count, Some(4));
    assert_eq!(extra_frequencies.as_slice(), &[2_500.0, 8_000.0]);

    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["band_count"], 4);
    assert_eq!(
        config.parameters["extra_frequencies"],
        serde_json::json!([2_500.0, 8_000.0])
    );
    let plugin = sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("dormant band values are ignored by the explicit per-channel route");
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn crossover_active_band_count_controls_factory_width_without_dropping_dormant_cutoffs() {
    use sotf_plugins::plugin_crossover::CrossoverTopology;

    let mut settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    if let PluginSettings::Crossover {
        output,
        topology,
        band_count,
        extra_frequencies,
        ..
    } = &mut settings
    {
        *output = "both".into();
        *topology = Some(CrossoverTopology::Bands);
        *band_count = Some(2);
        *extra_frequencies = vec![2_500.0, 8_000.0];
    }

    for (count, expected_width) in [(2, 4), (3, 6), (4, 8)] {
        settings.set_param_value(5, (count - 2) as f64);
        let config = settings.to_plugin_config(48_000.0);
        assert_eq!(
            config.parameters["extra_frequencies"],
            serde_json::json!([2_500.0, 8_000.0])
        );
        let plugin =
            sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
                .unwrap_or_else(|error| panic!("{count}-band state should instantiate: {error}"));
        assert_eq!(plugin.output_channels(), expected_width);
    }

    if let PluginSettings::Crossover {
        band_count,
        extra_frequencies,
        ..
    } = &mut settings
    {
        *band_count = Some(4);
        extra_frequencies.pop();
    }
    let config = settings.to_plugin_config(48_000.0);
    assert!(
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).is_err(),
        "a persisted 4-band count with one cutoff must fail admission instead of silently routing 3 bands"
    );
}

#[test]
fn crossover_count_expansion_materializes_defaults_and_refuses_an_exhausted_range() {
    let mut settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    settings.set_param_value(2, 2.0);
    settings.set_param_value(5, 1.0);
    let PluginSettings::Crossover {
        band_count,
        extra_frequencies,
        ..
    } = &settings
    else {
        unreachable!();
    };
    assert_eq!(*band_count, Some(3));
    assert_eq!(extra_frequencies, &[3_000.0]);

    settings.set_param_value(5, 2.0);
    let retained = {
        let PluginSettings::Crossover {
            band_count,
            extra_frequencies,
            ..
        } = &settings
        else {
            unreachable!();
        };
        assert_eq!(*band_count, Some(4));
        assert_eq!(extra_frequencies, &[3_000.0, 8_000.0]);
        extra_frequencies.clone()
    };
    settings.set_param_value(5, 0.0);
    settings.set_param_value(5, 2.0);
    let PluginSettings::Crossover {
        band_count,
        extra_frequencies,
        ..
    } = &settings
    else {
        unreachable!();
    };
    assert_eq!(*band_count, Some(4));
    assert_eq!(*extra_frequencies, retained);

    let config = settings.to_plugin_config(48_000.0);
    let plugin = sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("materialized defaults must pass the actual factory path");
    assert_eq!(plugin.output_channels(), 2 * 4);
    let PluginSettings::Crossover {
        topology, output, ..
    } = &settings
    else {
        unreachable!();
    };
    assert_eq!(
        *topology,
        Some(sotf_plugins::plugin_crossover::CrossoverTopology::Bands)
    );
    assert_eq!(output, "both");
    let mut high_primary = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    if let PluginSettings::Crossover {
        frequency,
        band_count,
        extra_frequencies,
        ..
    } = &mut high_primary
    {
        *frequency = 20_000.0;
        *band_count = Some(2);
        extra_frequencies.clear();
    }
    let before = serde_json::to_value(&high_primary).unwrap();
    high_primary.set_param_value(5, 1.0);
    assert_eq!(serde_json::to_value(&high_primary).unwrap(), before);

    let mut f32_collision = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    if let PluginSettings::Crossover {
        frequency,
        band_count,
        extra_frequencies,
        ..
    } = &mut f32_collision
    {
        *frequency = 19_999.999_9;
        *band_count = Some(2);
        extra_frequencies.clear();
    }
    let before = serde_json::to_value(&f32_collision).unwrap();
    f32_collision.set_param_value(5, 2.0);
    assert_eq!(
        serde_json::to_value(&f32_collision).unwrap(),
        before,
        "a count whose active cutoffs collapse at the factory's f32 boundary must be refused transactionally"
    );

    let mut near_upper_limit = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    if let PluginSettings::Crossover {
        frequency,
        band_count,
        extra_frequencies,
        ..
    } = &mut near_upper_limit
    {
        *frequency = 19_000.0;
        *band_count = Some(2);
        extra_frequencies.clear();
    }
    near_upper_limit.set_param_value(5, 2.0);
    let PluginSettings::Crossover {
        band_count,
        extra_frequencies,
        ..
    } = &near_upper_limit
    else {
        unreachable!();
    };
    assert_eq!(*band_count, Some(4));
    assert_eq!(extra_frequencies.len(), 2);
    assert!((extra_frequencies[0] - 19_333.333333333332).abs() < 1e-9);
    assert!((extra_frequencies[1] - 19_666.666666666668).abs() < 1e-9);
    let config = near_upper_limit.to_plugin_config(48_000.0);
    sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("interpolated cutoffs near the upper limit remain factory-valid");
}

#[test]
fn crossover_dormant_count_edit_keeps_legacy_per_channel_fallback_until_bands_reactivation() {
    let mut settings = PluginSettings::default_for(&PluginType::Crossover).unwrap();
    settings.set_param_value(2, 1.0);
    if let PluginSettings::Crossover {
        topology,
        band_count,
        extra_frequencies,
        channel_frequencies_hz,
        channel_modes,
        ..
    } = &mut settings
    {
        *topology = None;
        *band_count = Some(2);
        extra_frequencies.clear();
        *channel_frequencies_hz = Some(vec![500.0, 1_000.0]);
        *channel_modes = Some(vec!["lowpass".into()]);
    }

    let legacy_config = settings.to_plugin_config(48_000.0);
    let legacy_plugin = sotf_plugins::create_plugin(
        &legacy_config.plugin_type,
        &legacy_config.parameters,
        2,
        48_000,
    )
    .expect("legacy PerChannel scalar fallback must remain valid");
    assert_eq!(legacy_plugin.output_channels(), 2);

    settings.set_param_value(5, 1.0);
    let PluginSettings::Crossover {
        topology,
        band_count,
        extra_frequencies,
        channel_modes,
        ..
    } = &settings
    else {
        unreachable!();
    };
    assert_eq!(*topology, None);
    assert_eq!(*band_count, Some(3));
    assert!(extra_frequencies.is_empty());
    assert_eq!(
        channel_modes.as_deref(),
        Some(["lowpass".to_string()].as_slice())
    );
    let dormant_config = settings.to_plugin_config(48_000.0);
    let dormant_plugin = sotf_plugins::create_plugin(
        &dormant_config.plugin_type,
        &dormant_config.parameters,
        2,
        48_000,
    )
    .expect("editing dormant count must preserve legacy PerChannel fallback");
    assert_eq!(dormant_plugin.output_channels(), 2);

    settings.set_param_value(4, 0.0);
    let PluginSettings::Crossover {
        topology,
        extra_frequencies,
        ..
    } = &settings
    else {
        unreachable!();
    };
    assert_eq!(
        *topology,
        Some(sotf_plugins::plugin_crossover::CrossoverTopology::Bands)
    );
    assert_eq!(extra_frequencies, &[3_000.0]);
    let bands_config = settings.to_plugin_config(48_000.0);
    sotf_plugins::create_plugin(
        &bands_config.plugin_type,
        &bands_config.parameters,
        2,
        48_000,
    )
    .expect("switching to Bands must materialize its required active cutoff");
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
fn compressor_sidechain_detector_survives_presets_accessors_and_factory_conversion() {
    use sotf_plugins::{ParameterId, ParameterValue};

    // (kind, hz_index, order_index, detection_index, enabled_index, keys added since old presets)
    for (kind, hz_index, order_index, detection_index, enabled_index, new_keys) in [
        (
            PluginType::Compressor,
            9,
            10,
            11,
            18,
            &["sidechain_hpf_enabled"] as &[&str],
        ),
        (
            PluginType::MultibandCompressor,
            19,
            20,
            21,
            22,
            &[
                "sidechain_hpf_hz",
                "sidechain_hpf_order",
                "detection_mode",
                "sidechain_hpf_enabled",
            ] as &[&str],
        ),
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
        for key in new_keys {
            fields.remove(*key);
        }
        let mut settings: PluginSettings = serde_json::from_value(old_preset).unwrap();
        // Spec defaults backfill keys missing from older presets.
        assert_eq!(settings.param_value(hz_index), Some(80.0), "{kind:?}");
        assert_eq!(settings.param_value(order_index), Some(0.0), "{kind:?}");
        assert_eq!(settings.param_value(detection_index), Some(0.0), "{kind:?}");
        assert_eq!(settings.param_value(enabled_index), Some(0.0), "{kind:?}");
        // Accessor order matches PARAMS order.
        for (index, key) in [
            (hz_index, "sidechain_hpf_hz"),
            (order_index, "sidechain_hpf_order"),
            (detection_index, "detection_mode"),
            (enabled_index, "sidechain_hpf_enabled"),
        ] {
            assert_eq!(settings.param_specs()[index].engine_key, key, "{kind:?}");
        }
        settings.set_param_value(hz_index, 120.0);
        settings.set_param_value(order_index, 1.0);
        settings.set_param_value(detection_index, 1.0);
        settings.set_param_value(enabled_index, 1.0);
        assert_eq!(settings.param_value(hz_index), Some(120.0), "{kind:?}");
        assert_eq!(settings.param_value(order_index), Some(1.0), "{kind:?}");
        assert_eq!(
            settings.param_value_string(order_index),
            Some("4th".to_string()),
            "{kind:?}"
        );
        assert_eq!(settings.param_value(detection_index), Some(1.0), "{kind:?}");
        assert_eq!(
            settings.param_value_string(detection_index),
            Some("RMS".to_string()),
            "{kind:?}"
        );
        assert_eq!(settings.param_value(enabled_index), Some(1.0), "{kind:?}");
        // Structural detector params rebuild instead of live-updating.
        for index in [hz_index, order_index, detection_index, enabled_index] {
            assert_eq!(settings.engine_param_at(index), None, "{kind:?}");
        }
        let restored: PluginSettings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        let config = restored.to_plugin_config(48_000.0);
        let plugin =
            sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
                .unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("sidechain_hpf_hz")),
            Some(ParameterValue::Float(120.0)),
            "{kind:?}"
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("sidechain_hpf_order")),
            Some(ParameterValue::Int(1)),
            "{kind:?}"
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("detection_mode")),
            Some(ParameterValue::Int(1)),
            "{kind:?}"
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("sidechain_hpf_enabled")),
            Some(ParameterValue::Bool(true)),
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

#[test]
fn de_esser_lookahead_topology_and_modes_survive_presets_accessors_and_factory() {
    use sotf_plugins::{ParameterId, ParameterValue};

    // Presets saved before these controls existed keep zero lookahead, the
    // legacy minimum-phase bank, and L/R internal detection.
    let mut settings: PluginSettings =
        serde_json::from_value(serde_json::json!({ "DeEsser": {} })).unwrap();
    assert_eq!(settings.param_specs().len(), 14);
    assert_eq!(settings.param_value(10), Some(0.0));
    assert_eq!(settings.param_value(11), Some(0.0));
    assert_eq!(settings.param_value(12), Some(0.0));
    assert_eq!(settings.param_value(13), Some(0.0));
    assert_eq!(settings.param_specs()[10].engine_key, "lookahead_ms");
    assert_eq!(settings.param_specs()[11].engine_key, "split_topology");
    assert_eq!(settings.param_specs()[12].engine_key, "ms_mode");
    assert_eq!(settings.param_specs()[13].engine_key, "sidechain_external");

    settings.set_param_value(10, 5.0);
    settings.set_param_value(11, 1.0);
    settings.set_param_value(12, 1.0);
    settings.set_param_value(13, 1.0);
    assert_eq!(settings.param_value(10), Some(5.0));
    assert_eq!(settings.param_value(11), Some(1.0));
    assert_eq!(settings.param_value(12), Some(1.0));
    assert_eq!(settings.param_value(13), Some(1.0));
    // Structural controls require reconstruction; only M/S mode is live.
    assert_eq!(settings.engine_param_at(10), None);
    assert_eq!(settings.engine_param_at(11), None);
    assert_eq!(
        settings.engine_param_at(12),
        Some(("ms_mode".to_string(), "true".to_string()))
    );
    assert_eq!(settings.engine_param_at(13), None);

    let restored: PluginSettings =
        serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    assert_eq!(restored.param_value(10), Some(5.0));
    assert_eq!(restored.param_value(11), Some(1.0));

    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["lookahead_ms"], 5.0);
    assert_eq!(config.parameters["split_topology"], "Linear-Phase");
    assert_eq!(config.parameters["ms_mode"], true);
    assert_eq!(config.parameters["sidechain_external"], true);
    // Engine single-width chains cannot route the 2N external key bus, so the
    // facade rejects external construction loudly instead of returning a flag
    // without an audio path. The FFI two-bus path (input 2N, output N) is the
    // supported external route.
    let external_error =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
            .err()
            .expect("external-key construction must fail");
    assert!(
        external_error.contains("2N") && external_error.contains("external"),
        "external-key rejection must name the 2N bus, got: {external_error}"
    );

    // Internal-key construction with the same lookahead/topology/mode succeeds.
    let mut internal_params = config.parameters.clone();
    internal_params["sidechain_external"] = serde_json::json!(false);
    let plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &internal_params, 2, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("lookahead_ms")),
        Some(ParameterValue::Float(5.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("split_topology")),
        Some(ParameterValue::String("Linear-Phase".to_string()))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("ms_mode")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("sidechain_external")),
        Some(ParameterValue::Bool(false))
    );

    // Rejected construction (unknown key, unknown topology) leaves the
    // accepted config rebuildable with identical state.
    let mut bad = internal_params.clone();
    bad["unknown"] = serde_json::json!(1);
    assert!(sotf_plugins::create_plugin(&config.plugin_type, &bad, 2, 48_000).is_err());
    let mut bad_topology = internal_params.clone();
    bad_topology["split_topology"] = serde_json::json!("Nope");
    assert!(
        sotf_plugins::create_plugin(&config.plugin_type, &bad_topology, 2, 48_000).is_err()
    );
    let rebuilt =
        sotf_plugins::create_plugin(&config.plugin_type, &internal_params, 2, 48_000).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("split_topology")),
        Some(ParameterValue::String("Linear-Phase".to_string()))
    );
}

#[test]
fn declick_mode_bands_and_repair_survive_presets_accessors_and_factory() {
    use sotf_plugins::{ParameterId, ParameterValue};

    // Presets saved before these controls existed keep the legacy fullband
    // random behavior with zero repair extension and monitoring off.
    let mut settings: PluginSettings =
        serde_json::from_value(serde_json::json!({ "Declick": {} })).unwrap();
    assert_eq!(settings.param_specs().len(), 9);
    for (index, expected) in [
        (3, 0.0),
        (4, 0.0),
        (5, 4000.0),
        (6, 0.0),
        (7, 0.0),
        (8, 0.0),
    ] {
        assert_eq!(settings.param_value(index), Some(expected));
    }
    assert_eq!(settings.param_specs()[3].engine_key, "mode");
    assert_eq!(settings.param_specs()[4].engine_key, "bands");
    assert_eq!(settings.param_specs()[5].engine_key, "crossover_hz");
    assert_eq!(settings.param_specs()[6].engine_key, "frequency_skew");
    assert_eq!(settings.param_specs()[7].engine_key, "repair_width");
    assert_eq!(settings.param_specs()[8].engine_key, "audition_residual");

    settings.set_param_value(3, 1.0);
    settings.set_param_value(4, 2.0);
    settings.set_param_value(5, 3000.0);
    settings.set_param_value(6, 0.5);
    settings.set_param_value(7, 4.0);
    settings.set_param_value(8, 1.0);
    assert_eq!(settings.engine_param_at(3), None);
    assert_eq!(settings.engine_param_at(4), None);
    assert_eq!(settings.engine_param_at(5), None);
    assert_eq!(
        settings.engine_param_at(6),
        Some(("frequency_skew".to_string(), "0.5".to_string()))
    );
    assert_eq!(settings.engine_param_at(7), None);
    assert_eq!(
        settings.engine_param_at(8),
        Some(("audition_residual".to_string(), "true".to_string()))
    );

    let restored: PluginSettings =
        serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    // The audition monitor survives preset round-trip as `audition_residual`.
    assert_eq!(restored.param_value(8), Some(1.0));
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["mode"], 1);
    assert_eq!(config.parameters["bands"], 2);
    assert_eq!(config.parameters["crossover_hz"], 3000.0);
    assert_eq!(config.parameters["frequency_skew"], 0.5);
    assert_eq!(config.parameters["repair_width"], 4);
    assert_eq!(config.parameters["audition_residual"], true);
    let plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("bands")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("crossover_hz")),
        Some(ParameterValue::Float(3000.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("frequency_skew")),
        Some(ParameterValue::Float(0.5))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("repair_width")),
        Some(ParameterValue::Int(4))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("audition_residual")),
        Some(ParameterValue::Bool(true))
    );

    // A mistyped field is rejected; the accepted config still builds with
    // the audition monitor intact.
    let mut bad = config.parameters.clone();
    bad["bands"] = serde_json::json!("nope");
    assert!(sotf_plugins::create_plugin(&config.plugin_type, &bad, 2, 48_000).is_err());
    let rebuilt =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("bands")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("audition_residual")),
        Some(ParameterValue::Bool(true))
    );
}

#[test]
fn hiss_profile_curve_and_link_survive_presets_accessors_and_factory() {
    use sotf_plugins::{ParameterId, ParameterValue};

    // Presets saved before these controls existed keep the static floor,
    // flat curve, independent detectors, and a disabled transient guard.
    let mut settings: PluginSettings =
        serde_json::from_value(serde_json::json!({ "HissReducer": {} })).unwrap();
    assert_eq!(settings.param_specs().len(), 13);
    for (index, expected) in [
        (5, 0.0),
        (6, 0.0),
        (7, 0.0),
        (8, 1.0),
        (9, 1.0),
        (10, 1.0),
        (11, 0.0),
        (12, 0.0),
    ] {
        assert_eq!(settings.param_value(index), Some(expected));
    }
    assert_eq!(settings.param_specs()[5].engine_key, "learn_noise");
    assert_eq!(settings.param_specs()[6].engine_key, "use_captured_profile");
    assert_eq!(settings.param_specs()[7].engine_key, "clear_profile");
    assert_eq!(settings.param_specs()[8].engine_key, "curve_low");
    assert_eq!(settings.param_specs()[11].engine_key, "link_mode");
    assert_eq!(settings.param_specs()[12].engine_key, "transient_guard");

    settings.set_param_value(6, 1.0);
    settings.set_param_value(8, 0.5);
    settings.set_param_value(9, 0.75);
    settings.set_param_value(10, 0.25);
    settings.set_param_value(11, 1.0);
    settings.set_param_value(12, 1.0);
    assert_eq!(settings.engine_param_at(5), None);
    assert_eq!(
        settings.engine_param_at(6),
        Some(("use_captured_profile".to_string(), "true".to_string()))
    );
    assert_eq!(settings.engine_param_at(7), None);
    assert_eq!(
        settings.engine_param_at(8),
        Some(("curve_low".to_string(), "0.5".to_string()))
    );
    assert_eq!(
        settings.engine_param_at(11),
        Some(("link_mode".to_string(), "1".to_string()))
    );
    // The guard is a plain realtime bool, so it rides the generic
    // zero-dropout update route like the other scalar flags.
    assert_eq!(
        settings.engine_param_at(12),
        Some(("transient_guard".to_string(), "true".to_string()))
    );

    let restored: PluginSettings =
        serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["use_captured_profile"], true);
    assert_eq!(config.parameters["curve_low"], 0.5);
    assert_eq!(config.parameters["curve_mid"], 0.75);
    assert_eq!(config.parameters["curve_high"], 0.25);
    assert_eq!(config.parameters["link_mode"], 1);
    assert_eq!(config.parameters["transient_guard"], true);
    // Runtime-only triggers never reach construction JSON: the factory
    // denies unknown fields.
    assert!(config.parameters.get("learn_noise").is_none());
    assert!(config.parameters.get("clear_profile").is_none());
    let plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("curve_low")),
        Some(ParameterValue::Float(0.5))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("link_mode")),
        Some(ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("transient_guard")),
        Some(ParameterValue::Bool(true))
    );

    let mut bad = config.parameters.clone();
    bad["unknown"] = serde_json::json!(1);
    assert!(sotf_plugins::create_plugin(&config.plugin_type, &bad, 1, 48_000).is_err());
    let rebuilt =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("curve_low")),
        Some(ParameterValue::Float(0.5))
    );
}

#[test]
fn speech_denoiser_strength_and_model_survive_presets_accessors_and_factory() {
    use sotf_plugins::{ParameterId, ParameterValue};

    // Presets saved before these controls existed keep full strength on the
    // bundled model.
    let mut settings: PluginSettings =
        serde_json::from_value(serde_json::json!({ "SpeechDenoiser": {} })).unwrap();
    assert_eq!(settings.param_specs().len(), 3);
    assert_eq!(settings.param_value(1), Some(1.0));
    assert_eq!(settings.param_value(2), Some(0.0));
    assert_eq!(settings.param_specs()[1].engine_key, "strength");
    assert_eq!(settings.param_specs()[2].engine_key, "model");

    settings.set_param_value(1, 0.5);
    assert_eq!(settings.param_value(1), Some(0.5));
    assert_eq!(
        settings.engine_param_at(1),
        Some(("strength".to_string(), "0.5".to_string()))
    );
    assert_eq!(settings.engine_param_at(2), None);

    let restored: PluginSettings =
        serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["strength"], 0.5);
    assert_eq!(config.parameters["model"], "RNNoise Full");
    let plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("strength")),
        Some(ParameterValue::Float(0.5))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("model")),
        Some(ParameterValue::Int(0))
    );

    let mut bad = config.parameters.clone();
    bad["unknown"] = serde_json::json!(1);
    assert!(sotf_plugins::create_plugin(&config.plugin_type, &bad, 1, 48_000).is_err());
    let rebuilt =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("strength")),
        Some(ParameterValue::Float(0.5))
    );
}

#[test]
fn de_esser_lookahead_latency_and_wideband_audio_reach_factory() {
    use sotf_plugins::ProcessContext;

    // Wideband is the untouched legacy path; only the lookahead plumbing is new.
    let mut settings = PluginSettings::default_for(&PluginType::DeEsser).unwrap();
    settings.set_param_value(6, 0.0);
    settings.set_param_value(10, 5.0);
    let config = settings.to_plugin_config(48_000.0);
    let mut plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();

    let mut plain_settings = PluginSettings::default_for(&PluginType::DeEsser).unwrap();
    plain_settings.set_param_value(6, 0.0);
    let plain_config = plain_settings.to_plugin_config(48_000.0);
    let mut plain =
        sotf_plugins::create_plugin(&plain_config.plugin_type, &plain_config.parameters, 2, 48_000)
            .unwrap();
    plain.initialize(48_000).unwrap();
    assert_eq!(plugin.latency_samples() - plain.latency_samples(), 240);

    let frames = 1024;
    let input: Vec<f32> = (0..frames * 2)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 7000.0 * (n / 2) as f32 / 48_000.0).sin() * 0.5
        })
        .collect();
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap(),
        frames
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| *sample != 0.0));
}

#[test]
fn declick_legacy_audio_and_repair_latency_reach_factory() {
    use sotf_plugins::ProcessContext;

    let legacy = PluginSettings::default_for(&PluginType::Declick).unwrap();
    let legacy_config = legacy.to_plugin_config(48_000.0);
    let mut legacy_plugin = sotf_plugins::create_plugin(
        &legacy_config.plugin_type,
        &legacy_config.parameters,
        2,
        48_000,
    )
    .unwrap();
    legacy_plugin.initialize(48_000).unwrap();

    let frames = 1024;
    let input: Vec<f32> = (0..frames * 2)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 440.0 * (n / 2) as f32 / 48_000.0).sin() * 0.5
        })
        .collect();
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        legacy_plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap(),
        frames
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| *sample != 0.0));

    // Repair width extends latency on the new-mode path.
    let mut narrow = PluginSettings::default_for(&PluginType::Declick).unwrap();
    narrow.set_param_value(3, 1.0);
    narrow.set_param_value(4, 1.0);
    let mut wide = narrow.clone();
    wide.set_param_value(7, 4.0);
    let narrow_config = narrow.to_plugin_config(48_000.0);
    let wide_config = wide.to_plugin_config(48_000.0);
    let narrow_plugin = sotf_plugins::create_plugin(
        &narrow_config.plugin_type,
        &narrow_config.parameters,
        2,
        48_000,
    )
    .unwrap();
    let wide_plugin = sotf_plugins::create_plugin(
        &wide_config.plugin_type,
        &wide_config.parameters,
        2,
        48_000,
    )
    .unwrap();
    assert!(wide_plugin.latency_samples() > narrow_plugin.latency_samples());
}

#[test]
fn hiss_default_audio_reaches_factory() {
    use sotf_plugins::ProcessContext;

    let settings = PluginSettings::default_for(&PluginType::HissReducer).unwrap();
    let config = settings.to_plugin_config(48_000.0);
    let mut plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();

    let frames = 2048;
    let input: Vec<f32> = (0..frames)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin() * 0.4
                + (2.0 * std::f32::consts::PI * 9000.0 * n as f32 / 48_000.0).sin() * 0.1
        })
        .collect();
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap(),
        frames
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| *sample != 0.0));
}

#[test]
fn hiss_transient_guard_reaches_factory_audio_and_survives_reload() {
    use sotf_plugins::{ParameterId, ParameterValue, ProcessContext};

    // Guard-on spectral construction through engine settings renders real
    // audio and carries the flag through a JSON save/reload round trip.
    let mut settings = PluginSettings::default_for(&PluginType::HissReducer).unwrap();
    let guard_index = param_specs::index_of(settings.param_specs(), "transient_guard");
    assert_eq!(guard_index, 12);
    let spectral_index = param_specs::index_of(settings.param_specs(), "spectral_mode");
    settings.set_param_value(spectral_index, 1.0);
    settings.set_param_value(guard_index, 1.0);
    assert_eq!(settings.param_value(guard_index), Some(1.0));
    let config = settings.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["transient_guard"], true);

    let frames = 8192;
    let input: Vec<f32> = (0..frames)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin() * 0.4
                + (2.0 * std::f32::consts::PI * 9000.0 * n as f32 / 48_000.0).sin() * 0.1
        })
        .collect();
    let render = |parameters: &serde_json::Value| {
        let mut plugin =
            sotf_plugins::create_plugin(&config.plugin_type, parameters, 1, 48_000).unwrap();
        plugin.initialize(48_000).unwrap();
        let mut output = vec![f32::NAN; input.len()];
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap(),
            frames
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        output
    };
    let first = render(&config.parameters);
    assert!(first.iter().any(|sample| *sample != 0.0));

    // Serialize the converter output (the engine save shape), reload it,
    // and re-render bit-exactly with the guard still engaged.
    let saved = serde_json::to_string(&config.parameters).unwrap();
    let reloaded: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(reloaded["transient_guard"], true);
    let second = render(&reloaded);
    assert_eq!(first, second);
    let rebuilt =
        sotf_plugins::create_plugin(&config.plugin_type, &reloaded, 1, 48_000).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("transient_guard")),
        Some(ParameterValue::Bool(true))
    );

    // Old-state JSON without the key constructs guard-off and renders the
    // legacy path bit-identically to an explicit guard:false build.
    let mut legacy = config.parameters.clone();
    assert!(legacy.as_object_mut().unwrap().remove("transient_guard").is_some());
    let legacy_out = render(&legacy);
    let mut explicit_off = config.parameters.clone();
    explicit_off["transient_guard"] = serde_json::json!(false);
    assert_eq!(legacy_out, render(&explicit_off));
    let legacy_plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &legacy, 1, 48_000).unwrap();
    assert_eq!(
        legacy_plugin.get_parameter(&ParameterId::from("transient_guard")),
        Some(ParameterValue::Bool(false))
    );

    // Rejected candidates retain the accepted configuration: unknown keys
    // still fail closed while the saved config rebuilds verbatim.
    let mut bad = config.parameters.clone();
    bad["unknown"] = serde_json::json!(1);
    assert!(sotf_plugins::create_plugin(&config.plugin_type, &bad, 1, 48_000).is_err());
    assert_eq!(render(&reloaded), first);
}

#[test]
fn speech_strength_zero_emits_delayed_dry_through_factory() {
    use sotf_plugins::ProcessContext;

    let mut dry_settings = PluginSettings::default_for(&PluginType::SpeechDenoiser).unwrap();
    dry_settings.set_param_value(1, 0.0);
    let dry_config = dry_settings.to_plugin_config(48_000.0);
    let mut dry = sotf_plugins::create_plugin(
        &dry_config.plugin_type,
        &dry_config.parameters,
        1,
        48_000,
    )
    .unwrap();
    dry.initialize(48_000).unwrap();

    let wet_settings = PluginSettings::default_for(&PluginType::SpeechDenoiser).unwrap();
    let wet_config = wet_settings.to_plugin_config(48_000.0);
    let mut wet = sotf_plugins::create_plugin(
        &wet_config.plugin_type,
        &wet_config.parameters,
        1,
        48_000,
    )
    .unwrap();
    wet.initialize(48_000).unwrap();

    // Three 480-frame model windows; the first 960 frames are delay fill.
    let frames = 1440;
    let input: Vec<f32> = (0..frames)
        .map(|n| (2.0 * std::f32::consts::PI * 220.0 * n as f32 / 48_000.0).sin() * 0.5)
        .collect();
    let mut dry_output = vec![f32::NAN; input.len()];
    let mut wet_output = vec![f32::NAN; input.len()];
    assert_eq!(
        dry.process(&input, &mut dry_output, &ProcessContext::new(48_000, frames))
            .unwrap(),
        frames
    );
    assert_eq!(
        wet.process(&input, &mut wet_output, &ProcessContext::new(48_000, frames))
            .unwrap(),
        frames
    );
    assert!(dry_output.iter().all(|sample| sample.is_finite()));
    assert!(wet_output.iter().all(|sample| sample.is_finite()));
    assert!(dry_output[960..].iter().any(|sample| *sample != 0.0));
    assert_ne!(dry_output, wet_output);
}

#[test]
fn dither_non_default_config_reaches_facade_audio_and_survives_reload() {
    use sotf_plugins::{ParameterId, ParameterValue, ProcessContext};

    // Non-default dither config through engine settings, converter, facade,
    // audio, and save/reload. Parse owns range truthfulness (out-of-range
    // indices rejected); unknown keys are ignored per the lenient DSP
    // contract; direct `from_params` clamps (documented in the bridge test).
    let mut settings = PluginSettings::default_for(&PluginType::Dither).unwrap();
    assert_eq!(settings.param_specs().len(), 3);
    settings.set_param_value(0, 2.0);
    settings.set_param_value(1, 0.0);
    settings.set_param_value(2, 1.0);
    let restored: PluginSettings =
        serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["bit_depth"], 2);
    assert_eq!(config.parameters["noise_shaping"], false);
    assert_eq!(config.parameters["dither_type"], 1);

    let mut configured =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    assert_eq!(
        configured.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );
    configured.initialize(48_000).unwrap();

    let default_settings = PluginSettings::default_for(&PluginType::Dither).unwrap();
    let default_config = default_settings.to_plugin_config(48_000.0);
    let mut defaulted = sotf_plugins::create_plugin(
        &default_config.plugin_type,
        &default_config.parameters,
        2,
        48_000,
    )
    .unwrap();
    defaulted.initialize(48_000).unwrap();

    let frames = 1024;
    let input: Vec<f32> = (0..frames * 2)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 440.0 * (n / 2) as f32 / 48_000.0).sin() * 0.5
        })
        .collect();
    let render = |plugin: &mut Box<dyn sotf_plugins::Plugin>| {
        let mut output = vec![f32::NAN; input.len()];
        let rendered = plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        assert_eq!(rendered, frames);
        output
    };
    let configured_out = render(&mut configured);
    let default_out = render(&mut defaulted);
    assert!(configured_out.iter().all(|sample| sample.is_finite()));
    assert!(configured_out.iter().any(|sample| *sample != 0.0));
    assert_ne!(configured_out, default_out);

    // Save/reload reproduces the render bit-exactly.
    let reloaded_settings: PluginSettings =
        serde_json::from_value(serde_json::to_value(&restored).unwrap()).unwrap();
    let reloaded_config = reloaded_settings.to_plugin_config(48_000.0);
    let mut reloaded = sotf_plugins::create_plugin(
        &reloaded_config.plugin_type,
        &reloaded_config.parameters,
        2,
        48_000,
    )
    .unwrap();
    reloaded.initialize(48_000).unwrap();
    assert_eq!(render(&mut reloaded), configured_out);

    // Out-of-range indices are rejected; unknown keys are ignored (lenient).
    let mut bad = config.parameters.clone();
    bad["bit_depth"] = serde_json::json!(99);
    assert!(sotf_plugins::create_plugin(&config.plugin_type, &bad, 2, 48_000).is_err());
    let mut lenient = config.parameters.clone();
    lenient["unknown"] = serde_json::json!(1);
    let lenient_plugin =
        sotf_plugins::create_plugin(&config.plugin_type, &lenient, 2, 48_000).unwrap();
    assert_eq!(
        lenient_plugin.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );
    // Rejection leaves the accepted config rebuildable with identical audio.
    let mut rebuilt =
        sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    rebuilt.initialize(48_000).unwrap();
    assert_eq!(render(&mut rebuilt), configured_out);
}
