//! Slew coverage beyond nominal 1.0 / ramped-upward / 48 kHz / 1-2 ch.
//!
//! ADDITIVE coverage for review P1-4 and P1-5; existing `smooth_cutoff.rs`
//! tests are untouched. Each test names its finding arm. Structural asserts
//! (counts, bit-exactness, completion, latency) use exact equality; only the
//! P1-1/P2-8 energy bounds live in `slew_artifact_bounds.rs`.
//!
//! Tag: `SLEW-COVERAGE` for convergence, drain, and latency lines.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::f64::consts::TAU;

const AMPLITUDE: f64 = 0.5;
const QUALITIES: [ResamplerQuality; 3] = [
    ResamplerQuality::Fast,
    ResamplerQuality::Medium,
    ResamplerQuality::High,
];

fn make_rate(
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    chunk: usize,
    quality: ResamplerQuality,
    smoothing: bool,
) -> ResamplerPlugin {
    let mut plugin =
        ResamplerPlugin::with_quality(channels, input_rate, output_rate, chunk, quality).unwrap();
    plugin.initialize(input_rate).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("cutoff_smoothing"),
            ParameterValue::Bool(smoothing),
        )
        .unwrap();
    plugin
}

fn tones_rate(
    frames: usize,
    channels: usize,
    frequencies: &[f64],
    start: usize,
    rate: u32,
) -> Vec<f32> {
    debug_assert_eq!(frequencies.len(), channels);
    (0..frames)
        .flat_map(|frame| {
            frequencies.iter().map(move |frequency| {
                (AMPLITUDE * (TAU * frequency * (start + frame) as f64 / f64::from(rate)).sin())
                    as f32
            })
        })
        .collect()
}

fn feed_rate(
    plugin: &mut ResamplerPlugin,
    input: &[f32],
    frames: usize,
    output: &mut Vec<f32>,
    rate: u32,
) {
    let channels = input.len() / frames.max(1);
    let mut block = vec![f32::NAN; plugin.output_frames_for_input(frames) * channels];
    let written = plugin
        .process(input, &mut block, &ProcessContext::new(rate, frames))
        .unwrap();
    output.extend_from_slice(&block[..written * channels]);
    assert!(block[written * channels..].iter().all(|x| x.is_nan()));
}

fn finish_rate(plugin: &mut ResamplerPlugin, channels: usize, output: &mut Vec<f32>, rate: u32) {
    let bound = plugin.drain_call_bound().unwrap().get() as usize;
    for _ in 0..bound {
        let mut block = vec![f32::NAN; plugin.drain_output_frames_max() * channels];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(rate, 0))
            .unwrap();
        output.extend_from_slice(&block[..result.frames * channels]);
        assert!(block[result.frames * channels..].iter().all(|x| x.is_nan()));
        if result.complete {
            return;
        }
    }
    panic!("drain did not complete within its advertised bound");
}

fn peak_difference(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0_f32, f32::max)
}

