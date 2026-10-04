use nih_plug::prelude::{Param, ParamFlags, Params};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};

#[path = "params_native_eq_legacy_tests.rs"]
mod legacy_migration;

use super::{
    DynamicParams, EQ_NATIVE_STATE_FIELD, EqPairApplyAttempt, EqPairRoute,
    native_eq_pair_route_param_infos,
};

fn eq_infos(include_pair_controls: bool) -> Vec<BridgedParamInfo> {
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
    if include_pair_controls {
        infos.extend(native_eq_pair_route_param_infos());
    }
    infos
}

fn eq_params() -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("EQ", &eq_infos(true))
}

fn set_int(params: &DynamicParams, id: &str, value: i32) {
    let entry = params
        .param_map
        .get(id)
        .unwrap_or_else(|| panic!("missing {id}"));
    assert!(matches!(entry.kind, super::ParamKind::Int), "{id}");
    params.int_params[entry.index].set_plain_value_for_initialization(value);
}

fn set_bool(params: &DynamicParams, id: &str, value: bool) {
    let entry = params
        .param_map
        .get(id)
        .unwrap_or_else(|| panic!("missing {id}"));
    assert!(matches!(entry.kind, super::ParamKind::Bool), "{id}");
    params.bool_params[entry.index].set_plain_value_for_initialization(value);
}

fn set_numeric(params: &DynamicParams, id: &str, value: i32) {
    let entry = params
        .param_map
        .get(id)
        .unwrap_or_else(|| panic!("missing {id}"));
    match entry.kind {
        super::ParamKind::Int => {
            params.int_params[entry.index].set_plain_value_for_initialization(value);
        }
        super::ParamKind::Float => {
            params.float_params[entry.index].set_plain_value_for_initialization(value as f32);
        }
        super::ParamKind::Bool => panic!("{id} is boolean, not numeric"),
    }
}

fn route_draft_values(params: &DynamicParams) -> Vec<sotf_host::parameters::ParameterValue> {
    let ids = std::iter::once("stereo_pairs_enabled".to_string())
        .chain(std::iter::once("stereo_pairs_count".to_string()))
        .chain(std::iter::once("stereo_pairs_apply".to_string()))
        .chain((0..8).flat_map(|slot| {
            [
                format!("stereo_pair_{slot}_first"),
                format!("stereo_pair_{slot}_second"),
            ]
        }));
    ids.map(|id| params.value(&id).unwrap_or_else(|| panic!("missing {id}")))
        .collect()
}

fn set_draft(params: &DynamicParams, pairs: &[[usize; 2]]) {
    set_bool(params, "stereo_pairs_enabled", true);
    set_int(params, "stereo_pairs_count", pairs.len() as i32);
    for slot in 0..8 {
        let pair = pairs
            .get(slot)
            .copied()
            .unwrap_or([0, if slot == 0 { 1 } else { 0 }]);
        set_int(params, &format!("stereo_pair_{slot}_first"), pair[0] as i32);
        set_int(
            params,
            &format!("stereo_pair_{slot}_second"),
            pair[1] as i32,
        );
    }
}

fn serialized_route(params: &DynamicParams) -> serde_json::Value {
    serde_json::from_str(
        Params::serialize_fields(params)
            .get(EQ_NATIVE_STATE_FIELD)
            .expect("saved EQ route field"),
    )
    .expect("valid EQ route JSON")
}

#[test]
fn construction_controls_are_visible_non_automatable_restart_parameters() {
    let params = eq_params();
    let mut structural_ids = ["max_filters", "topology", "oversampling"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    structural_ids.extend((0..20).map(|band| format!("band_{band}_order")));
    structural_ids.extend((0..20).map(|filter| format!("filter_{filter}_placement")));

    for id in &structural_ids {
        let entry = params
            .param_map
            .get(id.as_str())
            .unwrap_or_else(|| panic!("missing {id}"));
        assert!(!entry.realtime, "{id} must not sync into the running DSP");
        assert!(entry.requires_restart, "{id}");
        let flags = match entry.kind {
            super::ParamKind::Float => params.float_params[entry.index].flags(),
            super::ParamKind::Bool => params.bool_params[entry.index].flags(),
            super::ParamKind::Int => params.int_params[entry.index].flags(),
        };
        assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "{id}");
        assert!(flags.contains(ParamFlags::REQUIRES_RESTART), "{id}");
        assert!(!flags.contains(ParamFlags::HIDDEN), "{id}");
    }

    for id in [
        "stereo_pairs_enabled",
        "stereo_pairs_count",
        "stereo_pair_0_first",
    ] {
        let entry = params
            .param_map
            .get(id)
            .unwrap_or_else(|| panic!("missing {id}"));
        assert!(entry.realtime, "draft edits should be visible immediately");
        assert!(!entry.requires_restart, "draft edits do not rebuild DSP");
        let flags = match entry.kind {
            super::ParamKind::Bool => params.bool_params[entry.index].flags(),
            super::ParamKind::Int => params.int_params[entry.index].flags(),
            super::ParamKind::Float => unreachable!("route controls are scalar bool/int"),
        };
        assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "{id}");
        assert!(!flags.contains(ParamFlags::HIDDEN), "{id}");
    }
    let apply = &params.param_map["stereo_pairs_apply"];
    assert!(apply.requires_restart);
    assert!(!apply.realtime);
    let flags = params.int_params[apply.index].flags();
    assert!(flags.contains(ParamFlags::NON_AUTOMATABLE));
    assert!(flags.contains(ParamFlags::REQUIRES_RESTART));
    assert!(!flags.contains(ParamFlags::HIDDEN));
}

