//! Portable tests of the actual RemoteIO feeder and callback, without hardware calls.

// Rust guideline compliant 2026-02-21
use super::super::misc::core_audio_ffi as ca;
use super::super::playback_state::PlaybackState;
use super::super::playback_thread::PlaybackThread;
use super::super::types::{RenderContext, accumulate_output_meter_and_clamp, render_callback};
use super::{
    adopt_stub_epoch, finalize_pending_stop_ack, run_feeder, snapshot_drained_receipt,
    swap_fold_stub_residual,
};
use crate::AudioFrame;
use crate::engine::{
    AudioEngineState, HostUpdateTicket, PendingStopAcks, PlaybackCommand, PlaybackConfiguration,
    PlaybackReconfigureRequest, PlaybackStopAck, PlaybackStopRequest, ProcessingMessage,
    ThreadEvent,
};
use arc_swap::ArcSwap;
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

// Callback counters are thread-local: ordinary feeder work is not claimed to be realtime.
thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
struct CallbackAllocator;
// SAFETY: Every pointer/layout is forwarded unchanged to CountingAlloc; counters use
// constant-initialized, nonallocating TLS and do not access allocated storage.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|active| {
            if active.get() {
                ALLOCS.with(|v| v.set(v.get() + 1));
            }
        });
        // SAFETY: Forward the caller's valid layout unchanged.
        unsafe { sotf_plugins::CountingAlloc.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|active| {
            if active.get() {
                FREES.with(|v| v.set(v.get() + 1));
            }
        });
        // SAFETY: Forward the allocated pointer and its original layout unchanged.
        unsafe { sotf_plugins::CountingAlloc.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

const FORMAT: PlaybackConfiguration = PlaybackConfiguration {
    sample_rate: 48_000,
    channels: 2,
};
const WAIT: Duration = Duration::from_secs(1);
fn frame(value: f32, frames: usize) -> ProcessingMessage {
    ProcessingMessage::Frame(AudioFrame::new(
        vec![value; frames * FORMAT.channels],
        frames,
        FORMAT.channels,
        FORMAT.sample_rate,
    ))
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "worker did not reach the expected state"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Supply the exact one-buffer interleaved f32 ABI used by the real callback.
fn render(context: &mut RenderContext, output: &mut [f32]) {
    assert_eq!(output.len() % context.channels, 0);
    let mut flags = 0;
    let mut buffers = ca::AudioBufferList {
        number_buffers: 1,
        buffers: [ca::AudioBuffer {
            number_channels: context.channels as u32,
            data_byte_size: std::mem::size_of_val(output).try_into().unwrap(),
            data: output.as_mut_ptr().cast(),
        }],
    };
    // SAFETY: The context and single-buffer list live for this synchronous call;
    // output is exclusively borrowed, aligned f32 storage of the declared length.
    // Calls are serialized. The callback never dereferences its timestamp argument.
    let status = unsafe {
        render_callback(
            std::ptr::from_mut(context).cast(),
            &mut flags,
            std::ptr::null(),
            0,
            (output.len() / context.channels) as u32,
            &mut buffers,
        )
    };
    assert_eq!(status, ca::noErr);
    assert!(!context.state.callback_active.load(Ordering::Acquire));
}

struct Fixture {
    // Drop the worker first, while all queues still exist, to guarantee shutdown on panic.
    worker: PlaybackThread,
    messages: Option<Sender<ProcessingMessage>>,
    events: crossbeam::channel::Receiver<ThreadEvent>,
    recycled: Receiver<Vec<f32>>,
    stopped: Receiver<Result<(), String>>,
    context: RenderContext,
}

impl Fixture {
    fn new(initial: &[f32]) -> Self {
        Self::with_capacity(initial, 16)
    }

    fn with_capacity(initial: &[f32], capacity: usize) -> Self {
        let (mut producer, consumer) = rtrb::RingBuffer::new(capacity);
        for &sample in initial {
            producer.push(sample).unwrap();
        }
        let state = Arc::new(PlaybackState::new(capacity));
        let (message_tx, message_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = crossbeam::channel::unbounded();
        let (recycle_tx, recycle_rx) = mpsc::sync_channel(64);
        let (stopped_tx, stopped_rx) = mpsc::sync_channel(1);
        let callback_state = Arc::clone(&state);
        let thread_handle = std::thread::spawn(move || {
            let result = run_feeder(
                message_rx,
                command_rx,
                event_tx,
                FORMAT.sample_rate,
                FORMAT.channels,
                producer,
                state,
                recycle_tx,
            );
            stopped_tx.send(result).ok();
        });
        Self {
            worker: PlaybackThread {
                command_tx,
                thread_handle: Some(thread_handle),
                pending_stop_acks: PendingStopAcks::default(),
                exit_status: crate::engine::worker_death::WorkerExitStatus::new(),
                retained_output_peak_bits: Arc::new(std::sync::atomic::AtomicU32::new(
                    0.0f32.to_bits(),
                )),
            },
            messages: Some(message_tx),
            events: event_rx,
            recycled: recycle_rx,
            stopped: stopped_rx,
            context: RenderContext {
                consumer,
                state: callback_state,
                sample_rate: FORMAT.sample_rate,
                channels: FORMAT.channels,
            },
        }
    }

    fn send(&self, message: ProcessingMessage) {
        self.messages.as_ref().unwrap().send(message).unwrap();
    }
    fn callback(&mut self, samples: usize) -> Vec<f32> {
        let mut output = vec![f32::NAN; samples];
        render(&mut self.context, &mut output);
        output
    }
    fn expect_recycled(&self, value: f32, samples: usize) {
        assert_eq!(
            self.recycled.recv_timeout(WAIT).unwrap(),
            vec![value; samples]
        );
    }
    fn finish(&mut self) {
        self.worker.shutdown();
        self.stopped.recv_timeout(WAIT).unwrap().unwrap();
    }
}

#[test]
fn fresh_frame_immediately_after_fifo_flush_survives_the_callback_flush() {
    let mut fixture = Fixture::new(&[0.75; 12]);
    fixture.send(ProcessingMessage::Flush);
    fixture.send(frame(0.25, 2));
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    assert!(
        fixture
            .recycled
            .recv_timeout(Duration::from_millis(20))
            .is_err()
    );
    assert_eq!(fixture.callback(4), vec![0.0; 4]);
    assert!(
        fixture
            .recycled
            .recv_timeout(Duration::from_millis(20))
            .is_err()
    );
    assert_eq!(fixture.callback(8), vec![0.0; 8]);
    fixture.expect_recycled(0.25, 4);
    assert_eq!(fixture.callback(4), vec![0.25; 4]);
    fixture.finish();
}

#[test]
fn fresh_stub_worker_reports_running_exit_status() {
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let worker = PlaybackThread {
        command_tx,
        thread_handle: None,
        pending_stop_acks: PendingStopAcks::default(),
        exit_status: crate::engine::worker_death::WorkerExitStatus::new(),
        retained_output_peak_bits: Arc::new(std::sync::atomic::AtomicU32::new(0.0f32.to_bits())),
    };
    assert_eq!(
        worker.exit_status(),
        Some(crate::engine::worker_death::WorkerExit::Running)
    );
}

#[test]
fn disconnected_processing_eos_still_handles_shutdown_without_waiting_for_audio() {
    let mut fixture = Fixture::new(&[0.75; 16]);
    fixture.send(ProcessingMessage::EndOfStream);
    fixture.send(ProcessingMessage::Frame(AudioFrame::new(
        vec![0.25; 4],
        2,
        2,
        44_100,
    )));
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::ProcessingError(_)
    ));
    fixture.expect_recycled(0.25, 4);
    fixture.messages.take();
    // A command after disconnection must be handled even with an undrainable ring.
    fixture
        .worker
        .send_command(PlaybackCommand::Mute(true))
        .unwrap();
    wait_until(|| fixture.context.state.muted.load(Ordering::Acquire));
    fixture
        .worker
        .send_command(PlaybackCommand::Shutdown)
        .unwrap();
    fixture.stopped.recv_timeout(WAIT).unwrap().unwrap();
    assert!(
        fixture.events.try_recv().is_err(),
        "shutdown must not report successful drain"
    );
}

#[test]
fn cold_render_callback_has_no_allocations_or_frees_on_audio_flush_or_underrun_paths() {
    for channels in [1, 2, 8] {
        let (mut producer, consumer) = rtrb::RingBuffer::new(64 * channels);
        for _ in 0..32 * channels {
            producer.push(0.25).unwrap();
        }
        let state = Arc::new(PlaybackState::new(64 * channels));
        let mut context = RenderContext {
            consumer,
            state,
            sample_rate: 48_000,
            channels,
        };
        let mut output = vec![f32::NAN; 17 * channels];
        std::thread::spawn(move || {
            ALLOCS.set(0);
            FREES.set(0);
            TRACKING.set(true);
            render(&mut context, &mut output);
            assert!(output.iter().all(|&sample| sample == 0.25));
            context.state.flush_requested.store(true, Ordering::Release);
            render(&mut context, &mut output);
            assert!(output.iter().all(|&sample| sample == 0.0));
            assert!(!context.state.flush_requested.load(Ordering::Acquire));
            render(&mut context, &mut output);
            assert!(output.iter().all(|&sample| sample == 0.0));
            TRACKING.set(false);
            assert_eq!((ALLOCS.get(), FREES.get()), (0, 0));
        })
        .join()
        .unwrap();
    }
}

#[test]
fn saturated_ring_and_oversized_pending_frames_keep_mute_and_shutdown_responsive() {
    for frames in [2, 17] {
        let fixture = Fixture::new(&[0.75; 16]);
        fixture.send(frame(0.25, frames));
        // Each loop handles a command, then its pending/message frame. Observing
        // Mute guarantees this frame is attempted before a later Shutdown.
        fixture
            .worker
            .send_command(PlaybackCommand::Mute(true))
            .unwrap();
        wait_until(|| fixture.context.state.muted.load(Ordering::Acquire));
        fixture
            .worker
            .send_command(PlaybackCommand::SetVolume(0.5))
            .unwrap();
        wait_until(|| fixture.context.state.volume.load(Ordering::Acquire) == 0.5_f32.to_bits());
        assert!(fixture.recycled.try_recv().is_err());
        fixture
            .worker
            .send_command(PlaybackCommand::Shutdown)
            .unwrap();
        fixture.stopped.recv_timeout(WAIT).unwrap().unwrap();
        fixture.expect_recycled(0.25, frames * FORMAT.channels);
        wait_until(|| fixture.worker.is_finished());
    }
}

#[test]
fn stop_and_resume_keep_dropping_old_audio_until_fifo_flush() {
    let mut fixture = Fixture::new(&[0.75; 12]);
    fixture
        .worker
        .send_command(PlaybackCommand::Stop(PlaybackStopRequest::default()))
        .unwrap();
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 0 })
        .unwrap();
    fixture.send(frame(0.25, 2));
    fixture.expect_recycled(0.25, 4);
    assert_eq!(fixture.callback(12), vec![0.0; 12]);
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    fixture.send(frame(0.5, 2));
    fixture.expect_recycled(0.5, 4);
    assert_eq!(fixture.callback(4), vec![0.0; 4]);
    fixture.send(ProcessingMessage::Flush);
    fixture.send(frame(0.75, 2));
    fixture.expect_recycled(0.75, 4);
    assert_eq!(fixture.callback(4), vec![0.75; 4]);
    fixture.finish();
}

