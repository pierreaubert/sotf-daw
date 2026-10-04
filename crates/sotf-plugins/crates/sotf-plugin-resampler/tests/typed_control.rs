//! Allocation-free typed refusals for the dynamic update path.
//!
//! Implements review P1-6: the compatibility `String` API cannot avoid heap
//! allocation on refusal, so the actual audio-thread update path uses
//! [`ResamplerControlError`](sotf_plugin_resampler::ResamplerControlError)
//! (`Copy`, owns no heap) via `try_set_ratio`, `try_set_ratio_relative`,
//! and `try_set_cutoff_smoothing`. The `String` API delegates to the typed
//! core with identical messages. This suite proves (0,0) allocations/frees
//! including refusals plus transactional stream preservation, and proves the
//! two APIs agree on messages.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerControlError, ResamplerPlugin, ResamplerQuality};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const CHUNK: usize = 256;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: System receives the original valid layout/pointer unchanged. The
// counters use constant-initialized TLS cells without allocating or borrowing.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Delegate the caller's allocation contract unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Delegate the original allocation's pointer and layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn tones(frames: usize, frequency: f64) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            (0.5 * (TAU * frequency * frame as f64 / f64::from(RATE)).sin()) as f32
        })
        .collect()
}

#[test]
fn typed_refusals_are_allocation_free_and_transactional() {
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        let (counts, refused, control) = std::thread::spawn(move || {
            // Setup with tracking off: construction allocates backend tables.
            let mut refused =
                ResamplerPlugin::with_quality(1, RATE, RATE, CHUNK, quality).unwrap();
            refused.initialize(f64::from(RATE)).unwrap();
            refused
                .set_parameter(
                    ParameterId::from("dynamic_ratio"),
                    ParameterValue::Bool(true),
                )
                .unwrap();
            refused.try_set_cutoff_smoothing(true).unwrap();
            let mut control =
                ResamplerPlugin::with_quality(1, RATE, RATE, CHUNK, quality).unwrap();
            control.initialize(f64::from(RATE)).unwrap();
            control
                .set_parameter(
                    ParameterId::from("dynamic_ratio"),
                    ParameterValue::Bool(true),
                )
                .unwrap();
            control.try_set_cutoff_smoothing(true).unwrap();
            let music = tones(512, 997.0);
            for plugin in [&mut refused, &mut control] {
                let mut block =
                    vec![0.0; plugin.output_frames_for_input(512)];
                plugin
                    .process(&music, &mut block, &ProcessContext::new(RATE, 512))
                    .unwrap();
            }
            // Preallocate drain buffers before tracking; every `vec!` inside
            // the tracked region would count as an allocation.
            let mut drain_refused =
                vec![0.0; refused.drain_output_frames_max().max(1)];
            let mut drain_control =
                vec![0.0; control.drain_output_frames_max().max(1)];
            // Valid typed updates allocate nothing.
            ALLOCATIONS.set(0);
            DEALLOCATIONS.set(0);
            TRACKING.set(true);
            refused.try_set_ratio(1.5, true).unwrap();
            control.try_set_ratio(1.5, true).unwrap();
            refused.try_set_ratio_relative(1.01, false).unwrap();
            control.try_set_ratio_relative(1.01, false).unwrap();
            // Backend refusals (out of nominal/2..nominal*2, non-finite,
            // non-positive) allocate nothing and change nothing.
            for bad in [10.0, 0.0, -1.0, f64::NAN, f64::INFINITY] {
                let before = refused.current_ratio().to_bits();
                assert!(matches!(
                    refused.try_set_ratio(bad, true),
                    Err(ResamplerControlError::Backend(_))
                ));
                assert_eq!(refused.current_ratio().to_bits(), before);
            }
            assert!(matches!(
                refused.try_set_ratio_relative(10.0, true),
                Err(ResamplerControlError::Backend(_))
            ));
            // Finalize both streams with valid drains (allocation-free).
            let _ = refused
                .drain(&mut drain_refused, &ProcessContext::new(RATE, 0))
                .unwrap();
            let _ = control
                .drain(&mut drain_control, &ProcessContext::new(RATE, 0))
                .unwrap();
            // Post-finalization typed refusals allocate nothing.
            assert_eq!(
                refused.try_set_ratio(1.2, false),
                Err(ResamplerControlError::Finalized("ratio"))
            );
            assert_eq!(
                refused.try_set_ratio_relative(1.1, false),
                Err(ResamplerControlError::Finalized("ratio"))
            );
            assert_eq!(
                refused.try_set_cutoff_smoothing(false),
                Err(ResamplerControlError::Finalized("cutoff smoothing"))
            );
            // Idempotent smoothing sync still succeeds after finalization.
            refused.try_set_cutoff_smoothing(true).unwrap();
            TRACKING.set(false);
            let counts = (ALLOCATIONS.get(), DEALLOCATIONS.get());
            // Drain both to completion (tracking off; valid path is already
            // proven allocation-free, and this keeps the comparison exact).
            let mut refused_out = Vec::new();
            let mut control_out = Vec::new();
            for (plugin, out) in
                [(&mut refused, &mut refused_out), (&mut control, &mut control_out)]
            {
                loop {
                    let mut block = vec![0.0; plugin.drain_output_frames_max().max(1)];
                    let result = plugin
                        .drain(&mut block, &ProcessContext::new(RATE, 0))
                        .unwrap();
                    out.extend_from_slice(&block[..result.frames]);
                    if result.complete {
                        break;
                    }
                }
            }
            (counts, refused_out, control_out)
        })
        .join()
        .unwrap();
        assert_eq!(counts, (0, 0), "{quality:?}: typed refusal allocs/frees");
        assert!(
            refused == control,
            "{quality:?}: refused typed updates must preserve the exact stream"
        );
        // Dynamic-disabled typed refusal on a fresh non-dynamic instance.
        let counts = std::thread::spawn(move || {
            let mut plugin =
                ResamplerPlugin::with_quality(1, RATE, RATE, CHUNK, quality).unwrap();
            ALLOCATIONS.set(0);
            DEALLOCATIONS.set(0);
            TRACKING.set(true);
            assert_eq!(
                plugin.try_set_ratio(1.5, true),
                Err(ResamplerControlError::DynamicDisabled)
            );
            assert_eq!(
                plugin.try_set_ratio_relative(1.01, true),
                Err(ResamplerControlError::DynamicDisabled)
            );
            TRACKING.set(false);
            (ALLOCATIONS.get(), DEALLOCATIONS.get())
        })
        .join()
        .unwrap();
        assert_eq!(
            counts,
            (0, 0),
            "{quality:?}: dynamic-disabled typed refusal allocs/frees"
        );
    }
}

