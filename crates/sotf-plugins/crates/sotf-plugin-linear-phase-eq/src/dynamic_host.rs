//! Dynamic linear-phase EQ updates for real hosts.
//!
//! Shared wrapper owning the realtime-safe host path for single-band FIR
//! updates: control snapshots, a worker prepares, audio commits once per
//! quantum, and control reclaims. Band shape edits (`type`, `freq`, `q`,
//! `gain`, `active`) use the prepared-crossfade API; placement, count, taps,
//! phase mode and auto gain stay structural with rebuild-path refusal.
//!
//! Two control paths converge on the same audio slots. Same-thread callers use
//! `&mut` methods between renders (`request_band_update`,
//! `submit_prepared_update`, `reclaim_retired`). Concurrent callers share the
//! `Arc<LinearPhaseEqControlHandle>` from `get_data` (or `control_handle`):
//! workers prepare from control snapshots and submit through the bounded
//! lock-free mailbox; audio pops the mailbox into the retained slots before
//! each commit, and pushes retired banks back for off-audio reclaim. No
//! locks, blocking calls, allocation, free, formatting or logging run on
//! audio. Queue-full, stale, reset and error payloads stay owned until
//! off-audio reclaim. `set_parameter` keeps the generic structural refusal
//! for `band_*`; native and FFI ordinary setters stay structural until their
//! own adoption.
//!
//! Queue capacity is two prepared updates plus a two-slot mailbox, so two
//! edits issued during a blend are retained and eventually applied after
//! retirement. Further concurrent requests fail loudly with the payload
//! dropped on the calling control thread. A permanently refused head (stale,
//! invalid, topology) retains until the caller explicitly cancels it via the
//! handle; audio evicts at most one head per quantum into a bounded outbox
//! for off-audio destruction, keeping any fresh queued edit, so later valid
//! updates apply without rebuilding the chain.
//!
//! Detached consumers (engine manager, UI) that only hold the `get_data`
//! handle observe immutable accepted state without wrapper access: audio
//! publishes each accepted band shape under a seqlock generation plus
//! lock-free refusal and queue mirrors. Control reads the accepted base with
//! `try_accepted_snapshot` (allocates; control only), polls generations with
//! `accepted_generation`, and reads detached status with `control_status`
//! (each field current as of its own load, not an atomic snapshot). A queued
//! submission is never presented as accepted: the
//! generation and snapshot advance only inside the real audio commit, and the
//! original exact-base stale validation is unchanged. Audio publication is a
//! fixed handful of atomic stores per quantum: no allocation, free, lock,
//! formatting or logging.
//!
//! ```rust,ignore
//! // Same-thread control between renders.
//! plugin.request_band_update(0, band)?;
//! // Concurrent worker via the shared handle.
//! let handle = plugin.control_handle();
//! let prepared = LinearPhaseEqPlugin::prepare_band_update(&base, 0, band)?;
//! handle.try_submit(prepared)?;
//! // Audio commits inside `process`; control reclaims between quanta.
//! handle.try_reclaim();
//! // Recover a permanently refused head, then submit a fresh edit.
//! handle.request_cancel();
//! handle.try_reclaim_cancelled();
//! // Detached observation needs only the `get_data` handle.
//! let base = handle.try_accepted_snapshot()?;
//! let status = handle.control_status();
//! ```

// Rust guideline compliant 2026-02-21

use crate::{
    BandConfig, BandSnapshot, CommitRefusal, LinearPhaseEqBandPlacement, LinearPhaseEqPlugin,
    LinearPhaseEqPluginParams, LiveFilterSnapshot, PreparedBandUpdate, RouteBanks,
};
use crate::params::MAX_FILTERS;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginDrainResult,
    PluginInfo, PluginResult, ProcessContext, TailLength, validate_process_block_f32,
    validate_process_block_f64,
};
use sotf_host::rt_mailbox::{RtConsumer, RtProducer, RtPushError, rt_mailbox};
use std::any::Any;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Retained audio slots plus mailbox depth for prepared updates.
///
/// Two audio slots (`pending` plus one queued edit) cover the required two
/// edits during a blend; the two-slot mailbox absorbs concurrent worker
/// submissions between quanta. All bounds are fixed at construction.
const PREPARED_CAPACITY: usize = 2;
/// Mailbox depth for retired banks awaiting off-audio reclaim.
///
/// One retired set per completed blend; two covers a completed blend plus an
/// in-flight holding slot while control catches up.
const RETIRED_CAPACITY: usize = 2;
/// Mailbox depth for cancelled head payloads awaiting off-audio destruction.
///
/// Audio evicts at most one head per quantum on explicit cancel request;
/// two covers back-to-back cancels before control drains the outbox.
const CANCELLED_CAPACITY: usize = 2;
/// Bounded mailbox pops per audio quantum and per reclaim call.
///
/// Fixed small bound; no unbounded drain loops on either thread.
const MAILBOX_POP_BOUND: usize = 4;
/// Bounded seqlock read attempts for accepted snapshots.
///
/// One initial attempt plus this many retries; a publication overlaps a read
/// only while audio commits (rare), so control retries stay cheap.
const ACCEPTED_SNAPSHOT_RETRIES: u32 = 4;
/// Refusal mirror encoding for "no refusal recorded".
///
/// Every encoded refusal packs discriminant and indices below this value.
const REFUSAL_NONE: u32 = u32::MAX;
/// Status word bits holding the retained queue count (`0..=2`).
const STATUS_RETAINED_MASK: u64 = 0b11;
/// Status word bit reporting an in-flight accepted blend.
const STATUS_BLEND_BIT: u64 = 1 << 2;
/// Status word bit reporting an occupied retired holding slot.
const STATUS_RETIRED_HOLD_BIT: u64 = 1 << 3;

