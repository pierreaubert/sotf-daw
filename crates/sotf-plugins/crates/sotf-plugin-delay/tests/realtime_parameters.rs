use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, assert_no_allocs,
};
use sotf_plugin_delay::DelayPlugin;

#[global_allocator]
static A: CountingAlloc = CountingAlloc;

#[test]
fn realtime_parameter_writes_do_not_allocate() {
    let mut plugin = DelayPlugin::new(2, 100.0, 0.3, 0.5);
    let feedback = ParameterId::from("feedback");
    let mix = ParameterId::from("mix");
    let lfo_rate = ParameterId::from("lfo_rate_hz");
    let allpass = ParameterId::from("allpass_feedback");

    assert_no_allocs("Delay realtime parameter writes", || {
        plugin
            .set_parameter(feedback.clone(), ParameterValue::Float(0.4))
            .unwrap();
        plugin
            .set_parameter(mix.clone(), ParameterValue::Float(0.6))
            .unwrap();
        plugin
            .set_parameter(lfo_rate.clone(), ParameterValue::Float(2.0))
            .unwrap();
        plugin
            .set_parameter(allpass.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
}

#[test]
fn cold_scalar_reads_match_snapshots_without_allocating() {
    for mut plugin in [
        DelayPlugin::new(2, 100.0, 0.3, 0.5),
        DelayPlugin::new_per_channel(vec![2.0, 7.0, 13.0]).unwrap(),
    ] {
        plugin.initialize(48_000.0).unwrap();
        let expected = plugin.current_values();
        let unknown = ParameterId::from("missing");
        let invalid_channel = ParameterId::from("delay_ms_64");
        std::thread::spawn(move || {
            assert_no_allocs("Delay cold scalar reads", || {
                for (id, value) in &expected {
                    assert_eq!(plugin.parametric_get_parameter(id).as_ref(), Some(value));
                }
                assert_eq!(plugin.parametric_get_parameter(&unknown), None);
                assert_eq!(plugin.parametric_get_parameter(&invalid_channel), None);
            });
        })
        .join()
        .unwrap();
    }
}
