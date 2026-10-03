use super::super::{
    PlaybackCommand, PlaybackConfiguration, PlaybackReconfigureRequest, PlaybackStopAck,
    ProcessingMessage, ThreadEvent, plan_output_access,
};
use super::apply::apply_volume_clamp;
use super::build::build_output_stream;
#[cfg(target_os = "macos")]
use super::core_audio_exclusive_mode_guard::CoreAudioExclusiveModeGuard;
use super::coreaudio_mod::coreaudio_output_device_id;
use super::frame_writer::{FrameWriteOutcome, write_frame_to_ring};
use super::misc::SPIN_MS_RINGBUFFER;
use super::misc::initial_buffer_size;
use super::misc::is_virtual_output_device_name;
use super::misc::prefill_silence;
use super::misc::recycle_frame_data;
use super::misc::select_playback_device;
use super::misc::send_playback_event;
#[cfg(target_os = "macos")]
use super::misc::set_output_access_status;
use super::misc::snapshot_output_meter;
use super::pick::choose_output_format;
use super::playback::playback_buffer_capacity;
use super::playback::playback_recovery_reason;
use super::playback_state::PlaybackState;
use super::playback_state::flush_completed;
use super::playback_state::rebuild_playback_stream;
use super::playback_state::request_flush;
use super::playback_state::{copy_playback_controls, read_ring_buffer};
use super::types::FlushMode;
use super::types::{RebuildPlaybackParams, RebuiltPlaybackStream};
use crate::{OutputAccessMode, OutputAccessStatus, SinkType};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

/// Main playback thread function.
#[allow(
    clippy::too_many_arguments,
    reason = "thread entry point: one argument per playback runtime resource"
)]
pub(super) fn run_playback_thread(
    message_rx: Receiver<ProcessingMessage>,
    command_rx: Receiver<PlaybackCommand>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    sample_rate: u32,
    buffer_ms: u32,
    initial_channels: usize,
    frame_size: usize,
    output_device: Option<String>,
    recycle_tx: SyncSender<Vec<f32>>,
    allow_virtual_output: bool,
    output_access: OutputAccessMode,
    sink_type: SinkType,
    shared_output_peak_bits: Arc<AtomicU32>,
    shared_clipped_sample_count: Arc<AtomicU64>,
    startup_tx: SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let mut runtime = match PlaybackRuntime::new(PlaybackRuntimeParams {
        message_rx,
        command_rx,
        event_tx,
        sample_rate,
        buffer_ms,
        initial_channels,
        frame_size,
        output_device,
        recycle_tx,
        allow_virtual_output,
        output_access,
        sink_type,
        shared_output_peak_bits,
        shared_clipped_sample_count,
    }) {
        Ok(runtime) => runtime,
        Err(err) => {
            startup_tx.send(Err(err.clone())).ok();
            return Err(err);
        }
    };
    if startup_tx.send(Ok(())).is_err() {
        return Ok(());
    }
    runtime.run()
}

struct PlaybackRuntimeParams {
    message_rx: Receiver<ProcessingMessage>,
    command_rx: Receiver<PlaybackCommand>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    sample_rate: u32,
    buffer_ms: u32,
    initial_channels: usize,
    frame_size: usize,
    output_device: Option<String>,
    recycle_tx: SyncSender<Vec<f32>>,
    allow_virtual_output: bool,
    output_access: OutputAccessMode,
    sink_type: SinkType,
    shared_output_peak_bits: Arc<AtomicU32>,
    shared_clipped_sample_count: Arc<AtomicU64>,
}

struct PlaybackRuntime {
    message_rx: Receiver<ProcessingMessage>,
    command_rx: Receiver<PlaybackCommand>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    recycle_tx: SyncSender<Vec<f32>>,
    host: Option<cpal::Host>,
    output_device: Option<String>,
    allow_virtual_output: bool,
    // Read only by the macOS exclusive-mode recovery path.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    output_access: OutputAccessMode,
    output_access_status: OutputAccessStatus,
    #[cfg(target_os = "macos")]
    backend_exclusive_active: bool,
    #[cfg(target_os = "macos")]
    coreaudio_exclusive_mode: CoreAudioExclusiveModeGuard,
    device: Option<Device>,
    device_name: String,
    coreaudio_device_id: Option<u32>,
    stream: Option<Stream>,
    config: StreamConfig,
    output_format: Option<SampleFormat>,
    lab_output: Option<LabOutput>,
    channels: usize,
    logical_channels: usize,
    frame_size: usize,
    buffer_ms: u32,
    producer: Producer<f32>,
    state: Arc<PlaybackState>,
    buffer_capacity: usize,
    conversion_buffer: Vec<f32>,
    accounting: PlaybackAccounting,
    drain: DrainState,
    recovery: RecoveryState,
    diagnostics: DiagnosticState,
}

#[derive(Default)]
struct PlaybackAccounting {
    frames_received: u64,
    frames_written: u64,
    frames_dropped: u64,
    frames_blocked: u64,
    total_samples_written: u64,
}

struct DrainState {
    end_of_stream: bool,
    drain_start: Option<Instant>,
    drain_timeout: Duration,
    flush_mode: FlushMode,
    // Control commands and FIFO stream markers travel through different
    // queues. Pause/Resume must not erase an outstanding stream boundary.
    stream_flush_pending: bool,
    paused: bool,
    /// Playback-side playback generation; tags meter/drained events.
    meter_epoch: u64,
    /// Lossless per-epoch peak over every swapped meter window.
    ///
    /// Folded before each snapshot send (periodic, flush, terminal), so a
    /// dropped send still leaves its window in the record the drained
    /// receipt carries. Reset by the epoch fence; same-epoch pause, seek,
    /// and gapless playback accumulate into it.
    epoch_peak_max: f32,
    /// Stream Flushes consumed, tagging drained receipts.
    ///
    /// Global monotonic generation (never reset): advanced once per
    /// consumed [`ProcessingMessage::Flush`] (normal path and
    /// rebuild-swallowed alike), so it trails the manager's
    /// `flushes_sent` by exactly the in-flight count while both
    /// workers live.
    flushes_processed: u64,
    /// Stop completion awaiting callback quiesce, if any.
    ///
    /// At most one: a superseding Stop or new-epoch Resume finalizes
    /// the pending acknowledgment before proceeding, and shutdown
    /// finalizes best-effort, so every Stop is answered exactly once.
    pending_stop_ack: Option<SyncSender<PlaybackStopAck>>,
}

impl DrainState {
    fn begin_drop(&mut self, mode: FlushMode) {
        if matches!(mode, FlushMode::DroppingUntilFlush) {
            self.stream_flush_pending = true;
            self.paused = false;
        } else {
            self.paused = true;
        }
        self.end_of_stream = false;
        self.drain_start = None;
        self.update_flush_mode(false);
    }

    fn resume(&mut self, callback_flush_completed: bool) {
        self.paused = false;
        self.update_flush_mode(callback_flush_completed);
    }

    /// Adopt a playback epoch from a Resume command, fencing the old stream.
    ///
    /// On change only: stores the epoch, clears EOS/drain flags so no
    /// stale drained receipt is emitted into the new epoch, and resets
    /// the epoch cumulative peak (a superseded record, if any, already
    /// carries it in its acknowledgment — the R14 finalize-first).
    ///
    /// The callback residual resets too, UNLESS `finalized` reports a
    /// terminal swap just ran: a callback window racing between that
    /// swap and this fence would otherwise be wiped (in neither
    /// record), so it is kept and attributed to the new epoch instead.
    /// That misattribution is conservative — a ceiling-safe superset
    /// of whatever lands between swap and fence — and explicit; the
    /// old record stays complete-through-finalize. When no swap ran,
    /// the discarded tail (up to a full inter-snapshot window)
    /// belongs to an ABANDONED epoch: no receipt exists for it, by
    /// design — that, not smallness, is the justification.
    ///
    /// Metering-boundary definition (exactness condition): the
    /// metering cutoff for a superseded epoch IS the finalize swap —
    /// legitimate because nothing observable occurs between swap and
    /// fence (adjacent statements, single thread: no snapshot, tag,
    /// or event), so boundary placement inside that interval is
    /// free. R11's rule refines to "metered-before-finalize-swap"
    /// for superseded epochs, "metered-before-fence" otherwise. The
    /// manager latch stays consistent: no old-tagged snapshot exists
    /// post-swap, and post-supersede old receipts are epoch-dropped.
    /// Same-epoch resumes (pause/resume, rollback) are a no-op by
    /// comparison.
    fn adopt_playback_epoch(&mut self, meter: &PlaybackState, epoch: u64, finalized: bool) {
        if epoch == self.meter_epoch {
            return;
        }
        self.meter_epoch = epoch;
        if !finalized {
            meter.reset_output_meter();
        }
        self.epoch_peak_max = 0.0;
        self.end_of_stream = false;
        self.drain_start = None;
    }

    /// Fold one swapped meter window into the epoch cumulative peak.
    ///
    /// Called for every swapped snapshot (periodic, flush, terminal) before
    /// its event is sent, so a dropped send still leaves its window's peak
    /// in the record the drained receipt carries.
    fn note_meter_snapshot(&mut self, peak: f32) {
        self.epoch_peak_max = self.epoch_peak_max.max(peak);
    }

    /// Build the drained receipt carrying the epoch cumulative peak.
    ///
    /// Every drained send goes through here so the terminal record always
    /// carries the complete per-epoch max. Each drained site emits the
    /// terminal snapshot first, so the terminal window is already folded.
    /// The receipt is born with the current consumed-Flush count, so the
    /// manager can tell pre-boundary drains from legitimate ones.
    fn drained_event(&self) -> ThreadEvent {
        ThreadEvent::PlaybackDrained {
            epoch: self.meter_epoch,
            epoch_peak_max: self.epoch_peak_max,
            flush_gen: self.flushes_processed,
        }
    }

