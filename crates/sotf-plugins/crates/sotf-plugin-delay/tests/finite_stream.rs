//! Finite delay continuation compared with independent taps and zero padding.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_delay::DelayPlugin;
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

#[test]
fn drain_retains_the_final_input_impulse() {
    let mut plugin = DelayPlugin::try_new_with_max_delay(2, 10.0, 0.0, 1.0, 20.0).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let mut input = [0.0; 34];
    input[32] = 0.75;
    input[33] = -0.25;
    plugin
        .process_in_place(&mut input, &ProcessContext::new(48_000, 17))
        .unwrap();
    let TailLength::Finite(bound) = plugin.tail_length() else {
        panic!("finite delay")
    };
    let mut output = input.to_vec();
    let mut destination = [999.0; 34];
    loop {
        let result = plugin
            .drain(&mut destination, &ProcessContext::new(48_000, 0))
            .unwrap();
        output.extend_from_slice(&destination[..result.frames * 2]);
        if result.complete {
            break;
        }
    }
    assert_eq!(output.len(), (17 + bound as usize) * 2);
    assert_eq!(output[496 * 2], 0.75);
    assert_eq!(output[496 * 2 + 1], -0.25);
}

#[test]
fn plugin_adapter_forwards_the_finite_tail_with_program_dimensions() {
    use sotf_host::{ParametricInPlacePluginAdapter, Plugin};
    let raw = DelayPlugin::try_new_with_max_delay(2, 5.0, 0.0, 1.0, 10.0).unwrap();
    let mut plugin: Box<dyn Plugin> = Box::new(ParametricInPlacePluginAdapter::new(raw));
    plugin.initialize(48_000.0).unwrap();
    let input = [0.75, -0.25];
    let mut first = [0.0; 2];
    plugin
        .process(&input, &mut first, &ProcessContext::new(48_000, 1))
        .unwrap();
    let mut output = first.to_vec();
    let mut tail = [0.0; 34];
    loop {
        let result = plugin
            .drain(&mut tail, &ProcessContext::new(48_000, 0))
            .unwrap();
        output.extend_from_slice(&tail[..result.frames * 2]);
        if result.complete {
            break;
        }
    }
    assert_eq!(&output[240 * 2..241 * 2], &input);
    assert_eq!(output.len(), 2 * (1 + 512));
}

fn finite_bound(plugin: &DelayPlugin) -> usize {
    let TailLength::Finite(bound) = plugin.tail_length() else {
        panic!("nonrecursive delay must have finite support");
    };
    bound as usize
}

fn feed(plugin: &mut DelayPlugin, rate: u32, source: &[f32], block: usize) -> Vec<f32> {
    let channels = plugin.channels();
    let mut output = source.to_vec();
    for samples in output.chunks_mut(block * channels) {
        plugin
            .process_in_place(
                samples,
                &ProcessContext::new(rate, samples.len() / channels),
            )
            .unwrap();
    }
    output
}

fn finish(plugin: &mut DelayPlugin, rate: u32, capacity: usize) -> Vec<f32> {
    let channels = plugin.channels();
    let bound = finite_bound(plugin);
    let mut destination = vec![123.0; capacity * channels];
    let mut output = Vec::new();
    loop {
        destination.fill(123.0);
        // The destination, not context.num_frames, controls drain capacity.
        let result = plugin
            .drain(&mut destination, &ProcessContext::new(rate, usize::MAX))
            .unwrap();
        assert!(result.frames <= capacity && result.frames <= plugin.drain_output_frames_max());
        assert!(
            destination[result.frames * channels..]
                .iter()
                .all(|&v| v == 123.0)
        );
        output.extend_from_slice(&destination[..result.frames * channels]);
        assert!(output.len() <= bound * channels);
        if result.complete {
            break;
        }
        assert!(result.frames > 0);
    }
    assert_eq!(output.len(), bound * channels);
    destination.fill(123.0);
    assert_eq!(
        plugin
            .drain(&mut destination, &ProcessContext::new(rate, 0))
            .unwrap(),
        sotf_host::plugin::PluginDrainResult::COMPLETE
    );
    assert!(destination.iter().all(|&v| v == 123.0));
    output
}

