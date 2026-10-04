//! Measured per-bin noise-spectrum capture helper.
//!
//! This module owns the allocation-free spectral measurement that backs the
//! Hiss measured-profile feature. [`SpectralCapture`] mirrors the live
//! [`SpectralHissReducer`](crate::spectral_hiss::SpectralHissReducer) WOLA
//! analysis exactly (1024-point unnormalized real FFT, periodic Hann window,
//! 256-sample hop) and averages raw per-bin powers across full capture
//! windows. Use it alongside the broadband RMS capture for colored hiss;
//! do not infer a spectrum from a scalar floor.
//!
//! Units and conventions (live WOLA, preserved):
//!
//! - Forward transform is unnormalized (`realfft`, no `1/N` scaling):
//!   `X[k] = sum x[n] w[n] e^(-j 2 pi k n / N)`.
//! - Window is periodic Hann, `w[n] = 0.5 (1 - cos(2 pi n / N))`, `N = 1024`,
//!   with mean square `0.375`, coherent gain `0.5`, and ENBW `1.5` bins.
//! - Hop is 256 samples (75% overlap). Only full `N`-sample windows
//!   contribute; partial startup/ending data never seeds a hop.
//! - One-sided bins `0..=512`; bin frequency is `bin * sample_rate / N` Hz.
//!   Bins index frequency, Hz locates it: the same bin maps to different Hz
//!   at different rates, so spectra restore only at the capture rate.
//! - Stored powers are raw unnormalized `|X|^2` per bin (windowed FFT power,
//!   averaged across hops), not time-domain variance and not PSD. The live
//!   Wiener comparator uses the same units, so no rescaling is needed.
//! - DC (bin 0) and Nyquist (bin 512) are real-only and single-count in a
//!   broadband Parseval reconstruction; interior bins double-count. Per-bin
//!   Wiener use needs no doubling: each bin compares directly to the live
//!   `power[]` entry in identical units.
//! - Parseval (unnormalized forward): `sum |x w|^2 = (1/N) sum_two-sided |X|^2`,
//!   hence time variance `sigma^2 = doubled_one_sided / (N^2 * 0.375)`.
//!   White noise of variance `sigma^2` averages `sigma^2 * N * 0.375`
//!   (`384 sigma^2`) per bin.
//!
//! Realtime contract: [`SpectralCapture::new`] allocates FFT plans, window,
//! and accumulators. [`SpectralCapture::start`],
//! [`SpectralCapture::accumulate`], [`SpectralCapture::take_completed`], and
//! [`SpectralCapture::cancel`] never allocate, free, lock, log, or run
//! unbounded work.

// Rust guideline compliant 2026-02-21
use math_audio_dsp::stft::{RealFftProcessor, generate_hann_window};

/// FFT length for measured spectra, matching live WOLA.
pub const SPECTRAL_PROFILE_FFT_SIZE: usize = 1024;
/// Hop advance for measured spectra, matching live WOLA.
pub const SPECTRAL_PROFILE_HOP_SIZE: usize = SPECTRAL_PROFILE_FFT_SIZE / 4;
/// One-sided bin count for measured spectra (`N/2 + 1`).
pub const SPECTRAL_PROFILE_NUM_BINS: usize = SPECTRAL_PROFILE_FFT_SIZE / 2 + 1;
/// Window identifier persisted with measured spectra.
pub const SPECTRAL_PROFILE_WINDOW: &str = "hann-periodic";
/// Mean square of the periodic Hann window.
///
/// Periodic Hann `w[n] = 0.5 (1 - cos(2 pi n / N))` has `mean(w^2) = 3/8`.
/// Live Parseval reconstruction divides by this value; changing the window
/// requires a new persisted identifier and migration, never silent reuse.
pub const SPECTRAL_PROFILE_WINDOW_ENERGY_MEAN: f64 = 0.375;

