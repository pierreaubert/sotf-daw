// Rust guideline compliant 2026-09-28
use super::{AecPlugin, AecPluginParams, Plugin, ProcessContext};
use sotf_host::plugin::TailLength;

const B: usize = 256;

#[test]
fn wrong_process_clock_preserves_constructor_and_initialized_audio_epochs() {
    for rate in [44_100, 48_000, 96_000] {
        for constructor_only in [false, true] {
            let make = || {
                if constructor_only {
                    AecPlugin::new(rate)
                } else {
                    AecPlugin::from_params(rate, AecPluginParams::default()).unwrap()
                }
            };
            for frames in [B + 1, 1, 0] {
                for wrong_rate in [rate / 2, 0] {
                    let mut actual = make();
                    let mut expected = make();
                    let input: Vec<f32> = (0..3 * B + 17)
                        .flat_map(|n| [(n % 13) as f32 / 64.0, (n % 7) as f32 / 32.0])
                        .collect();
                    assert_eq!(
                        collect_input(&mut actual, &input, rate, 73),
                        collect_input(&mut expected, &input, rate, 73)
                    );
                    let learned = actual.aec.learned_snapshot();
                    let suppressor = actual.post_filter.learned_snapshot();
                    let mut sentinel = [42.0; B + 1];
                    assert!(
                        actual
                            .process(
                                &input[..2 * frames],
                                &mut sentinel[..frames],
                                &ProcessContext::new(wrong_rate, frames)
                            )
                            .is_err(),
                        "rate={rate},constructor_only={constructor_only},frames={frames},wrong={wrong_rate}"
                    );
                    assert_eq!(sentinel, [42.0; B + 1]);
                    assert_eq!(actual.aec.learned_snapshot(), learned);
                    assert_eq!(actual.post_filter.learned_snapshot(), suppressor);
                    let mut got = collect_input(&mut actual, &input, rate, 137);
                    let mut reference = collect_input(&mut expected, &input, rate, 137);
                    collect_tail(&mut actual, rate, 17, &mut got);
                    collect_tail(&mut expected, rate, 17, &mut reference);
                    assert_eq!(got, reference);
                }
            }
        }
    }
}

#[test]
fn native_tail_tracks_prepared_partitions_and_survives_reset() {
    for rate in [44_100, 48_000, 96_000] {
        for echo_tail_ms in [50.0, 100.0, 500.0] {
            let mut plugin = AecPlugin::from_params(
                rate,
                AecPluginParams {
                    echo_tail_ms,
                    ..Default::default()
                },
            )
            .unwrap();
            for initialized_rate in [rate, 192_000] {
                if initialized_rate != rate {
                    plugin.initialize(initialized_rate).unwrap();
                }
                let samples = (initialized_rate as f64 * echo_tail_ms / 1000.0) as usize;
                let expected = TailLength::Finite(((samples.div_ceil(B) + 2) * B) as u64);
                assert_eq!(plugin.tail_length(), expected);
                assert!(plugin.initialize(0).is_err());
                assert_eq!(plugin.tail_length(), expected);
                plugin.reset();
                assert_eq!(plugin.tail_length(), expected);
            }
        }
    }
}

