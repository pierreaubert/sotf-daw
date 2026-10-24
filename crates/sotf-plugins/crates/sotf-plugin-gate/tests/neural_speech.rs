//! Explicit speech protection preserves the classical baseline and host contracts.

use sotf_host::CountingAlloc;
#[cfg(feature = "neural-speech")]
use sotf_host::{ParametricInPlacePlugin, ProcessContext, assert_no_allocs};
#[cfg(feature = "neural-speech")]
use sotf_plugin_gate::GateData;
use sotf_plugin_gate::{GateMode, GatePlugin, GatePluginParams, SpeechProtectionConfig};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn params() -> GatePluginParams {
    GatePluginParams {
        speech_protection: Some(SpeechProtectionConfig::default()),
        threshold_db: -10.0,
        attack_ms: 1.0,
        release_ms: 10.0,
        hold_ms: 0.0,
        range_db: 40.0,
        ..GatePluginParams::default()
    }
}

#[test]
fn invalid_policies_and_unsupported_modes_are_rejected() {
    for config in [
        SpeechProtectionConfig {
            min_probability: f32::NAN,
            ..Default::default()
        },
        SpeechProtectionConfig {
            min_probability: 0.49,
            ..Default::default()
        },
        SpeechProtectionConfig {
            max_age_ms: 119.0,
            ..Default::default()
        },
        SpeechProtectionConfig {
            max_age_ms: f32::INFINITY,
            ..Default::default()
        },
        SpeechProtectionConfig {
            source_channel: 2,
            ..Default::default()
        },
    ] {
        assert!(
            GatePlugin::try_from_params(
                2,
                GatePluginParams {
                    speech_protection: Some(config),
                    ..params()
                }
            )
            .is_err()
        );
    }
    for mode in [GateMode::Upward, GateMode::Duck] {
        assert!(GatePlugin::try_from_params(2, GatePluginParams { mode, ..params() }).is_err());
    }
    assert!(
        GatePlugin::try_from_params(
            2,
            GatePluginParams {
                link_channels: false,
                ..params()
            }
        )
        .is_err()
    );
}

#[cfg(not(feature = "neural-speech"))]
#[test]
fn unavailable_feature_rejects_explicit_request_instead_of_ignoring_it() {
    let error = GatePlugin::try_from_params(2, params()).err().unwrap();
    assert!(error.contains("neural-speech"));
}

#[cfg(feature = "neural-speech")]
fn data(gate: &GatePlugin) -> sotf_plugin_gate::SpeechProtectionData {
    gate.get_data()
        .unwrap()
        .downcast::<GateData>()
        .unwrap()
        .speech_protection
        .unwrap()
}

#[cfg(feature = "neural-speech")]
#[test]
fn cold_callbacks_and_reset_are_heap_free_and_warmup_matches_classical() {
    for rate in [8000_u32, 44100, 48000, 96000, 192000] {
        let settings = GatePluginParams {
            lookahead_ms: 5.0,
            ..params()
        };
        let mut protected = GatePlugin::try_from_params(2, settings.clone()).unwrap();
        let mut baseline_params = settings;
        baseline_params.speech_protection = None;
        let mut baseline = GatePlugin::try_from_params(2, baseline_params).unwrap();
        protected.initialize(f64::from(rate)).unwrap();
        baseline.initialize(f64::from(rate)).unwrap();
        assert_eq!(protected.latency_samples(), baseline.latency_samples());
        // Stay inside the full-window warmup; no inference can affect this block.
        let frames = rate as usize / 20;
        let mut actual = vec![0.02; frames * 2];
        let mut expected = actual.clone();
        let context = ProcessContext::new(rate, frames);
        assert_no_allocs("Gate neural cold callback", || {
            protected.process_in_place(&mut actual, &context).unwrap();
        });
        baseline.process_in_place(&mut expected, &context).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(data(&protected).submitted_windows, 0);
        assert_no_allocs("Gate neural reset", || protected.reset());
        assert_eq!(data(&protected).protection_mix, 0.0);
        assert_eq!(data(&protected).observed_windows, 0);
    }
}