// Compile-time locks against live-WOLA convention drift.
const _ASSERT_FFT_SIZE: () = assert!(
    SPECTRAL_PROFILE_FFT_SIZE == crate::spectral_hiss::SPECTRAL_HISS_FFT_SIZE
);
const _ASSERT_NUM_BINS: () = assert!(
    SPECTRAL_PROFILE_NUM_BINS == crate::spectral_hiss::SPECTRAL_HISS_NUM_BINS
);
const _ASSERT_HOP: () = assert!(
    SPECTRAL_PROFILE_HOP_SIZE == crate::spectral_hiss::SPECTRAL_HISS_FFT_SIZE / 4
);

/// Summary of a completed spectral capture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectralSummary {
    /// Sample rate the capture was measured at.
    pub sample_rate: f64,
    /// Channel count the capture was measured with.
    pub channels: usize,
    /// Input frames accumulated toward the target.
    pub frames_analyzed: u64,
    /// Full windows averaged into the spectrum (always positive).
    pub hops_analyzed: u64,
}

/// Validates a raw per-bin spectrum slice.
///
/// Slice layout is channel-major: `channels * NUM_BINS` entries with channel
/// `ch` occupying `ch * NUM_BINS..(ch + 1) * NUM_BINS`. Values are raw
/// unnormalized `|X|^2` means in live units.
///
/// # Errors
///
/// Returns an error for a channel-count/length mismatch or any non-finite
/// or negative power.
pub fn validate_spectrum_slice(spectrum: &[f32], channels: usize) -> Result<(), String> {
    if channels == 0 {
        return Err("spectral profile requires at least one channel".to_string());
    }
    let expected = channels
        .checked_mul(SPECTRAL_PROFILE_NUM_BINS)
        .ok_or_else(|| "spectral profile channel count overflow".to_string())?;
    if spectrum.len() != expected {
        return Err(format!(
            "spectral profile needs {expected} powers for {channels} channels, got {}",
            spectrum.len()
        ));
    }
    for (index, power) in spectrum.iter().enumerate() {
        if !power.is_finite() || *power < 0.0 {
            return Err(format!(
                "spectral profile power {index} is invalid: {power}"
            ));
        }
    }
    Ok(())
}

/// Pre-allocated measured-spectrum capture engine.
///
/// Mirrors live WOLA framing with a shared ring per channel: each input
/// frame advances the ring, and every completed `N`-sample window runs one
/// windowed forward FFT whose per-bin `|X|^2` accumulates in `f64`. Only
/// full windows contribute, so varied host callback partitions produce
/// identical spectra for identical sample order.
pub struct SpectralCapture {
    active: bool,
    channels: usize,
    sample_rate: f64,
    target_frames: u64,
    frames_seen: u64,
    hops: u64,
    input: Vec<Vec<f32>>,
    input_write: usize,
    input_fill: usize,
    fft: Vec<RealFftProcessor>,
    window: Vec<f32>,
    sum_power: Vec<f64>,
}

impl SpectralCapture {
    /// Creates an idle engine with buffers for `channels`.
    ///
    /// Allocates FFT plans (forward-only), the periodic Hann window, the
    /// framing rings, and the per-bin accumulators. Construction is the
    /// only allocating method; all later calls reuse this storage.
    pub fn new(channels: usize) -> Self {
        Self {
            active: false,
            channels,
            sample_rate: 48_000,
            target_frames: 0,
            frames_seen: 0,
            hops: 0,
            input: vec![vec![0.0; SPECTRAL_PROFILE_FFT_SIZE]; channels],
            input_write: 0,
            input_fill: 0,
            fft: (0..channels)
                .map(|_| RealFftProcessor::new_forward_only(SPECTRAL_PROFILE_FFT_SIZE))
                .collect(),
            window: generate_hann_window(SPECTRAL_PROFILE_FFT_SIZE),
            sum_power: vec![0.0; channels * SPECTRAL_PROFILE_NUM_BINS],
        }
    }

