//! Changed native controls must reach prepared DSP without callback allocation.

use super::default_sync_tests::schema_infos;
use super::*;

// RNNoise contributes one 480-frame model delay plus the 480-frame output queue.
const SPEECH_DELAY: usize = 960;

#[derive(Clone, Copy)]
enum Oracle {
    SpeechBypass,
    ChannelMuteDim,
    NeutralTransientTrim,
    UniformWidth,
}

fn input_sample(frame: usize, channel: usize) -> f32 {
    // Asymmetric deterministic channel signals, safely below the peak limiter.
    let phase = (frame * (17 + channel * 12) + channel * 7) % 101;
    (phase as f32 - 50.0) / 500.0
}

fn expected_sample(oracle: Oracle, frame: usize, channel: usize) -> f64 {
    let input = f64::from(input_sample(frame, channel));
    match oracle {
        Oracle::SpeechBypass => frame
            .checked_sub(SPEECH_DELAY)
            .map_or(0.0, |source| f64::from(input_sample(source, channel))),
        Oracle::ChannelMuteDim => {
            if channel == 0 {
                0.0
            } else {
                input * 10.0_f64.powf(-12.0 / 20.0)
            }
        }
        Oracle::NeutralTransientTrim => {
            // Neutral attack/sustain means no envelope-dependent shaping.
            // The only gain is the independently computed 10 ms trim transition.
            let target = 10.0_f64.powf(-6.0 / 20.0);
            let gain = target + (1.0 - target) * (-((frame + 1) as f64) / 480.0).exp();
            input * gain
        }
        Oracle::UniformWidth => {
            // Equal unity band widths cancel the complementary crossover terms.
            let left = f64::from(input_sample(frame, 0));
            let right = f64::from(input_sample(frame, 1));
            let width = 0.25 + 0.75 * (-((frame + 1) as f64) / 480.0).exp();
            let mid = (left + right) / 2.0;
            let side = (left - right) / 2.0 * width;
            if channel == 0 { mid + side } else { mid - side }
        }
    }
}

fn check_changed_sync(name: &str, changes: &[(&str, ParameterValue)], oracle: Option<Oracle>) {
    let mut infos = schema_infos(name);
    let initial = DynamicParams::from_infos(&infos);
    let mut plugin = super::configuration::create_plugin(name, 48_000.0, &initial).unwrap();
    plugin.initialize(48_000.0).unwrap();
    initial.sync_to_plugin(plugin.as_mut()).unwrap();
    let changes: Vec<_> = changes
        .iter()
        .map(|(id, value)| {
            let info = infos.iter_mut().find(|info| info.id == *id).unwrap();
            assert!(info.realtime, "{name}.{id}");
            let raw = match value {
                ParameterValue::Float(value) => f64::from(*value),
                ParameterValue::Bool(value) => f64::from(*value),
                ParameterValue::Int(value) => f64::from(*value),
                _ => panic!("primitive controls only"),
            };
            assert!((info.min_value..=info.max_value).contains(&raw));
            assert_ne!(info.default_value, raw, "fixture must change {name}.{id}");
            info.default_value = raw;
            let id = ParameterId::from(*id);
            assert_ne!(plugin.get_parameter(&id).as_ref(), Some(value));
            (id, value.clone())
        })
        .collect();
    let params = DynamicParams::from_infos(&infos);
    let (plugin, changes) = std::thread::spawn(move || {
        // No warmup or preapplication of the new values on the render thread.
        assert_no_alloc::assert_no_alloc(|| params.sync_to_plugin(plugin.as_mut())).unwrap();
        assert_no_alloc::assert_no_alloc(|| {
            for (id, expected) in &changes {
                assert_eq!(plugin.get_parameter(id).as_ref(), Some(expected));
            }
        });
        let mut input = [0.0; 512];
        let mut output = [0.0; 512];
        let mut offset = 0;
        for frames in [1, 17, 63, 256, 3, 127].into_iter().cycle() {
            if offset == 4096 {
                break;
            }
            let frames = frames.min(4096 - offset);
            for frame in 0..frames {
                for channel in 0..2 {
                    input[frame * 2 + channel] = input_sample(offset + frame, channel);
                }
            }
            let context = sotf_host::ProcessContext::new(48_000, frames);
            let written = assert_no_alloc::assert_no_alloc(|| {
                params.sync_to_plugin(plugin.as_mut())?;
                plugin.process(&input[..frames * 2], &mut output[..frames * 2], &context)
            })
            .unwrap();
            assert_eq!(written, frames);
            for frame in 0..frames {
                for channel in 0..2 {
                    let actual = f64::from(output[frame * 2 + channel]);
                    assert!(actual.is_finite());
                    if let Some(oracle) = oracle {
                        let expected = expected_sample(oracle, offset + frame, channel);
                        assert!(
                            (actual - expected).abs() < 3e-5,
                            "frame={}, channel={channel}: actual={actual}, expected={expected}",
                            offset + frame
                        );
                    }
                }
            }
            offset += frames;
        }
        (plugin, changes)
    })
    .join()
    .unwrap();
    let snapshot = plugin.parameters();
    for (id, expected) in changes {
        assert_eq!(plugin.get_parameter(&id), Some(expected.clone()));
        assert_eq!(
            snapshot
                .iter()
                .find(|parameter| parameter.id == id)
                .unwrap()
                .default_value,
            expected
        );
    }
}

