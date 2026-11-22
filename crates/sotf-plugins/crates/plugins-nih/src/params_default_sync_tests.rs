//! Check the plugin metadata used by every NIH wrapper against its DSP instance.

use super::*;

#[cfg(feature = "upmixer")]
#[test]
fn staged_upmixer_state_keeps_later_away_and_back_automation() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let entry = params.param_map.get("height_gain").unwrap();
    let original = params.float_params[entry.index].unmodulated_plain_value();
    let snapshot = PluginState {
        version: String::new(),
        params: [("height_gain".into(), NativeParamValue::F32(0.3))].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&snapshot));
    params.float_params[entry.index].set_plain_value_for_initialization(0.7);
    params.float_params[entry.index].set_plain_value_for_initialization(original);
    assert!(
        matches!(params.serialize_parameter_overrides().get("height_gain"), Some(NativeParamValue::F32(value)) if *value == original)
    );
    assert!(params.begin_upmixer_restore());
    assert_eq!(
        params.initialization_value("height_gain"),
        Some(ParameterValue::Float(original))
    );
    assert!(params.complete_upmixer_restore(48_000.0));
    assert_eq!(
        params.value("height_gain"),
        Some(ParameterValue::Float(original))
    );
}

#[cfg(feature = "upmixer")]
#[test]
fn invalid_staged_upmixer_replacement_keeps_the_preceding_snapshot() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let state = |value| PluginState {
        version: String::new(),
        params: [("height_gain".into(), value)].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&state(NativeParamValue::F32(0.3))));
    assert!(!params.stage_upmixer_state(&state(NativeParamValue::Bool(true))));
    assert!(!params.stage_upmixer_state(&state(NativeParamValue::F32(1000.0))));
    assert!(params.begin_upmixer_restore());
    assert_eq!(
        params.initialization_value("height_gain"),
        Some(ParameterValue::Float(0.3))
    );
    assert!(params.complete_upmixer_restore(48_000.0));
    assert_eq!(
        params.value("height_gain"),
        Some(ParameterValue::Float(0.3))
    );
}

#[cfg(feature = "upmixer")]
#[test]
fn staged_upmixer_commit_does_not_erase_a_later_host_edit() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let entry = params.param_map.get("height_gain").unwrap();
    let front_before = params.value("gain_front_direct").unwrap();
    let snapshot = PluginState {
        version: String::new(),
        params: [
            ("gain_front_direct".into(), NativeParamValue::F32(0.3)),
            ("height_gain".into(), NativeParamValue::F32(0.3)),
        ]
        .into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&snapshot));
    assert!(params.begin_upmixer_restore());
    let committed = params.complete_upmixer_restore_with_hook(
        48_000.0,
        || {
            params.float_params[entry.index].set_plain_value_for_initialization(0.7);
        },
        |_| {},
    );
    assert!(!committed);
    assert_eq!(
        params.value("height_gain"),
        Some(ParameterValue::Float(0.7))
    );
    assert_eq!(params.value("gain_front_direct"), Some(front_before));
}

#[cfg(feature = "upmixer")]
#[test]
fn contested_upmixer_commit_rolls_back_other_values_and_keeps_latest_edit() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let entry = params.param_map.get("gain_front_direct").unwrap();
    let height_before = params.value("height_gain").unwrap();
    let snapshot = PluginState {
        version: String::new(),
        params: [
            ("gain_front_direct".into(), NativeParamValue::F32(0.3)),
            ("height_gain".into(), NativeParamValue::F32(0.3)),
        ]
        .into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&snapshot));
    assert!(params.begin_upmixer_restore());
    let committed = params.complete_upmixer_restore_with_hook(
        48_000.0,
        || {
            params.float_params[entry.index].set_plain_value_for_initialization(0.7);
        },
        |_| {},
    );
    assert!(!committed);
    assert_eq!(
        params.value("gain_front_direct"),
        Some(ParameterValue::Float(0.7))
    );
    assert_eq!(params.value("height_gain"), Some(height_before));
}

