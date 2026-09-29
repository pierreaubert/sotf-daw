//! Finite-stream and callback-time oracles independent of production windows.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};
const RATE: u32 = 48_000;
fn plugin(channels: usize, low: bool, multi: bool, pnd: bool, unity: bool) -> DenoiserPlugin {
    let mut p = DenoiserPlugin::from_params(
        channels,
        DenoiserPluginParams {
            low_latency: low,
            multi_resolution: multi,
            polyphonic_detection: pnd,
            transparency: if unity { 1. } else { 0. },
            ..Default::default()
        },
    );
    p.initialize(RATE).unwrap();
    p
}
fn process(p: &mut DenoiserPlugin, input: &[f32], channels: usize, blocks: &[usize]) -> Vec<f32> {
    let mut result = input.to_vec();
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / channels {
        let n = blocks[call % blocks.len()].min(input.len() / channels - pos);
        assert_eq!(
            p.process_in_place(
                &mut result[pos * channels..(pos + n) * channels],
                &ProcessContext::new(RATE, n)
            )
            .unwrap(),
            n
        );
        pos += n;
        call += 1;
    }
    result
}
fn drain(p: &mut DenoiserPlugin, channels: usize, capacities: &[usize]) -> Vec<f32> {
    let mut result = Vec::new();
    for call in 0..20_000 {
        let n = capacities[call % capacities.len()];
        let mut output = vec![1234.; n * channels];
        let status = p.drain(&mut output, &ProcessContext::new(RATE, n)).unwrap();
        assert!(status.frames <= n);
        assert!(
            output[status.frames * channels..]
                .iter()
                .all(|&v| v == 1234.)
        );
        result.extend_from_slice(&output[..status.frames * channels]);
        if status.complete {
            return result;
        }
        assert!(status.frames > 0);
    }
    panic!("drain did not complete")
}
fn end(n: usize, t: usize) -> usize {
    2 * n + ((t - 1) / (n / 2)) * (n / 2)
}
#[test]
fn every_first_window_phase_preserves_first_and_final_unity_impulses() {
    for low in [false, true] {
        let mut p = plugin(1, low, false, false, true);
        let n = p.latency_samples();
        for phase in 0..n {
            p.reset();
            let t = phase + 1;
            let mut input = vec![0.; t];
            input[0] = 0.25;
            input[phase] -= 0.5;
            let mut actual = process(
                &mut p,
                &input,
                1,
                if phase % 2 == 0 {
                    &[1, 7, 137]
                } else {
                    &[4096]
                },
            );
            actual.extend(drain(&mut p, 1, &[1, 13, 4096]));
            assert_eq!(actual.len(), end(n, t));
            for (i, &a) in actual.iter().enumerate() {
                let b = i
                    .checked_sub(n)
                    .and_then(|i| input.get(i))
                    .copied()
                    .unwrap_or(0.);
                assert!((a - b).abs() < 2e-6, "N={n} phase={phase} i={i} {a} vs{b}");
            }
        }
    }
}
#[test]
fn dense_unity_crosses_start_end_and_accumulator_wraps() {
    for low in [false, true] {
        for channels in [1, 2, 6] {
            let mut p = plugin(channels, low, false, false, true);
            let n = p.latency_samples();
            let t = 10 * n + 73;
            let input: Vec<_> = (0..t * channels)
                .map(|i| 0.2 * (i as f64 * 0.719).cos() as f32)
                .collect();
            let mut actual = process(&mut p, &input, channels, &[137, 4096, 1]);
            actual.extend(drain(&mut p, channels, &[1, 4096]));
            assert_eq!(actual.len(), end(n, t) * channels);
            for (i, &a) in actual.iter().enumerate() {
                let b = i
                    .checked_sub(n * channels)
                    .and_then(|i| input.get(i))
                    .copied()
                    .unwrap_or(0.);
                assert!(
                    (a - b).abs() < 2e-6,
                    "low={low} ch={channels} i={i} {a} vs{b}"
                );
            }
        }
    }
}
#[test]
fn pnd_and_multi_resolution_follow_source_time_independent_of_callbacks() {
    for low in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                let input: Vec<_> = (0..32768)
                    .map(|i| {
                        let frequency = match (i / 3072) % 3 {
                            0 => 440.,
                            1 => 659.25,
                            _ => 987.77,
                        };
                        0.2 * (std::f64::consts::TAU * frequency * i as f64 / 48000.).sin() as f32
                            + 0.02 * (i as f64 * 1.719).cos() as f32
                    })
                    .collect();
                let a = process(
                    &mut plugin(1, low, multi, pnd, false),
                    &input,
                    1,
                    &[1, 7, 137],
                );
                let b = process(&mut plugin(1, low, multi, pnd, false), &input, 1, &[4096]);
                let error = a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0., f32::max);
                assert!(
                    error < 2e-6,
                    "low={low} multi={multi} pnd={pnd} max={error}"
                );
            }
        }
    }
}
#[test]
fn all_modes_have_exact_derived_bound_and_match_zero_continuation() {
    for low in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                for channels in [1, 2, 6] {
                    let mut p = plugin(channels, low, multi, pnd, false);
                    let n = p.latency_samples();
                    let t = 5 * n + 73;
                    let input: Vec<_> = (0..t * channels)
                        .map(|i| 0.2 * (i as f32 * 0.171).sin())
                        .collect();
                    let mut reference = plugin(channels, low, multi, pnd, false);
                    let mut padded = input.clone();
                    padded.resize((end(n, t) + 5 * n) * channels, 0.);
                    let expected = process(&mut reference, &padded, channels, &[1, 137, 7]);
                    let mut actual = process(&mut p, &input, channels, &[4096, 17]);
                    actual.extend(drain(&mut p, channels, &[1, 13, 8192]));
                    assert_eq!(actual.len(), end(n, t) * channels);
                    for (i, (&a, &b)) in actual.iter().zip(&expected).enumerate() {
                        assert!(
                            (a - b).abs() < 2e-6,
                            "low={low} multi={multi} pnd={pnd} ch={channels} i={i}: {a} vs{b}"
                        );
                    }
                    assert!(expected[actual.len()..].iter().all(|v| v.abs() < 2e-6));
                }
            }
        }
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
fn errors_controls_reset_and_reinitialization_preserve_stream_contract() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    for low in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                let mut p = plugin(2, low, multi, pnd, false);
                let mut reference = plugin(2, low, multi, pnd, false);
                let n = p.latency_samples();
                assert_eq!(p.tail_length(), TailLength::Finite((2 * n - 1) as u64));
                assert!(
                    p.drain(&mut [], &ProcessContext::new(RATE, 0))
                        .unwrap()
                        .complete
                );
                let input = vec![0.125; (n + 13) * 2];
                assert_eq!(
                    process(&mut p, &input, 2, &[137]),
                    process(&mut reference, &input, 2, &[137])
                );
                let mut sentinel = [1234.; 3];
                assert!(
                    p.drain(&mut sentinel, &ProcessContext::new(RATE, 1))
                        .is_err()
                );
                assert!(p.drain(&mut [], &ProcessContext::new(RATE, 0)).is_err());
                assert!(
                    p.drain(&mut sentinel[..2], &ProcessContext::new(96000, 1))
                        .is_err()
                );
                assert_eq!(sentinel, [1234.; 3]);
                assert!(p.initialize(0).is_err());
                let mut first = [0.; 2];
                assert!(
                    !p.drain(&mut first, &ProcessContext::new(RATE, 1))
                        .unwrap()
                        .complete
                );
                assert!(
                    p.parametric_set_parameter(
                        ParameterId::from("transparency"),
                        ParameterValue::Float(1.)
                    )
                    .is_err()
                );
                assert!(p.apply_values(Default::default()).is_err());
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
                actual.extend(drain(&mut p, 2, &[1, 4096]));
                assert_eq!(actual, drain(&mut reference, 2, &[4096]));
                assert!(
                    p.drain(&mut [], &ProcessContext::new(RATE, 0))
                        .unwrap()
                        .complete
                );
                p.reset();
                assert_eq!(
                    process(&mut p, &input, 2, &[1, 137]),
                    process(&mut plugin(2, low, multi, pnd, false), &input, 2, &[1, 137])
                );
                p.initialize(96000).unwrap();
                let mut fresh = plugin(2, low, multi, pnd, false);
                fresh.initialize(96000).unwrap();
                let mut a = input.clone();
                let mut b = input.clone();
                p.process_in_place(&mut a, &ProcessContext::new(96000, n + 13))
                    .unwrap();
                fresh
                    .process_in_place(&mut b, &ProcessContext::new(96000, n + 13))
                    .unwrap();
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn cold_process_eof_publication_and_reset_allocate_and_free_nothing() {
    for low in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                for channels in [1, 2, 6] {
                    let p = plugin(channels, low, multi, pnd, false);
                    std::thread::spawn(move || {
                        let mut p = p;
                        let n = p.latency_samples();
                        let mut input = vec![0.125; 8192 * channels];
                        let mut output = vec![0.; 3 * channels];
                        let counts = counted(|| {
                            for source_frames in [1, 9 * n + 1] {
                                let mut left = source_frames;
                                while left > 0 {
                                    let frames = left.min(4096);
                                    input[..frames * channels].fill(0.125);
                                    p.process_in_place(
                                        &mut input[..frames * channels],
                                        &ProcessContext::new(RATE, frames),
                                    )
                                    .unwrap();
                                    left -= frames;
                                }
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
                            }
                        });
                        assert_eq!(counts, (0, 0));
                    })
                    .join()
                    .unwrap();
                }
            }
        }
    }
}

