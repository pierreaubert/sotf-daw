//! Paired wall timing for frozen head-tail layouts; explicitly run outside CI.

// Rust guideline compliant 2026-02-21
use super::{
    NupcEngine, NupcKernel, PartitionKernel, RealFftPlanner, coalesce_partition_levels,
    plan_partitions,
};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

// Frozen constructor bodies: baseline eab4937 and archived rejected candidate.
// Keep both independent of the production constructor when retrying changes.
fn baseline_kernel(ir: &[f32], min_block: usize, head_taps: usize) -> NupcKernel {
    let head_len = head_taps.min(ir.len());
    if head_len == 0 {
        return NupcKernel::new(ir, min_block);
    }
    let tail = &ir[head_len..];
    // Keep every direct tap, but avoid inheriting a prime/non-power head
    // length as the FFT grid. The existing absolute-offset delay places
    // the tail correctly even when the first FFT block is shorter.
    let block_cap = min_block.min(head_len);
    let fft_block = if tail.is_empty() {
        block_cap // An empty tail needs no transform or positive block cap.
    } else {
        1_usize << block_cap.ilog2()
    };
    let specs = coalesce_partition_levels(plan_partitions(tail.len(), fft_block));
    let mut planner = RealFftPlanner::new();
    let levels = specs
        .iter()
        .map(|spec| {
            let absolute_offset = head_len + spec.offset;
            let output_delay = absolute_offset - spec.block_size;
            (PartitionKernel::new(spec, tail, &mut planner), output_delay)
        })
        .collect();
    NupcKernel {
        levels,
        min_block,
        head_taps: Some(Arc::from(&ir[..head_len])),
    }
}

fn candidate_kernel(ir: &[f32], min_block: usize, head_taps: usize) -> NupcKernel {
    let head_len = head_taps.min(ir.len());
    if head_len == 0 {
        return NupcKernel::new(ir, min_block);
    }
    let tail = &ir[head_len..];
    // Keep every direct tap, but avoid inheriting a prime/non-power head
    // length as the FFT grid. The existing absolute-offset delay places
    // the tail correctly even when the first FFT block is shorter.
    let block_cap = min_block.min(head_len);
    let fft_block = if tail.is_empty() {
        block_cap // An empty tail needs no transform or positive block cap.
    } else {
        1_usize << block_cap.ilog2()
    };
    let mut specs = plan_partitions(tail.len(), fft_block);
    // Preserve a final group that shares the preceding group's FFT size:
    // splitting their coalesced level would add another transform pair.
    let standalone_final =
        specs.len() < 2 || specs[specs.len() - 2].block_size != specs[specs.len() - 1].block_size;
    if standalone_final && let Some(last) = specs.last_mut() {
        let remaining = tail.len() - last.offset;
        if remaining < last.block_size {
            // Only the final group can contain this zero padding. Keep its
            // IR offset and one partition, but do not transform a huge
            // block for a short remainder. The base remains a lower bound.
            let block = remaining.next_power_of_two().max(fft_block);
            last.block_size = block;
            last.fft_size = 2 * block;
        }
    }
    let specs = coalesce_partition_levels(specs);
    let mut planner = RealFftPlanner::new();
    let levels = specs
        .iter()
        .map(|spec| {
            let absolute_offset = head_len + spec.offset;
            let output_delay = absolute_offset - spec.block_size;
            (PartitionKernel::new(spec, tail, &mut planner), output_delay)
        })
        .collect();
    NupcKernel {
        levels,
        min_block,
        head_taps: Some(Arc::from(&ir[..head_len])),
    }
}

const FRAMES: usize = 128;
const HEAD: usize = 257;

fn process(engines: &mut [NupcEngine], input: &[f32], output: &mut [f32], stereo: bool) {
    for (channel, engine) in engines.iter_mut().enumerate() {
        let source = if stereo { channel / 2 } else { channel };
        for frame in 0..FRAMES {
            output[channel * FRAMES + frame] =
                engine.process_sample(input[source * FRAMES + frame]);
        }
    }
}

