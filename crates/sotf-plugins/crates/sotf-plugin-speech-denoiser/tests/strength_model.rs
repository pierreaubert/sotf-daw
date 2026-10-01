//! Suppression-strength blend and model-selection oracles.
//!
//! Covers SPEECH-DENOISER-R1/R2/R3/A2 at the public plugin surface: default
//! transparency against the directly driven backend, exact strength
//! endpoints, an independent f64 blend oracle, automation determinism,
//! transactional rejection with old-model continuation, drain freezing,
//! fresh-restore equivalence, stereo image, cold allocation freedom, and
//! schema metadata. All signals are deterministic; no external corpus.

// Rust guideline compliant 2026-02-21

use plugins_denoiser::rnnoise::RnnoiseBackend;
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::plugin::ProcessContext;
use sotf_host::{CountingAlloc, assert_no_allocs};
use sotf_plugin_speech_denoiser::model::MODEL_LABELS;
use sotf_plugin_speech_denoiser::{
    SPEECH_DENOISER_FRAME_SIZE, SPEECH_DENOISER_LATENCY_FRAMES, SpeechDenoiserData,
    SpeechDenoiserPlugin, SpeechDenoiserPluginParams,
};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const RATE: u32 = 48000;
const LATENCY: usize = SPEECH_DENOISER_LATENCY_FRAMES;

fn configured(channels: usize, enabled: bool, strength: f32) -> SpeechDenoiserPlugin {
    let mut plugin = SpeechDenoiserPlugin::from_params(
        channels,
        SpeechDenoiserPluginParams {
            enabled,
            strength,
            ..SpeechDenoiserPluginParams::default()
        },
    );
    plugin.initialize(RATE).unwrap();
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

fn drive_backend(input: &[f32], channels: usize, bypass: bool, blocks: &[usize]) -> Vec<f32> {
    let mut backend = RnnoiseBackend::new();
    backend.initialize(RATE, channels).unwrap();
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut call = 0;
    while offset < input.len() / channels {
        let frames = blocks[call % blocks.len()].min(input.len() / channels - offset);
        let mut block = input[offset * channels..(offset + frames) * channels].to_vec();
        assert_eq!(backend.process(&mut block, frames, channels, bypass), frames);
        output.extend_from_slice(&block);
        offset += frames;
        call += 1;
    }
    output
}

fn signal(frames: usize, channels: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames * channels)
        .map(|_| {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            (state as f32 / u32::MAX as f32 - 0.5) * 1.4
        })
        .collect()
}

fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1., 1.)
    } else {
        0.
    }
}

fn dry_oracle(input: &[f32], channels: usize) -> Vec<f32> {
    (0..input.len())
        .map(|i| {
            i.checked_sub(LATENCY * channels)
                .map_or(0., |j| sanitize(input[j]))
        })
        .collect()
}

fn analyzer_frames(plugin: &SpeechDenoiserPlugin) -> u64 {
    plugin
        .get_data()
        .unwrap()
        .downcast::<SpeechDenoiserData>()
        .unwrap()
        .model_frames
}

#[test]
fn latency_constant_matches_backend_and_documented_delay() {
    assert_eq!(SPEECH_DENOISER_LATENCY_FRAMES, 960);
    assert_eq!(SPEECH_DENOISER_FRAME_SIZE, 480);
    for channels in [1, 2] {
        let plugin = configured(channels, true, 1.0);
        assert_eq!(plugin.latency_samples(), LATENCY);
    }
}

#[test]
fn default_and_disabled_audio_match_the_backend_bit_exactly() {
    for channels in [1, 2] {
        let input = signal(3 * SPEECH_DENOISER_FRAME_SIZE + 73, channels, 0xC0FFEE);
        for blocks in [&[480][..], &[1, 137, 479, 481, 1024]] {
            let mut enabled_plugin = configured(channels, true, 1.0);
            assert_eq!(
                process(&mut enabled_plugin, &input, channels, blocks),
                drive_backend(&input, channels, false, blocks),
                "channels={channels}"
            );
            let mut disabled_plugin = configured(channels, false, 1.0);
            assert_eq!(
                process(&mut disabled_plugin, &input, channels, blocks),
                drive_backend(&input, channels, true, blocks),
                "channels={channels}"
            );
        }
    }
}

