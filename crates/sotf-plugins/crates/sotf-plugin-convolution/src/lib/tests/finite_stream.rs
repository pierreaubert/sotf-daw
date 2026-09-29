//! Frozen backend publication and callback ownership across end of stream.

// Rust guideline compliant 2026-02-21
use super::{
    ConvolutionLoadStatus, IrLoadCompletion, empty_retired_ir_state, make_delta_ir_plugin,
    make_delta_ir_result,
};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use std::sync::{Arc, mpsc};

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
// SAFETY: This allocator forwards every pointer and layout to the existing counting allocator.
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
        unsafe { sotf_host::CountingAlloc.alloc(layout) }
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
        unsafe { sotf_host::CountingAlloc.dealloc(ptr, layout) }
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
fn heap_counts<T>(operation: impl FnOnce() -> T) -> (T, (usize, usize)) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    (value, COUNTS.with(Cell::get))
}

#[test]
fn ready_and_late_completions_remain_owned_until_reset_publication() {
    for ready_before_drain in [false, true] {
        let mut p = make_delta_ir_plugin(false, false);
        let mut reference = make_delta_ir_plugin(false, false);
        let mut input = [0.0; 259];
        input[258] = 0.75;
        let mut expected_input = input;
        p.process_in_place(&mut input, &ProcessContext::new(48_000, 259))
            .unwrap();
        reference
            .process_in_place(&mut expected_input, &ProcessContext::new(48_000, 259))
            .unwrap();
        let old = Arc::downgrade(&p.state);
        let replacement = make_delta_ir_result("pending-ir");
        let replacement_weak = Arc::downgrade(&replacement.state);
        let (tx, rx) = mpsc::channel();
        p.desired_generation = 7;
        p.ir_load_result_rx = Some(rx);
        p.completion_pending = true;
        p.ir_load_result_keepalive = Some(tx.clone());
        let (primary_tx, primary_rx) = mpsc::sync_channel(1);
        let (fallback_tx, fallback_rx) = mpsc::sync_channel(1);
        primary_tx
            .try_send(empty_retired_ir_state())
            .unwrap_or_else(|_| panic!("prepare primary"));
        fallback_tx
            .try_send(empty_retired_ir_state())
            .unwrap_or_else(|_| panic!("prepare fallback"));
        p.test_ir_reclaimers = Some((primary_tx, fallback_tx));
        let mut completion = Some(IrLoadCompletion {
            generation: 7,
            result: Ok(replacement),
        });
        if ready_before_drain {
            tx.send(completion.take().unwrap()).unwrap();
        }
        // Invalid attempts must neither consume a ready completion nor enter EOS.
        assert!(p.drain(&mut [], &ProcessContext::new(48_000, 0)).is_err());
        assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
        assert!(p.completion_pending);
        let mut output = [0.0; 17];
        let (_, first_counts) = heap_counts(|| {
            p.drain(&mut output, &ProcessContext::new(48_000, 0))
                .unwrap();
        });
        assert_eq!(first_counts, (0, 0));
        let mut reference_output = [0.0; 17];
        reference
            .process_in_place(&mut reference_output, &ProcessContext::new(48_000, 17))
            .unwrap();
        assert_eq!(output, reference_output);
        if !ready_before_drain {
            // Simulate the worker completing after EOS has already been accepted.
            let sender = tx.clone();
            let payload = completion.take().unwrap();
            std::thread::spawn(move || sender.send(payload).unwrap())
                .join()
                .unwrap();
        }
        // The only strong owner of the new state is still its mailbox payload.
        assert_eq!(replacement_weak.strong_count(), 1);
        assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
        let mut consumed = 17;
        loop {
            let (result, counts) = heap_counts(|| {
                p.drain(&mut output, &ProcessContext::new(48_000, 0))
                    .unwrap()
            });
            assert_eq!(counts, (0, 0));
            reference_output.fill(0.0);
            reference
                .process_in_place(
                    &mut reference_output[..result.frames],
                    &ProcessContext::new(48_000, result.frames),
                )
                .unwrap();
            assert_eq!(&output[..result.frames], &reference_output[..result.frames]);
            consumed += result.frames;
            if result.complete {
                break;
            }
        }
        assert_eq!(consumed, 1024);
        assert!(p.completion_pending);
        assert_eq!(replacement_weak.strong_count(), 1);
        assert!(Arc::ptr_eq(&p.state, &old.upgrade().unwrap()));
        assert_eq!(old.strong_count(), 1);
        // Reset keeps the mailbox intact. Ordinary processing resumes publication.
        p.reset();
        assert!(p.completion_pending);
        assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
        let (_, counts) = heap_counts(|| {
            p.process_in_place(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap()
        });
        assert_eq!(counts, (0, 0));
        assert!(!p.completion_pending);
        assert!(
            p.ir_load_result_rx.is_some(),
            "consumed receiver stays owned"
        );
        assert!(Arc::ptr_eq(&p.state, &replacement_weak.upgrade().unwrap()));
        // Saturated reclaimer queues leave the old final Arc owned by the plugin.
        assert!(p.retired_pending.is_some());
        assert_eq!(old.strong_count(), 1);
        drop(primary_rx.recv().unwrap());
        let (_, counts) = heap_counts(|| {
            p.process_in_place(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap()
        });
        assert_eq!(counts, (0, 0));
        assert!(p.retired_pending.is_none());
        assert_eq!(old.strong_count(), 1);
        drop(primary_rx.recv().unwrap());
        assert_eq!(old.strong_count(), 0);
        drop(fallback_rx.recv().unwrap());
    }
}

#[test]
fn pending_failure_and_stale_state_are_not_consumed_during_drain() {
    for failed in [false, true] {
        let mut p = make_delta_ir_plugin(false, false);
        p.process_in_place(&mut [0.75], &ProcessContext::new(48_000, 1))
            .unwrap();
        let active = Arc::downgrade(&p.state);
        let (tx, rx) = mpsc::channel();
        let stale = make_delta_ir_result("stale-ir");
        let stale_weak = Arc::downgrade(&stale.state);
        let (primary_tx, primary_rx) = mpsc::sync_channel(1);
        let (fallback_tx, _fallback_rx) = mpsc::sync_channel(1);
        p.test_ir_reclaimers = Some((primary_tx, fallback_tx));
        p.desired_generation = 3;
        p.ir_load_result_rx = Some(rx);
        p.completion_pending = true;
        p.ir_load_result_keepalive = Some(tx.clone());
        tx.send(IrLoadCompletion {
            generation: if failed { 3 } else { 2 },
            result: if failed {
                Err("prepared failure".to_string())
            } else {
                Ok(stale)
            },
        })
        .unwrap();
        let mut output = [0.0; 1024];
        let (result, counts) = heap_counts(|| {
            p.drain(&mut output, &ProcessContext::new(48_000, 0))
                .unwrap()
        });
        assert!(result.complete);
        assert_eq!(counts, (0, 0));
        assert!(p.completion_pending);
        assert_eq!(active.strong_count(), 1);
        if !failed {
            assert_eq!(stale_weak.strong_count(), 1);
        }
        p.reset();
        let (_, counts) = heap_counts(|| {
            p.process_in_place(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap()
        });
        assert_eq!(counts, (0, 0));
        assert!(!p.completion_pending);
        assert!(
            p.ir_load_result_rx.is_some(),
            "consumed receiver stays owned"
        );
        assert!(Arc::ptr_eq(&p.state, &active.upgrade().unwrap()));
        if failed {
            assert_eq!(p.load_status(), ConvolutionLoadStatus::Failed);
        } else {
            assert_eq!(stale_weak.strong_count(), 1);
            drop(primary_rx.recv().unwrap());
            assert_eq!(stale_weak.strong_count(), 0);
        }
    }
}

#[test]
fn held_output_replacement_and_clear_fades_finish_without_extending_old_ir_history() {
    for clear in [false, true] {
        let mut p = make_delta_ir_plugin(clear, clear);
        let mut reference = make_delta_ir_plugin(clear, clear);
        for plugin in [&mut p, &mut reference] {
            let frames = if clear { 1 } else { 1025 };
            plugin
                .process_in_place(
                    &mut vec![0.75; frames],
                    &ProcessContext::new(48_000, frames),
                )
                .unwrap();
            if clear {
                plugin
                    .parametric_set_parameter(
                        "ir_file".into(),
                        sotf_host::ParameterValue::String(String::new()),
                    )
                    .unwrap();
            } else {
                plugin.apply_ir_state(make_delta_ir_result("replacement"));
            }
        }
        let mut actual = [0.0; 1024];
        let result = p
            .drain(&mut actual, &ProcessContext::new(48_000, 0))
            .unwrap();
        let expected_frames = if clear { 128 } else { 1024 };
        assert_eq!(result.frames, expected_frames);
        assert!(result.complete);
        let mut expected = vec![0.0; expected_frames];
        reference
            .process_in_place(&mut expected, &ProcessContext::new(48_000, expected_frames))
            .unwrap();
        assert_eq!(&actual[..expected_frames], &expected);
        assert!(actual[0] > 0.5);
        assert_eq!(actual[127], 0.0);
    }
}