#[test]
fn pause_resume_waits_for_callback_flush_and_accepts_audio_afterwards() {
    for resume_before_flush in [false, true] {
        let mut fixture = Fixture::new(&[0.75; 12]);
        fixture.worker.send_command(PlaybackCommand::Pause).unwrap();
        wait_until(|| {
            fixture
                .context
                .state
                .flush_requested
                .load(Ordering::Acquire)
        });
        if resume_before_flush {
            fixture
                .worker
                .send_command(PlaybackCommand::Resume { epoch: 0 })
                .unwrap();
        }
        fixture.send(frame(0.25, 2));
        fixture.expect_recycled(0.25, 4);
        assert_eq!(fixture.callback(4), vec![0.0; 4]);
        assert_eq!(fixture.callback(8), vec![0.0; 8]);
        if !resume_before_flush {
            fixture.send(frame(0.5, 2));
            fixture.expect_recycled(0.5, 4);
            assert_eq!(fixture.callback(4), vec![0.0; 4]);
            fixture
                .worker
                .send_command(PlaybackCommand::Resume { epoch: 0 })
                .unwrap();
        }
        // This later command reply is a FIFO barrier for Resume processing.
        assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
        fixture.send(frame(0.75, 2));
        fixture.expect_recycled(0.75, 4);
        assert_eq!(fixture.callback(4), vec![0.75; 4]);
        fixture.finish();
    }
}

