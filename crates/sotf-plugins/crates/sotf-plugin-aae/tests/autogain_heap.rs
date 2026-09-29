//! Cold spatial processing is measured separately from public parameter cache rebuilds.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
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
fn measure_heap<T>(operation: impl FnOnce() -> T) -> (T, (usize, usize)) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    (value, COUNTS.with(Cell::get))
}

use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};

#[test]
fn oversized_cold_process_after_control_updates_and_reset_has_no_heap_activity() {
    for rate in [48_000, 192_000] {
        for enabled in [false, true] {
            let mut plugin = AaePlugin::from_params(AaePluginParams {
                auto_gain_enabled: enabled,
                auto_gain_max_db: 12.0,
                auto_gain_smoothing_ms: 100.0,
                ..Default::default()
            })
            .unwrap();
            plugin.initialize(rate).unwrap();
            let frames = 32769;
            let input: Vec<_> = (0..frames)
                .flat_map(|n| {
                    let x = (std::f64::consts::TAU * 997.0 * n as f64 / f64::from(rate)).sin()
                        as f32
                        * 0.003;
                    [x, x * 0.3]
                })
                .collect();
            let mut output = vec![0.0; frames * plugin.output_channels()];

            // IDs and buffers are prepared before entering the callback thread.
            // Keep each Arc-backed ID owner alive outside the measurement guards.
            let enabled_id = ParameterId::from("auto_gain_enabled");
            let max_id = ParameterId::from("auto_gain_max_db");
            let smoothing_id = ParameterId::from("auto_gain_smoothing_ms");
            let (plugin, setter_counts) = std::thread::spawn(move || {
                let context = ProcessContext::new(rate, frames);
                let (_, heap) = measure_heap(|| {
                    for _ in 0..4 {
                        assert_eq!(
                            plugin.process(&input, &mut output, &context).unwrap(),
                            frames
                        );
                    }
                });
                assert_eq!(heap, (0, 0), "cold enabled={enabled}, rate={rate}");
                // Public setter cache work is deliberately measured separately;
                // only the immediately following audio process has a zero-heap gate.
                let (_, enabled_counts) = measure_heap(|| {
                    plugin
                        .set_parameter(enabled_id.clone(), ParameterValue::Bool(true))
                        .unwrap()
                });
                let (_, heap) = measure_heap(|| {
                    for _ in 0..4 {
                        plugin.process(&input, &mut output, &context).unwrap();
                    }
                });
                assert_eq!(heap, (0, 0), "immediately after prepared enable");
                let (_, max_counts) = measure_heap(|| {
                    plugin
                        .set_parameter(max_id.clone(), ParameterValue::Float(9.0))
                        .unwrap()
                });
                let (_, smoothing_counts) = measure_heap(|| {
                    plugin
                        .set_parameter(smoothing_id.clone(), ParameterValue::Float(250.0))
                        .unwrap()
                });
                let (_, heap) = measure_heap(|| {
                    for _ in 0..4 {
                        plugin.process(&input, &mut output, &context).unwrap();
                    }
                });
                assert_eq!(heap, (0, 0), "after max/smoothing update");

                let (_, heap) = measure_heap(|| {
                    plugin.reset();
                    for _ in 0..4 {
                        plugin.process(&input, &mut output, &context).unwrap();
                    }
                });
                assert_eq!(heap, (0, 0), "reset and new metering epoch");
                (plugin, [enabled_counts, max_counts, smoothing_counts])
            })
            .join()
            .unwrap();
            eprintln!(
                "rate={rate}, initial_enabled={enabled}, public enable/max/smoothing allocations/frees={setter_counts:?}"
            );
            drop(plugin);
        }
    }
}
