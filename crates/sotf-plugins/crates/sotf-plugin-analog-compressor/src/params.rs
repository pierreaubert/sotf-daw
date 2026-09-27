//! Analog compressor parameter definitions — single source of truth.
//!
//! The dynamics core is a native single-band feed-forward peak compressor
//! built on host primitives (`EnvelopeFollower`): linked detection on the
//! maximum absolute input across channels, soft-knee gain computer, static +
//! auto makeup, and parallel mix. Parameter vocabulary matches the existing
//! single-band compressor ranges (threshold, ratio, attack, release, knee,
//! makeup, mix, auto-makeup).
//!
//! Param indices: 0=threshold, 1=ratio, 2=attack, 3=release, 4=knee,
//! 5=makeup, 6=mix, 7=auto_makeup, 8=analog_model, 9=analog_drive,
//! 10=analog_color, 11=analog_character, 12=analog_trim.

use serde::{Deserialize, Serialize};
use sotf_host::define_choice_string_deserializer;
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;
use sotf_plugin_analog_common::{
    MODEL_NAMES, character_param_spec, color_param_spec, drive_param_spec, model_param_spec,
    output_trim_param_spec,
};

define_choice_string_deserializer!(deserialize_analog_model, MODEL_NAMES);

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::float("Threshold", "threshold", -18.0, -60.0, 0.0, 0.5, "dB", "Dynamics")
        .doc("Level above which gain reduction starts"),
    ParamSpec::float("Ratio", "ratio", 4.0, 1.0, 20.0, 0.1, ":1", "Dynamics")
        .doc("Gain-reduction ratio above threshold"),
    ParamSpec::float("Attack", "attack", 10.0, 0.1, 100.0, 0.5, "ms", "Dynamics")
        .doc("Detector attack time"),
    ParamSpec::float("Release", "release", 100.0, 10.0, 1000.0, 5.0, "ms", "Dynamics")
        .doc("Detector release time"),
    ParamSpec::float("Knee", "knee", 6.0, 0.0, 20.0, 0.5, "dB", "Dynamics")
        .doc("Soft-knee width around threshold; 0 dB is hard knee"),
    ParamSpec::float("Makeup", "makeup", 0.0, -24.0, 24.0, 0.5, "dB", "Dynamics")
        .doc("Static output makeup gain"),
    ParamSpec::float("Mix", "mix", 1.0, 0.0, 1.0, 0.01, "%", "Output")
        .scaled(100.0)
        .output()
        .doc("Dry/wet parallel blend"),
    ParamSpec::bool_labeled("Auto Makeup", "auto_makeup", false, "On", "Off", "Dynamics")
        .doc("Add smoothed measured gain reduction back as makeup (capped at +24 dB)"),
    model_param_spec(0, "Analog"),
    drive_param_spec("Analog"),
    color_param_spec("Analog"),
    character_param_spec("Analog"),
    output_trim_param_spec("Analog"),
];

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[ControlSpec::toggle(7)],
    main: &[
        ControlGroup::new(
            "DYNAMICS",
            "DYNAMICS",
            &[
                ControlSpec::slider(0),
                ControlSpec::slider(1),
                ControlSpec::slider(4),
                ControlSpec::slider(5),
                ControlSpec::slider(2),
                ControlSpec::slider(3),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "ANALOG",
            "ANALOG",
            &[
                ControlSpec::selector(8),
                ControlSpec::slider(9),
                ControlSpec::slider(10),
                ControlSpec::slider(11),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.9)),
        ControlGroup::new(
            "OUTPUT",
            "OUTPUT",
            &[ControlSpec::knob(6), ControlSpec::knob(12)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.8)),
    ],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[],
    dynamic_sections: &[],
};

fn d_threshold() -> f32 {
    pk(PARAMS, "threshold").default_f64() as f32
}
fn d_ratio() -> f32 {
    pk(PARAMS, "ratio").default_f64() as f32
}
fn d_attack() -> f32 {
    pk(PARAMS, "attack").default_f64() as f32
}
fn d_release() -> f32 {
    pk(PARAMS, "release").default_f64() as f32
}
fn d_knee() -> f32 {
    pk(PARAMS, "knee").default_f64() as f32
}
fn d_makeup() -> f32 {
    pk(PARAMS, "makeup").default_f64() as f32
}
fn d_mix() -> f32 {
    pk(PARAMS, "mix").default_f64() as f32
}
fn d_auto_makeup() -> bool {
    pk(PARAMS, "auto_makeup").default_bool()
}
fn d_analog_model() -> String {
    let idx = pk(PARAMS, "analog_model").default_usize();
    MODEL_NAMES.get(idx).unwrap_or(&MODEL_NAMES[0]).to_string()
}
fn d_analog_drive() -> f32 {
    pk(PARAMS, "analog_drive").default_f64() as f32
}
fn d_analog_color() -> f32 {
    pk(PARAMS, "analog_color").default_f64() as f32
}
fn d_analog_character() -> f32 {
    pk(PARAMS, "analog_character").default_f64() as f32
}
fn d_analog_trim() -> f32 {
    pk(PARAMS, "analog_trim").default_f64() as f32
}