    /// Count one rebuild-swallowed Flush and invalidate armed drains.
    ///
    /// Stream rebuilds drain the message queue outright; each swallowed
    /// Flush still advanced the manager's count, so playback must count
    /// it too or trail forever. The swallowed boundary ALSO invalidates
    /// any armed drain — like a processed Flush (`stream_flushed`), a
    /// boundary means whatever follows re-drives its own terminal, so a
    /// stale armed EOS must not fire with a balanced generation (D7).
    /// Stop carve-out: a Stop boundary re-drives NOTHING — the StopAck
    /// IS its terminal — and needs none; invalidation stays correct
    /// (the armed drain belongs to a stream whose terminal is the ack,
    /// not a drain). Drop-until-Flush semantics are kept: only the
    /// next real Flush clears the pending boundary.
    fn note_swallowed_flush(&mut self) {
        self.flushes_processed = self.flushes_processed.wrapping_add(1);
        self.end_of_stream = false;
        self.drain_start = None;
    }

    /// Arm the drain for a rebuild-swallowed end-of-stream marker.
    ///
    /// A terminal marker must not vanish: the decoder generates EOS
    /// only at source exhaustion — every arm that abandons a stream
    /// emits Flush, never EOS (decoder Flush-before-attempt, all arms
    /// incl. mirrors) — so a swallowed EOS is a legitimate terminal
    /// the rebuild would otherwise wedge into stuck-Playing (A3).
    /// Safety when its stream was abandoned anyway: a Flush follows
    /// the marker (same guarantee) and invalidates the arm — in this
    /// batch, a later batch, or normal order — and N1 forward
    /// preservation means the boundary is never dropped. If no Flush
    /// ever follows, the resulting drain carries a stale generation
    /// and the manager holds transport instead of completing it.
    /// Sequential batch order keeps this sound: EOS-then-Flush nets
    /// cleared (matching normal order); Flush-then-EOS nets armed
    /// (the marker belongs to the post-boundary stream).
    fn note_swallowed_eos(&mut self) {
        self.end_of_stream = true;
        self.drain_start = Some(Instant::now());
    }

    /// Swap the callback residual and build the Stop terminal record.
    ///
    /// Report-then-clear: the swapped window folds into the epoch
    /// cumulative before the record is built, so the acknowledgment
    /// covers every callback-observed sample. Shared by the quiesce,
    /// supersede, and shutdown paths.
    fn take_stop_terminal(&mut self, meter: &PlaybackState) -> PlaybackStopAck {
        let event = snapshot_output_meter(meter, self.meter_epoch);
        if let ThreadEvent::PlaybackOutputMeter { peak_linear, .. } = &event {
            self.note_meter_snapshot(*peak_linear);
        }
        PlaybackStopAck {
            epoch: self.meter_epoch,
            epoch_peak_max: self.epoch_peak_max,
        }
    }

    fn stream_flushed(&mut self, callback_flush_completed: bool) {
        self.stream_flush_pending = false;
        self.end_of_stream = false;
        self.drain_start = None;
        self.flushes_processed = self.flushes_processed.wrapping_add(1);
        self.update_flush_mode(callback_flush_completed);
    }

    fn update_flush_mode(&mut self, callback_flush_completed: bool) {
        self.flush_mode = if self.stream_flush_pending {
            FlushMode::DroppingUntilFlush
        } else if self.paused {
            FlushMode::DroppingUntilResume
        } else if callback_flush_completed {
            FlushMode::Normal
        } else {
            FlushMode::WaitingForDrain
        };
    }

    fn callback_flushed(&mut self) {
        self.update_flush_mode(true);
    }

    fn drops_frames(&self) -> bool {
        matches!(
            self.flush_mode,
            FlushMode::DroppingUntilFlush | FlushMode::DroppingUntilResume
        )
    }
}

#[cfg(test)]
#[path = "runtime_protocol_tests.rs"]
mod protocol_tests;

struct RecoveryState {
    last_callback_count: u64,
    last_callback_check: Instant,
    callback_stall_timeout: Duration,
    last_stream_error_count: u64,
    last_recovery_attempt: Instant,
    recovery_retry_interval: Duration,
    last_device_identity_check: Instant,
    device_identity_check_interval: Duration,
    last_reported_underruns: u64,
}

struct DiagnosticState {
    stream_start_time: Instant,
    last_diagnostic_log: Instant,
    diagnostic_interval: Duration,
    last_meter_report: Instant,
    meter_interval: Duration,
}

/// Callback clock and ring consumer for the explicitly selected lab backend.
/// This shares the CPAL callback's ring, volume and meter kernels.
struct LabOutput {
    consumer: Consumer<f32>,
    scratch: Vec<f32>,
    next_tick: Instant,
}

const LAB_MAX_CHANNELS: usize = 16;
const LAB_SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

#[cfg(test)]
mod lab_tests {
    use super::*;
    use std::sync::mpsc::{channel, sync_channel};

    fn runtime() -> PlaybackRuntime {
        let (_message_tx, message_rx) = channel();
        let (_command_tx, command_rx) = channel();
        let (event_tx, _event_rx) = crossbeam::channel::bounded(64);
        PlaybackRuntime::new(PlaybackRuntimeParams {
            message_rx,
            command_rx,
            event_tx,
            sample_rate: 48_000,
            buffer_ms: 200,
            initial_channels: 2,
            frame_size: 64,
            output_device: None,
            recycle_tx: sync_channel(64).0,
            allow_virtual_output: false,
            output_access: OutputAccessMode::Shared,
            sink_type: SinkType::LabNull,
            shared_output_peak_bits: Arc::new(AtomicU32::new(0)),
            shared_clipped_sample_count: Arc::new(AtomicU64::new(0)),
        })
        .expect("lab runtime")
    }

    #[test]
    fn lab_uses_no_cpal_resources_and_consumes_processed_samples() {
        let mut runtime = runtime();
        assert!(runtime.host.is_none());
        assert!(runtime.device.is_none());
        assert!(runtime.stream.is_none());
        let frame = super::super::super::AudioFrame::new(vec![0.5; 128], 64, 2, 48_000);
        runtime.handle_frame(frame);
        for _ in 0..100 {
            runtime.lab_output.as_mut().unwrap().next_tick = Instant::now();
            runtime.tick_lab_output();
        }
        assert!(runtime.state.callback_count.load(Ordering::Relaxed) >= 100);
        assert!(runtime.state.total_callback_samples.load(Ordering::Relaxed) >= 128);
        assert!(f32::from_bits(runtime.state.output_peak_bits.load(Ordering::Relaxed)) > 0.0);
    }

    #[test]
    fn cancelled_lab_reconfigure_preserves_running_format() {
        let mut runtime = runtime();
        let ticket = super::super::super::HostUpdateTicket::new();
        assert!(ticket.try_begin_execution());
        assert!(ticket.cancel());
        assert!(
            runtime
                .rebuild_lab_output(96_000, 6, Some(&ticket))
                .is_err()
        );
        assert_eq!(runtime.config.sample_rate, 48_000);
        assert_eq!(runtime.logical_channels, 2);
        assert!(runtime.lab_output.is_some());
    }

    #[test]
    fn lab_rejects_unsupported_capabilities() {
        let mut runtime = runtime();
        assert!(runtime.rebuild_lab_output(12_345, 2, None).is_err());
        assert!(runtime.rebuild_lab_output(48_000, 17, None).is_err());
    }

