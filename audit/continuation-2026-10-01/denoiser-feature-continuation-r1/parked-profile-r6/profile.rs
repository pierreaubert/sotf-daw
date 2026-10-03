//! Versioned captured noise-profile carrier.
//!
//! A [`NoiseProfileData`] persists one learned noise floor: per-channel
//! per-bin mean powers in live WOLA units (raw unnormalized `|X|^2` means
//! under the sqrt-Hann analysis window, averaged across full capture
//! hops), channel-major (`channels * num_bins` entries, no doubling).
//! Bin `b` sits at `b * sample_rate / fft_size` Hz; the table covers DC
//! through Nyquist inclusive.
//!
//! Stored profiles restore only at the capture FFT/hop/window geometry,
//! channel count, and sample rate; anything else is rejected (explicit
//! import) or dropped (construction/initialize), never resampled.
//! Exporting allocates and is reserved for save/preset threads; the audio
//! path only reads the pre-sized adopted storage.

use serde::{Deserialize, Serialize};

/// Persisted [`NoiseProfileData`] schema version.
pub const DENOISER_PROFILE_FORMAT_VERSION: u32 = 1;
/// Window identifier the carrier is measured with.
pub const DENOISER_PROFILE_WINDOW: &str = "sqrt-hann";
/// FFT lengths the carrier accepts (the owned latency modes).
pub const DENOISER_PROFILE_FFT_SIZES: [usize; 2] = [512, 2048];
/// Largest accepted channel count (bounds the payload).
pub const DENOISER_PROFILE_MAX_CHANNELS: usize = 64;
/// Largest accepted one-sided bin count (bounds the payload).
pub const DENOISER_PROFILE_MAX_BINS: usize = 4097;

/// Persisted denoiser noise profile: measured per-bin powers.
///
/// Powers are live-unit means, not dB and not time-domain variance.
/// Compare Hiss `NoiseProfileData` only for the versioning/validation
/// contract shape; the units and geometry are denoiser-specific.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseProfileData {
    /// Schema version (must be [`DENOISER_PROFILE_FORMAT_VERSION`]).
    pub format_version: u32,
    /// FFT length the profile was measured with (512 or 2048).
    pub fft_size: usize,
    /// Hop advance the profile was measured with (half the FFT).
    pub hop_size: usize,
    /// Window identifier (must be `"sqrt-hann"`).
    pub window: String,
    /// Sample rate the profile was measured at.
    pub sample_rate: u32,
    /// Channel count the profile was measured with.
    pub channels: usize,
    /// One-sided bin count (`fft_size / 2 + 1`).
    pub num_bins: usize,
    /// Channel-major mean powers (`channels * num_bins` entries).
    pub power_per_channel_bin: Vec<f32>,
    /// Full hops averaged into the profile (always positive).
    pub hops_analyzed: u64,
}

impl NoiseProfileData {
    /// Validates internal consistency of imported profile data.
    ///
    /// Agreement with a live plugin (geometry/rate/channels) is checked
    /// separately by [`NoiseProfileData::validate_against`]: construction
    /// drops mismatched profiles while explicit import rejects them.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first corruption found (unsupported
    /// version/geometry, zero rate/channels/hops, inconsistent bin count
    /// or payload length, oversized payload, or non-finite/negative
    /// powers).
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != DENOISER_PROFILE_FORMAT_VERSION {
            return Err(format!(
                "unsupported denoiser profile format version {}",
                self.format_version
            ));
        }
        if !DENOISER_PROFILE_FFT_SIZES.contains(&self.fft_size) {
            return Err(format!(
                "denoiser profile FFT size {} is unsupported, expected 512 or 2048",
                self.fft_size
            ));
        }
        if self.hop_size != self.fft_size / 2 {
            return Err(format!(
                "denoiser profile hop size {} disagrees with FFT size {}",
                self.hop_size, self.fft_size
            ));
        }
        if self.window != DENOISER_PROFILE_WINDOW {
            return Err(format!(
                "denoiser profile window {:?} is unsupported, expected {DENOISER_PROFILE_WINDOW:?}",
                self.window
            ));
        }
        if self.sample_rate == 0 {
            return Err("denoiser profile sample rate must be nonzero".to_string());
        }
        if self.channels == 0 || self.channels > DENOISER_PROFILE_MAX_CHANNELS {
            return Err(format!(
                "denoiser profile channels {} outside 1..={}",
                self.channels, DENOISER_PROFILE_MAX_CHANNELS
            ));
        }
        if self.num_bins != self.fft_size / 2 + 1 {
            return Err(format!(
                "denoiser profile has {} bins for FFT size {}",
                self.num_bins, self.fft_size
            ));
        }
        if self.num_bins > DENOISER_PROFILE_MAX_BINS {
            return Err(format!(
                "denoiser profile bin count {} exceeds {DENOISER_PROFILE_MAX_BINS}",
                self.num_bins
            ));
        }
        if self.hops_analyzed == 0 {
            return Err("denoiser profile analyzed zero hops".to_string());
        }
        // Structural payload bound: at most 64 channels x 1025 bins
        // (256 KiB of f32), enforced by the caps above plus this exact
        // length agreement (no truncation, no padding).
        let expected = self.channels * self.num_bins;
        if self.power_per_channel_bin.len() != expected {
            return Err(format!(
                "denoiser profile has {} powers for {} channels x {} bins",
                self.power_per_channel_bin.len(),
                self.channels,
                self.num_bins
            ));
        }
        for (index, power) in self.power_per_channel_bin.iter().enumerate() {
            if !power.is_finite() || *power < 0.0 {
                return Err(format!(
                    "denoiser profile power {index} is not finite/nonnegative: {power}"
                ));
            }
        }
        Ok(())
    }

    /// Validates agreement with a live plugin configuration.
    ///
    /// `sample_rate` is `None` before initialization (construction defers
    /// the rate check to `initialize`, which drops mismatches) and
    /// `Some(live)` for explicit post-initialize import, which rejects
    /// stale geometry instead of adopting it.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first incompatibility found (FFT,
    /// hop, bin count, channel count, or sample rate disagreement).
    pub fn validate_against(
        &self,
        fft_size: usize,
        hop_size: usize,
        sample_rate: Option<u32>,
        channels: usize,
    ) -> Result<(), String> {
        if self.fft_size != fft_size {
            return Err(format!(
                "denoiser profile FFT size {} disagrees with live {fft_size}",
                self.fft_size
            ));
        }
        if self.hop_size != hop_size {
            return Err(format!(
                "denoiser profile hop size {} disagrees with live {hop_size}",
                self.hop_size
            ));
        }
        if self.num_bins != fft_size / 2 + 1 {
            return Err(format!(
                "denoiser profile has {} bins, live geometry has {}",
                self.num_bins,
                fft_size / 2 + 1
            ));
        }
        if self.channels != channels {
            return Err(format!(
                "denoiser profile has {} channels, live has {channels}",
                self.channels
            ));
        }
        if let Some(rate) = sample_rate {
            if self.sample_rate != rate {
                return Err(format!(
                    "denoiser profile rate {} disagrees with live {rate}",
                    self.sample_rate
                ));
            }
        }
        Ok(())
    }
}
