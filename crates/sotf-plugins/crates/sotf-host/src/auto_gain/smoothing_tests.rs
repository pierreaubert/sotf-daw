// Rust guideline compliant 2026-02-21
use super::{AutoGain, AutoGainParams};

fn gain(channels: usize, rate: u32, milliseconds: f32) -> AutoGain {
    AutoGain::new(
        channels,
        rate,
        AutoGainParams {
            enabled: true,
            max_gain_db: 12.0,
            smoothing_ms: milliseconds,
            ..Default::default()
        },
    )
    .unwrap()
}

fn same_state(actual: &AutoGain, expected: &AutoGain) {
    assert_eq!(actual.gain_db, expected.gain_db);
    assert_eq!(actual.target_gain_db, expected.target_gain_db);
    assert_eq!(actual.current_gain_linear, expected.current_gain_linear);
    assert_eq!(actual.current_gain_db(), expected.current_gain_db());
}

#[test]
fn public_measured_smoothing_controls_applied_audio() {
    let rate = 48_000;
    let input: Vec<_> = (0..rate)
        .flat_map(|frame| {
            let sample = (std::f64::consts::TAU * 997.0 * f64::from(frame) / f64::from(rate)).sin()
                as f32
                * 0.1;
            [sample, sample]
        })
        .collect();
    for ratio in [0.5, 2.0] {
        let uncompensated: Vec<_> = input.iter().map(|x| x * ratio).collect();
        let mut fast = gain(2, rate, 25.0);
        let mut slow = gain(2, rate, 1000.0);
        for meter in [&mut fast, &mut slow] {
            meter.measure_input(&input).unwrap();
            meter.measure_output(&uncompensated).unwrap();
        }
        let mut a = vec![1.0; rate as usize * 2];
        let mut b = a.clone();
        fast.apply_compensation(&mut a, rate as usize);
        slow.apply_compensation(&mut b, rate as usize);
        let difference = a
            .iter()
            .zip(&b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            difference > 0.1,
            "ratio={ratio}: smoothing omitted, difference={difference}"
        );
        let midpoint = rate as usize;
        if ratio < 1.0 {
            assert!(a[midpoint] > b[midpoint]);
        } else {
            assert!(a[midpoint] < b[midpoint]);
        }
    }
}

#[test]
fn scalar_block_and_discarded_steps_share_absolute_control_timeline() {
    let events = [
        (0, 6.0),
        (137, -9.0),
        (8193, 4.5),
        (20000, -0.000004),
        (33000, 0.0),
    ];
    for rate in [48_000, 192_000] {
        for channels in [1, 2, 8] {
            for pattern in [&[1][..], &[17, 137][..], &[8193][..]] {
                let mut scalar = gain(channels, rate, 37.0);
                let mut block = gain(channels, rate, 37.0);
                let mut discarded = gain(channels, rate, 37.0);
                let mut position = 0;
                let mut iteration = 0;
                let mut event = 0;
                while position < 40_017 {
                    if event < events.len() && position == events[event].0 {
                        for meter in [&mut scalar, &mut block, &mut discarded] {
                            meter.set_gain_target(events[event].1);
                            if position == 8193 {
                                meter.set_smoothing_ms(250.0);
                            }
                        }
                        event += 1;
                    }
                    let next = events.get(event).map_or(40_017, |e| e.0);
                    let frames = pattern[iteration % pattern.len()].min(next - position);
                    let mut expected: Vec<_> = (0..frames * channels)
                        .map(|n| 0.2 + (n % 13) as f32 / 20.0)
                        .collect();
                    let mut actual = expected.clone();
                    for frame in expected.chunks_mut(channels) {
                        let value = scalar.next_gain_linear();
                        for sample in frame {
                            *sample *= value;
                        }
                    }
                    block.apply_compensation(&mut actual, frames);
                    discarded.next_n(frames);
                    assert_eq!(
                        actual, expected,
                        "rate={rate} channels={channels} position={position}"
                    );
                    same_state(&block, &scalar);
                    same_state(&discarded, &scalar);
                    position += frames;
                    iteration += 1;
                }
            }
        }
    }
}