/// Neutral unread shape padding band cells beyond the configured prefix.
///
/// Only the configured `num_filters` prefix is ever published or read; this
/// keeps construction infallible when the initial snapshot is short.
const NEUTRAL_BAND: BandSnapshot = BandSnapshot {
    filter_type_index: 0,
    frequency: 1000.0,
    q: 1.0,
    gain_db: 0.0,
    active: true,
    placement: None,
};

/// Immutable accepted filter configuration with its generation.
///
/// Returned by
/// [`LinearPhaseEqControlHandle::try_accepted_snapshot`]. The snapshot is an
/// owned clone for worker preparation; the generation counts accepted
/// publications and advances only when real audio accepts a commit (or a
/// control-side rate change republishes).
#[derive(Debug, Clone, PartialEq)]
pub struct LinearPhaseEqAcceptedSnapshot {
    /// Accepted publication count (`0` is the initial construction state).
    pub generation: u64,
    /// Accepted configuration; never a queued-but-uncommitted candidate.
    pub snapshot: LiveFilterSnapshot,
}

/// Detached control status with individually observed fields.
///
/// Returned by [`LinearPhaseEqControlHandle::control_status`]. Every field is
/// a lock-free mirror read without allocation or blocking, but the struct is
/// not an atomic multi-field snapshot: each field is current as of its own
/// load, so fields may straddle audio quanta while audio runs. Polling
/// converges; the generation advances only on real audio commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearPhaseEqControlStatus {
    /// Last stable accepted publication count (`0` is initial).
    pub accepted_generation: u64,
    /// Retained audio-slot updates (`0..=2`, excluding the mailbox).
    pub retained_queued: u8,
    /// Whether an accepted blend is still morphing audio.
    pub blend_in_progress: bool,
    /// Whether the retired holding slot awaits off-audio reclamation.
    pub retired_held: bool,
    /// Last audio commit refusal mirror, if any.
    ///
    /// The discriminant is always exact. `BandIndexOutOfRange` indices
    /// saturate at 255; every reachable prepared update carries indices
    /// below 10, so reachable mirrors decode exactly.
    pub last_refusal: Option<CommitRefusal>,
}

/// Seqlock contention signal for accepted snapshot reads.
///
/// Returned when an audio publication overlapped every bounded read attempt.
/// Control callers should retry the read later; the previously read accepted
/// base stays valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearPhaseEqSnapshotBusy;

impl std::fmt::Display for LinearPhaseEqSnapshotBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("accepted snapshot busy: audio publication overlapped every read retry")
    }
}

impl std::error::Error for LinearPhaseEqSnapshotBusy {}

/// Lock-free accepted shape storage for one band slot.
///
/// One cell per slot up to `MAX_FILTERS`; only the configured prefix is
/// ever published or read. The single writer (audio commit, plus
/// construction before sharing) brackets stores in the handle seqlock;
/// readers load between `Acquire` generation checks.
struct AcceptedBandCells {
    /// Low 8 bits: filter type index; bit 8: active flag.
    type_and_active: AtomicU64,
    /// `f64::to_bits` frequency.
    frequency_bits: AtomicU64,
    /// `f64::to_bits` Q.
    q_bits: AtomicU64,
    /// `f64::to_bits` gain in dB.
    gain_bits: AtomicU64,
}

impl AcceptedBandCells {
    fn new(shape: &BandSnapshot) -> Self {
        let cells = Self {
            type_and_active: AtomicU64::new(0),
            frequency_bits: AtomicU64::new(0),
            q_bits: AtomicU64::new(0),
            gain_bits: AtomicU64::new(0),
        };
        cells.store(shape);
        cells
    }

    /// Store one accepted shape.
    ///
    /// Single-writer only (construction or the audio commit step inside a
    /// seqlock bracket); `Relaxed` stores are ordered by the surrounding
    /// `Release` generation updates.
    fn store(&self, shape: &BandSnapshot) {
        let packed = shape.filter_type_index.min(u8::MAX as usize) as u64
            | (u64::from(shape.active) << 8);
        self.type_and_active.store(packed, Ordering::Relaxed);
        self.frequency_bits
            .store(shape.frequency.to_bits(), Ordering::Relaxed);
        self.q_bits.store(shape.q.to_bits(), Ordering::Relaxed);
        self.gain_bits
            .store(shape.gain_db.to_bits(), Ordering::Relaxed);
    }

    /// Load one accepted shape.
    ///
    /// Control readers call this between seqlock generation checks; the
    /// placement comes from immutable construction state.
    fn load(&self, placement: Option<LinearPhaseEqBandPlacement>) -> BandSnapshot {
        let packed = self.type_and_active.load(Ordering::Relaxed);
        BandSnapshot {
            filter_type_index: (packed & 0xFF) as usize,
            frequency: f64::from_bits(self.frequency_bits.load(Ordering::Relaxed)),
            q: f64::from_bits(self.q_bits.load(Ordering::Relaxed)),
            gain_db: f64::from_bits(self.gain_bits.load(Ordering::Relaxed)),
            active: packed & (1 << 8) != 0,
            placement,
        }
    }
}

