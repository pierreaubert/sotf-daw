use super::{GetPluginDataCommand, ManagerCommandHandler, ManagerContext, ManagerResponse};
use sotf_plugins::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqControlHandle;
use sotf_plugins::plugin_linear_phase_eq::{BandConfig, LinearPhaseEqPlugin};
use std::sync::Arc;

/// Fetch the shared linear-phase EQ handle for one plugin index.
///
/// Reuses the existing `GetPluginData` round-trip (manager queue into the
/// processing thread and back), then downcasts the typed `Arc`. No new
/// processing command is needed: all FIR handoff flows through the shared
/// bounded mailbox plus lock-free status mirrors on this handle.
fn fetch_handle(
    ctx: &mut ManagerContext,
    plugin_index: usize,
) -> Result<Arc<LinearPhaseEqControlHandle>, String> {
    match GetPluginDataCommand(plugin_index).execute(ctx) {
        ManagerResponse::PluginData(data) => {
            Arc::downcast::<LinearPhaseEqControlHandle>(data).map_err(|_| {
                format!("plugin index {plugin_index} is not a linear-phase EQ")
            })
        }
        ManagerResponse::Error(reason) => Err(reason),
        _ => Err(format!(
            "unexpected plugin-data response for plugin index {plugin_index}"
        )),
    }
}

/// Snapshot, prepare and queue one linear-phase EQ band-shape edit.
///
/// The manager thread reads the immutable accepted base from the shared
/// handle, runs FIR design and convolver planning here (off audio), and
/// queues the bounded payload through the shared mailbox. Real audio pops
/// and commits the existing crossfade on its own quanta. `Ok` means queued,
/// not accepted: callers observe acceptance via `LinearPhaseEqStatusCommand`
/// (generation advance) and reprepare on `StaleBase` refusal. Prepare-time
/// failures (index, placement, range) and full-queue failures return `Error`
/// with live state untouched; the refused payload drops on this thread.
pub struct LinearPhaseEqRequestCommand {
    pub plugin_index: usize,
    pub band_index: usize,
    pub new_band: BandConfig,
}

impl ManagerCommandHandler for LinearPhaseEqRequestCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let handle = match fetch_handle(ctx, self.plugin_index) {
            Ok(handle) => handle,
            Err(reason) => return ManagerResponse::Error(reason),
        };
        let accepted = match handle.try_accepted_snapshot() {
            Ok(accepted) => accepted,
            Err(_) => {
                return ManagerResponse::Error(
                    "linear-phase EQ accepted snapshot busy; retry later".to_string(),
                );
            }
        };
        let prepared = match LinearPhaseEqPlugin::prepare_band_update(
            &accepted.snapshot,
            self.band_index,
            self.new_band.clone(),
        ) {
            Ok(prepared) => prepared,
            Err(reason) => return ManagerResponse::Error(reason),
        };
        match handle.try_submit(prepared) {
            Ok(()) => ManagerResponse::Ok,
            Err(reason) => ManagerResponse::Error(reason),
        }
    }
}

/// Request eviction of a wedged linear-phase EQ head payload.
///
/// Records one coalescing cancel generation on the shared handle; audio
/// evicts at most one head payload per quantum into the bounded outbox
/// without dropping. Observe completion via `LinearPhaseEqStatusCommand`
/// (retained count plus refusal clearing) and destroy the evicted payload
/// off audio.
pub struct LinearPhaseEqCancelCommand {
    pub plugin_index: usize,
}

impl ManagerCommandHandler for LinearPhaseEqCancelCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let handle = match fetch_handle(ctx, self.plugin_index) {
            Ok(handle) => handle,
            Err(reason) => return ManagerResponse::Error(reason),
        };
        handle.request_cancel();
        ManagerResponse::Ok
    }
}

/// Read detached linear-phase EQ accepted generation and queue status.
///
/// Fetches the shared handle through the existing queue round-trip, then
/// reads its infallible lock-free mirrors. The generation advances only
/// when real audio accepts a commit; the refusal mirror reports reachable
/// `StaleBase`/transient/topology outcomes for reprepare decisions.
pub struct LinearPhaseEqStatusCommand {
    pub plugin_index: usize,
}

impl ManagerCommandHandler for LinearPhaseEqStatusCommand {
    fn execute(&self, ctx: &mut ManagerContext) -> ManagerResponse {
        let handle = match fetch_handle(ctx, self.plugin_index) {
            Ok(handle) => handle,
            Err(reason) => return ManagerResponse::Error(reason),
        };
        ManagerResponse::LinearPhaseEqStatus(handle.control_status())
    }
}
