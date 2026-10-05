#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

// Rust guideline compliant 2026-02-21

use sotf_host::DawHost;
use sotf_host::external_plugin::{
    ExternalPlugin, ExternalPluginState, NativeBandSplitOutputLayout, NativePluginAudioSetup,
    PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::serialization::SerializablePlugin;
use std::f32::consts::TAU;
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 1024;
/// Guard samples bracket callback output so writes beyond its slice are visible.
const OUTPUT_GUARD_SAMPLES: usize = 8;
/// A finite value far outside the expected audio range marks untouched guards.
const OUTPUT_CANARY: f32 = f32::from_bits(0x4f12_3456);
const BAND_SPLIT_CLAP_ID: &str = "org.spinorama.sotf.band-split";
const BAND_SPLIT_VST3_CLASS_ID: &str = "536F746642616E6453706C7430303031";

#[test]
#[ignore = "requires SOTF_TEST_BANDSPLIT_CLAP_PLUGIN to point to the captured BandSplit CLAP library"]
fn exported_clap_band_counts_process_and_restore_both_channel_pairs() {
    verify_band_split_routes(
        PluginFormat::Clap,
        BAND_SPLIT_CLAP_ID,
        "SOTF_TEST_BANDSPLIT_CLAP_PLUGIN",
        NativeBandSplitOutputLayout::ClapPacked,
    );
}

#[test]
#[ignore = "requires SOTF_TEST_BANDSPLIT_VST3_PLUGIN to point to the captured BandSplit VST3 bundle"]
fn exported_vst3_bus_routes_process_and_restore_all_band_counts() {
    verify_band_split_routes(
        PluginFormat::Vst3,
        BAND_SPLIT_VST3_CLASS_ID,
        "SOTF_TEST_BANDSPLIT_VST3_PLUGIN",
        NativeBandSplitOutputLayout::Vst3Buses,
    );
}

#[test]
#[ignore = "requires SOTF_TEST_BANDSPLIT_VST3_PLUGIN to point to the captured BandSplit VST3 bundle"]
fn exported_vst3_old_two_parameter_state_uses_legacy_packed_layout() {
    let descriptor = descriptor(
        PluginFormat::Vst3,
        BAND_SPLIT_VST3_CLASS_ID,
        "SOTF_TEST_BANDSPLIT_VST3_PLUGIN",
    );
    let legacy_setup = band_split_setup(2, NativeBandSplitOutputLayout::Vst3LegacyPacked);
    let seed = ExternalPlugin::new_with_audio_setup(&descriptor, legacy_setup.clone(), SAMPLE_RATE)
        .expect("create legacy-compatible VST3 BandSplit instance");
    let mut old_state = save_state(&seed);
    edit_native_state(&mut old_state.opaque_state, PluginFormat::Vst3, |state| {
        let parameters = state
            .get_mut("params")
            .and_then(serde_json::Value::as_object_mut)
            .expect("VST3 state contains a parameter map");
        parameters.retain(|key, _| {
            !matches!(
                key.as_str(),
                "recombination_mode" | "num_bands" | "frequency_2" | "frequency_3"
            )
        });
    });
    old_state.audio_setup = None;

    let mut restored = ExternalPlugin::from_placeholder_state(&old_state, SAMPLE_RATE)
        .expect("restore historical two-control state through packed bus compatibility");
    assert_eq!(restored.input_channels(), 2);
    assert_eq!(restored.output_channels(), 4);
    assert_eq!(restored.audio_setup(), Some(&legacy_setup));
    let restored_state = save_state(&restored);
    assert_eq!(
        native_state_int(
            &restored_state.opaque_state,
            PluginFormat::Vst3,
            "num_bands"
        ),
        Some(0),
        "an old state with no band count defaults to two bands"
    );
    assert_eq!(
        native_state_int(
            &restored_state.opaque_state,
            PluginFormat::Vst3,
            "recombination_mode"
        ),
        Some(0),
        "an old state with no mode defaults to the legacy cascade"
    );
    assert_distinct_band_major_audio(&mut restored, 2);
}

#[test]
#[ignore = "requires SOTF_TEST_BANDSPLIT_CLAP_PLUGIN to point to the captured BandSplit CLAP library"]
fn exported_clap_rejects_conflicting_candidate_without_losing_populated_audio() {
    verify_conflicting_candidate_is_transactional(
        PluginFormat::Clap,
        BAND_SPLIT_CLAP_ID,
        "SOTF_TEST_BANDSPLIT_CLAP_PLUGIN",
        NativeBandSplitOutputLayout::ClapPacked,
    );
}

#[test]
#[ignore = "requires SOTF_TEST_BANDSPLIT_VST3_PLUGIN to point to the captured BandSplit VST3 bundle"]
fn exported_vst3_rejects_conflicting_candidate_without_losing_populated_audio() {
    verify_conflicting_candidate_is_transactional(
        PluginFormat::Vst3,
        BAND_SPLIT_VST3_CLASS_ID,
        "SOTF_TEST_BANDSPLIT_VST3_PLUGIN",
        NativeBandSplitOutputLayout::Vst3Buses,
    );
}

fn verify_conflicting_candidate_is_transactional(
    format: PluginFormat,
    plugin_id: &str,
    library_env: &str,
    output_layout: NativeBandSplitOutputLayout,
) {
    let descriptor = descriptor(format, plugin_id, library_env);
    let input = distinct_stereo_input();
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);

    for slope_index in 0..=1 {
        for mode_index in 0..=1 {
            let setup = band_split_setup(4, output_layout);
            let mut live =
                ExternalPlugin::new_with_audio_setup(&descriptor, setup.clone(), SAMPLE_RATE)
                    .unwrap_or_else(|error| panic!("load exported {format:?} BandSplit: {error}"));
            let mut saved = save_state(&live);
            edit_native_state(&mut saved.opaque_state, format, |state| {
                set_state_int(state, "crossover_type", slope_index);
                set_state_int(state, "recombination_mode", mode_index);
            });
            live.load_opaque_state(&saved.opaque_state)
                .unwrap_or_else(|error| panic!("set {format:?} BandSplit mode/slope: {error}"));
            let populated_state = save_state(&live);
            let mut control = ExternalPlugin::from_placeholder_state(&populated_state, SAMPLE_RATE)
                .unwrap_or_else(|error| panic!("construct {format:?} control twin: {error}"));

            for _ in 0..2 {
                let _ = render_block(&mut live, &input, &context, 8);
                let _ = render_block(&mut control, &input, &context, 8);
            }
            let before = save_state(&live);
            assert_eq!(before.audio_setup, Some(setup.clone()));
            assert_eq!(live.output_channels(), 8);

            // The outer setup remains a valid four-band request, but the
            // serialized native count conflicts with its four-bus geometry.
            // This makes the disposable replacement instance perform a real
            // native state restore before readback rejects the candidate.
            let mut conflicting = before.clone();
            edit_native_state(&mut conflicting.opaque_state, format, |state| {
                set_state_int(state, "num_bands", 0);
            });
            conflicting.validate().unwrap_or_else(|error| {
                panic!("conflicting native payload must pass outer state validation: {error}")
            });
            let mut preset = live
                .serialize()
                .unwrap_or_else(|error| panic!("serialize {format:?} BandSplit: {error}"));
            preset
                .set_external_plugin_state(&conflicting)
                .unwrap_or_else(|error| panic!("build conflicting candidate preset: {error}"));

            let failure = live
                .deserialize(&preset)
                .expect_err("native state with two-band count must not replace a four-band route");
            let failure_message = failure.to_string();
            assert!(
                failure_message.contains("rejected persisted state")
                    || failure_message.contains("failed to restore component state")
                    || failure_message.contains("native BandSplit structural parameters"),
                "candidate must reach native restore or structural readback, got: {failure_message}"
            );

            let after = save_state(&live);
            assert_eq!(
                after, before,
                "failed candidate must preserve complete live metadata and saved state"
            );
            assert_eq!(live.input_channels(), 2);
            assert_eq!(live.output_channels(), 8);

            let mut cold = ExternalPlugin::from_placeholder_state(&before, SAMPLE_RATE)
                .unwrap_or_else(|error| panic!("construct cold {format:?} twin: {error}"));
            let live_audio = render_block(&mut live, &input, &context, 8);
            let control_audio = render_block(&mut control, &input, &context, 8);
            let cold_audio = render_block(&mut cold, &input, &context, 8);
            // Both twins loaded the same binary state and received the same two warmup blocks.
            assert_waveforms_equal(
                &live_audio,
                &control_audio,
                &format!("{format:?} rejected candidate preserves populated DSP state"),
            );
            let cold_error = max_waveform_error(&control_audio, &cold_audio);
            assert!(
                cold_error > 1.0e-6,
                "the cold sensitivity twin did not distinguish populated recursive state: {cold_error}"
            );

            let retry_state = save_state(&control);
            retry_state
                .validate()
                .unwrap_or_else(|error| panic!("synchronized valid retry must validate: {error}"));
            let mut retry_preset = control
                .serialize()
                .unwrap_or_else(|error| panic!("serialize valid {format:?} retry: {error}"));
            retry_preset
                .set_external_plugin_state(&retry_state)
                .unwrap_or_else(|error| panic!("set valid {format:?} retry state: {error}"));
            live.deserialize(&retry_preset)
                .unwrap_or_else(|error| panic!("valid {format:?} candidate retry: {error}"));
            assert_eq!(save_state(&live), retry_state);
            let mut retry_reference =
                ExternalPlugin::from_placeholder_state(&retry_state, SAMPLE_RATE).unwrap_or_else(
                    |error| panic!("construct valid {format:?} retry reference: {error}"),
                );
            let retried_audio = render_block(&mut live, &input, &context, 8);
            let retry_reference_audio = render_block(&mut retry_reference, &input, &context, 8);
            assert_waveforms_match(
                &retried_audio,
                &retry_reference_audio,
                &format!("{format:?} valid candidate retry after refusal"),
            );
        }
    }
}

