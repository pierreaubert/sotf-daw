//! Analog limiter parameter definitions — single source of truth.
//!
//! The limiter core is `sotf_plugin_limiter::LimiterPlugin` (composition, not
//! a fork). This plugin carries an opinionated subset of its parameters:
//! threshold, release, lookahead, soft knee, true peak, and mix. The advanced
//! `isp_mode`, `dual_release`, `link_amount`, and `feed_forward` controls are
//! intentionally not exposed: ISP-limit mode requires hard knee at 100% wet,
//! which contradicts a color stage, and the remaining three are monitoring or
//! detector-topology details that stay at core defaults.
//!
//! Param indices: 0=threshold, 1=release, 2=lookahead, 3=soft, 4=true_peak,
//! 5=mix, 6=analog_model, 7=analog_drive, 8=analog_color, 9=analog_character,
//! 10=analog_trim.

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
    ParamSpec::float("Threshold", "threshold", -0.1, -20.0, 0.0, 0.1, "dB", "Dynamics")
        .doc("Ceiling level (max output)"),
    ParamSpec::float("Release", "release", 50.0, 10.0, 1000.0, 5.0, "ms", "Timing")
        .doc("Time to return to unity gain"),
    ParamSpec::float("Lookahead", "lookahead", 5.0, 0.0, 20.0, 0.5, "ms", "Timing")
        .structural()
        .setup()
        .doc("Graph latency / pre-delay for predictive peak catching"),
    ParamSpec::bool_labeled("Soft Knee", "soft", false, "Soft", "Hard", "Dynamics")
        .setup()
        .doc("One-dB gain-computer knee vs hard limiting onset"),
    ParamSpec::bool_labeled("True Peak", "true_peak", false, "On", "Off", "Detection")
        .setup()
        .doc("Rate-appropriate ITU-R BS.1770-compatible inter-sample peak detection"),
    ParamSpec::float("Mix", "mix", 1.0, 0.0, 1.0, 0.05, "%", "Output")
        .scaled(100.0)
        .output()
        .doc("Dry/wet blend"),
    model_param_spec(0, "Analog"),
    drive_param_spec("Analog"),
    color_param_spec("Analog"),
    character_param_spec("Analog"),
    output_trim_param_spec("Analog"),
];

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[ControlSpec::slider(2), ControlSpec::toggle(3), ControlSpec::toggle(4)],
    main: &[
        ControlGroup::new(
            "DYNAMICS",
            "DYNAMICS",
            &[ControlSpec::slider(0)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "ANALOG",
            "ANALOG",
            &[
                ControlSpec::selector(6),
                ControlSpec::slider(7),
                ControlSpec::slider(8),
                ControlSpec::slider(9),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.9)),
        ControlGroup::new(
            "TIMING",
            "TIMING",
            &[ControlSpec::slider(1)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.5)),
        ControlGroup::new(
            "OUTPUT",
            "OUTPUT",
            &[ControlSpec::knob(5), ControlSpec::knob(10)],
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
fn d_release() -> f32 {
    pk(PARAMS, "release").default_f64() as f32
}
fn d_lookahead() -> f32 {
    pk(PARAMS, "lookahead").default_f64() as f32
}
fn d_soft() -> bool {
    pk(PARAMS, "soft").default_bool()
}
fn d_true_peak() -> bool {
    pk(PARAMS, "true_peak").default_bool()
}
fn d_mix() -> f32 {
    pk(PARAMS, "mix").default_f64() as f32
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

/// Serializable analog-limiter state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalogLimiterPluginParams {
    #[serde(default = "d_threshold", alias = "threshold_db")]
    pub threshold: f32,
    #[serde(default = "d_release", alias = "release_ms")]
    pub release: f32,
    #[serde(default = "d_lookahead", alias = "lookahead_ms")]
    pub lookahead: f32,
    #[serde(default = "d_soft")]
    pub soft: bool,
    #[serde(default = "d_true_peak")]
    pub true_peak: bool,
    #[serde(default = "d_mix")]
    pub mix: f32,
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

impl Default for AnalogLimiterPluginParams {
    fn default() -> Self {
        Self {
            threshold: d_threshold(),
            release: d_release(),
            lookahead: d_lookahead(),
            soft: d_soft(),
            true_peak: d_true_peak(),
            mix: d_mix(),
            analog_model: d_analog_model(),
            analog_drive: d_analog_drive(),
            analog_color: d_analog_color(),
            analog_character: d_analog_character(),
            analog_trim: d_analog_trim(),
        }
    }
}

/// Limiter-core parameter keys carried by this plugin, in PARAMS order.
pub const CORE_KEYS: &[&str] = &[
    "threshold",
    "release",
    "lookahead",
    "soft",
    "true_peak",
    "mix",
];

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

impl PluginParamDef for AnalogLimiterPluginParams {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 1;
    const PLUGIN_TYPE_KEY: &'static str = "analog_limiter";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.threshold as f64),
            1 => Some(self.release as f64),
            2 => Some(self.lookahead as f64),
            3 => Some(if self.soft { 1.0 } else { 0.0 }),
            4 => Some(if self.true_peak { 1.0 } else { 0.0 }),
            5 => Some(self.mix as f64),
            6 => model_id_for_name(&self.analog_model).map(|id| id as f64),
            7 => Some(self.analog_drive as f64),
            8 => Some(self.analog_color as f64),
            9 => Some(self.analog_character as f64),
            10 => Some(self.analog_trim as f64),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.threshold = PARAMS[0].clamp_f64(value) as f32,
            1 => self.release = PARAMS[1].clamp_f64(value) as f32,
            2 => self.lookahead = PARAMS[2].clamp_f64(value) as f32,
            3 => self.soft = PARAMS[3].clamp_f64(value) >= 0.5,
            4 => self.true_peak = PARAMS[4].clamp_f64(value) >= 0.5,
            5 => self.mix = PARAMS[5].clamp_f64(value) as f32,
            6 => {
                if let Some(name) = MODEL_NAMES.get(value as usize) {
                    self.analog_model = name.to_string();
                }
            }
            7 => self.analog_drive = PARAMS[7].clamp_f64(value) as f32,
            8 => self.analog_color = PARAMS[8].clamp_f64(value) as f32,
            9 => self.analog_character = PARAMS[9].clamp_f64(value) as f32,
            10 => self.analog_trim = PARAMS[10].clamp_f64(value) as f32,
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
        let p = AnalogLimiterPluginParams::default();
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
        let mut p = AnalogLimiterPluginParams::default();
        p.set_param_value(0, 99.0);
        assert_eq!(p.param_value(0), Some(0.0));
        p.set_param_value(2, 1e9);
        assert_eq!(p.param_value(2), Some(20.0));
        // Model index out of range is ignored, never stored.
        p.set_param_value(6, 99.0);
        assert_eq!(p.param_value(6), Some(0.0));
    }
}
