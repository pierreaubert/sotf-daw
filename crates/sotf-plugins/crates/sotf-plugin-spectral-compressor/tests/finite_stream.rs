//! Finite-stream tests with independent delay and periodic-Hann WOLA oracles.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_spectral_compressor::{SpectralCompressorPlugin, SpectralCompressorPluginParams};

const RATE: u32 = 48_000;
const SIZES: [usize; 3] = [1024, 2048, 4096];

fn plugin(channels: usize, index: usize, mix: f32) -> SpectralCompressorPlugin {
    let mut p = SpectralCompressorPlugin::from_params(
        channels,
        SpectralCompressorPluginParams {
            fft_size_index: index,
            ratio: 1.0,
            mix,
            ..Default::default()
        },
    );
    p.initialize(RATE).unwrap();
    p
}

fn tail(n: usize, frames: usize) -> usize {
    if frames == 0 {
        0
    } else {
        2 * n + ((frames - 1) / (n / 4)) * (n / 4) - frames
    }
}

fn process(
    p: &mut SpectralCompressorPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let mut out = input.to_vec();
    let mut position = 0;
    let mut block = 0;
    while position < input.len() / channels {
        let frames = blocks[block % blocks.len()].min(input.len() / channels - position);
        p.process_in_place(
            &mut out[position * channels..(position + frames) * channels],
            &ProcessContext::new(RATE, frames),
        )
        .unwrap();
        position += frames;
        block += 1;
    }
    out
}

fn drain(p: &mut SpectralCompressorPlugin, channels: usize, capacities: &[usize]) -> Vec<f32> {
    let mut result = Vec::new();
    for call in 0..20_000 {
        let capacity = capacities[call % capacities.len()];
        let mut block = vec![1234.0; capacity * channels];
        let status = p
            .drain(&mut block, &ProcessContext::new(RATE, capacity))
            .unwrap();
        assert!(status.frames <= capacity);
        assert!(
            block[status.frames * channels..]
                .iter()
                .all(|&v| v == 1234.0)
        );
        result.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            return result;
        }
    }
    panic!("finite STFT drain did not complete");
}

#[test]
fn dry_final_markers_follow_exact_reported_delay_and_derived_eof_bound() {
    for (index, n) in SIZES.into_iter().enumerate() {
        let hop = n / 4;
        for channels in [1, 2, 3] {
            for frames in [1, hop - 1, hop, hop + 1, n - 1, n, n + 1] {
                for capacities in [&[1][..], &[13, 1, 4096][..], &[8192][..]] {
                    let mut input = vec![0.0; frames * channels];
                    for ch in 0..channels {
                        input[(frames - 1) * channels + ch] = 0.25 / (ch + 1) as f32;
                    }
                    let mut p = plugin(channels, index, 0.0);
                    assert_eq!(p.latency_samples(), n);
                    let mut actual = process(&mut p, &input, channels, &[1, 7, 113]);
                    let suffix = drain(&mut p, channels, capacities);
                    assert_eq!(suffix.len(), tail(n, frames) * channels);
                    actual.extend(suffix);
                    let mut expected = vec![0.0; actual.len()];
                    expected[n * channels..(n + frames) * channels].copy_from_slice(&input);
                    assert_eq!(
                        actual, expected,
                        "N={n} channels={channels} frames={frames}"
                    );
                }
            }
        }
    }
}

#[test]
fn unity_wet_response_reconstructs_every_initial_and_final_sample() {
    // At unity spectral gain the DFT/inverse-DFT pair is identity. Consequently
    // exact WOLA is source[n] × sum(window[n-jH]^2)/1.5, delayed by N.
    // Four periodic Hann-square windows at H=N/4 sum to exactly 1.5,
    // including the negative-time windows at startup and padded final windows.
    for (index, n) in SIZES.into_iter().enumerate() {
        let hop = n / 4;
        for channels in [1, 2, 3] {
            // Cross two complete OLA ring revolutions as well as both ends.
            let frames = 9 * n + hop + 17;
            let input: Vec<_> = (0..frames * channels)
                .map(|i| 0.2 + 0.15 * (i as f64 * 0.719).sin() as f32)
                .collect();
            let mut p = plugin(channels, index, 1.0);
            let mut actual = process(&mut p, &input, channels, &[7, 1, 313]);
            actual.extend(drain(&mut p, channels, &[1, 11, 8192]));
            assert_eq!(actual.len(), (frames + tail(n, frames)) * channels);
            let mut expected = vec![0.0_f64; actual.len()];
            for frame in 0..frames {
                for ch in 0..channels {
                    expected[(n + frame) * channels + ch] = f64::from(input[frame * channels + ch]);
                }
            }
            for (sample, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    (f64::from(actual) - expected).abs() < 2.0e-6,
                    "N={n} channels={channels} sample={sample}: {actual} vs {expected}"
                );
            }
        }
    }
}

