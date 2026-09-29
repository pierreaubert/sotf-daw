//! Cold spectral startup, zero continuation, and prepared wrappers retain storage.
// Rust guideline compliant 2026-02-21
use sotf_host::oversampling::AutoOversampledPlugin;
use sotf_host::{
    ParameterId, ParameterValue, ParametricInPlacePluginAdapter, Plugin, ProcessContext,
};
use sotf_plugin_multiband_expander::{MultibandExpanderPlugin, MultibandExpanderPluginParams};
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
fn cold_native_and_oversampled_spectral_drain_has_zero_allocations_and_deallocations() {
    for channels in [1, 2] {
        for factor in [1, 2, 4] {
            for frames in [1, 257, 8193] {
                for fading in [false, true] {
                    let params = MultibandExpanderPluginParams {
                        processing_mode: "spectral".into(),
                        mix: if fading { 1.1e-5 } else { 1.0 },
                        ..Default::default()
                    };
                    let mut native: Box<dyn Plugin> =
                        Box::new(ParametricInPlacePluginAdapter::new(
                            MultibandExpanderPlugin::with_params(channels, params),
                        ));
                    assert_eq!(native.drain_output_frames_max(), 256);
                    if factor > 1 {
                        native = Box::new(
                            AutoOversampledPlugin::new_with_max_frames(native, factor, frames)
                                .unwrap(),
                        );
                    }
                    native.initialize(48000).unwrap();
                    if fading {
                        native
                            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                            .unwrap();
                    }
                    let input = vec![0.125; frames * channels];
                    let mut output = vec![0.0; input.len()];
                    let mut tail = vec![123.0; 256 * channels];
                    std::thread::spawn(move || {
                        without_heap(|| {
                            for epoch in 0..2 {
                                if epoch > 0 {
                                    native.reset();
                                }
                                native
                                    .process(
                                        &input,
                                        &mut output,
                                        &ProcessContext::new(48000, frames),
                                    )
                                    .unwrap();
                                let context = ProcessContext::new(48000, 0);
                                native.begin_drain(&context).unwrap();
                                let bound = native.drain_call_bound().unwrap().get();
                                let mut complete = false;
                                for _ in 0..bound {
                                    assert!(native.drain_call_bound().is_some());
                                    let _ = native.tail_length();
                                    if native.drain(&mut tail, &context).unwrap().complete {
                                        complete = true;
                                        break;
                                    }
                                }
                                assert!(complete);
                                assert!(native.drain(&mut [], &context).unwrap().complete);
                                assert_eq!(native.drain_output_frames_max(), 256);
                            }
                        });
                    })
                    .join()
                    .unwrap();
                }
            }
        }
    }
}
