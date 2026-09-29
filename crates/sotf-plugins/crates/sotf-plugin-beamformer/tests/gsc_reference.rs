// Rust guideline compliant 2026-02-21
use sotf_plugin_beamformer::gsc::GscBeamformer;

const TAPS: usize = 32;

// Direct f64 equations with time-indexed history instead of the production
// rings. Uniform broadside blocking is x_i - mean(x), derived independently
// from the array projection rather than reading production coefficients.
fn reference(input: &[f32], mics: usize, delays: &[f32]) -> Vec<f64> {
    let frames = input.len() / mics;
    let mut weights = vec![vec![0.0; TAPS]; mics - 1];
    let mut references = vec![vec![0.0; frames]; mics - 1];
    let mut output = Vec::with_capacity(frames);
    let mut aligned = vec![0.0; mics];
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
        let current_power = references.iter().map(|row| row[n].powi(2)).sum::<f64>();
        if current_power >= mean * mean * f64::from(0.01_f32) {
            let scale = f64::from(0.01_f32) * error / power;
            for r in 0..mics - 1 {
                for lag in 0..TAPS.min(n + 1) {
                    weights[r][lag] += scale * references[r][n - lag];
                }
            }
        }
        assert!(error.is_finite());
        output.push(error.clamp(f64::from(f32::MIN), f64::from(f32::MAX)));
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
            for amplitude in [0.25, 1e-20, f32::MAX * 0.75] {
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
                let expected = reference(&input, mics, &delays);
                let mut gsc = GscBeamformer::new(mics, &delays, TAPS, 0.01);
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
    let expected = reference(&input, 4, &[0.0; 4]);
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
    // The blocking reference is .25 and the fixed output .5 during training,
    // so the learned FIR sum approaches two. Opposite full-range samples then
    // produce a mathematically finite cancellation result beyond f32 range.
    let mut input: Vec<f32> = (0..8000).flat_map(|_| [0.75, 0.25]).collect();
    input.extend((0..256).flat_map(|_| [f32::MAX, -f32::MAX]));
    input.resize(input.len() + 128 * 2, 0.0);
    let expected = reference(&input, 2, &[0.0; 2]);
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
