//! Independent public oversampling ceiling, alias, delay, and waveform oracles.
//!
//! The reconstruction oracle is a finite f64 Hann-windowed sinc, not the
//! production detector. Its 0.1 dB bound is a reconstruction test, not a claim
//! about every possible continuous-time reconstruction filter.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::LimiterPlugin;
use std::f64::consts::{PI, TAU};

const THRESHOLD_DB: f64 = -12.0;
const SAMPLE_TOLERANCE_DB: f64 = 0.000_01;
const RECONSTRUCTION_TOLERANCE_DB: f64 = 0.1;
const RATES: [u32; 4] = [44_100, 48_000, 96_000, 192_000];

fn set(plugin: &mut LimiterPlugin, key: &str, value: ParameterValue) {
    plugin
        .parametric_set_parameter(ParameterId::from(key), value)
        .unwrap();
}

fn make(rate: u32, channels: usize, choice: i32, isp: bool, lookahead_ms: f32) -> LimiterPlugin {
    let mut plugin = LimiterPlugin::new(channels, THRESHOLD_DB as f32, 10.0, lookahead_ms, false);
    set(&mut plugin, "oversampling", ParameterValue::Int(choice));
    set(&mut plugin, "true_peak", ParameterValue::Bool(isp));
    set(&mut plugin, "isp_mode", ParameterValue::Bool(isp));
    set(&mut plugin, "feed_forward", ParameterValue::Bool(true));
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn render(plugin: &mut LimiterPlugin, rate: u32, input: &[f32], pattern: &[usize]) -> Vec<f32> {
    plugin.reset();
    let channels = plugin.channels();
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut position = 0;
    let mut call = 0;
    while position < frames {
        let count = pattern[call % pattern.len()].min(frames - position);
        assert!(count > 0);
        let mut context = ProcessContext::new(rate, count);
        context.transport.sample_position = position as u64;
        assert_eq!(
            plugin
                .process_in_place(
                    &mut output[position * channels..(position + count) * channels],
                    &context,
                )
                .unwrap(),
            count
        );
        position += count;
        call += 1;
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1.0e-30).log10()
}

fn sample_peak(signal: &[f32]) -> f64 {
    signal
        .iter()
        .map(|&sample| f64::from(sample).abs())
        .fold(0.0, f64::max)
}

fn reconstructed_peak(signal: &[f32], rate: u32) -> f64 {
    let factor = if rate < 96_000 {
        4
    } else if rate < 192_000 {
        2
    } else {
        return sample_peak(signal);
    };
    // Exactly zero leading/trailing samples can be removed: the initial history
    // is zero and we explicitly append the complete 25-sample FIR support.
    let Some(first) = signal.iter().position(|&sample| sample != 0.0) else {
        return 0.0;
    };
    let last = signal.iter().rposition(|&sample| sample != 0.0).unwrap();
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
    let mut peak = sample_peak(signal);
    for sample in signal[first..=last]
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0.0, 25))
    {
        history.copy_within(..24, 1);
        history[0] = f64::from(sample);
        for kernel in &kernels[..factor] {
            let value = history.iter().zip(kernel).map(|(x, h)| x * h).sum::<f64>();
            peak = peak.max(value.abs());
        }
    }
    peak
}

fn burst(pattern: usize, index: usize) -> f32 {
    match pattern {
        0 => 1.0,
        1 => {
            if index.is_multiple_of(2) {
                1.0
            } else {
                -1.0
            }
        }
        2 => (TAU * 0.23 * index as f64 + 0.5).sin() as f32,
        3 => {
            if index.is_multiple_of(3) {
                1.0
            } else {
                -0.3
            }
        }
        4 => ((index as f64 * 13.67).sin() * 0.7 + (index as f64 * 3.29).cos() * 0.7) as f32,
        _ => unreachable!(),
    }
}

#[test]
fn independent_reconstruction_detects_peaks_between_samples() {
    let samples = [1.0, 1.0, -1.0, -1.0];
    assert_eq!(sample_peak(&samples), 1.0);
    for rate in [44_100, 48_000, 96_000] {
        assert!(reconstructed_peak(&samples, rate) > 1.1);
        let mut padded = vec![0.0; 31];
        padded.extend_from_slice(&samples);
        padded.resize(90, 0.0);
        assert_eq!(
            reconstructed_peak(&samples, rate),
            reconstructed_peak(&padded, rate)
        );
    }
    assert_eq!(reconstructed_peak(&samples, 192_000), 1.0);
}