fn verify_band_split_routes(
    format: PluginFormat,
    plugin_id: &str,
    library_env: &str,
    output_layout: NativeBandSplitOutputLayout,
) {
    let descriptor = descriptor(format, plugin_id, library_env);
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
    let input = distinct_stereo_input();

    for slope_index in 0..=1 {
        for mode_index in 0..=1 {
            let initial_setup = band_split_setup(2, output_layout);
            let mut plugin = ExternalPlugin::new_with_audio_setup(
                &descriptor,
                initial_setup.clone(),
                SAMPLE_RATE,
            )
            .unwrap_or_else(|error| panic!("load exported {format:?} BandSplit: {error}"));
            assert_eq!(plugin.audio_setup(), Some(&initial_setup));
            assert_eq!(plugin.output_channels(), 4);

            let mut parameter_state = save_state(&plugin).opaque_state;
            edit_native_state(&mut parameter_state, format, |state| {
                set_state_int(state, "crossover_type", slope_index);
                set_state_int(state, "recombination_mode", mode_index);
            });
            plugin
                .load_opaque_state(&parameter_state)
                .unwrap_or_else(|error| panic!("restore {format:?} slope/mode state: {error}"));

            for num_bands in 2..=4 {
                let setup = band_split_setup(num_bands, output_layout);
                if num_bands != 2 {
                    plugin
                        .reconfigure_audio_setup(setup.clone())
                        .unwrap_or_else(|error| {
                            panic!(
                                "reconfigure {format:?} to {num_bands} bands, slope {slope_index}, mode {mode_index}: {error}"
                            )
                        });
                }
                assert_eq!(plugin.audio_setup(), Some(&setup));
                assert_eq!(plugin.input_channels(), 2);
                assert_eq!(plugin.output_channels(), num_bands * 2);

                let saved = save_state(&plugin);
                assert_eq!(saved.audio_setup, Some(setup.clone()));
                assert_eq!(
                    native_state_int(&saved.opaque_state, format, "crossover_type"),
                    Some(slope_index as i64),
                    "slope choice must survive the {num_bands}-band layout"
                );
                assert_eq!(
                    native_state_int(&saved.opaque_state, format, "recombination_mode"),
                    Some(mode_index as i64),
                    "mode choice must survive the {num_bands}-band layout"
                );
                assert_eq!(
                    native_state_int(&saved.opaque_state, format, "num_bands"),
                    Some((num_bands - 2) as i64),
                    "the serialized hidden count must match the selected bus geometry"
                );

                let mut restored = ExternalPlugin::from_placeholder_state(&saved, SAMPLE_RATE)
                    .unwrap_or_else(|error| {
                        panic!("restore {format:?} {num_bands}-band state: {error}")
                    });
                let host_restored = ExternalPlugin::from_placeholder_state(&saved, SAMPLE_RATE)
                    .unwrap_or_else(|error| {
                        panic!("restore {format:?} state for DawHost: {error}")
                    });
                assert_eq!(restored.audio_setup(), Some(&setup));
                assert_eq!(restored.output_channels(), num_bands * 2);

                let actual = render_block(&mut plugin, &input, &context, num_bands * 2);
                let restored_audio = render_block(&mut restored, &input, &context, num_bands * 2);
                let reference = render_public_band_split_reference(
                    &input,
                    num_bands,
                    slope_index as usize,
                    mode_index as usize,
                );
                assert_distinct_pairs(&actual, num_bands);
                assert_distinct_pairs(&restored_audio, num_bands);
                assert_waveforms_match(
                    &actual,
                    &restored_audio,
                    &format!("{format:?} {num_bands}-band state reload"),
                );
                assert_waveforms_match(
                    &actual,
                    &reference,
                    &format!("{format:?} {num_bands}-band public DSP reference"),
                );

                let mut host = DawHost::new(2, SAMPLE_RATE);
                host.add_plugin(Box::new(host_restored))
                    .unwrap_or_else(|error| {
                        panic!(
                            "add {format:?} {num_bands}-band external plugin to DawHost: {error}"
                        )
                    });
                assert_eq!(host.input_channels(), 2);
                assert_eq!(host.output_channels(), num_bands * 2);
                let host_audio = render_daw_host_partitioned(&mut host, &input, num_bands * 2);
                assert_distinct_pairs(&host_audio, num_bands);
                assert_waveforms_match(
                    &reference,
                    &host_audio,
                    &format!("{format:?} {num_bands}-band DawHost chain"),
                );
            }
        }
    }
}

