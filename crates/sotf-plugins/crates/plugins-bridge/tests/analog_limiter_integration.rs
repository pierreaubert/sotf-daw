//! Analog limiter through the native bridge: factory, ParamBridge, state.
//!
//! Every test drives `plugins_bridge` public API (`create_plugin`,
//! `ParamBridge`, `state`, `prepare_standalone_plugin`) with real renders:
//! both factory spellings, normalized parameter roundtrips, structural
//! live-edit rejection, save/mutate/load persistence, and the emitted
//! threshold ceiling after color/trim.

// Rust guideline compliant 2026-02-21
use plugins_bridge::{create_plugin, prepare_standalone_plugin};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};

const RATE: u32 = 48_000;
const PARAM_IDS: [&str; 11] = [
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

fn bridge() -> plugins_bridge::param_bridge::ParamBridge {
    plugins_bridge::param_bridge::ParamBridge::new(sotf_plugin_analog_limiter::params::PARAMS)
}

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

fn render(plugin: &mut dyn Plugin, rate: u32, input: &[f32]) -> Vec<f32> {
    let channels = plugin.input_channels();
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

#[test]
fn bridge_param_order_kinds_and_realtime_flags_match_owned_schema() {
    let bridge = bridge();
    assert_eq!(bridge.count(), PARAM_IDS.len());
    let ids: Vec<_> = (0..bridge.count())
        .map(|index| bridge.info(index).unwrap().id)
        .collect();
    assert_eq!(ids, PARAM_IDS);
    for (index, id) in PARAM_IDS.iter().enumerate() {
        assert_eq!(bridge.find_index(id), Some(index));
    }
    // Automation contract: lookahead and analog_model are structural;
    // everything else is realtime. `.setup()` is a UI category (left
    // column), not an update mode (`param_spec.rs`: `setup` sets
    // `ParamCategory::Setup` while `update_mode` stays `Realtime`), so
    // soft/true_peak are realtime like in the core limiter schema. The
    // model is structural because replacement allocates and re-prepares
    // the color stage on the control thread: live changes are refused
    // (see `bridge_structural_live_edit_fails_without_touching_saved_state`)
    // and the model is adopted at construction or state restore.
    let realtime: Vec<_> = (0..bridge.count())
        .map(|index| bridge.info(index).unwrap().realtime)
        .collect();
    assert_eq!(
        realtime,
        [
            true,  // threshold
            true,  // release
            false, // lookahead (structural)
            true,  // soft (setup category, realtime mode)
            true,  // true_peak (setup category, realtime mode)
            true,  // mix
            false, // analog_model (structural: control-thread replacement)
            true,  // analog_drive
            true,  // analog_color
            true,  // analog_character
            true,  // analog_trim
        ]
    );
    let threshold = bridge.info(0).unwrap();
    assert_eq!(
        threshold.kind,
        plugins_bridge::param_bridge::BridgedParamKind::Float
    );
    assert_eq!(
        (
            threshold.min_value,
            threshold.max_value,
            threshold.default_value
        ),
        (-20.0, 0.0, -0.1)
    );
    let lookahead = bridge.info(2).unwrap();
    assert_eq!(
        (
            lookahead.min_value,
            lookahead.max_value,
            lookahead.default_value
        ),
        (0.0, 20.0, 5.0)
    );
    let model = bridge.info(6).unwrap();
    assert_eq!(
        model.kind,
        plugins_bridge::param_bridge::BridgedParamKind::Int
    );
    assert_eq!(
        (model.min_value, model.max_value, model.default_value),
        (0.0, 5.0, 0.0)
    );
    assert_eq!(model.steps, 6);
}

#[test]
fn bridge_factory_accepts_both_spellings_with_identical_audio() {
    let config = r#"{"threshold": -12.0, "analog_model": "Tape", "analog_drive": 6.0, "analog_color": 0.5, "analog_character": 0.25}"#;
    let input = hot_sine(4096, 2, RATE, 0.9);
    let mut outputs = Vec::new();
    for spelling in ["AnalogLimiter", "analog_limiter"] {
        let mut plugin = create_plugin(spelling, 2, RATE, config).unwrap();
        plugin.initialize(RATE).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("analog_model")),
            Some(ParameterValue::String("Tape".to_string())),
            "{spelling} must carry the model selection"
        );
        outputs.push(render(plugin.as_mut(), RATE, &input));
    }
    assert_eq!(outputs[0], outputs[1]);
    let peak = settled_peak(&outputs[0], 2);
    let ceiling = ceiling_f32(-12.0);
    assert!(peak <= ceiling, "peak {peak} exceeds ceiling {ceiling}");
    assert!(peak > ceiling * 0.1, "peak {peak} is suspiciously quiet");
}

