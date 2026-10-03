//! RemoteIO feeder protocol, independently testable without an AudioUnit.

use super::super::{PlaybackCommand, PlaybackStopAck, ProcessingMessage, ThreadEvent};
use super::misc::{SPIN_MS_RINGBUFFER, write_chunk_bulk};
use super::playback_state::PlaybackState;
use rtrb::Producer;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender};

#[expect(
    clippy::too_many_arguments,
    reason = "Keeps native worker dependencies explicit for portable protocol tests"
)]
pub(super) fn run_feeder(
    message_rx: Receiver<ProcessingMessage>,
    command_rx: Receiver<PlaybackCommand>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    sample_rate: u32,
    channels: usize,
    mut producer: Producer<f32>,
    state: Arc<PlaybackState>,
    recycle_tx: SyncSender<Vec<f32>>,
) -> Result<(), String> {
    if sample_rate == 0 || channels == 0 || state.capacity < channels {
        return Err("iOS feeder requires a valid format and space for one audio frame".into());
    }
    let buffer_capacity = state.capacity;
    // End-of-stream drain tracking
    let mut end_of_stream = false;
    let mut drain_start: Option<std::time::Instant> = None;
    let drain_timeout = std::time::Duration::from_secs(2);
    let mut flush_dropping = false;
    let mut pause_dropping = false;
    let mut resume_waiting_for_flush = false;
    let mut meter_epoch: u64 = 0;
    let mut flushes_processed: u64 = 0;
    let mut pending_stop_ack: Option<SyncSender<PlaybackStopAck>> = None;
    // Cumulative epoch peak (desktop `DrainState::epoch_peak_max`
    // mirror): every residual swap folds here; drained receipts and
    // Stop terminals report it.
    let mut epoch_peak_max = 0.0f32;
    // Keep ownership and a sample cursor across bounded ring writes. The cursor
    // advances only by whole interleaved frames, preserving channel alignment.
    let mut pending_frame: Option<(crate::AudioFrame, usize)> = None;
    // Main loop: read from processing queue and write to ring buffer
    loop {
        // Check for commands
        if let Ok(command) = command_rx.try_recv() {
            match command {
                PlaybackCommand::SetVolume(vol) => {
                    state.volume.store(vol.to_bits(), Ordering::Relaxed);
                }
                PlaybackCommand::Mute(muted) => {
                    state.muted.store(muted, Ordering::Relaxed);
                }
                PlaybackCommand::Pause => {
                    state.flush_requested.store(true, Ordering::Release);
                    pause_dropping = true;
                    end_of_stream = false;
                    drain_start = None;
                    if let Some((frame, _)) = pending_frame.take() {
                        recycle_tx.try_send(frame.data).ok();
                    }
                }
                PlaybackCommand::Resume { epoch } => {
                    // Every Resume re-arms callback emission (same-epoch
                    // pause/resume and Stop-then-Resume included): the
                    // cutoff holds until the operator resumes flow.
                    state.stop_latched.store(false, Ordering::Release);
                    let finalized = if epoch != meter_epoch {
                        // A new-epoch fence supersedes a pending Stop:
                        // finalize its record before adoption discards
                        // it. The fence skips its meter reset exactly
                        // when this swap ran (a racing callback window
                        // lands in the new record instead of being
                        // wiped into neither record).
                        finalize_pending_stop_ack(
                            &mut pending_stop_ack,
                            &state,
                            &mut epoch_peak_max,
                            meter_epoch,
                        )
                    } else {
                        false
                    };
                    adopt_stub_epoch(
                        &state,
                        &mut meter_epoch,
                        &mut epoch_peak_max,
                        &mut end_of_stream,
                        &mut drain_start,
                        epoch,
                        finalized,
                    );
                    resume_waiting_for_flush = true;
                }
                PlaybackCommand::UpdateSampleRate(new_rate) => {
                    if new_rate != sample_rate {
                        log::warn!(
                            "[Playback Thread iOS] Sample rate change {}→{} not supported at runtime on iOS",
                            sample_rate,
                            new_rate
                        );
                    }
                }
                PlaybackCommand::UpdateChannels(new_ch) => {
                    if new_ch != channels {
                        log::warn!(
                            "[Playback Thread iOS] Channel count change {}→{} not supported at runtime on iOS",
                            channels,
                            new_ch
                        );
                    }
                }
                PlaybackCommand::Reconfigure(request) => {
                    if !request.ticket.try_begin_execution() {
                        request
                            .reply_tx
                            .send(Err(
                                "iOS playback reconfiguration was cancelled before execution"
                                    .to_string(),
                            ))
                            .ok();
                    } else if !request.ticket.try_complete_execution() {
                        request
                            .reply_tx
                            .send(Err(
                                "iOS playback reconfiguration was cancelled before completion"
                                    .to_string(),
                            ))
                            .ok();
                    } else if request.requested.sample_rate == sample_rate
                        && request.requested.channels == channels
                    {
                        request
                            .reply_tx
                            .send(Ok(super::super::PlaybackConfiguration {
                                sample_rate,
                                channels,
                            }))
                            .ok();
                    } else {
                        request
                            .reply_tx
                            .send(Err(format!(
                                "iOS RemoteIO runtime reconfiguration from {}Hz/{}ch to {}Hz/{}ch is unsupported; rebuild the engine",
                                sample_rate,
                                channels,
                                request.requested.sample_rate,
                                request.requested.channels,
                            )))
                            .ok();
                    }
                }
                PlaybackCommand::Stop(request) => {
                    // Supersede: finalize any still-pending acknowledgment
                    // before arming the new one (report-then-clear: the
                    // old record answers instead of being lost).
                    finalize_pending_stop_ack(
                        &mut pending_stop_ack,
                        &state,
                        &mut epoch_peak_max,
                        meter_epoch,
                    );
                    // Latch the emission cutoff, then drop until Flush.
                    state.stop_latched.store(true, Ordering::Release);
                    state.flush_requested.store(true, Ordering::Release);
                    flush_dropping = true;
                    end_of_stream = false;
                    drain_start = None;
                    if let Some((frame, _)) = pending_frame.take() {
                        recycle_tx.try_send(frame.data).ok();
                    }
                    pending_stop_ack = Some(request.reply_tx);
                }
                PlaybackCommand::Shutdown => {
                    log::debug!("[Playback Thread iOS] Shutting down");
                    finalize_pending_stop_ack(
                        &mut pending_stop_ack,
                        &state,
                        &mut epoch_peak_max,
                        meter_epoch,
                    );
                    if let Some((frame, _)) = pending_frame.take() {
                        recycle_tx.try_send(frame.data).ok();
                    }
                    break;
                }
            }
        }

        let callback_flushed = flush_completed(&state, &producer);
        if resume_waiting_for_flush && callback_flushed {
            resume_waiting_for_flush = false;
            pause_dropping = false;
        }
        // Stop completion never stalls the loop: once quiesce (ring
        // empty plus callback inactive) is observed after the arm, the
        // terminal record (cumulative peak through the terminal swap)
        // answers exactly once.
        if pending_stop_ack.is_some() && callback_flushed {
            finalize_pending_stop_ack(
                &mut pending_stop_ack,
                &state,
                &mut epoch_peak_max,
                meter_epoch,
            );
        }

        // Read from message queue
        let (message, mut sample_offset) = if let Some((frame, offset)) = pending_frame.take() {
            (Ok(ProcessingMessage::Frame(frame)), offset)
        } else {
            (message_rx.try_recv(), 0)
        };
        match message {
            Ok(ProcessingMessage::Frame(frame)) => {
                if flush_dropping || pause_dropping {
                    recycle_tx.try_send(frame.data).ok();
                    continue;
                }

                if frame.sample_rate != sample_rate
                    || frame.num_channels != channels
                    || frame.num_frames.checked_mul(channels) != Some(frame.data.len())
                {
                    event_tx
                        .try_send(ThreadEvent::ProcessingError(format!(
                            "iOS playback requires valid {sample_rate}Hz/{channels}ch frames, received {}Hz/{}ch with {} samples for {} frames",
                            frame.sample_rate, frame.num_channels, frame.data.len(), frame.num_frames
                        )))
                        .ok();
                    recycle_tx.try_send(frame.data).ok();
                    continue;
                }

                // A FIFO Flush starts an asynchronous callback drain. Retain
                // following fresh audio until that drain can no longer erase it.
                if !callback_flushed {
                    pending_frame = Some((frame, sample_offset));
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }

                // Publish at most the available whole frames per iteration so
                // even a block larger than the ring makes progress. Reenter the
                // command loop before retrying a retained suffix.
                let remaining = frame.data.len() - sample_offset;
                if remaining == 0 {
                    recycle_tx.try_send(frame.data).ok();
                    continue;
                }
                let write_samples = (producer.slots() / channels * channels).min(remaining);
                if write_samples == 0 {
                    pending_frame = Some((frame, sample_offset));
                    std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_RINGBUFFER));
                    continue;
                }
                match producer.write_chunk_uninit(write_samples) {
                    Ok(chunk) => {
                        write_chunk_bulk(
                            chunk,
                            &frame.data[sample_offset..sample_offset + write_samples],
                        );
                        sample_offset += write_samples;
                    }
                    Err(_) => {
                        pending_frame = Some((frame, sample_offset));
                        std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_RINGBUFFER));
                        continue;
                    }
                }
                if sample_offset < frame.data.len() {
                    pending_frame = Some((frame, sample_offset));
                    continue;
                }
                recycle_tx.try_send(frame.data).ok();
            }
            Ok(ProcessingMessage::EndOfStream) => {
                if flush_dropping || pause_dropping {
                    continue;
                }
                log::debug!("[Playback Thread iOS] End of stream - starting drain");
                end_of_stream = true;
                drain_start = Some(std::time::Instant::now());
            }
            Ok(ProcessingMessage::Flush) => {
                // A boundary re-arms the callback unconditionally. No
                // pre-cutoff frame can arrive after this point: the
                // decoder queues its post-Stop Flush after its last
                // frame, processing retries-or-recycles in order and
                // resets DSP state at the Flush (no tails ring out),
                // so any frame queued behind the Flush postdates the
                // cutoff — correct to emit even when a superseded
                // completion is still outstanding. New-epoch
                // supersedes are stale-dropped by the epoch gate; a
                // same-epoch supersede folds the post-cutoff windows
                // into the same-epoch terminal instead (ceiling-safe
                // superset, unreachable in current manager flows,
                // which serialize Stop before later commands). The
                // same arm sets the flush flag, which discards the
                // stale ring content before any emission. The latch is
                // defense-in-depth (worker drop-mode does the cutting
                // off); its job is the no-boundary case (Stop with no
                // subsequent Flush), where it holds until a Resume.
                // Resume, by contrast, always re-arms: operator
                // intent is absolute.
                state.stop_latched.store(false, Ordering::Release);
                state.flush_requested.store(true, Ordering::Release);
                end_of_stream = false;
                drain_start = None;
                flush_dropping = false;
                flushes_processed = flushes_processed.wrapping_add(1);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if end_of_stream {
                    // Check if ring buffer has drained
                    if producer.slots() >= buffer_capacity {
                        log::info!("[Playback Thread iOS] Ring buffer drained");
                        let receipt = snapshot_drained_receipt(
                            &state,
                            &mut epoch_peak_max,
                            meter_epoch,
                            flushes_processed,
                        );
                        event_tx.try_send(receipt).ok();
                        end_of_stream = false;
                        drain_start = None;
                        continue;
                    }
                    if let Some(start) = drain_start
                        && start.elapsed() > drain_timeout
                    {
                        log::warn!("[Playback Thread iOS] Drain timeout, signaling completion");
                        // Partial stream: the receipt carries the
                        // cumulative at timeout (desktop semantics).
                        let receipt = snapshot_drained_receipt(
                            &state,
                            &mut epoch_peak_max,
                            meter_epoch,
                            flushes_processed,
                        );
                        event_tx.try_send(receipt).ok();
                        end_of_stream = false;
                        drain_start = None;
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if end_of_stream {
                    // Reenter the outer loop between waits so shutdown and
                    // control commands remain responsive after disconnection.
                    if producer.slots() >= buffer_capacity {
                        let receipt = snapshot_drained_receipt(
                            &state,
                            &mut epoch_peak_max,
                            meter_epoch,
                            flushes_processed,
                        );
                        event_tx.try_send(receipt).ok();
                    } else if drain_start
                        .get_or_insert_with(std::time::Instant::now)
                        .elapsed()
                        < drain_timeout
                    {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                }
                log::debug!("[Playback Thread iOS] Queue disconnected");
                break;
            }
        }
    }

    log::debug!("[Playback Thread iOS] Stopped");
    Ok(())
}