fn band_split_setup(
    num_bands: usize,
    output_layout: NativeBandSplitOutputLayout,
) -> NativePluginAudioSetup {
    NativePluginAudioSetup::BandSplit {
        num_bands: num_bands as u8,
        output_layout,
    }
}

fn descriptor(format: PluginFormat, id: &str, library_env: &str) -> PluginDescriptor {
    let library_path = PathBuf::from(
        std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
    );
    PluginDescriptor {
        id: id.into(),
        name: "SOTF: Band Split".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: library_path,
        audio_inputs: 2,
        audio_outputs: 8,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

fn save_state(plugin: &ExternalPlugin) -> ExternalPluginState {
    plugin
        .serialize()
        .expect("serialize BandSplit native plugin")
        .external_plugin_state()
        .expect("read external plugin state envelope")
        .expect("serialized state contains external plugin data")
}

fn distinct_stereo_input() -> Vec<f32> {
    let mut input = vec![0.0; FRAMES * 2];
    for frame in 0..FRAMES {
        let time = frame as f32 / SAMPLE_RATE as f32;
        let left = (TAU * 80.0 * time).sin() * 0.12
            + (TAU * 700.0 * time).sin() * 0.09
            + (TAU * 2_400.0 * time).sin() * 0.07
            + (TAU * 9_000.0 * time).sin() * 0.05;
        let right = (TAU * 120.0 * time).sin() * 0.08
            + (TAU * 900.0 * time).sin() * 0.11
            + (TAU * 3_000.0 * time).sin() * 0.06
            + (TAU * 11_000.0 * time).sin() * 0.1;
        input[frame * 2] = left;
        input[frame * 2 + 1] = right;
    }
    input
}

fn render_block(
    plugin: &mut ExternalPlugin,
    input: &[f32],
    context: &ProcessContext<'_>,
    output_channels: usize,
) -> Vec<f32> {
    let output_len = context.num_frames * output_channels;
    let output_start = OUTPUT_GUARD_SAMPLES;
    let output_end = output_start + output_len;
    let mut guarded_output = vec![OUTPUT_CANARY; output_len + OUTPUT_GUARD_SAMPLES * 2];
    guarded_output[output_start..output_end].fill(f32::NAN);
    assert_eq!(
        plugin
            .process(
                input,
                &mut guarded_output[output_start..output_end],
                context
            )
            .expect("process actual external plugin audio"),
        context.num_frames
    );
    assert!(
        guarded_output[..output_start]
            .iter()
            .chain(&guarded_output[output_end..])
            .all(|sample| sample.to_bits() == OUTPUT_CANARY.to_bits())
    );
    let output = guarded_output[output_start..output_end].to_vec();
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "external route output must remain finite"
    );
    output
}

fn render_public_band_split_reference(
    input: &[f32],
    num_bands: usize,
    slope_index: usize,
    mode_index: usize,
) -> Vec<f32> {
    let frequencies = match num_bands {
        2 => vec![300.0],
        3 => vec![300.0, 1_200.0],
        4 => vec![300.0, 1_200.0, 4_800.0],
        _ => panic!("unsupported BandSplit reference width {num_bands}"),
    };
    let crossover_type = match slope_index {
        0 => "LR24",
        1 => "LR48",
        _ => panic!("unsupported BandSplit slope index {slope_index}"),
    };
    let recombination_mode = match mode_index {
        0 => "legacy_cascade",
        1 => "phase_compensated",
        _ => panic!("unsupported BandSplit mode index {mode_index}"),
    };
    let parameters = serde_json::json!({
        "explicit_frequencies": frequencies,
        "num_bands": num_bands,
        "type": crossover_type,
        "recombination_mode": recombination_mode,
    });
    let mut plugin = sotf_plugins::create_plugin("band_split", &parameters, 2, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("construct public BandSplit reference: {error}"));
    plugin
        .initialize(f64::from(SAMPLE_RATE))
        .unwrap_or_else(|error| panic!("initialize public BandSplit reference: {error}"));

    let output_channels = num_bands * 2;
    let mut output = vec![f32::NAN; FRAMES * output_channels];
    let mut frame_offset = 0;
    for block_frames in [127, 509, 383, 5] {
        let input_start = frame_offset * 2;
        let input_end = input_start + block_frames * 2;
        let output_start = frame_offset * output_channels;
        let output_end = output_start + block_frames * output_channels;
        let context = ProcessContext::new(SAMPLE_RATE, block_frames);
        assert_eq!(
            plugin
                .process(
                    &input[input_start..input_end],
                    &mut output[output_start..output_end],
                    &context,
                )
                .unwrap_or_else(|error| panic!("process public BandSplit reference: {error}")),
            block_frames
        );
        frame_offset += block_frames;
    }
    assert_eq!(frame_offset, FRAMES);
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn render_daw_host_partitioned(
    host: &mut DawHost,
    input: &[f32],
    output_channels: usize,
) -> Vec<f32> {
    let output_len = FRAMES * output_channels;
    let output_start = OUTPUT_GUARD_SAMPLES;
    let output_end = output_start + output_len;
    let mut guarded_output = vec![OUTPUT_CANARY; output_len + OUTPUT_GUARD_SAMPLES * 2];
    guarded_output[output_start..output_end].fill(f32::NAN);
    let mut frame_offset = 0;
    for block_frames in [127, 509, 383, 5] {
        let input_start = frame_offset * 2;
        let input_end = input_start + block_frames * 2;
        let block_output_start = output_start + frame_offset * output_channels;
        let block_output_end = block_output_start + block_frames * output_channels;
        assert_eq!(
            host.process(
                &input[input_start..input_end],
                &mut guarded_output[block_output_start..block_output_end],
            )
            .unwrap_or_else(|error| panic!("process public DawHost chain: {error}")),
            block_frames
        );
        frame_offset += block_frames;
    }
    assert_eq!(frame_offset, FRAMES);
    assert!(
        guarded_output[..output_start]
            .iter()
            .chain(&guarded_output[output_end..])
            .all(|sample| sample.to_bits() == OUTPUT_CANARY.to_bits())
    );
    let output = guarded_output[output_start..output_end].to_vec();
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn assert_distinct_band_major_audio(plugin: &mut ExternalPlugin, num_bands: usize) {
    let input = distinct_stereo_input();
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
    let output = render_block(plugin, &input, &context, num_bands * 2);
    assert_distinct_pairs(&output, num_bands);
}

fn assert_distinct_pairs(output: &[f32], num_bands: usize) {
    let output_channels = num_bands * 2;
    assert_eq!(output.len() % output_channels, 0);
    for band in 0..num_bands {
        let mut left_peak = 0.0_f32;
        let mut right_peak = 0.0_f32;
        let mut stereo_difference = 0.0_f32;
        for frame in output.chunks_exact(output_channels) {
            let left = frame[band * 2];
            let right = frame[band * 2 + 1];
            left_peak = left_peak.max(left.abs());
            right_peak = right_peak.max(right.abs());
            stereo_difference = stereo_difference.max((left - right).abs());
        }
        assert!(left_peak > 1.0e-6, "band {band} left output is silent");
        assert!(right_peak > 1.0e-6, "band {band} right output is silent");
        assert!(
            stereo_difference > 1.0e-6,
            "band {band} must preserve its distinct left and right inputs"
        );
    }
}

fn assert_waveforms_match(actual: &[f32], expected: &[f32], route: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{route} output lengths differ"
    );
    assert!(actual.iter().all(|sample| sample.is_finite()));
    assert!(expected.iter().all(|sample| sample.is_finite()));
    let max_error = max_waveform_error(actual, expected);
    assert!(max_error <= 2.0e-5, "{route} max sample error {max_error}");
}

fn assert_waveforms_equal(actual: &[f32], expected: &[f32], route: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{route} output lengths differ"
    );
    assert!(
        actual.iter().all(|sample| sample.is_finite()),
        "{route} actual output must be finite"
    );
    assert!(
        expected.iter().all(|sample| sample.is_finite()),
        "{route} reference output must be finite"
    );
    assert_eq!(
        actual, expected,
        "{route} same-binary synchronized continuation must match exactly"
    );
}

fn max_waveform_error(actual: &[f32], expected: &[f32]) -> f32 {
    assert_eq!(actual.len(), expected.len());
    assert!(actual.iter().all(|sample| sample.is_finite()));
    assert!(expected.iter().all(|sample| sample.is_finite()));
    actual
        .iter()
        .zip(expected)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max)
}

