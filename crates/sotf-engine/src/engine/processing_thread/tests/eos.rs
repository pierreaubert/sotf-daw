//! End-of-stream delivery, bypass, and interruption tests on the real worker.

use super::super::processing_state::run_processing_thread;
use super::super::{ProcessingReply, ProcessingRequest};
use super::request;
use crate::engine::{
    AudioFrame, DecoderMessage, GcItem, PreparedHostUpdate, ProcessingCommand, ProcessingMessage,
    ProcessingResponse, ThreadEvent,
};
use arc_swap::ArcSwap;
use sotf_plugins::plugin::PluginDrainResult;
use sotf_plugins::{
    Parameter, ParameterId, ParameterValue, Plugin, PluginHost, PluginInfo, ProcessContext,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum DrainMode {
    Tail,
    Error,
    Pending,
}

struct TailPlugin {
    drain_calls: Arc<AtomicUsize>,
    reset_calls: Arc<AtomicUsize>,
    command_on_drain: Option<(Sender<ProcessingRequest>, ProcessingRequest)>,
    inject_after: usize,
    tail_frames: usize,
    remaining: usize,
    mode: DrainMode,
}

impl Plugin for TailPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("EOS tail probe", "1", "Test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("No parameters".to_owned())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn output_sample_rate(&self, _: f64) -> f64 {
        96_000.0
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames * 2
    }
    fn reset(&mut self) {
        self.reset_calls.fetch_add(1, Ordering::SeqCst);
        self.remaining = 0;
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        assert_eq!(context.sample_rate, 48_000.0);
        for (frame, &sample) in input.iter().enumerate() {
            output[frame * 2..frame * 2 + 2].fill(sample);
        }
        self.remaining = self.tail_frames;
        Ok(input.len() * 2)
    }
    fn drain_output_frames_max(&self) -> usize {
        1
    }
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        match self.mode {
            DrainMode::Tail => std::num::NonZeroU64::new(self.remaining.max(1) as u64),
            _ => None,
        }
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        assert_eq!(context.sample_rate, 48_000.0);
        assert_eq!(context.num_frames, 0);
        if self.drain_calls.load(Ordering::SeqCst) == self.inject_after
            && let Some((sender, command)) = self.command_on_drain.take()
        {
            sender.send(command).unwrap();
        }
        self.drain_calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            DrainMode::Error => return Err("deliberate drain failure".to_owned()),
            DrainMode::Pending => {
                return Ok(PluginDrainResult {
                    frames: 0,
                    complete: false,
                });
            }
            DrainMode::Tail => {}
        }
        if self.remaining == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        output[0] = self.remaining as f32;
        self.remaining -= 1;
        Ok(PluginDrainResult {
            frames: 1,
            complete: self.remaining == 0,
        })
    }
}

struct Worker {
    decoder_tx: Option<Sender<DecoderMessage>>,
    command_tx: Sender<ProcessingRequest>,
    response_rx: Receiver<ProcessingReply>,
    output_rx: Option<Receiver<ProcessingMessage>>,
    event_rx: crossbeam::channel::Receiver<ThreadEvent>,
    finished_rx: Receiver<Result<(), String>>,
    decoder_recycle_rx: Receiver<Vec<f32>>,
    drain_calls: Arc<AtomicUsize>,
    reset_calls: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
    _gc_rx: crossbeam::channel::Receiver<GcItem>,
}

impl Worker {
    fn new(tail_frames: usize, injected: Option<ProcessingCommand>, mode: DrainMode) -> Self {
        Self::new_at(tail_frames, injected, mode, 0)
    }