#[test]
fn zero_frames_preserve_gain_state_and_surplus_samples() {
    let mut meter = gain(2, 48_000, 100.0);
    meter.gain_db = 1.0;
    meter.target_gain_db = 1.0;
    meter.set_gain_target(6.0);
    meter.current_gain_linear = 0.75;
    meter.next_n(0);
    let mut output = [3.0, 4.0, 5.0];
    meter.apply_compensation(&mut output, 0);
    assert_eq!(output, [3.0, 4.0, 5.0]);
    assert_eq!(meter.gain_db, 1.0);
    assert_eq!(meter.current_gain_linear, 0.75);
}

#[test]
fn stationary_scaling_changes_only_the_requested_audio_prefix() {
    let mut meter = gain(2, 48_000, 100.0);
    meter.gain_db = 6.0;
    meter.target_gain_db = 6.0;
    meter.current_gain_linear = (6.0 * (std::f64::consts::LN_10 / 20.0)).exp();
    let value = meter.current_gain_linear as f32;
    let mut output = [0.25, -0.75, 912.5, -311.25];
    meter.apply_compensation(&mut output, 1);
    assert_eq!(output, [0.25 * value, -0.75 * value, 912.5, -311.25]);
}

#[test]
fn rounded_fixed_points_and_near_target_steps_match_scalar() {
    for target in [-6.0, 0.0, 6.0] {
        let mut scalar = gain(2, 48_000, 100.0);
        let mut block = gain(2, 48_000, 100.0);
        scalar.set_gain_target(target);
        block.set_gain_target(target);
        for _ in 0..48_000 * 30 {
            scalar.next_gain_linear();
            block.next_gain_linear();
        }
        let old_db = scalar.gain_db;
        let old_gain = scalar.current_gain_linear;
        let mut actual = vec![0.25; 2048];
        let mut expected = actual.clone();
        for frame in expected.chunks_mut(2) {
            let g = scalar.next_gain_linear();
            frame[0] *= g;
            frame[1] *= g;
        }
        block.apply_compensation(&mut actual, 1024);
        assert_eq!(actual, expected);
        same_state(&block, &scalar);
        assert_eq!(
            (old_db, old_gain),
            (scalar.gain_db, scalar.current_gain_linear)
        );
    }
    // The two states must both be stationary before a gain can be reused.
    let mut scalar = gain(1, 48_000, 100.0);
    let mut block = gain(1, 48_000, 100.0);
    for meter in [&mut scalar, &mut block] {
        meter.gain_db = 5.0;
        meter.target_gain_db = 5.0;
        meter.set_gain_target(6.0);
        meter.current_gain_linear = (6.0 * (std::f64::consts::LN_10 / 20.0)).exp() - 0.000001;
    }
    let mut actual = [1.0; 137];
    block.apply_compensation(&mut actual, 137);
    let expected = std::array::from_fn::<_, 137, _>(|_| scalar.next_gain_linear());
    assert_eq!(actual, expected);
    same_state(&block, &scalar);
}

#[test]
fn disabled_setter_reset_and_rate_changes_preserve_scalar_semantics() {
    for milliseconds in [0.0, 25.0] {
        let mut meter = gain(1, 48_000, milliseconds);
        meter.set_gain_target(6.0);
        for _ in 0..137 {
            meter.next_gain_linear();
        }
        let db_before = meter.gain_db;
        let linear = meter.current_gain_linear;
        meter.set_enabled(false);
        let db = if milliseconds == 0.0 { 0.0 } else { db_before };
        let mut output = [0.5; 17];
        meter.apply_compensation(&mut output, 17);
        meter.next_n(137);
        assert_eq!(meter.next_gain_linear(), 1.0);
        assert_eq!(output, [0.5; 17]);
        assert_eq!(meter.gain_db, db);
        assert_eq!(meter.current_gain_linear, linear);
        meter.set_enabled(true);
        meter.reset();
        assert_eq!(meter.next_gain_linear(), 1.0);
        assert!(meter.is_unity_gain_stable());
        meter.set_sample_rate(96_000).unwrap();
        meter.set_gain_target(-6.0);
        assert!(meter.next_gain_linear() < 1.0);
    }
}

