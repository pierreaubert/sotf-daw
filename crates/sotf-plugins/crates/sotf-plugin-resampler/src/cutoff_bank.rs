//! Prepared anti-alias cutoff selection for variable-ratio sinc conversion.

// Rust guideline compliant 2026-02-21
use rubato::{Async, Resampler};

/// Eight intervals per octave limit ordinary cutoff quantization to 8.3%.
const INTERVALS_PER_OCTAVE: i32 = 8;
/// Protect small negative clock corrections without dropping a whole grid interval.
const DRIFT_CUTOFF_SCALE: f64 = 0.999;

/// Cutoff ratios in the same order as the backend's prepared interpolators.
#[derive(Debug)]
pub(super) struct CutoffBank {
    ratios: Vec<f64>,
}

impl CutoffBank {
    pub(super) fn new(nominal: f64) -> Self {
        let nominal_cutoff = nominal.min(1.0);
        let mut ratios = Vec::with_capacity(18);
        // Slot zero preserves the original nominal filter and reset behavior.
        ratios.push(nominal_cutoff);
        for interval in -INTERVALS_PER_OCTAVE..=INTERVALS_PER_OCTAVE {
            let ratio = (nominal
                * 2.0_f64.powf(f64::from(interval) / f64::from(INTERVALS_PER_OCTAVE)))
            .min(1.0);
            if !ratios.contains(&ratio) {
                ratios.push(ratio);
            }
        }
        let drift = nominal_cutoff * DRIFT_CUTOFF_SCALE;
        if drift >= nominal / 2.0 && !ratios.contains(&drift) {
            ratios.push(drift);
        }
        Self { ratios }
    }

    /// Additional tables; the backend constructs the nominal table itself.
    pub(super) fn additional_ratios(&self) -> &[f64] {
        &self.ratios[1..]
    }

    pub(super) fn select(&self, backend: &mut Async<f32>, target_ratio: f64) {
        // The inverse-ratio ramp is monotonic. Use the narrower endpoint for
        // every sample in this block, including repeated pending updates.
        let ceiling = backend.resample_ratio().min(target_ratio).min(1.0);
        let mut selected = 0;
        let mut selected_ratio = 0.0;
        for (index, &ratio) in self.ratios.iter().enumerate() {
            if ratio <= ceiling && ratio > selected_ratio {
                selected = index;
                selected_ratio = ratio;
            }
        }
        // Accepted backend ratios always lie inside the prepared range.
        debug_assert!(selected_ratio > 0.0);
        let valid = backend.select_sinc_cutoff(selected);
        debug_assert!(valid);
    }
}