    #[test]
    fn lab_flush_and_stop_wait_for_consumption() {
        let mut runtime = runtime();
        let (request, receiver) = super::super::super::PlaybackStopRequest::new();
        runtime.handle_command(PlaybackCommand::Stop(request));
        for _ in 0..100 {
            runtime.lab_output.as_mut().unwrap().next_tick = Instant::now();
            runtime.tick_lab_output();
            if flush_completed(&runtime.state, &runtime.producer, runtime.buffer_capacity) {
                break;
            }
        }
        assert!(flush_completed(
            &runtime.state,
            &runtime.producer,
            runtime.buffer_capacity
        ));
        assert!(runtime.finalize_pending_stop_ack(true));
        assert!(
            receiver.try_recv().is_ok(),
            "stop acknowledgment follows drain"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeDecision {
    Proceed,
    Continue,
    Break,
}

impl PlaybackRuntime {
    fn new(params: PlaybackRuntimeParams) -> Result<Self, String> {
        if params.sink_type == SinkType::LabNull {
            return Self::new_lab(params);
        }
        let PlaybackRuntimeParams {
            message_rx,
            command_rx,
            event_tx,
            sample_rate,
            buffer_ms,
            initial_channels,
            frame_size,
            output_device,
            recycle_tx,
            allow_virtual_output,
            output_access,
            sink_type: _,
            shared_output_peak_bits,
            shared_clipped_sample_count,
        } = params;
        set_realtime_priority(sample_rate, frame_size);

        let host = crate::devices::get_host_for_device(output_device.as_deref());
        #[cfg(target_os = "macos")]
        let backend_exclusive_active = output_device
            .as_deref()
            .is_some_and(crate::devices::is_asio_device);

        let output_access_plan = plan_output_access(output_access, output_device.as_deref());
        // Mutated only by the macOS exclusive-mode activation below.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut output_access_status = output_access_plan.status;
        if output_access.requires_exclusive()
            && output_access_status == OutputAccessStatus::Unsupported
        {
            return Err(output_access_plan.reason.unwrap_or_else(|| {
                "Exclusive output is required, but the selected backend cannot open an exclusive stream"
                    .to_string()
            }));
        }

        let output_device = sanitize_output_device(output_device);
        let device = select_playback_device(&host, output_device.as_deref(), allow_virtual_output)?;
        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "Unknown".to_string());

        #[cfg(target_os = "macos")]
        let mut coreaudio_exclusive_mode = CoreAudioExclusiveModeGuard::inactive();

        #[cfg(target_os = "macos")]
        if output_access_status == OutputAccessStatus::ExclusivePending {
            let activated =
                coreaudio_exclusive_mode.activate_for_device(&device_name, output_access)?;
            output_access_status = activated;
        }

        let mut channels = initial_channels;
        let mut config = StreamConfig {
            channels: channels as u16,
            sample_rate,
            buffer_size: initial_buffer_size(output_access_status, frame_size),
        };

        let (output_format, hw_channels) = choose_output_format(&device, &config);
        if hw_channels != channels as u16 {
            log::info!(
                "[Playback Thread] Adjusting output channels from {} to {} (device limitation)",
                channels,
                hw_channels
            );
            channels = hw_channels as usize;
            config.channels = hw_channels;
        }

        let buffer_capacity = playback_buffer_capacity(sample_rate, channels, buffer_ms);
        let (mut producer, consumer) = RingBuffer::<f32>::new(buffer_capacity);
        let state = Arc::new(PlaybackState::new_sharing_meters(
            buffer_capacity,
            shared_output_peak_bits,
            shared_clipped_sample_count,
        ));
        prefill_silence(&mut producer, buffer_capacity / 2);
        let conversion_buffer = conversion_buffer_for_ring(buffer_capacity);

        let stream = build_output_stream(
            &device,
            &config,
            Arc::clone(&state),
            event_tx.clone(),
            consumer,
            output_format,
        )?;
        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {}", e))?;

        send_playback_event(
            &event_tx,
            ThreadEvent::PlaybackChannelsChanged(initial_channels),
            "initial playback channels",
        );
        let coreaudio_device_id = coreaudio_output_device_id(&device_name);
        send_playback_event(
            &event_tx,
            ThreadEvent::PlaybackOutputDeviceChanged(device_name.clone()),
            "initial playback output device",
        );
        send_playback_event(
            &event_tx,
            ThreadEvent::PlaybackOutputAccessChanged(output_access_status),
            "initial playback output access",
        );

        log::info!(
            "[Playback Thread] Started - {}Hz, {} channels, format: {:?}, access: {:?}, device: '{}'",
            sample_rate,
            channels,
            output_format,
            output_access_status,
            device_name
        );

        if let Ok(actual_config) = device.default_output_config() {
            log::debug!(
                "[Playback Thread] Device default config: {}Hz {}ch {:?} (using {}Hz {}ch)",
                actual_config.sample_rate(),
                actual_config.channels(),
                actual_config.sample_format(),
                sample_rate,
                channels,
            );
        }

        if is_virtual_output_device_name(&device_name) && !allow_virtual_output {
            log::debug!(
                "[Playback Thread] Output device '{}' appears to be virtual/loopback.",
                device_name
            );
        }

        Ok(Self {
            message_rx,
            command_rx,
            event_tx,
            recycle_tx,
            host: Some(host),
            output_device,
            allow_virtual_output,
            output_access,
            output_access_status,
            #[cfg(target_os = "macos")]
            backend_exclusive_active,
            #[cfg(target_os = "macos")]
            coreaudio_exclusive_mode,
            device: Some(device),
            device_name,
            coreaudio_device_id,
            stream: Some(stream),
            config,
            output_format: Some(output_format),
            lab_output: None,
            channels,
            logical_channels: initial_channels,
            frame_size,
            buffer_ms,
            producer,
            state,
            buffer_capacity,
            conversion_buffer,
            accounting: PlaybackAccounting::default(),
            drain: DrainState {
                end_of_stream: false,
                drain_start: None,
                drain_timeout: Duration::from_secs(2),
                flush_mode: FlushMode::Normal,
                stream_flush_pending: false,
                paused: false,
                meter_epoch: 0,
                epoch_peak_max: 0.0,
                flushes_processed: 0,
                pending_stop_ack: None,
            },
            recovery: RecoveryState {
                last_callback_count: 0,
                last_callback_check: Instant::now(),
                callback_stall_timeout: Duration::from_secs(3),
                last_stream_error_count: 0,
                last_recovery_attempt: Instant::now()
                    .checked_sub(Duration::from_secs(10))
                    .unwrap_or_else(Instant::now),
                recovery_retry_interval: Duration::from_millis(500),
                last_device_identity_check: Instant::now(),
                device_identity_check_interval: Duration::from_secs(2),
                last_reported_underruns: 0,
            },
            diagnostics: DiagnosticState {
                stream_start_time: Instant::now(),
                last_diagnostic_log: Instant::now(),
                diagnostic_interval: Duration::from_secs(5),
                last_meter_report: Instant::now(),
                meter_interval: Duration::from_millis(100),
            },
        })
    }

    fn new_lab(params: PlaybackRuntimeParams) -> Result<Self, String> {
        let PlaybackRuntimeParams {
            message_rx,
            command_rx,
            event_tx,
            sample_rate,
            buffer_ms,
            initial_channels,
            frame_size,
            output_device,
            recycle_tx,
            allow_virtual_output,
            output_access,
            sink_type: _,
            shared_output_peak_bits,
            shared_clipped_sample_count,
        } = params;
        if !LAB_SAMPLE_RATES.contains(&sample_rate)
            || initial_channels == 0
            || initial_channels > LAB_MAX_CHANNELS
        {
            return Err("Invalid lab playback format".to_string());
        }
        if output_device
            .as_deref()
            .is_some_and(|device| device != "Systemwide Lab Output")
        {
            return Err("Lab backend cannot open a physical output device".to_string());
        }
        if output_access.requires_exclusive() {
            return Err("Lab output does not support exclusive device access".to_string());
        }
        let buffer_capacity = playback_buffer_capacity(sample_rate, initial_channels, buffer_ms);
        let (mut producer, consumer) = RingBuffer::<f32>::new(buffer_capacity);
        prefill_silence(&mut producer, buffer_capacity / 2);
        let state = Arc::new(PlaybackState::new_sharing_meters(
            buffer_capacity,
            shared_output_peak_bits,
            shared_clipped_sample_count,
        ));
        send_playback_event(
            &event_tx,
            ThreadEvent::PlaybackChannelsChanged(initial_channels),
            "lab channels",
        );
        send_playback_event(
            &event_tx,
            ThreadEvent::PlaybackOutputDeviceChanged("Systemwide Lab Output".to_string()),
            "lab output",
        );
        send_playback_event(
            &event_tx,
            ThreadEvent::PlaybackOutputAccessChanged(OutputAccessStatus::Shared),
            "lab access",
        );
        let now = Instant::now();
        Ok(Self {
            message_rx,
            command_rx,
            event_tx,
            recycle_tx,
            host: None,
            output_device,
            allow_virtual_output,
            output_access,
            output_access_status: OutputAccessStatus::Shared,
            #[cfg(target_os = "macos")]
            backend_exclusive_active: false,
            #[cfg(target_os = "macos")]
            coreaudio_exclusive_mode: CoreAudioExclusiveModeGuard::inactive(),
            device: None,
            device_name: "Systemwide Lab Output".to_string(),
            coreaudio_device_id: None,
            stream: None,
            config: StreamConfig {
                channels: initial_channels as u16,
                sample_rate,
                buffer_size: initial_buffer_size(OutputAccessStatus::Shared, frame_size),
            },
            output_format: None,
            lab_output: Some(LabOutput {
                consumer,
                scratch: vec![0.0; initial_channels * frame_size.max(1)],
                next_tick: now,
            }),
            channels: initial_channels,
            logical_channels: initial_channels,
            frame_size,
            buffer_ms,
            producer,
            state,
            buffer_capacity,
            conversion_buffer: conversion_buffer_for_ring(buffer_capacity),
            accounting: PlaybackAccounting::default(),
            drain: DrainState {
                end_of_stream: false,
                drain_start: None,
                drain_timeout: Duration::from_secs(2),
                flush_mode: FlushMode::Normal,
                stream_flush_pending: false,
                paused: false,
                meter_epoch: 0,
                epoch_peak_max: 0.0,
                flushes_processed: 0,
                pending_stop_ack: None,
            },
            recovery: RecoveryState {
                last_callback_count: 0,
                last_callback_check: now,
                callback_stall_timeout: Duration::from_secs(3),
                last_stream_error_count: 0,
                last_recovery_attempt: now,
                recovery_retry_interval: Duration::from_millis(500),
                last_device_identity_check: now,
                device_identity_check_interval: Duration::from_secs(2),
                last_reported_underruns: 0,
            },
            diagnostics: DiagnosticState {
                stream_start_time: now,
                last_diagnostic_log: now,
                diagnostic_interval: Duration::from_secs(5),
                last_meter_report: now,
                meter_interval: Duration::from_millis(100),
            },
        })
    }

    fn tick_lab_output(&mut self) {
        let Some(lab) = self.lab_output.as_mut() else {
            return;
        };
        let now = Instant::now();
        if now < lab.next_tick {
            return;
        }
        let frames = self.frame_size.max(1);
        let period = Duration::from_secs_f64(frames as f64 / self.config.sample_rate as f64);
        lab.next_tick = now + period;
        self.state
            .output_callback_active
            .store(true, Ordering::Release);
        self.state.callback_count.fetch_add(1, Ordering::Relaxed);
        let count = lab.scratch.len();
        read_ring_buffer(
            &mut lab.consumer,
            &mut lab.scratch,
            count,
            &self.state,
            self.buffer_capacity,
        );
        apply_volume_clamp(
            &mut lab.scratch,
            &self.state,
            self.channels,
            self.config.sample_rate,
        );
        self.state
            .output_callback_active
            .store(false, Ordering::Release);
    }

    fn rebuild_lab_output(
        &mut self,
        sample_rate: u32,
        channels: usize,
        ticket: Option<&super::super::HostUpdateTicket>,
    ) -> Result<PlaybackConfiguration, String> {
        if !LAB_SAMPLE_RATES.contains(&sample_rate) || channels == 0 || channels > LAB_MAX_CHANNELS
        {
            return Err("Invalid lab playback format".to_string());
        }
        let buffer_capacity = playback_buffer_capacity(sample_rate, channels, self.buffer_ms);
        let (mut producer, consumer) = RingBuffer::<f32>::new(buffer_capacity);
        prefill_silence(&mut producer, buffer_capacity / 2);
        let state = Arc::new(PlaybackState::new_sharing_meters(
            buffer_capacity,
            Arc::clone(&self.state.output_peak_bits),
            Arc::clone(&self.state.clipped_sample_count),
        ));
        copy_playback_controls(&self.state, &state);
        let scratch = vec![0.0; channels * self.frame_size.max(1)];
        if ticket.is_some_and(|ticket| !ticket.try_complete_execution()) {
            return Err("Playback reconfiguration was cancelled before installation".to_string());
        }
        self.drain_pending_messages();
        self.producer = producer;
        self.state = state;
        self.lab_output = Some(LabOutput {
            consumer,
            scratch,
            next_tick: Instant::now(),
        });
        self.config.channels = channels as u16;
        self.config.sample_rate = sample_rate;
        self.channels = channels;
        self.logical_channels = channels;
        self.buffer_capacity = buffer_capacity;
        self.conversion_buffer = conversion_buffer_for_ring(buffer_capacity);
        self.recovery.last_callback_count = 0;
        self.recovery.last_callback_check = Instant::now();
        self.drain.callback_flushed();
        self.drain.end_of_stream = false;
        self.drain.drain_start = None;
        send_playback_event(
            &self.event_tx,
            ThreadEvent::PlaybackChannelsChanged(channels),
            "lab reconfigure channels",
        );
        Ok(PlaybackConfiguration {
            sample_rate,
            channels,
        })
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            self.tick_lab_output();
            if let Ok(command) = self.command_rx.try_recv()
                && self.handle_command(command) == RuntimeDecision::Break
            {
                break;
            }

            // Stop completion never stalls the loop: once quiesce (ring
            // empty plus callback inactive) is observed after the arm,
            // the terminal record answers exactly once.
            if self.drain.pending_stop_ack.is_some()
                && flush_completed(&self.state, &self.producer, self.buffer_capacity)
            {
                self.finalize_pending_stop_ack(true);
            }

            if self.wait_for_flush_drain() == RuntimeDecision::Continue {
                continue;
            }

            match self.handle_stream_recovery() {
                RuntimeDecision::Proceed => {}
                RuntimeDecision::Continue => continue,
                RuntimeDecision::Break => break,
            }

            self.emit_underrun_milestone();
            self.emit_output_meter();

            self.emit_periodic_diagnostics();

            if self.handle_next_message() == RuntimeDecision::Break {
                break;
            }
        }

        self.log_final_accounting();
        log::debug!("[Playback Thread] Stopped");
        Ok(())
    }

    fn handle_command(&mut self, command: PlaybackCommand) -> RuntimeDecision {
        match command {
            PlaybackCommand::SetVolume(vol) => {
                self.state.volume.store(vol.to_bits(), Ordering::Relaxed);
                RuntimeDecision::Proceed
            }
            PlaybackCommand::Mute(muted) => {
                self.state.muted.store(muted, Ordering::Relaxed);
                RuntimeDecision::Proceed
            }
            PlaybackCommand::Pause => {
                request_flush(&self.state);
                self.drain.begin_drop(FlushMode::DroppingUntilResume);
                RuntimeDecision::Proceed
            }
            PlaybackCommand::Resume { epoch } => {
                // Any Resume re-arms callback emission, including the
                // same-epoch pause/resume and Stop-then-Resume paths:
                // the cutoff only needs to hold until the operator
                // resumes flow (post-quiesce the ring is empty and
                // frames stay dropped until the next Flush).
                self.state.stop_latched.store(false, Ordering::Relaxed);
                let finalized = if epoch != self.drain.meter_epoch {
                    // A new-epoch fence supersedes a pending Stop:
                    // finalize its record before adoption discards it.
                    // The fence skips its meter reset exactly when this
                    // swap ran (D1: no wipe window, no stale flag — the
                    // decision is local to these adjacent statements).
                    // Pending-at-Resume occurs only via timeout
                    // recovery (healthy stop() sync-acks first), so
                    // the unit-interleaving test above IS the live
                    // coverage for the skip; healthy paths never take it.
                    self.finalize_pending_stop_ack(false)
                } else {
                    false
                };
                self.drain
                    .adopt_playback_epoch(&self.state, epoch, finalized);
                self.drain.resume(flush_completed(
                    &self.state,
                    &self.producer,
                    self.buffer_capacity,
                ));
                RuntimeDecision::Proceed
            }
            PlaybackCommand::UpdateSampleRate(new_sample_rate) => {
                self.handle_sample_rate_update(new_sample_rate)
            }
            PlaybackCommand::UpdateChannels(new_channels) => {
                self.handle_channel_update(new_channels)
            }
            PlaybackCommand::Reconfigure(request) => self.handle_reconfigure_request(request),
            PlaybackCommand::Stop(request) => {
                // Supersede: finalize any still-pending acknowledgment
                // before arming the new one (report-then-clear: the old
                // record answers instead of being lost).
                self.finalize_pending_stop_ack(true);
                // Latch the callback emission cutoff before anything can
                // quiesce: from here the callback discards instead of
                // emitting, including late pre-Stop arrivals. The
                // inter-tick residual is preserved for the terminal.
                self.state.stop_latched.store(true, Ordering::Relaxed);
                self.diagnostics.last_meter_report = Instant::now();
                request_flush(&self.state);
                self.drain.begin_drop(FlushMode::DroppingUntilFlush);
                self.drain.pending_stop_ack = Some(request.reply_tx);
                RuntimeDecision::Proceed
            }
            PlaybackCommand::Shutdown => {
                log::debug!("[Playback Thread] Shutting down");
                self.finalize_pending_stop_ack(false);
                RuntimeDecision::Break
            }
        }
    }

    /// Answer a pending Stop with its terminal record, if any.
    ///
    /// Quiesce, supersede, and shutdown paths share this: the terminal
    /// snapshot goes out first when the worker stays alive (R12
    /// order), then the swapped residual folds into the cumulative
    /// and the record replies. Best-effort send — a timed-out manager
    /// collects late; a gone manager needs nothing. Returns whether a
    /// terminal swap ran, so the epoch fence can skip its meter reset
    /// exactly then (a racing callback window lands in the new record
    /// instead of being wiped).
    fn finalize_pending_stop_ack(&mut self, emit_snapshot: bool) -> bool {
        let Some(reply_tx) = self.drain.pending_stop_ack.take() else {
            return false;
        };
        if emit_snapshot {
            self.emit_terminal_drain_snapshot();
        }
        let ack = self.drain.take_stop_terminal(&self.state);
        reply_tx.send(ack).ok();
        true
    }

    fn handle_reconfigure_request(
        &mut self,
        request: PlaybackReconfigureRequest,
    ) -> RuntimeDecision {
        if !request.ticket.try_begin_execution() {
            request
                .reply_tx
                .send(Err(
                    "Playback reconfiguration was cancelled before execution".to_string(),
                ))
                .ok();
            return RuntimeDecision::Proceed;
        }

        let (decision, result) = self.reconfigure_output(request.requested, &request.ticket);
        request.reply_tx.send(result).ok();
        decision
    }

    fn reconfigure_output(
        &mut self,
        requested: PlaybackConfiguration,
        ticket: &super::super::HostUpdateTicket,
    ) -> (RuntimeDecision, Result<PlaybackConfiguration, String>) {
        if requested.sample_rate == 0 {
            return (
                RuntimeDecision::Proceed,
                Err("Playback sample rate must be non-zero".to_string()),
            );
        }
        if requested.channels == 0 || requested.channels > u16::MAX as usize {
            return (
                RuntimeDecision::Proceed,
                Err(format!(
                    "Playback channel count {} is outside 1..={}",
                    requested.channels,
                    u16::MAX
                )),
            );
        }
        if requested.sample_rate == self.config.sample_rate
            && requested.channels == self.logical_channels
        {
            if !ticket.try_complete_execution() {
                return (
                    RuntimeDecision::Proceed,
                    Err("Playback reconfiguration was cancelled before installation".to_string()),
                );
            }
            return (
                RuntimeDecision::Proceed,
                Ok(PlaybackConfiguration {
                    sample_rate: self.config.sample_rate,
                    channels: self.logical_channels,
                }),
            );
        }

        let drained = self.drain_pending_messages();
        log::info!(
            "[Playback Thread] Reconfiguring output: {}Hz/{}ch -> {}Hz/{}ch (drained {} frames)",
            self.config.sample_rate,
            self.logical_channels,
            requested.sample_rate,
            requested.channels,
            drained,
        );
        if self.lab_output.is_some() {
            let actual =
                self.rebuild_lab_output(requested.sample_rate, requested.channels, Some(ticket));
            return (RuntimeDecision::Continue, actual);
        }
        if let Err(error) = self.stream.as_ref().expect("CPAL stream").pause() {
            log::warn!("[Playback Thread] Failed to pause old stream: {error}");
        }
        std::thread::sleep(Duration::from_millis(10));
        self.drain_pending_messages();

        match rebuild_playback_stream(
            self.host.as_ref().expect("CPAL host"),
            RebuildPlaybackParams {
                output_device: self.output_device.as_deref(),
                allow_virtual_output: self.allow_virtual_output,
                sample_rate: requested.sample_rate,
                requested_channels: requested.channels,
                buffer_ms: self.buffer_ms,
                buffer_size: initial_buffer_size(self.output_access_status, self.frame_size),
                event_tx: self.event_tx.clone(),
                old_state: &self.state,
            },
        ) {
            Ok(rebuilt) => {
                let actual = PlaybackConfiguration {
                    sample_rate: rebuilt.config.sample_rate,
                    channels: rebuilt.logical_channels,
                };
                if !ticket.try_complete_execution() {
                    drop(rebuilt);
                    return (
                        RuntimeDecision::Proceed,
                        Err("Playback reconfiguration was cancelled before installation"
                            .to_string()),
                    );
                }
                self.install_recovered_stream(rebuilt);
                (RuntimeDecision::Continue, Ok(actual))
            }
            Err(rebuild_error) => {
                // The processing host has already committed its topology. Do
                // not resume an output stream whose rate/channel contract may
                // now be incompatible; fail loudly and let the manager publish
                // a committed-but-output-failed state.
                let error = format!(
                    "Playback output reconfiguration to {}Hz/{}ch failed after host commit: {rebuild_error}",
                    requested.sample_rate, requested.channels
                );
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingError(error.clone()),
                    "unrecoverable atomic output reconfiguration failure",
                );
                (RuntimeDecision::Break, Err(error))
            }
        }
    }

