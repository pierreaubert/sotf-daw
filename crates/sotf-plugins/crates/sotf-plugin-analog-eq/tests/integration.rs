// Integration tests for sotf-plugin-analog-eq — exercises the public API only.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_analog_eq::{AnalogEqPlugin, AnalogEqPluginParams};

const SR: u32 = 48_000;
const FRAMES: usize = 512;

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

fn rms(buffer: &[f32]) -> f32 {
    (buffer.iter().map(|s| s * s).sum::<f32>() / buffer.len().max(1) as f32).sqrt()
}

#[test]
fn info_and_channels_match_construction() {
    let plugin = AnalogEqPlugin::new(2);
    assert_eq!(plugin.channels(), 2);
    let info = plugin.info();
    assert_eq!(info.name, "Analog EQ");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    // No added latency: biquads and the color stage are zero-latency.
    assert_eq!(plugin.latency_samples(), 0);
}

#[test]
fn default_state_is_neutral_eq_with_color_off() {
    let mut plugin = AnalogEqPlugin::new(2);
    plugin.initialize(SR).unwrap();
    let input = make_interleaved_sine(440.0, SR, FRAMES, 2, 0.5);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    // Flat bands + color 0% must be transparent.
    for (got, want) in buffer.iter().zip(input.iter()) {
        assert!(
            (got - want).abs() < 1e-5,
            "default state not transparent: {got} != {want}"
        );
    }
}

#[test]
fn peak_band_boosts_and_cuts() {
    let mut plugin = AnalogEqPlugin::new(1);
    plugin.initialize(SR).unwrap();
    plugin
        .set_parameter(ParameterId::from("mid1_gain"), ParameterValue::Float(12.0))
        .unwrap();
    // One long continuous block; measure the settled second half.
    let frames = 4096;
    let input = make_interleaved_sine(800.0, SR, frames, 1, 0.5);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, frames);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    let ratio = rms(&buffer[frames / 2..]) / rms(&input[frames / 2..]);
    assert!(
        (ratio - 10.0f32.powf(12.0 / 20.0)).abs() < 0.3,
        "expected ~+12 dB at band center, ratio={ratio}"
    );

    plugin
        .set_parameter(ParameterId::from("mid1_gain"), ParameterValue::Float(-12.0))
        .unwrap();
    plugin.reset();
    buffer.copy_from_slice(&input);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    let ratio = rms(&buffer[frames / 2..]) / rms(&input[frames / 2..]);
    assert!(
        (ratio - 10.0f32.powf(-12.0 / 20.0)).abs() < 0.1,
        "expected ~-12 dB at band center, ratio={ratio}"
    );
}

