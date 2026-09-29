//! Exercise the exact desktop state transitions without constructing CPAL.

use super::{
    DrainState, FlushMode, FrameWriteOutcome, PlaybackState, flush_completed, request_flush,
    write_frame_to_ring,
};
use crate::engine::AudioFrame;
use rtrb::RingBuffer;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn fresh_drain() -> DrainState {
    DrainState {
        end_of_stream: false,
        drain_start: None,
        drain_timeout: Duration::from_secs(2),
        flush_mode: FlushMode::Normal,
        stream_flush_pending: false,
        paused: false,
    }
}

fn stop_resume_before_stream_flush(callback_finishes_first: bool, pause_between: bool) {
    let mut drain = fresh_drain();
    let state = PlaybackState::new(8);
    let (mut producer, mut consumer) = RingBuffer::new(8);
    let (recycle_tx, _recycle_rx) = std::sync::mpsc::sync_channel(1);
    let mut conversion = Vec::with_capacity(8);

    // Production Stop requests callback flush and waits for a distinct FIFO
    // stream Flush. Resume arrives on the control queue before that marker.
    request_flush(&state);
    drain.begin_drop(FlushMode::DroppingUntilFlush);
    if pause_between {
        drain.begin_drop(FlushMode::DroppingUntilResume);
    }
    state
        .output_callback_active
        .store(!callback_finishes_first, Ordering::Release);
    drain.resume(flush_completed(&state, &producer, 8));
    if !callback_finishes_first {
        state.output_callback_active.store(false, Ordering::Release);
        assert!(flush_completed(&state, &producer, 8));
        // Same completion method used by runtime wait_for_flush_drain().
        if matches!(drain.flush_mode, FlushMode::WaitingForDrain) {
            drain.callback_flushed();
        }
    }

    // This frame precedes the stream Flush in the processing FIFO. The real
    // frame writer proves whether it would reach callback-visible storage.
    if !drain.drops_frames() {
        assert!(matches!(
            write_frame_to_ring(
                &mut producer,
                &recycle_tx,
                &mut conversion,
                1,
                AudioFrame::new(vec![0.5], 1, 1, 48_000),
            ),
            FrameWriteOutcome::Written { samples: 1 }
        ));
    }
    assert!(
        consumer.pop().is_err(),
        "pre-stop sample reached the output ring before stream Flush"
    );

    // Receiving the actual stream Flush clears only that boundary. Callback
    // completion is still required before the new stream may reach hardware.
    request_flush(&state);
    state.output_callback_active.store(true, Ordering::Release);
    drain.stream_flushed(flush_completed(&state, &producer, 8));
    assert_eq!(drain.flush_mode, FlushMode::WaitingForDrain);
    assert!(consumer.pop().is_err());
    state.output_callback_active.store(false, Ordering::Release);
    assert!(flush_completed(&state, &producer, 8));
    drain.callback_flushed();
    assert_eq!(drain.flush_mode, FlushMode::Normal);
    assert!(matches!(
        write_frame_to_ring(
            &mut producer,
            &recycle_tx,
            &mut conversion,
            1,
            AudioFrame::new(vec![0.75], 1, 1, 48_000),
        ),
        FrameWriteOutcome::Written { samples: 1 }
    ));
    assert_eq!(consumer.pop().unwrap(), 0.75);
}

#[test]
fn stop_then_resume_before_stream_flush_with_completed_callback_flush() {
    stop_resume_before_stream_flush(true, false);
}

#[test]
fn stop_then_resume_before_stream_flush_with_callback_still_active() {
    stop_resume_before_stream_flush(false, false);
}

#[test]
fn stop_pause_resume_preserves_the_stream_boundary() {
    stop_resume_before_stream_flush(true, true);
    stop_resume_before_stream_flush(false, true);
}

#[test]
fn pause_resume_waits_for_callback_without_requiring_stream_flush() {
    let mut drain = fresh_drain();
    drain.begin_drop(FlushMode::DroppingUntilResume);
    assert!(drain.drops_frames());
    drain.resume(false);
    assert_eq!(drain.flush_mode, FlushMode::WaitingForDrain);
    drain.callback_flushed();
    assert_eq!(drain.flush_mode, FlushMode::Normal);
    drain.begin_drop(FlushMode::DroppingUntilResume);
    drain.resume(true);
    assert_eq!(drain.flush_mode, FlushMode::Normal);
}

#[test]
fn stream_flush_does_not_implicitly_resume_paused_playback() {
    let mut drain = fresh_drain();
    drain.begin_drop(FlushMode::DroppingUntilFlush);
    drain.begin_drop(FlushMode::DroppingUntilResume);
    drain.stream_flushed(true);
    assert_eq!(drain.flush_mode, FlushMode::DroppingUntilResume);
    drain.callback_flushed();
    assert!(drain.drops_frames());
    drain.resume(true);
    assert_eq!(drain.flush_mode, FlushMode::Normal);
}

#[test]
fn callback_or_device_recovery_completion_preserves_transport_holds() {
    let mut drain = fresh_drain();
    drain.begin_drop(FlushMode::DroppingUntilFlush);
    drain.callback_flushed();
    assert_eq!(drain.flush_mode, FlushMode::DroppingUntilFlush);
    drain.begin_drop(FlushMode::DroppingUntilResume);
    drain.callback_flushed();
    assert!(drain.drops_frames());
    drain.resume(true);
    assert_eq!(drain.flush_mode, FlushMode::DroppingUntilFlush);
}