#[test]
fn unity_shortcut_requires_exact_gain_and_db_states() {
    let mut meter = gain(1, 48_000, 100.0);
    assert!(meter.is_unity_gain_stable());
    meter.current_gain_linear = 1.0 + 0.000001;
    assert!(!meter.is_unity_gain_stable());
    meter.current_gain_linear = 1.0;
    meter.set_gain_target(0.000001);
    assert!(!meter.is_unity_gain_stable());
    meter.set_enabled(false);
    assert!(meter.is_unity_gain_stable());
}

#[test]
fn applied_gain_tracks_independent_f64_cascade() {
    let mut worst = 0.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for milliseconds in [-1.0, 0.0, 25.0, 100.0, 1000.0, 5000.0] {
            for target in [-6.0_f32, 6.0] {
                let mut meter = gain(1, rate, milliseconds);
                meter.set_gain_target(target);
                let a = if milliseconds <= 0.0 {
                    0.0
                } else {
                    (-1.0 / (f64::from(milliseconds) * 0.001 * f64::from(rate))).exp()
                };
                let mut linear = 1.0_f64;
                let mut max_error = 0.0_f64;
                for n in 1..=rate as usize {
                    let mut actual = [1.0];
                    meter.apply_compensation(&mut actual, 1);
                    // Closed-form first stage and accurate base-ten conversion
                    // are independent of the production recursive/exp path.
                    let db = if a == 0.0 || f64::from(target).abs() * a.powi(n as i32 - 1) < 1e-5 {
                        f64::from(target)
                    } else {
                        f64::from(target) * (1.0 - a.powi(n as i32))
                    };
                    let wanted = 10.0_f64.powf(db / 20.0);
                    let time = if wanted < linear { 0.020 } else { 0.300 };
                    let coefficient = (-1.0 / (time * f64::from(rate))).exp();
                    linear = wanted + coefficient * (linear - wanted);
                    max_error = max_error.max((f64::from(actual[0]) - linear).abs() / linear);
                }
                worst = worst.max(max_error);
                // One f32 output rounding, with headroom for the independent
                // closed-form and recursive f64 evaluation orders.
                assert!(
                    max_error < 8e-8,
                    "rate={rate} smoothing={milliseconds} target={target} relative error={max_error:e}"
                );
            }
        }
    }
    eprintln!("AutoGain f64 cascade maximum relative error={worst:e}");
}

#[test]
fn reversal_coefficient_follows_intermediate_gain_not_final_target() {
    let rate = 48_000;
    let mut meter = gain(1, rate, 1000.0);
    let a = (-1.0 / (1.0 * f64::from(rate))).exp();
    let mut db = 0.0_f64;
    let mut linear = 1.0_f64;
    let mut max_relative = 0.0_f64;
    let mut releases_after_reversal = 0;
    for (segment, target) in [6.0_f32, -6.0, 0.0].into_iter().enumerate() {
        meter.set_gain_target(target);
        let start_db = db;
        for frame in 1..=12_000 {
            db = f64::from(target) + (start_db - f64::from(target)) * a.powi(frame);
            let intermediate = 10.0_f64.powf(db / 20.0);
            let release = intermediate >= linear;
            if segment == 1 && release {
                releases_after_reversal += 1;
            }
            let tau = if release { 0.300 } else { 0.020 };
            let coefficient = (-1.0 / (tau * f64::from(rate))).exp();
            linear = intermediate + coefficient * (linear - intermediate);
            let mut actual = [1.0];
            meter.apply_compensation(&mut actual, 1);
            max_relative = max_relative.max((f64::from(actual[0]) - linear).abs() / linear);
        }
    }
    assert!(
        releases_after_reversal > 100,
        "the oracle must distinguish the branch choice"
    );
    assert!(
        max_relative < 8e-8,
        "reversal relative error={max_relative:e}"
    );
    eprintln!(
        "AutoGain reversal relative error={max_relative:e}, release steps after negative target={releases_after_reversal}"
    );
}

#[test]
fn settled_gain_has_no_sample_rate_dependent_precision_floor() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for target in [-6.0, 6.0, (20.0_f64 * 2.0_f64.log10()) as f32] {
            let mut meter = gain(1, rate, 100.0);
            meter.set_gain_target(target);
            for seconds in [4, 30] {
                let frames = if seconds == 4 { 4 } else { 26 } * rate as usize;
                meter.next_n(frames);
                let actual = f64::from(meter.current_gain_db());
                assert!(
                    (actual - f64::from(target)).abs() < 0.0001,
                    "rate={rate} target={target} seconds={seconds}: actual={actual}"
                );
            }
        }
    }
}

