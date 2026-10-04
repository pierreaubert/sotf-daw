//! Prepared, exact EBU Tech 3342 gating and percentile selection.

// Rust guideline compliant 2026-02-21
use crate::analyzer::{
    LoudnessRangeConfig, LoudnessRangeData, LoudnessRangeMode, LoudnessRangeStatus,
};

pub(super) struct LoudnessRangeHistory {
    config: LoudnessRangeConfig,
    history: Vec<f64>,
    scratch: Vec<f64>,
    next: usize,
    dirty: bool,
    data: LoudnessRangeData,
}

impl LoudnessRangeHistory {
    pub(super) fn new(config: LoudnessRangeConfig, sample_rate: f64) -> Result<Self, String> {
        if !(1..=36_000).contains(&config.capacity_windows) {
            return Err("loudness range capacity must be between 1 and 36000 windows".into());
        }
        let bytes = config
            .capacity_windows
            .checked_mul(2 * size_of::<f64>())
            .filter(|bytes| *bytes <= isize::MAX as usize)
            .ok_or_else(|| "loudness range storage size overflow".to_string())?;
        let prepare = || {
            let mut values = Vec::new();
            values
                .try_reserve_exact(config.capacity_windows)
                .map_err(|error| format!("cannot prepare {bytes} bytes of LRA storage: {error}"))?;
            values.resize(config.capacity_windows, 0.0);
            Ok::<_, String>(values)
        };
        Ok(Self {
            config,
            history: prepare()?,
            scratch: prepare()?,
            next: 0,
            dirty: false,
            data: LoudnessRangeData {
                range_lu: None,
                is_stable: false,
                status: LoudnessRangeStatus::WarmingUp,
                mode: config.mode,
                retained_windows: 0,
                observed_windows: 0,
                capacity_windows: config.capacity_windows,
                timebase_is_exact: sample_rate.rem_euclid(10.0) == 0.0,
            },
        })
    }

    pub(super) fn config(&self) -> LoudnessRangeConfig {
        self.config
    }

    pub(super) fn empty_data(&self) -> LoudnessRangeData {
        LoudnessRangeData {
            range_lu: None,
            status: LoudnessRangeStatus::WarmingUp,
            retained_windows: 0,
            observed_windows: 0,
            ..self.data
        }
    }

    pub(super) fn reset(&mut self) {
        self.data = self.empty_data();
        self.next = 0;
        self.dirty = false;
    }

    pub(super) fn observe(&mut self, loudness: Result<f64, String>) {
        self.data.observed_windows = self.data.observed_windows.saturating_add(1);
        if matches!(
            self.data.status,
            LoudnessRangeStatus::CapacityExceeded | LoudnessRangeStatus::MeasurementError
        ) {
            return;
        }
        let energy = match loudness {
            Ok(f64::NEG_INFINITY) => 0.0,
            Ok(value) if value.is_finite() => 10.0_f64.powf((value + 0.691) / 10.0),
            _ => f64::NAN,
        };
        if !energy.is_finite() || energy < 0.0 {
            self.data.status = LoudnessRangeStatus::MeasurementError;
            self.data.range_lu = None;
            self.dirty = false;
            return;
        }
        if self.data.retained_windows == self.config.capacity_windows
            && self.config.mode == LoudnessRangeMode::WholeProgram
        {
            self.data.status = LoudnessRangeStatus::CapacityExceeded;
            self.data.range_lu = None;
            self.dirty = false;
            return;
        }
        self.history[self.next] = energy;
        self.next = (self.next + 1) % self.config.capacity_windows;
        self.data.retained_windows =
            (self.data.retained_windows + 1).min(self.config.capacity_windows);
        self.dirty = true;
    }

