//! Noise-profile capture and per-frequency reduction curve state.
//!
//! This module is allocation-free on the audio path: [`CaptureState`] owns
//! pre-sized buffers, and profile completion writes into caller-provided
//! storage. Exporting a [`NoiseProfileData`] allocates and is reserved for
//! save/preset threads.
//!
//! Mode applicability: capture measurement and profile persistence work in
//! both DSP modes. A stored profile modulates the time-domain threshold
//! directly and the spectral per-bin noise estimate through the shared
//! `plugins-denoiser` backend hooks; the reduction curve scales spectral
//! per-bin maximum reduction, and channel linking shares detectors in both
//! modes. The curve stays stored-only in time-domain mode (the single-band
//! time-domain detector has no per-frequency hook, by design).

use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Persisted [`NoiseProfileData`] schema version.
pub const PROFILE_FORMAT_VERSION: u32 = 1;
/// Noise capture duration in seconds.
pub const CAPTURE_SECONDS: f64 = 1.0;
/// Minimum representable profile floor in dBFS (silent capture).
pub const PROFILE_FLOOR_MIN_DB: f32 = -120.0;
/// Maximum accepted profile floor in dBFS (guards corrupt imports).
pub const PROFILE_FLOOR_MAX_DB: f32 = 6.0;
/// Headroom added to the measured floor for time-domain threshold following.
///
/// The detector only reduces while the slow high-band envelope sits below
/// the threshold with hysteresis, so the effective threshold must clear the
/// measured floor by more than envelope ripple plus the +3 dB exit band.
pub const PROFILE_THRESHOLD_MARGIN_DB: f32 = 6.0;
/// Fixed log-spaced reduction-curve anchors in Hz (low, mid, high).
pub const CURVE_ANCHOR_HZ: [f32; 3] = [1_000.0, 4_000.0, 12_000.0];
/// Choice labels for the `link_mode` parameter.
pub const LINK_LABELS: &[&str] = &["Independent", "Linked"];
/// `link_mode` value preserving per-channel independent processing.
pub const LINK_INDEPENDENT: i32 = 0;
/// `link_mode` value requesting linked-channel reduction.
pub const LINK_LINKED: i32 = 1;

/// Persisted hiss noise profile: per-channel high-band floors in dBFS.
///
/// Floors are broadband level references measured with the exact-mapped
/// one-pole high-band split also used by the time-domain reducer, so they
/// are meaningful at any sample rate. They are not a captured colored
/// spectrum: the spectral backend spreads each floor white across its
/// live high band (a documented coarse approximation), and per-band
/// measured capture stays an open requirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseProfileData {
    /// Must equal [`PROFILE_FORMAT_VERSION`].
    pub format_version: u32,
    /// Sample rate the capture was measured at (informational).
    pub sample_rate: u32,
    /// Channel count the floors were measured with.
    pub channels: usize,
    /// High-band cutoff in Hz used for the measurement.
    pub measurement_cutoff_hz: f32,
    /// Per-channel high-band RMS floor in dBFS.
    pub floor_db_per_channel: Vec<f32>,
    /// Analyzed frames backing the floors (always positive).
    pub frames_analyzed: u64,
}

impl NoiseProfileData {
    /// Validates internal consistency of imported profile data.
    ///
    /// Channel-count agreement with a live plugin is checked separately by
    /// the caller: construction drops mismatched profiles while explicit
    /// restore rejects them.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first corruption found (unsupported
    /// version, zero rate/channels/frames, inconsistent floor length,
    /// non-finite or out-of-range values).
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != PROFILE_FORMAT_VERSION {
            return Err(format!(
                "unsupported noise profile format version {}",
                self.format_version
            ));
        }
        if self.sample_rate == 0 {
            return Err("noise profile sample rate must be nonzero".to_string());
        }
        if self.channels == 0 {
            return Err("noise profile channels must be nonzero".to_string());
        }
        if self.floor_db_per_channel.len() != self.channels {
            return Err(format!(
                "noise profile has {} floors for {} channels",
                self.floor_db_per_channel.len(),
                self.channels
            ));
        }
        if self.frames_analyzed == 0 {
            return Err("noise profile analyzed zero frames".to_string());
        }
        if !self.measurement_cutoff_hz.is_finite() || self.measurement_cutoff_hz <= 0.0 {
            return Err("noise profile cutoff must be finite and positive".to_string());
        }
        for (index, floor) in self.floor_db_per_channel.iter().enumerate() {
            if !floor.is_finite()
                || *floor < PROFILE_FLOOR_MIN_DB
                || *floor > PROFILE_FLOOR_MAX_DB
            {
                return Err(format!(
                    "noise profile floor {index} is out of range: {floor}"
                ));
            }
        }
        Ok(())
    }

    /// Returns the loudest per-channel floor, the profile summary.
    pub fn overall_floor_db(&self) -> f32 {
        self.floor_db_per_channel
            .iter()
            .copied()
            .fold(PROFILE_FLOOR_MIN_DB, f32::max)
    }
}

