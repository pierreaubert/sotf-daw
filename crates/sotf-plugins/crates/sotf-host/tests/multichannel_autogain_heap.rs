//! Cold multichannel meter ingestion, publication, gain and reset retain storage.
// Rust guideline compliant 2026-02-21
use sotf_host::AutoGainParams;
use sotf_host::multichannel_auto_gain::MultichannelAutoGain;
use sotf_host::speaker_config::get_speaker_config;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
// SAFETY: This allocator forwards every original pointer and layout to System.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNTING.with(|active| {
            if active.get() {
                COUNTS.with(|counts| {
                    let (allocations, frees) = counts.get();
                    counts.set((allocations + 1, frees));
                });
            }
        });
        // SAFETY: The valid allocation request is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        COUNTING.with(|active| {
            if active.get() {
                COUNTS.with(|counts| {
                    let (allocations, frees) = counts.get();
                    counts.set((allocations, frees + 1));
                });
            }
        });
        // SAFETY: The pointer and its original allocation layout are unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct StopCounting;
impl Drop for StopCounting {
    fn drop(&mut self) {
        COUNTING.with(|active| active.set(false));
    }
}
fn without_heap<T>(operation: impl FnOnce() -> T) -> T {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    assert_eq!(COUNTS.with(Cell::get), (0, 0), "allocations and frees");
    value
}

#[test]
fn cold_paired_and_legacy_oversized_ingestion_have_no_allocations_or_frees() {
    for rate in [48_000, 192_000] {
        for legacy in [false, true] {
            let mut meter = MultichannelAutoGain::new(
                rate,
                AutoGainParams {
                    enabled: true,
                    max_gain_db: 12.0,
                    smoothing_ms: 100.0,
                    ..Default::default()
                },
            )
            .unwrap();
            let frames = 32769;
            let input: Vec<_> = (0..frames)
                .flat_map(|n| {
                    let x = (std::f64::consts::TAU * 997.0 * n as f64 / f64::from(rate)).sin()
                        as f32
                        * 0.01;
                    [x, -x * 0.5]
                })
                .collect();
            let mut output = vec![0.003; frames * 6];
            let (meter, output) = std::thread::spawn(move || {
                let cfg = get_speaker_config("5.1").unwrap();
                without_heap(|| {
                    for epoch in 0..2 {
                        for _ in 0..8 {
                            output.fill(0.003);
                            if legacy {
                                meter.measure_input(&input).unwrap();
                                meter
                                    .measure_and_apply(&mut output, frames, 6, cfg)
                                    .unwrap();
                            } else {
                                meter
                                    .measure_aligned_and_apply(&input, &mut output, frames, 6, cfg)
                                    .unwrap();
                            }
                        }
                        // The paired disabled path does not ingest or advance; its
                        // immediately re-enabled call resumes prepared state.
                        meter.set_enabled(false);
                        meter
                            .measure_aligned_and_apply(&input, &mut output, frames, 6, cfg)
                            .unwrap();
                        meter.set_enabled(true);
                        meter.set_max_gain_db(6.0);
                        meter.set_smoothing_ms(250.0);
                        meter
                            .measure_aligned_and_apply(&input, &mut output, frames, 6, cfg)
                            .unwrap();
                        if epoch == 0 {
                            meter.reset();
                        }
                    }
                });
                (meter, output)
            })
            .join()
            .unwrap();
            drop((meter, output));
        }
    }
}