#[cfg(feature = "upmixer")]
#[test]
fn contested_upmixer_commit_keeps_a_host_write_paused_before_its_parameter_store() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let index = params
        .sync_entries
        .iter()
        .position(|entry| entry.id.as_str() == "gain_front_direct")
        .unwrap();
    let entry = &params.sync_entries[index];
    let snapshot = PluginState {
        version: String::new(),
        params: [("gain_front_direct".into(), NativeParamValue::F32(0.3))].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&snapshot));
    assert!(params.begin_upmixer_restore());
    let committed = params.complete_upmixer_restore_with_hook(
        48_000.0,
        || {
            // The host has entered its pre-write hook but has not stored its value yet.
            record_upmixer_write(&params.upmixer_revisions[index], 0.7_f32.to_bits());
        },
        |_| {},
    );
    assert!(!committed);
    assert_eq!(
        params.value("gain_front_direct"),
        Some(ParameterValue::Float(0.7))
    );
    params.float_params[entry.index].set_prepared_value_untracked(0.7, 48_000.0);
    assert_eq!(
        params.value("gain_front_direct"),
        Some(ParameterValue::Float(0.7))
    );
}

#[cfg(feature = "upmixer")]
#[test]
fn contested_upmixer_rollback_keeps_every_later_host_edit() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let index = params
        .sync_entries
        .iter()
        .position(|entry| entry.id.as_str() == "gain_front_direct")
        .unwrap();
    let entry = &params.sync_entries[index];
    let snapshot = PluginState {
        version: String::new(),
        params: [("gain_front_direct".into(), NativeParamValue::F32(0.3))].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&snapshot));
    assert!(params.begin_upmixer_restore());
    let writes = std::cell::Cell::new(0);
    let committed = params.complete_upmixer_restore_with_hook(
        48_000.0,
        || {
            params.float_params[entry.index].set_plain_value_for_initialization(0.5);
        },
        |touched| {
            if touched == index && writes.get() < 96 {
                let value = if writes.get() % 2 == 0 { 0.7 } else { 0.8 };
                params.float_params[entry.index].set_plain_value_for_initialization(value);
                writes.set(writes.get() + 1);
            }
        },
    );
    assert!(!committed);
    assert_eq!(writes.get(), 96);
    assert_eq!(
        params.value("gain_front_direct"),
        Some(ParameterValue::Float(0.8))
    );
}

#[cfg(feature = "upmixer")]
#[test]
fn failed_upmixer_candidate_restores_the_previous_pending_snapshot() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let state = |value| PluginState {
        version: String::new(),
        params: [("height_gain".into(), NativeParamValue::F32(value))].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&state(0.3)));
    assert!(params.stage_upmixer_state(&state(0.7)));
    assert!(params.begin_upmixer_restore());
    {
        let _attempt = UpmixerRestoreAttempt::new(Arc::clone(&params));
        assert_eq!(
            params.initialization_value("height_gain"),
            Some(ParameterValue::Float(0.7))
        );
        // Candidate construction fails before commit.
    }
    assert!(
        matches!(params.serialize_parameter_overrides().get("height_gain"), Some(NativeParamValue::F32(value)) if *value == 0.3)
    );
    assert!(params.begin_upmixer_restore());
    assert_eq!(
        params.initialization_value("height_gain"),
        Some(ParameterValue::Float(0.3))
    );
    assert!(params.complete_upmixer_restore(48_000.0));
    assert_eq!(
        params.value("height_gain"),
        Some(ParameterValue::Float(0.3))
    );
}

#[cfg(feature = "upmixer")]
#[test]
fn failed_old_upmixer_candidate_keeps_a_newer_pending_snapshot() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let state = |value| PluginState {
        version: String::new(),
        params: [("height_gain".into(), NativeParamValue::F32(value))].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&state(0.3)));
    assert!(params.begin_upmixer_restore());
    {
        let _attempt = UpmixerRestoreAttempt::new(Arc::clone(&params));
        assert!(params.stage_upmixer_state(&state(0.7)));
        assert!(!params.complete_upmixer_restore(48_000.0));
    }
    assert!(params.begin_upmixer_restore());
    assert_eq!(
        params.initialization_value("height_gain"),
        Some(ParameterValue::Float(0.7))
    );
    assert!(params.complete_upmixer_restore(48_000.0));
}

