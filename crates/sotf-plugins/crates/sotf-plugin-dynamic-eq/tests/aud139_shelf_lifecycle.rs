//! Shelf lifecycle and realtime heap checks for AUD139.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_dynamic_eq::{DynEqBandParams, DynEqShape, DynamicEqPlugin, DynamicEqPluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACK_HEAP: Cell<bool> = const { Cell::new(false) };
    static HEAP_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static HEAP_DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct ShelfLifecycleAllocator;

fn record_allocation() {
    let _ = TRACK_HEAP.try_with(|tracking| {
        if tracking.get() {
            let _ = HEAP_ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}

fn record_deallocation() {
    let _ = TRACK_HEAP.try_with(|tracking| {
        if tracking.get() {
            let _ = HEAP_DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// SAFETY: All pointers and layouts are forwarded unchanged to System. The
// thread-local counters are constant-initialized and do not allocate.
unsafe impl GlobalAlloc for ShelfLifecycleAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: Forward the caller's valid layout unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation();
        // SAFETY: Forward the original pointer and layout unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        // SAFETY: Forward the original pointer/layout and requested size.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: ShelfLifecycleAllocator = ShelfLifecycleAllocator;

fn measure_heap<R>(operation: impl FnOnce() -> R) -> (R, usize, usize) {
    HEAP_ALLOCATIONS.set(0);
    HEAP_DEALLOCATIONS.set(0);
    TRACK_HEAP.set(true);
    let result = operation();
    TRACK_HEAP.set(false);
    (result, HEAP_ALLOCATIONS.get(), HEAP_DEALLOCATIONS.get())
}

fn plugin(linked: bool) -> DynamicEqPlugin {
    plugin_with_mix(linked, 1.0)
}

fn plugin_with_mix(linked: bool, mix: f32) -> DynamicEqPlugin {
    DynamicEqPlugin::try_from_params_at_sample_rate(
        2,
        DynamicEqPluginParams {
            num_bands: 3,
            threshold: -35.0,
            ratio: 4.0,
            attack_ms: 2.0,
            release_ms: 80.0,
            knee: 4.0,
            link_channels: linked,
            mix,
            bands: vec![
                DynEqBandParams {
                    shape: DynEqShape::Peak,
                    frequency: 1_000.0,
                    q: 1.2,
                    gain: 8.0,
                    ..DynEqBandParams::default()
                },
                DynEqBandParams {
                    shape: DynEqShape::LowShelf,
                    frequency: 250.0,
                    gain: 6.0,
                    shelf_slope: 0.55,
                    ..DynEqBandParams::default()
                },
                DynEqBandParams {
                    shape: DynEqShape::HighShelf,
                    frequency: 3_000.0,
                    gain: -9.0,
                    shelf_slope: 0.8,
                    ..DynEqBandParams::default()
                },
            ],
            stereo_pairs: None,
        },
        48_000,
    )
    .unwrap()
}

fn signal(frames: usize) -> Vec<f32> {
    let mut samples = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        let time = frame as f64 / 48_000.0;
        let low = (2.0 * std::f64::consts::PI * 100.0 * time).sin();
        let mid = (2.0 * std::f64::consts::PI * 1_000.0 * time).sin();
        let high = (2.0 * std::f64::consts::PI * 8_000.0 * time).sin();
        let amplitude = if frame < frames / 2 { 0.12 } else { 0.7 };
        let left = amplitude * (0.45 * low + 0.25 * mid + 0.3 * high);
        let right = amplitude * 0.35 * (0.45 * low + 0.25 * mid + 0.3 * high);
        samples[frame * 2] = left as f32;
        samples[frame * 2 + 1] = right as f32;
    }
    samples
}

#[test]
fn shelf_process_and_populated_reset_do_not_allocate_or_deallocate() {
    let frames = 512;
    let context = ProcessContext::new(48_000, frames);
    let input = signal(frames);

    for linked in [true, false] {
        let mut first = plugin(linked);
        let mut first_buffer = input.clone();
        let (first_result, first_allocations, first_deallocations) =
            measure_heap(|| first.process_in_place(&mut first_buffer, &context));
        assert!(first_result.is_ok());
        assert_eq!(
            first_allocations, 0,
            "linked={linked}: first process allocated"
        );
        assert_eq!(
            first_deallocations, 0,
            "linked={linked}: first process deallocated"
        );

        let mut repeated_buffer = input.clone();
        let (repeated_result, repeated_allocations, repeated_deallocations) =
            measure_heap(|| first.process_in_place(&mut repeated_buffer, &context));
        assert!(repeated_result.is_ok());
        assert_eq!(
            repeated_allocations, 0,
            "linked={linked}: repeated process allocated"
        );
        assert_eq!(
            repeated_deallocations, 0,
            "linked={linked}: repeated process deallocated"
        );

        let mut populated = plugin(linked);
        let mut warmup = signal(4_096);
        populated
            .process_in_place(&mut warmup, &ProcessContext::new(48_000, 4_096))
            .unwrap();
        let (_, reset_allocations, reset_deallocations) = measure_heap(|| populated.reset());
        assert_eq!(
            reset_allocations, 0,
            "linked={linked}: populated reset allocated"
        );
        assert_eq!(
            reset_deallocations, 0,
            "linked={linked}: populated reset deallocated"
        );

        let mut post_reset_buffer = input.clone();
        let (post_reset_result, post_reset_allocations, post_reset_deallocations) =
            measure_heap(|| populated.process_in_place(&mut post_reset_buffer, &context));
        assert!(post_reset_result.is_ok());
        assert_eq!(
            post_reset_allocations, 0,
            "linked={linked}: post-reset process allocated"
        );
        assert_eq!(
            post_reset_deallocations, 0,
            "linked={linked}: post-reset process deallocated"
        );
    }
}

#[test]
fn dry_to_wet_shelf_epochs_reset_populated_filter_state() {
    let frames = 8_192;
    let wet_input = signal(frames);
    let dry_input = signal(4_096);

    for linked in [true, false] {
        let mut plugin = plugin(linked);
        for epoch in 0..2 {
            plugin
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                .unwrap();
            let mut fade_to_dry = dry_input.clone();
            plugin
                .process_in_place(
                    &mut fade_to_dry,
                    &ProcessContext::new(48_000, dry_input.len() / 2),
                )
                .unwrap();

            let mut settled_dry = dry_input.clone();
            plugin
                .process_in_place(
                    &mut settled_dry,
                    &ProcessContext::new(48_000, dry_input.len() / 2),
                )
                .unwrap();
            assert_eq!(settled_dry, dry_input, "linked={linked}, epoch={epoch}");

            plugin
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
                .unwrap();
            let mut actual = wet_input.clone();
            plugin
                .process_in_place(&mut actual, &ProcessContext::new(48_000, frames))
                .unwrap();

            let mut fresh = plugin_with_mix(linked, 0.0);
            fresh
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
                .unwrap();
            let mut expected = wet_input.clone();
            fresh
                .process_in_place(&mut expected, &ProcessContext::new(48_000, frames))
                .unwrap();
            assert_eq!(actual.len(), wet_input.len());
            assert!(actual.iter().all(|sample| sample.is_finite()));
            assert_eq!(actual, expected, "linked={linked}, epoch={epoch}");
        }
    }
}
