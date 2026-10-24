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
    for rate in [8000, 44100, 48000, 96000, 192000] {
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