    fn new_at(
        tail_frames: usize,
        injected: Option<ProcessingCommand>,
        mode: DrainMode,
        inject_after: usize,
    ) -> Self {
        let (decoder_tx, decoder_rx) = mpsc::channel();
        // A rendezvous channel guarantees that the injected command is
        // observed while the EOS send is pending, independent of scheduling.
        let (output_tx, output_rx) = mpsc::sync_channel(0);
        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let (event_tx, event_rx) = crossbeam::channel::bounded(32);
        let (gc_tx, gc_rx) = crossbeam::channel::bounded(32);
        let (_recycle_tx, recycle_rx) = mpsc::channel();
        let (decoder_recycle_tx, decoder_recycle_rx) = mpsc::sync_channel(4);
        let (finished_tx, finished_rx) = mpsc::channel();
        let drain_calls = Arc::new(AtomicUsize::new(0));
        let reset_calls = Arc::new(AtomicUsize::new(0));
        let mut host = PluginHost::new(1, 48_000);
        host.add_plugin(Box::new(TailPlugin {
            drain_calls: Arc::clone(&drain_calls),
            reset_calls: Arc::clone(&reset_calls),
            command_on_drain: injected.map(|command| (command_tx.clone(), request(command))),
            inject_after,
            tail_frames,
            remaining: 0,
            mode,
        }))
        .unwrap();
        host.build().unwrap();
        let update = PreparedHostUpdate::prepare(host, 48_000, 1, 0).unwrap();
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
                48_000,
                1,
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
            decoder_recycle_rx,
            drain_calls,
            reset_calls,
            thread: Some(thread),
            _gc_rx: gc_rx,
        };
        assert!(matches!(
            worker.response_rx.recv_timeout(WAIT).unwrap().response,
            ProcessingResponse::PluginChainUpdated { .. },
        ));
        // Prime the DSP and consume the normal converted frame before EOS.
        worker.send_frame(0.25);
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("expected priming frame");
        };
        assert_eq!(frame.data, vec![0.25; 2]);
        assert_eq!(frame.sample_rate, 96_000);
        worker.decoder_recycle_rx.recv_timeout(WAIT).unwrap();
        worker
    }

    fn output(&self) -> &Receiver<ProcessingMessage> {
        self.output_rx.as_ref().unwrap()
    }
    fn send_frame(&self, sample: f32) {
        self.decoder_tx
            .as_ref()
            .unwrap()
            .send(DecoderMessage::Frame(
                AudioFrame::try_new(vec![sample], 1, 1, 48_000).unwrap(),
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
    fn send_pending_frame(&self, sample: f32) {
        self.send_frame(sample);
        // Recycling happens after DSP, before the rendezvous send. Once this
        // arrives the frame is committed to that send path, not the idle loop.
        self.decoder_recycle_rx.recv_timeout(WAIT).unwrap();
    }
    fn finished(&self) {
        self.finished_rx
            .recv_timeout(WAIT)
            .expect("processing did not terminate promptly")
            .unwrap();
    }
}

#[test]
fn shutdown_during_normal_frame_backpressure_exits_the_worker() {
    let worker = Worker::new(3, None, DrainMode::Tail);
    worker.send_pending_frame(0.5);
    worker
        .command_tx
        .send(request(ProcessingCommand::Shutdown))
        .unwrap();
    // Keep both decoder and output endpoints alive. An inner-loop-only break
    // would leave the worker waiting for another decoder message indefinitely.
    worker.finished();
    assert!(worker.output().try_recv().is_err());
}

#[test]
fn stop_during_normal_frame_backpressure_discards_the_unsent_frame() {
    let worker = Worker::new(3, None, DrainMode::Tail);
    worker.send_pending_frame(0.5);
    worker
        .command_tx
        .send(request(ProcessingCommand::Stop))
        .unwrap();
    // This acknowledged no-op follows Stop on the command queue and proves
    // Stop was handled before opening the rendezvous output receiver.
    worker
        .command_tx
        .send(request(ProcessingCommand::Bypass(false)))
        .unwrap();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::Ok
    ));
    worker.send_frame(0.75);
    let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
        panic!("expected the new stream's frame");
    };
    assert_eq!(frame.data, vec![0.75; 2]);
    assert_eq!(worker.reset_calls.load(Ordering::SeqCst), 1);
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Also clean up failed pre-fix runs: disconnect both data directions so
        // an incorrectly continuing worker cannot outlive a failed assertion.
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
fn ordinary_eos_follows_every_tail_frame_once() {
    let worker = Worker::new(3, None, DrainMode::Tail);
    worker.eos();
    for expected in [3.0, 2.0, 1.0] {
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("EOS preceded the complete tail");
        };
        assert_eq!(frame.num_frames, 1);
        assert_eq!(frame.sample_rate, 96_000);
        assert_eq!(frame.data, vec![expected]);
    }
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 3);
    assert!(worker.output().try_recv().is_err());
}

