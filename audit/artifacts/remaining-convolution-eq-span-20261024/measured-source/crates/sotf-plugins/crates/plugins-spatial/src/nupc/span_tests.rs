//! Large-span chronological and independent sparse-convolution references.

// Rust guideline compliant 2026-02-21
use super::NupcKernel;

#[test]
fn spans_match_scalar_and_f64_through_large_levels_and_dirty_reset() {
    let input: Vec<f32> = (0..513)
        .map(|i| ((i * 17 % 61) as f32 - 30.0) / 256.0)
        .collect();
    let mut cases = 0;
    let mut maximum_error = 0.0_f64;
    for (block, head, length) in [
        (257, 0, 262_147),
        (257, 257, 262_147),
        (1024, 0, 262_147),
        (1024, 257, 262_147),
        (1024, 0, 1_047_810),
        (1024, 257, 1_047_810),
    ] {
        let positions = [0, 1, 31, 257, 1023, 8193, 131_071, length - 1];
        let mut ir = vec![0.0; length];
        for (i, &position) in positions.iter().enumerate() {
            ir[position] = ((i * 13 % 41) as f32 - 20.0) / 128.0;
        }
        let kernel = NupcKernel::new_with_head(&ir, block, head);
        for (index, count) in [(0, 1), (31, 32)] {
            let mut scalar = kernel.instantiate_with_phase(index, count);
            let latency = scalar.latency_samples();
            let total = input.len() + length - 1 + latency + 37;
            let signal: Vec<f32> = input
                .iter()
                .copied()
                .chain(std::iter::repeat_n(0.0, total - input.len()))
                .collect();
            let expected_scalar: Vec<f32> = signal
                .iter()
                .map(|&sample| scalar.process_sample(sample))
                .collect();
            let mut engine = kernel.instantiate_with_phase(index, count);
            for sample in signal.iter().take(777) {
                engine.process_sample(*sample);
            }
            engine.reset();
            let mut actual = vec![f32::NAN; total];
            let mut at = 0;
            let mut call = 0;
            while at < total {
                let frames = [0, 1, 31, 128, 511, 1025, 4093][call % 7].min(total - at);
                engine.process_block(&signal[at..at + frames], &mut actual[at..at + frames]);
                at += frames;
                call += 1;
            }
            let difference = actual
                .iter()
                .zip(&expected_scalar)
                .position(|(a, b)| a.to_bits() != b.to_bits());
            assert!(
                difference.is_none(),
                "block={block} head={head} length={length} phase={index}/{count} first bit difference={difference:?}"
            );
            let mut error = 0.0_f64;
            for (frame, &sample) in actual.iter().enumerate() {
                let expected: f64 = positions
                    .iter()
                    .filter_map(|&tap| {
                        let source = frame.checked_sub(latency)?.checked_sub(tap)?;
                        input
                            .get(source)
                            .map(|&value| f64::from(value) * f64::from(ir[tap]))
                    })
                    .sum();
                error = error.max((f64::from(sample) - expected).abs());
            }
            assert!(
                error < 2e-5,
                "block={block} head={head} length={length} phase={index}/{count} f64 error={error}"
            );
            maximum_error = maximum_error.max(error);
            cases += 1;
        }
    }
    println!("nupc_span_sparse cases={cases} maximum_error={maximum_error:e} exact_bits=true");
    assert_eq!(cases, 12);
}
