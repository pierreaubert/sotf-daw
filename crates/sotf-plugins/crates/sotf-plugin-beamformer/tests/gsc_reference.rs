// Rust guideline compliant 2026-02-21
use sotf_plugin_beamformer::gsc::GscBeamformer;

const TAPS: usize = 32;

// Direct f64 equations with time-indexed history instead of the production
// rings. Uniform broadside blocking is x_i - mean(x), derived independently
// from the array projection rather than reading production coefficients.
fn reference(input: &[f32], mics: usize, delays: &[f32], rate: f64) -> Vec<f64> {
    let frames = input.len() / mics;
    let mut weights = vec![vec![0.0; TAPS]; mics - 1];
    let mut references = vec![vec![0.0; frames]; mics - 1];
    let mut output = Vec::with_capacity(frames);
    let mut aligned = vec![0.0; mics];
    let alpha = (-1.0 / (0.010 * rate)).exp();
    let (mut coherent, mut total) = (0.0, 0.0);
    let mix_alpha = (-1.0 / (0.005 * rate)).exp();
    let mut mix = 1.0;
    for n in 0..frames {
        for mic in 0..mics {
            let delay = f64::from(delays[mic]);
            let integer = delay.floor() as usize;
            let fraction = delay.fract();
            let sample = |age| {
                n.checked_sub(age)
                    .map_or(0.0, |t| f64::from(input[t * mics + mic]))
            };
            aligned[mic] = sample(integer) * (1.0 - fraction) + sample(integer + 1) * fraction;
        }
        let mean = aligned.iter().sum::<f64>() / mics as f64;
        for r in 0..mics - 1 {
            references[r][n] = aligned[r] - mean;
        }
        let mut estimate = 0.0;
        let mut power = f64::from(1e-6_f32);
        for r in 0..mics - 1 {
            for lag in 0..TAPS.min(n + 1) {
                let x = references[r][n - lag];
                estimate += weights[r][lag] * x;
                power += x * x;
            }
        }
        let error = mean - estimate;
        // Spatial coherence uses a smoothed projection onto the uniform
        // steering vector, independently of the blocking-matrix coordinates.
        let sum = aligned.iter().sum::<f64>();
        coherent = alpha * coherent + (1.0 - alpha) * sum * sum / mics as f64;
        total = alpha * total + (1.0 - alpha) * aligned.iter().map(|x| x * x).sum::<f64>();
        let noise_dominant = coherent < 0.65 * total;
        mix = mix_alpha * mix + (1.0 - mix_alpha) * if noise_dominant { 1.0 } else { 0.0 };
        if noise_dominant {
            let scale = f64::from(0.01_f32) * error / power;
            for r in 0..mics - 1 {
                for lag in 0..TAPS.min(n + 1) {
                    weights[r][lag] += scale * references[r][n - lag];
                }
            }
            // Derive the complete microphone FIR explicitly, rather than
            // using the production blocking Gram shortcut. Uniform summation
            // is orthogonal to every ideal blocking row.
            let mut adaptive_energy = 0.0;
            for lag in 0..TAPS {
                for mic in 0..mics {
                    let coefficient: f64 = weights
                        .iter()
                        .enumerate()
                        .map(|(r, row)| {
                            row[lag] * (if r == mic { 1.0 } else { 0.0 } - 1.0 / mics as f64)
                        })
                        .sum();
                    adaptive_energy += coefficient * coefficient;
                }
            }
            let capacity = 1.0 - 1.0 / mics as f64;
            if adaptive_energy > capacity {
                let scale = (capacity / adaptive_energy).sqrt();
                for row in &mut weights {
                    for weight in row {
                        *weight *= scale;
                    }
                }
            }
        }
        assert!(error.is_finite());
        output.push((mean - mix * estimate).clamp(f64::from(f32::MIN), f64::from(f32::MAX)));
    }
    output
}

