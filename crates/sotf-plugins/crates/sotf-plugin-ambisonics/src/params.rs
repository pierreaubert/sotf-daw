//! Ambisonics Decoder plugin parameter definitions — single source of truth.
//!
//! This file owns:
//! - Parameter specs (PARAMS array)
//! - UI layout (LAYOUT)
//! - Serializable state (Params struct with serde defaults)
//! - Index<->field mapping (PluginParamDef impl)
//!

// Rust guideline compliant 2026-02-21
//! Adding a parameter: add to PARAMS, add field to Params, add match arms.
//! Nothing else needs to change.

use serde::{Deserialize, Serialize};
use sotf_host::define_choice_string_deserializer;
#[cfg(test)]
use sotf_host::param_specs::ParamType;
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

define_choice_string_deserializer!(deserialize_target_layout, TARGET_LAYOUTS);
define_choice_string_deserializer!(deserialize_algorithm, ALGORITHMS);

// ============================================================================
// Parameter Specifications
// ============================================================================

pub const TARGET_LAYOUTS: &[&str] = &[
    "5.1",
    "7.1",
    "5.1.2",
    "5.1.4",
    "7.1.2",
    "7.1.4",
    "9.1.4",
    "9.1.6",
    // Appended last: user-defined geometry via `CustomDecoderConfig`.
    // Existing indices 0-7 are unchanged for saved-state compatibility.
    crate::custom_layout::CUSTOM_LAYOUT_KEY,
];

pub const ALGORITHMS: &[&str] = &["mode_matching", "allrad"];

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::int("Order", "order", 1, 1, 7, 1, "", "Ambisonics")
        .structural()
        .doc("Ambisonics order (1-7)"),
    ParamSpec::choice(
        "Target Layout",
        "target_layout",
        0,
        TARGET_LAYOUTS,
        "Ambisonics",
    )
    .structural()
    .doc("Target speaker layout for decode"),
    ParamSpec::bool_param("Max-rE", "max_re_weighting", true, "Ambisonics")
        .structural()
        .doc("Apply max-rE energy optimization"),
    ParamSpec::bool_param("Dual-Band", "dual_band", false, "Ambisonics")
        .structural()
        .doc("Separate LF/HF decode weights"),
    ParamSpec::choice("Algorithm", "algorithm", 0, ALGORITHMS, "Ambisonics")
        .structural()
        .doc("Decode algorithm: regularized mode matching or AllRAD/VBAP"),
];

// ============================================================================
// UI Layout
// ============================================================================

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[],
    main: &[
        ControlGroup::new(
            "decode",
            "Decode setup",
            &[
                ControlSpec::knob(0),     // order
                ControlSpec::selector(1), // target_layout
                ControlSpec::selector(4), // algorithm
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "primary",
            "Decode options",
            &[ControlSpec::toggle(2), ControlSpec::toggle(3)], // max_re_weighting, dual_band
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.9).keep_visible()),
    ],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[ColumnConstraint::main(200.0)],
    dynamic_sections: &[],
};

// ============================================================================
// Serializable Parameter State
// ============================================================================

/// Ambisonics Decoder plugin parameters.
///
/// All serde defaults are derived from PARAMS — adding a field here with
/// the correct default function is enough to support old presets that
/// don't have the new field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    #[serde(default = "d_order")]
    pub order: usize,
    #[serde(
        default = "d_target_layout",
        deserialize_with = "deserialize_target_layout"
    )]
    pub target_layout: String,
    #[serde(default = "d_max_re_weighting")]
    pub max_re_weighting: bool,
    #[serde(default = "d_dual_band")]
    pub dual_band: bool,
    #[serde(default = "d_algorithm", deserialize_with = "deserialize_algorithm")]
    pub algorithm: String,
}