#[test]
fn long_high_rate_transition_tracks_closed_form_db_and_independent_linear_pole() {
    let rate = 192_000;
    let mut meter = gain(1, rate, 5000.0);
    let target = 6.0_f64;
    meter.set_gain_target(target as f32);
    let a = (-1.0 / (5.0 * f64::from(rate))).exp();
    let b = (-1.0 / (0.3 * f64::from(rate))).exp();
    let mut reference = 1.0_f64;
    let mut worst = 0.0_f64;
    for frame in 1..=rate as usize * 20 {
        let db = target * (1.0 - a.powi(frame as i32));
        let linear = 10.0_f64.powf(db / 20.0);
        reference = linear + b * (reference - linear);
        let actual = f64::from(meter.next_gain_linear());
        worst = worst.max((actual - reference).abs() / reference);
    }
    assert!(worst < 1.0e-7, "long transition relative error={worst:e}");
    eprintln!("AutoGain 192 kHz / 5000 ms / 20 s maximum relative error={worst:e}");
}

#[test]
fn rounded_output_equality_does_not_end_internal_transition() {
    let mut scalar = gain(1, 192_000, 5000.0);
    let mut discarded = gain(1, 192_000, 5000.0);
    let mut block = gain(1, 192_000, 5000.0);
    for meter in [&mut scalar, &mut discarded, &mut block] {
        meter.set_gain_target(6.0);
    }
    let first = scalar.next_gain_linear();
    let db = scalar.gain_db;
    let linear = scalar.current_gain_linear;
    let second = scalar.next_gain_linear();
    assert_eq!(first, second, "fixture must exercise identical f32 outputs");
    assert_ne!(scalar.gain_db, db);
    assert_ne!(scalar.current_gain_linear, linear);
    let mut expected = vec![first, second];
    expected.extend((2..8193).map(|_| scalar.next_gain_linear()));
    let mut actual = vec![1.0; 8193];
    block.apply_compensation(&mut actual, 8193);
    discarded.next_n(8193);
    assert_eq!(actual, expected);
    assert!(actual.last().unwrap() > &first);
    same_state(&discarded, &scalar);
    same_state(&block, &scalar);
}

#[test]
fn coefficient_updates_and_strict_db_snap_preserve_lifecycle() {
    for milliseconds in [-1.0, 0.0, 25.0, 5000.0] {
        let mut meter = gain(1, 192_000, milliseconds);
        meter.set_gain_target(6.0);
        meter.next_n(137);
        let state = (
            meter.gain_db,
            meter.target_gain_db,
            meter.current_gain_linear,
        );
        meter.set_smoothing_ms(1000.0);
        assert_eq!(
            state,
            (
                meter.gain_db,
                meter.target_gain_db,
                meter.current_gain_linear
            )
        );
        assert_eq!(meter.smoothing_coeff, (-1.0_f64 / 192_000.0).exp());
        meter.set_sample_rate(48_000).unwrap();
        assert_eq!(
            state,
            (
                meter.gain_db,
                meter.target_gain_db,
                meter.current_gain_linear
            )
        );
        assert_eq!(meter.smoothing_coeff, (-1.0_f64 / 48_000.0).exp());
        assert_eq!(meter.attack_coeff, (-1.0_f64 / (0.020 * 48_000.0)).exp());
        assert_eq!(meter.release_coeff, (-1.0_f64 / (0.300 * 48_000.0)).exp());
    }
    for difference in [0.999e-5, 1e-5, 1.001e-5] {
        let mut meter = gain(1, 48_000, 100.0);
        meter.gain_db = difference;
        meter.set_gain_target(0.0);
        meter.next_gain_linear();
        if difference < 1e-5 {
            assert_eq!(meter.gain_db, 0.0);
        } else {
            assert!(
                meter.gain_db > 0.0,
                "strict snap boundary must not be broadened"
            );
        }
    }
}
