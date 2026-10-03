use nih_plug::prelude::{Param, ParamFlags};
use plugins_bridge::param_bridge::BridgedParamInfo;

use super::DynamicParams;

fn declick_infos() -> Vec<BridgedParamInfo> {
    // Canonical nine from the bridge specs (names, ranges, defaults,
    // realtime split); the NIH consumer leaf pins the count separately.
    let bridge =
        plugins_bridge::param_bridge::ParamBridge::new(crate::wrapper::get_param_specs("Declick"));
    (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>()
}

fn declick_params(infos: &[BridgedParamInfo]) -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("Declick", infos)
}

#[test]
fn structural_declick_controls_are_visible_manual_restart_controls() {
    let params = declick_params(&declick_infos());

    for id in ["mode", "bands", "crossover_hz", "repair_width"] {
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

    // Live controls stay realtime, automatable, and restart-free.
    for id in [
        "enabled",
        "sensitivity",
        "link_channels",
        "frequency_skew",
        "audition_residual",
    ] {
        let entry = params.param_map.get(id).expect("live parameter");
        assert!(entry.realtime, "{id} must sync into the running DSP");
        assert!(!entry.requires_restart, "{id}");
        let flags = match entry.kind {
            super::ParamKind::Float => params.float_params[entry.index].flags(),
            super::ParamKind::Bool => params.bool_params[entry.index].flags(),
            super::ParamKind::Int => params.int_params[entry.index].flags(),
        };
        assert!(!flags.contains(ParamFlags::HIDDEN), "{id}");
        assert!(!flags.contains(ParamFlags::REQUIRES_RESTART), "{id}");
    }
}

#[test]
fn restartable_declick_values_do_not_mask_nonrestartable_guards() {
    let baseline_infos = declick_infos();
    let baseline = declick_params(&baseline_infos);

    for (id, value) in [
        ("mode", 1.0),
        ("bands", 2.0),
        ("crossover_hz", 8000.0),
        ("repair_width", 3.0),
    ] {
        let mut changed_infos = declick_infos();
        changed_infos
            .iter_mut()
            .find(|info| info.id == id)
            .expect("restart info")
            .default_value = value;
        let changed = declick_params(&changed_infos);
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
}