#[test]
fn bridge_factory_honors_legacy_db_ms_aliases() {
    let mut plugin = create_plugin(
        "analog_limiter",
        2,
        RATE,
        r#"{"threshold_db": -12.0, "release_ms": 50.0, "lookahead_ms": 2.0}"#,
    )
    .unwrap();
    plugin.initialize(RATE).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-12.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("lookahead")),
        Some(ParameterValue::Float(2.0))
    );
}

#[test]
fn bridge_normalized_roundtrips_cover_float_bool_and_model_choice() {
    let bridge = bridge();
    let mut plugin = create_plugin("AnalogLimiter", 2, RATE, "{}").unwrap();
    // Threshold: normalized 0.4 over [-20, 0] is exactly -12 dB.
    bridge.set_normalized(plugin.as_mut(), 0, 0.4).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-12.0))
    );
    assert!((bridge.get_normalized(plugin.as_ref(), 0).unwrap() - 0.4).abs() < 1e-12);
    // Bool: soft on.
    bridge.set_normalized(plugin.as_mut(), 3, 1.0).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("soft")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(bridge.get_normalized(plugin.as_ref(), 3), Some(1.0));
    // Model choice: normalized 0.6 over six models selects index 3 ("Tape"),
    // delivered to the DSP as its canonical String label.
    bridge.set_normalized(plugin.as_mut(), 6, 0.6).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
    assert!((bridge.get_normalized(plugin.as_ref(), 6).unwrap() - 0.6).abs() < 1e-12);
    // Raw path agrees.
    bridge
        .set_raw(plugin.as_mut(), "analog_drive", 6.0)
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_drive")),
        Some(ParameterValue::Float(6.0))
    );
    assert_eq!(bridge.get_raw(plugin.as_ref(), "analog_drive"), Some(6.0));
}

#[test]
fn bridge_structural_live_edit_fails_without_touching_saved_state() {
    let bridge = bridge();
    let mut plugin = create_plugin("AnalogLimiter", 2, RATE, r#"{"lookahead": 2.0}"#).unwrap();
    plugin.initialize(RATE).unwrap();
    let saved = plugins_bridge::state::save_state(plugin.as_ref());
    // Lookahead is structural: post-init changes must fail ...
    assert!(bridge.set_normalized(plugin.as_mut(), 2, 0.5).is_err());
    // ... without touching the saved configuration ...
    assert_eq!(plugins_bridge::state::save_state(plugin.as_ref()), saved);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("lookahead")),
        Some(ParameterValue::Float(2.0))
    );
    // ... while repeating the committed value stays a no-op success.
    bridge.set_normalized(plugin.as_mut(), 2, 0.1).unwrap();
    // The analog model is likewise structural: a live switch to Tape is
    // refused without touching saved state ...
    assert!(bridge.set_normalized(plugin.as_mut(), 6, 0.6).is_err());
    assert_eq!(plugins_bridge::state::save_state(plugin.as_ref()), saved);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Harmonics".to_string()))
    );
    // ... while repeating the committed model stays a no-op success.
    bridge.set_normalized(plugin.as_mut(), 6, 0.0).unwrap();
    // Realtime params still automate live.
    bridge.set_normalized(plugin.as_mut(), 0, 0.4).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-12.0))
    );
}

#[test]
fn bridge_cross_model_state_restore_requires_reconstruction() {
    // Changed structural values require reconstruction by the caller
    // (`state.rs`): loading Tape state into an initialized Harmonics
    // instance fails the in-place load, leaving audio and config
    // untouched. Adopting the restored model means constructing (or
    // caller-reconstructing) with the saved state instead. The two
    // fixtures differ ONLY in the model: in-place load is documented
    // non-transactional, so any other delta could write before the
    // refusal.
    let mut harmonics = create_plugin(
        "AnalogLimiter",
        2,
        RATE,
        r#"{"threshold": -12.0, "analog_color": 0.5}"#,
    )
    .unwrap();
    harmonics.initialize(RATE).unwrap();
    let mut tape = create_plugin(
        "AnalogLimiter",
        2,
        RATE,
        r#"{"threshold": -12.0, "analog_model": "Tape", "analog_color": 0.5}"#,
    )
    .unwrap();
    tape.initialize(RATE).unwrap();
    let input = hot_sine(4096, 2, RATE, 0.9);
    let before = render(harmonics.as_mut(), RATE, &input);
    harmonics.reset();
    let saved = plugins_bridge::state::save_state(tape.as_ref());
    assert!(plugins_bridge::state::load_state(harmonics.as_mut(), &saved).is_err());
    assert_eq!(
        harmonics.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Harmonics".to_string()))
    );
    assert_eq!(render(harmonics.as_mut(), RATE, &input), before);
    // The supported adoption path: fresh construction from the saved state.
    let mut adopted = create_plugin("AnalogLimiter", 2, RATE, "{}").unwrap();
    plugins_bridge::state::load_state(adopted.as_mut(), &saved).unwrap();
    adopted.initialize(RATE).unwrap();
    assert_eq!(
        adopted.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
    tape.reset();
    assert_eq!(
        render(adopted.as_mut(), RATE, &input),
        render(tape.as_mut(), RATE, &input)
    );
}

#[test]
fn bridge_realtime_detector_controls_automate_live() {
    // Behavioral proof for the realtime flags on the setup-category
    // detector controls: post-init changes to soft and true_peak must
    // succeed (plain field updates, no rebuild, no error) and keep
    // emitting finite audio under the ceiling. The analog model is NOT
    // part of this proof: replacement allocates and re-prepares on the
    // control thread, so it is structural and refused live (pinned by
    // `bridge_structural_live_edit_fails_without_touching_saved_state`).
    let bridge = bridge();
    let mut plugin = create_plugin(
        "AnalogLimiter",
        2,
        RATE,
        r#"{"threshold": -12.0, "analog_color": 0.5}"#,
    )
    .unwrap();
    plugin.initialize(RATE).unwrap();
    bridge.set_normalized(plugin.as_mut(), 3, 1.0).unwrap();
    bridge.set_normalized(plugin.as_mut(), 4, 1.0).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("soft")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("true_peak")),
        Some(ParameterValue::Bool(true))
    );
    let input = hot_sine(4096, 2, RATE, 0.9);
    let output = render(plugin.as_mut(), RATE, &input);
    let peak = settled_peak(&output, 2);
    let ceiling = ceiling_f32(-12.0);
    assert!(peak <= ceiling, "peak {peak} exceeds ceiling {ceiling}");
    assert!(peak > ceiling * 0.1, "peak {peak} is suspiciously quiet");
}