/// Encode a refusal as discriminant plus saturated index payload.
fn refusal_code(refusal: CommitRefusal) -> (u8, u8, u8) {
    match refusal {
        CommitRefusal::NoPreparedUpdate => (0, 0, 0),
        CommitRefusal::UpdateInProgress => (1, 0, 0),
        CommitRefusal::RetiredUnclaimed => (2, 0, 0),
        CommitRefusal::Drained => (3, 0, 0),
        CommitRefusal::StaleBase => (4, 0, 0),
        CommitRefusal::BandIndexOutOfRange { index, live } => {
            (5, index.min(255) as u8, live.min(255) as u8)
        }
        CommitRefusal::PlacementMismatch => (6, 0, 0),
        CommitRefusal::TopologyMismatch => (7, 0, 0),
        CommitRefusal::FirLengthMismatch => (8, 0, 0),
        CommitRefusal::NoStage => (9, 0, 0),
        CommitRefusal::TargetNotFresh => (10, 0, 0),
        CommitRefusal::InvalidBand => (11, 0, 0),
    }
}

/// Encode a refusal mirror word (`REFUSAL_NONE` when clear).
fn encode_refusal(refusal: Option<CommitRefusal>) -> u32 {
    let Some(reason) = refusal else {
        return REFUSAL_NONE;
    };
    let (discriminant, first, second) = refusal_code(reason);
    (u32::from(discriminant) << 16) | (u32::from(first) << 8) | u32::from(second)
}

/// Decode a refusal mirror word.
///
/// Unknown discriminants decode to `None`; only `encode_refusal` writes the
/// mirror, so unknown codes are unreachable through the public API.
fn decode_refusal(word: u32) -> Option<CommitRefusal> {
    if word == REFUSAL_NONE {
        return None;
    }
    let discriminant = (word >> 16) & 0xFF;
    let first = ((word >> 8) & 0xFF) as usize;
    let second = (word & 0xFF) as usize;
    match discriminant {
        0 => Some(CommitRefusal::NoPreparedUpdate),
        1 => Some(CommitRefusal::UpdateInProgress),
        2 => Some(CommitRefusal::RetiredUnclaimed),
        3 => Some(CommitRefusal::Drained),
        4 => Some(CommitRefusal::StaleBase),
        5 => Some(CommitRefusal::BandIndexOutOfRange {
            index: first,
            live: second,
        }),
        6 => Some(CommitRefusal::PlacementMismatch),
        7 => Some(CommitRefusal::TopologyMismatch),
        8 => Some(CommitRefusal::FirLengthMismatch),
        9 => Some(CommitRefusal::NoStage),
        10 => Some(CommitRefusal::TargetNotFresh),
        11 => Some(CommitRefusal::InvalidBand),
        _ => None,
    }
}

/// Shared control handle for concurrent dynamic updates.
///
/// Obtained via `get_data` (typed `Arc` transport) or `control_handle`.
/// Control and worker threads submit prepared updates, request head cancel,
/// and reclaim retired or cancelled payloads through `&self` methods; audio
/// never locks. Queue methods are nonblocking with a single `try_lock`
/// attempt; cancel is a lock-free sequence increment. Detached readers that
/// only hold this handle observe immutable accepted state through
/// `try_accepted_snapshot`, `accepted_generation` and `control_status`;
/// those mirrors advance only when real audio accepts a commit, so a queued
/// submission is never presented as accepted.
pub struct LinearPhaseEqControlHandle {
    prepared_tx: Mutex<RtProducer<PreparedBandUpdate>>,
    retired_rx: Mutex<RtConsumer<RouteBanks>>,
    cancelled_rx: Mutex<RtConsumer<PreparedBandUpdate>>,
    cancel_seq: AtomicU64,
    /// Immutable construction topology; set before sharing, never mutated.
    accepted_channels: usize,
    /// Immutable configured band count, clamped to readable cells.
    accepted_num_filters: usize,
    /// Immutable FIR length index.
    accepted_fir_length_index: usize,
    /// Immutable phase mode index.
    accepted_phase_mode_index: usize,
    /// Immutable auto gain flag.
    accepted_auto_gain: bool,
    /// Immutable per-band placements for the configured prefix.
    accepted_placements: [Option<LinearPhaseEqBandPlacement>; MAX_FILTERS],
    /// Immutable explicit stereo pairs.
    accepted_pairs: Vec<[usize; 2]>,
    /// Accepted sample rate, published at construction and on rate change.
    accepted_sample_rate: AtomicU32,
    /// Seqlock accepted generation (even when stable, odd mid-publication).
    accepted_gen: AtomicU64,
    /// Accepted per-band shapes for the configured prefix.
    accepted_bands: [AcceptedBandCells; MAX_FILTERS],
    /// Last audio commit refusal mirror (`REFUSAL_NONE` when clear).
    refusal_mirror: AtomicU32,
    /// Packed queue/blend/retired mirror (see `STATUS_*` bits).
    status_word: AtomicU64,
}

impl std::fmt::Debug for LinearPhaseEqControlHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearPhaseEqControlHandle")
            .finish_non_exhaustive()
    }
}