#[test]
fn first_and_final_impulses_survive_every_phase_of_the_initial_window() {
    for (index, n) in SIZES.into_iter().enumerate() {
        let mut p = plugin(1, index, 1.0);
        for phase in 0..n {
            p.reset();
            let frames = phase + 1;
            let mut input = vec![0.0; frames];
            input[0] = 0.25;
            input[phase] -= 0.5;
            let blocks = if phase % 2 == 0 {
                &[1, 7, 113][..]
            } else {
                &[8192][..]
            };
            let mut actual = process(&mut p, &input, 1, blocks);
            actual.extend(drain(&mut p, 1, &[7, n / 4 + 1, 8192]));
            assert_eq!(actual.len(), frames + tail(n, frames));
            for (position, value) in actual.iter().enumerate() {
                let expected = position
                    .checked_sub(n)
                    .and_then(|i| input.get(i))
                    .copied()
                    .unwrap_or(0.0);
                assert!(
                    (value - expected).abs() < 2.0e-6,
                    "N={n} phase={phase} position={position}: {value} vs {expected}"
                );
            }
        }
    }
}

#[test]
fn nonlinear_adaptive_and_delta_tails_match_separate_zero_continuation() {
    for (index, n) in SIZES.into_iter().enumerate() {
        for target in 0..3 {
            for delta in [false, true] {
                let channels = 2;
                let frames = n + n / 4 + 13;
                let input: Vec<_> = (0..frames * channels)
                    .map(|i| ((i as f32 * 0.31).sin() + (i as f32 * 0.091).cos()) * 0.3)
                    .collect();
                let params = SpectralCompressorPluginParams {
                    fft_size_index: index,
                    target_mode: target,
                    ratio: 6.0,
                    threshold_db: -30.0,
                    mix: 0.7,
                    adaptive_threshold: true,
                    delta_listen: delta,
                    channel_link: 0.5,
                    ..Default::default()
                };
                let mut actual_plugin =
                    SpectralCompressorPlugin::from_params(channels, params.clone());
                actual_plugin.initialize(RATE).unwrap();
                let mut expected_plugin = SpectralCompressorPlugin::from_params(channels, params);
                expected_plugin.initialize(RATE).unwrap();
                let mut padded = input.clone();
                // Continue past a full accumulator revolution: discarded
                // negative-time synthesis must never reappear after wrapping.
                padded.resize((frames + tail(n, frames) + 5 * n) * channels, 0.0);
                let expected = process(&mut expected_plugin, &padded, channels, &[1, 7, 113]);
                let mut actual = process(&mut actual_plugin, &input, channels, &[313, 1, 7]);
                actual.extend(drain(&mut actual_plugin, channels, &[1, 13, 8192]));
                assert_eq!(actual.len(), (frames + tail(n, frames)) * channels);
                for (i, (&a, &b)) in actual.iter().zip(&expected).enumerate() {
                    assert!(
                        (a - b).abs() < 2.0e-6,
                        "N={n} target={target} delta={delta} sample={i}: {a} vs {b}"
                    );
                }
                assert!(expected[actual.len()..].iter().all(|v| v.abs() < 2.0e-6));
            }
        }
    }
}

#[test]
fn every_final_hop_phase_matches_zero_continuation_with_active_smoothing() {
    use sotf_host::{ParameterId, ParameterValue};
    // Sweep every source endpoint phase, not only aligned transform boundaries.
    let n = SIZES[0];
    for phase in 0..n / 4 {
        let frames = n + phase + 1;
        let mut input = vec![0.0; frames];
        input[0] = 0.25;
        input[frames - 1] = -0.5;
        let mut actual_plugin = plugin(1, 0, 1.0);
        let mut expected_plugin = plugin(1, 0, 1.0);
        let prefix = n - 7;
        let mut actual = process(&mut actual_plugin, &input[..prefix], 1, &[7, 1, 113]);
        let mut expected = process(&mut expected_plugin, &input[..prefix], 1, &[313, 17]);
        for p in [&mut actual_plugin, &mut expected_plugin] {
            for (key, value) in [("ratio", 6.0), ("threshold", -35.0), ("mix", 0.4)] {
                p.parametric_set_parameter(ParameterId::from(key), ParameterValue::Float(value))
                    .unwrap();
            }
        }
        actual.extend(process(&mut actual_plugin, &input[prefix..], 1, &[1, 13]));
        actual.extend(drain(&mut actual_plugin, 1, &[1, 19, 4096]));
        let mut suffix = input[prefix..].to_vec();
        suffix.resize(frames - prefix + tail(n, frames) + n, 0.0);
        expected.extend(process(&mut expected_plugin, &suffix, 1, &[17, 7]));
        assert_eq!(actual.len(), frames + tail(n, frames));
        for (i, (&a, &b)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                (a - b).abs() < 2.0e-6,
                "phase={phase} sample={i}: {a} vs {b}"
            );
        }
        assert!(expected[actual.len()..].iter().all(|v| v.abs() < 2.0e-6));
    }
}