    fn handle_sample_rate_update(&mut self, new_sample_rate: u32) -> RuntimeDecision {
        if self.lab_output.is_some() {
            if let Err(error) =
                self.rebuild_lab_output(new_sample_rate, self.logical_channels, None)
            {
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingError(error),
                    "lab sample-rate update",
                );
                return RuntimeDecision::Break;
            }
            return RuntimeDecision::Continue;
        }
        log::debug!(
            "[Playback Thread] RECEIVED UpdateSampleRate({}) command, current sample_rate={}",
            new_sample_rate,
            self.config.sample_rate
        );
        if new_sample_rate == self.config.sample_rate {
            log::debug!(
                "[Playback Thread] UpdateSampleRate({}) - no change needed",
                new_sample_rate
            );
            return RuntimeDecision::Proceed;
        }

        log::info!(
            "[Playback Thread] Updating sample rate: {} -> {}",
            self.config.sample_rate,
            new_sample_rate
        );

        let mut drained_count = self.drain_pending_messages();
        if drained_count > 0 {
            log::debug!(
                "[Playback Thread] Drained {} stale frames during sample rate update",
                drained_count
            );
        }

        let mut new_config = StreamConfig {
            channels: self.config.channels,
            sample_rate: new_sample_rate,
            buffer_size: self.config.buffer_size,
        };

