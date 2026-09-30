//! Frozen backend publication and callback ownership across end of stream.

// Rust guideline compliant 2026-02-21
use super::super::types::ConvolutionPluginParams;
use super::{
    ConvolutionLoadStatus, ConvolutionPlugin, IrLoadCompletion, MAX_IR_MEMORY_BYTES,
    empty_retired_ir_state, make_delta_ir_plugin, make_delta_ir_result,
};
use crate::misc::PARTITION_SIZE;
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use std::sync::{Arc, mpsc};

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    static ALLOCATED_BYTES: Cell<usize> = const { Cell::new(0) };
    static FREED_BYTES: Cell<usize> = const { Cell::new(0) };
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
                ALLOCATED_BYTES.with(|bytes| bytes.set(bytes.get().saturating_add(layout.size())));
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
                FREED_BYTES.with(|bytes| bytes.set(bytes.get().saturating_add(layout.size())));
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

fn heap_allocated_bytes<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    ALLOCATED_BYTES.with(|bytes| bytes.set(0));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    (value, ALLOCATED_BYTES.with(Cell::get))
}

fn heap_retained_bytes<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    ALLOCATED_BYTES.with(|bytes| bytes.set(0));
    FREED_BYTES.with(|bytes| bytes.set(0));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    let allocated = ALLOCATED_BYTES.with(Cell::get);
    let freed = FREED_BYTES.with(Cell::get);
    // This is the net requested-byte delta for the live returned value. It is
    // neither peak resident memory nor cumulative allocation churn.
    assert!(
        allocated >= freed,
        "tracked construction freed {freed} bytes after allocating {allocated}"
    );
    let retained = allocated - freed;
    (value, retained)
}

