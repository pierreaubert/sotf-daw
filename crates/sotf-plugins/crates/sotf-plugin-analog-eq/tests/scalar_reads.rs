//! Scalar reads must retain their types without constructing a parameter map.

use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, assert_no_allocs,
};
use sotf_plugin_analog_eq::AnalogEqPlugin;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn cold_primitive_reads_match_default_and_nondefault_snapshots() {
    for changed in [false, true] {
        let mut plugin = AnalogEqPlugin::from_params(2, Default::default()).unwrap();
        if changed {
            for (name, value) in [
                ("analog_drive", 3.0),
                ("analog_color", 0.7),
                ("analog_trim", -2.0),
            ] {
                plugin
                    .set_parameter(ParameterId::from(name), ParameterValue::Float(value))
                    .unwrap();
            }
        }
        plugin.initialize(48_000).unwrap();
        let snapshot = plugin.current_values();
        for (id, value) in &snapshot {
            assert_eq!(plugin.get_parameter(id).as_ref(), Some(value));
        }
        let scalars: Vec<_> = snapshot
            .into_iter()
            .filter(|(_, value)| !matches!(value, ParameterValue::String(_)))
            .collect();
        let unknown = ParameterId::from("unknown_parameter");
        std::thread::spawn(move || {
            assert_no_allocs("analog cold scalar reads", || {
                for (id, value) in &scalars {
                    assert_eq!(plugin.get_parameter(id).as_ref(), Some(value));
                }
                assert_eq!(plugin.get_parameter(&unknown), None);
            });
        })
        .join()
        .unwrap();
    }
}