        drained_count += self.drain_pending_messages();

        if let Err(e) = self.stream.as_ref().expect("CPAL stream").pause() {
            log::warn!("[Playback Thread] Failed to pause old stream: {}", e);
        }
        std::thread::sleep(Duration::from_millis(10));
        drained_count += self.drain_pending_messages();

        let (new_format, new_hw_ch) =
            choose_output_format(self.device.as_ref().expect("CPAL device"), &new_config);
        let mut new_channels = self.channels;
        if new_hw_ch != new_config.channels {
            log::warn!(
                "[Playback Thread] Adjusting rebuild channels from {} to {}",
                new_config.channels,
                new_hw_ch
            );
            new_config.channels = new_hw_ch;
            new_channels = new_hw_ch as usize;
        }

        let new_buffer_capacity =
            playback_buffer_capacity(new_sample_rate, new_channels, self.buffer_ms);
        let (new_producer, new_consumer) = RingBuffer::<f32>::new(new_buffer_capacity);
        let new_state = Arc::new(PlaybackState::new_sharing_meters(
            new_buffer_capacity,
            Arc::clone(&self.state.output_peak_bits),
            Arc::clone(&self.state.clipped_sample_count),
        ));
        copy_playback_controls(&self.state, &new_state);

        log::info!(
            "[Playback Thread] Building new stream with sample rate: {}Hz, {}ch, format: {:?} (drained {} frames)",
            new_sample_rate,
            new_channels,
            new_format,
            drained_count
        );