impl LinearPhaseEqControlHandle {
    /// Queue a worker-prepared update for audio commit.
    ///
    /// Nonblocking control call. On a full mailbox or a contended lock the
    /// payload drops on the calling thread and a loud error returns; live
    /// config and history stay untouched.
    ///
    /// # Errors
    ///
    /// Returns a full-queue or busy error without touching live state.
    pub fn try_submit(&self, prepared: PreparedBandUpdate) -> Result<(), String> {
        let mut producer = match self.prepared_tx.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                drop(prepared);
                return Err("linear-phase EQ control queue busy; retry later".to_string());
            }
        };
        match producer.push(prepared) {
            Ok(()) => Ok(()),
            Err(RtPushError::Full(returned)) => {
                drop(returned);
                Err("linear-phase EQ dynamic queue full".to_string())
            }
        }
    }

    /// Drop reclaimable retired banks on the calling thread.
    ///
    /// Nonblocking control call with a bounded pop count. Returns the number
    /// reclaimed. A contended lock reclaims nothing; retry later.
    pub fn try_reclaim(&self) -> usize {
        let mut consumer = match self.retired_rx.try_lock() {
            Ok(guard) => guard,
            Err(_) => return 0,
        };
        let mut reclaimed = 0;
        for _ in 0..MAILBOX_POP_BOUND {
            match consumer.pop() {
                Ok(banks) => {
                    reclaimed += 1;
                    drop(banks);
                }
                Err(_) => break,
            }
        }
        reclaimed
    }

    /// Request eviction of the audio head slot.
    ///
    /// Lock-free control call recording one coalescing cancel generation.
    /// Audio evicts at most one head payload per quantum into the cancelled
    /// outbox (pending preferred, else queued) without dropping; a full
    /// outbox defers eviction until control drains it. Observe completion via
    /// the wrapper queue length plus `try_reclaim_cancelled` count.
    pub fn request_cancel(&self) {
        self.cancel_seq.fetch_add(1, Ordering::Release);
    }

    /// Drop cancelled head payloads on the calling thread.
    ///
    /// Nonblocking control call with a bounded pop count. Returns the number
    /// destroyed off audio. A contended lock reclaims nothing; retry later.
    pub fn try_reclaim_cancelled(&self) -> usize {
        let mut consumer = match self.cancelled_rx.try_lock() {
            Ok(guard) => guard,
            Err(_) => return 0,
        };
        let mut reclaimed = 0;
        for _ in 0..MAILBOX_POP_BOUND {
            match consumer.pop() {
                Ok(prepared) => {
                    reclaimed += 1;
                    drop(prepared);
                }
                Err(_) => break,
            }
        }
        reclaimed
    }

    /// Move cancelled head payloads out for worker-thread drop.
    ///
    /// Nonblocking control call with a bounded pop count. Returns owned
    /// payloads for the caller to send to a worker thread; dropping the
    /// returned vector frees prepared state off audio. An empty vector means
    /// nothing was pending or the lock was contended.
    pub fn take_cancelled(&self) -> Vec<PreparedBandUpdate> {
        let mut payloads = Vec::new();
        let Ok(mut consumer) = self.cancelled_rx.try_lock() else {
            return payloads;
        };
        for _ in 0..MAILBOX_POP_BOUND {
            match consumer.pop() {
                Ok(prepared) => payloads.push(prepared),
                Err(_) => break,
            }
        }
        payloads
    }

    /// Read the last stable accepted generation.
    ///
    /// Infallible lock-free control poll returning the count of accepted
    /// publications (`0` is the initial construction state). The count
    /// advances by one per accepted audio commit, or when a control-side
    /// rate change republishes; a queued submission never advances it.
    pub fn accepted_generation(&self) -> u64 {
        (self.accepted_gen.load(Ordering::Acquire) & !1) >> 1
    }

    /// Read the immutable accepted snapshot for worker preparation.
    ///
    /// Control thread only; allocates the owned snapshot plus band and pair
    /// vectors. The returned base is accepted live state, never a queued
    /// candidate: hand it to `LinearPhaseEqPlugin::prepare_band_update` on a
    /// worker, then deliver with `try_submit`. Orphaned handles (wrapper
    /// dropped after a rebuild) keep returning their frozen last state.
    ///
    /// # Errors
    ///
    /// Returns [`LinearPhaseEqSnapshotBusy`] when an audio publication
    /// overlapped every bounded read attempt; retry later.
    pub fn try_accepted_snapshot(
        &self,
    ) -> Result<LinearPhaseEqAcceptedSnapshot, LinearPhaseEqSnapshotBusy> {
        for _ in 0..=ACCEPTED_SNAPSHOT_RETRIES {
            let before = self.accepted_gen.load(Ordering::Acquire);
            if before & 1 == 1 {
                continue;
            }
            let sample_rate = self.accepted_sample_rate.load(Ordering::Relaxed);
            let mut bands = Vec::with_capacity(self.accepted_num_filters);
            for (index, cells) in self
                .accepted_bands
                .iter()
                .enumerate()
                .take(self.accepted_num_filters)
            {
                bands.push(cells.load(self.accepted_placements[index]));
            }
            let after = self.accepted_gen.load(Ordering::Acquire);
            if before == after {
                return Ok(LinearPhaseEqAcceptedSnapshot {
                    generation: before >> 1,
                    snapshot: LiveFilterSnapshot {
                        channels: self.accepted_channels,
                        sample_rate,
                        num_filters: self.accepted_num_filters,
                        fir_length_index: self.accepted_fir_length_index,
                        phase_mode_index: self.accepted_phase_mode_index,
                        auto_gain: self.accepted_auto_gain,
                        bands,
                        stereo_pairs: self.accepted_pairs.clone(),
                    },
                });
            }
        }
        Err(LinearPhaseEqSnapshotBusy)
    }

    /// Read detached control status with individually observed fields.
    ///
    /// Infallible lock-free control poll: no allocation, free, lock or wait.
    /// Each field is current as of its own load rather than an atomic
    /// multi-field snapshot, so fields may straddle audio quanta; the
    /// generation and refusal match the wrapper getters once the publishing
    /// quantum completes, and polling converges with no false-pass
    /// direction (refusal clears before the status publish it pairs with).
    pub fn control_status(&self) -> LinearPhaseEqControlStatus {
        let word = self.status_word.load(Ordering::Relaxed);
        LinearPhaseEqControlStatus {
            accepted_generation: self.accepted_generation(),
            retained_queued: (word & STATUS_RETAINED_MASK) as u8,
            blend_in_progress: word & STATUS_BLEND_BIT != 0,
            retired_held: word & STATUS_RETIRED_HOLD_BIT != 0,
            last_refusal: decode_refusal(self.refusal_mirror.load(Ordering::Relaxed)),
        }
    }

    /// Publish one accepted band shape under the seqlock.
    ///
    /// Audio commit step only: brackets the cell stores in odd/even
    /// generation updates with `Release` ordering. Fixed four atomic stores;
    /// no allocation, free or lock.
    pub(crate) fn publish_accepted_band(&self, band_index: usize, shape: &BandSnapshot) {
        let Some(cells) = self.accepted_bands.get(band_index) else {
            debug_assert!(false, "committed band index exceeds published cells");
            return;
        };
        self.accepted_gen.fetch_add(1, Ordering::Release);
        cells.store(shape);
        self.accepted_gen.fetch_add(1, Ordering::Release);
    }

    /// Publish an accepted sample rate under the seqlock.
    ///
    /// Exclusive `&mut` wrapper context only (rate-change initialize runs
    /// outside audio and never concurrently with the commit step). Bumps the
    /// accepted generation; band shapes are unchanged by a rate change.
    pub(crate) fn publish_sample_rate(&self, sample_rate: u32) {
        self.accepted_gen.fetch_add(1, Ordering::Release);
        self.accepted_sample_rate
            .store(sample_rate, Ordering::Relaxed);
        self.accepted_gen.fetch_add(1, Ordering::Release);
    }

    /// Mirror the last commit refusal (or its clearing).
    ///
    /// Single lock-free store from the audio commit step or exclusive
    /// same-thread control; no allocation, free or lock.
    pub(crate) fn publish_refusal(&self, refusal: Option<CommitRefusal>) {
        self.refusal_mirror
            .store(encode_refusal(refusal), Ordering::Relaxed);
    }

    /// Mirror retained queue, blend and holding-slot state.
    ///
    /// Single lock-free store from the audio commit step or exclusive
    /// same-thread control; no allocation, free or lock.
    pub(crate) fn publish_queue_status(
        &self,
        retained: usize,
        blend_in_progress: bool,
        retired_held: bool,
    ) {
        let mut word = retained.min(STATUS_RETAINED_MASK as usize) as u64;
        if blend_in_progress {
            word |= STATUS_BLEND_BIT;
        }
        if retired_held {
            word |= STATUS_RETIRED_HOLD_BIT;
        }
        self.status_word.store(word, Ordering::Relaxed);
    }

    /// Read the published accepted sample rate.
    ///
    /// Used by the wrapper to detect rate-change initialize calls without an
    /// allocating snapshot; always equals the live DSP rate.
    pub(crate) fn published_sample_rate(&self) -> u32 {
        self.accepted_sample_rate.load(Ordering::Relaxed)
    }
}

