//! Shared preallocated profile snapshot for hosted export.
//!
//! This module publishes the stored noise profile (floors plus measured
//! spectrum) and live capture state to out-of-band readers (engine
//! settings, presets, UI) through [`ParametricInPlacePlugin::get_data`].
//! The plugin owns one [`ProfileSnapshot`] behind an [`Arc`] created at
//! construction and never replaced, so `get_data` performs an [`Arc`]
//! clone only: no allocation, deallocation, lock, or wait on any
//! realtime path.
//!
//! Consistency uses a bounded generation protocol over atomic element
//! storage in safe Rust (no `unsafe`, no shared non-atomic buffers):
//!
//! - Every `f32` payload word is an [`AtomicU32`] bit pattern; every
//!   counter is an [`AtomicU64`]/[`AtomicU32`]. All operations use
//!   [`Ordering::SeqCst`]: publication is rare (capture completion,
//!   restore, clear, use-flag and rate changes), so the simplest
//!   ordering is also cheap enough.
//! - The single producer (all `&mut self` plugin methods, externally
//!   synchronized by the host) brackets each publication with an odd/even
//!   [`AtomicU64`] generation pair. Readers retry a bounded number of
//!   times on an odd or changed generation and then report [`SnapshotBusy`];
//!   neither side ever spins unboundedly or blocks.
//! - Stored-profile payload and its metadata (presence, spectrum flag, use
//!   flag, capture/processing rates, cutoff, frames, hops) publish under
//!   one generation, so readers never observe mixed metadata/payload.
//!   Capture progress/activity are single-word live atomics outside the
//!   protocol (point-in-time by design) so per-callback progress updates
//!   never churn the generation.
//!
//! [`ParametricInPlacePlugin::get_data`]: sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin::get_data
//! [`Arc`]: std::sync::Arc
//! [`AtomicU32`]: std::sync::atomic::AtomicU32
//! [`AtomicU64`]: std::sync::atomic::AtomicU64
//! [`Ordering::SeqCst`]: std::sync::atomic::Ordering::SeqCst

// Rust guideline compliant 2026-10-21
use crate::profile::{
    NoiseProfileData, PROFILE_FORMAT_VERSION, PROFILE_FORMAT_VERSION_V1, SpectralProfileData,
};
use plugins_denoiser::spectral_profile::{
    SPECTRAL_PROFILE_FFT_SIZE, SPECTRAL_PROFILE_HOP_SIZE, SPECTRAL_PROFILE_NUM_BINS,
    SPECTRAL_PROFILE_WINDOW,
};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Bounded reader retries per export attempt.
pub const SNAPSHOT_READER_RETRIES: u32 = 4;

/// Scalar publication bundle (payload slices pass separately).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfilePublishMeta {
    /// A stored profile is present.
    pub present: bool,
    /// The stored profile carries a measured spectrum.
    pub spectral: bool,
    /// Sample rate the stored profile was captured at.
    pub capture_rate: u32,
    /// High-band cutoff the stored profile was measured at.
    pub cutoff_hz: f32,
    /// Frames backing the stored floors.
    pub frames: u64,
    /// Full spectral windows backing the stored spectrum.
    pub hops: u64,
    /// The live use-captured-profile flag.
    pub use_flag: bool,
    /// Live processing rate.
    pub processing_rate: u32,
}
/// Flags bit: a stored profile is present.
const FLAG_HAS_PROFILE: u64 = 1;
/// Flags bit: the stored profile carries a measured spectrum.
const FLAG_HAS_SPECTRAL: u64 = 2;

/// Reader contention signal: a publication overlapped every retry.
///
/// Returned when the generation stayed odd or kept changing across
/// [`SNAPSHOT_READER_RETRIES`] attempts. Distinct from absence of a
/// profile ([`ProfileSnapshot::try_export`] returns `Ok(None)` then).
/// Callers on control/UI threads should retry the export later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotBusy;

