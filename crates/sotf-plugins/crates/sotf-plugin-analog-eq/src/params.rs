//! Analog EQ parameter definitions — single source of truth for spec arrays.
//!
//! This file owns:
//! - Parameter specs (PARAMS array)
//! - UI layout (LAYOUT)
//! - Serializable state (AnalogEqPluginParams with serde defaults)
//! - Index<->field mapping (PluginParamDef impl)
//!
//! Adding a parameter: add to PARAMS, add field to AnalogEqPluginParams, add match
//! arms in `param_value`/`set_param_value` and in the plugin's apply path.
//! Nothing else needs to change.
//!
//! Param indices: 0=low_freq, 1=low_gain, 2=mid1_freq, 3=mid1_gain, 4=mid1_q,
//! 5=mid2_freq, 6=mid2_gain, 7=mid2_q, 8=high_freq, 9=high_gain,
//! 10=analog_model, 11=analog_drive, 12=analog_color, 13=analog_character,
//! 14=analog_trim.

use serde::{Deserialize, Serialize};
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;
use sotf_host::define_choice_string_deserializer;
use sotf_plugin_analog_common::{
    MODEL_NAMES, character_param_spec, color_param_spec, drive_param_spec, model_param_spec,
    output_trim_param_spec,
};

define_choice_string_deserializer!(deserialize_analog_model, MODEL_NAMES);

// ============================================================================
// Parameter Specifications
// ============================================================================

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::float("Low Freq", "low_freq", 100.0, 20.0, 500.0, 1.0, "Hz", "Low")
        .doc("Low-shelf corner frequency"),
    ParamSpec::float("Low Gain", "low_gain", 0.0, -24.0, 24.0, 0.1, "dB", "Low")
        .doc("Low-shelf gain"),
    ParamSpec::float("LowMid Freq", "mid1_freq", 800.0, 100.0, 5000.0, 1.0, "Hz", "Low Mid")
        .doc("First peak center frequency"),
    ParamSpec::float("LowMid Gain", "mid1_gain", 0.0, -24.0, 24.0, 0.1, "dB", "Low Mid")
        .doc("First peak gain"),
    ParamSpec::float("LowMid Q", "mid1_q", 1.0, 0.1, 10.0, 0.05, "", "Low Mid")
        .doc("First peak resonance"),
    ParamSpec::float(
        "HighMid Freq",
        "mid2_freq",
        3000.0,
        500.0,
        12000.0,
        1.0,
        "Hz",
        "High Mid",
    )
    .doc("Second peak center frequency"),
    ParamSpec::float(
        "HighMid Gain",
        "mid2_gain",
        0.0,
        -24.0,
        24.0,
        0.1,
        "dB",
        "High Mid",
    )
    .doc("Second peak gain"),
    ParamSpec::float("HighMid Q", "mid2_q", 1.0, 0.1, 10.0, 0.05, "", "High Mid")
        .doc("Second peak resonance"),
    ParamSpec::float(
        "High Freq",
        "high_freq",
        10000.0,
        2000.0,
        20000.0,
        10.0,
        "Hz",
        "High",
    )
    .doc("High-shelf corner frequency"),
    ParamSpec::float("High Gain", "high_gain", 0.0, -24.0, 24.0, 0.1, "dB", "High")
        .doc("High-shelf gain"),
    model_param_spec(0, "Analog"),
    drive_param_spec("Analog"),
    color_param_spec("Analog"),
    character_param_spec("Analog"),
    output_trim_param_spec("Analog"),
];

