use super::{ManagerCommandHandler, ManagerContext, ManagerResponse};
use crate::decoder::AudioSource;
use crate::engine::{DecoderCommand, PlaybackCommand, PlaybackState};
use std::sync::Arc;

/// Start playback of a source from the beginning.
pub struct PlayCommand(pub AudioSource);

impl ManagerCommandHandler for PlayCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let source = &self.0;
        log::debug!("[Manager Thread] Play: {}", source.display_name());

        let attempt = ctx.next_decode_attempt();
        let request_id =
            match ctx.send_counted_stream_command(DecoderCommand::Play(source.clone(), attempt)) {
                Ok(request_id) => request_id,
                Err(e) => return ManagerResponse::Error(e),
            };
        // Bump at SEND-OK (before the ack wait): the queued command
        // carries the tag and the decoder will adopt it — even on a
        // later ack timeout, when the adoption lands late.
        ctx.store_decode_attempt(attempt);

        match super::super::wait::wait_for_decoder_ack(
            ctx.decoder,
            request_id,
            std::time::Duration::from_millis(super::super::consts::DECODER_COMMAND_TIMEOUT_MS),
        ) {
            Ok(()) => {
                let mut new_state = (**ctx.state.load()).clone();
                let epoch = new_state.playback_epoch.wrapping_add(1);
                if let Err(e) = ctx.playback.send_command(PlaybackCommand::Resume { epoch }) {
                    return ManagerResponse::Error(e);
                }
                new_state.current_file = source.as_path().map(|p| p.to_path_buf());
                new_state.current_source = Some(source.clone());
                new_state.playback_state = PlaybackState::Playing;
                new_state.position = 0.0;
                // New playback epoch: advance the generation and clear the latch.
                new_state.playback_epoch = epoch;
                new_state.playback_peak_max_linear = 0.0;
                // A new epoch starts a new peak record (complete until
                // a worker death says otherwise — P3 sets it false).
                new_state.peak_record_complete = true;
                new_state.seeking = false;
                new_state.seeking_since = None;
                new_state.last_error = None;
                ctx.state.store(Arc::new(new_state));
                ManagerResponse::Ok
            }
            Err(e) => ManagerResponse::Error(e),
        }
    }
}
