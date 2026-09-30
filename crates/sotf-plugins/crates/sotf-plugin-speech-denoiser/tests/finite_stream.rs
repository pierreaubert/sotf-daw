//! Disabled output has bounded support even when the model remains warm.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_speech_denoiser::{
    SpeechDenoiserData, SpeechDenoiserPlugin, SpeechDenoiserPluginParams,
};
use std::fs;
use std::path::Path;

const RATE: u32 = 48000;
const DELAY: usize = 960;
const QUANTUM: usize = 480;

fn configured(channels: usize, enabled: bool) -> SpeechDenoiserPlugin {
    let mut plugin =
        SpeechDenoiserPlugin::from_params(channels, SpeechDenoiserPluginParams { enabled });
    plugin.initialize(RATE).unwrap();
    plugin
}

fn process(plugin: &mut SpeechDenoiserPlugin, input: &[f32], blocks: &[usize]) -> Vec<f32> {
    let channels = plugin.channels();
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

fn drain(plugin: &mut SpeechDenoiserPlugin, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.channels();
    let mut output = Vec::new();
    let mut calls = 0;
    loop {
        assert!(calls <= DELAY);
        let capacity = capacities[calls % capacities.len()];
        let mut block = vec![1234.; capacity * channels];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 0))
            .unwrap();
        assert!(result.frames <= capacity.min(QUANTUM));
        assert!(
            block[result.frames * channels..]
                .iter()
                .all(|&x| x == 1234.)
        );
        output.extend_from_slice(&block[..result.frames * channels]);
        calls += 1;
        if result.complete {
            break;
        }
        assert!(result.frames > 0);
    }
    output
}

#[test]
fn disabled_final_marker_is_returned_at_its_independent_delayed_position() {
    for channels in [1, 2] {
        let mut plugin = configured(channels, false);
        let mut input = vec![0.; 73 * channels];
        for ch in 0..channels {
            input[72 * channels + ch] = 0.25 / (ch + 1) as f32;
        }
        let mut output = process(&mut plugin, &input, &[17, 1]);
        let tail = drain(&mut plugin, &[1, 17, 479, 480, 777]);
        assert_eq!(tail.len(), DELAY * channels);
        output.extend(tail);
        for (index, &actual) in output.iter().enumerate() {
            let expected = index.checked_sub(DELAY * channels).map_or(0., |i| input[i]);
            assert_eq!(actual, expected, "channels={channels}, index={index}");
        }
        assert_eq!(plugin.tail_length(), TailLength::Finite(DELAY as u64));
    }
}

fn signal(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|i| (((i * 37 + 11) % 257) as f32 - 128.) / 512.)
        .collect()
}

fn enabled(plugin: &mut SpeechDenoiserPlugin, value: bool) {
    plugin
        .parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(value))
        .unwrap();
}

#[test]
fn all_model_phases_and_dense_callbacks_preserve_complete_disabled_program() {
    let mut cases = 0;
    for channels in [1, 2] {
        let mut plugin = configured(channels, false);
        for phase in 0..QUANTUM {
            plugin.reset();
            let input = signal(phase + 1, channels);
            let mut output = process(
                &mut plugin,
                &input,
                if phase % 2 == 0 { &[1] } else { &[137, 8193] },
            );
            output.extend(drain(&mut plugin, &[1, 17, 479, 480, 777]));
            assert_eq!(output.len(), input.len() + DELAY * channels);
            assert!(output[..DELAY * channels].iter().all(|&x| x == 0.));
            assert_eq!(&output[DELAY * channels..], &input);
            cases += 1;
        }
        for blocks in [
            &[1][..],
            &[7],
            &[137],
            &[479],
            &[480],
            &[481],
            &[8193],
            &[1, 137, 4096],
        ] {
            plugin.reset();
            let input = signal(20 * QUANTUM + 73, channels);
            let mut output = process(&mut plugin, &input, blocks);
            output.extend(drain(&mut plugin, &[17, 480, 1]));
            assert!(output[..DELAY * channels].iter().all(|&x| x == 0.));
            assert_eq!(&output[DELAY * channels..], &input);
            cases += 1;
        }
    }
    assert_eq!(cases, 976);
}

