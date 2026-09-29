//! Public finite-response oracles; recursive wet tails remain a separate policy.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePluginAdapter, Plugin, ProcessContext, TailLength};
use sotf_plugin_analog_limiter::{AnalogLimiterPlugin, AnalogLimiterPluginParams};

fn make(channels: usize, rate: u32, model: &str, lookahead: f32, color: f32) -> Box<dyn Plugin> {
    let params = AnalogLimiterPluginParams {
        threshold: -1.0,
        lookahead,
        analog_model: model.into(),
        analog_color: color,
        analog_drive: 7.0,
        analog_character: 0.8,
        analog_trim: -3.0,
        ..Default::default()
    };
    let mut plugin = ParametricInPlacePluginAdapter::new(
        AnalogLimiterPlugin::from_params(channels, params).unwrap(),
    );
    Plugin::initialize(&mut plugin, rate).unwrap();
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
fn zero_color_recovers_exact_delayed_program_for_every_model() {
    for model in [
        "Harmonics",
        "Static",
        "Hammerstein",
        "Tape",
        "Transformer",
        "Console Preamp",
    ] {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for channels in [1, 2, 6] {
                for lookahead in [1.0_f32, 0.0, 5.0, 20.0] {
                    // Native limiter quantizes these lookaheads by truncating to whole samples.
                    let delay = (f64::from(lookahead) * f64::from(rate) / 1000.0).floor() as usize;
                    let mut plugin = make(channels, rate, model, lookahead, 0.0);
                    let input: Vec<f32> = (0..121 * channels)
                        .map(|i| ((i * 7 % 23) as f32 - 11.0) / 64.0)
                        .collect();
                    let mut output = process(plugin.as_mut(), rate, &input);
                    output.extend(drain(plugin.as_mut(), rate, 17));
                    let mut expected = vec![0.0; delay * channels];
                    expected.extend_from_slice(&input);
                    assert_eq!(
                        output, expected,
                        "model={model}, rate={rate}, channels={channels}, delay={delay}"
                    );
                    assert_eq!(plugin.tail_length(), TailLength::Finite(delay as u64));
                }
            }
        }
    }
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
        for model in [
            "Harmonics",
            "Static",
            "Hammerstein",
            "Tape",
            "Transformer",
            "Console Preamp",
        ] {
            for lookahead in [0.0, 5.0] {
                cold_measure(make(channels, 48_000, model, lookahead, 0.0));
            }
        }
    }
}

#[test]
fn color_transitions_require_a_zero_color_reset_epoch() {
    use sotf_host::plugin::PluginDrainResult;
    use sotf_host::{ParameterId, ParameterValue};
    for model in [
        "Harmonics",
        "Static",
        "Hammerstein",
        "Tape",
        "Transformer",
        "Console Preamp",
    ] {
        let rate = 48_000;
        let mut plugin = make(2, rate, model, 1.0, 1.0);
        process(plugin.as_mut(), rate, &[0.25; 34]);
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        plugin
            .set_parameter(
                ParameterId::from("analog_color"),
                ParameterValue::Float(0.0),
            )
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .unwrap(),
            PluginDrainResult::COMPLETE
        );
        // Unsupported wet EOS has not frozen controls or accepted-input processing.
        plugin
            .set_parameter(
                ParameterId::from("analog_drive"),
                ParameterValue::Float(9.0),
            )
            .unwrap();
        process(plugin.as_mut(), rate, &[0.0; 100]);
        plugin.reset();
        assert_eq!(plugin.tail_length(), TailLength::Finite(48));
        // Even overwritten pending color targets conservatively invalidate proof.
        plugin
            .set_parameter(
                ParameterId::from("analog_color"),
                ParameterValue::Float(0.75),
            )
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from("analog_color"),
                ParameterValue::Float(0.0),
            )
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        plugin.reset();
        let input = [0.125; 34];
        let mut actual = process(plugin.as_mut(), rate, &input);
        actual.extend(drain(plugin.as_mut(), rate, 13));
        let mut expected = vec![0.0; 96];
        expected.extend_from_slice(&input);
        assert_eq!(actual, expected);
        plugin
            .set_parameter(
                ParameterId::from("analog_color"),
                ParameterValue::Float(0.0),
            )
            .unwrap();
        assert!(
            plugin
                .set_parameter(
                    ParameterId::from("analog_color"),
                    ParameterValue::Float(0.5)
                )
                .is_err()
        );
    }
}

#[test]
fn zero_color_model_replacement_preserves_core_history() {
    use sotf_host::{ParameterId, ParameterValue};
    let rate = 48_000;
    let mut plugin = make(2, rate, "Harmonics", 1.0, 0.0);
    let input = [0.25, -0.125, 0.5, -0.25];
    let mut actual = process(plugin.as_mut(), rate, &input);
    plugin
        .set_parameter(
            ParameterId::from("analog_model"),
            ParameterValue::String("Tape".into()),
        )
        .unwrap();
    assert_eq!(plugin.tail_length(), TailLength::Finite(48));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("analog_drive")),
        Some(ParameterValue::Float(7.0))
    );
    actual.extend(drain(plugin.as_mut(), rate, 1));
    let mut expected = vec![0.0; 96];
    expected.extend_from_slice(&input);
    assert_eq!(actual, expected);
    plugin
        .set_parameter(
            ParameterId::from("analog_model"),
            ParameterValue::String("Tape".into()),
        )
        .unwrap();
}