/// Per-frequency reduction curve over [`CURVE_ANCHOR_HZ`].
///
/// Values scale the spectral per-bin maximum reduction (1.0 keeps the full
/// configured strength, 0.0 disables reduction at that frequency). The
/// flat all-1.0 default preserves legacy behavior exactly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReductionCurve {
    /// Reduction scale at or below 1 kHz.
    pub low: f32,
    /// Reduction scale at 4 kHz.
    pub mid: f32,
    /// Reduction scale at or above 12 kHz.
    pub high: f32,
}

impl Default for ReductionCurve {
    fn default() -> Self {
        Self {
            low: 1.0,
            mid: 1.0,
            high: 1.0,
        }
    }
}

impl ReductionCurve {
    /// Clamps one curve control point, repairing non-finite input to 1.0.
    pub fn canonicalize(value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Returns true when the curve keeps full reduction everywhere.
    pub fn is_flat(&self) -> bool {
        self.low == 1.0 && self.mid == 1.0 && self.high == 1.0
    }

    /// Interpolates the reduction scale at `freq_hz`.
    ///
    /// Interpolation is linear in log frequency between the fixed anchors
    /// and clamps outside them, so anchor values reproduce exactly.
    pub fn gain_at(&self, freq_hz: f32) -> f32 {
        if !freq_hz.is_finite() || freq_hz <= CURVE_ANCHOR_HZ[0] {
            return self.low;
        }
        if freq_hz >= CURVE_ANCHOR_HZ[2] {
            return self.high;
        }
        let (lo_f, hi_f, lo_g, hi_g) = if freq_hz < CURVE_ANCHOR_HZ[1] {
            (
                CURVE_ANCHOR_HZ[0],
                CURVE_ANCHOR_HZ[1],
                self.low,
                self.mid,
            )
        } else {
            (
                CURVE_ANCHOR_HZ[1],
                CURVE_ANCHOR_HZ[2],
                self.mid,
                self.high,
            )
        };
        let position =
            (f64::from(freq_hz) / f64::from(lo_f)).ln() / (f64::from(hi_f) / f64::from(lo_f)).ln();
        (f64::from(lo_g) + position * (f64::from(hi_g) - f64::from(lo_g))) as f32
    }
}

/// Summary of a completed capture written into profile storage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompletedCapture {
    /// Sample rate the capture was measured at.
    pub sample_rate: u32,
    /// Channel count the capture was measured with.
    pub channels: usize,
    /// High-band cutoff in Hz used for the measurement.
    pub measurement_cutoff_hz: f32,
    /// Analyzed frames backing the floors.
    pub frames_analyzed: u64,
}

/// Pre-allocated high-band RMS capture engine.
///
/// Measurement replicates the backend exact-mapped one-pole split
/// (`alpha = 1 - exp(-2 pi fc / sr)`) so captured floors describe the same
/// high band the time-domain reducer acts on. All buffers are sized at
/// construction; [`CaptureState::start`], [`CaptureState::accumulate`],
/// [`CaptureState::take_completed`], and [`CaptureState::cancel`] never
/// allocate.
#[derive(Debug, Clone)]
pub struct CaptureState {
    active: bool,
    channels: usize,
    sample_rate: u32,
    cutoff_hz: f32,
    alpha: f32,
    target_frames: u64,
    frames: u64,
    lowpass: Vec<f32>,
    sum_power: Vec<f64>,
}

impl CaptureState {
    /// Creates an idle capture engine with buffers for `channels`.
    pub fn new(channels: usize) -> Self {
        Self {
            active: false,
            channels,
            sample_rate: 48_000,
            cutoff_hz: 4_000.0,
            alpha: 0.0,
            target_frames: 0,
            frames: 0,
            lowpass: vec![0.0; channels],
            sum_power: vec![0.0; channels],
        }
    }

