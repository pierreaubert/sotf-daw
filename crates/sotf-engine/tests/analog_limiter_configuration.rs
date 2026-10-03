//! Persisted engine AnalogLimiter controls reach the DSP and the ceiling.
//!
//! Every test drives the engine settings route (`PluginSettings::default_for`,
//! serde persistence, `to_plugin_config`) into the public factory with real
//! renders: model-index mapping, legacy tolerance, exact control delivery,
//! and the emitted threshold ceiling across rates and widths.

// Rust guideline compliant 2026-02-21
use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::{ParameterId, ParameterValue, Plugin, ProcessContext, create_plugin};

const FLAT_TOP_TOL: f64 = 2e-6;

fn ceiling_f32(threshold_db: f32) -> f32 {
    10f32.powf(threshold_db / 20.0)
}

fn hot_sine(frames: usize, channels: usize, rate: u32, peak: f32) -> Vec<f32> {
    let mut input = vec![0.0; frames * channels];
    for frame in 0..frames {
        let sample = peak * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / rate as f32).sin();
        for channel in 0..channels {
            input[frame * channels + channel] = sample;
        }
    }
    input
}

fn render(plugin: &mut dyn Plugin, rate: u32, input: &[f32], channels: usize) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len()];
    for (input, output) in input
        .chunks(512 * channels)
        .zip(output.chunks_mut(512 * channels))
    {
        let frames = input.len() / channels;
        assert_eq!(
            plugin
                .process(input, output, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn settled_peak(output: &[f32], channels: usize) -> f32 {
    output[2048 * channels..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max)
}

fn persisted(settings: &PluginSettings) -> PluginSettings {
    let encoded = serde_json::to_vec(settings).unwrap();
    serde_json::from_slice(&encoded).unwrap()
}

/// Fixture overrides for authored limiter settings; defaults match the
/// owned schema so call sites name only what the case varies.
#[derive(Clone, Copy, Debug)]
struct LimiterFixture {
    threshold: f64,
    model: f64,
    drive: f64,
    color: f64,
    character: f64,
    trim: f64,
    mix: f64,
    lookahead: f64,
}

impl Default for LimiterFixture {
    fn default() -> Self {
        Self {
            threshold: -0.1,
            model: 0.0,
            drive: 0.0,
            color: 0.0,
            character: 0.5,
            trim: 0.0,
            mix: 1.0,
            lookahead: 5.0,
        }
    }
}

fn settings_with(fixture: LimiterFixture) -> PluginSettings {
    let mut settings = PluginSettings::default_for(&PluginType::AnalogLimiter).unwrap();
    let PluginSettings::AnalogLimiter {
        threshold: threshold_field,
        lookahead: lookahead_field,
        mix: mix_field,
        analog_model,
        analog_drive,
        analog_color,
        analog_character,
        analog_trim,
        ..
    } = &mut settings
    else {
        panic!("AnalogLimiter default must have AnalogLimiter settings");
    };
    *threshold_field = fixture.threshold;
    *lookahead_field = fixture.lookahead;
    *mix_field = fixture.mix;
    *analog_model = fixture.model;
    *analog_drive = fixture.drive;
    *analog_color = fixture.color;
    *analog_character = fixture.character;
    *analog_trim = fixture.trim;
    persisted(&settings)
}

#[test]
fn engine_analog_limiter_defaults_follow_owned_schema_order() {
    let defaults = PluginSettings::default_for(&PluginType::AnalogLimiter).unwrap();
    let expected = [
        "threshold",
        "release",
        "lookahead",
        "soft",
        "true_peak",
        "mix",
        "analog_model",
        "analog_drive",
        "analog_color",
        "analog_character",
        "analog_trim",
    ];
    assert_eq!(defaults.param_specs().len(), expected.len());
    for (spec, key) in defaults.param_specs().iter().zip(expected) {
        assert_eq!(spec.engine_key, key);
    }
    let PluginSettings::AnalogLimiter {
        threshold,
        release,
        lookahead,
        soft,
        true_peak,
        mix,
        analog_model,
        analog_drive,
        analog_color,
        analog_character,
        analog_trim,
    } = &defaults
    else {
        panic!("AnalogLimiter default must have AnalogLimiter settings");
    };
    for (key, actual, expected) in [
        ("threshold", *threshold, -0.1),
        ("release", *release, 50.0),
        ("lookahead", *lookahead, 5.0),
        ("mix", *mix, 1.0),
        ("analog_model", *analog_model, 0.0),
        ("analog_drive", *analog_drive, 0.0),
        ("analog_color", *analog_color, 0.0),
        ("analog_character", *analog_character, 0.5),
        ("analog_trim", *analog_trim, 0.0),
    ] {
        assert!(
            (actual - expected).abs() < 1e-12,
            "{key} default {actual} != schema {expected}"
        );
    }
    assert!(!soft);
    assert!(!true_peak);
}

#[test]
fn engine_analog_limiter_model_index_maps_to_canonical_label() {
    for (index, label) in [(0.0, "Harmonics"), (3.0, "Tape"), (5.0, "Console Preamp")] {
        let config = settings_with(LimiterFixture {
            threshold: -12.0,
            model: index,
            ..Default::default()
        })
        .to_plugin_config(48_000.0);
        assert_eq!(config.plugin_type, "analog_limiter");
        assert_eq!(config.parameters["analog_model"], label);
        let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
        plugin.initialize(48_000).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("analog_model")),
            Some(ParameterValue::String(label.to_string()))
        );
    }
    // Out-of-range indices fall back to Harmonics rather than failing.
    let config = settings_with(LimiterFixture {
        threshold: -12.0,
        model: 99.0,
        ..Default::default()
    })
    .to_plugin_config(48_000.0);
    assert_eq!(config.parameters["analog_model"], "Harmonics");
}

#[test]
fn engine_analog_limiter_legacy_settings_without_analog_fields_still_construct() {
    // Saved documents predating the analog block carry only the six core
    // controls; missing analog fields deserialize to zero and the model
    // index 0 selects Harmonics.
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "AnalogLimiter": {
            "threshold": -12.0,
            "release": 50.0,
            "lookahead": 5.0,
            "soft": false,
            "true_peak": false,
            "mix": 1.0,
        }
    }))
    .unwrap();
    let config = legacy.to_plugin_config(48_000.0);
    assert_eq!(config.parameters["analog_model"], "Harmonics");
    assert_eq!(config.parameters["analog_drive"], 0.0);
    assert_eq!(config.parameters["analog_color"], 0.0);
    assert_eq!(config.parameters["analog_character"], 0.0);
    assert_eq!(config.parameters["analog_trim"], 0.0);
    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    let input = hot_sine(4096, 2, 48_000, 0.9);
    let output = render(plugin.as_mut(), 48_000, &input, 2);
    let peak = settled_peak(&output, 2);
    let ceiling = ceiling_f32(-12.0);
    assert!(peak <= ceiling, "peak {peak} exceeds ceiling {ceiling}");
    assert!(
        (f64::from(peak) - f64::from(ceiling)).abs() <= FLAT_TOP_TOL,
        "peak {peak} does not reach ceiling {ceiling}"
    );
}