fn native_state_int(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<i64> {
    let state = parse_native_state(opaque_state, format)?;
    state.get("params")?.get(id)?.get("i32")?.as_i64()
}

fn edit_native_state(
    opaque_state: &mut Vec<u8>,
    format: PluginFormat,
    update: impl FnOnce(&mut serde_json::Value),
) {
    let prefix = if format == PluginFormat::Clap { 8 } else { 0 };
    let mut state: serde_json::Value = serde_json::from_slice(
        opaque_state
            .get(prefix..)
            .expect("CLAP state has an eight-byte prefix"),
    )
    .expect("native plugin state is NIH-plug JSON");
    if format == PluginFormat::Clap {
        let encoded_length: [u8; 8] = opaque_state[..8]
            .try_into()
            .expect("CLAP state prefix contains its payload length");
        assert_eq!(
            usize::try_from(u64::from_le_bytes(encoded_length)).expect("state length fits usize"),
            opaque_state.len() - 8,
            "CLAP state length prefix must cover the full payload"
        );
    }
    update(&mut state);
    let payload = serde_json::to_vec(&state).expect("serialize updated native state");
    opaque_state.clear();
    if format == PluginFormat::Clap {
        opaque_state.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    }
    opaque_state.extend_from_slice(&payload);
}

fn parse_native_state(opaque_state: &[u8], format: PluginFormat) -> Option<serde_json::Value> {
    let payload = match format {
        PluginFormat::Clap => {
            let length: [u8; 8] = opaque_state.get(..8)?.try_into().ok()?;
            let length = usize::try_from(u64::from_le_bytes(length)).ok()?;
            let payload = opaque_state.get(8..)?;
            (payload.len() == length).then_some(payload)?
        }
        PluginFormat::Vst3 => opaque_state,
        PluginFormat::AudioUnit => return None,
    };
    serde_json::from_slice(payload).ok()
}

fn set_state_int(state: &mut serde_json::Value, id: &str, value: i32) {
    state
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|parameters| parameters.get_mut(id))
        .map(|parameter| *parameter = serde_json::json!({ "i32": value }))
        .unwrap_or_else(|| panic!("native state has no {id} parameter"));
}
