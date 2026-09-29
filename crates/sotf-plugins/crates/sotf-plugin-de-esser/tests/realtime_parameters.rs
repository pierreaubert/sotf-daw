use sotf_host::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_host::{CountingAlloc, ParameterId, ParameterValue, assert_no_allocs};
use sotf_plugin_de_esser::{DeEsserPlugin, DeEsserPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn realtime_parameter_updates_do_not_allocate() {
    let mut plugin = DeEsserPlugin::new(2);
    let updates = [
        ("threshold", ParameterValue::Float(-30.0)),
        ("ratio", ParameterValue::Float(8.0)),
        ("attack", ParameterValue::Float(2.0)),
        ("release", ParameterValue::Float(50.0)),
        ("mix", ParameterValue::Float(0.5)),
        ("range_db", ParameterValue::Float(6.0)),
        ("stereo_link", ParameterValue::Float(0.75)),
    ];
    let updates: Vec<_> = updates
        .into_iter()
        .map(|(name, value)| (ParameterId::from(name), value))
        .collect();
    for (id, value) in updates {
        assert_no_allocs("De-Esser realtime parameter update", || {
            plugin
                .parametric_set_parameter(id.clone(), value.clone())
                .unwrap();
        });
    }
}

#[test]
fn linked_range_processing_does_not_allocate() {
    for mode in ["Wideband", "Split-Band"] {
        let params = DeEsserPluginParams {
            mode: mode.into(),
            range_db: 6.0,
            stereo_link: 1.0,
            ..Default::default()
        };
        let mut plugin = DeEsserPlugin::from_params(2, params).unwrap();
        plugin.initialize(48_000).unwrap();
        let context = ProcessContext::new(48_000, 257);
        let mut signal = vec![0.4; context.num_frames * 2];
        // Include multiple diagnostic publications and hold a snapshot so the
        // cache's reader path is exercised while checking the audio callback.
        let _snapshot = plugin.get_data().unwrap();
        assert_no_allocs("De-Esser linked range processing", || {
            for _ in 0..16 {
                plugin.process_in_place(&mut signal, &context).unwrap();
            }
        });
    }
}