#[cfg(feature = "neural-speech")]
#[test]
fn incompatible_setters_and_failed_reinitialize_preserve_the_running_gate() {
    use sotf_host::{ParameterId, ParameterValue};

    let mut gate = GatePlugin::try_from_params(2, params()).unwrap();
    for (key, value) in [
        ("mode", ParameterValue::Int(1)),
        ("link_channels", ParameterValue::Bool(false)),
    ] {
        let id = ParameterId::from(key);
        let previous = gate.parametric_get_parameter(&id);
        assert!(gate.parametric_set_parameter(id.clone(), value).is_err());
        assert_eq!(gate.parametric_get_parameter(&id), previous);
    }
    gate.initialize(48000.0).unwrap();
    let context = ProcessContext::new(48000, 137);
    let mut first = [0.025; 274];
    gate.process_in_place(&mut first, &context).unwrap();
    let mut control = GatePlugin::try_from_params(2, params()).unwrap();
    control.initialize(48000.0).unwrap();
    let mut control_first = [0.025; 274];
    control
        .process_in_place(&mut control_first, &context)
        .unwrap();
    assert_eq!(first, control_first);
    assert!(gate.initialize(384000.0).is_err());
    let mut actual = [0.025; 274];
    let mut expected = actual;
    gate.process_in_place(&mut actual, &context).unwrap();
    control.process_in_place(&mut expected, &context).unwrap();
    assert_eq!(actual, expected);
}

#[cfg(feature = "neural-speech")]
#[test]
fn worker_observations_cannot_deepen_attenuation_or_modify_sidechain() {
    let mut settings = params();
    settings.sidechain_external = true;
    let mut baseline_settings = settings.clone();
    baseline_settings.speech_protection = None;
    let mut protected = GatePlugin::try_from_params(2, settings).unwrap();
    let mut baseline = GatePlugin::try_from_params(2, baseline_settings).unwrap();
    protected.initialize(48000.0).unwrap();
    baseline.initialize(48000.0).unwrap();
    let mut observed = 0;
    let mut peak_mix = 0.0_f32;
    let mut source = vec![0.0; 480 * 4];
    let mut actual = source.clone();
    let mut expected = source.clone();
    for block in 0..80 {
        for (index, frame) in source.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let phase = (block * 480 + index) as f64 * 2.0 * std::f64::consts::PI * 180.0 / 48000.0;
            let signal = (0.25 * phase.sin() + 0.08 * (2.0 * phase).sin()) as f32;
            frame.copy_from_slice(&[signal, -0.5 * signal, 0.001, -0.002]);
        }
        actual.copy_from_slice(&source);
        expected.copy_from_slice(&source);
        let context = ProcessContext::new(48000, 480);
        assert_no_allocs("Gate neural active callback", || {
            protected.process_in_place(&mut actual, &context).unwrap();
        });
        baseline.process_in_place(&mut expected, &context).unwrap();
        for ((output, classical), input) in actual
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0.iter())
            .zip(source.as_chunks::<4>().0.iter())
        {
            assert_eq!(&output[2..], &input[2..]);
            assert!((output[1] + 0.5 * output[0]).abs() < 1e-7);
            for channel in 0..2 {
                assert!(output[channel].abs() + 1e-7 >= classical[channel].abs());
                assert!(output[channel].abs() <= input[channel].abs() + 1e-7);
            }
        }
        let status = data(&protected);
        observed = observed.max(status.observed_windows);
        peak_mix = peak_mix.max(status.protection_mix);
        // Test scheduler pacing only; the callback never waits for the worker.
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    eprintln!("Gate learned observations={observed}, maximum protection={peak_mix}");
    assert!(observed > 0, "no actual model output reached the gate");
    assert!(
        peak_mix > 0.1,
        "mode remained a permanent classical fallback"
    );
    assert_no_allocs("Gate neural active reset", || protected.reset());
    let reset = data(&protected);
    assert_eq!(reset.model_score, None);
    assert_eq!(reset.protection_mix, 0.0);
    assert_eq!(reset.submitted_windows, 0);
}

fn silero_params() -> GatePluginParams {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../models/silero-vad/silero_vad_16k_batch1.onnx");
    let mut settings = params();
    settings.speech_protection.as_mut().unwrap().model =
        sotf_plugin_gate::SpeechProtectionModel::Silero {
            model_path: path,
            sha256: sotf_plugin_gate::SILERO_MODEL_SHA256.to_owned(),
        };
    settings
}

