//! Exact Gate lookahead tail, external-key layout, and realtime lifecycle oracles.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::PluginDrainResult;
use sotf_host::{
    ParameterId, ParameterValue, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, Plugin,
    ProcessContext, TailLength,
};
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
fn settings(lookahead_ms: f32, mode: GateMode, external: bool, mix: f32) -> GatePluginParams {
    GatePluginParams {
        lookahead_ms,
        mode,
        sidechain_external: external,
        mix,
        ..Default::default()
    }
}
fn make(rate: u32, channels: usize, params: GatePluginParams) -> Box<dyn Plugin> {
    let mut p: Box<dyn Plugin> = Box::new(ParametricInPlacePluginAdapter::new(
        GatePlugin::try_from_params(channels, params).unwrap(),
    ));
    p.initialize(rate).unwrap();
    p
}
fn markers(frames: usize, channels: usize, external: bool) -> (Vec<f32>, Vec<f32>) {
    let stride = channels * if external { 2 } else { 1 };
    let mut source = vec![0.0; frames * stride];
    let mut program = vec![0.0; frames * channels];
    for ch in 0..channels {
        source[ch] = 0.001 * (ch + 1) as f32;
        source[(frames - 1) * stride + ch] = -0.001 * (ch + 1) as f32;
        program[ch] = source[ch];
        program[(frames - 1) * channels + ch] = source[(frames - 1) * stride + ch];
    }
    (source, program)
}
fn feed(p: &mut dyn Plugin, rate: u32, input: &[f32], chunk: usize) -> Vec<f32> {
    let ic = p.input_channels();
    let oc = p.output_channels();
    let mut result = Vec::new();
    for source in input.chunks(chunk * ic) {
        let frames = source.len() / ic;
        let mut output = vec![12345.0; frames * oc];
        assert_eq!(
            p.process(source, &mut output, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
        result.extend_from_slice(&output);
    }
    result
}
fn finish(p: &mut dyn Plugin, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let channels = p.output_channels();
    let mut result = Vec::new();
    for call in 0..20000 {
        let cap = capacities[call % capacities.len()];
        let mut output = vec![12345.0; (cap + 1) * channels];
        let ended = p
            .drain(
                &mut output[..cap * channels],
                &ProcessContext::new(rate, usize::MAX),
            )
            .unwrap();
        assert!(ended.frames <= cap && ended.frames <= p.drain_output_frames_max());
        assert!(
            output[ended.frames * channels..]
                .iter()
                .all(|&x| x == 12345.0)
        );
        result.extend_from_slice(&output[..ended.frames * channels]);
        if ended.complete {
            return result;
        }
        assert!(ended.frames > 0);
    }
    panic!("bounded native Gate drain did not finish");
}
fn physical_delay(rate: u32, ms: f32) -> usize {
    if ms > 0.0 {
        ((f64::from(ms) * f64::from(rate) / 1000.0).round() as usize).max(1)
    } else {
        0
    }
}

#[test]
fn every_mode_and_key_layout_retains_exact_delayed_first_and_last_samples() {
    for rate in [44100, 48000, 96000, 192000] {
        for channels in [1, 2, 6] {
            for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
                for external in [false, true] {
                    for lookahead in [0.0, 0.001, 5.0, 20.0] {
                        let delay = physical_delay(rate, lookahead);
                        for frames in [1, 121, delay + 31] {
                            let mut p =
                                make(rate, channels, settings(lookahead, mode, external, 0.0));
                            let (source, program) = markers(frames, channels, external);
                            let mut actual = feed(p.as_mut(), rate, &source, 17);
                            actual.extend(finish(p.as_mut(), rate, &[1, 7, 263]));
                            let mut expected = vec![0.0; delay * channels];
                            expected.extend_from_slice(&program);
                            assert_eq!(
                                actual, expected,
                                "{rate}/{channels}/{mode:?}/external={external}/lookahead={lookahead}/frames={frames}"
                            );
                            assert_eq!(p.latency_samples(), delay);
                            assert_eq!(p.tail_length(), TailLength::Finite(delay as u64));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn frozen_controls_continue_detectors_and_envelopes_exactly_like_zero_input() {
    for rate in [44100, 48000, 96000, 192000] {
        for channels in [1, 2, 6] {
            for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
                for external in [false, true] {
                    for variant in 0..4 {
                        let mut params = settings(
                            5.0,
                            mode,
                            external,
                            if variant % 2 == 0 { 1.0 } else { 0.35 },
                        );
                        params.threshold_db = -24.0;
                        params.ratio = 3.0;
                        params.attack_ms = 0.5;
                        params.release_ms = 10.0;
                        params.range_db = 12.0;
                        params.max_boost_db = 6.0;
                        params.link_channels = variant % 2 == 0;
                        params.hold_ms = if variant >= 2 { 7.0 } else { 0.0 };
                        params.hysteresis_db = 2.0;
                        params.knee_db = 4.0;
                        if variant == 1 || variant == 2 {
                            params.detection_mode = "RMS".into();
                            params.sidechain_hpf_hz = 80.0;
                            params.sidechain_hpf_order = if variant == 2 {
                                "4th".into()
                            } else {
                                "2nd".into()
                            };
                        }
                        let mut p = make(rate, channels, params.clone());
                        let mut reference = make(rate, channels, params);
                        let stride = p.input_channels();
                        let source: Vec<_> = (0..777 * stride)
                            .map(|i| {
                                let frame = i / stride;
                                let ch = i % stride;
                                ((frame as f64 * (0.039 + 0.091 * ch as f64)).sin() * 0.08
                                    + (frame as f64 * 0.73).cos() * 0.02)
                                    as f32
                            })
                            .collect();
                        let mut actual = feed(p.as_mut(), rate, &source, 127);
                        let mut expected = feed(reference.as_mut(), rate, &source, 31);
                        // A pending threshold ramp must continue during drain, with fixed target.
                        for plugin in [&mut p, &mut reference] {
                            plugin
                                .set_parameter(
                                    ParameterId::from("threshold"),
                                    ParameterValue::Float(-18.0),
                                )
                                .unwrap();
                        }
                        expected.extend(feed(
                            reference.as_mut(),
                            rate,
                            &vec![0.0; physical_delay(rate, 5.0) * stride],
                            13,
                        ));
                        actual.extend(finish(p.as_mut(), rate, &[1, 3, 255, 1024]));
                        assert_eq!(
                            actual, expected,
                            "{rate}/{channels}/{mode:?}/external={external}/variant={variant}"
                        );
                        let extra = feed(reference.as_mut(), rate, &vec![0.0; 512 * stride], 7);
                        assert!(extra.iter().all(|&x| x == 0.0));
                    }
                }
            }
        }
    }
}

#[test]
fn malformed_drain_preserves_input_and_key_history_before_and_during_eos() {
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        for external in [false, true] {
            let rate = 48000;
            let channels = 2;
            let params = settings(5.0, mode, external, 0.5);
            let mut p = make(rate, channels, params.clone());
            let mut reference = make(rate, channels, params);
            let (source, _) = markers(97, channels, external);
            assert_eq!(
                feed(p.as_mut(), rate, &source, 11),
                feed(reference.as_mut(), rate, &source, 31)
            );
            let mut canary = [12345.0; 9];
            for (length, sample_rate) in [(0, rate), (1, rate), (3, rate), (8, rate + 1)] {
                assert!(
                    p.drain(&mut canary[..length], &ProcessContext::new(sample_rate, 0))
                        .is_err()
                );
                assert_eq!(canary, [12345.0; 9]);
            }
            for plugin in [&mut p, &mut reference] {
                plugin
                    .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-18.0))
                    .unwrap();
            }
            let (suffix, _) = markers(13, channels, external);
            assert_eq!(
                feed(p.as_mut(), rate, &suffix, 7),
                feed(reference.as_mut(), rate, &suffix, 13)
            );
            let mut first = [0.0; 2];
            let started = p.drain(&mut first, &ProcessContext::new(rate, 0)).unwrap();
            assert_eq!(started.frames, 1);
            assert!(!started.complete);
            for (length, sample_rate) in [(0, rate), (1, rate), (3, rate), (8, rate + 1)] {
                assert!(
                    p.drain(&mut canary[..length], &ProcessContext::new(sample_rate, 0))
                        .is_err()
                );
                assert_eq!(canary, [12345.0; 9]);
            }
            let mut actual = first.to_vec();
            actual.extend(finish(p.as_mut(), rate, &[1, 19]));
            let stride = reference.input_channels();
            let expected = feed(reference.as_mut(), rate, &vec![0.0; 240 * stride], 127);
            assert_eq!(actual, expected);
            assert_eq!(
                p.drain(&mut [], &ProcessContext::new(rate, 0)).unwrap(),
                PluginDrainResult::COMPLETE
            );
        }
    }
}

#[test]
fn empty_eos_controls_reset_and_reinitialize_have_explicit_lifecycle() {
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        for external in [false, true] {
            for lookahead in [0.0, 0.001, 5.0] {
                let rate = 48000;
                let params = settings(lookahead, mode, external, 0.5);
                let mut p = GatePlugin::try_from_params(2, params.clone()).unwrap();
                assert_eq!(p.tail_length(), TailLength::Unknown);
                assert!(
                    p.drain(&mut [0.0; 2], &ProcessContext::new(rate, 0))
                        .is_err()
                );
                p.initialize(rate).unwrap();
                assert_eq!(
                    p.drain(&mut [], &ProcessContext::new(rate, 0)).unwrap(),
                    PluginDrainResult::COMPLETE
                );
                p.parametric_set_parameter(
                    ParameterId::from("threshold"),
                    ParameterValue::Float(-18.0),
                )
                .unwrap();
                let (mut input, _) = markers(121, 2, external);
                p.process_in_place(&mut input, &ProcessContext::new(rate, 121))
                    .unwrap();
                p.drain(&mut [0.0; 2], &ProcessContext::new(rate, 0))
                    .unwrap();
                let values = p.current_values();
                for (id, value) in &values {
                    p.parametric_set_parameter(id.clone(), value.clone())
                        .unwrap();
                }
                p.apply_values(values.clone()).unwrap();
                let mut changed = values;
                changed.insert(ParameterId::from("threshold"), ParameterValue::Float(-30.0));
                assert!(p.apply_values(changed).is_err());
                assert!(
                    p.parametric_set_parameter(
                        ParameterId::from("threshold"),
                        ParameterValue::Float(-30.0)
                    )
                    .is_err()
                );
                let original = input.clone();
                assert!(
                    p.process_in_place(&mut input, &ProcessContext::new(rate, 121))
                        .is_err()
                );
                assert_eq!(input, original);
                assert_eq!(
                    p.process_in_place(&mut [], &ProcessContext::new(rate, 0))
                        .unwrap(),
                    0
                );
                for reinitialize in [false, true] {
                    if reinitialize {
                        p.initialize(rate).unwrap();
                    } else {
                        p.reset();
                    }
                    let mut restored = params.clone();
                    restored.threshold_db = -18.0;
                    let mut fresh = GatePlugin::try_from_params(2, restored).unwrap();
                    fresh.initialize(rate).unwrap();
                    let (mut actual, _) = markers(997, 2, external);
                    let mut expected = actual.clone();
                    p.process_in_place(&mut actual, &ProcessContext::new(rate, 997))
                        .unwrap();
                    fresh
                        .process_in_place(&mut expected, &ProcessContext::new(rate, 997))
                        .unwrap();
                    assert_eq!(actual, expected, "reinitialize={reinitialize}");
                }
            }
        }
    }
}

#[test]
fn cold_process_tail_drain_controls_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2, 6] {
        for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
            for external in [false, true] {
                for lookahead in [0.0, 0.001, 20.0] {
                    let rate = 48000;
                    let mut params = settings(lookahead, mode, external, 0.5);
                    params.link_channels = false;
                    params.detection_mode = "RMS".into();
                    params.sidechain_hpf_hz = 80.0;
                    params.sidechain_hpf_order = "4th".into();
                    let mut p = make(rate, channels, params);
                    let (source, _) = markers(121, channels, external);
                    let mut output = vec![0.0; 121 * channels];
                    let mut drain = vec![0.0; 257 * channels];
                    let id = ParameterId::from("threshold");
                    let current = p.get_parameter(&id).unwrap();
                    without_heap(|| {
                        assert!(p.drain_call_bound().is_some());
                        assert_eq!(
                            p.process(&source, &mut output, &ProcessContext::new(rate, 121))
                                .unwrap(),
                            121
                        );
                        assert_eq!(
                            p.tail_length(),
                            TailLength::Finite(physical_delay(rate, lookahead) as u64)
                        );
                        let mut total = 0;
                        loop {
                            assert!(p.drain_call_bound().is_some());
                            let result =
                                p.drain(&mut drain, &ProcessContext::new(rate, 0)).unwrap();
                            total += result.frames;
                            p.set_parameter(id.clone(), current.clone()).unwrap();
                            if result.complete {
                                break;
                            }
                        }
                        assert_eq!(total, physical_delay(rate, lookahead));
                        p.reset();
                        p.process(&source, &mut output, &ProcessContext::new(rate, 121))
                            .unwrap();
                    });
                }
            }
        }
    }
}

