//! Portable tests of the actual RemoteIO feeder and callback, without hardware calls.

// Rust guideline compliant 2026-02-21
use super::super::misc::core_audio_ffi as ca;
use super::super::playback_state::PlaybackState;
use super::super::playback_thread::PlaybackThread;
use super::super::types::{RenderContext, render_callback};
use super::run_feeder;
use crate::AudioFrame;
use crate::engine::{
    HostUpdateTicket, PlaybackCommand, PlaybackConfiguration, PlaybackReconfigureRequest,
    ProcessingMessage, ThreadEvent,
};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
    fixture.worker.send_command(PlaybackCommand::Stop).unwrap();
    wait_until(|| {
        fixture
            .context
            .state
            .flush_requested
            .load(Ordering::Acquire)
    });
    fixture
        .worker
        .send_command(PlaybackCommand::Resume)
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
                .send_command(PlaybackCommand::Resume)
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
                .send_command(PlaybackCommand::Resume)
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
            ThreadEvent::PlaybackDrained
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
    fixture.worker.send_command(PlaybackCommand::Stop).unwrap();
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
        ThreadEvent::PlaybackDrained
    ));
    fixture.finish();
}
