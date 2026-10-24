//! Equal-size runtime levels share transforms without changing absolute tap support.

// Rust guideline compliant 2026-02-21
use super::NupcEngine;
use super::partition_level::{PartitionKernel, PartitionLevel};
use super::time_domain_head::TimeDomainHead;
use super::types::plan_partitions;
use realfft::RealFftPlanner;
use std::sync::Arc;

fn engine(ir: &[f32], block: usize, head: usize) -> NupcEngine {
    if head == 0 {
        NupcEngine::new(ir, block)
    } else {
        NupcEngine::new_with_head(ir, block, head)
    }
}

// Explicit pre-coalescing construction, with its separate output delay per
// original group. It shares real-FFT primitives, so the independent f64 direct
// references remain necessary to establish actual convolution accuracy.
fn unmerged(ir: &[f32], block: usize, head: usize) -> NupcEngine {
    let head = head.min(ir.len());
    let min_block = if head == 0 { block } else { block.min(head) };
    let mut planner = RealFftPlanner::new();
    let levels = plan_partitions(ir.len() - head, min_block)
        .iter()
        .map(|spec| {
            let delay = if head == 0 {
                block + spec.offset - spec.block_size
            } else {
                head + spec.offset - spec.block_size
            };
            PartitionLevel::from_kernel(
                PartitionKernel::new(spec, &ir[head..], &mut planner),
                delay,
            )
        })
        .collect();
    NupcEngine {
        levels,
        min_block: block,
        identity_delay: None,
        td_head: if head == 0 {
            None
        } else {
            Some(TimeDomainHead::from_taps(Arc::from(&ir[..head])))
        },
        td_head_len: head,
    }
}

#[test]
fn runtime_has_one_level_per_block_size_with_unchanged_partition_coverage() {
    let mut cases = 0;
    let mut failures = 0;
    let mut old_levels = 0;
    let mut actual_levels = 0;
    for block in [1, 3, 32, 257, 1024] {
        for length in [
            0,
            1,
            block,
            2 * block,
            2 * block + 1,
            4 * block,
            4 * block + 1,
            12 * block + 7,
        ] {
            for head in [0, 1, block + 1] {
                let ir = vec![0.01; length];
                let e = engine(&ir, block, head);
                let effective_head = head.min(length);
                let min_block = if effective_head == 0 {
                    block
                } else {
                    // Head construction now selects an FFT-friendly cap before
                    // coalescing. Keep the old unmerged renderer below unchanged
                    // as an independent layout/equivalence reference.
                    let cap = block.min(effective_head);
                    let upper = cap.next_power_of_two();
                    if upper == cap { cap } else { upper / 2 }
                };
                let original = plan_partitions(length - effective_head, min_block);
                let mut sizes: Vec<_> = original.iter().map(|s| s.block_size).collect();
                sizes.dedup();
                let actual_sizes: Vec<_> = e.levels.iter().map(|s| s.kernel.block_size).collect();
                failures += usize::from(actual_sizes != sizes);
                assert_eq!(
                    original.iter().map(|s| s.count).sum::<usize>(),
                    e.levels
                        .iter()
                        .map(|s| s.kernel.ir_partitions.len())
                        .sum::<usize>()
                );
                old_levels += original.len();
                actual_levels += e.levels.len();
                cases += 1;
            }
        }
    }
    println!(
        "nupc_coalesced_coverage cases={cases} failures={failures} original_levels={old_levels} actual_levels={actual_levels}"
    );
    assert_eq!(cases, 120);
    assert_eq!(
        failures, 0,
        "each source block needs one transform pair per block size"
    );
}

#[test]
fn merged_levels_match_separate_delayed_groups_through_reset_and_complete_tail() {
    let mut cases = 0;
    let mut maximum_error = 0.0_f64;
    for block in [1, 3, 32, 257, 1024] {
        for length in [2 * block + 1, 4 * block + 1, 16 * block + 7] {
            let mut ir: Vec<_> = (0..length)
                .map(|i| (i as f32 * 0.37).sin() * 0.02)
                .collect();
            ir[0] += 0.5;
            ir[length - 1] -= 0.3;
            for head in [0, 1, block + 1] {
                let mut actual = engine(&ir, block, head);
                let mut reference = unmerged(&ir, block, head);
                assert_eq!(actual.latency_samples(), reference.latency_samples());
                // Interrupt state at a non-aligned source position, then reset.
                for i in 0..block + 7 {
                    actual.process_sample((i as f32 * 0.29).cos() * 0.1);
                    reference.process_sample((i as f32 * 0.29).cos() * 0.1);
                }
                actual.reset();
                reference.reset();
                let source_frames = 259;
                let total = actual.latency_samples() + source_frames + length + 2 * block + 7;
                let mut waveform = Vec::with_capacity(total);
                for i in 0..total {
                    let input = if i < source_frames {
                        (i as f32 * 0.31).sin() * 0.1
                    } else {
                        0.0
                    };
                    let a = actual.process_sample(input);
                    let b = reference.process_sample(input);
                    maximum_error = maximum_error.max((f64::from(a) - f64::from(b)).abs());
                    assert!(
                        (a - b).abs() < 2e-5,
                        "block={block} length={length} head={head} sample={i}: {a} vs {b}"
                    );
                    waveform.push(a);
                }
                actual.reset();
                let mut partitioned = vec![0.0; total];
                let mut at = 0;
                let mut call = 0;
                while at < total {
                    let count = [1, 31, 257, 4096][call % 4].min(total - at);
                    let input: Vec<_> = (at..at + count)
                        .map(|i| {
                            if i < source_frames {
                                (i as f32 * 0.31).sin() * 0.1
                            } else {
                                0.0
                            }
                        })
                        .collect();
                    actual.process_block(&input, &mut partitioned[at..at + count]);
                    at += count;
                    call += 1;
                }
                assert_eq!(waveform, partitioned);
                cases += 1;
            }
        }
    }
    println!("nupc_coalesced_unmerged cases={cases} maximum_error={maximum_error:e}");
    assert_eq!(cases, 45);
}
