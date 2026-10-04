use sotf_host::ParametricInPlacePlugin;
use sotf_host::{CountingAlloc, ParameterId, ParameterValue, assert_no_allocs};
use sotf_plugin_hiss_reducer::HissReducerPlugin;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn realtime_parameter_updates_do_not_allocate() {
    let mut plugin = HissReducerPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let updates = [
        ("enabled", ParameterValue::Bool(false)),
        ("threshold_db", ParameterValue::Float(-36.0)),
        ("frequency_hz", ParameterValue::Float(6_000.0)),
        ("strength", ParameterValue::Float(0.7)),
    ];
    let updates: Vec<_> = updates
        .into_iter()
        .map(|(name, value)| (ParameterId::from(name), value))
        .collect();

    for (id, value) in updates {
        assert_no_allocs("Hiss Reducer realtime parameter update", || {
            plugin
                .parametric_set_parameter(id.clone(), value.clone())
                .unwrap();
        });
    }
}

#[test]
fn new_control_setters_do_not_allocate() {
    use sotf_plugin_hiss_reducer::HissReducerPluginParams;

    for spectral in [false, true] {
        let mut plugin = HissReducerPlugin::from_params(
            2,
            HissReducerPluginParams {
                spectral_mode: spectral,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(48_000.0).unwrap();
        // Identifiers are built outside: Arc construction allocates,
        // cloning is a refcount bump.
        let updates = [
            ("use_captured_profile", ParameterValue::Bool(true)),
            ("curve_low", ParameterValue::Float(0.0)),
            ("curve_mid", ParameterValue::Float(0.5)),
            ("curve_high", ParameterValue::Float(1.0)),
            ("link_mode", ParameterValue::Int(1)),
            ("transient_guard", ParameterValue::Bool(true)),
            ("use_captured_profile", ParameterValue::Bool(false)),
            ("link_mode", ParameterValue::Int(0)),
            ("transient_guard", ParameterValue::Bool(false)),
        ];
        let updates: Vec<_> = updates
            .into_iter()
            .map(|(name, value)| (ParameterId::from(name), value))
            .collect();

        for (id, value) in updates {
            assert_no_allocs("Hiss Reducer new-control update", || {
                plugin
                    .parametric_set_parameter(id.clone(), value.clone())
                    .unwrap();
            });
        }
        // The toggled-off state reads back (no silent latch).
        assert_eq!(plugin.link_mode(), 0);
        assert!(!plugin.transient_guard());
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("use_captured_profile")),
            Some(ParameterValue::Bool(false))
        );
    }
}

