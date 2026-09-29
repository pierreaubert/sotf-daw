//! Disabled output has bounded support even when the model remains warm.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_speech_denoiser::{SpeechDenoiserPlugin, SpeechDenoiserPluginParams};

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
fn enabled_wet_eof_retains_its_explicitly_unknown_unfrozen_policy() {
    let mut plugin = configured(1, true);
    process(&mut plugin, &[0.25; 479], &[137]);
    let mut output = [1234.; QUANTUM];
    let result = plugin
        .drain(&mut output, &ProcessContext::new(RATE, 0))
        .unwrap();
    assert_eq!(result.frames, 0);
    assert!(result.complete);
    assert!(output.iter().all(|&x| x == 1234.));
    assert_eq!(plugin.tail_length(), TailLength::Unknown);
    assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
    process(&mut plugin, &[0.1], &[1]);
    enabled(&mut plugin, false);
    assert_eq!(drain(&mut plugin, &[480]).len(), DELAY);
}