    pub(super) fn query(&mut self) -> LoudnessRangeData {
        if !self.dirty {
            return self.data;
        }
        self.dirty = false;
        let absolute_gate = 10.0_f64.powf((-70.0 + 0.691) / 10.0);
        let history = &self.history[..self.data.retained_windows];
        let max = history.iter().copied().fold(0.0_f64, f64::max);
        let mut count = 0;
        let mut normalized_sum = 0.0;
        for &energy in history {
            if energy >= absolute_gate {
                normalized_sum += energy / max;
                count += 1;
            }
        }
        if count == 0 {
            self.data.range_lu = None;
            self.data.status = LoudnessRangeStatus::BelowGate;
            return self.data;
        }
        // Average before rescaling: summing large finite energies cannot overflow.
        let relative_gate = (normalized_sum / count as f64) * max * 0.01;
        let gate = absolute_gate.max(relative_gate);
        let mut survivors = 0;
        for &energy in history {
            if energy >= gate {
                self.scratch[survivors] = energy;
                survivors += 1;
            }
        }
        // The maximum always survives a threshold 20 LU below the mean.
        debug_assert!(survivors > 0);
        // Exact integer form of round((n - 1) * p), without rounding ambiguity.
        let lower_rank = ((survivors - 1) * 10 + 50) / 100;
        let upper_rank = ((survivors - 1) * 95 + 50) / 100;
        let (below_upper, upper, _) =
            self.scratch[..survivors].select_nth_unstable_by(upper_rank, f64::total_cmp);
        let upper = *upper;
        let lower = if lower_rank == upper_rank {
            upper
        } else {
            *below_upper
                .select_nth_unstable_by(lower_rank, f64::total_cmp)
                .1
        };
        let range = 10.0 * (upper.log10() - lower.log10());
        if range.is_finite() && range >= 0.0 {
            self.data.range_lu = Some(range);
            self.data.status = LoudnessRangeStatus::Valid;
        } else {
            self.data.range_lu = None;
            self.data.status = LoudnessRangeStatus::MeasurementError;
        }
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(levels: &[f64]) -> Option<f64> {
        // Independent direct level-domain implementation with a full sort.
        let absolute: Vec<_> = levels.iter().copied().filter(|x| *x >= -70.0).collect();
        if absolute.is_empty() {
            return None;
        }
        let mean = absolute
            .iter()
            .map(|x| 10.0_f64.powf(x / 10.0))
            .sum::<f64>()
            / absolute.len() as f64;
        let threshold = 10.0 * mean.log10() - 20.0;
        let mut survivors: Vec<_> = absolute.into_iter().filter(|x| *x >= threshold).collect();
        survivors.sort_by(f64::total_cmp);
        let percentile = |p: f64| survivors[((survivors.len() - 1) as f64 * p).round() as usize];
        Some(percentile(0.95) - percentile(0.10))
    }

    fn history(mode: LoudnessRangeMode, capacity: usize) -> LoudnessRangeHistory {
        LoudnessRangeHistory::new(
            LoudnessRangeConfig {
                mode,
                capacity_windows: capacity,
            },
            48_000,
        )
        .unwrap()
    }

    #[test]
    fn exact_selection_matches_independent_sorted_loudness_vectors() {
        let mut cases = vec![
            vec![],
            vec![f64::NEG_INFINITY; 7],
            vec![-70.0001],
            vec![-70.0],
            vec![-20.0; 101],
            vec![-30.0, -20.0],
            vec![-90.0, -60.0, -35.0, -20.0],
            vec![-70.0, -70.0001, -69.9999],
        ];
        for count in [2, 3, 6, 11, 21, 99, 100, 101, 1_001] {
            for shift in [-10.0, 0.0, 12.0] {
                cases.push(
                    (0..count)
                        .map(|i| {
                            let n = (i * 197 + 17) % 101;
                            -65.0 + n as f64 * 0.5 + shift
                        })
                        .collect(),
                );
            }
        }
        for levels in cases {
            let mut h = history(LoudnessRangeMode::WholeProgram, levels.len().max(1));
            for &level in &levels {
                h.observe(Ok(level));
            }
            let result = h.query();
            match (reference(&levels), result.range_lu) {
                (Some(expected), Some(actual)) => assert!((actual - expected).abs() < 1e-10),
                (None, None) => {}
                values => panic!("different availability: {values:?}"),
            }
            assert_eq!(result, h.query(), "cached queries must not change state");
        }
    }

    #[test]
    fn rolling_silence_ages_out_history_and_whole_program_exhaustion_latches() {
        let mut rolling = history(LoudnessRangeMode::Rolling, 3);
        for level in [-40.0, -30.0, -20.0, -10.0] {
            rolling.observe(Ok(level));
        }
        let data = rolling.query();
        assert_eq!(data.retained_windows, 3);
        assert_eq!(data.observed_windows, 4);
        assert!(
            (data.range_lu.unwrap() - reference(&[-30.0, -20.0, -10.0]).unwrap()).abs() < 1e-10
        );
        for _ in 0..3 {
            rolling.observe(Ok(f64::NEG_INFINITY));
        }
        assert_eq!(rolling.query().status, LoudnessRangeStatus::BelowGate);
        let mut program = history(LoudnessRangeMode::WholeProgram, 2);
        for level in [-20.0, -30.0] {
            program.observe(Ok(level));
        }
        assert_eq!(program.query().status, LoudnessRangeStatus::Valid);
        program.observe(Ok(-25.0));
        assert_eq!(
            program.query().status,
            LoudnessRangeStatus::CapacityExceeded
        );
        program.observe(Ok(-40.0));
        assert_eq!(program.query().observed_windows, 4);
        assert_eq!(program.query().retained_windows, 2);
        program.reset();
        assert_eq!(program.query().status, LoudnessRangeStatus::WarmingUp);
        program.observe(Ok(-20.0));
        assert_eq!(program.query().range_lu, Some(0.0));
    }

    #[test]
    fn invalid_observations_latch_and_wide_finite_energy_does_not_overflow_sum() {
        for value in [f64::NAN, f64::INFINITY, 4_000.0] {
            let mut h = history(LoudnessRangeMode::Rolling, 8);
            h.observe(Ok(value));
            h.observe(Ok(-20.0));
            assert_eq!(h.query().status, LoudnessRangeStatus::MeasurementError);
            assert_eq!(h.query().range_lu, None);
            h.reset();
            h.observe(Ok(-20.0));
            assert_eq!(h.query().range_lu, Some(0.0));
        }
        let mut h = history(LoudnessRangeMode::Rolling, 100);
        for i in 0..100 {
            h.observe(Ok(if i < 50 { 3_075.0 } else { 3_065.0 }));
        }
        assert!((h.query().range_lu.unwrap() - 10.0).abs() < 1e-10);
        for capacity in [0, 36_001, usize::MAX] {
            assert!(
                LoudnessRangeHistory::new(
                    LoudnessRangeConfig {
                        capacity_windows: capacity,
                        ..Default::default()
                    },
                    48_000,
                )
                .is_err()
            );
        }
    }
}