#[cfg(feature = "upmixer")]
#[test]
fn upmixer_pending_snapshot_history_is_bounded_without_losing_the_latest() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let state = |value| PluginState {
        version: String::new(),
        params: [("height_gain".into(), NativeParamValue::F32(value))].into(),
        fields: BTreeMap::new(),
    };
    for index in 0..9 {
        assert!(params.stage_upmixer_state(&state(index as f32 / 10.0)));
    }
    assert!(!params.stage_upmixer_state(&state(0.9)));
    assert!(
        matches!(params.serialize_parameter_overrides().get("height_gain"), Some(NativeParamValue::F32(value)) if *value == 0.8)
    );
    assert!(params.begin_upmixer_restore());
    assert!(params.complete_upmixer_restore(48_000.0));
    assert!(params.stage_upmixer_state(&state(0.9)));
}

#[cfg(feature = "upmixer")]
#[test]
fn upmixer_can_stage_a_new_snapshot_after_a_long_running_session() {
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &schema_infos("Upmixer"));
    let index = params
        .sync_entries
        .iter()
        .position(|entry| entry.id.as_str() == "height_gain")
        .unwrap();
    params.upmixer_revisions[index].store(
        (u64::from(u32::MAX) << 32) | u64::from(0.7_f32.to_bits()),
        Ordering::Release,
    );
    let snapshot = PluginState {
        version: String::new(),
        params: [("height_gain".into(), NativeParamValue::F32(0.3))].into(),
        fields: BTreeMap::new(),
    };
    assert!(params.stage_upmixer_state(&snapshot));
    assert!(params.begin_upmixer_restore());
    assert!(params.complete_upmixer_restore(48_000.0));
    assert_eq!(
        params.value("height_gain"),
        Some(ParameterValue::Float(0.3))
    );
}

pub(super) fn schema_infos(name: &str) -> Vec<BridgedParamInfo> {
    let bridge = plugins_bridge::ParamBridge::new(crate::wrapper::get_param_specs(name));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    if infos.is_empty() || matches!(name, "EQ" | "LinearPhaseEQ") {
        let plugin = plugins_bridge::create_plugin(
            name,
            crate::wrapper::plugin_constructor_channels(name),
            48_000.0,
            &crate::wrapper::default_plugin_config(name),
        )
        .unwrap();
        for parameter in plugin.parameters() {
            if !infos.iter().any(|info| info.id == parameter.id.as_str())
                && let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter)
            {
                infos.push(info);
            }
        }
    }
    for info in &mut infos {
        info.id = crate::wrapper::legacy_external_param_id(name, &info.id).into_owned();
    }
    infos
}

#[cfg(feature = "upmixer")]
#[test]
fn native_upmixer_parameter_indices_are_stable_across_instances() {
    let infos = schema_infos("Upmixer");
    assert!(!infos.is_empty());
    let parameter_ids = || {
        DynamicParams::from_infos_for_plugin("Upmixer", &infos)
            .param_map()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect::<Vec<_>>()
    };
    let expected = parameter_ids();
    assert_eq!(expected.len(), infos.len());
    assert_eq!(
        expected
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        expected.len()
    );
    for _ in 0..10 {
        assert_eq!(parameter_ids(), expected);
    }
}

#[cfg(feature = "upmixer")]
#[test]
fn native_upmixer_syncs_nondefault_realtime_controls() {
    let mut infos = schema_infos("Upmixer");
    for (id, value) in [("height_gain", 0.5), ("bypass_heights", 1.0)] {
        let info = infos.iter_mut().find(|info| info.id == id).unwrap();
        assert!(info.realtime);
        info.default_value = value;
    }
    let params = DynamicParams::from_infos_for_plugin("Upmixer", &infos);
    let mut plugin = plugins_bridge::create_plugin("Upmixer", 2, 48_000.0, "{}").unwrap();
    plugin.initialize(48_000.0).unwrap();
    for _ in 0..2 {
        params.sync_to_plugin(plugin.as_mut()).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("height_gain")),
            Some(ParameterValue::Float(0.5))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("bypass_heights")),
            Some(ParameterValue::Bool(true))
        );
    }
}

