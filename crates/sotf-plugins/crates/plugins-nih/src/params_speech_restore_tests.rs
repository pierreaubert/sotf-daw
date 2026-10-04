//! Speech native restore preflight and rejection tests.
//!
//! Drives actual `DynamicParams` plus bridge SpeechDenoiser construction
//! through scalar preflight, populated-history rejection, and valid
//! nondefault restore. A rejected restore must leave accepted
//! parameters, DSP state, and waveform history untouched with valid
//! processing continuing; only validate-before-mutation achieves that.

// Rust guideline compliant 2026-02-21

use super::{DynamicParams, ParamKind};
use nih_plug::prelude::Params;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::{Plugin, ProcessContext};
use std::collections::BTreeMap;
use std::sync::Arc;

fn speech_infos() -> Vec<BridgedParamInfo> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("SpeechDenoiser"));
    (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect()
}

fn speech_params() -> Arc<DynamicParams> {
    let infos = speech_infos();
    DynamicParams::from_infos_for_plugin("SpeechDenoiser", &infos)
}

fn speech_state(pairs: &[(&str, ParamValue)]) -> PluginState {
    PluginState {
        version: "speech-restore-test-state".to_owned(),
        params: pairs
            .iter()
            .map(|(id, value)| (id.to_string(), value.clone()))
            .collect(),
        fields: BTreeMap::new(),
    }
}

fn build_speech_dsp(params: &DynamicParams, sample_rate: u32, max_block: usize) -> Box<dyn Plugin> {
    let plugin =
        crate::params::configuration::create_plugin("SpeechDenoiser", f64::from(sample_rate), params).unwrap();
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, max_block).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    plugin
}

fn render_lcg_blocks(
    plugin: &mut Box<dyn Plugin>,
    inputs: usize,
    outputs: usize,
    sample_rate: u32,
    frames: usize,
    blocks: usize,
    seed: u32,
) -> Vec<f32> {
    let mut state = seed;
    let mut output = Vec::with_capacity(frames * outputs * blocks);
    for _ in 0..blocks {
        let input = (0..frames * inputs)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state as f32 / u32::MAX as f32) * 0.5 - 0.25
            })
            .collect::<Vec<_>>();
        let mut block_out = vec![0.0; frames * outputs];
        let produced = plugin
            .process(
                &input,
                &mut block_out,
                &ProcessContext::new(sample_rate, frames),
            )
            .unwrap();
        assert_eq!(produced, frames);
        output.extend_from_slice(&block_out);
    }
    output
}

fn assert_finite_nonzero(output: &[f32], context: &str) {
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{context}: output must stay finite"
    );
    assert!(
        output.iter().any(|sample| sample.abs() > 1.0e-6),
        "{context}: output must stay nonzero"
    );
}

#[test]
fn speech_model_selector_is_visible_manual_structural() {
    let params = speech_params();
    let model = params.param_map.get("model").expect("model control exists");
    assert!(
        matches!(model.kind, ParamKind::Int),
        "model stays an integer control"
    );
    assert!(!model.realtime, "model stays off the realtime path");
    assert!(
        model.requires_restart,
        "model is a manual structural selector, not a hidden control"
    );
    assert_eq!(
        super::speech_model_choice_labels(),
        Some(&["RNNoise Full", "RNNoise Legacy LQ", "RNNoise Legacy SH"][..]),
        "display labels track the canonical DSP registry"
    );
    assert_eq!(params.value("model"), Some(ParameterValue::Int(0)));
    assert_eq!(params.value("enabled"), Some(ParameterValue::Bool(true)));
    assert_eq!(params.value("strength"), Some(ParameterValue::Float(1.0)));
    let enabled = params.param_map.get("enabled").expect("enabled exists");
    assert!(enabled.realtime);
    assert!(!enabled.requires_restart);
    let strength = params.param_map.get("strength").expect("strength exists");
    assert!(strength.realtime);
    assert!(!strength.requires_restart);
}

