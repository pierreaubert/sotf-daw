//! Public finite-stream oracles and cold callback allocation/deallocation checks.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::PluginDrainResult;
use sotf_host::{
    ParameterId, ParameterValue, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, Plugin,
    ProcessContext, TailLength,
};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
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
fn settings(lookahead_ms: f32, isp: bool, mix: f32) -> LimiterPluginParams {
    LimiterPluginParams {
        threshold_db: -6.0,
        release_ms: 10.0,
        lookahead_ms,
        soft: false,
        true_peak: isp,
        isp_mode: isp,
        dual_release: false,
        mix,
        feed_forward: true,
        link_amount: 1.0,
        oversampling: 0,
    }
}
fn make(rate: u32, channels: usize, params: LimiterPluginParams) -> Box<dyn Plugin> {
    let mut plugin: Box<dyn Plugin> = Box::new(ParametricInPlacePluginAdapter::new(
        LimiterPlugin::from_params(channels, params),
    ));
    plugin.initialize(rate).unwrap();
    plugin
}
fn input_markers(frames: usize, channels: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * channels];
    for channel in 0..channels {
        input[channel] = 0.001 * (channel + 1) as f32;
        input[(frames - 1) * channels + channel] = -0.001 * (channel + 1) as f32;
    }
    input
}
fn feed(plugin: &mut dyn Plugin, rate: u32, input: &[f32], chunk: usize) -> Vec<f32> {
    let channels = plugin.input_channels();
    let mut result = vec![f32::NAN; input.len()];
    for (input, output) in input
        .chunks(chunk * channels)
        .zip(result.chunks_mut(chunk * channels))
    {
        let frames = input.len() / channels;
        assert_eq!(
            plugin
                .process(input, output, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
    }
    result
}
fn finish(plugin: &mut dyn Plugin, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut result = Vec::new();
    let max_frames = plugin.drain_output_frames_max();
    for call in 0..20_000 {
        let capacity = capacities[call % capacities.len()];
        // Leave one complete canary frame beyond the slice given to the plugin.
        let mut output = vec![12345.0; (capacity + 1) * channels];
        let drained = plugin
            .drain(
                &mut output[..capacity * channels],
                &ProcessContext::new(rate, 0),
            )
            .unwrap();
        assert!(drained.frames <= max_frames && drained.frames <= capacity);
        assert!(
            output[drained.frames * channels..]
                .iter()
                .all(|&v| v == 12345.0)
        );
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            return result;
        }
        assert!(
            drained.frames > 0,
            "native limiter drain must make bounded progress"
        );
    }
    panic!("drain did not finish within the independent maximum lookahead bound");
}
fn expected_delay(rate: u32, lookahead_ms: f32, isp: bool) -> usize {
    let lookahead = (f64::from(rate) * f64::from(lookahead_ms) / 1000.0).floor() as usize;
    let detector_delay = if rate < 96_000 {
        6
    } else if rate < 192_000 {
        12
    } else {
        0
    };
    lookahead + if isp { 3 * detector_delay } else { 0 }
}

#[test]
fn first_and_final_markers_follow_exact_independent_delay() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for (lookahead, isp, mix) in [
                (0.0, false, 1.0),
                (5.0, false, 0.0),
                (5.0, false, 0.4),
                (5.0, true, 1.0),
                (20.0, true, 1.0),
            ] {
                let delay = expected_delay(rate, lookahead, isp);
                for frames in [1, 121, delay + 31] {
                    let mut plugin = make(rate, channels, settings(lookahead, isp, mix));
                    let source = input_markers(frames, channels);
                    let mut got = feed(plugin.as_mut(), rate, &source, 17);
                    got.extend(finish(plugin.as_mut(), rate, &[1, 7, 263]));
                    let mut expected = vec![0.0; delay * channels];
                    expected.extend_from_slice(&source);
                    assert_eq!(
                        got, expected,
                        "rate={rate}, channels={channels}, lookahead={lookahead}, ISP={isp}, mix={mix}, frames={frames}"
                    );
                    assert_eq!(plugin.tail_length(), TailLength::Finite(delay as u64));
                    assert_eq!(plugin.latency_samples(), delay);
                }
            }
        }
    }
}

