//! Analog limiter through the public facade: catalog, factory, and adapter audio.
//!
//! Every test drives `sotf_plugins` public API (`catalog_entry`,
//! `supported_plugin_types`, `create_plugin`, the `Plugin` adapter) rather
//! than calling the DSP crate directly. Audio assertions use real renders:
//! the published threshold ceiling is verified on emitted samples after
//! color/trim at fully wet mix, with dry/unclamped and lifecycle controls.

// Rust guideline compliant 2026-02-21
use serde_json::json;
use sotf_plugins::{
    ParameterId, ParameterValue, Plugin, PluginCategory, ProcessContext, catalog_entry,
    create_plugin, supported_plugin_types,
};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const FRAMES: usize = 4096;
const SETTLE_FRAMES: usize = 2048;
/// Absolute flat-top tolerance, matching the engine limiter contract test.
const FLAT_TOP_TOL: f64 = 2e-6;

fn ceiling_f32(threshold_db: f32) -> f32 {
    // Same exact conversion as the owned guard: 10^(dB/20) in f32.
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
    output[SETTLE_FRAMES * channels..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max)
}

fn make(plugin_type: &str, config: &serde_json::Value) -> Box<dyn Plugin> {
    let mut plugin = create_plugin(plugin_type, config, CHANNELS, RATE).unwrap();
    plugin.initialize(RATE).unwrap();
    plugin
}

#[test]
fn facade_catalog_lists_analog_limiter_once_with_stable_identity() {
    let entry = catalog_entry("analog_limiter").expect("catalog must list analog_limiter");
    assert_eq!(entry.canonical_type, "analog_limiter");
    assert_eq!(entry.aliases, ["analog_limiter"].as_slice());
    assert_eq!(entry.metadata.exposed_name, "Analog Limiter");
    assert!(matches!(entry.category, PluginCategory::Processor));
    // Picker exposure: the generic application picker enumerates
    // `generic_app_catalog_entries`, so this is what makes "Analog Limiter"
    // appear in the sibling add-plugin list with the Generated layout UI.
    assert!(entry.is_generic_app_plugin());
    assert!(entry.is_allowed_in_ab_compare());
    // Facade spelling contract: every catalog alias in every entry is
    // snake_case; lookup is case-insensitive but NOT underscore-
    // insensitive, so "ANALOG_LIMITER" resolves while "AnalogLimiter"
    // (no underscore, different length) does not. The PascalCase dual
    // spelling lives one layer out, in the bridge factory and FFI
    // family map, which pin it with their own tests.
    let upper = catalog_entry("ANALOG_LIMITER").expect("case variant must resolve");
    assert_eq!(upper.canonical_type, entry.canonical_type);
    assert!(catalog_entry("AnalogLimiter").is_none());
    let supported: Vec<_> = supported_plugin_types().collect();
    assert_eq!(
        supported
            .iter()
            .filter(|name| **name == "analog_limiter")
            .count(),
        1,
        "analog_limiter must appear exactly once in supported_plugin_types"
    );
    assert!(catalog_entry("analog_limiter_typo").is_none());
}

#[test]
fn facade_factory_accepts_snake_spelling_and_case_variants() {
    let config = json!({"threshold": -12.0});
    for spelling in ["analog_limiter", "ANALOG_LIMITER"] {
        let plugin = create_plugin(spelling, &config, CHANNELS, RATE).unwrap();
        assert_eq!(plugin.input_channels(), CHANNELS);
        assert_eq!(plugin.output_channels(), CHANNELS);
    }
    for unknown in ["analog_limiter_typo", "AnalogLimiter"] {
        let error = create_plugin(unknown, &config, CHANNELS, RATE)
            .err()
            .expect("unknown plugin type must fail construction");
        assert!(
            error.contains("Unknown plugin type"),
            "unexpected error for {unknown}: {error}"
        );
    }
}

#[test]
fn facade_constructor_params_reach_dsp_with_exact_values() {
    let config = json!({
        "threshold": -12.0,
        "release": 50.0,
        "lookahead": 5.0,
        "soft": true,
        "true_peak": true,
        "mix": 1.0,
        "analog_model": "Tape",
        "analog_drive": 6.0,
        "analog_color": 0.5,
        "analog_character": 0.25,
        "analog_trim": -6.0,
    });
    let plugin = make("analog_limiter", &config);
    for (id, expected) in [
        ("threshold", ParameterValue::Float(-12.0)),
        ("release", ParameterValue::Float(50.0)),
        ("lookahead", ParameterValue::Float(5.0)),
        ("soft", ParameterValue::Bool(true)),
        ("true_peak", ParameterValue::Bool(true)),
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
fn facade_rejects_invalid_constructor_params() {
    for (name, config) in [
        ("unknown model", json!({"analog_model": "NoSuchModel"})),
        ("mistyped threshold", json!({"threshold": "loud"})),
        ("mistyped mix", json!({"mix": [1.0]})),
    ] {
        assert!(
            create_plugin("analog_limiter", &config, CHANNELS, RATE).is_err(),
            "{name} must fail construction"
        );
    }
    // A rejection must not poison later construction with valid params.
    let plugin = make("analog_limiter", &json!({"threshold": -12.0}));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-12.0))
    );
}

