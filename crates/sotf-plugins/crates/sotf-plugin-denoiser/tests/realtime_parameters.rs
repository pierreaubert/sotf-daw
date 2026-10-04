//! Cold scalar automation and parameter-cache compatibility.

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
// SAFETY: all memory operations delegate unchanged to System. The thread-local
// counters neither allocate nor inspect the allocated memory.
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

use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};

#[test]
fn changed_scalar_controls_and_profile_triggers_are_allocation_free() {
    for low_latency in [false, true] {
        for multi_resolution in [false, true] {
            let mut plugin = DenoiserPlugin::from_params(
                2,
                DenoiserPluginParams {
                    low_latency,
                    multi_resolution,
                    ..Default::default()
                },
            );
            plugin.initialize(48_000.0).unwrap();
            let changes: Vec<_> = plugin
                .parameter_schema()
                .into_iter()
                .filter(|parameter| {
                    parameter.update_mode == UpdateMode::Realtime
                        || matches!(parameter.id.as_str(), "learn_noise" | "clear_profile")
                })
                .map(|parameter| {
                    let value = match parameter.default_value {
                        ParameterValue::Float(_) => {
                            let min = parameter.min_value.as_ref().unwrap().as_float().unwrap();
                            let max = parameter.max_value.as_ref().unwrap().as_float().unwrap();
                            ParameterValue::Float(min + (max - min) * 0.37)
                        }
                        ParameterValue::Int(_) => {
                            let min = parameter.min_value.as_ref().unwrap().as_int().unwrap();
                            let max = parameter.max_value.as_ref().unwrap().as_int().unwrap();
                            ParameterValue::Int(min + (max - min) / 3)
                        }
                        ParameterValue::Bool(value) => ParameterValue::Bool(!value),
                        _ => panic!("unexpected nonprimitive denoiser control"),
                    };
                    (parameter.id, value)
                })
                .collect();
            assert_eq!(changes.len(), 31);
            std::thread::spawn(move || {
                let mut audio = [0.0; 514];
                OPERATIONS.set(0);
                TRACKING.set(true);
                for repeat in 0..3 {
                    for (index, (id, value)) in changes.iter().enumerate() {
                        plugin.parametric_validate_parameter(id, value).unwrap();
                        plugin
                            .parametric_set_parameter(id.clone(), value.clone())
                            .unwrap();
                        let frames = [1, 17, 257][(index + repeat) % 3];
                        for (sample, value) in audio[..frames * 2].iter_mut().enumerate() {
                            *value = ((sample + index * 11) as f32 * 0.131).sin() * 0.1;
                        }
                        plugin
                            .process_in_place(
                                &mut audio[..frames * 2],
                                &ProcessContext::new(48_000, frames),
                            )
                            .unwrap();
                        assert!(audio[..frames * 2].iter().all(|value| value.is_finite()));
                    }
                }
                TRACKING.set(false);
                assert_eq!(OPERATIONS.get(), 0, "callback heap operations");
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn cold_shaped_curve_and_audition_callbacks_are_allocation_free() {
    for low_latency in [false, true] {
        for multi_resolution in [false, true] {
            let mut plugin = DenoiserPlugin::from_params(
                2,
                DenoiserPluginParams {
                    low_latency,
                    multi_resolution,
                    curve_low: 0.2,
                    curve_mid: 0.7,
                    curve_high: 0.4,
                    audition_residual: true,
                    ..Default::default()
                },
            );
            plugin.initialize(48_000.0).unwrap();
            std::thread::spawn(move || {
                let mut audio = vec![0.0; 4096 * 2];
                for (i, sample) in audio.iter_mut().enumerate() {
                    *sample = (i as f32 * 0.131).sin() * 0.1;
                }
                let mut tail = vec![0.0; 512 * 2];
                OPERATIONS.set(0);
                TRACKING.set(true);
                // Cold first process, irregular follow-ups, first drain,
                // reset, and reuse: no heap operations on the callback thread.
                for frames in [1, 257, 4096, 63] {
                    plugin
                        .process_in_place(
                            &mut audio[..frames * 2],
                            &ProcessContext::new(48_000, frames),
                        )
                        .unwrap();
                }
                let status = plugin
                    .drain(&mut tail, &ProcessContext::new(48_000, 512))
                    .unwrap();
                assert!(status.frames > 0);
                plugin.reset();
                plugin
                    .process_in_place(&mut audio[..1024 * 2], &ProcessContext::new(48_000, 1024))
                    .unwrap();
                TRACKING.set(false);
                assert_eq!(OPERATIONS.get(), 0, "cold callback heap operations");
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn scalar_triggers_and_structural_rejections_keep_snapshot_semantics() {
    let mut plugin = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    plugin.initialize(48_000.0).unwrap();
    let learn = ParameterId::from("learn_noise");
    let use_profile = ParameterId::from("use_captured_profile");
    let clear = ParameterId::from("clear_profile");
    plugin
        .parametric_set_parameter(learn.clone(), ParameterValue::Bool(true))
        .unwrap();
    assert_eq!(
        plugin.parametric_get_parameter(&learn),
        Some(ParameterValue::Bool(true))
    );
    plugin
        .parametric_set_parameter(use_profile.clone(), ParameterValue::Bool(true))
        .unwrap();
    plugin
        .parametric_set_parameter(clear.clone(), ParameterValue::Bool(true))
        .unwrap();
    for id in [&learn, &use_profile, &clear] {
        assert_eq!(
            plugin.parametric_get_parameter(id),
            Some(ParameterValue::Bool(false))
        );
    }
    // Scalar setters historically clamp finite inputs through param_bridge.
    let reduction = ParameterId::from("reduction_db");
    plugin
        .parametric_set_parameter(reduction.clone(), ParameterValue::Float(1.0e6))
        .unwrap();
    assert_eq!(
        plugin.parametric_get_parameter(&reduction),
        Some(ParameterValue::Float(40.0))
    );
    let before = plugin.current_values();
    for (id, value) in [
        ("low_latency", ParameterValue::Bool(true)),
        ("multi_resolution", ParameterValue::Bool(true)),
        ("reduction_db", ParameterValue::Float(f32::NAN)),
        ("floor_db", ParameterValue::Bool(false)),
        ("missing", ParameterValue::Float(0.0)),
    ] {
        assert!(
            plugin
                .parametric_set_parameter(ParameterId::from(id), value)
                .is_err()
        );
        assert_eq!(plugin.current_values(), before);
    }
}