#[test]
fn native_band_split_constructs_with_requested_cutoff_at_fractional_low_rate() {
    let params = DynamicParams::from_infos_for_plugin("BandSplit", &schema_infos("BandSplit"));
    let cutoff = ParameterId::from("frequency");
    assert_eq!(
        params.value("frequency"),
        Some(ParameterValue::Float(300.0))
    );
    let mut plugin = super::configuration::create_plugin("BandSplit", 1_234.567_8, &params)
        .expect("native 300 Hz cutoff must be used during construction");
    plugin.initialize(1_234.567_8).unwrap();
    params.sync_to_plugin(plugin.as_mut()).unwrap();
    assert_eq!(
        plugin.get_parameter(&cutoff),
        Some(ParameterValue::Float(300.0))
    );
    let input = vec![0.1_f32; 64 * plugin.input_channels()];
    let mut output = vec![0.0_f32; 64 * plugin.output_channels()];
    let written = plugin
        .process(
            &input,
            &mut output,
            &sotf_host::ProcessContext::new(1_234.567_8, 64),
        )
        .unwrap();
    assert_eq!(written, 64);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        params.value("frequency"),
        Some(ParameterValue::Float(300.0))
    );

    let mut at_48k = super::configuration::create_plugin("BandSplit", 48_000.0, &params).unwrap();
    at_48k.initialize(48_000.0).unwrap();
    params.sync_to_plugin(at_48k.as_mut()).unwrap();
    assert_eq!(
        at_48k.get_parameter(&cutoff),
        Some(ParameterValue::Float(300.0))
    );
}

#[test]
fn native_hiss_uses_effective_low_rate_cutoff_without_changing_host_request() {
    let params = DynamicParams::from_infos_for_plugin("HissReducer", &schema_infos("HissReducer"));
    let cutoff = ParameterId::from("frequency_hz");
    assert_eq!(
        params.value("frequency_hz"),
        Some(ParameterValue::Float(4_000.0))
    );
    let mut plugin = super::configuration::create_plugin("HissReducer", 8_000.0, &params).unwrap();
    plugin.initialize(8_000.0).unwrap();
    params
        .sync_to_plugin_for_activation(plugin.as_mut(), 8_000.0)
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&cutoff),
        Some(ParameterValue::Float(3_600.0))
    );
    let input = vec![0.1_f32; 128];
    let mut output = vec![0.0_f32; 128];
    let written = plugin
        .process(
            &input,
            &mut output,
            &sotf_host::ProcessContext::new(8_000.0, 64),
        )
        .unwrap();
    assert_eq!(written, 64);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        params.value("frequency_hz"),
        Some(ParameterValue::Float(4_000.0))
    );

    let mut at_48k = super::configuration::create_plugin("HissReducer", 48_000.0, &params).unwrap();
    at_48k.initialize(48_000.0).unwrap();
    params
        .sync_to_plugin_for_activation(at_48k.as_mut(), 48_000.0)
        .unwrap();
    assert_eq!(
        at_48k.get_parameter(&cutoff),
        Some(ParameterValue::Float(4_000.0))
    );
}

