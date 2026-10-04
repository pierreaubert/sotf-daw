//! Dense native release/channel matrix with independent oracles.
//!
//! Covers bursts, two-tone, phase, lookahead, release (+dual), rates,
//! linked/unlinked/partial channels, and threshold automation. Sample ceiling
//! bound is 0.00001 dB in every hard/wet mode. The reconstructed true-peak
//! bound is 0.1 dB when ISP protection is enabled with covering lookahead.
//! Disabled protection is still measured, but only promises a sample ceiling.
//! No production fast-math reuse in oracles.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use std::f64::consts::{PI, TAU};

const THRESHOLD_DB: f64 = -12.0;
const SAMPLE_TOLERANCE_DB: f64 = 0.000_01;
const TP_TOLERANCE_DB: f64 = 0.1;
const RATES: [u32; 4] = [44_100, 48_000, 96_000, 192_000];

#[allow(clippy::too_many_arguments)]
fn make(
    rate: u32,
    channels: usize,
    lookahead_ms: f32,
    release_ms: f32,
    dual: bool,
    link: f32,
    threshold_db: f32,
    isp: bool,
) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": threshold_db,
        "release_ms": release_ms,
        "lookahead_ms": lookahead_ms,
        "soft": false,
        "true_peak": isp,
        "isp_mode": isp,
        "dual_release": dual,
        "mix": 1.0,
        "feed_forward": false,
        "link_amount": link,
        "oversampling": 0,
    }))
    .unwrap();
    let mut plugin = LimiterPlugin::from_params(channels, params);
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn drain_all(plugin: &mut LimiterPlugin, rate: u32, channels: usize) -> Vec<f32> {
    let mut tail = Vec::new();
    let mut scratch = vec![0.0; 256 * channels];
    for _ in 0..20_000 {
        let result = plugin
            .drain(&mut scratch, &ProcessContext::new(rate, 0))
            .unwrap();
        tail.extend_from_slice(&scratch[..result.frames * channels]);
        if result.complete {
            return tail;
        }
    }
    panic!("native limiter drain did not complete");
}

fn render_full(
    plugin: &mut LimiterPlugin,
    rate: u32,
    input: &[f32],
    pattern: &[usize],
) -> Vec<f32> {
    let channels = plugin.channels();
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut position = 0;
    let mut call = 0;
    while position < frames {
        let count = pattern[call % pattern.len()].min(frames - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + count) * channels],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
        call += 1;
    }
    let tail = drain_all(plugin, rate, channels);
    output.extend_from_slice(&tail);
    assert!(output.iter().all(|s| s.is_finite()));
    output
}

fn sample_peak_db(signal: &[f32]) -> f64 {
    let peak = signal
        .iter()
        .map(|s| f64::from(*s).abs())
        .fold(0.0, f64::max);
    20.0 * peak.max(1.0e-12).log10()
}

fn reconstructed_peak_db(signal: &[f32], rate: u32) -> f64 {
    let factor = if rate < 96_000 {
        4
    } else if rate < 192_000 {
        2
    } else {
        return sample_peak_db(signal);
    };
    let kernels: [[f64; 25]; 4] = std::array::from_fn(|phase| {
        std::array::from_fn(|past| {
            let j = factor * past + phase;
            if j > 48 {
                return 0.0;
            }
            let offset = j as f64 - 24.0;
            let angle = PI * offset / factor as f64;
            let sinc = if offset == 0.0 {
                1.0
            } else {
                angle.sin() / angle
            };
            sinc * 0.5 * (1.0 - (TAU * j as f64 / 48.0).cos())
        })
    });
    let mut history = [0.0; 25];
    let mut peak: f64 = 0.0;
    for sample in signal
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0.0, 25))
    {
        history.copy_within(..24, 1);
        history[0] = f64::from(sample);
        for kernel in &kernels[..factor] {
            let value: f64 = history.iter().zip(kernel).map(|(x, h)| x * h).sum();
            peak = peak.max(value.abs());
        }
    }
    20.0 * peak.max(1.0e-12).log10()
}

