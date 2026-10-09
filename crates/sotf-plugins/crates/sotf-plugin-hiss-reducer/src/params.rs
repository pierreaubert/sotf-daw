use crate::profile::LINK_LABELS;
use serde::{Deserialize, Serialize};
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::bool_param("Enabled", "enabled", true, "General")
        .doc("Enable high-frequency hiss reduction"),
    ParamSpec::float(
        "Threshold",
        "threshold_db",
        -30.0,
        -60.0,
        -10.0,
        0.5,
        "dB",
        "General",
    )
    .doc("Absolute high-band RMS dBFS threshold for persistent-noise reduction"),
    ParamSpec::float(
        "Frequency",
        "frequency_hz",
        4000.0,
        1000.0,
        16000.0,
        100.0,
        "Hz",
        "General",
    )
    .doc("One-pole high-band edge (limited to 0.45 x sample rate)"),
    ParamSpec::float("Strength", "strength", 0.5, 0.0, 1.0, 0.01, "", "General")
        .scaled(100.0)
        .doc("Hiss attenuation strength"),
    ParamSpec::bool_param("Spectral mode", "spectral_mode", false, "General")
        .doc("Use the fixed-latency STFT minimum-statistics reducer")
        .structural(),
    ParamSpec::bool_labeled(
        "Learn Noise",
        "learn_noise",
        false,
        "Active",
        "Off",
        "Noise Profile",
    )
    .structural()
    .doc("Trigger: capture a 1 s noise-only reference. Fired via named set_parameter; the indexed API never fires it"),
    ParamSpec::bool_param(
        "Use Profile",
        "use_captured_profile",
        false,
        "Noise Profile",
    )
    .doc("Use the captured noise profile (time-domain threshold follows the measured floor; spectral per-bin noise uses it)"),
    ParamSpec::bool_labeled(
        "Clear Profile",
        "clear_profile",
        false,
        "Trigger",
        "Off",
        "Noise Profile",
    )
    .structural()
    .doc("Trigger: discard the captured profile. Always reads 0.0; indexed set is a no-op, use named set_parameter"),
    ParamSpec::float(
        "Curve Low",
        "curve_low",
        1.0,
        0.0,
        1.0,
        0.01,
        "",
        "Curve",
    )
    .scaled(100.0)
    .doc("Spectral-only reduction scale at or below 1 kHz (applied spectrally; time-domain stores it)"),
    ParamSpec::float(
        "Curve Mid",
        "curve_mid",
        1.0,
        0.0,
        1.0,
        0.01,
        "",
        "Curve",
    )
    .scaled(100.0)
    .doc("Spectral-only reduction scale at 4 kHz (applied spectrally; time-domain stores it)"),
    ParamSpec::float(
        "Curve High",
        "curve_high",
        1.0,
        0.0,
        1.0,
        0.01,
        "",
        "Curve",
    )
    .scaled(100.0)
    .doc("Spectral-only reduction scale at or above 12 kHz (applied spectrally; time-domain stores it)"),
    ParamSpec::choice("Link", "link_mode", 0, LINK_LABELS, "Stereo")
        .doc("Channel linking: Independent keeps per-channel detectors; Linked shares them"),
    ParamSpec::bool_param(
        "Transient Guard",
        "transient_guard",
        false,
        "Spectral",
    )
    .doc("Spectral-only broadband-onset guard: lifts reduction during confirmed transients"),
];

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[
        ControlSpec::toggle(0),
        ControlSpec::toggle(4),
        ControlSpec::button_set(11, LINK_LABELS),
    ],
    main: &[
        ControlGroup::new(
            "HISS",
            "HISS",
            &[
                ControlSpec::knob(1),
                ControlSpec::knob(2),
                ControlSpec::slider(3),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "PROFILE",
            "PROFILE",
            &[
                ControlSpec::toggle(5),
                ControlSpec::toggle(6),
                ControlSpec::toggle(7),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.8)),
        ControlGroup::new(
            "CURVE",
            "CURVE",
            &[
                ControlSpec::knob(8).enabled_when(ParamCondition::bool(4, true)),
                ControlSpec::knob(9).enabled_when(ParamCondition::bool(4, true)),
                ControlSpec::knob(10).enabled_when(ParamCondition::bool(4, true)),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.6)),
        ControlGroup::new(
            "GUARD",
            "GUARD",
            &[ControlSpec::toggle(12).enabled_when(ParamCondition::bool(4, true))],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.4)),
    ],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[ColumnConstraint::main(320.0)],
    dynamic_sections: &[],
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    #[serde(default = "d_enabled")]
    pub enabled: bool,
    #[serde(default = "d_threshold_db")]
    pub threshold_db: f64,
    #[serde(default = "d_frequency_hz")]
    pub frequency_hz: f64,
    #[serde(default = "d_strength")]
    pub strength: f64,
    #[serde(default = "d_spectral_mode")]
    pub spectral_mode: bool,
    #[serde(default = "d_learn_noise")]
    pub learn_noise: bool,
    #[serde(default = "d_use_captured_profile")]
    pub use_captured_profile: bool,
    #[serde(default = "d_clear_profile")]
    pub clear_profile: bool,
    #[serde(default = "d_curve_low")]
    pub curve_low: f64,
    #[serde(default = "d_curve_mid")]
    pub curve_mid: f64,
    #[serde(default = "d_curve_high")]
    pub curve_high: f64,
    #[serde(default = "d_link_mode")]
    pub link_mode: i32,
    #[serde(default = "d_transient_guard")]
    pub transient_guard: bool,
}

