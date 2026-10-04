//! AUD142 checks for the fixed native Crossover controls and typed adapter.

use crate::native_crossover;
use crate::params::DynamicParams;
use crate::wrapper;
use nih_plug::prelude::Params;
use nih_plug::wrapper::state::{ParamValue as NativeValue, PluginState};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::ProcessContext;
use std::collections::BTreeMap;

fn infos_with(overrides: &[(&str, f64)]) -> Vec<BridgedParamInfo> {
    let bridge = ParamBridge::new(wrapper::get_param_specs("Crossover"));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    for (id, value) in overrides {
        let info = infos
            .iter_mut()
            .find(|info| info.id == *id)
            .unwrap_or_else(|| panic!("native Crossover schema is missing {id}"));
        info.default_value = *value;
    }
    infos
}

fn params_with(overrides: &[(&str, f64)]) -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("Crossover", &infos_with(overrides))
}

#[test]
fn native_schema_keeps_frequency_first_and_appends_fixed_ids() {
    let specs = wrapper::get_param_specs("Crossover");
    let ids: Vec<_> = specs.iter().map(|spec| spec.engine_key).collect();
    assert_eq!(
        &ids[..12],
        [
            "frequency",
            "family",
            "mode",
            "fir_taps",
            "topology",
            "band_count",
            "frequency_2",
            "frequency_3",
            "channel_frequency_0",
            "channel_mode_0",
            "channel_frequency_1",
            "channel_mode_1",
        ]
    );
    assert_eq!(ids.len(), 40, "16 channels add two stable controls each");
    for channel in 0..16 {
        assert_eq!(ids[8 + channel * 2], format!("channel_frequency_{channel}"));
        assert_eq!(ids[9 + channel * 2], format!("channel_mode_{channel}"));
        assert!(native_crossover::is_structural(ids[8 + channel * 2]));
        assert!(native_crossover::is_structural(ids[9 + channel * 2]));
    }

    let frequency = &specs[0];
    assert_eq!(frequency.name, "Frequency");
    assert_eq!(frequency.unit, "Hz");
    assert_eq!(frequency.default_f64(), 1000.0);
    assert_eq!(frequency.min_f64(), 20.0);
    assert_eq!(frequency.max_f64(), 20000.0);
    assert_eq!(frequency.update_mode, UpdateMode::Realtime);

    let infos = infos_with(&[]);
    let params = DynamicParams::from_infos_for_plugin("Crossover", &infos);
    let map = params.param_map();
    assert_eq!(
        map.iter().map(|entry| entry.0.as_str()).collect::<Vec<_>>(),
        ids
    );
    let (_, frequency, _) = map.iter().find(|entry| entry.0 == "frequency").unwrap();
    for normalized in [0.0, 0.25, 0.5, 0.75, 1.0] {
        // SAFETY: the parameter pointer is owned by `params`, which remains
        // alive for every preview in this test.
        let actual = unsafe { frequency.preview_plain(normalized) };
        let expected = 20.0_f64 + f64::from(normalized) * (20_000.0 - 20.0);
        assert!(
            (f64::from(actual) - expected).abs() <= 0.01,
            "normalized {normalized}: {actual} != {expected}"
        );
    }

    assert_eq!(
        native_crossover::choice_labels("family").unwrap(),
        sotf_plugins::param_specs::crossover::CROSSOVER_TYPES
    );
    assert!(native_crossover::is_structural("family"));
    assert!(native_crossover::is_structural("channel_frequency_1"));
    assert!(native_crossover::is_structural("channel_mode_15"));
    assert!(!native_crossover::is_structural("frequency_2"));
}

#[test]
fn all_native_family_and_output_choices_construct_the_selected_route() {
    let families = sotf_plugins::param_specs::crossover::CROSSOVER_TYPES;
    for (family_index, family) in families.iter().enumerate() {
        for (mode_index, (mode, expected_width)) in [("lowpass", 2), ("highpass", 2), ("both", 4)]
            .into_iter()
            .enumerate()
        {
            let params =
                params_with(&[("family", family_index as f64), ("mode", mode_index as f64)]);
            let plugin = crate::params::configuration::create_plugin("Crossover", 48_000, &params)
                .unwrap_or_else(|error| panic!("{family} {mode}: {error}"));
            assert_eq!(plugin.input_channels(), 2, "{family} {mode}");
            assert_eq!(plugin.output_channels(), expected_width, "{family} {mode}");
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("type")),
                Some(ParameterValue::String((*family).to_string())),
                "runtime canonical family for {family}"
            );
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("mode")),
                Some(ParameterValue::String(mode.to_string())),
                "runtime lowercase mode for {family}"
            );
        }
    }
}

