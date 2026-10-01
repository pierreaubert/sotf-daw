use nih_plug::prelude::{Param, ParamFlags};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};

use super::DynamicParams;

fn dynamic_eq_infos() -> Vec<BridgedParamInfo> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("DynamicEQ"));
    let mut infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();

    if infos.is_empty() {
        let plugin = plugins_bridge::create_plugin(
            "DynamicEQ",
            crate::wrapper::plugin_constructor_channels("DynamicEQ"),
            48_000,
            &crate::wrapper::default_plugin_config("DynamicEQ"),
        )
        .expect("create DynamicEQ to inspect its complete parameter schema");
        for parameter in plugin.parameters() {
            if let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter) {
                infos.push(info);
            }
        }
    }

    infos
}

fn dynamic_eq_params(infos: &[BridgedParamInfo]) -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("DynamicEQ", infos)
}

#[test]
fn shelf_shape_and_slope_are_visible_manual_restart_controls() {
    let params = dynamic_eq_params(&dynamic_eq_infos());

    for band in 0..8 {
        let shape_id = format!("band_{band}_shape");
        let shape = params.param_map.get(&shape_id).expect("shape parameter");
        assert!(
            !shape.realtime,
            "{shape_id} must not sync into the running DSP"
        );
        assert!(shape.requires_restart, "{shape_id}");
        let flags = params.int_params[shape.index].flags();
        assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "{shape_id}");
        assert!(flags.contains(ParamFlags::REQUIRES_RESTART), "{shape_id}");
        assert!(!flags.contains(ParamFlags::HIDDEN), "{shape_id}");

        let slope_id = format!("band_{band}_shelf_slope");
        let slope = params.param_map.get(&slope_id).expect("slope parameter");
        assert!(
            !slope.realtime,
            "{slope_id} must not sync into the running DSP"
        );
        assert!(slope.requires_restart, "{slope_id}");
        let flags = params.float_params[slope.index].flags();
        assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "{slope_id}");
        assert!(flags.contains(ParamFlags::REQUIRES_RESTART), "{slope_id}");
        assert!(!flags.contains(ParamFlags::HIDDEN), "{slope_id}");

        let placement_id = format!("band_{band}_placement");
        let placement = params
            .param_map
            .get(&placement_id)
            .expect("placement parameter");
        assert!(
            !placement.realtime,
            "{placement_id} must not sync into the running DSP"
        );
        assert!(placement.requires_restart, "{placement_id}");
        let flags = params.int_params[placement.index].flags();
        assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "{placement_id}");
        assert!(
            flags.contains(ParamFlags::REQUIRES_RESTART),
            "{placement_id}"
        );
        assert!(!flags.contains(ParamFlags::HIDDEN), "{placement_id}");
    }

    let shape = &params.int_params[params.param_map["band_0_shape"].index];
    assert_eq!(
        shape.normalized_value_to_string(0.0, false),
        "Peak",
        "the old value remains the default"
    );
    assert_eq!(
        shape.normalized_value_to_string(1.0 / 3.0, false),
        "Low shelf"
    );
    assert_eq!(
        shape.normalized_value_to_string(2.0 / 3.0, false),
        "High shelf"
    );
    assert_eq!(shape.normalized_value_to_string(1.0, false), "Tilt");

    let placement = &params.int_params[params.param_map["band_0_placement"].index];
    assert_eq!(
        placement.normalized_value_to_string(0.0, false),
        "Stereo",
        "placement default stays stereo"
    );
    assert_eq!(placement.normalized_value_to_string(0.25, false), "Left");
    assert_eq!(placement.normalized_value_to_string(0.5, false), "Right");
    assert_eq!(placement.normalized_value_to_string(0.75, false), "Mid");
    assert_eq!(placement.normalized_value_to_string(1.0, false), "Side");

    let legacy_peak = &params.param_map["band_0_frequency"];
    let flags = params.float_params[legacy_peak.index].flags();
    assert!(flags.contains(ParamFlags::HIDDEN));
    assert!(!flags.contains(ParamFlags::REQUIRES_RESTART));
}

#[test]
fn restartable_shelf_values_do_not_mask_other_structural_changes() {
    let baseline_infos = dynamic_eq_infos();
    let baseline = dynamic_eq_params(&baseline_infos);

    let mut changed_shape_infos = dynamic_eq_infos();
    changed_shape_infos
        .iter_mut()
        .find(|info| info.id == "band_0_shape")
        .expect("shape info")
        .default_value = 2.0;
    let changed_shape = dynamic_eq_params(&changed_shape_infos);
    assert_ne!(
        baseline.structural_fingerprint(),
        changed_shape.structural_fingerprint()
    );
    assert_eq!(
        baseline.non_restartable_structural_fingerprint(),
        changed_shape.non_restartable_structural_fingerprint()
    );

    let mut changed_slope_infos = dynamic_eq_infos();
    changed_slope_infos
        .iter_mut()
        .find(|info| info.id == "band_0_shelf_slope")
        .expect("slope info")
        .default_value = 0.75;
    let changed_slope = dynamic_eq_params(&changed_slope_infos);
    assert_ne!(
        baseline.structural_fingerprint(),
        changed_slope.structural_fingerprint()
    );
    assert_eq!(
        baseline.non_restartable_structural_fingerprint(),
        changed_slope.non_restartable_structural_fingerprint()
    );

    let mut changed_placement_infos = dynamic_eq_infos();
    changed_placement_infos
        .iter_mut()
        .find(|info| info.id == "band_0_placement")
        .expect("placement info")
        .default_value = 4.0;
    let changed_placement = dynamic_eq_params(&changed_placement_infos);
    assert_ne!(
        baseline.structural_fingerprint(),
        changed_placement.structural_fingerprint()
    );
    assert_eq!(
        baseline.non_restartable_structural_fingerprint(),
        changed_placement.non_restartable_structural_fingerprint()
    );

    let mut changed_frequency_infos = dynamic_eq_infos();
    changed_frequency_infos
        .iter_mut()
        .find(|info| info.id == "band_0_frequency")
        .expect("frequency info")
        .default_value = 2_000.0;
    let changed_frequency = dynamic_eq_params(&changed_frequency_infos);
    assert_ne!(
        baseline.non_restartable_structural_fingerprint(),
        changed_frequency.non_restartable_structural_fingerprint(),
        "a frequency or any other non-restartable construction value stays guarded"
    );
}
