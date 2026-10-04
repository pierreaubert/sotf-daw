//! Detector history changes gains but cannot synthesize an audio tail.

// Rust guideline compliant 2026-02-21
use sotf_host::{
    ParameterId, ParameterValue, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, Plugin,
    ProcessContext, TailLength,
};
use sotf_plugin_transient_shaper::{TransientShaperPlugin, TransientShaperPluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[test]
fn warmed_envelopes_and_unfinished_controls_have_exact_zero_output() {
    for rate in [44_100, 48_000, 192_000] {
        for channels in [1, 2, 6] {
            for (attack, sustain) in [
                (100.0, 100.0),
                (-100.0, -100.0),
                (100.0, -100.0),
                (-100.0, 100.0),
            ] {
                for mix in [0.0, 0.37, 1.0] {
                    let mut plugin = TransientShaperPlugin::from_params(
                        channels,
                        TransientShaperPluginParams {
                            attack,
                            sustain,
                            sensitivity_db: -12.0,
                            output_gain_db: 12.0,
                            mix,
                        },
                    )
                    .unwrap();
                    plugin.initialize(f64::from(rate)).unwrap();
                    let mut input: Vec<f32> = (0..1027 * channels)
                        .map(|i| ((i * 37 % 127) as f32 - 63.0) / 32.0)
                        .collect();
                    plugin
                        .process_in_place(&mut input, &ProcessContext::new(rate, 1027))
                        .unwrap();
                    assert!(input.iter().any(|sample| *sample != 0.0));
                    assert!(input.iter().all(|sample| sample.is_finite()));
                    for (name, value) in [
                        ("attack", -attack),
                        ("sustain", -sustain),
                        ("sensitivity", 12.0),
                        ("output_gain", -12.0),
                        ("mix", 1.0 - mix),
                    ] {
                        plugin
                            .parametric_set_parameter(
                                ParameterId::from(name),
                                ParameterValue::Float(value),
                            )
                            .unwrap();
                    }
                    for frames in [1, 7, 137, 8193] {
                        let mut silence = vec![0.0; frames * channels];
                        plugin
                            .process_in_place(&mut silence, &ProcessContext::new(rate, frames))
                            .unwrap();
                        assert!(
                            silence.iter().all(|sample| *sample == 0.0),
                            "rate={rate}, channels={channels}, attack={attack}, sustain={sustain}, mix={mix}, frames={frames}"
                        );
                    }
                    plugin.reset();
                    let mut silence = vec![0.0; channels];
                    plugin
                        .process_in_place(&mut silence, &ProcessContext::new(rate, 1))
                        .unwrap();
                    assert!(silence.iter().all(|sample| *sample == 0.0));
                }
            }
        }
    }
}

#[test]
fn native_tail_metadata_and_public_adapter_preserve_unfrozen_completion() {
    let plugin = TransientShaperPlugin::new(2);
    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
    assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
    let mut plugin = ParametricInPlacePluginAdapter::new(plugin);
    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
    plugin.initialize(48_000.0).unwrap();
    let context = ProcessContext::new(48_000, 0);
    for _ in 0..2 {
        assert_eq!(plugin.drain_output_frames_max(), 0);
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
        plugin.begin_drain(&context).unwrap();
        let mut canary = [1234.0; 8];
        let result = plugin.drain(&mut canary, &context).unwrap();
        assert!(result.complete);
        assert_eq!(result.frames, 0);
        assert_eq!(canary, [1234.0; 8]);
    }
    plugin
        .set_parameter(ParameterId::from("attack"), ParameterValue::Float(100.0))
        .unwrap();
    plugin.reset();
    let mut output = [0.0; 2];
    plugin
        .process(&[0.125; 2], &mut output, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert!(output.iter().all(|sample| *sample > 0.0));
    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
}

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;
// SAFETY: All allocator operations forward their original contracts to System.
// Constant thread-local counters own no heap storage.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACK.try_with(|track| {
            if track.get() {
                ALLOCS.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: Forward the original allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACK.try_with(|track| {
            if track.get() {
                FREES.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: Forward the original pointer/layout pair.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn cold_adapter_tail_queries_and_noop_completion_allocate_and_free_nothing() {
    for channels in [1, 2, 6] {
        let mut plugin = ParametricInPlacePluginAdapter::new(TransientShaperPlugin::new(channels));
        plugin.initialize(48_000.0).unwrap();
        std::thread::spawn(move || {
            let context = ProcessContext::new(48_000, 0);
            let mut output = [1234.0; 6];
            ALLOCS.set(0);
            FREES.set(0);
            TRACK.set(true);
            for _ in 0..2 {
                assert_eq!(plugin.tail_length(), TailLength::Finite(0));
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                assert_eq!(plugin.drain_output_frames_max(), 0);
                plugin.begin_drain(&context).unwrap();
                assert!(plugin.drain(&mut output, &context).unwrap().complete);
                plugin.reset();
            }
            TRACK.set(false);
            assert_eq!((ALLOCS.get(), FREES.get()), (0, 0));
            assert_eq!(output, [1234.0; 6]);
        })
        .join()
        .unwrap();
    }
}