#[test]
fn strength_zero_enabled_matches_disabled_dry_exactly() {
    for channels in [1, 2] {
        let input = signal(4 * SPEECH_DENOISER_FRAME_SIZE + 31, channels, 0x5EED);
        let expected = dry_oracle(&input, channels);
        for blocks in [&[1][..], &[137, 8193], &[480], &[481]] {
            let mut zero = configured(channels, true, 0.0);
            assert_eq!(
                process(&mut zero, &input, channels, blocks),
                expected,
                "channels={channels}"
            );
            let mut disabled = configured(channels, false, 1.0);
            assert_eq!(
                process(&mut disabled, &input, channels, blocks),
                expected,
                "channels={channels}"
            );
        }
    }
}

#[test]
fn constant_strength_is_partition_invariant() {
    for channels in [1, 2] {
        let input = signal(5 * SPEECH_DENOISER_FRAME_SIZE + 17, channels, 0x9A8711);
        for strength in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let mut continuous = configured(channels, true, strength);
            let mut partitioned = configured(channels, true, strength);
            assert_eq!(
                process(&mut partitioned, &input, channels, &[1, 137, 479, 481, 1024]),
                process(&mut continuous, &input, channels, &[8193]),
                "channels={channels} strength={strength}"
            );
            assert_eq!(analyzer_frames(&partitioned), analyzer_frames(&continuous));
        }
    }
}

#[test]
fn mid_strength_matches_independent_f64_blend_oracle() {
    // Implementation rounding: three f32 operations per sample against an f64
    // reference, each within half an ulp of a signal bounded by 1.0. The
    // 5e-7 bound exceeds the worst-case ~1.8e-7 by a fixed margin.
    const BLEND_TOLERANCE: f32 = 5e-7;
    let mut worst = 0.0f32;
    for channels in [1, 2] {
        let input = signal(3 * SPEECH_DENOISER_FRAME_SIZE + 91, channels, 0xB1E4D);
        let dry = dry_oracle(&input, channels);
        let mut wet_plugin = configured(channels, true, 1.0);
        let wet = process(&mut wet_plugin, &input, channels, &[137, 1, 479]);
        for strength in [0.25, 0.5, 0.75] {
            let mut plugin = configured(channels, true, strength);
            let actual = process(&mut plugin, &input, channels, &[137, 1, 479]);
            assert_eq!(actual.len(), wet.len());
            assert_eq!(analyzer_frames(&plugin), analyzer_frames(&wet_plugin));
            for ((actual, wet), dry) in actual.iter().zip(&wet).zip(&dry) {
                let expected =
                    f64::from(*dry) + f64::from(strength) * (f64::from(*wet) - f64::from(*dry));
                let error = (*actual - expected as f32).abs();
                worst = worst.max(error);
                assert!(
                    error < BLEND_TOLERANCE,
                    "channels={channels} strength={strength}: error={error}"
                );
            }
        }
    }
    println!("strength blend oracle worst-case abs error: {worst}");
}

#[test]
fn strength_automation_slews_and_settles_deterministically() {
    for channels in [1, 2] {
        let prefix = signal(2000, channels, 0xA070);
        let suffix = signal(2000, channels, 0x5E77);
        let strength = ParameterId::from("strength");
        let mut first = configured(channels, true, 1.0);
        let mut second = configured(channels, true, 1.0);
        let mut first_out = process(&mut first, &prefix, channels, &[137]);
        let mut second_out = process(&mut second, &prefix, channels, &[137]);
        assert_eq!(first_out, second_out);
        for plugin in [&mut first, &mut second] {
            plugin
                .parametric_set_parameter(strength.clone(), ParameterValue::Float(0.0))
                .unwrap();
        }
        // Cross-partition twins: the 480-frame slew advances once per frame, so
        // different blockings of the same post-automation audio stay
        // bit-identical.
        first_out.extend(process(&mut first, &suffix, channels, &[479, 1]));
        second_out.extend(process(&mut second, &suffix, channels, &[17]));
        assert_eq!(first_out, second_out);
        // The 480-frame slew has settled long before the suffix ends, so the
        // tail equals the exact delayed dry signal bit-for-bit.
        let mut tail_input = prefix.clone();
        tail_input.extend_from_slice(&suffix);
        let dry = dry_oracle(&tail_input, channels);
        let tail = 480 * channels;
        assert_eq!(
            &first_out[first_out.len() - tail..],
            &dry[dry.len() - tail..]
        );
        // Restoring full strength returns to the wet path exactly.
        for plugin in [&mut first, &mut second] {
            plugin
                .parametric_set_parameter(strength.clone(), ParameterValue::Float(1.0))
                .unwrap();
        }
        let extra = signal(960, channels, 0xE7A);
        first_out.extend(process(&mut first, &extra, channels, &[137]));
        second_out.extend(process(&mut second, &extra, channels, &[17]));
        assert_eq!(first_out, second_out);
    }
}

