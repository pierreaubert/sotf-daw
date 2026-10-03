use super::{ManagerCommandHandler, ManagerContext, ManagerResponse};
use crate::engine::{
    DecoderCommand, PlaybackCommand, PlaybackState, PlaybackStopRequest, apply_stop_ack,
};
use std::sync::Arc;

/// Stop playback and clear the current source.
pub struct StopCommand;

impl ManagerCommandHandler for StopCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        log::debug!("[Manager Thread] Stop");

        // Fold any late acknowledgment from an earlier timed-out Stop
        // before this Stop's own barrier runs.
        ctx.playback.collect_ready_stop_acks(ctx.state);

        let request_id = match ctx.send_counted_stream_command(DecoderCommand::Stop) {
            Ok(request_id) => request_id,
            Err(e) => return ManagerResponse::Error(e),
        };
        if let Err(e) = super::super::wait::wait_for_decoder_ack(
            ctx.decoder,
            request_id,
            std::time::Duration::from_millis(super::super::consts::DECODER_COMMAND_TIMEOUT_MS),
        ) {
            return ManagerResponse::Error(e);
        }

        let (request, reply_rx) = PlaybackStopRequest::new();
        // Best-effort: the playback thread may have already exited after
        // end-of-stream drain. This is expected during auto-advance. An
        // exited worker emits nothing further, so the emission cutoff is
        // trivially established; any queued drained receipt still folds
        // through the visitor.
        if let Err(e) = ctx.playback.send_command(PlaybackCommand::Stop(request)) {
            log::debug!(
                "[Manager Thread] Stop send to playback failed (already exited): {}",
                e
            );
            let mut new_state = (**ctx.state.load()).clone();
            new_state.playback_state = PlaybackState::Stopped;
            new_state.current_file = None;
            new_state.current_source = None;
            new_state.position = 0.0;
            new_state.seeking = false;
            new_state.seeking_since = None;
            new_state.output_peak_linear = 0.0;
            new_state.output_clipping_detected = false;
            // Death-text preservation: on a poisoned transport the death
            // record is forensic; a Stop must not clear it (Stop still
            // converges the rest of the transport above).
            if !new_state.worker_death_poisoned {
                new_state.last_error = None;
            }
            ctx.state.store(Arc::new(new_state));
            return ManagerResponse::Ok;
        }

        let mut new_state = (**ctx.state.load()).clone();
        match reply_rx.recv_timeout(std::time::Duration::from_millis(
            super::super::consts::PLAYBACK_STOP_ACK_TIMEOUT_MS,
        )) {
            Ok(ack) => {
                // Sync-complete: the terminal record covers every
                // callback-observed sample before the cutoff.
                apply_stop_ack(&mut new_state, &ack);
                // Death text wins over the sync-clear (the fold above
                // is kept: max-composition is monotonic-safe).
                if !new_state.worker_death_poisoned {
                    new_state.last_error = None;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Transport still reaches Stopped; only completion is
                // pending. The stashed receiver folds the record in on
                // a later tick, so the timeout degrades timeliness,
                // never correctness.
                log::warn!(
                    "[Manager Thread] Stop acknowledgment timed out after {}ms; collecting late",
                    super::super::consts::PLAYBACK_STOP_ACK_TIMEOUT_MS
                );
                // Death text wins over the timeout note (still
                // warn-logged above; the stash below is kept).
                if !new_state.worker_death_poisoned {
                    new_state.last_error = Some(format!(
                        "Timed out waiting for playback Stop acknowledgment after {}ms",
                        super::super::consts::PLAYBACK_STOP_ACK_TIMEOUT_MS
                    ));
                }
                ctx.playback.stash_pending_stop_ack(reply_rx);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Death text wins over the disconnect note (consistent
                // with — but less specific than — the death record).
                if !new_state.worker_death_poisoned {
                    new_state.last_error =
                        Some("Playback worker exited before answering Stop".to_string());
                }
            }
        }
        new_state.playback_state = PlaybackState::Stopped;
        new_state.current_file = None;
        new_state.current_source = None;
        new_state.position = 0.0;
        new_state.seeking = false;
        new_state.seeking_since = None;
        new_state.output_peak_linear = 0.0;
        new_state.output_clipping_detected = false;
        ctx.state.store(Arc::new(new_state));

        ManagerResponse::Ok
    }
}