#[cfg(not(feature = "silero-speech"))]
#[test]
fn missing_silero_runtime_feature_rejects_the_explicit_model() {
    let error = GatePlugin::try_from_params(2, silero_params())
        .err()
        .unwrap();
    assert!(error.contains("silero-speech"));
}

#[cfg(feature = "silero-speech")]
#[test]
fn silero_cold_callbacks_and_reset_are_heap_free_with_unchanged_lookahead() {
    for rate in [8000_u32, 44100, 48000, 96000, 192000] {
        let mut settings = silero_params();
        settings.lookahead_ms = 5.0;
        let mut baseline_settings = settings.clone();
        baseline_settings.speech_protection = None;
        let mut gate = GatePlugin::try_from_params(2, settings).unwrap();
        let mut baseline = GatePlugin::try_from_params(2, baseline_settings).unwrap();
        gate.initialize(f64::from(rate)).unwrap();
        baseline.initialize(f64::from(rate)).unwrap();
        assert_eq!(gate.latency_samples(), baseline.latency_samples());
        let frames = rate as usize / 20;
        let mut actual = vec![0.025; frames * 2];
        let mut expected = actual.clone();
        let context = ProcessContext::new(rate, frames);
        assert_no_allocs("Silero cold audio callback", || {
            gate.process_in_place(&mut actual, &context).unwrap();
        });
        baseline.process_in_place(&mut expected, &context).unwrap();
        assert_eq!(actual, expected);
        assert!(!data(&gate).warmup_complete);
        assert_no_allocs("Silero cold reset", || gate.reset());
        assert_eq!(data(&gate).model_score, None);
    }
}

#[cfg(feature = "silero-speech")]
#[test]
fn actual_silero_speech_protects_linked_program_without_heap_activity_or_key_mutation() {
    let bytes =
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm");
    let speech: Vec<f32> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|x| f32::from(i16::from_le_bytes(*x)) / 32768.0)
        .collect();
    let mut settings = silero_params();
    settings.sidechain_external = true;
    let mut baseline_settings = settings.clone();
    baseline_settings.speech_protection = None;
    let mut gate = GatePlugin::try_from_params(2, settings).unwrap();
    let mut baseline = GatePlugin::try_from_params(2, baseline_settings).unwrap();
    gate.initialize(48000.0).unwrap();
    baseline.initialize(48000.0).unwrap();
    let mut input = [0.0; 1920];
    let mut actual = input;
    let mut expected = input;
    let context = ProcessContext::new(48000, 480);
    let mut peak_mix = 0.0_f32;
    let mut observed = 0;
    let mut maximum_age = 0.0_f32;
    for (block, audio) in speech.as_chunks::<480>().0.iter().enumerate() {
        for (frame, &source) in input.as_chunks_mut::<4>().0.iter_mut().zip(audio) {
            frame.copy_from_slice(&[source, -0.5 * source, 0.001, -0.002]);
        }
        actual.copy_from_slice(&input);
        expected.copy_from_slice(&input);
        assert_no_allocs("Silero active audio callback", || {
            gate.process_in_place(&mut actual, &context).unwrap();
        });
        baseline.process_in_place(&mut expected, &context).unwrap();
        for ((output, classical), source) in actual
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0.iter())
            .zip(input.as_chunks::<4>().0.iter())
        {
            assert_eq!(&output[2..], &source[2..]);
            assert!((output[1] + 0.5 * output[0]).abs() < 1e-7);
            for channel in 0..2 {
                assert!(output[channel].abs() + 1e-7 >= classical[channel].abs());
                assert!(output[channel].abs() <= source[channel].abs() + 1e-7);
            }
        }
        if block < 12 {
            assert_eq!(actual, expected);
        }
        let status = data(&gate);
        peak_mix = peak_mix.max(status.protection_mix);
        observed = observed.max(status.observed_windows);
        if status.observation_usable {
            assert!(status.warmup_complete && status.model_frames >= 4);
            maximum_age = maximum_age.max(status.source_age_ms.unwrap());
            assert!(status.source_age_ms.unwrap() <= 150.0);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    eprintln!(
        "Silero actual speech: observed={observed}, peak_mix={peak_mix}, usable_max_age_ms={maximum_age}"
    );
    assert!(observed > 0 && peak_mix > 0.1);
    let before = data(&gate);
    assert!(gate.initialize(384000.0).is_err());
    let after = data(&gate);
    assert_eq!(after.model_frames, before.model_frames);
    assert_eq!(after.model_score, before.model_score);
    assert_eq!(after.submitted_windows, before.submitted_windows);
    assert_no_allocs("Silero active reset", || gate.reset());
    baseline.reset();
    assert_eq!(data(&gate).model_score, None);
    let mut actual = [0.02; 1920];
    let mut expected = actual;
    gate.process_in_place(&mut actual, &context).unwrap();
    baseline.process_in_place(&mut expected, &context).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(data(&gate).protection_mix, 0.0);
}

