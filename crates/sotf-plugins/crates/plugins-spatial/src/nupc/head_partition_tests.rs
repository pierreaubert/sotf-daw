//! Direct-head tails use FFT-friendly blocks without shortening causal taps.

// Rust guideline compliant 2026-02-21
use super::NupcKernel;

#[test]
fn every_supported_head_selects_power_two_tail_blocks_under_the_requested_cap() {
    let ir = vec![0.01; 8195];
    let mut cases = 0;
    let mut failures = 0;
    let mut levels = 0;
    for cap in [32, 1024] {
        for head in 32..=512 {
            let engine = NupcKernel::new_with_head(&ir, cap, head).instantiate();
            let first = &engine.levels[0];
            let block = first.kernel.block_size;
            let limit = cap.min(head);
            let valid = block.is_power_of_two()
                && block <= limit
                && block * 2 > limit
                && engine.levels.iter().all(|level| {
                    level.kernel.block_size.is_power_of_two()
                        && level.kernel.fft_size == 2 * level.kernel.block_size
                });
            failures += usize::from(!valid);
            assert_eq!(
                engine.head_taps(),
                head,
                "FFT selection shortened the direct head"
            );
            assert_eq!(engine.latency_samples(), 0);
            assert_eq!(first.output_delay.len() + block, head);
            assert!(
                engine
                    .levels
                    .iter()
                    .map(|level| level.kernel.ir_partitions.len() * level.kernel.block_size)
                    .sum::<usize>()
                    >= ir.len() - head
            );
            if cap == 1024 && head == 257 {
                let blocks: Vec<_> = engine
                    .levels
                    .iter()
                    .map(|level| level.kernel.block_size)
                    .collect();
                println!(
                    "head_fft_example head=257 blocks={blocks:?} first_delay={}",
                    first.output_delay.len()
                );
            }
            cases += 1;
            levels += engine.levels.len();
        }
    }
    println!("head_fft_selection cases={cases} levels={levels} failures={failures}");
    assert_eq!(cases, 962);
    assert_eq!(
        failures, 0,
        "non-power head lengths still force non-power FFTs"
    );
}