#[test]
fn apply_commits_pair_route_and_failed_apply_keeps_the_previous_state() {
    let params = eq_params();
    let committed = EqPairRoute {
        enabled: true,
        pairs: vec![[0, 1], [2, 3]],
    };

    set_draft(&params, &committed.pairs);
    set_int(&params, "stereo_pairs_apply", 1);
    let mut attempt = EqPairApplyAttempt::new(params.clone());
    let (candidate_route, publish_draft) = params.eq_pair_route_for_initialization(4).unwrap();
    assert_eq!(candidate_route, committed);
    assert!(publish_draft);
    params
        .complete_eq_pair_route(candidate_route, publish_draft)
        .expect("accepted candidate commits the draft");
    attempt.commit();

    assert_eq!(
        params.value("stereo_pairs_apply"),
        Some(sotf_host::parameters::ParameterValue::Int(0))
    );
    assert_eq!(
        params.eq_pair_route_for_initialization(4).unwrap(),
        (committed.clone(), false)
    );
    assert_eq!(
        serialized_route(&params),
        serde_json::json!({"version": 1, "enabled": true, "pairs": [[0, 1], [2, 3]]})
    );

    // Editing the draft without pressing Apply must not alter the committed
    // route that state serialization and candidate initialization use.
    set_draft(&params, &[[4, 5], [6, 7]]);
    assert_eq!(
        params.eq_pair_route_for_initialization(8).unwrap(),
        (committed.clone(), false)
    );
    assert_eq!(
        serialized_route(&params)["pairs"],
        serde_json::json!([[0, 1], [2, 3]])
    );

    // A duplicate channel is rejected before candidate construction. The
    // attempt guard resets the one-shot Apply command, leaving the committed
    // route intact and retryable.
    set_draft(&params, &[[0, 2], [2, 3]]);
    set_int(&params, "stereo_pairs_apply", 1);
    let failed_attempt = EqPairApplyAttempt::new(params.clone());
    assert!(params.eq_pair_route_for_initialization(8).is_err());
    drop(failed_attempt);
    assert_eq!(
        params.value("stereo_pairs_apply"),
        Some(sotf_host::parameters::ParameterValue::Int(0))
    );
    assert_eq!(
        params.eq_pair_route_for_initialization(8).unwrap(),
        (committed, false)
    );
    assert_eq!(
        serialized_route(&params)["pairs"],
        serde_json::json!([[0, 1], [2, 3]])
    );
}

#[test]
fn state_restore_commits_explicit_route_and_schema_failure_is_transactional() {
    let params = eq_params();
    let fields = std::collections::BTreeMap::from([(
        EQ_NATIVE_STATE_FIELD.to_string(),
        serde_json::json!({"version": 1, "enabled": true, "pairs": [[1, 0], [3, 2]]}).to_string(),
    )]);
    Params::deserialize_fields(params.as_ref(), &fields);
    let restored = EqPairRoute {
        enabled: true,
        pairs: vec![[1, 0], [3, 2]],
    };
    assert_eq!(
        params.eq_pair_route_for_initialization(4).unwrap(),
        (restored.clone(), true)
    );
    params
        .complete_eq_pair_route(restored.clone(), true)
        .expect("validated restore commits");
    assert_eq!(
        params.eq_pair_route_for_initialization(4).unwrap(),
        (restored, false)
    );

    let malformed_schema = DynamicParams::from_infos_for_plugin("EQ", &eq_infos(false));
    assert!(
        malformed_schema
            .complete_eq_pair_route(
                EqPairRoute {
                    enabled: true,
                    pairs: vec![[0, 1]],
                },
                true,
            )
            .is_err()
    );
    let state = malformed_schema.eq_pair_route_state.as_ref().unwrap();
    assert!(state.lock().unwrap().committed.is_none());
}