/// Bridge host wrapper with realtime-safe dynamic band updates.
///
/// Holds the live DSP plus retained audio slots, mailbox endpoints and a
/// shared control handle. Factory construction delegates to the DSP
/// constructor, so defaults, phase, latency, static audio and finite EOF
/// match the static plugin exactly.
pub struct LinearPhaseEqDynamicPlugin {
    inner: LinearPhaseEqPlugin,
    pending: Option<PreparedBandUpdate>,
    queued: Option<PreparedBandUpdate>,
    retired_hold: Option<RouteBanks>,
    prepared_rx: RtConsumer<PreparedBandUpdate>,
    retired_tx: RtProducer<RouteBanks>,
    cancelled_tx: RtProducer<PreparedBandUpdate>,
    handle: Arc<LinearPhaseEqControlHandle>,
    cancel_ack: u64,
    last_refusal: Option<CommitRefusal>,
    scratch_f64: Vec<f32>,
}

impl std::fmt::Debug for LinearPhaseEqDynamicPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearPhaseEqDynamicPlugin")
            .field("pending", &self.pending.is_some())
            .field("queued", &self.queued.is_some())
            .field("retired_hold", &self.retired_hold.is_some())
            .field("last_refusal", &self.last_refusal)
            .finish_non_exhaustive()
    }
}