#[test]
fn invalid_source_format_or_dimensions_report_error_without_relabeling_audio() {
    let mut fixture = Fixture::new(&[]);
    for (rate, channels) in [(44_100, 2), (48_000, 1), (48_000, 6)] {
        fixture.send(ProcessingMessage::Frame(AudioFrame::new(
            vec![0.25; 2 * channels],
            2,
            channels,
            rate,
        )));
        let error = fixture.events.recv_timeout(WAIT).unwrap();
        assert!(matches!(error, ThreadEvent::ProcessingError(_)));
        fixture.expect_recycled(0.25, 2 * channels);
        assert_eq!(fixture.callback(4), vec![0.0; 4]);
    }
    for (frames, samples) in [(2, 3), (usize::MAX, 2), (0, 2)] {
        fixture.send(ProcessingMessage::Frame(AudioFrame {
            data: vec![0.25; samples],
            num_frames: frames,
            num_channels: 2,
            sample_rate: 48_000,
        }));
        assert!(matches!(
            fixture.events.recv_timeout(WAIT).unwrap(),
            ThreadEvent::ProcessingError(_)
        ));
        fixture.expect_recycled(0.25, samples);
        assert_eq!(fixture.callback(4), vec![0.0; 4]);
    }
    fixture.send(frame(0.5, 2));
    fixture.expect_recycled(0.5, 4);
    assert_eq!(fixture.callback(4), vec![0.5; 4]);
    fixture.finish();
}

#[test]
fn legacy_reconfiguration_reports_support_and_honors_cancelled_tickets() {
    let mut fixture = Fixture::new(&[0.75; 12]);
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    assert!(fixture.worker.reconfigure(44_100, 2).is_err());
    assert!(fixture.worker.reconfigure(48_000, 6).is_err());
    let ticket = HostUpdateTicket::new();
    assert!(ticket.cancel());
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    fixture
        .worker
        .send_command(PlaybackCommand::Reconfigure(PlaybackReconfigureRequest {
            requested: FORMAT,
            ticket,
            reply_tx,
        }))
        .unwrap();
    assert!(reply_rx.recv_timeout(WAIT).unwrap().is_err());
    // Same-format acknowledgements, unsupported changes and cancellation all
    // preserve the existing ring and its exact old-format signal.
    assert_eq!(fixture.callback(12), vec![0.75; 12]);
    fixture.send(frame(0.5, 2));
    fixture.expect_recycled(0.5, 4);
    assert_eq!(fixture.callback(4), vec![0.5; 4]);
    fixture.finish();
}

