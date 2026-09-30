//! Public finite-stream regression for child-tail composition in A/B Compare.

use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_ab_compare::{ABComparePlugin, ABComparePluginParams, PathConfig};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const ACCEPTED_FRAMES: usize = 8;
const CHILD_DELAY_FRAMES: usize = 48;
const CHILD_RING_FRAMES: usize = 64;
const CONTROL_FRAMES: usize = 128;
const MIX_CONTROL_FRAMES: usize = 512;
const AUTOGAIN_CONTROL_FRAMES: usize = 9_600;

fn finite_delay_params() -> ABComparePluginParams {
    ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "delay".to_owned(),
            parameters: serde_json::json!({
                "channel_delays_ms": [1.0, 1.0],
                "feedback": 0.0,
                "mix": 1.0,
                "lfo_rate_hz": 0.0,
                "lfo_depth_ms": 0.0,
                "pitch_preserving": false,
                "allpass_feedback": false,
            }),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    }
}

fn gain_path(gain_db: f32) -> PathConfig {
    PathConfig::Plugin {
        plugin_type: "gain".to_owned(),
        parameters: serde_json::json!({ "gain_db": gain_db }),
    }
}

fn control_params(name: &str) -> ABComparePluginParams {
    let mut params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "delay".to_owned(),
            parameters: serde_json::json!({
                "channel_delays_ms": [1.0, 1.0],
                "feedback": 0.0,
                "mix": 1.0,
                "lfo_rate_hz": 0.0,
                "lfo_depth_ms": 0.0,
                "pitch_preserving": false,
                "allpass_feedback": false,
            }),
        },
        path_b: gain_path(0.0),
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };

    match name {
        "pure-a" => params.mix = -1.0,
        "pure-b" => params.mix = 1.0,
        "half-mix" => params.mix = 0.0,
        "difference" => params.difference_mode = true,
        "phase-b" => {
            params.mix = 1.0;
            params.phase_invert_b = true;
        }
        "bypass" => {
            params.mix = -1.0;
            params.bypass = true;
        }
        "auto-gain" => {
            params.path_a = gain_path(-6.0);
            params.path_b = gain_path(6.0);
            params.mix = 1.0;
            params.auto_gain_enabled = true;
        }
        other => panic!("unknown AUD137 control case {other}"),
    }
    params
}

fn control_frames(name: &str) -> usize {
    if name == "auto-gain" {
        AUTOGAIN_CONTROL_FRAMES
    } else {
        MIX_CONTROL_FRAMES
    }
}

fn control_chunks(name: &str) -> Vec<usize> {
    if name == "auto-gain" {
        vec![480; AUTOGAIN_CONTROL_FRAMES / 480]
    } else {
        vec![1, 31, 128, 7, 345]
    }
}

fn accepted_programme() -> Vec<f32> {
    let mut input = vec![0.0; ACCEPTED_FRAMES * CHANNELS];
    input[0] = 1.0;
    input[(ACCEPTED_FRAMES - 1) * CHANNELS + 1] = 0.5;
    input
}

fn dense_programme(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * CHANNELS];
    for frame in 0..frames {
        let left = (frame * 7 % 31) as f32 - 15.0;
        let right = (frame * 11 % 37) as f32 - 18.0;
        input[frame * CHANNELS] = left / 20.0;
        input[frame * CHANNELS + 1] = right / 25.0;
    }
    input
}

fn delayed_programme_reference(input: &[f32]) -> Vec<f32> {
    assert!(input.len().is_multiple_of(CHANNELS));
    let frames = input.len() / CHANNELS;
    let mut expected = vec![0.0; input.len()];
    for frame in CHILD_DELAY_FRAMES..frames {
        let source_frame = frame - CHILD_DELAY_FRAMES;
        expected[frame * CHANNELS..(frame + 1) * CHANNELS]
            .copy_from_slice(&input[source_frame * CHANNELS..(source_frame + 1) * CHANNELS]);
    }
    expected
}