// ============================================================================
// UI Layout
// ============================================================================

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[],
    main: &[
        ControlGroup::new(
            "LOW",
            "LOW",
            &[ControlSpec::slider(0), ControlSpec::slider(1)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.8)),
        ControlGroup::new(
            "LOW MID",
            "LOW MID",
            &[
                ControlSpec::slider(2),
                ControlSpec::slider(3),
                ControlSpec::slider(4),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.8)),
        ControlGroup::new(
            "HIGH MID",
            "HIGH MID",
            &[
                ControlSpec::slider(5),
                ControlSpec::slider(6),
                ControlSpec::slider(7),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.8)),
        ControlGroup::new(
            "HIGH",
            "HIGH",
            &[ControlSpec::slider(8), ControlSpec::slider(9)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.8)),
        ControlGroup::new(
            "ANALOG",
            "ANALOG",
            &[
                ControlSpec::selector(10),
                ControlSpec::slider(11),
                ControlSpec::slider(12),
                ControlSpec::slider(13),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new("OUTPUT", "OUTPUT", &[ControlSpec::knob(14)])
            .with_layout(GroupLayoutHints::inferred().priority(0.9)),
    ],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[],
    dynamic_sections: &[],
};

// ============================================================================
// Serializable State
// ============================================================================

fn d_low_freq() -> f64 {
    pk(PARAMS, "low_freq").default_f64()
}
fn d_low_gain() -> f64 {
    pk(PARAMS, "low_gain").default_f64()
}
fn d_mid1_freq() -> f64 {
    pk(PARAMS, "mid1_freq").default_f64()
}
fn d_mid1_gain() -> f64 {
    pk(PARAMS, "mid1_gain").default_f64()
}
fn d_mid1_q() -> f64 {
    pk(PARAMS, "mid1_q").default_f64()
}
fn d_mid2_freq() -> f64 {
    pk(PARAMS, "mid2_freq").default_f64()
}
fn d_mid2_gain() -> f64 {
    pk(PARAMS, "mid2_gain").default_f64()
}
fn d_mid2_q() -> f64 {
    pk(PARAMS, "mid2_q").default_f64()
}
fn d_high_freq() -> f64 {
    pk(PARAMS, "high_freq").default_f64()
}
fn d_high_gain() -> f64 {
    pk(PARAMS, "high_gain").default_f64()
}
fn d_analog_model() -> String {
    let idx = pk(PARAMS, "analog_model").default_usize();
    MODEL_NAMES.get(idx).unwrap_or(&MODEL_NAMES[0]).to_string()
}
fn d_analog_drive() -> f64 {
    pk(PARAMS, "analog_drive").default_f64()
}
fn d_analog_color() -> f64 {
    pk(PARAMS, "analog_color").default_f64()
}
fn d_analog_character() -> f64 {
    pk(PARAMS, "analog_character").default_f64()
}
fn d_analog_trim() -> f64 {
    pk(PARAMS, "analog_trim").default_f64()
}

/// Serializable analog-EQ state. Choice params serialize as display strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalogEqPluginParams {
    #[serde(default = "d_low_freq")]
    pub low_freq: f64,
    #[serde(default = "d_low_gain")]
    pub low_gain: f64,
    #[serde(default = "d_mid1_freq")]
    pub mid1_freq: f64,
    #[serde(default = "d_mid1_gain")]
    pub mid1_gain: f64,
    #[serde(default = "d_mid1_q")]
    pub mid1_q: f64,
    #[serde(default = "d_mid2_freq")]
    pub mid2_freq: f64,
    #[serde(default = "d_mid2_gain")]
    pub mid2_gain: f64,
    #[serde(default = "d_mid2_q")]
    pub mid2_q: f64,
    #[serde(default = "d_high_freq")]
    pub high_freq: f64,
    #[serde(default = "d_high_gain")]
    pub high_gain: f64,
    #[serde(
        default = "d_analog_model",
        deserialize_with = "deserialize_analog_model"
    )]
    pub analog_model: String,
    #[serde(default = "d_analog_drive")]
    pub analog_drive: f64,
    #[serde(default = "d_analog_color")]
    pub analog_color: f64,
    #[serde(default = "d_analog_character")]
    pub analog_character: f64,
    #[serde(default = "d_analog_trim")]
    pub analog_trim: f64,
}