#[test]
fn nonlinear_tail_matches_separately_zero_continued_stream() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for (isp, mix, soft) in [(false, 1.0, false), (false, 0.35, true), (true, 1.0, false)] {
                for link in [0.0, 0.4, 1.0] {
                    let mut params = settings(5.0, isp, mix);
                    params.soft = soft;
                    params.dual_release = true;
                    params.link_amount = link;
                    let mut actual = make(rate, channels, params.clone());
                    let mut reference = make(rate, channels, params);
                    let source: Vec<_> = (0..777 * channels)
                        .map(|i| {
                            let frame = i / channels;
                            let channel = i % channels;
                            ((frame as f64 * (0.47 + channel as f64 * 0.21)).sin() * 1.6
                                + (frame as f64 * 0.71 + channel as f64).cos() * 0.4)
                                as f32
                        })
                        .collect();
                    let mut got = feed(actual.as_mut(), rate, &source, 127);
                    let mut expected = feed(reference.as_mut(), rate, &source, 31);
                    let silence = vec![0.0; expected_delay(rate, 5.0, isp) * channels];
                    expected.extend(feed(reference.as_mut(), rate, &silence, 13));
                    got.extend(finish(actual.as_mut(), rate, &[3, 1, 255, 1000]));
                    assert_eq!(
                        got, expected,
                        "{rate}/{channels}/ISP={isp}/mix={mix}/link={link}"
                    );
                    // The bound follows audio storage, not detector or release state.
                    let extra = feed(reference.as_mut(), rate, &vec![0.0; 512 * channels], 7);
                    assert!(extra.iter().all(|&v| v == 0.0));
                }
            }
        }
    }
}

#[test]
fn drain_validation_is_transactional_before_and_during_eos() {
    let rate = 48_000;
    let channels = 2;
    let mut plugin = make(rate, channels, settings(5.0, true, 1.0));
    let mut reference = make(rate, channels, settings(5.0, true, 1.0));
    let source = input_markers(97, channels);
    assert_eq!(
        feed(plugin.as_mut(), rate, &source, 11),
        feed(reference.as_mut(), rate, &source, 31)
    );
    let mut canary = [12345.0; 9];
    for (length, requested_rate) in [(0, rate), (1, rate), (3, rate), (8, rate + 1)] {
        assert!(
            plugin
                .drain(
                    &mut canary[..length],
                    &ProcessContext::new(requested_rate, 0)
                )
                .is_err()
        );
        assert_eq!(canary, [12345.0; 9]);
    }
    // Failed requests must not enter EOS or freeze a valid automation update.
    for p in [&mut plugin, &mut reference] {
        p.set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-9.0))
            .unwrap();
    }
    let suffix = input_markers(13, channels);
    assert_eq!(
        feed(plugin.as_mut(), rate, &suffix, 7),
        feed(reference.as_mut(), rate, &suffix, 13)
    );
    let mut prefix = [0.0; 2];
    let first = plugin
        .drain(&mut prefix, &ProcessContext::new(rate, usize::MAX))
        .unwrap();
    assert_eq!(first.frames, 1);
    assert!(!first.complete);
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut canary, &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut canary[..8], &ProcessContext::new(rate + 1, 0))
            .is_err()
    );
    assert_eq!(canary, [12345.0; 9]);
    let mut got = prefix.to_vec();
    got.extend(finish(plugin.as_mut(), rate, &[1, 23]));
    let expected = feed(reference.as_mut(), rate, &vec![0.0; 258 * channels], 127);
    assert_eq!(got, expected);
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
}

