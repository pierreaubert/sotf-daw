//! Command objects for the engine manager thread.
//!
//! Each variant of [`ManagerCommand`](crate::engine::ManagerCommand) is handled by a small
//! struct implementing [`ManagerCommandHandler`]. This keeps `handle_command` short and makes
//! individual commands unit-testable.

use crate::engine::{
    AudioEngineState, DecoderCommand, DecoderThread, EngineConfig, ManagerResponse, PlaybackThread,
    ProcessingThread,
};
use arc_swap::ArcSwap;
use std::sync::Arc;

use super::config_update_queue::ConfigUpdateQueue;

mod bypass_processing;
mod cancel_next;
mod get_plugin_data;
mod get_position;
mod get_state;
mod linear_phase_eq;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod maintain_isolated_external_plugin_workers;
mod mute;
mod pause;
mod play;
mod play_at;
mod queue_next;
mod reload_config;
mod resume;
mod seek;
mod set_plugin_parameter;
mod set_volume;
mod shutdown;
mod stop;
mod update_plugin_chain;
mod update_plugin_graph;

pub use bypass_processing::BypassProcessingCommand;
pub use cancel_next::CancelNextCommand;
pub use get_plugin_data::GetPluginDataCommand;
pub use get_position::GetPositionCommand;
pub use get_state::GetStateCommand;
pub use linear_phase_eq::{
    LinearPhaseEqCancelCommand, LinearPhaseEqRequestCommand, LinearPhaseEqStatusCommand,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use maintain_isolated_external_plugin_workers::MaintainIsolatedExternalPluginWorkersCommand;
pub use mute::MuteCommand;
pub use pause::PauseCommand;
pub use play::PlayCommand;
pub use play_at::PlayAtCommand;
pub use queue_next::QueueNextCommand;
pub use reload_config::ReloadConfigCommand;
pub use resume::ResumeCommand;
pub use seek::SeekCommand;
pub use set_plugin_parameter::SetPluginParameterCommand;
pub use set_volume::SetVolumeCommand;
pub use shutdown::ShutdownCommand;
pub use stop::StopCommand;
pub use update_plugin_chain::UpdatePluginChainCommand;
pub use update_plugin_graph::UpdatePluginGraphCommand;

/// Mutable context passed to every command handler.
pub struct ManagerContext<'a> {
    pub decoder: &'a mut DecoderThread,
    pub processing: &'a mut ProcessingThread,
    pub playback: &'a mut PlaybackThread,
    pub state: &'a Arc<ArcSwap<AudioEngineState>>,
    pub config: &'a EngineConfig,
    pub config_queue: &'a mut ConfigUpdateQueue,
}

impl ManagerContext<'_> {
    /// Send a stream command, counting its Flush on success.
    ///
    /// Every [`emits_stream_flush`](crate::engine::emits_stream_flush)
    /// command funnels through here: on SEND-OK exactly one Flush is
    /// forthcoming from the decoder, so `flushes_sent` advances by
    /// one. Failures queue nothing and count nothing.
    ///
    /// # Panics
    ///
    /// Debug builds panic when the command emits no Flush; counting
    /// it would wedge the generation gate against receipts that can
    /// never arrive.
    pub fn send_counted_stream_command(&mut self, command: DecoderCommand) -> Result<u64, String> {
        debug_assert!(
            crate::engine::emits_stream_flush(&command),
            "counted send requires a Flush-emitting command"
        );
        let request_id = self.decoder.send_command(command)?;
        let mut new_state = (**self.state.load()).clone();
        new_state.flushes_sent = new_state.flushes_sent.wrapping_add(1);
        self.state.store(Arc::new(new_state));
        Ok(request_id)
    }

    /// Compute the next decode attempt WITHOUT storing it.
    ///
    /// The tag must be chosen before the send it identifies (it
    /// travels ON the Play/PlayAt command); persistence happens in
    /// [`ManagerContext::store_decode_attempt`] after SEND-OK.
    pub fn next_decode_attempt(&self) -> crate::engine::DecodeAttempt {
        self.state.load().decoder_attempt.wrapping_add(1)
    }

    /// Persist a decode attempt after its Play/PlayAt SEND-OK.
    ///
    /// Stored immediately — even when the later ack times out or
    /// fails: send-ok means queued means the decoder will adopt the
    /// carried tag (adopt-first in every arm, NACK paths included),
    /// so the manager must advance to match. Monotonic, never reset.
    pub fn store_decode_attempt(&mut self, attempt: crate::engine::DecodeAttempt) {
        let mut new_state = (**self.state.load()).clone();
        new_state.decoder_attempt = attempt;
        self.state.store(Arc::new(new_state));
    }
}

/// Trait implemented by every manager command.
pub trait ManagerCommandHandler {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse;
}
