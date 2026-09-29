//! Check the plugin metadata used by every NIH wrapper against its DSP instance.

use super::*;

pub(super) fn schema_infos(name: &str) -> Vec<BridgedParamInfo> {
    let bridge = plugins_bridge::ParamBridge::new(crate::wrapper::get_param_specs(name));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    if infos.is_empty() || matches!(name, "EQ" | "LinearPhaseEQ") {
        let plugin = plugins_bridge::create_plugin(
            name,
            crate::wrapper::plugin_constructor_channels(name),
            48_000,
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
        let mut plugin = plugins_bridge::create_plugin(name, 2, 48_000, "{}").unwrap();
        plugin.initialize(48_000).unwrap();
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
    let mut plugin = plugins_bridge::create_plugin("AAE", 2, 48_000, "{}").unwrap();
    plugin.initialize(48_000).unwrap();
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
    ] {
        let specs = crate::wrapper::get_param_specs(name);
        let bridge = plugins_bridge::ParamBridge::new(specs);
        let mut infos: Vec<_> = (0..bridge.count()).filter_map(|i| bridge.info(i)).collect();
        let channels = crate::wrapper::plugin_constructor_channels(name);
        let config = crate::wrapper::default_plugin_config(name);
        let mut plugin = match plugins_bridge::create_plugin(name, channels, 48_000, &config) {
            Ok(plugin) => plugin,
            Err(error) => {
                failures.push(format!("{name} construction: {error}"));
                continue;
            }
        };
        if let Err(error) = plugin.initialize(48_000) {
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
        let params = DynamicParams::from_infos(&infos);
        plugin = match super::configuration::create_plugin(name, 48_000, &params) {
            Ok(plugin) => plugin,
            Err(error) => {
                failures.push(format!("{name} restored construction: {error}"));
                continue;
            }
        };
        if let Err(error) = plugin.initialize(48_000) {
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
            let result = super::configuration::create_plugin(name, 48_000, &params).and_then(
                |mut plugin| {
                    let before: Vec<_> = plugin
                        .parameters()
                        .into_iter()
                        .map(|parameter| {
                            let value = plugin.get_parameter(&parameter.id);
                            (parameter.id, value)
                        })
                        .collect();
                    plugin.initialize(48_000)?;
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
                    | ("Upmixer", "binaural_preview")
                    | ("AmbisonicsDecoder", "order" | "target_layout")
                    | ("Binaural", "input_channels")
                    | ("Beamformer", "num_mics")
                    | ("BandMerge", "bands")
                    | (
                        "Compressor",
                        "sidechain_hpf_hz"
                            | "sidechain_hpf_order"
                            | "detection_mode"
                            | "program_dependent_release"
                            | "sidechain_external"
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