#[test]
#[ignore = "Explicit ABBA wall diagnostic; never a CI timing gate"]
fn final_tail_paired_callback_cost_abba() {
    // Preselected from the accepted 48 kHz admission matrix. The two 97917
    // cases preserve the same shared final FFT group and are unchanged controls.
    let cases = [
        ("mono2", 2, 1, 1_051_901, false, 262_144),
        ("full2", 2, 2, 525_565, false, 131_072),
        ("mono12", 12, 1, 262_397, false, 65_536),
        ("full12_control", 12, 12, 97_917, false, 16_384),
        ("mono32_control", 32, 1, 97_917, false, 16_384),
        ("full32", 32, 32, 32_125, false, 8_192),
        ("true_stereo", 4, 4, 262_397, true, 65_536),
    ];
    for (name, channels, ir_channels, length, stereo, expected_period) in cases {
        let length: usize = length;
        let irs: Vec<Vec<f32>> = (0..ir_channels)
            .map(|channel| {
                (0..length)
                    .map(|i| (((i * 17 + channel * 29) % 101) as f32 - 50.25) * 1e-6)
                    .collect()
            })
            .collect();
        let baseline: Vec<_> = irs
            .iter()
            .map(|ir| baseline_kernel(ir, 1024, HEAD))
            .collect();
        let candidate: Vec<_> = irs
            .iter()
            .map(|ir| candidate_kernel(ir, 1024, HEAD))
            .collect();
        let period = baseline[0].instantiate().largest_block_size();
        assert_eq!(period, expected_period);
        let calls = 256.max((4 * period).div_ceil(FRAMES));
        let warm_calls = (length + 37).div_ceil(FRAMES);
        let input_channels = if stereo { 2 } else { channels };
        // Normal, nonzero samples, preallocated and identical for every trial.
        let inputs: Vec<Vec<f32>> = (0..17)
            .map(|block| {
                (0..input_channels * FRAMES)
                    .map(|i| (((i * 13 + block * 31) % 127) as f32 - 63.5) / 128.0)
                    .collect()
            })
            .collect();
        let mut reference = vec![0.0_f32; calls * channels * FRAMES];
        for (trial, use_candidate) in [false, true, true, false].into_iter().enumerate() {
            let kernels = if use_candidate { &candidate } else { &baseline };
            let mut engines: Vec<_> = (0..channels)
                .map(|c| kernels[c % ir_channels].instantiate())
                .collect();
            let mut output = vec![0.0; channels * FRAMES];
            let mut durations = Vec::with_capacity(calls);
            for block in 0..warm_calls {
                output.fill(0.0);
                process(
                    &mut engines,
                    &inputs[block % inputs.len()],
                    &mut output,
                    stereo,
                );
                black_box(&output);
            }
            let mut max_error = 0.0_f32;
            let mut checksum = 0.0_f64;
            for block in 0..calls {
                output.fill(0.0);
                let input = &inputs[(warm_calls + block) % inputs.len()];
                let start = Instant::now();
                process(&mut engines, input, &mut output, stereo);
                let elapsed = start.elapsed().as_nanos() as u64;
                black_box(&output);
                durations.push(elapsed);
                let reference_block =
                    &mut reference[block * output.len()..(block + 1) * output.len()];
                if trial == 0 {
                    reference_block.copy_from_slice(&output);
                }
                for (&sample, &expected) in output.iter().zip(reference_block.iter()) {
                    assert!(sample.is_finite());
                    max_error = max_error.max((sample - expected).abs());
                    checksum += f64::from(sample);
                }
            }
            assert!(max_error <= 2e-5, "{name}: {max_error}");
            if name.ends_with("control") || !use_candidate {
                assert_eq!(max_error, 0.0);
            }
            let total_ns: u64 = durations.iter().sum();
            let max_ns = durations.iter().max().copied().unwrap();
            println!(
                "PAIRED {{\"case\":\"{name}\",\"trial\":{trial},\"candidate\":{use_candidate},\"channels\":{channels},\"ir_channels\":{ir_channels},\"ir_frames\":{length},\"calls\":{calls},\"warm_calls\":{warm_calls},\"baseline_period\":{period},\"total_ns\":{total_ns},\"max_ns\":{max_ns},\"max_error\":{max_error},\"checksum\":{checksum},\"durations_ns\":{durations:?}}}"
            );
        }
    }
}
