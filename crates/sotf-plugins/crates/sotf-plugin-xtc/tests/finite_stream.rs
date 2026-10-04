//! Independent finite-stream support and lifecycle regressions.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};

const RATE: u32 = 48_000;

fn neutral(n: usize, rate: u32) -> XtcPlugin {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size: n,
            bypass_xtc_filters: true,
            auto_gain_enabled: false,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn drain(plugin: &mut XtcPlugin, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut result = Vec::new();
    for &capacity in capacities.iter().cycle().take(40_000) {
        let mut output = vec![123.0; capacity * channels];
        let emitted = plugin
            .drain(&mut output, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(emitted.frames <= capacity);
        assert!(
            output[emitted.frames * channels..]
                .iter()
                .all(|v| *v == 123.0)
        );
        result.extend_from_slice(&output[..emitted.frames * channels]);
        if emitted.complete {
            return result;
        }
        assert!(emitted.frames > 0);
    }
    panic!("finite XTC tail did not complete");
}

#[test]
fn final_marker_is_retained_after_eof() {
    let n = 128;
    let mut plugin = neutral(n, RATE);
    let mut input = [0.0; 34];
    input[32] = 0.75;
    input[33] = -0.25;
    let mut output = vec![0.0; input.len()];
    plugin
        .process(&input, &mut output, &ProcessContext::new(RATE, 17))
        .unwrap();
    output.extend(drain(&mut plugin, RATE, &[7, 32]));
    assert_eq!(output.len(), n * 4);
    assert!((output[(n + 16) * 2] - 0.75).abs() < 1.5e-6);
    assert!((output[(n + 16) * 2 + 1] + 0.25).abs() < 1.5e-6);
}

fn process(plugin: &mut XtcPlugin, rate: u32, input: &[f32], blocks: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let frames = input.len() / 2;
    let mut output = vec![0.0; frames * channels];
    let mut position = 0;
    for &block in blocks.iter().cycle() {
        let n = block.min(frames - position);
        if n == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &input[position * 2..(position + n) * 2],
                    &mut output[position * channels..(position + n) * channels],
                    &ProcessContext::new(rate, n),
                )
                .unwrap(),
            n
        );
        position += n;
    }
    output
}

#[test]
fn neutral_tail_matches_independent_delayed_source_at_all_small_hop_phases() {
    let mut worst = 0.0_f64;
    let mut minimum_snr = f64::INFINITY;
    let mut cases = 0;
    for n in [128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        let hop = n / 4;
        let lengths: Vec<_> = if n <= 512 {
            (1..=hop).collect()
        } else {
            vec![1, hop - 1, hop, hop + 1, n - 1, n + 1, n * 5 + 73]
        };
        for rate in [44_100, 192_000] {
            let mut plugin = neutral(n, rate);
            for &frames in &lengths {
                let input: Vec<_> = (0..frames * 2)
                    .map(|i| ((i * 23 % 127) as i32 - 63) as f32 / 256.0)
                    .collect();
                let expected_tail = 2 * n - hop + (hop - frames % hop) % hop;
                for capacities in [&[1][..], &[17, hop + 1, 3][..], &[hop][..]] {
                    plugin.reset();
                    let mut actual = process(&mut plugin, rate, &input, &[16385, 137, 1]);
                    let tail = drain(&mut plugin, rate, capacities);
                    assert_eq!(tail.len(), expected_tail * 2);
                    actual.extend(tail);
                    assert!(actual[..n * 2].iter().all(|value| *value == 0.0));
                    let mut signal_energy = 0.0;
                    let mut error_energy = 0.0;
                    for (i, &value) in actual.iter().enumerate() {
                        let expected = i
                            .checked_sub(n * 2)
                            .and_then(|j| input.get(j))
                            .copied()
                            .unwrap_or(0.0);
                        let error = f64::from((value - expected).abs());
                        worst = worst.max(error);
                        signal_energy += f64::from(expected).powi(2);
                        error_energy += error * error;
                        assert!(
                            error < 1.5e-6,
                            "N={n} rate={rate} S={frames} sample={i} error={error}"
                        );
                    }
                    if error_energy > 0.0 {
                        minimum_snr =
                            minimum_snr.min(10.0 * (signal_energy / error_energy).log10());
                    }
                    cases += 1;
                }
            }
        }
    }
    eprintln!(
        "XTC neutral finite matrix: cases={cases}, max_absolute_error={worst:e}, minimum_snr_db={minimum_snr}"
    );
}