impl std::fmt::Display for SnapshotBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("profile snapshot busy: publication overlapped every read retry")
    }
}

impl std::error::Error for SnapshotBusy {}

/// Three-way profile applicability at the processing rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFallback {
    /// No profile stored, or the use flag is off: listeners disengaged.
    Disabled,
    /// Profile stored but per-bin comparison unavailable (v1 floors-only
    /// payload, or capture rate differs from the processing rate): the
    /// backend spreads floors white across the live high band.
    Floor,
    /// Stored spectrum compared per bin (use flag on, v2 payload, capture
    /// rate equals the processing rate).
    Measured,
}

/// Generation-consistent snapshot metadata without payload copy.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileStatus {
    /// Even generation counter of this consistent read.
    pub generation: u64,
    /// A stored profile is present.
    pub present: bool,
    /// The stored profile carries a measured spectrum.
    pub spectral: bool,
    /// The live use-captured-profile flag.
    pub use_flag: bool,
    /// Sample rate the stored profile was captured at (0 when absent).
    pub capture_rate: u32,
    /// Live processing rate at publication time.
    pub processing_rate: u32,
    /// High-band cutoff the stored profile was measured at.
    pub cutoff_hz: f32,
    /// Frames backing the stored floors.
    pub frames: u64,
    /// Full spectral windows backing the stored spectrum.
    pub hops: u64,
    /// True when per-bin comparison is actually engaged.
    pub engaged: bool,
    /// Three-way applicability at the processing rate.
    pub fallback: ProfileFallback,
}

/// Generation-consistent owned profile export for control threads.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileExport {
    /// Even generation counter of this consistent read.
    pub generation: u64,
    /// Exact stored profile (v1 floors-only or v2 with spectrum).
    pub profile: NoiseProfileData,
    /// The live use-captured-profile flag at publication time.
    pub use_flag: bool,
    /// Live processing rate at publication time.
    pub processing_rate: u32,
    /// True when per-bin comparison is actually engaged.
    pub engaged: bool,
}

/// Preallocated shared profile snapshot published via `get_data`.
///
/// Created once per plugin at construction with channel/bin capacity fixed;
/// the owning plugin never replaces the [`Arc`](std::sync::Arc), so
/// retained readers keep the payload alive and normal processing can never
/// destroy it. All methods take `&self`: writers run only from
/// host-synchronized `&mut self` plugin methods (single producer), while
/// readers run concurrently from any thread; every shared word is an
/// atomic under [`Ordering::SeqCst`], so no lock or `unsafe` is needed.
/// Readers never mutate DSP state.
#[derive(Debug)]
pub struct ProfileSnapshot {
    channels: usize,
    generation: AtomicU64,
    flags: AtomicU64,
    use_flag: AtomicU32,
    capture_rate: AtomicU32,
    processing_rate: AtomicU32,
    cutoff_bits: AtomicU32,
    frames: AtomicU64,
    hops: AtomicU64,
    floors: Box<[AtomicU32]>,
    spectrum: Box<[AtomicU32]>,
    capture_active: AtomicU32,
    capture_progress_bits: AtomicU32,
}