#[test]
fn all_840_bursts_respect_the_final_sample_and_isp_ceilings() {
    let mut cases = 0;
    for rate in RATES {
        for isp in [false, true] {
            for choice in 0..=2 {
                let mut plugin = make(rate, 1, choice, isp, if isp { 0.25 } else { 0.0 });
                let mut worst_sample = -600.0_f64;
                let mut worst_reconstructed = -600.0_f64;
                for length in [1, 2, 3, 5, 13, 31, 127] {
                    for pattern in 0..5 {
                        let mut input = vec![0.0; 8192];
                        for (index, sample) in input[2048..2048 + length].iter_mut().enumerate() {
                            *sample = burst(pattern, index);
                        }
                        let output = render(&mut plugin, rate, &input, &[257]);
                        let sample_db = db(sample_peak(&output));
                        let reconstructed_db = db(reconstructed_peak(&output, rate));
                        assert!(
                            sample_db <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB,
                            "final sample ceiling: rate={rate}, ISP={isp}, choice={choice}, length={length}, pattern={pattern}, peak={sample_db} dBFS"
                        );
                        if isp {
                            assert!(
                                reconstructed_db <= THRESHOLD_DB + RECONSTRUCTION_TOLERANCE_DB,
                                "final reconstructed ceiling: rate={rate}, choice={choice}, length={length}, pattern={pattern}, peak={reconstructed_db} dBTP"
                            );
                        }
                        worst_sample = worst_sample.max(sample_db);
                        worst_reconstructed = worst_reconstructed.max(reconstructed_db);
                        cases += 1;
                    }
                }
                eprintln!(
                    "BURST rate={rate} ISP={isp} factor={} sample={worst_sample:.6} dBFS reconstructed={worst_reconstructed:.6} dBTP",
                    1 << choice
                );
            }
        }
    }
    assert_eq!(cases, 840);
}

fn amplitude(signal: &[f32], frequency: f64, rate: u32) -> f64 {
    let (mut real, mut imaginary) = (0.0, 0.0);
    for (index, &sample) in signal.iter().enumerate() {
        let angle = TAU * frequency * index as f64 / f64::from(rate);
        real += f64::from(sample) * angle.cos();
        imaginary += f64::from(sample) * angle.sin();
    }
    2.0 * real.hypot(imaginary) / signal.len() as f64
}

#[test]
fn coherent_alias_measurements_preserve_specific_improvements_without_factor_ordering() {
    for isp in [false, true] {
        for frequency in [7000.0_f64, 11_000.0, 17_000.0, 21_000.0] {
            let mut input: Vec<_> = (0..96_000)
                .map(|index| {
                    (0.9 * (TAU * frequency * index as f64 / 48_000.0 + 0.37).sin()) as f32
                })
                .collect();
            input.resize(input.len() + 8192, 0.0);
            let alias = ((3.0 * frequency + 24_000.0).rem_euclid(48_000.0) - 24_000.0).abs();
            let mut spurs = [0.0; 3];
            for (choice, spur) in spurs.iter_mut().enumerate() {
                let mut plugin = make(48_000, 1, choice as i32, isp, if isp { 0.25 } else { 0.0 });
                let output = render(&mut plugin, 48_000, &input, &[257]);
                // One full second contains integer cycles at both frequencies;
                // the first second allows the release/envelope to settle.
                let steady = &output[48_000..96_000];
                let fundamental = amplitude(steady, frequency, 48_000);
                *spur = db(amplitude(steady, alias, 48_000) / fundamental);
                assert!(
                    (db(fundamental) - THRESHOLD_DB).abs() < 0.25,
                    "alias suppression must retain level: ISP={isp}, frequency={frequency}, choice={choice}, fundamental={} dBFS",
                    db(fundamental)
                );
                assert!(db(sample_peak(&output)) <= THRESHOLD_DB + SAMPLE_TOLERANCE_DB);
                if isp {
                    assert!(
                        db(reconstructed_peak(&output, 48_000))
                            <= THRESHOLD_DB + RECONSTRUCTION_TOLERANCE_DB
                    );
                }
                eprintln!(
                    "TONE ISP={isp} frequency={frequency} factor={} fundamental={:.6} dBFS alias={spur:.3} dBc",
                    1 << choice,
                    db(fundamental)
                );
            }
            // The isolated public prototype established >13/29 dB at 11 kHz
            // and >37/26 dB at 17 kHz for 2x/4x. Retain conservative, meaningful
            // reductions at these frequencies only. ISP and other frequencies
            // have different tradeoffs; 4x need not improve on 2x.
            if !isp && [11_000.0, 17_000.0].contains(&frequency) {
                assert!(
                    spurs[0] - spurs[1] > 10.0,
                    "2x alias reduction: {frequency} Hz, {spurs:?}"
                );
                assert!(
                    spurs[0] - spurs[2] > 20.0,
                    "4x alias reduction: {frequency} Hz, {spurs:?}"
                );
            }
        }
    }
}