fn render_programme_chunks(
    plugin: &mut ABComparePlugin,
    input: &[f32],
    chunks: &[usize],
) -> Vec<f32> {
    assert_eq!(chunks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = Vec::with_capacity(input.len());
    let mut input_frame = 0;
    for &frames in chunks {
        let start = input_frame * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        assert_eq!(
            plugin
                .process(&input[start..end], &mut block, &context)
                .unwrap(),
            frames
        );
        output.extend_from_slice(&block);
        input_frame += frames;
    }
    output
}

fn render_control_case(name: &str) -> (ABComparePluginParams, Vec<f32>, Vec<f32>) {
    let frames = control_frames(name);
    let input = dense_programme(frames);
    let mut plugin = ABComparePlugin::from_params(CHANNELS, control_params(name)).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    let output = render_programme_chunks(&mut plugin, &input, &control_chunks(name));
    (control_params(name), input, output)
}

fn decode_f32_le(bytes: &[u8]) -> Vec<f32> {
    assert!(bytes.len().is_multiple_of(std::mem::size_of::<f32>()));
    let (chunks, remainder) = bytes.as_chunks::<{ std::mem::size_of::<f32>() }>();
    assert!(remainder.is_empty());
    chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn analytical_expected_stream(input: &[f32]) -> Vec<f32> {
    assert_eq!(input.len(), ACCEPTED_FRAMES * CHANNELS);
    let mut expected = vec![0.0; (ACCEPTED_FRAMES + CHILD_RING_FRAMES) * CHANNELS];
    for frame in 0..ACCEPTED_FRAMES {
        let output_frame = frame + CHILD_DELAY_FRAMES;
        expected[output_frame * CHANNELS..(output_frame + 1) * CHANNELS]
            .copy_from_slice(&input[frame * CHANNELS..(frame + 1) * CHANNELS]);
    }
    expected
}

fn create_public_plugin() -> ABComparePlugin {
    let mut plugin = ABComparePlugin::from_params(CHANNELS, finite_delay_params()).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    plugin
}

fn process_and_drain(plugin: &mut ABComparePlugin, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let context = ProcessContext::new(SAMPLE_RATE, ACCEPTED_FRAMES);
    let mut programme_output = vec![f32::NAN; input.len()];
    let processed = plugin
        .process(input, &mut programme_output, &context)
        .unwrap();
    assert_eq!(processed, ACCEPTED_FRAMES);

    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&drain_context).unwrap();
    let output_capacity_frames = plugin.drain_output_frames_max().max(CHILD_RING_FRAMES);
    let mut block = vec![f32::NAN; output_capacity_frames * CHANNELS];
    let mut tail = Vec::new();

    for _ in 0..128 {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, &drain_context).unwrap();
        assert!(result.frames <= output_capacity_frames);
        tail.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return (programme_output, tail);
        }
    }

    panic!("A/B Compare did not complete within the test drain-call allowance");
}

fn write_f32_le(path: &std::path::Path, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn public_process_keeps_finite_delay_output_under_irregular_callbacks() {
    let input = dense_programme(CONTROL_FRAMES);
    let expected = delayed_programme_reference(&input);
    let mut plugin = create_public_plugin();
    let actual = render_programme_chunks(&mut plugin, &input, &[1, 7, 13, 31, 76]);

    assert!(actual.iter().any(|sample| sample.abs() > 0.1));
    assert_eq!(actual.len(), expected.len());
    for (frame, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "sample {frame}: expected {expected}, got {actual}"
        );
    }
}

#[test]
fn ab_compare_drain_preserves_a_finite_child_delay_tail() {
    let input = accepted_programme();
    let expected = analytical_expected_stream(&input);
    assert!(
        expected[ACCEPTED_FRAMES * CHANNELS..]
            .iter()
            .any(|sample| sample.abs() > 0.25)
    );

    let mut plugin = create_public_plugin();
    assert_eq!(plugin.latency_samples(), 0);
    let (programme_output, drain_output) = process_and_drain(&mut plugin, &input);
    let mut actual = programme_output;
    actual.extend_from_slice(&drain_output);

    assert_eq!(
        actual.len(),
        expected.len(),
        "the child delay's nonzero accepted programme suffix must be returned during drain"
    );
    for (frame, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "sample {frame}: expected {expected}, got {actual}"
        );
    }
}

