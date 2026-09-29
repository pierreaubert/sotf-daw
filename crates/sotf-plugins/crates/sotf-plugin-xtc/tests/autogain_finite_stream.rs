//! AutoGain finite streams use the same clock as ordinary zero continuation.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext, TailLength};
use sotf_plugin_xtc::{XtcData, XtcPlugin, XtcPluginParams};

fn make(n: usize, rate: u32) -> XtcPlugin {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size: n,
            auto_gain_enabled: true,
            bypass_xtc_filters: false,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn program(rate: u32) -> Vec<f32> {
    let mut random = 0x1234_5678_u32;
    (0..rate as usize * 6 + 17)
        .flat_map(|frame| {
            let t = frame as f64 / f64::from(rate);
            let amplitude = if t < 2.0 {
                0.03
            } else if t < 4.0 {
                0.3
            } else {
                1.25
            };
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let dense = f64::from(random >> 8) / f64::from(1_u32 << 24) * 2.0 - 1.0;
            [
                (amplitude
                    * ((std::f64::consts::TAU * 431.0 * t).sin()
                        + 0.3 * (std::f64::consts::TAU * 3821.0 * t).sin()
                        + 0.08 * dense)) as f32,
                (amplitude
                    * (0.7 * (std::f64::consts::TAU * 911.0 * t).sin()
                        - 0.2 * (std::f64::consts::TAU * 3821.0 * t).cos()
                        - 0.08 * dense)) as f32,
            ]
        })
        .collect()
}

fn process(plugin: &mut XtcPlugin, input: &[f32], rate: u32, pattern: &[usize]) -> Vec<f32> {
    let frames = input.len() / 2;
    let mut output = vec![987.0; input.len() + 4];
    let mut position = 0;
    let mut block = 0;
    while position < frames {
        let count = pattern[block % pattern.len()].min(frames - position);
        let mut context = ProcessContext::new(rate, count);
        context.transport.sample_position = position as u64;
        assert_eq!(
            plugin.process(
                &input[position * 2..(position + count) * 2],
                &mut output[2 + position * 2..2 + (position + count) * 2],
                &context
            ),
            Ok(count)
        );
        position += count;
        block += 1;
    }
    assert_eq!(&output[..2], &[987.0; 2]);
    assert_eq!(&output[output.len() - 2..], &[987.0; 2]);
    output[2..output.len() - 2].to_vec()
}

fn enabled(plugin: &mut XtcPlugin, value: bool) {
    plugin
        .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(value))
        .unwrap();
}

fn data(plugin: &XtcPlugin) -> XtcData {
    plugin
        .get_data()
        .unwrap()
        .downcast_ref::<XtcData>()
        .unwrap()
        .clone()
}

fn render_history(
    plugin: &mut XtcPlugin,
    source: &[f32],
    rate: u32,
    pattern: &[usize],
    route: usize,
) -> Vec<f32> {
    let frames = source.len() / 2;
    let ramp = (rate as usize + 50) / 100;
    let mut events = vec![
        (0, true),
        (rate as usize * 2, false),
        (rate as usize * 3 + 137, true),
    ];
    if route == 1 {
        events.push((frames - ramp - 17, false));
    } else if route == 2 {
        events.push((frames - 1, false));
    }
    events.push((frames, route == 0));
    let mut result = Vec::with_capacity(source.len());
    for pair in events.windows(2) {
        let (start, value) = pair[0];
        let end = pair[1].0;
        enabled(plugin, value);
        enabled(plugin, value); // Repeated native snapshots must not restart fades.
        result.extend(process(plugin, &source[start * 2..end * 2], rate, pattern));
    }
    let snapshot = data(plugin);
    assert!(snapshot.auto_gain.input_lufs.is_finite());
    assert!(snapshot.auto_gain.output_lufs.is_finite());
    assert!(
        snapshot.auto_gain.gain_db.abs() > 0.01,
        "AutoGain never became active: {snapshot:?}"
    );
    assert!(
        snapshot.limiter_envelope < 0.99,
        "limiter never became active: {snapshot:?}"
    );
    result
}

fn assert_audio(actual: &[f32], expected: &[f32], label: &str) -> f32 {
    assert_eq!(actual.len(), expected.len(), "{label}");
    let mut worst = 0.0_f32;
    for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        let error = (actual - expected).abs();
        assert!(
            actual.is_finite() && error < 1.5e-6,
            "{label}, sample={index}: actual={actual}, expected={expected}, error={error}"
        );
        worst = worst.max(error);
    }
    worst
}

