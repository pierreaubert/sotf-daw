//! Factory and persisted-state coverage for the limiter's structural factor.

// Rust guideline compliant 2026-02-21
use plugins_bridge::{create_plugin, prepare_standalone_plugin};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};

fn config(choice: Option<i32>) -> String {
    let mut value = serde_json::json!({"threshold_db":-12.0,"lookahead_ms":2.0});
    if let Some(choice) = choice {
        value["oversampling"] = choice.into();
    }
    value.to_string()
}

fn make(rate: u32, channels: usize, choice: Option<i32>, standalone: bool) -> Box<dyn Plugin> {
    let mut plugin = create_plugin("Limiter", channels, f64::from(rate), &config(choice)).unwrap();
    assert_eq!(plugin.preferred_oversampling(), None);
    if standalone {
        plugin = prepare_standalone_plugin(plugin, 257).unwrap();
    }
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn markers(channels: usize) -> Vec<f32> {
    let mut input = vec![0.0; 4096 * channels];
    for channel in 0..channels {
        input[channel] = 0.001 * (channel + 1) as f32;
        input[2047 * channels + channel] = -0.001 * (channel + 1) as f32;
    }
    input
}

fn render(plugin: &mut dyn Plugin, rate: u32, input: &[f32]) -> Vec<f32> {
    let channels = plugin.input_channels();
    let mut output = vec![f32::NAN; input.len()];
    for (input, output) in input
        .chunks(257 * channels)
        .zip(output.chunks_mut(257 * channels))
    {
        let frames = input.len() / channels;
        assert_eq!(
            plugin
                .process(input, output, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
    }
    output
}

#[test]
fn limiter_factor_keeps_legacy_indices_and_integer_normalization() {
    let bridge =
        plugins_bridge::param_bridge::ParamBridge::new(sotf_plugin_limiter::params::PARAMS);
    let ids: Vec<_> = (0..bridge.count())
        .map(|index| bridge.info(index).unwrap().id)
        .collect();
    assert_eq!(
        ids,
        [
            "threshold",
            "release",
            "lookahead",
            "soft",
            "true_peak",
            "isp_mode",
            "dual_release",
            "mix",
            "link_amount",
            "feed_forward",
            "oversampling"
        ]
    );
    let info = bridge.info(10).unwrap();
    assert_eq!(
        info.kind,
        plugins_bridge::param_bridge::BridgedParamKind::Int
    );
    assert!(!info.realtime);
    assert_eq!(
        (info.min_value, info.max_value, info.default_value),
        (0.0, 2.0, 0.0)
    );
    for choice in 0..=2 {
        let mut plugin = create_plugin("Limiter", 2, 48_000.0, &config(None)).unwrap();
        bridge
            .set_normalized(plugin.as_mut(), 10, f64::from(choice) / 2.0)
            .unwrap();
        assert_eq!(
            plugin.get_parameter(&"oversampling".into()),
            Some(ParameterValue::Int(choice))
        );
        assert_eq!(
            bridge.get_normalized(plugin.as_ref(), 10),
            Some(f64::from(choice) / 2.0)
        );
        plugin.initialize(48_000.0).unwrap();
        bridge
            .set_normalized(plugin.as_mut(), 10, f64::from(choice) / 2.0)
            .unwrap();
        let saved = plugins_bridge::state::save_state(plugin.as_ref());
        assert!(
            bridge
                .set_normalized(plugin.as_mut(), 10, f64::from((choice + 1) % 3) / 2.0)
                .is_err()
        );
        assert_eq!(plugins_bridge::state::save_state(plugin.as_ref()), saved);
    }
}

#[test]
fn limiter_factory_state_roundtrip_and_standalone_have_one_oversampling_stage() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2, 6] {
            let input = markers(channels);
            let mut old = make(rate, channels, None, true);
            let old_output = render(old.as_mut(), rate, &input);
            for choice in 0..=2 {
                let mut direct = make(rate, channels, Some(choice), false);
                let mut standalone = make(rate, channels, Some(choice), true);
                let delay =
                    (u64::from(rate) * 2 / 1000) as usize + if choice == 0 { 0 } else { 512 };
                assert_eq!(direct.latency_samples(), delay);
                assert_eq!(standalone.latency_samples(), delay);
                let state = plugins_bridge::state::save_state(standalone.as_ref());
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&state).unwrap()["oversampling"],
                    choice
                );
                let mut restored = create_plugin("Limiter", channels, f64::from(rate), "{}").unwrap();
                plugins_bridge::state::load_state(restored.as_mut(), &state).unwrap();
                restored = prepare_standalone_plugin(restored, 257).unwrap();
                restored.initialize(f64::from(rate)).unwrap();
                assert_eq!(restored.latency_samples(), delay);
                let output = render(standalone.as_mut(), rate, &input);
                assert!(output == render(direct.as_mut(), rate, &input));
                assert!(output == render(restored.as_mut(), rate, &input));
                if choice == 0 {
                    assert_eq!(output, old_output);
                } else {
                    assert!(
                        output != old_output,
                        "nondefault choice cannot silently remain 1x"
                    );
                }
                for origin in [0, 2047] {
                    for channel in 0..channels {
                        let peak = (origin..origin + 1024)
                            .max_by(|&a, &b| {
                                output[a * channels + channel]
                                    .abs()
                                    .total_cmp(&output[b * channels + channel].abs())
                            })
                            .unwrap();
                        assert_eq!(peak, origin + delay);
                        assert!(output[peak * channels + channel].abs() > 0.0005);
                    }
                }
                // An unchanged snapshot must retain the active configuration.
                plugins_bridge::state::load_state(standalone.as_mut(), &state).unwrap();
                assert_eq!(
                    standalone.get_parameter(&ParameterId::from("oversampling")),
                    Some(ParameterValue::Int(choice))
                );
            }
        }
    }
}