#[test]
fn static_taps_match_an_independent_fractional_delay_sum() {
    for delay in [0.0_f32, 1.0, 2.0, 2.25, 17.5, 63.75] {
        for mix in [0.0_f32, 0.375, 1.0] {
            let source: Vec<f32> = (0..107)
                .flat_map(|frame| {
                    [
                        ((frame * 13 % 31) as f32 - 15.0) / 64.0,
                        if frame == 106 { -0.75 } else { 0.0 },
                    ]
                })
                .collect();
            for capacity in [1, 17, 1024, 4097] {
                // At 1 kHz the delay in milliseconds equals its sample count.
                let mut plugin =
                    DelayPlugin::try_new_with_max_delay(2, delay, 0.0, mix, 64.0).unwrap();
                plugin.initialize(1000.0).unwrap();
                let mut output = feed(&mut plugin, 1000, &source, 17);
                output.extend(finish(&mut plugin, 1000, capacity));
                let integer = delay.floor() as isize;
                let fraction = f64::from(delay.fract());
                for (frame, pair) in output.as_chunks::<2>().0.iter().enumerate() {
                    for (channel, &actual) in pair.iter().enumerate() {
                        let sample = |lag: isize| -> f64 {
                            let index = frame as isize - lag;
                            if index < 0 {
                                0.0
                            } else {
                                source
                                    .get(index as usize * 2 + channel)
                                    .copied()
                                    .map(f64::from)
                                    .unwrap_or(0.0)
                            }
                        };
                        let wet = if fraction == 0.0 {
                            sample(integer)
                        } else {
                            // Lagrange basis from its defining product, independent
                            // of the plugin's expanded four-coefficient formula.
                            [-1_i32, 0, 1, 2]
                                .into_iter()
                                .map(|node| {
                                    let weight = [-1_i32, 0, 1, 2]
                                        .into_iter()
                                        .filter(|&other| other != node)
                                        .map(|other| {
                                            (fraction - f64::from(other)) / f64::from(node - other)
                                        })
                                        .product::<f64>();
                                    weight * sample(integer + node as isize)
                                })
                                .sum::<f64>()
                        };
                        let expected = f64::from(1.0 - mix) * sample(0) + f64::from(mix) * wet;
                        assert!(
                            (f64::from(actual) - expected).abs() < 2e-6,
                            "delay={delay} mix={mix} frame={frame} channel={channel}: {actual} != {expected}"
                        );
                    }
                }
            }
        }
    }
}

fn moving_delay(rate: u32, clean: bool, per_channel: bool) -> DelayPlugin {
    let mut plugin = if per_channel {
        DelayPlugin::new_per_channel_with_max_delay(vec![1.25, 7.75], 20.0).unwrap()
    } else {
        DelayPlugin::try_new_with_max_delay(2, 7.75, 0.0, 0.75, 20.0).unwrap()
    };
    if !per_channel {
        plugin
            .parametric_set_parameter(
                "pitch_preserving".into(),
                sotf_host::ParameterValue::Bool(clean),
            )
            .unwrap();
        plugin
            .parametric_set_parameter(
                "allpass_feedback".into(),
                sotf_host::ParameterValue::Bool(true),
            )
            .unwrap();
        if !clean {
            plugin
                .parametric_set_parameter(
                    "lfo_rate_hz".into(),
                    sotf_host::ParameterValue::Float(7.0),
                )
                .unwrap();
            plugin
                .parametric_set_parameter(
                    "lfo_depth_ms".into(),
                    sotf_host::ParameterValue::Float(3.0),
                )
                .unwrap();
        }
    }
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

#[test]
fn modulation_pending_automation_and_large_blocks_match_zero_continuation() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for (clean, per_channel) in [(false, false), (true, false), (false, true)] {
            let source: Vec<f32> = (0..17003)
                .flat_map(|frame| {
                    [
                        ((frame * 11 % 73) as f32 - 36.0) / 128.0,
                        ((frame * 7 % 61) as f32 - 30.0) / 256.0,
                    ]
                })
                .collect();
            for (block, capacity) in [
                (1, 17),
                (255, 1),
                (1023, 1024),
                (1024, 255),
                (1025, 4097),
                (17003, 1024),
            ] {
                let mut plugin = moving_delay(rate, clean, per_channel);
                let mut reference = moving_delay(rate, clean, per_channel);
                let mut output = feed(&mut plugin, rate, &source[..source.len() - 34], block);
                let mut expected = feed(&mut reference, rate, &source[..source.len() - 34], 257);
                let id = if per_channel {
                    "delay_ms_1"
                } else {
                    "delay_ms"
                };
                for p in [&mut plugin, &mut reference] {
                    p.parametric_set_parameter(id.into(), sotf_host::ParameterValue::Float(15.5))
                        .unwrap();
                }
                output.extend(feed(&mut plugin, rate, &source[source.len() - 34..], block));
                expected.extend(feed(
                    &mut reference,
                    rate,
                    &source[source.len() - 34..],
                    257,
                ));
                let bound = finite_bound(&plugin);
                output.extend(finish(&mut plugin, rate, capacity));
                expected.extend(feed(&mut reference, rate, &vec![0.0; bound * 2], 31));
                assert_eq!(
                    output, expected,
                    "rate={rate} clean={clean} per_channel={per_channel} block={block} cap={capacity}"
                );
                let silence = feed(&mut reference, rate, &[0.0; 258], 17);
                assert!(silence.iter().all(|&sample| sample == 0.0));
            }
        }
    }
}

