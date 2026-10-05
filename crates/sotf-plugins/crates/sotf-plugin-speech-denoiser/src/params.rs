//! Speech denoiser parameter schema and defaults.
//!
//! Single source of truth for the `enabled`, `strength`, and `model`
//! parameters: ParamSpec entries, UI layout, serializable v2 state, and the
//! index mapping used by automation. Index 0 (`enabled`) keeps its v1
//! identity and default; `strength` and `model` were appended without
//! reordering older entries.

// Rust guideline compliant 2026-02-21

use crate::model::{DEFAULT_MODEL_INDEX, MODEL_LABELS};
use serde::{Deserialize, Serialize};
use sotf_host::define_choice_index_deserializer;
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::bool_param("Enabled", "enabled", true, "General")
        .doc("Enable RNNoise speech denoising"),
    ParamSpec::float("Strength", "strength", 1.0, 0.0, 1.0, 0.01, "%", "General")
        .scaled(100.0)
        .doc("Suppression strength: full wet at 100%, delayed dry at 0%"),
    ParamSpec::choice(
        "Model",
        "model",
        DEFAULT_MODEL_INDEX,
        MODEL_LABELS,
        "General",
    )
    .structural()
    .setup()
    .doc("Bundled inference model identity"),
];

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[],
    main: &[ControlGroup::new(
        "SPEECH",
        "SPEECH",
        &[
            ControlSpec::toggle(0),
            ControlSpec::knob(1),
            ControlSpec::choice(2),
        ],
    )
    .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible())],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[ColumnConstraint::main(180.0)],
    dynamic_sections: &[],
};

define_choice_index_deserializer!(deserialize_model_index, MODEL_LABELS);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    #[serde(default = "d_enabled")]
    pub enabled: bool,
    #[serde(default = "d_strength")]
    pub strength: f64,
    #[serde(default = "d_model", deserialize_with = "deserialize_model_index")]
    pub model: usize,
}

fn d_enabled() -> bool {
    pk(PARAMS, "enabled").default_bool()
}

fn d_strength() -> f64 {
    pk(PARAMS, "strength").default_f64()
}

fn d_model() -> usize {
    pk(PARAMS, "model").default_usize()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            enabled: d_enabled(),
            strength: d_strength(),
            model: d_model(),
        }
    }
}

impl PluginParamDef for Params {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    /// Schema v2 appends `strength`/`model`; v1 state migrates via defaults.
    const VERSION: u32 = 2;
    const PLUGIN_TYPE_KEY: &'static str = "speech_denoiser";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.enabled { 1.0 } else { 0.0 }),
            1 => Some(self.strength),
            2 => Some(self.model as f64),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.enabled = PARAMS[0].clamp_f64(value) > 0.5,
            1 => self.strength = PARAMS[1].clamp_f64(value),
            2 => self.model = PARAMS[2].clamp_f64(value) as usize,
            _ => {}
        }
    }

    /// Migrates older JSON to the v2 schema.
    ///
    /// V1 state carries only `enabled`; missing `strength`/`model` fields
    /// deserialize to full wet and the bundled model, which reproduce v1
    /// audio bit-exactly, so no rewrite is required.
    fn migrate(value: serde_json::Value, _from_version: u32) -> serde_json::Value {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_v2_roundtrip_and_v1_state_loads_with_new_defaults() {
        assert_eq!(<Params as PluginParamDef>::VERSION, 2);
        assert_eq!(PARAMS.len(), 3);
        assert_eq!(PARAMS[0].engine_key, "enabled");
        assert_eq!(PARAMS[1].engine_key, "strength");
        assert_eq!(PARAMS[2].engine_key, "model");
        // V1 state without the appended fields keeps v1 audio by default.
        let v1: Params = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!v1.enabled);
        assert_eq!(v1.strength, 1.0);
        assert_eq!(v1.model, DEFAULT_MODEL_INDEX);
        let missing: Params = serde_json::from_str("{}").unwrap();
        assert!(missing.enabled);
        assert_eq!(missing.strength, 1.0);
        assert_eq!(missing.model, DEFAULT_MODEL_INDEX);
        for enabled in [false, true] {
            for strength in [0.0, 0.5, 1.0] {
                let params = Params {
                    enabled,
                    strength,
                    model: 0,
                };
                let json = serde_json::to_string(&params).unwrap();
                let decoded: Params = serde_json::from_str(&json).unwrap();
                assert_eq!(decoded.enabled, enabled);
                assert_eq!(decoded.strength, strength);
                assert_eq!(decoded.model, 0);
            }
        }
    }

    #[test]
    fn schema_accepts_model_label_and_rejects_unknown_or_malformed() {
        let labeled: Params = serde_json::from_str(r#"{"model":"RNNoise Full"}"#).unwrap();
        assert_eq!(labeled.model, 0);
        let indexed: Params = serde_json::from_str(r#"{"model":0}"#).unwrap();
        assert_eq!(indexed.model, 0);
        let legacy: Params = serde_json::from_str(r#"{"model":"RNNoise Legacy SH"}"#).unwrap();
        assert_eq!(legacy.model, 2);
        assert!(serde_json::from_str::<Params>(r#"{"enabled":1}"#).is_err());
        assert!(serde_json::from_str::<Params>(r#"{"strength":"full"}"#).is_err());
        assert!(serde_json::from_str::<Params>(r#"{"model":"RNNoise Light"}"#).is_err());
        assert!(serde_json::from_str::<Params>(r#"{"model":3}"#).is_err());
        assert!(serde_json::from_str::<Params>(r#"{"enabled":true,"future":2}"#).is_err());
        assert!(serde_json::from_str::<Params>(r#"{"version":2,"enabled":true}"#).is_err());
    }

    #[test]
    fn indexed_access_clamps_and_covers_all_parameters() {
        let mut params = Params::default();
        assert_eq!(params.param_value(0), Some(1.0));
        assert_eq!(params.param_value(1), Some(1.0));
        assert_eq!(params.param_value(2), Some(0.0));
        assert_eq!(params.param_value(3), None);
        params.set_param_value(0, 0.0);
        params.set_param_value(1, 5.0);
        params.set_param_value(2, 7.0);
        params.set_param_value(9, 1.0);
        assert!(!params.enabled);
        assert_eq!(params.strength, 1.0);
        assert_eq!(params.model, 2);
        params.set_param_value(2, 1.0);
        assert_eq!(params.model, 1);
    }
}
