//! Fractional block phases retain absolute convolution timing and reset history.

// Rust guideline compliant 2026-02-21
use super::NupcKernel;

fn kernel(ir: &[f32], block: usize, head: usize) -> NupcKernel {
    if head == 0 {
        NupcKernel::new(ir, block)
    } else {
        NupcKernel::new_with_head(ir, block, head)
    }
}

#[test]
fn prepared_phase_and_reset_follow_each_level_block_size() {
    let mut cases = 0;
    let mut checks = 0;
    let mut failures = 0;
    for length in [1, 1025, 4099, 65539] {
        for block in [32, 1024] {
            for head in [0, 257, 512] {
                let ir = vec![0.0; length];
                let prepared = kernel(&ir, block, head);
                let mut fractions = Vec::new();
                for count in [1, 2, 12, 32] {
                    fractions.extend((0..count).map(|index| (index, count)));
                }
                fractions.extend([(1023, 1024), (usize::MAX - 1, usize::MAX)]);
                for (index, count) in fractions {
                    let mut engine = prepared.instantiate_with_phase(index, count);
                    for reset in [false, true] {
                        if reset {
                            for sample in 0..37 {
                                engine.process_sample(sample as f32 / 64.0);
                            }
                            engine.reset();
                        }
                        for level in &engine.levels {
                            let expected = (level.kernel.block_size as u128 * index as u128
                                / count as u128)
                                as usize;
                            checks += 2;
                            failures += usize::from(level.input_fill != expected);
                            failures += usize::from(level.output_queue_pos != expected);
                        }
                    }
                    assert_eq!(engine.latency_samples(), if head == 0 { block } else { 0 });
                    cases += 1;
                }
            }
        }
    }
    println!("nupc_phase_layout cases={cases} checks={checks} failures={failures}");
    assert_eq!(cases, 1176);
    assert_eq!(failures, 0, "nonzero phases still synchronize transforms");
}

#[test]
fn phased_engines_match_f64_convolution_and_exact_replay_through_complete_tail() {
    let input: Vec<f32> = (0..513)
        .map(|i| ((i * 17 % 61) as f32 - 30.0) / 256.0)
        .collect();
    let mut maximum_error = 0.0_f64;
    let mut cases = 0;
    for length in [97, 65539] {
        let mut ir = vec![0.0; length];
        let positions: Vec<usize> = if length == 97 {
            (0..length).collect()
        } else {
            vec![
                0,
                1,
                31,
                255,
                256,
                257,
                1023,
                1024,
                2049,
                16385,
                32771,
                length - 1,
            ]
        };
        for (i, &tap) in positions.iter().enumerate() {
            ir[tap] = ((i * 13 % 41) as f32 - 20.0) / 128.0;
        }
        for head in [0, 257, 512] {
            let prepared = kernel(&ir, 1024, head);
            for (index, count) in [
                (0, 1),
                (1, 2),
                (11, 12),
                (31, 32),
                (1023, 1024),
                (usize::MAX - 1, usize::MAX),
            ] {
                let mut engine = prepared.instantiate_with_phase(index, count);
                let latency = engine.latency_samples();
                let frames = input.len() + ir.len() - 1 + latency + 17;
                let signal: Vec<f32> = (0..frames)
                    .map(|i| input.get(i).copied().unwrap_or(0.0))
                    .collect();
                let actual: Vec<f32> = signal.iter().map(|&v| engine.process_sample(v)).collect();
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
                    let error = (f64::from(sample) - expected).abs();
                    maximum_error = maximum_error.max(error);
                    assert!(
                        error < 2e-5,
                        "length={length} head={head} phase={index}/{count} frame={frame} error={error}"
                    );
                }
                engine.reset();
                for i in 0..777 {
                    engine.process_sample(signal[i % signal.len()]);
                }
                engine.reset();
                let mut replay = vec![0.0; frames];
                let mut position = 0;
                let mut call = 0;
                let pattern = [1, 127, 257, 1025, 3];
                while position < frames {
                    let n = pattern[call % pattern.len()].min(frames - position);
                    engine.process_block(
                        &signal[position..position + n],
                        &mut replay[position..position + n],
                    );
                    position += n;
                    call += 1;
                }
                assert_eq!(actual, replay, "phase/reset changed callback partitioning");
                cases += 1;
            }
        }
    }
    println!("nupc_phase_f64 cases={cases} maximum_error={maximum_error:e} exact_replay=true");
    assert_eq!(cases, 36);
}