#[test]
fn rejected_strength_writes_retain_config_and_audio() {
    for channels in [1, 2] {
        let prefix = signal(1024, channels, 0x9EC7);
        let suffix = signal(1024, channels, 0xC0DE);
        let mut plugin = configured(channels, true, 0.5);
        let mut twin = configured(channels, true, 0.5);
        let id = ParameterId::from("strength");
        let mut actual = process(&mut plugin, &prefix, channels, &[137]);
        let mut expected = process(&mut twin, &prefix, channels, &[137]);
        for invalid in [
            ParameterValue::Float(f32::NAN),
            ParameterValue::Float(f32::INFINITY),
            ParameterValue::Float(f32::NEG_INFINITY),
            ParameterValue::Float(-0.5),
            ParameterValue::Float(1.5),
            ParameterValue::Float(f32::MAX),
            ParameterValue::Int(1),
            ParameterValue::Bool(true),
            ParameterValue::String("full".to_string()),
        ] {
            assert!(
                plugin.parametric_set_parameter(id.clone(), invalid).is_err(),
                "channels={channels}"
            );
        }
        assert_eq!(
            plugin.parametric_get_parameter(&id),
            Some(ParameterValue::Float(0.5))
        );
        actual.extend(process(&mut plugin, &suffix, channels, &[17]));
        expected.extend(process(&mut twin, &suffix, channels, &[17]));
        assert_eq!(actual, expected);
    }
}

#[test]
fn model_selection_validates_and_continues_on_failure() {
    for channels in [1, 2] {
        let prefix = signal(1024, channels, 0xA0DE1);
        let suffix = signal(1024, channels, 0xFA11);
        let id = ParameterId::from("model");
        // Same-value adoption is a no-op before and after initialization.
        let mut pre = SpeechDenoiserPlugin::new(channels);
        pre.parametric_set_parameter(id.clone(), ParameterValue::Int(0))
            .unwrap();
        pre.parametric_set_parameter(
            id.clone(),
            ParameterValue::String("rnnoise full".to_string()),
        )
        .unwrap();
        assert!(
            pre.parametric_set_parameter(id.clone(), ParameterValue::Int(1))
                .is_err()
        );
        let mut plugin = configured(channels, true, 1.0);
        let mut twin = configured(channels, true, 1.0);
        let mut actual = process(&mut plugin, &prefix, channels, &[137]);
        let mut expected = process(&mut twin, &prefix, channels, &[137]);
        plugin
            .parametric_set_parameter(id.clone(), ParameterValue::Int(0))
            .unwrap();
        plugin
            .parametric_set_parameter(
                id.clone(),
                ParameterValue::String("RNNoise Full".to_string()),
            )
            .unwrap();
        for invalid in [
            ParameterValue::Int(1),
            ParameterValue::Int(-1),
            ParameterValue::String("RNNoise Light".to_string()),
            ParameterValue::String(String::new()),
            ParameterValue::Float(0.0),
            ParameterValue::Bool(true),
        ] {
            assert!(
                plugin.parametric_set_parameter(id.clone(), invalid).is_err(),
                "channels={channels}"
            );
        }
        assert_eq!(
            plugin.parametric_get_parameter(&id),
            Some(ParameterValue::Int(0))
        );
        actual.extend(process(&mut plugin, &suffix, channels, &[17]));
        expected.extend(process(&mut twin, &suffix, channels, &[17]));
        assert_eq!(actual, expected);
    }
}

