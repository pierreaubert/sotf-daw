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

#[test]
fn cold_first_process_call_does_not_allocate_on_new_paths() {
    // P2-D4: the warmed suite covers Wideband/Split-LR4 steady processing;
    // the first post-initialize call on the FIR, M/S, external-key and
    // lookahead paths was outside the no-alloc scope. Construction and
    // initialize may allocate (setup thread); the first audio callback on
    // each path must not. All DSP state is preallocated (delay lines,
    // scratch frames, cache spares), so first-call work is pure arithmetic.
    let sample_rate = 48_000u32;
    for (mode, topology, lookahead_ms, ms_mode, external) in [
        ("Split-Band", "Linear-Phase", 0.0f32, false, false),
        ("Split-Band", "Minimum-Phase", 0.0, true, false),
        ("Wideband", "Minimum-Phase", 2.0, true, false),
        ("Wideband", "Minimum-Phase", 0.0, false, true),
        ("Split-Band", "Linear-Phase", 5.0, true, true),
    ] {
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -24.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: mode.into(),
            mix: 1.0,
            split_topology: topology.into(),
            lookahead_ms,
            ms_mode,
            sidechain_external: external,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
        plugin.initialize(sample_rate).unwrap();
        assert_eq!(plugin.input_channels(), if external { 4 } else { 2 });
        let frames = 256;
        let mut signal = vec![0.4; frames * plugin.input_channels()];
        let context = ProcessContext::new(sample_rate, frames);
        assert_no_allocs("De-Esser cold first process", || {
            plugin.process_in_place(&mut signal, &context).unwrap();
        });
    }
}

#[test]
fn drain_and_reset_do_not_allocate() {
    // P2-D4: retained-tail emission (zero-continuation through the full DSP
    // path, here FIR + lookahead + M/S + external key with engaged GR) and
    // full state reset must not touch the allocator. The drain scratch is
    // sized at initialize; reset only fills state and snaps smoothers.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -24.0,
        ratio: 8.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Split-Band".to_string(),
        mix: 1.0,
        split_topology: "Linear-Phase".to_string(),
        lookahead_ms: 2.0,
        ms_mode: true,
        sidechain_external: true,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
    plugin.initialize(sample_rate).unwrap();
    // Stream engaged input first, outside the measured region.
    let stride = plugin.input_channels();
    assert_eq!(stride, 4);
    let frames = 4_096;
    let mut signal = vec![0.0f32; frames * stride];
    for i in 0..frames {
        let tone = 0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin();
        signal[i * stride] = tone;
        signal[i * stride + 1] = tone * 0.5;
        signal[i * stride + 2] = tone;
        signal[i * stride + 3] = tone;
    }
    plugin
        .process_in_place(&mut signal, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let mut out = vec![0.0f32; 256 * 2];
    let drain_context = ProcessContext::new(sample_rate, 256);
    assert_no_allocs("De-Esser stereo drain", || {
        loop {
            let result = plugin.drain(&mut out, &drain_context).unwrap();
            if result.complete {
                break;
            }
        }
    });
    assert_no_allocs("De-Esser reset", || {
        plugin.reset();
    });
}
