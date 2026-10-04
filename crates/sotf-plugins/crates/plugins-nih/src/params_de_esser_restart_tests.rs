use nih_plug::prelude::{Param, ParamFlags};
use plugins_bridge::param_bridge::BridgedParamInfo;

use super::DynamicParams;

fn de_esser_infos() -> Vec<BridgedParamInfo> {
    // Specs provide stable integer Choice metadata for String-typed runtime
    // controls (mode, split_topology). Merge runtime for any missing IDs.
    let bridge = plugins_bridge::param_bridge::ParamBridge::new(
        crate::wrapper::get_param_specs("DeEsser"),
    );
    let mut infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();
    let plugin = plugins_bridge::create_plugin(
        "DeEsser",
        crate::wrapper::plugin_constructor_channels("DeEsser"),
        48_000.0,
        &crate::wrapper::default_plugin_config("DeEsser"),
    )
    .expect("create DeEsser to inspect its complete parameter schema");
    for parameter in plugin.parameters() {
        if let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter)
            && !infos.iter().any(|existing| existing.id == info.id)
        {
            infos.push(info);
        }
    }
    infos
}

fn de_esser_params(infos: &[BridgedParamInfo]) -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("DeEsser", infos)
}

#[test]
fn structural_lookahead_split_and_key_are_visible_manual_restart_controls() {
    let params = de_esser_params(&de_esser_infos());

    for id in ["lookahead_ms", "split_topology", "sidechain_external"] {
        let entry = params.param_map.get(id).expect("restart parameter");
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

    let split = &params.int_params[params.param_map["split_topology"].index];
    assert_eq!(
        split.normalized_value_to_string(0.0, false),
        "Minimum-Phase"
    );
    assert_eq!(split.normalized_value_to_string(1.0, false), "Linear-Phase");

    // ms_mode is setup but realtime-safe: no restart, still non-automatable.
    let ms_mode = params.param_map.get("ms_mode").expect("ms_mode parameter");
    assert!(ms_mode.realtime, "ms_mode must sync into the running DSP");
    assert!(!ms_mode.requires_restart, "ms_mode");

    // Legacy structural controls stay hidden and guarded, not restartable.
    for id in ["frequency", "q", "mode"] {
        let entry = params.param_map.get(id).expect("legacy structural");
        assert!(!entry.realtime, "{id}");
        assert!(!entry.requires_restart, "{id}");
    }
}

#[test]
fn restartable_deesser_values_do_not_mask_other_structural_changes() {
    let baseline_infos = de_esser_infos();
    let baseline = de_esser_params(&baseline_infos);

    for (id, value) in [
        ("lookahead_ms", 5.0),
        ("split_topology", 1.0),
        ("sidechain_external", 1.0),
    ] {
        let mut changed_infos = de_esser_infos();
        changed_infos
            .iter_mut()
            .find(|info| info.id == id)
            .expect("restart info")
            .default_value = value;
        let changed = de_esser_params(&changed_infos);
        assert_ne!(
            baseline.structural_fingerprint(),
            changed.structural_fingerprint(),
            "{id} must change the construction fingerprint"
        );
        assert_eq!(
            baseline.non_restartable_structural_fingerprint(),
            changed.non_restartable_structural_fingerprint(),
            "{id} must not mask non-restartable guards"
        );
    }

    let mut changed_frequency_infos = de_esser_infos();
    changed_frequency_infos
        .iter_mut()
        .find(|info| info.id == "frequency")
        .expect("frequency info")
        .default_value = 8_000.0;
    let changed_frequency = de_esser_params(&changed_frequency_infos);
    assert_ne!(
        baseline.non_restartable_structural_fingerprint(),
        changed_frequency.non_restartable_structural_fingerprint(),
        "a frequency or any other non-restartable construction value stays guarded"
    );
}