impl LinearPhaseEqDynamicPlugin {
    /// Build a dynamic wrapper from factory parameters.
    ///
    /// Delegates to the DSP constructor and allocates the bounded mailboxes.
    /// Control thread only. The prepared queue starts empty.
    ///
    /// # Errors
    ///
    /// Returns the DSP construction error for invalid rates, mixes, bands or
    /// topologies.
    pub fn from_params(
        channels: usize,
        sample_rate: u32,
        params: LinearPhaseEqPluginParams,
    ) -> Result<Self, String> {
        let inner = LinearPhaseEqPlugin::from_params(channels, sample_rate, params)?;
        let (prepared_tx, prepared_rx) =
            rt_mailbox::<PreparedBandUpdate>(PREPARED_CAPACITY);
        let (retired_tx, retired_rx) = rt_mailbox::<RouteBanks>(RETIRED_CAPACITY);
        let (cancelled_tx, cancelled_rx) =
            rt_mailbox::<PreparedBandUpdate>(CANCELLED_CAPACITY);
        // Seed detached accepted state from the initial live configuration.
        // Control thread only; the snapshot allocates once here and the
        // handle is not yet shared, so plain stores need no fencing.
        let initial = inner.snapshot_config();
        let accepted_num_filters = initial
            .num_filters
            .min(MAX_FILTERS)
            .min(initial.bands.len());
        let mut accepted_placements = [None; MAX_FILTERS];
        for (slot, band) in accepted_placements.iter_mut().zip(initial.bands.iter()) {
            *slot = band.placement;
        }
        let handle = Arc::new(LinearPhaseEqControlHandle {
            prepared_tx: Mutex::new(prepared_tx),
            retired_rx: Mutex::new(retired_rx),
            cancelled_rx: Mutex::new(cancelled_rx),
            cancel_seq: AtomicU64::new(0),
            accepted_channels: initial.channels,
            accepted_num_filters,
            accepted_fir_length_index: initial.fir_length_index,
            accepted_phase_mode_index: initial.phase_mode_index,
            accepted_auto_gain: initial.auto_gain,
            accepted_placements,
            accepted_pairs: initial.stereo_pairs.clone(),
            accepted_sample_rate: AtomicU32::new(initial.sample_rate),
            accepted_gen: AtomicU64::new(0),
            accepted_bands: std::array::from_fn(|index| {
                AcceptedBandCells::new(initial.bands.get(index).unwrap_or(&NEUTRAL_BAND))
            }),
            refusal_mirror: AtomicU32::new(REFUSAL_NONE),
            status_word: AtomicU64::new(0),
        });
        Ok(Self {
            inner,
            pending: None,
            queued: None,
            retired_hold: None,
            prepared_rx,
            retired_tx,
            cancelled_tx,
            handle,
            cancel_ack: 0,
            last_refusal: None,
            scratch_f64: Vec::new(),
        })
    }

    /// Clone the shared control handle for worker threads.
    ///
    /// The same `Arc` is returned by `get_data` for engine and host routing.
    /// Cloning increments the refcount without allocating.
    pub fn control_handle(&self) -> Arc<LinearPhaseEqControlHandle> {
        Arc::clone(&self.handle)
    }

    /// Capture live config for worker-side preparation.
    ///
    /// Control thread only; allocates the snapshot. Hand the snapshot plus a
    /// band edit to `LinearPhaseEqPlugin::prepare_band_update` on a worker,
    /// then deliver the result with `submit_prepared_update` (same thread) or
    /// `LinearPhaseEqControlHandle::try_submit` (concurrent handle).
    pub fn snapshot_for_update(&self) -> LiveFilterSnapshot {
        self.inner.snapshot_config()
    }

    /// Queue a worker-prepared update on the calling thread.
    ///
    /// Same-thread control path between renders. Fills `pending` first, then
    /// `queued`. When both are occupied the call drops `prepared` on the
    /// calling thread and reports full; live config and history stay
    /// untouched. Concurrent callers use the shared handle mailbox instead.
    ///
    /// # Errors
    ///
    /// Returns a full-queue error when two updates are already retained.
    pub fn submit_prepared_update(
        &mut self,
        prepared: PreparedBandUpdate,
    ) -> Result<(), String> {
        if self.pending.is_none() {
            self.pending = Some(prepared);
            self.publish_status();
            return Ok(());
        }
        if self.queued.is_none() {
            self.queued = Some(prepared);
            self.publish_status();
            return Ok(());
        }
        debug_assert_eq!(self.pending_len(), PREPARED_CAPACITY);
        drop(prepared);
        Err("linear-phase EQ dynamic queue full (2 pending)".to_string())
    }

    /// Snapshot, prepare and queue one band edit on control.
    ///
    /// Convenience for control-thread callers without a dedicated worker. Uses
    /// the existing DSP preparation API, so topology and range validation
    /// match the worker path exactly.
    ///
    /// # Errors
    ///
    /// Returns preparation errors (index, placement, range) or a full-queue
    /// error. All errors leave live state untouched.
    pub fn request_band_update(
        &mut self,
        band_index: usize,
        new_band: BandConfig,
    ) -> Result<(), String> {
        let base = self.inner.snapshot_config();
        let prepared =
            LinearPhaseEqPlugin::prepare_band_update(&base, band_index, new_band)?;
        self.submit_prepared_update(prepared)
    }

    /// Drop reclaimable retired banks on the calling thread.
    ///
    /// Same-thread control path between quanta. Drains the retired mailbox
    /// with a bounded nonblocking attempt, then takes the holding slot plus
    /// any DSP-retired route. Call after a blend completes and before relying
    /// on the next commit succeeding.
    pub fn reclaim_retired(&mut self) -> bool {
        let mut reclaimed = self.handle.try_reclaim() > 0;
        if self.retired_hold.take().is_some() {
            reclaimed = true;
        }
        if self.inner.take_retired_route().is_some() {
            reclaimed = true;
        }
        self.publish_status();
        reclaimed
    }