#[test]
fn nominal_half_smoothed_schedule_preserves_counts_and_reconverges() {
    // P1-4b: 48->24 (nominal 0.5, 18 tables). Settle at 0.25 (rank 0), ramp
    // to 1.0 (rank 17) at block 8; the 17-step slew converges during block
    // 24, so blocks 25+ plus the drain reconverge bit-exactly.
    for quality in QUALITIES {
        let mut smoothed = make_rate(48_000, 24_000, 1, 256, quality, true);
        let mut instant = make_rate(48_000, 24_000, 1, 256, quality, false);
        smoothed.set_ratio(0.25, false).unwrap();
        instant.set_ratio(0.25, false).unwrap();
        let mut smooth_blocks = Vec::new();
        let mut instant_blocks = Vec::new();
        let mut position = 0;
        for block in 0..30 {
            if block == 8 {
                smoothed.set_ratio(1.0, true).unwrap();
                instant.set_ratio(1.0, true).unwrap();
            }
            let input: Vec<f32> = (0..256).map(|n| ((position + n) % 23) as f32 / 128.0).collect();
            position += 256;
            let mut a = Vec::new();
            let mut b = Vec::new();
            feed_rate(&mut smoothed, &input, 256, &mut a, 48_000);
            feed_rate(&mut instant, &input, 256, &mut b, 48_000);
            smooth_blocks.push(a);
            instant_blocks.push(b);
        }
        for (index, (a, b)) in smooth_blocks.iter().zip(&instant_blocks).enumerate() {
            if index <= 8 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: pre-widening must be bit-exact"
                );
            }
            if index >= 25 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: converged output must be bit-exact"
                );
            }
        }
        let transition_smooth: Vec<f32> =
            smooth_blocks[9..13].iter().flatten().copied().collect();
        let transition_instant: Vec<f32> =
            instant_blocks[9..13].iter().flatten().copied().collect();
        let peak = peak_difference(&transition_smooth, &transition_instant);
        assert!(
            peak > 1e-6,
            "{quality:?}: nominal-0.5 slew must act on audio, peak {peak}"
        );
        let mut drain_smooth = Vec::new();
        let mut drain_instant = Vec::new();
        finish_rate(&mut smoothed, 1, &mut drain_smooth, 48_000);
        finish_rate(&mut instant, 1, &mut drain_instant, 48_000);
        assert!(
            drain_smooth == drain_instant,
            "{quality:?}: drain must reconverge bit-exactly"
        );
        let len_smooth: usize =
            smooth_blocks.iter().map(Vec::len).sum::<usize>() + drain_smooth.len();
        let len_instant: usize =
            instant_blocks.iter().map(Vec::len).sum::<usize>() + drain_instant.len();
        eprintln!("SLEW-COVERAGE {quality:?} nominal-0.5 lengths {len_smooth}");
        assert_eq!(len_smooth, len_instant, "{quality:?}: lengths diverged");
    }
}

#[test]
fn instant_upward_slew_converges_one_block_early() {
    // P1-4c + P2-9 evidence: instant (ramp=false) 0.5->2.0 advances one
    // table at control time plus one per backend chunk, so block 8 already
    // differs and convergence lands at block 16 (one block before the
    // ramped case at block 17). The index is recorded, not fitted.
    for quality in QUALITIES {
        let mut smoothed = make_rate(48_000, 48_000, 2, 256, quality, true);
        let mut instant = make_rate(48_000, 48_000, 2, 256, quality, false);
        smoothed.set_ratio(0.5, false).unwrap();
        instant.set_ratio(0.5, false).unwrap();
        let mut smooth_blocks = Vec::new();
        let mut instant_blocks = Vec::new();
        let mut position = 0;
        for block in 0..29 {
            if block == 8 {
                smoothed.set_ratio(2.0, false).unwrap();
                instant.set_ratio(2.0, false).unwrap();
            }
            let input = tones_rate(256, 2, &[997.0, 18_000.0], position, 48_000);
            position += 256;
            let mut a = Vec::new();
            let mut b = Vec::new();
            feed_rate(&mut smoothed, &input, 256, &mut a, 48_000);
            feed_rate(&mut instant, &input, 256, &mut b, 48_000);
            smooth_blocks.push(a);
            instant_blocks.push(b);
        }
        for (index, (a, b)) in smooth_blocks.iter().zip(&instant_blocks).enumerate() {
            if index < 8 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: pre-change must be bit-exact"
                );
            }
            if index >= 16 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: converged output must be bit-exact"
                );
            }
        }
        let peak = peak_difference(&smooth_blocks[8], &instant_blocks[8]);
        assert!(
            peak > 1e-6,
            "{quality:?}: control-time step must act on block 8, peak {peak}"
        );
        let mut drain_smooth = Vec::new();
        let mut drain_instant = Vec::new();
        finish_rate(&mut smoothed, 2, &mut drain_smooth, 48_000);
        finish_rate(&mut instant, 2, &mut drain_instant, 48_000);
        assert!(
            drain_smooth == drain_instant,
            "{quality:?}: drain must reconverge bit-exactly"
        );
        let len_smooth: usize =
            smooth_blocks.iter().map(Vec::len).sum::<usize>() + drain_smooth.len();
        let len_instant: usize =
            instant_blocks.iter().map(Vec::len).sum::<usize>() + drain_instant.len();
        eprintln!("SLEW-COVERAGE {quality:?} instant-upward converges at block 16");
        assert_eq!(len_smooth, len_instant, "{quality:?}: lengths diverged");
    }
}