#[test]
fn facade_color_off_hot_signal_flat_tops_at_ceiling() {
    let ceiling = ceiling_f32(-12.0);
    let input = hot_sine(FRAMES, CHANNELS, RATE, 0.9);
    for spelling in ["analog_limiter", "ANALOG_LIMITER"] {
        let mut plugin = make(spelling, &json!({"threshold": -12.0}));
        let output = render(plugin.as_mut(), RATE, &input, CHANNELS);
        let peak = settled_peak(&output, CHANNELS);
        assert!(
            peak <= ceiling,
            "{spelling}: peak {peak} exceeds ceiling {ceiling}"
        );
        assert!(
            (f64::from(peak) - f64::from(ceiling)).abs() <= FLAT_TOP_TOL,
            "{spelling}: peak {peak} does not reach ceiling {ceiling}"
        );
    }
}

#[test]
fn facade_colored_hot_signal_respects_ceiling_and_stays_nonzero() {
    let ceiling = ceiling_f32(-12.0);
    let input = hot_sine(FRAMES, CHANNELS, RATE, 0.9);
    let mut plugin = make(
        "analog_limiter",
        &json!({
            "threshold": -12.0,
            "analog_model": "Tape",
            "analog_drive": 6.0,
            "analog_color": 0.5,
            "analog_character": 0.25,
            "analog_trim": 0.0,
        }),
    );
    let output = render(plugin.as_mut(), RATE, &input, CHANNELS);
    let peak = settled_peak(&output, CHANNELS);
    assert!(
        peak <= ceiling,
        "colored peak {peak} exceeds ceiling {ceiling}"
    );
    assert!(
        peak > ceiling * 0.1,
        "colored peak {peak} is suspiciously quiet (ceiling {ceiling})"
    );
}

#[test]
fn facade_case_variants_render_bit_identical_audio() {
    // Case-insensitive lookup canonicalizes before construction, so a
    // case variant renders byte-identical audio to the canonical snake
    // spelling. (PascalCase-without-underscore is not a facade spelling;
    // the bridge/FFI dual-spelling layers pin it separately.)
    let input = hot_sine(FRAMES, CHANNELS, RATE, 0.9);
    let config = json!({
        "threshold": -12.0,
        "analog_model": "Tape",
        "analog_drive": 6.0,
        "analog_color": 0.5,
        "analog_character": 0.25,
    });
    let mut snake = make("analog_limiter", &config);
    let mut upper = make("ANALOG_LIMITER", &config);
    let snake_out = render(snake.as_mut(), RATE, &input, CHANNELS);
    let upper_out = render(upper.as_mut(), RATE, &input, CHANNELS);
    assert_eq!(snake_out, upper_out);
}

#[test]
fn facade_dry_blend_passes_hot_signal_unclamped() {
    // Positive control for the dry exception: mix 0 with lookahead 0 and no
    // color must pass a hot signal without clamping (mirrors the owned
    // dry_blend_exceeds_ceiling_positive_control).
    let ceiling = ceiling_f32(-6.0);
    let input = hot_sine(FRAMES, CHANNELS, RATE, 2.0);
    let mut plugin = make(
        "analog_limiter",
        &json!({"threshold": -6.0, "lookahead": 0.0, "mix": 0.0}),
    );
    let output = render(plugin.as_mut(), RATE, &input, CHANNELS);
    let peak = settled_peak(&output, CHANNELS);
    assert!(
        peak > ceiling * 2.0,
        "dry peak {peak} was clamped toward ceiling {ceiling}"
    );
}

#[test]
fn facade_live_threshold_step_applies_to_next_block() {
    let input = hot_sine(FRAMES, CHANNELS, RATE, 0.9);
    let mut plugin = make("analog_limiter", &json!({"threshold": -12.0}));
    let before = render(plugin.as_mut(), RATE, &input, CHANNELS);
    assert!(settled_peak(&before, CHANNELS) <= ceiling_f32(-12.0));
    plugin
        .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-18.0))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-18.0))
    );
    // The immediate guard flat-tops at the new target from the first block.
    let first = render(plugin.as_mut(), RATE, &input[..512 * CHANNELS], CHANNELS);
    let new_ceiling = ceiling_f32(-18.0);
    let first_peak = first
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    assert!(
        first_peak <= new_ceiling,
        "post-step peak {first_peak} exceeds new ceiling {new_ceiling}"
    );
    assert!(
        first_peak > new_ceiling * 0.5,
        "post-step peak {first_peak} lost the hot signal (ceiling {new_ceiling})"
    );
}

