//! Branched graph end-of-stream delivery on production engine paths.
//!
//! Real external-key DeEsser graphs built by
//! [`build_plugin_graph_host`](super::super::build::build_plugin_graph_host)
//! drain their lookahead and linear-phase tails through the actual
//! engine EOS loop as well as direct host drain. This module is
//! self-contained: the small graph builders mirror the sidechain test
//! fixtures without editing them, and the worker harness mirrors the
//! EOS tests, so the other lanes' files stay untouched.

use super::super::build::build_plugin_graph_host;
use super::super::processing_state::run_processing_thread;
use super::super::{ProcessingReply, ProcessingRequest};
use super::request;
use crate::engine::{
    AudioFrame, DecoderMessage, GcItem, PluginGraphConfig, PluginGraphEdgeConfig,
    PluginGraphNodeConfig, PreparedHostUpdate, ProcessingCommand, ProcessingMessage,
    ProcessingResponse, ThreadEvent,
};
use arc_swap::ArcSwap;
use sotf_plugins::PluginHost;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;
const WAIT: Duration = Duration::from_secs(2);
// A cold final-frame impulse below the -20 dB threshold: no reduction
// engages, so the tail carries the pure delayed impulse.
const IMPULSE_PEAK: f32 = 0.01;

/// 4-in/2-out channel selector: output `i` carries host input `select[i]`.
fn eos_matrix_params(select: [usize; 2]) -> serde_json::Value {
    let mut matrix = vec![0.0f32; 8];
    matrix[select[0]] = 1.0;
    matrix[4 + select[1]] = 1.0;
    serde_json::json!({
        "input_channels": 4,
        "output_channels": 2,
        "matrix": matrix,
    })
}

fn eos_deesser_params(mode: &str, topology: &str, lookahead_ms: f64) -> serde_json::Value {
    serde_json::json!({
        "frequency": 7000.0,
        "q": 1.5,
        "threshold": -20.0,
        "ratio": 8.0,
        "attack_ms": 0.5,
        "release_ms": 20.0,
        "mode": mode,
        "mix": 1.0,
        "range_db": 60.0,
        "stereo_link": 0.0,
        "lookahead_ms": lookahead_ms,
        "split_topology": topology,
        "ms_mode": false,
        "sidechain_external": true,
    })
}

/// Program matrix -> external-key DeEsser via audio, key matrix ->
/// DeEsser via sidechain. Selections index the 4-channel host input.
fn eos_deesser_key_graph(mode: &str, topology: &str, lookahead_ms: f64) -> PluginGraphConfig {
    PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "matrix", eos_matrix_params([0, 1]), 4).unwrap(),
            PluginGraphNodeConfig::try_new(2, "matrix", eos_matrix_params([2, 3]), 4).unwrap(),
            PluginGraphNodeConfig::try_new(
                3,
                "de_esser",
                eos_deesser_params(mode, topology, lookahead_ms),
                2,
            )
            .unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 3),
            PluginGraphEdgeConfig::sidechain(2, 3),
        ],
    )
    .unwrap()
}

fn eos_build(graph: &PluginGraphConfig) -> PluginHost {
    let (host, warnings) = build_plugin_graph_host(graph, SAMPLE_RATE, 4).unwrap();
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    host
}

/// 4-channel host input: silence except a program impulse of `peak` on
/// the last frame of [0, 1]; the key bus [2, 3] stays silent.
fn eos_impulse_input(frames: usize, peak: f32) -> Vec<f32> {
    let mut input = vec![0.0f32; frames * 4];
    input[(frames - 1) * 4] = peak;
    input[(frames - 1) * 4 + 1] = peak;
    input
}

/// 4-channel host input: 8 kHz program at `program_peak` on [0, 1],
/// independent 8 kHz key at `key_peak` on [2, 3].
fn eos_tone_input(frames: usize, program_peak: f32, key_peak: f32) -> Vec<f32> {
    let mut input = vec![0.0f32; frames * 4];
    for i in 0..frames {
        let t = i as f32 / SAMPLE_RATE as f32;
        let program = program_peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        let key = key_peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        input[i * 4] = program;
        input[i * 4 + 1] = program;
        input[i * 4 + 2] = key;
        input[i * 4 + 3] = key;
    }
    input
}

