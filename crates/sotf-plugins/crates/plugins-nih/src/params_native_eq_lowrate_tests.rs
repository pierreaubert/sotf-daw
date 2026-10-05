//! Native EQ keeps the host's requested controls while preparing valid DSP bands.

use super::*;
use sotf_host::parameters::{ParameterId, ParameterValue};

fn default_native_eq() -> std::sync::Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("EQ", &super::default_sync_tests::schema_infos("EQ"))
}

fn route() -> EqPairRoute {
    EqPairRoute {
        enabled: false,
        pairs: Vec::new(),
    }
}

#[test]
fn native_eq_frequency_preserves_valid_requests_and_rejects_invalid_clock() {
    let effective = crate::wrapper::native_eq_effective_frequency;
    assert_eq!(effective(20_000.0, 44_100.0).unwrap(), 20_000.0);
    assert_eq!(effective(1_000.0, 48_000.0).unwrap(), 1_000.0);
    let low = effective(1_000.0, 1_234.567_8).unwrap();
    assert!(f64::from(low) < 1_234.567_8 / 2.0);
    assert!(f64::from(low) >= 20.0);
    let at_nyquist = effective(1_000.0, 2_000.0).unwrap();
    assert!(at_nyquist < 1_000.0);
    for rate in [f64::NAN, f64::INFINITY, 0.0, -1.0, 20.0] {
        assert!(effective(1_000.0, rate).is_err());
    }
    for requested in [f32::NAN, f32::INFINITY, 19.0, 20_001.0] {
        assert!(effective(requested, 48_000.0).is_err());
    }
}

#[test]
fn native_eq_keeps_requested_state_across_validator_clocks_and_reactivation() {
    let params = default_native_eq();
    assert!(plugins_bridge::create_plugin(
        "EQ",
        2,
        1_234.567_8,
        &crate::wrapper::eq_config_json(|_| None),
    )
    .is_err());
    let mut direct = plugins_bridge::create_plugin(
        "EQ",
        2,
        48_000.0,
        &crate::wrapper::eq_config_json(|_| None),
    )
    .unwrap();
    let direct_id = ParameterId::from("band_0_freq");
    let old_direct = direct.get_parameter(&direct_id);
    assert!(direct
        .set_parameter(direct_id.clone(), ParameterValue::Float(20_001.0))
        .is_err());
    assert_eq!(direct.get_parameter(&direct_id), old_direct);
    let id = ParameterId::from("band_0_freq");
    let requested = params.value("band_0_freq");
    for rate in [
        8_000.0, 22_050.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0,
        384_000.0, 768_000.0, 1_234.567_8, 12_345.678, 45_678.901, 123_456.78,
    ] {
        let mut plugin = super::configuration::create_native_eq_plugin(rate, &params, 2, &route())
            .unwrap_or_else(|error| panic!("{rate}: {error}"));
        plugin.initialize(rate).unwrap();
        params.sync_to_native_eq_plugin(plugin.as_mut(), rate).unwrap();
        let effective = crate::wrapper::native_eq_effective_frequency(1_000.0, rate).unwrap();
        assert_eq!(plugin.get_parameter(&id), Some(ParameterValue::Float(effective)));
        assert_eq!(params.value("band_0_freq"), Some(ParameterValue::Float(1_000.0)));
        let input = [0.1_f32; 256];
        let mut output = [0.0_f32; 256];
        let context = sotf_host::ProcessContext::new(rate, 128);
        assert_eq!(plugin.process(&input, &mut output, &context).unwrap(), 128);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
    assert_eq!(params.value("band_0_freq"), requested);
}

#[test]
fn native_eq_changed_request_sync_is_allocation_free_and_keeps_host_value() {
    let params = default_native_eq();
    let rate = 1_234.567_8;
    let mut plugin =
        super::configuration::create_native_eq_plugin(rate, &params, 2, &route()).unwrap();
    plugin.initialize(rate).unwrap();
    params.sync_to_native_eq_plugin(plugin.as_mut(), rate).unwrap();

    let mut infos = super::default_sync_tests::schema_infos("EQ");
    let frequency = infos.iter_mut().find(|info| info.id == "band_0_freq").unwrap();
    frequency.default_value = 300.0;
    let within_nyquist = DynamicParams::from_infos_for_plugin("EQ", &infos);
    assert_no_alloc::assert_no_alloc(|| {
        within_nyquist.sync_to_native_eq_plugin(plugin.as_mut(), rate)
    })
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_freq")),
        Some(ParameterValue::Float(300.0))
    );
    assert_no_alloc::assert_no_alloc(|| {
        within_nyquist.sync_to_native_eq_plugin(plugin.as_mut(), rate)
    })
        .unwrap();
    assert_eq!(within_nyquist.value("band_0_freq"), Some(ParameterValue::Float(300.0)));

    infos.iter_mut().find(|info| info.id == "band_0_freq").unwrap().default_value = 2_000.0;
    let changed = DynamicParams::from_infos_for_plugin("EQ", &infos);
    assert_no_alloc::assert_no_alloc(|| changed.sync_to_native_eq_plugin(plugin.as_mut(), rate))
        .unwrap();
    assert_no_alloc::assert_no_alloc(|| changed.sync_to_native_eq_plugin(plugin.as_mut(), rate))
        .unwrap();
    let effective = crate::wrapper::native_eq_effective_frequency(2_000.0, rate).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_freq")),
        Some(ParameterValue::Float(effective))
    );
    assert_eq!(changed.value("band_0_freq"), Some(ParameterValue::Float(2_000.0)));

    let mut restored =
        super::configuration::create_native_eq_plugin(48_000.0, &changed, 2, &route()).unwrap();
    restored.initialize(48_000.0).unwrap();
    changed.sync_to_native_eq_plugin(restored.as_mut(), 48_000.0).unwrap();
    assert_eq!(
        restored.get_parameter(&ParameterId::from("band_0_freq")),
        Some(ParameterValue::Float(2_000.0))
    );
}
