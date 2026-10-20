//! Independent output pitch trajectories and stereo phase, audit entry 36.
//!
//! Full-complex f64 DFT uses forward-unscaled/inverse-1/N normalization.
//! Positive-frequency analytic isolation has a cosine-tapered passband around
//! the measured partial. Phase is unwrapped before a 10 ms frequency average.
//! Measurements exclude the first/last 0.5 s and account for 2047-frame latency.

// Rust guideline compliant 2026-02-21
use rustfft::{FftPlanner, num_complex::Complex};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_pnd::{PndPlugin, PndPluginParams};
use std::f64::consts::{PI, TAU};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;
const LATENCY: usize = 2047;

#[derive(Clone, Copy, Debug)]
enum Motion {
    Offset,
    Wow,
    Flutter,
    Vibrato,
}
impl Motion {
    fn ratio(self, t: f64) -> f64 {
        match self {
            Self::Offset => 1.01,
            Self::Wow => 1.0 + 0.01 * (TAU * 0.5 * t).sin(),
            Self::Flutter => 1.0 + 0.003 * (TAU * 6.0 * t).sin(),
            Self::Vibrato => 2.0_f64.powf(20.0 * (TAU * 5.0 * t).sin() / 1200.0),
        }
    }
}
fn signal(rate: u32, motion: Motion, weak: bool) -> Vec<f32> {
    let mut phase = 0.0_f64;
    (0..rate as usize * 4)
        .flat_map(|n| {
            let mut frame = [0.0; 2];
            for (ch, x) in frame.iter_mut().enumerate() {
                let offset = if ch == 0 { 0.0 } else { 0.7 };
                let value = if weak {
                    0.0001 * (phase + offset).sin()
                        + 0.2 * (2.0 * phase + offset).sin()
                        + 0.1 * (3.0 * phase + offset).sin()
                } else {
                    0.3 * (phase + offset).sin()
                };
                *x = (value * if ch == 0 { 1.0 } else { 0.5 }) as f32;
            }
            phase += TAU * 440.0 * motion.ratio(n as f64 / f64::from(rate)) / f64::from(rate);
            frame
        })
        .collect()
}
fn render(input: &[f32], rate: u32, strength: f32, block: usize) -> Vec<f32> {
    let mut p = PndPlugin::from_params(
        2,
        PndPluginParams {
            reference_frequency_hz: 440.0,
            correction_strength: strength,
            drift_smoothing: 0.001,
            confidence_threshold: 0.5,
            multi_channel_analysis: true,
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    assert_eq!(p.latency_samples(), LATENCY);
    let mut output = vec![f32::NAN; input.len()];
    for (source, destination) in input.chunks(block * 2).zip(output.chunks_mut(block * 2)) {
        let frames = source.len() / 2;
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                p.process(source, destination, &ProcessContext::new(rate, frames))
                    .unwrap(),
                frames
            );
        });
        assert_eq!(heap, (0, 0));
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}
fn analytic(signal: &[f32], channel: usize, rate: u32, center: f64) -> Vec<Complex<f64>> {
    let frames = signal.len() / 2;
    let size = (frames * 2).next_power_of_two();
    let mut data = vec![Complex::new(0.0, 0.0); size];
    for (x, frame) in data.iter_mut().zip(signal.as_chunks::<2>().0) {
        x.re = f64::from(frame[channel]);
    }
    let mut planner = FftPlanner::<f64>::new();
    planner.plan_fft_forward(size).process(&mut data);
    for (k, x) in data.iter_mut().enumerate() {
        let frequency = k as f64 * f64::from(rate) / size as f64;
        let distance = (frequency / center - 1.0).abs();
        let weight = if k >= size / 2 || distance >= 0.25 {
            0.0
        } else if distance <= 0.1 {
            1.0
        } else {
            0.5 + 0.5 * (PI * (distance - 0.1) / 0.15).cos()
        };
        *x *= weight * 2.0 / size as f64;
    }
    planner.plan_fft_inverse(size).process(&mut data);
    data.truncate(frames);
    data
}
fn frequencies(data: &[Complex<f64>], rate: u32, harmonic: f64) -> Vec<f64> {
    let mut phase = vec![0.0; data.len()];
    for n in 1..data.len() {
        phase[n] = phase[n - 1] + (data[n] * data[n - 1].conj()).arg();
    }
    let half = (0.005 * f64::from(rate)).round() as usize;
    let mut output = vec![0.0; data.len()];
    for n in half..data.len() - half {
        output[n] = (phase[n + half] - phase[n - half]) * f64::from(rate)
            / (2 * half) as f64
            / TAU
            / harmonic;
    }
    output
}