#[test]
fn ordinary_zero_input_has_finite_support_while_learning_and_toggles_continue() {
    for rate in [44_100, 48_000, 96_000] {
        for echo_tail_ms in [50.0, 125.0] {
            for (post_filter_enabled, toggle) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                for (phase, callback) in [(1, 1), (255, 73), (256, 1024)] {
                    let mut plugin = AecPlugin::from_params(
                        rate,
                        AecPluginParams {
                            echo_tail_ms,
                            post_filter_enabled,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    let support = ((rate as f64 * echo_tail_ms / 1000.0) as usize).div_ceil(B) * B;
                    let bound = support + 2 * B;
                    let mut impulse = vec![0.0; support];
                    impulse[0] = 0.25;
                    impulse[support - 1] = -0.125;
                    plugin.aec.install_test_foreground(&impulse);
                    let input: Vec<f32> = (0..3 * B + phase)
                        .flat_map(|n| [(n % 11) as f32 / 64.0, 0.25 + (n % 7) as f32 / 64.0])
                        .collect();
                    let _ = collect_input(&mut plugin, &input, rate, callback);
                    assert!(plugin.aec.background_weight_energy() > 0.0);
                    let learned = plugin.aec.learned_snapshot();
                    let suppressor = plugin.post_filter.learned_snapshot();
                    if toggle {
                        plugin
                            .set_parameter(
                                sotf_host::parameters::ParameterId::from("post_filter_enabled"),
                                sotf_host::parameters::ParameterValue::Bool(!post_filter_enabled),
                            )
                            .unwrap();
                        assert_ne!(plugin.post_filter_mix, plugin.post_filter_mix_target);
                    }
                    let zeros = vec![0.0; 2 * (bound + 3 * B)];
                    let output = collect_input(&mut plugin, &zeros, rate, callback);
                    assert_eq!(plugin.tail_length(), TailLength::Finite(bound as u64));
                    assert!(output[..bound].iter().any(|sample| sample.abs() > 1e-5));
                    assert!(
                        output[bound..].iter().all(|&sample| sample == 0.0),
                        "rate={rate},tail={echo_tail_ms},post={post_filter_enabled},toggle={toggle},phase={phase}"
                    );
                    assert_ne!(
                        plugin.aec.learned_snapshot(),
                        learned,
                        "ordinary zeros must keep learning"
                    );
                    assert_ne!(plugin.post_filter.learned_snapshot(), suppressor);
                }
            }
        }
    }
}

fn collect_input(plugin: &mut AecPlugin, input: &[f32], rate: u32, callback: usize) -> Vec<f32> {
    let mut output = vec![0.0; input.len() / 2];
    for start in (0..output.len()).step_by(callback) {
        let end = (start + callback).min(output.len());
        plugin
            .process(
                &input[2 * start..2 * end],
                &mut output[start..end],
                &ProcessContext::new(rate, end - start),
            )
            .unwrap();
    }
    output
}

fn collect_tail(plugin: &mut AecPlugin, rate: u32, capacity: usize, output: &mut Vec<f32>) {
    let mut scratch = vec![f32::NAN; capacity];
    for _ in 0..20_000 {
        scratch.fill(f32::NAN);
        let result = plugin
            .drain(&mut scratch, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(result.frames <= capacity);
        assert!(scratch[result.frames..].iter().all(|x| x.is_nan()));
        output.extend_from_slice(&scratch[..result.frames]);
        if result.complete {
            return;
        }
        assert!(result.frames > 0);
    }
    panic!("AEC drain did not terminate");
}

#[test]
fn drain_matches_direct_nonzero_last_partition_convolution_and_freezes_learning() {
    for rate in [44_100, 48_000, 96_000] {
        for length in [1, 127, 255, 256, 257, 511] {
            for (callback, capacity) in [(1, 1), (73, 17), (256, 256)] {
                let mut plugin = AecPlugin::from_params(
                    rate,
                    AecPluginParams {
                        echo_tail_ms: 50.0,
                        post_filter_enabled: false,
                        ..Default::default()
                    },
                )
                .unwrap();
                plugin.initialize(rate).unwrap();
                let support = ((rate as f64 * 0.05) as usize).div_ceil(B) * B;
                let mut impulse = vec![0.0; support];
                impulse[0] = 0.25;
                impulse[support - 1] = -0.125;
                plugin.aec.install_test_foreground(&impulse);
                let mut input = vec![0.0; length * 2];
                for (n, frame) in input.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    frame[0] = (n % 11) as f32 / 64.0;
                    frame[1] = 0.25 + (n % 7) as f32 / 64.0;
                }
                let mut output = collect_input(&mut plugin, &input, rate, callback);
                let learned = plugin.aec.learned_snapshot();
                let post_filter = plugin.post_filter.learned_snapshot();
                collect_tail(&mut plugin, rate, capacity, &mut output);
                assert_eq!(plugin.aec.learned_snapshot(), learned);
                assert_eq!(plugin.post_filter.learned_snapshot(), post_filter);
                let phase = (length - 1) % B + 1;
                assert_eq!(output.len(), length + support + 2 * B - phase);
                for (frame, &actual) in output.iter().enumerate() {
                    let expected = if let Some(n) = frame.checked_sub(B) {
                        let mic = input.get(2 * n).copied().unwrap_or(0.0) as f64;
                        let reference = input.get(2 * n + 1).copied().unwrap_or(0.0) as f64;
                        let last = n
                            .checked_sub(support - 1)
                            .and_then(|j| input.get(2 * j + 1))
                            .copied()
                            .unwrap_or(0.0) as f64;
                        mic - 0.25 * reference + 0.125 * last
                    } else {
                        0.0
                    };
                    assert!(
                        (actual as f64 - expected).abs() < 2e-6,
                        "rate={rate},length={length},callback={callback},capacity={capacity},frame={frame}: {actual} != {expected}"
                    );
                }
                let final_tap = B + length + support - 2;
                assert!((output[final_tap] - 0.125 * input[2 * length - 1]).abs() < 2e-6);
                assert!(output[final_tap].abs() > 0.03);
            }
        }
    }
}

#[test]
fn drain_uses_frozen_nonconstant_suppressor_gains_with_direct_time_domain_oracle() {
    for length in [1, 127, 255] {
        for capacity in [1, 17, 256] {
            let mut plugin = AecPlugin::from_params(
                48_000,
                AecPluginParams {
                    echo_tail_ms: 50.0,
                    post_filter_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            plugin.initialize(48_000).unwrap();
            let support = 10 * B;
            let mut impulse = vec![0.0; support];
            impulse[support - 1] = -0.125;
            plugin.aec.install_test_foreground(&impulse);
            // G(k)=0.5+0.25cos(2πk/512): circular taps 0.5 at zero,
            // 0.125 at ±1, applied to [zero block, error block].
            let gains: Vec<f32> = (0..=B)
                .map(|k| {
                    (0.5 + 0.25 * (std::f64::consts::TAU * k as f64 / (2 * B) as f64).cos()) as f32
                })
                .collect();
            plugin.post_filter.install_test_gains(&gains);
            plugin.post_filter_mix = 1.0;
            plugin.post_filter_mix_target = 1.0;
            let mut input = vec![0.0; length * 2];
            for (n, frame) in input.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                frame[0] = 0.125 + (n % 13) as f32 / 64.0;
                frame[1] = 0.25;
            }
            let mut output = collect_input(&mut plugin, &input, 48_000, 73);
            let learned = plugin.aec.learned_snapshot();
            let post_filter = plugin.post_filter.learned_snapshot();
            collect_tail(&mut plugin, 48_000, capacity, &mut output);
            assert_eq!(plugin.aec.learned_snapshot(), learned);
            assert_eq!(plugin.post_filter.learned_snapshot(), post_filter);
            let error = |n: usize| -> f64 {
                input.get(2 * n).copied().unwrap_or(0.0) as f64
                    + 0.125
                        * n.checked_sub(support - 1)
                            .and_then(|j| input.get(2 * j + 1))
                            .copied()
                            .unwrap_or(0.0) as f64
            };
            for (frame, &actual) in output.iter().enumerate() {
                let expected = frame.checked_sub(B).map_or(0.0, |n| {
                    0.5 * error(n)
                        + if n % B > 0 { 0.125 * error(n - 1) } else { 0.0 }
                        + if n % B < B - 1 {
                            0.125 * error(n + 1)
                        } else {
                            0.0
                        }
                });
                assert!(
                    (actual as f64 - expected).abs() < 2e-6,
                    "length={length},capacity={capacity},frame={frame}: {actual} != {expected}"
                );
            }
        }
    }
}
