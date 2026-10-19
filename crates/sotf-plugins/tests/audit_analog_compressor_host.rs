//! Analog Compressor live controls through actual host setters and audio callbacks.

// Rust guideline compliant 2026-02-21
use sotf_host::CountingAlloc;
use sotf_host::test_utils::{assert_no_allocs_or_deallocs, measure_heap_activity};
use sotf_plugins::{DawHost, ParameterValue, create_plugin};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn real_host_setters_keep_queued_dynamics_and_color_application_heap_free() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [2, 8] {
            for model in sotf_plugin_analog_common::MODEL_NAMES {
                let preset = serde_json::json!({"analog_model":model,"analog_color":0.6,"analog_drive":9.0,
                    "threshold":-24.0,"ratio":8.0});
                let mut host = DawHost::new(channels, rate);
                host.set_max_block_frames(4096).unwrap();
                host.add_plugin(
                    create_plugin("analog_compressor", &preset, channels, rate).unwrap(),
                )
                .unwrap();
                host.build().unwrap();
                let immediate = measure_heap_activity(|| {
                    host.set_plugin_parameter_immediate(
                        0,
                        "threshold",
                        ParameterValue::Float(-18.0),
                    )
                    .unwrap()
                });
                let queued = measure_heap_activity(|| {
                    for (key, value) in [
                        ("threshold", -24.0),
                        ("ratio", 8.0),
                        ("attack", 3.0),
                        ("release", 80.0),
                        ("knee", 9.0),
                        ("range_db", 12.0),
                        ("hold_ms", 7.5),
                        ("makeup", 2.0),
                        ("mix", 0.6),
                        ("analog_drive", 12.0),
                        ("analog_color", 0.8),
                        ("analog_character", 0.7),
                        ("analog_trim", -3.0),
                    ] {
                        host.set_plugin_parameter(0, key, ParameterValue::Float(value))
                            .unwrap();
                    }
                    host.set_plugin_parameter(0, "auto_makeup", ParameterValue::Bool(true))
                        .unwrap();
                });
                let input: Vec<f32> = (0..channels * 513)
                    .map(|i| 0.1 * (i as f64 * 0.137).sin() as f32)
                    .collect();
                let mut output = vec![0.0; input.len()];
                assert_no_allocs_or_deallocs(
                    "Analog Compressor actual queued host callback",
                    || {
                        host.process(&input, &mut output).unwrap();
                    },
                );
                assert!(output.iter().all(|x| x.is_finite()));
                if rate == 48_000 && channels == 2 && *model == "Harmonics" {
                    println!(
                        "control-thread heap counts immediate={immediate:?}; queueing fourteen controls={queued:?}; audio callback=(0,0)"
                    );
                }
            }
        }
    }
}

#[test]
fn factory_presets_immediate_and_queued_controls_match_native_audio() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in sotf_plugin_analog_common::MODEL_NAMES {
            let preset = serde_json::json!({"analog_model":model,"analog_color":0.7,"analog_drive":12.0,"threshold":-24.0});
            let saved = serde_json::to_string(&preset).unwrap();
            let reloaded = serde_json::from_str(&saved).unwrap();
            let mut host = DawHost::new(2, rate);
            host.set_max_block_frames(8192).unwrap();
            host.add_plugin(create_plugin("analog_compressor", &reloaded, 2, rate).unwrap())
                .unwrap();
            host.build().unwrap();
            let mut direct = create_plugin("analog_compressor", &preset, 2, rate).unwrap();
            direct.initialize(f64::from(rate)).unwrap();
            for (key, value) in [
                ("threshold", -18.0),
                ("attack", 0.1),
                ("release", 1000.0),
                ("ratio", 20.0),
                ("hold_ms", 7.5),
                ("range_db", 6.0),
                ("makeup", 3.0),
                ("mix", 0.0),
                ("mix", 1.0),
                ("analog_color", 0.0),
                ("analog_color", 1.0),
                ("analog_trim", -6.0),
            ] {
                host.set_plugin_parameter(0, key, ParameterValue::Float(value))
                    .unwrap();
                direct
                    .set_parameter(key.into(), ParameterValue::Float(value))
                    .unwrap();
                let input: Vec<f32> = (0..4099)
                    .flat_map(|i| {
                        let x = 0.17 * (i as f64 * 0.137).sin() as f32;
                        [x, -x * 0.37]
                    })
                    .collect();
                let mut actual = vec![0.0; input.len()];
                let mut expected = actual.clone();
                for (src, dst) in input.chunks(62).zip(actual.chunks_mut(62)) {
                    host.process(src, dst).unwrap();
                }
                for (src, dst) in input.chunks(8192 * 2).zip(expected.chunks_mut(8192 * 2)) {
                    direct
                        .process(
                            src,
                            dst,
                            &sotf_plugins::ProcessContext::new(rate, src.len() / 2),
                        )
                        .unwrap();
                }
                assert!(
                    actual == expected,
                    "{rate} {model} {key}: queued host and native callback output differ"
                );
            }
        }
    }
}