#[test]
fn oversized_frame_preserves_every_channel_sample_and_drains_after_the_final_frame() {
    for capacity in [16, 17] {
        let mut fixture = Fixture::with_capacity(&[], capacity);
        let source: Vec<f32> = (0..74)
            .map(|sample| (sample as f32 - 36.0) / 128.0)
            .collect();
        fixture.send(ProcessingMessage::Frame(AudioFrame::new(
            source.clone(),
            37,
            2,
            48_000,
        )));
        fixture.send(ProcessingMessage::EndOfStream);
        let mut actual = Vec::with_capacity(source.len());
        for frames in [3, 5, 2, 7].into_iter().cycle() {
            let samples = (frames * 2).min(source.len() - actual.len());
            if samples == 0 {
                break;
            }
            wait_until(|| {
                let available = fixture.context.consumer.slots();
                assert_eq!(
                    available % 2,
                    0,
                    "published a partial interleaved frame at capacity {capacity}"
                );
                available >= samples
            });
            assert!(
                fixture.events.try_recv().is_err(),
                "reported EOS while audio remained"
            );
            actual.extend_from_slice(&fixture.callback(samples));
        }
        assert_eq!(actual, source, "capacity={capacity}");
        assert_eq!(fixture.recycled.recv_timeout(WAIT).unwrap(), source);
        assert!(matches!(
            fixture.events.recv_timeout(WAIT).unwrap(),
            ThreadEvent::PlaybackDrained { .. }
        ));
        assert_eq!(fixture.context.consumer.slots(), 0);
        fixture.finish();
    }
}

#[test]
fn oversized_frame_stop_discards_only_unplayed_suffix_and_flush_accepts_fresh_audio() {
    let mut fixture = Fixture::with_capacity(&[], 17);
    let source: Vec<f32> = (0..74)
        .map(|sample| (sample as f32 - 36.0) / 128.0)
        .collect();
    fixture.send(ProcessingMessage::Frame(AudioFrame::new(
        source.clone(),
        37,
        2,
        48_000,
    )));
    fixture.send(ProcessingMessage::EndOfStream);
    wait_until(|| fixture.context.consumer.slots() >= 8);
    assert_eq!(fixture.callback(8), source[..8]);
    fixture
        .worker
        .send_command(PlaybackCommand::Stop(PlaybackStopRequest::default()))
        .unwrap();
    // The original buffer cannot be fully published after only eight consumed
    // samples; its recycling is therefore a deterministic Stop acknowledgement.
    assert_eq!(fixture.recycled.recv_timeout(WAIT).unwrap(), source);
    assert_eq!(fixture.callback(16), vec![0.0; 16]);
    assert_eq!(fixture.context.consumer.slots(), 0);
    assert!(
        fixture.events.try_recv().is_err(),
        "Stop reported the discarded old EOS"
    );
    let fresh = vec![0.8125, -0.6875, 0.625, -0.5];
    fixture.send(ProcessingMessage::Flush);
    fixture.send(ProcessingMessage::Frame(AudioFrame::new(
        fresh.clone(),
        2,
        2,
        48_000,
    )));
    fixture.send(ProcessingMessage::EndOfStream);
    assert_eq!(fixture.recycled.recv_timeout(WAIT).unwrap(), fresh);
    assert_eq!(fixture.callback(4), fresh);
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::PlaybackDrained { .. }
    ));
    fixture.finish();
}

#[test]
fn resume_with_new_epoch_fences_old_eos_and_tags_fresh_drained() {
    let mut fixture = Fixture::new(&[]);
    // Old stream: two frames with the old EOS between them on the message
    // FIFO. The second frame's recycle proves the feeder consumed past the
    // EOS (same-thread program order), so the old EOS is provably ARMED —
    // not merely queued — before the Resume below.
    fixture.send(frame(0.5, 2));
    fixture.send(ProcessingMessage::EndOfStream);
    fixture.send(frame(0.6, 2));
    fixture.expect_recycled(0.5, 4);
    fixture.expect_recycled(0.6, 4);
    // No messages in flight and no callbacks ran: the only end_of_stream
    // clearer that can run before the Flush below is the Resume fence
    // (drain completion needs callbacks; the 2 s timeout is unreachable
    // here). The fence provably acts on armed state.
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 1 })
        .unwrap();
    // FIFO barrier: this reply proves the Resume was processed.
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    // Fresh stream markers + audio + EOS. The Flush clears the (already
    // fenced) EOS flags and starts the callback drain; the fresh frame is
    // retained until that drain can no longer erase it, so it survives.
    fixture.send(ProcessingMessage::Flush);
    fixture.send(frame(0.25, 2));
    fixture.send(ProcessingMessage::EndOfStream);
    // The test callback races the feeder's Flush processing: wait for the
    // flush request (precedent pattern) so the stale drain is silence.
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    // Stale drains first (discarded silence), completing the flush; only
    // then is the retained fresh frame released for recycling.
    assert_eq!(fixture.callback(8), vec![0.0; 8]);
    fixture.expect_recycled(0.25, 4);
    // Fresh audio survives the callback flush (retain-to-survive).
    assert_eq!(fixture.callback(4), vec![0.25; 4]);
    // Exactly one drained receipt, tagged with the fresh epoch. Singleton
    // is deterministic: the only armed EOS is the fresh one, and the send
    // clears it, so no second receipt is state-possible. The event queue
    // accumulates, so a drained{0} would arrive first and fail the
    // pattern. The stub carries the cumulative peak (P4 parity): the
    // callback above emitted exactly [0.25; 4], so 0.25 is derived from
    // that prior assertion — the old 0.0 encoded the pre-P4 no-tap gap.
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::PlaybackDrained {
            epoch: 1,
            epoch_peak_max,
            // Exactly one Flush crossed the feeder in this test.
            flush_gen: 1,
        } if epoch_peak_max.to_bits() == 0.25f32.to_bits()
    ));
    assert!(fixture.events.try_recv().is_err());
    fixture.finish();
}

