// Integration tests for sotf-plugin-analog-limiter — exercises the public API only.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_analog_limiter::{AnalogLimiterPlugin, AnalogLimiterPluginParams};

const SR: u32 = 48_000;

fn make_interleaved_sine(
    freq_hz: f32,
    sample_rate: u32,
    frames: usize,
    channels: usize,
    amplitude: f32,
) -> Vec<f32> {
    let mut buffer = vec![0.0; frames * channels];
    for frame in 0..frames {
        let sample =
            amplitude * (std::f32::consts::TAU * freq_hz * frame as f32 / sample_rate as f32).sin();
        for ch in 0..channels {
            buffer[frame * channels + ch] = sample;
        }
    }
    buffer
}

fn max_abs(buffer: &[f32]) -> f32 {
    buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max)
}

#[test]
fn info_channels_and_core_latency() {
    let mut plugin = AnalogLimiterPlugin::new(2);
    assert_eq!(plugin.channels(), 2);
    let info = plugin.info();
    assert_eq!(info.name, "Analog Limiter");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    plugin.initialize(f64::from(SR)).unwrap();
    // Latency is whatever the wrapped core reports for identical settings:
    // build the core directly as an independent oracle.
    let core = sotf_plugin_limiter::LimiterPlugin::from_params(
        2,
        sotf_plugin_limiter::LimiterPluginParams {
            threshold_db: -0.1,
            release_ms: 50.0,
            lookahead_ms: 5.0,
            soft: false,
            true_peak: false,
            mix: 1.0,
            isp_mode: false,
            dual_release: false,
            feed_forward: false,
            link_amount: 1.0,
            oversampling: 0,
        },
    );
    let mut core = core;
    core.initialize(SR).unwrap();
    assert_eq!(plugin.latency_samples(), core.latency_samples());
}

#[test]
fn hot_signal_is_limited_to_ceiling() {
    let mut plugin = AnalogLimiterPlugin::new(1);
    plugin.initialize(SR).unwrap();
    // +6 dBFS sine into a −0.1 dB ceiling, fully wet, color off.
    let input = make_interleaved_sine(440.0, SR, 4096, 1, 2.0);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, 4096);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    // Skip the lookahead pre-roll, then assert the ceiling holds.
    let settled = &buffer[1024..];
    assert!(
        max_abs(settled) < 1.05,
        "ceiling violated: max={}",
        max_abs(settled)
    );
    assert!(buffer.iter().all(|s| s.is_finite()));
}

#[test]
fn quiet_signal_passes_transparent_with_color_off() {
    let mut plugin = AnalogLimiterPlugin::new(1);
    plugin.initialize(SR).unwrap();
    let input = make_interleaved_sine(440.0, SR, 4096, 1, 0.3);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, 4096);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    // Below threshold: no gain reduction; lookahead delays but preserves level.
    let rms = |b: &[f32]| (b.iter().map(|s| s * s).sum::<f32>() / b.len() as f32).sqrt();
    let ratio = rms(&buffer[1024..]) / rms(&input[1024..]);
    assert!(
        (ratio - 1.0).abs() < 0.02,
        "unexpected GR on quiet signal: {ratio}"
    );
}

#[test]
fn color_stage_adds_character_when_driven() {
    let mut plugin = AnalogLimiterPlugin::new(1);
    plugin.initialize(SR).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("analog_drive"),
            ParameterValue::Float(12.0),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("analog_color"),
            ParameterValue::Float(1.0),
        )
        .unwrap();
    let input = make_interleaved_sine(440.0, SR, 4096, 1, 0.4);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, 4096);
    for _ in 0..4 {
        buffer.copy_from_slice(&input);
        plugin.process_in_place(&mut buffer, &context).unwrap();
    }
    let diff = buffer
        .iter()
        .zip(input.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(diff > 0.01, "driven color stage had no effect");
}

