//! Static target response for Dynamic EQ editors.

// Rust guideline compliant 2026-02-21
use crate::{DynEqBandParams, DynEqShape};
use math_audio_iir_fir::{Biquad, BiquadFilterType};

/// Calculate the full-target magnitude response of one band in decibels.
///
/// Uses the DSP's coefficient designers for Peak, shelves, and Tilt. This is
/// the band's target curve, before dynamic dry/wet modulation, routing, or
/// bypass. Callers select the applicable channel and active bands separately.
/// This control-thread helper does not alter the processing path.
///
/// Returns `None` for non-finite inputs, invalid filter parameters, or a
/// frequency outside DC through Nyquist.
pub fn target_band_response_db(
    band: &DynEqBandParams,
    frequency: f64,
    sample_rate: f64,
) -> Option<f64> {
    let center = f64::from(band.frequency);
    let gain = f64::from(band.gain);
    if !sample_rate.is_finite()
        || sample_rate <= 0.0
        || !frequency.is_finite()
        || !(0.0..=sample_rate * 0.5).contains(&frequency)
        || !center.is_finite()
        || !(0.0..sample_rate * 0.5).contains(&center)
        || !gain.is_finite()
        || !(-24.0..=24.0).contains(&gain)
    {
        return None;
    }
    let coefficients = match band.shape {
        DynEqShape::Peak => {
            let q = f64::from(band.q);
            if !q.is_finite() || !(0.1..=10.0).contains(&q) {
                return None;
            }
            Biquad::<f64>::new(BiquadFilterType::Peak, center, sample_rate, q, gain).coefficients()
        }
        DynEqShape::LowShelf | DynEqShape::HighShelf => {
            super::dyn_eq_band::design_shelf_coefficients(
                band.shape,
                center,
                sample_rate,
                gain,
                f64::from(band.shelf_slope),
            )?
        }
        DynEqShape::Tilt => {
            super::dyn_eq_band::design_tilt_coefficients(center, sample_rate, gain)?
        }
    };
    let omega = std::f64::consts::TAU * frequency / sample_rate;
    let (sin, cos) = omega.sin_cos();
    let (sin2, cos2) = (2.0 * omega).sin_cos();
    let numerator = (coefficients.b0 + coefficients.b1 * cos + coefficients.b2 * cos2)
        .hypot(-coefficients.b1 * sin - coefficients.b2 * sin2);
    let denominator = (1.0 + coefficients.a1 * cos + coefficients.a2 * cos2)
        .hypot(-coefficients.a1 * sin - coefficients.a2 * sin2);
    let value = 20.0 * (numerator / denominator).log10();
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_response_has_shape_specific_endpoints_and_pivot() {
        let mut band = DynEqBandParams {
            frequency: 1000.0,
            gain: 6.0,
            ..Default::default()
        };
        let check = |band: &DynEqBandParams, frequency: f64, expected: f64| {
            let actual = target_band_response_db(band, frequency, 48_000.0).unwrap();
            assert!(
                (actual - expected).abs() < 1e-6,
                "{frequency}: {actual} != {expected}"
            );
        };
        check(&band, 1000.0, 6.0);
        band.shape = DynEqShape::LowShelf;
        check(&band, 0.0, 6.0);
        check(&band, 24_000.0, 0.0);
        band.shape = DynEqShape::HighShelf;
        check(&band, 0.0, 0.0);
        check(&band, 24_000.0, 6.0);
        band.shape = DynEqShape::Tilt;
        check(&band, 0.0, 6.0);
        check(&band, 1000.0, 0.0);
        check(&band, 24_000.0, -6.0);
    }

    #[test]
    fn target_response_rejects_invalid_inputs() {
        let band = DynEqBandParams::default();
        assert!(target_band_response_db(&band, f64::NAN, 48_000.0).is_none());
        assert!(target_band_response_db(&band, 25_000.0, 48_000.0).is_none());
        assert!(target_band_response_db(&band, 1000.0, 0.0).is_none());
        let shelf = DynEqBandParams {
            shape: DynEqShape::LowShelf,
            shelf_slope: 0.0,
            ..band
        };
        assert!(target_band_response_db(&shelf, 1000.0, 48_000.0).is_none());
    }
}