#[test]
fn stop_answers_quiesce_terminal_and_flush_rearms_callback() {
    let mut fixture = Fixture::new(&[0.75; 12]);
    let (request, ack_rx) = PlaybackStopRequest::new();
    fixture
        .worker
        .send_command(PlaybackCommand::Stop(request))
        .unwrap();
    // Quiesce is unreachable without callbacks: the ack stays pending.
    assert!(ack_rx.try_recv().is_err());
    // Prove the Stop arm landed: the flag has no other setter yet, so
    // this also proves the latch is set.
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    // Phase 0 (hold without a boundary): drain the stale ring with no
    // Flush in flight. The callback discards under the flag, the
    // terminal answers on quiesce (cumulative record; 0.0 here because
    // the discard branch meters nothing — P4 tap parity) — and the
    // latch MUST still be set: quiesce answers the completion but never
    // disarms the cutoff. Only a boundary or
    // a Resume re-arms. White-box by necessity: with no Flush every
    // frame stays dropped, so no behavioral probe here can tell
    // latch-discard from underrun; the desktop unit test pins the
    // discard mechanics with the flag clear.
    assert_eq!(fixture.callback(12), vec![0.0; 12]);
    let ack = ack_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(ack.epoch, 0);
    assert_eq!(ack.epoch_peak_max, 0.0);
    assert!(
        fixture.context.state.stop_latched.load(Ordering::Acquire),
        "quiesce must answer the Stop without disarming its cutoff"
    );
    // Phase 1 (boundary re-arm): a Flush re-arms unconditionally —
    // there is no scheduling conjunction left to pin (a hold would
    // need the worker ahead of the test's drain, an ordering no
    // barrier in this harness can force: Flush processing is silent
    // while the ring is full). The recycle proves the hot frame was
    // written, not dropped, so the fresh emission proves the re-arm.
    fixture.send(ProcessingMessage::Flush);
    fixture.send(frame(0.7, 2));
    fixture.expect_recycled(0.7, 4);
    assert_eq!(fixture.callback(4), vec![0.7; 4]);
    // Phase 2 (Resume re-arm): operator intent re-arms unconditionally.
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 0 })
        .unwrap();
    // FIFO barrier: this reply proves the Resume was processed.
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    fixture.send(frame(0.5, 2));
    fixture.expect_recycled(0.5, 4);
    assert_eq!(fixture.callback(4), vec![0.5; 4]);
    fixture.finish();
}

#[test]
fn resume_clears_stop_latch_without_any_flush() {
    // D4(b) isolation: the ONLY latch-clear in this sequence is the
    // Resume arm — no Flush is ever sent, and no quiesce observation
    // clears either. White-box: the latch is defense-in-depth (worker
    // drop-mode already holds the ring), so atomics are the only
    // probe; the Reconfigure barrier proves the Resume was processed.
    let mut fixture = Fixture::new(&[0.75; 12]);
    let (request, _ack_rx) = PlaybackStopRequest::new();
    fixture
        .worker
        .send_command(PlaybackCommand::Stop(request))
        .unwrap();
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    assert!(fixture.context.state.stop_latched.load(Ordering::Acquire));
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 0 })
        .unwrap();
    // FIFO barrier: this reply proves the Resume was processed.
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    assert!(!fixture.context.state.stop_latched.load(Ordering::Acquire));
    fixture.finish();
}

#[test]
fn new_epoch_resume_finalizes_pending_stop_before_fence() {
    let mut fixture = Fixture::new(&[0.75; 12]);
    let (request, ack_rx) = PlaybackStopRequest::new();
    fixture
        .worker
        .send_command(PlaybackCommand::Stop(request))
        .unwrap();
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    // Supersede pre-quiesce: no callbacks ran, so the ring is full.
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 1 })
        .unwrap();
    // The pending record answers with the PRE-fence epoch.
    let ack = ack_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(ack.epoch, 0);
    // Then the fence adopts the new epoch: fresh EOS plus a callback
    // drain yields exactly one drained receipt tagged with it. The
    // callback discards under either flag order (the stale Stop flag
    // or the fresh Flush flag), and the receipt itself proves the
    // worker consumed past the boundary.
    fixture.send(ProcessingMessage::Flush);
    fixture.send(ProcessingMessage::EndOfStream);
    assert_eq!(fixture.callback(12), vec![0.0; 12]);
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::PlaybackDrained {
            epoch: 1,
            epoch_peak_max: 0.0,
            flush_gen: 1,
        }
    ));
    assert!(fixture.events.try_recv().is_err());
    fixture.finish();
}

