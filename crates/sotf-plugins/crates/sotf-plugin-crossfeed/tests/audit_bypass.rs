//! Mix endpoint, hard bypass and smoothed comparison behavior.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

const MODES: [CrossfeedMode; 4] = [
    CrossfeedMode::Bauer,
    CrossfeedMode::Meier,
    CrossfeedMode::Mb,
    CrossfeedMode::Hrtf,
];

fn make(mode: CrossfeedMode, rate: u32, mix: f32, auto_gain: bool) -> CrossfeedPlugin {
    let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams {
        mode,
        mix,
        autogain_enabled: auto_gain,
        ..Default::default()
    })
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn process(plugin: &mut CrossfeedPlugin, rate: u32, audio: &mut [f32], partitions: &[usize]) {
    let mut position = 0;
    let mut block = 0;
    while position < audio.len() / 2 {
        let n = partitions[block % partitions.len()].min(audio.len() / 2 - position);
        let context = ProcessContext::new(rate, n);
        sotf_host::assert_no_allocs("crossfeed mix ramp", || {
            plugin
                .process_in_place(&mut audio[position * 2..(position + n) * 2], &context)
                .unwrap();
        });
        position += n;
        block += 1;
    }
}

#[test]
fn mix_endpoints_are_exact_after_smoothing() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for mode in MODES {
            let mut ramped = make(mode, rate, 0.0, false);
            let mut wet = make(mode, rate, 1.0, false);
            ramped
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
                .unwrap();
            let input: Vec<f32> = (0..rate)
                .flat_map(|i| {
                    [
                        (i as f32 * 0.013).sin() * 0.2,
                        (i as f32 * 0.037).cos() * 0.1,
                    ]
                })
                .collect();
            let mut a = input.clone();
            let mut b = input;
            process(&mut ramped, rate, &mut a, &[257, 1, 31]);
            process(&mut wet, rate, &mut b, &[8192]);
            let start = a.len() - 512;
            let error = a[start..]
                .iter()
                .zip(&b[start..])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                a[start..] == b[start..],
                "{rate} {mode:?}: mix did not reach wet endpoint, error {error}"
            );
            ramped
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                .unwrap();
            let mut settling = vec![0.1; rate as usize * 2];
            process(&mut ramped, rate, &mut settling, &[8192]);
            let input = [-0.0_f32, 0.0, 0.1234567, -0.7654321];
            let mut actual = input;
            ramped
                .process_in_place(&mut actual, &ProcessContext::new(rate, 2))
                .unwrap();
            assert!(
                actual
                    .iter()
                    .zip(input)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{rate} {mode:?}: dry mix changed input bits {actual:?}"
            );
        }
    }
}

#[test]
fn dry_mix_preserves_input_bits_including_signed_zero() {
    for mode in MODES {
        let mut plugin = make(mode, 48_000, 0.0, false);
        let input = [-0.0_f32, 0.0, 0.1234567, -0.7654321];
        let mut actual = input;
        plugin
            .process_in_place(&mut actual, &ProcessContext::new(48_000, 2))
            .unwrap();
        assert!(
            actual
                .iter()
                .zip(input)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "{mode:?}: dry mix changed input bits {actual:?}"
        );
    }
}

#[test]
fn smoothed_comparison_matches_an_independent_ramp_with_current_filter_history() {
    let mut worst = 0.0_f64;
    let mut max_step = 0.0_f32;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for mode in MODES {
            let mut dry = make(mode, rate, 0.0, false);
            let mut wet = make(mode, rate, 1.0, false);
            // One second of steady input leaves only the DC transfer. Dry mix
            // must retain that complete wet history instead of cold-starting it.
            let input: Vec<f32> = (0..rate).flat_map(|_| [0.2, -0.1]).collect();
            let mut a = input.clone();
            let mut b = input;
            process(&mut dry, rate, &mut a, &[257]);
            process(&mut wet, rate, &mut b, &[8192]);
            assert!(
                a.iter()
                    .zip([0.2, -0.1].iter().cycle())
                    .all(|(a, b)| a == b)
            );
            let steady = [b[b.len() - 2], b[b.len() - 1]];
            assert!(
                (steady[0] - 0.2).abs().max((steady[1] + 0.1).abs()) > 0.01,
                "exercise a nontransparent wet response"
            );
            let coefficient = (-1.0 / (0.02 * f64::from(rate))).exp();
            // The oracle uses a closed-form 20 ms exponential, not the
            // production control recurrence or any smoothing helper.
            for (from, to) in [(0.0, 1.0), (1.0, 0.0), (0.0, 1.0)] {
                dry.set_parameter(ParameterId::from("mix"), ParameterValue::Float(to))
                    .unwrap();
                let mut audio: Vec<f32> = (0..rate).flat_map(|_| [0.2, -0.1]).collect();
                process(&mut dry, rate, &mut audio, &[1, 31, 257]);
                let previous = [
                    0.2 * (1.0 - from) + steady[0] * from,
                    -0.1 * (1.0 - from) + steady[1] * from,
                ];
                let mut last = previous;
                for (n, frame) in audio.as_chunks::<2>().0.iter().enumerate() {
                    let mix = f64::from(to) + f64::from(from - to) * coefficient.powi(n as i32 + 1);
                    for channel in 0..2 {
                        let input = if channel == 0 { 0.2 } else { -0.1 };
                        let expected = input * (1.0 - mix) + f64::from(steady[channel]) * mix;
                        let error = (f64::from(frame[channel]) - expected).abs();
                        worst = worst.max(error);
                        // Budget includes f32 recursive rounding and the explicit
                        // target snap; bounded independently before execution.
                        assert!(
                            error < 2e-5,
                            "{rate} {mode:?} {from}->{to} frame {n}: ramp error {error}"
                        );
                        let step = (frame[channel] - last[channel]).abs();
                        max_step = max_step.max(step);
                        // At DC the source has no slope: all movement is control.
                        assert!(step < 0.001, "{rate} {mode:?}: mix-switch step {step}");
                    }
                    last = *frame;
                }
                let endpoint = if to == 0.0 { [0.2, -0.1] } else { steady };
                assert!(
                    audio[audio.len() - 2..] == endpoint,
                    "settled comparison endpoint"
                );
            }
        }
    }
    eprintln!("independent smoothed comparison: max error {worst}, max step {max_step}");
}

