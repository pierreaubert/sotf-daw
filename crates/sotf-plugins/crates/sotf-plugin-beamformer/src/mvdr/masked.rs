//! QA-only mask-weighted covariance with continuously retained native fallback.

// Rust guideline compliant 2026-02-21
use super::MvdrBeamformer;
use crate::noise_mask::{NoiseMask, spatial_features};
use nalgebra::Complex;

/// Hard cap matching the host's 512-point real FFT; no extra analysis is queued.
const MAX_BINS: usize = 257;

/// QA decision counts; no allocation or callback telemetry transport is required.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MaskCounters {
    /// Confident noise bins selected for weighted learning.
    pub noise_bins: u64,
    /// Confident target bins whose noise covariance is held.
    pub target_bins: u64,
    /// Uncertain bins using the continuously running native covariance/weights.
    pub uncertain_bins: u64,
    /// Invalid-feature bins using native fallback.
    pub invalid_bins: u64,
    /// Disabled frames using native fallback.
    pub disabled_frames: u64,
    /// Frames without a model or supported context.
    pub unavailable_frames: u64,
}

/// Prepared parallel MVDR state for explicit unadmitted mask qualification.
#[derive(Debug)]
pub struct MaskPath {
    model: Option<NoiseMask>,
    learned: MvdrBeamformer,
    enabled: bool,
    ready: bool,
    counters: MaskCounters,
}

impl MaskPath {
    /// Allocate bounded parallel state on the control thread, initially disabled.
    ///
    /// # Panics
    /// Panics for invalid internal dimensions; callers must provide2..8 mics/2..257 bins.
    pub fn new(model: Option<NoiseMask>, microphones: usize, bins: usize) -> Self {
        assert!((2..=8).contains(&microphones) && (2..=MAX_BINS).contains(&bins));
        Self {
            model,
            learned: MvdrBeamformer::new(microphones, bins),
            enabled: false,
            ready: false,
            counters: MaskCounters::default(),
        }
    }

    /// Toggle selection; the production covariance continues independently.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Clear all learned history/counters, retaining model and enable choice.
    pub fn reset(&mut self) {
        self.learned.reset();
        self.ready = false;
        self.counters = MaskCounters::default();
    }

    /// Copy decision counters without allocation.
    pub fn counters(&self) -> MaskCounters {
        self.counters
    }

    /// Indicate whether any complete real-evidence frame has prepared weights.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Apply retained weights during drain without inference or covariance learning.
    pub fn apply_frozen(&mut self, channels: &[Vec<Complex<f32>>]) -> &[Complex<f32>] {
        self.learned.apply_weights_into(channels)
    }

    /// Process current evidence using bounded learned bins and exact native fallback.
    ///
    /// `baseline` must already contain this frame's native update/solve. Uncertain,
    /// invalid and disabled bins copy its retained covariance and weights exactly.
    /// Both states remain bounded by the production MVDR weight solver.
    ///
    /// # Panics
    /// Panics on internal dimension mismatch with the prepared native baseline.
    pub fn apply(
        &mut self,
        baseline: &MvdrBeamformer,
        channels: &[Vec<Complex<f32>>],
        steering: &[Vec<Complex<f32>>],
        context_supported: bool,
    ) -> &[Complex<f32>] {
        assert_eq!(baseline.num_mics, self.learned.num_mics);
        assert_eq!(baseline.spectrum_size, self.learned.spectrum_size);
        let bins = self.learned.spectrum_size;
        let mut masks = [0.0_f32; MAX_BINS];
        let mut selected = [false; MAX_BINS];
        if !self.enabled {
            self.counters.disabled_frames = self.counters.disabled_frames.saturating_add(1);
        } else if self.model.is_none() || !context_supported {
            self.counters.unavailable_frames = self.counters.unavailable_frames.saturating_add(1);
        } else if let Some(model) = &self.model {
            for k in 0..bins {
                let probability = steering
                    .get(k)
                    .and_then(|d| spatial_features(channels, d, k, k as f64 / (bins - 1) as f64))
                    .and_then(|f| model.probability(&f));
                match probability {
                    Some(p) if p >= 0.85 => {
                        selected[k] = true;
                        masks[k] = p;
                        self.counters.noise_bins = self.counters.noise_bins.saturating_add(1);
                    }
                    Some(p) if p <= 0.15 => {
                        selected[k] = true;
                        self.counters.target_bins = self.counters.target_bins.saturating_add(1);
                    }
                    Some(_) => {
                        self.counters.uncertain_bins =
                            self.counters.uncertain_bins.saturating_add(1);
                    }
                    None => {
                        self.counters.invalid_bins = self.counters.invalid_bins.saturating_add(1);
                    }
                }
            }
        }
        self.learned
            .update_noise_covariance_masked(channels, &masks[..bins]);
        let mm = baseline.num_mics * baseline.num_mics;
        for (k, &take_learned) in selected[..bins].iter().enumerate() {
            if !take_learned {
                self.learned.noise_cov[k * mm..(k + 1) * mm]
                    .copy_from_slice(&baseline.noise_cov[k * mm..(k + 1) * mm]);
            }
        }
        if selected[..bins].iter().any(|&v| v) {
            self.learned.compute_weights(steering);
        }
        for (k, &take_learned) in selected[..bins].iter().enumerate() {
            if !take_learned {
                self.learned.weights_buf[k].copy_from_slice(&baseline.weights_buf[k]);
            }
        }
        self.ready = true;
        self.learned.apply_weights_into(channels)
    }
}

