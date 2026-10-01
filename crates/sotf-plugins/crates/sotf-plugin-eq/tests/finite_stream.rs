//! Finite empty-bank EQ output is preserved through its public adapter.

// Rust guideline compliant 2026-02-21

use math_audio_iir_fir::{Biquad, BiquadFilterType};
use sotf_host::plugin::{PluginCompiledOp, PluginDrainResult};
use sotf_host::{
    ParameterId, ParameterSet, ParameterValue, ParametricPlugin, Plugin, ProcessContext, TailLength,
};
use sotf_plugin_eq::{BiquadFilterConfig, EqFilterTopology, EqPlugin, EqPluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountAlloc;

// SAFETY: Requests are forwarded unchanged to System. Const TLS counters do
// not allocate, retain pointers, or access the allocation contents.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|counts| {
                let (a, d) = counts.get();
                counts.set((a + 1, d));
            });
        }
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|counts| {
                let (a, d) = counts.get();
                counts.set((a, d + 1));
            });
        }
        // SAFETY: Pointer and layout match an allocation forwarded to System.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

fn make(factor: i32, channels: usize) -> Box<dyn Plugin> {
    let mut plugin = EqPlugin::new(channels, Vec::new()).into_boxed_plugin();
    plugin
        .set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(factor),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    plugin.initialize(48_000).unwrap();
    plugin
}

fn process(plugin: &mut dyn Plugin, input: &[f32]) -> Vec<f32> {
    let mut output = vec![123.0; input.len()];
    let frames = input.len() / plugin.input_channels();
    assert_eq!(
        plugin
            .process(input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap(),
        frames
    );
    output
}

fn drain(plugin: &mut dyn Plugin, capacity: usize) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut result = Vec::new();
    for _ in 0..2048 {
        let mut output = vec![123.0; capacity * channels + 3];
        let drained = plugin
            .drain(
                &mut output[..capacity * channels],
                &ProcessContext::new(48_000, 0),
            )
            .unwrap();
        assert!(drained.frames <= capacity && drained.frames <= plugin.drain_output_frames_max());
        assert!(
            output[drained.frames * channels..]
                .iter()
                .all(|&sample| sample == 123.0)
        );
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            return result;
        }
    }
    panic!("EQ finite drain did not complete");
}

