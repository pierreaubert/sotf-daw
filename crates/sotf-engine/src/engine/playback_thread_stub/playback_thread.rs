#[cfg(target_os = "ios")]
use super::super::worker_death::record_worker_exit;
use super::super::worker_death::{WorkerExit, WorkerExitStatus};
use super::super::{
    AudioEngineState, HostUpdateTicket, PendingStopAcks, PlaybackCommand, PlaybackConfiguration,
    PlaybackReconfigureRequest, PlaybackStopAck,
};
#[cfg(target_os = "ios")]
use super::super::{ProcessingMessage, ThreadEvent};
#[cfg(target_os = "ios")]
use super::audio_unit_handle::run_playback_ios;
use arc_swap::ArcSwap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
#[cfg(target_os = "ios")]
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::{Receiver, Sender};

pub struct PlaybackThread {
    pub(super) command_tx: Sender<PlaybackCommand>,
    pub(super) thread_handle: Option<std::thread::JoinHandle<()>>,
    pub(super) pending_stop_acks: PendingStopAcks,
    pub(super) exit_status: WorkerExitStatus,
    /// Wrapper-held clone of the feeder's shared output-peak atomic.
    ///
    /// Created here and shared into the feeder state (which has no
    /// rebuild paths, so one share lasts forever), so the manager can
    /// fold the residual peak after a worker death even though the
    /// feeder is gone. Desktop `PlaybackThread` mirror.
    pub(super) retained_output_peak_bits: Arc<AtomicU32>,
}

impl PlaybackThread {
    #[cfg(target_os = "ios")]
    pub fn new(
        message_rx: Receiver<ProcessingMessage>,
        event_tx: crossbeam::channel::Sender<ThreadEvent>,
        sample_rate: u32,
        buffer_ms: u32,
        channels: usize,
        frame_size: usize,
        _output_device: Option<String>,
        recycle_tx: SyncSender<Vec<f32>>,
        _allow_virtual_output: bool,
        _output_access: crate::OutputAccessMode,
    ) -> Result<Self, String> {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let exit_status = WorkerExitStatus::new();
        let worker_exit_status = exit_status.clone();
        // Wrapper-created peak atomic, shared with the feeder state so
        // the wrapper retains the peak past thread exit for the death
        // fold (desktop mirror; no stub rebuild paths exist).
        let shared_output_peak_bits = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let retained_output_peak_bits = Arc::clone(&shared_output_peak_bits);

        let thread_handle = std::thread::Builder::new()
            .name("playback-ios".to_string())
            .spawn(move || {
                let error_tx = event_tx.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_playback_ios(
                        message_rx,
                        command_rx,
                        event_tx,
                        sample_rate,
                        buffer_ms,
                        channels,
                        frame_size,
                        recycle_tx,
                        shared_output_peak_bits,
                    )
                }));
                match &result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        log::error!("[Playback Thread iOS] Error: {error}");
                        error_tx
                            .try_send(ThreadEvent::ProcessingError(format!(
                                "iOS playback error: {error}"
                            )))
                            .ok();
                    }
                    Err(_) => {
                        log::error!("[Playback Thread iOS] Panicked");
                        error_tx
                            .try_send(ThreadEvent::ThreadPanic("playback-ios".to_string()))
                            .ok();
                    }
                }
                record_worker_exit(&worker_exit_status, &result);
            })
            .map_err(|e| format!("Failed to spawn playback thread: {}", e))?;

        Ok(Self {
            command_tx,
            thread_handle: Some(thread_handle),
            pending_stop_acks: PendingStopAcks::default(),
            exit_status,
            retained_output_peak_bits,
        })
    }

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
        self.await_reply(reply_rx, ticket)
    }

    fn await_reply(
        &self,
        reply_rx: Receiver<Result<PlaybackConfiguration, String>>,
        ticket: HostUpdateTicket,
    ) -> Result<PlaybackConfiguration, String> {
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
        match reply_rx.recv_timeout(TIMEOUT) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("iOS playback reconfiguration reply channel disconnected".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if ticket.cancel() => Err(
                "iOS playback reconfiguration timed out and was cancelled before completion"
                    .to_string(),
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => loop {
                match reply_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(result) => break result,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break Err("iOS playback worker exited before replying".to_string());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) if self.is_finished() => {
                        break Err("iOS playback worker stopped before replying".to_string());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
            },
        }
    }

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
    /// The feeder accumulates into this same atomic for its whole
    /// life (no rebuild paths), so the clone stays valid past thread
    /// exit for the worker-death residual fold. Desktop mirror.
    pub(crate) fn retained_output_peak_bits(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.retained_output_peak_bits)
    }

    pub fn shutdown(&mut self) {
        self.send_command(PlaybackCommand::Shutdown).ok();
        if let Some(handle) = self.thread_handle.take()
            && super::super::join_timeout(handle, std::time::Duration::from_secs(5)).is_err()
        {
            log::warn!("[Playback Thread iOS] Shutdown join timed out; thread left detached");
        }
    }
}

impl Drop for PlaybackThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}
