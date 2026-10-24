//! Real-input NUPC storage and f64 linear-convolution references.

// Rust guideline compliant 2026-02-21
use super::NupcEngine;

#[test]
fn real_spectra_store_only_nonnegative_frequency_bins() {
    let mut checks = 0;
    let mut failures = 0;
    let mut legacy_complex_values = 0;
    let mut actual_complex_values = 0;
    for block in [1, 3, 16, 64, 257, 1024] {
        let engine = NupcEngine::new(&vec![0.01; block * 9 + 1], block);
        for level in &engine.levels {
            let expected = level.kernel.fft_size / 2 + 1;
            for spectrum in level
                .kernel
                .ir_partitions
                .iter()
                .chain(&level.fdl)
                .chain([&level.fft_spectrum, &level.fft_sum])
            {
                checks += 1;
                failures += usize::from(spectrum.len() != expected);
                legacy_complex_values += level.kernel.fft_size;
                actual_complex_values += spectrum.len();
            }
        }
    }
    println!(
        "nupc_half_spectra checks={checks} failures={failures} legacy_complex_values={legacy_complex_values} actual_complex_values={actual_complex_values}"
    );
    assert_eq!(failures, 0, "real transforms need only N/2+1 complex bins");
}

#[test]
fn real_fft_render_matches_independent_f64_direct_convolution_through_complete_tail() {
    let input: Vec<f32> = (0..137)
        .map(|i| 0.03 + if i % 2 == 0 { 0.02 } else { -0.02 } + (i as f32 * 0.31).sin() * 0.05)
        .collect();
    let mut cases = 0;
    let mut maximum_error = 0.0_f64;
    for block in [1, 3, 16, 64, 257, 1024] {
        for length in [1, 2 * block - 1, 4 * block + 3] {
            let mut ir: Vec<f32> = (0..length)
                .map(|i| (i as f32 * 0.17).cos() * 0.01)
                .collect();
            ir[0] += 0.5;
            ir[length - 1] -= 0.2;
            let mut oracle = vec![0.0_f64; input.len() + ir.len() - 1];
            for (n, &x) in input.iter().enumerate() {
                for (k, &h) in ir.iter().enumerate() {
                    oracle[n + k] += f64::from(x) * f64::from(h);
                }
            }
            for head in [0, 1, block + 1] {
                let mut engine = if head == 0 {
                    NupcEngine::new(&ir, block)
                } else {
                    NupcEngine::new_with_head(&ir, block, head)
                };
                let latency = engine.latency_samples();
                let total = latency + oracle.len() + 2 * block + 7;
                let source: Vec<f32> = (0..total)
                    .map(|i| input.get(i).copied().unwrap_or(0.0))
                    .collect();
                let actual: Vec<f32> = source.iter().map(|&x| engine.process_sample(x)).collect();
                let mut case_error = 0.0_f64;
                for (i, &sample) in actual.iter().enumerate() {
                    let expected = i
                        .checked_sub(latency)
                        .and_then(|n| oracle.get(n))
                        .copied()
                        .unwrap_or(0.0);
                    case_error = case_error.max((f64::from(sample) - expected).abs());
                }
                assert!(
                    case_error < 2e-5,
                    "block={block} length={length} head={head} error={case_error}"
                );
                maximum_error = maximum_error.max(case_error);
                engine.reset();
                let mut partitioned = vec![0.0; total];
                let mut at = 0;
                let mut call = 0;
                while at < total {
                    let count = [1, 31, 257, 4096][call % 4].min(total - at);
                    engine.process_block(&source[at..at + count], &mut partitioned[at..at + count]);
                    at += count;
                    call += 1;
                }
                assert_eq!(actual, partitioned);
                cases += 1;
            }
        }
    }
    println!("nupc_real_direct_reference cases={cases} maximum_error={maximum_error:e}");
    assert_eq!(cases, 54);
}

#[test]
fn large_partition_levels_preserve_sparse_absolute_taps_and_last_tail_sample() {
    let mut cases = 0;
    let mut maximum_error = 0.0_f64;
    for block in [257, 1024] {
        for head in [0, 257] {
            let length = 262_147;
            let taps = [
                (0, 0.5_f32),
                (1023, -0.25),
                (8193, 0.2),
                (131_071, -0.1),
                (length - 1, 0.3),
            ];
            let mut ir = vec![0.0; length];
            for (index, gain) in taps {
                ir[index] = gain;
            }
            let input: Vec<f32> = (0..259).map(|i| (i as f32 * 0.31).sin() * 0.1).collect();
            let mut engine = if head == 0 {
                NupcEngine::new(&ir, block)
            } else {
                NupcEngine::new_with_head(&ir, block, head)
            };
            let latency = engine.latency_samples();
            let total = latency + input.len() + length + 2 * block;
            let mut oracle = vec![0.0_f64; total];
            for (n, &sample) in input.iter().enumerate() {
                for (index, gain) in taps {
                    oracle[latency + n + index] += f64::from(sample) * f64::from(gain);
                }
            }
            let mut case_error = 0.0_f64;
            for (i, expected) in oracle.into_iter().enumerate() {
                let actual = engine.process_sample(input.get(i).copied().unwrap_or(0.0));
                case_error = case_error.max((f64::from(actual) - expected).abs());
            }
            assert!(
                case_error < 2e-5,
                "block={block} head={head} error={case_error}"
            );
            maximum_error = maximum_error.max(case_error);
            cases += 1;
        }
    }
    println!("nupc_large_sparse_reference cases={cases} maximum_error={maximum_error:e}");
    assert_eq!(cases, 4);
}

#[test]
fn prepared_unit_delay_preserves_samples_and_reset_through_arbitrary_callbacks() {
    let source: Vec<f32> = (0..2053)
        .map(|i| {
            if i % 31 == 0 {
                -0.0
            } else {
                (i as f32 * 0.13).sin() * 0.2
            }
        })
        .collect();
    for delay in [0, 1, 32, 544, 4128] {
        let mut engine = NupcEngine::new_unit_delay(delay);
        assert_eq!(engine.latency_samples(), delay);
        assert_eq!(engine.head_taps(), 0);
        assert_eq!(engine.largest_block_size(), 0);
        assert!(
            engine.levels.is_empty(),
            "known identity needs no FFT histories"
        );
        let input: Vec<f32> = source
            .iter()
            .copied()
            .chain(std::iter::repeat_n(0.0, delay + 37))
            .collect();
        let expected: Vec<f32> = (0..input.len())
            .map(|i| {
                i.checked_sub(delay)
                    .and_then(|j| input.get(j))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect();
        let mut actual: Vec<f32> = input.iter().map(|&x| engine.process_sample(x)).collect();
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "delay={delay}"
        );
        engine.reset();
        actual.fill(f32::NAN);
        let mut at = 0;
        let mut call = 0;
        while at < input.len() {
            let count = [1, 0, 17, 128, 511][call % 5].min(input.len() - at);
            engine.process_block(&input[at..at + count], &mut actual[at..at + count]);
            at += count;
            call += 1;
        }
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "reset delay={delay}"
        );
        if delay >= 32 {
            let mut ir = vec![0.0; 8192];
            ir[delay - 32] = 1.0;
            let mut legacy = NupcEngine::new(&ir, 32);
            let max_error = input
                .iter()
                .zip(&expected)
                .map(|(&x, &y)| (legacy.process_sample(x) - y).abs())
                .fold(0.0_f32, f32::max);
            assert!(max_error < 2e-5, "legacy delay={delay}: {max_error}");
        }
    }
}
