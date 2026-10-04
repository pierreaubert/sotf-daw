//! Audio-thread admission for EQ native state restores.
//!
//! The vendor `deserialize_object` gate itself is covered by dependency-free
//! tests in `crates/3rdparties/nih-plug/src/wrapper/state.rs`. These tests
//! cover this lane's side: the EQ admission hook refuses every restore shape
//! (legacy migration and current-version validation both allocate), the
//! generated wrappers report EQ refusal with unrelated-plugin compatibility,
//! and a faithful mirror of the vendor restore sequence proves refusal leaves
//! a populated instance untouched while control-thread retry migrates.

// Rust guideline compliant 2026-02-21
use nih_plug::prelude::Plugin as NihPlugin;
use nih_plug::prelude::{Param, Params};
use nih_plug::wrapper::state::{ParamValue, PluginState};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};
use sotf_host::parameters::ParameterValue;

use super::{
    DynamicParams, EQ_NATIVE_STATE_FIELD, EqPairRoute, ParamKind, eq_state_restore_allows_audio_thread,
    native_eq_pair_route_param_infos,
};

crate::sotf_nih_plugin!(
    AdmissionEqProbe,
    plugin_type: "EQ",
    name: "EQ Admission Probe",
    clap_id: "org.sotf.eq-admission-probe",
    vst3_class_id: *b"SotfAdmEqT000001",
    channels: 2
);

crate::sotf_nih_plugin!(
    AdmissionGainProbe,
    plugin_type: "Gain",
    name: "Gain Admission Probe",
    clap_id: "org.sotf.gain-admission-probe",
    vst3_class_id: *b"SotfAdmGnT000001",
    channels: 2
);

fn eq_infos() -> Vec<BridgedParamInfo> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("EQ"));
    let mut infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();
    let plugin = plugins_bridge::create_plugin(
        "EQ",
        crate::wrapper::plugin_constructor_channels("EQ"),
        48_000.0,
        &crate::wrapper::default_plugin_config("EQ"),
    )
    .expect("construct native EQ to inspect its dynamic parameter schema");
    for parameter in plugin.parameters() {
        if infos.iter().any(|info| info.id == parameter.id.as_str()) {
            continue;
        }
        if let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter) {
            infos.push(info);
        }
    }
    infos.extend(native_eq_pair_route_param_infos());
    infos
}

fn eq_params() -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("EQ", &eq_infos())
}

