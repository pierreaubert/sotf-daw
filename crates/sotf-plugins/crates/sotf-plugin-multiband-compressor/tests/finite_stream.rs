//! Public finite-response oracles; recursive wet tails remain a separate policy.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePluginAdapter, Plugin, ProcessContext, TailLength};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

fn make(channels: usize, rate: u32, bands: usize, lookahead_ms: f32, mix: f32) -> Box<dyn Plugin> {
    let params = MultibandCompressorPluginParams {
        num_bands: bands,
        per_band_lookahead_ms: lookahead_ms,
        mix,
        ..Default::default()
    };
    let mut plugin = ParametricInPlacePluginAdapter::new(MultibandCompressorPlugin::from_params(
        channels, params,
    ));
    Plugin::initialize(&mut plugin, f64::from(rate)).unwrap();
    Box::new(plugin)
}

fn process(plugin: &mut dyn Plugin, rate: u32, input: &[f32]) -> Vec<f32> {
    let channels = plugin.input_channels();
    let mut output = vec![0.0; input.len()];
    let mut offset = 0;
    let frames = input.len() / channels;
    for size in [1, 17, 97].into_iter().cycle() {
        if offset == frames {
            break;
        }
        let count = size.min(frames - offset);
        let span = offset * channels..(offset + count) * channels;
        assert_eq!(
            plugin
                .process(
                    &input[span.clone()],
                    &mut output[span],
                    &ProcessContext::new(rate, count)
                )
                .unwrap(),
            count
        );
        offset += count;
    }
    output
}

fn drain(plugin: &mut dyn Plugin, rate: u32, capacity: usize) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut result = Vec::new();
    for _ in 0..20_000 {
        let mut buffer = vec![123.0; capacity * channels + 3];
        let step = plugin
            .drain(
                &mut buffer[..capacity * channels],
                &ProcessContext::new(rate, 0),
            )
            .unwrap();
        assert!(step.frames <= capacity && step.frames <= plugin.drain_output_frames_max());
        assert!(buffer[step.frames * channels..].iter().all(|x| *x == 123.0));
        result.extend_from_slice(&buffer[..step.frames * channels]);
        if step.complete {
            return result;
        }
    }
    panic!("finite response did not complete");
}

#[test]
fn dry_output_preserves_first_and_final_samples_through_eos() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for lookahead_ms in [0.001_f32, 0.0, 5.0, 20.0] {
                let delay = if lookahead_ms == 0.0 {
                    0
                } else {
                    ((f64::from(lookahead_ms) * f64::from(rate) / 1000.0).round() as usize).max(1)
                };
                for frames in [1, 121, delay + 31] {
                    let mut plugin = make(channels, rate, 3, lookahead_ms, 0.0);
                    let input: Vec<f32> = (0..frames * channels)
                        .map(|i| ((i * 7 % 23) as f32 - 11.0) / 64.0)
                        .collect();
                    let mut output = process(plugin.as_mut(), rate, &input);
                    output.extend(drain(plugin.as_mut(), rate, 17));
                    let mut expected = vec![0.0; delay * channels];
                    expected.extend_from_slice(&input);
                    assert_eq!(
                        output, expected,
                        "rate={rate}, channels={channels}, delay={delay}, frames={frames}"
                    );
                    assert_eq!(plugin.tail_length(), TailLength::Finite(delay as u64));
                }
            }
        }
    }
}

#[test]
fn one_band_wet_tail_equals_zero_continuation_and_then_stays_silent() {
    for rate in [44_100, 96_000] {
        for channels in [1, 2, 6] {
            for lookahead in [0.001, 5.0, 20.0] {
                let mut actual = make(channels, rate, 1, lookahead, 1.0);
                let mut reference = make(channels, rate, 1, lookahead, 1.0);
                let delay = actual.latency_samples();
                let input: Vec<f32> = (0..713 * channels)
                    .map(|i| if i % 97 < 33 { 0.55 } else { 0.0003 })
                    .collect();
                assert_eq!(
                    process(actual.as_mut(), rate, &input),
                    process(reference.as_mut(), rate, &input)
                );
                for plugin in [&mut actual, &mut reference] {
                    plugin
                        .set_parameter(
                            sotf_host::ParameterId::from("threshold"),
                            sotf_host::ParameterValue::Float(-27.0),
                        )
                        .unwrap();
                }
                let expected = process(
                    reference.as_mut(),
                    rate,
                    &vec![0.0; (delay + 64) * channels],
                );
                assert_eq!(
                    drain(actual.as_mut(), rate, 1),
                    expected[..delay * channels]
                );
                assert!(expected[delay * channels..].iter().all(|x| *x == 0.0));
                assert_eq!(actual.tail_length(), TailLength::Finite(delay as u64));
            }
        }
    }
}