fn d_enabled() -> bool {
    pk(PARAMS, "enabled").default_bool()
}
fn d_threshold_db() -> f64 {
    pk(PARAMS, "threshold_db").default_f64()
}
fn d_frequency_hz() -> f64 {
    pk(PARAMS, "frequency_hz").default_f64()
}
fn d_strength() -> f64 {
    pk(PARAMS, "strength").default_f64()
}
fn d_spectral_mode() -> bool {
    pk(PARAMS, "spectral_mode").default_bool()
}
fn d_learn_noise() -> bool {
    pk(PARAMS, "learn_noise").default_bool()
}
fn d_use_captured_profile() -> bool {
    pk(PARAMS, "use_captured_profile").default_bool()
}
fn d_clear_profile() -> bool {
    pk(PARAMS, "clear_profile").default_bool()
}
fn d_curve_low() -> f64 {
    pk(PARAMS, "curve_low").default_f64()
}
fn d_curve_mid() -> f64 {
    pk(PARAMS, "curve_mid").default_f64()
}
fn d_curve_high() -> f64 {
    pk(PARAMS, "curve_high").default_f64()
}
fn d_link_mode() -> i32 {
    pk(PARAMS, "link_mode").default_i32()
}
fn d_transient_guard() -> bool {
    pk(PARAMS, "transient_guard").default_bool()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            enabled: d_enabled(),
            threshold_db: d_threshold_db(),
            frequency_hz: d_frequency_hz(),
            strength: d_strength(),
            spectral_mode: d_spectral_mode(),
            learn_noise: d_learn_noise(),
            use_captured_profile: d_use_captured_profile(),
            clear_profile: d_clear_profile(),
            curve_low: d_curve_low(),
            curve_mid: d_curve_mid(),
            curve_high: d_curve_high(),
            link_mode: d_link_mode(),
            transient_guard: d_transient_guard(),
        }
    }
}

impl PluginParamDef for Params {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 2;
    const PLUGIN_TYPE_KEY: &'static str = "hiss_reducer";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.enabled { 1.0 } else { 0.0 }),
            1 => Some(self.threshold_db),
            2 => Some(self.frequency_hz),
            3 => Some(self.strength),
            4 => Some(if self.spectral_mode { 1.0 } else { 0.0 }),
            5 => Some(if self.learn_noise { 1.0 } else { 0.0 }),
            6 => Some(if self.use_captured_profile { 1.0 } else { 0.0 }),
            7 => Some(0.0),
            8 => Some(self.curve_low),
            9 => Some(self.curve_mid),
            10 => Some(self.curve_high),
            11 => Some(self.link_mode as f64),
            12 => Some(if self.transient_guard { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.enabled = PARAMS[0].clamp_f64(value) > 0.5,
            1 => self.threshold_db = PARAMS[1].clamp_f64(value),
            2 => self.frequency_hz = PARAMS[2].clamp_f64(value),
            3 => self.strength = PARAMS[3].clamp_f64(value),
            4 => self.spectral_mode = PARAMS[4].clamp_f64(value) > 0.5,
            // Triggers store UI state here but never fire; the live plugin
            // fires them via named set_parameter only.
            5 => self.learn_noise = PARAMS[5].clamp_f64(value) > 0.5,
            6 => self.use_captured_profile = PARAMS[6].clamp_f64(value) > 0.5,
            7 => {}
            8 => self.curve_low = PARAMS[8].clamp_f64(value),
            9 => self.curve_mid = PARAMS[9].clamp_f64(value),
            10 => self.curve_high = PARAMS[10].clamp_f64(value),
            11 => self.link_mode = PARAMS[11].clamp_f64(value) as i32,
            12 => self.transient_guard = PARAMS[12].clamp_f64(value) > 0.5,
            _ => {}
        }
    }
}