    /// Move reclaimable banks out for worker-thread drop.
    ///
    /// Same-thread control path. Takes the holding slot plus any DSP-retired
    /// route without dropping, and drains the retired mailbox with a bounded
    /// attempt into the returned vector, so the caller can send the banks to
    /// a worker thread. Dropping the returned vector frees convolver state
    /// plus stashed base snapshots; do it off audio.
    pub fn take_reclaimable_retired(&mut self) -> Vec<RouteBanks> {
        let mut banks = Vec::new();
        if let Ok(mut consumer) = self.handle.retired_rx.try_lock() {
            for _ in 0..MAILBOX_POP_BOUND {
                match consumer.pop() {
                    Ok(retired) => banks.push(retired),
                    Err(_) => break,
                }
            }
        }
        if let Some(held) = self.retired_hold.take() {
            banks.push(held);
        }
        if let Some(retired) = self.inner.take_retired_route() {
            banks.push(retired);
        }
        self.publish_status();
        banks
    }

    /// Drop queued updates on control without touching live state.
    ///
    /// Same-thread control path. Drops `pending` and `queued` on the calling
    /// thread and clears the refusal record; mailbox contents stay queued for
    /// audio (drain them by processing or submit nothing further). Returns the
    /// number cancelled. Live config, history and audio stay exactly as
    /// accepted.
    pub fn cancel_pending(&mut self) -> usize {
        let mut cancelled = 0;
        if self.pending.take().is_some() {
            cancelled += 1;
        }
        if self.queued.take().is_some() {
            cancelled += 1;
        }
        self.last_refusal = None;
        self.handle.publish_refusal(None);
        self.publish_status();
        cancelled
    }

    /// Report the last audio commit refusal, if any.
    ///
    /// Set on every attempted commit during `process`; cleared on success and
    /// on `cancel_pending`. Copy the `Copy` enum to control and format there;
    /// never format on audio.
    pub fn last_refusal(&self) -> Option<CommitRefusal> {
        self.last_refusal
    }

    /// Count retained audio-slot updates (`0..=2`, excluding the mailbox).
    pub fn pending_len(&self) -> usize {
        usize::from(self.pending.is_some()) + usize::from(self.queued.is_some())
    }

    /// Report whether a committed blend is still morphing audio.
    pub fn update_in_progress(&self) -> bool {
        self.inner.update_in_progress()
    }

    /// Report whether retired banks await off-audio reclamation.
    pub fn retired_held(&self) -> bool {
        self.retired_hold.is_some()
    }

    /// Read-only access to the live DSP for accepted getters.
    ///
    /// Use for latency, response and parameter inspection only. Do not call
    /// commit APIs on the returned reference; audio commits run inside
    /// `process` so tests exercise the real host path.
    pub fn inner(&self) -> &LinearPhaseEqPlugin {
        &self.inner
    }

    /// Run one realtime-safe commit step before audio.
    ///
    /// Honors one coalesced head-cancel request first (evicting at most one
    /// payload into the cancelled outbox, deferred without dropping when the
    /// outbox is full), then pops the prepared mailbox into the retained
    /// slots with a fixed bound, moves retired banks toward the retired
    /// mailbox (holding on full, no drop), promotes a queued edit when
    /// `pending` is empty, then attempts at most one
    /// `try_commit_prepared_update`. Success clears the refusal record and
    /// publishes the accepted band plus generation; refusal retains the slot
    /// and records the `Copy` reason; a head eviction acknowledges the
    /// recorded refusal. The detached queue/blend/retired mirrors publish
    /// every quantum. No allocation, free, lock, blocking call, formatting
    /// or logging.
    fn commit_step(&mut self) {
        let observed = self.handle.cancel_seq.load(Ordering::Acquire);
        if observed != self.cancel_ack {
            // Evict the head slot only (pending preferred), restoring to the
            // same known-empty slot when the outbox is full. Never drops.
            if let Some(payload) = self.pending.take() {
                match self.cancelled_tx.push(payload) {
                    Ok(()) => {
                        self.cancel_ack = observed;
                        self.last_refusal = None;
                        self.handle.publish_refusal(None);
                    }
                    Err(RtPushError::Full(returned)) => {
                        self.pending = Some(returned);
                    }
                }
            } else if let Some(payload) = self.queued.take() {
                match self.cancelled_tx.push(payload) {
                    Ok(()) => {
                        self.cancel_ack = observed;
                        self.last_refusal = None;
                        self.handle.publish_refusal(None);
                    }
                    Err(RtPushError::Full(returned)) => {
                        self.queued = Some(returned);
                    }
                }
            } else {
                self.cancel_ack = observed;
            }
        }
        for _ in 0..MAILBOX_POP_BOUND {
            if self.pending.is_none() {
                let Ok(prepared) = self.prepared_rx.pop() else {
                    break;
                };
                self.pending = Some(prepared);
            } else if self.queued.is_none() {
                let Ok(prepared) = self.prepared_rx.pop() else {
                    break;
                };
                self.queued = Some(prepared);
            } else {
                break;
            }
        }
        if let Some(held) = self.retired_hold.take() {
            match self.retired_tx.push(held) {
                Ok(()) => {}
                Err(RtPushError::Full(returned)) => {
                    self.retired_hold = Some(returned);
                }
            }
        }
        if self.retired_hold.is_none()
            && let Some(retired) = self.inner.take_retired_route()
        {
            match self.retired_tx.push(retired) {
                Ok(()) => {}
                Err(RtPushError::Full(returned)) => {
                    self.retired_hold = Some(returned);
                }
            }
        }
        if self.pending.is_none() {
            self.pending = self.queued.take();
        }
        if self.pending.is_some() {
            // Copy the commit projection before the call consumes the slot on
            // success (`BandSnapshot` is `Copy`; no allocation).
            let projection = self
                .pending
                .as_ref()
                .map(|candidate| (candidate.band_index, candidate.new_band));
            match self.inner.try_commit_prepared_update(&mut self.pending) {
                Ok(()) => {
                    self.last_refusal = None;
                    self.handle.publish_refusal(None);
                    if let Some((band_index, shape)) = projection {
                        self.handle.publish_accepted_band(band_index, &shape);
                    }
                }
                Err(refusal) => {
                    self.last_refusal = Some(refusal);
                    self.handle.publish_refusal(Some(refusal));
                }
            }
        }
        self.publish_status();
    }

