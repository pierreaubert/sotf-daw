use crate::PluginViewTheme;
use gpui_audio_kit::VerticalSliderTheme;

/// Format a label with keyboard shortcut indicator
/// e.g., "Threshold" with key 't' -> "\[T\]hreshold"
pub fn format_shortcut_label(label: &str, shortcut_key: Option<char>) -> String {
    match shortcut_key {
        Some(key) => {
            let key_lower = key.to_lowercase().to_string();
            if let Some((pos, label_char)) = label
                .char_indices()
                .find(|(_, label_char)| label_char.to_lowercase().to_string() == key_lower)
            {
                let end = pos + label_char.len_utf8();
                let highlighted_char = label_char.to_uppercase().to_string();
                format!("{}[{}]{}", &label[..pos], highlighted_char, &label[end..])
            } else {
                format!("[{}] {}", key.to_ascii_uppercase(), label)
            }
        }
        None => label.to_string(),
    }
}

pub(super) fn curve_endpoints(curve_points: &[(f32, f32)]) -> Option<((f32, f32), (f32, f32))> {
    Some((*curve_points.first()?, *curve_points.last()?))
}

/// Convert PluginViewTheme to VerticalSliderTheme for gpui-audio-kit VerticalSlider
pub(super) fn theme_to_vertical_slider_theme(theme: &PluginViewTheme) -> VerticalSliderTheme {
    VerticalSliderTheme {
        surface: theme.surface,
        surface_hover: theme.surface_hover,
        track_bg: theme.background,
        accent: theme.accent,
        accent_muted: theme.accent_muted,
        border: theme.border,
        text_secondary: theme.text_secondary,
        text_primary: theme.text_primary,
        text_muted: theme.text_muted,
        text_on_accent: theme.text_on_accent,
        background_secondary: theme.background_secondary,
        peak_marker: theme.meter_colors.peak,
    }
}

/// Compute the output dB for a given input dB on the transfer curve
pub(super) fn compute_transfer(
    input_db: f64,
    threshold_db: f64,
    ratio: f64,
    knee_db: f64,
    is_limiter: bool,
) -> f64 {
    if is_limiter && knee_db <= 0.0 {
        input_db.min(threshold_db)
    } else if knee_db <= 0.0 {
        // A hard knee has no interpolation interval. At the threshold, the
        // soft-knee expression would divide zero by zero and poison the path.
        if input_db <= threshold_db {
            input_db
        } else {
            threshold_db + (input_db - threshold_db) / ratio
        }
    } else if input_db < threshold_db - knee_db / 2.0 {
        input_db
    } else if input_db > threshold_db + knee_db / 2.0 {
        threshold_db + (input_db - threshold_db) / ratio
    } else {
        let knee_input = input_db - (threshold_db - knee_db / 2.0);
        let knee_ratio = knee_input / knee_db;
        input_db + (knee_ratio * knee_ratio / 2.0) * (1.0 / ratio - 1.0) * knee_db
    }
}

#[cfg(test)]
mod tests {

    use super::super::{curve_endpoints, format_shortcut_label};
    use super::compute_transfer;

    #[test]
    fn format_shortcut_label_highlights_after_utf8_prefix() {
        assert_eq!(format_shortcut_label("Étage", Some('t')), "É[T]age");
    }

    #[test]
    fn format_shortcut_label_highlights_unicode_key() {
        assert_eq!(format_shortcut_label("éclair", Some('é')), "[É]clair");
    }

    #[test]
    fn hard_knee_threshold_and_sampled_curve_are_finite() {
        // -45 dB is one of the renderer's 64 evenly spaced input samples.
        for threshold in [-60.0, -45.0, -30.0, 0.0] {
            assert_eq!(
                compute_transfer(threshold, threshold, 10.0, 0.0, false),
                threshold
            );
            for sample in 0..=64 {
                let input = -60.0 + sample as f64 * 60.0 / 64.0;
                let expected = if input <= threshold {
                    input
                } else {
                    threshold + (input - threshold) / 10.0
                };
                let output = compute_transfer(input, threshold, 10.0, 0.0, false);
                assert!(output.is_finite());
                assert!((output - expected).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn soft_knee_meets_the_hard_regions_continuously() {
        assert!((compute_transfer(-23.0, -20.0, 4.0, 6.0, false) + 23.0).abs() < 1e-12);
        assert!((compute_transfer(-17.0, -20.0, 4.0, 6.0, false) + 19.25).abs() < 1e-12);
        assert_eq!(
            compute_transfer(-20.0, -20.0, f64::INFINITY, 0.0, true),
            -20.0
        );
    }

    #[test]
    fn limiter_soft_knee_matches_the_one_db_gain_computer() {
        for (input, output) in [
            (-20.5, -20.5),
            (-20.0, -20.125),
            (-19.5, -20.0),
            (-10.0, -20.0),
        ] {
            assert!(
                (compute_transfer(input, -20.0, f64::INFINITY, 1.0, true) - output).abs() < 1e-12
            );
        }
    }

    #[test]
    fn curve_endpoints_returns_none_for_empty_points() {
        assert_eq!(curve_endpoints(&[]), None);
    }

    #[test]
    fn curve_endpoints_returns_first_and_last_points() {
        assert_eq!(
            curve_endpoints(&[(1.0, 2.0), (3.0, 4.0)]),
            Some(((1.0, 2.0), (3.0, 4.0)))
        );
    }
}