#[test]
fn mixed_batch_with_rejected_entry_is_atomic() {
    // Transactionality regression (review P1-5, COMMON §2): a batch carrying
    // one rejected entry must leave the accepted configuration and the audio
    // history bit-identical to an untouched twin, regardless of map order
    // (`ParameterSet` iterates deterministically, but the pre-check pass makes
    // order irrelevant by construction).
    //
    // With a single bundled model, a changed model entry is rejected as an
    // unknown identity; a valid-but-live model change (structural rebuild
    // path) becomes testable once the shared multi-model backend lands, and
    // rides the same pre-check code as the drain-freeze case below.
    for channels in [1, 2] {
        let strength = ParameterId::from("strength");
        let model = ParameterId::from("model");
        let enabled = ParameterId::from("enabled");
        // Live instance: a valid strength change paired with a rejected model
        // entry applies nothing.
        let prefix = signal(1024, channels, 0xBA7C);
        let suffix = signal(1024, channels, 0xA70C);
        let mut plugin = configured(channels, true, 0.5);
        let mut twin = configured(channels, true, 0.5);
        let mut actual = process(&mut plugin, &prefix, channels, &[137]);
        let mut expected = process(&mut twin, &prefix, channels, &[137]);
        let mut mixed = ParameterSet::new();
        mixed.insert(strength.clone(), ParameterValue::Float(0.7));
        mixed.insert(model.clone(), ParameterValue::Int(1));
        assert!(plugin.apply_values_realtime(&mixed).is_err());
        assert_eq!(
            plugin.parametric_get_parameter(&strength),
            Some(ParameterValue::Float(0.5))
        );
        assert_eq!(
            plugin.parametric_get_parameter(&model),
            Some(ParameterValue::Int(0))
        );
        actual.extend(process(&mut plugin, &suffix, channels, &[17]));
        expected.extend(process(&mut twin, &suffix, channels, &[17]));
        assert_eq!(actual, expected);
        // Post-drain freeze: individually frozen entries are rejected together
        // without touching the remaining drain program.
        let mut plugin = configured(channels, true, 0.5);
        let mut twin = configured(channels, true, 0.5);
        let input = signal(1024, channels, 0xD8A17);
        let mut actual = process(&mut plugin, &input, channels, &[137]);
        let mut expected = process(&mut twin, &input, channels, &[137]);
        let context = ProcessContext::new(RATE, 0);
        for (instance, history) in [(&mut plugin, &mut actual), (&mut twin, &mut expected)] {
            let mut first = vec![0.0; channels];
            let result = instance.drain(&mut first, &context).unwrap();
            history.extend_from_slice(&first[..result.frames * channels]);
        }
        let mut frozen = ParameterSet::new();
        frozen.insert(strength.clone(), ParameterValue::Float(0.7));
        frozen.insert(enabled.clone(), ParameterValue::Bool(false));
        assert!(plugin.apply_values_realtime(&frozen).is_err());
        assert_eq!(
            plugin.parametric_get_parameter(&strength),
            Some(ParameterValue::Float(0.5))
        );
        assert_eq!(
            plugin.parametric_get_parameter(&enabled),
            Some(ParameterValue::Bool(true))
        );
        let mut block = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
        for (instance, history) in [(&mut plugin, &mut actual), (&mut twin, &mut expected)] {
            loop {
                let result = instance.drain(&mut block, &context).unwrap();
                history.extend_from_slice(&block[..result.frames * channels]);
                if result.complete {
                    break;
                }
            }
        }
        assert_eq!(actual, expected);
    }
}

#[test]
fn drain_freezes_strength_and_model_until_reset() {
    for channels in [1, 2] {
        for enabled in [false, true] {
            let input = signal(3 * SPEECH_DENOISER_FRAME_SIZE + 7, channels, 0xD9A14);
            let mut plugin = configured(channels, enabled, 1.0);
            let mut reference = configured(channels, enabled, 1.0);
            let mut actual = process(&mut plugin, &input, channels, &[137]);
            let mut expected = process(&mut reference, &input, channels, &[137]);
            let strength = ParameterId::from("strength");
            let model = ParameterId::from("model");
            let context = ProcessContext::new(RATE, 0);
            let mut first = vec![0.0; channels];
            let result = plugin.drain(&mut first, &context).unwrap();
            assert_eq!(result.frames, 1);
            actual.extend_from_slice(&first);
            // Changed controls are frozen; same-value snapshots stay accepted.
            assert!(
                plugin
                    .parametric_set_parameter(strength.clone(), ParameterValue::Float(0.5))
                    .is_err()
            );
            plugin
                .parametric_set_parameter(strength.clone(), ParameterValue::Float(1.0))
                .unwrap();
            plugin
                .parametric_set_parameter(model.clone(), ParameterValue::Int(0))
                .unwrap();
            let mut values = ParameterSet::new();
            values.insert(strength.clone(), ParameterValue::Float(0.5));
            assert!(plugin.apply_values_realtime(&values).is_err());
            let mut block = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
            loop {
                let result = plugin.drain(&mut block, &context).unwrap();
                actual.extend_from_slice(&block[..result.frames * channels]);
                if result.complete {
                    break;
                }
            }
            expected.extend(process(
                &mut reference,
                &vec![0.0; LATENCY * channels],
                channels,
                &[SPEECH_DENOISER_FRAME_SIZE],
            ));
            assert_eq!(actual, expected);
            assert!(
                plugin
                    .parametric_set_parameter(strength.clone(), ParameterValue::Float(0.5))
                    .is_err()
            );
            plugin.reset();
            plugin
                .parametric_set_parameter(strength, ParameterValue::Float(0.5))
                .unwrap();
        }
    }
}