    /// Mirror queue, blend and holding-slot state to the handle.
    ///
    /// Lock-free atomic stores only; safe on audio and in exclusive
    /// same-thread control. Accepted generation and snapshot publish
    /// separately, only when a commit actually succeeds.
    fn publish_status(&self) {
        self.handle.publish_queue_status(
            self.pending_len(),
            self.inner.update_in_progress(),
            self.retired_hold.is_some(),
        );
    }

    fn ensure_scratch_f64(&mut self, len: usize) {
        if self.scratch_f64.len() < len {
            self.scratch_f64.resize(len, 0.0);
        }
    }
}

impl Plugin for LinearPhaseEqDynamicPlugin {
    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn info(&self) -> PluginInfo {
        self.inner.info()
    }

    fn input_channels(&self) -> usize {
        self.inner.channels()
    }

    fn output_channels(&self) -> usize {
        self.inner.channels()
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.inner.parametric_parameters()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.inner.parametric_set_parameter(id, value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.inner.parametric_get_parameter(id)
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        let before = self.handle.published_sample_rate();
        self.inner.initialize(sample_rate)?;
        // A same-rate initialize may still reset stream state (completing a
        // blend); mirror the resulting queue state without touching accepted
        // generations. A real rate change republishes the accepted rate and
        // bumps the generation; band shapes are unchanged by the rebuild.
        self.publish_status();
        if sample_rate != before {
            self.handle.publish_sample_rate(sample_rate);
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.inner.reset();
        // Reset completes an in-flight blend instantly and preserves the
        // accepted configuration, so only the queue/blend mirror refreshes;
        // the accepted generation, snapshot and refusal record are kept.
        self.publish_status();
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let channels = self.inner.channels();
        validate_process_block_f32(input, output, context, channels, channels)?;
        self.commit_step();
        output.copy_from_slice(input);
        self.inner.process_in_place(output, context)
    }

    // Note: the f32 `process` and `drain` paths are allocation-free, but
    // this f64 bridge may grow a reusable scratch buffer on first large use.
    // The host never dispatches it for this plugin (`supports_f64` is false,
    // so graphs use the host f32 bridge); direct f64 callers accept amortized
    // growth instead of the inner default, which allocates every call.
    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let channels = self.inner.channels();
        validate_process_block_f64(input, output, context, channels, channels)?;
        self.commit_step();
        self.ensure_scratch_f64(input.len());
        let scratch = &mut self.scratch_f64[..input.len()];
        for (dst, &src) in scratch.iter_mut().zip(input.iter()) {
            *dst = src as f32;
        }
        let frames = self.inner.process_in_place(scratch, context)?;
        for (dst, &src) in output.iter_mut().zip(scratch.iter()) {
            *dst = src as f64;
        }
        Ok(frames)
    }

    fn latency_samples(&self) -> usize {
        self.inner.latency_samples()
    }

    fn tail_length(&self) -> TailLength {
        self.inner.tail_length()
    }

    fn signal_delay_samples(&self) -> f64 {
        self.inner.latency_samples() as f64
    }

    fn drain_output_frames_max(&self) -> usize {
        self.inner.drain_output_frames_max()
    }

    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        self.inner.prepare_drain_metadata()
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        self.inner.begin_drain(context)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.inner.drain_call_bound()
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        self.inner.drain(output, context)
    }

    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        // Decline compiled dispatch so the host always uses `process`, where
        // the dynamic commit step runs exactly once per quantum. Forwarding
        // to the inner op could render audio without committing (if a future
        // inner op returns `Some`), while committing here as well would run
        // the step twice when the host falls back to `process` on `None`.
        let _ = (op, input, output, context);
        None
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        self.inner.compile_metadata()
    }

    fn realtime_quantum_frames(&self) -> usize {
        self.inner.realtime_quantum_frames()
    }

    fn cost_class(&self) -> PluginCostClass {
        self.inner.cost_class()
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(Arc::clone(&self.handle) as Arc<dyn Any + Send + Sync>)
    }

    fn supports_f64(&self) -> bool {
        self.inner.supports_f64()
    }
}
