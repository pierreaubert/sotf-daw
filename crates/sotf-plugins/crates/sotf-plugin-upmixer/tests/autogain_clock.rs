//! Public causal AutoGain regressions with an inactive final output cap.
// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};
fn plugin(rate: u32, enabled: bool) -> UpmixerPlugin {
    let mut params = UpmixerPluginParams::default();
    params.output.auto_gain_enabled = enabled;
    params.output.auto_gain_max_db = 12.0;
    params.output.auto_gain_smoothing_ms = 100.0;
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn source(rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(rate);
            let level = [0.02, 0.1, 0.04][frame / (rate as usize / 3) % 3];
            [
                (level * (std::f64::consts::TAU * 1000.0 * time).sin()) as f32,
                (level * 0.3 * (std::f64::consts::TAU * 3700.0 * time).sin()) as f32,
            ]
        })
        .collect()
}

fn render(plugin: &mut dyn Plugin, input: &[f32], rate: u32, pattern: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut output = vec![f32::NAN; input.len() / 2 * channels];
    let mut position = 0;
    let mut iteration = 0;
    while position < input.len() / 2 {
        let frames = pattern[iteration % pattern.len()].min(input.len() / 2 - position);
        let mut context = ProcessContext::new(rate, frames);
        context.transport.sample_position = position as u64;
        assert_eq!(
            plugin
                .process(
                    &input[2 * position..2 * (position + frames)],
                    &mut output[channels * position..channels * (position + frames)],
                    &context
                )
                .unwrap(),
            frames
        );
        position += frames;
        iteration += 1;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

fn assert_audio_equal(actual: &[f32], expected: &[f32], label: &str) {
    assert_eq!(actual.len(), expected.len());
    let first = actual.iter().zip(expected).position(|(a, b)| a != b);
    let difference = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(
        first.is_none(),
        "{label}: first={first:?}, max_difference={difference}"
    );
}

#[test]
fn fixed_raw_processing_has_partition_independent_autogain() {
    let rate = 48_000;
    let input = source(rate, 6 * rate as usize + 17);
    // The disabled control proves this fixture's raw renderer and final cap
    // do not independently change with callback partitioning.
    for enabled in [false, true] {
        let expected = render(&mut plugin(rate, enabled), &input, rate, &[137]);
        if !enabled {
            // Recorded before and after the clock correction as raw-audio evidence;
            // no platform-specific digest is embedded as a portable assertion.
            let digest = expected.iter().fold(0xcbf2_9ce4_8422_2325_u64, |state, x| {
                (state ^ u64::from(x.to_bits())).wrapping_mul(0x100_0000_01b3)
            });
            eprintln!(
                "upmixer disabled raw digest={digest:016x} samples={}",
                expected.len()
            );
        }
        for pattern in [&[512][..], &[8192][..]] {
            let actual = render(&mut plugin(rate, enabled), &input, rate, pattern);
            assert_audio_equal(
                &actual,
                &expected,
                &format!("enabled={enabled} pattern={pattern:?}"),
            );
        }
    }
}

#[test]
fn future_callback_suffix_cannot_retime_prefix_gain() {
    let rate = 48_000;
    let warm = source(rate, 109 * 4096);
    let future_a = source(rate, 8192);
    let mut future_b = future_a.clone();
    for frame in future_b[8192..].as_chunks_mut::<2>().0 {
        frame[0] *= 0.1;
        frame[1] *= 10.0;
    }
    for enabled in [false, true] {
        let mut a = plugin(rate, enabled);
        let mut b = plugin(rate, enabled);
        assert_eq!(
            render(&mut a, &warm, rate, &[4096]),
            render(&mut b, &warm, rate, &[4096])
        );
        let output_a = render(&mut a, &future_a, rate, &[8192]);
        let output_b = render(&mut b, &future_b, rate, &[8192]);
        let prefix = 4096 * a.output_channels();
        assert_audio_equal(
            &output_a[..prefix],
            &output_b[..prefix],
            &format!("enabled={enabled}"),
        );
    }
}

fn render_with_control_events(
    plugin: &mut dyn Plugin,
    input: &[f32],
    rate: u32,
    pattern: &[usize],
) -> Vec<f32> {
    use sotf_host::{ParameterId, ParameterValue};
    let events = [
        (
            rate as usize + 17,
            "auto_gain_enabled",
            ParameterValue::Bool(false),
        ),
        (
            rate as usize + 3911,
            "auto_gain_enabled",
            ParameterValue::Bool(true),
        ),
        (
            2 * rate as usize + 19,
            "auto_gain_max_db",
            ParameterValue::Float(8.0),
        ),
        (
            3 * rate as usize + 71,
            "auto_gain_smoothing_ms",
            ParameterValue::Float(500.0),
        ),
    ];
    let channels = plugin.output_channels();
    let total = input.len() / 2;
    let mut output = vec![f32::NAN; total * channels];
    let mut position = 0;
    let mut event = 0;
    let mut iteration = 0;
    while position < total {
        if event < events.len() && position == events[event].0 {
            plugin
                .set_parameter(ParameterId::from(events[event].1), events[event].2.clone())
                .unwrap();
            event += 1;
        }
        let end = events.get(event).map_or(total, |e| e.0).min(total);
        let frames = pattern[iteration % pattern.len()].min(end - position);
        let mut context = ProcessContext::new(rate, frames);
        context.transport.sample_position = position as u64;
        assert_eq!(
            plugin
                .process(
                    &input[2 * position..2 * (position + frames)],
                    &mut output[channels * position..channels * (position + frames)],
                    &context
                )
                .unwrap(),
            frames
        );
        iteration += 1;
        position += frames;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

#[test]
fn fixed_position_autogain_controls_and_reset_preserve_sample_clock() {
    use sotf_host::{ParameterId, ParameterValue};
    let rate = 48_000;
    let input = source(rate, 6 * rate as usize + 17);
    let expected = render_with_control_events(&mut plugin(rate, true), &input, rate, &[137]);
    let mut actual_plugin = plugin(rate, true);
    let actual = render_with_control_events(&mut actual_plugin, &input, rate, &[1, 17, 8193, 512]);
    assert_audio_equal(&actual, &expected, "fixed-position controls");
    actual_plugin
        .set_parameter(
            ParameterId::from("auto_gain_max_db"),
            ParameterValue::Float(12.0),
        )
        .unwrap();
    actual_plugin
        .set_parameter(
            ParameterId::from("auto_gain_smoothing_ms"),
            ParameterValue::Float(100.0),
        )
        .unwrap();
    actual_plugin.reset();
    let expected = render(&mut plugin(rate, true), &input, rate, &[137]);
    let actual = render(&mut actual_plugin, &input, rate, &[8193, 17]);
    assert_audio_equal(&actual, &expected, "reset vs fresh");
}

#[test]
fn warm_autogain_drain_matches_ordinary_zero_continuation_at_partial_capacities() {
    let rate = 48_000;
    // The programme ends 111 frames before a 10 Hz boundary, after real meter
    // warmup. Native tail generation must cross it exactly once, regardless of
    // how slowly a consumer reads each canonical output cache.
    let input = source(rate, 6 * rate as usize + rate as usize / 10 - 111);
    let mut oracle = plugin(rate, true);
    let expected_programme = render(&mut oracle, &input, rate, &[137]);
    let hop = oracle.drain_output_frames_max();
    assert!(hop > 0);
    let mut expected_tail: Option<Vec<f32>> = None;
    for capacity in [1, 17, hop] {
        let mut actual = plugin(rate, true);
        let actual_programme = render(&mut actual, &input, rate, &[1, 17, 8193, 512]);
        assert_audio_equal(
            &actual_programme,
            &expected_programme,
            "programme before drain",
        );
        let channels = actual.output_channels();
        let mut tail = Vec::new();
        let mut output = vec![9876.0; capacity * channels + channels];
        let mut calls = 0;
        loop {
            let result = actual
                .drain(
                    &mut output[..capacity * channels],
                    &ProcessContext::new(rate, capacity),
                )
                .unwrap();
            assert!(result.frames <= capacity);
            assert!(output[capacity * channels..].iter().all(|x| *x == 9876.0));
            assert!(
                output[..result.frames * channels]
                    .iter()
                    .all(|x| x.is_finite())
            );
            tail.extend_from_slice(&output[..result.frames * channels]);
            calls += 1;
            assert!(calls < 1_000_000, "drain must terminate");
            if result.complete {
                break;
            }
        }
        assert!(tail.len() / channels >= 111);
        if let Some(expected) = &expected_tail {
            assert_audio_equal(&tail, expected, "partial-cache drain");
        } else {
            let zeros = vec![0.0; tail.len() / channels * 2];
            let expected = render(&mut oracle, &zeros, rate, &[hop]);
            assert_audio_equal(&tail, &expected, "ordinary zero continuation");
            expected_tail = Some(expected);
        }
        let complete = actual
            .drain(
                &mut output[..capacity * channels],
                &ProcessContext::new(rate, capacity),
            )
            .unwrap();
        assert_eq!(complete.frames, 0);
        assert!(complete.complete);
    }
}