impl MvdrBeamformer {
    /// Update QA covariance using finite per-bin noise weights in0..1.
    ///
    /// Effective observation weight is `(1-alpha)*mask`, retaining its complement.
    /// Zero masks leave a bin unchanged. Invalid geometry/masks/input reject the
    /// entire frame without mutation. Overflowing bins retain prior covariance.
    /// The actual bounded native weight solve remains responsible for output.
    pub fn update_noise_covariance_masked(
        &mut self,
        channels: &[Vec<Complex<f32>>],
        masks: &[f32],
    ) -> bool {
        let m = self.num_mics;
        if channels.len() != m
            || masks.len() != self.spectrum_size
            || masks
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || channels.iter().any(|c| {
                c.len() < self.spectrum_size
                    || c[..self.spectrum_size]
                        .iter()
                        .any(|v| !v.re.is_finite() || !v.im.is_finite())
            })
        {
            return false;
        }
        self.frame_count = self.frame_count.saturating_add(1);
        let mm = m * m;
        let mut changed = false;
        for (k, &mask) in masks.iter().enumerate() {
            if mask == 0.0 {
                continue;
            }
            let beta = (1.0 - self.alpha) * mask;
            let keep = if mask == 1.0 { self.alpha } else { 1.0 - beta };
            let offset = k * mm;
            let mut valid = true;
            for i in 0..m {
                for j in 0..m {
                    let outer = channels[i][k] * channels[j][k].conj();
                    let previous = self.noise_cov[offset + i * m + j];
                    let value = Complex::new(
                        previous.re * keep + outer.re * beta,
                        previous.im * keep + outer.im * beta,
                    );
                    valid &= value.re.is_finite() && value.im.is_finite();
                    self.scratch_outer[i * m + j] = value;
                }
            }
            if !valid {
                valid = true;
                for i in 0..m {
                    for j in 0..m {
                        let wide = |v: Complex<f32>| Complex::new(f64::from(v.re), f64::from(v.im));
                        let outer = wide(channels[i][k]) * wide(channels[j][k]).conj();
                        let previous = self.noise_cov[offset + i * m + j];
                        let value = Complex::new(
                            (f64::from(previous.re) * f64::from(keep) + outer.re * f64::from(beta))
                                as f32,
                            (f64::from(previous.im) * f64::from(keep) + outer.im * f64::from(beta))
                                as f32,
                        );
                        valid &= value.re.is_finite() && value.im.is_finite();
                        self.scratch_outer[i * m + j] = value;
                    }
                }
            }
            if valid {
                self.noise_cov[offset..offset + mm].copy_from_slice(&self.scratch_outer[..mm]);
                changed = true;
            }
        }
        self.weights_dirty |= changed;
        changed
    }
}
