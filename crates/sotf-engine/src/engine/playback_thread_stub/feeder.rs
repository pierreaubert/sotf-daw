//! RemoteIO feeder protocol, independently testable without an AudioUnit.

use super::super::{PlaybackCommand, ProcessingMessage, ThreadEvent};
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
                PlaybackCommand::Resume => {
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
                PlaybackCommand::Stop => {
                    state.flush_requested.store(true, Ordering::Release);
                    flush_dropping = true;
                    end_of_stream = false;
                    drain_start = None;
                    if let Some((frame, _)) = pending_frame.take() {
                        recycle_tx.try_send(frame.data).ok();
                    }
                }
                PlaybackCommand::Shutdown => {
                    log::debug!("[Playback Thread iOS] Shutting down");
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
                state.flush_requested.store(true, Ordering::Release);
                end_of_stream = false;
                drain_start = None;
                flush_dropping = false;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if end_of_stream {
                    // Check if ring buffer has drained
                    if producer.slots() >= buffer_capacity {
                        log::info!("[Playback Thread iOS] Ring buffer drained");
                        event_tx.try_send(ThreadEvent::PlaybackDrained).ok();
                        end_of_stream = false;
                        drain_start = None;
                        continue;
                    }
                    if let Some(start) = drain_start
                        && start.elapsed() > drain_timeout
                    {
                        log::warn!("[Playback Thread iOS] Drain timeout, signaling completion");
                        event_tx.try_send(ThreadEvent::PlaybackDrained).ok();
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
                        event_tx.try_send(ThreadEvent::PlaybackDrained).ok();
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