#[test]
fn speech_preflight_accepts_valid_and_migrating_states() {
    let params = speech_params();
    let valid: &[&[(&str, ParamValue)]] = &[
        &[
            ("enabled", ParamValue::Bool(true)),
            ("strength", ParamValue::F32(0.5)),
            ("model", ParamValue::I32(1)),
        ],
        &[("model", ParamValue::I32(0))],
        &[("model", ParamValue::I32(1))],
        &[("model", ParamValue::I32(2))],
        &[("strength", ParamValue::F32(0.0))],
        &[("strength", ParamValue::F32(1.0))],
        &[("enabled", ParamValue::Bool(false))],
        &[],
        &[("future_knob", ParamValue::I32(7))],
    ];
    for (case, pairs) in valid.iter().enumerate() {
        assert!(
            params.validate_state(&speech_state(pairs), true, false, Some(48_000.0)),
            "valid case {case} must pass preflight"
        );
    }
    // Non-speech schemas bypass: the gate must not affect other plugins.
    let generic = DynamicParams::from_infos(&[]);
    assert!(
        generic.validate_state(
            &speech_state(&[("model", ParamValue::I32(99))]),
            true,
            false,
            None
        ),
        "schema gate bypasses non-speech params"
    );
}

#[test]
fn speech_preflight_rejects_invalid_scalars() {
    let params = speech_params();
    let invalid: &[(&str, &[(&str, ParamValue)])] = &[
        (
            "model-99",
            &[
                ("enabled", ParamValue::Bool(true)),
                ("strength", ParamValue::F32(0.5)),
                ("model", ParamValue::I32(99)),
            ],
        ),
        ("model-negative", &[("model", ParamValue::I32(-1))]),
        ("model-past-registry", &[("model", ParamValue::I32(3))]),
        ("model-bool", &[("model", ParamValue::Bool(true))]),
        ("model-float", &[("model", ParamValue::F32(1.0))]),
        (
            "model-string",
            &[("model", ParamValue::String("RNNoise Full".to_string()))],
        ),
        ("strength-high", &[("strength", ParamValue::F32(1.5))]),
        ("strength-negative", &[("strength", ParamValue::F32(-0.1))]),
        ("strength-nan", &[("strength", ParamValue::F32(f32::NAN))]),
        (
            "strength-infinite",
            &[("strength", ParamValue::F32(f32::INFINITY))],
        ),
        (
            "strength-neg-infinite",
            &[("strength", ParamValue::F32(f32::NEG_INFINITY))],
        ),
        ("strength-int", &[("strength", ParamValue::I32(1))]),
        ("enabled-int", &[("enabled", ParamValue::I32(1))]),
        ("enabled-float", &[("enabled", ParamValue::F32(1.0))]),
        (
            "enabled-string",
            &[("enabled", ParamValue::String("true".to_string()))],
        ),
    ];
    for (label, pairs) in invalid {
        for (is_active, is_audio_thread) in [(true, false), (true, true), (false, false)] {
            assert!(
                !params.validate_state(
                    &speech_state(pairs),
                    is_active,
                    is_audio_thread,
                    Some(48_000.0)
                ),
                "{label} must fail preflight (active={is_active}, audio={is_audio_thread})"
            );
        }
    }
}

#[test]
fn speech_audio_thread_restore_is_refused() {
    let populated = speech_state(&[
        ("enabled", ParamValue::Bool(true)),
        ("strength", ParamValue::F32(0.5)),
        ("model", ParamValue::I32(1)),
    ]);
    assert!(
        !super::speech_state_restore_allows_audio_thread(&populated),
        "structural speech restores must run off the callback"
    );
    assert!(
        !super::speech_state_restore_allows_audio_thread(&speech_state(&[])),
        "the admission hook refuses unconditionally, mirroring EQ"
    );
}