#[test]
fn parameter_roundtrip_and_rejection() {
    // Lookahead is structural: the core rejects post-construction changes
    // ("lookahead changes latency and requires a graph rebuild"), so it is
    // set at construction below, not via set_parameter.
    let params = AnalogLimiterPluginParams {
        lookahead: 10.0,
        ..Default::default()
    };
    let mut plugin = AnalogLimiterPlugin::try_from_params(2, params).unwrap();
    plugin.initialize(SR).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("lookahead")),
        Some(ParameterValue::Float(10.0))
    );
    assert!(
        plugin
            .set_parameter(ParameterId::from("lookahead"), ParameterValue::Float(4.0))
            .is_err(),
        "structural lookahead change must be rejected like the core rejects it"
    );
    // The analog model is likewise structural (replacement allocates and
    // re-prepares on the control thread): live changes are refused, while
    // repeating the committed model stays a no-op success.
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("analog_model"),
                ParameterValue::String("Transformer".to_string())
            )
            .is_err(),
        "structural analog_model change must be rejected on an initialized instance"
    );
    plugin
        .set_parameter(
            ParameterId::from("analog_model"),
            ParameterValue::String("Harmonics".to_string()),
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Harmonics".to_string()))
    );
    let cases: &[(&str, ParameterValue)] = &[
        ("threshold", ParameterValue::Float(-6.0)),
        ("release", ParameterValue::Float(100.0)),
        ("soft", ParameterValue::Bool(true)),
        ("true_peak", ParameterValue::Bool(true)),
        ("mix", ParameterValue::Float(0.8)),
        ("analog_drive", ParameterValue::Float(3.0)),
    ];
    for (id, value) in cases {
        plugin
            .set_parameter(ParameterId::from(*id), value.clone())
            .unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(*id)),
            Some(value.clone())
        );
    }
    // Core ranges still enforced through the wrapper.
    assert!(
        plugin
            .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(5.0))
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("analog_model"),
                ParameterValue::String("Fairchild".to_string())
            )
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(ParameterId::from("isp_mode"), ParameterValue::Bool(true))
            .is_err(),
        "dropped core params must stay unreachable"
    );
}

#[test]
fn from_params_rejects_bad_model_and_zero_channels() {
    let params = AnalogLimiterPluginParams {
        analog_model: "Nope".to_string(),
        ..Default::default()
    };
    assert!(AnalogLimiterPlugin::try_from_params(2, params).is_err());
    assert!(AnalogLimiterPlugin::try_from_params(0, AnalogLimiterPluginParams::default()).is_err());
}

#[test]
fn oversized_blocks_are_chunked() {
    let mut plugin = AnalogLimiterPlugin::new(2);
    plugin.initialize(SR).unwrap();
    let frames = 16384;
    let mut buffer = make_interleaved_sine(440.0, SR, frames, 2, 0.3);
    let context = ProcessContext::new(SR, frames);
    let done = plugin.process_in_place(&mut buffer, &context).unwrap();
    assert_eq!(done, frames);
    assert!(buffer.iter().all(|s| s.is_finite()));
}

#[test]
fn factory_roundtrip_through_json() {
    // The bridge/factory path deserializes params from JSON.
    let params: AnalogLimiterPluginParams =
        serde_json::from_value(serde_json::json!({"threshold": -3.0})).unwrap();
    assert_eq!(params.threshold, -3.0);
    // The legacy core spelling is accepted too.
    let legacy: AnalogLimiterPluginParams =
        serde_json::from_value(serde_json::json!({"threshold_db": -4.0})).unwrap();
    assert_eq!(legacy.threshold, -4.0);
    let plugin = AnalogLimiterPlugin::try_from_params(2, params).unwrap();
    assert_eq!(plugin.channels(), 2);
}

