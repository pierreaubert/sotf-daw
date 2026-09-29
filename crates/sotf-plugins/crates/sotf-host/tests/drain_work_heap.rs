//! Cold callback preparation, bound queries, and serial EOS have no heap activity.
// Rust guideline compliant 2026-02-21
use sotf_host::oversampling::{AutoOversampledPlugin, OversampledPlugin};
use sotf_host::plugin::{InPlacePlugin, InPlacePluginAdapter, PluginDrainResult, PluginInfo};
use sotf_host::{DawHost, Parameter, ParameterId, ParameterValue, Plugin, ProcessContext};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::num::NonZeroU64;

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

struct Finite {
    remaining: usize,
    prepared: bool,
}
impl InPlacePlugin for Finite {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Finite heap probe", "1", "tests")
    }
    fn channels(&self) -> usize {
        2
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process_in_place(
        &mut self,
        _: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.remaining = 4109;
        self.prepared = false;
        Ok(context.num_frames)
    }
    fn reset(&mut self) {
        self.remaining = 0;
        self.prepared = false;
    }
    fn begin_drain(&mut self, _: &ProcessContext) -> Result<(), String> {
        self.prepared = true;
        Ok(())
    }
    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        self.prepared
            .then(|| NonZeroU64::new(self.remaining.div_ceil(2053).max(1) as u64).unwrap())
    }
    fn drain_output_frames_max(&self) -> usize {
        2053
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        _: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        assert!(self.prepared);
        let frames = self.remaining.min(output.len() / 2).min(2053);
        output[..frames * 2].fill(0.0);
        self.remaining -= frames;
        Ok(PluginDrainResult {
            frames,
            complete: self.remaining == 0,
        })
    }
}

#[test]
fn cold_preparation_query_drain_and_reset_have_zero_allocations_and_deallocations() {
    for dynamic in [false, true] {
        for factor in [2, 4] {
            let finite = Finite {
                remaining: 0,
                prepared: false,
            };
            let plugin: Box<dyn Plugin> = if dynamic {
                Box::new(
                    AutoOversampledPlugin::new(Box::new(InPlacePluginAdapter::new(finite)), factor)
                        .unwrap(),
                )
            } else {
                Box::new(InPlacePluginAdapter::new(
                    OversampledPlugin::new(finite, factor, 2).unwrap(),
                ))
            };
            let mut host = DawHost::new(2, 48000);
            host.add_plugin(plugin).unwrap();
            host.build().unwrap();
            // One accepted frame leaves the first FFT/filter work for EOS.
            host.process(&[0.25, -0.125], &mut [0.0; 2]).unwrap();
            std::thread::spawn(move || {
                let mut output = [0.0; 512];
                without_heap(|| {
                    for epoch in 0..2 {
                        if epoch > 0 {
                            host.reset();
                            host.process(&[0.25, -0.125], &mut [0.0; 2]).unwrap();
                        }
                        let mut completed = false;
                        for _ in 0..128 {
                            if host.drain(&mut output).unwrap().complete {
                                completed = true;
                                break;
                            }
                        }
                        assert!(completed);
                        assert!(host.drain(&mut []).unwrap().complete);
                    }
                });
            })
            .join()
            .unwrap();
        }
    }
}