fn write_true_stereo_ir(path: &std::path::Path, frames: usize, sample_rate: u32) {
    let channels = 4_u16;
    let data_size = (frames * channels as usize * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_size as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for frame in 0..frames {
        for path in 0..4 {
            let sample = if frame % 997 == path * 13 {
                2_048_i16
            } else {
                0
            };
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn true_stereo_estimate_bounds_retained_prepared_storage() {
    use std::sync::atomic::{AtomicU64, Ordering};

    const RATE: u32 = 48_000;
    const IR_FRAMES: usize = 8_197;
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "sotf-convolution-memory-bound-{}-{}.wav",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    write_true_stereo_ir(&path, IR_FRAMES, RATE);

    for (use_nupc, zero_latency_head, head_taps) in [
        (false, false, 128),
        (true, false, 128),
        (true, true, 257),
        (true, true, 512),
    ] {
        let estimate = ConvolutionPlugin::estimated_true_stereo_backend_bytes(
            &[IR_FRAMES; 4],
            use_nupc,
            zero_latency_head,
            head_taps,
        );
        assert!(estimate <= MAX_IR_MEMORY_BYTES);
        let (plugin, retained_bytes) = heap_retained_bytes(|| {
            ConvolutionPlugin::from_params_with_routing(
                2,
                RATE,
                ConvolutionPluginParams {
                    ir_file: path.to_string_lossy().into_owned(),
                    mix: 1.0,
                    gain_db: 0.0,
                    use_nupc,
                    zero_latency_head,
                    head_taps,
                },
                true,
            )
            .unwrap()
        });
        assert_eq!(plugin.nupc_engines.len(), if use_nupc { 4 } else { 0 });
        assert!(
            retained_bytes <= estimate,
            "mode={use_nupc}/{zero_latency_head}/{head_taps}: constructor retained {retained_bytes} bytes, estimate is {estimate}"
        );
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn true_stereo_fft_reserve_covers_budget_admissible_non_power_head_levels() {
    use plugins_spatial::nupc;
    use rustfft::FftPlanner;
    use rustfft::num_complex::Complex;
    use std::collections::BTreeSet;

    let mut checked_fft_sizes = BTreeSet::new();

    // The public schema admits every integer head length in this inclusive
    // range. The largest accepted IR plan for a given head contains every
    // smaller partition level, so measuring its unique FFT sizes covers all
    // plans that can pass this monotone estimate for that setting.
    for head_taps in 32..=512 {
        // One byte per IR sample is already a lower bound for the four-path
        // retained backend. The budget in frames is therefore a rejected
        // upper endpoint independent of sample rate or the 30-second limit.
        // Binary search then derives the actual accepted boundary rather
        // than assuming one fixed sample rate or duration cap.
        let mut low = 1_usize;
        let mut high = MAX_IR_MEMORY_BYTES;
        assert!(
            ConvolutionPlugin::estimated_true_stereo_backend_bytes(
                &[high; 4], true, true, head_taps,
            ) > MAX_IR_MEMORY_BYTES,
            "upper endpoint should be rejected for head_taps={head_taps}"
        );
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            let estimate = ConvolutionPlugin::estimated_true_stereo_backend_bytes(
                &[middle; 4],
                true,
                true,
                head_taps,
            );
            if estimate <= MAX_IR_MEMORY_BYTES {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        assert!(
            ConvolutionPlugin::estimated_true_stereo_backend_bytes(
                &[low + 1; 4],
                true,
                true,
                head_taps,
            ) > MAX_IR_MEMORY_BYTES,
            "search endpoint for head_taps={head_taps} did not reach the budget boundary"
        );

        let head_length = head_taps.min(low);
        let minimum_block = PARTITION_SIZE.min(head_length);
        let tail_length = low - head_length;
        for spec in nupc::plan_partitions(tail_length, minimum_block) {
            checked_fft_sizes.insert(spec.fft_size);
        }
    }

    // No-head NUPC is a separate public configuration. Its FFT sizes are
    // powers of two but can reach a higher level than a 512-tap head because
    // the time-domain head consumes part of the IR budget.
    let mut low = 1_usize;
    let mut high = MAX_IR_MEMORY_BYTES;
    assert!(
        ConvolutionPlugin::estimated_true_stereo_backend_bytes(&[high; 4], true, false, 0,)
            > MAX_IR_MEMORY_BYTES,
        "no-head upper endpoint should be rejected"
    );
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if ConvolutionPlugin::estimated_true_stereo_backend_bytes(&[middle; 4], true, false, 0)
            <= MAX_IR_MEMORY_BYTES
        {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    assert!(
        ConvolutionPlugin::estimated_true_stereo_backend_bytes(&[low + 1; 4], true, false, 0,)
            > MAX_IR_MEMORY_BYTES,
        "no-head search endpoint did not reach the budget boundary"
    );
    for spec in nupc::plan_partitions(low, PARTITION_SIZE) {
        checked_fft_sizes.insert(spec.fft_size);
    }

    assert!(checked_fft_sizes.contains(&514));
    assert!(checked_fft_sizes.contains(&1028));
    assert!(checked_fft_sizes.contains(&2056));
    assert!(checked_fft_sizes.iter().any(|size| !size.is_power_of_two()));

    for fft_size in checked_fft_sizes {
        let ((), allocated_bytes) = heap_allocated_bytes(|| {
            let mut planner = FftPlanner::<f32>::new();
            let forward = planner.plan_fft_forward(fft_size);
            let inverse = planner.plan_fft_inverse(fft_size);
            let scratch = forward
                .get_inplace_scratch_len()
                .max(inverse.get_inplace_scratch_len());
            assert!(
                scratch <= fft_size * 8,
                "rustfft {fft_size}-point scratch {scratch} exceeds Bluestein reserve"
            );
            std::hint::black_box((forward, inverse));
        });
        let reserved = fft_size * 26 * std::mem::size_of::<Complex<f32>>() + 4 * 1024;
        assert!(
            allocated_bytes <= reserved,
            "rustfft {fft_size}-point forward/inverse plans allocated {allocated_bytes} bytes, estimate reserves {reserved}"
        );
    }
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