#[test]
fn eos_freezes_real_changes_but_idempotent_controls_and_reset_work() {
    for (lookahead, isp) in [(0.0, false), (5.0, true)] {
        let rate = 48_000;
        let mut plugin = make(rate, 2, settings(lookahead, isp, 1.0));
        feed(plugin.as_mut(), rate, &input_markers(1, 2), 1);
        let mut first = [0.0; 2];
        plugin
            .drain(&mut first, &ProcessContext::new(rate, 0))
            .unwrap();
        for parameter in plugin.parameters() {
            let id = parameter.id;
            let value = plugin.get_parameter(&id).unwrap();
            plugin.set_parameter(id.clone(), value.clone()).unwrap();
            assert_eq!(plugin.get_parameter(&id), Some(value));
        }
        assert!(
            plugin
                .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-8.0))
                .is_err()
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("threshold")),
            Some(ParameterValue::Float(-6.0))
        );
        let mut direct = [0.125; 2];
        let mut inner = LimiterPlugin::from_params(2, settings(lookahead, isp, 1.0));
        inner.initialize(rate).unwrap();
        inner
            .process_in_place(&mut [0.001, 0.002], &ProcessContext::new(rate, 1))
            .unwrap();
        inner
            .drain(&mut [0.0; 2], &ProcessContext::new(rate, 0))
            .unwrap();
        let values = inner.current_values();
        inner.apply_values(values.clone()).unwrap();
        let mut changed = values;
        changed.insert(ParameterId::from("threshold"), ParameterValue::Float(-8.0));
        assert!(inner.apply_values(changed).is_err());
        assert!(
            inner
                .process_in_place(&mut direct, &ProcessContext::new(rate, 1))
                .is_err()
        );
        assert_eq!(direct, [0.125; 2]);
        assert_eq!(
            inner
                .process_in_place(&mut [], &ProcessContext::new(rate, 0))
                .unwrap(),
            0
        );
        plugin.reset();
        let source = input_markers(377, 2);
        let mut fresh = make(rate, 2, settings(lookahead, isp, 1.0));
        assert_eq!(
            feed(plugin.as_mut(), rate, &source, 31),
            feed(fresh.as_mut(), rate, &source, 127)
        );
        assert_eq!(
            finish(plugin.as_mut(), rate, &[3]),
            finish(fresh.as_mut(), rate, &[127])
        );
        plugin.initialize(rate).unwrap();
        plugin
            .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-8.0))
            .unwrap();
        feed(plugin.as_mut(), rate, &source, 17);
    }
}

#[test]
fn empty_and_zero_delay_streams_do_not_invent_tail_samples() {
    for rate in [48_000, 192_000] {
        for isp in [false, true] {
            let lookahead = if isp && rate < 192_000 { 5.0 } else { 0.0 };
            let mut plugin = make(rate, 2, settings(lookahead, isp, 1.0));
            assert_eq!(
                plugin
                    .drain(&mut [], &ProcessContext::new(rate, 0))
                    .unwrap(),
                PluginDrainResult::COMPLETE
            );
            assert!(feed(plugin.as_mut(), rate, &[], 1).is_empty());
            plugin
                .set_parameter(ParameterId::from("release"), ParameterValue::Float(50.0))
                .unwrap();
            let source = input_markers(19, 2);
            let mut got = feed(plugin.as_mut(), rate, &source, 3);
            got.extend(finish(plugin.as_mut(), rate, &[1]));
            let mut expected = vec![0.0; expected_delay(rate, lookahead, isp) * 2];
            expected.extend_from_slice(&source);
            assert_eq!(got, expected);
        }
    }
    let mut uninitialized: Box<dyn Plugin> = Box::new(ParametricInPlacePluginAdapter::new(
        LimiterPlugin::new(2, -6.0, 10.0, 5.0, false),
    ));
    assert_eq!(uninitialized.tail_length(), TailLength::Unknown);
    assert!(
        uninitialized
            .drain(&mut [0.0; 2], &ProcessContext::new(44_100, 0))
            .is_err()
    );
}