#[test]
fn speech_bypass_changes_on_first_sync_without_allocating() {
    check_changed_sync(
        "SpeechDenoiser",
        &[("enabled", ParameterValue::Bool(false))],
        Some(Oracle::SpeechBypass),
    );
}

#[test]
fn channel_controls_match_independent_gain_arithmetic() {
    check_changed_sync(
        "ChannelMuteSolo",
        &[
            ("fade_ms", ParameterValue::Float(0.0)),
            ("dim_gain_db", ParameterValue::Float(-12.0)),
            ("mute_0", ParameterValue::Bool(true)),
            ("dim_1", ParameterValue::Bool(true)),
        ],
        Some(Oracle::ChannelMuteDim),
    );
}

#[test]
fn transient_trim_matches_independent_one_pole_gain() {
    check_changed_sync(
        "TransientShaper",
        &[
            ("output_gain", ParameterValue::Float(-6.0)),
            ("mix", ParameterValue::Float(0.3)),
        ],
        Some(Oracle::NeutralTransientTrim),
    );
}

#[test]
fn imager_width_matches_independent_mid_side_arithmetic() {
    check_changed_sync(
        "StereoImager",
        &[("width", ParameterValue::Float(0.25))],
        Some(Oracle::UniformWidth),
    );
}

// The independent waveform cases above test gain/timing. This matrix exercises
// every exported realtime primitive, including setters not used by those oracles.
fn check_all_realtime_controls(name: &str) {
    let changes: Vec<_> = schema_infos(name)
        .into_iter()
        .filter(|info| info.realtime)
        .map(|info| {
            let mut value = info.min_value + (info.max_value - info.min_value) * 0.37;
            let value = match info.kind {
                BridgedParamKind::Float => {
                    if (value as f32) == (info.default_value as f32) {
                        value = info.min_value;
                    }
                    ParameterValue::Float(value as f32)
                }
                BridgedParamKind::Int => {
                    value = value.round();
                    if value == info.default_value {
                        value = info.max_value;
                    }
                    ParameterValue::Int(value as i32)
                }
                BridgedParamKind::Bool => ParameterValue::Bool(info.default_value == 0.0),
                BridgedParamKind::FilePath => {
                    panic!("file paths are not realtime primitive controls")
                }
            };
            (info.id, value)
        })
        .collect();
    assert!(!changes.is_empty());
    let borrowed: Vec<_> = changes
        .iter()
        .map(|(id, value)| (id.as_str(), value.clone()))
        .collect();
    check_changed_sync(name, &borrowed, None);
}

macro_rules! all_realtime_case {
    ($test:ident, $name:literal) => {
        #[test]
        fn $test() {
            check_all_realtime_controls($name);
        }
    };
}

