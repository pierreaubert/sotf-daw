use super::{ManagerCommandHandler, ManagerContext, ManagerResponse};
use crate::engine::{DecoderCommand, PlaybackState};
use std::sync::Arc;

/// Seek to a position (in seconds) in the current source.
pub struct SeekCommand(pub f64);

impl SeekCommand {
    /// Terminal "No decoder": surface a drained async root cause when
    /// one is pending; benign seeks (nothing pending) return
    /// unstamped, exactly as before.
    fn finish_no_decoder(
        ctx: &mut ManagerContext,
        sync_err: String,
        attempt: crate::engine::DecodeAttempt,
    ) -> ManagerResponse {
        let causes = super::super::thread_event_visitor::select_current_decoder_errors(
            ctx.decoder.drain_async_errors(),
            attempt,
        );
        if causes.is_empty() {
            return ManagerResponse::Error(sync_err);
        }
        let chained =
            super::super::thread_event_visitor::chain_decoder_causes(sync_err.clone(), &causes);
        let mut new_state = (**ctx.state.load()).clone();
        super::super::thread_event_visitor::record_decoder_error(&mut new_state, chained);
        ctx.state.store(Arc::new(new_state));
        ManagerResponse::Error(sync_err)
    }
}

impl ManagerCommandHandler for SeekCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let position = self.0;
        log::debug!("[Manager Thread] Seek to {:.2}s", position);

        let request_id = match ctx.send_counted_stream_command(DecoderCommand::Seek(position)) {
            Ok(request_id) => request_id,
            Err(e) => return ManagerResponse::Error(e),
        };

        match super::super::wait::wait_for_decoder_ack(
            ctx.decoder,
            request_id,
            std::time::Duration::from_millis(super::super::consts::DECODER_COMMAND_TIMEOUT_MS),
        ) {
            Ok(()) => {
                let mut new_state = (**ctx.state.load()).clone();
                new_state.position = position;
                new_state.seeking = true;
                new_state.seeking_since = Some(std::time::Instant::now());
                ctx.state.store(Arc::new(new_state));
                ManagerResponse::Ok
            }
            Err(e) if e == "No decoder" => {
                let current = ctx.state.load();
                let source = current.current_source.clone();
                let live = current.playback_state != PlaybackState::Stopped;
                let attempt = current.decoder_attempt;
                drop(current);
                // Terminal unless a live transport has a source to
                // reopen. Only the terminal branches drain: a recovery
                // never consumes a pending async error it cannot use
                // (it stays queued and is judged under the post-bump
                // attempt at the next tick — no loss, no early eat).
                let Some(source) = source else {
                    return Self::finish_no_decoder(ctx, e, attempt);
                };
                if !live {
                    return Self::finish_no_decoder(ctx, e, attempt);
                }

                log::debug!(
                    "[Manager Thread] Seek found no active decoder; reopening current source at {:.2}s",
                    position
                );
                let reopen_attempt = ctx.next_decode_attempt();
                let play_request_id = match ctx.send_counted_stream_command(DecoderCommand::PlayAt(
                    source.clone(),
                    position,
                    reopen_attempt,
                )) {
                    Ok(request_id) => request_id,
                    Err(play_err) => return ManagerResponse::Error(play_err),
                };
                // Bump at SEND-OK (before the ack wait): see PlayCommand.
                // The reopen is a new decode phase under a new tag.
                ctx.store_decode_attempt(reopen_attempt);

                match super::super::wait::wait_for_decoder_ack(
                    ctx.decoder,
                    play_request_id,
                    std::time::Duration::from_millis(
                        super::super::consts::DECODER_COMMAND_TIMEOUT_MS,
                    ),
                ) {
                    Ok(()) => {
                        let mut new_state = (**ctx.state.load()).clone();
                        new_state.current_file = source.as_path().map(|p| p.to_path_buf());
                        new_state.current_source = Some(source);
                        new_state.playback_state = PlaybackState::Playing;
                        new_state.position = position;
                        new_state.seeking = true;
                        new_state.seeking_since = Some(std::time::Instant::now());
                        ctx.state.store(Arc::new(new_state));
                        ManagerResponse::Ok
                    }
                    Err(play_err) => ManagerResponse::Error(play_err),
                }
            }
            Err(e) => ManagerResponse::Error(e),
        }
    }
}
