//! Static gate targets shared by the processor and its response plot.

// Rust guideline compliant 2026-02-21
use crate::GateMode;

/// Calculate the signed wet gain target before detector history and envelope timing.
///
/// Uses the processor's knee and range calculation. Hysteresis, hold, detector
/// filtering, dry/wet mixing and lookahead are not part of this static target.
/// Returns `None` for non-finite inputs, ratio below one or negative limits.
/// A zero downward/duck range retains the processor's finite 240-dB ceiling.
///
/// # Examples
/// ```
/// use sotf_plugin_gate::{GateMode, target_gain_db};
/// assert_eq!(target_gain_db(-30.0, -20.0, 2.0, 0.0, 40.0, 12.0, GateMode::Downward), Some(-10.0));
/// ```
#[expect(
    clippy::too_many_arguments,
    reason = "The target uses the seven canonical gate fields"
)]
pub fn target_gain_db(
    input_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    range_db: f32,
    max_boost_db: f32,
    mode: GateMode,
) -> Option<f32> {
    if ![
        input_db,
        threshold_db,
        ratio,
        knee_db,
        range_db,
        max_boost_db,
    ]
    .iter()
    .all(|value| value.is_finite())
        || ratio < 1.0
        || knee_db < 0.0
        || range_db < 0.0
        || max_boost_db < 0.0
    {
        return None;
    }
    let gain = match mode {
        GateMode::Downward => {
            -downward_attenuation_db(input_db, threshold_db, ratio, knee_db, range_db)
        }
        GateMode::Upward => {
            above_threshold_magnitude(input_db, threshold_db, ratio, knee_db, max_boost_db)
        }
        GateMode::Duck => -above_threshold_magnitude(
            input_db,
            threshold_db,
            ratio,
            knee_db,
            if range_db > 0.0 { range_db } else { 240.0 },
        ),
    };
    gain.is_finite().then_some(gain)
}

pub(crate) fn downward_attenuation_db(
    input_db: f32,
    threshold: f32,
    ratio: f32,
    knee_db: f32,
    range_db: f32,
) -> f32 {
    let knee = knee_db.max(0.0);
    // A 1 dB input decrease below threshold produces ratio dB at output.
    let slope = ratio.max(1.0) - 1.0;

    let atten = if knee < 0.1 {
        // Hard knee
        if input_db >= threshold {
            0.0
        } else {
            (threshold - input_db) * slope
        }
    } else if input_db > threshold + knee / 2.0 {
        // Above knee zone -- no attenuation
        0.0
    } else if input_db < threshold - knee / 2.0 {
        // Below knee zone -- full gate
        (threshold - input_db) * slope
    } else {
        // Within knee zone: quadratic easing from 0 dB attenuation at
        // threshold + knee/2 to the full below-threshold slope at
        // threshold - knee/2. The curve is continuous at both boundaries
        // and intentionally softer near the opening point.
        let below = threshold + knee / 2.0 - input_db;
        let kf = below / knee;
        kf * kf * (knee / 2.0) * slope
    };

    // A zero range is documented as unlimited. Keep a finite ceiling to
    // avoid inf/NaN propagation when processing denormal/invalid input.
    if range_db > 0.0 {
        atten.min(range_db)
    } else {
        atten.min(240.0)
    }
}

pub(crate) fn above_threshold_magnitude(
    level_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    limit: f32,
) -> f32 {
    let above = level_db - threshold_db;
    let hinge = if knee_db < 0.1 {
        above.max(0.0)
    } else if above <= -knee_db / 2.0 {
        0.0
    } else if above >= knee_db / 2.0 {
        above
    } else {
        let distance = above + knee_db / 2.0;
        distance * distance / (2.0 * knee_db)
    };
    (hinge * (ratio - 1.0)).min(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directions_and_caps_match_the_static_targets() {
        assert_eq!(
            target_gain_db(-30.0, -20.0, 2.0, 0.0, 5.0, 4.0, GateMode::Downward),
            Some(-5.0)
        );
        assert_eq!(
            target_gain_db(-10.0, -20.0, 2.0, 0.0, 5.0, 4.0, GateMode::Downward),
            Some(0.0)
        );
        assert_eq!(
            target_gain_db(-10.0, -20.0, 2.0, 0.0, 5.0, 4.0, GateMode::Upward),
            Some(4.0)
        );
        assert_eq!(
            target_gain_db(-10.0, -20.0, 2.0, 0.0, 5.0, 4.0, GateMode::Duck),
            Some(-5.0)
        );
        assert_eq!(
            target_gain_db(-30.0, -20.0, 2.0, 0.0, 5.0, 4.0, GateMode::Duck),
            Some(0.0)
        );
    }
    #[test]
    fn knees_are_finite_and_continuous_for_every_mode() {
        for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
            for knee in [0.0, 8.0] {
                for level in [-24.0, -20.0, -16.0] {
                    assert!(
                        target_gain_db(level, -20.0, 2.0, knee, 40.0, 12.0, mode)
                            .unwrap()
                            .is_finite()
                    );
                }
            }
            assert_eq!(
                target_gain_db(f32::NAN, -20.0, 2.0, 0.0, 40.0, 12.0, mode),
                None
            );
        }
        assert_eq!(
            target_gain_db(-20.0, -20.0, 2.0, 8.0, 40.0, 12.0, GateMode::Downward),
            Some(-1.0)
        );
        assert_eq!(
            target_gain_db(-20.0, -20.0, 2.0, 8.0, 40.0, 12.0, GateMode::Upward),
            Some(1.0)
        );
        assert_eq!(
            target_gain_db(-20.0, -20.0, 2.0, 8.0, 40.0, 12.0, GateMode::Duck),
            Some(-1.0)
        );
    }
}