#[test]
fn downward_narrowing_bit_exact_fast_medium_and_new_pairs() {
    // P1-4d: Fast/Medium join High on the deep-stop 0.5 case, plus 1.0->0.75
    // narrowing and the trivial 2.0->1.0 same-table pair. Downward moves
    // jump immediately in both modes, so every pair is bit-exact.
    for quality in [ResamplerQuality::Fast, ResamplerQuality::Medium] {
        let frames = 48_000 / 2 + 37;
        let input = tones_rate(frames, 1, &[18_000.0], 0, 48_000);
        let mut smoothed = make_rate(48_000, 48_000, 1, 256, quality, true);
        let mut instant = make_rate(48_000, 48_000, 1, 256, quality, false);
        smoothed.set_ratio(0.5, false).unwrap();
        instant.set_ratio(0.5, false).unwrap();
        let mut actual = Vec::new();
        let mut expected = Vec::new();
        feed_rate(&mut smoothed, &input, frames, &mut actual, 48_000);
        feed_rate(&mut instant, &input, frames, &mut expected, 48_000);
        finish_rate(&mut smoothed, 1, &mut actual, 48_000);
        finish_rate(&mut instant, 1, &mut expected, 48_000);
        assert!(
            actual == expected,
            "{quality:?}: downward narrowing must be bit-exact"
        );
    }
    for quality in QUALITIES {
        for (from, to) in [(1.0, 0.75), (2.0, 1.0)] {
            let music = tones_rate(2048, 1, &[997.0], 0, 48_000);
            let mut smoothed = make_rate(48_000, 48_000, 1, 256, quality, true);
            let mut instant = make_rate(48_000, 48_000, 1, 256, quality, false);
            smoothed.set_ratio(from, false).unwrap();
            instant.set_ratio(from, false).unwrap();
            let mut actual = Vec::new();
            let mut expected = Vec::new();
            feed_rate(&mut smoothed, &music[..1024], 1024, &mut actual, 48_000);
            feed_rate(&mut instant, &music[..1024], 1024, &mut expected, 48_000);
            smoothed.set_ratio(to, false).unwrap();
            instant.set_ratio(to, false).unwrap();
            feed_rate(&mut smoothed, &music[1024..], 1024, &mut actual, 48_000);
            feed_rate(&mut instant, &music[1024..], 1024, &mut expected, 48_000);
            finish_rate(&mut smoothed, 1, &mut actual, 48_000);
            finish_rate(&mut instant, 1, &mut expected, 48_000);
            assert!(
                actual == expected,
                "{quality:?} {from}->{to}: downward move must be bit-exact"
            );
        }
    }
}