fn prepare_transition(plugin: &mut SpeechDenoiserPlugin, history: usize, phase: usize) {
    let channels = plugin.channels();
    match history {
        0 => {
            process(plugin, &signal(phase, channels), &[137, 1]);
            enabled(plugin, false);
        }
        1 => {
            enabled(plugin, false);
            process(plugin, &signal(3 * QUANTUM + 17, channels), &[481, 7]);
            enabled(plugin, true);
            process(plugin, &signal(137, channels), &[17]);
            enabled(plugin, false);
            process(plugin, &signal(phase, channels), &[1, 137]);
        }
        2 => {
            process(plugin, &signal(7 * QUANTUM + 13, channels), &[8193]);
            enabled(plugin, false);
            process(plugin, &signal(phase, channels), &[479]);
        }
        _ => unreachable!(),
    }
}

#[test]
fn disabled_eof_during_live_fades_matches_ordinary_zero_continuation_exactly() {
    let mut cases = 0;
    for channels in [1, 2] {
        for phase in [1, 479, 480, 481, 1027] {
            for history in 0..3 {
                for capacities in [&[1][..], &[17], &[480], &[1, 17, 479, 777]] {
                    let mut plugin = configured(channels, true);
                    let mut reference = configured(channels, true);
                    prepare_transition(&mut plugin, history, phase);
                    prepare_transition(&mut reference, history, phase);
                    let actual = drain(&mut plugin, capacities);
                    let expected =
                        process(&mut reference, &vec![0.; DELAY * channels], &[137, 1, 479]);
                    assert_eq!(
                        actual, expected,
                        "channels={channels}, phase={phase}, history={history}"
                    );
                    let further = process(&mut reference, &vec![0.; 3072 * channels], &[480, 17]);
                    assert!(further.iter().all(|&x| x == 0.));
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 120);
}

#[test]
fn call_bounds_count_remaining_frames_after_partial_drains() {
    for channels in [1, 2] {
        for source in [1, 479, 480, 481, 1027, 8193] {
            for prior in [0, 1, 479, 480, 959] {
                let mut plugin = configured(channels, false);
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                process(&mut plugin, &signal(source, channels), &[137]);
                let mut advanced = 0;
                let mut output = vec![0.; QUANTUM * channels];
                while advanced < prior {
                    let frames = QUANTUM.min(prior - advanced);
                    let result = plugin
                        .drain(
                            &mut output[..frames * channels],
                            &ProcessContext::new(RATE, 0),
                        )
                        .unwrap();
                    assert_eq!(result.frames, frames);
                    advanced += frames;
                }
                let bound = plugin.drain_call_bound().unwrap().get();
                assert_eq!(bound as usize, (DELAY - prior).div_ceil(QUANTUM));
                let mut calls = 0;
                loop {
                    calls += 1;
                    if plugin
                        .drain(&mut output, &ProcessContext::new(RATE, 0))
                        .unwrap()
                        .complete
                    {
                        break;
                    }
                    assert!(calls < bound);
                }
                assert_eq!(calls, bound);
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                assert!(
                    plugin
                        .drain(&mut [], &ProcessContext::new(RATE, 0))
                        .unwrap()
                        .complete
                );
                assert_eq!(plugin.drain_output_frames_max(), QUANTUM);
            }
        }
    }
}

#[test]
fn rejected_calls_preserve_waveform_and_finite_eof_freezes_controls_until_reset() {
    let mut uninitialized = SpeechDenoiserPlugin::new(2);
    assert_eq!(uninitialized.tail_length(), TailLength::Unknown);
    assert_eq!(uninitialized.drain_call_bound(), None);
    assert!(
        uninitialized
            .drain(&mut [1234.; 2], &ProcessContext::new(RATE, 0))
            .is_err()
    );
    for channels in [1, 2] {
        let mut plugin = configured(channels, false);
        let mut reference = configured(channels, false);
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap()
                .complete
        );
        enabled(&mut plugin, true);
        enabled(&mut plugin, false);
        let input = signal(479, channels);
        assert_eq!(
            process(&mut plugin, &input, &[17]),
            process(&mut reference, &input, &[17])
        );
        let mut canary = vec![1234.; 480 * channels];
        assert!(
            plugin
                .drain(&mut canary, &ProcessContext::new(44100, 0))
                .is_err()
        );
        assert!(canary.iter().all(|&x| x == 1234.));
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .is_err()
        );
        if channels == 2 {
            assert!(
                plugin
                    .drain(&mut canary[..3], &ProcessContext::new(RATE, 0))
                    .is_err()
            );
            assert!(canary.iter().all(|&x| x == 1234.));
        }
        // Rejected drain did not freeze ordinary processing or lose model/ring state.
        let more = signal(17, channels);
        assert_eq!(
            process(&mut plugin, &more, &[1]),
            process(&mut reference, &more, &[1])
        );
        let first = plugin
            .drain(&mut canary[..channels], &ProcessContext::new(RATE, 0))
            .unwrap();
        assert_eq!(first.frames, 1);
        let mut actual = canary[..channels].to_vec();
        let before = canary.clone();
        assert!(
            plugin
                .process_in_place(&mut canary, &ProcessContext::new(RATE, 1))
                .is_err()
        );
        assert_eq!(canary, before);
        assert!(
            plugin
                .parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                .is_err()
        );
        enabled(&mut plugin, false);
        let mut same = plugin.current_values();
        plugin.apply_values_realtime(&same).unwrap();
        same.insert(ParameterId::from("enabled"), ParameterValue::Bool(true));
        assert!(plugin.apply_values_realtime(&same).is_err());
        assert!(plugin.apply_values(same).is_err());
        assert!(plugin.initialize(44100).is_err());
        assert!(
            plugin
                .process_in_place(&mut [], &ProcessContext::new(RATE, 0))
                .is_ok()
        );
        actual.extend(drain(&mut plugin, &[17, 480]));
        assert_eq!(
            actual,
            process(&mut reference, &vec![0.; DELAY * channels], &[137])
        );
        let mut terminal = vec![1234.; channels];
        assert_eq!(
            plugin
                .drain(&mut terminal, &ProcessContext::new(RATE, 0))
                .unwrap()
                .frames,
            0
        );
        assert!(terminal.iter().all(|&x| x == 1234.));
        assert!(
            plugin
                .parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                .is_err()
        );
        plugin.reset();
        let mut fresh = configured(channels, false);
        assert_eq!(
            process(&mut plugin, &input, &[137]),
            process(&mut fresh, &input, &[137])
        );
        assert_eq!(drain(&mut plugin, &[17]), drain(&mut fresh, &[480]));
        plugin.initialize(RATE).unwrap();
        enabled(&mut plugin, true);
    }
}

#[test]
fn enabled_partial_eof_preserves_accepted_program_and_terminal_state() {
    for channels in [1, 2] {
        let input = signal(3 * QUANTUM + 73, channels);
        let more = signal(7, channels);
        let mut plugin = configured(channels, true);
        let mut reference = configured(channels, true);

        // An empty EOF is complete without freezing the next accepted stream.
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap()
                .complete
        );
        let mut actual = process(&mut plugin, &input, &[1, 137, 479]);
        let mut expected = process(&mut reference, &input, &[479, 1, 1024]);

        // A rejected unfinished zero-capacity drain is transactional.
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .is_err()
        );
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 2);
        actual.extend(process(&mut plugin, &more, &[1]));
        expected.extend(process(&mut reference, &more, &[17]));

        actual.extend(drain(&mut plugin, &[1, 137, QUANTUM]));
        expected.extend(process(
            &mut reference,
            &vec![0.0; DELAY * channels],
            &[QUANTUM],
        ));
        let omitted_suffix_peak = expected[input.len()..]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            omitted_suffix_peak > 1.0e-5,
            "channels={channels}: omitted suffix is silent (peak={omitted_suffix_peak})"
        );
        assert_eq!(actual.len(), expected.len());
        assert_eq!(actual, expected, "channels={channels}");
        assert_eq!(plugin.tail_length(), TailLength::Unknown);
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);

        let final_snapshot = *plugin
            .get_data()
            .unwrap()
            .downcast::<SpeechDenoiserData>()
            .unwrap();
        assert!(final_snapshot.model_frames > 0);

        // Zero-frame process and repeated complete drain are state-neutral.
        let mut canary = vec![0.25; channels];
        assert_eq!(
            plugin
                .process_in_place(&mut canary, &ProcessContext::new(RATE, 0))
                .unwrap(),
            0
        );
        assert_eq!(canary, vec![0.25; channels]);
        assert_eq!(
            *plugin
                .get_data()
                .unwrap()
                .downcast::<SpeechDenoiserData>()
                .unwrap(),
            final_snapshot
        );
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap()
                .complete
        );
        assert_eq!(plugin.tail_length(), TailLength::Unknown);

        let before = canary.clone();
        assert!(
            plugin
                .process_in_place(&mut canary, &ProcessContext::new(RATE, 1))
                .is_err()
        );
        assert_eq!(canary, before);
        assert!(plugin
            .parametric_set_parameter(
                ParameterId::from("enabled"),
                ParameterValue::Bool(false),
            )
            .is_err());

        // Explicit reset restores a fresh process timeline and clears telemetry.
        plugin.reset();
        let reset_snapshot = *plugin
            .get_data()
            .unwrap()
            .downcast::<SpeechDenoiserData>()
            .unwrap();
        assert_eq!(reset_snapshot, SpeechDenoiserData::default());
        let mut fresh = configured(channels, true);
        assert_eq!(
            process(&mut plugin, &input, &[1, 137, 479]),
            process(&mut fresh, &input, &[479, 1, 1024]),
        );
    }
}