#[test]
fn analog_model_reconstruction_and_bulk_update_preserve_shared_controls() {
    // Structural model: adoption happens at construction (pre-init sets,
    // bulk apply, or params struct), never via live switch. Live bulks
    // carrying a model change are refused atomically with history intact.
    use sotf_host::parametric_plugin::ParameterSet;
    for name in [
        "Harmonics",
        "Static",
        "Hammerstein",
        "Tape",
        "Transformer",
        "Console Preamp",
    ] {
        let controls = [
            ("analog_drive", ParameterValue::Float(12.0)),
            ("analog_color", ParameterValue::Float(0.73)),
            ("analog_character", ParameterValue::Float(0.21)),
            ("analog_trim", ParameterValue::Float(-6.0)),
        ];
        let model = ParameterValue::String(name.to_string());
        let mut expected_plugin = AnalogLimiterPlugin::new(2);
        expected_plugin
            .set_parameter(ParameterId::from("analog_model"), model.clone())
            .unwrap();
        for (id, value) in &controls {
            expected_plugin
                .set_parameter(ParameterId::from(*id), value.clone())
                .unwrap();
        }
        expected_plugin.initialize(SR).unwrap();
        let input = make_interleaved_sine(997.0, SR, 4096, 2, 0.3);
        let mut expected_live = input.clone();
        expected_plugin
            .process_in_place(&mut expected_live, &ProcessContext::new(SR, 4096))
            .unwrap();
        expected_plugin.reset();
        let mut expected = input.clone();
        expected_plugin
            .process_in_place(&mut expected, &ProcessContext::new(SR, 4096))
            .unwrap();

        // Every insertion position places the model among the shared controls;
        // each new HashMap also has an independent iteration seed.
        for model_position in 0..=controls.len() {
            let mut plugin = AnalogLimiterPlugin::new(2);
            let mut values = ParameterSet::new();
            for position in 0..=controls.len() {
                if position == model_position {
                    values.insert(ParameterId::from("analog_model"), model.clone());
                }
                if let Some((id, value)) = controls.get(position) {
                    values.insert(ParameterId::from(*id), value.clone());
                }
            }
            plugin.apply_values(values).unwrap();
            plugin.initialize(SR).unwrap();
            for (id, value) in &controls {
                assert_eq!(
                    plugin.get_parameter(&ParameterId::from(*id)),
                    Some(value.clone())
                );
            }
            let mut actual_live = input.clone();
            plugin
                .process_in_place(&mut actual_live, &ProcessContext::new(SR, 4096))
                .unwrap();
            let live_error = actual_live
                .iter()
                .zip(&expected_live)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(
                live_error, 0.0,
                "model={name}, insertion={model_position}, smoothing"
            );
            plugin.reset();
            let mut actual = input.clone();
            plugin
                .process_in_place(&mut actual, &ProcessContext::new(SR, 4096))
                .unwrap();
            assert_eq!(actual, expected, "model={name}, insertion={model_position}");
        }
        // Rejected bulk atomicity: a live bulk carrying a model change
        // writes nothing (refusal precedes all mutation) and the stream
        // continues byte-identical to an uninterrupted twin.
        let other = if name == "Tape" { "Static" } else { "Tape" }.to_string();
        let setup_committed = || {
            let mut plugin = AnalogLimiterPlugin::new(2);
            plugin
                .set_parameter(ParameterId::from("analog_model"), model.clone())
                .unwrap();
            plugin.initialize(SR).unwrap();
            for (id, value) in &controls {
                plugin
                    .set_parameter(ParameterId::from(*id), value.clone())
                    .unwrap();
            }
            plugin
        };
        let mut twin = setup_committed();
        let mut twin_first = input.clone();
        twin.process_in_place(&mut twin_first, &ProcessContext::new(SR, 4096))
            .unwrap();
        let mut twin_second = input.clone();
        twin.process_in_place(&mut twin_second, &ProcessContext::new(SR, 4096))
            .unwrap();
        let mut refused = setup_committed();
        let mut refused_first = input.clone();
        refused
            .process_in_place(&mut refused_first, &ProcessContext::new(SR, 4096))
            .unwrap();
        assert_eq!(refused_first, twin_first, "model={name}, pre-refusal");
        let snapshot = refused.current_values();
        let mut hostile = ParameterSet::new();
        hostile.insert(
            ParameterId::from("analog_model"),
            ParameterValue::String(other),
        );
        hostile.insert(
            ParameterId::from("analog_drive"),
            ParameterValue::Float(1.0),
        );
        assert!(refused.apply_values(hostile).is_err(), "model={name}");
        assert_eq!(
            refused.current_values(),
            snapshot,
            "model={name}, refused bulk must write nothing"
        );
        let mut refused_second = input.clone();
        refused
            .process_in_place(&mut refused_second, &ProcessContext::new(SR, 4096))
            .unwrap();
        assert_eq!(refused_second, twin_second, "model={name}, history intact");

        // Deterministic reproduction: set all controls first, select the
        // model last — pre-init, through the supported adoption path.
        // Getters must agree with the actual rendered DSP state.
        let mut ordered = AnalogLimiterPlugin::new(2);
        for (id, value) in &controls {
            ordered
                .set_parameter(ParameterId::from(*id), value.clone())
                .unwrap();
        }
        ordered
            .set_parameter(ParameterId::from("analog_model"), model.clone())
            .unwrap();
        ordered.initialize(SR).unwrap();
        ordered.reset();
        let mut actual = input.clone();
        ordered
            .process_in_place(&mut actual, &ProcessContext::new(SR, 4096))
            .unwrap();
        assert_eq!(actual, expected, "model={name}, model last pre-init");

        // Fresh reconstruction adoption: the params-struct path (what the
        // factory deserializes) reproduces the reference for every model.
        let params = AnalogLimiterPluginParams {
            analog_model: name.to_string(),
            analog_drive: 12.0,
            analog_color: 0.73,
            analog_character: 0.21,
            analog_trim: -6.0,
            ..Default::default()
        };
        let mut reconstructed = AnalogLimiterPlugin::try_from_params(2, params).unwrap();
        reconstructed.initialize(SR).unwrap();
        reconstructed.reset();
        let mut actual = input.clone();
        reconstructed
            .process_in_place(&mut actual, &ProcessContext::new(SR, 4096))
            .unwrap();
        assert_eq!(actual, expected, "model={name}, reconstructed");
    }
}