fn d_order() -> usize {
    pk(PARAMS, "order").default_usize()
}
fn d_target_layout() -> String {
    TARGET_LAYOUTS[0].to_string()
}
fn d_max_re_weighting() -> bool {
    pk(PARAMS, "max_re_weighting").default_bool()
}
fn d_dual_band() -> bool {
    pk(PARAMS, "dual_band").default_bool()
}
fn d_algorithm() -> String {
    ALGORITHMS[pk(PARAMS, "algorithm")
        .default_usize()
        .min(ALGORITHMS.len() - 1)]
    .to_owned()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            order: d_order(),
            target_layout: d_target_layout(),
            max_re_weighting: d_max_re_weighting(),
            dual_band: d_dual_band(),
            algorithm: d_algorithm(),
        }
    }
}

// ============================================================================
// PluginParamDef implementation
// ============================================================================

impl PluginParamDef for Params {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 2;
    const PLUGIN_TYPE_KEY: &'static str = "ambisonics_decoder";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.order as f64),
            1 => {
                let idx = TARGET_LAYOUTS
                    .iter()
                    .position(|&t| t == self.target_layout)
                    .unwrap_or(0);
                Some(idx as f64)
            }
            2 => Some(if self.max_re_weighting { 1.0 } else { 0.0 }),
            3 => Some(if self.dual_band { 1.0 } else { 0.0 }),
            4 => Some(
                ALGORITHMS
                    .iter()
                    .position(|&algorithm| algorithm == self.algorithm)
                    .unwrap_or(0) as f64,
            ),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.order = PARAMS[0].clamp_f64(value) as usize,
            1 => {
                let idx = (value as usize).min(TARGET_LAYOUTS.len() - 1);
                self.target_layout = TARGET_LAYOUTS[idx].to_string();
            }
            2 => self.max_re_weighting = PARAMS[2].clamp_f64(value) > 0.5,
            3 => self.dual_band = PARAMS[3].clamp_f64(value) > 0.5,
            4 => {
                let index = (value as usize).min(ALGORITHMS.len() - 1);
                self.algorithm = ALGORITHMS[index].to_owned();
            }
            _ => {}
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_index_coverage() {
        let p = Params::default();
        for i in 0..PARAMS.len() {
            assert!(
                p.param_value(i).is_some(),
                "param_value({}) returned None",
                i
            );
        }
        assert!(
            p.param_value(PARAMS.len()).is_none(),
            "param_value beyond PARAMS.len() should return None"
        );
    }

    #[test]
    fn roundtrip_serde() {
        let original = Params::default();
        let json = serde_json::to_value(&original).unwrap();
        let restored: Params = serde_json::from_value(json).unwrap();
        assert_eq!(original, restored);
        assert_eq!(original.order, restored.order);
        assert_eq!(original.target_layout, restored.target_layout);
        assert_eq!(original.max_re_weighting, restored.max_re_weighting);
        assert_eq!(original.dual_band, restored.dual_band);
        assert_eq!(original.algorithm, restored.algorithm);
    }

    #[test]
    fn deserialize_empty_json_uses_defaults() {
        let p: Params = serde_json::from_str("{}").unwrap();
        assert_eq!(p.order, pk(PARAMS, "order").default_usize());
        assert_eq!(p.target_layout, TARGET_LAYOUTS[0]);
        assert_eq!(
            p.max_re_weighting,
            pk(PARAMS, "max_re_weighting").default_bool()
        );
        assert_eq!(p.dual_band, pk(PARAMS, "dual_band").default_bool());
        assert_eq!(p.algorithm, ALGORITHMS[0]);
    }

    #[test]
    fn custom_choice_is_appended_without_moving_named_indices() {
        assert_eq!(
            TARGET_LAYOUTS,
            &[
                "5.1", "7.1", "5.1.2", "5.1.4", "7.1.2", "7.1.4", "9.1.4", "9.1.6", "custom",
            ]
        );
    }

    #[test]
    fn plugin_type_key_matches_factory_key() {
        assert_eq!(Params::PLUGIN_TYPE_KEY, "ambisonics_decoder");
    }

    #[test]
    fn order_parameter_advertises_full_supported_range() {
        let ParamType::Int { min, max, .. } = PARAMS[0].param_type else {
            panic!("order parameter must remain integral");
        };
        assert_eq!(min, 1);
        assert_eq!(max, 7);
        assert_eq!(PARAMS[0].doc, "Ambisonics order (1-7)");
    }
}