/// Serializable analog-compressor state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalogCompressorPluginParams {
    #[serde(default = "d_threshold")]
    pub threshold: f32,
    #[serde(default = "d_ratio")]
    pub ratio: f32,
    #[serde(default = "d_attack")]
    pub attack: f32,
    #[serde(default = "d_release")]
    pub release: f32,
    #[serde(default = "d_knee")]
    pub knee: f32,
    #[serde(default = "d_makeup")]
    pub makeup: f32,
    #[serde(default = "d_mix")]
    pub mix: f32,
    #[serde(default = "d_auto_makeup")]
    pub auto_makeup: bool,
    #[serde(
        default = "d_analog_model",
        deserialize_with = "deserialize_analog_model"
    )]
    pub analog_model: String,
    #[serde(default = "d_analog_drive")]
    pub analog_drive: f32,
    #[serde(default = "d_analog_color")]
    pub analog_color: f32,
    #[serde(default = "d_analog_character")]
    pub analog_character: f32,
    #[serde(default = "d_analog_trim")]
    pub analog_trim: f32,
}

impl Default for AnalogCompressorPluginParams {
    fn default() -> Self {
        Self {
            threshold: d_threshold(),
            ratio: d_ratio(),
            attack: d_attack(),
            release: d_release(),
            knee: d_knee(),
            makeup: d_makeup(),
            mix: d_mix(),
            auto_makeup: d_auto_makeup(),
            analog_model: d_analog_model(),
            analog_drive: d_analog_drive(),
            analog_color: d_analog_color(),
            analog_character: d_analog_character(),
            analog_trim: d_analog_trim(),
        }
    }
}

/// Resolve a model display string (or snake_case/lowercase alias) to its id.
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

impl PluginParamDef for AnalogCompressorPluginParams {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 1;
    const PLUGIN_TYPE_KEY: &'static str = "analog_compressor";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.threshold as f64),
            1 => Some(self.ratio as f64),
            2 => Some(self.attack as f64),
            3 => Some(self.release as f64),
            4 => Some(self.knee as f64),
            5 => Some(self.makeup as f64),
            6 => Some(self.mix as f64),
            7 => Some(if self.auto_makeup { 1.0 } else { 0.0 }),
            8 => model_id_for_name(&self.analog_model).map(|id| id as f64),
            9 => Some(self.analog_drive as f64),
            10 => Some(self.analog_color as f64),
            11 => Some(self.analog_character as f64),
            12 => Some(self.analog_trim as f64),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.threshold = PARAMS[0].clamp_f64(value) as f32,
            1 => self.ratio = PARAMS[1].clamp_f64(value) as f32,
            2 => self.attack = PARAMS[2].clamp_f64(value) as f32,
            3 => self.release = PARAMS[3].clamp_f64(value) as f32,
            4 => self.knee = PARAMS[4].clamp_f64(value) as f32,
            5 => self.makeup = PARAMS[5].clamp_f64(value) as f32,
            6 => self.mix = PARAMS[6].clamp_f64(value) as f32,
            7 => self.auto_makeup = PARAMS[7].clamp_f64(value) >= 0.5,
            8 => {
                if let Some(name) = MODEL_NAMES.get(value as usize) {
                    self.analog_model = name.to_string();
                }
            }
            9 => self.analog_drive = PARAMS[9].clamp_f64(value) as f32,
            10 => self.analog_color = PARAMS[10].clamp_f64(value) as f32,
            11 => self.analog_character = PARAMS[11].clamp_f64(value) as f32,
            12 => self.analog_trim = PARAMS[12].clamp_f64(value) as f32,
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
        let p = AnalogCompressorPluginParams::default();
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
        let mut p = AnalogCompressorPluginParams::default();
        p.set_param_value(0, 99.0);
        assert_eq!(p.param_value(0), Some(0.0));
        p.set_param_value(0, -99.0);
        assert_eq!(p.param_value(0), Some(-60.0));
        // Model index out of range is ignored, never stored.
        p.set_param_value(8, 99.0);
        assert_eq!(p.param_value(8), Some(0.0));
        p.set_param_value(8, 2.0);
        assert_eq!(p.param_value(8), Some(2.0));
    }
}
