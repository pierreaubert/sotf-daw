//! Threshold-adjacent excitation must not create a large recursive tail jump.

use sotf_plugin_aae::fdn::Fdn;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

#[test]
fn limiter_threshold_crossing_preserves_continuity_in_static_and_modulated_networks() {
    let mut cases = 0;
    let mut worst = 0.0_f32;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for room in [0.2, 3.0] {
            for modulation in [0.0, 0.5, 1.0] {
                let mut below = Fdn::new(rate, room, 1.2, 1.0, 1.0, modulation, 0.0);
                let mut above = Fdn::new(rate, room, 1.2, 1.0, 1.0, modulation, 0.0);
                let mut difference = 0.0_f32;
                let mut peak = 0.0_f32;
                sotf_host::assert_no_allocs("recursive limiter render", || {
                    for frame in 0..rate as usize * 3 {
                        // Eight-way input distribution places the first feedback
                        // write immediately on opposite sides of the 0 dB ceiling.
                        let excitation = if frame == 0 { 8.0_f32.sqrt() } else { 0.0 };
                        let a = below.process(excitation * (1.0 - 1e-5));
                        let b = above.process(excitation * (1.0 + 1e-5));
                        for (a, b) in a.into_iter().zip(b) {
                            assert!(a.is_finite() && b.is_finite());
                            difference = difference.max((a - b).abs());
                            peak = peak.max(a.abs()).max(b.abs());
                        }
                    }
                });
                println!(
                    "limiter_tail rate={rate} room={room} modulation={modulation} difference={difference} peak={peak}"
                );
                assert!(peak > 0.1);
                assert!(
                    difference < 0.001,
                    "threshold-adjacent excitation caused tail jump {difference}"
                );
                worst = worst.max(difference);
                cases += 1;
            }
        }
    }
    println!("limiter_recursive cases={cases} max_difference={worst}");
}