#[test]
fn nondefault_realtime_values_preserve_runtime_types() {
    // Stepped floats must remain floats even when their range contains a small,
    // integral number of steps. Choice indices and booleans retain their types.
    for (name, id, expected) in [
        ("Gate", "threshold", ParameterValue::Float(-32.0)),
        ("Gate", "hold", ParameterValue::Float(25.0)),
        ("Gate", "range_db", ParameterValue::Float(48.0)),
        ("Crossfeed", "mb_low_freq_hz", ParameterValue::Float(180.0)),
        ("Crossfeed", "enabled", ParameterValue::Bool(false)),
        ("AAE", "room_size", ParameterValue::Float(1.5)),
        ("AAE", "content_aware", ParameterValue::Bool(false)),
        ("AAE", "auto_gain_enabled", ParameterValue::Bool(true)),
        ("Dither", "dither_type", ParameterValue::Int(2)),
        ("Dither", "bit_depth", ParameterValue::Int(2)),
    ] {
        let mut infos = schema_infos(name);
        let info = infos
            .iter_mut()
            .find(|info| info.id == id)
            .unwrap_or_else(|| panic!("missing {name}.{id}"));
        assert!(info.realtime, "{name}.{id} must remain realtime");
        let raw_value = match expected {
            ParameterValue::Float(value) => {
                assert_eq!(info.kind, BridgedParamKind::Float, "{name}.{id}");
                f64::from(value)
            }
            ParameterValue::Bool(value) => {
                assert_eq!(info.kind, BridgedParamKind::Bool, "{name}.{id}");
                f64::from(value)
            }
            ParameterValue::Int(value) => {
                assert_eq!(info.kind, BridgedParamKind::Int, "{name}.{id}");
                f64::from(value)
            }
            _ => unreachable!("this test covers scalar realtime controls"),
        };
        assert_ne!(info.default_value, raw_value, "{name}.{id}");
        // Model the parameter state restored by the DAW before activation.
        info.default_value = raw_value;
        let params = DynamicParams::from_infos(&infos);
        let mut plugin = plugins_bridge::create_plugin(name, 2, 48_000.0, "{}").unwrap();
        plugin.initialize(48_000.0).unwrap();
        let parameter_id = ParameterId::from(id);
        assert_ne!(plugin.get_parameter(&parameter_id), Some(expected.clone()));
        params
            .sync_to_plugin(plugin.as_mut())
            .unwrap_or_else(|error| panic!("{name}.{id}: {error}"));
        assert_eq!(
            plugin.get_parameter(&parameter_id),
            Some(expected.clone()),
            "{name}.{id}"
        );
        params.sync_to_plugin(plugin.as_mut()).unwrap();
        assert_eq!(
            plugin.get_parameter(&parameter_id),
            Some(expected),
            "{name}.{id} repeated sync"
        );
    }
}

#[test]
fn aae_room_preset_schema_matches_setup_only_runtime_contract() {
    use sotf_host::param_specs::UpdateMode;

    let infos = schema_infos("AAE");
    let room_preset = infos.iter().find(|info| info.id == "room_preset").unwrap();
    assert!(!room_preset.realtime);
    assert_eq!(room_preset.kind, BridgedParamKind::Int);
    let mut plugin = plugins_bridge::create_plugin("AAE", 2, 48_000.0, "{}").unwrap();
    plugin.initialize(48_000.0).unwrap();
    let parameters = plugin.parameters();
    let runtime = parameters
        .iter()
        .find(|parameter| parameter.id.as_str() == "room_preset")
        .unwrap();
    assert_eq!(runtime.update_mode, UpdateMode::Structural);
    assert_eq!(
        plugin.get_parameter(&runtime.id),
        Some(ParameterValue::String("medium".to_string()))
    );
    DynamicParams::from_infos(&infos)
        .sync_to_plugin(plugin.as_mut())
        .unwrap();
    assert!(
        plugin
            .set_parameter(
                runtime.id.clone(),
                ParameterValue::String("large".to_string())
            )
            .is_err()
    );
}

