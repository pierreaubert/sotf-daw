//! Checks synthetic legacy EQ states against migration and complete-schema admission.

// Rust guideline compliant 2026-02-21
use nih_plug::prelude::{Param, Params};
use nih_plug::wrapper::state::{ParamValue, PluginState};
use sotf_host::parameters::ParameterValue;

use super::super::{DynamicParams, ParamKind, migrate_eq_native_state};
use super::{
    EQ_NATIVE_STATE_FIELD, EqPairApplyAttempt, EqPairRoute, eq_params, serialized_route, set_draft,
    set_int,
};

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

#[test]
fn synthetic_legacy_missing_placements_migrates_to_complete_implicit_route() {
    let params = eq_params();
    let mut state = current_state(&params);
    // This is constructed test input, not a captured historical host preset.
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
        .insert("band_0_order".into(), ParamValue::I32(6));
    state
        .params
        .insert("band_1_order".into(), ParamValue::I32(8));
    state
        .params
        .insert("band_0_gain".into(), ParamValue::F32(7.0));
    assert_eq!(state.params.len(), 105);

    migrate_eq_native_state(&mut state);

    for band in 0..20 {
        assert!(
            matches!(
                state.params.get(&format!("filter_{band}_placement")),
                Some(ParamValue::I32(0))
            ),
            "legacy missing placement {band} must become implicit, not inherit current controls"
        );
    }
    assert!(matches!(state.params["band_0_order"], ParamValue::I32(2)));
    assert!(matches!(state.params["band_1_order"], ParamValue::I32(3)));
    assert!(matches!(state.params["band_2_order"], ParamValue::I32(0)));
    assert!(matches!(state.params["band_0_gain"], ParamValue::F32(7.0)));
    let route: serde_json::Value = serde_json::from_str(&state.fields[EQ_NATIVE_STATE_FIELD])
        .expect("migration creates valid route JSON");
    assert_eq!(
        route,
        serde_json::json!({"version": 1, "enabled": false, "pairs": []})
    );
    assert!(params.validate_state(&state, false, false, Some(48_000.0)));

    // Once marked as current, another migration must not reinterpret order
    // index 2 as the legacy raw order 2 and accidentally turn order 6 into 2.
    let once = serde_json::to_value(&state).unwrap();
    migrate_eq_native_state(&mut state);
    assert_eq!(serde_json::to_value(&state).unwrap(), once);
}

#[test]
fn current_version_missing_placement_is_rejected_without_migration_repair() {
    let params = eq_params();
    let mut state = current_state(&params);
    assert!(params.validate_state(&state, false, false, Some(48_000.0)));
    state.params.remove("filter_7_placement");
    let before = serde_json::to_value(&state).unwrap();

    migrate_eq_native_state(&mut state);

    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert!(!params.validate_state(&state, false, false, Some(48_000.0)));
}

#[test]
fn invalid_legacy_order_cannot_become_a_valid_current_choice_index() {
    let params = eq_params();
    // These are invalid raw orders, but all are valid current choice indices.
    // A migration must not silently reinterpret them as orders 2, 4 or 8.
    for invalid_order in [0, 1, 3] {
        let mut state = current_state(&params);
        state.fields.clear();
        for band in 0..20 {
            state
                .params
                .insert(format!("band_{band}_order"), ParamValue::I32(2));
        }
        state
            .params
            .insert("band_7_order".into(), ParamValue::I32(invalid_order));

        migrate_eq_native_state(&mut state);

        assert!(
            !params.validate_state(&state, false, false, Some(48_000.0)),
            "invalid legacy raw order {invalid_order} was accepted as a current index"
        );
    }
}

#[test]
fn corrected_apply_recovers_after_restored_route_exceeds_layout_width() {
    let params = eq_params();
    let committed = EqPairRoute {
        enabled: true,
        pairs: vec![[0, 1]],
    };
    params.complete_eq_pair_route(committed, true).unwrap();
    let saved_before = serialized_route(&params);
    let restored_fields = std::collections::BTreeMap::from([(
        EQ_NATIVE_STATE_FIELD.to_owned(),
        serde_json::json!({"version": 1, "enabled": true, "pairs": [[4, 5]]}).to_string(),
    )]);
    params.deserialize_fields(&restored_fields);

    // A valid saved route can still be incompatible with the negotiated width.
    // The attempt is unsuccessful, and committed state must remain untouched.
    let failed = EqPairApplyAttempt::new(params.clone());
    assert!(params.eq_pair_route_for_initialization(2).is_err());
    drop(failed);
    assert_eq!(serialized_route(&params), saved_before);

    set_draft(&params, &[[1, 0]]);
    set_int(&params, "stereo_pairs_apply", 1);
    let mut retry = EqPairApplyAttempt::new(params.clone());
    let (route, publish) = params
        .eq_pair_route_for_initialization(2)
        .expect("explicit correction must supersede the failed restored route");
    assert_eq!(route.pairs, [[1, 0]]);
    assert!(publish);
    params.complete_eq_pair_route(route, publish).unwrap();
    retry.commit();
    assert_eq!(
        serialized_route(&params)["pairs"],
        serde_json::json!([[1, 0]])
    );
    assert_eq!(
        params.value("stereo_pairs_apply"),
        Some(ParameterValue::Int(0))
    );
}