#[test]
fn global_bypass_eos_discards_retained_host_tail() {
    let worker = Worker::new(3, None, DrainMode::Tail);
    worker
        .command_tx
        .send(request(ProcessingCommand::Bypass(true)))
        .unwrap();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::Ok
    ));
    worker.send_frame(0.75);
    let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
        panic!("expected bypassed frame");
    };
    assert_eq!(frame.sample_rate, 48_000);
    assert_eq!(frame.data, vec![0.75]);
    let resets_before = worker.reset_calls.load(Ordering::SeqCst);
    worker.eos();
    assert!(
        matches!(
            worker.output().recv_timeout(WAIT).unwrap(),
            ProcessingMessage::EndOfStream
        ),
        "bypass emitted retained DSP tail"
    );
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 0);
    assert_eq!(worker.reset_calls.load(Ordering::SeqCst), resets_before + 1);
}

fn assert_shutdown_interrupts(tail_frames: usize) {
    let worker = Worker::new(
        tail_frames,
        Some(ProcessingCommand::Shutdown),
        DrainMode::Tail,
    );
    worker.eos();
    worker.finished();
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            worker.output().recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "shutdown emitted an ordinary EOS or stale tail"
    );
}

#[test]
fn shutdown_interrupts_pending_tail_delivery() {
    assert_shutdown_interrupts(3);
}

#[test]
fn shutdown_interrupts_pending_eos() {
    assert_shutdown_interrupts(0);
}

#[test]
fn drain_error_notifies_and_disconnects_without_success_eos() {
    let worker = Worker::new(3, None, DrainMode::Error);
    worker.eos();
    assert!(matches!(
        worker.event_rx.recv_timeout(WAIT).unwrap(),
        ThreadEvent::ProcessingError(message) if message.contains("deliberate drain failure"),
    ));
    worker.finished();
    assert!(
        matches!(
            worker.output().recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "failed drain emitted ordinary EOS"
    );
}

#[test]
fn disconnected_playback_receiver_terminates_tail_drain() {
    let mut worker = Worker::new(3, None, DrainMode::Tail);
    drop(worker.output_rx.take());
    worker.eos();
    worker.finished();
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        worker.event_rx.recv_timeout(WAIT).unwrap(),
        ThreadEvent::ProcessingError(_)
    ));
}

#[test]
fn blocked_playback_receiver_times_out_without_success_eos() {
    for tail_frames in [0, 3] {
        let worker = Worker::new(tail_frames, None, DrainMode::Tail);
        worker.eos();
        // Leave the receiver connected without consuming. The existing
        // bounded send wait must fail and close the output stream.
        assert!(matches!(
            worker.event_rx.recv_timeout(WAIT).unwrap(),
            ThreadEvent::ProcessingError(message) if message.contains("queue stuck"),
        ));
        worker.finished();
        assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            worker.output().recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected),
        ));
    }
}

#[test]
fn bypass_during_blocked_tail_send_discards_unsent_dsp_audio() {
    let worker = Worker::new(3, Some(ProcessingCommand::Bypass(true)), DrainMode::Tail);
    worker.eos();
    // No output receive is pending, so the bypass must be handled while
    // the first tail frame is still unsent on the rendezvous channel.
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::Ok
    ));
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
    assert_eq!(worker.reset_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_interrupts_zero_output_drain_progress() {
    let worker = Worker::new(3, Some(ProcessingCommand::Shutdown), DrainMode::Pending);
    worker.eos();
    worker.finished();
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        worker.output().recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn cancelled_stop_preserves_pending_normal_frame() {
    let worker = Worker::new(0, None, DrainMode::Tail);
    worker.send_pending_frame(0.5);
    let cancelled = request(ProcessingCommand::Stop);
    assert!(cancelled.ticket.cancel());
    worker.command_tx.send(cancelled).unwrap();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::Error(message) if message.contains("cancelled")
    ));
    let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
        panic!("cancelled Stop lost the pending frame");
    };
    assert_eq!(frame.data, vec![0.5, 0.5]);
}

