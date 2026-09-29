//! Cold Crossfeed callbacks keep prepared reference and meter storage.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};
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
fn cold_meter_intervals_controls_and_reset_have_no_heap_activity() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for mode in [
            CrossfeedMode::Bauer,
            CrossfeedMode::Meier,
            CrossfeedMode::Mb,
            CrossfeedMode::Hrtf,
        ] {
            let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams {
                mode,
                mix: 1.0,
                max_block_frames: 8193,
                autogain_enabled: true,
                ..Default::default()
            })
            .unwrap();
            plugin.initialize(rate).unwrap();
            let mut buffer: Vec<_> = (0..8193)
                .flat_map(|frame| {
                    let sample = (std::f64::consts::TAU * 997.0 * frame as f64 / f64::from(rate))
                        .sin() as f32
                        * 0.1;
                    [sample, -0.5 * sample]
                })
                .collect();
            let updates = [
                (
                    ParameterId::from("autogain_target_lufs"),
                    ParameterValue::Float(-20.0),
                ),
                (
                    ParameterId::from("autogain_max_gain_db"),
                    ParameterValue::Float(10.0),
                ),
                (
                    ParameterId::from("autogain_smoothing_ms"),
                    ParameterValue::Float(500.0),
                ),
                (
                    ParameterId::from("autogain_enabled"),
                    ParameterValue::Bool(false),
                ),
                (
                    ParameterId::from("autogain_enabled"),
                    ParameterValue::Bool(true),
                ),
            ];
            let (plugin, buffer, updates) = std::thread::spawn(move || {
                without_heap(|| {
                    for iteration in 0..64 {
                        let frames = [8193, 17, 137][iteration % 3];
                        plugin
                            .process_in_place(
                                &mut buffer[..frames * 2],
                                &ProcessContext::new(rate, frames),
                            )
                            .unwrap();
                    }
                    for (id, value) in &updates {
                        plugin
                            .parametric_set_parameter(id.clone(), value.clone())
                            .unwrap();
                        plugin
                            .process_in_place(&mut buffer, &ProcessContext::new(rate, 8193))
                            .unwrap();
                    }
                    plugin.reset();
                    plugin
                        .process_in_place(&mut buffer, &ProcessContext::new(rate, 8193))
                        .unwrap();
                    plugin
                        .process_in_place(&mut [], &ProcessContext::new(rate, 0))
                        .unwrap();
                });
                (plugin, buffer, updates)
            })
            .join()
            .unwrap();
            drop((plugin, buffer, updates));
        }
    }
}