#[test]
fn enabled_drain_applies_strength_like_zero_continuation() {
    for channels in [1, 2] {
        let input = signal(2 * SPEECH_DENOISER_FRAME_SIZE + 129, channels, 0xD0A1);
        let mut plugin = configured(channels, true, 0.5);
        let mut reference = configured(channels, true, 0.5);
        let mut actual = process(&mut plugin, &input, channels, &[137, 1]);
        let mut expected = process(&mut reference, &input, channels, &[481]);
        let context = ProcessContext::new(RATE, 0);
        let mut block = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
        loop {
            let result = plugin.drain(&mut block, &context).unwrap();
            actual.extend_from_slice(&block[..result.frames * channels]);
            if result.complete {
                break;
            }
        }
        expected.extend(process(
            &mut reference,
            &vec![0.0; LATENCY * channels],
            channels,
            &[17, SPEECH_DENOISER_FRAME_SIZE],
        ));
        assert_eq!(actual, expected);
    }
}

#[test]
fn fresh_restore_matches_live_configured_twin() {
    for channels in [1, 2] {
        for enabled in [false, true] {
            let input = signal(2 * SPEECH_DENOISER_FRAME_SIZE + 55, channels, 0xE570);
            let params = SpeechDenoiserPluginParams {
                enabled,
                strength: 0.25,
                ..SpeechDenoiserPluginParams::default()
            };
            // Factory restore from serialized state.
            let json = serde_json::to_string(&params).unwrap();
            let restored: SpeechDenoiserPluginParams = serde_json::from_str(&json).unwrap();
            let mut from_factory = SpeechDenoiserPlugin::from_params(channels, restored);
            from_factory.initialize(RATE).unwrap();
            // Live twin configured before its first audio callback.
            let mut live = SpeechDenoiserPlugin::new(channels);
            live.parametric_set_parameter(
                ParameterId::from("enabled"),
                ParameterValue::Bool(enabled),
            )
            .unwrap();
            live.parametric_set_parameter(
                ParameterId::from("strength"),
                ParameterValue::Float(0.25),
            )
            .unwrap();
            live.parametric_set_parameter(
                ParameterId::from("model"),
                ParameterValue::String("RNNoise Full".to_string()),
            )
            .unwrap();
            live.initialize(RATE).unwrap();
            // Snapshot restore through the borrowed realtime path.
            let mut snapshot = SpeechDenoiserPlugin::new(channels);
            snapshot.apply_values_realtime(&from_factory.current_values()).unwrap();
            snapshot.initialize(RATE).unwrap();
            let expected = process(&mut from_factory, &input, channels, &[137, 1]);
            assert_eq!(process(&mut live, &input, channels, &[137, 1]), expected);
            assert_eq!(process(&mut snapshot, &input, channels, &[137, 1]), expected);
        }
    }
}