#[test]
fn enabled_eof_matches_zero_continuation_for_every_terminal_model_residue() {
    for channels in [1, 2] {
        let mut plugin = configured(channels, true);
        let mut reference = configured(channels, true);
        for residue in 1..=QUANTUM {
            plugin.reset();
            reference.reset();
            let input = signal(2 * QUANTUM + residue, channels);
            let mut actual = process(&mut plugin, &input, &[137, 1, 479]);
            actual.extend(drain(&mut plugin, &[1, 17, 479, QUANTUM]));

            let mut expected = process(&mut reference, &input, &[481, 17, 8193]);
            expected.extend(process(
                &mut reference,
                &vec![0.0; DELAY * channels],
                &[17, QUANTUM],
            ));
            assert_eq!(actual.len(), input.len() + DELAY * channels);
            assert_eq!(actual, expected, "channels={channels}, residue={residue}");
        }
    }
}

#[test]
fn enabled_eof_is_partition_invariant_for_mono_and_stereo() {
    let partitions: &[&[usize]] = &[
        &[1],
        &[17],
        &[137, 1, 479],
        &[QUANTUM],
        &[QUANTUM + 1],
        &[8193],
        &[1, 137, 4096],
    ];
    let drain_partitions: &[&[usize]] = &[&[1], &[17, 479], &[QUANTUM], &[1, 137, 479, 777]];

    for channels in [1, 2] {
        let input = signal(3 * QUANTUM + 73, channels);
        for (index, actual_blocks) in partitions.iter().enumerate() {
            let reference_blocks = partitions[partitions.len() - 1 - index];
            let drain_blocks = drain_partitions[index % drain_partitions.len()];
            let mut plugin = configured(channels, true);
            let mut reference = configured(channels, true);

            let mut actual = process(&mut plugin, &input, actual_blocks);
            actual.extend(drain(&mut plugin, drain_blocks));
            let mut expected = process(&mut reference, &input, reference_blocks);
            expected.extend(process(
                &mut reference,
                &vec![0.0; DELAY * channels],
                &[QUANTUM],
            ));
            assert_eq!(actual, expected, "channels={channels}, partition={index}");
        }
    }
}

