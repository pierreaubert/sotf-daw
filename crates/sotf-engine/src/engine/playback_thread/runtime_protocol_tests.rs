//! Exercise the exact desktop state transitions without constructing CPAL.

use super::{
    DrainState, FlushMode, FrameWriteOutcome, PlaybackState, flush_completed, request_flush,
    snapshot_output_meter, write_frame_to_ring,
};
use crate::engine::{AudioFrame, ThreadEvent};
use rtrb::RingBuffer;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn fresh_drain() -> DrainState {
    DrainState {
        end_of_stream: false,
        drain_start: None,
        drain_timeout: Duration::from_secs(2),
        flush_mode: FlushMode::Normal,
        stream_flush_pending: false,
        paused: false,
        meter_epoch: 0,
        epoch_peak_max: 0.0,
        flushes_processed: 0,
        pending_stop_ack: None,
    }
}

/// Mirror of the production per-window order: swap the snapshot, fold its
/// peak into the epoch cumulative, then return the event for sending.
fn snapshot_and_fold(meter: &PlaybackState, drain: &mut DrainState) -> ThreadEvent {
    let event = snapshot_output_meter(meter, drain.meter_epoch);
    let peak = match &event {
        ThreadEvent::PlaybackOutputMeter { peak_linear, .. } => *peak_linear,
        other => panic!("expected meter snapshot, got {other:?}"),
    };
    drain.note_meter_snapshot(peak);
    event
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

#[test]
fn adopt_playback_epoch_same_epoch_preserves_residual_and_flags() {
    let mut drain = fresh_drain();
    drain.meter_epoch = 3;
    drain.end_of_stream = true;
    drain.epoch_peak_max = 0.80;
    let state = PlaybackState::new(8);
    // Same atomic write the callback performs (apply_volume path).
    state
        .output_peak_bits
        .fetch_max(0.75f32.to_bits(), Ordering::Relaxed);
    drain.adopt_playback_epoch(&state, 3, false);
    assert_eq!(drain.meter_epoch, 3);
    assert!(drain.end_of_stream);
    assert_eq!(drain.epoch_peak_max, 0.80);
    assert_eq!(
        f32::from_bits(state.output_peak_bits.load(Ordering::Relaxed)),
        0.75
    );
}

#[test]
fn adopt_playback_epoch_new_epoch_without_finalize_resets_residual() {
    let mut drain = fresh_drain();
    drain.meter_epoch = 3;
    drain.end_of_stream = true;
    drain.drain_start = Some(Instant::now());
    drain.epoch_peak_max = 0.90;
    let state = PlaybackState::new(8);
    state
        .output_peak_bits
        .fetch_max(0.90f32.to_bits(), Ordering::Relaxed);
    state.clipped_sample_count.store(7, Ordering::Relaxed);
    // No terminal swap ran (nothing pending): the fence still resets.
    drain.adopt_playback_epoch(&state, 4, false);
    assert_eq!(drain.meter_epoch, 4);
    assert!(!drain.end_of_stream);
    assert!(drain.drain_start.is_none());
    assert_eq!(drain.epoch_peak_max, 0.0);
    assert_eq!(
        f32::from_bits(state.output_peak_bits.load(Ordering::Relaxed)),
        0.0
    );
    assert_eq!(state.clipped_sample_count.load(Ordering::Relaxed), 0);
}

#[test]
fn snapshot_output_meter_carries_tagged_peak_and_clears_residual() {
    let state = PlaybackState::new(8);
    state
        .output_peak_bits
        .fetch_max(0.71f32.to_bits(), Ordering::Relaxed);
    let event = snapshot_output_meter(&state, 9);
    match event {
        ThreadEvent::PlaybackOutputMeter {
            peak_linear,
            clipping_detected,
            epoch,
        } => {
            assert_eq!(peak_linear, 0.71);
            assert!(!clipping_detected);
            assert_eq!(epoch, 9);
        }
        other => panic!("expected meter snapshot, got {other:?}"),
    }
    // The swap cleared the residual: the next snapshot is silent.
    let event = snapshot_output_meter(&state, 9);
    match event {
        ThreadEvent::PlaybackOutputMeter { peak_linear, .. } => {
            assert_eq!(peak_linear, 0.0);
        }
        other => panic!("expected meter snapshot, got {other:?}"),
    }
}

#[test]
fn snapshot_output_meter_reports_pre_clamp_peak_and_clipping() {
    let state = PlaybackState::new(8);
    state
        .output_peak_bits
        .fetch_max(1.5f32.to_bits(), Ordering::Relaxed);
    state.clipped_sample_count.store(3, Ordering::Relaxed);
    let event = snapshot_output_meter(&state, 9);
    match event {
        ThreadEvent::PlaybackOutputMeter {
            peak_linear,
            clipping_detected,
            epoch,
        } => {
            assert_eq!(peak_linear, 1.5);
            assert!(clipping_detected);
            assert_eq!(epoch, 9);
        }
        other => panic!("expected meter snapshot, got {other:?}"),
    }
    assert_eq!(state.clipped_sample_count.load(Ordering::Relaxed), 0);
}

#[test]
fn epoch_cumulative_folds_max_and_never_decreases() {
    let mut drain = fresh_drain();
    assert_eq!(drain.epoch_peak_max, 0.0);
    drain.note_meter_snapshot(0.30);
    assert_eq!(drain.epoch_peak_max, 0.30);
    drain.note_meter_snapshot(0.90);
    assert_eq!(drain.epoch_peak_max, 0.90);
    drain.note_meter_snapshot(0.50);
    assert_eq!(drain.epoch_peak_max, 0.90);
}

#[test]
fn drained_event_carries_epoch_and_cumulative_peak() {
    let mut drain = fresh_drain();
    drain.meter_epoch = 7;
    drain.note_meter_snapshot(0.40);
    drain.note_meter_snapshot(0.85);
    match drain.drained_event() {
        ThreadEvent::PlaybackDrained {
            epoch,
            epoch_peak_max,
            flush_gen,
        } => {
            assert_eq!(epoch, 7);
            assert_eq!(epoch_peak_max, 0.85);
            assert_eq!(flush_gen, 0);
        }
        other => panic!("expected drained receipt, got {other:?}"),
    }
}

#[test]
fn dropped_periodic_report_survives_in_terminal_record() {
    let meter = PlaybackState::new(8);
    let mut drain = fresh_drain();
    drain.meter_epoch = 5;
    let (event_tx, event_rx) = crossbeam::channel::bounded(1);
    // Occupy the only slot so the first snapshot send deterministically drops.
    event_tx
        .try_send(ThreadEvent::PlaybackUnderrun(0))
        .expect("setup send must succeed");

    // Loudest window first: folded, then dropped on the full channel.
    meter
        .output_peak_bits
        .fetch_max(0.90f32.to_bits(), Ordering::Relaxed);
    let event = snapshot_and_fold(&meter, &mut drain);
    assert!(
        event_tx.try_send(event).is_err(),
        "full channel must drop the loudest window's report"
    );

    // Quieter windows send normally.
    event_rx.recv().expect("drain the setup event");
    meter
        .output_peak_bits
        .fetch_max(0.30f32.to_bits(), Ordering::Relaxed);
    let event = snapshot_and_fold(&meter, &mut drain);
    event_tx.try_send(event).expect("open channel must deliver");

    // Terminal window, then the receipt carries the dropped loudest window.
    event_rx.recv().expect("drain the quiet report");
    meter
        .output_peak_bits
        .fetch_max(0.50f32.to_bits(), Ordering::Relaxed);
    let event = snapshot_and_fold(&meter, &mut drain);
    event_tx.try_send(event).expect("terminal report sends");
    match drain.drained_event() {
        ThreadEvent::PlaybackDrained {
            epoch,
            epoch_peak_max,
            flush_gen,
        } => {
            assert_eq!(epoch, 5);
            assert_eq!(epoch_peak_max, 0.90);
            assert_eq!(flush_gen, 0);
        }
        other => panic!("expected drained receipt, got {other:?}"),
    }
}

#[test]
fn stream_flushed_advances_consumed_generation_exactly_once() {
    let mut drain = fresh_drain();
    assert_eq!(drain.flushes_processed, 0);
    // Drop, resume, and callback-completion paths never advance the
    // count: only a consumed Flush does.
    drain.begin_drop(FlushMode::DroppingUntilFlush);
    drain.resume(true);
    drain.callback_flushed();
    assert_eq!(drain.flushes_processed, 0);
    drain.stream_flushed(true);
    assert_eq!(drain.flushes_processed, 1);
    drain.stream_flushed(false);
    assert_eq!(drain.flushes_processed, 2);
    match drain.drained_event() {
        ThreadEvent::PlaybackDrained { flush_gen, .. } => {
            assert_eq!(flush_gen, 2);
        }
        other => panic!("expected drained receipt, got {other:?}"),
    }
}

#[test]
fn swallowed_flushes_count_without_clearing_the_drop_boundary() {
    let mut drain = fresh_drain();
    drain.begin_drop(FlushMode::DroppingUntilFlush);
    drain.note_swallowed_flush();
    drain.note_swallowed_flush();
    assert_eq!(drain.flushes_processed, 2);
    // Drop-until-Flush is kept: only a real Flush re-arms flow.
    assert!(drain.drops_frames());
    drain.stream_flushed(true);
    assert_eq!(drain.flushes_processed, 3);
    assert!(!drain.drops_frames());
}

#[test]
fn swallowed_flush_invalidates_armed_drain_but_counts() {
    // D7: a swallowed boundary must not let a stale armed EOS fire
    // with a balanced generation (instant false drain under the new
    // stream). Counting still advances: the manager counted the send.
    let mut drain = fresh_drain();
    drain.end_of_stream = true;
    drain.drain_start = Some(Instant::now());
    drain.note_swallowed_flush();
    assert!(!drain.end_of_stream);
    assert!(drain.drain_start.is_none());
    assert_eq!(drain.flushes_processed, 1);
}

#[test]
fn swallowed_eos_arms_drain_so_finished_stream_completes() {
    // A3: a swallowed terminal marker must not vanish into
    // stuck-Playing. EOS is generated only at source exhaustion
    // (abandoning arms emit Flush, never EOS), so any swallowed
    // marker belongs to a finished stream; a following Flush (same
    // guarantee) invalidates the arm if the stream was abandoned.
    let mut drain = fresh_drain();
    drain.note_swallowed_eos();
    assert!(drain.end_of_stream);
    assert!(drain.drain_start.is_some());
    // Arming is pure marker state: no generation consumed.
    assert_eq!(drain.flushes_processed, 0);
}

#[test]
fn swallowed_batch_eos_then_flush_nets_cleared() {
    // Stale natural-EOF marker followed by the seek boundary in one
    // batch: the boundary invalidates the stale terminal, matching
    // normal order (EOS arms, later Flush clears).
    let mut drain = fresh_drain();
    drain.note_swallowed_eos();
    drain.note_swallowed_flush();
    assert!(!drain.end_of_stream);
    assert!(drain.drain_start.is_none());
    assert_eq!(drain.flushes_processed, 1);
}

#[test]
fn swallowed_batch_flush_then_eos_nets_armed() {
    // Boundary first, marker second: queue order matches stream
    // order, so the marker belongs to the post-boundary stream and
    // its terminal is legitimate.
    let mut drain = fresh_drain();
    drain.note_swallowed_flush();
    drain.note_swallowed_eos();
    assert!(drain.end_of_stream);
    assert!(drain.drain_start.is_some());
    assert_eq!(drain.flushes_processed, 1);
}

#[test]
fn stop_terminal_reports_then_clears_before_fence_supersede() {
    let meter = PlaybackState::new(8);
    let mut drain = fresh_drain();
    drain.meter_epoch = 5;
    drain.note_meter_snapshot(0.40);
    meter
        .output_peak_bits
        .fetch_max(0.70f32.to_bits(), Ordering::Relaxed);
    // Supersede order: finalize the pending record BEFORE the fence.
    let ack = drain.take_stop_terminal(&meter);
    assert_eq!(ack.epoch, 5);
    assert_eq!(ack.epoch_peak_max, 0.70);
    assert_eq!(drain.epoch_peak_max, 0.70);
    assert_eq!(
        meter.output_peak_bits.load(Ordering::Relaxed),
        0.0f32.to_bits()
    );
    // The swap ran, so the fence skips the residual reset (D1); the
    // swapped residual is already empty here, so nothing changes.
    drain.adopt_playback_epoch(&meter, 6, true);
    assert_eq!(drain.meter_epoch, 6);
    assert_eq!(drain.epoch_peak_max, 0.0);
    // The answered record survives the fence that superseded it.
    assert_eq!(ack.epoch_peak_max, 0.70);
}

#[test]
fn adopt_after_finalize_keeps_racing_window_in_new_record() {
    // D1: a callback fetch_max landing between the terminal swap and
    // the epoch fence must survive — in the new record (conservative
    // misattribution: ceiling-safe superset of whatever lands there),
    // never wiped into neither record. Hand-driven atomics reproduce
    // the exact interleaving deterministically; no threads, no timing.
    let meter = PlaybackState::new(8);
    let mut drain = fresh_drain();
    drain.meter_epoch = 5;
    meter
        .output_peak_bits
        .fetch_max(0.70f32.to_bits(), Ordering::Relaxed);
    // Supersede order: finalize the pending record BEFORE the fence.
    let ack = drain.take_stop_terminal(&meter);
    assert_eq!(ack.epoch, 5);
    assert_eq!(ack.epoch_peak_max, 0.70);
    // The racing callback window: observed after the swap, fenced after.
    meter
        .output_peak_bits
        .fetch_max(0.42f32.to_bits(), Ordering::Relaxed);
    drain.adopt_playback_epoch(&meter, 6, true);
    assert_eq!(drain.meter_epoch, 6);
    // The cumulative still resets: only the unclaimed window transfers,
    // never the old epoch's whole record.
    assert_eq!(drain.epoch_peak_max, 0.0);
    // The racing window survived the fence in the fresh residual...
    assert_eq!(
        meter.output_peak_bits.load(Ordering::Relaxed),
        0.42f32.to_bits()
    );
    // ...and the new epoch's next snapshot folds it (misattributed, present).
    let event = snapshot_and_fold(&meter, &mut drain);
    assert!(matches!(
        event,
        ThreadEvent::PlaybackOutputMeter { epoch: 6, .. }
    ));
    assert_eq!(drain.epoch_peak_max, 0.42);
}