#[test]
fn drained_receipt_carries_consumed_flush_count() {
    let mut fixture = Fixture::new(&[]);
    fixture.send(ProcessingMessage::Flush);
    fixture.send(ProcessingMessage::Flush);
    fixture.send(ProcessingMessage::EndOfStream);
    // The ring is empty throughout, so the drain completes with no
    // callbacks; the receipt carries both consumed boundaries.
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::PlaybackDrained {
            epoch: 0,
            epoch_peak_max: 0.0,
            flush_gen: 2,
        }
    ));
    assert!(fixture.events.try_recv().is_err());
    fixture.finish();
}

#[test]
fn stub_stashed_stop_acks_fold_late_into_shared_state() {
    let mut fixture = Fixture::new(&[]);
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_epoch: 5,
        playback_peak_max_linear: 0.30,
        ..AudioEngineState::default()
    }));
    let (late_tx, late_rx) = mpsc::sync_channel(1);
    fixture.worker.stash_pending_stop_ack(late_rx);
    fixture.worker.collect_ready_stop_acks(&state);
    assert_eq!(state.load().playback_peak_max_linear, 0.30);
    late_tx
        .send(PlaybackStopAck {
            epoch: 5,
            epoch_peak_max: 0.90,
        })
        .unwrap();
    fixture.worker.collect_ready_stop_acks(&state);
    assert_eq!(state.load().playback_peak_max_linear, 0.90);
    fixture.finish();
}

#[test]
fn stub_tap_meters_pre_clamp_and_counts_clips() {
    // Direct call of the production tap (also called by render_callback):
    // peak observes pre-clamp magnitudes, clips count >1.0 plus
    // non-finite, output clamps in place. Boundary ±1.0 is NOT clipped
    // (strictly-greater rule, desktop-exact).
    let state = PlaybackState::new(8);
    let mut samples = vec![0.5, 1.5, -2.0, f32::NAN, -0.25, 1.0, -1.0];
    accumulate_output_meter_and_clamp(&mut samples, &state);
    assert_eq!(
        state.output_peak_bits.load(Ordering::Relaxed),
        2.0f32.to_bits()
    );
    assert_eq!(state.clipped_sample_count.load(Ordering::Relaxed), 3);
    assert_eq!(samples[0], 0.5);
    assert_eq!(samples[1], 1.0);
    assert_eq!(samples[2], -1.0);
    assert!(samples[3].is_nan());
    assert_eq!(samples[4], -0.25);
    assert_eq!(samples[5], 1.0);
    assert_eq!(samples[6], -1.0);
}

#[test]
fn stub_fence_after_finalize_keeps_racing_window() {
    // Hand-driven D1 analogue (desktop
    // adopt_after_finalize_keeps_racing_window_in_new_record): swap,
    // racing fetch_max, fence(true) preserves the window in the new
    // epoch's atomics, and the next swap folds it. No threads.
    let state = PlaybackState::new(8);
    let mut meter_epoch = 5u64;
    let mut epoch_peak_max = 0.0f32;
    let mut end_of_stream = true;
    let mut drain_start = Some(Instant::now());
    // Terminal swap folds 0.70 (production swap-fold).
    state
        .output_peak_bits
        .fetch_max(0.70f32.to_bits(), Ordering::Relaxed);
    swap_fold_stub_residual(&state, &mut epoch_peak_max);
    assert_eq!(epoch_peak_max, 0.70);
    // The ack carries epoch + cumulative (production finalize).
    let (request, ack_rx) = PlaybackStopRequest::new();
    let mut pending = Some(request.reply_tx);
    assert!(finalize_pending_stop_ack(
        &mut pending,
        &state,
        &mut epoch_peak_max,
        meter_epoch
    ));
    let ack = ack_rx.try_recv().unwrap();
    assert_eq!(ack.epoch, 5);
    assert_eq!(ack.epoch_peak_max, 0.70);
    // Racing window lands between the swap and the fence.
    state
        .output_peak_bits
        .fetch_max(0.42f32.to_bits(), Ordering::Relaxed);
    // Fence(true): epoch adopts, cumulative resets, flags clear, but
    // the residual is KEPT (never wiped into neither record).
    adopt_stub_epoch(
        &state,
        &mut meter_epoch,
        &mut epoch_peak_max,
        &mut end_of_stream,
        &mut drain_start,
        6,
        true,
    );
    assert_eq!(meter_epoch, 6);
    assert_eq!(epoch_peak_max, 0.0);
    assert!(!end_of_stream);
    assert!(drain_start.is_none());
    assert_eq!(
        state.output_peak_bits.load(Ordering::Relaxed),
        0.42f32.to_bits()
    );
    // The new epoch's next swap folds it (misattributed, present).
    swap_fold_stub_residual(&state, &mut epoch_peak_max);
    assert_eq!(epoch_peak_max, 0.42);
}

