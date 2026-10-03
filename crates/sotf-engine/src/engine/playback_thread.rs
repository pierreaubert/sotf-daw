use super::worker_death::{WorkerExit, WorkerExitStatus, record_worker_exit};
use super::{
    AudioEngineState, HostUpdateTicket, PendingStopAcks, PlaybackCommand, PlaybackConfiguration,
    PlaybackReconfigureRequest, PlaybackStopAck, ProcessingMessage, ThreadEvent,
};
use crate::OutputAccessMode;
use arc_swap::ArcSwap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, Sender, SyncSender};

mod apply;
mod build;
mod core_audio_exclusive_mode_guard;
mod coreaudio_mod;
mod frame_writer;
mod misc;
mod pick;
mod playback;
mod playback_state;
mod runtime;
#[cfg(test)]
mod tests;
mod types;

#[cfg(feature = "playback-runtime-harness")]
pub(in crate::engine) use apply::apply_volume_clamp;
#[allow(unused_imports)]
#[cfg(any(test, feature = "playback-runtime-harness"))]
pub(in crate::engine) use frame_writer::{
    FrameWriteOutcome, required_conversion_capacity, write_frame_to_ring,
};
use misc::send_playback_event;
#[cfg(feature = "playback-runtime-harness")]
pub(in crate::engine) use misc::write_chunk_bulk;
#[cfg(feature = "playback-runtime-harness")]
pub(in crate::engine) use playback_state::{PlaybackState, read_ring_buffer};
use runtime::run_playback_thread;

const PLAYBACK_RECONFIGURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Playback thread handle
pub struct PlaybackThread {
    command_tx: Sender<PlaybackCommand>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    pending_stop_acks: PendingStopAcks,
    exit_status: WorkerExitStatus,
    /// Wrapper-held clone of the runtime's shared output-peak atomic.
    ///
    /// Created here and passed into the runtime (which shares it with every
    /// rebuild via `new_sharing_meters`), so the manager can fold the
    /// residual peak after a worker death even though the runtime is gone.
    retained_output_peak_bits: Arc<AtomicU32>,
}

impl PlaybackThread {
    #[cfg(test)]
    pub(in crate::engine) fn command_probe() -> (Self, Receiver<PlaybackCommand>) {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        (
            Self {
                command_tx,
                thread_handle: None,
                pending_stop_acks: PendingStopAcks::default(),
                exit_status: WorkerExitStatus::new(),
                retained_output_peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            },
            command_rx,
        )
    }

