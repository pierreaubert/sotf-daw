use crate::engine::volume_ramp::VolumeRampState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub(super) struct PlaybackState {
    pub(super) capacity: usize,
    pub(super) volume: Arc<AtomicU32>,
    pub(super) muted: Arc<AtomicBool>,
    pub(super) volume_ramp: VolumeRampState,
    pub(super) flush_requested: Arc<AtomicBool>,
    pub(super) callback_active: AtomicBool,
    /// Window peak accumulator (post-volume, pre-clamp magnitudes).
    ///
    /// Same names/types/observe rule as the desktop runtime: the render
    /// callback `fetch_max`es each window's peak; the feeder swaps the
    /// residual into the epoch cumulative at drains and Stop terminals.
    /// There is no periodic live report on the stub (accepted P4.5 gap).
    pub(super) output_peak_bits: Arc<AtomicU32>,
    /// Window clipped-sample counter (same observe rule as desktop).
    ///
    /// Swapped alongside the peak at every swap point; the count has no
    /// live report channel on the stub (P4.5 gap) — the counting rule
    /// itself is the parity, observed by tests between swaps.
    pub(super) clipped_sample_count: Arc<AtomicU64>,
    /// Stop-armed emission latch.
    ///
    /// Same contract as the desktop runtime: while set, the render
    /// callback discards ring content instead of emitting it — but,
    /// as on desktop, the latch is defense-in-depth (worker
    /// drop-mode, set in the same Stop arm, does the cutting off;
    /// every ring write is drop-checked). Cleared by any Resume or
    /// any Flush; its job is the no-boundary case.
    pub(super) stop_latched: AtomicBool,
}

impl PlaybackState {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            muted: Arc::new(AtomicBool::new(false)),
            volume_ramp: VolumeRampState::new(1.0),
            flush_requested: Arc::new(AtomicBool::new(false)),
            callback_active: AtomicBool::new(false),
            stop_latched: AtomicBool::new(false),
            output_peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            clipped_sample_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Build state sharing a wrapper-created peak atomic (P4.6 death fold).
    ///
    /// The wrapper retains its clone past thread exit; the render
    /// callback accumulates into the shared atomic, so the manager can
    /// fold the residual after a worker death. No rebuild paths exist
    /// on iOS, so one share lasts forever. The clip counter stays
    /// state-local (fresh zero): clips have no death-fold consumer.
    pub(super) fn new_sharing_peak(capacity: usize, output_peak_bits: Arc<AtomicU32>) -> Self {
        Self {
            capacity,
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            muted: Arc::new(AtomicBool::new(false)),
            volume_ramp: VolumeRampState::new(1.0),
            flush_requested: Arc::new(AtomicBool::new(false)),
            callback_active: AtomicBool::new(false),
            stop_latched: AtomicBool::new(false),
            output_peak_bits,
            clipped_sample_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Zero both meter atomics (desktop `reset_output_meter` mirror).
    ///
    /// Used by the epoch fence on abandoned tails (no terminal swap
    /// ran): the old epoch has no record, so its residual is dropped
    /// by design rather than carried.
    pub(super) fn reset_output_meter(&self) {
        self.output_peak_bits
            .store(0.0f32.to_bits(), Ordering::Relaxed);
        self.clipped_sample_count.store(0, Ordering::Relaxed);
    }
}
