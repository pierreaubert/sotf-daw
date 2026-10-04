//! Finite-stream checks against direct convolution and zero-input continuation.

use crate::{
    BinauralDecoderPlugin, RoomModel,
    binaural_decoder_plugin::{ReflectionChannelTap, ReflectionDelayGroup},
    filter,
    types::BinauralState,
};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use std::sync::Arc;

const FFT_SIZE: usize = 64;
const SAMPLE_RATE: u32 = 48_000;

#[test]
fn declared_drain_work_covers_finite_and_long_capped_reverb() {
    for reverb in [false, true] {
        for source in [1, 16, 17, 65] {
            for partial in [false, true] {
                let mut plugin = finite_decoder(&[0.7, -0.1], &[0.4, 0.1]);
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                if reverb {
                    plugin
                        .set_parameter("late_reverb_enabled".into(), ParameterValue::Bool(true))
                        .unwrap();
                    plugin
                        .set_parameter("late_reverb_rt60".into(), ParameterValue::Float(1.0))
                        .unwrap();
                }
                process_partitioned(&mut plugin, &vec![0.125; source], &[7]);
                if partial {
                    plugin
                        .drain(&mut [0.0; 2], &ProcessContext::new(SAMPLE_RATE, 0))
                        .unwrap();
                }
                let bound = plugin.drain_call_bound().unwrap().get();
                if reverb {
                    assert!(bound > 4096);
                }
                let mut calls = 0;
                let mut output = vec![0.0; plugin.drain_output_frames_max() * 2];
                loop {
                    calls += 1;
                    assert!(calls <= bound);
                    let result = plugin
                        .drain(&mut output, &ProcessContext::new(SAMPLE_RATE, 0))
                        .unwrap();
                    if result.complete {
                        break;
                    }
                }
                assert_eq!(calls, bound);
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                plugin.reset();
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
            }
        }
    }
}

fn finite_decoder(left_ir: &[f32], right_ir: &[f32]) -> BinauralDecoderPlugin {
    let mut plugin = BinauralDecoderPlugin::new(
        1,
        FFT_SIZE,
        None,
        0.0,
        0.0,
        false,
        120.0,
        2.0,
        0.0,
        RoomModel {
            max_order: 0,
            ..Default::default()
        },
    );
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut response = filter::ir_to_freq(left_ir, FFT_SIZE, &plugin.fft.fft_r2c);
    response.extend(filter::ir_to_freq(right_ir, FFT_SIZE, &plugin.fft.fft_r2c));
    let state = Arc::new(BinauralState {
        hrtf_filters_freq: vec![response],
        diffuse_field_eq_filter: None,
        _hrtf_data: None,
    });
    plugin.state.store(Arc::clone(&state));
    plugin.crossfade.current_state_snapshot = state;
    plugin
}

