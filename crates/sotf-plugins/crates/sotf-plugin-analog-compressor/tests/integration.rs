// Integration tests for sotf-plugin-analog-compressor — exercises the public API only.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_analog_compressor::{AnalogCompressorPlugin, AnalogCompressorPluginParams};

const SR: u32 = 48_000;
const FRAMES: usize = 4096;

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

fn rms_settled(buffer: &[f32]) -> f32 {
    (buffer.iter().map(|s| s * s).sum::<f32>() / buffer.len() as f32).sqrt()
}

#[test]
fn info_channels_and_zero_latency() {
    let plugin = AnalogCompressorPlugin::new(2);
    assert_eq!(plugin.channels(), 2);
    let info = plugin.info();
    assert_eq!(info.name, "Analog Compressor");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(plugin.latency_samples(), 0);
}

#[test]
fn quiet_signal_gets_no_gain_reduction() {
    let mut plugin = AnalogCompressorPlugin::new(1);
    plugin.initialize(SR).unwrap();
    // −20 dBFS sine, threshold −18 dB: detector barely tickles; expect ~unity.
    let input = make_interleaved_sine(440.0, SR, FRAMES, 1, 0.1);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    let ratio = rms_settled(&buffer[2048..]) / rms_settled(&input[2048..]);
    assert!(
        (ratio - 1.0).abs() < 0.05,
        "unexpected GR on quiet signal: {ratio}"
    );
}

#[test]
fn steady_hot_signal_matches_gain_computer() {
    let mut plugin = AnalogCompressorPlugin::new(1);
    plugin.initialize(SR).unwrap();
    // 0 dBFS sine, threshold −12 dB, ratio 4:1, hard knee:
    // GR = (0 − (−12)) × (1 − 1/4) = 9 dB.
    for (key, value) in [
        ("threshold", -12.0),
        ("ratio", 4.0),
        ("knee", 0.0),
        ("attack", 1.0),
        ("release", 50.0),
    ] {
        plugin
            .set_parameter(ParameterId::from(key), ParameterValue::Float(value))
            .unwrap();
    }
    let input = make_interleaved_sine(440.0, SR, FRAMES, 1, 1.0);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    let ratio = rms_settled(&buffer[2048..]) / rms_settled(&input[2048..]);
    let expected = 10.0f32.powf(-9.0 / 20.0);
    assert!(
        (ratio - expected).abs() < 0.05,
        "expected ~9 dB GR, ratio={ratio} expected={expected}"
    );
}

#[test]
fn makeup_and_mix_behave() {
    let mut plugin = AnalogCompressorPlugin::new(1);
    plugin.initialize(SR).unwrap();
    for (key, value) in [
        ("threshold", -12.0),
        ("ratio", 4.0),
        ("knee", 0.0),
        ("attack", 1.0),
        ("release", 50.0),
        ("makeup", 9.0),
    ] {
        plugin
            .set_parameter(ParameterId::from(key), ParameterValue::Float(value))
            .unwrap();
    }
    let input = make_interleaved_sine(440.0, SR, FRAMES, 1, 1.0);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    // 9 dB GR + 9 dB makeup ≈ unity.
    let ratio = rms_settled(&buffer[2048..]) / rms_settled(&input[2048..]);
    assert!(
        (ratio - 1.0).abs() < 0.08,
        "makeup did not restore level: {ratio}"
    );

    // Mix at 0% is dry regardless of GR.
    plugin
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
        .unwrap();
    plugin.reset();
    buffer.copy_from_slice(&input);
    plugin.process_in_place(&mut buffer, &context).unwrap();
    for (got, want) in buffer.iter().zip(input.iter()) {
        assert!((got - want).abs() < 1e-5, "mix=0 is not dry");
    }
}

#[test]
fn auto_makeup_recovers_level() {
    let mut plugin = AnalogCompressorPlugin::new(1);
    plugin.initialize(SR).unwrap();
    for (key, value) in [
        ("threshold", -12.0),
        ("ratio", 4.0),
        ("knee", 0.0),
        ("attack", 1.0),
        ("release", 50.0),
    ] {
        plugin
            .set_parameter(ParameterId::from(key), ParameterValue::Float(value))
            .unwrap();
    }
    plugin
        .set_parameter(ParameterId::from("auto_makeup"), ParameterValue::Bool(true))
        .unwrap();
    let input = make_interleaved_sine(440.0, SR, FRAMES, 1, 1.0);
    let mut buffer = input.clone();
    let context = ProcessContext::new(SR, FRAMES);
    // Run long enough for the slow makeup follower to converge.
    for _ in 0..8 {
        buffer.copy_from_slice(&input);
        plugin.process_in_place(&mut buffer, &context).unwrap();
    }
    let ratio = rms_settled(&buffer[2048..]) / rms_settled(&input[2048..]);
    assert!(
        (ratio - 1.0).abs() < 0.25,
        "auto makeup did not recover level: {ratio}"
    );
}

#[test]
fn color_stage_adds_character_when_driven() {
    let mut plugin = AnalogCompressorPlugin::new(1);
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
    let mut plugin = AnalogCompressorPlugin::new(2);
    plugin.initialize(SR).unwrap();
    let cases: &[(&str, ParameterValue)] = &[
        ("threshold", ParameterValue::Float(-24.0)),
        ("ratio", ParameterValue::Float(8.0)),
        ("attack", ParameterValue::Float(30.0)),
        ("release", ParameterValue::Float(300.0)),
        ("knee", ParameterValue::Float(12.0)),
        ("makeup", ParameterValue::Float(6.0)),
        ("mix", ParameterValue::Float(0.5)),
        ("auto_makeup", ParameterValue::Bool(true)),
        (
            "analog_model",
            ParameterValue::String("Console Preamp".to_string()),
        ),
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
    // Core ranges still enforced.
    assert!(
        plugin
            .set_parameter(ParameterId::from("ratio"), ParameterValue::Float(0.5))
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(6.0))
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("analog_model"),
                ParameterValue::String("1176".to_string())
            )
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(ParameterId::from("nope"), ParameterValue::Float(1.0))
            .is_err()
    );
}

#[test]
fn from_params_rejects_bad_model_and_zero_channels() {
    let params = AnalogCompressorPluginParams {
        analog_model: "Nope".to_string(),
        ..Default::default()
    };
    assert!(AnalogCompressorPlugin::try_from_params(2, params).is_err());
    assert!(
        AnalogCompressorPlugin::try_from_params(0, AnalogCompressorPluginParams::default())
            .is_err()
    );
}

#[test]
fn oversized_blocks_are_chunked() {
    let mut plugin = AnalogCompressorPlugin::new(2);
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
    let params: AnalogCompressorPluginParams =
        serde_json::from_value(serde_json::json!({"threshold": -6.0, "ratio": 3.0})).unwrap();
    assert_eq!(params.threshold, -6.0);
    assert_eq!(params.ratio, 3.0);
    let plugin = AnalogCompressorPlugin::try_from_params(2, params).unwrap();
    assert_eq!(plugin.channels(), 2);
}