#[test]
fn stereo_image_survives_all_strengths() {
    // Swap-invariance bound: the backend holds channel-swap error under 1e-6
    // and the per-channel blend scales that error by at most the strength, so
    // 2e-6 covers the blend plus one f32 rounding.
    const SWAP_TOLERANCE: f32 = 2e-6;
    let mut worst = 0.0f32;
    for strength in [0.0, 0.5, 1.0] {
        let frames = 3 * SPEECH_DENOISER_FRAME_SIZE;
        let mut input = vec![0.0; frames * 2];
        for frame in 0..frames {
            let sample = (frame as f32 * 0.071).sin() * 0.35;
            input[2 * frame] = sample;
            input[2 * frame + 1] = -sample;
        }
        let mut swapped = input.clone();
        for frame in swapped.as_chunks_mut::<2>().0 {
            frame.swap(0, 1);
        }
        let mut direct = configured(2, true, strength);
        let output = process(&mut direct, &input, 2, &[137, 1]);
        // Skip the latency region: anti-phase energy and image live in the
        // delayed program, not in the startup zeros.
        let program = &output[LATENCY * 2..];
        let left: f32 = program.iter().step_by(2).map(|s| s * s).sum();
        let right: f32 = program.iter().skip(1).step_by(2).map(|s| s * s).sum();
        let cross: f32 = program
            .as_chunks::<2>()
            .0
            .iter()
            .map(|stereo| stereo[0] * stereo[1])
            .sum();
        assert!(left > 1e-5, "strength={strength}: left collapsed");
        assert!(right > 1e-5, "strength={strength}: right collapsed");
        assert!(
            cross < -0.9 * (left * right).sqrt(),
            "strength={strength}: image not preserved"
        );
        let mut twin = configured(2, true, strength);
        let mut swapped_output = process(&mut twin, &swapped, 2, &[137, 1]);
        for frame in swapped_output.as_chunks_mut::<2>().0 {
            frame.swap(0, 1);
        }
        // Strength 0 is an exact per-channel delay, hence bit-exact here.
        if strength == 0.0 {
            assert_eq!(swapped_output, output);
        } else {
            for (direct, swapped) in output.iter().zip(&swapped_output) {
                let error = (direct - swapped).abs();
                worst = worst.max(error);
                assert!(
                    error < SWAP_TOLERANCE,
                    "strength={strength}: swap error={error}"
                );
            }
        }
    }
    println!("stereo swap worst-case abs error: {worst}");
}

#[test]
fn strength_path_is_cold_allocation_free() {
    for channels in [1, 2] {
        let mut plugin = configured(channels, true, 1.0);
        let mut buffer = vec![0.1; SPEECH_DENOISER_FRAME_SIZE * channels];
        let context = ProcessContext::new(RATE, SPEECH_DENOISER_FRAME_SIZE);
        let strength = ParameterId::from("strength");
        let model = ParameterId::from("model");
        assert_no_allocs("Speech Denoiser strength automation", || {
            plugin
                .parametric_set_parameter(strength.clone(), ParameterValue::Float(0.5))
                .unwrap();
            plugin.process_in_place(&mut buffer, &context).unwrap();
            // Same-value structural writes are no-ops safe to receive on the
            // callback; the Int path borrows and allocates nothing.
            plugin
                .parametric_set_parameter(model.clone(), ParameterValue::Int(0))
                .unwrap();
            let _ = plugin.parametric_get_parameter(&strength);
            let _ = plugin.parametric_get_parameter(&model);
            plugin
                .parametric_set_parameter(strength.clone(), ParameterValue::Float(1.0))
                .unwrap();
            plugin.process_in_place(&mut buffer, &context).unwrap();
        });
        let mut output = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
        let drain_context = ProcessContext::new(RATE, 0);
        assert_no_allocs("Speech Denoiser strength drain", || {
            let _ = plugin.drain(&mut output, &drain_context).unwrap();
        });
    }
}

#[test]
fn schema_metadata_names_ranges_and_update_modes() {
    use sotf_host::plugin_params::PluginParamDef;
    use sotf_plugin_speech_denoiser::params::{LAYOUT, PARAMS, Params};

    assert_eq!(<Params as PluginParamDef>::VERSION, 2);
    assert_eq!(PARAMS.len(), 3);
    let strength = &PARAMS[1];
    assert_eq!(strength.engine_key, "strength");
    assert_eq!(strength.min_f64(), 0.0);
    assert_eq!(strength.max_f64(), 1.0);
    assert_eq!(strength.default_f64(), 1.0);
    let model = &PARAMS[2];
    assert_eq!(model.engine_key, "model");
    assert_eq!(model.choice_labels(), MODEL_LABELS);
    let plugin = SpeechDenoiserPlugin::new(1);
    let schema = plugin.parameter_schema();
    assert_eq!(schema[0].update_mode, UpdateMode::Realtime);
    assert_eq!(schema[1].update_mode, UpdateMode::Realtime);
    assert_eq!(schema[2].update_mode, UpdateMode::Structural);
    assert!(LAYOUT.validate(PARAMS.len(), "speech_denoiser").is_empty());
    assert_eq!(LAYOUT.referenced_indices(), vec![0, 1, 2]);
}
