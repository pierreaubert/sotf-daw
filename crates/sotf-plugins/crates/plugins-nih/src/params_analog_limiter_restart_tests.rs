//! Analog limiter model restart policy: visibility, fingerprints, selection.
//!
//! Mirrors `params_de_esser_restart_tests.rs`: the structural model is a
//! visible manual restart control (never synced into running DSP), the
//! lookahead stays a legacy hidden structural guard, and all six models
//! construct readably and render through the NIH construction path.

use nih_plug::prelude::{Param, ParamFlags};
use plugins_bridge::param_bridge::BridgedParamInfo;
use sotf_host::plugin::ProcessContext;
use sotf_plugins::plugin_analog_common::MODEL_NAMES;

use super::DynamicParams;
use crate::params::configuration;

fn analog_limiter_infos() -> Vec<BridgedParamInfo> {
    // Specs provide stable integer Choice metadata for the String-typed
    // runtime model control. Merge runtime for any missing IDs.
    let bridge = plugins_bridge::param_bridge::ParamBridge::new(crate::wrapper::get_param_specs(
        "AnalogLimiter",
    ));
    let mut infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();
    let plugin = plugins_bridge::create_plugin(
        "AnalogLimiter",
        crate::wrapper::plugin_constructor_channels("AnalogLimiter"),
        48_000.0,
        &crate::wrapper::default_plugin_config("AnalogLimiter"),
    )
    .expect("create AnalogLimiter to inspect its complete parameter schema");
    for parameter in plugin.parameters() {
        if let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter)
            && !infos.iter().any(|existing| existing.id == info.id)
        {
            infos.push(info);
        }
    }
    infos
}

fn analog_limiter_params(infos: &[BridgedParamInfo]) -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("AnalogLimiter", infos)
}

#[test]
fn analog_model_is_a_visible_manual_restart_control() {
    let params = analog_limiter_params(&analog_limiter_infos());

    let entry = params.param_map.get("analog_model").expect("model entry");
    assert!(!entry.realtime, "model must not sync into the running DSP");
    assert!(entry.requires_restart, "model");
    assert!(
        matches!(entry.kind, super::ParamKind::Int),
        "model must keep integer Choice metadata"
    );
    let flags = params.int_params[entry.index].flags();
    assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "model");
    assert!(flags.contains(ParamFlags::REQUIRES_RESTART), "model");
    assert!(!flags.contains(ParamFlags::HIDDEN), "model");

    // All six choices display their canonical labels in the host.
    let display = &params.int_params[entry.index];
    for (index, label) in MODEL_NAMES.iter().enumerate() {
        let normalized = index as f32 / (MODEL_NAMES.len() - 1) as f32;
        assert_eq!(
            display.normalized_value_to_string(normalized, false),
            label.to_string(),
            "model index {index}"
        );
    }

    // Lookahead stays a legacy structural control: hidden and guarded,
    // not restartable.
    let lookahead = params.param_map.get("lookahead").expect("lookahead entry");
    assert!(!lookahead.realtime, "lookahead");
    assert!(!lookahead.requires_restart, "lookahead");

    // Realtime controls keep syncing with no restart involvement.
    let threshold = params.param_map.get("threshold").expect("threshold entry");
    assert!(
        threshold.realtime,
        "threshold must sync into the running DSP"
    );
    assert!(!threshold.requires_restart, "threshold");
}

#[test]
fn restartable_model_value_does_not_mask_lookahead_guard() {
    let baseline_infos = analog_limiter_infos();
    let baseline = analog_limiter_params(&baseline_infos);

    let mut changed_infos = analog_limiter_infos();
    changed_infos
        .iter_mut()
        .find(|info| info.id == "analog_model")
        .expect("model info")
        .default_value = 3.0;
    let changed = analog_limiter_params(&changed_infos);
    assert_ne!(
        baseline.structural_fingerprint(),
        changed.structural_fingerprint(),
        "model must change the construction fingerprint"
    );
    assert_eq!(
        baseline.non_restartable_structural_fingerprint(),
        changed.non_restartable_structural_fingerprint(),
        "model must not mask non-restartable guards"
    );

    let mut changed_lookahead_infos = analog_limiter_infos();
    changed_lookahead_infos
        .iter_mut()
        .find(|info| info.id == "lookahead")
        .expect("lookahead info")
        .default_value = 7.5;
    let changed_lookahead = analog_limiter_params(&changed_lookahead_infos);
    assert_ne!(
        baseline.non_restartable_structural_fingerprint(),
        changed_lookahead.non_restartable_structural_fingerprint(),
        "lookahead stays a non-restartable guarded value"
    );
}

#[test]
fn all_six_models_construct_readable_and_render() {
    for (index, label) in MODEL_NAMES.iter().enumerate() {
        let mut infos = analog_limiter_infos();
        infos
            .iter_mut()
            .find(|info| info.id == "analog_model")
            .expect("model info")
            .default_value = index as f64;
        let params = analog_limiter_params(&infos);
        let mut plugin = configuration::create_plugin("AnalogLimiter", 48_000.0, &params)
            .expect("construct with restored model");
        plugin.initialize(48_000.0).unwrap();
        assert_eq!(
            plugin.get_parameter(&sotf_host::parameters::ParameterId::from("analog_model")),
            Some(sotf_host::parameters::ParameterValue::String(
                label.to_string()
            )),
            "model index {index} must read back its label"
        );
        // Default threshold (-0.1 dB) with a hot-but-below-ceiling tone:
        // finite, nonzero, untouched.
        let mut input = vec![0.0f32; 1024 * 2];
        for frame in 0..1024 {
            let sample = 0.9 * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / 48_000.0).sin();
            input[frame * 2] = sample;
            input[frame * 2 + 1] = sample;
        }
        let mut output = vec![f32::NAN; input.len()];
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, 1024))
                .unwrap(),
            1024
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        let peak = output
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0f32, f32::max);
        assert!(
            (peak - 0.9).abs() < 1e-3,
            "model {label}: peak {peak} must pass transparently"
        );
    }
}
