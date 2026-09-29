//! Cold AutoGain gain advancement and reset retain all prepared storage.
// Rust guideline compliant 2026-02-21
use sotf_host::{AutoGain, AutoGainParams};
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
fn cold_gain_paths_target_changes_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2, 8] {
        let mut meter = AutoGain::new(
            channels,
            48_000,
            AutoGainParams {
                enabled: true,
                max_gain_db: 12.0,
                smoothing_ms: 25.0,
                ..Default::default()
            },
        )
        .unwrap();
        let input: Vec<_> = (0..48_000)
            .flat_map(|n| {
                let sample =
                    (std::f64::consts::TAU * 997.0 * n as f64 / 48_000.0).sin() as f32 * 0.1;
                std::iter::repeat_n(sample, channels)
            })
            .collect();
        let measured: Vec<_> = input.iter().map(|x| x * 0.5).collect();
        meter.measure_input(&input).unwrap();
        meter.measure_output(&measured).unwrap();
        let mut output = vec![0.25; 8193 * channels];
        let (meter, output) = std::thread::spawn(move || {
            without_heap(|| {
                meter.apply_compensation(&mut output, 8193);
                meter.set_smoothing_ms(400.0);
                meter.next_gain_linear();
                meter.next_n(137);
                meter.set_target_lufs(Some(-32.0)).unwrap();
                meter.refresh_output_measurement();
                meter.apply_compensation(&mut output, 8193);
                meter.next_n(48_000 * 6);
                meter.apply_compensation(&mut output, 8193);
                meter.set_enabled(false);
                meter.apply_compensation(&mut output, 8193);
                meter.next_n(8193);
                meter.set_enabled(true);
                meter.reset();
                meter.apply_compensation(&mut output, 8193);
                meter.next_n(0);
                meter.apply_compensation(&mut output, 0);
            });
            (meter, output)
        })
        .join()
        .unwrap();
        drop((meter, output));
    }
}
