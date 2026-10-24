//! Allocation and deallocation guards for the corrected multiway LR24 path.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_crossover::CrossoverPlugin;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct AuditAllocator;

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOC_EVENTS: Cell<usize> = const { Cell::new(0) };
    static DEALLOC_EVENTS: Cell<usize> = const { Cell::new(0) };
    static REALLOC_EVENTS: Cell<usize> = const { Cell::new(0) };
}

#[global_allocator]
static ALLOCATOR: AuditAllocator = AuditAllocator;

// SAFETY: Allocation and deallocation are delegated directly to `System`.
// The thread-local counters do not affect pointer ownership or layout.
unsafe impl GlobalAlloc for AuditAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACK_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = ALLOC_EVENTS.try_with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: The caller supplied a valid allocation layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACK_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = DEALLOC_EVENTS.try_with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: The pointer and layout are forwarded unchanged to `System`.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = TRACK_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = REALLOC_EVENTS.try_with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: The pointer, old layout, and new size are forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

fn assert_no_allocator_events(action: impl FnOnce()) {
    struct DisableGuard;
    impl Drop for DisableGuard {
        fn drop(&mut self) {
            TRACK_ALLOCATIONS.with(|enabled| enabled.set(false));
        }
    }

    ALLOC_EVENTS.with(|count| count.set(0));
    DEALLOC_EVENTS.with(|count| count.set(0));
    REALLOC_EVENTS.with(|count| count.set(0));
    TRACK_ALLOCATIONS.with(|enabled| enabled.set(true));
    let guard = DisableGuard;
    action();
    drop(guard);
    assert_eq!(
        ALLOC_EVENTS.with(Cell::get),
        0,
        "allocation during audio operation"
    );
    assert_eq!(
        DEALLOC_EVENTS.with(Cell::get),
        0,
        "deallocation during audio operation"
    );
    assert_eq!(
        REALLOC_EVENTS.with(Cell::get),
        0,
        "reallocation during audio operation"
    );
}

#[test]
fn unrelated_thread_allocations_do_not_fail_audio_thread_guard() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let start = std::sync::Arc::new(AtomicBool::new(false));
    let finish = std::sync::Arc::new(AtomicBool::new(false));
    let worker_start = std::sync::Arc::clone(&start);
    let worker_finish = std::sync::Arc::clone(&finish);
    let worker = std::thread::spawn(move || {
        while !worker_start.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
        let buffer = vec![0u8; 4096];
        std::hint::black_box(&buffer);
        worker_finish.store(true, Ordering::Release);
    });
    assert_no_allocator_events(|| {
        start.store(true, Ordering::Release);
        while !finish.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
    });
    worker.join().unwrap();
}

#[test]
fn multiway_process_and_reset_do_not_allocate_or_deallocate() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 1_024;

    let mut plugin =
        CrossoverPlugin::new_multiway(2, "LR24", 1_000.0, "both", &[1_200.0, 3_500.0]).unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input: Vec<f32> = (0..FRAMES * 2)
        .map(|index| ((index as f32 * 0.017).sin() + (index as f32 * 0.003).cos()) * 0.2)
        .collect();
    let mut output = vec![0.0; FRAMES * plugin.output_channels()];
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);

    assert_no_allocator_events(|| {
        plugin.process(&input, &mut output, &context).unwrap();
    });
    assert_no_allocator_events(|| {
        plugin.process(&input, &mut output, &context).unwrap();
    });
    assert_no_allocator_events(|| plugin.reset());
    assert_no_allocator_events(|| {
        plugin.process(&input, &mut output, &context).unwrap();
    });

    // The new-family paths have separate SOS state per branch and must reuse
    // all setup-time storage across callbacks and repeated populated resets.
    for kind in ["LR12", "LR48", "BW48", "Bessel12"] {
        let mut family =
            CrossoverPlugin::new_multiway(2, kind, 300.0, "both", &[2_200.0, 9_000.0]).unwrap();
        family.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let mut family_output = vec![0.0; FRAMES * family.output_channels()];
        let family_context = ProcessContext::new(SAMPLE_RATE, FRAMES);
        assert_no_allocator_events(|| {
            family
                .process(&input, &mut family_output, &family_context)
                .unwrap();
        });
        let fresh_epoch_output = family_output.clone();
        assert_no_allocator_events(|| {
            family
                .process(&input, &mut family_output, &family_context)
                .unwrap();
        });
        assert_no_allocator_events(|| family.reset());
        assert_no_allocator_events(|| {
            family
                .process(&input, &mut family_output, &family_context)
                .unwrap();
        });
        assert_eq!(
            family_output, fresh_epoch_output,
            "{kind} populated reset must reproduce the fresh epoch"
        );
        assert_no_allocator_events(|| family.reset());
        assert_no_allocator_events(|| {
            family
                .process(&input, &mut family_output, &family_context)
                .unwrap();
        });
        assert_eq!(
            family_output, fresh_epoch_output,
            "{kind} repeated populated reset must reproduce the fresh epoch"
        );
    }
}

#[test]
fn active_iir_cutoff_updates_reuse_cached_parameter_metadata() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 257;

    let mut plugin =
        CrossoverPlugin::new_multiway(2, "LR48", 300.0, "both", &[2_000.0, 6_000.0]).unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input: Vec<f32> = (0..FRAMES * 2)
        .map(|index| ((index as f32 * 0.071).sin() + (index as f32 * 0.019).cos()) * 0.2)
        .collect();
    let mut output = vec![0.0; FRAMES * plugin.output_channels()];
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
    let primary = ParameterId::from("frequency");
    let second = ParameterId::from("frequency_2");
    let third = ParameterId::from("frequency_3");

    let mut frames_written = 0;
    assert_no_allocator_events(|| {
        plugin
            .set_parameter(primary.clone(), ParameterValue::Float(400.0))
            .unwrap();
        plugin
            .set_parameter(second.clone(), ParameterValue::Float(2_400.0))
            .unwrap();
        plugin
            .set_parameter(third.clone(), ParameterValue::Float(6_500.0))
            .unwrap();
        frames_written = plugin.process(&input, &mut output, &context).unwrap();
    });

    assert_eq!(frames_written, FRAMES);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-5));
    for (id, expected) in [(&primary, 400.0), (&second, 2_400.0), (&third, 6_500.0)] {
        assert_eq!(
            plugin.get_parameter(id),
            Some(ParameterValue::Float(expected))
        );
    }
}
