//! Static expansion targets shared by DSP-aware visualizations.

// Rust guideline compliant 2026-02-21
use crate::MultibandExpanderPlugin;

/// Calculate static attenuation below the expansion threshold, in decibels.
///
/// Uses the same knee and range calculation as the audio processor. This is
/// the target before detector history, hysteresis, timing, mix and makeup.
/// Returns `None` for non-finite inputs, a ratio below one, or negative knee/range.
///
/// # Examples
/// ```
/// use sotf_plugin_multiband_expander::target_attenuation_db;
/// assert_eq!(target_attenuation_db(-30.0, -20.0, 2.0, 0.0, 40.0), Some(10.0));
/// ```
pub fn target_attenuation_db(
    input_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    range_db: f32,
) -> Option<f32> {
    if ![input_db, threshold_db, ratio, knee_db, range_db]
        .iter()
        .all(|v| v.is_finite())
        || ratio < 1.0
        || knee_db < 0.0
        || range_db < 0.0
    {
        return None;
    }
    let attenuation = MultibandExpanderPlugin::calculate_expansion_attenuation(
        input_db,
        threshold_db,
        ratio,
        knee_db,
        range_db,
    );
    attenuation.is_finite().then_some(attenuation)
}

#[cfg(test)]
mod tests {
    use super::target_attenuation_db;

    #[test]
    fn expands_below_threshold_with_the_processor_range_cap() {
        assert_eq!(
            target_attenuation_db(-10.0, -20.0, 4.0, 0.0, 20.0),
            Some(0.0)
        );
        assert_eq!(
            target_attenuation_db(-25.0, -20.0, 4.0, 0.0, 20.0),
            Some(15.0)
        );
        assert_eq!(
            target_attenuation_db(-40.0, -20.0, 4.0, 0.0, 20.0),
            Some(20.0)
        );
        assert_eq!(
            target_attenuation_db(-40.0, -20.0, 1.0, 0.0, 20.0),
            Some(0.0)
        );
    }

    #[test]
    fn soft_knee_matches_processor_and_invalid_values_do_not_render() {
        assert_eq!(
            target_attenuation_db(-20.0, -20.0, 2.0, 8.0, 40.0),
            Some(1.0)
        );
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(target_attenuation_db(invalid, -20.0, 2.0, 8.0, 40.0), None);
            assert_eq!(target_attenuation_db(-30.0, invalid, 2.0, 8.0, 40.0), None);
            assert_eq!(
                target_attenuation_db(-30.0, -20.0, invalid, 8.0, 40.0),
                None
            );
        }
        assert_eq!(target_attenuation_db(-30.0, -20.0, 0.5, 8.0, 40.0), None);
        assert_eq!(target_attenuation_db(-30.0, -20.0, 2.0, -1.0, 40.0), None);
        assert_eq!(target_attenuation_db(-30.0, -20.0, 2.0, 8.0, -1.0), None);
    }
}
