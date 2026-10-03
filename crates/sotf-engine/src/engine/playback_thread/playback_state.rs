use super::build::build_output_stream;
use super::misc::prefill_silence;
use super::misc::select_playback_device;
use super::pick::choose_output_format;
use super::playback::playback_buffer_capacity;
use super::types::RebuildPlaybackParams;
use super::types::RebuiltPlaybackStream;
use crate::engine::volume_ramp::VolumeRampState;
use cpal::StreamConfig;
use cpal::traits::{DeviceTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Shared state between thread and cpal callback (all fields are lock-free atomics)
pub(in crate::engine) struct PlaybackState {
    pub(super) capacity: usize,
    pub(super) volume: Arc<AtomicU32>, // Atomic f32 stored as u32 bits
    pub(super) muted: Arc<AtomicBool>,
    pub(super) volume_ramp: VolumeRampState,
    pub(super) flush_requested: Arc<AtomicBool>,
    /// Stop-armed emission latch (defense-in-depth).
    ///
    /// While set, the callback discards ring content instead of
    /// emitting it — but the latch never operatively discards: the
    /// Stop arm sets worker drop-mode in the same straight line,
    /// drop-mode clears only via a Flush (which clears the latch
    /// first) or a Resume (same), and every ring write is
    /// drop-checked, so a set latch always coincides with an empty
    /// or flag-covered ring. The operative cutoff is worker-drop
    /// plus the transient flag plus end-to-end FIFO plus the
    /// quiesce-gated terminal swap; the latch only backstops a
    /// future bypass of drop-mode. Cleared by any Resume or any
    /// Flush; its job is the no-boundary case.
    pub(super) stop_latched: AtomicBool,
    pub(super) underrun_count: Arc<AtomicU64>,
    pub(super) last_buffer_level: Arc<AtomicU64>, // For tracking buffer fill percentage
    pub(super) total_callback_samples: Arc<AtomicU64>,
    pub(super) callback_count: Arc<AtomicU64>,
    /// True while the hardware callback may still publish samples from its current buffer.
    pub(super) output_callback_active: Arc<AtomicBool>,
    pub(super) stream_error_count: Arc<AtomicU64>,
    /// Maximum finite post-volume sample magnitude since the last meter report.
    pub(super) output_peak_bits: Arc<AtomicU32>,
    /// Number of post-volume samples above 0 dBFS or non-finite since the last report.
    pub(super) clipped_sample_count: Arc<AtomicU64>,
}

impl PlaybackState {
    pub(in crate::engine) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            muted: Arc::new(AtomicBool::new(false)),
            volume_ramp: VolumeRampState::new(1.0),
            flush_requested: Arc::new(AtomicBool::new(false)),
            stop_latched: AtomicBool::new(false),
            underrun_count: Arc::new(AtomicU64::new(0)),
            last_buffer_level: Arc::new(AtomicU64::new(100)),
            total_callback_samples: Arc::new(AtomicU64::new(0)),
            callback_count: Arc::new(AtomicU64::new(0)),
            output_callback_active: Arc::new(AtomicBool::new(false)),
            stream_error_count: Arc::new(AtomicU64::new(0)),
            output_peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            clipped_sample_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Rebuild constructor sharing the meter atomics with a predecessor.
    ///
    /// Controls are configuration (copied by the caller via
    /// `copy_playback_controls`); meter is telemetry (shared, never
    /// copied). Sharing makes rebuild order irrelevant: accumulation
    /// continues into the same atomics on every path (reconfigure,
    /// sample-rate, channels, recovery), failure-path resume is
    /// symmetric (nothing moved, single atomics, no double count),
    /// and no transfer race can exist.
    pub(super) fn new_sharing_meters(
        capacity: usize,
        output_peak_bits: Arc<AtomicU32>,
        clipped_sample_count: Arc<AtomicU64>,
    ) -> Self {
        Self {
            output_peak_bits,
            clipped_sample_count,
            ..Self::new(capacity)
        }
    }

    pub(super) fn reset_output_meter(&self) {
        self.output_peak_bits
            .store(0.0f32.to_bits(), Ordering::Relaxed);
        self.clipped_sample_count.store(0, Ordering::Relaxed);
    }
}

pub(super) fn copy_playback_controls(from: &PlaybackState, to: &PlaybackState) {
    to.volume
        .store(from.volume.load(Ordering::Relaxed), Ordering::Relaxed);
    to.muted
        .store(from.muted.load(Ordering::Relaxed), Ordering::Relaxed);
    let target = if to.muted.load(Ordering::Relaxed) {
        0.0
    } else {
        f32::from_bits(to.volume.load(Ordering::Relaxed))
    };
    to.volume_ramp.snap_to(target);
    // Meter atomics are SHARED via `new_sharing_meters`, never copied
    // here: accumulation continues uninterrupted on every rebuild
    // path, and rebuild order is irrelevant. Control flags
    // (flush/latch/active) deliberately do NOT transfer: the new
    // stream starts undisrupted, and an inherited latch with no
    // upcoming boundary would wedge it silent.
}

pub(super) fn rebuild_playback_stream(
    host: &cpal::Host,
    params: RebuildPlaybackParams<'_>,
) -> Result<RebuiltPlaybackStream, String> {
    let device = select_playback_device(host, params.output_device, params.allow_virtual_output)?;
    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let mut config = StreamConfig {
        channels: params.requested_channels as u16,
        sample_rate: params.sample_rate,
        buffer_size: params.buffer_size,
    };

    let (output_format, hw_channels) = choose_output_format(&device, &config);
    if hw_channels != config.channels {
        log::info!(
            "[Playback Thread] Recovery adjusted channels from {} to {} for '{}'",
            config.channels,
            hw_channels,
            device_name
        );
        config.channels = hw_channels;
    }

    let channels = hw_channels as usize;
    let buffer_capacity = playback_buffer_capacity(params.sample_rate, channels, params.buffer_ms);
    let (mut producer, consumer) = RingBuffer::<f32>::new(buffer_capacity);
    let state = Arc::new(PlaybackState::new_sharing_meters(
        buffer_capacity,
        Arc::clone(&params.old_state.output_peak_bits),
        Arc::clone(&params.old_state.clipped_sample_count),
    ));
    copy_playback_controls(params.old_state, &state);
    prefill_silence(&mut producer, buffer_capacity / 2);

    let stream = build_output_stream(
        &device,
        &config,
        Arc::clone(&state),
        params.event_tx,
        consumer,
        output_format,
    )?;
    stream
        .play()
        .map_err(|e| format!("Failed to start recovered stream: {}", e))?;

    Ok(RebuiltPlaybackStream {
        device,
        device_name,
        stream,
        producer,
        state,
        config,
        output_format,
        channels,
        logical_channels: params.requested_channels,
        buffer_capacity,
    })
}

pub(super) fn request_flush(state: &PlaybackState) {
    state.flush_requested.store(true, Ordering::Relaxed);
}

pub(super) fn flush_completed(
    state: &PlaybackState,
    producer: &Producer<f32>,
    buffer_capacity: usize,
) -> bool {
    if state.flush_requested.load(Ordering::Relaxed) && producer.slots() >= buffer_capacity {
        state.flush_requested.store(false, Ordering::Relaxed);
    }

    !state.flush_requested.load(Ordering::Relaxed)
        && !state.output_callback_active.load(Ordering::Acquire)
}

/// Read f32 samples from the ring buffer into a scratch buffer.
/// Returns `true` if an underrun occurred (not enough data). Handles underrun by zero-filling.
#[inline(always)]
pub(in crate::engine) fn read_ring_buffer(
    consumer: &mut Consumer<f32>,
    scratch: &mut [f32],
    requested: usize,
    state: &PlaybackState,
    capacity: usize,
) -> bool {
    // Fused discard: a latched Stop discards exactly like a flush
    // request. Relaxed staleness is benign — both paths discard and
    // emit zeros; any sample emitted before the latch is observed is
    // still metered and captured by the terminal swap before the ack.
    if state.flush_requested.load(Ordering::Relaxed) || state.stop_latched.load(Ordering::Relaxed) {
        let available = consumer.slots().min(requested);
        if available > 0
            && let Ok(chunk) = consumer.read_chunk(available)
        {
            chunk.commit_all();
        }
        scratch[..requested].fill(0.0);

        if consumer.slots() == 0 {
            state.flush_requested.store(false, Ordering::Relaxed);
        }
        // stop_latched persists here by design: only a Resume or a
        // stream Flush clears it, so arrivals with no subsequent
        // boundary keep discarding instead of emitting.

        let fill_percent = (consumer.slots() * 100).checked_div(capacity).unwrap_or(0);
        state
            .last_buffer_level
            .store(fill_percent as u64, Ordering::Relaxed);

        return false;
    }

    let mut underrun = false;

    // Try to read requested amount
    if let Ok(chunk) = consumer.read_chunk(requested) {
        let (first, second) = chunk.as_slices();
        let first_len = first.len();
        let second_len = second.len();

        if first_len > 0 {
            scratch[..first_len].copy_from_slice(first);
        }
        if second_len > 0 {
            scratch[first_len..first_len + second_len].copy_from_slice(second);
        }

        chunk.commit_all();
        state
            .total_callback_samples
            .fetch_add(requested as u64, Ordering::Relaxed);
    } else {
        // Not enough data (underrun)
        let available = consumer.slots().min(requested);

        if let Ok(chunk) = consumer.read_chunk(available) {
            let (first, second) = chunk.as_slices();
            let first_len = first.len();
            let second_len = second.len();

            if first_len > 0 {
                scratch[..first_len].copy_from_slice(first);
            }
            if second_len > 0 {
                scratch[first_len..first_len + second_len].copy_from_slice(second);
            }
            chunk.commit_all();
        }
        state
            .total_callback_samples
            .fetch_add(available as u64, Ordering::Relaxed);

        // Zero pad the rest
        if available < requested {
            scratch[available..requested].fill(0.0);
        }

        underrun = true;
        state.underrun_count.fetch_add(1, Ordering::Relaxed);
    }

    // Update buffer level metric
    let slots = consumer.slots();
    let fill_percent = (slots * 100).checked_div(capacity).unwrap_or(0);
    state
        .last_buffer_level
        .store(fill_percent as u64, Ordering::Relaxed);

    underrun
}