impl Default for AnalogEqPluginParams {
    fn default() -> Self {
        Self {
            low_freq: d_low_freq(),
            low_gain: d_low_gain(),
            mid1_freq: d_mid1_freq(),
            mid1_gain: d_mid1_gain(),
            mid1_q: d_mid1_q(),
            mid2_freq: d_mid2_freq(),
            mid2_gain: d_mid2_gain(),
            mid2_q: d_mid2_q(),
            high_freq: d_high_freq(),
            high_gain: d_high_gain(),
            analog_model: d_analog_model(),
            analog_drive: d_analog_drive(),
            analog_color: d_analog_color(),
            analog_character: d_analog_character(),
            analog_trim: d_analog_trim(),
        }
    }
}

/// Resolve a model display string (or snake_case/lowercase alias) to its id.
/// Unknown names are rejected rather than guessed.
pub fn model_id_for_name(name: &str) -> Option<u32> {
    MODEL_NAMES
        .iter()
        .position(|candidate| {
            *candidate == name
                || candidate.to_lowercase() == name
                || candidate.to_lowercase().replace(' ', "_") == name
        })
        .map(|index| index as u32)
}

impl PluginParamDef for AnalogEqPluginParams {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 1;
    const PLUGIN_TYPE_KEY: &'static str = "analog_eq";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.low_freq),
            1 => Some(self.low_gain),
            2 => Some(self.mid1_freq),
            3 => Some(self.mid1_gain),
            4 => Some(self.mid1_q),
            5 => Some(self.mid2_freq),
            6 => Some(self.mid2_gain),
            7 => Some(self.mid2_q),
            8 => Some(self.high_freq),
            9 => Some(self.high_gain),
            10 => model_id_for_name(&self.analog_model).map(|id| id as f64),
            11 => Some(self.analog_drive),
            12 => Some(self.analog_color),
            13 => Some(self.analog_character),
            14 => Some(self.analog_trim),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.low_freq = PARAMS[0].clamp_f64(value),
            1 => self.low_gain = PARAMS[1].clamp_f64(value),
            2 => self.mid1_freq = PARAMS[2].clamp_f64(value),
            3 => self.mid1_gain = PARAMS[3].clamp_f64(value),
            4 => self.mid1_q = PARAMS[4].clamp_f64(value),
            5 => self.mid2_freq = PARAMS[5].clamp_f64(value),
            6 => self.mid2_gain = PARAMS[6].clamp_f64(value),
            7 => self.mid2_q = PARAMS[7].clamp_f64(value),
            8 => self.high_freq = PARAMS[8].clamp_f64(value),
            9 => self.high_gain = PARAMS[9].clamp_f64(value),
            10 => {
                if let Some(name) = MODEL_NAMES.get(value as usize) {
                    self.analog_model = name.to_string();
                }
            }
            11 => self.analog_drive = PARAMS[11].clamp_f64(value),
            12 => self.analog_color = PARAMS[12].clamp_f64(value),
            13 => self.analog_character = PARAMS[13].clamp_f64(value),
            14 => self.analog_trim = PARAMS[14].clamp_f64(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::param_specs::ParamType;

    #[test]
    fn defaults_match_schema() {
        let p = AnalogEqPluginParams::default();
        for (index, spec) in PARAMS.iter().enumerate() {
            let value = p.param_value(index).unwrap_or_else(|| {
                panic!("param_value({}) is None for {}", index, spec.engine_key)
            });
            let expected = match spec.param_type {
                ParamType::Float { default, .. } => default,
                ParamType::Int { default, .. } => default as f64,
                ParamType::Bool { default, .. } => default as u8 as f64,
                ParamType::Choice { default_index, .. } => default_index as f64,
                ParamType::FilePath => unreachable!(),
            };
            assert!(
                (value - expected).abs() < 1e-6,
                "default drift for {}",
                spec.engine_key
            );
        }
    }

    #[test]
    fn indexed_set_clamps_to_spec_range() {
        let mut p = AnalogEqPluginParams::default();
        p.set_param_value(0, 1e9);
        assert_eq!(p.param_value(0), Some(500.0));
        p.set_param_value(4, 1e9);
        assert_eq!(p.param_value(4), Some(10.0));
        // Model index out of range is ignored, never stored.
        p.set_param_value(10, 99.0);
        assert_eq!(p.param_value(10), Some(0.0));
    }
}