fn harmonic_plugin(channels: usize, low: bool) -> DenoiserPlugin {
    let mut p = plugin(channels, low, false, false, false);
    // Exercise the public live setter: this option is not yet serialized by
    // DenoiserPluginParams, so a JSON-only fixture would leave it disabled.
    p.parametric_set_parameter(
        sotf_host::ParameterId::from("harmonic_percussive"),
        sotf_host::ParameterValue::Bool(true),
    )
    .unwrap();
    p
}

fn harmonic_history(channels: usize) -> Vec<f32> {
    (0..32768 * channels)
        .map(|i| {
            let frequency = 1000. + 137. * (i % channels) as f32;
            0.7 * (std::f32::consts::TAU * (i / channels) as f32 * frequency / RATE as f32).sin()
        })
        .collect()
}

#[test]
fn harmonic_percussive_warm_reset_and_reinitialize_match_fresh_output() {
    for low in [false, true] {
        for channels in [1, 2, 6] {
            let warm = harmonic_history(channels);
            let input: Vec<_> = (0..32768 * channels)
                .map(|i| (((i * 65537 + 17) % 131071) as f32 / 131071. - 0.5) * 0.1)
                .collect();
            for reinitialize in [false, true] {
                let mut p = harmonic_plugin(channels, low);
                let mut fresh = harmonic_plugin(channels, low);
                process(&mut p, &warm, channels, &[137]);
                if reinitialize {
                    p.initialize(RATE).unwrap();
                } else {
                    p.reset();
                }
                assert_eq!(
                    p.parametric_get_parameter(&sotf_host::ParameterId::from(
                        "harmonic_percussive"
                    )),
                    Some(sotf_host::ParameterValue::Bool(true))
                );
                let actual = process(&mut p, &input, channels, &[137, 7, 4096]);
                let expected = process(&mut fresh, &input, channels, &[137, 7, 4096]);
                let difference = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0., f32::max);
                assert_eq!(
                    difference, 0.,
                    "low={low} channels={channels} reinitialize={reinitialize}"
                );
            }
        }
    }
}

#[test]
fn harmonic_percussive_cold_thread_reset_allocates_and_frees_nothing() {
    for low in [false, true] {
        for channels in [1, 2, 6] {
            let mut p = harmonic_plugin(channels, low);
            process(&mut p, &harmonic_history(channels), channels, &[137]);
            std::thread::spawn(move || {
                assert_eq!(counted(|| p.reset()), (0, 0));
            })
            .join()
            .unwrap();
        }
    }
}