#[test]
fn independent_frequency_probe_resolves_the_analytic_source_trajectories() {
    for rate in [44_100_u32, 96_000] {
        for weak in [false, true] {
            for motion in [
                Motion::Offset,
                Motion::Wow,
                Motion::Flutter,
                Motion::Vibrato,
            ] {
                let source = signal(rate, motion, weak);
                let harmonic = if weak { 2.0 } else { 1.0 };
                let data = analytic(&source, 0, rate, 440.0 * harmonic);
                let pitch = frequencies(&data, rate, harmonic);
                let (mut square, mut count) = (0.0, 0);
                for n in (rate as usize / 2..rate as usize * 7 / 2).step_by(64) {
                    let expected = 440.0 * motion.ratio(n as f64 / f64::from(rate));
                    let cents = 1200.0 * (pitch[n] / expected).log2();
                    square += cents * cents;
                    count += 1;
                }
                let error = (square / count as f64).sqrt();
                println!(
                    "{}",
                    serde_json::json!({"case":"probe_calibration","rate":rate,"weak_fundamental":weak,"motion":format!("{motion:?}"),"rms_error_cents":error})
                );
                assert!(error < 0.2, "probe {error}: {rate}/{weak}/{motion:?}");
            }
        }
    }
}

#[test]
fn rendered_pitch_and_stereo_phase_follow_known_motion_and_strength_policy() {
    for rate in [44_100_u32, 96_000] {
        for weak in [false, true] {
            for motion in [
                Motion::Offset,
                Motion::Wow,
                Motion::Flutter,
                Motion::Vibrato,
            ] {
                let input = signal(rate, motion, weak);
                let strength = if matches!(motion, Motion::Vibrato) {
                    0.0
                } else {
                    1.0
                };
                let output = render(&input, rate, strength, 137);
                assert_eq!(output, render(&input, rate, strength, 1024));
                let harmonic = if weak { 2.0 } else { 1.0 };
                let left = analytic(&output, 0, rate, 440.0 * harmonic);
                let right = analytic(&output, 1, rate, 440.0 * harmonic);
                let pitch = frequencies(&left, rate, harmonic);
                let mut count = 0;
                let (mut input_square, mut output_square, mut phase_error_square) = (0.0, 0.0, 0.0);
                let mut unity_error = 0.0_f64;
                let mut unity_peak = 0.0_f64;
                for source in (rate as usize / 2..rate as usize * 7 / 2 - LATENCY).step_by(64) {
                    let n = source + LATENCY;
                    assert!(left[n].norm() > 1e-6 && right[n].norm() > 1e-6);
                    let expected = motion.ratio(source as f64 / f64::from(rate));
                    let target = if strength == 0.0 { expected } else { 1.0 };
                    input_square += (1200.0 * expected.log2()).powi(2);
                    output_square += (1200.0 * (pitch[n] / 440.0 / target).log2()).powi(2);
                    let phase_error =
                        (right[n] * left[n].conj() * Complex::from_polar(1.0, -0.7)).arg();
                    phase_error_square += phase_error * phase_error;
                    count += 1;
                }
                // Check every neutral frame/channel, independently of probe stride.
                if strength == 0.0 {
                    for source in rate as usize / 2..rate as usize * 7 / 2 - LATENCY {
                        let n = source + LATENCY;
                        for ch in 0..2 {
                            unity_error = unity_error.max(
                                (f64::from(output[n * 2 + ch]) - f64::from(input[source * 2 + ch]))
                                    .abs(),
                            );
                            unity_peak = unity_peak.max(f64::from(input[source * 2 + ch]).abs());
                        }
                    }
                }
                let input_rms = (input_square / count as f64).sqrt();
                let output_rms = (output_square / count as f64).sqrt();
                let phase_rms = (phase_error_square / count as f64).sqrt();
                println!(
                    "{}",
                    serde_json::json!({"case":"rendered_motion","rate":rate,"weak_fundamental":weak,"motion":format!("{motion:?}"),"strength":strength,"input_rms_cents":input_rms,"output_rms_error_cents":output_rms,"stereo_phase_rms_error_rad":phase_rms,"unity_relative_error":(strength==0.0).then(||unity_error/unity_peak)})
                );
                assert!(
                    phase_rms < 0.05,
                    "phase {phase_rms}: {rate}/{weak}/{motion:?}"
                );
                match motion {
                    Motion::Offset => {
                        assert!(output_rms < 8.0, "offset {output_rms}: {rate}/{weak}")
                    }
                    Motion::Wow => assert!(
                        output_rms < input_rms * 0.7,
                        "wow {output_rms}/{input_rms}: {rate}/{weak}"
                    ),
                    Motion::Flutter => {} // Characterize bandwidth; no suppression claim.
                    Motion::Vibrato => assert!(
                        unity_error / unity_peak < 2e-4,
                        "vibrato unity {unity_error}/{unity_peak}"
                    ),
                }
            }
        }
    }
}
