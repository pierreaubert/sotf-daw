use serde::{Deserialize, Serialize};
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

/// Click-family selector: isolated random clicks, or periodic repetition
/// with phase prediction.
pub const MODE_OPTIONS: &[&str] = &["Random", "Periodic"];
/// Detection banding: legacy fullband, or complementary low/high splits.
pub const BANDS_OPTIONS: &[&str] = &["Fullband", "2-band", "3-band"];

sotf_host::define_choice_index_deserializer!(deserialize_mode, MODE_OPTIONS);
sotf_host::define_choice_index_deserializer!(deserialize_bands, BANDS_OPTIONS);

/// Index of the first appended (post-legacy) parameter.
///
/// Indices 0-2 are the frozen legacy parameters; everything from here on
/// was appended with serde defaults and must keep legacy defaults neutral.
pub const FIRST_EXTENDED_PARAM: usize = 3;

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::bool_param("Enabled", "enabled", true, "General")
        .doc("Enable time-domain click repair"),
    ParamSpec::float(
        "Sensitivity",
        "sensitivity",
        10.0,
        1.0,
        100.0,
        1.0,
        "",
        "General",
    )
    .doc("Click detection sensitivity; lower values detect more clicks"),
    ParamSpec::bool_param("Link Channels", "link_channels", true, "General")
        .doc("Link click decisions in adjacent channel pairs to preserve spatial coherence"),
    ParamSpec::choice("Mode", "mode", 0, MODE_OPTIONS, "General")
        .structural()
        .doc("Click family: random isolated clicks, or periodic repetition with phase prediction"),
    ParamSpec::choice("Bands", "bands", 0, BANDS_OPTIONS, "Multiband")
        .structural()
        .doc("Detection banding; fullband preserves the legacy single-stream behavior"),
    ParamSpec::float(
        "Crossover",
        "crossover_hz",
        4000.0,
        80.0,
        12_000.0,
        10.0,
        "Hz",
        "Multiband",
    )
    .structural()
    .doc("Band split frequency: the 2-band edge, or the 3-band geometric center"),
    ParamSpec::float(
        "Frequency Skew",
        "frequency_skew",
        0.0,
        -1.0,
        1.0,
        0.01,
        "",
        "Multiband",
    )
    .doc("Bias detection toward high (+) or low (-) bands; 0 is neutral"),
    ParamSpec::int(
        "Repair Width",
        "repair_width",
        0,
        0,
        8,
        1,
        "samples",
        "General",
    )
    .structural()
    .doc("Symmetric repair extension in samples; adds equal latency on new-mode paths"),
    ParamSpec::bool_param("Audition Residual", "audition_residual", false, "Monitor")
        .doc("Output the aligned residual (removed clicks) instead of repaired audio"),
];

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[
        ControlSpec::toggle(0),
        ControlSpec::toggle(2),
        ControlSpec::button_set(3, MODE_OPTIONS),
        ControlSpec::button_set(4, BANDS_OPTIONS),
        ControlSpec::toggle(8),
    ],
    main: &[
        ControlGroup::new(
            "REPAIR",
            "REPAIR",
            &[ControlSpec::slider(1), ControlSpec::slider(7)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "MULTIBAND",
            "MULTIBAND",
            &[
                ControlSpec::slider(5).enabled_when(ParamCondition::choice_in(4, &[1, 2])),
                ControlSpec::slider(6).enabled_when(ParamCondition::choice_in(4, &[1, 2])),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.4)),
    ],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[ColumnConstraint::main(300.0)],
    dynamic_sections: &[],
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    #[serde(default = "d_enabled")]
    pub enabled: bool,
    #[serde(default = "d_sensitivity")]
    pub sensitivity: f64,
    #[serde(default = "d_link_channels")]
    pub link_channels: bool,
    #[serde(default = "d_mode", deserialize_with = "deserialize_mode")]
    pub mode: usize,
    #[serde(default = "d_bands", deserialize_with = "deserialize_bands")]
    pub bands: usize,
    #[serde(default = "d_crossover_hz")]
    pub crossover_hz: f64,
    #[serde(default = "d_frequency_skew")]
    pub frequency_skew: f64,
    #[serde(default = "d_repair_width")]
    pub repair_width: usize,
    #[serde(default = "d_audition_residual")]
    pub audition_residual: bool,
}

fn d_enabled() -> bool {
    pk(PARAMS, "enabled").default_bool()
}
fn d_sensitivity() -> f64 {
    pk(PARAMS, "sensitivity").default_f64()
}
fn d_link_channels() -> bool {
    pk(PARAMS, "link_channels").default_bool()
}
fn d_mode() -> usize {
    pk(PARAMS, "mode").default_usize()
}
fn d_bands() -> usize {
    pk(PARAMS, "bands").default_usize()
}
fn d_crossover_hz() -> f64 {
    pk(PARAMS, "crossover_hz").default_f64()
}
fn d_frequency_skew() -> f64 {
    pk(PARAMS, "frequency_skew").default_f64()
}
fn d_repair_width() -> usize {
    pk(PARAMS, "repair_width").default_usize()
}
fn d_audition_residual() -> bool {
    pk(PARAMS, "audition_residual").default_bool()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            enabled: d_enabled(),
            sensitivity: d_sensitivity(),
            link_channels: d_link_channels(),
            mode: d_mode(),
            bands: d_bands(),
            crossover_hz: d_crossover_hz(),
            frequency_skew: d_frequency_skew(),
            repair_width: d_repair_width(),
            audition_residual: d_audition_residual(),
        }
    }
}

impl PluginParamDef for Params {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    // Appended fields with serde defaults need no migration; VERSION only
    // bumps on renames or removals.
    const VERSION: u32 = 2;
    const PLUGIN_TYPE_KEY: &'static str = "declick";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.enabled { 1.0 } else { 0.0 }),
            1 => Some(self.sensitivity),
            2 => Some(if self.link_channels { 1.0 } else { 0.0 }),
            3 => Some(self.mode as f64),
            4 => Some(self.bands as f64),
            5 => Some(self.crossover_hz),
            6 => Some(self.frequency_skew),
            7 => Some(self.repair_width as f64),
            8 => Some(if self.audition_residual { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.enabled = PARAMS[0].clamp_f64(value) > 0.5,
            1 => self.sensitivity = PARAMS[1].clamp_f64(value),
            2 => self.link_channels = PARAMS[2].clamp_f64(value) > 0.5,
            3 => self.mode = PARAMS[3].clamp_f64(value).round() as usize,
            4 => self.bands = PARAMS[4].clamp_f64(value).round() as usize,
            5 => self.crossover_hz = PARAMS[5].clamp_f64(value),
            6 => self.frequency_skew = PARAMS[6].clamp_f64(value),
            7 => self.repair_width = PARAMS[7].clamp_f64(value).round() as usize,
            8 => self.audition_residual = PARAMS[8].clamp_f64(value) > 0.5,
            _ => {}
        }
    }
}
