//! Crossfeed parameter registration contract.
//!
//! Every PARAMS entry must be discoverable through the live DSP descriptors
//! with matching metadata, and every live descriptor must come from PARAMS.
//! This guards the index-17 `head_yaw_deg` registration: the control used to
//! be appended outside PARAMS, which hid it from engine, native, and FFI
//! consumers while remaining reachable through the raw DSP API.

use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_plugin_crossfeed::params::PARAMS;
use sotf_plugin_crossfeed::{CrossfeedPlugin, CrossfeedPluginParams};

fn default_plugin() -> CrossfeedPlugin {
    CrossfeedPlugin::new(CrossfeedPluginParams::default()).unwrap()
}

#[test]
fn descriptor_ids_match_params_exactly_in_order() {
    let plugin = default_plugin();
    let descriptors = plugin.parameters();
    let spec_keys: Vec<&str> = PARAMS.iter().map(|spec| spec.engine_key).collect();
    let descriptor_ids: Vec<&str> = descriptors.iter().map(|param| param.id.as_str()).collect();
    assert_eq!(
        descriptor_ids, spec_keys,
        "live descriptors must be exactly the PARAMS keys in PARAMS order"
    );
    assert!(
        descriptor_ids.contains(&"head_yaw_deg"),
        "head_yaw_deg must be a registered descriptor"
    );
}

#[test]
fn descriptor_metadata_matches_spec() {
    let plugin = default_plugin();
    let descriptors = plugin.parameters();
    assert_eq!(descriptors.len(), PARAMS.len());
    for (spec, descriptor) in PARAMS.iter().zip(descriptors.iter()) {
        assert_eq!(
            descriptor.update_mode, spec.update_mode,
            "update mode mismatch for '{}'",
            spec.engine_key
        );
        let expected_default = spec.default_f64();
        let actual_default = match descriptor.default_value {
            ParameterValue::Float(v) => v as f64,
            ParameterValue::Int(v) => v as f64,
            ParameterValue::Bool(v) => f64::from(u8::from(v)),
            ParameterValue::String(_) => continue,
        };
        assert!(
            (actual_default - expected_default).abs() < 1e-6,
            "default mismatch for '{}': descriptor {actual_default}, spec {expected_default}",
            spec.engine_key
        );
    }
}

#[test]
fn head_yaw_descriptor_preserves_legacy_shape() {
    let plugin = default_plugin();
    let yaw = plugin
        .parameters()
        .into_iter()
        .find(|param| param.id.as_str() == "head_yaw_deg")
        .expect("head_yaw_deg descriptor must exist");
    assert_eq!(yaw.name, "Head Yaw");
    assert_eq!(yaw.group, "Head Tracking");
    assert_eq!(yaw.min_value, Some(ParameterValue::Float(-90.0)));
    assert_eq!(yaw.max_value, Some(ParameterValue::Float(90.0)));
    assert_eq!(yaw.default_value, ParameterValue::Float(0.0));
    assert_eq!(yaw.update_mode, UpdateMode::Realtime);
}

#[test]
fn head_yaw_roundtrip_clamp_and_reject_contract() {
    let mut plugin = default_plugin();
    plugin.initialize(48_000.0).unwrap();
    let id = ParameterId::from("head_yaw_deg");

    plugin
        .set_parameter(id.clone(), ParameterValue::Float(30.0))
        .unwrap();
    assert_eq!(plugin.get_parameter(&id), Some(ParameterValue::Float(30.0)));

    // Out-of-range values clamp; this is the accepted contract, not a reject.
    plugin
        .set_parameter(id.clone(), ParameterValue::Float(120.0))
        .unwrap();
    assert_eq!(plugin.get_parameter(&id), Some(ParameterValue::Float(90.0)));
    plugin
        .set_parameter(id.clone(), ParameterValue::Float(-120.0))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&id),
        Some(ParameterValue::Float(-90.0))
    );

    // Non-finite values are rejected on the realtime path.
    for hostile in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(
            plugin
                .set_parameter(id.clone(), ParameterValue::Float(hostile))
                .is_err(),
            "hostile yaw {hostile} must be rejected"
        );
    }

    // Wrong-typed values are rejected on the realtime path, as before.
    assert!(
        plugin
            .set_parameter(id.clone(), ParameterValue::Int(10))
            .is_err(),
        "integer yaw must be rejected by the realtime setter"
    );
    assert_eq!(
        plugin.get_parameter(&id),
        Some(ParameterValue::Float(-90.0)),
        "rejected updates must leave the accepted value untouched"
    );
}

