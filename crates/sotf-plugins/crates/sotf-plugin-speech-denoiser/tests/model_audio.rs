//! Per-model selection, audio difference, and lifecycle oracles.
//!
//! Covers the real-model R2 surface: registry identity shared verbatim with
//! the backend, alternate selection changing nonzero inference audio (mono
//! and linked stereo), per-model bypass/reset/partition/EOF/latency,
//! deterministic fresh restores, failed-restore continuation, and per-model
//! allocation freedom. Difference asserts prove the alternates are served;
//! they make no quality ranking between models.

// Rust guideline compliant 2026-02-21

use plugins_denoiser::rnnoise::available_models;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_host::{CountingAlloc, assert_no_allocs};
use sotf_plugin_speech_denoiser::model::MODEL_LABELS;
use sotf_plugin_speech_denoiser::{
    SPEECH_DENOISER_FRAME_SIZE, SPEECH_DENOISER_LATENCY_FRAMES, SpeechDenoiserModel,
    SpeechDenoiserPlugin, SpeechDenoiserPluginParams,
};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const RATE: u32 = 48000;
const LATENCY: usize = SPEECH_DENOISER_LATENCY_FRAMES;
const ALL_MODELS: [SpeechDenoiserModel; 3] = [
    SpeechDenoiserModel::RnnoiseFull,
    SpeechDenoiserModel::RnnoiseLegacyLq,
    SpeechDenoiserModel::RnnoiseLegacySh,
];

fn configured(
    channels: usize,
    enabled: bool,
    strength: f32,
    model: SpeechDenoiserModel,
) -> SpeechDenoiserPlugin {
    let mut plugin = SpeechDenoiserPlugin::from_params(
        channels,
        SpeechDenoiserPluginParams {
            enabled,
            strength,
            model,
        },
    );
    plugin.initialize(f64::from(RATE)).unwrap();
    plugin
}

fn process(
    plugin: &mut SpeechDenoiserPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut call = 0;
    while offset < input.len() / channels {
        let frames = blocks[call % blocks.len()].min(input.len() / channels - offset);
        let mut block = input[offset * channels..(offset + frames) * channels].to_vec();
        block.extend_from_slice(&[1234., -5678.]);
        assert_eq!(
            plugin
                .process_in_place(&mut block, &ProcessContext::new(RATE, frames))
                .unwrap(),
            frames
        );
        assert_eq!(&block[frames * channels..], &[1234., -5678.]);
        output.extend_from_slice(&block[..frames * channels]);
        offset += frames;
        call += 1;
    }
    output
}

fn drain_to_end(plugin: &mut SpeechDenoiserPlugin, channels: usize) -> Vec<f32> {
    let context = ProcessContext::new(RATE, 0);
    let mut output = Vec::new();
    let mut block = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
    loop {
        let result = plugin.drain(&mut block, &context).unwrap();
        output.extend_from_slice(&block[..result.frames * channels]);
        if result.complete {
            break;
        }
    }
    output
}

fn signal(frames: usize, channels: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    let mut out = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        let voice = (frame as f32 * 0.061).sin() * 0.25 + (frame as f32 * 0.122).sin() * 0.12;
        for _ in 0..channels {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let noise = (state as f32 / u32::MAX as f32 - 0.5) * 0.3;
            out.push((voice + noise).clamp(-0.9, 0.9));
        }
    }
    out
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn registry_identity_matches_backend_verbatim() {
    assert_eq!(MODEL_LABELS.len(), 3);
    assert_eq!(MODEL_LABELS[0], "RNNoise Full");
    assert_eq!(SpeechDenoiserModel::default(), SpeechDenoiserModel::RnnoiseFull);
    assert_eq!(SpeechDenoiserModel::default().index(), 0);
    let backend = available_models();
    assert_eq!(backend.len(), MODEL_LABELS.len());
    for (index, model) in ALL_MODELS.iter().enumerate() {
        assert_eq!(model.index(), index);
        assert_eq!(model.label(), MODEL_LABELS[index]);
        assert_eq!(model.label(), backend[index].label);
        assert_eq!(model.backend_id(), backend[index].id);
    }
    let plugin = SpeechDenoiserPlugin::new(1);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("model")),
        Some(ParameterValue::Int(0))
    );
}

#[test]
fn alternate_selection_changes_nonzero_audio() {
    for channels in [1, 2] {
        let input = signal(5 * SPEECH_DENOISER_FRAME_SIZE, channels, 0xA01D);
        let mut bundled = configured(channels, true, 1.0, SpeechDenoiserModel::RnnoiseFull);
        let reference = process(&mut bundled, &input, channels, &[137, 1]);
        let mut alternates = Vec::new();
        for model in [SpeechDenoiserModel::RnnoiseLegacyLq, SpeechDenoiserModel::RnnoiseLegacySh] {
            let mut plugin = configured(channels, true, 1.0, model);
            let output = process(&mut plugin, &input, channels, &[137, 1]);
            assert!(output.iter().all(|s| s.is_finite()), "{model:?}");
            let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            println!("{model:?} {channels}ch: peak={peak:.4}");
            assert!(peak > 1e-4, "{model:?} produced near-silence");
            let diff = max_abs_diff(&output, &reference);
            println!("{model:?} {channels}ch: max diff vs bundled={diff:.4}");
            assert_ne!(output, reference, "{model:?} matches bundled audio");
            alternates.push(output);
        }
        let cross = max_abs_diff(&alternates[0], &alternates[1]);
        println!("lq-vs-sh {channels}ch: max diff={cross:.4}");
        assert_ne!(alternates[0], alternates[1], "alternates match each other");
    }
}

