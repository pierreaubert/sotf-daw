//! Public sample-clock, bypass, lifecycle and cold memory-operation oracles.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_speech_denoiser::{
    SPEECH_DENOISER_FRAME_SIZE, SpeechDenoiserData, SpeechDenoiserPlugin,
    SpeechDenoiserPluginParams,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const RATE: u32 = 48000;
const LATENCY: usize = 960;

fn plugin(channels: usize, enabled: bool) -> SpeechDenoiserPlugin {
    let mut p = SpeechDenoiserPlugin::from_params(
        channels,
        SpeechDenoiserPluginParams {
            enabled,
            ..SpeechDenoiserPluginParams::default()
        },
    );
    p.initialize(f64::from(RATE)).unwrap();
    p
}

fn process(
    p: &mut SpeechDenoiserPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let mut result = Vec::with_capacity(input.len());
    let mut position = 0;
    let mut call = 0;
    while position < input.len() / channels {
        let frames = blocks[call % blocks.len()].min(input.len() / channels - position);
        let mut block = input[position * channels..(position + frames) * channels].to_vec();
        block.extend_from_slice(&[1234., -5678.]);
        assert_eq!(
            p.process_in_place(&mut block, &ProcessContext::new(RATE, frames))
                .unwrap(),
            frames
        );
        assert_eq!(&block[frames * channels..], &[1234., -5678.]);
        result.extend_from_slice(&block[..frames * channels]);
        position += frames;
        call += 1;
    }
    result
}

fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1., 1.)
    } else {
        0.
    }
}

fn assert_delayed(output: &[f32], input: &[f32], channels: usize) {
    for (i, &actual) in output.iter().enumerate() {
        let expected = i
            .checked_sub(LATENCY * channels)
            .map_or(0., |j| sanitize(input[j]));
        assert_eq!(actual, expected, "channels={channels} sample={i}");
    }
}

#[test]
fn bypass_dense_waveform_has_exact_total_latency_through_ring_wraps() {
    assert_eq!(SPEECH_DENOISER_FRAME_SIZE, 480);
    for channels in [1, 2] {
        let frames = 20 * SPEECH_DENOISER_FRAME_SIZE + 73;
        let mut input: Vec<_> = (0..frames * channels)
            .map(|i| (((i * 37 + 11) % 257) as f32 - 128.) / 512.)
            .collect();
        input[13] = f32::NAN;
        input[479 * channels] = f32::INFINITY;
        input[480 * channels] = -17.;
        input[481 * channels] = 17.;
        input[frames * channels - 1] = -0.75;
        input.resize((frames + LATENCY) * channels, 0.);
        let mut p = plugin(channels, false);
        for blocks in [
            &[1][..],
            &[7],
            &[137],
            &[479],
            &[480],
            &[481],
            &[8193],
            &[1, 137, 4096],
        ] {
            p.reset();
            let actual = process(&mut p, &input, channels, blocks);
            assert_delayed(&actual, &input, channels);
            assert_eq!(p.latency_samples(), LATENCY);
            assert_eq!(p.tail_length(), TailLength::Finite(LATENCY as u64));
        }
    }
}

#[test]
fn every_first_model_frame_phase_preserves_dry_first_and_final_markers() {
    for channels in [1, 2] {
        let mut p = plugin(channels, false);
        for phase in 0..SPEECH_DENOISER_FRAME_SIZE {
            p.reset();
            let mut input = vec![0.; (phase + 1 + LATENCY) * channels];
            for ch in 0..channels {
                input[ch] = 0.25 * (ch + 1) as f32;
                input[phase * channels + ch] -= 0.5 * (ch + 1) as f32;
            }
            let actual = process(
                &mut p,
                &input,
                channels,
                if phase % 2 == 0 { &[1] } else { &[137, 8193] },
            );
            assert_delayed(&actual, &input, channels);
        }
    }
}

#[test]
fn enabled_impulse_main_peak_agrees_with_declared_delay() {
    for channels in [1, 2] {
        let mut p = plugin(channels, true);
        for position in [0, 1, 72, 479, 480] {
            p.reset();
            let mut input = vec![0.; 3840 * channels];
            for ch in 0..channels {
                input[position * channels + ch] = 0.75;
            }
            let actual = process(&mut p, &input, channels, &[1, 137, 479]);
            for ch in 0..channels {
                let peak = actual
                    .iter()
                    .skip(ch)
                    .step_by(channels)
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                    .unwrap()
                    .0;
                assert_eq!(
                    peak,
                    position + LATENCY,
                    "channels={channels} position={position} ch={ch}"
                );
            }
            assert_eq!(p.latency_samples(), LATENCY);
        }
    }
}