#[test]
fn final_marker_survives_empty_bank_oversampling() {
    for factor in [2, 4] {
        let mut candidate = make(factor, 2);
        let mut reference = make(factor, 2);
        let input = [0.5, -0.25];
        assert_eq!(
            process(candidate.as_mut(), &input),
            process(reference.as_mut(), &input)
        );
        let mut expected = Vec::new();
        for _ in 0..4 {
            expected.extend(process(reference.as_mut(), &[0.0; 512]));
        }
        assert!(expected.iter().any(|sample| sample.abs() > 0.1));
        let mut actual = Vec::new();
        for _ in 0..2048 {
            let mut output = [123.0; 34];
            let result = candidate
                .drain(&mut output, &ProcessContext::new(48_000, 0))
                .unwrap();
            actual.extend_from_slice(&output[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        assert_eq!(actual.len(), expected.len(), "factor {factor}");
        assert_eq!(actual, expected, "factor {factor}");
    }
}

#[test]
fn canonical_tail_preserves_autogain_and_is_independent_of_destination_capacity() {
    for factor in [2, 4] {
        for channels in [1, 2] {
            for enabled in [false, true] {
                for frames in [1, 255, 256, 257, 4096] {
                    let input: Vec<f32> = (0..frames * channels)
                        .map(|index| ((index * 13 % 31) as f32 - 15.0) / 32.0)
                        .collect();
                    let mut expected = None;
                    for capacity in [1, 17, 256, 257, 4097] {
                        let mut candidate = make(factor, channels);
                        let mut reference = make(factor, channels);
                        for plugin in [&mut candidate, &mut reference] {
                            plugin
                                .set_parameter(
                                    ParameterId::from("auto_gain_enabled"),
                                    ParameterValue::Bool(enabled),
                                )
                                .unwrap();
                            // Nine input callbacks make the first canonical drain
                            // refill cross the existing ten-callback meter cadence.
                            for _ in 0..8 {
                                process(plugin.as_mut(), &vec![0.125; channels]);
                            }
                        }
                        assert_eq!(
                            process(candidate.as_mut(), &input),
                            process(reference.as_mut(), &input)
                        );
                        assert_eq!(candidate.tail_length(), TailLength::Finite(1024));
                        let mut padded = Vec::new();
                        for _ in 0..4 {
                            padded.extend(process(reference.as_mut(), &vec![0.0; 256 * channels]));
                        }
                        let actual = drain(candidate.as_mut(), capacity);
                        assert_eq!(actual.len(), 1024 * channels);
                        assert_eq!(
                            actual, padded,
                            "factor{factor}/channels{channels}/enabled{enabled}/frames{frames}/capacity{capacity}"
                        );
                        if let Some(expected) = &expected {
                            assert_eq!(&actual, expected);
                        }
                        expected = Some(actual);
                        let remainder = process(reference.as_mut(), &vec![0.0; 4096 * channels]);
                        assert!(remainder.iter().all(|sample| sample.abs() < 1e-7));
                    }
                }
            }
        }
    }
}

#[test]
fn empty_bank_audio_is_invariant_across_process_partitions() {
    for factor in [2, 4] {
        let input: Vec<f32> = (0..8191 * 2)
            .map(|index| ((index * 7 % 29) as f32 - 14.0) / 32.0)
            .collect();
        let mut expected = None;
        for blocks in [&[4096, 4095][..], &[1, 17, 255, 31, 1024][..]] {
            let mut plugin = make(factor, 2);
            let mut actual = Vec::new();
            let mut offset = 0;
            for &block in blocks.iter().cycle() {
                let frames = block.min(8191 - offset);
                if frames == 0 {
                    break;
                }
                actual.extend(process(
                    plugin.as_mut(),
                    &input[offset * 2..(offset + frames) * 2],
                ));
                offset += frames;
            }
            actual.extend(drain(plugin.as_mut(), 17));
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            }
            expected = Some(actual);
        }
    }
}

#[test]
fn invalid_drain_and_frozen_controls_are_transactional_and_reset_replays() {
    for factor in [2, 4] {
        let mut plugin = make(factor, 2);
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap(),
            PluginDrainResult::COMPLETE
        );
        let input = [0.5, -0.25];
        let prefix = process(plugin.as_mut(), &input);
        let mut canary = [123.0; 6];
        for capacity in [0, 1, 3] {
            assert!(
                plugin
                    .drain(&mut canary[..capacity], &ProcessContext::new(48_000, 0))
                    .is_err()
            );
        }
        assert!(
            plugin
                .drain(&mut canary[..4], &ProcessContext::new(44_100, 0))
                .is_err()
        );
        assert_eq!(canary, [123.0; 6]);
        let mut tail = Vec::new();
        let first = plugin
            .drain(&mut canary[..2], &ProcessContext::new(48_000, 0))
            .unwrap();
        tail.extend_from_slice(&canary[..first.frames * 2]);
        canary.fill(123.0);
        assert!(
            plugin
                .process(&input, &mut canary, &ProcessContext::new(48_000, 1))
                .is_err()
        );
        assert_eq!(canary, [123.0; 6]);
        for (name, value) in [
            ("oversampling", ParameterValue::Int(factor)),
            ("auto_gain_enabled", ParameterValue::Bool(false)),
        ] {
            plugin
                .set_parameter(ParameterId::from(name), value)
                .unwrap();
        }
        assert!(
            plugin
                .set_parameter(
                    ParameterId::from("auto_gain_enabled"),
                    ParameterValue::Bool(true)
                )
                .is_err()
        );
        let eq = plugin
            .as_any_mut()
            .unwrap()
            .downcast_mut::<EqPlugin>()
            .unwrap();
        let mut values = ParameterSet::new();
        values.insert(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        );
        values.insert(ParameterId::from("max_filters"), ParameterValue::Int(1));
        assert!(eq.apply_values(values).is_err());
        assert!(eq.set_filters(Vec::new()).is_err());
        assert!(
            eq.set_channel_filters(vec![Vec::new(), Vec::new()])
                .is_err()
        );
        tail.extend(drain(plugin.as_mut(), 17));
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(48_000, 0))
                .unwrap(),
            PluginDrainResult::COMPLETE
        );
        for initialize in [false, true] {
            if initialize {
                plugin.initialize(48_000).unwrap();
            } else {
                plugin.reset();
            }
            assert_eq!(process(plugin.as_mut(), &input), prefix);
            assert_eq!(drain(plugin.as_mut(), 4097), tail);
        }
    }
}

#[test]
fn one_x_empty_bank_freezes_nonempty_eos_in_ordinary_and_compiled_paths() {
    let mut plugin = make(1, 2);
    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    process(plugin.as_mut(), &[0.25, -0.5]);
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    let mut output = [123.0; 2];
    assert!(
        plugin
            .process(&[0.25, -0.5], &mut output, &ProcessContext::new(48_000, 1))
            .is_err()
    );
    assert!(
        plugin
            .process_compiled_f32(
                PluginCompiledOp::EqBiquadBank,
                &[0.25, -0.5],
                &mut output,
                &ProcessContext::new(48_000, 1)
            )
            .unwrap()
            .is_err()
    );
    assert_eq!(output, [123.0; 2]);
    plugin.reset();
    assert_eq!(process(plugin.as_mut(), &[0.25, -0.5]), [0.25, -0.5]);
}