fn process_partitioned(
    plugin: &mut BinauralDecoderPlugin,
    input: &[f32],
    partitions: &[usize],
) -> Vec<f32> {
    let mut rendered = Vec::new();
    let mut position = 0;
    for &requested in partitions.iter().cycle() {
        if position == input.len() {
            break;
        }
        let frames = requested.min(input.len() - position);
        let mut output = vec![f32::NAN; frames * 2];
        let written = plugin
            .process(
                &input[position..position + frames],
                &mut output,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .unwrap();
        assert_eq!(written, frames);
        rendered.extend(output);
        position += frames;
    }
    rendered
}

fn finish(plugin: &mut BinauralDecoderPlugin, rendered: &mut Vec<f32>) {
    finish_with_capacity(plugin, rendered, plugin.drain_output_frames_max().max(1));
}

fn finish_with_capacity(
    plugin: &mut BinauralDecoderPlugin,
    rendered: &mut Vec<f32>,
    capacity: usize,
) {
    let mut output = vec![f32::NAN; capacity * 2];
    for _ in 0..100_000 {
        output.fill(f32::NAN);
        let result = plugin
            .drain(&mut output, &ProcessContext::new(SAMPLE_RATE, capacity))
            .unwrap();
        assert!(result.frames <= capacity);
        assert!(result.frames <= plugin.drain_output_frames_max());
        assert!(output[result.frames * 2..].iter().all(|x| x.is_nan()));
        rendered.extend_from_slice(&output[..result.frames * 2]);
        if result.complete {
            let again = plugin
                .drain(&mut output, &ProcessContext::new(SAMPLE_RATE, capacity))
                .unwrap();
            assert_eq!(again, sotf_host::plugin::PluginDrainResult::COMPLETE);
            return;
        }
        assert!(result.frames > 0, "drain must make progress");
    }
    panic!("drain did not terminate");
}

#[test]
fn source_reflection_delay_extends_the_finite_tail() {
    let mut plugin = finite_decoder(&[0.5], &[0.25]);
    plugin.smoothing.externalization = sotf_host::smoothing::Smoother::new(0.5, 50.0, SAMPLE_RATE);
    plugin.room.reflection_groups = vec![ReflectionDelayGroup {
        delay_samples: 173,
        taps: vec![ReflectionChannelTap {
            channel: 0,
            left_gain: 0.6,
            right_gain: -0.3,
        }],
    }];
    let mut input = vec![0.0; 67];
    input[66] = 1.0;
    let mut actual = process_partitioned(&mut plugin, &input, &[7, 13]);
    finish_with_capacity(&mut plugin, &mut actual, 1);
    assert_eq!(actual.len() / 2, input.len() + FFT_SIZE + 173);
    for frame in 0..actual.len() / 2 {
        let expected = if frame == 66 + FFT_SIZE {
            [0.5, 0.25]
        } else if frame == 66 + FFT_SIZE + 173 {
            [0.3, -0.15]
        } else {
            [0.0, 0.0]
        };
        for channel in 0..2 {
            assert!((actual[frame * 2 + channel] - expected[channel]).abs() < 2.0e-6);
        }
    }
}

#[test]
fn recursive_reverb_has_an_explicit_cap_and_matches_zero_continuation() {
    for rt60 in [0.125_f32, 0.25] {
        let make = || {
            let mut plugin = finite_decoder(&[0.7], &[0.4]);
            for (key, value) in [
                ("late_reverb_enabled", ParameterValue::Bool(true)),
                ("late_reverb_mix", ParameterValue::Float(0.75)),
                ("late_reverb_rt60", ParameterValue::Float(rt60)),
            ] {
                plugin.set_parameter(ParameterId::from(key), value).unwrap();
            }
            plugin
        };
        let frames = 17;
        let mut input = vec![0.0; frames];
        input[frames - 1] = 1.0;
        // Exact binary RT60 choices make this expectation independent of
        // production's ceil/float rounding. The recursive cap adds 3 RT60s.
        let finite_end = FFT_SIZE * 2 + FFT_SIZE / 4;
        let total_frames = finite_end + (3.0 * rt60 * SAMPLE_RATE as f32) as usize;
        let mut continued = input.clone();
        continued.resize(total_frames, 0.0);
        let reference = process_partitioned(&mut make(), &continued, &[16]);
        assert!(
            reference[(finite_end + 1000) * 2..]
                .iter()
                .any(|x| x.abs() > 1.0e-5)
        );

        for capacity in [1, 16, 513] {
            let mut plugin = make();
            let mut actual = process_partitioned(&mut plugin, &input, &[7, 3]);
            finish_with_capacity(&mut plugin, &mut actual, capacity);
            assert_eq!(actual.len(), total_frames * 2);
            assert!(
                actual
                    .iter()
                    .zip(&reference)
                    .all(|(a, b)| (a - b).abs() < 2.0e-6)
            );
            // Completion explicitly discards the unbounded recursive residue.
            assert_eq!(plugin.room.fdn.process_stereo(0.0, 0.0), (0.0, 0.0));
        }
    }
}

#[test]
fn invalid_drain_capacity_is_transactional_before_and_during_drain() {
    let mut actual = finite_decoder(&[0.7, -0.3], &[0.4, 0.2]);
    let mut reference = finite_decoder(&[0.7, -0.3], &[0.4, 0.2]);
    process_partitioned(&mut actual, &[1.0, -0.2, 0.3], &[2]);
    process_partitioned(&mut reference, &[1.0, -0.2, 0.3], &[2]);
    for _ in 0..3 {
        for length in [0, 1, 3, 33] {
            let mut invalid = vec![17.0; length];
            assert!(
                actual
                    .drain(&mut invalid, &ProcessContext::new(SAMPLE_RATE, 16))
                    .is_err()
            );
            assert!(invalid.iter().all(|&x| x == 17.0));
        }
        let mut a = [0.0; 14];
        let mut b = [0.0; 14];
        let context = ProcessContext::new(SAMPLE_RATE, 7);
        assert_eq!(
            actual.drain(&mut a, &context).unwrap(),
            reference.drain(&mut b, &context).unwrap()
        );
        assert_eq!(a, b);
    }
    let mut a = Vec::new();
    let mut b = Vec::new();
    finish(&mut actual, &mut a);
    finish(&mut reference, &mut b);
    assert!(a == b);
}

#[test]
fn drain_lifecycle_empty_silent_completed_and_reset() {
    let mut plugin = finite_decoder(&[0.7, -0.3], &[0.4, 0.2]);
    let context = ProcessContext::new(SAMPLE_RATE, 1);
    assert_eq!(
        plugin.drain(&mut [], &context).unwrap(),
        sotf_host::plugin::PluginDrainResult::COMPLETE
    );
    process_partitioned(&mut plugin, &[0.0], &[1]);
    let first = plugin.drain(&mut [0.0; 2], &context).unwrap();
    assert_eq!(first.frames, 1);
    assert!(!first.complete);
    for completed in [false, true] {
        if completed {
            let mut silence = Vec::new();
            finish(&mut plugin, &mut silence);
            assert!(silence.iter().all(|&x| x == 0.0));
        }
        let mut output = [42.0; 2];
        assert!(plugin.process(&[1.0], &mut output, &context).is_err());
        assert_eq!(output, [42.0; 2]);
        let id = ParameterId::from("late_reverb_rt60");
        let before = plugin.get_parameter(&id);
        assert!(
            plugin
                .set_parameter(id.clone(), ParameterValue::Float(5.0))
                .is_err()
        );
        assert_eq!(plugin.get_parameter(&id), before);
    }
    plugin.reset();
    let mut actual = process_partitioned(&mut plugin, &[1.0, 0.2], &[1]);
    finish(&mut plugin, &mut actual);
    let mut fresh = finite_decoder(&[0.7, -0.3], &[0.4, 0.2]);
    let mut expected = process_partitioned(&mut fresh, &[1.0, 0.2], &[1]);
    finish(&mut fresh, &mut expected);
    assert!(actual == expected);
}

#[test]
fn finite_hrir_drain_matches_direct_convolution_and_zero_padded_reference() {
    let mut left_ir = vec![0.0; 49];
    left_ir[0] = 0.7;
    left_ir[13] = -0.2;
    left_ir[48] = 0.125;
    let mut right_ir = vec![0.0; 49];
    right_ir[3] = 0.4;
    right_ir[31] = 0.175;
    right_ir[48] = -0.25;

    for frames in [1, 15, 16, 17, 63, 64, 65, 193] {
        let mut input = vec![0.0; frames];
        input[0] = 0.25;
        input[frames - 1] = 1.0;
        for partitions in [&[1][..], &[15, 17, 3, 97][..], &[64][..]] {
            let mut plugin = finite_decoder(&left_ir, &right_ir);
            let mut actual = process_partitioned(&mut plugin, &input, partitions);
            finish(&mut plugin, &mut actual);

            // Each hop contributes one N-frame IFFT block. The last block starts
            // at floor((T-1)/H)*H; the public timeline adds N startup frames.
            let hop = FFT_SIZE / 4;
            let total_frames = FFT_SIZE + ((frames - 1) / hop) * hop + FFT_SIZE;
            assert_eq!(actual.len(), total_frames * 2, "input frames={frames}");

            let mut continued = input.clone();
            continued.resize(total_frames, 0.0);
            let reference = process_partitioned(
                &mut finite_decoder(&left_ir, &right_ir),
                &continued,
                &[7, 23, 1],
            );
            for frame in 0..total_frames {
                for (channel, ir) in [&left_ir, &right_ir].into_iter().enumerate() {
                    let expected: f64 = (0..ir.len())
                        .filter_map(|tap| {
                            frame
                                .checked_sub(FFT_SIZE + tap)
                                .filter(|&source| source < frames)
                                .map(|source| f64::from(input[source]) * f64::from(ir[tap]))
                        })
                        .sum();
                    let index = frame * 2 + channel;
                    assert!(
                        (f64::from(actual[index]) - expected).abs() < 2.0e-6,
                        "frames={frames} frame={frame} channel={channel}: {} != {expected}",
                        actual[index]
                    );
                    assert!((actual[index] - reference[index]).abs() < 2.0e-6);
                }
            }
        }
    }
}
