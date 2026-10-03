// ============================================================================
// Audio Engine Types
// ============================================================================

use crate::decoder::AudioSource;
use sotf_plugins::PluginHost;
use sotf_plugins::plugin_linear_phase_eq::BandConfig;
use sotf_plugins::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqControlStatus;
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};

// Re-export shared types from the engine types module.
pub use crate::{
    AudioEngineState, AudioFrame, DsdOutputMode, DsdOutputStatus, EngineOversamplingPolicy,
    IsolatedExternalPluginSandboxBackend, IsolatedExternalPluginSandboxStatus,
    IsolatedExternalPluginWorkerEvent, IsolatedExternalPluginWorkerStatus, LatencyCompensationMode,
    NetworkEndpointConfig, NetworkEndpointMode, NetworkEndpointStatus, OutputAccessMode,
    OutputAccessStatus, PLUGIN_BUILD_DIAGNOSTIC_PREFIX, PlaybackState, PluginBuildDiagnostic,
    PluginBuildTarget, PluginConfig, PluginGraphConfig, PluginGraphEdgeConfig, PluginGraphEdgeKind,
    PluginGraphNodeConfig, StreamMetadata,
};

// ============================================================================
// Plugin Data Cache - Lock-free(ish) shared cache for analyzer data
// ============================================================================

/// One snapshot of plugin analyzer data (one slot per plugin in the chain).
pub type PluginDataVec = Vec<Option<Arc<dyn Any + Send + Sync>>>;

/// Shared cache for plugin analyzer data.
/// The processing thread writes after each frame; the UI reads without
/// blocking the audio pipeline via lock-free ArcSwap.
pub type PluginDataCache = Arc<arc_swap::ArcSwap<PluginDataVec>>;

/// A complete plugin-host replacement prepared away from the processing thread.
///
/// Besides the built host, this owns every heap-backed object needed to commit
/// the change: analyzer-cache storage and both possible latency-alignment delay
/// lines. The processing thread only validates the base snapshot and moves
/// these allocations into its active state.
pub struct PreparedHostUpdate {
    pub(super) generation: u64,
    pub(super) host: Box<PluginHost>,
    pub(super) expected_output_channels: usize,
    pub(super) expected_latency_samples: usize,
    pub(super) output_channels: usize,
    pub(super) output_sample_rate: u32,
    pub(super) latency_samples: usize,
    pub(super) analyzer_cache: Arc<PluginDataVec>,
    pub(super) old_path_delay: PreparedTransitionDelay,
    pub(super) new_path_delay: PreparedTransitionDelay,
    pub(super) ticket: HostUpdateTicket,
}

const HOST_UPDATE_PENDING: u8 = 0;
const HOST_UPDATE_COMMITTED: u8 = 1;
const HOST_UPDATE_CANCELLED: u8 = 2;
const HOST_UPDATE_EXECUTING: u8 = 3;
const HOST_UPDATE_COMPLETED: u8 = 4;

#[derive(Clone, Debug)]
pub(super) struct HostUpdateTicket(std::sync::Arc<AtomicU8>);

impl HostUpdateTicket {
    pub(super) fn new() -> Self {
        Self(std::sync::Arc::new(AtomicU8::new(HOST_UPDATE_PENDING)))
    }

