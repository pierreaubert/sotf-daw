//! link_mode toolbar wire-form tests.
//!
//! Bounds are fixed here before any candidate runs: construction accepts
//! the documented integer indices (0/1), exact labels
//! ("Independent"/"Linked"), and integral wire floats (1.0); missing
//! fields keep the legacy default 0; unknown labels, out-of-range or
//! fractional numbers, nulls, and bools reject transactionally with the
//! factory parse contract. Named i32 controls and defaults are unchanged.

// Rust guideline compliant 2026-10-21
use serde_json::json;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

fn from_wire(value: serde_json::Value) -> Result<HissReducerPluginParams, String> {
    // Same construction entry the factory uses: whole-struct parse first,
    // so any wire-form error fails before a plugin exists.
    serde_json::from_value(value)
        .map_err(|error| format!("Failed to parse hiss reducer params: {error}"))
}

fn base_wire() -> serde_json::Map<String, serde_json::Value> {
    json!({
        "enabled": true,
        "frequency_hz": 4000.0,
        "threshold_db": -30.0,
        "strength": 0.5,
    })
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn link_mode_wire_forms_accept_int_label_float_and_default() {
    // (wire value or omission, expected canonical index)
    let accepted: [(Option<serde_json::Value>, i32); 7] = [
        (Some(json!(0)), 0),
        (Some(json!(1)), 1),
        (Some(json!("Independent")), 0),
        (Some(json!("Linked")), 1),
        (Some(json!(0.0)), 0),
        (Some(json!(1.0)), 1),
        (None, 0),
    ];
    for (index, (wire, expected)) in accepted.iter().enumerate() {
        let mut object = base_wire();
        if let Some(value) = wire {
            object.insert("link_mode".to_string(), value.clone());
        }
        let params = from_wire(serde_json::Value::Object(object))
            .unwrap_or_else(|error| panic!("case {index} wire {wire:?} must construct: {error}"));
        assert_eq!(params.link_mode, *expected, "case {index} canonical value");
        let plugin = HissReducerPlugin::from_params(1, params);
        assert_eq!(plugin.link_mode(), *expected, "case {index} live value");
    }

    // Documented case-insensitive fallback from the registry helper.
    let mut folded = base_wire();
    folded.insert("link_mode".to_string(), json!("linked"));
    let params = from_wire(serde_json::Value::Object(folded)).unwrap();
    assert_eq!(params.link_mode, 1);

    // Legacy five-field default object (no link_mode key at all).
    let legacy = json!({
        "enabled": true,
        "frequency_hz": 4000.0,
        "threshold_db": -30.0,
        "strength": 0.5,
    });
    let params = from_wire(legacy).unwrap();
    assert_eq!(params.link_mode, 0);
    let plugin = HissReducerPlugin::from_params(2, params);
    assert_eq!(plugin.link_mode(), 0);

    // Current i32 named controls are unchanged by the wire-form fix.
    let mut plugin = HissReducerPlugin::new(1);
    plugin.initialize(48_000.0).unwrap();
    plugin
        .set_parameter(ParameterId::from("link_mode"), ParameterValue::Int(1))
        .unwrap();
    assert_eq!(plugin.link_mode(), 1);
    plugin
        .set_parameter(ParameterId::from("link_mode"), ParameterValue::Int(0))
        .unwrap();
    assert_eq!(plugin.link_mode(), 0);
}

#[test]
fn link_mode_wire_forms_reject_unknown_noncanonical() {
    let rejected = [
        ("unknown-label", json!("Linked2")),
        ("empty-label", json!("")),
        ("index-above-range", json!(2)),
        ("negative-index", json!(-1)),
        ("fractional", json!(0.5)),
        ("null", serde_json::Value::Null),
        ("bool", json!(true)),
    ];
    for (label, wire) in rejected {
        let mut object = base_wire();
        object.insert("link_mode".to_string(), wire.clone());
        let error = from_wire(serde_json::Value::Object(object)).unwrap_err();
        assert!(
            error.contains("link_mode") || error.contains("choice") || error.contains("expected"),
            "{label} wire {wire} must fail as invalid choice, got: {error}"
        );
    }

    // Transactional rejection: no partial plugin or state exists after a
    // failed construction, and a previously built plugin is untouched.
    let mut plugin = HissReducerPlugin::new(1);
    plugin.initialize(48_000.0).unwrap();
    assert_eq!(plugin.link_mode(), 0);
    let mut object = base_wire();
    object.insert("link_mode".to_string(), json!("Mono"));
    assert!(from_wire(serde_json::Value::Object(object)).is_err());
    assert_eq!(plugin.link_mode(), 0);
}