    /// Arms a capture at `sample_rate` for `target_frames` frames.
    ///
    /// Restarting while active discards the partial measurement. The target
    /// is supplied by the owner so broadband and spectral captures share
    /// one frame budget.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero channel count, sample rate, or target.
    pub fn start<S: Into<f64>>(&mut self, sample_rate: S, target_frames: u64) -> Result<(), String> {
        let sample_rate = sample_rate.into();
        if self.channels == 0 {
            return Err("spectral capture requires at least one channel".to_string());
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("spectral capture requires a nonzero sample rate".to_string());
        }
        if target_frames == 0 {
            return Err("spectral capture requires a nonzero target".to_string());
        }
        self.sample_rate = sample_rate;
        self.target_frames = target_frames;
        self.frames_seen = 0;
        self.hops = 0;
        self.input_write = 0;
        self.input_fill = 0;
        for channel in &mut self.input {
            channel.fill(0.0);
        }
        self.sum_power.fill(0.0);
        self.active = true;
        Ok(())
    }

    /// Discards any partial measurement and disarms capture.
    pub fn cancel(&mut self) {
        self.active = false;
        self.frames_seen = 0;
        self.hops = 0;
        self.input_write = 0;
        self.input_fill = 0;
        for channel in &mut self.input {
            channel.fill(0.0);
        }
        self.sum_power.fill(0.0);
    }

    /// Returns true while a capture is accumulating.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns true once the capture target has been reached.
    pub fn is_complete(&self) -> bool {
        self.active && self.target_frames > 0 && self.frames_seen >= self.target_frames
    }

    /// Returns capture progress in 0.0..=1.0 (0.0 while idle).
    pub fn progress(&self) -> f32 {
        if !self.active || self.target_frames == 0 {
            return 0.0;
        }
        (self.frames_seen as f32 / self.target_frames as f32).clamp(0.0, 1.0)
    }

    /// Returns the frames accumulated toward the current target.
    pub fn frames_accumulated(&self) -> u64 {
        self.frames_seen
    }

    /// Returns the full windows analyzed so far.
    pub fn hops_analyzed(&self) -> u64 {
        self.hops
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
        let mask = SPECTRAL_PROFILE_FFT_SIZE - 1;
        for frame in 0..frames {
            let base = frame * self.channels;
            for (ch, ring) in self.input.iter_mut().enumerate() {
                let sample = input[base + ch];
                ring[self.input_write] = if sample.is_finite() { sample } else { 0.0 };
            }
            self.input_write = (self.input_write + 1) & mask;
            self.input_fill += 1;
            if self.input_fill == SPECTRAL_PROFILE_FFT_SIZE {
                self.analyze_hop();
                self.input_fill = SPECTRAL_PROFILE_FFT_SIZE - SPECTRAL_PROFILE_HOP_SIZE;
            }
        }
        self.frames_seen += frames as u64;
    }

    /// Runs one windowed FFT and accumulates per-bin powers.
    fn analyze_hop(&mut self) {
        let mask = SPECTRAL_PROFILE_FFT_SIZE - 1;
        for ch in 0..self.channels {
            for i in 0..SPECTRAL_PROFILE_FFT_SIZE {
                let source = (self.input_write + i) & mask;
                self.fft[ch].time_buffer[i] = self.input[ch][source] * self.window[i];
            }
            self.fft[ch].forward();
            let base = ch * SPECTRAL_PROFILE_NUM_BINS;
            for bin in 0..SPECTRAL_PROFILE_NUM_BINS {
                let value = self.fft[ch].freq_buffer[bin];
                let power = value.re * value.re + value.im * value.im;
                self.sum_power[base + bin] += f64::from(power);
            }
        }
        self.hops += 1;
    }