#[test]
fn smoothing_covers_eight_channels_and_ninety_six_khz() {
    // P1-4e: one 8 ch run at 48 kHz and one 96 kHz run. Structural asserts
    // only (counts, bit-exactness, reconvergence); energy bounds stay in the
    // 48 kHz artifact suite where the 18 kHz probe sits in transition.
    let eight = [997.0, 2_000.0, 5_000.0, 8_000.0, 11_000.0, 14_000.0, 17_000.0, 18_000.0];
    let mut smoothed = make_rate(48_000, 48_000, 8, 256, ResamplerQuality::High, true);
    let mut instant = make_rate(48_000, 48_000, 8, 256, ResamplerQuality::High, false);
    smoothed.set_ratio(0.5, false).unwrap();
    instant.set_ratio(0.5, false).unwrap();
    let mut smooth_blocks = Vec::new();
    let mut instant_blocks = Vec::new();
    let mut position = 0;
    for block in 0..29 {
        if block == 8 {
            smoothed.set_ratio(2.0, true).unwrap();
            instant.set_ratio(2.0, true).unwrap();
        }
        let input = tones_rate(256, 8, &eight, position, 48_000);
        position += 256;
        let mut a = Vec::new();
        let mut b = Vec::new();
        feed_rate(&mut smoothed, &input, 256, &mut a, 48_000);
        feed_rate(&mut instant, &input, 256, &mut b, 48_000);
        smooth_blocks.push(a);
        instant_blocks.push(b);
    }
    for (index, (a, b)) in smooth_blocks.iter().zip(&instant_blocks).enumerate() {
        if index <= 8 {
            assert!(a == b, "8ch block {index}: pre-widening must be bit-exact");
        }
        if index >= 17 {
            assert!(a == b, "8ch block {index}: converged must be bit-exact");
        }
    }
    let transition_smooth: Vec<f32> =
        smooth_blocks[9..13].iter().flatten().copied().collect();
    let transition_instant: Vec<f32> =
        instant_blocks[9..13].iter().flatten().copied().collect();
    let peak = peak_difference(&transition_smooth, &transition_instant);
    assert!(peak > 1e-6, "8ch slew must act on audio, peak {peak}");
    let mut drain_smooth = Vec::new();
    let mut drain_instant = Vec::new();
    finish_rate(&mut smoothed, 8, &mut drain_smooth, 48_000);
    finish_rate(&mut instant, 8, &mut drain_instant, 48_000);
    assert!(drain_smooth == drain_instant, "8ch drain must reconverge");
    let len_smooth: usize =
        smooth_blocks.iter().map(Vec::len).sum::<usize>() + drain_smooth.len();
    let len_instant: usize =
        instant_blocks.iter().map(Vec::len).sum::<usize>() + drain_instant.len();
    eprintln!("SLEW-COVERAGE 8ch lengths {len_smooth}");
    assert_eq!(len_smooth, len_instant, "8ch lengths diverged");

    // 96 kHz: the 30 kHz probe sits above the narrow cutoff (22.7 kHz) and
    // below the wide cutoff (45.5 kHz) for High, so the slew acts on audio.
    let mut smoothed96 = make_rate(96_000, 96_000, 2, 256, ResamplerQuality::High, true);
    let mut instant96 = make_rate(96_000, 96_000, 2, 256, ResamplerQuality::High, false);
    smoothed96.set_ratio(0.5, false).unwrap();
    instant96.set_ratio(0.5, false).unwrap();
    let mut smooth96 = Vec::new();
    let mut instant96b = Vec::new();
    let mut position96 = 0;
    for block in 0..29 {
        if block == 8 {
            smoothed96.set_ratio(2.0, true).unwrap();
            instant96.set_ratio(2.0, true).unwrap();
        }
        let input = tones_rate(256, 2, &[997.0, 30_000.0], position96, 96_000);
        position96 += 256;
        let mut a = Vec::new();
        let mut b = Vec::new();
        feed_rate(&mut smoothed96, &input, 256, &mut a, 96_000);
        feed_rate(&mut instant96, &input, 256, &mut b, 96_000);
        smooth96.push(a);
        instant96b.push(b);
    }
    for (index, (a, b)) in smooth96.iter().zip(&instant96b).enumerate() {
        if index <= 8 {
            assert!(a == b, "96kHz block {index}: pre-widening must be bit-exact");
        }
        if index >= 17 {
            assert!(a == b, "96kHz block {index}: converged must be bit-exact");
        }
    }
    let transition96_smooth: Vec<f32> = smooth96[9..13].iter().flatten().copied().collect();
    let transition96_instant: Vec<f32> =
        instant96b[9..13].iter().flatten().copied().collect();
    let peak96 = peak_difference(&transition96_smooth, &transition96_instant);
    assert!(peak96 > 1e-6, "96kHz slew must act on audio, peak {peak96}");
    let mut drain96_smooth = Vec::new();
    let mut drain96_instant = Vec::new();
    finish_rate(&mut smoothed96, 2, &mut drain96_smooth, 96_000);
    finish_rate(&mut instant96, 2, &mut drain96_instant, 96_000);
    assert!(drain96_smooth == drain96_instant, "96kHz drain must reconverge");
    let len96_smooth: usize =
        smooth96.iter().map(Vec::len).sum::<usize>() + drain96_smooth.len();
    let len96_instant: usize =
        instant96b.iter().map(Vec::len).sum::<usize>() + drain96_instant.len();
    eprintln!("SLEW-COVERAGE 96kHz lengths {len96_smooth}");
    assert_eq!(len96_smooth, len96_instant, "96kHz lengths diverged");
}