#[test]
fn errors_bulk_snapshots_and_reset_preserve_finite_history() {
    use sotf_host::parametric_plugin::ParameterSet;
    use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};
    let params = AnalogLimiterPluginParams {
        lookahead: 1.0,
        analog_color: 0.0,
        ..Default::default()
    };
    let mut plugin = AnalogLimiterPlugin::from_params(2, params).unwrap();
    plugin.initialize(48_000).unwrap();
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    let input = [0.25, -0.125];
    let mut first = input;
    plugin
        .process_in_place(&mut first, &ProcessContext::new(48_000, 1))
        .unwrap();
    let mut canary = [77.0; 35];
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut canary, &ProcessContext::new(48_000, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut canary[..34], &ProcessContext::new(96_000, 0))
            .is_err()
    );
    assert_eq!(canary, [77.0; 35]);
    let step = plugin
        .drain(&mut canary[..2], &ProcessContext::new(48_000, 0))
        .unwrap();
    assert_eq!(step.frames, 1);
    assert!(!step.complete);
    let old = plugin.current_values();
    plugin.apply_values(old.clone()).unwrap();
    let mut invalid: ParameterSet = old.clone();
    invalid.insert(ParameterId::from("threshold"), ParameterValue::Float(-12.0));
    invalid.insert(
        ParameterId::from("analog_color"),
        ParameterValue::Float(0.5),
    );
    assert!(plugin.apply_values(invalid).is_err());
    assert_eq!(plugin.current_values(), old);
    let mut tail = canary[..2].to_vec();
    while !plugin
        .drain(&mut canary[..2], &ProcessContext::new(48_000, 0))
        .unwrap()
        .complete
    {
        tail.extend_from_slice(&canary[..2]);
    }
    tail.extend_from_slice(&canary[..2]);
    let mut expected = vec![0.0; 47 * 2];
    expected.extend_from_slice(&input);
    assert_eq!(tail, expected);
    let mut rejected = input;
    assert!(
        plugin
            .process_in_place(&mut rejected, &ProcessContext::new(48_000, 1))
            .is_err()
    );
    assert_eq!(rejected, input);
    for reinitialize in [false, true] {
        if reinitialize {
            plugin.initialize(48_000).unwrap();
        } else {
            plugin.reset();
        }
        let mut block = input;
        plugin
            .process_in_place(&mut block, &ProcessContext::new(48_000, 1))
            .unwrap();
        assert_eq!(block, first);
        let mut all = vec![0.0; 48 * 2];
        let result = plugin
            .drain(&mut all, &ProcessContext::new(48_000, 0))
            .unwrap();
        assert_eq!(result.frames, 48);
        assert!(result.complete);
        assert_eq!(all, expected);
    }
}

#[test]
fn nonlinear_core_tail_preserves_the_ordinary_zero_continuation() {
    use sotf_host::{ParameterId, ParameterValue};
    for rate in [44_100, 96_000] {
        for channels in [1, 2, 6] {
            for (soft, true_peak, mix) in
                [(false, false, 1.0), (true, false, 0.4), (false, true, 1.0)]
            {
                let create = || {
                    let params = AnalogLimiterPluginParams {
                        threshold: -12.0,
                        release: 20.0,
                        lookahead: 5.0,
                        soft,
                        true_peak,
                        mix,
                        analog_color: 0.0,
                        ..Default::default()
                    };
                    let mut plugin = ParametricInPlacePluginAdapter::new(
                        AnalogLimiterPlugin::from_params(channels, params).unwrap(),
                    );
                    Plugin::initialize(&mut plugin, rate).unwrap();
                    Box::new(plugin) as Box<dyn Plugin>
                };
                let mut actual = create();
                let mut reference = create();
                let input: Vec<f32> = (0..713 * channels)
                    .map(|i| match i % 97 {
                        0..33 => 0.8,
                        33..71 => -0.5,
                        _ => 0.1,
                    })
                    .collect();
                assert_eq!(
                    process(actual.as_mut(), rate, &input),
                    process(reference.as_mut(), rate, &input)
                );
                for plugin in [&mut actual, &mut reference] {
                    plugin
                        .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-10.0))
                        .unwrap();
                }
                let delay = actual.latency_samples();
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
            }
        }
    }
}

#[test]
fn empty_and_zero_delay_draining_only_freezes_an_accepted_stream() {
    use sotf_host::{ParameterId, ParameterValue};
    let mut plugin = make(1, 48_000, "Static", 0.0, 0.0);
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    plugin
        .set_parameter(
            ParameterId::from("analog_drive"),
            ParameterValue::Float(9.0),
        )
        .unwrap();
    process(plugin.as_mut(), 48_000, &[0.25]);
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    plugin
        .set_parameter(
            ParameterId::from("analog_drive"),
            ParameterValue::Float(9.0),
        )
        .unwrap();
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("analog_drive"),
                ParameterValue::Float(3.0)
            )
            .is_err()
    );
    let mut output = [91.0];
    assert!(
        plugin
            .process(&[0.25], &mut output, &ProcessContext::new(48_000, 1))
            .is_err()
    );
    // Generic adapter copies input before an inner error; no DSP state advances.
    plugin.reset();
    assert_eq!(process(plugin.as_mut(), 48_000, &[0.25]), vec![0.25]);
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
        AnalogLimiterPlugin::from_params(2, AnalogLimiterPluginParams::default()).unwrap();
    assert!(sotf_host::ParametricInPlacePlugin::drain_call_bound(&uninitialized).is_none());
    for rate in [44100, 192000] {
        for color in [0., 1.] {
            for lookahead in [0., 5., 20.] {
                for partial in [0, 1, 255] {
                    check_call_bound(make(2, rate, "Tape", lookahead, color), rate, partial);
                }
            }
        }
    }
}
