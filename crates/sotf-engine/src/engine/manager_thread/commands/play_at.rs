use super::{ManagerCommandHandler, ManagerContext, ManagerResponse};
use crate::decoder::AudioSource;
use crate::engine::{DecoderCommand, PlaybackCommand, PlaybackState};
use std::sync::Arc;

/// Start playback of a source at a specific position (in seconds).
pub struct PlayAtCommand(pub AudioSource, pub f64);

impl ManagerCommandHandler for PlayAtCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let source = &self.0;
        let position = self.1;
        log::debug!(
            "[Manager Thread] PlayAt: {} at {:.2}s",
            source.display_name(),
            position
        );

        let attempt = ctx.next_decode_attempt();
        let request_id = match ctx.send_counted_stream_command(DecoderCommand::PlayAt(
            source.clone(),
            position,
            attempt,
        )) {
            Ok(request_id) => request_id,
            Err(e) => return ManagerResponse::Error(e),
        };
        // Bump at SEND-OK (before the ack wait): see PlayCommand.
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
                new_state.position = position;
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
            Err(e) => {
                // Synchronous honesty: the decoder-side failure is known
                // (the ack is explicit), so record it without depending on
                // the lossy event channel. The DecoderError event, when it
                // lands, re-applies the same transition. Drained async
                // causes chain behind the sync symptom (root-cause
                // preservation); identity when none are pending.
                let mut new_state = (**ctx.state.load()).clone();
                let causes = super::super::thread_event_visitor::select_current_decoder_errors(
                    ctx.decoder.drain_async_errors(),
                    new_state.decoder_attempt,
                );
                let chained =
                    super::super::thread_event_visitor::chain_decoder_causes(e.clone(), &causes);
                super::super::thread_event_visitor::record_decoder_error(&mut new_state, chained);
                ctx.state.store(Arc::new(new_state));
                ManagerResponse::Error(e)
            }
        }
    }
}
