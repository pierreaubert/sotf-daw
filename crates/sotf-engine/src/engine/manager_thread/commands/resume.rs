use super::{ManagerCommandHandler, ManagerContext, ManagerResponse};
use crate::engine::{AudioEngineState, DecoderCommand, PlaybackCommand, PlaybackState};
use std::sync::Arc;

/// Resume paused playback.
pub struct ResumeCommand;

#[derive(Debug, PartialEq, Eq)]
enum ResumeDecision {
    Resume,
    AlreadyPlaying,
    Reject,
}

fn resume_decision(state: &AudioEngineState, driver_mode: bool) -> ResumeDecision {
    if (state.current_source.is_none() && !driver_mode) || state.worker_death_poisoned {
        return ResumeDecision::Reject;
    }
    match state.playback_state {
        PlaybackState::Paused => ResumeDecision::Resume,
        PlaybackState::Playing => ResumeDecision::AlreadyPlaying,
        PlaybackState::Stopped => ResumeDecision::Reject,
    }
}

impl ManagerCommandHandler for ResumeCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        log::debug!("[Manager Thread] Resume");

        match resume_decision(&ctx.state.load(), ctx.config.driver_mode) {
            ResumeDecision::AlreadyPlaying => return ManagerResponse::Ok,
            ResumeDecision::Reject => {
                return ManagerResponse::Error("Cannot resume without a paused source".into());
            }
            ResumeDecision::Resume => {}
        }

        let request_id = match ctx.decoder.send_command(DecoderCommand::Resume) {
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
                // Same epoch: resume continues the current playback session.
                let epoch = new_state.playback_epoch;
                if let Err(e) = ctx.playback.send_command(PlaybackCommand::Resume { epoch }) {
                    return ManagerResponse::Error(e);
                }
                new_state.playback_state = PlaybackState::Playing;
                ctx.state.store(Arc::new(new_state));
                ManagerResponse::Ok
            }
            Err(e) => ManagerResponse::Error(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AudioEngineState;

    #[test]
    fn resume_requires_a_paused_committed_source() {
        let mut state = AudioEngineState::default();
        assert_eq!(resume_decision(&state, false), ResumeDecision::Reject);
        state.current_source = Some(crate::decoder::AudioSource::File("test.wav".into()));
        assert_eq!(resume_decision(&state, false), ResumeDecision::Reject);
        state.playback_state = PlaybackState::Paused;
        assert_eq!(resume_decision(&state, false), ResumeDecision::Resume);
        state.playback_state = PlaybackState::Playing;
        assert_eq!(resume_decision(&state, false), ResumeDecision::AlreadyPlaying);
        state.current_source = None;
        assert_eq!(resume_decision(&state, false), ResumeDecision::Reject);
        state.playback_state = PlaybackState::Paused;
        assert_eq!(resume_decision(&state, true), ResumeDecision::Resume);
    }
}
