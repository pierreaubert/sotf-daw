//! Gain smoothing retains coefficients but cannot produce audio from silence.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext, TailLength};
use sotf_plugin_band_merge::BandMergePlugin;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[test]
fn warmed_program_and_unfinished_gain_mute_transitions_have_exact_zero_output() {
    for rate in [44_100, 48_000, 192_000] {
        for channels in [1, 2, 6] {
            for bands in 2..=8 {
                let mut plugin = BandMergePlugin::new(channels, bands).unwrap();
                plugin.initialize(rate).unwrap();
                let input: Vec<f32> = (0..1027 * channels * bands)
                    .map(|i| ((i * 37 % 127) as f32 - 63.0) / 256.0)
                    .collect();
                let mut output = vec![0.0; 1027 * channels];
                plugin
                    .process(&input, &mut output, &ProcessContext::new(rate, 1027))
                    .unwrap();
                assert!(output.iter().any(|sample| *sample != 0.0));
                for band in 0..bands {
                    plugin
                        .set_parameter(
                            ParameterId::from(format!("band_{band}_gain_db")),
                            ParameterValue::Float(if band % 2 == 0 { 24.0 } else { -60.0 }),
                        )
                        .unwrap();
                    plugin
                        .set_parameter(
                            ParameterId::from(format!("band_{band}_mute")),
                            ParameterValue::Bool(band % 2 == 0),
                        )
                        .unwrap();
                }
                // Arm the diagnostic as well: it observes the sum without an audio feedback path.
                plugin.get_parameter(&ParameterId::from("reconstruction_error_db"));
                for frames in [1, 7, 137, 8193] {
                    let input = vec![0.0; frames * channels * bands];
                    let mut output = vec![1234.0; frames * channels];
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(rate, frames))
                        .unwrap();
                    assert!(
                        output.iter().all(|sample| *sample == 0.0),
                        "rate={rate}, channels={channels}, bands={bands}, frames={frames}"
                    );
                }
                plugin.reset();
                let mut output = vec![1234.0; channels];
                plugin
                    .process(
                        &vec![0.0; channels * bands],
                        &mut output,
                        &ProcessContext::new(rate, 1),
                    )
                    .unwrap();
                assert!(output.iter().all(|sample| *sample == 0.0));
            }
        }
    }
}

#[test]
fn native_tail_metadata_matches_immediate_unfrozen_completion() {
    let mut plugin = BandMergePlugin::new(2, 3).unwrap();
    for initialize in [false, true] {
        if initialize {
            plugin.initialize(48_000).unwrap();
        }
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
        assert_eq!(plugin.drain_output_frames_max(), 0);
        plugin.begin_drain(&ProcessContext::new(48_000, 0)).unwrap();
        for _ in 0..2 {
            let mut canary = [1234.0; 8];
            let result = plugin
                .drain(&mut canary, &ProcessContext::new(48_000, 0))
                .unwrap();
            assert!(result.complete);
            assert_eq!(result.frames, 0);
            assert_eq!(canary, [1234.0; 8]);
        }
    }
    plugin
        .set_parameter(
            ParameterId::from("band_0_gain_db"),
            ParameterValue::Float(6.0),
        )
        .unwrap();
    plugin.reset();
    let mut output = [0.0; 2];
    plugin
        .process(&[0.125; 6], &mut output, &ProcessContext::new(48_000, 1))
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
fn cold_tail_queries_and_noop_completion_allocate_and_free_nothing() {
    for bands in [2, 8] {
        let mut plugin = BandMergePlugin::new(2, bands).unwrap();
        plugin.initialize(48_000).unwrap();
        std::thread::spawn(move || {
            let context = ProcessContext::new(48_000, 0);
            let mut output = [1234.0; 2];
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
            assert_eq!(output, [1234.0; 2]);
        })
        .join()
        .unwrap();
    }
}