#[test]
fn upward_preempted_by_downward_is_bit_exact_from_preemption() {
    // P1-5a: ramp 0.5->2.0 at block 8, then jump back to 0.5 at block 11
    // (mid-slew, ranks 1..2 already emitted). Both modes jump to narrow
    // immediately; FIR output depends only on the shared input window plus
    // the current table, so blocks 11+ reconverge bit-exactly at once.
    for quality in QUALITIES {
        let mut smoothed = make_rate(48_000, 48_000, 2, 256, quality, true);
        let mut instant = make_rate(48_000, 48_000, 2, 256, quality, false);
        smoothed.set_ratio(0.5, false).unwrap();
        instant.set_ratio(0.5, false).unwrap();
        let mut smooth_blocks = Vec::new();
        let mut instant_blocks = Vec::new();
        let mut position = 0;
        for block in 0..29 {
            if block == 8 {
                smoothed.set_ratio(2.0, true).unwrap();
                instant.set_ratio(2.0, true).unwrap();
            }
            if block == 11 {
                smoothed.set_ratio(0.5, false).unwrap();
                instant.set_ratio(0.5, false).unwrap();
            }
            let input = tones_rate(256, 2, &[997.0, 18_000.0], position, 48_000);
            position += 256;
            let mut a = Vec::new();
            let mut b = Vec::new();
            feed_rate(&mut smoothed, &input, 256, &mut a, 48_000);
            feed_rate(&mut instant, &input, 256, &mut b, 48_000);
            smooth_blocks.push(a);
            instant_blocks.push(b);
        }
        for (index, (a, b)) in smooth_blocks.iter().zip(&instant_blocks).enumerate() {
            if index <= 8 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: pre-widening must be bit-exact"
                );
            }
            if index >= 11 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: post-preemption must be bit-exact"
                );
            }
        }
        let preempt_smooth: Vec<f32> =
            smooth_blocks[9..11].iter().flatten().copied().collect();
        let preempt_instant: Vec<f32> =
            instant_blocks[9..11].iter().flatten().copied().collect();
        let peak = peak_difference(&preempt_smooth, &preempt_instant);
        assert!(
            peak > 1e-6,
            "{quality:?}: slew must act before preemption, peak {peak}"
        );
        let mut drain_smooth = Vec::new();
        let mut drain_instant = Vec::new();
        finish_rate(&mut smoothed, 2, &mut drain_smooth, 48_000);
        finish_rate(&mut instant, 2, &mut drain_instant, 48_000);
        assert!(
            drain_smooth == drain_instant,
            "{quality:?}: drain must reconverge bit-exactly"
        );
        let len_smooth: usize =
            smooth_blocks.iter().map(Vec::len).sum::<usize>() + drain_smooth.len();
        let len_instant: usize =
            instant_blocks.iter().map(Vec::len).sum::<usize>() + drain_instant.len();
        assert_eq!(len_smooth, len_instant, "{quality:?}: lengths diverged");
    }
}

