//! Preserve the public zero-attenuation endpoint after every optional gain path.

// Rust guideline compliant 2026-02-21
use crate::DenoiserPlugin;
use math_audio_dsp::fast_math::fast_log10;

// Finish a live endpoint transition exactly once its residual gain effect is
// below one part per million. Fresh/reset streams snap before their first FFT.
const ENDPOINT_SNAP: f32 = 1e-6;

impl DenoiserPlugin {
    pub(super) fn apply_zero_reduction_endpoint(&mut self) {
        let target = if self.params.reduction_db == 0.0 {
            1.0
        } else {
            0.0
        };
        if !self.gains.zero_mix_initialized {
            self.gains.zero_mix = target;
            self.gains.zero_mix_initialized = true;
        } else if self.gains.zero_mix != target {
            let coefficient = if target > self.gains.zero_mix {
                self.coeffs.attack_coeff
            } else {
                self.coeffs.release_coeff
            };
            self.gains.zero_mix = target + coefficient * (self.gains.zero_mix - target);
            if (self.gains.zero_mix - target).abs() < ENDPOINT_SNAP {
                self.gains.zero_mix = target;
            }
        }
        let mix = self.gains.zero_mix;
        if mix == 0.0 {
            // The normal positive-reduction path keeps its exact waveform and
            // telemetry; no extra floating-point gain work occurs here.
            return;
        }
        if mix == 1.0 {
            for channel in &mut self.gains.smoothed_gain {
                channel.fill(1.0);
            }
            self.ui.avg_reduction_db = 0.0;
            return;
        }
        let mut sum = 0.0_f32;
        let mut count = 0_usize;
        for channel in &mut self.gains.smoothed_gain {
            for gain in channel {
                *gain += mix * (1.0 - *gain);
                sum += *gain;
                count += 1;
            }
        }
        self.ui.avg_reduction_db = if count > 0 {
            -20.0 * fast_log10((sum / count as f32).clamp(1e-10, 1.0))
        } else {
            0.0
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::{DenoiserPlugin, DenoiserPluginParams};
    use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};

    #[test]
    fn live_zero_endpoint_uses_the_prepared_physical_gain_clock_and_exact_endpoints() {
        let mut cases = 0;
        let mut maximum_error = 0.0_f64;
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for low_latency in [false, true] {
                let mut plugin = DenoiserPlugin::from_params(
                    2,
                    DenoiserPluginParams {
                        low_latency,
                        ..Default::default()
                    },
                );
                plugin.initialize(rate).unwrap();
                plugin.apply_zero_reduction_endpoint();
                let hop_seconds = plugin.config.hop_size as f64 / rate;
                for (target_db, time_seconds, rising) in [(0.0, 0.005, true), (10.0, 0.05, false)] {
                    plugin
                        .parametric_set_parameter(
                            ParameterId::from("reduction_db"),
                            ParameterValue::Float(target_db),
                        )
                        .unwrap();
                    let pole = (-hop_seconds / time_seconds).exp();
                    for step in 1..=1024 {
                        for channel in &mut plugin.gains.smoothed_gain {
                            channel.fill(0.2);
                        }
                        plugin.apply_zero_reduction_endpoint();
                        let expected_mix = if rising {
                            1.0 - pole.powi(step)
                        } else {
                            pole.powi(step)
                        };
                        let expected = 0.2 + 0.8 * expected_mix;
                        let error = (f64::from(plugin.gains.smoothed_gain[0][0]) - expected).abs();
                        maximum_error = maximum_error.max(error);
                        assert!(
                            error < 2e-6,
                            "rate={rate} low={low_latency} step={step} error={error}"
                        );
                    }
                    assert_eq!(plugin.gains.zero_mix, if rising { 1.0 } else { 0.0 });
                    cases += 1;
                }
            }
        }
        println!(
            "zero_reduction_clock cases={cases} maximum_error={maximum_error:e} exact_endpoints=true"
        );
    }
}
