//! Whole-callback Gate processing, compact output, precision, and heap regressions.

// Rust guideline compliant 2026-02-21
use sotf_host::{
    CountingAlloc, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, Plugin, ProcessContext,
    assert_no_allocs,
};
#[cfg(feature = "neural-speech")]
use sotf_host::{InPlacePlugin, InPlacePluginAdapter};
use sotf_plugin_gate::{GateMode, GatePlugin, GatePluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn same_audio(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    let first = actual
        .iter()
        .zip(expected)
        .enumerate()
        .find(|(_, (a, b))| a.to_bits() != b.to_bits());
    assert!(first.is_none(), "first audio mismatch: {first:?}");
}

#[test]
fn direct_buffers_match_wide_in_place_for_every_classical_mode_and_f32_precision_f64() {
    let frames = 4099;
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        for channels in [1, 2, 5] {
            for external in [false, true] {
                for linked in [false, true] {
                    for detection in ["Peak", "RMS"] {
                        let settings = GatePluginParams {
                            mode,
                            sidechain_external: external,
                            link_channels: linked,
                            detection_mode: detection.into(),
                            lookahead_ms: 5.0,
                            sidechain_hpf_hz: 90.0,
                            threshold_db: -24.0,
                            knee_db: 6.0,
                            mix: 0.7,
                            ..GatePluginParams::default()
                        };
                        let stride = channels * if external { 2 } else { 1 };
                        let input: Vec<f32> = (0..frames * stride)
                            .map(|n| {
                                ((n as f32 * 0.113).sin() * 0.13)
                                    * if n / stride % 83 < 40 { 1.0 } else { 0.03 }
                            })
                            .collect();
                        let input64: Vec<f64> = input
                            .iter()
                            .map(|&x| f64::from(x) + 2.0_f64.powi(-50))
                            .collect();
                        let quantized: Vec<f32> = input64.iter().map(|&x| x as f32).collect();
                        let mut wide = input.clone();
                        let mut reference =
                            GatePlugin::try_from_params(channels, settings.clone()).unwrap();
                        reference.initialize(48_000.0).unwrap();
                        let context = ProcessContext::new(48_000, frames);
                        reference.process_in_place(&mut wide, &context).unwrap();
                        let expected: Vec<f32> = wide
                            .chunks_exact(stride)
                            .flat_map(|frame| frame[..channels].iter().copied())
                            .collect();
                        let mut gate =
                            GatePlugin::try_from_params(channels, settings.clone()).unwrap();
                        gate.initialize(48_000.0).unwrap();
                        assert!(gate.supports_bounded_subdivision());
                        let mut actual = vec![f32::NAN; frames * channels];
                        assert_no_allocs("direct classical f32 callback", || {
                            assert_eq!(
                                gate.process_out_of_place(&input, &mut actual, &context)
                                    .unwrap(),
                                frames
                            );
                        });
                        same_audio(&actual, &expected);
                        gate.reset();
                        reference.reset();
                        let mut quantized_wide = quantized;
                        reference
                            .process_in_place(&mut quantized_wide, &context)
                            .unwrap();
                        let expected64: Vec<f64> = quantized_wide
                            .chunks_exact(stride)
                            .flat_map(|frame| frame[..channels].iter().map(|&x| f64::from(x)))
                            .collect();
                        let mut actual64 = vec![f64::NAN; frames * channels];
                        let saved = input64.clone();
                        assert_no_allocs("direct classical f64 callback", || {
                            assert_eq!(
                                gate.process_out_of_place_f64(&input64, &mut actual64, &context)
                                    .unwrap(),
                                frames
                            );
                        });
                        assert_eq!(input64, saved);
                        let first = actual64
                            .iter()
                            .zip(&expected64)
                            .enumerate()
                            .find(|(_, (a, b))| a.to_bits() != b.to_bits());
                        assert!(
                            first.is_none(),
                            "f64 mismatch: mode={mode:?}, channels={channels}, external={external}, linked={linked}, detection={detection}: {first:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn f64_range_errors_leave_output_and_following_signal_history_pristine() {
    let settings = GatePluginParams {
        sidechain_external: true,
        lookahead_ms: 5.0,
        ..Default::default()
    };
    let mut plugin = ParametricInPlacePluginAdapter::new(
        GatePlugin::try_from_params(2, settings.clone()).unwrap(),
    );
    let mut control =
        ParametricInPlacePluginAdapter::new(GatePlugin::try_from_params(2, settings).unwrap());
    Plugin::initialize(&mut plugin, 48_000.0).unwrap();
    Plugin::initialize(&mut control, 48_000.0).unwrap();
    let frames = 1025;
    let mut input = vec![0.13_f64; frames * 4];
    let mut output = vec![0.17; frames * 2];
    let saved = output.clone();
    input[frames * 4 - 1] = f64::MAX;
    let context = ProcessContext::new(48_000, frames);
    assert!(plugin.process_f64(&input, &mut output, &context).is_err());
    assert_eq!(output, saved);
    input[frames * 4 - 1] = 0.13;
    let mut expected = output.clone();
    plugin.process_f64(&input, &mut output, &context).unwrap();
    control
        .process_f64(&input, &mut expected, &context)
        .unwrap();
    assert_eq!(output, expected);
}

#[cfg(feature = "neural-speech")]
fn speech_settings(silero: bool) -> GatePluginParams {
    let mut policy = sotf_plugin_gate::SpeechProtectionConfig::default();
    if silero {
        policy.model = sotf_plugin_gate::SpeechProtectionModel::Silero {
            model_path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../models/silero-vad/silero_vad_16k_batch1.onnx"),
            sha256: sotf_plugin_gate::SILERO_MODEL_SHA256.into(),
        };
    }
    GatePluginParams {
        speech_protection: Some(policy),
        sidechain_external: true,
        threshold_db: -10.0,
        lookahead_ms: 5.0,
        ..Default::default()
    }
}

#[cfg(feature = "neural-speech")]
#[test]
fn realtime_speech_adapters_accept_large_cold_f32_and_f64_callbacks_without_heap_or_key_mutation() {
    #[cfg(feature = "silero-speech")]
    let models = [false, true];
    #[cfg(not(feature = "silero-speech"))]
    let models = [false];
    let frames = 65_539;
    let input32: Vec<f32> = (0..frames)
        .flat_map(|n| {
            let value = (n as f32 * 0.027).sin() * 0.1;
            [value, -0.5 * value, 0.001, -0.002]
        })
        .collect();
    let input64: Vec<f64> = input32
        .iter()
        .map(|&x| f64::from(x) + 2.0_f64.powi(-50))
        .collect();
    let saved32 = input32.clone();
    let saved64 = input64.clone();
    for silero in models {
        for view in 0..2 {
            for double in [false, true] {
                let gate = GatePlugin::try_from_params(2, speech_settings(silero)).unwrap();
                assert!(!gate.supports_bounded_subdivision());
                let adapted = ParametricInPlacePluginAdapter::new(gate);
                let mut plugin: Box<dyn Plugin> = if view == 0 {
                    Box::new(adapted)
                } else {
                    Box::new(InPlacePluginAdapter::new(adapted))
                };
                plugin.initialize(48_000.0).unwrap();
                assert_eq!(plugin.latency_samples(), 240);
                let context = ProcessContext::new(48_000, frames);
                if double {
                    let mut output = vec![0.0; frames * 2];
                    assert_no_allocs("realtime learned direct f64 callback", || {
                        assert_eq!(
                            plugin.process_f64(&input64, &mut output, &context).unwrap(),
                            frames
                        );
                        plugin.reset();
                    });
                    assert!(output.iter().all(|&x| x.is_finite() && x.abs() <= 0.10001));
                } else {
                    let mut output = vec![0.0; frames * 2];
                    assert_no_allocs("realtime learned direct f32 callback", || {
                        assert_eq!(
                            plugin.process(&input32, &mut output, &context).unwrap(),
                            frames
                        );
                        plugin.reset();
                    });
                    assert!(output.iter().all(|&x| x.is_finite() && x.abs() <= 0.10001));
                }
            }
        }
    }
    assert_eq!(input32, saved32);
    assert_eq!(input64, saved64);
}

#[cfg(feature = "neural-speech")]
#[test]
fn speech_preparation_mode_controls_subdivision_claim_and_wide_f64_keys_stay_exact() {
    use sotf_host::ProcessingMode;
    let mut gate = GatePlugin::try_from_params(2, speech_settings(false)).unwrap();
    gate.initialize_with_mode(48_000.0, ProcessingMode::Offline)
        .unwrap();
    assert!(gate.supports_bounded_subdivision());
    gate.reset();
    assert!(gate.supports_bounded_subdivision());
    gate.initialize(48_000.0).unwrap();
    assert!(!gate.supports_bounded_subdivision());
    let mut adapted = ParametricInPlacePluginAdapter::new(gate);
    InPlacePlugin::initialize(&mut adapted, 48_000.0).unwrap();
    let frames = 4099;
    let mut input: Vec<f64> = (0..frames)
        .flat_map(|_| {
            [
                0.1,
                -0.05,
                0.001 + 2.0_f64.powi(-45),
                -0.002 - 2.0_f64.powi(-47),
            ]
        })
        .collect();
    let saved = input.clone();
    assert_no_allocs("direct wide f64 fallback", || {
        assert_eq!(
            InPlacePlugin::process_in_place_f64(
                &mut adapted,
                &mut input,
                &ProcessContext::new(48_000, frames)
            )
            .unwrap(),
            frames
        );
    });
    for (actual, source) in input
        .as_chunks::<4>()
        .0
        .iter()
        .zip(saved.as_chunks::<4>().0)
    {
        assert_eq!(&actual[2..], &source[2..]);
    }
}

#[test]
fn native_entry_points_retain_lookahead_until_eos_and_require_reset_for_new_input() {
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        for route in 0..3 {
            let settings = GatePluginParams {
                mode,
                sidechain_external: true,
                lookahead_ms: 5.0,
                mix: 0.0,
                ..Default::default()
            };
            let mut gate = GatePlugin::try_from_params(2, settings).unwrap();
            gate.initialize(48_000.0).unwrap();
            let frames = 64;
            let mut input32 = vec![0.0; frames * 4];
            input32[(frames - 1) * 4] = 0.25;
            input32[(frames - 1) * 4 + 1] = -0.125;
            let input64: Vec<f64> = input32.iter().copied().map(f64::from).collect();
            let context = ProcessContext::new(48_000, frames);
            let mut output32 = vec![1.0; frames * 2];
            let mut output64 = vec![1.0; frames * 2];
            let mut wide64 = input64.clone();
            assert_no_allocs("native input retains lookahead state", || match route {
                0 => {
                    gate.process_out_of_place(&input32, &mut output32, &context)
                        .unwrap();
                }
                1 => {
                    gate.process_out_of_place_f64(&input64, &mut output64, &context)
                        .unwrap();
                }
                _ => {
                    gate.process_in_place_f64(&mut wide64, &context).unwrap();
                }
            });
            assert_eq!(gate.latency_samples(), 240);
            let mut tail = Vec::with_capacity(480);
            let mut scratch = [0.0; 34];
            assert_no_allocs("native retained lookahead drain", || {
                loop {
                    let result = gate
                        .drain(&mut scratch, &ProcessContext::new(48_000, 0))
                        .unwrap();
                    tail.extend_from_slice(&scratch[..result.frames * 2]);
                    if result.complete {
                        break;
                    }
                }
            });
            assert_eq!(tail.len(), 480);
            assert!(tail[..478].iter().all(|&value| value == 0.0));
            assert_eq!(&tail[478..], &[0.25, -0.125]);
            output32.fill(0.17);
            output64.fill(0.17);
            let saved_wide = wide64.clone();
            let rejected = match route {
                0 => gate.process_out_of_place(&input32, &mut output32, &context),
                1 => gate.process_out_of_place_f64(&input64, &mut output64, &context),
                _ => gate.process_in_place_f64(&mut wide64, &context),
            };
            assert!(rejected.is_err());
            assert!(output32.iter().all(|&x| x == 0.17));
            assert!(output64.iter().all(|&x| x == 0.17));
            assert_eq!(wide64, saved_wide);
            gate.reset();
            assert_eq!(
                gate.process_out_of_place(&input32, &mut output32, &context)
                    .unwrap(),
                frames
            );
        }
    }
}