#[test]
fn color_stage_adds_harmonics_when_enabled() {
    let mut plugin = AnalogEqPlugin::new(1);
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
    let input = make_interleaved_sine(440.0, SR, FRAMES, 1, 0.4);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    for _ in 0..16 {
        buffer.copy_from_slice(&input);
        plugin.process_in_place(&mut buffer, &context).unwrap();
    }
    // Driven color must audibly change the signal (harmonics + level).
    let diff = buffer
        .iter()
        .zip(input.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(diff > 0.01, "driven color stage had no effect");
    assert!(buffer.iter().all(|s| s.is_finite()));
}

#[test]
fn parameter_roundtrip_and_rejection() {
    let mut plugin = AnalogEqPlugin::new(2);
    plugin.initialize(SR).unwrap();
    let cases: &[(&str, ParameterValue)] = &[
        ("low_freq", ParameterValue::Float(200.0)),
        ("mid1_q", ParameterValue::Float(2.5)),
        ("high_gain", ParameterValue::Float(-6.0)),
        ("analog_model", ParameterValue::String("Tape".to_string())),
        ("analog_drive", ParameterValue::Float(6.0)),
        ("analog_color", ParameterValue::Float(0.5)),
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
    // Out of range.
    assert!(
        plugin
            .set_parameter(ParameterId::from("mid1_q"), ParameterValue::Float(99.0))
            .is_err()
    );
    // Unknown model name is rejected, previous model kept.
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("analog_model"),
                ParameterValue::String("Pultec".to_string())
            )
            .is_err()
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
    // Unknown parameter id is rejected.
    assert!(
        plugin
            .set_parameter(ParameterId::from("nope"), ParameterValue::Float(1.0))
            .is_err()
    );
}

#[test]
fn from_params_rejects_bad_model_and_zero_channels() {
    let params = AnalogEqPluginParams {
        analog_model: "Nope".to_string(),
        ..Default::default()
    };
    assert!(AnalogEqPlugin::try_from_params(2, params).is_err());
    assert!(AnalogEqPlugin::try_from_params(0, AnalogEqPluginParams::default()).is_err());
}

#[test]
fn oversized_blocks_are_chunked() {
    let mut plugin = AnalogEqPlugin::new(2);
    plugin.initialize(SR).unwrap();
    // 16384 frames exceeds the 8192 prepared ceiling; the stage chunks it.
    let frames = 16384;
    let mut buffer = make_interleaved_sine(440.0, SR, frames, 2, 0.3);
    let context = ProcessContext::new(SR, frames);
    let done = plugin.process_in_place(&mut buffer, &context).unwrap();
    assert_eq!(done, frames);
    assert!(buffer.iter().all(|s| s.is_finite()));
}

#[test]
fn reset_clears_state() {
    let mut plugin = AnalogEqPlugin::new(1);
    plugin.initialize(SR).unwrap();
    plugin
        .set_parameter(ParameterId::from("mid1_gain"), ParameterValue::Float(12.0))
        .unwrap();
    let input = make_interleaved_sine(800.0, SR, FRAMES, 1, 0.5);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    plugin.reset();
    // After reset with identical params, output matches a fresh plugin.
    let mut fresh = AnalogEqPlugin::new(1);
    fresh.initialize(SR).unwrap();
    fresh
        .set_parameter(ParameterId::from("mid1_gain"), ParameterValue::Float(12.0))
        .unwrap();
    let mut expected = input.clone();
    fresh.process_in_place(&mut expected, &context).unwrap();
    let mut buffer2 = input.clone();
    plugin.process_in_place(&mut buffer2, &context).unwrap();
    for (a, b) in buffer2.iter().zip(expected.iter()) {
        assert!((a - b).abs() < 1e-6);
    }
}

#[test]
fn analog_model_switch_and_bulk_update_preserve_shared_controls() {
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
        let mut expected_plugin = AnalogEqPlugin::new(2);
        expected_plugin.initialize(SR).unwrap();
        expected_plugin
            .set_parameter(ParameterId::from("analog_model"), model.clone())
            .unwrap();
        for (id, value) in &controls {
            expected_plugin
                .set_parameter(ParameterId::from(*id), value.clone())
                .unwrap();
        }
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
            let mut plugin = AnalogEqPlugin::new(2);
            plugin.initialize(SR).unwrap();
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
        // Deterministic reproduction: set all controls first, select another
        // model last. Getters must agree with the actual rendered DSP state.
        let mut switched = AnalogEqPlugin::new(2);
        switched.initialize(SR).unwrap();
        switched
            .set_parameter(
                ParameterId::from("analog_model"),
                ParameterValue::String(if name == "Tape" { "Static" } else { "Tape" }.to_string()),
            )
            .unwrap();
        for (id, value) in &controls {
            switched
                .set_parameter(ParameterId::from(*id), value.clone())
                .unwrap();
        }
        switched
            .set_parameter(ParameterId::from("analog_model"), model)
            .unwrap();
        switched.reset();
        let mut actual = input.clone();
        switched
            .process_in_place(&mut actual, &ProcessContext::new(SR, 4096))
            .unwrap();
        assert_eq!(actual, expected, "model={name}, switched last");
    }
}