#[test]
fn public_waveforms_are_bit_identical_across_partitions_and_reset() {
    let patterns: &[&[usize]] = &[&[1], &[127], &[257], &[1, 255, 17, 9217, 3, 511]];
    for rate in RATES {
        for channels in [1, 2, 6] {
            for choice in [1, 2] {
                for isp in [false, true] {
                    let mut plugin = make(rate, channels, choice, isp, 0.37);
                    let mut input: Vec<_> = (0..10_003 * channels)
                        .map(|index| ((index as f64 * 0.177).sin() * 0.73) as f32)
                        .collect();
                    input.resize(input.len() + 2048 * channels, 0.0);
                    let reference = render(&mut plugin, rate, &input, &[input.len() / channels]);
                    for pattern in patterns {
                        let output = render(&mut plugin, rate, &input, pattern);
                        assert!(
                            reference == output,
                            "partition/reset: rate={rate}, channels={channels}, choice={choice}, ISP={isp}, pattern={pattern:?}, first difference={:?}",
                            reference.iter().zip(&output).position(|(a, b)| a != b)
                        );
                    }
                }
            }
        }
    }
}

fn independent_delay(rate: u32, choice: i32, isp: bool, lookahead_ms: f32) -> usize {
    let lookahead = (f64::from(rate) * f64::from(lookahead_ms) / 1000.0).floor() as usize;
    let detector = if rate < 96_000 {
        6
    } else if rate < 192_000 {
        12
    } else {
        0
    };
    if choice == 0 {
        lookahead + if isp { 3 * detector } else { 0 }
    } else {
        // 256 frames of explicit converter buffering + two 128-frame FIR
        // group delays; high-rate core lookahead is exactly F*lookahead.
        512 + lookahead + if isp { 4 * detector } else { 0 }
    }
}

#[test]
fn first_and_late_low_level_impulses_peak_at_the_declared_physical_delay() {
    for rate in RATES {
        for channels in [1, 2, 6] {
            for choice in 0..=2 {
                for (isp, lookahead) in [(false, 0.0), (false, 0.37), (true, 0.37)] {
                    let mut plugin = make(rate, channels, choice, isp, lookahead);
                    let delay = independent_delay(rate, choice, isp, lookahead);
                    assert_eq!(plugin.latency_samples(), delay);
                    let mut input = vec![0.0; 8192 * channels];
                    for channel in 0..channels {
                        input[channel] = 0.001 * (channel + 1) as f32;
                        input[2047 * channels + channel] = -0.001 * (channel + 1) as f32;
                    }
                    for pattern in [&[1][..], &[127, 257, 3][..]] {
                        let output = render(&mut plugin, rate, &input, pattern);
                        for origin in [0, 2047] {
                            for channel in 0..channels {
                                let peak = (origin..origin + 1500)
                                    .max_by(|&a, &b| {
                                        output[a * channels + channel]
                                            .abs()
                                            .total_cmp(&output[b * channels + channel].abs())
                                    })
                                    .unwrap();
                                assert_eq!(
                                    peak,
                                    origin + delay,
                                    "impulse: rate={rate}, channels={channels}, choice={choice}, ISP={isp}, lookahead={lookahead}, origin={origin}, channel={channel}"
                                );
                                assert!(
                                    output[peak * channels + channel].abs()
                                        > 0.0005 * (channel + 1) as f32
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn dry_and_half_wet_outputs_use_one_exactly_aligned_final_mix() {
    for rate in RATES {
        for channels in [1, 2, 6] {
            for choice in [1, 2] {
                let mut plugin = make(rate, channels, choice, false, 0.37);
                let delay = independent_delay(rate, choice, false, 0.37);
                let mut input: Vec<_> = (0..4099 * channels)
                    .map(|index| ((index * 17 % 101) as f32 - 50.0) / 64.0)
                    .collect();
                input.resize(input.len() + (delay + 512) * channels, 0.0);
                let wet = render(&mut plugin, rate, &input, &[257]);
                for mix in [0.0, 0.5] {
                    set(&mut plugin, "mix", ParameterValue::Float(mix));
                    let output = render(&mut plugin, rate, &input, &[1, 127, 4096, 3]);
                    for (index, (&actual, &wet)) in output.iter().zip(&wet).enumerate() {
                        let dry = index
                            .checked_sub(delay * channels)
                            .map_or(0.0, |index| input[index]);
                        let expected = f64::from(dry) * (1.0 - f64::from(mix))
                            + f64::from(wet) * f64::from(mix);
                        if mix == 0.0 {
                            assert_eq!(actual, dry, "dry delay at sample {index}");
                        } else {
                            assert!(
                                (f64::from(actual) - expected).abs() <= 1.0e-7,
                                "single final blend: rate={rate}, channels={channels}, choice={choice}, index={index}, actual={actual}, expected={expected}"
                            );
                        }
                    }
                }
            }
        }
    }
}