#[test]
fn drain_starting_mid_slew_completes_with_audio() {
    // P1-5b: feed 11 blocks (ramp at block 8, slew at ranks 1..2), then drain
    // with no further input. Both modes complete within the advertised bound
    // with finite nonzero audio and equal drain lengths.
    for quality in QUALITIES {
        let mut smoothed = make_rate(48_000, 48_000, 2, 256, quality, true);
        let mut instant = make_rate(48_000, 48_000, 2, 256, quality, false);
        smoothed.set_ratio(0.5, false).unwrap();
        instant.set_ratio(0.5, false).unwrap();
        let mut position = 0;
        for block in 0..11 {
            if block == 8 {
                smoothed.set_ratio(2.0, true).unwrap();
                instant.set_ratio(2.0, true).unwrap();
            }
            let input = tones_rate(256, 2, &[997.0, 18_000.0], position, 48_000);
            position += 256;
            let mut a = Vec::new();
            let mut b = Vec::new();
            feed_rate(&mut smoothed, &input, 256, &mut a, 48_000);
            feed_rate(&mut instant, &input, 256, &mut b, 48_000);
        }
        let mut drain_smooth = Vec::new();
        let mut drain_instant = Vec::new();
        finish_rate(&mut smoothed, 2, &mut drain_smooth, 48_000);
        finish_rate(&mut instant, 2, &mut drain_instant, 48_000);
        assert!(
            !drain_smooth.is_empty() && !drain_instant.is_empty(),
            "{quality:?}: mid-slew drain must emit audio"
        );
        assert!(
            drain_smooth.iter().all(|sample| sample.is_finite())
                && drain_instant.iter().all(|sample| sample.is_finite()),
            "{quality:?}: mid-slew drain must stay finite"
        );
        for (label, drain) in [("smoothed", &drain_smooth), ("instant", &drain_instant)] {
            let rms = (drain.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>()
                / drain.len().max(1) as f64)
                .sqrt();
            assert!(
                rms > 1e-6,
                "{quality:?} {label}: mid-slew drain must be nonzero, rms {rms}"
            );
        }
        eprintln!(
            "SLEW-COVERAGE {quality:?} mid-slew drain lengths {}",
            drain_smooth.len()
        );
        assert_eq!(
            drain_smooth.len(),
            drain_instant.len(),
            "{quality:?}: mid-slew drain lengths diverged"
        );
    }
}

#[test]
fn transition_per_block_counts_are_equal() {
    // P1-5c: cutoff selection never affects timing, so every transition
    // block (9..16, ranks 1..8) emits the same produced count in both modes.
    // Checked for all blocks 0..29; any nonzero delta is a failure.
    for quality in QUALITIES {
        let mut smoothed = make_rate(48_000, 48_000, 2, 256, quality, true);
        let mut instant = make_rate(48_000, 48_000, 2, 256, quality, false);
        smoothed.set_ratio(0.5, false).unwrap();
        instant.set_ratio(0.5, false).unwrap();
        let mut position = 0;
        for block in 0..29 {
            if block == 8 {
                smoothed.set_ratio(2.0, true).unwrap();
                instant.set_ratio(2.0, true).unwrap();
            }
            let input = tones_rate(256, 2, &[997.0, 18_000.0], position, 48_000);
            position += 256;
            let mut a = Vec::new();
            let mut b = Vec::new();
            feed_rate(&mut smoothed, &input, 256, &mut a, 48_000);
            feed_rate(&mut instant, &input, 256, &mut b, 48_000);
            assert_eq!(
                a.len(),
                b.len(),
                "{quality:?} block {block}: produced counts diverged"
            );
        }
    }
}

#[test]
fn latency_and_signal_delay_ignore_smoothing() {
    // P1-5d: smoothing changes filter response only, never the clock, so
    // latency and signal delay are exactly equal with smoothing on vs off
    // for the same instance configuration, quality, and ratio.
    for quality in QUALITIES {
        for (input_rate, output_rate) in [
            (48_000, 48_000),
            (48_000, 24_000),
            (96_000, 24_000),
            (44_100, 48_000),
            (48_000, 44_100),
        ] {
            let nominal = f64::from(output_rate) / f64::from(input_rate);
            for ratio in [nominal, nominal * 0.5, nominal * 2.0] {
                let mut smoothed =
                    make_rate(input_rate, output_rate, 2, 256, quality, true);
                let mut instant =
                    make_rate(input_rate, output_rate, 2, 256, quality, false);
                smoothed.set_ratio(ratio, false).unwrap();
                instant.set_ratio(ratio, false).unwrap();
                assert_eq!(
                    smoothed.latency_samples(),
                    instant.latency_samples(),
                    "{quality:?} {input_rate}->{output_rate} ratio={ratio}: latency diverged"
                );
                assert_eq!(
                    smoothed.signal_delay_samples().to_bits(),
                    instant.signal_delay_samples().to_bits(),
                    "{quality:?} {input_rate}->{output_rate} ratio={ratio}: signal delay diverged"
                );
            }
        }
    }
    eprintln!("SLEW-COVERAGE latency/signal-delay equal with smoothing on vs off");
}
