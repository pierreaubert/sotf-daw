//! Convolution state restores without an IR publish scalar controls immediately.

// Rust guideline compliant 2026-02-21
use super::{DynamicParams, ParamKind};
use nih_plug::prelude::{Param, Params};
use nih_plug::wrapper::state::{ParamValue, PluginState};
use plugins_bridge::param_bridge::ParamBridge;
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::Plugin;
use std::collections::BTreeMap;

fn convolution_params() -> std::sync::Arc<DynamicParams> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("Convolution"));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    if infos.is_empty() {
        let plugin = plugins_bridge::create_plugin(
            "Convolution",
            crate::wrapper::plugin_constructor_channels("Convolution"),
            48_000.0,
            &crate::wrapper::default_plugin_config("Convolution"),
        )
        .expect("construct default Convolution parameter schema");
        infos.extend(
            plugin
                .parameters()
                .iter()
                .filter_map(crate::wrapper::bridged_info_from_parameter),
        );
    }
    for info in &mut infos {
        info.id = crate::wrapper::legacy_external_param_id("Convolution", &info.id).to_string();
    }
    DynamicParams::from_infos_for_plugin("Convolution", &infos)
}

fn current_state(params: &DynamicParams) -> PluginState {
    let values = params
        .sync_entries
        .iter()
        .map(|entry| {
            let value = match entry.kind {
                ParamKind::Float => {
                    ParamValue::F32(params.float_params[entry.index].unmodulated_plain_value())
                }
                ParamKind::Bool => {
                    ParamValue::Bool(params.bool_params[entry.index].unmodulated_plain_value())
                }
                ParamKind::Int => {
                    ParamValue::I32(params.int_params[entry.index].unmodulated_plain_value())
                }
            };
            (entry.id.as_str().to_owned(), value)
        })
        .collect::<BTreeMap<_, _>>();
    PluginState {
        version: "convolution-no-ir-test-state".to_owned(),
        params: values,
        fields: params.serialize_fields(),
    }
}

fn visible_float(params: &DynamicParams, id: &str) -> f32 {
    let entry = params.param_map.get(id).expect("host-visible Convolution ID");
    assert!(matches!(entry.kind, ParamKind::Float));
    params.float_params[entry.index].unmodulated_plain_value()
}

#[test]
fn dry_state_restore_publishes_mix_and_gain_without_pending_resource() {
    let params = convolution_params();
    let mut state = current_state(&params);
    assert_eq!(visible_float(&params, "mix"), 1.0);
    assert_eq!(visible_float(&params, "gain_db"), 0.0);
    state.params.insert("mix".to_owned(), ParamValue::F32(0.375));
    state
        .params
        .insert("gain_db".to_owned(), ParamValue::F32(-12.5));

    assert!(params.validate_state(&state, false, false, Some(48_000.0)));
    assert!(!params.defer_state_parameter_values());
    assert!(params.serialize_parameter_overrides().is_empty());

    // NIH applies these accepted values to the same host-visible FloatParams
    // when defer_state_parameter_values is false.
    for (id, value) in [("mix", 0.375), ("gain_db", -12.5)] {
        let entry = params.param_map.get(id).expect("Convolution scalar ID");
        params.float_params[entry.index].set_plain_value_for_initialization(value);
        assert_eq!(visible_float(&params, id), value);
        assert_eq!(
            params.initialization_value(id),
            Some(ParameterValue::Float(value))
        );
    }
    params.complete_convolution_state_restore(48_000.0);
    assert_eq!(visible_float(&params, "mix"), 0.375);
    assert_eq!(visible_float(&params, "gain_db"), -12.5);
}

#[test]
fn invalid_ir_restore_preserves_visible_scalars_and_dry_resource_state() {
    let params = convolution_params();
    let mut state = current_state(&params);
    let prior_fields = params.serialize_fields();
    state.params.insert("mix".to_owned(), ParamValue::F32(0.25));
    state
        .params
        .insert("gain_db".to_owned(), ParamValue::F32(-7.0));
    state.fields.insert(
        super::CONVOLUTION_IR_RESOURCE_FIELD.to_owned(),
        serde_json::json!({"version": 1, "path": "/not-a-real-convolution-ir.wav"}).to_string(),
    );

    assert!(!params.validate_state(&state, false, false, Some(48_000.0)));
    assert!(!params.defer_state_parameter_values());
    assert_eq!(visible_float(&params, "mix"), 1.0);
    assert_eq!(visible_float(&params, "gain_db"), 0.0);
    assert_eq!(params.serialize_fields(), prior_fields);
    assert!(params.serialize_parameter_overrides().is_empty());
}

#[test]
fn removing_committed_ir_keeps_parameter_restore_transactional() {
    let params = convolution_params();
    params
        .convolution_state
        .as_ref()
        .expect("Convolution resource state")
        .lock()
        .expect("Convolution state lock")
        .committed_ir_path = Some(std::path::PathBuf::from("/previous-ir.wav"));
    let mut state = current_state(&params);
    state.fields.insert(
        super::CONVOLUTION_IR_RESOURCE_FIELD.to_owned(),
        serde_json::json!({"version": 1, "path": ""}).to_string(),
    );
    state.params.insert("mix".to_owned(), ParamValue::F32(0.25));

    assert!(params.validate_state(&state, false, false, Some(48_000.0)));
    assert!(params.defer_state_parameter_values());
    assert_eq!(visible_float(&params, "mix"), 1.0);
    assert!(matches!(
        params.serialize_parameter_overrides().get("mix"),
        Some(ParamValue::F32(value)) if *value == 0.25
    ));
    assert_eq!(
        params
            .convolution_state
            .as_ref()
            .expect("Convolution resource state")
            .lock()
            .expect("Convolution state lock")
            .committed_ir_path
            .as_deref(),
        Some(std::path::Path::new("/previous-ir.wav"))
    );
}