#[test]
fn immediate_bypass_is_exact_and_reentry_has_no_stale_history() {
    let mut largest_jump = 0.0_f32;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for mode in MODES {
            for auto_gain in [false, true] {
                let mut plugin = make(mode, rate, 1.0, auto_gain);
                let mut dc: Vec<f32> = (0..rate).flat_map(|_| [0.2, -0.1]).collect();
                process(&mut plugin, rate, &mut dc, &[257]);
                let previous = [dc[dc.len() - 2], dc[dc.len() - 1]];
                // Hard bypass is intentionally immediate and reset-on-entry.
                // Measure the discontinuity rather than labelling it click-free.
                let id = ParameterId::from("enabled");
                sotf_host::assert_no_allocs("crossfeed hard bypass", || {
                    plugin
                        .set_parameter(id.clone(), ParameterValue::Bool(false))
                        .unwrap();
                });
                let mut bypass = [0.2, -0.1];
                plugin
                    .process_in_place(&mut bypass, &ProcessContext::new(rate, 1))
                    .unwrap();
                assert!(bypass == [0.2, -0.1]);
                largest_jump = largest_jump
                    .max((bypass[0] - previous[0]).abs())
                    .max((bypass[1] - previous[1]).abs());
                plugin
                    .set_parameter(id, ParameterValue::Bool(true))
                    .unwrap();
                let mut reentry = vec![0.0; 257 * 2];
                reentry[0] = 0.1;
                let mut fresh = reentry.clone();
                process(&mut plugin, rate, &mut reentry, &[1, 7, 31]);
                process(
                    &mut make(mode, rate, 1.0, auto_gain),
                    rate,
                    &mut fresh,
                    &[257],
                );
                assert!(
                    reentry == fresh,
                    "{rate} {mode:?} AG={auto_gain}: stale filter, delay or gain history after bypass"
                );
            }
        }
    }
    assert!(
        largest_jump > 0.01,
        "fixture must expose the hard-switch discontinuity"
    );
    eprintln!("largest intentional immediate bypass DC jump: {largest_jump}");
}

#[test]
fn rapid_mix_retargets_remain_continuous_and_match_the_independent_clock() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for mode in MODES {
            let mut plugin = make(mode, rate, 0.0, false);
            let mut reference = make(mode, rate, 1.0, false);
            let mut history: Vec<f32> = (0..rate).flat_map(|_| [0.2, -0.1]).collect();
            let mut wet = history.clone();
            process(&mut plugin, rate, &mut history, &[257]);
            process(&mut reference, rate, &mut wet, &[8192]);
            let steady = [wet[wet.len() - 2], wet[wet.len() - 1]];
            let coefficient = (-1.0 / (0.02 * f64::from(rate))).exp();
            let mut state = 0.0_f64;
            let mut last = [0.2_f32, -0.1];
            for (target, frames) in [
                (1.0, 17),
                (0.0, 31),
                (0.8, 127),
                (0.1, 5),
                (1.0, 257),
                (0.0, 13),
            ] {
                plugin
                    .set_parameter(ParameterId::from("mix"), ParameterValue::Float(target))
                    .unwrap();
                let mut audio: Vec<f32> = (0..frames).flat_map(|_| [0.2, -0.1]).collect();
                process(&mut plugin, rate, &mut audio, &[1, 7, 31]);
                for (n, frame) in audio.as_chunks::<2>().0.iter().enumerate() {
                    let mix = f64::from(target)
                        + (state - f64::from(target)) * coefficient.powi(n as i32 + 1);
                    for channel in 0..2 {
                        let input = if channel == 0 { 0.2 } else { -0.1 };
                        let expected = input * (1.0 - mix) + f64::from(steady[channel]) * mix;
                        assert!(
                            (f64::from(frame[channel]) - expected).abs() < 2e-5,
                            "rapid retarget oracle mismatch"
                        );
                        assert!(
                            (frame[channel] - last[channel]).abs() < 0.001,
                            "rapid retarget step exceeds fixed bound"
                        );
                    }
                    last = *frame;
                }
                state = f64::from(target) + (state - f64::from(target)) * coefficient.powi(frames);
            }
        }
    }
}