#[test]
fn failed_speech_restore_preserves_history_and_processing() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 256;
    let params = speech_params();
    // Nondefault accepted state so scalar preservation is observable.
    let entry = params.param_map.get("strength").expect("strength exists");
    params.float_params[entry.index].set_plain_value_for_initialization(0.5);
    let accepted_fingerprint = params.structural_fingerprint();
    let accepted_strength = params.value("strength");
    let mut plugin = build_speech_dsp(&params, RATE, 512);
    let mut twin = build_speech_dsp(&params, RATE, 512);
    assert_eq!(plugin.latency_samples(), 960);
    let history = render_lcg_blocks(&mut plugin, 2, 2, RATE, FRAMES, 4, 0x57EE_D001);
    let twin_history = render_lcg_blocks(&mut twin, 2, 2, RATE, FRAMES, 4, 0x57EE_D001);
    assert_eq!(history, twin_history, "twins share populated history");
    assert_finite_nonzero(&history, "history");
    // Invalid restore attempts stop at preflight, mirroring the codec
    // order: no parameter writes, no rebuild, no history reset.
    let attempts: &[(&str, &[(&str, ParamValue)])] = &[
        (
            "model-99",
            &[
                ("enabled", ParamValue::Bool(true)),
                ("strength", ParamValue::F32(0.5)),
                ("model", ParamValue::I32(99)),
            ],
        ),
        (
            "strength-1.5",
            &[
                ("enabled", ParamValue::Bool(true)),
                ("strength", ParamValue::F32(1.5)),
                ("model", ParamValue::I32(0)),
            ],
        ),
    ];
    for (label, pairs) in attempts {
        assert!(
            !params.validate_state(&speech_state(pairs), true, false, Some(48_000.0)),
            "{label} must not validate"
        );
        assert_eq!(
            params.value("model"),
            Some(ParameterValue::Int(0)),
            "{label} must preserve the accepted model"
        );
        assert_eq!(
            params.value("enabled"),
            Some(ParameterValue::Bool(true)),
            "{label} must preserve enabled"
        );
        assert_eq!(
            params.value("strength"),
            accepted_strength,
            "{label} must preserve strength"
        );
        assert_eq!(
            params.structural_fingerprint(),
            accepted_fingerprint,
            "{label} must not trip the reactivation guard"
        );
    }
    // Continuation renders bit-exact against the untouched twin.
    let continued = render_lcg_blocks(&mut plugin, 2, 2, RATE, FRAMES, 2, 0x57EE_D002);
    let twin_continued = render_lcg_blocks(&mut twin, 2, 2, RATE, FRAMES, 2, 0x57EE_D002);
    assert_eq!(
        continued, twin_continued,
        "rejected restores preserve waveform history"
    );
    assert_finite_nonzero(&continued, "continuation");
}

#[test]
fn valid_speech_restore_applies_and_renders() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 256;
    let params = speech_params();
    let before = params.structural_fingerprint();
    let state = speech_state(&[
        ("enabled", ParamValue::Bool(true)),
        ("strength", ParamValue::F32(0.5)),
        ("model", ParamValue::I32(1)),
    ]);
    assert!(
        params.validate_state(&state, true, false, Some(48_000.0)),
        "valid nondefault restore must pass preflight"
    );
    // Codec-faithful application after acceptance, then a fresh
    // detached candidate proves the applied configuration renders.
    for (id, value) in &state.params {
        let entry = params.param_map.get(id).expect("restored id exists");
        match value {
            ParamValue::Bool(plain) => {
                params.bool_params[entry.index].set_plain_value_for_initialization(*plain);
            }
            ParamValue::F32(plain) => {
                params.float_params[entry.index].set_plain_value_for_initialization(*plain);
            }
            ParamValue::I32(plain) => {
                params.int_params[entry.index].set_plain_value_for_initialization(*plain);
            }
            ParamValue::String(_) => panic!("speech scalars are typed"),
        }
    }
    assert_eq!(params.value("model"), Some(ParameterValue::Int(1)));
    assert_ne!(
        params.structural_fingerprint(),
        before,
        "applied model change must arm reactivation"
    );
    let mut plugin = build_speech_dsp(&params, RATE, 512);
    let mut reference = build_speech_dsp(&params, RATE, 512);
    let output = render_lcg_blocks(&mut plugin, 2, 2, RATE, FRAMES, 2, 0x57EE_D003);
    let expected = render_lcg_blocks(&mut reference, 2, 2, RATE, FRAMES, 2, 0x57EE_D003);
    assert_eq!(output, expected, "applied restore renders bit-exact");
    assert_finite_nonzero(&output, "restored");
}