#[test]
fn ordinary_toggle_histories_use_the_input_clock_and_freeze_the_selected_eof_route() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    let n = 128;
    for history in [
        vec![(true, 17), (false, 19), (true, 29)],
        vec![(false, 37), (true, 1)],
        vec![(true, 63), (false, 7)],
        vec![(false, 17)],
        vec![(false, 17), (true, 0)],
        vec![(true, 9), (false, 0), (true, 0)],
    ] {
        let mut actual = neutral(n, RATE);
        let mut reference = neutral(n, RATE);
        let mut accepted_frames = 0;
        for &(enabled, frames) in &history {
            for plugin in [&mut actual, &mut reference] {
                plugin
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(enabled))
                    .unwrap();
            }
            let input: Vec<_> = (0..frames * 2).map(|i| (i % 17) as f32 / 64.0).collect();
            assert_eq!(
                process(&mut actual, RATE, &input, &[7]),
                process(&mut reference, RATE, &input, &[7])
            );
            accepted_frames += frames;
        }
        let enabled = history.last().unwrap().0;
        // Every listed fade is shorter than 10 ms; both paths remain observable.
        let expected_frames = 2 * n - n / 4 + (n / 4 - accepted_frames % (n / 4)) % (n / 4);
        assert_eq!(actual.tail_length(), TailLength::Finite((2 * n - 1) as u64));
        let actual_tail = drain(&mut actual, RATE, &[1, 17, 33]);
        assert_eq!(
            actual_tail,
            process(
                &mut reference,
                RATE,
                &vec![0.0; expected_frames * 2],
                &[n / 4]
            )
        );
        assert!(
            actual
                .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(!enabled))
                .is_err()
        );
        assert!(
            actual
                .process(&[0.0; 2], &mut [999.0; 2], &ProcessContext::new(RATE, 1))
                .is_err()
        );
        actual.reset();
        actual
            .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
            .unwrap();
        // Reset snaps to the configured endpoint; a live setter alone ramps.
        actual.reset();
        let input = [0.125; 70];
        let mut fresh = neutral(n, RATE);
        assert_eq!(
            process(&mut actual, RATE, &input, &[17]),
            process(&mut fresh, RATE, &input, &[17])
        );
        assert_eq!(
            drain(&mut actual, RATE, &[7]),
            drain(&mut fresh, RATE, &[7])
        );
    }
}

#[test]
fn invalid_destinations_and_failed_reinitialize_preserve_audio_and_eof_state() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    let mut uninitialized = XtcPlugin::new(XtcPluginParams::default(), RATE).unwrap();
    assert_eq!(uninitialized.tail_length(), TailLength::Unknown);
    assert!(
        uninitialized
            .drain(&mut [123.0; 2], &ProcessContext::new(RATE, 0))
            .is_err()
    );
    let mut actual = neutral(128, RATE);
    let mut reference = neutral(128, RATE);
    assert!(
        actual
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    let input = [0.125; 34];
    assert_eq!(
        process(&mut actual, RATE, &input, &[17]),
        process(&mut reference, RATE, &input, &[17])
    );
    for (len, rate) in [(0, RATE), (3, RATE), (4, 96_000)] {
        let mut output = [123.0; 4];
        assert!(
            actual
                .drain(&mut output[..len], &ProcessContext::new(rate, 0))
                .is_err()
        );
        assert_eq!(output, [123.0; 4]);
    }
    assert!(actual.initialize(0).is_err());
    assert_eq!(
        process(&mut actual, RATE, &input, &[17]),
        process(&mut reference, RATE, &input, &[17])
    );
    let mut first = [123.0; 2];
    let mut expected_first = first;
    actual
        .drain(&mut first, &ProcessContext::new(RATE, 0))
        .unwrap();
    reference
        .drain(&mut expected_first, &ProcessContext::new(RATE, 0))
        .unwrap();
    assert_eq!(first, expected_first);
    for parameter in actual.parameters() {
        let value = actual.get_parameter(&parameter.id).unwrap();
        actual.set_parameter(parameter.id, value).unwrap();
    }
    for (name, value) in [
        ("source_mode", ParameterValue::String("synthetic".into())),
        ("enabled", ParameterValue::Bool(true)),
        ("fft_size", ParameterValue::Int(128)),
    ] {
        actual
            .set_parameter(ParameterId::from(name), value)
            .unwrap();
    }
    assert!(
        actual
            .set_parameter(
                ParameterId::from("kappa_target"),
                ParameterValue::Float(42.0)
            )
            .is_err()
    );
    let mut rejected = [123.0; 2];
    assert!(
        actual
            .process(&[0.0; 2], &mut rejected, &ProcessContext::new(RATE, 1))
            .is_err()
    );
    assert_eq!(rejected, [123.0; 2]);
    assert_eq!(
        actual
            .process(&[], &mut [], &ProcessContext::new(RATE, 0))
            .unwrap(),
        0
    );
    assert_eq!(
        drain(&mut actual, RATE, &[1, 17, 64]),
        drain(&mut reference, RATE, &[32])
    );
    assert!(
        actual
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
}

#[test]
fn declared_work_bound_counts_partial_and_final_cache_delivery() {
    for frames in [1, 31, 32, 33, 127, 513] {
        for first_capacity in [0, 1, 17, 32] {
            let mut plugin = neutral(128, RATE);
            process(&mut plugin, RATE, &vec![0.125; frames * 2], &[17]);
            if first_capacity > 0 {
                plugin
                    .drain(
                        &mut vec![0.0; first_capacity * 2],
                        &ProcessContext::new(RATE, 0),
                    )
                    .unwrap();
            }
            let bound = plugin.drain_call_bound().unwrap().get();
            let mut calls = 0;
            loop {
                calls += 1;
                let result = plugin
                    .drain(&mut [0.0; 64], &ProcessContext::new(RATE, 0))
                    .unwrap();
                assert!(calls <= bound);
                if result.complete {
                    break;
                }
            }
            assert_eq!(calls, bound);
            assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
        }
    }
}