#[test]
#[ignore = "manual AUD137 pre-edit source and sample capture"]
fn capture_aud137_pre_edit_finite_delay_baseline() {
    let directory = std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
        .expect("set SOTF_AUDIT_BASELINE_DIR to a writable audit artifact directory");
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();

    let input = accepted_programme();
    let expected = analytical_expected_stream(&input);
    let mut plugin = create_public_plugin();
    let (programme_output, drain_output) = process_and_drain(&mut plugin, &input);
    let mut actual = programme_output.clone();
    actual.extend_from_slice(&drain_output);

    write_f32_le(&directory.join("aud137-input-f32le.bin"), &input);
    write_f32_le(
        &directory.join("aud137-pre-edit-process-f32le.bin"),
        &programme_output,
    );
    write_f32_le(
        &directory.join("aud137-pre-edit-drain-f32le.bin"),
        &drain_output,
    );
    write_f32_le(
        &directory.join("aud137-analytical-expected-f32le.bin"),
        &expected,
    );
    write_f32_le(
        &directory.join("aud137-pre-edit-emitted-f32le.bin"),
        &actual,
    );
    std::fs::write(
        directory.join("aud137-pre-edit-capture.txt"),
        format!(
            "public ABComparePlugin, 48000 Hz, stereo, accepted_frames={ACCEPTED_FRAMES}, child_delay_frames={CHILD_DELAY_FRAMES}, expected_child_ring_drain_frames={CHILD_RING_FRAMES}, latency_samples={}, programme_frames={}, drain_frames={}, drain_complete=true, expected_total_frames={}\n",
            plugin.latency_samples(),
            programme_output.len() / CHANNELS,
            drain_output.len() / CHANNELS,
            expected.len() / CHANNELS,
        ),
    )
    .unwrap();

    assert_eq!(actual.len(), ACCEPTED_FRAMES * CHANNELS);
    assert!(
        expected[ACCEPTED_FRAMES * CHANNELS..]
            .iter()
            .any(|sample| sample.abs() > 0.25)
    );
}

#[test]
#[ignore = "manual AUD137 pre-edit ordinary-output control capture"]
fn capture_aud137_pre_edit_dense_process_control() {
    let directory = std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
        .expect("set SOTF_AUDIT_BASELINE_DIR to a writable audit artifact directory");
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();

    let input = dense_programme(CONTROL_FRAMES);
    let expected = delayed_programme_reference(&input);
    let mut plugin = create_public_plugin();
    let actual = render_programme_chunks(&mut plugin, &input, &[1, 7, 13, 31, 76]);

    assert!(actual.iter().any(|sample| sample.abs() > 0.1));
    write_f32_le(
        &directory.join("aud137-dense-control-input-f32le.bin"),
        &input,
    );
    write_f32_le(
        &directory.join("aud137-pre-edit-dense-process-f32le.bin"),
        &actual,
    );
    write_f32_le(
        &directory.join("aud137-dense-control-analytic-f32le.bin"),
        &expected,
    );
    std::fs::write(
        directory.join("aud137-dense-control-capture.txt"),
        format!(
            "public ABComparePlugin ordinary process, 48000 Hz, stereo, accepted_frames={CONTROL_FRAMES}, chunks=[1,7,13,31,76], child_delay_frames={CHILD_DELAY_FRAMES}, latency_samples={}, output_peak={}, expected_full_vector=true\n",
            plugin.latency_samples(),
            actual.iter().map(|sample| sample.abs()).fold(0.0_f32, f32::max),
        ),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
#[ignore = "manual AUD137 pre-edit mixer-control capture"]
fn capture_aud137_pre_edit_mixer_control_vectors() {
    let directory = std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
        .expect("set SOTF_AUDIT_BASELINE_DIR to a writable audit artifact directory");
    let directory = std::path::PathBuf::from(directory).join("mixer-controls");
    std::fs::create_dir_all(&directory).unwrap();

    for name in [
        "pure-a",
        "pure-b",
        "half-mix",
        "difference",
        "phase-b",
        "bypass",
        "auto-gain",
    ] {
        let (params, input, output) = render_control_case(name);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.1));
        write_f32_le(&directory.join(format!("{name}-input-f32le.bin")), &input);
        write_f32_le(&directory.join(format!("{name}-output-f32le.bin")), &output);
        std::fs::write(
            directory.join(format!("{name}-params.json")),
            serde_json::to_vec_pretty(&params).unwrap(),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "manual AUD137 replay against saved pre-edit mixer-control arrays"]
fn replay_aud137_pre_edit_mixer_control_vectors() {
    let directory = std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
        .expect("set SOTF_AUDIT_BASELINE_DIR to the preserved pre-edit artifact directory");
    let directory = std::path::PathBuf::from(directory).join("mixer-controls");

    for name in [
        "pure-a",
        "pure-b",
        "half-mix",
        "difference",
        "phase-b",
        "bypass",
        "auto-gain",
    ] {
        let (params, input, output) = render_control_case(name);
        let saved_params = std::fs::read(directory.join(format!("{name}-params.json"))).unwrap();
        assert_eq!(saved_params, serde_json::to_vec_pretty(&params).unwrap());
        assert_eq!(
            input,
            decode_f32_le(
                &std::fs::read(directory.join(format!("{name}-input-f32le.bin"))).unwrap()
            )
        );
        assert_eq!(
            output,
            decode_f32_le(
                &std::fs::read(directory.join(format!("{name}-output-f32le.bin"))).unwrap()
            ),
            "pre-edit ordinary process output changed for {name}"
        );
    }
}