    /// Create and start the playback thread
    #[allow(
        clippy::too_many_arguments,
        reason = "constructor mirrors run_playback_thread argument list"
    )]
    pub fn new(
        message_rx: Receiver<ProcessingMessage>,
        event_tx: crossbeam::channel::Sender<ThreadEvent>,
        sample_rate: u32,
        buffer_ms: u32,
        channels: usize,
        frame_size: usize,
        output_device: Option<String>,
        recycle_tx: SyncSender<Vec<f32>>,
        allow_virtual_output: bool,
        output_access: OutputAccessMode,
    ) -> Result<Self, String> {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let exit_status = WorkerExitStatus::new();
        let worker_exit_status = exit_status.clone();
        // Wrapper-created meter atomics, shared with the runtime so the
        // wrapper retains the peak past thread exit for the death fold.
        let shared_output_peak_bits = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let retained_output_peak_bits = Arc::clone(&shared_output_peak_bits);
        let shared_clipped_sample_count = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let thread_handle = std::thread::Builder::new()
            .name("playback".to_string())
            .spawn(move || {
                let error_tx = event_tx.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_playback_thread(
                        message_rx,
                        command_rx,
                        event_tx,
                        sample_rate,
                        buffer_ms,
                        channels,
                        frame_size,
                        output_device,
                        recycle_tx,
                        allow_virtual_output,
                        output_access,
                        shared_output_peak_bits,
                        shared_clipped_sample_count,
                        startup_tx,
                    )
                }));
                match &result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        log::error!("[Playback Thread] Error: {}", e);
                        send_playback_event(
                            &error_tx,
                            ThreadEvent::ProcessingError(format!("Playback thread error: {e}")),
                            "thread error",
                        );
                    }
                    Err(_) => {
                        log::error!("[Playback Thread] Panicked");
                        send_playback_event(
                            &error_tx,
                            ThreadEvent::ThreadPanic("playback".to_string()),
                            "thread panic",
                        );
                    }
                }
                record_worker_exit(&worker_exit_status, &result);
            })
            .map_err(|e| format!("Failed to spawn playback thread: {}", e))?;

        match startup_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self {
                command_tx,
                thread_handle: Some(thread_handle),
                pending_stop_acks: PendingStopAcks::default(),
                exit_status,
                retained_output_peak_bits,
            }),
            Ok(Err(err)) => {
                let _ = thread_handle.join();
                Err(err)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = thread_handle.join();
                Err("Playback thread exited during startup".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                drop(command_tx);
                if super::join_timeout(thread_handle, std::time::Duration::from_secs(1)).is_err() {
                    log::warn!(
                        "[Playback Thread] Startup timed out and worker did not exit within 1s; leaving it detached"
                    );
                }
                Err("Playback thread startup timed out after 10s".to_string())
            }
        }
    }

    /// Send a command to the playback thread
    pub fn send_command(&self, command: PlaybackCommand) -> Result<(), String> {
        self.command_tx
            .send(command)
            .map_err(|e| format!("Failed to send command: {}", e))
    }

    /// Stash a Stop acknowledgment receiver for late collection.
    pub(in crate::engine) fn stash_pending_stop_ack(
        &mut self,
        receiver: Receiver<PlaybackStopAck>,
    ) {
        self.pending_stop_acks.push(receiver);
    }

    /// Fold arrived late Stop acknowledgments into shared state.
    ///
    /// Called on Stop and on every manager tick; non-blocking.
    /// Stores only when a record actually folded.
    pub(in crate::engine) fn collect_ready_stop_acks(
        &mut self,
        state: &Arc<ArcSwap<AudioEngineState>>,
    ) {
        let mut new_state = (**state.load()).clone();
        if self.pending_stop_acks.collect_ready(&mut new_state) > 0 {
            state.store(Arc::new(new_state));
        }
    }

    pub(in crate::engine) fn reconfigure(
        &self,
        sample_rate: u32,
        channels: usize,
    ) -> Result<PlaybackConfiguration, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        let ticket = HostUpdateTicket::new();
        self.send_command(PlaybackCommand::Reconfigure(PlaybackReconfigureRequest {
            requested: PlaybackConfiguration {
                sample_rate,
                channels,
            },
            ticket: ticket.clone(),
            reply_tx,
        }))?;

        match reply_rx.recv_timeout(PLAYBACK_RECONFIGURE_TIMEOUT) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("Playback reconfiguration reply channel disconnected".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if ticket.cancel() => Err(format!(
                "Playback reconfiguration timed out and was cancelled before installation after {}ms",
                PLAYBACK_RECONFIGURE_TIMEOUT.as_millis()
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                loop {
                    match reply_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                        Ok(result) => break result,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            break Err("Playback reconfiguration worker exited before replying"
                                .to_string());
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) if self.is_finished() => {
                            break Err("Playback reconfiguration worker stopped before replying"
                                .to_string());
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            }
        }
    }

    /// Whether the worker has stopped before the manager requested shutdown.
    pub fn is_finished(&self) -> bool {
        self.thread_handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
    }

    /// Load the recorded exit disposition (`None` means slot corruption).
    pub(crate) fn exit_status(&self) -> Option<WorkerExit> {
        self.exit_status.load()
    }

    /// Clone the wrapper-retained shared output-peak atomic.
    ///
    /// The runtime accumulates into this same atomic on every path (initial
    /// stream and all rebuilds), so the clone stays valid past thread exit
    /// for the worker-death residual fold.
    pub(crate) fn retained_output_peak_bits(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.retained_output_peak_bits)
    }

    /// Shutdown the playback thread
    pub fn shutdown(&mut self) {
        if let Err(e) = self.send_command(PlaybackCommand::Shutdown) {
            log::trace!("[Playback Thread] Shutdown command receiver dropped: {}", e);
        }
        if let Some(handle) = self.thread_handle.take()
            && super::join_timeout(handle, std::time::Duration::from_secs(5)).is_err()
        {
            log::warn!("[Playback Thread] Shutdown join timed out; thread left detached");
        }
    }
}

impl Drop for PlaybackThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}
