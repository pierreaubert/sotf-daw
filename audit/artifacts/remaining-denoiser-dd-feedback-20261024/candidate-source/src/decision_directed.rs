//! Keep clean-power prediction independent of user-controlled suppression gains.

// Rust guideline compliant 2026-02-21
const EPSILON: f32 = 1e-10;

// The estimator uses H0 = xi/(1+xi), not the delivered xi/(xi+reduction)
// gain or its later floor/curve/smoothing. See the baseline equations in
// https://www.isca-archive.org/interspeech_2008/alam08_interspeech.html.
pub(super) fn estimate_clean_power(
    observed_power: f32,
    noise_power: f32,
    previous_clean_power: f32,
    alpha: f32,
    enabled: bool,
) -> (f32, f32) {
    let noise = noise_power.max(EPSILON);
    let instantaneous = (observed_power / noise - 1.0).max(0.0);
    let prior = if enabled {
        alpha * previous_clean_power / noise + (1.0 - alpha) * instantaneous
    } else {
        instantaneous
    };
    let estimator_gain = prior / (1.0 + prior);
    let clean_power = estimator_gain * estimator_gain * observed_power;
    (prior, clean_power)
}

#[cfg(test)]
mod tests {
    use super::estimate_clean_power;
    use crate::{DenoiserPlugin, DenoiserPluginParams};
    use rustfft::num_complex::Complex;
    use sotf_host::ParametricInPlacePlugin;

    #[test]
    fn clean_power_recurrence_matches_independent_f64_model() {
        let mut cases = 0;
        let mut maximum_error = 0.0_f64;
        for alpha in [0.5_f32, 0.8, 0.98, 0.999] {
            for enabled in [false, true] {
                for scale in [0.001_f32, 1.0, 1000.0] {
                    let mut actual = 0.0_f32;
                    let mut reference = 0.0_f64;
                    for (power, noise) in [
                        (0.0_f32, 1.0_f32),
                        (1.0, 1.0),
                        (20.0, 1.0),
                        (40.0, 1.0),
                        (80.0, 4.0),
                        (60.0, 4.0),
                        (20.0, 4.0),
                        (4.0, 4.0),
                        (1.0, 4.0),
                        (0.0, 1.0),
                    ] {
                        let power = power * scale;
                        let noise = noise * scale;
                        let observed = f64::from(power);
                        let variance = f64::from(noise).max(f64::from(1e-10_f32));
                        let instantaneous = (observed / variance - 1.0).max(0.0);
                        let expected_prior = if enabled {
                            f64::from(alpha) * reference / variance
                                + (1.0 - f64::from(alpha)) * instantaneous
                        } else {
                            instantaneous
                        };
                        let expected_gain = expected_prior / (1.0 + expected_prior);
                        reference = expected_gain.powi(2) * observed;
                        let (prior, next) =
                            estimate_clean_power(power, noise, actual, alpha, enabled);
                        let error = (f64::from(prior) - expected_prior).abs()
                            / (1.0 + expected_prior.abs());
                        let power_error =
                            (f64::from(next) - reference).abs() / (1.0 + reference.abs());
                        maximum_error = maximum_error.max(error).max(power_error);
                        assert!(error < 3e-6 && power_error < 3e-6);
                        actual = next;
                        cases += 1;
                    }
                }
            }
        }
        println!("clean_power_reference cases={cases} maximum_scaled_error={maximum_error:e}");
        assert_eq!(cases, 240);
    }

    #[test]
    fn application_controls_cannot_change_the_clean_power_estimator() {
        let powers = [1.0_f32, 1.0, 20.0, 40.0, 80.0, 60.0, 20.0, 4.0, 1.0];
        let noises = [1.0_f32, 1.0, 1.0, 1.0, 4.0, 4.0, 4.0, 4.0, 4.0];
        let mut reference = Vec::new();
        let mut configurations = 0;
        for (reduction, floor, transparency, smooth, curve) in [
            (0.0, -20.0, 0.0, false, 1.0),
            (3.0, -20.0, 0.0, false, 1.0),
            (10.0, -40.0, 0.3, true, 0.5),
            (20.0, -60.0, 1.0, true, 0.8),
            (40.0, -10.0, 0.7, false, 0.2),
        ] {
            let mut plugin = DenoiserPlugin::from_params(
                2,
                DenoiserPluginParams {
                    reduction_db: reduction,
                    floor_db: floor,
                    transparency,
                    temporal_smoothing_enabled: smooth,
                    spectral_smoothing_enabled: smooth,
                    curve_low: curve,
                    curve_mid: 1.0,
                    curve_high: curve,
                    polyphonic_detection: false,
                    spatial_denoise: false,
                    psychoacoustic_masking: false,
                    low_latency: true,
                    ..Default::default()
                },
            );
            plugin.initialize(48_000.0).unwrap();
            for (frame, (&power, &noise)) in powers.iter().zip(&noises).enumerate() {
                for channel in 0..2 {
                    plugin.mcra.noise_psd[channel].fill(noise);
                    plugin.fft.freq_domain[channel].fill(Complex::new(power.sqrt(), 0.0));
                }
                plugin.calculate_wiener_gains();
                if configurations == 0 {
                    reference.push(plugin.decision_directed.prev_clean_power.clone());
                } else {
                    assert_eq!(plugin.decision_directed.prev_clean_power, reference[frame]);
                }
            }
            plugin.reset();
            assert!(
                plugin
                    .decision_directed
                    .prev_clean_power
                    .iter()
                    .flatten()
                    .all(|x| *x == 0.0)
            );
            configurations += 1;
        }
        println!(
            "clean_power_control_invariance configurations={configurations} frames=9 exact=true reset=true"
        );
        assert_eq!(configurations, 5);
    }
}