#[cfg(feature = "silero-speech")]
#[test]
fn silero_backpressure_retires_observations_and_reset_stays_heap_free() {
    let mut gate = GatePlugin::try_from_params(2, silero_params()).unwrap();
    gate.initialize(48000.0).unwrap();
    let context = ProcessContext::new(48000, 480);
    let mut audio = [0.0; 960];
    // Deliberately outrun wall-clock serving. This is a backpressure test,
    // not a DAW deadline or model throughput benchmark.
    for _ in 0..1000 {
        assert_no_allocs("Silero backpressure callback", || {
            gate.process_in_place(&mut audio, &context).unwrap();
        });
    }
    let status = data(&gate);
    assert!(
        status.dropped_windows > 0,
        "test did not exercise backpressure"
    );
    assert_eq!(status.protection_mix, 0.0);
    eprintln!(
        "Silero backpressure: submitted={}, dropped={}",
        status.submitted_windows, status.dropped_windows
    );
    assert_no_allocs("Silero backpressure reset", || gate.reset());
    let status = data(&gate);
    assert_eq!(status.model_score, None);
    assert_eq!(status.dropped_windows, 0);
    assert!(!status.warmup_complete);
}

#[cfg(feature = "silero-speech")]
#[test]
fn causal_silero_worker_preserves_classical_output_on_recorded_instrument_controls() {
    let fixtures: [(&str, &[u8]); 2] = [
        (
            "violin",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm"),
        ),
        (
            "cymbal",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/cymbal.pcm"),
        ),
    ];
    for (name, bytes) in fixtures {
        let source: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| f32::from(i16::from_le_bytes(*v)) / 32768.0)
            .collect();
        let mut settings = silero_params();
        settings.sidechain_external = true;
        let mut baseline_settings = settings.clone();
        baseline_settings.speech_protection = None;
        let mut gate = GatePlugin::try_from_params(2, settings).unwrap();
        let mut baseline = GatePlugin::try_from_params(2, baseline_settings).unwrap();
        gate.initialize(48000.0).unwrap();
        baseline.initialize(48000.0).unwrap();
        let context = ProcessContext::new(48000, 480);
        let mut input = [0.0; 1920];
        let mut actual = input;
        let mut expected = input;
        let mut peak_score = 0.0_f32;
        let mut ready = false;
        let mut observed = 0;
        for block in source.as_chunks::<480>().0 {
            for (frame, &sample) in input.as_chunks_mut::<4>().0.iter_mut().zip(block) {
                frame.copy_from_slice(&[sample, -0.5 * sample, 0.001, -0.002]);
            }
            actual.copy_from_slice(&input);
            expected.copy_from_slice(&input);
            assert_no_allocs("Silero instrument callback", || {
                gate.process_in_place(&mut actual, &context).unwrap();
            });
            baseline.process_in_place(&mut expected, &context).unwrap();
            assert_eq!(actual, expected, "unexpected speech protection on {name}");
            let status = data(&gate);
            peak_score = peak_score.max(status.model_score.unwrap_or(0.0));
            ready |= status.warmup_complete;
            observed = observed.max(status.observed_windows);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            ready && observed > 16,
            "control did not exercise a serving model"
        );
        eprintln!(
            "Silero causal control {name}: peak_observed_score={peak_score}, observed={observed}"
        );
    }
}