#[test]
fn recursive_wet_keeps_legacy_eos_until_mix_is_actually_dry() {
    use sotf_host::plugin::PluginDrainResult;
    use sotf_host::{ParameterId, ParameterValue};
    let rate = 48_000;
    let mut plugin = make(2, rate, 3, 5.0, 1.0);
    process(plugin.as_mut(), rate, &[0.25; 26]);
    assert_eq!(plugin.tail_length(), TailLength::Infinite);
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    plugin
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
        .unwrap();
    assert_eq!(
        plugin.tail_length(),
        TailLength::Infinite,
        "a target-only fade is still recursive"
    );
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    for _ in 0..200 {
        process(plugin.as_mut(), rate, &[0.0; 1024]);
        if matches!(plugin.tail_length(), TailLength::Finite(_)) {
            break;
        }
    }
    assert_eq!(plugin.tail_length(), TailLength::Finite(240));
    // Refill the dry ring after the fade to prove the accepted finite horizon.
    let input = [0.125; 34];
    let mut actual = process(plugin.as_mut(), rate, &input);
    actual.extend(drain(plugin.as_mut(), rate, 13));
    let mut expected = vec![0.0; 240 * 2];
    expected.extend_from_slice(&input);
    assert_eq!(actual, expected);
    plugin
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
        .unwrap();
    assert!(
        plugin
            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
            .is_err()
    );
}