impl ProfileSnapshot {
    /// Creates an empty snapshot with capacity for `channels`.
    ///
    /// Allocates the atomic payload tables once; all later writer and
    /// reader calls reuse this storage.
    pub fn new(channels: usize) -> Self {
        let floors = (0..channels)
            .map(|_| AtomicU32::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let spectrum = (0..channels * SPECTRAL_PROFILE_NUM_BINS)
            .map(|_| AtomicU32::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            channels,
            generation: AtomicU64::new(0),
            flags: AtomicU64::new(0),
            use_flag: AtomicU32::new(0),
            capture_rate: AtomicU32::new(0),
            processing_rate: AtomicU32::new(0),
            cutoff_bits: AtomicU32::new(0),
            frames: AtomicU64::new(0),
            hops: AtomicU64::new(0),
            floors,
            spectrum,
            capture_active: AtomicU32::new(0),
            capture_progress_bits: AtomicU32::new(0),
        }
    }

    /// Returns the channel capacity fixed at construction.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Publishes the stored profile and live flags atomically.
    ///
    /// Copies at most the prepared channel/bin width; called only on
    /// capture completion, restore, clear, and construction. Never
    /// allocates, frees, locks, logs, waits, or spins.
    pub fn publish_profile(&self, meta: &ProfilePublishMeta, floors: &[f32], spectrum: &[f32]) {
        debug_assert_eq!(floors.len(), self.channels);
        debug_assert_eq!(spectrum.len(), self.channels * SPECTRAL_PROFILE_NUM_BINS);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut flags = 0u64;
        if meta.present {
            flags |= FLAG_HAS_PROFILE;
        }
        if meta.spectral {
            flags |= FLAG_HAS_SPECTRAL;
        }
        self.flags.store(flags, Ordering::SeqCst);
        self.use_flag
            .store(u32::from(meta.use_flag), Ordering::SeqCst);
        self.capture_rate.store(meta.capture_rate, Ordering::SeqCst);
        self.processing_rate
            .store(meta.processing_rate, Ordering::SeqCst);
        self.cutoff_bits
            .store(meta.cutoff_hz.to_bits(), Ordering::SeqCst);
        self.frames.store(meta.frames, Ordering::SeqCst);
        self.hops.store(meta.hops, Ordering::SeqCst);
        for (slot, sample) in self.floors.iter().zip(floors.iter()) {
            slot.store(sample.to_bits(), Ordering::SeqCst);
        }
        for (slot, sample) in self.spectrum.iter().zip(spectrum.iter()) {
            slot.store(sample.to_bits(), Ordering::SeqCst);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Publishes live flags without touching the stored payload.
    ///
    /// Bumps the generation so engagement inputs stay consistent with the
    /// payload; called only on use-flag and processing-rate changes.
    /// Never allocates, frees, locks, logs, waits, or spins.
    pub fn publish_live_flags(&self, use_flag: bool, processing_rate: u32) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.use_flag.store(u32::from(use_flag), Ordering::SeqCst);
        self.processing_rate
            .store(processing_rate, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Publishes capture activity outside the generation protocol.
    ///
    /// Point-in-time by design; safe to call every callback while a
    /// capture runs. Never allocates, frees, locks, logs, or waits.
    pub fn set_capture_progress(&self, active: bool, progress: f32) {
        self.capture_active
            .store(u32::from(active), Ordering::SeqCst);
        self.capture_progress_bits
            .store(progress.to_bits(), Ordering::SeqCst);
    }

    /// Reads consistent metadata without copying any payload.
    ///
    /// Control/UI-thread reader; may be called while the audio thread
    /// publishes. Retries boundedly, then reports contention.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotBusy`] when a publication overlapped every retry.
    pub fn try_status(&self) -> Result<ProfileStatus, SnapshotBusy> {
        for _ in 0..SNAPSHOT_READER_RETRIES {
            let generation = self.generation.load(Ordering::SeqCst);
            if (generation & 1) != 0 {
                continue;
            }
            let status = self.read_status(generation);
            if self.generation.load(Ordering::SeqCst) == generation {
                return Ok(status);
            }
        }
        Err(SnapshotBusy)
    }

    /// Exports the exact stored profile with consistent metadata.
    ///
    /// Control/UI-thread reader; allocates the owned payload vectors.
    /// Returns `Ok(None)` when consistently no profile is stored, which
    /// is distinct from [`SnapshotBusy`]. The payload always carries the
    /// stored capture rate/cutoff, independent of the live use flag and
    /// processing rate; engagement is reported alongside.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotBusy`] when a publication overlapped every retry.
    pub fn try_export(&self) -> Result<Option<ProfileExport>, SnapshotBusy> {
        for _ in 0..SNAPSHOT_READER_RETRIES {
            let generation = self.generation.load(Ordering::SeqCst);
            if (generation & 1) != 0 {
                continue;
            }
            let status = self.read_status(generation);
            let profile = status.present.then(|| self.read_profile(&status));
            if self.generation.load(Ordering::SeqCst) == generation {
                return Ok(profile.map(|profile| ProfileExport {
                    generation,
                    use_flag: status.use_flag,
                    processing_rate: status.processing_rate,
                    engaged: status.engaged,
                    profile,
                }));
            }
        }
        Err(SnapshotBusy)
    }

    /// Reads point-in-time capture activity and progress.
    ///
    /// Never fails or retries: single-word atomics outside the generation
    /// protocol. Safe on any thread, including the audio callback.
    pub fn capture_state(&self) -> (bool, f32) {
        (
            self.capture_active.load(Ordering::SeqCst) != 0,
            f32::from_bits(self.capture_progress_bits.load(Ordering::SeqCst)),
        )
    }

    /// Reads one metadata pass; the caller validates the generation.
    fn read_status(&self, generation: u64) -> ProfileStatus {
        let flags = self.flags.load(Ordering::SeqCst);
        let present = (flags & FLAG_HAS_PROFILE) != 0;
        let spectral = present && (flags & FLAG_HAS_SPECTRAL) != 0;
        let use_flag = self.use_flag.load(Ordering::SeqCst) != 0;
        let capture_rate = self.capture_rate.load(Ordering::SeqCst);
        let processing_rate = self.processing_rate.load(Ordering::SeqCst);
        let engaged =
            use_flag && present && spectral && capture_rate == processing_rate && capture_rate != 0;
        let fallback = if !use_flag || !present {
            ProfileFallback::Disabled
        } else if engaged {
            ProfileFallback::Measured
        } else {
            ProfileFallback::Floor
        };
        ProfileStatus {
            generation,
            present,
            spectral,
            use_flag,
            capture_rate,
            processing_rate,
            cutoff_hz: f32::from_bits(self.cutoff_bits.load(Ordering::SeqCst)),
            frames: self.frames.load(Ordering::SeqCst),
            hops: self.hops.load(Ordering::SeqCst),
            engaged,
            fallback,
        }
    }

    /// Reads one owned payload; the caller validates the generation.
    ///
    /// The writer publishes only validated live-store contents, so the
    /// rebuilt blob is valid by construction without revalidation.
    fn read_profile(&self, status: &ProfileStatus) -> NoiseProfileData {
        let floors = self
            .floors
            .iter()
            .map(|slot| f32::from_bits(slot.load(Ordering::SeqCst)))
            .collect::<Vec<_>>();
        let spectral = status.spectral.then(|| {
            let power_per_channel_bin = self
                .spectrum
                .iter()
                .map(|slot| f32::from_bits(slot.load(Ordering::SeqCst)))
                .collect::<Vec<_>>();
            SpectralProfileData {
                fft_size: SPECTRAL_PROFILE_FFT_SIZE,
                hop_size: SPECTRAL_PROFILE_HOP_SIZE,
                window: SPECTRAL_PROFILE_WINDOW.to_string(),
                sample_rate: status.capture_rate,
                channels: self.channels,
                num_bins: SPECTRAL_PROFILE_NUM_BINS,
                power_per_channel_bin,
                hops_analyzed: status.hops,
            }
        });
        NoiseProfileData {
            format_version: if spectral.is_some() {
                PROFILE_FORMAT_VERSION
            } else {
                PROFILE_FORMAT_VERSION_V1
            },
            sample_rate: status.capture_rate,
            channels: self.channels,
            measurement_cutoff_hz: status.cutoff_hz,
            floor_db_per_channel: floors,
            frames_analyzed: status.frames,
            spectral,
        }
    }
}