#[test]
fn stop_interrupts_pending_tail_without_publishing_old_eos() {
    for (frames, mode) in [
        (0, DrainMode::Tail),
        (3, DrainMode::Tail),
        (3, DrainMode::Pending),
    ] {
        let worker = Worker::new(frames, Some(ProcessingCommand::Stop), mode);
        worker.eos();
        // The probe enqueues Stop before incrementing its visible drain count.
        // Wait for the actual drain invocation, then acknowledge a later no-op.
        let deadline = std::time::Instant::now() + WAIT;
        while worker.drain_calls.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        worker
            .command_tx
            .send(request(ProcessingCommand::Bypass(false)))
            .unwrap();
        assert!(matches!(
            worker.response_rx.recv_timeout(WAIT).unwrap().response,
            ProcessingResponse::Ok
        ));
        worker.send_frame(0.75);
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("Stop published the discarded stream's EOS");
        };
        assert_eq!(frame.data, vec![0.75, 0.75]);
    }
}

#[test]
fn rate_only_host_commit_discards_pending_old_clock_tail() {
    let mut replacement = PluginHost::new(1, 48_000);
    replacement.build().unwrap();
    let update = PreparedHostUpdate::prepare(replacement, 48_000, 1, 0).unwrap();
    let worker = Worker::new(
        3,
        Some(ProcessingCommand::CommitHostUpdate(update)),
        DrainMode::Tail,
    );
    worker.eos();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::PluginChainUpdated {
            output_sample_rate: 48_000,
            ..
        }
    ));
    assert!(
        matches!(
            worker.output().recv_timeout(WAIT).unwrap(),
            ProcessingMessage::EndOfStream
        ),
        "old-clock tail escaped after the replacement was acknowledged"
    );
}

#[test]
fn shutdown_interrupts_pending_flush_with_live_channels() {
    let worker = Worker::new(0, None, DrainMode::Tail);
    let resets = worker.reset_calls.load(Ordering::SeqCst);
    worker
        .decoder_tx
        .as_ref()
        .unwrap()
        .send(DecoderMessage::Flush)
        .unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while worker.reset_calls.load(Ordering::SeqCst) == resets {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    worker
        .command_tx
        .send(request(ProcessingCommand::Shutdown))
        .unwrap();
    worker.finished();
}

#[test]
fn stop_interrupts_pending_flush_without_dropping_the_barrier() {
    let worker = Worker::new(0, None, DrainMode::Tail);
    let resets = worker.reset_calls.load(Ordering::SeqCst);
    worker
        .decoder_tx
        .as_ref()
        .unwrap()
        .send(DecoderMessage::Flush)
        .unwrap();
    // Arm-entry proof (precedent pattern): the Flush arm ran reset, so the
    // Stop below cannot be consumed early — it must interrupt the forward.
    let deadline = std::time::Instant::now() + WAIT;
    while worker.reset_calls.load(Ordering::SeqCst) == resets {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    worker
        .command_tx
        .send(request(ProcessingCommand::Stop))
        .unwrap();
    // The barrier survives Stop (pre-fix: dropped, and this recv times out).
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::Flush
    ));
    // The worker continues normally: a second barrier flows afterwards.
    worker
        .decoder_tx
        .as_ref()
        .unwrap()
        .send(DecoderMessage::Flush)
        .unwrap();
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::Flush
    ));
    assert!(worker.event_rx.try_recv().is_err());
    worker
        .command_tx
        .send(request(ProcessingCommand::Shutdown))
        .unwrap();
    worker.finished();
}