#[test]
fn errors_are_retryable_and_supported_eos_requires_reset() {
    use sotf_host::plugin::PluginDrainResult;
    use sotf_host::{ParameterId, ParameterValue};
    let rate = 48_000;
    let input = [0.25, -0.125, 0.5, -0.25];
    let mut actual = make(2, rate, 3, 5.0, 0.0);
    let mut reference = make(2, rate, 3, 5.0, 0.0);
    assert_eq!(
        actual
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    let first = process(actual.as_mut(), rate, &input);
    assert_eq!(first, process(reference.as_mut(), rate, &input));
    let mut canary = [77.0; 35];
    assert!(
        actual
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        actual
            .drain(&mut canary, &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        actual
            .drain(&mut canary[..34], &ProcessContext::new(44_100, 0))
            .is_err()
    );
    assert_eq!(canary, [77.0; 35]);
    let threshold = actual
        .get_parameter(&ParameterId::from("threshold"))
        .unwrap();
    actual
        .set_parameter(ParameterId::from("threshold"), threshold.clone())
        .unwrap();
    let mut prefix = [77.0; 2];
    let step = actual
        .drain(&mut prefix, &ProcessContext::new(rate, 0))
        .unwrap();
    assert_eq!(step.frames, 1);
    assert!(!step.complete);
    assert!(
        actual
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        actual
            .drain(&mut canary[..34], &ProcessContext::new(96_000, 0))
            .is_err()
    );
    actual
        .set_parameter(ParameterId::from("threshold"), threshold)
        .unwrap();
    assert!(
        actual
            .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-33.0))
            .is_err()
    );
    let mut tail = prefix.to_vec();
    tail.extend(drain(actual.as_mut(), rate, 17));
    assert_eq!(tail, drain(reference.as_mut(), rate, 256));
    assert!(
        actual
            .process(&input, &mut canary[..4], &ProcessContext::new(rate, 2))
            .is_err()
    );
    // The generic in-place adapter copies input before an inner processing
    // error. Plugin::process does not promise output preservation on Err;
    // the samples outside its destination remain protected.
    assert_eq!(&canary[4..], &[77.0; 31]);
    assert_eq!(
        actual
            .process(&[], &mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        0
    );
    assert_eq!(
        actual
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    actual.reset();
    assert_eq!(first, process(actual.as_mut(), rate, &input));
    assert_eq!(tail, drain(actual.as_mut(), rate, 7));
    actual.initialize(f64::from(rate)).unwrap();
    assert_eq!(first, process(actual.as_mut(), rate, &input));
    assert_eq!(tail, drain(actual.as_mut(), rate, 257));
}

#[test]
fn supported_bulk_snapshots_and_zero_delay_keep_the_eos_contract() {
    use sotf_host::parametric_plugin::ParameterSet;
    use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};
    let params = MultibandCompressorPluginParams {
        num_bands: 1,
        mix: 0.0,
        ..Default::default()
    };
    let mut plugin = MultibandCompressorPlugin::from_params(1, params);
    plugin.initialize(48_000.0).unwrap();
    plugin
        .process_in_place(&mut [0.25], &ProcessContext::new(48_000, 1))
        .unwrap();
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    let old = plugin.current_values();
    plugin.apply_values(old.clone()).unwrap();
    let mut changed: ParameterSet = old.clone();
    changed.insert(ParameterId::from("threshold"), ParameterValue::Float(-33.0));
    changed.insert(ParameterId::from("mix"), ParameterValue::Float(0.5));
    assert!(plugin.apply_values(changed).is_err());
    assert_eq!(plugin.current_values(), old);
    let mut rejected = [0.25];
    assert!(
        plugin
            .process_in_place(&mut rejected, &ProcessContext::new(48_000, 1))
            .is_err()
    );
    assert_eq!(rejected, [0.25]);
    let mut compiled_output = [19.0];
    assert!(
        plugin
            .process_compiled_f32(
                sotf_host::plugin::PluginCompiledOp::MultibandCompressor,
                &[0.5],
                &mut compiled_output,
                &ProcessContext::new(48_000, 1)
            )
            .unwrap()
            .is_err()
    );
    assert_eq!(compiled_output, [19.0]);
}

mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static TRACK: Cell<bool> = const { Cell::new(false) };
        static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    }
    struct Count;
    // SAFETY: Forward each allocation and release unchanged to System. The
    // constant thread-local counters allocate nothing and do not inspect memory.
    unsafe impl GlobalAlloc for Count {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                let _ = COUNTS.try_with(|c| {
                    let (a, d) = c.get();
                    c.set((a + 1, d));
                });
            }
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                let _ = COUNTS.try_with(|c| {
                    let (a, d) = c.get();
                    c.set((a, d + 1));
                });
            }
            // SAFETY: Forward the original pointer and matching allocation layout.
            unsafe { System.dealloc(ptr, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Count = Count;
    pub fn measure(run: impl FnOnce()) -> (usize, usize) {
        COUNTS.set((0, 0));
        TRACK.set(true);
        run();
        TRACK.set(false);
        COUNTS.get()
    }
}

fn cold_measure(mut plugin: Box<dyn Plugin>) {
    let channels = plugin.input_channels();
    let input = vec![0.125; 10003 * channels];
    let mut output = vec![0.0; input.len()];
    let mut tail = vec![123.0; 17 * channels];
    let counts = std::thread::spawn(move || {
        heap::measure(|| {
            let context = ProcessContext::new(48_000, 10003);
            plugin.process(&input, &mut output, &context).unwrap();
            let _ = plugin.tail_length();
            let _ = plugin.drain_output_frames_max();
            assert!(plugin.drain_call_bound().is_some());
            loop {
                if plugin
                    .drain(&mut tail, &ProcessContext::new(48_000, 0))
                    .unwrap()
                    .complete
                {
                    break;
                }
            }
            plugin.reset();
            plugin.process(&input, &mut output, &context).unwrap();
        })
    })
    .join()
    .unwrap();
    assert_eq!(counts, (0, 0));
}

#[test]
fn cold_finite_process_drain_query_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2, 6] {
        for (bands, mix) in [(1, 1.0), (3, 0.0)] {
            for lookahead in [0.0, 0.001, 5.0] {
                cold_measure(make(channels, 48_000, bands, lookahead, mix));
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
        MultibandCompressorPlugin::from_params(2, MultibandCompressorPluginParams::default());
    assert!(sotf_host::ParametricInPlacePlugin::drain_call_bound(&uninitialized).is_none());
    for rate in [44100, 192000] {
        for (bands, mix) in [(1, 1.), (3, 0.), (3, 1.)] {
            for lookahead in [0., 0.001, 5., 20.] {
                for partial in [0, 1, 255] {
                    check_call_bound(make(2, rate, bands, lookahead, mix), rate, partial);
                }
            }
        }
    }
}