    /// Arms a one-second capture at `sample_rate` and `cutoff_hz`.
    ///
    /// Restarting while active discards the partial measurement. The cutoff
    /// is clamped to the same valid band the backend accepts.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero channel count or sample rate.
    pub fn start(&mut self, sample_rate: u32, cutoff_hz: f32) -> Result<(), String> {
        if self.channels == 0 {
            return Err("noise capture requires at least one channel".to_string());
        }
        if sample_rate == 0 {
            return Err("noise capture requires a nonzero sample rate".to_string());
        }
        let rate = sample_rate as f32;
        let cutoff = if cutoff_hz.is_finite() {
            cutoff_hz.max(20.0)
        } else {
            4_000.0
        }
        .min(rate * 0.45)
        .max(20.0);
        self.sample_rate = sample_rate;
        self.cutoff_hz = cutoff;
        self.alpha = 1.0 - (-2.0 * PI * cutoff / rate).exp();
        self.target_frames = ((CAPTURE_SECONDS * f64::from(sample_rate)).round() as u64).max(1);
        self.frames = 0;
        self.lowpass.fill(0.0);
        self.sum_power.fill(0.0);
        self.active = true;
        Ok(())
    }

    /// Discards any partial measurement and disarms capture.
    pub fn cancel(&mut self) {
        self.active = false;
        self.frames = 0;
        self.lowpass.fill(0.0);
        self.sum_power.fill(0.0);
    }

    /// Returns true while a capture is accumulating.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns true once the capture target has been reached.
    pub fn is_complete(&self) -> bool {
        self.active && self.target_frames > 0 && self.frames >= self.target_frames
    }

    /// Returns capture progress in 0.0..=1.0 (0.0 while idle).
    pub fn progress(&self) -> f32 {
        if !self.active || self.target_frames == 0 {
            return 0.0;
        }
        (self.frames as f32 / self.target_frames as f32).clamp(0.0, 1.0)
    }

    /// Returns the frames accumulated toward the current target.
    pub fn frames_accumulated(&self) -> u64 {
        self.frames
    }

    /// Returns the armed capture target in frames (0 while never armed).
    pub fn target_frames(&self) -> u64 {
        self.target_frames
    }

    /// Accumulates interleaved input frames into the measurement.
    ///
    /// Non-finite samples are measured as silence. Trailing samples that do
    /// not form a complete frame are ignored. Does nothing while idle.
    pub fn accumulate(&mut self, input: &[f32]) {
        if !self.active || self.channels == 0 {
            return;
        }
        let frames = input.len() / self.channels;
        for frame in 0..frames {
            let base = frame * self.channels;
            for ch in 0..self.channels {
                let sample = input[base + ch];
                let dry = if sample.is_finite() { sample } else { 0.0 };
                let low = self.alpha * dry + (1.0 - self.alpha) * self.lowpass[ch];
                self.lowpass[ch] = if low.abs() < 1e-20 { 0.0 } else { low };
                let high = dry - self.lowpass[ch];
                self.sum_power[ch] += f64::from(high) * f64::from(high);
            }
        }
        self.frames += frames as u64;
    }

    /// Writes completed floors into `floors_db` and disarms capture.
    ///
    /// Returns `None` (leaving everything untouched) unless the capture
    /// target is reached and `floors_db` holds exactly one entry per
    /// channel. Silent captures clamp to [`PROFILE_FLOOR_MIN_DB`].
    pub fn take_completed(&mut self, floors_db: &mut [f32]) -> Option<CompletedCapture> {
        if !self.is_complete() || floors_db.len() != self.channels {
            return None;
        }
        let frames = self.frames.max(1) as f64;
        for (channel, floor) in floors_db.iter_mut().enumerate() {
            let mean_power = self.sum_power[channel] / frames;
            let floor_db = (10.0 * mean_power.log10()) as f32;
            *floor = floor_db.clamp(PROFILE_FLOOR_MIN_DB, PROFILE_FLOOR_MAX_DB);
        }
        let summary = CompletedCapture {
            sample_rate: self.sample_rate,
            channels: self.channels,
            measurement_cutoff_hz: self.cutoff_hz,
            frames_analyzed: self.frames,
        };
        self.cancel();
        Some(summary)
    }
}