#[test]
fn stub_fence_without_finalize_resets_abandoned_tail() {
    let state = PlaybackState::new(8);
    let mut meter_epoch = 6u64;
    let mut epoch_peak_max = 0.70f32;
    let mut end_of_stream = true;
    let mut drain_start = Some(Instant::now());
    state
        .output_peak_bits
        .fetch_max(0.90f32.to_bits(), Ordering::Relaxed);
    state.clipped_sample_count.store(5, Ordering::Relaxed);
    // No pending Stop: finalize reports no swap ran.
    let mut pending = None;
    assert!(!finalize_pending_stop_ack(
        &mut pending,
        &state,
        &mut epoch_peak_max,
        meter_epoch
    ));
    // Fence(false): abandoned tail resets everything (the old epoch
    // has no record, so its residual drops by design).
    adopt_stub_epoch(
        &state,
        &mut meter_epoch,
        &mut epoch_peak_max,
        &mut end_of_stream,
        &mut drain_start,
        7,
        false,
    );
    assert_eq!(meter_epoch, 7);
    assert_eq!(epoch_peak_max, 0.0);
    assert!(!end_of_stream);
    assert!(drain_start.is_none());
    assert_eq!(
        state.output_peak_bits.load(Ordering::Relaxed),
        0.0f32.to_bits()
    );
    assert_eq!(state.clipped_sample_count.load(Ordering::Relaxed), 0);
    // Same-epoch resumes are a no-op by comparison, for both flags.
    state
        .output_peak_bits
        .fetch_max(0.11f32.to_bits(), Ordering::Relaxed);
    epoch_peak_max = 0.30;
    end_of_stream = true;
    adopt_stub_epoch(
        &state,
        &mut meter_epoch,
        &mut epoch_peak_max,
        &mut end_of_stream,
        &mut drain_start,
        7,
        true,
    );
    adopt_stub_epoch(
        &state,
        &mut meter_epoch,
        &mut epoch_peak_max,
        &mut end_of_stream,
        &mut drain_start,
        7,
        false,
    );
    assert_eq!(meter_epoch, 7);
    assert_eq!(epoch_peak_max, 0.30);
    assert!(end_of_stream);
    assert_eq!(
        state.output_peak_bits.load(Ordering::Relaxed),
        0.11f32.to_bits()
    );
}

#[test]
fn drained_builder_folds_residual_before_tagging_receipt() {
    // Direct call of the sole production builder: the swapped window
    // max-composes with the cumulative (never overwrites), the
    // receipt tags epoch + cumulative + flush gen, and the swap
    // clears both atomics.
    let state = PlaybackState::new(8);
    let mut cumulative = 0.9f32;
    state
        .output_peak_bits
        .store(0.5f32.to_bits(), Ordering::Relaxed);
    state.clipped_sample_count.store(7, Ordering::Relaxed);
    let receipt = snapshot_drained_receipt(&state, &mut cumulative, 4, 2);
    match receipt {
        ThreadEvent::PlaybackDrained {
            epoch,
            epoch_peak_max,
            flush_gen,
        } => {
            assert_eq!(epoch, 4);
            assert_eq!(epoch_peak_max, 0.9);
            assert_eq!(flush_gen, 2);
        }
        other => panic!("builder must emit a drained receipt, got {other:?}"),
    }
    assert_eq!(cumulative, 0.9);
    assert_eq!(
        state.output_peak_bits.load(Ordering::Relaxed),
        0.0f32.to_bits()
    );
    assert_eq!(state.clipped_sample_count.load(Ordering::Relaxed), 0);
}

#[test]
fn stub_natural_drain_carries_cumulative_peak() {
    let mut fixture = Fixture::new(&[]);
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 1 })
        .unwrap();
    // FIFO barrier: this reply proves the Resume was processed.
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    fixture.send(frame(0.5, 4));
    fixture.send(frame(0.25, 2));
    fixture.expect_recycled(0.5, 8);
    fixture.expect_recycled(0.25, 4);
    // Rendered outputs prove what the tap observed (derivation chain
    // for the receipt peak below).
    assert_eq!(fixture.callback(8), vec![0.5; 8]);
    assert_eq!(fixture.callback(4), vec![0.25; 4]);
    fixture.send(ProcessingMessage::EndOfStream);
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::PlaybackDrained {
            epoch: 1,
            epoch_peak_max,
            flush_gen: 0,
        } if epoch_peak_max.to_bits() == 0.5f32.to_bits()
    ));
    assert!(fixture.events.try_recv().is_err());
    fixture.finish();
}

#[test]
fn stub_drain_timeout_carries_cumulative_partial() {
    let mut fixture = Fixture::new(&[]);
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 1 })
        .unwrap();
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    fixture.send(frame(0.5, 4));
    fixture.expect_recycled(0.5, 8);
    // Render half: 0.5 accumulates, 4 samples hold the ring non-empty
    // so the natural site cannot fire — only the 2 s timeout can.
    assert_eq!(fixture.callback(4), vec![0.5; 4]);
    fixture.send(ProcessingMessage::EndOfStream);
    // Generous budget for the 2 s production timeout (fail-loud on a
    // wedged drain, never flaky: the receipt arrives exactly once).
    assert!(matches!(
        fixture
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        ThreadEvent::PlaybackDrained {
            epoch: 1,
            epoch_peak_max,
            flush_gen: 0,
        } if epoch_peak_max.to_bits() == 0.5f32.to_bits()
    ));
    assert!(fixture.events.try_recv().is_err());
    fixture.finish();
}