/// Answer a pending Stop with its terminal record, if any.
///
/// Report-then-clear (desktop `finalize_pending_stop_ack` mirror,
/// minus the snapshot event — the stub sends no periodic meter):
/// the swapped residual folds into the cumulative before the record
/// is built, so the acknowledgment covers every callback-observed
/// sample. Best-effort send — a timed-out manager collects late; a
/// gone manager needs nothing. Returns whether a terminal swap ran,
/// so the epoch fence can skip its meter reset exactly then (a
/// racing callback window lands in the new record instead of being
/// wiped).
fn finalize_pending_stop_ack(
    pending: &mut Option<SyncSender<PlaybackStopAck>>,
    meter: &PlaybackState,
    epoch_peak_max: &mut f32,
    meter_epoch: u64,
) -> bool {
    let Some(reply_tx) = pending.take() else {
        return false;
    };
    swap_fold_stub_residual(meter, epoch_peak_max);
    reply_tx
        .send(PlaybackStopAck {
            epoch: meter_epoch,
            epoch_peak_max: *epoch_peak_max,
        })
        .ok();
    true
}

/// Adopt a playback epoch from a Resume command, fencing the old stream.
///
/// Desktop mirror (`DrainState::adopt_playback_epoch`): on change only,
/// stores the epoch, clears EOS/drain flags, and resets the epoch
/// cumulative peak (a superseded record already carries it — the
/// finalize-first in the Resume arm). The callback residual resets too,
/// UNLESS `finalized` reports a terminal swap just ran: a callback
/// window racing between that swap and this fence is kept and
/// attributed to the new epoch (conservative, ceiling-safe; it folds
/// at the next swap), never wiped into neither record. Abandoned tails
/// (no swap ran) reset by design. Same-epoch resumes are a no-op by
/// comparison.
fn adopt_stub_epoch(
    meter: &PlaybackState,
    meter_epoch: &mut u64,
    epoch_peak_max: &mut f32,
    end_of_stream: &mut bool,
    drain_start: &mut Option<std::time::Instant>,
    epoch: u64,
    finalized: bool,
) {
    if epoch == *meter_epoch {
        return;
    }
    *meter_epoch = epoch;
    if !finalized {
        meter.reset_output_meter();
    }
    *epoch_peak_max = 0.0;
    *end_of_stream = false;
    *drain_start = None;
}