#[test]
fn native_band_count_and_per_channel_adapter_retain_dormant_values() {
    for (band_count, output_width) in [(0.0, 4), (1.0, 6), (2.0, 8)] {
        let params = params_with(&[
            ("mode", 2.0),
            ("band_count", band_count),
            ("frequency_2", 2500.0),
            ("frequency_3", 7000.0),
        ]);
        let plugin =
            crate::params::configuration::create_plugin("Crossover", 48_000, &params).unwrap();
        assert_eq!(plugin.output_channels(), output_width);
        assert_eq!(
            params.value("channel_frequency_0"),
            Some(ParameterValue::Float(1000.0)),
            "native state keeps dormant per-channel values for Bands"
        );
        assert!(
            plugin
                .get_parameter(&ParameterId::from("channel_frequency_0"))
                .is_none()
        );
        if band_count >= 1.0 {
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("frequency_2")),
                Some(ParameterValue::Float(2500.0))
            );
        }
        if band_count >= 2.0 {
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("frequency_3")),
                Some(ParameterValue::Float(7000.0))
            );
        }
    }

    let params = params_with(&[
        ("family", 2.0),
        ("mode", 2.0), // Dormant global Both must not replace per-channel modes.
        ("topology", 1.0),
        ("band_count", 2.0),
        ("frequency_2", 2500.0),
        ("frequency_3", 7000.0),
        ("channel_frequency_0", 500.0),
        ("channel_mode_0", 0.0),
        ("channel_frequency_1", 5000.0),
        ("channel_mode_1", 1.0),
    ]);
    let plugin = crate::params::configuration::create_plugin("Crossover", 48_000, &params)
        .expect("complete explicit per-channel native values form an active route");
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (2, 2));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("channel_frequency_0")),
        Some(ParameterValue::Float(500.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("channel_frequency_1")),
        Some(ParameterValue::Float(5000.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("channel_mode_0")),
        Some(ParameterValue::String("lowpass".to_string()))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("channel_mode_1")),
        Some(ParameterValue::String("highpass".to_string()))
    );
    assert_eq!(
        params.value("mode"),
        Some(ParameterValue::Int(2)),
        "native state keeps dormant global output choice for Per-channel"
    );
    let active_ids: Vec<_> = plugin
        .parameters()
        .iter()
        .map(|parameter| parameter.id.as_str().to_string())
        .collect();
    assert!(
        !active_ids
            .iter()
            .any(|id| id == "frequency" || id == "mode")
    );
    assert!(active_ids.iter().any(|id| id == "channel_frequency_0"));
    // The historical scalar getter aliases the first active per-channel
    // cutoff, while the parameter list omits the inactive global controls.
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("frequency")),
        Some(ParameterValue::Float(500.0)),
        "legacy frequency getter maps to the first active per-channel cutoff"
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::String("lowpass".to_string())),
        "runtime global mode is a dormant fallback in Per-channel topology"
    );
}

#[test]
fn cold_iir_cutoff_sync_and_process_do_not_allocate() {
    const FRAMES: usize = 257;
    let old_params = params_with(&[
        ("family", 2.0),
        ("mode", 2.0),
        ("band_count", 2.0),
        ("frequency", 300.0),
        ("frequency_2", 2000.0),
        ("frequency_3", 6000.0),
    ]);
    let mut plugin =
        crate::params::configuration::create_plugin("Crossover", 48_000, &old_params).unwrap();
    plugin.initialize(48_000.0).unwrap();

    // Build changed native values without touching the prepared instance.
    // This exercises the first sync after activation, before any warm callback.
    let changed_params = params_with(&[
        ("family", 2.0),
        ("mode", 2.0),
        ("band_count", 2.0),
        ("frequency", 400.0),
        ("frequency_2", 2400.0),
        ("frequency_3", 6500.0),
    ]);
    let input: Vec<f32> = (0..FRAMES * 2)
        .map(|index| ((index as f32 * 0.071).sin() + (index as f32 * 0.019).cos()) * 0.2)
        .collect();
    let mut output = vec![0.0; FRAMES * 8];
    let context = ProcessContext::new(48_000, FRAMES);

    let frames_written = assert_no_alloc::assert_no_alloc(|| {
        changed_params.sync_to_plugin(plugin.as_mut())?;
        plugin.process(&input, &mut output, &context)
    })
    .unwrap();

    assert_eq!(frames_written, FRAMES);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-5));
    for (id, expected) in [
        ("frequency", 400.0),
        ("frequency_2", 2400.0),
        ("frequency_3", 6500.0),
    ] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(id)),
            Some(ParameterValue::Float(expected))
        );
    }
}

