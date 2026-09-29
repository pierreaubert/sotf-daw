//! Single-value host automation must not allocate a bulk parameter map.

use sotf_host::{
    CountingAlloc, ParameterId, ParameterSet, ParameterValue, ParametricInPlacePlugin,
    ProcessContext, assert_no_allocs,
};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn realtime_getters_setters_and_processing_do_not_allocate() {
    let mut plugin = LimiterPlugin::from_params(
        6,
        serde_json::from_str::<LimiterPluginParams>("{}").unwrap(),
    );
    plugin.initialize(48_000).unwrap();
    let updates: Vec<_> = [
        ("threshold", ParameterValue::Float(-4.0)),
        ("release", ParameterValue::Float(75.0)),
        ("soft", ParameterValue::Bool(true)),
        ("true_peak", ParameterValue::Bool(true)),
        ("dual_release", ParameterValue::Bool(true)),
        ("mix", ParameterValue::Float(0.75)),
        ("feed_forward", ParameterValue::Bool(true)),
        ("link_amount", ParameterValue::Float(0.4)),
    ]
    .into_iter()
    .map(|(id, value)| (ParameterId::from(id), value))
    .collect();
    let mut samples = vec![0.25; 256 * 6];
    for (id, value) in &updates {
        assert_no_allocs("Limiter realtime parameter and audio processing", || {
            plugin.parametric_validate_parameter(id, value).unwrap();
            plugin
                .parametric_set_parameter(id.clone(), value.clone())
                .unwrap();
            assert_eq!(plugin.parametric_get_parameter(id), Some(value.clone()));
            plugin
                .process_in_place(&mut samples, &ProcessContext::new(48_000, 256))
                .unwrap();
        });
    }
    let bulk = plugin.current_values();
    for (id, value) in updates {
        assert_eq!(bulk.get(&id), Some(&value));
    }
    assert_no_allocs("Limiter reset", || plugin.reset());
}

#[test]
fn bulk_and_direct_parameter_paths_report_identical_values() {
    let mut plugin = LimiterPlugin::from_params(
        2,
        serde_json::from_str::<LimiterPluginParams>("{}").unwrap(),
    );
    let mut values = ParameterSet::new();
    values.insert(ParameterId::from("threshold"), ParameterValue::Float(-6.0));
    values.insert(ParameterId::from("lookahead"), ParameterValue::Float(8.0));
    values.insert(ParameterId::from("isp_mode"), ParameterValue::Bool(true));
    plugin.apply_values(values).unwrap();
    plugin.initialize(48_000).unwrap();
    for (id, value) in plugin.current_values() {
        assert_eq!(plugin.parametric_get_parameter(&id), Some(value));
    }
    assert_eq!(plugin.latency_samples(), 384 + 18);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("unknown")),
        None
    );
}
