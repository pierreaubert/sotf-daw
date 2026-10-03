//! Live limiter-chain mutations with every-sample PCM proof.
//!
//! Complements the running-engine smoke suite: these tests drive the real
//! processing commands (`CommitHostUpdate`, `SetParameter`, `Bypass`) and
//! the real `process_frame` path with real analog-limiter DSP, asserting
//! every emitted sample. No wall clock, no meter snapshots, no device.

use super::super::build::build_plugin_host;
use super::{ProcessingState, handle_processing_command, request};
use crate::engine::{PluginConfig, PreparedHostUpdate, ProcessingCommand, ProcessingResponse};
use sotf_plugins::PluginHost;

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// f32 rounding slack for ceiling assertions: guard and mixer rounding.
const BOUND_SLACK: f32 = 1e-6;
/// 50 ms fade at the 48 kHz test rate.
const FADE_FRAMES: usize = 2_400;

/// Precise ceiling formula, pinning the production conversion.
fn ceiling_db(threshold_db: f32) -> f32 {
    10f32.powf(threshold_db / 20.0)
}

fn limiter_config(model: &str, threshold_db: f64, lookahead_ms: f64) -> PluginConfig {
    PluginConfig::new(
        "analog_limiter",
        serde_json::json!({
            "threshold": threshold_db,
            "release": 50.0,
            "lookahead": lookahead_ms,
            "soft": false,
            "true_peak": false,
            "mix": 1.0,
            "analog_model": model,
            "analog_drive": 6.0,
            "analog_color": 0.5,
            "analog_character": 0.25,
            "analog_trim": 0.0,
        }),
    )
}

fn limiter_host(model: &str, threshold_db: f64, lookahead_ms: f64) -> PluginHost {
    let config = limiter_config(model, threshold_db, lookahead_ms);
    let (mut host, _warnings) =
        build_plugin_host(std::slice::from_ref(&config), RATE, CHANNELS).unwrap();
    host.build().unwrap();
    host
}

/// Hot stereo program: 0.9 peak at 440 Hz, interleaved.
fn hot_sine(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * CHANNELS];
    for frame in 0..frames {
        let sample = 0.9 * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / RATE as f32).sin();
        for channel in 0..CHANNELS {
            input[frame * CHANNELS + channel] = sample;
        }
    }
    input
}

/// Render input frames through the real path in mixed block sizes,
/// returning every emitted sample.
fn render(state: &mut ProcessingState, input: &[f32]) -> Vec<f32> {
    let mut rendered = Vec::new();
    let partition = [512usize, 0, 128, 300, 1, 513, 64];
    let total = input.len() / CHANNELS;
    let mut pos = 0;
    let mut block = 0;
    while pos < total {
        let count = partition[block % partition.len()].min(total - pos);
        let mut output = vec![f32::NAN; state.output_frames_for_input(count) * CHANNELS];
        let actual = state
            .process_frame(
                &input[pos * CHANNELS..(pos + count) * CHANNELS],
                &mut output,
                count,
            )
            .unwrap();
        rendered.extend_from_slice(&output[..actual * CHANNELS]);
        pos += count;
        block += 1;
        assert!(block < 100_000);
    }
    rendered
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max)
}

fn send(command: ProcessingCommand, state: &mut ProcessingState) -> ProcessingResponse {
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(4);
    assert!(
        !handle_processing_command(request(command), state, &response_tx, &event_tx).is_shutdown()
    );
    response_rx.recv().expect("handler must reply").response
}

fn new_state() -> ProcessingState {
    ProcessingState::new(
        CHANNELS,
        RATE,
        #[cfg(feature = "streaming")]
        None,
    )
}