all_realtime_case!(channel_all_exported_controls, "ChannelMuteSolo");
all_realtime_case!(transient_all_exported_controls, "TransientShaper");
all_realtime_case!(imager_all_exported_controls, "StereoImager");
all_realtime_case!(compressor_all_exported_controls, "Compressor");
all_realtime_case!(
    multiband_compressor_all_exported_controls,
    "MultibandCompressor"
);
all_realtime_case!(denoiser_all_exported_controls, "Denoiser");
all_realtime_case!(dynamic_eq_all_exported_controls, "DynamicEQ");

#[test]
fn invalid_scalar_writes_preserve_existing_values() {
    for name in [
        "SpeechDenoiser",
        "ChannelMuteSolo",
        "TransientShaper",
        "StereoImager",
    ] {
        let params = DynamicParams::from_infos(&schema_infos(name));
        let mut plugin = super::configuration::create_plugin(name, 48_000.0, &params).unwrap();
        plugin.initialize(48_000.0).unwrap();
        params.sync_to_plugin(plugin.as_mut()).unwrap();
        let snapshot = plugin.parameters();
        let assert_unchanged = |plugin: &dyn sotf_host::Plugin| {
            for parameter in &snapshot {
                assert_eq!(
                    plugin.get_parameter(&parameter.id),
                    Some(parameter.default_value.clone()),
                    "{name}.{}",
                    parameter.id
                );
            }
        };
        assert!(
            plugin
                .set_parameter(
                    ParameterId::from("unknown_parameter"),
                    ParameterValue::Bool(true)
                )
                .is_err()
        );
        assert_unchanged(plugin.as_ref());
        for parameter in &snapshot {
            let invalid = match parameter.default_value {
                ParameterValue::Float(_) => ParameterValue::Bool(true),
                ParameterValue::Bool(_) => ParameterValue::Float(1.0),
                _ => continue,
            };
            assert!(
                plugin.set_parameter(parameter.id.clone(), invalid).is_err(),
                "{name}.{}",
                parameter.id
            );
            assert_unchanged(plugin.as_ref());
            if matches!(parameter.default_value, ParameterValue::Float(_)) {
                for invalid in [f32::NAN, f32::INFINITY, -f32::INFINITY, f32::MAX] {
                    assert!(
                        plugin
                            .set_parameter(parameter.id.clone(), ParameterValue::Float(invalid))
                            .is_err(),
                        "{name}.{}",
                        parameter.id
                    );
                    assert_unchanged(plugin.as_ref());
                }
            }
        }
    }
}

#[test]
fn speech_large_callbacks_are_cold_allocation_free_and_preserve_sentinels() {
    for channels in [1, 2] {
        for bypass in [false, true] {
            let mut plugin =
                plugins_bridge::create_plugin("SpeechDenoiser", channels, 48_000.0, "{}").unwrap();
            plugin.initialize(48_000.0).unwrap();
            let enabled = ParameterId::from("enabled");
            let mut samples = vec![0.0; 4097 * channels + 2];
            for (sample, value) in samples[..4097 * channels].iter_mut().enumerate() {
                *value = input_sample(sample / channels, sample % channels);
            }
            let input = samples[..4097 * channels].to_vec();
            samples[4097 * channels..].copy_from_slice(&[1234.0, -5678.0]);
            let (plugin, samples) = std::thread::spawn(move || {
                assert_no_alloc::assert_no_alloc(|| {
                    // Native sync retains its entry ID; the owned setter gets a
                    // cheap Arc clone whose destruction cannot retire the ID.
                    plugin.set_parameter(enabled.clone(), ParameterValue::Bool(!bypass))?;
                    plugin.process(
                        &input,
                        &mut samples[..4097 * channels],
                        &sotf_host::ProcessContext::new(48_000, 4097),
                    )
                })
                .unwrap();
                (plugin, samples)
            })
            .join()
            .unwrap();
            assert_eq!(&samples[4097 * channels..], &[1234.0, -5678.0]);
            assert_eq!(plugin.latency_samples(), SPEECH_DELAY);
            for (sample, &actual) in samples[..4097 * channels].iter().enumerate() {
                assert!(actual.is_finite());
                if bypass {
                    let expected = sample
                        .checked_sub(SPEECH_DELAY * channels)
                        .map_or(0.0, |source| {
                            input_sample(source / channels, source % channels)
                        });
                    assert_eq!(actual, expected, "channels={channels}, sample={sample}");
                }
            }
        }
    }
}