#[test]
fn host_reachable_values_are_accepted_without_allocating() {
    use sotf_plugin_hiss_reducer::HissReducerPluginParams;

    // Every realtime parameter at its registry minimum, midpoint, and
    // maximum: exactly the value space the FFI render-loop path can
    // forward (it clamps normalized input to 0..=1 and denormalizes into
    // [min, max] with the correct type), so each of these must take the
    // Ok path — which allocates nothing — rather than the Err(String)
    // path. State-dependent rejections (structural flip post-init,
    // post-drain freeze) stay off this path; see the R5 handoff note.
    for spectral in [false, true] {
        let mut plugin = HissReducerPlugin::from_params(
            2,
            HissReducerPluginParams {
                spectral_mode: spectral,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(48_000.0).unwrap();
        let updates = [
            ("enabled", ParameterValue::Bool(true)),
            ("enabled", ParameterValue::Bool(false)),
            ("threshold_db", ParameterValue::Float(-60.0)),
            ("threshold_db", ParameterValue::Float(-35.0)),
            ("threshold_db", ParameterValue::Float(-10.0)),
            ("frequency_hz", ParameterValue::Float(1_000.0)),
            ("frequency_hz", ParameterValue::Float(8_000.0)),
            ("frequency_hz", ParameterValue::Float(16_000.0)),
            ("strength", ParameterValue::Float(0.0)),
            ("strength", ParameterValue::Float(0.5)),
            ("strength", ParameterValue::Float(1.0)),
            ("use_captured_profile", ParameterValue::Bool(true)),
            ("use_captured_profile", ParameterValue::Bool(false)),
            ("curve_low", ParameterValue::Float(0.0)),
            ("curve_low", ParameterValue::Float(1.0)),
            ("curve_mid", ParameterValue::Float(0.0)),
            ("curve_mid", ParameterValue::Float(1.0)),
            ("curve_high", ParameterValue::Float(0.0)),
            ("curve_high", ParameterValue::Float(1.0)),
            ("link_mode", ParameterValue::Int(0)),
            ("link_mode", ParameterValue::Int(1)),
            ("transient_guard", ParameterValue::Bool(true)),
            ("transient_guard", ParameterValue::Bool(false)),
        ];
        let updates: Vec<_> = updates
            .into_iter()
            .map(|(name, value)| (ParameterId::from(name), value))
            .collect();

        for (id, value) in updates {
            assert_no_allocs("Hiss Reducer host-reachable update", || {
                plugin
                    .parametric_set_parameter(id.clone(), value.clone())
                    .unwrap();
            });
        }
        // Boundary state reads back (min/max land exactly, no clamping
        // drift on the accepted path).
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("threshold_db")),
            Some(ParameterValue::Float(-10.0))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("frequency_hz")),
            Some(ParameterValue::Float(16_000.0))
        );
    }
}

#[test]
fn cold_engaged_process_drain_and_reset_do_not_allocate() {
    use sotf_host::plugin::ProcessContext;
    use sotf_plugin_hiss_reducer::HissReducerPluginParams;
    use sotf_plugin_hiss_reducer::profile::NoiseProfileData;

    for spectral in [false, true] {
        let mut plugin = HissReducerPlugin::from_params(
            2,
            HissReducerPluginParams {
                spectral_mode: spectral,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(48_000.0).unwrap();
        // Everything that allocates (profile blob, identifiers, buffers)
        // is built before counting starts.
        let data = NoiseProfileData {
            format_version: 1,
            sample_rate: 48_000.0,
            channels: 2,
            measurement_cutoff_hz: 4_000.0,
            floor_db_per_channel: vec![-40.0, -42.0],
            frames_analyzed: 48_000,
            spectral: None,
        };
        let use_id = ParameterId::from("use_captured_profile");
        let low_id = ParameterId::from("curve_low");
        let link_id = ParameterId::from("link_mode");
        let guard_id = ParameterId::from("transient_guard");
        let mut block = vec![0.05f32; 4096 * 2];
        let mut drain_block = vec![0.0f32; 256 * 2];

        assert_no_allocs("Hiss Reducer cold engaged lifecycle", || {
            // Install the profile without processing so the first
            // process call below is genuinely cold and fully engaged.
            plugin.restore_profile(&data).unwrap();
            plugin
                .parametric_set_parameter(use_id.clone(), ParameterValue::Bool(true))
                .unwrap();
            plugin
                .parametric_set_parameter(low_id.clone(), ParameterValue::Float(0.0))
                .unwrap();
            plugin
                .parametric_set_parameter(link_id.clone(), ParameterValue::Int(1))
                .unwrap();
            plugin
                .parametric_set_parameter(guard_id.clone(), ParameterValue::Bool(true))
                .unwrap();
            plugin
                .process_in_place(&mut block, &ProcessContext::new(48_000, 4096))
                .unwrap();
            if spectral {
                while !plugin
                    .drain(&mut drain_block, &ProcessContext::new(48_000, 256))
                    .unwrap()
                    .complete
                {}
            }
            plugin.reset();
            // Post-reset processing stays allocation-free too.
            plugin
                .process_in_place(&mut block, &ProcessContext::new(48_000, 4096))
                .unwrap();
        });
        assert!(plugin.has_captured_profile());
        assert!(plugin.transient_guard());
        assert!(block.iter().all(|s| s.is_finite()));
    }
}
