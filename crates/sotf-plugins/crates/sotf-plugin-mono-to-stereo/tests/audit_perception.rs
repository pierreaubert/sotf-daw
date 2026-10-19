//! Independent mono-fold, correlation, localization and width-clock checks.

// Rust guideline compliant 2026-02-21
use rustfft::{FftPlanner, num_complex::Complex64};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_mono_to_stereo::{MonoToStereoPlugin, MonoToStereoPluginParams};

fn plugin(rate: u32, width: f32, haas: f32, shaped: bool) -> MonoToStereoPlugin {
    let mut plugin = MonoToStereoPlugin::try_from_params(
        1,
        MonoToStereoPluginParams {
            stereo_width: width,
            haas_delay_ms: haas,
            freq_dependent: shaped,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn transfer(rate: u32, width: f64, shaped: bool, haas: usize, omega: f64) -> Complex64 {
    let z = Complex64::from_polar(1.0, -omega);
    let mut response = Complex64::new(1.0, 0.0);
    if width != 0.0 {
        let radius = 0.9999 + width * (if shaped { 0.998 } else { 0.68 } - 0.9999);
        for index in 0..3 {
            let frequency = 300.0 * (2000.0_f64 / 300.0).powf(index as f64 / 2.0);
            let a1 = -2.0 * radius * (std::f64::consts::TAU * frequency / f64::from(rate)).cos();
            let a2 = radius * radius;
            response *= (a2 + a1 * z + z * z) / (1.0 + a1 * z + a2 * z * z);
        }
        let frequency = if shaped {
            4000.0_f64.min(f64::from(rate) * 0.45)
        } else {
            300.0
        };
        let t = (std::f64::consts::PI * frequency / f64::from(rate)).tan();
        let target = ((1.0 - t) / (1.0 + t)).clamp(-0.98, 0.98);
        let coefficient = -0.9999 + width * (target + 0.9999);
        response *= (z - coefficient) / (1.0 - coefficient * z);
    }
    response * z.powu(haas as u32)
}

#[test]
fn mono_fold_correlation_and_first_transient_match_independent_allpass_delay_equations() {
    const N: usize = 32768;
    let fft = FftPlanner::new().plan_fft_forward(N);
    for rate in [44_100, 48_000, 96_000] {
        for width in [0.0, 0.25, 0.75, 1.0] {
            for shaped in [false, true] {
                for haas_ms in [0.0, 5.0] {
                    let delay = (haas_ms * rate as f32 / 1000.0).round() as usize;
                    let mut plugin = plugin(rate, width, haas_ms, shaped);
                    assert_eq!(plugin.latency_samples(), 0);
                    let mut input = vec![0.0; N];
                    input[0] = 1.0;
                    let mut output = vec![0.0; N * 2];
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(rate, N))
                        .unwrap();
                    assert_eq!(
                        output[0], 1.0,
                        "first transient stays in the direct left channel"
                    );
                    assert!(
                        output
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .skip(1)
                            .all(|x| x[0] == 0.0)
                    );
                    assert!(
                        output
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .take(delay)
                            .all(|x| x[1] == 0.0),
                        "no right pre-echo"
                    );
                    assert!(
                        output[delay * 2 + 1].abs() > 1e-6,
                        "right onset must include the requested Haas offset"
                    );
                    let right: Vec<_> = output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|x| f64::from(x[1]))
                        .collect();
                    let right_energy = right.iter().map(|x| x * x).sum::<f64>();
                    assert!(
                        (right_energy - 1.0).abs() < 0.02,
                        "allpass energy width={width} fs={rate}"
                    );
                    // For a complete impulse response, normalized zero-lag
                    // correlation is h_R[0] / ||h_R||, not a promise of zero.
                    let correlation = right[0] / right_energy.sqrt();
                    let fold_energy = output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|x| ((f64::from(x[0]) + f64::from(x[1])) * 0.5).powi(2))
                        .sum::<f64>();
                    let radius =
                        0.9999 + f64::from(width) * (if shaped { 0.998 } else { 0.68 } - 0.9999);
                    let frequency = if shaped {
                        4000.0_f64.min(f64::from(rate) * 0.45)
                    } else {
                        300.0
                    };
                    let t = (std::f64::consts::PI * frequency / f64::from(rate)).tan();
                    let coefficient = -0.9999
                        + f64::from(width) * (((1.0 - t) / (1.0 + t)).clamp(-0.98, 0.98) + 0.9999);
                    // Independent feedthrough prediction for a unit-energy
                    // allpass impulse: three second-order numerator constants
                    // followed by the first-order numerator constant.
                    let predicted_correlation = if delay > 0 {
                        0.0
                    } else if width == 0.0 {
                        1.0
                    } else {
                        -coefficient * radius.powi(6)
                    };
                    assert!((correlation - predicted_correlation).abs() < 0.02);
                    assert!((fold_energy - (1.0 + predicted_correlation) * 0.5).abs() < 0.01);
                    let mut spectrum: Vec<_> =
                        right.into_iter().map(|x| Complex64::new(x, 0.0)).collect();
                    fft.process(&mut spectrum);
                    for frequency in [80.0, 100.0, 300.0, 775.0, 2000.0, 5000.0, 12000.0] {
                        let bin = (frequency * N as f64 / f64::from(rate)).round() as usize;
                        let omega = std::f64::consts::TAU * bin as f64 / N as f64;
                        let expected = transfer(rate, f64::from(width), shaped, delay, omega);
                        let error = (spectrum[bin] - expected).norm();
                        assert!(
                            error < 0.02,
                            "fs={rate} width={width} shaped={shaped} delay={delay} f={frequency}: complex error={error}"
                        );
                        let fold = (Complex64::new(1.0, 0.0) + spectrum[bin]) * 0.5;
                        let expected_fold = (Complex64::new(1.0, 0.0) + expected) * 0.5;
                        assert!((fold - expected_fold).norm() < 0.01);
                        if width == 0.0 && delay > 0 && frequency == 100.0 {
                            assert!(
                                fold.norm() < 0.03,
                                "5 ms Haas produces a mono notch near 100 Hz even at zero width"
                            );
                        }
                        assert!(
                            (spectrum[bin].re - expected.re).abs() < 0.02,
                            "tone correlation follows cos(phase)"
                        );
                    }
                }
            }
        }
    }
}

