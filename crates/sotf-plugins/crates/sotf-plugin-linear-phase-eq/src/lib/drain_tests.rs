//! Direct finite convolution and lifecycle checks for the prepared FIR EQ.

use crate::{BandConfig, LinearPhaseEqPlugin, LinearPhaseEqPluginParams};
use sotf_host::plugin::{PluginDrainResult, TailLength};
use sotf_host::{
    ParameterId, ParameterSet, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountAlloc;

// SAFETY: Every request is forwarded unchanged to the system allocator. The
// const thread-local counters do not allocate or expose the returned pointers.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a + 1, d));
            });
        }
        // SAFETY: Forward the caller's valid layout without modification.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a, d + 1));
            });
        }
        // SAFETY: Pointer and layout belong to the matching system allocation.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

/// Run `f` on the calling thread with allocation/free counting enabled.
///
/// Returns the observed `(allocations, frees)` alongside `f`'s value.
/// Shared with the dynamic-update lifecycle tests.
pub(crate) fn count_allocs<R>(f: impl FnOnce() -> R) -> ((usize, usize), R) {
    COUNTS.set((0, 0));
    COUNTING.set(true);
    let value = f();
    COUNTING.set(false);
    (COUNTS.get(), value)
}

fn make(channels: usize, rate: u32, length: usize, phase: usize, mix: f32) -> LinearPhaseEqPlugin {
    LinearPhaseEqPlugin::from_params(
        channels,
        rate,
        LinearPhaseEqPluginParams {
            num_filters: 1,
            fir_length_index: length,
            phase_mode_index: phase,
            auto_gain: length % 2 == 1,
            mix,
            filters: vec![BandConfig {
                filter_type: if length.is_multiple_of(2) {
                    "Peak"
                } else {
                    "Highpass"
                }
                .into(),
                frequency: 1300.0,
                q: 0.8,
                gain_db: 8.0,
                active: true,
                placement: None,
            }],
            stereo_pairs: None,
        },
    )
    .unwrap()
}

fn process(plugin: &mut LinearPhaseEqPlugin, input: &[f32], rate: u32) -> Vec<f32> {
    let mut result = input.to_vec();
    let channels = plugin.channels();
    let frames = input.len() / channels;
    let mut position = 0;
    for block in [1, 31, 127, 7, 1031].into_iter().cycle() {
        let count = block.min(frames - position);
        if count == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process_in_place(
                    &mut result[position * channels..(position + count) * channels],
                    &ProcessContext::new(rate, count)
                )
                .unwrap(),
            count
        );
        position += count;
    }
    result
}

fn drain(plugin: &mut LinearPhaseEqPlugin, rate: u32, capacity: usize) -> Vec<f32> {
    let channels = plugin.channels();
    let mut result = Vec::new();
    for _ in 0..20000 {
        let mut output = vec![123.0; capacity * channels + 3];
        let drained = plugin
            .drain(
                &mut output[..capacity * channels],
                &ProcessContext::new(rate, 0),
            )
            .unwrap();
        assert!(drained.frames <= capacity && drained.frames <= plugin.drain_output_frames_max());
        assert!(
            output[drained.frames * channels..]
                .iter()
                .all(|&x| x == 123.0)
        );
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            return result;
        }
    }
    panic!("FIR EQ drain did not finish");
}

