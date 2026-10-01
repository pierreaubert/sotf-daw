//! Prepared anti-alias cutoff selection for variable-ratio sinc conversion.
//!
//! The bank holds one same-length coefficient table per prepared cutoff
//! ratio. Selection never allocates, destroys coefficients, or resets
//! input history and timing; every table shares the backend's timing and
//! history geometry, so selection changes filter response but never the
//! emitted clock, frame counts, latency, or drain completion.
//!
//! Smoothing policy (opt-in via the `cutoff_smoothing` parameter, default
//! off): narrowing jumps to the safe target immediately so a downward ratio
//! change can never expose a transient alias burst, while widening advances
//! at most one prepared table per selection call. Selection runs at control
//! time and once per backend chunk (ramped upward advances only per chunk,
//! since the control-time select is a no-op while the backend still reports
//! the narrow ratio), so the slew is deterministic in stream position and
//! independent of host callback partitioning.

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
    /// Backend slots ordered by ascending cutoff ratio for slew steps.
    order: Vec<usize>,
    /// Currently selected slot, tracked across every selection call.
    current: usize,
    /// Asymmetric slew enabled; narrowing still jumps immediately.
    smoothing: bool,
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
        let mut order: Vec<usize> = (0..ratios.len()).collect();
        order.sort_by(|&a, &b| ratios[a].total_cmp(&ratios[b]));
        Self {
            ratios,
            order,
            current: 0,
            smoothing: false,
        }
    }

    /// Additional tables; the backend constructs the nominal table itself.
    pub(super) fn additional_ratios(&self) -> &[f64] {
        &self.ratios[1..]
    }

    /// Safe target slot for a backend/target ratio pair.
    ///
    /// The inverse-ratio ramp is monotonic, so the ceiling uses the narrower
    /// endpoint for every sample in the block, including repeated pending
    /// updates. Pure in the ratio pair; unit tests drive it directly.
    pub(super) fn target_slot(&self, backend_ratio: f64, target_ratio: f64) -> usize {
        let ceiling = backend_ratio.min(target_ratio).min(1.0);
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
        selected
    }

    /// Move the tracked selection toward a safe target slot.
    ///
    /// Without smoothing, or when narrowing, this jumps immediately to the
    /// target. With smoothing and a wider target, it advances exactly one
    /// prepared table in cutoff order, bounding the per-chunk response step
    /// to one grid interval (about 8.3% bandwidth, less near the drift
    /// anchor). Returns the newly selected slot.
    pub(super) fn advance_toward(&mut self, target: usize) -> usize {
        let target_ratio = self.ratios[target];
        if !self.smoothing || target_ratio <= self.ratios[self.current] {
            self.current = target;
            return target;
        }
        let current_rank = self
            .order
            .iter()
            .position(|&slot| slot == self.current)
            .expect("tracked slot is a prepared table");
        let target_rank = self
            .order
            .iter()
            .position(|&slot| slot == target)
            .expect("target slot is a prepared table");
        debug_assert!(
            target_rank > current_rank,
            "wider target must rank above the tracked slot"
        );
        self.current = self.order[current_rank.saturating_add(1).min(target_rank)];
        self.current
    }

    pub(super) fn select(&mut self, backend: &mut Async<f32>, target_ratio: f64) {
        let target = self.target_slot(backend.resample_ratio(), target_ratio);
        let selected = self.advance_toward(target);
        let valid = backend.select_sinc_cutoff(selected);
        debug_assert!(valid);
    }

    /// Track the backend's reset to slot zero; the smoothing flag persists.
    pub(super) fn reset(&mut self) {
        self.current = 0;
    }

    pub(super) fn set_smoothing(&mut self, enabled: bool) {
        self.smoothing = enabled;
    }

    pub(super) fn smoothing(&self) -> bool {
        self.smoothing
    }

    /// Tracked slot for white-box trajectory tests.
    #[cfg(test)]
    pub(super) fn current_slot(&self) -> usize {
        self.current
    }

    /// Cutoff ratio of a prepared slot for white-box trajectory tests.
    #[cfg(test)]
    pub(super) fn cutoff_ratio(&self, slot: usize) -> f64 {
        self.ratios[slot]
    }

    /// Rank of a slot in ascending cutoff order for trajectory tests.
    #[cfg(test)]
    pub(super) fn cutoff_rank(&self, slot: usize) -> usize {
        self.order
            .iter()
            .position(|&candidate| candidate == slot)
            .expect("slot is a prepared table")
    }

    /// Number of prepared tables for trajectory termination bounds.
    #[cfg(test)]
    pub(super) fn table_count(&self) -> usize {
        self.ratios.len()
    }
}
