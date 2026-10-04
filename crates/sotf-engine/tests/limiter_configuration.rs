//! Persisted engine Limiter controls reach the DSP and its channel-link law.

// Rust guideline compliant 2026-02-21
use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::{ParameterId, ParameterValue, ProcessContext, create_plugin};

fn settings(link: f64, feed: bool) -> PluginSettings {
    let mut settings = PluginSettings::default_for(&PluginType::Limiter).unwrap();
    let PluginSettings::Limiter {
        threshold_db,
        release_ms,
        lookahead_ms,
        link_amount,
        feed_forward,
        ..
    } = &mut settings
    else {
        panic!("Limiter default must have Limiter settings");
    };
    *threshold_db = -12.0;
    *release_ms = 10.0;
    *lookahead_ms = 0.0;
    *link_amount = link;
    *feed_forward = feed;
    let encoded = serde_json::to_vec(&settings).unwrap();
    serde_json::from_slice(&encoded).unwrap()
}

#[test]
fn engine_limiter_link_and_compatibility_controls_reach_dsp() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2, 6] {
            for link in [0.0, 0.35, 1.0] {
                for feed in [false, true] {
                    let config = settings(link, feed).to_plugin_config(f64::from(rate));
                    let mut plugin =
                        create_plugin(&config.plugin_type, &config.parameters, channels, rate)
                            .unwrap();
                    plugin.initialize(f64::from(rate)).unwrap();
                    assert_eq!(
                        plugin.get_parameter(&ParameterId::from("link_amount")),
                        Some(ParameterValue::Float(link as f32))
                    );
                    assert_eq!(
                        plugin.get_parameter(&ParameterId::from("feed_forward")),
                        Some(ParameterValue::Bool(feed))
                    );
                }
            }
        }
    }
}

#[test]
fn engine_unlinked_limiter_preserves_quiet_neighbor_channel() {
    let input: Vec<f32> = (0..2048).flat_map(|_| [0.9, 0.1]).collect();
    for link in [0.0, 1.0] {
        let config = settings(link, false).to_plugin_config(48_000.0);
        let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let mut output = vec![f32::NAN; input.len()];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 2048))
            .unwrap();
        let ceiling = 10.0_f64.powf(-12.0 / 20.0);
        for frame in output[1024..].as_chunks::<2>().0 {
            assert!((f64::from(frame[0]) - ceiling).abs() < 2e-6);
            if link == 0.0 {
                assert!(
                    (frame[1] - 0.1).abs() < 1e-6,
                    "unlinked neighbor: {}",
                    frame[1]
                );
            } else {
                assert!(frame[1] < 0.04, "linked negative control: {}", frame[1]);
            }
        }
    }
}

#[test]
fn limiter_legacy_settings_keep_native_default_and_stable_parameter_indices() {
    let defaults = PluginSettings::default_for(&PluginType::Limiter).unwrap();
    let expected = [
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
        "oversampling",
    ];
    assert_eq!(defaults.param_specs().len(), expected.len());
    for (spec, key) in defaults.param_specs().iter().zip(expected) {
        assert_eq!(spec.engine_key, key);
    }
    let mut old_json = serde_json::to_value(defaults).unwrap();
    old_json["Limiter"]
        .as_object_mut()
        .unwrap()
        .remove("oversampling");
    let restored: PluginSettings = serde_json::from_value(old_json).unwrap();
    assert_eq!(restored.param_value(10), Some(0.0));
    let config = restored.to_plugin_config(48000.0);
    assert_eq!(config.parameters["oversampling"], 0);
    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48000).unwrap();
    plugin.initialize(48000.0).unwrap();
    assert_eq!(plugin.latency_samples(), 240);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("oversampling")),
        Some(ParameterValue::Int(0))
    );
}

#[test]
fn limiter_oversampling_persists_through_engine_accessors_and_real_dry_audio() {
    for rate in [44100, 48000, 96000] {
        for channels in [1, 2, 6] {
            for choice in 0..=2 {
                let mut configured = settings(0.35, true);
                configured.set_param_value(2, 0.125);
                configured.set_param_value(7, 0.0);
                configured.set_param_value(10, f64::from(choice));
                assert_eq!(configured.param_value(10), Some(f64::from(choice)));
                let saved = serde_json::to_vec(&configured).unwrap();
                let restored: PluginSettings = serde_json::from_slice(&saved).unwrap();
                let config = restored.to_plugin_config(f64::from(rate));
                assert_eq!(config.parameters["oversampling"], choice);
                let mut plugin =
                    create_plugin(&config.plugin_type, &config.parameters, channels, rate).unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                assert_eq!(
                    plugin.get_parameter(&ParameterId::from("oversampling")),
                    Some(ParameterValue::Int(choice))
                );
                assert!(plugin.preferred_oversampling().is_none());
                let delay = (f64::from(rate) * 0.000125).floor() as usize
                    + if choice == 0 { 0 } else { 512 };
                assert_eq!(plugin.latency_samples(), delay);
                let source: Vec<f32> = (0..1031 * channels)
                    .map(|i| ((i * 23 % 127) as i32 - 63) as f32 / 512.0)
                    .collect();
                let mut output = vec![987.0; source.len()];
                let mut cursor = 0;
                for &size in [17, 1, 511, 137].iter().cycle() {
                    let frames = size.min(1031 - cursor);
                    if frames == 0 {
                        break;
                    }
                    let range = cursor * channels..(cursor + frames) * channels;
                    assert_eq!(
                        plugin
                            .process(
                                &source[range.clone()],
                                &mut output[range],
                                &ProcessContext::new(rate, frames)
                            )
                            .unwrap(),
                        frames
                    );
                    cursor += frames;
                }
                let mut drained = vec![987.0; plugin.drain_output_frames_max().max(1) * channels];
                let mut complete = false;
                for _ in 0..64 {
                    drained.fill(987.0);
                    let result = plugin
                        .drain(&mut drained, &ProcessContext::new(rate, 0))
                        .unwrap();
                    assert!(
                        drained[result.frames * channels..]
                            .iter()
                            .all(|&s| s == 987.0)
                    );
                    output.extend_from_slice(&drained[..result.frames * channels]);
                    if result.complete {
                        complete = true;
                        break;
                    }
                }
                assert!(complete);
                assert!(output.len() >= source.len() + delay * channels);
                for (i, &actual) in output.iter().enumerate() {
                    let expected = i
                        .checked_sub(delay * channels)
                        .and_then(|j| source.get(j))
                        .copied()
                        .unwrap_or(0.0);
                    assert_eq!(
                        actual, expected,
                        "rate={rate}, channels={channels}, choice={choice}, sample={i}"
                    );
                }
            }
        }
    }
}
