use super::super::{
    DecoderAsyncError, DecoderCommand, DecoderMessage, DecoderResponse, ThreadEvent,
};
use super::consts::SPIN_MS_SLEEP_DECODER;
use super::decoder_state::DecoderState;
use crate::DsdOutputMode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(all(target_os = "macos", feature = "hal")), allow(dead_code))]
pub(super) struct HalInputGuardTrip {
    pub(super) peak: f32,
    pub(super) invalid_samples: usize,
    pub(super) over_limit_samples: usize,
}

/// Action returned by decode loop
pub(super) enum DecoderLoopAction {
    Continue,
    Stop,
    Interrupted(DecoderCommand),
}

/// Main decoder thread function
#[allow(
    clippy::too_many_arguments,
    reason = "thread entry point: one argument per decoder runtime resource"
)]
pub(super) fn run_decoder_thread(
    message_tx: SyncSender<DecoderMessage>,
    command_rx: Receiver<DecoderCommand>,
    response_tx: Sender<DecoderResponse>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    target_sample_rate: u32,
    frame_size: usize,
    recycle_rx: Receiver<Vec<f32>>,
    dsd_output: DsdOutputMode,
    async_error_tx: Sender<DecoderAsyncError>,
    cancelled_attempt_through: Arc<AtomicU64>,
) -> Result<(), String> {
    // The HAL reader feeds every downstream audio stage. Leaving it at the
    // default QoS lets unrelated filesystem/UI work starve it even though the
    // CoreAudio callback itself is realtime, which presents as ring underruns.
    // Use the same soft realtime/QoS class as DSP processing; hard
    // THREAD_TIME_CONSTRAINT_POLICY remains reserved for backend callbacks.
    #[cfg(target_os = "macos")]
    {
        match super::super::rt_priority::set_realtime_priority(
            super::super::rt_priority::RtPriority::Processing,
            None,
        ) {
            Ok(true) => log::info!("[Decoder Thread] Audio-work priority set successfully"),
            Ok(false) => {
                log::debug!("[Decoder Thread] Audio-work priority unavailable on platform")
            }
            Err(error) => {
                log::warn!("[Decoder Thread] Failed to set audio-work priority: {error}")
            }
        }
    }

    let mut state = DecoderState::new(recycle_rx, dsd_output);

    log::info!(
        "[Decoder Thread] Started - target {}Hz, frame size {}",
        target_sample_rate,
        frame_size
    );

    loop {
        // Check for commands (non-blocking when playing/silent, blocking when stopped)
        let is_active = (state.decoder.is_some() && !state.paused) || state.driver_source.active;
        let command = if is_active {
            command_rx.try_recv().ok()
        } else {
            // Bounded wait when stopped/paused.  This keeps shutdown latency low
            // and lets us detect a dropped command sender promptly instead of
            // blocking forever on a channel that will never receive a command.
            match command_rx.recv_timeout(Duration::from_millis(SPIN_MS_SLEEP_DECODER)) {
                Ok(cmd) => Some(cmd),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };

        if let Some(cmd) = command {
            match cmd {
                DecoderCommand::Play(source, attempt) => {
                    if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                        response_tx
                            .send(DecoderResponse::Error("Play request cancelled".into()))
                            .ok();
                        continue;
                    }
                    // Adopt-first: the tag identifies this phase even
                    // when the open below NACKs (tried-and-died still
                    // consumed the attempt — manager stays converged).
                    state.current_attempt = attempt;
                    // Flush-before-ack: see DecoderMessage::Flush invariant.
                    message_tx.send(DecoderMessage::Flush).ok();
                    state.stop();
                    #[cfg(feature = "streaming")]
                    state.clear_stream_metadata(&event_tx);
                    if let Err(e) = state.play(source, target_sample_rate, frame_size) {
                        log::warn!("[Decoder Thread] Play failed: {}", e);
                        event_tx
                            .try_send(
                                ThreadEvent::DecoderError(e)
                                    .with_decode_attempt(state.current_attempt),
                            )
                            .ok();
                        response_tx
                            .send(DecoderResponse::Error(
                                "Failed to start playback".to_string(),
                            ))
                            .ok();
                    } else if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                        state.stop();
                        response_tx
                            .send(DecoderResponse::Error("Play request cancelled".into()))
                            .ok();
                    } else {
                        response_tx.send(DecoderResponse::Ok).ok();
                    }
                }
                DecoderCommand::PlayAt(source, position, attempt) => {
                    if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                        response_tx
                            .send(DecoderResponse::Error("Play request cancelled".into()))
                            .ok();
                        continue;
                    }
                    // Adopt-first: see the Play arm.
                    state.current_attempt = attempt;
                    // Flush-before-ack: see DecoderMessage::Flush invariant.
                    message_tx.send(DecoderMessage::Flush).ok();
                    state.stop();
                    #[cfg(feature = "streaming")]
                    state.clear_stream_metadata(&event_tx);
                    if let Err(e) = state.play(source, target_sample_rate, frame_size) {
                        log::warn!("[Decoder Thread] PlayAt (load) failed: {}", e);
                        event_tx
                            .try_send(
                                ThreadEvent::DecoderError(e)
                                    .with_decode_attempt(state.current_attempt),
                            )
                            .ok();
                        response_tx
                            .send(DecoderResponse::Error(
                                "Failed to start playback".to_string(),
                            ))
                            .ok();
                    } else if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                        state.stop();
                        response_tx
                            .send(DecoderResponse::Error("Play request cancelled".into()))
                            .ok();
                    } else if let Err(e) = state.seek(position) {
                        log::warn!("[Decoder Thread] PlayAt (seek) failed: {}", e);
                        // Revert the wrong-position load: a freshly loaded
                        // decoder is safe to abandon (nothing downstream
                        // depends on it yet), so a failed seek unloads it
                        // instead of emitting from the wrong position under
                        // the old epoch. The failure stays explicit below.
                        state.stop();
                        event_tx
                            .try_send(
                                ThreadEvent::DecoderError(e)
                                    .with_decode_attempt(state.current_attempt),
                            )
                            .ok();
                        response_tx
                            .send(DecoderResponse::Error(
                                "Failed to seek during play_at".to_string(),
                            ))
                            .ok();
                    } else if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                        state.stop();
                        response_tx
                            .send(DecoderResponse::Error("Play request cancelled".into()))
                            .ok();
                    } else {
                        event_tx
                            .try_send(
                                ThreadEvent::SeekComplete
                                    .with_decode_attempt(state.current_attempt),
                            )
                            .ok();
                        response_tx.send(DecoderResponse::Ok).ok();
                    }
                }
                DecoderCommand::StartSilentSource(channels) => {
                    state.start_silent_source(channels);
                }
                DecoderCommand::StartCaptureSource(source, channels) => {
                    state.start_capture_source(source, channels);
                }
                DecoderCommand::Pause => {
                    state.paused = true;
                    log::debug!("[Decoder Thread] Paused");
                    response_tx.send(DecoderResponse::Ok).ok();
                }
                DecoderCommand::Resume => {
                    state.paused = false;
                    log::debug!("[Decoder Thread] Resumed");
                    response_tx.send(DecoderResponse::Ok).ok();
                }
                DecoderCommand::Seek(position) => {
                    message_tx.send(DecoderMessage::Flush).ok();
                    if let Err(e) = state.seek(position) {
                        log::warn!("[Decoder Thread] Seek failed: {}", e);
                        response_tx.send(DecoderResponse::Error(e)).ok();
                    } else {
                        event_tx
                            .try_send(
                                ThreadEvent::SeekComplete
                                    .with_decode_attempt(state.current_attempt),
                            )
                            .ok();
                        response_tx.send(DecoderResponse::Ok).ok();
                    }
                }
                DecoderCommand::QueueNext(source) => {
                    log::debug!("[Decoder Thread] Queued next: {}", source.display_name());
                    state.queued_next = Some(source);
                    response_tx.send(DecoderResponse::Ok).ok();
                }
                DecoderCommand::CancelNext => {
                    log::debug!("[Decoder Thread] Cancelled queued next");
                    state.queued_next = None;
                    response_tx.send(DecoderResponse::Ok).ok();
                }
                DecoderCommand::Stop => {
                    state.stop();
                    #[cfg(feature = "streaming")]
                    state.clear_stream_metadata(&event_tx);
                    message_tx.send(DecoderMessage::Flush).ok();
                    log::debug!("[Decoder Thread] Stopped");
                    response_tx.send(DecoderResponse::Ok).ok();
                }
                DecoderCommand::Shutdown => {
                    log::debug!("[Decoder Thread] Shutting down");
                    break;
                }
            }
        }

        // The manager can time out between the final open check and this
        // iteration's first frame. Do not publish a late-started stream.
        if state.current_attempt != 0
            && state.decoder.is_some()
            && state.current_attempt <= cancelled_attempt_through.load(Ordering::Acquire)
        {
            state.stop();
        }

        #[cfg(feature = "streaming")]
        state.poll_stream_metadata(&event_tx);

        // Generate frames based on mode
        if state.driver_source.active && !state.paused {
            // HAL Input / Silent Source mode
            match state.process_hal_input(
                &message_tx,
                &command_rx,
                &response_tx,
                frame_size,
                target_sample_rate,
            ) {
                Ok((true, _)) => {
                    // Frame processed successfully
                    // Don't sleep if connected - rely on backpressure from message_tx
                }
                Ok((false, pending_cmd)) => {
                    // No frame processed — either not enough data or send was
                    // interrupted by a command. If a command was consumed from
                    // command_rx by send_or_interrupt, handle it now so it isn't lost.
                    if let Some(cmd) = pending_cmd {
                        // Re-inject the command by processing it inline.
                        // This mirrors the command handling at the top of the loop.
                        match cmd {
                            DecoderCommand::Stop => {
                                state.stop();
                                #[cfg(feature = "streaming")]
                                state.clear_stream_metadata(&event_tx);
                                message_tx.send(DecoderMessage::Flush).ok();
                                log::debug!("[Decoder Thread] Stopped (from HAL interrupt)");
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                            DecoderCommand::Shutdown => {
                                log::debug!("[Decoder Thread] Shutting down (from HAL interrupt)");
                                return Ok(());
                            }
                            DecoderCommand::Pause => {
                                state.paused = true;
                                log::debug!("[Decoder Thread] Paused (from HAL interrupt)");
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                            DecoderCommand::Resume => {
                                state.paused = false;
                                log::debug!("[Decoder Thread] Resumed (from HAL interrupt)");
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                            DecoderCommand::Seek(position) => {
                                message_tx.send(DecoderMessage::Flush).ok();
                                if let Err(e) = state.seek(position) {
                                    response_tx.send(DecoderResponse::Error(e)).ok();
                                } else {
                                    event_tx
                                        .try_send(
                                            ThreadEvent::SeekComplete
                                                .with_decode_attempt(state.current_attempt),
                                        )
                                        .ok();
                                    response_tx.send(DecoderResponse::Ok).ok();
                                }
                            }
                            DecoderCommand::Play(path, attempt) => {
                                if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Play request cancelled".into(),
                                        ))
                                        .ok();
                                    continue;
                                }
                                // Adopt-first: see the top-of-loop Play arm.
                                state.current_attempt = attempt;
                                message_tx.send(DecoderMessage::Flush).ok();
                                state.stop();
                                #[cfg(feature = "streaming")]
                                state.clear_stream_metadata(&event_tx);
                                if let Err(e) = state.play(path, target_sample_rate, frame_size) {
                                    log::warn!(
                                        "[Decoder Thread] Play failed (from HAL interrupt): {}",
                                        e
                                    );
                                    event_tx
                                        .try_send(
                                            ThreadEvent::DecoderError(e)
                                                .with_decode_attempt(state.current_attempt),
                                        )
                                        .ok();
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Failed to start playback".to_string(),
                                        ))
                                        .ok();
                                } else if attempt
                                    <= cancelled_attempt_through.load(Ordering::Acquire)
                                {
                                    state.stop();
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Play request cancelled".into(),
                                        ))
                                        .ok();
                                } else {
                                    response_tx.send(DecoderResponse::Ok).ok();
                                }
                            }
                            DecoderCommand::PlayAt(path, position, attempt) => {
                                if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Play request cancelled".into(),
                                        ))
                                        .ok();
                                    continue;
                                }
                                // Adopt-first: see the top-of-loop Play arm.
                                state.current_attempt = attempt;
                                message_tx.send(DecoderMessage::Flush).ok();
                                state.stop();
                                #[cfg(feature = "streaming")]
                                state.clear_stream_metadata(&event_tx);
                                if let Err(e) = state.play(path, target_sample_rate, frame_size) {
                                    log::warn!(
                                        "[Decoder Thread] PlayAt failed (from HAL interrupt): {}",
                                        e
                                    );
                                    event_tx
                                        .try_send(
                                            ThreadEvent::DecoderError(e)
                                                .with_decode_attempt(state.current_attempt),
                                        )
                                        .ok();
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Failed to start playback".to_string(),
                                        ))
                                        .ok();
                                } else if attempt
                                    <= cancelled_attempt_through.load(Ordering::Acquire)
                                {
                                    state.stop();
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Play request cancelled".into(),
                                        ))
                                        .ok();
                                } else if let Err(e) = state.seek(position) {
                                    log::warn!(
                                        "[Decoder Thread] PlayAt seek failed (from HAL interrupt): {}",
                                        e
                                    );
                                    // Same revert as the top-of-loop PlayAt
                                    // arm: a freshly loaded decoder is safe
                                    // to abandon, so a failed seek unloads
                                    // it instead of emitting from the wrong
                                    // position under the old epoch.
                                    state.stop();
                                    event_tx
                                        .try_send(
                                            ThreadEvent::DecoderError(e)
                                                .with_decode_attempt(state.current_attempt),
                                        )
                                        .ok();
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Failed to seek during play_at".to_string(),
                                        ))
                                        .ok();
                                } else if attempt
                                    <= cancelled_attempt_through.load(Ordering::Acquire)
                                {
                                    state.stop();
                                    response_tx
                                        .send(DecoderResponse::Error(
                                            "Play request cancelled".into(),
                                        ))
                                        .ok();
                                } else {
                                    event_tx
                                        .try_send(
                                            ThreadEvent::SeekComplete
                                                .with_decode_attempt(state.current_attempt),
                                        )
                                        .ok();
                                    response_tx.send(DecoderResponse::Ok).ok();
                                }
                            }
                            DecoderCommand::StartSilentSource(channels) => {
                                state.start_silent_source(channels);
                            }
                            DecoderCommand::StartCaptureSource(source, channels) => {
                                state.start_capture_source(source, channels);
                            }
                            DecoderCommand::QueueNext(path) => {
                                log::debug!(
                                    "[Decoder Thread] Queued next (from HAL interrupt): {:?}",
                                    path
                                );
                                state.queued_next = Some(path);
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                            DecoderCommand::CancelNext => {
                                log::debug!(
                                    "[Decoder Thread] Cancelled queued next (from HAL interrupt)"
                                );
                                state.queued_next = None;
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                Err(e) => {
                    crate::rate_limited_log!(warn, 5, "[Decoder Thread] HAL input error: {}", e);
                    state.stop();
                    #[cfg(feature = "streaming")]
                    state.clear_stream_metadata(&event_tx);
                    // Reliable async report (single path — no lossy
                    // event twin): the manager tick applies it iff the
                    // tag is still current. Unbounded send never
                    // blocks; this is the worker loop, not an audio
                    // callback, so the node allocation is allowed.
                    // Bounded production: this error stopped the
                    // phase — resumption needs a new attempt.
                    async_error_tx
                        .send(DecoderAsyncError {
                            attempt: state.current_attempt,
                            message: e,
                        })
                        .ok();
                }
            }

            // Explicit capture transports already supply a clock through
            // availability and downstream backpressure. Do not pace their
            // successful reads a second time (or treat them as disconnected HAL).
            if !state.driver_source.has_explicit_source() {
                #[cfg(all(target_os = "macos", feature = "hal"))]
                {
                    if state.hal_reader.as_ref().is_none_or(|r| !r.is_connected()) {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
                #[cfg(not(all(target_os = "macos", feature = "hal")))]
                {
                    // Non-HAL silent source mode: sleep to maintain frame rate
                    let frame_duration_ms =
                        (frame_size as f64 / target_sample_rate as f64 * 1000.0) as u64;
                    std::thread::sleep(std::time::Duration::from_millis(frame_duration_ms));
                }
            }
        } else if state.decoder.is_some() && !state.paused {
            // File playback mode: decode from file
            match state.decode_chunk(
                &message_tx,
                &command_rx,
                &response_tx,
                &event_tx,
                frame_size,
                target_sample_rate,
            ) {
                Ok(DecoderLoopAction::Continue) => {
                    // Continue
                }
                Ok(DecoderLoopAction::Stop) => {
                    // End of stream - stop
                    state.stop();
                    #[cfg(feature = "streaming")]
                    state.clear_stream_metadata(&event_tx);
                }
                Ok(DecoderLoopAction::Interrupted(cmd)) => {
                    // Handle interruption command immediately
                    match cmd {
                        DecoderCommand::Play(path, attempt) => {
                            if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                response_tx
                                    .send(DecoderResponse::Error("Play request cancelled".into()))
                                    .ok();
                                continue;
                            }
                            // Adopt-first: see the top-of-loop Play arm.
                            state.current_attempt = attempt;
                            message_tx.send(DecoderMessage::Flush).ok();
                            state.stop();
                            #[cfg(feature = "streaming")]
                            state.clear_stream_metadata(&event_tx);
                            if let Err(e) = state.play(path, target_sample_rate, frame_size) {
                                log::warn!("[Decoder Thread] Play failed: {}", e);
                                event_tx
                                    .try_send(
                                        ThreadEvent::DecoderError(e)
                                            .with_decode_attempt(state.current_attempt),
                                    )
                                    .ok();
                                response_tx
                                    .send(DecoderResponse::Error(
                                        "Failed to start playback".to_string(),
                                    ))
                                    .ok();
                            } else if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                state.stop();
                                response_tx
                                    .send(DecoderResponse::Error("Play request cancelled".into()))
                                    .ok();
                            } else {
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                        }
                        DecoderCommand::PlayAt(path, position, attempt) => {
                            if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                response_tx
                                    .send(DecoderResponse::Error("Play request cancelled".into()))
                                    .ok();
                                continue;
                            }
                            // Adopt-first: see the top-of-loop Play arm.
                            state.current_attempt = attempt;
                            message_tx.send(DecoderMessage::Flush).ok();
                            state.stop();
                            #[cfg(feature = "streaming")]
                            state.clear_stream_metadata(&event_tx);
                            if let Err(e) = state.play(path, target_sample_rate, frame_size) {
                                log::warn!("[Decoder Thread] PlayAt (load) failed: {}", e);
                                event_tx
                                    .try_send(
                                        ThreadEvent::DecoderError(e)
                                            .with_decode_attempt(state.current_attempt),
                                    )
                                    .ok();
                                response_tx
                                    .send(DecoderResponse::Error(
                                        "Failed to start playback".to_string(),
                                    ))
                                    .ok();
                            } else if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                state.stop();
                                response_tx
                                    .send(DecoderResponse::Error("Play request cancelled".into()))
                                    .ok();
                            } else if let Err(e) = state.seek(position) {
                                log::warn!("[Decoder Thread] PlayAt (seek) failed: {}", e);
                                // Same revert as the top-of-loop PlayAt
                                // arm: a freshly loaded decoder is safe to
                                // abandon, so a failed seek unloads it
                                // instead of emitting from the wrong
                                // position under the old epoch.
                                state.stop();
                                event_tx
                                    .try_send(
                                        ThreadEvent::DecoderError(e)
                                            .with_decode_attempt(state.current_attempt),
                                    )
                                    .ok();
                                response_tx
                                    .send(DecoderResponse::Error(
                                        "Failed to seek during play_at".to_string(),
                                    ))
                                    .ok();
                            } else if attempt <= cancelled_attempt_through.load(Ordering::Acquire) {
                                state.stop();
                                response_tx
                                    .send(DecoderResponse::Error("Play request cancelled".into()))
                                    .ok();
                            } else {
                                event_tx
                                    .try_send(
                                        ThreadEvent::SeekComplete
                                            .with_decode_attempt(state.current_attempt),
                                    )
                                    .ok();
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                        }
                        DecoderCommand::StartSilentSource(channels) => {
                            state.start_silent_source(channels);
                        }
                        DecoderCommand::StartCaptureSource(source, channels) => {
                            state.start_capture_source(source, channels);
                        }
                        DecoderCommand::Pause => {
                            state.paused = true;
                            log::debug!("[Decoder Thread] Paused");
                            response_tx.send(DecoderResponse::Ok).ok();
                        }
                        DecoderCommand::Resume => {
                            state.paused = false;
                            log::debug!("[Decoder Thread] Resumed");
                            response_tx.send(DecoderResponse::Ok).ok();
                        }
                        DecoderCommand::Seek(position) => {
                            message_tx.send(DecoderMessage::Flush).ok();
                            if let Err(e) = state.seek(position) {
                                log::warn!("[Decoder Thread] Seek failed: {}", e);
                                response_tx.send(DecoderResponse::Error(e)).ok();
                            } else {
                                event_tx
                                    .try_send(
                                        ThreadEvent::SeekComplete
                                            .with_decode_attempt(state.current_attempt),
                                    )
                                    .ok();
                                response_tx.send(DecoderResponse::Ok).ok();
                            }
                        }
                        DecoderCommand::QueueNext(path) => {
                            log::debug!(
                                "[Decoder Thread] Queued next (from interrupt): {:?}",
                                path
                            );
                            state.queued_next = Some(path);
                            response_tx.send(DecoderResponse::Ok).ok();
                        }
                        DecoderCommand::CancelNext => {
                            log::debug!("[Decoder Thread] Cancelled queued next (from interrupt)");
                            state.queued_next = None;
                            response_tx.send(DecoderResponse::Ok).ok();
                        }
                        DecoderCommand::Stop => {
                            state.stop();
                            #[cfg(feature = "streaming")]
                            state.clear_stream_metadata(&event_tx);
                            message_tx.send(DecoderMessage::Flush).ok();
                            log::debug!("[Decoder Thread] Stopped");
                            response_tx.send(DecoderResponse::Ok).ok();
                        }
                        DecoderCommand::Shutdown => {
                            log::debug!("[Decoder Thread] Shutting down");
                            break;
                        }
                    }
                }
                Err(e) => {
                    crate::rate_limited_log!(warn, 5, "[Decoder Thread] Error: {}", e);
                    state.stop();
                    #[cfg(feature = "streaming")]
                    state.clear_stream_metadata(&event_tx);
                    // Reliable async report: see the HAL arm above
                    // (mid-phase decode failure incl. queue-stuck).
                    async_error_tx
                        .send(DecoderAsyncError {
                            attempt: state.current_attempt,
                            message: e,
                        })
                        .ok();
                }
            }
        } else {
            // Idle: small sleep to avoid busy loop when paused/stopped
            std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_SLEEP_DECODER));
        }
    }

    // log::debug!("[Decoder Thread] Stopped");
    Ok(())
}
