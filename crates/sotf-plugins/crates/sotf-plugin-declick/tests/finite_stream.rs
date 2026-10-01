//! Independent finite-stream and lookahead-support checks.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_declick::{DeclickPlugin, DeclickPluginParams};

const DELAY: usize = 8;

fn plugin(
    channels: usize,
    rate: u32,
    enabled: bool,
    sensitivity: f32,
    linked: bool,
) -> DeclickPlugin {
    DeclickPlugin::from_params(
        channels,
        rate,
        DeclickPluginParams {
            enabled,
            sensitivity,
            link_channels: linked,
            ..Default::default()
        },
    )
    .unwrap()
}

fn process(
    plugin: &mut DeclickPlugin,
    signal: &[f32],
    channels: usize,
    rate: u32,
    blocks: &[usize],
) -> Vec<f32> {
    let mut result = signal.to_vec();
    let mut frame = 0;
    let mut block = 0;
    while frame < signal.len() / channels {
        let frames = blocks[block % blocks.len()].min(signal.len() / channels - frame);
        plugin
            .process_in_place(
                &mut result[frame * channels..(frame + frames) * channels],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        frame += frames;
        block += 1;
    }
    result
}

fn drain(plugin: &mut DeclickPlugin, channels: usize, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let mut result = Vec::new();
    for call in 0..32 {
        let capacity = capacities[call % capacities.len()];
        let mut output = vec![1234.0; capacity * channels];
        let status = plugin
            .drain(&mut output, &ProcessContext::new(rate, capacity))
            .unwrap();
        assert!(status.frames <= capacity);
        assert!(
            output[status.frames * channels..]
                .iter()
                .all(|&value| value == 1234.0)
        );
        result.extend_from_slice(&output[..status.frames * channels]);
        if status.complete {
            return result;
        }
    }
    panic!("bounded drain failed to finish");
}

#[test]
fn final_disabled_marker_is_preserved_by_public_drain() {
    for rate in [8_000, 48_000, 192_000] {
        for channels in [1, 2, 3, 6] {
            for frames in [1, 7, 8, 9, 16, 17, 31, 129] {
                let mut input = vec![0.0; frames * channels];
                for channel in 0..channels {
                    input[(frames - 1) * channels + channel] = 0.25 / (channel + 1) as f32;
                }
                let mut instance = plugin(channels, rate, false, 10.0, true);
                let mut actual = process(&mut instance, &input, channels, rate, &[1, 7, 13]);
                actual.extend(drain(&mut instance, channels, rate, &[1, 3, 64]));
                let mut expected = vec![0.0; DELAY * channels];
                expected.extend(input);
                assert_eq!(
                    actual, expected,
                    "rate={rate} channels={channels} frames={frames}"
                );
            }
        }
    }
}

#[test]
fn enabled_repair_cannot_synthesize_audio_beyond_eight_frames_of_zero_continuation() {
    let mut cases = 0;
    for rate in [8_000, 48_000, 192_000] {
        for channels in [1, 2, 3] {
            for sensitivity in [1.0, 10.0, 100.0] {
                for linked in [false, true] {
                    for frames in [1, 7, 8, 9, 16, 17, 31, 32, 33, 127] {
                        for pattern in 0..8 {
                            let mut input = vec![0.0; (frames + 64) * channels];
                            let mut random = 0x7adf_9913_u32;
                            for frame in 0..frames {
                                random ^= random << 13;
                                random ^= random >> 17;
                                random ^= random << 5;
                                for channel in 0..channels {
                                    let value = match pattern {
                                        0 => 1.0,
                                        1 => {
                                            if frame % 2 == 0 {
                                                -1.0
                                            } else {
                                                1.0
                                            }
                                        }
                                        2 => {
                                            if frame + 1 == frames {
                                                10.0
                                            } else {
                                                0.01
                                            }
                                        }
                                        3 => {
                                            if frame + 6 >= frames {
                                                -10.0
                                            } else {
                                                0.01
                                            }
                                        }
                                        4 => {
                                            if frame + 7 >= frames {
                                                10.0
                                            } else {
                                                -0.01
                                            }
                                        }
                                        5 => (frame as f32 / frames as f32) * 2.0 - 1.0,
                                        6 => (random as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32,
                                        _ => {
                                            if frame % 3 == 0 {
                                                1.0e-18
                                            } else {
                                                1.0e-24
                                            }
                                        }
                                    };
                                    input[frame * channels + channel] =
                                        value / (channel + 1) as f32;
                                }
                            }
                            let mut instance = plugin(channels, rate, true, sensitivity, linked);
                            let output =
                                process(&mut instance, &input, channels, rate, &[1, 7, 29]);
                            assert!(
                                output[(frames + DELAY) * channels..]
                                    .iter()
                                    .all(|&v| v == 0.0),
                                "rate={rate} channels={channels} sensitivity={sensitivity} linked={linked} frames={frames} pattern={pattern}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 4320);
}

#[test]
fn repaired_edge_matches_separate_zero_padded_processing() {
    for channels in [1, 2, 3, 6] {
        for frames in [1, 7, 8, 9, 17, 31, 127] {
            for linked in [false, true] {
                let rate = 48_000;
                let input: Vec<_> = (0..frames * channels)
                    .map(|i| {
                        if i / channels + 3 >= frames {
                            4.0 / (i % channels + 1) as f32
                        } else {
                            (i as f32 * 0.31).sin() * 0.1
                        }
                    })
                    .collect();
                let mut padded = input.clone();
                padded.resize((frames + 64) * channels, 0.0);
                let expected = process(
                    &mut plugin(channels, rate, true, 1.0, linked),
                    &padded,
                    channels,
                    rate,
                    &[13, 1, 9],
                );
                let mut instance = plugin(channels, rate, true, 1.0, linked);
                let mut actual = process(&mut instance, &input, channels, rate, &[1, 7, 29]);
                actual.extend(drain(&mut instance, channels, rate, &[1, 19, 2]));
                assert_eq!(actual, expected[..(frames + DELAY) * channels]);
            }
        }
    }
}

#[test]
fn drain_errors_controls_reset_and_empty_stream_are_transactional() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    let rate = 48_000;
    let channels = 2;
    let input = vec![0.25; 19 * channels];
    let mut p = plugin(channels, rate, true, 1.0, true);
    assert_eq!(p.tail_length(), TailLength::Finite(8));
    assert!(
        p.drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap()
            .complete
    );
    let mut reference = plugin(channels, rate, true, 1.0, true);
    assert_eq!(
        process(&mut p, &input, channels, rate, &[7]),
        process(&mut reference, &input, channels, rate, &[7])
    );
    let mut sentinel = [1234.0; 3];
    assert!(
        p.drain(&mut sentinel, &ProcessContext::new(rate, 1))
            .is_err()
    );
    assert!(p.drain(&mut [], &ProcessContext::new(rate, 0)).is_err());
    assert!(
        p.drain(&mut sentinel[..2], &ProcessContext::new(96_000, 1))
            .is_err()
    );
    assert_eq!(sentinel, [1234.0; 3]);
    assert!(p.initialize(0).is_err());
    let mut first = [0.0; 2];
    let status = p.drain(&mut first, &ProcessContext::new(rate, 1)).unwrap();
    assert!(!status.complete);
    assert!(
        p.parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
            .is_err()
    );
    assert!(p.apply_values(Default::default()).is_err());
    let mut rejected = [0.5; 2];
    assert!(
        p.process_in_place(&mut rejected, &ProcessContext::new(rate, 1))
            .is_err()
    );
    assert_eq!(rejected, [0.5; 2]);
    assert_eq!(
        p.process_in_place(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        0
    );
    assert!(p.drain(&mut [], &ProcessContext::new(rate, 0)).is_err());
    let mut actual = first.to_vec();
    actual.extend(drain(&mut p, channels, rate, &[1, 64]));
    assert_eq!(actual, drain(&mut reference, channels, rate, &[64]));
    assert!(
        p.drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap()
            .complete
    );
    p.reset();
    assert_eq!(
        process(&mut p, &input, channels, rate, &[1, 7]),
        process(
            &mut plugin(channels, rate, true, 1.0, true),
            &input,
            channels,
            rate,
            &[1, 7]
        )
    );
    p.initialize(96_000).unwrap();
    assert_eq!(
        process(&mut p, &input, channels, 96_000, &[1, 7]),
        process(
            &mut plugin(channels, 96_000, true, 1.0, true),
            &input,
            channels,
            96_000,
            &[1, 7]
        )
    );
}

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
struct CallbackAllocator;
// SAFETY: All allocation contracts are forwarded unchanged to System; the
// tracking cells have constant initializers and perform no allocation.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCS.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: Forward the caller's original allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                FREES.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: Pointer/layout are the unchanged original allocation pair.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn counted(action: impl FnOnce()) -> (usize, usize) {
    ALLOCS.with(|n| n.set(0));
    FREES.with(|n| n.set(0));
    TRACKING.with(|tracking| tracking.set(true));
    action();
    TRACKING.with(|tracking| tracking.set(false));
    (ALLOCS.with(Cell::get), FREES.with(Cell::get))
}

#[test]
fn cold_process_drain_and_reset_allocate_and_free_nothing() {
    // Legacy plus every owned topology: periodic tracking (with its bounded
    // autocorrelation recompute), multiband crossover/supervisor, widened
    // latency, and the residual tap.
    let topologies: Vec<(&str, DeclickPluginParams)> = vec![
        (
            "legacy",
            DeclickPluginParams {
                sensitivity: 1.0,
                ..Default::default()
            },
        ),
        (
            "periodic",
            DeclickPluginParams {
                mode: 1,
                sensitivity: 1.0,
                ..Default::default()
            },
        ),
        (
            "multiband-skewed",
            DeclickPluginParams {
                bands: 2,
                crossover_hz: 4000.0,
                frequency_skew: 0.75,
                sensitivity: 1.0,
                ..Default::default()
            },
        ),
        (
            "widened",
            DeclickPluginParams {
                bands: 1,
                repair_width: 8,
                sensitivity: 1.0,
                ..Default::default()
            },
        ),
        (
            "all-modes",
            DeclickPluginParams {
                mode: 1,
                bands: 2,
                crossover_hz: 8000.0,
                frequency_skew: -0.5,
                repair_width: 8,
                audition_residual: true,
                sensitivity: 1.0,
                ..Default::default()
            },
        ),
    ];
    for channels in [1, 2, 6] {
        for enabled in [false, true] {
            for (name, template) in &topologies {
                let mut params = template.clone();
                params.enabled = enabled;
                let p = DeclickPlugin::from_params(channels, 48_000, params).unwrap();
                // Owned label: `thread::spawn` requires a 'static closure.
                let label = (*name).to_owned();
                std::thread::spawn(move || {
                    let mut p = p;
                    let mut input = vec![0.25; 33 * channels];
                    let mut output = vec![0.0; 3 * channels];
                    let counts = counted(|| {
                        for _ in 0..2 {
                            p.process_in_place(&mut input, &ProcessContext::new(48_000, 33))
                                .unwrap();
                            while !p
                                .drain(&mut output, &ProcessContext::new(48_000, 3))
                                .unwrap()
                                .complete
                            {}
                            assert!(
                                p.drain(&mut output, &ProcessContext::new(48_000, 3))
                                    .unwrap()
                                    .complete
                            );
                            p.reset();
                            input.fill(0.25);
                        }
                    });
                    assert_eq!(
                        counts,
                        (0, 0),
                        "{label} channels={channels} enabled={enabled}"
                    );
                })
                .join()
                .unwrap();
            }
        }
    }
}