#[test]
fn active_autogain_finite_stream_matches_independently_partitioned_zero_continuation() {
    let patterns: [&[usize]; 4] = [&[1], &[17], &[137], &[8193, 137, 1]];
    let mut worst = 0.0_f32;
    let mut cases = 0;
    for (rate_index, rate) in [44_100, 48_000, 96_000, 192_000].into_iter().enumerate() {
        let source = program(rate);
        for (size_index, n) in [128, 2048, 16384].into_iter().enumerate() {
            let route = (rate_index + size_index) % 3;
            let pattern = patterns[(rate_index * 3 + size_index) % patterns.len()];
            let label = format!("N={n},rate={rate},route={route},pattern={pattern:?}");
            let mut actual = make(n, rate);
            let mut reference = make(n, rate);
            assert_eq!(actual.latency_samples(), n);
            assert!(
                actual
                    .drain(&mut [], &ProcessContext::new(rate, 0))
                    .unwrap()
                    .complete
            );
            let mut result = render_history(&mut actual, &source, rate, pattern, route);
            let mut expected = render_history(&mut reference, &source, rate, &[257, 509], route);
            assert!(result[..n * 2].iter().all(|&sample| sample == 0.0));
            worst = worst.max(assert_audio(&result, &expected, &label));

            let hop = n / 4;
            let frames = source.len() / 2;
            let tail_frames = if route == 1 {
                n
            } else {
                2 * n - hop + (hop - frames % hop) % hop
            };
            let metadata = TailLength::Finite(if route == 1 {
                n as u64
            } else {
                (2 * n - 1) as u64
            });
            assert_eq!(actual.tail_length(), metadata);
            // Ordinary zeros have a different partition from both process and
            // drain. No assumption about a canonical AutoGain callback is used.
            expected.extend(process(
                &mut reference,
                &vec![0.0; tail_frames * 2],
                rate,
                &[137, 1, 8193, 17],
            ));
            let mut emitted = 0;
            let mut calls = 0;
            loop {
                // First capacity1 leaves a partial hop cache in every case.
                let capacity = [1, 17, 137, 8193][calls % 4];
                let mut storage = vec![987.0; capacity * 2 + 4];
                let step = actual
                    .drain(
                        &mut storage[2..2 + capacity * 2],
                        &ProcessContext::new(rate, 0),
                    )
                    .unwrap();
                assert!(step.frames > 0 && step.frames <= capacity.min(hop));
                assert_eq!(&storage[..2], &[987.0; 2]);
                assert!(
                    storage[2 + step.frames * 2..]
                        .iter()
                        .all(|&sample| sample == 987.0)
                );
                result.extend_from_slice(&storage[2..2 + step.frames * 2]);
                emitted += step.frames;
                calls += 1;
                assert_eq!(actual.tail_length(), metadata);
                assert!(actual.drain_call_bound().unwrap().get() > 0);
                assert!(calls < 20_000);
                if step.complete {
                    break;
                }
            }
            assert_eq!(emitted, tail_frames, "{label}");
            assert_eq!(result.len(), (frames + tail_frames) * 2);
            worst = worst.max(assert_audio(&result, &expected, &label));
            let a = data(&actual);
            let e = data(&reference);
            assert!(
                (a.auto_gain.input_lufs - e.auto_gain.input_lufs).abs() < 1e-9,
                "{label}"
            );
            assert!(
                (a.auto_gain.output_lufs - e.auto_gain.output_lufs).abs() < 1e-9,
                "{label}"
            );
            assert!(
                (a.auto_gain.gain_db - e.auto_gain.gain_db).abs() < 1e-9,
                "{label}"
            );
            assert_eq!(actual.drain_call_bound().unwrap().get(), 1);
            assert!(
                actual
                    .drain(&mut [], &ProcessContext::new(rate, 0))
                    .unwrap()
                    .complete
            );
            let mut sentinel = [987.0; 2];
            assert!(
                actual
                    .process(&[0.0; 2], &mut sentinel, &ProcessContext::new(rate, 1))
                    .is_err()
            );
            assert_eq!(sentinel, [987.0; 2]);
            enabled(&mut actual, route == 0);
            assert!(
                actual
                    .set_parameter(
                        ParameterId::from("enabled"),
                        ParameterValue::Bool(route != 0)
                    )
                    .is_err()
            );
            actual.reset();
            assert!(
                process(&mut actual, &vec![0.0; (2 * n + 17) * 2], rate, &[17, 8193])
                    .iter()
                    .all(|&sample| sample == 0.0)
            );
            cases += 1;
        }
    }
    eprintln!("AUD104 active finite stream cases={cases}, maximum waveform difference={worst:e}");
}