#[test]
fn legacy_choice_ids_preserve_saved_state_identity() {
    for (name, canonical, external) in [
        ("Crossfeed", "mode", "crossfeed_mode"),
        ("Crossfeed", "preset", "crossfeed_preset"),
        ("LinearPhaseEQ", "fir_length_index", "fir_length"),
        ("LinearPhaseEQ", "phase_mode_index", "phase_mode"),
        ("SpectralCompressor", "fft_size_index", "fft_size"),
        ("BandSplit", "type", "crossover_type"),
    ] {
        let mut infos = schema_infos(name);
        let info = infos.iter_mut().find(|info| info.id == external).unwrap();
        assert_eq!(info.kind, BridgedParamKind::Int, "{name}.{external}");
        let value = if info.default_value == info.max_value {
            info.min_value
        } else {
            info.max_value
        };
        assert_ne!(value, info.default_value);
        info.default_value = value;
        let params = DynamicParams::from_infos(&infos);
        assert_eq!(
            params.value(external),
            Some(ParameterValue::Int(value as i32)),
            "{name}.{external}"
        );
        assert!(
            params.value(canonical).is_none(),
            "{name}.{canonical} must not rename the DAW ID"
        );
    }
}

#[test]
fn all_wrapper_defaults_sync_to_initialized_plugins() {
    let mut failures = Vec::new();
    for name in [
        "EQ",
        "Compressor",
        "Limiter",
        "Gate",
        "Gain",
        "Delay",
        "Expander",
        "Crossfeed",
        "Saturation",
        "Denoiser",
        "SpeechDenoiser",
        "HissReducer",
        "Declick",
        "Downmix",
        "MonoToStereo",
        "StereoImager",
        "TransientShaper",
        "DeEsser",
        "DynamicEQ",
        "MultibandCompressor",
        "MultibandExpander",
        "Convolution",
        "FletcherMunson",
        "LoudnessCompensation",
        "ChannelMuteSolo",
        "Upmixer",
        "AAE",
        "XTC",
        "Binaural",
        "Matrix",
        "PND",
        "ABCompare",
        "Crossover",
        "BandSplit",
        "BandMerge",
        "AEC",
        "Beamformer",
        "LinearPhaseEQ",
        "SpectralCompressor",
        "LoudnessMonitor",
        "SpectrumAnalyzer",
        "AmbisonicsDecoder",
        "Dither",
        "AnalogLimiter",
    ] {
        let specs = crate::wrapper::get_param_specs(name);
        let bridge = plugins_bridge::ParamBridge::new(specs);
        let mut infos: Vec<_> = (0..bridge.count()).filter_map(|i| bridge.info(i)).collect();
        let channels = crate::wrapper::plugin_constructor_channels(name);
        let config = crate::wrapper::default_plugin_config(name);
        let mut plugin = match plugins_bridge::create_plugin(name, channels, 48_000.0, &config) {
            Ok(plugin) => plugin,
            Err(error) => {
                failures.push(format!("{name} construction: {error}"));
                continue;
            }
        };
        if let Err(error) = plugin.initialize(48_000.0) {
            failures.push(format!("{name} initialization: {error}"));
            continue;
        }
        if infos.is_empty() || matches!(name, "EQ" | "LinearPhaseEQ") {
            for parameter in plugin.parameters() {
                if !infos.iter().any(|info| info.id == parameter.id.as_str())
                    && let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter)
                {
                    infos.push(info);
                }
            }
        }
        for info in &mut infos {
            info.id = crate::wrapper::legacy_external_param_id(name, &info.id).into_owned();
        }
        let params = DynamicParams::from_infos_for_plugin(name, &infos);
        plugin = match super::configuration::create_plugin(name, 48_000.0, &params) {
            Ok(plugin) => plugin,
            Err(error) => {
                failures.push(format!("{name} restored construction: {error}"));
                continue;
            }
        };
        if let Err(error) = plugin.initialize(48_000.0) {
            failures.push(format!("{name} restored initialization: {error}"));
            continue;
        }
        if let Err(error) = params.sync_to_plugin(plugin.as_mut()) {
            failures.push(format!("{name} synchronization: {error}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn denoiser_saved_profile_commands_do_not_run_during_restoration() {
    let mut infos = schema_infos("Denoiser");
    for info in &mut infos {
        if matches!(info.id.as_str(), "learn_noise" | "clear_profile") {
            info.default_value = 1.0;
        }
    }
    let params = DynamicParams::from_infos(&infos);
    let mut plugin = super::configuration::create_plugin("Denoiser", 48_000.0, &params).unwrap();
    for id in ["learn_noise", "clear_profile"] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(id)),
            Some(ParameterValue::Bool(false))
        );
    }
    plugin.initialize(48_000.0).unwrap();
    for id in ["learn_noise", "clear_profile"] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(id)),
            Some(ParameterValue::Bool(false))
        );
    }
}

