//! Crossfeed's measurement publication follows accepted samples, not callback partitions.
// Rust guideline compliant 2026-02-21
use sotf_host::{
    AutoGain, AutoGainParams, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};

fn plugin(rate: u32, mode: CrossfeedMode, enabled: bool, target: f32) -> CrossfeedPlugin {
    let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams {
        mode,
        mix: 1.0,
        max_block_frames: 8193,
        autogain_enabled: enabled,
        autogain_target_lufs: target,
        autogain_max_gain_db: 12.0,
        autogain_smoothing_ms: 100.0,
        ..Default::default()
    })
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn source(rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let t = frame as f64 / f64::from(rate);
            let level = [0.02, 0.1, 0.04][frame / (rate as usize / 3) % 3];
            [
                (level * (std::f64::consts::TAU * 1000.0 * t).sin()) as f32,
                (level * 0.3 * (std::f64::consts::TAU * 3700.0 * t).sin()) as f32,
            ]
        })
        .collect()
}

fn render(plugin: &mut CrossfeedPlugin, input: &[f32], rate: u32, pattern: &[usize]) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut position = 0;
    let mut iteration = 0;
    while position < input.len() / 2 {
        let frames = pattern[iteration % pattern.len()].min(input.len() / 2 - position);
        let mut context = ProcessContext::new(rate, frames);
        context.transport.sample_position = position as u64;
        assert_eq!(
            plugin
                .process_in_place(&mut output[2 * position..2 * (position + frames)], &context)
                .unwrap(),
            frames
        );
        position += frames;
        iteration += 1;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

fn max_difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

#[test]
fn public_audio_is_independent_of_callback_partition() {
    for rate in [48_000, 96_000] {
        let input = source(rate, 6 * rate as usize + 17);
        for enabled in [false, true] {
            let expected = render(
                &mut plugin(rate, CrossfeedMode::Bauer, enabled, -30.0),
                &input,
                rate,
                &[137],
            );
            let actual = render(
                &mut plugin(rate, CrossfeedMode::Bauer, enabled, -30.0),
                &input,
                rate,
                &[8192],
            );
            assert!(
                actual == expected,
                "rate={rate} enabled={enabled} difference={}",
                max_difference(&actual, &expected)
            );
        }
    }
}

#[test]
fn later_input_suffix_cannot_change_earlier_compensated_output() {
    for rate in [48_000, 96_000] {
        let warm = source(rate, 109 * 4096);
        let first = source(rate, 8192);
        let mut second = first.clone();
        for frame in second[8192..].as_chunks_mut::<2>().0 {
            frame[0] *= 0.1;
            frame[1] *= 10.0;
        }
        for enabled in [false, true] {
            let mut a = plugin(rate, CrossfeedMode::Bauer, enabled, -30.0);
            let mut b = plugin(rate, CrossfeedMode::Bauer, enabled, -30.0);
            assert_eq!(
                render(&mut a, &warm, rate, &[4096]),
                render(&mut b, &warm, rate, &[4096])
            );
            let a = render(&mut a, &first, rate, &[8192]);
            let b = render(&mut b, &second, rate, &[8192]);
            assert!(
                a[..8192] == b[..8192],
                "rate={rate} enabled={enabled} prefix difference={}",
                max_difference(&a[..8192], &b[..8192])
            );
        }
    }
}

#[test]
fn independent_public_gain_reference_publishes_only_after_completed_intervals() {
    for (rate, mode) in [
        (44_100, CrossfeedMode::Bauer),
        (48_000, CrossfeedMode::Meier),
        (96_000, CrossfeedMode::Mb),
        (192_000, CrossfeedMode::Hrtf),
    ] {
        let input = source(rate, 6 * rate as usize + 17);
        let raw = render(&mut plugin(rate, mode, false, -30.0), &input, rate, &[512]);
        for target in [-40.0, -12.0] {
            let mut gain = AutoGain::new(
                2,
                rate,
                AutoGainParams {
                    enabled: true,
                    max_gain_db: 12.0,
                    smoothing_ms: 100.0,
                    ..Default::default()
                },
            )
            .unwrap();
            gain.set_target_lufs(Some(target)).unwrap();
            let mut expected = raw.clone();
            let interval = rate as usize / 10;
            for (source, destination) in input
                .chunks(interval * 2)
                .zip(expected.chunks_mut(interval * 2))
            {
                gain.ingest_input(source).unwrap();
                gain.ingest_output(destination).unwrap();
                for frame in destination.as_chunks_mut::<2>().0 {
                    let multiplier = gain.next_gain_linear();
                    frame[0] *= multiplier;
                    frame[1] *= multiplier;
                }
                if source.len() == interval * 2 {
                    gain.refresh_input_measurement();
                    gain.refresh_output_measurement();
                }
            }
            assert!(
                max_difference(&expected, &raw) > 1e-4,
                "oracle must exercise actual compensation"
            );
            for pattern in [&[1][..], &[17, 137, 8193][..], &[512][..]] {
                let actual = render(&mut plugin(rate, mode, true, target), &input, rate, pattern);
                assert!(
                    actual == expected,
                    "rate={rate} mode={mode:?} target={target} difference={}",
                    max_difference(&actual, &expected)
                );
            }
        }
    }
}

fn set(plugin: &mut CrossfeedPlugin, name: &str, value: ParameterValue) {
    plugin
        .parametric_set_parameter(ParameterId::from(name), value)
        .unwrap();
}

#[test]
fn dynamic_mix_ramp_remains_an_explicit_partition_dependent_negative_control() {
    let rate = 48_000;
    let input = source(rate, 8193);
    let mut a = plugin(rate, CrossfeedMode::Bauer, false, -30.0);
    let mut b = plugin(rate, CrossfeedMode::Bauer, false, -30.0);
    set(&mut a, "mix", ParameterValue::Float(0.0));
    set(&mut b, "mix", ParameterValue::Float(0.0));
    let a = render(&mut a, &input, rate, &[137]);
    let b = render(&mut b, &input, rate, &[8193]);
    assert!(max_difference(&a, &b) > 1e-5);
}

#[test]
fn reset_and_rejected_callbacks_preserve_the_measurement_epoch() {
    let rate = 48_000;
    let warm = source(rate, rate as usize / 2 + 17);
    let probe = source(rate, 8193);
    for mode in [
        CrossfeedMode::Bauer,
        CrossfeedMode::Meier,
        CrossfeedMode::Mb,
        CrossfeedMode::Hrtf,
    ] {
        let mut actual = plugin(rate, mode, true, -30.0);
        let mut reference = plugin(rate, mode, true, -30.0);
        assert_eq!(
            render(&mut actual, &warm, rate, &[137]),
            render(&mut reference, &warm, rate, &[137])
        );
        let mut malformed = [0.25; 5];
        assert!(
            actual
                .process_in_place(&mut malformed, &ProcessContext::new(rate, 2))
                .is_err()
        );
        assert_eq!(malformed, [0.25; 5]);
        let mut wrong_rate = [0.25; 4];
        assert!(
            actual
                .process_in_place(&mut wrong_rate, &ProcessContext::new(96_000, 2))
                .is_err()
        );
        assert_eq!(wrong_rate, [0.25; 4]);
        let mut too_large = vec![0.25; 8194 * 2];
        assert!(
            actual
                .process_in_place(&mut too_large, &ProcessContext::new(rate, 8194))
                .is_err()
        );
        assert!(too_large.iter().all(|&x| x == 0.25));
        assert!(actual.initialize(0.0).is_err());
        assert_eq!(
            actual
                .process_in_place(&mut [], &ProcessContext::new(rate, 0))
                .unwrap(),
            0
        );
        assert_eq!(
            render(&mut actual, &probe, rate, &[17, 8193]),
            render(&mut reference, &probe, rate, &[17, 8193])
        );
        actual.reset();
        let mut fresh = plugin(rate, mode, true, -30.0);
        assert_eq!(
            render(&mut actual, &warm, rate, &[8193]),
            render(&mut fresh, &warm, rate, &[137])
        );
        // Reinitialization resets meter phase along with its newly constructed
        // monitors. Preserve the preexisting gain-state initialization policy.
        actual.initialize(96_000.0).unwrap();
        fresh.initialize(96_000.0).unwrap();
        let input = source(96_000, 96_017);
        assert_eq!(
            render(&mut actual, &input, 96_000, &[17]),
            render(&mut fresh, &input, 96_000, &[8193])
        );
    }
}

#[test]
fn realtime_controls_and_disabled_intervals_follow_an_independent_active_clock() {
    let rate = 48_000;
    let input = source(rate, rate as usize * 2 + 17);
    let mut raw = plugin(rate, CrossfeedMode::Bauer, false, -30.0);
    let mut candidate = plugin(rate, CrossfeedMode::Bauer, true, -30.0);
    let mut gain = AutoGain::new(
        2,
        rate,
        AutoGainParams {
            enabled: true,
            max_gain_db: 12.0,
            smoothing_ms: 100.0,
            ..Default::default()
        },
    )
    .unwrap();
    gain.set_target_lufs(Some(-30.0)).unwrap();
    let events = [
        (20_017, 0),
        (23_031, 1),
        (39_111, 2),
        (51_003, 3),
        (62_039, 4),
        (72_067, 5),
    ];
    let mut event = 0;
    let mut enabled = true;
    let mut off = false;
    let mut active_frames = 0;
    let mut position = 0;
    let mut iteration = 0;
    while position < input.len() / 2 {
        if event < events.len() && position == events[event].0 {
            match events[event].1 {
                0 | 1 => {
                    enabled = events[event].1 == 1;
                    set(
                        &mut candidate,
                        "autogain_enabled",
                        ParameterValue::Bool(enabled),
                    );
                    gain.set_enabled(enabled);
                }
                2 => {
                    set(
                        &mut candidate,
                        "autogain_target_lufs",
                        ParameterValue::Float(-12.0),
                    );
                    gain.set_target_lufs(Some(-12.0)).unwrap();
                }
                3 => {
                    set(
                        &mut candidate,
                        "autogain_smoothing_ms",
                        ParameterValue::Float(500.0),
                    );
                    gain.set_smoothing_ms(500.0);
                }
                4 | 5 => {
                    off = events[event].1 == 4;
                    let mode = if off { 0 } else { 1 };
                    set(&mut candidate, "mode", ParameterValue::Int(mode));
                    set(&mut raw, "mode", ParameterValue::Int(mode));
                }
                _ => unreachable!(),
            }
            event += 1;
        }
        let next = events.get(event).map_or(input.len() / 2, |event| event.0);
        let frames = [17, 8193, 137][iteration % 3].min(next - position);
        let source = &input[2 * position..2 * (position + frames)];
        let mut expected = render(&mut raw, source, rate, &[frames]);
        if off {
            gain.reset();
            active_frames = 0;
        } else if enabled {
            // The independent oracle advances one frame at a time, including
            // zero-time publication at every 4800th active sample boundary.
            for (dry, wet) in source
                .as_chunks::<2>()
                .0
                .iter()
                .zip(expected.as_chunks_mut::<2>().0)
            {
                gain.ingest_input(dry).unwrap();
                gain.ingest_output(wet).unwrap();
                let value = gain.next_gain_linear();
                wet[0] *= value;
                wet[1] *= value;
                active_frames += 1;
                if active_frames == 4800 {
                    active_frames = 0;
                    gain.refresh_input_measurement();
                    gain.refresh_output_measurement();
                }
            }
        }
        let actual = render(&mut candidate, source, rate, &[frames]);
        assert_eq!(actual, expected, "position={position}");
        position += frames;
        iteration += 1;
    }
}