#[test]
fn limiter_rebuild_transition_holds_ceiling_every_sample() {
    let mut state = new_state();
    *state.host = limiter_host("Harmonics", -12.0, 5.0);
    let ceiling = ceiling_db(-12.0);

    let pre = render(&mut state, &hot_sine(2_400));
    assert!(!pre.is_empty());
    for (i, &sample) in pre.iter().enumerate() {
        assert!(
            sample.abs() <= ceiling + BOUND_SLACK,
            "pre frame {i}: {sample} exceeds {ceiling}"
        );
    }

    // Real rebuild command to the Tape chain on the live state.
    let prepared = PreparedHostUpdate::prepare(
        limiter_host("Tape", -12.0, 5.0),
        RATE,
        CHANNELS,
        state.host.total_latency_samples(),
    )
    .unwrap();
    let response = send(ProcessingCommand::CommitHostUpdate(prepared), &mut state);
    assert!(
        matches!(
            response,
            ProcessingResponse::PluginChainUpdated {
                previous_latency_samples: 240,
                latency_samples: 240,
                latency_changed: false,
                ..
            }
        ),
        "rebuild must commit with unchanged latency"
    );
    assert!(state.prev_host.is_some());

    // Transition plus post-transition program: every sample bounded, hot overall.
    let post = render(&mut state, &hot_sine(4_800));
    assert!(post.len() >= FADE_FRAMES * CHANNELS);
    for (i, &sample) in post.iter().enumerate() {
        assert!(
            sample.abs() <= ceiling + BOUND_SLACK,
            "post sample {i}: {sample} exceeds {ceiling}"
        );
    }
    assert!(
        peak(&post) > 0.2,
        "transition run never exercised the limiter: {}",
        peak(&post)
    );
    assert!(
        state.prev_host.is_none(),
        "transition did not retire after {} emitted frames",
        post.len() / CHANNELS
    );
}

#[test]
fn limiter_live_threshold_automation_tightens_bound_mid_stream() {
    let mut state = new_state();
    *state.host = limiter_host("Tape", -12.0, 5.0);

    let pre = render(&mut state, &hot_sine(2_400));
    let ceiling_12 = ceiling_db(-12.0);
    for (i, &sample) in pre.iter().enumerate() {
        assert!(
            sample.abs() <= ceiling_12 + BOUND_SLACK,
            "pre sample {i}: {sample} exceeds {ceiling_12}"
        );
    }
    assert!(peak(&pre) > 0.2, "pre run never exercised the limiter");

    // Real live parameter command between blocks.
    let response = send(
        ProcessingCommand::SetParameter {
            plugin_index: 0,
            param_id: "threshold".to_string(),
            value: "-18.0".to_string(),
        },
        &mut state,
    );
    assert!(
        !matches!(response, ProcessingResponse::Error(_)),
        "live threshold set was rejected"
    );

    // The final guard tracks the target immediately: post-command samples
    // obey the new ceiling from the first sample, with nonzero program.
    let post = render(&mut state, &hot_sine(2_400));
    let ceiling_18 = ceiling_db(-18.0);
    for (i, &sample) in post.iter().enumerate() {
        assert!(
            sample.abs() <= ceiling_18 + BOUND_SLACK,
            "post sample {i}: {sample} exceeds {ceiling_18}"
        );
    }
    assert!(
        peak(&post) > 0.05,
        "post run emitted no program: {}",
        peak(&post)
    );
}

#[test]
fn limiter_live_bypass_routes_unlimited_program_mid_stream() {
    let mut state = new_state();
    *state.host = limiter_host("Tape", -12.0, 5.0);
    let ceiling = ceiling_db(-12.0);

    let pre = render(&mut state, &hot_sine(1_200));
    for (i, &sample) in pre.iter().enumerate() {
        assert!(
            sample.abs() <= ceiling + BOUND_SLACK,
            "pre sample {i}: {sample} exceeds {ceiling}"
        );
    }

    send(ProcessingCommand::Bypass(true), &mut state);
    assert!(state.bypassed);
    let input_mid = hot_sine(1_200);
    let mid = render(&mut state, &input_mid);
    assert_eq!(mid.len(), input_mid.len(), "bypass must preserve geometry");
    assert_eq!(mid, input_mid, "bypass must pass input through bit-exactly");
    assert!(
        peak(&mid) > 0.5,
        "bypassed program is not the hot input: {}",
        peak(&mid)
    );

    send(ProcessingCommand::Bypass(false), &mut state);
    assert!(!state.bypassed);
    let post = render(&mut state, &hot_sine(1_200));
    for (i, &sample) in post.iter().enumerate() {
        assert!(
            sample.abs() <= ceiling + BOUND_SLACK,
            "post sample {i}: {sample} exceeds {ceiling}"
        );
    }
    assert!(peak(&post) > 0.2, "re-engaged run never limited");
}