#[test]
fn empty_calls_do_not_choose_the_first_audio_bypass_state() {
    let enabled = ParameterId::from("enabled");
    for channels in [1, 2] {
        for initial in [false, true] {
            let mut p = plugin(channels, !initial);
            let mut fresh = plugin(channels, initial);
            assert_eq!(
                p.process_in_place(&mut [], &ProcessContext::new(RATE, 0))
                    .unwrap(),
                0
            );
            p.parametric_set_parameter(enabled.clone(), ParameterValue::Bool(initial))
                .unwrap();
            let first = vec![0.25; 137 * channels];
            assert_eq!(
                process(&mut p, &first, channels, &[1]),
                process(&mut fresh, &first, channels, &[1])
            );
            for candidate in [&mut p, &mut fresh] {
                candidate
                    .parametric_set_parameter(enabled.clone(), ParameterValue::Bool(!initial))
                    .unwrap();
            }
            let remaining: Vec<_> = (0..1920 * channels)
                .map(|i| (i as f32 * 0.113).sin() * 0.3)
                .collect();
            assert_eq!(
                process(&mut p, &remaining, channels, &[137]),
                process(&mut fresh, &remaining, channels, &[137])
            );
        }
    }
}

#[test]
fn errors_reset_and_reinitialize_preserve_current_settings_and_history_contract() {
    let enabled = ParameterId::from("enabled");
    let mut uninitialized = SpeechDenoiserPlugin::new(1);
    let mut sentinel = [1234.];
    assert!(
        uninitialized
            .process_in_place(&mut sentinel, &ProcessContext::new(RATE, 1))
            .is_err()
    );
    assert_eq!(sentinel, [1234.]);
    assert_eq!(uninitialized.latency_samples(), LATENCY);
    for channels in [1, 2] {
        for state in [false, true] {
            let mut p = plugin(channels, state);
            let mut twin = plugin(channels, state);
            let input: Vec<_> = (0..2400 * channels)
                .map(|i| (i as f32 * 0.071).sin() * 0.3)
                .collect();
            assert_eq!(
                process(&mut p, &input, channels, &[137]),
                process(&mut twin, &input, channels, &[137])
            );
            assert!(p.initialize(44100.0).is_err());
            let mut sentinel = vec![1234.; channels];
            assert!(
                p.process_in_place(&mut sentinel, &ProcessContext::new(96000, 1))
                    .is_err()
            );
            assert!(
                p.process_in_place(&mut sentinel, &ProcessContext::new(RATE, 2))
                    .is_err()
            );
            assert_eq!(sentinel, vec![1234.; channels]);
            assert_eq!(
                process(&mut p, &input, channels, &[479, 481]),
                process(&mut twin, &input, channels, &[479, 481])
            );
            for reinitialize in [false, true] {
                p.parametric_set_parameter(enabled.clone(), ParameterValue::Bool(!state))
                    .unwrap();
                process(&mut p, &input[..137 * channels], channels, &[137]);
                if reinitialize {
                    p.initialize(f64::from(RATE)).unwrap();
                } else {
                    p.reset();
                }
                let mut fresh = plugin(channels, !state);
                assert_eq!(
                    p.parametric_get_parameter(&enabled),
                    Some(ParameterValue::Bool(!state))
                );
                assert_eq!(
                    process(&mut p, &input, channels, &[1, 137, 8193]),
                    process(&mut fresh, &input, channels, &[1, 137, 8193])
                );
                assert_eq!(p.latency_samples(), LATENCY);
                p.parametric_set_parameter(enabled.clone(), ParameterValue::Bool(state))
                    .unwrap();
            }
        }
    }
}

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;
// SAFETY: The caller's allocation contracts are passed unchanged to System.
// Constant thread-local counters do not allocate or retain resources.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACK.try_with(|v| {
            if v.get() {
                ALLOCS.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: Forward the original layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACK.try_with(|v| {
            if v.get() {
                FREES.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: Forward the original pointer/layout pair.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn counted(action: impl FnOnce()) -> (usize, usize) {
    ALLOCS.set(0);
    FREES.set(0);
    TRACK.set(true);
    action();
    TRACK.set(false);
    (ALLOCS.get(), FREES.get())
}

#[test]
fn cold_callbacks_toggles_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2] {
        for frames in [1, 137, 480, 8193] {
            let mut p = plugin(channels, true);
            std::thread::spawn(move || {
                let mut buffer = vec![0.125; frames * channels];
                let context = ProcessContext::new(RATE, frames);
                let id = ParameterId::from("enabled");
                let mut values = sotf_host::ParameterSet::new();
                values.insert(id.clone(), ParameterValue::Bool(true));
                let counts = counted(|| {
                    p.process_in_place(&mut [], &ProcessContext::new(RATE, 0))
                        .unwrap();
                    for enabled in [true, false, true] {
                        *values.get_mut(&id).unwrap() = ParameterValue::Bool(enabled);
                        p.apply_values_realtime(&values).unwrap();
                        for _ in 0..(4800_usize.div_ceil(frames)) {
                            buffer.fill(0.125);
                            p.process_in_place(&mut buffer, &context).unwrap();
                            assert_eq!(p.latency_samples(), LATENCY);
                            assert_eq!(
                                p.tail_length(),
                                if enabled {
                                    TailLength::Unknown
                                } else {
                                    TailLength::Finite(LATENCY as u64)
                                }
                            );
                            let _ = p.get_data();
                        }
                    }
                    p.reset();
                    buffer.fill(0.125);
                    p.process_in_place(&mut buffer, &context).unwrap();
                });
                assert_eq!(counts, (0, 0), "channels={channels} frames={frames}");
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn first_disabled_drain_queries_snapshots_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2] {
        for frames in [1, 479, 481, 8193] {
            for initially_enabled in [false, true] {
                let mut p = plugin(channels, initially_enabled);
                let mut input = vec![0.125; frames * channels];
                p.process_in_place(&mut input, &ProcessContext::new(RATE, frames))
                    .unwrap();
                p.parametric_set_parameter(
                    ParameterId::from("enabled"),
                    ParameterValue::Bool(false),
                )
                .unwrap();
                let snapshot = p.current_values();
                let id = ParameterId::from("enabled");
                std::thread::spawn(move || {
                    let mut output = [0.; 960];
                    let context = ProcessContext::new(RATE, 0);
                    let counts = counted(|| {
                        for epoch in 0..2 {
                            if epoch > 0 {
                                input.fill(0.125);
                                p.process_in_place(&mut input, &ProcessContext::new(RATE, frames))
                                    .unwrap();
                            }
                            assert_eq!(p.drain_output_frames_max(), 480);
                            assert_eq!(p.drain_call_bound().unwrap().get(), 2);
                            let mut returned = 0;
                            let mut calls = 0;
                            loop {
                                p.apply_values_realtime(&snapshot).unwrap();
                                p.parametric_set_parameter(id.clone(), ParameterValue::Bool(false))
                                    .unwrap();
                                assert_eq!(p.tail_length(), TailLength::Finite(LATENCY as u64));
                                assert!(p.drain_call_bound().is_some());
                                let capacity = [1, 17, 480][calls % 3];
                                let result = p
                                    .drain(&mut output[..capacity * channels], &context)
                                    .unwrap();
                                returned += result.frames;
                                calls += 1;
                                if result.complete {
                                    break;
                                }
                                assert!(calls < 960);
                            }
                            assert_eq!(returned, LATENCY);
                            assert_eq!(p.drain_call_bound().unwrap().get(), 1);
                            assert!(p.drain(&mut [], &context).unwrap().complete);
                            p.reset();
                            assert_eq!(p.drain_call_bound().unwrap().get(), 1);
                        }
                    });
                    assert_eq!(
                        counts,
                        (0, 0),
                        "channels={channels} frames={frames} enabled={initially_enabled}"
                    );
                })
                .join()
                .unwrap();
            }
        }
    }
}

#[test]
fn enabled_eof_process_drain_backend_cutoff_and_zero_calls_allocate_and_free_nothing() {
    for channels in [1, 2] {
        let mut p = plugin(channels, true);
        let mut input = vec![0.125; 3 * SPEECH_DENOISER_FRAME_SIZE * channels];
        let mut output = vec![0.0; SPEECH_DENOISER_FRAME_SIZE * channels];
        let process_context = ProcessContext::new(RATE, 3 * SPEECH_DENOISER_FRAME_SIZE);
        let drain_context = ProcessContext::new(RATE, 0);
        let mut zero_frame_canary = [0.375; 2];

        let counts = counted(|| {
            assert_eq!(
                p.process_in_place(&mut input, &process_context).unwrap(),
                process_context.num_frames
            );
            assert_eq!(p.drain_call_bound().unwrap().get(), 2);
            for call in 0..2 {
                let result = p.drain(&mut output, &drain_context).unwrap();
                assert_eq!(result.frames, SPEECH_DENOISER_FRAME_SIZE);
                assert_eq!(result.complete, call == 1);
            }
            assert_eq!(
                p.process_in_place(
                    &mut zero_frame_canary[..channels],
                    &ProcessContext::new(RATE, 0),
                )
                .unwrap(),
                0
            );
            assert!(p.drain(&mut [], &drain_context).unwrap().complete);
        });

        assert_eq!(counts, (0, 0), "channels={channels}");
        assert_eq!(
            &zero_frame_canary[..channels],
            &vec![0.375; channels],
            "zero-frame process changed its caller buffer"
        );
        let snapshot = p
            .get_data()
            .unwrap()
            .downcast::<SpeechDenoiserData>()
            .unwrap();
        assert!(snapshot.model_frames > 0, "final telemetry was cleared");
        assert_eq!(p.tail_length(), TailLength::Unknown);
    }
}
