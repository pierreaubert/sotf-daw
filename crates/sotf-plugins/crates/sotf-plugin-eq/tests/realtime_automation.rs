//! Exercise both allocation and destruction at automation boundaries.
use math_audio_iir_fir::{Biquad, BiquadFilterType};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::ProcessContext;
use sotf_plugin_eq::EqPlugin;
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

// SAFETY: allocations and deallocations are delegated unchanged to System;
// the thread-local counters do not allocate or retain the memory pointers.
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
fn band_automation_reuses_storage_through_completion_and_reset() {
    for (order, topology, oversampling) in [(2, 0, 1), (8, 0, 1), (2, 1, 1), (4, 0, 2)] {
        let mut plugin = EqPlugin::new(
            2,
            vec![Biquad::new(
                BiquadFilterType::Peak,
                1000.0,
                48000.0,
                1.0,
                0.0,
            )],
        )
        .into_boxed_plugin();
        for (id, value) in [
            ("band_0_order", order),
            ("topology", topology),
            ("oversampling", oversampling),
        ] {
            plugin
                .set_parameter(ParameterId::from(id), ParameterValue::Int(value))
                .unwrap();
        }
        plugin.initialize(48000).unwrap();
        let updates = [
            (
                ParameterId::from("band_0_freq"),
                ParameterValue::Float(1730.0),
            ),
            (ParameterId::from("band_0_gain"), ParameterValue::Float(7.0)),
            (ParameterId::from("band_0_q"), ParameterValue::Float(2.0)),
            (
                ParameterId::from("band_0_filter_type"),
                ParameterValue::Int(6),
            ),
        ];
        let input = [0.1; 128];
        let mut output = [0.0; 128];
        let context = ProcessContext::new(48000, 64);
        plugin.process(&input, &mut output, &context).unwrap();
        OPERATIONS.set(0);
        TRACKING.set(true);
        // Repeated updates exercise replacement of an unfinished transition.
        for _ in 0..2 {
            for (id, value) in &updates {
                plugin.set_parameter(id.clone(), value.clone()).unwrap();
                plugin.process(&input, &mut output, &context).unwrap();
            }
            // Finish the transition, recycle its buffers, then start another.
            for _ in 0..10 {
                plugin.process(&input, &mut output, &context).unwrap();
            }
        }
        plugin
            .set_parameter(updates[0].0.clone(), ParameterValue::Float(2300.0))
            .unwrap();
        plugin.reset();
        plugin
            .set_parameter(updates[1].0.clone(), ParameterValue::Float(-3.0))
            .unwrap();
        plugin.process(&input, &mut output, &context).unwrap();
        TRACKING.set(false);
        assert_eq!(
            OPERATIONS.get(),
            0,
            "order={order}, topology={topology}, oversampling={oversampling}"
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
}

#[test]
fn svf_automation_preserves_integrator_history() {
    let mut plugin = EqPlugin::new(
        1,
        vec![Biquad::new(
            BiquadFilterType::Lowpass,
            1000.0,
            48000.0,
            1.0,
            0.0,
        )],
    )
    .into_boxed_plugin();
    plugin
        .set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
        .unwrap();
    plugin.initialize(48000).unwrap();
    let context = ProcessContext::new(48000, 1);
    let mut output = [0.0];
    plugin.process(&[1.0], &mut output, &context).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("band_0_freq"),
            ParameterValue::Float(1200.0),
        )
        .unwrap();
    plugin.process(&[0.0], &mut output, &context).unwrap();
    assert!(
        output[0].abs() > 1e-3,
        "automation erased the stored impulse tail"
    );
}