#[test]
fn enabled_eof_after_enable_transitions_matches_zero_continuation() {
    for channels in [1, 2] {
        let mut plugin = configured(channels, true);
        let mut reference = configured(channels, true);
        let mut actual = Vec::new();
        let mut expected = Vec::new();

        for (frames, block_pattern) in [(487, &[137, 1][..]), (31, &[17][..])] {
            let input = signal(frames, channels);
            actual.extend(process(&mut plugin, &input, block_pattern));
            expected.extend(process(&mut reference, &input, &[479, 1]));
        }
        enabled(&mut plugin, false);
        enabled(&mut reference, false);
        let bypassed = signal(19, channels);
        actual.extend(process(&mut plugin, &bypassed, &[1, 17]));
        expected.extend(process(&mut reference, &bypassed, &[19]));
        enabled(&mut plugin, true);
        enabled(&mut reference, true);

        let final_partial = signal(QUANTUM + 73, channels);
        actual.extend(process(&mut plugin, &final_partial, &[1, 137, 479]));
        expected.extend(process(&mut reference, &final_partial, &[480, 481]));
        actual.extend(drain(&mut plugin, &[1, 479, QUANTUM]));
        expected.extend(process(
            &mut reference,
            &vec![0.0; DELAY * channels],
            &[137, 1, 479],
        ));

        assert_eq!(actual, expected, "channels={channels}");
        assert_eq!(plugin.tail_length(), TailLength::Unknown);
    }
}

