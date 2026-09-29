// Rust guideline compliant 2026-09-28
use super::{BeamformerPlugin, Plugin, ProcessContext};
use crate::{BeamformerPluginParams, gsc::GscBeamformer};
use nalgebra::Complex;

const N: usize = 512;
const H: usize = N / 2;

#[test]
fn learned_mvdr_covariance_and_weights_are_unchanged_by_drain() {
    let mut plugin = BeamformerPlugin::new(2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    let initial = plugin.mvdr.noise_cov_snapshot().to_vec();
    let input: Vec<f32> = (0..601)
        .flat_map(|n| {
            [
                (0.13 * n as f32).sin() * 0.25,
                (0.31 * n as f32).cos() * 0.125,
            ]
        })
        .collect();
    let mut output = collect_input(&mut plugin, &input, 48_000, 73);
    let covariance = plugin.mvdr.noise_cov_snapshot().to_vec();
    assert_ne!(covariance, initial, "fixture must learn from real input");
    let weights = plugin.mvdr.weights_buf.clone();
    let solves = plugin.mvdr.weight_solve_count;
    collect_tail(&mut plugin, 48_000, 17, &mut output);
    assert_eq!(plugin.mvdr.noise_cov_snapshot(), covariance);
    assert_eq!(plugin.mvdr.weights_buf, weights);
    assert_eq!(plugin.mvdr.weight_solve_count, solves);
}

fn collect_input(
    plugin: &mut BeamformerPlugin,
    input: &[f32],
    rate: u32,
    callback: usize,
) -> Vec<f32> {
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

fn collect_tail(plugin: &mut BeamformerPlugin, rate: u32, capacity: usize, output: &mut Vec<f32>) {
    let mut scratch = vec![f32::NAN; capacity];
    for _ in 0..2048 {
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
    panic!("Beamformer drain did not terminate");
}

#[test]
fn spectral_drain_matches_independent_windowed_circular_convolution_and_freezes_weights() {
    // Deliberately non-identity transfer, including a tap that spreads energy
    // into the last synthesis sample. The oracle never calls a DSP FFT.
    let taps = [(0, 0.5), (31, 0.25), (N - 2, -0.125)];
    let weights: Vec<Vec<Complex<f32>>> = (0..=H)
        .map(|k| {
            let mut transfer = Complex::new(0.0, 0.0);
            for (tap, gain) in taps {
                let angle = std::f64::consts::TAU * k as f64 * tap as f64 / N as f64;
                transfer += Complex::new(
                    (gain * angle.cos() / 2.0) as f32,
                    (gain * angle.sin() / 2.0) as f32,
                );
            }
            vec![transfer; 2]
        })
        .collect();
    let window: Vec<f64> = (0..N)
        .map(|j| (0.5 - 0.5 * (std::f64::consts::TAU * j as f64 / N as f64).cos()).sqrt())
        .collect();
    for algorithm in [0, 1] {
        for rate in [44_100, 48_000, 96_000] {
            for length in [1, 2, 255, 256, 257, 258, 511, 512, 513, 997] {
                for (callback, capacity) in [(1, 1), (73, 17), (512, 256)] {
                    let mut plugin = BeamformerPlugin::from_params(
                        rate,
                        BeamformerPluginParams {
                            num_mics: 2,
                            beamformer_type: algorithm,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    plugin.initialize(rate).unwrap();
                    plugin.mvdr.compute_weights(&plugin.steering_vectors);
                    plugin.mvdr.weights_buf.clone_from(&weights);
                    if let Some(sd) = plugin.superdirective.as_mut() {
                        sd.install_test_weights(&weights);
                    }
                    // Equal microphones keep the MVDR coherent-target detector
                    // closed during real input, preserving the injected weights.
                    let source: Vec<f32> = (0..length)
                        .map(|n| 0.125 + (n % 11) as f32 / 64.0)
                        .collect();
                    let input: Vec<f32> = source.iter().flat_map(|&x| [x, x]).collect();
                    let mut output = collect_input(&mut plugin, &input, rate, callback);
                    let covariance = plugin.mvdr.noise_cov_snapshot().to_vec();
                    let solves = plugin.mvdr.weight_solve_count;
                    assert_eq!(plugin.mvdr.weights_buf, weights);
                    collect_tail(&mut plugin, rate, capacity, &mut output);
                    assert_eq!(plugin.mvdr.noise_cov_snapshot(), covariance);
                    assert_eq!(plugin.mvdr.weights_buf, weights);
                    assert_eq!(plugin.mvdr.weight_solve_count, solves);
                    let phase = (length - 1) % H + 1;
                    assert_eq!(output.len(), length + 2 * N - phase);
                    let mut expected = vec![0.0_f64; output.len()];
                    for start in (-(H as isize)..length as isize).step_by(H) {
                        for j in 0..N {
                            if start < 0 && j < H {
                                continue;
                            }
                            let mut sample = 0.0;
                            for (tap, gain) in taps {
                                let k = (j + N - tap) % N;
                                let source_frame = start + k as isize;
                                if source_frame >= 0 && (source_frame as usize) < source.len() {
                                    sample +=
                                        gain * source[source_frame as usize] as f64 * window[k];
                                }
                            }
                            expected[(start + N as isize + j as isize) as usize] +=
                                sample * window[j];
                        }
                    }
                    for (frame, (&actual, &expected)) in output.iter().zip(&expected).enumerate() {
                        assert!(
                            (actual as f64 - expected).abs() < 2e-6,
                            "algorithm={algorithm},rate={rate},length={length},callback={callback},capacity={capacity},frame={frame}: {actual} != {expected}"
                        );
                    }
                    if phase >= 2 {
                        assert!(
                            expected.last().unwrap().abs() > 1e-7,
                            "fixture must exercise the full synthesis bound"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn gsc_drain_matches_fractional_delay_and_nonzero_last_learned_tap() {
    for rate in [44_100, 48_000, 96_000] {
        for delay in [3.5_f32, 7.25] {
            for length in [1, 31, 32, 257, 997] {
                for capacity in [1, 17, 256] {
                    let mut plugin = BeamformerPlugin::from_params(
                        rate,
                        BeamformerPluginParams {
                            num_mics: 2,
                            beamformer_type: 2,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    plugin.initialize(rate).unwrap();
                    plugin.gsc = GscBeamformer::new(2, &[delay, 0.0], 32, 0.1);
                    let source: Vec<f32> = (0..length)
                        .map(|n| 0.125 + (n % 13) as f32 / 64.0)
                        .collect();
                    let input: Vec<f32> = source.iter().flat_map(|&x| [x, 0.0]).collect();
                    let mut output = collect_input(&mut plugin, &input, rate, 73);
                    // Deterministic learned state at EOF, with existing signal
                    // histories intact. Prior adaptive output is not an oracle.
                    let mut weights = vec![0.0; 32];
                    weights[0] = 0.25;
                    weights[31] = -0.125;
                    plugin.gsc.install_test_weights(&weights);
                    let learned = plugin.gsc.learned_snapshot();
                    collect_tail(&mut plugin, rate, capacity, &mut output);
                    assert_eq!(plugin.gsc.learned_snapshot(), learned);
                    assert_eq!(output.len(), length + delay.ceil() as usize + 31);
                    let aligned = |n: usize| -> f64 {
                        let whole = delay.floor() as usize;
                        let fraction = f64::from(delay.fract());
                        let a = n
                            .checked_sub(whole)
                            .and_then(|j| source.get(j))
                            .copied()
                            .unwrap_or(0.0) as f64;
                        let b = n
                            .checked_sub(whole + 1)
                            .and_then(|j| source.get(j))
                            .copied()
                            .unwrap_or(0.0) as f64;
                        (1.0 - fraction) * a + fraction * b
                    };
                    for (frame, &actual) in output.iter().enumerate().skip(length) {
                        let blocked = 0.5 * aligned(frame);
                        let previous = frame.checked_sub(31).map_or(0.0, |n| 0.5 * aligned(n));
                        let expected = 0.5 * aligned(frame) - 0.25 * blocked + 0.125 * previous;
                        assert!(
                            (actual as f64 - expected).abs() < 2e-6,
                            "rate={rate},delay={delay},length={length},capacity={capacity},frame={frame}: {actual} != {expected}"
                        );
                    }
                    assert!(
                        output.last().unwrap().abs() > 1e-3,
                        "last learned tap must be emitted"
                    );
                }
            }
        }
    }
}
