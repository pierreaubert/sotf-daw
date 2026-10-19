//! Crossfeed factory presets and actual queued host mix/bypass controls.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, ParameterId, ParameterValue, ProcessContext, create_plugin};

#[test]
fn queued_mix_endpoints_and_reset_bypass_match_the_native_path() {
    for rate in [48_000, 192_000] {
        for mode in ["Bauer", "Meier", "Mb", "Hrtf"] {
            for auto_gain in [false, true] {
                let preset =
                    serde_json::json!({"mode":mode,"mix":0.0,"autogain_enabled":auto_gain});
                let reloaded =
                    serde_json::from_str(&serde_json::to_string(&preset).unwrap()).unwrap();
                let mut direct = create_plugin("crossfeed", &preset, 2, rate).unwrap();
                direct.initialize(f64::from(rate)).unwrap();
                let mut host = DawHost::new(2, rate);
                host.add_plugin(create_plugin("crossfeed", &reloaded, 2, rate).unwrap())
                    .unwrap();
                host.build().unwrap();
                let input: Vec<f32> = (0..rate / 2)
                    .flat_map(|i| {
                        [
                            (i as f32 * 0.017).sin() * 0.1,
                            (i as f32 * 0.071).cos() * 0.05,
                        ]
                    })
                    .collect();
                for target in [1.0, 0.0, 1.0, 0.0] {
                    host.set_plugin_parameter(0, "mix", ParameterValue::Float(target))
                        .unwrap();
                    direct
                        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(target))
                        .unwrap();
                    let mut actual = vec![0.0; input.len()];
                    let mut expected = actual.clone();
                    for (src, dst) in input.chunks(8192 * 2).zip(expected.chunks_mut(8192 * 2)) {
                        direct
                            .process(src, dst, &ProcessContext::new(rate, src.len() / 2))
                            .unwrap();
                    }
                    for (src, dst) in input.chunks(257 * 2).zip(actual.chunks_mut(257 * 2)) {
                        host.process(src, dst).unwrap();
                    }
                    assert!(
                        actual == expected,
                        "{rate} {mode} AG={auto_gain} mix={target}: callback or queued control mismatch"
                    );
                    if !auto_gain && target == 0.0 {
                        assert!(
                            actual[actual.len() - 512..] == input[input.len() - 512..],
                            "dry endpoint must be exact"
                        );
                    }
                }
                host.set_plugin_parameter(0, "enabled", ParameterValue::Bool(false))
                    .unwrap();
                let mut actual = vec![0.0; 514];
                host.process(&input[..514], &mut actual).unwrap();
                assert!(
                    actual == input[..514],
                    "hard bypass is exact even with AutoGain"
                );
                host.set_plugin_parameter(0, "enabled", ParameterValue::Bool(true))
                    .unwrap();
                host.process(&input[..514], &mut actual).unwrap();
                let mut fresh = create_plugin(
                    "crossfeed",
                    &serde_json::json!({"mode":mode,"mix":0.0,"autogain_enabled":auto_gain}),
                    2,
                    rate,
                )
                .unwrap();
                fresh.initialize(f64::from(rate)).unwrap();
                let mut expected = vec![0.0; 514];
                fresh
                    .process(
                        &input[..514],
                        &mut expected,
                        &ProcessContext::new(rate, 257),
                    )
                    .unwrap();
                assert!(
                    actual == expected,
                    "reset bypass must not replay prior crossfeed or gain state"
                );
            }
        }
    }
}