fn recursive_filter(topology: EqFilterTopology) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: "Peak".into(),
        freq: 1000.0,
        q: 0.8,
        db_gain: 0.0,
        order: 2,
        topology,
        placement: None,
        lambda: None,
        kautz_sections: Vec::new(),
    }
}

#[test]
fn every_recursive_bank_retains_unknown_and_legacy_completion() {
    for topology in [
        EqFilterTopology::Biquad,
        EqFilterTopology::WarpedBiquad,
        EqFilterTopology::KautzFilter,
    ] {
        for per_channel in [false, true] {
            let mut params = EqPluginParams::default();
            if per_channel {
                params.channel_filters = Some(vec![Vec::new(), vec![recursive_filter(topology)]]);
            } else {
                params.filters.push(recursive_filter(topology));
            }
            let mut plugin = EqPlugin::from_params(2, 48_000, params)
                .unwrap()
                .into_boxed_plugin();
            for svf in [false, true] {
                if svf && topology != EqFilterTopology::Biquad {
                    continue;
                }
                if svf {
                    plugin
                        .set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
                        .unwrap();
                }
                assert_eq!(plugin.tail_length(), TailLength::Unknown);
                process(plugin.as_mut(), &[0.25, -0.5]);
                let mut canary = [123.0; 4];
                assert_eq!(
                    plugin
                        .drain(&mut canary, &ProcessContext::new(48_000, 0))
                        .unwrap(),
                    PluginDrainResult::COMPLETE
                );
                assert_eq!(canary, [123.0; 4]);
                process(plugin.as_mut(), &[0.0; 2]);
            }
        }
    }
}

#[test]
fn removing_recursive_banks_retains_only_finite_oversampler_history() {
    let filter = Biquad::new(BiquadFilterType::Peak, 1200.0, 48_000.0, 0.8, 12.0);
    let mut candidate = EqPlugin::new(2, vec![filter.clone()]).into_boxed_plugin();
    let mut reference = EqPlugin::new(2, vec![filter]).into_boxed_plugin();
    for plugin in [&mut candidate, &mut reference] {
        plugin
            .set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(4))
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from("auto_gain_enabled"),
                ParameterValue::Bool(false),
            )
            .unwrap();
        plugin.initialize(48_000).unwrap();
        process(plugin.as_mut(), &[0.25; 514]);
        plugin
            .as_any_mut()
            .unwrap()
            .downcast_mut::<EqPlugin>()
            .unwrap()
            .set_filters(Vec::new())
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Finite(1024));
    }
    let mut expected = Vec::new();
    for _ in 0..4 {
        expected.extend(process(reference.as_mut(), &[0.0; 512]));
    }
    assert_eq!(drain(candidate.as_mut(), 17), expected);
}

#[test]
fn cold_maximum_processing_drain_and_reset_have_no_heap_operations() {
    for factor in [2, 4] {
        for channels in [1, 2] {
            for cold_drain in [false, true] {
                let mut plugin = make(factor, channels);
                plugin
                    .set_parameter(
                        ParameterId::from("auto_gain_enabled"),
                        ParameterValue::Bool(true),
                    )
                    .unwrap();
                let input = vec![0.25; 4096 * channels];
                let mut processed = vec![0.0; input.len()];
                let mut output = vec![123.0; 17 * channels];
                // Advance only on the control thread when testing a cold drain.
                // The first refill will exercise measurement/cache publication.
                if cold_drain {
                    for _ in 0..9 {
                        process(plugin.as_mut(), &input);
                    }
                }
                let counts = std::thread::spawn(move || {
                    ACTIVE.set(true);
                    if !cold_drain {
                        plugin
                            .process(&input, &mut processed, &ProcessContext::new(48_000, 4096))
                            .unwrap();
                    }
                    loop {
                        if plugin
                            .drain(&mut output, &ProcessContext::new(48_000, 0))
                            .unwrap()
                            .complete
                        {
                            break;
                        }
                    }
                    plugin.reset();
                    ACTIVE.set(false);
                    COUNTS.get()
                })
                .join()
                .unwrap();
                assert_eq!(
                    counts,
                    (0, 0),
                    "factor{factor}/channels{channels}/cold_drain{cold_drain}"
                );
            }
        }
    }
}