#[test]
fn enabled_eof_preflight_errors_do_not_consume_input_or_freeze_the_plugin() {
    for channels in [1, 2] {
        let mut plugin = configured(channels, true);
        let mut reference = configured(channels, true);
        let input = signal(2 * QUANTUM + 17, channels);
        let mut actual = process(&mut plugin, &input, &[137, 1]);
        let mut expected = process(&mut reference, &input, &[QUANTUM]);

        let mut output = vec![1234.0; QUANTUM * channels];
        assert!(
            plugin
                .drain(&mut output, &ProcessContext::new(44100, 0))
                .is_err()
        );
        assert!(output.iter().all(|sample| *sample == 1234.0));
        if channels == 2 {
            assert!(
                plugin
                    .drain(
                        &mut output[..QUANTUM * channels - 1],
                        &ProcessContext::new(RATE, 0)
                    )
                    .is_err()
            );
            assert!(output.iter().all(|sample| *sample == 1234.0));
        }
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .is_err()
        );
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 2);

        let more = signal(23, channels);
        actual.extend(process(&mut plugin, &more, &[1, 17]));
        expected.extend(process(&mut reference, &more, &[23]));
        actual.extend(drain(&mut plugin, &[QUANTUM]));
        expected.extend(process(
            &mut reference,
            &vec![0.0; DELAY * channels],
            &[QUANTUM],
        ));
        assert_eq!(actual, expected, "channels={channels}");
    }
}