fn current_state(params: &DynamicParams) -> PluginState {
    let mut values = params
        .sync_entries
        .iter()
        .map(|entry| {
            let value = match entry.kind {
                ParamKind::Float => {
                    ParamValue::F32(params.float_params[entry.index].unmodulated_plain_value())
                }
                ParamKind::Int => {
                    ParamValue::I32(params.int_params[entry.index].unmodulated_plain_value())
                }
                ParamKind::Bool => {
                    ParamValue::Bool(params.bool_params[entry.index].unmodulated_plain_value())
                }
            };
            (entry.id.as_str().to_owned(), value)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    values.extend(params.serialize_parameter_overrides());
    PluginState {
        version: "synthetic-test-state".to_owned(),
        params: values,
        fields: params.serialize_fields(),
    }
}

/// A synthetic legacy state: no versioned field, no placement or pair
/// controls, raw band orders. This is constructed test input, not a captured
/// historical host preset.
fn legacy_state(params: &DynamicParams) -> PluginState {
    let mut state = current_state(params);
    state.fields.clear();
    state
        .params
        .retain(|id, _| !id.starts_with("filter_") && !id.starts_with("stereo_pair"));
    for band in 0..20 {
        state
            .params
            .insert(format!("band_{band}_order"), ParamValue::I32(2));
    }
    state
        .params
        .insert("band_0_gain".into(), ParamValue::F32(7.0));
    state
}

fn set_int(params: &DynamicParams, id: &str, value: i32) {
    let entry = params
        .param_map
        .get(id)
        .unwrap_or_else(|| panic!("missing {id}"));
    assert!(matches!(entry.kind, ParamKind::Int), "{id}");
    params.int_params[entry.index].set_plain_value_for_initialization(value);
}

fn set_float(params: &DynamicParams, id: &str, value: f32) {
    let entry = params
        .param_map
        .get(id)
        .unwrap_or_else(|| panic!("missing {id}"));
    assert!(matches!(entry.kind, ParamKind::Float), "{id}");
    params.float_params[entry.index].set_plain_value_for_initialization(value);
}

fn param_snapshot(params: &DynamicParams) -> Vec<(String, Option<ParameterValue>)> {
    params
        .sync_entries
        .iter()
        .map(|entry| {
            (
                entry.id.as_str().to_owned(),
                params.value(entry.id.as_str()),
            )
        })
        .collect()
}

fn route_snapshot(params: &DynamicParams) -> (Option<EqPairRoute>, Option<EqPairRoute>, bool) {
    let state = params
        .eq_pair_route_state
        .as_ref()
        .expect("EQ route state exists")
        .lock()
        .expect("route state is available");
    (
        state.committed.clone(),
        state.pending_restore.clone(),
        state.invalid_restore,
    )
}

/// Mirror of the vendor `deserialize_object` sequencing for the hooks this
/// lane owns: admission runs before migration, validation runs before any
/// live parameter or route mutation, and field restore runs last. The
/// vendor's parameter-write loop is vendor-owned and covered by the vendor
/// admission tests; this mirror stops at field restore.
fn restore_like_wrapper<P: NihPlugin>(
    params: &DynamicParams,
    state: &mut PluginState,
    is_audio_thread: bool,
) -> bool {
    if is_audio_thread && !P::state_restore_allows_audio_thread(state) {
        return false;
    }
    P::filter_state(state);
    if !params.validate_state(state, false, is_audio_thread, Some(48_000.0)) {
        return false;
    }
    params.deserialize_fields(&state.fields);
    true
}

#[test]
fn eq_hook_refuses_every_restore_shape_without_mutation() {
    let params = eq_params();
    let current = current_state(&params);
    assert!(params.validate_state(&current, false, false, Some(48_000.0)));
    let legacy = legacy_state(&params);
    let mut malformed = current_state(&params);
    malformed.params.remove("filter_7_placement");

    for (name, state) in [
        ("legacy", legacy),
        ("current", current),
        ("malformed", malformed),
    ] {
        let before = serde_json::to_value(&state).expect("state serializes");
        assert!(
            !eq_state_restore_allows_audio_thread(&state),
            "{name} EQ state must not restore on the audio thread"
        );
        // The admission check itself must not migrate, annotate, or lock.
        assert_eq!(serde_json::to_value(&state).expect("state serializes"), before);
    }
}

#[test]
fn generated_wrappers_report_eq_refusal_and_unrelated_compat() {
    let params = eq_params();
    let legacy = legacy_state(&params);
    let current = current_state(&params);
    assert!(
        !AdmissionEqProbe::state_restore_allows_audio_thread(&legacy),
        "generated EQ wrapper must refuse legacy audio-thread restores"
    );
    assert!(
        !AdmissionEqProbe::state_restore_allows_audio_thread(&current),
        "generated EQ wrapper must refuse current audio-thread restores"
    );
    assert!(
        AdmissionGainProbe::state_restore_allows_audio_thread(&legacy),
        "unrelated plugins keep the compatible default"
    );
    assert!(
        AdmissionGainProbe::state_restore_allows_audio_thread(&current),
        "unrelated plugins keep the compatible default"
    );
}

#[test]
fn audio_thread_refusal_leaves_populated_instance_untouched() {
    let params = eq_params();
    // Populate the instance: explicit placements plus a committed route.
    set_int(&params, "filter_0_placement", 2);
    set_int(&params, "filter_1_placement", 4);
    set_float(&params, "band_0_gain", 7.5);
    params
        .complete_eq_pair_route(
            EqPairRoute {
                enabled: true,
                pairs: vec![[0, 1]],
            },
            true,
        )
        .expect("explicit route commits");
    let values_before = param_snapshot(&params);
    let route_before = route_snapshot(&params);

    for (name, mut state) in [
        ("legacy", legacy_state(&params)),
        ("current", current_state(&params)),
    ] {
        let state_before = serde_json::to_value(&state).expect("state serializes");
        assert!(
            !restore_like_wrapper::<AdmissionEqProbe>(&params, &mut state, true),
            "{name} audio-thread restore must be refused"
        );
        // No migration annotation, no parameter write, no route callback.
        assert_eq!(
            serde_json::to_value(&state).expect("state serializes"),
            state_before,
            "{name} refusal must not mutate the state object"
        );
        assert_eq!(param_snapshot(&params), values_before, "{name} params moved");
        assert_eq!(route_snapshot(&params), route_before, "{name} route moved");
    }
}

#[test]
fn control_thread_retry_migrates_legacy_and_stages_pending_route() {
    let params = eq_params();
    set_int(&params, "filter_0_placement", 2);
    params
        .complete_eq_pair_route(
            EqPairRoute {
                enabled: true,
                pairs: vec![[0, 1]],
            },
            true,
        )
        .expect("explicit route commits");
    let committed_before = route_snapshot(&params).0;

    let mut state = legacy_state(&params);
    assert!(
        !restore_like_wrapper::<AdmissionEqProbe>(&params, &mut state, true),
        "audio-thread attempt is refused first"
    );
    assert!(
        restore_like_wrapper::<AdmissionEqProbe>(&params, &mut state, false),
        "control-thread retry migrates the legacy state"
    );
    // Migration completed the schema: implicit placements plus the disabled
    // pair route and its versioned field.
    assert!(
        state.fields.contains_key(EQ_NATIVE_STATE_FIELD),
        "retry stamps the versioned field"
    );
    for band in 0..20 {
        assert!(
            matches!(
                state.params.get(&format!("filter_{band}_placement")),
                Some(ParamValue::I32(0))
            ),
            "legacy placement {band} migrates to implicit"
        );
    }
    // Field restore stages the disabled pending route; the committed route
    // is untouched until Apply or activation consumes the restore.
    let (committed, pending, invalid) = route_snapshot(&params);
    assert_eq!(committed, committed_before);
    assert_eq!(
        pending,
        Some(EqPairRoute {
            enabled: false,
            pairs: Vec::new(),
        })
    );
    assert!(!invalid);
}

#[test]
fn malformed_restores_reject_transactionally_on_both_threads() {
    let params = eq_params();
    set_int(&params, "filter_0_placement", 3);
    params
        .complete_eq_pair_route(
            EqPairRoute {
                enabled: true,
                pairs: vec![[2, 3]],
            },
            true,
        )
        .expect("explicit route commits");
    let values_before = param_snapshot(&params);
    let route_before = route_snapshot(&params);

    // Malformed legacy: raw order 1 has no 2/4/6/8 mapping and must not be
    // reinterpreted as a current choice index.
    let mut bad_legacy = legacy_state(&params);
    bad_legacy
        .params
        .insert("band_7_order".into(), ParamValue::I32(1));
    // Malformed current: a required placement entry is missing.
    let mut bad_current = current_state(&params);
    bad_current.params.remove("filter_7_placement");

    for (name, state) in [("bad legacy", bad_legacy), ("bad current", bad_current)] {
        for is_audio_thread in [true, false] {
            let mut attempt = state.clone();
            assert!(
                !restore_like_wrapper::<AdmissionEqProbe>(&params, &mut attempt, is_audio_thread),
                "{name} must reject on is_audio_thread={is_audio_thread}"
            );
            assert_eq!(
                param_snapshot(&params),
                values_before,
                "{name} params moved on is_audio_thread={is_audio_thread}"
            );
            assert_eq!(
                route_snapshot(&params),
                route_before,
                "{name} route moved on is_audio_thread={is_audio_thread}"
            );
        }
    }

    // The control-thread malformed attempts ran migration before validation
    // rejected them, so their state objects carry the annotation while the
    // live instance stayed untouched.
    let mut annotated = legacy_state(&params);
    annotated
        .params
        .insert("band_7_order".into(), ParamValue::I32(1));
    assert!(!restore_like_wrapper::<AdmissionEqProbe>(
        &params,
        &mut annotated,
        false
    ));
    assert!(
        annotated
            .fields
            .keys()
            .any(|field| field.starts_with(EQ_NATIVE_STATE_FIELD)
                && field != EQ_NATIVE_STATE_FIELD),
        "rejected legacy order leaves a migration-error annotation"
    );
}