#[test]
fn malformed_saved_route_is_refused_without_replacing_committed_route() {
    let params = eq_params();
    let committed = EqPairRoute {
        enabled: true,
        pairs: vec![[0, 1]],
    };
    params
        .complete_eq_pair_route(committed.clone(), true)
        .unwrap();

    let fields = std::collections::BTreeMap::from([(
        EQ_NATIVE_STATE_FIELD.to_string(),
        "{\"version\":1,\"enabled\":true,\"pairs\":[[0,0]]}".to_string(),
    )]);
    Params::deserialize_fields(params.as_ref(), &fields);
    assert_eq!(
        params.eq_pair_route_for_initialization(2),
        Err("saved EQ stereo-pair route is invalid".to_string())
    );
    assert_eq!(
        serialized_route(&params)["pairs"],
        serde_json::json!([[0, 1]])
    );
}

#[test]
fn wrong_integer_field_types_are_refused_before_commit_or_draft_publication() {
    for wrong_type_id in ["stereo_pairs_count", "stereo_pair_1_first"] {
        let mut infos = eq_infos(true);
        let info = infos
            .iter_mut()
            .find(|info| info.id == wrong_type_id)
            .unwrap_or_else(|| panic!("missing schema field {wrong_type_id}"));
        info.kind = plugins_bridge::param_bridge::BridgedParamKind::Float;
        let params = DynamicParams::from_infos_for_plugin("EQ", &infos);

        let committed = EqPairRoute {
            enabled: true,
            pairs: vec![[4, 5]],
        };
        params
            .complete_eq_pair_route(committed.clone(), false)
            .expect("seed the previously committed route without publishing drafts");

        set_bool(&params, "stereo_pairs_enabled", true);
        for (id, value) in [
            ("stereo_pairs_count", 2),
            ("stereo_pair_0_first", 0),
            ("stereo_pair_0_second", 1),
            ("stereo_pair_1_first", 2),
            ("stereo_pair_1_second", 3),
        ] {
            set_numeric(&params, id, value);
        }
        set_int(&params, "stereo_pairs_apply", 1);

        let draft_before = route_draft_values(&params);
        let committed_before = serialized_route(&params);
        let error = params
            .complete_eq_pair_route(
                EqPairRoute {
                    enabled: true,
                    pairs: vec![[0, 1], [2, 3]],
                },
                true,
            )
            .expect_err("wrong type must be rejected before publication");
        assert!(error.contains(wrong_type_id), "{error}");

        assert_eq!(
            params
                .eq_pair_route_state
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .committed,
            Some(committed)
        );
        assert_eq!(
            route_draft_values(&params),
            draft_before,
            "wrong type field {wrong_type_id}"
        );
        assert_eq!(
            serialized_route(&params),
            committed_before,
            "wrong type field {wrong_type_id}"
        );
    }
}

#[test]
fn eq_native_layouts_cover_all_eleven_speaker_widths() {
    use crate::wrapper::{EQ_NATIVE_LAYOUTS, eq_native_layout_index};
    use nih_plug::context::PluginApi;

    let expected_widths = [2_usize, 1, 4, 6, 8, 8, 10, 10, 12, 14, 16];
    assert_eq!(EQ_NATIVE_LAYOUTS.len(), 11);
    for (index, layout) in EQ_NATIVE_LAYOUTS.iter().enumerate() {
        let width = expected_widths[index];
        assert_eq!(
            layout.main_input_channels.unwrap().get() as usize,
            width,
            "layout {index} input width"
        );
        assert_eq!(
            layout.main_output_channels.unwrap().get() as usize,
            width,
            "layout {index} output width"
        );
        assert!(layout.aux_input_ports.is_empty(), "layout {index}");
        assert!(layout.aux_output_ports.is_empty(), "layout {index}");
        for api in [PluginApi::Clap, PluginApi::Vst3] {
            assert_eq!(
                eq_native_layout_index(layout, api),
                Some(index),
                "layout {index} round-trips for {api:?}"
            );
        }
    }
}

#[test]
fn eq_native_channel_maps_are_permutations_within_each_layout() {
    use crate::wrapper::{EQ_NATIVE_LAYOUTS, eq_native_channel_to_sotf};
    use nih_plug::context::PluginApi;

    for (index, layout) in EQ_NATIVE_LAYOUTS.iter().enumerate() {
        let width = layout.main_input_channels.unwrap().get() as usize;
        for api in [PluginApi::Clap, PluginApi::Vst3] {
            let mut seen = vec![false; width];
            for channel in 0..width {
                let mapped = eq_native_channel_to_sotf(index, api, channel)
                    .expect("every host channel maps");
                assert!(mapped < width, "layout {index} channel {channel}");
                assert!(!seen[mapped], "layout {index} duplicate map {mapped}");
                seen[mapped] = true;
            }
            assert!(seen.iter().all(|seen| *seen), "layout {index}");
        }
    }
}