#[test]
fn enabled_partial_eof_preserves_accepted_program_like_zero_continuation() {
    let channels = 1;
    let accepted_frames = 3 * QUANTUM + 73;
    let input: Vec<f32> = (0..accepted_frames)
        .map(|frame| {
            let time = frame as f32 / RATE as f32;
            let phase = 2.0 * std::f32::consts::PI * 180.0 * time;
            0.23 * phase.sin() + 0.08 * (2.0 * phase).sin()
        })
        .collect();

    let mut actual_plugin = configured(channels, true);
    let mut reference_plugin = configured(channels, true);
    let mut actual = process(&mut actual_plugin, &input, &[1, 137, 479]);
    actual.extend(drain(&mut actual_plugin, &[1, 137, QUANTUM]));

    let mut expected = process(&mut reference_plugin, &input, &[479, 1, 1024]);
    expected.extend(process(
        &mut reference_plugin,
        &vec![0.0; DELAY * channels],
        &[QUANTUM],
    ));

    let delayed_program_peak = expected[DELAY * channels..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(delayed_program_peak > 1.0e-5, "reference emitted silence");
    let omitted_suffix_peak = expected[input.len()..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(
        omitted_suffix_peak > 1.0e-5,
        "omitted 960-frame suffix is silent (peak={omitted_suffix_peak})"
    );
    assert_eq!(
        actual.len(),
        expected.len(),
        "accepted frames were not drained"
    );
    assert_eq!(
        actual, expected,
        "drain differs from ordinary zero continuation"
    );
    assert_eq!(actual_plugin.tail_length(), TailLength::Unknown);
}

fn write_f32le(path: &Path, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn read_f32le(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).unwrap();
    assert!(bytes.len().is_multiple_of(std::mem::size_of::<f32>()));
    let (chunks, remainder) = bytes.as_chunks::<{ std::mem::size_of::<f32>() }>();
    assert!(remainder.is_empty());
    chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

#[test]
#[ignore = "manual AUD136 pre-edit audio capture"]
fn capture_aud136_pre_edit_audio_baselines() {
    let output_dir = std::env::var_os("SOTF_AUDIT_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .expect("set SOTF_AUDIT_ARTIFACT_DIR to a target-local audit directory");
    fs::create_dir_all(&output_dir).unwrap();

    let accepted_frames = 3 * QUANTUM + 73;
    let input: Vec<f32> = (0..accepted_frames)
        .map(|frame| {
            let time = frame as f32 / RATE as f32;
            let phase = 2.0 * std::f32::consts::PI * 180.0 * time;
            0.23 * phase.sin() + 0.08 * (2.0 * phase).sin()
        })
        .collect();
    write_f32le(&output_dir.join("input_mono.f32le"), &input);

    let mut enabled_plugin = configured(1, true);
    let mut enabled_output = process(&mut enabled_plugin, &input, &[479, 1, 1024]);
    enabled_output.extend(process(&mut enabled_plugin, &vec![0.0; DELAY], &[QUANTUM]));
    assert_eq!(enabled_output.len(), input.len() + DELAY);
    assert!(
        enabled_output[input.len()..]
            .iter()
            .any(|sample| sample.abs() > 1.0e-5)
    );
    write_f32le(
        &output_dir.join("enabled_ordinary_process_zero_continuation.f32le"),
        &enabled_output,
    );

    let mut disabled_plugin = configured(1, false);
    let mut disabled_output = process(&mut disabled_plugin, &input, &[479, 1, 1024]);
    let disabled_tail = drain(&mut disabled_plugin, &[480]);
    assert_eq!(disabled_tail.len(), DELAY);
    disabled_output.extend(disabled_tail);
    assert_eq!(disabled_output.len(), input.len() + DELAY);
    write_f32le(
        &output_dir.join("disabled_process_and_drain.f32le"),
        &disabled_output,
    );
    fs::write(
        output_dir.join("metadata.txt"),
        format!(
            "rate_hz={RATE}\nchannels=1\naccepted_frames={accepted_frames}\ncontinuation_frames={DELAY}\nformat=f32le_interleaved\n"
        ),
    )
    .unwrap();
}

#[test]
#[ignore = "manual AUD136 pre-edit enabled/disabled sample replay"]
fn replay_aud136_pre_edit_audio_baselines_bit_exact() {
    let baseline_dir = std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
        .map(std::path::PathBuf::from)
        .expect("set SOTF_AUDIT_BASELINE_DIR to the preserved pre-edit audio directory");
    let input = read_f32le(&baseline_dir.join("input_mono.f32le"));
    let expected_enabled =
        fs::read(baseline_dir.join("enabled_ordinary_process_zero_continuation.f32le")).unwrap();
    let expected_disabled =
        fs::read(baseline_dir.join("disabled_process_and_drain.f32le")).unwrap();

    let mut enabled_plugin = configured(1, true);
    let mut enabled_output = process(&mut enabled_plugin, &input, &[479, 1, 1024]);
    enabled_output.extend(process(&mut enabled_plugin, &vec![0.0; DELAY], &[QUANTUM]));
    let mut enabled_bytes = Vec::with_capacity(enabled_output.len() * 4);
    for sample in enabled_output {
        enabled_bytes.extend_from_slice(&sample.to_le_bytes());
    }
    assert_eq!(enabled_bytes, expected_enabled);

    let mut disabled_plugin = configured(1, false);
    let mut disabled_output = process(&mut disabled_plugin, &input, &[479, 1, 1024]);
    disabled_output.extend(drain(&mut disabled_plugin, &[QUANTUM]));
    let mut disabled_bytes = Vec::with_capacity(disabled_output.len() * 4);
    for sample in disabled_output {
        disabled_bytes.extend_from_slice(&sample.to_le_bytes());
    }
    assert_eq!(disabled_bytes, expected_disabled);
}
