//! DAW parameter synchronization and reset must reuse their allocated storage.
use sotf_host::ParametricInPlacePluginAdapter;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_dither::DitherPlugin;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}

struct TrackingAllocator;

fn record() {
    let _ = TRACKING.try_with(|tracking| {
        if tracking.get() {
            let _ = OPERATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// SAFETY: all memory operations delegate unchanged to System. Thread-local
// counters neither allocate nor retain the caller's memory pointers.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[test]
fn parameter_reads_updates_processing_and_reset_do_not_allocate_or_free() {
    // 12 channels is the catalog maximum (see CHANGELOG 0.5.12).
    for channels in [1, 2, 6, 12] {
        let mut plugin = ParametricInPlacePluginAdapter::new(DitherPlugin::new(channels));
        plugin.initialize(96_000.0).unwrap();
        let ids = ["bit_depth", "noise_shaping", "dither_type"].map(ParameterId::from);
        let input = vec![0.1; 127 * channels];
        let mut output = vec![0.0; input.len()];
        let context = ProcessContext::new(96_000, 127);
        OPERATIONS.set(0);
        TRACKING.set(true);
        for bit_depth in 0..3 {
            for dither_type in 0..3 {
                for shaping in [false, true] {
                    let values = [
                        ParameterValue::Int(bit_depth),
                        ParameterValue::Bool(shaping),
                        ParameterValue::Int(dither_type),
                    ];
                    for (id, value) in ids.iter().zip(values) {
                        plugin.set_parameter(id.clone(), value.clone()).unwrap();
                        assert_eq!(plugin.get_parameter(id), Some(value));
                    }
                    plugin.process(&input, &mut output, &context).unwrap();
                    plugin.reset();
                }
            }
        }
        TRACKING.set(false);
        assert_eq!(OPERATIONS.get(), 0, "channels={channels}");
    }
}
