//! Cold PND drain, snapshot no-ops and reset must neither allocate nor free.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext, TailLength};
use sotf_plugin_pnd::PndPlugin;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct Counter;
fn record(allocation: bool) {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNTS.try_with(|counts| {
            let (allocations, frees) = counts.get();
            counts.set((
                allocations + usize::from(allocation),
                frees + usize::from(!allocation),
            ));
        });
    }
}
// SAFETY: Original pointers, layouts and results are forwarded to System.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(true);
        // SAFETY: The allocator request is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(false);
        // SAFETY: The original allocation and layout are forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;
struct Stop;
impl Drop for Stop {
    fn drop(&mut self) {
        ACTIVE.set(false);
    }
}
fn no_heap(f: impl FnOnce()) {
    COUNTS.set((0, 0));
    ACTIVE.set(true);
    let guard = Stop;
    f();
    drop(guard);
    assert_eq!(COUNTS.get(), (0, 0));
}

#[test]
fn cold_first_drain_fft_snapshot_and_reset_have_no_heap_operations() {
    for channels in [1, 2, 6] {
        for history in [1, 511, 513] {
            let mut p = PndPlugin::new(channels);
            p.initialize(48_000.0).unwrap();
            p.process(
                &vec![0.1; history * channels],
                &mut vec![0.0; history * channels],
                &ProcessContext::new(48_000, history),
            )
            .unwrap();
            let snapshot: Vec<_> = p
                .parameters()
                .into_iter()
                .map(|parameter| {
                    let value = p.get_parameter(&parameter.id).unwrap();
                    (parameter.id, value)
                })
                .collect();
            std::thread::spawn(move || {
                let mut output = [0.0; 512 * 6];
                no_heap(|| {
                    assert_eq!(p.tail_length(), TailLength::Finite(4094));
                    assert_eq!(p.drain_output_frames_max(), 512);
                    assert!(p.drain_call_bound().is_some());
                    let mut done = p
                        .drain(
                            &mut output[..512 * channels],
                            &ProcessContext::new(48_000, 0),
                        )
                        .unwrap()
                        .complete;
                    for (id, value) in &snapshot {
                        p.set_parameter(id.clone(), value.clone()).unwrap();
                    }
                    while !done {
                        done = p
                            .drain(&mut output[..7 * channels], &ProcessContext::new(48_000, 0))
                            .unwrap()
                            .complete;
                    }
                    assert!(
                        p.drain(&mut [], &ProcessContext::new(48_000, 0))
                            .unwrap()
                            .complete
                    );
                    p.reset();
                    assert!(
                        p.drain(&mut [], &ProcessContext::new(48_000, 0))
                            .unwrap()
                            .complete
                    );
                    p.process(
                        &[0.1; 6][..channels],
                        &mut output[..channels],
                        &ProcessContext::new(48_000, 1),
                    )
                    .unwrap();
                    while !p
                        .drain(
                            &mut output[..512 * channels],
                            &ProcessContext::new(48_000, 0),
                        )
                        .unwrap()
                        .complete
                    {}
                });
            })
            .join()
            .unwrap();
        }
    }
}