#[test]
fn channel_change_interrupts_pending_flush_without_dropping_the_barrier() {
    let mut replacement = PluginHost::new(2, 48_000);
    replacement.build().unwrap();
    // Arg 3 guards the CURRENT host (1ch): the commit rejects on mismatch.
    let update = PreparedHostUpdate::prepare(replacement, 48_000, 1, 0).unwrap();
    let worker = Worker::new(0, None, DrainMode::Tail);
    let resets = worker.reset_calls.load(Ordering::SeqCst);
    worker
        .decoder_tx
        .as_ref()
        .unwrap()
        .send(DecoderMessage::Flush)
        .unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while worker.reset_calls.load(Ordering::SeqCst) == resets {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    worker
        .command_tx
        .send(request(ProcessingCommand::CommitHostUpdate(update)))
        .unwrap();
    // The barrier survives the mid-forward host swap (pre-fix: dropped).
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::Flush
    ));
    // The injected update applied: output is now stereo.
    let update_response = worker.response_rx.recv_timeout(WAIT).unwrap().response;
    assert!(
        matches!(
            update_response,
            ProcessingResponse::PluginChainUpdated {
                output_channels: 2,
                ..
            }
        ),
        "expected stereo PluginChainUpdated, got {update_response:?}"
    );
    // Normal operation continues on the new host.
    worker
        .decoder_tx
        .as_ref()
        .unwrap()
        .send(DecoderMessage::Frame(
            AudioFrame::try_new(vec![0.5, 0.5], 1, 2, 48_000).unwrap(),
        ))
        .unwrap();
    let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
        panic!("expected post-update frame");
    };
    assert_eq!(frame.data, vec![0.5, 0.5]);
    assert!(worker.event_rx.try_recv().is_err());
    worker
        .command_tx
        .send(request(ProcessingCommand::Shutdown))
        .unwrap();
    worker.finished();
}

#[test]
fn nonconvergent_drain_reports_failure_and_disconnects() {
    let worker = Worker::new(3, None, DrainMode::Pending);
    worker.eos();
    assert!(matches!(worker.event_rx.recv_timeout(WAIT).unwrap(),
        ThreadEvent::ProcessingError(message) if message.contains("did not converge")));
    worker.finished();
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 4_096);
    assert!(matches!(
        worker.output().recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn legitimate_tail_longer_than_4096_calls_reaches_eos_with_final_marker() {
    let worker = Worker::new(5001, None, DrainMode::Tail);
    worker.eos();
    for expected in (1..=5001).rev() {
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("EOS preceded the long finite tail");
        };
        assert_eq!(frame.data, vec![expected as f32]);
    }
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 5001);
    assert!(worker.event_rx.try_recv().is_err());
}

#[test]
fn accepted_host_replacement_near_old_limit_gets_its_own_finite_quota() {
    let new_calls = Arc::new(AtomicUsize::new(0));
    let mut replacement = PluginHost::new(1, 48_000);
    replacement
        .add_plugin(Box::new(TailPlugin {
            drain_calls: Arc::clone(&new_calls),
            reset_calls: Arc::default(),
            command_on_drain: None,
            inject_after: 0,
            tail_frames: 5001,
            remaining: 5001,
            mode: DrainMode::Tail,
        }))
        .unwrap();
    replacement.build().unwrap();
    let update = PreparedHostUpdate::prepare(replacement, 48_000, 1, 0).unwrap();
    let worker = Worker::new_at(
        5001,
        Some(ProcessingCommand::CommitHostUpdate(update)),
        DrainMode::Tail,
        4095,
    );
    worker.eos();
    for expected in (907..=5001).rev() {
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("early EOS")
        };
        assert_eq!(frame.data, vec![expected as f32]);
    }
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    // Same-format replacement retains the already rendered pending frame906.
    let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
        panic!("missing pending frame")
    };
    assert_eq!(frame.data, vec![906.0]);
    for expected in (1..=5001).rev() {
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("truncated replacement")
        };
        assert_eq!(frame.data, vec![expected as f32]);
    }
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 4096);
    assert_eq!(new_calls.load(Ordering::SeqCst), 5001);
}

fn terminal_replacement_update() -> (PreparedHostUpdate, Arc<AtomicUsize>) {
    terminal_replacement_update_with_frames(3)
}