#[test]
fn batch_path_rejects_integer_yaw_and_preserves_state_and_audio() {
    use sotf_host::parametric_plugin::ParameterSet;
    use sotf_host::plugin::ProcessContext;

    let mut plugin = default_plugin();
    plugin.initialize(48_000.0).unwrap();
    let mut twin = default_plugin();
    twin.initialize(48_000.0).unwrap();
    // Other float controls keep bridge Int coercion; the typed guard is
    // yaw-specific and must not extend to them.
    for target in [&mut plugin, &mut twin] {
        let mut legal = ParameterSet::new();
        legal.insert(ParameterId::from("itd_delay_ms"), ParameterValue::Int(1));
        target.apply_values(legal).unwrap();
    }
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("itd_delay_ms")),
        Some(ParameterValue::Float(1.0))
    );

    let before = plugin.current_values();
    let mut hostile = ParameterSet::new();
    hostile.insert(ParameterId::from("head_yaw_deg"), ParameterValue::Int(45));
    assert!(
        plugin.apply_values(hostile).is_err(),
        "integer yaw must be rejected by the batch path"
    );
    assert_eq!(
        plugin.current_values(),
        before,
        "rejected yaw update must leave accepted settings untouched"
    );
    assert_eq!(
        plugin.current_values(),
        twin.current_values(),
        "rejected yaw update must not diverge from the untouched twin"
    );

    // Populate filter/delay history identically, then reject, then prove
    // the populated continuation renders identically after the rejection.
    let input: Vec<f32> = (0..512)
        .flat_map(|n: u32| {
            let tone =
                (0.3 * (2.0 * std::f64::consts::PI * 431.0 * f64::from(n) / 48_000.0).cos()) as f32;
            [tone, -tone]
        })
        .collect();
    let render_all = |target: &mut CrossfeedPlugin, signal: &[f32]| {
        let mut output = signal.to_vec();
        for chunk in output.chunks_mut(256 * 2) {
            let frames = chunk.len() / 2;
            target
                .process_in_place(chunk, &ProcessContext::new(48_000, frames))
                .unwrap();
        }
        output
    };
    let _ = render_all(&mut plugin, &input);
    let _ = render_all(&mut twin, &input);

    let mut hostile_after = ParameterSet::new();
    hostile_after.insert(ParameterId::from("head_yaw_deg"), ParameterValue::Int(-30));
    assert!(
        plugin.apply_values(hostile_after).is_err(),
        "integer yaw must be rejected on populated state too"
    );
    let output = render_all(&mut plugin, &input);
    let twin_output = render_all(&mut twin, &input);
    assert_eq!(
        output, twin_output,
        "populated audio after rejected yaw update must match the untouched twin"
    );
}

#[test]
fn preset_action_resets_yaw_to_preset_default() {
    // DSP preset application rebuilds the complete configuration from the
    // preset, including yaw 0.0. Pin this: engine/sibling preset selection
    // preserves yaw instead, and the two layers must not drift silently.
    let mut plugin = default_plugin();
    plugin.initialize(48_000.0).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("head_yaw_deg"),
            ParameterValue::Float(45.0),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("preset"), ParameterValue::Int(2))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("head_yaw_deg")),
        Some(ParameterValue::Float(0.0)),
        "DSP-internal preset action must reset yaw to the preset default"
    );
}
