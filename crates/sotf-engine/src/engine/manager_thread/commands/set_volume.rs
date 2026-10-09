use super::{ManagerCommandHandler, ManagerContext, ManagerResponse};
use crate::engine::{PlaybackCommand, ProcessingCommand, ProcessingThread};
use std::sync::Arc;

/// Set the playback volume.
pub struct SetVolumeCommand(pub f32);

pub(in crate::engine::manager_thread) fn sync_playback_volume(
    processing: &ProcessingThread,
    volume: f32,
) -> Result<(), String> {
    let request_id = processing.send_command(ProcessingCommand::SetPlaybackVolume(volume))?;
    super::super::wait::wait_for_processing_ack(
        processing,
        request_id,
        std::time::Duration::from_millis(super::super::consts::PROCESSING_COMMAND_TIMEOUT_MS),
    )
}

impl ManagerCommandHandler for SetVolumeCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let volume = self.0;
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return ManagerResponse::Error(
                "playback volume must be finite and between 0 and 1".into(),
            );
        }
        log::debug!("[Manager Thread] Set volume: {:.2}", volume);

        // A dead processing worker still permits sink-side volume latching.
        // A live worker must acknowledge the same attenuation used by the sink.
        if !ctx.processing.is_finished()
            && let Err(error) = sync_playback_volume(ctx.processing, volume)
        {
            return ManagerResponse::Error(error);
        }

        {
            let mut new_state = (**ctx.state.load()).clone();
            new_state.volume = volume;
            ctx.state.store(Arc::new(new_state));
        }

        // Best-effort: the playback thread may have already exited after
        // end-of-stream drain. The volume is stored in state and will be
        // applied when the next engine starts.
        if let Err(e) = ctx
            .playback
            .send_command(PlaybackCommand::SetVolume(volume))
        {
            log::debug!(
                "[Manager Thread] SetVolume send failed (playback ended): {}",
                e
            );
        }

        ManagerResponse::Ok
    }
}