fn terminal_replacement_update_with_frames(
    frames: usize,
) -> (PreparedHostUpdate, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut replacement = PluginHost::new(1, 48_000);
    replacement
        .add_plugin(Box::new(TailPlugin {
            drain_calls: Arc::clone(&calls),
            reset_calls: Arc::default(),
            command_on_drain: None,
            inject_after: 0,
            tail_frames: frames,
            remaining: frames,
            mode: DrainMode::Tail,
        }))
        .unwrap();
    replacement.build().unwrap();
    (
        PreparedHostUpdate::prepare(replacement, 48_000, 1, 0).unwrap(),
        calls,
    )
}

fn assert_terminal_replacement_is_drained(old_tail_frames: usize) {
    let (update, new_calls) = terminal_replacement_update();
    let worker = Worker::new(
        old_tail_frames,
        Some(ProcessingCommand::CommitHostUpdate(update)),
        DrainMode::Tail,
    );
    worker.eos();
    // Output is a rendezvous channel. No receiver is admitted until the
    // injected replacement is acknowledged, fixing the interleaving exactly.
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    if old_tail_frames == 1 {
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("missing already rendered old terminal frame");
        };
        // Preserve the established same-format pending-frame delivery policy.
        assert_eq!(frame.data, vec![1.0]);
        assert_eq!(frame.sample_rate, 96_000);
    }
    for expected in [3.0, 2.0, 1.0] {
        let message = worker.output().recv_timeout(WAIT).unwrap();
        let ProcessingMessage::Frame(frame) = message else {
            panic!(
                "stale EOS discarded replacement tail: old_tail_frames={old_tail_frames}, \
                 expected={expected}, new_drain_calls={}",
                new_calls.load(Ordering::SeqCst)
            );
        };
        assert_eq!(frame.data, vec![expected]);
        assert_eq!(frame.sample_rate, 96_000);
    }
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
    assert_eq!(new_calls.load(Ordering::SeqCst), 3);
}

#[test]
fn terminal_replacement_after_final_tail_send_drains_new_host() {
    assert_terminal_replacement_is_drained(1);
}

#[test]
fn terminal_replacement_during_blocked_eos_drains_new_host() {
    assert_terminal_replacement_is_drained(0);
}