/// Swap the callback residual into the epoch cumulative peak.
///
/// Desktop mirror of the swap half of `snapshot_output_meter` plus
/// `note_meter_snapshot` (the stub builds no periodic event): swaps
/// both meter atomics and max-folds the window peak. Every swap point
/// (three drained sites, Stop terminals, epoch fences) funnels here,
/// so no residual is ever swapped-then-discarded. Swapped clips drop
/// (no live clip channel on the stub — P4.5 gap); the counting rule
/// itself is the parity.
fn swap_fold_stub_residual(meter: &PlaybackState, epoch_peak_max: &mut f32) {
    let window_peak = f32::from_bits(
        meter
            .output_peak_bits
            .swap(0.0f32.to_bits(), Ordering::Relaxed),
    );
    meter.clipped_sample_count.swap(0, Ordering::Relaxed);
    *epoch_peak_max = epoch_peak_max.max(window_peak);
}

/// Swap the residual and build the drained receipt (sole builder).
///
/// Combines the desktop trio (`snapshot_output_meter` swap +
/// `note_meter_snapshot` fold + `drained_event` build): the swapped
/// window folds into the cumulative BEFORE the receipt is built, so
/// the record covers every callback-observed sample. Called pre-send
/// at all three drained sites; the only `PlaybackDrained` literal in
/// production (grep-verifiable).
fn snapshot_drained_receipt(
    meter: &PlaybackState,
    epoch_peak_max: &mut f32,
    meter_epoch: u64,
    flushes_processed: u64,
) -> ThreadEvent {
    swap_fold_stub_residual(meter, epoch_peak_max);
    ThreadEvent::PlaybackDrained {
        epoch: meter_epoch,
        epoch_peak_max: *epoch_peak_max,
        flush_gen: flushes_processed,
    }
}

fn flush_completed(state: &PlaybackState, producer: &Producer<f32>) -> bool {
    if !state.flush_requested.load(Ordering::Acquire) {
        return true;
    }
    if producer.slots() == state.capacity && !state.callback_active.load(Ordering::Acquire) {
        // Empty ring and no old callback in flight: clearing an empty flush
        // is safe even if hardware has stopped requesting buffers.
        state.flush_requested.store(false, Ordering::Release);
        true
    } else {
        false
    }
}

#[cfg(test)]
#[path = "feeder_tests.rs"]
mod tests;