fn automated(rate: u32, haas: f32, shaped: bool, parts: &[usize], empty: bool) -> Vec<f32> {
    let mut plugin = plugin(rate, 1.0, haas, shaped);
    let events = [
        (511, 0.0),
        (17001, 1.0),
        (24017, 0.0),
        (41003, 0.75),
        (45007, 0.25),
    ];
    let frames = 52000;
    let input: Vec<_> = (0..frames)
        .map(|i| (i as f32 * 0.071).sin() * 0.1 + (i as f32 * 0.019).cos() * 0.03)
        .collect();
    let mut output = vec![0.0; frames * 2];
    let mut frame = 0;
    let mut block = 0;
    while frame < frames {
        for &(at, width) in &events {
            if frame == at {
                plugin
                    .set_parameter(
                        ParameterId::from("stereo_width"),
                        ParameterValue::Float(width),
                    )
                    .unwrap();
                if empty {
                    plugin
                        .process(&[], &mut [], &ProcessContext::new(rate, 0))
                        .unwrap();
                }
            }
        }
        let next = events
            .iter()
            .map(|&(at, _)| at)
            .find(|&at| at > frame)
            .unwrap_or(frames);
        let n = parts[block % parts.len()].min(next - frame);
        plugin
            .process(
                &input[frame..frame + n],
                &mut output[frame * 2..(frame + n) * 2],
                &ProcessContext::new(rate, n),
            )
            .unwrap();
        frame += n;
        block += 1;
    }
    output
}

#[test]
fn width_zero_reentry_and_retargeting_follow_the_absolute_sample_clock() {
    for rate in [44_100, 48_000, 96_000] {
        for haas in [0.0, 5.0] {
            for shaped in [false, true] {
                let reference = automated(rate, haas, shaped, &[1], false);
                assert!(reference.iter().all(|x| x.is_finite() && x.abs() < 1.0));
                for parts in [&[8192][..], &[7, 31, 257][..]] {
                    let output = automated(rate, haas, shaped, parts, false);
                    let error = reference
                        .iter()
                        .zip(&output)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        output == reference,
                        "fs={rate} haas={haas} shaped={shaped} max error={error}"
                    );
                }
                assert!(
                    automated(rate, haas, shaped, &[1], true) == reference,
                    "zero-frame callbacks must not alter priming or smoothing"
                );
            }
        }
    }
}

#[test]
fn wrong_prepared_rate_is_rejected_before_audio_or_state_changes() {
    let mut tested = plugin(48_000, 1.0, 5.0, true);
    let mut reference = plugin(48_000, 1.0, 5.0, true);
    let input = [0.1; 257];
    let mut output = [73.0; 514];
    assert!(
        tested
            .process(&input, &mut output, &ProcessContext::new(96_000, 257))
            .is_err()
    );
    assert_eq!(output, [73.0; 514]);
    let mut expected = [0.0; 514];
    tested
        .process(&input, &mut output, &ProcessContext::new(48_000, 257))
        .unwrap();
    reference
        .process(&input, &mut expected, &ProcessContext::new(48_000, 257))
        .unwrap();
    assert_eq!(output, expected);
}

#[test]
fn enabling_haas_after_identity_uses_recent_source_history() {
    for rate in [44_100, 48_000, 96_000] {
        let mut plugin = plugin(rate, 0.0, 0.0, true);
        let delay = (rate as f32 * 0.005).round() as usize;
        let frames = 2 * delay + 17;
        let input: Vec<_> = (0..frames)
            .map(|i| i as f32 / frames as f32 * 0.2)
            .collect();
        let mut output = vec![0.0; frames * 2];
        plugin
            .process(&input, &mut output, &ProcessContext::new(rate, frames))
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from("haas_delay_ms"),
                ParameterValue::Float(5.0),
            )
            .unwrap();
        let mut continued = vec![0.0; delay * 2];
        plugin
            .process(
                &vec![-0.1; delay],
                &mut continued,
                &ProcessContext::new(rate, delay),
            )
            .unwrap();
        for i in 0..delay {
            assert_eq!(continued[i * 2], -0.1);
            assert_eq!(
                continued[i * 2 + 1],
                input[frames - delay + i],
                "fs={rate} enabling Haas must use preceding source, sample {i}"
            );
        }
    }
}