fn assert_rejected_terminal_replacements_preserve_completion(old_tail_frames: usize) {
    for rejection in [
        "metadata",
        "generation",
        "prepared ticket",
        "request ticket",
    ] {
        let (mut update, new_calls) = terminal_replacement_update();
        match rejection {
            "metadata" => update.expected_latency_samples = 17,
            "generation" => update = update.with_generation(1),
            "prepared ticket" => assert!(update.ticket().cancel()),
            "request ticket" => {}
            _ => unreachable!(),
        }
        let worker = Worker::new(old_tail_frames, None, DrainMode::Tail);
        worker.eos();
        let deadline = std::time::Instant::now() + WAIT;
        while worker.drain_calls.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let command = request(ProcessingCommand::CommitHostUpdate(update));
        if rejection == "request ticket" {
            assert!(command.ticket.cancel());
        }
        worker.command_tx.send(command).unwrap();
        let ProcessingResponse::Error(error) =
            worker.response_rx.recv_timeout(WAIT).unwrap().response
        else {
            panic!("{rejection} replacement was accepted");
        };
        assert!(error.contains("stale") || error.contains("cancelled"));
        if old_tail_frames == 1 {
            let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap()
            else {
                panic!("rejected replacement lost the pending terminal frame");
            };
            assert_eq!(frame.data, vec![1.0]);
        }
        assert!(matches!(
            worker.output().recv_timeout(WAIT).unwrap(),
            ProcessingMessage::EndOfStream
        ));
        assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
        assert_eq!(new_calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn terminal_replacement_rejection_preserves_final_tail_completion() {
    assert_rejected_terminal_replacements_preserve_completion(1);
}

#[test]
fn terminal_replacement_rejection_preserves_pending_eos_completion() {
    assert_rejected_terminal_replacements_preserve_completion(0);
}

fn terminal_replacement_blocked_new_tail() -> (Worker, Arc<AtomicUsize>) {
    let (update, new_calls) = terminal_replacement_update();
    let worker = Worker::new(
        0,
        Some(ProcessingCommand::CommitHostUpdate(update)),
        DrainMode::Tail,
    );
    worker.eos();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    let deadline = std::time::Instant::now() + WAIT;
    while new_calls.load(Ordering::SeqCst) == 0 {
        assert!(std::time::Instant::now() < deadline);
        assert!(
            matches!(
                worker.finished_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ),
            "worker terminated before draining the replacement"
        );
        std::thread::yield_now();
    }
    (worker, new_calls)
}

#[test]
fn terminal_replacement_restarted_drain_observes_stop() {
    let (worker, new_calls) = terminal_replacement_blocked_new_tail();
    worker
        .command_tx
        .send(request(ProcessingCommand::Stop))
        .unwrap();
    worker
        .command_tx
        .send(request(ProcessingCommand::Bypass(false)))
        .unwrap();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::Ok
    ));
    worker.send_frame(0.75);
    let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
        panic!("Stop retained a stale EOS");
    };
    assert_eq!(frame.data.len(), 2);
    assert_eq!(frame.data[0], 0.75);
    // Stop resets the existing 50 ms convex transition. Both reset hosts
    // receive this fresh sample, so the convex gains sum to one at frame 1.
    let alpha = 1.0 / (96_000.0 * 0.050);
    let expected = 0.75 * ((1.0 - alpha) + alpha);
    assert!((f64::from(frame.data[1]) - expected).abs() <= f64::from(f32::EPSILON));
    assert_eq!(new_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn terminal_replacement_restarted_drain_observes_shutdown() {
    let (worker, new_calls) = terminal_replacement_blocked_new_tail();
    worker
        .command_tx
        .send(request(ProcessingCommand::Shutdown))
        .unwrap();
    worker.finished();
    assert!(matches!(
        worker.output().recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    assert_eq!(new_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn terminal_replacement_empty_host_completes_once() {
    for old_tail_frames in [0, 1] {
        let (update, new_calls) = terminal_replacement_update_with_frames(0);
        let worker = Worker::new(
            old_tail_frames,
            Some(ProcessingCommand::CommitHostUpdate(update)),
            DrainMode::Tail,
        );
        worker.eos();
        assert!(matches!(
            worker.response_rx.recv_timeout(WAIT).unwrap().response,
            ProcessingResponse::PluginChainUpdated { .. }
        ));
        if old_tail_frames == 1 {
            let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap()
            else {
                panic!("empty replacement lost the old pending frame");
            };
            assert_eq!(frame.data, vec![1.0]);
        }
        assert!(matches!(
            worker.output().recv_timeout(WAIT).unwrap(),
            ProcessingMessage::EndOfStream
        ));
        assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
        assert_eq!(new_calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn terminal_replacement_repeated_generation_zero_commits_keep_pending_frame_once() {
    let (first_update, first_calls) = terminal_replacement_update();
    let (last_update, last_calls) = terminal_replacement_update();
    let worker = Worker::new(
        1,
        Some(ProcessingCommand::CommitHostUpdate(first_update)),
        DrainMode::Tail,
    );
    worker.eos();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    // Both successful commits have the permitted generation zero. The old
    // frame is still blocked, so neither replacement has processed its tail.
    worker
        .command_tx
        .send(request(ProcessingCommand::CommitHostUpdate(last_update)))
        .unwrap();
    assert!(matches!(
        worker.response_rx.recv_timeout(WAIT).unwrap().response,
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    for expected in [1.0, 3.0, 2.0, 1.0] {
        let ProcessingMessage::Frame(frame) = worker.output().recv_timeout(WAIT).unwrap() else {
            panic!("repeated replacement published a premature EOS");
        };
        assert_eq!(frame.data, vec![expected]);
    }
    assert!(matches!(
        worker.output().recv_timeout(WAIT).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    assert_eq!(worker.drain_calls.load(Ordering::SeqCst), 1);
    assert_eq!(first_calls.load(Ordering::SeqCst), 0);
    assert_eq!(last_calls.load(Ordering::SeqCst), 3);
}