#[test]
fn bypass_reset_partitions_eof_latency_hold_per_model() {
    for channels in [1, 2] {
        for model in ALL_MODELS {
            let input = signal(5 * SPEECH_DENOISER_FRAME_SIZE + 17, channels, 0x9E50);
            // Bypass replays the exact delayed dry signal for every model.
            let mut bypassed = configured(channels, false, 1.0, model);
            let bypassed_out = process(&mut bypassed, &input, channels, &[137, 1]);
            let expected: Vec<f32> = input
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    i.checked_sub(LATENCY * channels)
                        .map_or(0.0, |j| input[j])
                })
                .collect();
            assert_eq!(bypassed_out, expected, "{model:?}");
            // Odd partitions match one continuous stream bit-exactly.
            let mut continuous = configured(channels, true, 1.0, model);
            let mut partitioned = configured(channels, true, 1.0, model);
            assert_eq!(continuous.latency_samples(), LATENCY);
            assert_eq!(
                process(&mut partitioned, &input, channels, &[1, 137, 479, 481]),
                process(&mut continuous, &input, channels, &[8193]),
                "{model:?}"
            );
            // Reset returns to a fresh twin; EOF drain equals zero continuation.
            continuous.reset();
            let mut fresh = configured(channels, true, 1.0, model);
            assert_eq!(
                process(&mut continuous, &input, channels, &[137]),
                process(&mut fresh, &input, channels, &[137]),
                "{model:?}"
            );
            let mut draining = configured(channels, true, 1.0, model);
            let mut zeros = configured(channels, true, 1.0, model);
            let mut actual = process(&mut draining, &input, channels, &[137, 1]);
            let mut expected = process(&mut zeros, &input, channels, &[137, 1]);
            actual.extend(drain_to_end(&mut draining, channels));
            expected.extend(process(&mut zeros, &vec![0.0; LATENCY * channels], channels, &[17]));
            assert_eq!(actual, expected, "{model:?}");
            assert_eq!(
                draining.parametric_get_parameter(&ParameterId::from("model")),
                Some(ParameterValue::Int(model.index() as i32)),
                "{model:?} identity drifted"
            );
        }
    }
}

#[test]
fn fresh_restore_is_deterministic_per_model() {
    for channels in [1, 2] {
        for model in ALL_MODELS {
            let input = signal(2 * SPEECH_DENOISER_FRAME_SIZE + 55, channels, 0xE570);
            let params = SpeechDenoiserPluginParams {
                enabled: true,
                strength: 0.75,
                model,
            };
            let json = serde_json::to_string(&params).unwrap();
            let restored: SpeechDenoiserPluginParams = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.model, model);
            let mut from_factory = SpeechDenoiserPlugin::from_params(channels, restored);
            from_factory.initialize(f64::from(RATE)).unwrap();
            let mut live = SpeechDenoiserPlugin::new(channels);
            live.parametric_set_parameter(
                ParameterId::from("strength"),
                ParameterValue::Float(0.75),
            )
            .unwrap();
            live.parametric_set_parameter(
                ParameterId::from("model"),
                ParameterValue::Int(model.index() as i32),
            )
            .unwrap();
            live.initialize(f64::from(RATE)).unwrap();
            assert_eq!(
                process(&mut live, &input, channels, &[137, 1]),
                process(&mut from_factory, &input, channels, &[137, 1]),
                "{model:?}"
            );
        }
    }
}

#[test]
fn failed_model_restore_retains_accepted_plugin() {
    for channels in [1, 2] {
        let prefix = signal(1024, channels, 0xFA11);
        let suffix = signal(1024, channels, 0xE0FF);
        let mut plugin = configured(channels, true, 1.0, SpeechDenoiserModel::RnnoiseLegacyLq);
        let mut twin = configured(channels, true, 1.0, SpeechDenoiserModel::RnnoiseLegacyLq);
        let mut actual = process(&mut plugin, &prefix, channels, &[137]);
        let mut expected = process(&mut twin, &prefix, channels, &[137]);
        // Unknown identities and live changes both fail; the accepted model
        // and its populated history continue untouched.
        for rejected in [
            ParameterValue::Int(3),
            ParameterValue::String("RNNoise Light".to_string()),
            ParameterValue::Int(0),
            ParameterValue::Int(2),
        ] {
            assert!(
                plugin
                    .parametric_set_parameter(ParameterId::from("model"), rejected)
                    .is_err(),
                "channels={channels}"
            );
        }
        assert_eq!(
            plugin.parametric_get_parameter(&ParameterId::from("model")),
            Some(ParameterValue::Int(1))
        );
        assert!(serde_json::from_str::<SpeechDenoiserPluginParams>(r#"{"model":9}"#).is_err());
        actual.extend(process(&mut plugin, &suffix, channels, &[17]));
        expected.extend(process(&mut twin, &suffix, channels, &[17]));
        assert_eq!(actual, expected);
    }
}

#[test]
fn per_model_first_callback_and_drain_are_allocation_free() {
    for model in ALL_MODELS {
        for channels in [1, 2] {
            let mut plugin = configured(channels, true, 1.0, model);
            let mut buffer = vec![0.1; SPEECH_DENOISER_FRAME_SIZE * channels];
            let context = ProcessContext::new(RATE, SPEECH_DENOISER_FRAME_SIZE);
            assert_no_allocs("first callback", || {
                plugin.process_in_place(&mut buffer, &context).unwrap();
                plugin.process_in_place(&mut buffer, &context).unwrap();
            });
            let mut output = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
            let drain_context = ProcessContext::new(RATE, 0);
            assert_no_allocs("drain", || {
                let _ = plugin.drain(&mut output, &drain_context).unwrap();
            });
        }
    }
}
