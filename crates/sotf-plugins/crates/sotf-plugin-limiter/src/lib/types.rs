use crate::params::{
    OVERSAMPLING_OPTIONS, default_dual_release, default_feed_forward, default_isp_mode,
    default_link_amount, default_lookahead_ms, default_mix, default_oversampling,
    default_release_ms, default_soft, default_threshold_db, default_true_peak,
};
use serde::{Deserialize, Serialize};

sotf_host::define_choice_index_deserializer!(deserialize_oversampling, OVERSAMPLING_OPTIONS);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimiterPluginParams {
    #[serde(default = "default_threshold_db")]
    pub threshold_db: f32,
    #[serde(default = "default_release_ms")]
    pub release_ms: f32,
    #[serde(default = "default_lookahead_ms")]
    pub lookahead_ms: f32,
    #[serde(default = "default_soft")]
    pub soft: bool,
    #[serde(default = "default_true_peak")]
    pub true_peak: bool,
    #[serde(default = "default_isp_mode")]
    pub isp_mode: bool,
    #[serde(default = "default_dual_release")]
    pub dual_release: bool,
    #[serde(default = "default_mix")]
    pub mix: f32,
    #[serde(default = "default_feed_forward")]
    pub feed_forward: bool,
    #[serde(default = "default_link_amount")]
    pub link_amount: f32,
    /// Structural choice index: 0 = 1x, 1 = 2x, 2 = 4x.
    #[serde(
        default = "default_oversampling",
        deserialize_with = "deserialize_oversampling"
    )]
    pub oversampling: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAMS;
    use sotf_host::param_specs::find_by_key as pk;

    #[test]
    fn oversampling_constructor_wire_forms_preserve_integer_serialization() {
        for (index, label) in OVERSAMPLING_OPTIONS.iter().enumerate() {
            for value in [
                serde_json::json!(index),
                serde_json::json!(index as f64),
                serde_json::json!(label),
            ] {
                let input = serde_json::json!({"oversampling": value});
                let constructor: LimiterPluginParams =
                    serde_json::from_value(input.clone()).unwrap();
                let state: crate::params::Params = serde_json::from_value(input).unwrap();
                assert_eq!(constructor.oversampling, index);
                assert_eq!(state.oversampling, index);
                assert_eq!(
                    serde_json::to_value(constructor).unwrap()["oversampling"],
                    index
                );
                assert_eq!(serde_json::to_value(state).unwrap()["oversampling"], index);
            }
        }
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!(3),
            serde_json::json!(0.5),
            serde_json::json!("8x"),
            serde_json::json!("2"),
            serde_json::json!(true),
            serde_json::Value::Null,
        ] {
            let input = serde_json::json!({"oversampling": invalid});
            assert!(serde_json::from_value::<LimiterPluginParams>(input.clone()).is_err());
            assert!(serde_json::from_value::<crate::params::Params>(input).is_err());
        }
    }

    #[test]
    fn deserialize_empty_json_uses_param_specs_defaults() {
        let p: LimiterPluginParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p.threshold_db, pk(PARAMS, "threshold").default_f64() as f32);
        assert_eq!(p.release_ms, pk(PARAMS, "release").default_f64() as f32);
        assert_eq!(p.lookahead_ms, pk(PARAMS, "lookahead").default_f64() as f32);
        assert_eq!(p.soft, pk(PARAMS, "soft").default_bool());
        assert_eq!(p.true_peak, pk(PARAMS, "true_peak").default_bool());
        assert_eq!(p.isp_mode, pk(PARAMS, "isp_mode").default_bool());
        assert_eq!(p.dual_release, pk(PARAMS, "dual_release").default_bool());
        assert_eq!(p.mix, pk(PARAMS, "mix").default_f64() as f32);
        assert_eq!(p.feed_forward, pk(PARAMS, "feed_forward").default_bool());
        assert_eq!(
            p.link_amount,
            pk(PARAMS, "link_amount").default_f64() as f32
        );
    }
}

/// Data exposed by the limiter for UI monitoring
#[derive(Debug, Clone, Default)]
pub struct LimiterData {
    /// Gain reduction in dB (positive value, e.g., 6.0 means -6 dB gain).
    /// At 2x/4x this is a conservative indication of actual limiting gains over
    /// both finite downsampler contributors, aligned with the final guard and
    /// effective mix. It is not a waveform amplitude or filter-loss ratio.
    pub gain_reduction_db: f32,
    /// Peak input level in dB
    pub peak_db: f32,
    /// Whether the limiter is actively limiting
    pub is_limiting: bool,
    /// Per-channel inter-sample true peak in dBTP (empty when true_peak is disabled)
    pub isp_dbtp: Vec<f32>,
    /// Peak emitted-output level in dB, post-gain and post-mix.
    ///
    /// Native 1x and 2x/4x both report the maximum absolute final sample over
    /// the existing 100 ms publication interval. Unaffected by input-only peaks.
    pub output_peak_db: f32,
    /// Per-channel final-output inter-sample true peak in dBTP.
    ///
    /// Both native and oversampled paths measure the emitted output. The legacy
    /// `isp_dbtp` field is preserved: native reports input peaks there while the
    /// oversampled path reports output peaks. New consumers prefer this field.
    /// Holds -120.0 per channel when true-peak metering is disabled.
    pub output_isp_dbtp: Vec<f32>,
}
