//! Raw Gate kernels independently validate adapter partitioning and scratch reuse.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_gate::{GateMode, GatePlugin, GatePluginParams};
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

fn input(frames: usize, phase: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let program = ((frame + phase) % 29) as f32 / 100.0 - 0.13;
            let key = if (frame + phase) % 701 < 330 {
                0.23
            } else {
                0.0001
            };
            [program, -program * 0.5, key, -key * 0.7]
        })
        .collect()
}
#[test]
fn all_gate_modes_match_unsplit_kernel_across_structure_and_live_controls() {
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        for linked in [false, true] {
            for rms in [false, true] {
                for lookahead in [0.0, 3.7] {
                    let settings = GatePluginParams {
                        mode,
                        sidechain_external: true,
                        link_channels: linked,
                        detection_mode: if rms { "RMS".into() } else { "Peak".into() },
                        sidechain_hpf_hz: 85.0,
                        lookahead_ms: lookahead,
                        threshold_db: -26.0,
                        hold_ms: 2.7,
                        release_ms: 29.0,
                        knee_db: 4.0,
                        hysteresis_db: 2.0,
                        ..GatePluginParams::default()
                    };
                    let mut raw = GatePlugin::try_from_params(2, settings.clone()).unwrap();
                    raw.initialize(48_000).unwrap();
                    let mut adapter = plugins_bridge::create_plugin(
                        "Gate",
                        2,
                        48_000,
                        &serde_json::to_string(&settings).unwrap(),
                    )
                    .unwrap();
                    adapter.initialize(48_000).unwrap();
                    for lifecycle in 0..3 {
                        if lifecycle == 1 {
                            raw.reset();
                            adapter.reset();
                        } else if lifecycle == 2 {
                            raw.initialize(48_000).unwrap();
                            adapter.initialize(48_000).unwrap();
                        }
                        for (phase, frames) in
                            [1, 17, 255, 256, 257, 8192, 17003].into_iter().enumerate()
                        {
                            let threshold = ParameterValue::Float(-26.0 + phase as f32);
                            raw.parametric_set_parameter(
                                ParameterId::from("threshold"),
                                threshold.clone(),
                            )
                            .unwrap();
                            adapter
                                .set_parameter(ParameterId::from("threshold"), threshold)
                                .unwrap();
                            let input = input(frames, phase);
                            let mut reference = input.clone();
                            raw.process_in_place(
                                &mut reference,
                                &ProcessContext::new(48_000, frames),
                            )
                            .unwrap();
                            let mut output = vec![f32::NAN; frames * 2];
                            adapter
                                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
                                .unwrap();
                            for (actual, expected) in output
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .zip(reference.as_chunks::<4>().0)
                            {
                                assert_eq!(
                                    actual,
                                    &expected[..2],
                                    "{mode:?} linked={linked} rms={rms} lookahead={lookahead} frames={frames}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn real_gate_cold_f32_fallback_f64_and_reset_have_no_heap_traffic() {
    for mode in ["Downward", "Upward", "Duck"] {
        for double in [false, true] {
            let config =
                serde_json::json!({"mode":mode,"sidechain_external":true,"lookahead_ms":3.7,
            "detection_mode":"RMS","sidechain_hpf_hz":85.0})
                .to_string();
            let mut plugin = plugins_bridge::create_plugin("Gate", 2, 48_000, &config).unwrap();
            plugin.initialize(48_000).unwrap();
            let input32 = input(17003, 0);
            let input64: Vec<_> = input32.iter().map(|x| f64::from(*x)).collect();
            let mut output32 = vec![0.0; 34006];
            let mut output64 = vec![0.0; 34006];
            std::thread::spawn(move || {
                without_heap(|| {
                    if double {
                        plugin
                            .process_f64(
                                &input64,
                                &mut output64,
                                &ProcessContext::new(48_000, 17003),
                            )
                            .unwrap();
                    } else {
                        plugin
                            .process(&input32, &mut output32, &ProcessContext::new(48_000, 17003))
                            .unwrap();
                    }
                    plugin.reset();
                });
            })
            .join()
            .unwrap();
        }
    }
}
#[test]
fn rejected_late_gate_key_and_wrong_rate_preserve_the_next_waveform() {
    for double in [false, true] {
        let config = r#"{"mode":"Upward","sidechain_external":true,"lookahead_ms":2.0}"#;
        let mut tested = plugins_bridge::create_plugin("Gate", 2, 48_000, config).unwrap();
        let mut reference = plugins_bridge::create_plugin("Gate", 2, 48_000, config).unwrap();
        tested.initialize(48_000).unwrap();
        reference.initialize(48_000).unwrap();
        let input = input(9001, 0);
        let mut invalid = input.clone();
        *invalid.last_mut().unwrap() = f32::NAN;
        if double {
            let input: Vec<_> = input.iter().map(|x| f64::from(*x)).collect();
            let invalid: Vec<_> = invalid.iter().map(|x| f64::from(*x)).collect();
            let mut output = vec![99.0; 18002];
            let mut expected = vec![0.0; 18002];
            assert!(
                tested
                    .process_f64(&invalid, &mut output, &ProcessContext::new(48_000, 9001))
                    .is_err()
            );
            assert!(
                tested
                    .process_f64(&input, &mut output, &ProcessContext::new(44_100, 9001))
                    .is_err()
            );
            assert!(output.iter().all(|x| *x == 99.0));
            tested
                .process_f64(&input, &mut output, &ProcessContext::new(48_000, 9001))
                .unwrap();
            reference
                .process_f64(&input, &mut expected, &ProcessContext::new(48_000, 9001))
                .unwrap();
            assert_eq!(output, expected);
        } else {
            let mut output = vec![99.0; 18002];
            let mut expected = vec![0.0; 18002];
            assert!(
                tested
                    .process(&invalid, &mut output, &ProcessContext::new(48_000, 9001))
                    .is_err()
            );
            assert!(
                tested
                    .process(&input, &mut output, &ProcessContext::new(44_100, 9001))
                    .is_err()
            );
            assert!(output.iter().all(|x| *x == 99.0));
            tested
                .process(&input, &mut output, &ProcessContext::new(48_000, 9001))
                .unwrap();
            reference
                .process(&input, &mut expected, &ProcessContext::new(48_000, 9001))
                .unwrap();
            assert_eq!(output, expected);
        }
    }
}