#[test]
fn bridge_state_roundtrip_and_standalone_render_bit_identical() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2, 6] {
            let mut input = vec![0.0; 4096 * channels];
            for channel in 0..channels {
                input[channel] = 0.001 * (channel + 1) as f32;
                input[2047 * channels + channel] = -0.001 * (channel + 1) as f32;
            }
            let config = r#"{"threshold": -12.0, "lookahead": 2.0, "analog_model": "Tape", "analog_color": 0.0}"#;
            let mut direct = create_plugin("AnalogLimiter", channels, rate, config).unwrap();
            direct.initialize(rate).unwrap();
            let mut standalone = prepare_standalone_plugin(
                create_plugin("analog_limiter", channels, rate, config).unwrap(),
                257,
            )
            .unwrap();
            standalone.initialize(rate).unwrap();
            let delay = (u64::from(rate) * 2 / 1000) as usize;
            assert_eq!(direct.latency_samples(), delay);
            assert_eq!(standalone.latency_samples(), delay);
            let state = plugins_bridge::state::save_state(standalone.as_ref());
            let saved: serde_json::Value = serde_json::from_slice(&state).unwrap();
            assert_eq!(saved["threshold"], -12.0);
            assert_eq!(saved["analog_model"], "Tape");
            let mut restored = create_plugin("AnalogLimiter", channels, rate, "{}").unwrap();
            plugins_bridge::state::load_state(restored.as_mut(), &state).unwrap();
            restored = prepare_standalone_plugin(restored, 257).unwrap();
            restored.initialize(rate).unwrap();
            assert_eq!(restored.latency_samples(), delay);
            let output = render(standalone.as_mut(), rate, &input);
            assert_eq!(output, render(direct.as_mut(), rate, &input));
            assert_eq!(output, render(restored.as_mut(), rate, &input));
            for origin in [0, 2047] {
                for channel in 0..channels {
                    let peak = (origin..origin + 1024)
                        .max_by(|&a, &b| {
                            output[a * channels + channel]
                                .abs()
                                .total_cmp(&output[b * channels + channel].abs())
                        })
                        .unwrap();
                    assert_eq!(peak, origin + delay);
                    assert!(output[peak * channels + channel].abs() > 0.0005);
                }
            }
            // Reloading the same snapshot retains the configuration.
            plugins_bridge::state::load_state(standalone.as_mut(), &state).unwrap();
            assert_eq!(
                standalone.get_parameter(&ParameterId::from("analog_model")),
                Some(ParameterValue::String("Tape".to_string()))
            );
        }
    }
}

#[test]
fn bridge_rejected_state_preserves_audio_and_config() {
    let config = r#"{"threshold": -12.0, "analog_model": "Tape", "analog_color": 0.5}"#;
    let input = hot_sine(4096, 2, RATE, 0.9);
    let mut plugin = create_plugin("AnalogLimiter", 2, RATE, config).unwrap();
    plugin.initialize(RATE).unwrap();
    let before = render(plugin.as_mut(), RATE, &input);
    plugin.reset();
    assert!(plugins_bridge::state::load_state(plugin.as_mut(), b"\x00\x01not-json").is_err());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-12.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
    let after = render(plugin.as_mut(), RATE, &input);
    assert_eq!(before, after);
}