#[test]
fn adaptive_gsc_matches_direct_f64_equations_across_scales_and_delays() {
    let mut maximum_normalized_error = 0.0_f64;
    for mics in 2..=8 {
        for fractional in [false, true] {
            let delays: Vec<f32> = (0..mics)
                .map(|mic| if fractional { mic as f32 * 0.375 } else { 0.0 })
                .collect();
            for (rate, amplitude) in [16_000.0, 44_100.0, 48_000.0, 96_000.0]
                .into_iter()
                .flat_map(|rate| {
                    [0.25, 1e-20, f32::MAX * 0.75]
                        .into_iter()
                        .map(move |amplitude| (rate, amplitude))
                })
            {
                let mut input: Vec<f32> = (0..2048)
                    .flat_map(|n| {
                        (0..mics).map(move |mic| {
                            let target = (n as f64 * 0.13).sin() * 0.4;
                            let interference = (n as f64 * 0.071 + mic as f64 * 1.4).cos() * 0.5;
                            ((target + interference) * f64::from(amplitude)) as f32
                        })
                    })
                    .collect();
                input.resize(input.len() + 128 * mics, 0.0);
                let expected = reference(&input, mics, &delays, rate);
                let mut gsc = GscBeamformer::new_with_sample_rate(mics, &delays, TAPS, 0.01, rate);
                let actual: Vec<f32> = input
                    .chunks_exact(mics)
                    .map(|frame| gsc.process_sample(frame))
                    .collect();
                for (&actual, &expected) in actual.iter().zip(&expected) {
                    assert!(actual.is_finite());
                    let error = (f64::from(actual) - expected).abs() / f64::from(amplitude);
                    maximum_normalized_error = maximum_normalized_error.max(error);
                    assert!(
                        error < 2e-6,
                        "mics={mics}, fractional={fractional}, amplitude={amplitude}, normalized_error={error}"
                    );
                }
                assert!(actual[actual.len() - 64..].iter().all(|&x| x == 0.0));
                gsc.reset();
                let reset: Vec<f32> = input
                    .chunks_exact(mics)
                    .map(|frame| gsc.process_sample(frame))
                    .collect();
                assert_eq!(reset, actual);
            }
        }
    }
    eprintln!("GSC direct-f64 maximum normalized error: {maximum_normalized_error:e}");
}

#[test]
fn extreme_burst_then_small_references_matches_wide_reference_without_reset() {
    let mut input = vec![f32::MAX, -f32::MAX, -f32::MAX, -f32::MAX];
    input.resize(257 * 4, 0.0);
    input.extend((0..2048).flat_map(|n| {
        let v = (n as f32 * 0.17).sin() * 1e-4;
        [v, -v, v * 0.5, v * 0.5]
    }));
    let expected = reference(&input, 4, &[0.0; 4], 48_000.0);
    let mut gsc = GscBeamformer::new(4, &[0.0; 4], TAPS, 0.01);
    for (n, frame) in input.as_chunks::<4>().0.iter().enumerate() {
        let actual = gsc.process_sample(frame);
        assert!(actual.is_finite());
        let scale = if n == 0 { f64::from(f32::MAX) } else { 1e-4 };
        assert!(
            (f64::from(actual) - expected[n]).abs() / scale < 2e-6,
            "frame={n}"
        );
    }
}

#[test]
fn learned_cancellation_clamps_only_unrepresentable_output() {
    // Noise-dominant training gives a .5 blocking reference and .25 fixed
    // output. A reference impulse followed by common negative full-scale audio
    // combines retained cancellation history with an already full-scale sum.
    let mut input: Vec<f32> = (0..8000).flat_map(|_| [0.75, -0.25]).collect();
    input.extend_from_slice(&[f32::MAX, -f32::MAX]);
    input.extend((0..256).flat_map(|_| [-f32::MAX, -f32::MAX]));
    input.resize(input.len() + 128 * 2, 0.0);
    let expected = reference(&input, 2, &[0.0; 2], 48_000.0);
    assert!(expected.iter().any(|&x| x == f64::from(f32::MIN)));
    let mut gsc = GscBeamformer::new(2, &[0.0; 2], TAPS, 0.01);
    for (n, frame) in input.as_chunks::<2>().0.iter().enumerate() {
        let actual = gsc.process_sample(frame);
        assert!(actual.is_finite());
        let scale = if n < 8000 { 1.0 } else { f64::from(f32::MAX) };
        assert!(
            (f64::from(actual) - expected[n]).abs() / scale < 2e-6,
            "frame={n}"
        );
    }
}