#[test]
fn cold_process_drain_queries_controls_and_reset_neither_allocate_nor_free() {
    for channels in [1, 2, 6] {
        for (rate, lookahead, isp) in [
            (44_100, 5.0, false),
            (48_000, 20.0, true),
            (96_000, 5.0, true),
            (192_000, 0.0, true),
        ] {
            let mut plugin = make(rate, channels, settings(lookahead, isp, 1.0));
            let source = input_markers(121, channels);
            let mut output = vec![0.0; source.len()];
            let id = ParameterId::from("threshold");
            let mut drain = vec![0.0; 257 * channels];
            without_heap(|| {
                assert!(plugin.drain_call_bound().is_some());
                plugin
                    .set_parameter(id.clone(), ParameterValue::Float(-6.0))
                    .unwrap();
                assert_eq!(
                    plugin
                        .process(&source, &mut output, &ProcessContext::new(rate, 121))
                        .unwrap(),
                    121
                );
                assert_eq!(
                    plugin.tail_length(),
                    TailLength::Finite(expected_delay(rate, lookahead, isp) as u64)
                );
                let mut total = 0;
                loop {
                    assert!(plugin.drain_call_bound().is_some());
                    let result = plugin
                        .drain(&mut drain, &ProcessContext::new(rate, 0))
                        .unwrap();
                    total += result.frames;
                    plugin
                        .set_parameter(id.clone(), ParameterValue::Float(-6.0))
                        .unwrap();
                    if result.complete {
                        break;
                    }
                }
                assert_eq!(total, expected_delay(rate, lookahead, isp));
                plugin.reset();
                assert_eq!(
                    plugin
                        .process(&source, &mut output, &ProcessContext::new(rate, 121))
                        .unwrap(),
                    121
                );
            });
        }
    }
}

#[test]
fn rejected_input_neither_changes_audio_nor_enters_eos() {
    let mut plugin = LimiterPlugin::from_params(2, settings(5.0, true, 1.0));
    let mut block = [0.25; 4];
    assert!(
        plugin
            .process_in_place(&mut block, &ProcessContext::new(48_000, 2))
            .is_err()
    );
    plugin.initialize(48_000).unwrap();
    for context in [
        ProcessContext::new(44_100, 2),
        ProcessContext::new(48_000, 3),
        ProcessContext::new(48_000, usize::MAX),
    ] {
        assert!(plugin.process_in_place(&mut block, &context).is_err());
        assert_eq!(block, [0.25; 4]);
    }
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    plugin
        .parametric_set_parameter(ParameterId::from("release"), ParameterValue::Float(50.0))
        .unwrap();
    let mut reference = LimiterPlugin::from_params(2, settings(5.0, true, 1.0));
    reference.initialize(48_000).unwrap();
    reference
        .parametric_set_parameter(ParameterId::from("release"), ParameterValue::Float(50.0))
        .unwrap();
    let mut actual = input_markers(997, 2);
    let mut expected = actual.clone();
    plugin
        .process_in_place(&mut actual, &ProcessContext::new(48_000, 997))
        .unwrap();
    reference
        .process_in_place(&mut expected, &ProcessContext::new(48_000, 997))
        .unwrap();
    assert_eq!(actual, expected);
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
    let uninitialized = LimiterPlugin::from_params(2, settings(5., false, 1.));
    assert!(sotf_host::ParametricInPlacePlugin::drain_call_bound(&uninitialized).is_none());
    for rate in [44100, 192000] {
        for isp in [false, true] {
            // ISP needs at least six lookahead samples at low input rates.
            // Zero/tiny delay remain covered by the ordinary limiter cases.
            let lookaheads: &[f32] = if isp {
                &[1., 5., 20.]
            } else {
                &[0., 0.001, 5., 20.]
            };
            for &lookahead in lookaheads {
                for partial in [0, 1, 255] {
                    check_call_bound(make(rate, 2, settings(lookahead, isp, 1.)), rate, partial);
                }
            }
        }
    }
}
