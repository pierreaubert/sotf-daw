//! Independent spectral finite-stream and preserved classic-mode contracts.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};
const RATE: u32 = 48_000;
fn plugin(channels: usize, enabled: bool, strength: f32) -> HissReducerPlugin {
    let mut p = HissReducerPlugin::from_params(
        channels,
        HissReducerPluginParams {
            spectral_mode: true,
            enabled,
            strength,
            ..Default::default()
        },
    );
    p.initialize(RATE).unwrap();
    p
}
fn process(
    p: &mut HissReducerPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let mut out = input.to_vec();
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / channels {
        let n = blocks[call % blocks.len()].min(input.len() / channels - pos);
        p.process_in_place(
            &mut out[pos * channels..(pos + n) * channels],
            &ProcessContext::new(RATE, n),
        )
        .unwrap();
        pos += n;
        call += 1;
    }
    out
}
fn drain(p: &mut HissReducerPlugin, channels: usize, blocks: &[usize]) -> Vec<f32> {
    let mut out = Vec::new();
    for call in 0..4096 {
        let n = blocks[call % blocks.len()];
        let mut b = vec![1234.; n * channels];
        let s = p.drain(&mut b, &ProcessContext::new(RATE, n)).unwrap();
        assert!(s.frames <= n);
        assert!(b[s.frames * channels..].iter().all(|&v| v == 1234.));
        out.extend_from_slice(&b[..s.frames * channels]);
        if s.complete {
            return out;
        }
        assert!(s.frames > 0);
    }
    panic!("drain incomplete")
}
fn end(t: usize) -> usize {
    2048 + ((t - 1) / 256) * 256
}
#[test]
fn unity_and_disabled_first_final_markers_survive_every_hop_phase() {
    for enabled in [false, true] {
        for channels in [1, 2, 6] {
            let mut p = plugin(channels, enabled, 0.);
            for phase in 0..256 {
                p.reset();
                let t = phase + 1;
                let mut input = vec![0.; t * channels];
                for ch in 0..channels {
                    input[ch] = 0.25 / (ch + 1) as f32;
                    input[phase * channels + ch] -= 0.5 / (ch + 1) as f32;
                }
                let mut actual = process(&mut p, &input, channels, &[7, 137, 1]);
                actual.extend(drain(&mut p, channels, &[1, 17, 4096]));
                assert_eq!(actual.len(), end(t) * channels);
                for (i, &a) in actual.iter().enumerate() {
                    let b = i
                        .checked_sub(1024 * channels)
                        .and_then(|i| input.get(i))
                        .copied()
                        .unwrap_or(0.);
                    assert!(
                        (a - b).abs() < 2e-6,
                        "enabled={enabled} ch={channels} phase={phase} i={i}: {a} vs{b}"
                    );
                }
            }
        }
    }
}
#[test]
fn nonlinear_tail_matches_independent_zero_padding_beyond_ring_wrap() {
    for channels in [1, 2, 6] {
        let t = 6000 + 73;
        let input: Vec<_> = (0..t * channels)
            .map(|i| 0.01 * (i as f32 * 2.319).sin())
            .collect();
        let mut a = plugin(channels, true, 1.);
        let mut b = plugin(channels, true, 1.);
        let mut padded = input.clone();
        padded.resize((end(t) + 4096) * channels, 0.);
        let expected = process(&mut b, &padded, channels, &[1, 137]);
        let mut actual = process(&mut a, &input, channels, &[4096, 17]);
        actual.extend(drain(&mut a, channels, &[1, 13, 4096]));
        assert_eq!(actual.len(), end(t) * channels);
        for (&a, &b) in actual.iter().zip(&expected) {
            assert!((a - b).abs() < 2e-6);
        }
        assert!(expected[actual.len()..].iter().all(|v| v.abs() < 2e-6));
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
fn spectral_lifecycle_is_transactional_and_classic_mode_keeps_legacy_behavior() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    let mut p = plugin(2, true, 1.);
    let mut twin = plugin(2, true, 1.);
    assert_eq!(p.tail_length(), TailLength::Finite(2047));
    assert!(
        p.drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    let input = vec![0.125; 2074];
    assert_eq!(
        process(&mut p, &input, 2, &[137]),
        process(&mut twin, &input, 2, &[137])
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
        p.parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
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
    let mut a = first.to_vec();
    a.extend(drain(&mut p, 2, &[1, 4096]));
    assert_eq!(a, drain(&mut twin, 2, &[4096]));
    assert!(
        p.drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    p.reset();
    assert_eq!(
        process(&mut p, &input, 2, &[1, 137]),
        process(&mut plugin(2, true, 1.), &input, 2, &[1, 137])
    );
    p.initialize(96000).unwrap();
    let mut fresh = plugin(2, true, 1.);
    fresh.initialize(96000).unwrap();
    let mut a = input.clone();
    let mut b = input.clone();
    p.process_in_place(&mut a, &ProcessContext::new(96000, 1037))
        .unwrap();
    fresh
        .process_in_place(&mut b, &ProcessContext::new(96000, 1037))
        .unwrap();
    assert_eq!(a, b);
    let mut classic = HissReducerPlugin::new(1);
    classic.initialize(RATE).unwrap();
    assert_eq!(classic.tail_length(), TailLength::Unknown);
    assert_eq!(classic.drain_output_frames_max(), 0);
    assert!(
        classic
            .drain(&mut [], &ProcessContext::new(96000, 0))
            .unwrap()
            .complete
    );
    classic
        .parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
        .unwrap();
    classic
        .process_in_place(&mut [0.25], &ProcessContext::new(RATE, 1))
        .unwrap();
}
#[test]
fn spectral_cold_process_drain_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2, 6] {
        for enabled in [false, true] {
            let p = plugin(channels, enabled, 1.);
            std::thread::spawn(move || {
                let mut p = p;
                let mut input = vec![0.125; 4097 * channels];
                let mut output = vec![0.; 3 * channels];
                let counts = counted(|| {
                    for frames in [1, 4097] {
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
                        input.fill(0.125);
                    }
                });
                assert_eq!(counts, (0, 0));
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn live_bypass_and_strength_transitions_continue_through_eof() {
    use sotf_host::{ParameterId, ParameterValue};
    for channels in [1, 2, 6] {
        let prefix: Vec<_> = (0..4096 * channels)
            .map(|i| 0.01 * (i as f32 * 2.319).sin())
            .collect();
        let suffix = vec![0.015; 13 * channels];
        let mut p = plugin(channels, true, 1.);
        let mut reference = plugin(channels, true, 1.);
        assert_eq!(
            process(&mut p, &prefix, channels, &[137]),
            process(&mut reference, &prefix, channels, &[137])
        );
        for p in [&mut p, &mut reference] {
            p.parametric_set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
                .unwrap();
            p.parametric_set_parameter(ParameterId::from("strength"), ParameterValue::Float(0.25))
                .unwrap();
        }
        let mut actual = process(&mut p, &suffix, channels, &[1, 7]);
        actual.extend(drain(&mut p, channels, &[1, 4096]));
        let mut padded = suffix.clone();
        padded.resize((end(4109) - 4096 + 4096) * channels, 0.);
        let expected = process(&mut reference, &padded, channels, &[137, 1]);
        assert_eq!(actual.len(), (end(4109) - 4096) * channels);
        assert_eq!(actual, expected[..actual.len()]);
        assert!(expected[actual.len()..].iter().all(|v| v.abs() < 2e-6));
    }
}