fn eos_rms(samples: &[f32]) -> f32 {
    let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Drain `host` to completion, returning the concatenated tail.
fn eos_drain(host: &mut PluginHost) -> Vec<f32> {
    let mut tail = Vec::new();
    for _ in 0..1024 {
        let mut chunk = vec![f32::NAN; host.drain_output_frames_max() * host.output_channels()];
        let result = host.drain(&mut chunk).unwrap();
        assert!(result.frames <= host.drain_output_frames_max());
        tail.extend_from_slice(&chunk[..result.frames * host.output_channels()]);
        if result.complete {
            return tail;
        }
    }
    panic!("engine graph drain did not complete");
}

struct Worker {
    decoder_tx: Option<Sender<DecoderMessage>>,
    command_tx: Sender<ProcessingRequest>,
    response_rx: Receiver<ProcessingReply>,
    output_rx: Option<Receiver<ProcessingMessage>>,
    event_rx: crossbeam::channel::Receiver<ThreadEvent>,
    finished_rx: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
    _gc_rx: crossbeam::channel::Receiver<GcItem>,
}

impl Worker {
    fn new(host: PluginHost) -> Self {
        let (decoder_tx, decoder_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(0);
        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let (event_tx, event_rx) = crossbeam::channel::bounded(32);
        let (gc_tx, gc_rx) = crossbeam::channel::bounded(32);
        let (_recycle_tx, recycle_rx) = mpsc::channel();
        let (decoder_recycle_tx, _decoder_recycle_rx) = mpsc::sync_channel(4);
        let (finished_tx, finished_rx) = mpsc::channel();
        // `expected_*` describe the worker's CURRENT host, not the
        // candidate: the worker starts on a 4-channel passthrough host
        // with zero latency (`ProcessingState::new`), and
        // `commit_host_update` rejects the update when they differ.
        // Production passes the active `current.num_channels`
        // (`manager_thread/apply.rs`); the eos.rs harness passes (1, 0)
        // for its 1-channel worker.
        let update = PreparedHostUpdate::prepare(host, SAMPLE_RATE, 4, 0).unwrap();
        command_tx
            .send(request(ProcessingCommand::CommitHostUpdate(update)))
            .unwrap();
        let thread = std::thread::spawn(move || {
            let result = run_processing_thread(
                decoder_rx,
                output_tx,
                command_rx,
                response_tx,
                event_tx,
                SAMPLE_RATE,
                4,
                Arc::new(ArcSwap::from_pointee(Vec::new())),
                gc_tx,
                recycle_rx,
                decoder_recycle_tx,
                Arc::new(AtomicU64::new(0)),
                #[cfg(feature = "streaming")]
                None,
            );
            finished_tx.send(result).ok();
        });
        let worker = Self {
            decoder_tx: Some(decoder_tx),
            command_tx,
            response_rx,
            output_rx: Some(output_rx),
            event_rx,
            finished_rx,
            thread: Some(thread),
            _gc_rx: gc_rx,
        };
        let reply = worker.response_rx.recv_timeout(WAIT).unwrap();
        assert!(
            matches!(
                reply.response,
                ProcessingResponse::PluginChainUpdated { .. }
            ),
            "expected PluginChainUpdated after initial host commit, got {:?}",
            reply.response
        );
        worker
    }

    fn output(&self) -> &Receiver<ProcessingMessage> {
        self.output_rx.as_ref().unwrap()
    }

    fn send(&self, data: Vec<f32>, frames: usize) {
        self.decoder_tx
            .as_ref()
            .unwrap()
            .send(DecoderMessage::Frame(
                AudioFrame::try_new(data, frames, 4, SAMPLE_RATE).unwrap(),
            ))
            .unwrap();
    }

    fn eos(&self) {
        self.decoder_tx
            .as_ref()
            .unwrap()
            .send(DecoderMessage::EndOfStream)
            .unwrap();
    }

    /// Explicit worker termination. Decoder EOS ends the stream, never
    /// the thread: the engine stays alive for the next track and only
    /// `Shutdown` breaks the processing loop (`Stop` resets stream
    /// state and likewise stays alive). Neither command emits a
    /// response ack; termination is observed via `finished` plus the
    /// output disconnect.
    fn shutdown(&self) {
        self.command_tx
            .send(request(ProcessingCommand::Shutdown))
            .unwrap();
    }

    fn finished(&self) {
        self.finished_rx
            .recv_timeout(WAIT)
            .expect("processing did not terminate promptly")
            .unwrap();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.output_rx.take());
        drop(self.decoder_tx.take());
        self.command_tx
            .send(request(ProcessingCommand::Shutdown))
            .ok();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
fn worker_delivers_external_key_lookahead_tail_at_eos() {
    // Actual engine EOS: decoder frames plus EndOfStream through the
    // real processing loop with a 2 ms lookahead DeEsser. The cold
    // final-frame impulse must land at the exact tail index with its
    // exact value; nothing else may be nonzero.
    let host = eos_build(&eos_deesser_key_graph("Wideband", "Minimum-Phase", 2.0));
    assert_eq!(host.total_latency_samples(), 96);
    let worker = Worker::new(host);
    let frames = 32usize;
    worker.send(eos_impulse_input(frames, IMPULSE_PEAK), frames);
    worker.eos();

    let mut program = Vec::new();
    loop {
        match worker.output().recv_timeout(WAIT).unwrap() {
            ProcessingMessage::Frame(frame) => {
                assert_eq!(frame.sample_rate, SAMPLE_RATE);
                assert_eq!(frame.data.len() % 2, 0);
                program.extend_from_slice(&frame.data);
            }
            ProcessingMessage::EndOfStream => break,
            other => panic!("unexpected worker message: {other:?}"),
        }
    }
    while let Ok(event) = worker.event_rx.try_recv() {
        assert!(
            !matches!(
                event,
                ThreadEvent::DecoderError(_)
                    | ThreadEvent::ProcessingError(_)
                    | ThreadEvent::ThreadPanic(_)
            ),
            "engine EOS must not report errors: {event:?}"
        );
    }

    assert_eq!(program.len(), (frames + 96) * 2);
    assert!(program.iter().all(|sample| sample.is_finite()));
    let tail = &program[frames * 2..];
    let peak = tail.iter().map(|sample| sample.abs()).sum::<f32>();
    assert!(peak > 0.0, "tail must carry the impulse");
    for (index, &sample) in tail.iter().enumerate() {
        let frame = index / 2;
        if frame == 95 {
            assert!(
                (sample - IMPULSE_PEAK).abs() <= 1e-6,
                "impulse value must survive: {sample}"
            );
        } else {
            assert!(
                sample.abs() <= 1e-6,
                "only the impulse position is nonzero: [{frame}] = {sample}"
            );
        }
    }

    // The stream contract above is proven while the engine stays alive;
    // now stop the worker explicitly and validate termination. The
    // disconnect proves exactly-once EOS: no duplicate EOS marker and no
    // trailing frames follow the drained stream.
    worker.shutdown();
    worker.finished();
    assert!(
        matches!(
            worker.output().try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ),
        "worker must disconnect after shutdown, emitting nothing further"
    );
}

#[test]
fn linear_phase_deesser_delivers_exact_1120_frame_tail() {
    // Split-Band linear-phase plus 2 ms lookahead retains 96 lookahead
    // plus 1024 FIR samples; the graph drain must deliver all of them.
    let mut host = eos_build(&eos_deesser_key_graph("Split-Band", "Linear-Phase", 2.0));
    assert_eq!(host.output_channels(), 2);
    assert_eq!(host.total_latency_samples(), 608);
    let frames = 64usize;
    let input = eos_impulse_input(frames, IMPULSE_PEAK);
    let mut output = vec![0.0f32; frames * 2];
    assert_eq!(host.process(&input, &mut output).unwrap(), frames);

    let tail = eos_drain(&mut host);
    assert_eq!(tail.len(), 1120 * 2, "exact FIR plus lookahead tail");
    assert!(tail.iter().all(|sample| sample.is_finite()));
    // Linear-phase group delay (512) plus lookahead (96) places the
    // impulse peak; a two-frame window guards bank-summation detail.
    let peak = tail
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
        .unwrap()
        .0
        / 2;
    assert!(
        peak.abs_diff(607) <= 2,
        "impulse peak must sit at the latency horizon, got frame {peak}"
    );
    assert!(
        tail[peak * 2].abs() > IMPULSE_PEAK * 0.5,
        "crossover must preserve the impulse energy"
    );

    // Completed streams repeat, and reset restores the identical tail.
    let repeat = host.drain(&mut []).unwrap();
    assert_eq!(repeat.frames, 0);
    assert!(repeat.complete);
    host.reset();
    let mut output = vec![0.0f32; frames * 2];
    assert_eq!(host.process(&input, &mut output).unwrap(), frames);
    assert_eq!(eos_drain(&mut host), tail);
}

#[test]
fn capacity_retry_matches_untouched_twin() {
    let mut host = eos_build(&eos_deesser_key_graph("Wideband", "Minimum-Phase", 2.0));
    let mut twin = eos_build(&eos_deesser_key_graph("Wideband", "Minimum-Phase", 2.0));
    let frames = 48usize;
    let input = eos_impulse_input(frames, IMPULSE_PEAK);
    let mut output = vec![0.0f32; frames * 2];
    host.process(&input, &mut output).unwrap();
    let mut twin_out = vec![0.0f32; frames * 2];
    twin.process(&input, &mut twin_out).unwrap();

    let bound = host.drain_output_frames_max();
    assert!(bound > 0);
    let mut short = vec![12345.0; (bound - 1) * host.output_channels()];
    let error = host.drain(&mut short).unwrap_err();
    assert!(error.contains("too small"), "unexpected error: {error}");
    assert!(short.iter().all(|&sample| sample == 12345.0));

    assert_eq!(eos_drain(&mut host), eos_drain(&mut twin));
}

#[test]
fn hot_key_reduces_more_than_silent_key_at_eos() {
    // The key bus steers the detector independently: identical programs
    // reduce only when the key carries sibilance, and both tails still
    // complete with exact lengths.
    let frames = 4_096usize;
    let mut hot = eos_build(&eos_deesser_key_graph("Wideband", "Minimum-Phase", 2.0));
    let mut silent = eos_build(&eos_deesser_key_graph("Wideband", "Minimum-Phase", 2.0));
    let hot_input = eos_tone_input(frames, 0.3, 0.3);
    let silent_input = eos_tone_input(frames, 0.3, 0.0);
    let mut output = vec![0.0f32; frames * 2];
    hot.process(&hot_input, &mut output).unwrap();
    let mut output = vec![0.0f32; frames * 2];
    silent.process(&silent_input, &mut output).unwrap();

    let hot_tail = eos_drain(&mut hot);
    let silent_tail = eos_drain(&mut silent);
    assert_eq!(hot_tail.len(), 96 * 2);
    assert_eq!(silent_tail.len(), 96 * 2);
    let hot_rms = eos_rms(&hot_tail);
    let silent_rms = eos_rms(&silent_tail);
    assert!(
        hot_rms < silent_rms,
        "hot key must reduce the tail: {hot_rms} vs {silent_rms}"
    );
    let max_diff = hot_tail
        .iter()
        .zip(silent_tail.iter())
        .map(|(&a, &b)| (a - b).abs())
        .max_by(f32::total_cmp)
        .unwrap();
    assert!(
        max_diff > 0.01,
        "independent key must steer the detector audibly: {max_diff}"
    );
}