#[test]
fn facade_reset_reproduces_identical_audio() {
    let input = hot_sine(FRAMES, CHANNELS, RATE, 0.9);
    let mut plugin = make(
        "analog_limiter",
        &json!({"threshold": -12.0, "analog_model": "Tape", "analog_color": 0.5}),
    );
    let first = render(plugin.as_mut(), RATE, &input, CHANNELS);
    plugin.reset();
    let second = render(plugin.as_mut(), RATE, &input, CHANNELS);
    assert_eq!(first, second);
}

#[test]
fn facade_latency_matches_lookahead_and_color_adds_none() {
    // Core lookahead latency only: 5 ms at 48 kHz is 240 samples; the color
    // stage adds none, so the colored instance reports the same value.
    for (lookahead_ms, expected) in [(5.0, 240), (0.0, 0)] {
        let plain = make("analog_limiter", &json!({"lookahead": lookahead_ms}));
        assert_eq!(plain.latency_samples(), expected);
        let colored = make(
            "analog_limiter",
            &json!({"lookahead": lookahead_ms, "analog_color": 1.0}),
        );
        assert_eq!(colored.latency_samples(), expected);
    }
}

#[test]
fn facade_zero_color_stream_drains_finite_within_quota() {
    const CANARY: f32 = 12345.0;
    let input = hot_sine(1101, CHANNELS, RATE, 0.9);
    let mut plugin = make("analog_limiter", &json!({"threshold": -12.0}));
    let _ = render(plugin.as_mut(), RATE, &input, CHANNELS);
    plugin.begin_drain(&ProcessContext::new(RATE, 0)).unwrap();
    let quota = plugin
        .drain_call_bound()
        .expect("drain must advertise a call bound")
        .get();
    assert!((1..10_000).contains(&quota));
    let ceiling = ceiling_f32(-12.0);
    let mut drained_frames = 0;
    for _ in 1..=quota {
        let capacity = plugin.drain_output_frames_max();
        let mut output = vec![CANARY; capacity * CHANNELS + CHANNELS];
        let result = plugin
            .drain(
                &mut output[..capacity * CHANNELS],
                &ProcessContext::new(RATE, capacity),
            )
            .unwrap();
        assert!(result.frames <= capacity);
        let written = result.frames * CHANNELS;
        assert!(output[..written].iter().all(|sample| sample.is_finite()));
        assert!(
            output[..written]
                .iter()
                .all(|sample| sample.abs() <= ceiling),
            "drained audio must respect the ceiling"
        );
        assert!(output[written..].iter().all(|sample| *sample == CANARY));
        drained_frames += result.frames;
        if result.complete {
            assert!(drained_frames > 0, "hot stream should drain tail audio");
            // Processing after a completed drain requires a reset.
            let mut tail = vec![0.0; 64 * CHANNELS];
            assert!(
                plugin
                    .process(
                        &input[..64 * CHANNELS],
                        &mut tail,
                        &ProcessContext::new(RATE, 64)
                    )
                    .is_err()
            );
            plugin.reset();
            let mut tail = vec![f32::NAN; 64 * CHANNELS];
            assert_eq!(
                plugin
                    .process(
                        &input[..64 * CHANNELS],
                        &mut tail,
                        &ProcessContext::new(RATE, 64)
                    )
                    .unwrap(),
                64
            );
            return;
        }
    }
    panic!("drain exceeded its advertised quota of {quota} calls");
}

#[test]
fn facade_colored_stream_completes_drain_without_finite_tail_claim() {
    // Colored recurrence has no finite-audio-support claim: drain reports
    // COMPLETE immediately with zero tail frames (legacy EOS behavior).
    let input = hot_sine(1101, CHANNELS, RATE, 0.9);
    let mut plugin = make(
        "analog_limiter",
        &json!({"threshold": -12.0, "analog_color": 0.5}),
    );
    let _ = render(plugin.as_mut(), RATE, &input, CHANNELS);
    plugin.begin_drain(&ProcessContext::new(RATE, 0)).unwrap();
    assert_eq!(plugin.drain_output_frames_max(), 0);
    let mut output = vec![0.0; 64 * CHANNELS];
    let result = plugin
        .drain(&mut output, &ProcessContext::new(RATE, 64))
        .unwrap();
    assert!(result.complete);
    assert_eq!(result.frames, 0);
}