#[test]
fn dormant_global_cutoffs_do_not_reject_prepared_per_channel_audio() {
    const FRAMES: usize = 257;
    let structural = [
        ("family", 2.0),
        ("mode", 2.0), // Retained dormant global Both value.
        ("topology", 1.0),
        ("band_count", 2.0),
        ("frequency_2", 2500.0),
        ("frequency_3", 7000.0),
        ("channel_frequency_0", 500.0),
        ("channel_mode_0", 0.0),
        ("channel_frequency_1", 5000.0),
        ("channel_mode_1", 1.0),
    ];
    let mut old_overrides = structural.to_vec();
    old_overrides.push(("frequency", 1000.0));
    let old_params = params_with(&old_overrides);
    let mut plugin =
        crate::params::configuration::create_plugin("Crossover", 48_000, &old_params).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let mut twin =
        crate::params::configuration::create_plugin("Crossover", 48_000, &old_params).unwrap();
    twin.initialize(48_000.0).unwrap();

    let mut new_overrides = structural.to_vec();
    new_overrides.push(("frequency", 1500.0));
    let changed_params = params_with(&new_overrides);
    let input: Vec<f32> = (0..FRAMES * 2)
        .map(|index| ((index as f32 * 0.043).sin() + (index as f32 * 0.011).cos()) * 0.2)
        .collect();
    let mut output = vec![0.0; FRAMES * 2];
    let mut twin_output = vec![0.0; FRAMES * 2];
    let context = ProcessContext::new(48_000, FRAMES);

    let frames_written = assert_no_alloc::assert_no_alloc(|| {
        changed_params.sync_to_plugin(plugin.as_mut())?;
        plugin.process(&input, &mut output, &context)
    })
    .unwrap();

    assert_eq!(frames_written, FRAMES);
    let twin_frames = assert_no_alloc::assert_no_alloc(|| {
        old_params.sync_to_plugin(twin.as_mut())?;
        twin.process(&input, &mut twin_output, &context)
    })
    .unwrap();
    assert_eq!(twin_frames, FRAMES);
    assert_eq!(
        output, twin_output,
        "dormant edits must preserve route audio"
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-5));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("frequency")),
        Some(ParameterValue::Float(500.0)),
        "pending native topology values must not be written to the prepared route"
    );
}

#[test]
fn frequency_only_native_state_migrates_over_explicit_legacy_defaults() {
    let mut state = PluginState {
        version: "legacy".to_string(),
        params: BTreeMap::from([("frequency".to_string(), NativeValue::F32(725.0))]),
        fields: BTreeMap::new(),
    };
    wrapper::migrate_crossover_state(&mut state);

    assert!(matches!(
        state.params.get("frequency"),
        Some(NativeValue::F32(value)) if (*value - 725.0).abs() <= f32::EPSILON
    ));
    for (id, expected) in [
        ("family", NativeValue::I32(0)),
        ("mode", NativeValue::I32(0)),
        ("fir_taps", NativeValue::I32(255)),
        ("topology", NativeValue::I32(0)),
        ("band_count", NativeValue::I32(0)),
        ("frequency_2", NativeValue::F32(3000.0)),
        ("frequency_3", NativeValue::F32(8000.0)),
        ("channel_frequency_0", NativeValue::F32(1000.0)),
        ("channel_mode_0", NativeValue::I32(0)),
        ("channel_frequency_1", NativeValue::F32(3000.0)),
        ("channel_mode_1", NativeValue::I32(1)),
    ] {
        match expected {
            NativeValue::F32(expected) => assert!(
                matches!(
                    state.params.get(id),
                    Some(NativeValue::F32(actual)) if (*actual - expected).abs() <= f32::EPSILON
                ),
                "migrated {id}"
            ),
            NativeValue::I32(expected) => assert!(
                matches!(
                    state.params.get(id),
                    Some(NativeValue::I32(actual)) if *actual == expected
                ),
                "migrated {id}"
            ),
            _ => unreachable!("this migration fixture uses only scalar float and integer values"),
        }
    }
    assert_eq!(
        state.fields[crate::params::CROSSOVER_STATE_RESTORE_MARKER],
        "1"
    );
}