    /// Writes mean per-bin powers and disarms capture.
    ///
    /// Output layout is channel-major (`channels * NUM_BINS`). Returns
    /// `None` (leaving everything untouched) unless the frame target is
    /// reached, at least one full window was analyzed, and `powers` holds
    /// exactly one bin table per channel.
    pub fn take_completed(&mut self, powers: &mut [f32]) -> Option<SpectralSummary> {
        if !self.is_complete()
            || self.hops == 0
            || powers.len() != self.channels * SPECTRAL_PROFILE_NUM_BINS
        {
            return None;
        }
        let hops = self.hops as f64;
        for (slot, accumulated) in powers.iter_mut().zip(self.sum_power.iter()) {
            *slot = (accumulated / hops) as f32;
        }
        let summary = SpectralSummary {
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames_analyzed: self.frames_seen,
            hops_analyzed: self.hops,
        };
        self.cancel();
        Some(summary)
    }
}

impl Clone for SpectralCapture {
    fn clone(&self) -> Self {
        Self {
            active: self.active,
            channels: self.channels,
            sample_rate: self.sample_rate,
            target_frames: self.target_frames,
            frames_seen: self.frames_seen,
            hops: self.hops,
            input: self.input.clone(),
            input_write: self.input_write,
            input_fill: self.input_fill,
            fft: (0..self.channels)
                .map(|_| RealFftProcessor::new_forward_only(SPECTRAL_PROFILE_FFT_SIZE))
                .collect(),
            window: self.window.clone(),
            sum_power: self.sum_power.clone(),
        }
    }
}

impl std::fmt::Debug for SpectralCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectralCapture")
            .field("active", &self.active)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("target_frames", &self.target_frames)
            .field("frames_seen", &self.frames_seen)
            .field("hops", &self.hops)
            .field("input_fill", &self.input_fill)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_validation_rejects_bad_shapes_and_values() {
        let good = vec![1.0; SPECTRAL_PROFILE_NUM_BINS];
        assert!(validate_spectrum_slice(&good, 1).is_ok());
        assert!(validate_spectrum_slice(&good, 0).is_err());
        assert!(validate_spectrum_slice(&[1.0; 7], 1).is_err());
        let mut bad = good.clone();
        bad[3] = f32::NAN;
        assert!(validate_spectrum_slice(&bad, 1).is_err());
        bad[3] = -1.0;
        assert!(validate_spectrum_slice(&bad, 1).is_err());
        bad[3] = f32::INFINITY;
        assert!(validate_spectrum_slice(&bad, 1).is_err());
    }

    #[test]
    fn start_rejects_degenerate_arguments() {
        let mut capture = SpectralCapture::new(0);
        assert!(capture.start(48_000, 48_000).is_err());
        let mut capture = SpectralCapture::new(1);
        assert!(capture.start(0, 48_000).is_err());
        assert!(capture.start(48_000, 0).is_err());
        assert!(!capture.is_active());
    }

    #[test]
    fn partitions_do_not_change_hops_or_powers() {
        fn run(partitions: &[usize]) -> (Vec<f32>, u64) {
            let mut capture = SpectralCapture::new(1);
            capture.start(48_000, 48_000).unwrap();
            let mut state = 0x1234_5678u32;
            let mut input = Vec::with_capacity(48_000);
            for _ in 0..48_000 {
                state = state
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                input.push((state as f32 / u32::MAX as f32) * 2.0 - 1.0);
            }
            let mut offset = 0;
            let mut part = 0;
            while offset < input.len() {
                let count = partitions[part % partitions.len()].min(input.len() - offset);
                capture.accumulate(&input[offset..offset + count]);
                offset += count;
                part += 1;
            }
            let mut powers = vec![0.0; SPECTRAL_PROFILE_NUM_BINS];
            let summary = capture.take_completed(&mut powers).unwrap();
            (powers, summary.hops_analyzed)
        }
        let (single, hops_single) = run(&[48_000]);
        let (varied, hops_varied) = run(&[1, 64, 511, 73, 997]);
        assert_eq!(hops_single, hops_varied);
        assert_eq!(single, varied);
    }
}
