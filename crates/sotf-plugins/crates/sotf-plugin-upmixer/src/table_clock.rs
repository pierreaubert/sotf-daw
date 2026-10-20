//! Frequency-table trajectories follow accepted source analysis time.

// Rust guideline compliant 2026-02-21
use crate::UpmixerPlugin;

impl UpmixerPlugin {
    pub(super) fn is_table_clock_parameter(index: usize) -> bool {
        matches!(index, 6 | 14 | 22)
    }

    pub(super) fn flush_table_clock(&mut self) {
        let n = std::mem::take(&mut self.param_smoothers.table_pending_frames);
        if n == 0 {
            return;
        }
        let new_lfe = self.param_smoothers.lfe_cutoff_hz_smoother.next_n(n);
        let new_bp = self.param_smoothers.bandpass_hz_smoother.next_n(n);
        let new_hf = self.param_smoothers.height_hf_cap_hz_smoother.next_n(n);

        // Retain the existing change thresholds, table equations and units.
        // Commit the coupled frequencies together before regenerating their caches.
        let lfe_changed = (new_lfe - self.params.lfe_cutoff_hz).abs() > 0.01;
        let bp_changed = (new_bp - self.params.bandpass_hz).abs() > 0.01;
        let hf_changed = (new_hf - self.height.height_hf_cap_hz).abs() > 0.1;
        if lfe_changed {
            self.params.lfe_cutoff_hz = new_lfe;
            self.update_crossover_gains();
        }
        if bp_changed {
            self.params.bandpass_hz = new_bp;
        }
        if hf_changed {
            self.height.height_hf_cap_hz = new_hf;
        }
        if bp_changed || hf_changed {
            self.precompute_height_freq_weights();
        }
        if lfe_changed || bp_changed {
            self.recache_bin_indices();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::{Plugin, ProcessContext};

    fn processor(rate: f64) -> UpmixerPlugin {
        let params = serde_json::from_value(serde_json::json!({
            "fft_size":2048,"speaker_config":"2.0","enable_hr_direct":false
        }))
        .unwrap();
        let mut p = UpmixerPlugin::from_params(params);
        p.initialize(rate).unwrap();
        p
    }

    fn current(p: &UpmixerPlugin) -> [f32; 3] {
        [
            p.param_smoothers.lfe_cutoff_hz_smoother.current(),
            p.param_smoothers.bandpass_hz_smoother.current(),
            p.param_smoothers.height_hf_cap_hz_smoother.current(),
        ]
    }

    fn evolve(initial: f64, target: f64, rate: f64, frames: i32) -> f64 {
        let coefficient = f64::from((-1.0 / (rate * 0.05)).exp() as f32);
        target + coefficient.powi(frames) * (initial - target)
    }

    fn check(actual: f32, expected: f64, span: f64, frames: usize) {
        // Bound the retained f32 repeated-squaring/multiply/add arithmetic by gamma_n.
        let rounding = (frames + 4) as f64 * f64::from(f32::EPSILON);
        let bound = rounding / (1.0 - rounding) * (1.0 + span.abs());
        assert!(
            (f64::from(actual) - expected).abs() <= bound,
            "actual={actual} expected={expected} bound={bound}"
        );
    }

    #[test]
    fn all_three_tables_advance_the_existing_one_pole_on_the_analysis_grid() {
        for rate in [44_100.0, 48_000.0, 192_000.0] {
            let mut p = processor(rate);
            let initial = current(&p);
            let targets = [180.0, 150.0, 8000.0];
            for (index, target) in [6, 14, 22].into_iter().zip(targets) {
                p.set_param_value(index, target);
            }
            p.advance_analysis_clock(256);
            for ((actual, initial), target) in current(&p).into_iter().zip(initial).zip(targets) {
                check(
                    actual,
                    evolve(f64::from(initial), target, rate, 256),
                    f64::from(initial) - target,
                    256,
                );
                assert_ne!(actual, initial);
            }
            assert_eq!(p.param_smoothers.table_pending_frames, 0);
            assert_eq!(p.params.lfe_cutoff_hz, current(&p)[0]);
            assert_eq!(p.params.bandpass_hz, current(&p)[1]);
            assert_eq!(p.height.height_hf_cap_hz, current(&p)[2]);
            assert_eq!(
                p.cache.cached_bandpass_bin,
                (p.params.bandpass_hz * 2048.0 / rate as f32) as usize
            );
        }
    }

    #[test]
    fn interrupted_table_targets_preserve_elapsed_time_and_ignore_unrelated_gain_events() {
        for rate in [44_100.0, 48_000.0, 192_000.0] {
            let mut p = processor(rate);
            let initial = current(&p);
            for (index, target) in [(6, 180.0), (14, 150.0), (22, 8000.0)] {
                p.set_param_value(index, target);
            }
            p.advance_analysis_clock(37);
            p.set_param_value(1, 0.2); // A gain observation does not observe frequency tables.
            assert_eq!(current(&p), initial);
            assert_eq!(p.param_smoothers.table_pending_frames, 37);
            p.process(&[], &mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            assert!(
                p.process(&[], &mut [], &ProcessContext::new(rate, 1))
                    .is_err()
            );
            assert_eq!(p.param_smoothers.table_pending_frames, 37);
            p.set_param_value(6, 60.0);
            let first: [f64; 3] = std::array::from_fn(|i| {
                evolve(f64::from(initial[i]), [180.0, 150.0, 8000.0][i], rate, 37)
            });
            for (i, actual) in current(&p).into_iter().enumerate() {
                check(actual, first[i], 8000.0, 37);
            }
            p.advance_analysis_clock(19);
            p.set_param_value(6, 60.0);
            assert_eq!(p.param_smoothers.table_pending_frames, 19);
            assert_eq!(p.gains.clock_phase, 56);
            p.advance_analysis_clock(200);
            for (i, actual) in current(&p).into_iter().enumerate() {
                check(
                    actual,
                    evolve(first[i], [60.0, 150.0, 8000.0][i], rate, 219),
                    8000.0,
                    256,
                );
            }
            assert_eq!(p.param_smoothers.table_pending_frames, 0);
            assert_eq!(p.gains.clock_phase, 0);
        }
    }

    #[test]
    fn reset_and_reinitialization_commit_pending_table_time_using_the_old_clock() {
        let mut p = processor(48_000.0);
        p.set_param_value(6, 180.0);
        p.advance_analysis_clock(37);
        assert!(p.initialize(f64::NAN).is_err());
        assert_eq!(p.param_smoothers.table_pending_frames, 37);
        p.reset();
        let first = evolve(120.0, 180.0, 48_000.0, 37);
        check(current(&p)[0], first, 60.0, 37);
        assert_eq!(p.param_smoothers.table_pending_frames, 0);
        assert_eq!(p.gains.clock_phase, 0);
        p.advance_analysis_clock(19);
        p.initialize(96_000.0).unwrap();
        let old = evolve(120.0, 180.0, 48_000.0, 56);
        check(current(&p)[0], old, 60.0, 56);
        p.advance_analysis_clock(256);
        check(current(&p)[0], evolve(old, 180.0, 96_000.0, 256), 60.0, 312);
    }

    #[test]
    fn fft_reconfiguration_commits_old_time_before_starting_the_new_analysis_grid() {
        let mut p = processor(48_000.0);
        p.set_param_value(6, 180.0);
        p.advance_analysis_clock(137);
        p.resize_fft(2);
        let old = evolve(120.0, 180.0, 48_000.0, 137);
        check(current(&p)[0], old, 60.0, 137);
        assert_eq!(p.param_smoothers.table_pending_frames, 0);
        assert_eq!(p.gains.clock_phase, 0);
        assert_eq!(p.spectral.lfe_low_gains.len(), 2);
        assert_eq!(p.height.height_freq_weights.len(), 2);
        p.advance_analysis_clock(1);
        check(current(&p)[0], evolve(old, 180.0, 48_000.0, 1), 60.0, 138);
        assert_eq!(p.param_smoothers.table_pending_frames, 0);
        assert_eq!(p.gains.clock_phase, 0);
    }

    #[test]
    fn raw_bypass_advances_table_clock_and_exit_commits_partial_time() {
        let mut p = processor(48_000.0);
        p.set_parameter(
            "bypass_all_processing".into(),
            sotf_host::ParameterValue::Bool(true),
        )
        .unwrap();
        p.set_param_value(6, 180.0);
        let input = vec![0.125; 137 * 2];
        let mut output = vec![0.0; 137 * 2];
        p.process(&input, &mut output, &ProcessContext::new(48_000, 137))
            .unwrap();
        assert_eq!(input, output);
        assert_eq!(p.param_smoothers.table_pending_frames, 137);
        p.set_parameter(
            "bypass_all_processing".into(),
            sotf_host::ParameterValue::Bool(false),
        )
        .unwrap();
        assert_eq!(p.param_smoothers.table_pending_frames, 0);
        check(
            current(&p)[0],
            evolve(120.0, 180.0, 48_000.0, 137),
            60.0,
            137,
        );
    }
}
