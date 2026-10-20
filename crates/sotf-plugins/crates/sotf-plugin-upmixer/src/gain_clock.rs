//! Fixed analysis-clock gain trajectories independent of host callback sizes.

// Rust guideline compliant 2026-02-21
use crate::UpmixerPlugin;

impl UpmixerPlugin {
    pub(super) fn is_gain_clock_parameter(index: usize) -> bool {
        matches!(index, 1..=5 | 8 | 12..=13 | 16..=17 | 23..=28)
    }

    pub(super) fn advance_analysis_clock(&mut self, mut frames: usize) {
        // Both FFT hops are powers of two. The smaller hop is their common
        // analysis quantum, even while HR is disabled. A target event may
        // split a quantum but never moves this grid.
        let quantum = self.core.hop_size.min(self.fft.hr_fft_size / 2);
        while frames > 0 {
            let step = frames.min(quantum - self.gains.clock_phase);
            self.gains.pending_frames += step;
            self.param_smoothers.table_pending_frames += step;
            self.gains.clock_phase += step;
            frames -= step;
            if self.gains.clock_phase == quantum {
                self.flush_gain_clock();
                self.flush_table_clock();
                self.gains.clock_phase = 0;
            }
        }
    }

    pub(super) fn flush_gain_clock(&mut self) {
        let n = std::mem::take(&mut self.gains.pending_frames);
        // next_n(0) is not an elapsed-time operation for every smoother;
        // empty callbacks and same-value snapshots must not snap controls.
        if n == 0 {
            return;
        }
        self.gains.gain_front_direct.next_n(n);
        self.gains.gain_front_ambient.next_n(n);
        self.gains.gain_rear_ambient.next_n(n);
        self.height.height_gain.next_n(n);
        self.gains.lfe_gain.next_n(n);
        self.subharmonic.subharmonic_gain.next_n(n);
        self.gains.stereo_width.next_n(n);
        self.gains.center_spread.next_n(n);
        self.surround.ambient_boost.next_n(n);
        self.dialogue.dialogue_weight.next_n(n);
        self.surround.surround_direct_bleed.next_n(n);
        self.surround.rear_ambient_boost.next_n(n);
        self.surround.rear_late_reflection.next_n(n);
        self.height.height_direct_leak.next_n(n);
        self.height.height_transient_reduction.next_n(n);
        self.gains.hr_sharpen.next_n(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::smoothing::Smoother;
    use sotf_host::{Plugin, ProcessContext};

    fn processor(rate: f64) -> UpmixerPlugin {
        let params = serde_json::from_value(serde_json::json!({"fft_size":2048,
            "speaker_config":"2.0","enable_hr_direct":false,"auto_gain_enabled":false}))
        .unwrap();
        let mut p = UpmixerPlugin::from_params(params);
        p.initialize(rate).unwrap();
        p.gains.gain_front_direct.reset(1.0);
        p
    }

    fn coefficient(rate: f64) -> f64 {
        // Independent f64 closed form with the stored f32 one-pole coefficient.
        f64::from((-1.0 / (rate * 0.05)).exp() as f32)
    }

    fn smoothers(p: &mut UpmixerPlugin) -> [&mut Smoother; 16] {
        [
            &mut p.gains.gain_front_direct,
            &mut p.gains.gain_front_ambient,
            &mut p.gains.gain_rear_ambient,
            &mut p.height.height_gain,
            &mut p.gains.lfe_gain,
            &mut p.subharmonic.subharmonic_gain,
            &mut p.gains.stereo_width,
            &mut p.gains.center_spread,
            &mut p.gains.hr_sharpen,
            &mut p.surround.ambient_boost,
            &mut p.height.height_transient_reduction,
            &mut p.height.height_direct_leak,
            &mut p.surround.surround_direct_bleed,
            &mut p.surround.rear_ambient_boost,
            &mut p.surround.rear_late_reflection,
            &mut p.dialogue.dialogue_weight,
        ]
    }

    #[test]
    fn every_clocked_control_advances_its_declared_one_pole() {
        let indices = [1, 2, 3, 4, 5, 8, 12, 13, 16, 17, 23, 24, 25, 26, 27, 28];
        for rate in [44_100.0, 48_000.0, 192_000.0] {
            let mut p = processor(rate);
            let initial = smoothers(&mut p).map(|s| {
                let value = s.target();
                s.reset(value);
                value
            });
            let target = std::array::from_fn::<_, 16, _>(|i| {
                let high = crate::params::PARAMS[indices[i]].clamp_f64(1000.0) as f32;
                if initial[i] != high {
                    high
                } else {
                    crate::params::PARAMS[indices[i]].clamp_f64(-1000.0) as f32
                }
            });
            for (index, target) in indices.into_iter().zip(target) {
                p.set_param_value(index, f64::from(target));
            }
            p.advance_analysis_clock(256);
            for (i, s) in smoothers(&mut p).into_iter().enumerate() {
                let expected = f64::from(target[i])
                    + coefficient(rate).powi(256) * f64::from(initial[i] - target[i]);
                // gamma_n for the existing f32 power/multiply/add arithmetic.
                let roundoff = 260.0 * f64::from(f32::EPSILON);
                let bound =
                    roundoff / (1.0 - roundoff) * (1.0 + f64::from((initial[i] - target[i]).abs()));
                assert!(
                    (f64::from(s.current()) - expected).abs() < bound,
                    "index={} clock did not advance",
                    indices[i]
                );
                assert_ne!(
                    s.current(),
                    initial[i],
                    "index={} was not exercised",
                    indices[i]
                );
            }
        }
    }

    #[test]
    fn interrupted_targets_account_for_elapsed_subhop_time_without_moving_grid() {
        for rate in [44_100.0, 48_000.0, 192_000.0] {
            let mut p = processor(rate);
            p.set_param_value(1, 0.0);
            p.advance_analysis_clock(37);
            assert_eq!(p.gains.gain_front_direct.current(), 1.0);
            p.process(&[], &mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (37, 37));
            assert!(
                p.process(&[], &mut [], &ProcessContext::new(rate, 1))
                    .is_err()
            );
            assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (37, 37));
            p.set_param_value(1, 0.5);
            let first = coefficient(rate).powi(37);
            assert!((f64::from(p.gains.gain_front_direct.current()) - first).abs() < 2e-6);
            assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (37, 0));
            p.advance_analysis_clock(19);
            p.set_param_value(1, 0.5); // Same target is not an observation boundary.
            assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (56, 19));
            p.advance_analysis_clock(200);
            let expected = 0.5 + coefficient(rate).powi(219) * (first - 0.5);
            assert!((f64::from(p.gains.gain_front_direct.current()) - expected).abs() < 2e-6);
            assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (0, 0));
        }
    }

    #[test]
    fn reset_and_new_clock_commit_elapsed_frames_with_the_old_coefficient() {
        let mut p = processor(48_000.0);
        p.set_param_value(1, 0.0);
        p.advance_analysis_clock(37);
        assert!(p.initialize(f64::NAN).is_err());
        assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (37, 37));
        p.reset();
        let first = coefficient(48_000.0).powi(37);
        assert!((f64::from(p.gains.gain_front_direct.current()) - first).abs() < 2e-6);
        assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (0, 0));
        p.advance_analysis_clock(19);
        p.initialize(96_000.0).unwrap();
        let before_new = coefficient(48_000.0).powi(56);
        assert!((f64::from(p.gains.gain_front_direct.current()) - before_new).abs() < 2e-6);
        p.advance_analysis_clock(256);
        let expected = before_new * coefficient(96_000.0).powi(256);
        // The retained f32 powi helper uses rounded repeated squaring. Bound
        // its accumulated arithmetic error by gamma_n instead of treating
        // the f64 curve as an exact f32 numerical oracle. The old-clock
        // flush above remains independently checked within 2e-6.
        let rounding = 312.0 * f64::from(f32::EPSILON);
        let arithmetic_bound = rounding / (1.0 - rounding);
        assert!(
            (f64::from(p.gains.gain_front_direct.current()) - expected).abs() < arithmetic_bound,
            "current={} target={} expected={expected} phase={} pending={} main_hop={} hr_fft={}",
            p.gains.gain_front_direct.current(),
            p.gains.gain_front_direct.target(),
            p.gains.clock_phase,
            p.gains.pending_frames,
            p.core.hop_size,
            p.fft.hr_fft_size
        );
    }

    #[test]
    fn raw_bypass_advances_controls_and_leaving_bypass_commits_partial_time() {
        let mut p = processor(48_000.0);
        p.set_parameter(
            "bypass_all_processing".into(),
            sotf_host::ParameterValue::Bool(true),
        )
        .unwrap();
        p.set_param_value(1, 0.0);
        let input = vec![0.125; 137 * 2];
        let mut output = vec![0.0; 137 * 2];
        p.process(&input, &mut output, &ProcessContext::new(48_000, 137))
            .unwrap();
        assert_eq!(output, input);
        assert_eq!(p.latency_samples(), 0);
        assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (137, 137));
        p.set_parameter(
            "bypass_all_processing".into(),
            sotf_host::ParameterValue::Bool(false),
        )
        .unwrap();
        assert_eq!(p.latency_samples(), 2048);
        assert_eq!((p.gains.clock_phase, p.gains.pending_frames), (0, 0));
        assert!(
            (f64::from(p.gains.gain_front_direct.current()) - coefficient(48_000.0).powi(137))
                .abs()
                < 2e-6
        );
    }
}
