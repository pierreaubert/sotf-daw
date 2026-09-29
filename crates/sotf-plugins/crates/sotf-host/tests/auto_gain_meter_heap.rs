//! Cold and hour-long AutoGain measurement paths reuse prepared storage.
// Rust guideline compliant 2026-02-21
use sotf_host::auto_gain::{AutoGain, AutoGainParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CallbackAllocator;

// SAFETY: Allocation and deallocation contracts are forwarded unchanged to System.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counts| {
                let (allocations, frees) = counts.get();
                counts.set((allocations + 1, frees));
            });
        }
        // SAFETY: System receives the original valid allocation layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counts| {
                let (allocations, frees) = counts.get();
                counts.set((allocations, frees + 1));
            });
        }
        // SAFETY: The pointer and its original allocation layout are unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

struct StopCounting;

impl Drop for StopCounting {
    fn drop(&mut self) {
        COUNTING.with(|active| active.set(false));
    }
}

fn measured(operation: impl FnOnce()) -> (usize, usize) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    operation();
    drop(guard);
    COUNTS.with(Cell::get)
}

#[test]
fn cold_measurement_controls_and_reset_have_no_heap_activity() {
    for channels in [1, 2, 6, 24] {
        for rate in [48_000, 192_000] {
            let mut gain = AutoGain::new(
                channels,
                rate,
                AutoGainParams {
                    enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let input: Vec<f32> = (0..8193 * channels)
                .map(|i| ((i % 127) as f32 - 63.0) / 1024.0)
                .collect();
            let mut output = input.clone();
            let (gain, input, output, counts) = std::thread::spawn(move || {
                let mut counts = [(0, 0); 2];
                for count in &mut counts {
                    *count = measured(|| {
                        gain.ingest_input(&input).unwrap();
                        gain.ingest_output(&output).unwrap();
                        gain.refresh_input_measurement();
                        gain.refresh_output_measurement();
                        gain.apply_compensation(&mut output, 8193);
                        gain.set_enabled(false);
                        gain.set_enabled(true);
                        gain.set_smoothing_ms(10.0);
                        gain.set_target_lufs(Some(-24.0)).unwrap();
                        gain.set_max_gain_db(9.0);
                        std::hint::black_box(gain.get_data());
                        gain.reset();
                    });
                }
                (gain, input, output, counts)
            })
            .join()
            .unwrap();
            assert_eq!(counts, [(0, 0); 2], "channels={channels}, rate={rate}");
            drop((gain, input, output));
        }
    }
}

#[test]
fn hour_long_measurement_and_repeated_queries_have_no_heap_activity() {
    for (channels, rate) in [(1, 20), (2, 8_000), (6, 20), (24, 20)] {
        let mut gain = AutoGain::new_default(channels, rate).unwrap();
        let input: Vec<f32> = (0..rate as usize / 10 * channels)
            .map(|i| {
                if rate == 20 {
                    0.0
                } else if i % 4 < 2 {
                    0.001
                } else {
                    -0.001
                }
            })
            .collect();
        let (gain, input, counts) = std::thread::spawn(move || {
            let mut counts = [(0, 0); 2];
            for count in &mut counts {
                *count = measured(|| {
                    for _ in 0..36_013 {
                        gain.measure_input(&input).unwrap();
                        gain.measure_output(&input).unwrap();
                        gain.refresh_input_measurement();
                        gain.refresh_output_measurement();
                    }
                    std::hint::black_box(gain.get_data());
                    gain.reset();
                });
            }
            (gain, input, counts)
        })
        .join()
        .unwrap();
        assert_eq!(counts, [(0, 0); 2], "channels={channels}, rate={rate}");
        drop((gain, input));
    }
}