#[test]
fn stub_disconnect_drain_carries_cumulative_peak() {
    let mut fixture = Fixture::new(&[]);
    fixture
        .worker
        .send_command(PlaybackCommand::Resume { epoch: 1 })
        .unwrap();
    assert_eq!(fixture.worker.reconfigure(48_000, 2).unwrap(), FORMAT);
    fixture.send(frame(0.5, 4));
    fixture.expect_recycled(0.5, 8);
    // mpsc FIFO + drain-then-disconnect semantics: the feeder provably
    // arms EOS before it can observe the disconnect, and the full ring
    // holds the natural site closed — so only the disconnect site can
    // answer once the test drains the ring below. No race.
    fixture.send(ProcessingMessage::EndOfStream);
    fixture.messages.take();
    assert_eq!(fixture.callback(8), vec![0.5; 8]);
    assert!(matches!(
        fixture.events.recv_timeout(WAIT).unwrap(),
        ThreadEvent::PlaybackDrained {
            epoch: 1,
            epoch_peak_max,
            flush_gen: 0,
        } if epoch_peak_max.to_bits() == 0.5f32.to_bits()
    ));
    assert!(fixture.events.try_recv().is_err());
    fixture.finish();
}

#[test]
fn stub_stop_ack_reports_pre_clamp_overshoot() {
    // P2-shaped falsifier pair: emitted output clamps at ±1.0 while
    // the Stop ack reports the pre-clamp overshoot (1.75). A post-clamp
    // tap would hide the overshoot behind a false 1.0 ceiling.
    let mut fixture = Fixture::with_capacity(&[], 64);
    fixture.send(frame(1.5, 4));
    fixture.send(frame(0.25, 4));
    fixture.send(frame(-1.75, 1));
    fixture.expect_recycled(1.5, 8);
    fixture.expect_recycled(0.25, 8);
    fixture.expect_recycled(-1.75, 2);
    assert_eq!(fixture.callback(8), vec![1.0; 8]);
    assert_eq!(fixture.callback(8), vec![0.25; 8]);
    assert_eq!(fixture.callback(2), vec![-1.0; 2]);
    // Clip count: 8 over-1.0 positives + 2 over-1.0 negatives; the
    // 0.25 block is clean. Read before Stop (finalize swaps to zero).
    assert_eq!(
        fixture
            .context
            .state
            .clipped_sample_count
            .load(Ordering::Relaxed),
        10
    );
    assert_eq!(
        fixture
            .context
            .state
            .output_peak_bits
            .load(Ordering::Relaxed),
        1.75f32.to_bits()
    );
    let (request, ack_rx) = PlaybackStopRequest::new();
    fixture
        .worker
        .send_command(PlaybackCommand::Stop(request))
        .unwrap();
    // No flush_requested wait: with the ring already empty the feeder
    // can self-clear before any poll observes it. Drive one callback
    // (flush-clear or plain underrun — both emit silence and meter
    // only zeros, which cannot move the latched 1.75); the ack receive
    // below is the barrier proving Stop processed and finalized.
    assert_eq!(fixture.callback(2), vec![0.0; 2]);
    let ack = ack_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(ack.epoch, 0);
    assert_eq!(ack.epoch_peak_max, 1.75);
    // The terminal swap cleared both atomics.
    assert_eq!(
        fixture
            .context
            .state
            .output_peak_bits
            .load(Ordering::Relaxed),
        0.0f32.to_bits()
    );
    assert_eq!(
        fixture
            .context
            .state
            .clipped_sample_count
            .load(Ordering::Relaxed),
        0
    );
    fixture.finish();
}

#[test]
fn stub_sharing_peak_constructor_aliases_passed_atomic() {
    // P4.6 mechanism (Linux-verifiable): the constructor shares the
    // wrapper's atomic (no copy) — accumulation through the state is
    // visible through the passed Arc and vice versa.
    let shared = Arc::new(AtomicU32::new(0.0f32.to_bits()));
    let state = PlaybackState::new_sharing_peak(8, Arc::clone(&shared));
    assert!(Arc::ptr_eq(&state.output_peak_bits, &shared));
    state
        .output_peak_bits
        .fetch_max(0.5f32.to_bits(), Ordering::Relaxed);
    assert_eq!(f32::from_bits(shared.load(Ordering::Relaxed)), 0.5);
    assert_eq!(state.clipped_sample_count.load(Ordering::Relaxed), 0);
}

#[test]
fn stub_wrapper_retained_probe_returns_shared_atomic() {
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let retained = Arc::new(AtomicU32::new(0.25f32.to_bits()));
    let worker = PlaybackThread {
        command_tx,
        thread_handle: None,
        pending_stop_acks: PendingStopAcks::default(),
        exit_status: crate::engine::worker_death::WorkerExitStatus::new(),
        retained_output_peak_bits: Arc::clone(&retained),
    };
    assert!(Arc::ptr_eq(&worker.retained_output_peak_bits(), &retained));
}