// Check the public adapter contract against observed calls, including a prefix
// drained below maximum capacity before the bound is queried again.
fn check_call_bound(mut plugin: Box<dyn Plugin>, rate: u32, partial: usize) {
    let channels = plugin.output_channels();
    let input = vec![0.125; 71 * plugin.input_channels()];
    let mut processed = vec![0.; 71 * channels];
    for _ in 0..2 {
        plugin.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
        assert_eq!(
            plugin.drain_call_bound().expect("native work bound").get(),
            1
        );
        let maximum = plugin.drain_output_frames_max();
        let mut output = vec![1234.; (maximum + 1) * channels];
        assert!(
            plugin
                .drain(
                    &mut output[..maximum * channels],
                    &ProcessContext::new(rate, 0)
                )
                .unwrap()
                .complete
        );
        assert!(output.iter().all(|&sample| sample == 1234.));
        plugin
            .process(&input, &mut processed, &ProcessContext::new(rate, 71))
            .unwrap();
        let expected_frames = match plugin.tail_length() {
            TailLength::Finite(frames) => frames as usize,
            TailLength::Infinite | TailLength::Unknown => 0, // Existing unsupported-path COMPLETE0 policy.
        };
        let before = plugin.drain_call_bound();
        if expected_frames > 0 {
            assert!(
                plugin
                    .drain(&mut [], &ProcessContext::new(rate, 0))
                    .is_err()
            );
            assert_eq!(plugin.drain_call_bound(), before);
        }
        let mut published = 0;
        if partial > 0 {
            let mut prefix = vec![0.; partial * channels];
            published += plugin
                .drain(&mut prefix, &ProcessContext::new(rate, 0))
                .unwrap()
                .frames;
        }
        plugin.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
        let declared = plugin.drain_call_bound().expect("native work bound").get();
        let mut calls = 0;
        loop {
            assert_eq!(plugin.drain_call_bound().unwrap().get(), declared - calls);
            calls += 1;
            assert!(calls <= declared);
            let maximum = plugin.drain_output_frames_max();
            output.resize((maximum + 1) * channels, 1234.);
            output.fill(1234.);
            let step = plugin
                .drain(
                    &mut output[..maximum * channels],
                    &ProcessContext::new(rate, 0),
                )
                .unwrap();
            assert!(
                output[step.frames * channels..]
                    .iter()
                    .all(|&sample| sample == 1234.)
            );
            published += step.frames;
            if step.complete {
                break;
            }
            assert!(step.frames > 0);
        }
        assert_eq!(calls, declared);
        assert_eq!(published, expected_frames);
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
        plugin.reset();
    }
}

#[test]
fn drain_call_bound_tracks_full_capacity_partial_empty_and_reset() {
    let uninitialized =
        GatePlugin::try_from_params(2, settings(5., GateMode::Downward, false, 1.)).unwrap();
    assert!(sotf_host::ParametricInPlacePlugin::drain_call_bound(&uninitialized).is_none());
    for rate in [44100, 192000] {
        for external in [false, true] {
            for lookahead in [0., 0.001, 5., 20.] {
                for partial in [0, 1, 255] {
                    check_call_bound(
                        make(
                            rate,
                            2,
                            settings(lookahead, GateMode::Downward, external, 0.),
                        ),
                        rate,
                        partial,
                    );
                }
            }
        }
    }
}