        match build_output_stream(
            self.device.as_ref().expect("CPAL device"),
            &new_config,
            Arc::clone(&new_state),
            self.event_tx.clone(),
            new_consumer,
            new_format,
        ) {
            Ok(new_stream) => {
                if let Err(e) = new_stream.play() {
                    log::error!("[Playback Thread] Failed to start new stream: {}", e);
                    return self.resume_previous_stream_after_start_failure(
                        format!(
                            "Playback stream start failed for {} sample rate: {}",
                            new_sample_rate, e
                        ),
                        "sample-rate stream start failure",
                        "sample-rate stream start fallback",
                    );
                } else {
                    self.install_rebuilt_parts(
                        new_stream,
                        new_config,
                        new_state,
                        new_channels,
                        new_producer,
                        new_buffer_capacity,
                        new_format,
                    );
                    send_playback_event(
                        &self.event_tx,
                        ThreadEvent::PlaybackChannelsChanged(self.logical_channels),
                        "sample-rate rebuild channels",
                    );
                    self.drain_pending_messages();
                    log::info!(
                        "[Playback Thread] STREAM REBUILT successfully with {}Hz {}ch",
                        new_sample_rate,
                        self.channels
                    );
                }
            }
            Err(e) => {
                log::error!(
                    "[Playback Thread] Failed to build stream for sample rate {}: {}",
                    new_sample_rate,
                    e
                );
                if let Err(resume_err) = self.stream.as_ref().expect("CPAL stream").play() {
                    log::error!(
                        "[Playback Thread] Failed to resume old stream: {}",
                        resume_err
                    );
                }
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingError(format!(
                        "Playback stream rebuild failed for sample rate {}: {}",
                        new_sample_rate, e
                    )),
                    "sample-rate rebuild failure",
                );
            }
        }
        RuntimeDecision::Proceed
    }

    fn handle_channel_update(&mut self, mut new_channels: usize) -> RuntimeDecision {
        if self.lab_output.is_some() {
            if let Err(error) = self.rebuild_lab_output(self.config.sample_rate, new_channels, None)
            {
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingError(error),
                    "lab channel update",
                );
                return RuntimeDecision::Break;
            }
            return RuntimeDecision::Continue;
        }
        let logical_channels = new_channels;
        log::debug!(
            "[Playback Thread] RECEIVED UpdateChannels({}) command, current channels={}",
            new_channels,
            self.logical_channels
        );
        if new_channels == self.logical_channels {
            log::debug!(
                "[Playback Thread] UpdateChannels({}) - no change needed (already at {} channels)",
                new_channels,
                self.logical_channels
            );
            return RuntimeDecision::Proceed;
        }

        log::info!(
            "[Playback Thread] Updating channel count: {} -> {}",
            self.logical_channels,
            new_channels
        );

        let probe_config = StreamConfig {
            channels: new_channels as u16,
            sample_rate: self.config.sample_rate,
            buffer_size: self.config.buffer_size,
        };
        let (new_format, new_hw_ch) =
            choose_output_format(self.device.as_ref().expect("CPAL device"), &probe_config);
        if new_hw_ch as usize != new_channels {
            log::info!(
                "[Playback Thread] Device adjusts requested {}ch to {}ch",
                new_channels,
                new_hw_ch
            );
            new_channels = new_hw_ch as usize;
        }

        if new_channels == self.channels {
            log::info!(
                "[Playback Thread] Device adjusted channels back to {} (same as current), \
                 skipping rebuild. Processing chain output will be converted in the frame receive path.",
                self.channels
            );
            self.logical_channels = logical_channels;
            send_playback_event(
                &self.event_tx,
                ThreadEvent::PlaybackChannelsChanged(self.logical_channels),
                "logical playback channels changed",
            );
            return RuntimeDecision::Proceed;
        }

        log::trace!(
            "[Playback Thread] UpdateChannels: Draining pending frames with old channel count"
        );
        let mut drained_count = self.drain_pending_messages();

        let mut new_config = StreamConfig {
            channels: new_channels as u16,
            sample_rate: self.config.sample_rate,
            buffer_size: self.config.buffer_size,
        };
        new_config.channels = new_hw_ch;

        let new_buffer_capacity =
            playback_buffer_capacity(self.config.sample_rate, new_channels, self.buffer_ms);
        let (new_producer, new_consumer) = RingBuffer::<f32>::new(new_buffer_capacity);
        let new_state = Arc::new(PlaybackState::new_sharing_meters(
            new_buffer_capacity,
            Arc::clone(&self.state.output_peak_bits),
            Arc::clone(&self.state.clipped_sample_count),
        ));
        copy_playback_controls(&self.state, &new_state);

        drained_count += self.drain_pending_messages();

        if let Err(e) = self.stream.as_ref().expect("CPAL stream").pause() {
            log::warn!("[Playback Thread] Failed to pause old stream: {}", e);
        }
        std::thread::sleep(Duration::from_millis(10));
        drained_count += self.drain_pending_messages();

        log::info!(
            "[Playback Thread] Building new stream with config: {}ch, {}Hz, format: {:?} (drained {} frames)",
            new_config.channels,
            new_config.sample_rate,
            new_format,
            drained_count
        );

        match build_output_stream(
            self.device.as_ref().expect("CPAL device"),
            &new_config,
            Arc::clone(&new_state),
            self.event_tx.clone(),
            new_consumer,
            new_format,
        ) {
            Ok(new_stream) => {
                log::info!("[Playback Thread] Stream built, starting playback...");
                if let Err(e) = new_stream.play() {
                    log::error!("[Playback Thread] Failed to start new stream: {}", e);
                    self.resume_previous_stream_after_start_failure(
                        format!(
                            "Playback stream start failed for {} channels: {}",
                            new_channels, e
                        ),
                        "channel stream start failure",
                        "channel stream start fallback",
                    )
                } else {
                    self.install_rebuilt_parts(
                        new_stream,
                        new_config,
                        new_state,
                        new_channels,
                        new_producer,
                        new_buffer_capacity,
                        new_format,
                    );
                    send_playback_event(
                        &self.event_tx,
                        ThreadEvent::PlaybackChannelsChanged(logical_channels),
                        "channel rebuild channels",
                    );
                    self.logical_channels = logical_channels;

                    let final_drained = self.drain_pending_messages();
                    if final_drained > 0 {
                        log::debug!(
                            "[Playback Thread] Drained {} additional frames after stream rebuild",
                            final_drained
                        );
                    }

                    log::warn!(
                        "[Playback Thread] STREAM REBUILT successfully with {} channels",
                        self.channels
                    );
                    RuntimeDecision::Proceed
                }
            }
            Err(e) => {
                log::error!(
                    "[Playback Thread] Failed to build stream for {} channels: {}",
                    new_channels,
                    e
                );
                let resume_result = self.stream.as_ref().expect("CPAL stream").play();
                if let Err(resume_err) = resume_result {
                    log::error!(
                        "[Playback Thread] Failed to resume old stream after rebuild failure: {}. \
                         Playback is dead, exiting playback loop.",
                        resume_err
                    );
                    send_playback_event(
                        &self.event_tx,
                        ThreadEvent::ProcessingError(format!(
                            "Playback stream unrecoverable: rebuild failed ({}) \
                             and old stream failed to resume ({})",
                            e, resume_err
                        )),
                        "unrecoverable channel rebuild failure",
                    );
                    return RuntimeDecision::Break;
                }

                log::warn!(
                    "[Playback Thread] Falling back to old stream ({} channels) after rebuild failure",
                    self.channels
                );
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingWarning(format!(
                        "Playback stream rebuild failed for {} channels, falling back to previous configuration",
                        new_channels
                    )),
                    "channel rebuild fallback",
                );
                RuntimeDecision::Proceed
            }
        }
    }

    fn wait_for_flush_drain(&mut self) -> RuntimeDecision {
        if !matches!(self.drain.flush_mode, FlushMode::WaitingForDrain) {
            return RuntimeDecision::Proceed;
        }

        if flush_completed(&self.state, &self.producer, self.buffer_capacity) {
            // Report the pre-flush residual before the swap clears it.
            self.send_output_meter_snapshot();
            self.diagnostics.last_meter_report = Instant::now();
            self.drain.callback_flushed();
            RuntimeDecision::Proceed
        } else {
            std::thread::sleep(Duration::from_millis(1));
            RuntimeDecision::Continue
        }
    }

    fn handle_stream_recovery(&mut self) -> RuntimeDecision {
        if self.lab_output.is_some() {
            return RuntimeDecision::Proceed;
        }
        let current_stream_errors = self.state.stream_error_count.load(Ordering::Relaxed);
        let current_callbacks = self.state.callback_count.load(Ordering::Relaxed);
        let coreaudio_identity_reason = self.coreaudio_identity_recovery_reason();
        let recovery_reason = playback_recovery_reason(
            current_stream_errors,
            &mut self.recovery.last_stream_error_count,
            current_callbacks,
            &mut self.recovery.last_callback_count,
            &mut self.recovery.last_callback_check,
            self.recovery.callback_stall_timeout,
            self.accounting.frames_received,
            self.accounting.frames_written,
            coreaudio_identity_reason,
        );

        let Some(reason) = recovery_reason else {
            return RuntimeDecision::Proceed;
        };

        if self.recovery.last_recovery_attempt.elapsed() < self.recovery.recovery_retry_interval {
            std::thread::sleep(Duration::from_millis(10));
            return RuntimeDecision::Continue;
        }
        self.recovery.last_recovery_attempt = Instant::now();

        let drained_count = self.drain_pending_messages();
        let warning = format!(
            "Audio device '{}' needs playback stream recovery: {}",
            self.device_name, reason
        );
        log::warn!(
            "[Playback Thread] {} (drained {} queued frames)",
            warning,
            drained_count
        );
        send_playback_event(
            &self.event_tx,
            ThreadEvent::ProcessingWarning(warning),
            "stream recovery warning",
        );

        if let Err(e) = self.stream.as_ref().expect("CPAL stream").pause() {
            log::warn!(
                "[Playback Thread] Failed to pause stream before recovery: {}",
                e
            );
        }

        #[cfg(target_os = "macos")]
        if self.output_access.prefers_exclusive() && !self.backend_exclusive_active {
            match self
                .coreaudio_exclusive_mode
                .activate_for_device(&self.device_name, self.output_access)
            {
                Ok(status) => {
                    set_output_access_status(
                        &self.event_tx,
                        &mut self.output_access_status,
                        status,
                        "recovered playback output access",
                    );
                }
                Err(e) => {
                    log::error!("[Playback Thread] {}", e);
                    send_playback_event(
                        &self.event_tx,
                        ThreadEvent::ProcessingError(e),
                        "exclusive recovery failure",
                    );
                    return RuntimeDecision::Break;
                }
            }
        }

        match rebuild_playback_stream(
            self.host.as_ref().expect("CPAL host"),
            RebuildPlaybackParams {
                output_device: self.output_device.as_deref(),
                allow_virtual_output: self.allow_virtual_output,
                sample_rate: self.config.sample_rate,
                requested_channels: self.logical_channels,
                buffer_ms: self.buffer_ms,
                buffer_size: initial_buffer_size(self.output_access_status, self.frame_size),
                event_tx: self.event_tx.clone(),
                old_state: &self.state,
            },
        ) {
            Ok(rebuilt) => {
                self.install_recovered_stream(rebuilt);
                RuntimeDecision::Continue
            }
            Err(e) => {
                let msg = format!(
                    "Playback stream recovery failed for '{}': {}",
                    self.device_name, e
                );
                log::error!("[Playback Thread] {}", msg);
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingWarning(msg),
                    "stream recovery failure",
                );
                if let Err(resume_err) = self.stream.as_ref().expect("CPAL stream").play() {
                    log::warn!(
                        "[Playback Thread] Failed to resume previous stream after recovery failure: {}",
                        resume_err
                    );
                }
                self.recovery.last_callback_check = Instant::now();
                std::thread::sleep(Duration::from_millis(250));
                RuntimeDecision::Continue
            }
        }
    }

    fn coreaudio_identity_recovery_reason(&mut self) -> Option<String> {
        if self.recovery.last_device_identity_check.elapsed()
            <= self.recovery.device_identity_check_interval
        {
            return None;
        }
        self.recovery.last_device_identity_check = Instant::now();
        let current_device_id = coreaudio_output_device_id(&self.device_name);
        if current_device_id.is_some() && current_device_id != self.coreaudio_device_id {
            let previous_device_id = self.coreaudio_device_id;
            self.coreaudio_device_id = current_device_id;
            Some(format!(
                "CoreAudio device id changed for '{}' ({:?} -> {:?})",
                self.device_name, previous_device_id, current_device_id
            ))
        } else {
            None
        }
    }

    fn install_recovered_stream(&mut self, rebuilt: RebuiltPlaybackStream) {
        log::info!(
            "[Playback Thread] Recovered playback stream: device='{}', {}Hz, {}ch, format={:?}",
            rebuilt.device_name,
            rebuilt.config.sample_rate,
            rebuilt.channels,
            rebuilt.output_format
        );

        self.device = Some(rebuilt.device);
        self.device_name = rebuilt.device_name;
        self.stream = Some(rebuilt.stream);
        self.producer = rebuilt.producer;
        self.state = rebuilt.state;
        self.config = rebuilt.config;
        self.output_format = Some(rebuilt.output_format);
        self.channels = rebuilt.channels;
        self.logical_channels = rebuilt.logical_channels;
        self.buffer_capacity = rebuilt.buffer_capacity;
        self.conversion_buffer = conversion_buffer_for_ring(self.buffer_capacity);
        self.coreaudio_device_id = coreaudio_output_device_id(&self.device_name);
        self.recovery.last_device_identity_check = Instant::now();
        self.recovery.last_callback_count = 0;
        self.recovery.last_stream_error_count = 0;
        self.recovery.last_callback_check = Instant::now();
        self.recovery.last_reported_underruns = 0;
        self.drain.callback_flushed();
        self.drain.end_of_stream = false;
        self.drain.drain_start = None;

        send_playback_event(
            &self.event_tx,
            ThreadEvent::PlaybackChannelsChanged(self.logical_channels),
            "playback channels changed",
        );
        send_playback_event(
            &self.event_tx,
            ThreadEvent::PlaybackOutputDeviceChanged(self.device_name.clone()),
            "playback output device changed",
        );
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "installs all pieces of a rebuilt output stream"
    )]
    fn install_rebuilt_parts(
        &mut self,
        stream: Stream,
        config: StreamConfig,
        state: Arc<PlaybackState>,
        channels: usize,
        producer: Producer<f32>,
        buffer_capacity: usize,
        output_format: SampleFormat,
    ) {
        self.stream = Some(stream);
        self.config = config;
        self.state = state;
        self.channels = channels;
        self.producer = producer;
        self.buffer_capacity = buffer_capacity;
        self.output_format = Some(output_format);
        self.conversion_buffer = conversion_buffer_for_ring(buffer_capacity);
    }

    fn resume_previous_stream_after_start_failure(
        &mut self,
        start_failure: String,
        unrecoverable_context: &str,
        fallback_context: &str,
    ) -> RuntimeDecision {
        match self.stream.as_ref().expect("CPAL stream").play() {
            Ok(()) => {
                log::warn!(
                    "[Playback Thread] {}, resumed previous stream configuration",
                    start_failure
                );
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingWarning(format!(
                        "{start_failure}; resumed previous playback stream"
                    )),
                    fallback_context,
                );
                RuntimeDecision::Proceed
            }
            Err(resume_err) => {
                log::error!(
                    "[Playback Thread] Failed to resume old stream after start failure: {}",
                    resume_err
                );
                send_playback_event(
                    &self.event_tx,
                    ThreadEvent::ProcessingError(format!(
                        "{start_failure}; previous playback stream also failed to resume: {resume_err}"
                    )),
                    unrecoverable_context,
                );
                RuntimeDecision::Break
            }
        }
    }

    fn emit_underrun_milestone(&mut self) {
        let current_underruns = self.state.underrun_count.load(Ordering::Relaxed);
        if self.accounting.frames_received == 0 {
            self.recovery.last_reported_underruns = current_underruns;
            return;
        }
        if should_emit_underrun_milestone(
            self.drain.end_of_stream,
            current_underruns,
            self.recovery.last_reported_underruns,
        ) {
            send_playback_event(
                &self.event_tx,
                ThreadEvent::PlaybackUnderrun(current_underruns),
                "playback underrun",
            );
            self.recovery.last_reported_underruns = current_underruns;
        }
    }

    fn emit_periodic_diagnostics(&mut self) {
        if self.diagnostics.last_diagnostic_log.elapsed() <= self.diagnostics.diagnostic_interval {
            return;
        }

        self.send_playback_stats_snapshot("PERIODIC");
        self.diagnostics.last_diagnostic_log = Instant::now();
    }

    /// Send one playback statistics snapshot with current counters.
    ///
    /// Shared by the periodic diagnostics and the terminal drain snapshot so
    /// both report identical fields from identical loads. Runs on the
    /// playback worker thread (never the CPAL callback); the channel send is
    /// best-effort on the bounded event queue like every other emission.
    /// Tagged with the adopted playback epoch.
    fn send_playback_stats_snapshot(&self, report_kind: &str) {
        let elapsed = self.diagnostics.stream_start_time.elapsed().as_secs_f64();
        let total_cb = self.state.callback_count.load(Ordering::Relaxed);
        let total_cb_samples = self.state.total_callback_samples.load(Ordering::Relaxed);
        let effective_hz = if elapsed > 0.0 && self.channels > 0 {
            (total_cb_samples as f64 / self.channels as f64 / elapsed) as u64
        } else {
            0
        };
        let fill = {
            let slots = self.producer.slots();
            ((self.buffer_capacity - slots) * 100)
                .checked_div(self.buffer_capacity)
                .unwrap_or(0)
        };
        log::debug!(
            "[Playback Thread] {}: callbacks={}, effective={}Hz (expected {}Hz), \
             buffer_fill={}%, blocked={}, dropped={}, received={}, format={:?}",
            report_kind,
            total_cb,
            effective_hz,
            self.config.sample_rate,
            fill,
            self.accounting.frames_blocked,
            self.accounting.frames_dropped,
            self.accounting.frames_received,
            self.output_format,
        );
        send_playback_event(
            &self.event_tx,
            ThreadEvent::PlaybackStats {
                callback_count: total_cb,
                buffer_fill_percent: fill as u64,
                stream_error_count: self.state.stream_error_count.load(Ordering::Relaxed),
                frames_received: self.accounting.frames_received,
                frames_written: self.accounting.frames_written,
                frames_dropped: self.accounting.frames_dropped,
                effective_sample_rate: effective_hz,
                epoch: self.drain.meter_epoch,
            },
            "playback stats",
        );
    }

    fn emit_output_meter(&mut self) {
        if self.diagnostics.last_meter_report.elapsed() < self.diagnostics.meter_interval {
            return;
        }

        self.send_output_meter_snapshot();
        self.diagnostics.last_meter_report = Instant::now();
    }

    /// Send one output meter snapshot, swapping the residual peak.
    ///
    /// Shared by the periodic meter, the terminal drain snapshot, and
    /// flush completion. The swap hands the callback-observed residual to
    /// the event flow exactly once; the manager-side stopped-meter reset
    /// is unchanged. Tagged with the adopted playback epoch. The window
    /// folds into the epoch cumulative before the send, so a dropped
    /// send cannot lose it from the terminal record.
    fn send_output_meter_snapshot(&mut self) {
        let event = snapshot_output_meter(&self.state, self.drain.meter_epoch);
        if let ThreadEvent::PlaybackOutputMeter { peak_linear, .. } = &event {
            self.drain.note_meter_snapshot(*peak_linear);
        }
        send_playback_event(&self.event_tx, event, "playback output meter");
    }

    /// Publish final counters plus residual meter before a drained receipt.
    ///
    /// Streams shorter than the diagnostics interval would otherwise finish
    /// with stale zero counters: the ring drains, `PlaybackDrained` fires,
    /// and no periodic snapshot ever ran. Emitting the terminal snapshot
    /// first keeps every drained receipt truthful in arrival order — the
    /// manager observes final counters before `Stopped` — using the same
    /// fields and loads as the periodic reports (no new stats fields).
    /// Timer cadence restarts so no duplicate idle-window report follows
    /// immediately. Timeout versus real-drain receipt semantics are
    /// unchanged; only the actuals preceding each receipt are published.
    fn emit_terminal_drain_snapshot(&mut self) {
        self.send_playback_stats_snapshot("TERMINAL");
        self.send_output_meter_snapshot();
        self.diagnostics.last_diagnostic_log = Instant::now();
        self.diagnostics.last_meter_report = Instant::now();
    }

    fn handle_next_message(&mut self) -> RuntimeDecision {
        match self.message_rx.try_recv() {
            Ok(ProcessingMessage::Frame(frame)) => self.handle_frame(frame),
            Ok(ProcessingMessage::EndOfStream) => self.handle_end_of_stream(),
            Ok(ProcessingMessage::Flush) => self.handle_flush(),
            Err(TryRecvError::Empty) => self.handle_empty_queue(),
            Err(TryRecvError::Disconnected) => self.handle_disconnected_queue(),
        }
    }

    fn handle_frame(&mut self, frame: super::super::AudioFrame) -> RuntimeDecision {
        if self.drain.drops_frames() {
            self.accounting.frames_dropped += 1;
            recycle_frame_data(&self.recycle_tx, frame.data, "flush drop");
            return RuntimeDecision::Continue;
        }

        let required = required_frame_ring_space(
            frame.num_frames,
            frame.num_channels,
            frame.data.len(),
            self.channels,
            self.buffer_capacity,
        );
        let mut counted_block = false;
        while required <= self.buffer_capacity && self.producer.slots() < required {
            self.tick_lab_output();
            if !counted_block {
                self.accounting.frames_blocked += 1;
                counted_block = true;
            }
            if let Ok(command) = self.command_rx.try_recv() {
                if self.handle_command(command) == RuntimeDecision::Break {
                    recycle_frame_data(&self.recycle_tx, frame.data, "shutdown frame drop");
                    return RuntimeDecision::Break;
                }
                if self.drain.drops_frames() {
                    self.accounting.frames_dropped += 1;
                    recycle_frame_data(&self.recycle_tx, frame.data, "paused frame drop");
                    return RuntimeDecision::Continue;
                }
            }
            std::thread::sleep(Duration::from_millis(SPIN_MS_RINGBUFFER));
        }

        let first_frame = self.accounting.frames_received == 0;
        self.accounting.frames_received += 1;

        match write_frame_to_ring(
            &mut self.producer,
            &self.recycle_tx,
            &mut self.conversion_buffer,
            self.channels,
            frame,
        ) {
            FrameWriteOutcome::Written { samples } => {
                self.accounting.frames_written += 1;
                self.accounting.total_samples_written += samples as u64;
                if first_frame {
                    self.state.underrun_count.store(0, Ordering::Relaxed);
                    self.recovery.last_reported_underruns = 0;
                }
            }
            FrameWriteOutcome::Dropped => {
                self.accounting.frames_dropped += 1;
            }
            FrameWriteOutcome::ConversionBufferTooSmall => {
                self.accounting.frames_dropped += 1;
                static EVENT_GATE: AtomicU64 = AtomicU64::new(0);
                if crate::rate_limit::allow(&EVENT_GATE, 5_000_000_000) {
                    send_playback_event(
                        &self.event_tx,
                        ThreadEvent::ProcessingError(
                            "Playback conversion buffer invariant failed".to_string(),
                        ),
                        "conversion buffer invariant",
                    );
                }
            }
        }

        RuntimeDecision::Continue
    }

    fn handle_end_of_stream(&mut self) -> RuntimeDecision {
        if self.drain.drops_frames() {
            return RuntimeDecision::Continue;
        }
        log::debug!("[Playback Thread] End of stream - starting drain");
        self.drain.end_of_stream = true;
        self.drain.drain_start = Some(Instant::now());
        RuntimeDecision::Continue
    }

    fn handle_flush(&mut self) -> RuntimeDecision {
        // A boundary re-arms the callback unconditionally: no
        // pre-cutoff frame can be queued behind it (decoder FIFO,
        // processing retries-or-recycles in order and resets DSP
        // state at the Flush), so anything arriving later postdates
        // the cutoff — correct to emit even when a superseded
        // completion is still outstanding. New-epoch supersedes are
        // stale-dropped by the epoch gate; a same-epoch supersede
        // (Stop-then-seek) folds the post-cutoff windows into the
        // same-epoch terminal instead (ceiling-safe superset,
        // unreachable in current manager flows, which serialize Stop
        // before later commands). The latch is defense-in-depth —
        // worker drop-mode does the cutting off. Mirrors the stub
        // feeder arm.
        self.state.stop_latched.store(false, Ordering::Relaxed);
        request_flush(&self.state);
        let completed = flush_completed(&self.state, &self.producer, self.buffer_capacity);
        if completed {
            // Report the pre-flush residual before the swap clears it.
            self.send_output_meter_snapshot();
            self.diagnostics.last_meter_report = Instant::now();
        }
        self.drain.stream_flushed(completed);
        RuntimeDecision::Continue
    }

    fn handle_empty_queue(&mut self) -> RuntimeDecision {
        if !self.drain.end_of_stream {
            std::thread::sleep(Duration::from_millis(1));
            return RuntimeDecision::Continue;
        }

        if self.producer.slots() >= self.buffer_capacity {
            log::info!("[Playback Thread] Ring buffer drained, signaling completion");
            self.emit_terminal_drain_snapshot();
            send_playback_event(
                &self.event_tx,
                self.drain.drained_event(),
                "ring buffer drained",
            );
            // Keep the output worker alive for the next source. A normal EOF
            // is a transport state transition, not a worker shutdown; the
            // manager may issue another Play command immediately afterward.
            self.drain.end_of_stream = false;
            self.drain.drain_start = None;
            return RuntimeDecision::Continue;
        }

        if let Some(start) = self.drain.drain_start
            && start.elapsed() > self.drain.drain_timeout
        {
            self.emit_drain_timeout_event("Playback stalled", "drain timeout error");
            self.drain.end_of_stream = false;
            self.drain.drain_start = None;
            return RuntimeDecision::Continue;
        }

        std::thread::sleep(Duration::from_millis(5));
        RuntimeDecision::Continue
    }

    fn handle_disconnected_queue(&mut self) -> RuntimeDecision {
        if self.drain.end_of_stream {
            log::debug!(
                "[Playback Thread] Queue disconnected during drain, waiting for ring buffer"
            );
            self.wait_for_disconnect_drain();
        } else {
            log::debug!("[Playback Thread] Queue disconnected");
        }
        RuntimeDecision::Break
    }

    fn wait_for_disconnect_drain(&mut self) {
        let drain_start = Instant::now();
        let drain_timeout = Duration::from_secs(2);
        loop {
            self.tick_lab_output();
            if self.producer.slots() >= self.buffer_capacity {
                log::info!(
                    "[Playback Thread] Ring buffer drained (post-disconnect), signaling completion"
                );
                self.emit_terminal_drain_snapshot();
                send_playback_event(
                    &self.event_tx,
                    self.drain.drained_event(),
                    "post-disconnect drained",
                );
                break;
            }

            if drain_start.elapsed() > drain_timeout {
                self.emit_post_disconnect_timeout_event();
                break;
            }

            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn emit_drain_timeout_event(&mut self, error_prefix: &str, error_context: &str) {
        // Both terminal branches below (stall error or mostly-drained
        // completion) carry the actual finals first; the timeout-versus-
        // drain receipt distinction itself is unchanged.
        self.emit_terminal_drain_snapshot();
        let current_slots = self.producer.slots();
        let drain_percent = (current_slots * 100)
            .checked_div(self.buffer_capacity)
            .unwrap_or(100);
        if drain_percent < 80 {
            let msg = format!(
                "{}: ring buffer {}% full after {}s drain timeout \
                 (cpal callbacks not consuming audio). Device: '{}'",
                error_prefix,
                100 - drain_percent,
                self.drain.drain_timeout.as_secs(),
                self.device_name,
            );
            log::error!("[Playback Thread] {}", msg);
            send_playback_event(
                &self.event_tx,
                ThreadEvent::ProcessingError(msg),
                error_context,
            );
        } else {
            log::warn!(
                "[Playback Thread] Drain timeout, buffer mostly empty ({}% drained), signaling completion",
                drain_percent
            );
            send_playback_event(
                &self.event_tx,
                self.drain.drained_event(),
                "drain timeout completion",
            );
        }
    }

    fn emit_post_disconnect_timeout_event(&mut self) {
        // Both terminal branches below carry the actual finals first; the
        // timeout-versus-drain receipt distinction itself is unchanged.
        self.emit_terminal_drain_snapshot();
        let current_slots = self.producer.slots();
        let drain_percent = (current_slots * 100)
            .checked_div(self.buffer_capacity)
            .unwrap_or(100);
        if drain_percent < 80 {
            let msg = format!(
                "Playback stalled after disconnect: ring buffer {}% full after drain timeout. Device: '{}'",
                100 - drain_percent,
                self.device_name,
            );
            log::error!("[Playback Thread] {}", msg);
            send_playback_event(
                &self.event_tx,
                ThreadEvent::ProcessingError(msg),
                "post-disconnect drain timeout error",
            );
        } else {
            log::warn!(
                "[Playback Thread] Drain timeout after disconnect, buffer mostly empty ({}% drained)",
                drain_percent
            );
            send_playback_event(
                &self.event_tx,
                self.drain.drained_event(),
                "post-disconnect drain timeout completion",
            );
        }
    }

    fn drain_pending_messages(&mut self) -> usize {
        let mut drained = 0;
        let mut swallowed_flushes = 0u64;
        while let Ok(message) = self.message_rx.try_recv() {
            match message {
                ProcessingMessage::Frame(frame) => {
                    recycle_frame_data(&self.recycle_tx, frame.data, "rebuild stale frame");
                }
                ProcessingMessage::Flush => {
                    // Swallowed by the rebuild: counted (the manager
                    // counted it at send) and armed drains invalidated
                    // (a boundary re-drives the terminal). Drop-mode is
                    // NOT released: only the next real Flush clears it.
                    swallowed_flushes += 1;
                    self.drain.note_swallowed_flush();
                }
                ProcessingMessage::EndOfStream => {
                    // Swallowed by the rebuild, but a terminal marker
                    // must not vanish: arm the drain so the finished
                    // stream completes instead of wedging Playing.
                    self.drain.note_swallowed_eos();
                }
            }
            drained += 1;
        }
        if swallowed_flushes > 0 {
            log::debug!(
                "[Playback Thread] Rebuild swallowed {swallowed_flushes} stream Flush(es); counted, armed drains invalidated"
            );
        }
        drained
    }

    fn log_final_accounting(&self) {
        let elapsed = self.diagnostics.stream_start_time.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        let total_samples = self.state.total_callback_samples.load(Ordering::Relaxed);
        let total_callbacks = self.state.callback_count.load(Ordering::Relaxed);
        let total_frames = if self.channels > 0 {
            total_samples / self.channels as u64
        } else {
            0
        };
        let effective_rate = if elapsed_secs > 0.0 {
            (total_frames as f64 / elapsed_secs) as u64
        } else {
            0
        };
        let sample_rate = self.config.sample_rate;
        let audio_duration = if sample_rate > 0 {
            total_frames as f64 / sample_rate as f64
        } else {
            0.0
        };
        let avg_samples_per_callback = total_samples.checked_div(total_callbacks).unwrap_or(0);
        log::info!(
            "[Playback Thread] CALLBACK RATE: {} callbacks, {} total samples ({} frames) in {:.3}s = {} effective Hz (expected {}Hz), audio_duration={:.3}s, avg_samples/callback={}, channels={}",
            total_callbacks,
            total_samples,
            total_frames,
            elapsed_secs,
            effective_rate,
            sample_rate,
            audio_duration,
            avg_samples_per_callback,
            self.channels,
        );

        let written_audio = if self.channels > 0 && sample_rate > 0 {
            self.accounting.total_samples_written as f64
                / (self.channels as f64 * sample_rate as f64)
        } else {
            0.0
        };
        log::info!(
            "[Playback Thread] FRAME ACCOUNTING: received={}, written={}, dropped={}, blocked={}, written_samples={}, written_audio={:.3}s",
            self.accounting.frames_received,
            self.accounting.frames_written,
            self.accounting.frames_dropped,
            self.accounting.frames_blocked,
            self.accounting.total_samples_written,
            written_audio,
        );
    }
}

fn set_realtime_priority(sample_rate: u32, frame_size: usize) {
    let _ = (sample_rate, frame_size);
    // CPAL owns the hardware callback and its hard realtime scheduling. The
    // producer still has an audio deadline, so give it the same soft realtime
    // QoS as processing without claiming THREAD_TIME_CONSTRAINT_POLICY.
    #[cfg(target_os = "macos")]
    {
        match super::super::rt_priority::set_realtime_priority(
            super::super::rt_priority::RtPriority::Processing,
            None,
        ) {
            Ok(true) => log::info!("[Playback Thread] Audio-work priority set successfully"),
            Ok(false) => {
                log::debug!("[Playback Thread] Audio-work priority unavailable on platform")
            }
            Err(error) => {
                log::warn!("[Playback Thread] Failed to set audio-work priority: {error}")
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    log::debug!("[Playback Thread] Backend callback owns scheduling; feeder priority unchanged");
}

fn sanitize_output_device(output_device: Option<String>) -> Option<String> {
    output_device.map(|device| {
        if crate::devices::is_asio_device(&device) {
            crate::devices::strip_asio_prefix(&device).to_string()
        } else {
            device
        }
    })
}

fn conversion_buffer_for_ring(buffer_capacity: usize) -> Vec<f32> {
    Vec::with_capacity(buffer_capacity)
}

pub(super) fn required_frame_ring_space(
    num_frames: usize,
    input_channels: usize,
    data_len: usize,
    output_channels: usize,
    buffer_capacity: usize,
) -> usize {
    if input_channels == output_channels {
        data_len
    } else {
        num_frames.saturating_mul(output_channels)
    }
    .min(buffer_capacity)
}

pub(super) fn should_emit_underrun_milestone(
    end_of_stream: bool,
    current_underruns: u64,
    last_reported_underruns: u64,
) -> bool {
    if end_of_stream || current_underruns == last_reported_underruns {
        return false;
    }

    current_underruns == 1 || current_underruns.is_multiple_of(100) || last_reported_underruns == 0
}