#[test]
fn every_exposed_structural_control_restores_or_rejects_unsupported_layout() {
    let mut failures = Vec::new();
    for name in [
        "EQ",
        "Compressor",
        "Limiter",
        "Gate",
        "Delay",
        "Crossfeed",
        "Saturation",
        "Denoiser",
        "HissReducer",
        "Downmix",
        "MonoToStereo",
        "DeEsser",
        "DynamicEQ",
        "MultibandCompressor",
        "MultibandExpander",
        "Convolution",
        "FletcherMunson",
        "LoudnessCompensation",
        "Upmixer",
        "AAE",
        "Binaural",
        "Matrix",
        "PND",
        "ABCompare",
        "BandSplit",
        "BandMerge",
        "AEC",
        "Beamformer",
        "LinearPhaseEQ",
        "SpectralCompressor",
        "SpectrumAnalyzer",
        "AmbisonicsDecoder",
        "AnalogLimiter",
    ] {
        let defaults = schema_infos(name);
        for index in 0..defaults.len() {
            let info = &defaults[index];
            if info.realtime || info.kind == BridgedParamKind::FilePath {
                continue;
            }
            let mut infos = defaults.clone();
            if name == "LinearPhaseEQ" && info.id.starts_with("band_") {
                infos
                    .iter_mut()
                    .find(|info| info.id == "num_filters")
                    .unwrap()
                    .default_value = 10.0;
            }
            if name == "Downmix" && info.id == "matrix_ltrt" {
                infos
                    .iter_mut()
                    .find(|info| info.id == "phase_coherence")
                    .unwrap()
                    .default_value = 0.0;
            }
            let value = match info.kind {
                BridgedParamKind::Bool => 1.0 - info.default_value,
                BridgedParamKind::Int => {
                    let step = if name == "EQ" && info.id.ends_with("_order") {
                        2.0
                    } else {
                        1.0
                    };
                    if info.default_value + step <= info.max_value {
                        info.default_value + step
                    } else {
                        info.default_value - step
                    }
                }
                BridgedParamKind::Float => {
                    let delta = (info.max_value - info.min_value) / 8.0;
                    if info.default_value + delta <= info.max_value {
                        info.default_value + delta
                    } else {
                        info.default_value - delta
                    }
                }
                BridgedParamKind::FilePath => unreachable!(),
            };
            infos[index].default_value = value;
            let params = DynamicParams::from_infos(&infos);
            let result = super::configuration::create_plugin(name, 48_000.0, &params).and_then(
                |mut plugin| {
                    let before: Vec<_> = plugin
                        .parameters()
                        .into_iter()
                        .map(|parameter| {
                            let value = plugin.get_parameter(&parameter.id);
                            (parameter.id, value)
                        })
                        .collect();
                    plugin.initialize(48_000.0)?;
                    for (id, expected) in before {
                        if plugin.get_parameter(&id) != expected {
                            return Err(format!(
                                "{id} changed during initialize after structural restoration"
                            ));
                        }
                    }
                    Ok(())
                },
            );
            let must_reject = matches!(
                (name, info.id.as_str()),
                ("AAE" | "Upmixer", "speaker_config")
                    | ("Upmixer", "binaural_preview" | "bass_mode" | "input_format")
                    | ("Binaural", "input_channels")
                    | ("Beamformer", "num_mics")
                    | ("BandMerge", "bands")
                    | (
                        "Compressor",
                        "program_dependent_release" | "sidechain_external"
                    )
            );
            match (result, must_reject) {
                (Ok(()), true) => failures.push(format!(
                    "{name}.{}={value}: unsupported change was silently accepted",
                    info.id
                )),
                (Err(error), false) => {
                    failures.push(format!("{name}.{}={value}: {error}", info.id))
                }
                (Err(error), true) => assert!(!error.is_empty()),
                (Ok(()), false) => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn compressor_detector_roundtrip_restores_and_processes() {
    // Native Compressor/MultibandCompressor detector controls restore through
    // construction: HPF 120 Hz enabled 4th order with RMS detection builds,
    // survives initialization, and processes nonzero audio. The two still
    // unsupported legacy controls keep their loud restore rejection.
    for name in ["Compressor", "MultibandCompressor"] {
        let mut infos = schema_infos(name);
        for (id, kind, value) in [
            ("sidechain_hpf_hz", BridgedParamKind::Float, 120.0),
            ("sidechain_hpf_order", BridgedParamKind::Int, 1.0),
            ("sidechain_hpf_enabled", BridgedParamKind::Bool, 1.0),
            ("detection_mode", BridgedParamKind::Int, 1.0),
        ] {
            let info = infos
                .iter_mut()
                .find(|info| info.id == id)
                .unwrap_or_else(|| panic!("missing {name}.{id}"));
            assert_eq!(info.kind, kind, "{name}.{id}");
            assert!(!info.realtime, "{name}.{id} restores at construction");
            assert_ne!(info.default_value, value, "{name}.{id}");
            info.default_value = value;
        }
        let params = DynamicParams::from_infos(&infos);
        let mut plugin = super::configuration::create_plugin(name, 48_000.0, &params)
            .unwrap_or_else(|error| panic!("{name} detector restore: {error}"));
        for (id, expected) in [
            ("sidechain_hpf_hz", ParameterValue::Float(120.0)),
            ("sidechain_hpf_order", ParameterValue::Int(1)),
            ("sidechain_hpf_enabled", ParameterValue::Bool(true)),
            ("detection_mode", ParameterValue::Int(1)),
        ] {
            assert_eq!(
                plugin.get_parameter(&ParameterId::from(id)),
                Some(expected),
                "{name}.{id}"
            );
        }
        plugin.initialize(48_000.0).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("detection_mode")),
            Some(ParameterValue::Int(1)),
            "{name}.detection_mode survives initialize"
        );
        // Process smoke: LF-heavy program through the restored detector.
        let frames = 512usize;
        let mut input = vec![0.0_f32; frames * 2];
        for frame in 0..frames {
            let t = frame as f32 / 48_000.0;
            let sample = 0.5 * (2.0 * std::f32::consts::PI * 50.0 * t).sin()
                + 0.25 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            input[frame * 2] = sample;
            input[frame * 2 + 1] = sample * 0.5;
        }
        let mut output = vec![0.0_f32; frames * 2];
        let context = sotf_host::ProcessContext::new(48_000, frames);
        let written = plugin
            .process(&input, &mut output, &context)
            .unwrap_or_else(|error| panic!("{name} detector process: {error}"));
        assert_eq!(written, frames, "{name}");
        assert!(
            output.iter().all(|sample| sample.is_finite()),
            "{name} non-finite output"
        );
        assert!(
            output.iter().any(|sample| sample.abs() > 1.0e-3),
            "{name} silent output"
        );
        assert_ne!(output, input, "{name} detector run is bit-transparent");
    }
    // Still-unsupported legacy controls reject non-default restoration loudly.
    for id in ["program_dependent_release", "sidechain_external"] {
        let mut infos = schema_infos("Compressor");
        infos
            .iter_mut()
            .find(|info| info.id == id)
            .unwrap_or_else(|| panic!("missing Compressor.{id}"))
            .default_value = 1.0;
        let params = DynamicParams::from_infos(&infos);
        let error = match super::configuration::create_plugin("Compressor", 48_000.0, &params) {
            Ok(_) => panic!("unsupported control must reject"),
            Err(error) => error,
        };
        assert!(
            error.contains("not implemented"),
            "unexpected rejection: {error}"
        );
    }
}
