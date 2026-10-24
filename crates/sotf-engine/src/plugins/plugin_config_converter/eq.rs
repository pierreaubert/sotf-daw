//! Converters for EQ-style filter plugins.

use super::PluginConfig;
use crate::plugins::PluginSettings;
use serde_json::json;

pub fn convert_linear_phase_eq(
    settings: &PluginSettings,
    _sample_rate: f64,
) -> Option<PluginConfig> {
    let PluginSettings::LinearPhaseEq {
        num_filters,
        fir_length,
        phase_mode,
        auto_gain,
        accuracy_mode,
        mix,
        filters,
        stereo_pairs,
    } = settings
    else {
        return None;
    };
    let any_soloed = filters.iter().any(|f| f.solo);
    let band_configs: Vec<serde_json::Value> = filters
        .iter()
        .filter(|f| !f.muted && (!any_soloed || f.solo))
        .map(|f| {
            json!({
                "filter_type": f.filter_type.long_name().to_lowercase(),
                "frequency": f.frequency,
                "q": f.q,
                "gain_db": f.gain_db,
                "active": true,
                "placement": f.placement,
            })
        })
        .collect();
    Some(PluginConfig::new(
        "linear_phase_eq",
        json!({
            "num_filters": *num_filters as usize,
            "fir_length_index": *fir_length as usize,
            "phase_mode_index": *phase_mode as usize,
            "auto_gain": auto_gain,
            "accuracy_mode": accuracy_mode,
            "mix": *mix as f32,
            "filters": band_configs,
            "stereo_pairs": stereo_pairs,
        }),
    ))
}

#[cfg(test)]
mod accuracy_tests {
    use super::*;

    #[test]
    fn linear_phase_eq_accuracy_converter_preserves_opt_in_and_manual_length() {
        for accuracy in [false, true] {
            let settings: PluginSettings = serde_json::from_value(json!({
                "LinearPhaseEq": {
                    "fir_length": 2.0,
                    "accuracy_mode": accuracy,
                    "filters": [],
                }
            }))
            .unwrap();
            assert_eq!(settings.param_value(1), Some(2.0));
            assert_eq!(settings.param_value(5), Some(f64::from(accuracy)));
            let config = convert_linear_phase_eq(&settings, 96_000.0).unwrap();
            assert_eq!(config.parameters["fir_length_index"], 2);
            assert_eq!(config.parameters["accuracy_mode"], accuracy);
            let restored: PluginSettings =
                serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
            assert_eq!(
                convert_linear_phase_eq(&restored, 96_000.0)
                    .unwrap()
                    .parameters,
                config.parameters
            );
        }
        let old: PluginSettings =
            serde_json::from_value(json!({"LinearPhaseEq":{"fir_length":2.0}})).unwrap();
        assert_eq!(
            convert_linear_phase_eq(&old, 96_000.0).unwrap().parameters["accuracy_mode"],
            false
        );
    }
}