#[test]
fn typed_and_string_apis_agree_on_messages() {
    // Compatibility: the String API delegates to the typed core, so messages
    // match exactly. No allocation tracking here; `to_string` on the control
    // thread is the documented compatibility cost.
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        // Dynamic-disabled.
        let mut plugin =
            ResamplerPlugin::with_quality(1, RATE, RATE, CHUNK, quality).unwrap();
        let string_err = plugin.set_ratio(1.5, true).unwrap_err();
        let typed_err = plugin.try_set_ratio(1.5, true).unwrap_err();
        assert_eq!(string_err, typed_err.to_string());
        assert_eq!(typed_err, ResamplerControlError::DynamicDisabled);
        // Backend rejection.
        plugin
            .set_parameter(
                ParameterId::from("dynamic_ratio"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        let string_err = plugin.set_ratio(10.0, false).unwrap_err();
        let typed_err = plugin.try_set_ratio(10.0, false).unwrap_err();
        assert_eq!(string_err, typed_err.to_string());
        assert!(matches!(typed_err, ResamplerControlError::Backend(_)));
        let string_err = plugin.set_ratio_relative(10.0, false).unwrap_err();
        let typed_err = plugin.try_set_ratio_relative(10.0, false).unwrap_err();
        assert_eq!(string_err, typed_err.to_string());
        // Finalized.
        let music = tones(512, 997.0);
        let mut block = vec![0.0; plugin.output_frames_for_input(512)];
        plugin
            .process(&music, &mut block, &ProcessContext::new(RATE, 512))
            .unwrap();
        let mut drain = vec![0.0; plugin.drain_output_frames_max().max(1)];
        let _ = plugin
            .drain(&mut drain, &ProcessContext::new(RATE, 0))
            .unwrap();
        let string_err = plugin.set_ratio(1.2, false).unwrap_err();
        let typed_err = plugin.try_set_ratio(1.2, false).unwrap_err();
        assert_eq!(string_err, typed_err.to_string());
        assert_eq!(typed_err, ResamplerControlError::Finalized("ratio"));
        let string_err = plugin
            .set_parameter(
                ParameterId::from("cutoff_smoothing"),
                ParameterValue::Bool(true),
            )
            .unwrap_err();
        let typed_err = plugin.try_set_cutoff_smoothing(true).unwrap_err();
        assert_eq!(string_err, typed_err.to_string());
        assert_eq!(
            typed_err,
            ResamplerControlError::Finalized("cutoff smoothing")
        );
    }
}