#[test]
fn engine_analog_limiter_persisted_controls_reach_dsp_exactly() {
    let config = settings_with(LimiterFixture {
        threshold: -12.0,
        model: 3.0,
        drive: 6.0,
        color: 0.5,
        character: 0.25,
        trim: -6.0,
        ..Default::default()
    })
    .to_plugin_config(48_000.0);
    assert_eq!(config.plugin_type, "analog_limiter");
    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    for (id, expected) in [
        ("threshold", ParameterValue::Float(-12.0)),
        ("release", ParameterValue::Float(50.0)),
        ("lookahead", ParameterValue::Float(5.0)),
        ("soft", ParameterValue::Bool(false)),
        ("true_peak", ParameterValue::Bool(false)),
        ("mix", ParameterValue::Float(1.0)),
        ("analog_model", ParameterValue::String("Tape".to_string())),
        ("analog_drive", ParameterValue::Float(6.0)),
        ("analog_color", ParameterValue::Float(0.5)),
        ("analog_character", ParameterValue::Float(0.25)),
        ("analog_trim", ParameterValue::Float(-6.0)),
    ] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(id)),
            Some(expected),
            "param {id} must reach the DSP exactly"
        );
    }
}

#[test]
fn engine_analog_limiter_ceiling_holds_across_rates_and_widths() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2, 6] {
            // Color-off leg: hot signal flat-tops at the ceiling.
            let config = settings_with(LimiterFixture {
                threshold: -12.0,
                ..Default::default()
            })
            .to_plugin_config(f64::from(rate));
            let mut plugin =
                create_plugin(&config.plugin_type, &config.parameters, channels, rate).unwrap();
            plugin.initialize(rate).unwrap();
            let input = hot_sine(4096, channels, rate, 0.9);
            let output = render(plugin.as_mut(), rate, &input, channels);
            let peak = settled_peak(&output, channels);
            let ceiling = ceiling_f32(-12.0);
            assert!(
                peak <= ceiling,
                "rate={rate} ch={channels}: peak {peak} exceeds ceiling {ceiling}"
            );
            assert!(
                (f64::from(peak) - f64::from(ceiling)).abs() <= FLAT_TOP_TOL,
                "rate={rate} ch={channels}: peak {peak} does not reach ceiling {ceiling}"
            );
            // Colored leg: ceiling respected, signal nonzero.
            let config = settings_with(LimiterFixture {
                threshold: -12.0,
                model: 3.0,
                drive: 6.0,
                color: 0.5,
                character: 0.25,
                ..Default::default()
            })
            .to_plugin_config(f64::from(rate));
            let mut plugin =
                create_plugin(&config.plugin_type, &config.parameters, channels, rate).unwrap();
            plugin.initialize(rate).unwrap();
            let output = render(plugin.as_mut(), rate, &input, channels);
            let peak = settled_peak(&output, channels);
            assert!(
                peak <= ceiling,
                "rate={rate} ch={channels}: colored peak {peak} exceeds ceiling {ceiling}"
            );
            assert!(
                peak > ceiling * 0.1,
                "rate={rate} ch={channels}: colored peak {peak} is suspiciously quiet"
            );
        }
    }
}

#[test]
fn engine_analog_limiter_dry_setting_passes_hot_signal_unclamped() {
    let config = settings_with(LimiterFixture {
        threshold: -6.0,
        mix: 0.0,
        lookahead: 0.0,
        ..Default::default()
    })
    .to_plugin_config(48_000.0);
    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    let input = hot_sine(4096, 2, 48_000, 2.0);
    let output = render(plugin.as_mut(), 48_000, &input, 2);
    let peak = settled_peak(&output, 2);
    assert!(
        peak > ceiling_f32(-6.0) * 2.0,
        "dry peak {peak} was clamped"
    );
}