#[test]
fn first_wet_sample_at_unity_must_survive_at_the_advertised_delay() {
    let n = SIZES[0];
    let mut input = vec![0.0; 3 * n];
    input[0] = 0.25;
    let output = process(&mut plugin(1, 0, 1.0), &input, 1, &[1, 7, 113]);
    assert!(
        (output[n] - 0.25).abs() < 2.0e-6,
        "first wet sample was lost: {}",
        output[n]
    );
}

#[test]
fn destination_errors_preserve_history_and_controls_until_reset() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    for (index, n) in SIZES.into_iter().enumerate() {
        let channels = 2;
        let mut p = plugin(channels, index, 0.5);
        let mut reference = plugin(channels, index, 0.5);
        assert_eq!(p.tail_length(), TailLength::Finite((2 * n - 1) as u64));
        assert!(
            p.drain(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap()
                .complete
        );
        let input = vec![0.25; (n + 13) * channels];
        assert_eq!(
            process(&mut p, &input, channels, &[113]),
            process(&mut reference, &input, channels, &[113])
        );
        let mut sentinel = [1234.0; 3];
        assert!(
            p.drain(&mut sentinel, &ProcessContext::new(RATE, 1))
                .is_err()
        );
        assert!(p.drain(&mut [], &ProcessContext::new(RATE, 0)).is_err());
        assert!(
            p.drain(&mut sentinel[..2], &ProcessContext::new(96_000, 1))
                .is_err()
        );
        assert_eq!(sentinel, [1234.0; 3]);
        assert!(p.initialize(0).is_err());
        let mut first = [0.0; 2];
        assert!(
            !p.drain(&mut first, &ProcessContext::new(RATE, 1))
                .unwrap()
                .complete
        );
        assert!(
            p.parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                .is_err()
        );
        assert!(p.apply_values(Default::default()).is_err());
        assert!(
            p.drain(&mut sentinel, &ProcessContext::new(RATE, 1))
                .is_err()
        );
        let mut rejected = [0.5; 2];
        assert!(
            p.process_in_place(&mut rejected, &ProcessContext::new(RATE, 1))
                .is_err()
        );
        assert_eq!(rejected, [0.5; 2]);
        assert_eq!(
            p.process_in_place(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap(),
            0
        );
        let mut actual = first.to_vec();
        actual.extend(drain(&mut p, channels, &[1, 4096]));
        assert_eq!(actual, drain(&mut reference, channels, &[4096]));
        assert!(
            p.drain(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap()
                .complete
        );
        p.reset();
        assert_eq!(
            process(&mut p, &input, channels, &[1, 113]),
            process(
                &mut plugin(channels, index, 0.5),
                &input,
                channels,
                &[1, 113]
            )
        );
        p.initialize(96_000).unwrap();
        let mut fresh = plugin(channels, index, 0.5);
        fresh.initialize(96_000).unwrap();
        let mut a = input.clone();
        let mut b = input.clone();
        p.process_in_place(&mut a, &ProcessContext::new(96_000, n + 13))
            .unwrap();
        fresh
            .process_in_place(&mut b, &ProcessContext::new(96_000, n + 13))
            .unwrap();
        assert_eq!(a, b);
    }
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
fn cold_transform_drain_and_reset_allocate_and_free_nothing() {
    for (index, n) in SIZES.into_iter().enumerate() {
        for channels in [1, 2, 6] {
            let p = plugin(channels, index, 1.0);
            std::thread::spawn(move || {
                let mut p = p;
                let mut input = vec![0.25; (n + 1) * channels];
                let mut output = vec![0.0; 3 * channels];
                let counts = counted(|| {
                    // Include EOF before the first transform: drain must run
                    // the cold priming FFTs without preparing more storage.
                    for frames in [1, n + 1] {
                        p.process_in_place(
                            &mut input[..frames * channels],
                            &ProcessContext::new(RATE, frames),
                        )
                        .unwrap();
                        while !p
                            .drain(&mut output, &ProcessContext::new(RATE, 3))
                            .unwrap()
                            .complete
                        {}
                        assert!(
                            p.drain(&mut output, &ProcessContext::new(RATE, 3))
                                .unwrap()
                                .complete
                        );
                        p.reset();
                        input.fill(0.25);
                    }
                });
                assert_eq!(counts, (0, 0));
            })
            .join()
            .unwrap();
        }
    }
}