    pub(super) fn try_commit(&self) -> bool {
        self.0
            .compare_exchange(
                HOST_UPDATE_PENDING,
                HOST_UPDATE_COMMITTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Claim a potentially blocking operation without publishing its result.
    /// Playback stream creation remains cancellable until the new stream is
    /// ready to be installed atomically.
    pub(super) fn try_begin_execution(&self) -> bool {
        self.0
            .compare_exchange(
                HOST_UPDATE_PENDING,
                HOST_UPDATE_EXECUTING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn try_complete_execution(&self) -> bool {
        self.0
            .compare_exchange(
                HOST_UPDATE_EXECUTING,
                HOST_UPDATE_COMPLETED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Cancel work that has not made its result externally visible. Host swaps
    /// commit in one step; playback builds may also be cancelled while the new
    /// stream is being prepared, before installation.
    pub(super) fn cancel(&self) -> bool {
        loop {
            let state = self.0.load(Ordering::Acquire);
            match state {
                HOST_UPDATE_PENDING | HOST_UPDATE_EXECUTING => {
                    if self
                        .0
                        .compare_exchange(
                            state,
                            HOST_UPDATE_CANCELLED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return true;
                    }
                }
                HOST_UPDATE_CANCELLED => return true,
                HOST_UPDATE_COMMITTED | HOST_UPDATE_COMPLETED => return false,
                _ => return false,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackConfiguration {
    pub(super) sample_rate: u32,
    pub(super) channels: usize,
}

#[derive(Clone, Debug)]
pub struct PlaybackReconfigureRequest {
    pub(super) requested: PlaybackConfiguration,
    pub(super) ticket: HostUpdateTicket,
    pub(super) reply_tx: std::sync::mpsc::SyncSender<Result<PlaybackConfiguration, String>>,
}

impl PreparedHostUpdate {
    /// Validate and prepare a host replacement on a control/worker thread.
    pub fn prepare(
        host: PluginHost,
        input_sample_rate: u32,
        expected_output_channels: usize,
        expected_latency_samples: usize,
    ) -> Result<Self, String> {
        let output_channels = host.output_channels();
        if output_channels == 0 {
            return Err("prepared plugin host must expose at least one output channel".into());
        }
        if input_sample_rate == 0 {
            return Err("prepared plugin host requires a non-zero input sample rate".into());
        }
        let output_sample_rate = host.output_sample_rate(input_sample_rate);
        if output_sample_rate == 0 {
            return Err("prepared plugin host must expose a non-zero output sample rate".into());
        }
        let latency_samples = host.total_latency_samples();
        let delay_frames = latency_samples.abs_diff(expected_latency_samples);
        let delay_len = delay_frames
            .checked_mul(output_channels.max(expected_output_channels))
            .ok_or_else(|| "prepared host transition delay capacity overflow".to_string())?;
        let (old_path_delay, new_path_delay) = if expected_latency_samples < latency_samples {
            (
                PreparedTransitionDelay::new(delay_len),
                PreparedTransitionDelay::default(),
            )
        } else {
            (
                PreparedTransitionDelay::default(),
                PreparedTransitionDelay::new(delay_len),
            )
        };
        let analyzer_cache = Arc::new(vec![None; host.plugin_count()]);

        Ok(Self {
            generation: 0,
            host: Box::new(host),
            expected_output_channels,
            expected_latency_samples,
            output_channels,
            output_sample_rate,
            latency_samples,
            analyzer_cache,
            old_path_delay,
            new_path_delay,
            ticket: HostUpdateTicket::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn prepared_analyzer_slots(&self) -> usize {
        self.analyzer_cache.len()
    }

    pub(super) fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    pub(super) fn ticket(&self) -> HostUpdateTicket {
        self.ticket.clone()
    }
}

impl std::fmt::Debug for PreparedHostUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedHostUpdate")
            .field("generation", &self.generation)
            .field("expected_output_channels", &self.expected_output_channels)
            .field("expected_latency_samples", &self.expected_latency_samples)
            .field("output_channels", &self.output_channels)
            .field("output_sample_rate", &self.output_sample_rate)
            .field("latency_samples", &self.latency_samples)
            .finish_non_exhaustive()
    }
}

/// Preallocated interleaved sample delay used only during a host transition.
#[derive(Default)]
pub struct PreparedTransitionDelay {
    samples: Vec<f32>,
    cursor: usize,
}

impl PreparedTransitionDelay {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; len],
            cursor: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn process_in_place(&mut self, block: &mut [f32]) {
        if self.samples.is_empty() {
            return;
        }
        for sample in block {
            std::mem::swap(sample, &mut self.samples[self.cursor]);
            self.cursor += 1;
            if self.cursor == self.samples.len() {
                self.cursor = 0;
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
    }
}

#[cfg(test)]
mod prepared_host_update_tests {
    use super::PreparedTransitionDelay;

    #[test]
    fn transition_delay_is_sample_exact_across_block_partitions() {
        let mut delay = PreparedTransitionDelay::new(3);
        let mut first = [1.0, 2.0];
        let mut second = [3.0, 4.0, 5.0];

        delay.process_in_place(&mut first);
        delay.process_in_place(&mut second);

        assert_eq!(first, [0.0, 0.0]);
        assert_eq!(second, [0.0, 1.0, 2.0]);
    }

    #[test]
    fn transition_delay_reset_discards_buffered_samples() {
        let mut delay = PreparedTransitionDelay::new(3);
        delay.process_in_place(&mut [1.0, 2.0]);
        delay.reset();
        let mut output = [3.0, 4.0, 5.0, 6.0];
        delay.process_in_place(&mut output);
        assert_eq!(output, [0.0, 0.0, 0.0, 3.0]);
    }
}

#[cfg(test)]
mod transport_ack_tests {
    use super::{AudioEngineState, DecoderCommand};
    use super::{PendingStopAcks, PlaybackStopAck, apply_stop_ack, emits_stream_flush};
    use crate::decoder::AudioSource;

    fn file_source() -> AudioSource {
        AudioSource::File(std::path::PathBuf::from("/tmp/t14.wav"))
    }

    #[test]
    fn stream_flush_predicate_covers_all_ten_decoder_commands() {
        // Every Flush-emitting arm (top-of-loop plus both interrupt
        // mirrors) sends exactly one Flush for these four, before
        // attempting its work — even on attempt failure.
        assert!(emits_stream_flush(&DecoderCommand::Play(file_source(), 7)));
        assert!(emits_stream_flush(&DecoderCommand::PlayAt(
            file_source(),
            1.0,
            7
        )));
        assert!(emits_stream_flush(&DecoderCommand::Seek(2.0)));
        assert!(emits_stream_flush(&DecoderCommand::Stop));
        // All remaining commands emit no Flush in any arm.
        assert!(!emits_stream_flush(&DecoderCommand::Pause));
        assert!(!emits_stream_flush(&DecoderCommand::Resume));
        assert!(!emits_stream_flush(&DecoderCommand::QueueNext(
            file_source()
        )));
        assert!(!emits_stream_flush(&DecoderCommand::CancelNext));
        assert!(!emits_stream_flush(&DecoderCommand::StartSilentSource(2)));
        assert!(!emits_stream_flush(&DecoderCommand::Shutdown));
    }

    #[test]
    fn stop_ack_folds_on_epoch_match_and_drops_stale() {
        let mut state = AudioEngineState {
            playback_epoch: 5,
            playback_peak_max_linear: 0.30,
            ..AudioEngineState::default()
        };
        assert!(apply_stop_ack(
            &mut state,
            &PlaybackStopAck {
                epoch: 5,
                epoch_peak_max: 0.90,
            }
        ));
        assert_eq!(state.playback_peak_max_linear, 0.90);
        // Quieter ack never lowers the latch.
        assert!(apply_stop_ack(
            &mut state,
            &PlaybackStopAck {
                epoch: 5,
                epoch_peak_max: 0.10,
            }
        ));
        assert_eq!(state.playback_peak_max_linear, 0.90);
        // Stale epoch: dropped without effect.
        assert!(!apply_stop_ack(
            &mut state,
            &PlaybackStopAck {
                epoch: 4,
                epoch_peak_max: 1.00,
            }
        ));
        assert_eq!(state.playback_peak_max_linear, 0.90);
    }

    #[test]
    fn pending_stash_holds_unready_and_folds_late_without_loss() {
        let mut state = AudioEngineState {
            playback_epoch: 5,
            playback_peak_max_linear: 0.30,
            ..AudioEngineState::default()
        };
        let (unready_tx, unready_rx) = std::sync::mpsc::sync_channel(1);
        let (late_tx, late_rx) = std::sync::mpsc::sync_channel(1);
        let mut stash = PendingStopAcks::default();
        stash.push(unready_rx);
        stash.push(late_rx);
        // Nothing arrived: both stay stashed, nothing folds.
        assert_eq!(stash.collect_ready(&mut state), 0);
        assert_eq!(stash.len(), 2);
        assert_eq!(state.playback_peak_max_linear, 0.30);
        // Late arrival folds eventually; the unready entry stays.
        late_tx
            .send(PlaybackStopAck {
                epoch: 5,
                epoch_peak_max: 0.90,
            })
            .unwrap();
        assert_eq!(stash.collect_ready(&mut state), 1);
        assert_eq!(stash.len(), 1);
        assert_eq!(state.playback_peak_max_linear, 0.90);
        // Worker death without reply drops the entry, never wedges.
        drop(unready_tx);
        assert_eq!(stash.collect_ready(&mut state), 0);
        assert_eq!(stash.len(), 0);
    }

    #[test]
    fn pending_stash_drops_stale_epoch_without_contamination() {
        let mut state = AudioEngineState {
            playback_epoch: 6,
            playback_peak_max_linear: 0.0,
            ..AudioEngineState::default()
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut stash = PendingStopAcks::default();
        stash.push(rx);
        tx.send(PlaybackStopAck {
            epoch: 5,
            epoch_peak_max: 0.90,
        })
        .unwrap();
        // Superseded by a new epoch: collected, dropped, released.
        assert_eq!(stash.collect_ready(&mut state), 0);
        assert!(stash.is_empty());
        assert_eq!(state.playback_peak_max_linear, 0.0);
    }
}

// ============================================================================
// Queue Messages - Messages passed through queues
// ============================================================================

/// Messages sent from decoder to processing
#[derive(Clone, Debug)]
pub enum DecoderMessage {
    /// Audio frame
    Frame(AudioFrame),
    /// End of stream reached
    EndOfStream,
    /// Flush the queue (used during seek)
    ///
    /// Ordering invariant (load-bearing for playback-epoch attribution):
    /// every decoder command that resets stream position (Play, PlayAt,
    /// Seek, Stop) sends this BEFORE its acknowledgement, so downstream
    /// stages observe the boundary before the manager acts on the ack.
    /// The Play/PlayAt sites are the epoch-fence dependency: the manager
    /// sends Resume only after the ack, so Flush always precedes the
    /// fence it must precede.
    ///
    /// Counting contract (load-bearing for the flush-generation gate):
    /// every stream command arm sends exactly one Flush per processed
    /// command, in all three decoder sites (top-of-loop plus both
    /// interrupt mirrors), before attempting the command's work — even
    /// on attempt failure. No other decoder path emits Flush (frames
    /// and EOS travel the interruptible sender; gapless transitions
    /// emit neither). The manager counts one per SEND-OK of
    /// [`emits_stream_flush`] commands, and playback counts one per
    /// consumed Flush (including rebuild-swallowed ones), so the
    /// playback count never exceeds the manager count while both
    /// workers live.
    Flush,
}

/// Messages sent from processing to playback
#[derive(Clone, Debug)]
pub enum ProcessingMessage {
    /// Processed audio frame
    Frame(AudioFrame),
    /// End of stream reached
    EndOfStream,
    /// Flush the queue
    Flush,
}

// ============================================================================
// Control Commands - Commands sent to threads
// ============================================================================

/// Decode-session identity, tagging Play/PlayAt commands and async errors.
///
/// The manager assigns `current + 1` at every Play/PlayAt SEND-OK
/// (including seeks-that-reopen and ack timeouts — send-ok means
/// queued means the decoder will adopt the tag) and persists it
/// immediately; the decoder adopts the carried tag at every arm
/// start (all six sites), before the fallible open, so NACK paths
/// stay converged too. Gapless transitions, silent source,
/// pause/resume, seeks-within-source, and stops keep the current
/// tag (same session). Never reset: fresh engine = fresh threads =
/// 0 both sides, and restore never resurrects a live session.
/// Monotonic; gaps (send failure after tag choice) are harmless —
/// only equality is ever tested.
pub type DecodeAttempt = u64;

/// Asynchronous decoder failure, delivered reliably to the manager.
///
/// Emitted by the decoder worker loop's async `Err` arms (mid-phase
/// decode failure, HAL input failure, queue-stuck) over a dedicated
/// unbounded channel the manager tick drains. `attempt` is the
/// decoder's adopted tag at failure time; the manager applies the
/// message iff it equals the current attempt (stale tags belong to
/// superseded sessions and are dropped with a warn log). Atomic
/// messages: no torn reads, no loss, no new threads.
#[derive(Clone, Debug, PartialEq)]
pub struct DecoderAsyncError {
    /// Decode-session identity at failure time.
    pub attempt: DecodeAttempt,
    /// Root-cause message (preserved verbatim for diagnosis).
    pub message: String,
}

/// Commands for the decoder thread
#[derive(Clone, Debug)]
pub enum DecoderCommand {
    /// Start playing an audio source, tagged with its decode attempt.
    Play(AudioSource, DecodeAttempt),
    /// Start playing an audio source at a specific position in seconds,
    /// tagged with its decode attempt.
    PlayAt(AudioSource, f64, DecodeAttempt),
    /// Start silent source (for HAL input plugins).
    /// Sends empty frames at regular intervals for source plugins using the
    /// configured pipeline input channel count.
    StartSilentSource(usize),
    /// Pause decoding
    Pause,
    /// Resume decoding
    Resume,
    /// Seek to position in seconds
    Seek(f64),
    /// Queue the next source for gapless playback.
    /// When the current source ends, the decoder seamlessly transitions to this source
    /// without sending EndOfStream or Flush, avoiding any gap in audio output.
    QueueNext(AudioSource),
    /// Cancel a previously queued next source.
    CancelNext,
    /// Stop decoding and cleanup
    Stop,
    /// Shutdown the thread
    Shutdown,
}

/// Whether a decoder command emits a stream Flush.
///
/// True exactly for the commands whose every arm (top-of-loop plus
/// both interrupt mirrors) sends one [`DecoderMessage::Flush`] before
/// attempting its work: Play, PlayAt, Seek, Stop. All other commands
/// (Pause, Resume, QueueNext, CancelNext, StartSilentSource,
/// Shutdown) emit none. The manager funnels every send of a true
/// command through the counted-send helper so `flushes_sent` stays
/// exact; any new Flush-emitting command must extend this predicate
/// (and its exhaustive test) first.
pub(in crate::engine) fn emits_stream_flush(command: &DecoderCommand) -> bool {
    matches!(
        command,
        DecoderCommand::Play(_, _)
            | DecoderCommand::PlayAt(_, _, _)
            | DecoderCommand::Seek(_)
            | DecoderCommand::Stop
    )
}

#[derive(Clone, Debug)]
pub enum DecoderResponse {
    Ok,
    Error(String),
}

/// Commands for the processing thread
pub enum ProcessingCommand {
    /// Commit a fully validated and allocation-prepared plugin-host replacement.
    CommitHostUpdate(PreparedHostUpdate),
    /// Set a plugin parameter
    SetParameter {
        plugin_index: usize,
        param_id: String,
        value: String, // Generic string value (JSON for complex types, or stringified primitives)
    },
    /// Bypass all processing (pass-through)
    Bypass(bool),
    /// Query plugin data (e.g. analyzer results)
    GetPluginData(usize),
    /// Poll isolated external plugin workers for process lifecycle events
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    PollIsolatedExternalPluginWorkers,
    /// Stop processing
    Stop,
    /// Shutdown the thread
    Shutdown,
}

impl std::fmt::Debug for ProcessingCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommitHostUpdate(update) => {
                f.debug_tuple("CommitHostUpdate").field(update).finish()
            }
            Self::SetParameter {
                plugin_index,
                param_id,
                value,
            } => f
                .debug_struct("SetParameter")
                .field("plugin_index", plugin_index)
                .field("param_id", param_id)
                .field("value", value)
                .finish(),
            Self::Bypass(bypass) => f.debug_tuple("Bypass").field(bypass).finish(),
            Self::GetPluginData(index) => f.debug_tuple("GetPluginData").field(index).finish(),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            Self::PollIsolatedExternalPluginWorkers => {
                write!(f, "PollIsolatedExternalPluginWorkers")
            }
            Self::Stop => write!(f, "Stop"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Response from processing thread
#[derive(Clone, Debug)]
pub enum ProcessingResponse {
    /// Ok response
    Ok,
    /// Plugin chain updated with new output channel count and latency
    PluginChainUpdated {
        generation: u64,
        output_channels: usize,
        output_sample_rate: u32,
        previous_latency_samples: usize,
        latency_samples: usize,
        latency_changed: bool,
    },
    /// A parameter was applied and the host metadata was re-queried.
    ParameterUpdated {
        output_channels: usize,
        output_sample_rate: u32,
        latency_samples: usize,
    },
    /// Plugin data
    PluginData(Arc<dyn Any + Send + Sync>),
    /// Error
    Error(String),
}

/// Commands for the playback thread
#[derive(Clone, Debug)]
pub enum PlaybackCommand {
    /// Set output volume (linear, 0.0 = silence, 1.0 = unity)
    SetVolume(f32),
    /// Mute/unmute
    Mute(bool),
    /// Discard buffered audio and hold incoming frames until resumed.
    Pause,
    /// Resume accepting frames after a pause once the callback-side flush completes.
    ///
    /// Carries the manager's playback epoch: a changed epoch fences the
    /// previous stream (residual reset, EOS flags cleared); an unchanged
    /// epoch resumes within the same epoch (pause/resume, rollback).
    Resume { epoch: u64 },
    /// Update output channel count (requires rebuilding stream)
    UpdateChannels(usize),
    /// Update output sample rate (requires rebuilding stream)
    UpdateSampleRate(u32),
    /// Atomically rebuild the output for a processing-host topology change and
    /// report the actual hardware configuration before the manager publishes it.
    Reconfigure(PlaybackReconfigureRequest),
    /// Stop playback with a completion acknowledgment.
    ///
    /// The worker latches a callback emission cutoff, discards the
    /// ring, and replies on the carried channel once quiesce (ring
    /// empty plus callback inactive) is observed; the reply carries
    /// the terminal peak record. A superseding Stop or new-epoch
    /// Resume finalizes a still-pending acknowledgment first, so
    /// every Stop is answered exactly once on every worker path.
    Stop(PlaybackStopRequest),
    /// Shutdown the thread
    Shutdown,
}

/// Stop request carrying its completion channel.
///
/// Mirrors [`PlaybackReconfigureRequest`]: the reply channel travels
/// with the command so the acknowledgment cannot be dropped or
/// misattributed. Capacity one never blocks the worker's single
/// reply; the manager waits with a timeout and stashes the receiver
/// for late collection instead.
#[derive(Clone, Debug)]
pub struct PlaybackStopRequest {
    pub(super) reply_tx: SyncSender<PlaybackStopAck>,
}

impl PlaybackStopRequest {
    /// Pair a Stop request with its acknowledgment receiver.
    pub fn new() -> (Self, Receiver<PlaybackStopAck>) {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        (Self { reply_tx }, reply_rx)
    }
}

impl Default for PlaybackStopRequest {
    /// Discard the acknowledgment; for tests that ignore completion.
    fn default() -> Self {
        Self::new().0
    }
}

/// Terminal peak record answering a playback Stop.
///
/// `epoch` is the playback-side stream generation the record belongs
/// to; the manager folds `epoch_peak_max` only on epoch match. The
/// record covers every callback-observed sample through the terminal
/// swap, including windows whose periodic reports were dropped. In a
/// same-epoch supersede it may additionally cover post-cutoff
/// windows (ceiling-safe superset; unreachable in current manager
/// flows, which serialize Stop before later commands).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackStopAck {
    pub epoch: u64,
    pub epoch_peak_max: f32,
}

/// Fold a Stop acknowledgment into transport state.
///
/// Returns true when the ack's epoch matches (peak folded into the
/// latch via max); stale epochs are dropped without effect. Never
/// transitions transport: completion and peak recovery stay separate
/// so a queued drained receipt and a Stop ack compose in any order.
pub(in crate::engine) fn apply_stop_ack(
    state: &mut AudioEngineState,
    ack: &PlaybackStopAck,
) -> bool {
    if ack.epoch != state.playback_epoch {
        log::debug!(
            "[Manager Thread] Dropping stale Stop ack for epoch {} (current {})",
            ack.epoch,
            state.playback_epoch
        );
        return false;
    }
    state.playback_peak_max_linear = state.playback_peak_max_linear.max(ack.epoch_peak_max);
    true
}

/// Receivers awaiting late Stop acknowledgments.
///
/// The manager stashes one receiver per timed-out Stop wait; the
/// worker always replies eventually (quiesce, supersede, or shutdown
/// finalization), so collection degrades timeliness, never
/// correctness. At most one entry per Stop; entries leave on reply
/// or worker death, so the stash cannot grow without bound.
#[derive(Debug, Default)]
pub(in crate::engine) struct PendingStopAcks {
    receivers: Vec<Receiver<PlaybackStopAck>>,
}

impl PendingStopAcks {
    /// Stash a receiver whose synchronous wait timed out.
    pub(in crate::engine) fn push(&mut self, receiver: Receiver<PlaybackStopAck>) {
        self.receivers.push(receiver);
    }

    /// Fold every arrived acknowledgment; drop dead receivers.
    ///
    /// Returns the number of folded records. Non-blocking: unready
    /// receivers stay stashed for a later tick.
    pub(in crate::engine) fn collect_ready(&mut self, state: &mut AudioEngineState) -> usize {
        let mut folded = 0;
        let mut index = 0;
        while index < self.receivers.len() {
            match self.receivers[index].try_recv() {
                Ok(ack) => {
                    if apply_stop_ack(state, &ack) {
                        log::debug!(
                            "[Manager Thread] Collected late Stop ack for epoch {} (peak {})",
                            ack.epoch,
                            ack.epoch_peak_max
                        );
                        folded += 1;
                    }
                    self.receivers.swap_remove(index);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    index += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    log::debug!("[Manager Thread] Stop ack receiver disconnected; dropping stash");
                    self.receivers.swap_remove(index);
                }
            }
        }
        folded
    }

    /// Count stashed receivers (tests and diagnostics only).
    #[cfg(test)]
    pub(in crate::engine) fn len(&self) -> usize {
        self.receivers.len()
    }

    /// Whether no receiver is stashed (tests only).
    #[cfg(test)]
    pub(in crate::engine) fn is_empty(&self) -> bool {
        self.receivers.is_empty()
    }
}

/// Commands for the manager thread
#[derive(Clone, Debug)]
pub enum ManagerCommand {
    // Playback control
    Play(AudioSource),
    /// Play a source starting at a specific position
    PlayAt(AudioSource, f64),
    Pause,
    Resume,
    Stop,
    Seek(f64),
    /// Queue the next source for gapless playback.
    /// When the current track ends, the decoder seamlessly starts the queued source
    /// without any gap in audio output.
    QueueNext(AudioSource),
    /// Cancel a previously queued next source.
    CancelNext,

    // Volume control
    SetVolume(f32),
    Mute(bool),

    // Plugin control
    UpdatePluginChain(Vec<PluginConfig>),
    UpdatePluginGraph(PluginGraphConfig),
    SetPluginParameter {
        plugin_index: usize,
        param_id: String,
        value: String, // Generic string value (JSON for complex types, or stringified primitives)
    },
    BypassProcessing(bool),
    /// Snapshot, prepare and queue one linear-phase EQ band-shape edit.
    ///
    /// The manager thread snapshots the accepted base from the shared
    /// wrapper handle, prepares FIR design off audio, and queues the bounded
    /// payload; real audio commits the existing crossfade. `Ok` means
    /// queued, not accepted: observe acceptance via `LinearPhaseEqStatus`.
    LinearPhaseEqRequest {
        plugin_index: usize,
        band_index: usize,
        new_band: BandConfig,
    },
    /// Request eviction of a wedged linear-phase EQ head payload.
    LinearPhaseEqCancel {
        plugin_index: usize,
    },
    /// Read detached linear-phase EQ accepted generation and queue status.
    LinearPhaseEqStatus {
        plugin_index: usize,
    },

    /// Poll isolated external plugin worker status without starting or restarting workers.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    MaintainIsolatedExternalPluginWorkers,

    // Queries
    GetState,
    GetPosition,
    GetPluginData(usize),

    // Lifecycle
    ReloadConfig,
    Shutdown,
}

/// Response from manager thread
#[allow(
    clippy::large_enum_variant,
    reason = "boxing the state response would change the manager API and allocation behavior"
)]
#[derive(Clone)]
pub enum ManagerResponse {
    Ok,
    State(AudioEngineState),
    Position(f64),
    PluginData(Arc<dyn Any + Send + Sync>),
    LinearPhaseEqStatus(LinearPhaseEqControlStatus),
    Error(String),
    Shutdown,
}

// ============================================================================
// Thread Events - Events sent from threads to manager
// ============================================================================

/// Events sent from worker threads to manager
#[derive(Clone, Debug)]
pub enum ThreadEvent {
    /// Decoder reached end of stream
    DecoderEndOfStream,
    /// Decoder seamlessly transitioned to a queued next source (gapless playback)
    DecoderGaplessTransition(AudioSource),
    /// Decoder error
    DecoderError(String),
    /// Live stream metadata update (ICY/content-type/bitrate).
    StreamMetadataChanged(Option<StreamMetadata>),
    PlaybackChannelsChanged(usize),
    PlaybackOutputDeviceChanged(String),
    PlaybackOutputAccessChanged(crate::OutputAccessStatus),
    /// Playback thread hardware-consumption diagnostics changed.
    ///
    /// `epoch` is the playback-side stream generation; the manager applies
    /// only snapshots whose epoch matches the current playback epoch.
    PlaybackStats {
        callback_count: u64,
        buffer_fill_percent: u64,
        stream_error_count: u64,
        frames_received: u64,
        frames_written: u64,
        frames_dropped: u64,
        effective_sample_rate: u64,
        epoch: u64,
    },
    /// Post-volume, pre-clamp output level for the most recent meter window.
    ///
    /// `epoch` is the playback-side stream generation; the manager latches
    /// only snapshots whose epoch matches the current playback epoch.
    PlaybackOutputMeter {
        peak_linear: f32,
        clipping_detected: bool,
        epoch: u64,
    },
    /// Playback thread has fully drained its ring buffer after end-of-stream.
    ///
    /// `epoch` is the playback-side stream generation; the manager honors
    /// only receipts for the current playback epoch. `epoch_peak_max` is the
    /// lossless per-epoch peak over every swapped meter window (periodic,
    /// flush, and terminal), so the manager recovers windows whose periodic
    /// reports were dropped in transport; the stub likewise reports its
    /// cumulative peak over every swapped window (P4 parity — no periodic
    /// reports exist on the stub, so drains and Stop terminals carry it).
    /// `flush_gen` is the playback-side count of consumed stream Flushes at
    /// birth; the manager transitions transport only when it has sent no
    /// newer Flush (`gen >= sent`), while the peak folds on every epoch
    /// match regardless of generation.
    PlaybackDrained {
        epoch: u64,
        epoch_peak_max: f32,
        flush_gen: u64,
    },
    /// Playback buffer underrun count update
    PlaybackUnderrun(u64),
    /// Processing error (fatal — sets PlaybackState::Stopped)
    ProcessingError(String),
    /// Non-fatal processing warning (sets last_error but does NOT change playback state)
    ProcessingWarning(String),
    /// Thread panicked
    ThreadPanic(String),
    /// Position update
    PositionUpdate(f64),
    /// Seek completed
    SeekComplete,
    /// Plugin chain total latency changed (in samples at the processing sample rate)
    PluginLatencyUpdate(usize),
    /// Isolated external plugin worker status snapshot.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    IsolatedExternalPluginWorkerStatuses(Vec<IsolatedExternalPluginWorkerStatus>),
}