#[test]
fn exact_finite_output_matches_independent_direct_convolution() {
    for length in 0..4 {
        for phase in 0..2 {
            for rate in [32000, 96000] {
                for channels in [1, 3] {
                    for mix in [0.0, 0.375, 1.0] {
                        let mut plugin = make(channels, rate, length, phase, mix);
                        let coefficients = plugin.fir_coeffs.clone();
                        let support = coefficients.len() - 1 + 32;
                        let delay = if phase == 0 {
                            coefficients.len() / 2 + 32
                        } else {
                            32
                        };
                        assert_eq!(plugin.tail_length(), TailLength::Finite(support as u64));
                        let frames = 33 + length * 17;
                        let input: Vec<f32> = (0..frames * channels)
                            .map(|i| {
                                if i / channels == frames - 1 {
                                    (i % channels + 1) as f32 * 0.25
                                } else {
                                    ((i * 13 % 23) as f32 - 11.0) / 64.0
                                }
                            })
                            .collect();
                        let mut actual = process(&mut plugin, &input, rate);
                        actual.extend(drain(&mut plugin, rate, [1, 17, 4097][length % 3]));
                        assert_eq!(actual.len(), (frames + support) * channels);
                        // Direct f64 convolution of coefficient data is independent
                        // of NUPC partition queues, FFTs and the drain implementation.
                        let mut expected = vec![0.0f64; actual.len()];
                        for frame in 0..frames {
                            for channel in 0..channels {
                                let value = f64::from(input[frame * channels + channel]);
                                expected[(frame + delay) * channels + channel] +=
                                    value * f64::from(1.0 - mix);
                                for (tap, &coefficient) in coefficients.iter().enumerate() {
                                    expected[(frame + 32 + tap) * channels + channel] +=
                                        value * f64::from(coefficient) * f64::from(mix);
                                }
                            }
                        }
                        for (index, (&actual, &expected)) in
                            actual.iter().zip(&expected).enumerate()
                        {
                            assert!(
                                (f64::from(actual) - expected).abs() < 2e-6,
                                "length{length}/phase{phase}/{rate}/{channels}/mix{mix} sample{index}: {actual} vs {expected}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn invalid_drain_is_transactional_and_reset_or_reinitialize_replays() {
    let mut plugin = make(3, 48000, 0, 1, 0.375);
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(48000, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    let input = [0.25, -0.5, 0.125];
    let prefix = process(&mut plugin, &input, 48000);
    let mut canary = [123.0; 7];
    for capacity in [0, 2, 7] {
        assert!(
            plugin
                .drain(&mut canary[..capacity], &ProcessContext::new(48000, 0))
                .is_err()
        );
        assert_eq!(canary, [123.0; 7]);
    }
    assert!(
        plugin
            .drain(&mut canary[..6], &ProcessContext::new(44100, 0))
            .is_err()
    );
    let tail = drain(&mut plugin, 48000, 13);
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(48000, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    assert!(
        plugin
            .process_in_place(&mut canary[..3], &ProcessContext::new(48000, 1))
            .is_err()
    );
    let mut values = ParameterSet::new();
    values.insert(ParameterId::from("mix"), ParameterValue::Float(1.0));
    assert!(plugin.apply_values(values).is_err());
    assert_eq!(plugin.mix_value, 0.375);
    for initialize in [false, true] {
        if initialize {
            plugin.initialize(48000).unwrap();
        } else {
            plugin.reset();
        }
        assert_eq!(process(&mut plugin, &input, 48000), prefix);
        assert_eq!(drain(&mut plugin, 48000, 1025), tail);
    }
}

#[test]
fn pending_mix_ramp_matches_ordinary_zero_continuation() {
    for phase in 0..2 {
        let mut candidate = make(2, 48000, 1, phase, 0.0);
        let mut reference = make(2, 48000, 1, phase, 0.0);
        for plugin in [&mut candidate, &mut reference] {
            let mut values = ParameterSet::new();
            values.insert(ParameterId::from("mix"), ParameterValue::Float(1.0));
            plugin.apply_values(values).unwrap();
        }
        let input: Vec<f32> = (0..137 * 2)
            .map(|i| (i as f32 * 0.271).sin() * 0.25)
            .collect();
        assert_eq!(
            process(&mut candidate, &input, 48000),
            process(&mut reference, &input, 48000)
        );
        let support = 32 + candidate.fir_coeffs.len() - 1;
        let tail = drain(&mut candidate, 48000, 1);
        let zeros = vec![0.0; (support + 4096) * 2];
        let expected = process(&mut reference, &zeros, 48000);
        assert_eq!(tail, expected[..support * 2]);
        assert!(expected[support * 2..].iter().all(|x| x.abs() < 1e-7));
    }
}

#[test]
fn ordinary_same_rate_initialize_keeps_history_and_rate_change_clears_it() {
    let mut candidate = make(2, 48000, 0, 0, 0.375);
    let mut reference = make(2, 48000, 0, 0, 0.375);
    assert_eq!(
        process(&mut candidate, &[0.25, -0.5], 48000),
        process(&mut reference, &[0.25, -0.5], 48000)
    );
    candidate.initialize(48000).unwrap();
    let zeros = vec![0.0; 1100 * 2];
    assert_eq!(
        process(&mut candidate, &zeros, 48000),
        process(&mut reference, &zeros, 48000)
    );
    process(&mut candidate, &[0.5, -0.25], 48000);
    candidate.initialize(44100).unwrap();
    let mut fresh = make(2, 44100, 0, 0, 0.375);
    assert_eq!(
        process(&mut candidate, &zeros, 44100),
        process(&mut fresh, &zeros, 44100)
    );
    assert_eq!(
        drain(&mut candidate, 44100, 17),
        drain(&mut fresh, 44100, 257)
    );
}

#[test]
fn cold_processing_drain_and_reset_do_not_allocate_or_free() {
    for length in 0..4 {
        for phase in 0..2 {
            for channels in [1, 3] {
                for cold_drain in [false, true] {
                    let mut plugin = make(channels, 48000, length, phase, 0.375);
                    let mut input = vec![0.25; 17003 * channels];
                    let mut output = vec![123.0; 17 * channels];
                    if cold_drain {
                        plugin
                            .process_in_place(&mut input, &ProcessContext::new(48000, 17003))
                            .unwrap();
                    }
                    let counts = std::thread::spawn(move || {
                        COUNTING.set(true);
                        if !cold_drain {
                            plugin
                                .process_in_place(&mut input, &ProcessContext::new(48000, 17003))
                                .unwrap();
                        }
                        loop {
                            if plugin
                                .drain(&mut output, &ProcessContext::new(48000, 0))
                                .unwrap()
                                .complete
                            {
                                break;
                            }
                        }
                        plugin.reset();
                        COUNTING.set(false);
                        COUNTS.get()
                    })
                    .join()
                    .unwrap();
                    assert_eq!(
                        counts,
                        (0, 0),
                        "length{length}/phase{phase}/channels{channels}/cold_drain{cold_drain}"
                    );
                }
            }
        }
    }
}