#[test]
fn invalid_attempts_preserve_audio_and_controls_then_reset_rearms() {
    use sotf_host::{ParameterId, ParameterValue};
    let rate = 48_000;
    let mut plugin = moving_delay(rate, true, false);
    let mut reference = moving_delay(rate, true, false);
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        sotf_host::plugin::PluginDrainResult::COMPLETE
    );
    let source = vec![0.5; 34];
    assert_eq!(
        feed(&mut plugin, rate, &source, 17),
        feed(&mut reference, rate, &source, 17)
    );
    for (length, sample_rate) in [(0, rate), (3, rate), (34, 44_100)] {
        let mut output = vec![321.0; length];
        assert!(
            plugin
                .drain(&mut output, &ProcessContext::new(sample_rate, 0))
                .is_err()
        );
        assert!(output.iter().all(|&v| v == 321.0));
    }
    // A failed drain must not freeze controls or consume stream state.
    for p in [&mut plugin, &mut reference] {
        p.parametric_set_parameter("mix".into(), ParameterValue::Float(0.25))
            .unwrap();
    }
    let mut first = [0.0; 2];
    plugin
        .drain(&mut first, &ProcessContext::new(rate, 0))
        .unwrap();
    let mut reference_first = [0.0; 2];
    reference
        .drain(&mut reference_first, &ProcessContext::new(rate, 0))
        .unwrap();
    assert_eq!(first, reference_first);
    let snapshot = plugin.current_values();
    plugin.apply_values(snapshot.clone()).unwrap();
    for (id, value) in &snapshot {
        plugin
            .parametric_set_parameter(id.clone(), value.clone())
            .unwrap();
    }
    let mut changed = snapshot.clone();
    changed.insert(ParameterId::from("mix"), ParameterValue::Float(0.5));
    assert!(plugin.apply_values(changed).is_err());
    assert!(
        plugin
            .parametric_set_parameter("feedback".into(), ParameterValue::Float(0.5))
            .is_err()
    );
    assert_eq!(plugin.current_values(), snapshot);
    let mut input = [0.5; 2];
    assert!(
        plugin
            .process_in_place(&mut input, &ProcessContext::new(rate, 1))
            .is_err()
    );
    assert_eq!(input, [0.5; 2]);
    assert_eq!(
        plugin
            .process_in_place(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        0
    );
    let mut tail_a = [0.0; 34];
    let mut tail_b = [0.0; 34];
    loop {
        let a = plugin
            .drain(&mut tail_a, &ProcessContext::new(rate, 0))
            .unwrap();
        let b = reference
            .drain(&mut tail_b, &ProcessContext::new(rate, 0))
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(tail_a, tail_b);
        if a.complete {
            break;
        }
    }
    for reinitialize in [false, true] {
        if reinitialize {
            plugin.initialize(f64::from(rate)).unwrap();
        } else {
            plugin.reset();
        }
        let mut fresh = moving_delay(rate, true, false);
        fresh
            .parametric_set_parameter("mix".into(), ParameterValue::Float(0.25))
            .unwrap();
        fresh.reset();
        assert_eq!(
            feed(&mut plugin, rate, &source, 17),
            feed(&mut fresh, rate, &source, 17)
        );
        finish(&mut plugin, rate, 17);
    }
}

#[test]
fn recursive_history_preserves_the_existing_unsupported_drain_policy() {
    use sotf_host::{ParameterValue, plugin::PluginDrainResult};
    for feedback in [-0.5, 0.5] {
        let mut plugin = DelayPlugin::try_new_with_max_delay(1, 1.0, feedback, 1.0, 20.0).unwrap();
        plugin.initialize(48_000.0).unwrap();
        feed(&mut plugin, 48_000, &[1.0], 1);
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        assert_eq!(plugin.drain_output_frames_max(), 0);
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap(),
            PluginDrainResult::COMPLETE
        );
        plugin
            .parametric_set_parameter("feedback".into(), ParameterValue::Float(0.0))
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap(),
            PluginDrainResult::COMPLETE
        );
        plugin.reset();
        assert!(matches!(plugin.tail_length(), TailLength::Finite(_)));
    }
}

#[test]
fn cold_process_and_drain_release_no_heap_storage() {
    for channels in [1, 2, 6] {
        for drain_on_new_thread in [false, true] {
            let rate = 192_000;
            let mut plugin =
                DelayPlugin::try_new_with_max_delay(channels, 7.25, 0.0, 0.75, 20.0).unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
            let mut input = vec![0.25; 17 * channels];
            let mut output = vec![0.0; 1025 * channels];
            let current_mix = sotf_host::ParameterId::from("mix");
            if drain_on_new_thread {
                plugin
                    .process_in_place(&mut input, &ProcessContext::new(rate, 17))
                    .unwrap();
            }
            std::thread::spawn(move || {
                without_heap(|| {
                    if !drain_on_new_thread {
                        plugin
                            .process_in_place(&mut input, &ProcessContext::new(rate, 17))
                            .unwrap();
                    }
                    loop {
                        let result = plugin
                            .drain(&mut output, &ProcessContext::new(rate, 0))
                            .unwrap();
                        plugin
                            .parametric_set_parameter(
                                current_mix.clone(),
                                sotf_host::ParameterValue::Float(0.75),
                            )
                            .unwrap();
                        if result.complete {
                            break;
                        }
                    }
                    plugin.reset();
                    plugin
                        .process_in_place(&mut input, &ProcessContext::new(rate, 17))
                        .unwrap();
                });
            })
            .join()
            .unwrap();
        }
    }
}