fn channel_stream(signal: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    signal
        .chunks_exact(channels)
        .map(|frame| frame[channel])
        .collect()
}

fn burst_sample(pattern: usize, index: usize) -> f32 {
    match pattern {
        0 => {
            if index == 0 {
                1.2
            } else {
                0.0
            }
        }
        1 => {
            if index.is_multiple_of(2) {
                1.1
            } else {
                -1.1
            }
        }
        _ => {
            // Dense two-tone snippet inside the burst.
            let t = index as f64;
            (0.8 * (TAU * 0.11 * t).sin() + 0.6 * (TAU * 0.23 * t + 1.3).sin()) as f32
        }
    }
}

#[test]
fn burst_ceiling_holds_across_rate_link_release() {
    let mut worst_sample = f64::MIN;
    let mut worst_tp = f64::MIN;
    let mut worst_unprotected_tp = f64::MIN;
    let mut worst_case = String::new();
    let mut count = 0;
    for rate in RATES {
        for link in [0.0, 1.0] {
            for (release_ms, dual) in [(10.0, false), (200.0, true)] {
                for lookahead_ms in [0.0, 5.0] {
                    for isp in [false, true] {
                        // ISP needs covering lookahead; 5 ms covers all rates.
                        if isp && lookahead_ms < 1.0 {
                            continue;
                        }
                        for channels in [1, 2, 6] {
                            for length in [1, 3, 13, 127] {
                                for pattern in 0..3 {
                                    let frames = 4096;
                                    let mut input = vec![0.0; frames * channels];
                                    let start = 2048;
                                    for i in 0..length {
                                        let sample = burst_sample(pattern, i);
                                        for ch in 0..channels {
                                            input[(start + i) * channels + ch] = sample;
                                        }
                                    }
                                    let mut plugin = make(
                                        rate,
                                        channels,
                                        lookahead_ms,
                                        release_ms,
                                        dual,
                                        link,
                                        THRESHOLD_DB as f32,
                                        isp,
                                    );
                                    let emitted =
                                        render_full(&mut plugin, rate, &input, &[1, 127, 257]);
                                    let peak = sample_peak_db(&emitted);
                                    if peak > worst_sample {
                                        worst_sample = peak;
                                        worst_case = format!(
                                            "sample rate={rate} link={link} rel={release_ms} dual={dual} la={lookahead_ms} isp={isp} ch={channels} len={length} pat={pattern}"
                                        );
                                    }
                                    assert!(
                                        peak <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB,
                                        "sample ceiling {peak:.6} exceeds bound at {worst_case}"
                                    );
                                    // Per-channel reconstructed TP.
                                    for ch in 0..channels {
                                        let stream = channel_stream(&emitted, channels, ch);
                                        let tp = reconstructed_peak_db(&stream, rate);
                                        if isp && tp > worst_tp {
                                            worst_tp = tp;
                                            worst_case = format!(
                                                "tp rate={rate} link={link} rel={release_ms} dual={dual} la={lookahead_ms} isp={isp} ch={channels}/{ch} len={length} pat={pattern}"
                                            );
                                        }
                                        if isp {
                                            assert!(
                                                tp <= THRESHOLD_DB + TP_TOLERANCE_DB,
                                                "TP ceiling {tp:.4} exceeds bound at {worst_case}"
                                            );
                                        } else {
                                            worst_unprotected_tp = worst_unprotected_tp.max(tp);
                                        }
                                    }
                                    // Limiter must engage, not pass silence or over-attenuate.
                                    assert!(
                                        peak > THRESHOLD_DB - 1.5,
                                        "burst peak {peak:.3} too low (blanket attenuation?) at {worst_case}"
                                    );
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "burst matrix: {count} renders, worst sample {worst_sample:.6} dB, worst protected TP {worst_tp:.4} dB, unprotected TP {worst_unprotected_tp:.4} dB at {worst_case}"
    );
    assert!(worst_sample <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
    assert!(worst_tp <= THRESHOLD_DB + TP_TOLERANCE_DB);
}

#[test]
fn disabled_true_peak_protection_only_promises_the_sample_ceiling() {
    // A three-sample alternating burst is a counterexample to a true-peak
    // guarantee when both protection switches and lookahead are disabled.
    // Keep this case to prevent a future oracle from conflating the modes.
    let rate = 44_100;
    let mut input = vec![0.0; 4096];
    input[2048..2051].copy_from_slice(&[1.1, -1.1, 1.1]);
    let mut plugin = make(rate, 1, 0.0, 10.0, false, 0.0, THRESHOLD_DB as f32, false);
    let output = render_full(&mut plugin, rate, &input, &[1, 127, 257]);
    let sample = sample_peak_db(&output);
    let reconstructed = reconstructed_peak_db(&output, rate);
    assert!(sample <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
    assert!(sample > THRESHOLD_DB - 0.001);
    assert!(reconstructed > THRESHOLD_DB + 0.5,
        "counterexample must expose intersample overshoot: {reconstructed:.4} dBTP");
}

#[test]
fn two_tone_phase_ceiling_and_limiter_engages() {
    let mut worst = f64::MIN;
    let mut count = 0;
    for rate in RATES {
        // Keep tones below Nyquist at all rates.
        let pairs = [(1000.0, 1500.0), (7000.0, 11_000.0)];
        for (f1, f2) in pairs {
            if f2 >= f64::from(rate) * 0.45 {
                continue;
            }
            for phase in [0.0, std::f64::consts::FRAC_PI_2, PI] {
                for link in [0.0, 1.0] {
                    for (release_ms, dual) in [(50.0, false), (50.0, true)] {
                        for channels in [1, 2] {
                            let frames = (f64::from(rate) * 0.25) as usize;
                            let mut input = vec![0.0; frames * channels];
                            for frame in 0..frames {
                                let t = frame as f64 / f64::from(rate);
                                let sample = (0.9 * (TAU * f1 * t).sin()
                                    + 0.9 * (TAU * f2 * t + phase).sin())
                                    as f32;
                                for ch in 0..channels {
                                    input[frame * channels + ch] = sample;
                                }
                            }
                            let mut plugin = make(
                                rate,
                                channels,
                                5.0,
                                release_ms,
                                dual,
                                link,
                                THRESHOLD_DB as f32,
                                false,
                            );
                            let emitted =
                                render_full(&mut plugin, rate, &input, &[257, 63, 1]);
                            let peak = sample_peak_db(&emitted);
                            worst = worst.max(peak);
                            assert!(
                                peak <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB,
                                "two-tone peak {peak:.6} exceeds bound (rate={rate} f={f1}/{f2} phase={phase:.2} link={link} dual={dual})"
                            );
                            assert!(
                                peak > THRESHOLD_DB - 1.5,
                                "two-tone peak {peak:.3} too low (rate={rate} phase={phase:.2})"
                            );
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    eprintln!("two-tone matrix: {count} renders, worst peak {worst:.6} dB");
}

#[test]
fn single_release_dc_step_matches_one_pole_oracle() {
    // 0 lookahead, hard knee, mono: envelope decay is exactly the documented
    // one-pole release. Oracle uses independent f64 arithmetic.
    let mut worst: f64 = 0.0;
    for rate in [48_000, 96_000] {
        for release_ms in [10.0, 50.0, 200.0] {
            let mut plugin = make(rate, 1, 0.0, release_ms, false, 1.0, -12.0, false);
            let loud_frames = (f64::from(rate) * 0.1) as usize;
            let quiet_frames = (f64::from(rate) * 0.5) as usize;
            let mut input = vec![1.0; loud_frames];
            input.extend(std::iter::repeat_n(0.125, quiet_frames));
            let mut output = input.clone();
            // Single block for exact sample-index alignment (no drain needed at 0 delay).
            plugin
                .process_in_place(&mut output, &ProcessContext::new(rate, input.len()))
                .unwrap();
            let ceiling = 10.0f64.powf(-12.0 / 20.0);
            let rc = (-1.0 / (f64::from(release_ms) * 0.001 * f64::from(rate))).exp();
            // Overload envelope in dB.
            let mut envelope = 20.0 * (1.0 / ceiling).log10();
            // Check quiet tail from 5 ms after the step (smoother already settled;
            // threshold static so no smoother transient).
            let settle = (f64::from(rate) * 0.005) as usize;
            for (i, &actual) in output[loud_frames + settle..].iter().enumerate() {
                envelope *= rc;
                let expected = 0.125 * 10.0f64.powf(-envelope / 20.0);
                let actual_db = 20.0 * f64::from(actual).abs().max(1.0e-12).log10();
                let expected_db = 20.0 * expected.max(1.0e-12).log10();
                let error = (actual_db - expected_db).abs();
                // Skip the deep tail where both are near the -18 dB floor; the
                // early/mid decay is the discriminating region.
                if expected_db > -17.5 {
                    worst = worst.max(error);
                    assert!(
                        error <= 0.15,
                        "release oracle error {error:.4} dB at tail {i} (rate={rate} rel={release_ms})"
                    );
                }
            }
            // Loud segment holds the ceiling.
            let loud_peak = sample_peak_db(&output[..loud_frames]);
            assert!((loud_peak + 12.0).abs() < 0.05, "loud {loud_peak:.4}");
        }
    }
    eprintln!("worst single-release oracle error: {worst:.5} dB");
}

#[test]
fn dual_release_recovers_monotonically_without_ceiling_violation() {
    for rate in [48_000, 192_000] {
        let mut plugin = make(rate, 1, 0.0, 50.0, true, 1.0, -12.0, false);
        let loud_frames = 4800;
        let quiet_frames = (f64::from(rate) * 0.6) as usize;
        let mut input = vec![1.0; loud_frames];
        input.extend(std::iter::repeat_n(0.125, quiet_frames));
        let mut output = input.clone();
        plugin
            .process_in_place(&mut output, &ProcessContext::new(rate, input.len()))
            .unwrap();
        assert!(sample_peak_db(&output) <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
        // Recovery is monotonic non-decreasing toward the quiet level.
        let tail = &output[loud_frames..];
        for pair in tail.windows(2).step_by(97) {
            assert!(
                pair[1] >= pair[0] - 1.0e-6,
                "dual release non-monotonic at rate {rate}"
            );
        }
        // Eventually recovers toward the unattenuated quiet level (-18.06 dB).
        // Gain never exceeds unity, so output cannot pass the input level.
        let settled = tail[tail.len() - 256..]
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        let settled_db = 20.0 * f64::from(settled).max(1.0e-12).log10();
        assert!(
            settled_db > -20.5 && settled_db <= -18.0 + 0.15,
            "dual settled {settled_db:.3} dB, expected recovery toward -18.06 without overshoot"
        );
    }
}

#[test]
fn threshold_automation_with_link_modes_holds_ceiling() {
    for link in [0.0, 1.0] {
        let rate = 48_000;
        let channels = 2;
        let mut plugin = make(rate, channels, 5.0, 50.0, false, link, -6.0, false);
        let frames = 24_000;
        // Left loud, right quiet to expose link coupling.
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let t = frame as f64 / f64::from(rate);
                let loud = (0.9 * (TAU * 1000.0 * t).sin()) as f32;
                [loud, 0.1]
            })
            .collect();
        let mut output = input.clone();
        let mut position = 0;
        while position < frames {
            if position == frames / 2 {
                plugin
                    .parametric_set_parameter(
                        ParameterId::from("threshold"),
                        ParameterValue::Float(-18.0),
                    )
                    .unwrap();
            }
            // Deliver the threshold event at its exact sample position.
            let next_boundary = if position < frames / 2 {
                frames / 2
            } else {
                frames
            };
            let count = 257.min(next_boundary - position);
            plugin
                .process_in_place(
                    &mut output[position * channels..(position + count) * channels],
                    &ProcessContext::new(rate, count),
                )
                .unwrap();
            position += count;
        }
        let tail = drain_all(&mut plugin, rate, channels);
        output.extend_from_slice(&tail);
        // Overall peak respects the looser early ceiling.
        assert!(sample_peak_db(&output) <= -6.0 + SAMPLE_TOLERANCE_DB + 0.5);
        // Late program (after smoother + latency settle) respects the tighter ceiling.
        let latency = 240; // 5 ms at 48 kHz.
        // The threshold smoother has a 5 ms time constant. Its 12 dB step
        // needs ln(12/0.3) time constants to enter the unchanged 0.3 dB
        // bound; two time constants are not a settled automation tail.
        let settle_frames = (f64::from(rate) * 0.005 * (12.0_f64 / 0.3).ln()).ceil() as usize;
        let late_start = (frames / 2 + latency + settle_frames) * channels;
        let late = &output[late_start..frames * channels];
        let late_peak = sample_peak_db(late);
        assert!(
            late_peak <= -18.0 + 0.3,
            "link={link} late peak {late_peak:.3} exceeds settled ceiling"
        );
        if link == 0.0 {
            // Unlinked quiet channel passes near-untouched in the early half.
            let early_right: Vec<f32> = output[latency * channels..frames / 2 * channels]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|f| f[1])
                .collect();
            let mean = early_right.iter().sum::<f32>() / early_right.len() as f32;
            assert!(
                (mean - 0.1).abs() < 0.02,
                "unlinked quiet channel mean {mean:.4}, expected ~0.1"
            );
        } else {
            // Fully linked: loud envelope attenuates the quiet channel on average.
            // (Instantaneous peaks still hit 0.1 at left zero-crossings.)
            let early_right: Vec<f32> = output[latency * channels..frames / 2 * channels]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|f| f[1].abs())
                .collect();
            let mean = early_right.iter().sum::<f32>() / early_right.len() as f32;
            assert!(
                mean < 0.09,
                "linked quiet channel mean {mean:.4} shows no coupling"
            );
        }
    }
}

#[test]
fn thirty_two_channel_link_stress_holds_ceiling() {
    for link in [0.0, 1.0] {
        let rate = 48_000;
        let channels = 32;
        let mut plugin = make(rate, channels, 5.0, 50.0, true, link, -12.0, false);
        let frames = 4096;
        let mut input = vec![0.0; frames * channels];
        for frame in 1000..1127 {
            let sample = if frame % 2 == 0 { 1.1 } else { -1.1 };
            for ch in 0..channels {
                input[frame * channels + ch] = sample;
            }
        }
        let emitted = render_full(&mut plugin, rate, &input, &[1, 300, 17]);
        assert!(sample_peak_db(&emitted) <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
        if link == 1.0 {
            // Fully linked identical input stays identical across channels.
            for frame in 0..frames {
                let first = emitted[frame * channels];
                for ch in 1..channels {
                    assert_eq!(
                        emitted[frame * channels + ch],
                        first,
                        "linked 32ch diverged at frame {frame}"
                    );
                }
            }
        }
    }
}

#[test]
fn old_preset_defaults_and_save_reload_preserve_audio() {
    // Old JSON missing newer fields must default exactly and render identically
    // to an explicit-default construction; save/reload must be bit-identical.
    let rate = 48_000;
    let channels = 2;
    let old: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": -6.0,
        "release_ms": 50.0,
        "lookahead_ms": 5.0,
        "soft": false,
    }))
    .unwrap();
    assert!(!old.true_peak);
    assert!(!old.isp_mode);
    assert!(!old.dual_release);
    assert_eq!(old.mix, 1.0);
    assert!(!old.feed_forward);
    assert_eq!(old.link_amount, 1.0);
    assert_eq!(old.oversampling, 0);
    let explicit: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": -6.0,
        "release_ms": 50.0,
        "lookahead_ms": 5.0,
        "soft": false,
        "true_peak": false,
        "isp_mode": false,
        "dual_release": false,
        "mix": 1.0,
        "feed_forward": false,
        "link_amount": 1.0,
        "oversampling": 0,
    }))
    .unwrap();
    let input: Vec<f32> = (0..4096)
        .flat_map(|frame| {
            let t = frame as f64 / f64::from(rate);
            [
                (0.7 * (TAU * 1200.0 * t).sin()) as f32,
                (0.7 * (TAU * 800.0 * t + 0.9).sin()) as f32,
            ]
        })
        .collect();
    let mut a = LimiterPlugin::from_params(channels, old);
    a.initialize(f64::from(rate)).unwrap();
    let mut b = LimiterPlugin::from_params(channels, explicit.clone());
    b.initialize(f64::from(rate)).unwrap();
    let out_a = render_full(&mut a, rate, &input, &[257, 63]);
    let out_b = render_full(&mut b, rate, &input, &[257, 63]);
    assert_eq!(out_a, out_b);
    // Save/reload round-trip.
    let saved = serde_json::to_value(&explicit).unwrap();
    let reloaded: LimiterPluginParams = serde_json::from_value(saved).unwrap();
    let mut c = LimiterPlugin::from_params(channels, reloaded);
    c.initialize(f64::from(rate)).unwrap();
    let out_c = render_full(&mut c, rate, &input, &[257, 63]);
    assert_eq!(out_b, out_c);
    // Malformed state is rejected before construction (transactional).
    assert!(serde_json::from_value::<LimiterPluginParams>(serde_json::json!({
        "threshold_db": "loud",
        "oversampling": 7,
    }))
    .is_err());
}

#[test]
fn release_lookahead_edges_and_eof_tail() {
    // Maximum release + maximum lookahead + ISP at the top rate.
    let rate = 192_000;
    let mut plugin = make(rate, 2, 20.0, 1000.0, true, 0.5, -12.0, true);
    let latency = plugin.latency_samples();
    assert!(latency > 3800 && latency < 4000, "latency {latency}");
    let frames = 8000;
    let input = vec![0.9; frames * 2];
    let emitted = render_full(&mut plugin, rate, &input, &[256, 1, 100]);
    assert!(sample_peak_db(&emitted) <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
    // Tail length equals the finite drain bound (lookahead + ISP delay).
    assert_eq!(emitted.len(), (frames + latency) * 2);
    // Zero-lookahead native path at 192 kHz allows ISP (zero detector delay).
    let mut plugin = make(rate, 1, 0.0, 10.0, false, 1.0, -12.0, true);
    assert_eq!(plugin.latency_samples(), 0);
    let input = vec![1.2, -1.2, 0.5, -0.5];
    let emitted = render_full(&mut plugin, rate, &input, &[1]);
    assert!(sample_peak_db(&emitted) <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
    // Partial-link law is deterministic across block partitions.
    for pattern in [&[1][..], &[127][..], &[257, 3][..]] {
        let mut a = make(48_000, 2, 5.0, 50.0, false, 0.37, -12.0, false);
        let mut b = make(48_000, 2, 5.0, 50.0, false, 0.37, -12.0, false);
        let input: Vec<f32> = (0..2048)
            .flat_map(|frame| {
                let t = frame as f64 / 48_000.0;
                [
                    (0.8 * (TAU * 900.0 * t).sin()) as f32,
                    (0.8 * (TAU * 1400.0 * t + 0.4).sin()) as f32,
                ]
            })
            .collect();
        let out_a = render_full(&mut a, 48_000, &input, pattern);
        let out_b = render_full(&mut b, 48_000, &input, &[2048]);
        assert_eq!(out_a, out_b, "partial-link partition {pattern:?}");
    }
}
