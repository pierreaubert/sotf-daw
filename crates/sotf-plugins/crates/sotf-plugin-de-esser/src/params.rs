//! De-Esser plugin parameter definitions — single source of truth.
//!
//! This file owns:
//! - Parameter specs (PARAMS array)
//! - UI layout (LAYOUT)
//! - Serializable state (Params struct with serde defaults)
//! - Index↔field mapping (PluginParamDef impl)
//!
//! Adding a parameter: add to PARAMS, add field to Params, add match arms.
//! Nothing else needs to change.

use serde::{Deserialize, Serialize};
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

// ============================================================================
// Mode Constants
// ============================================================================

pub const MODES: &[&str] = &["Wideband", "Split-Band"];

// ============================================================================
// Parameter Specifications
// ============================================================================

pub const PARAMS: &[ParamSpec] = &[
    // Detection
    ParamSpec::float(
        "Frequency",
        "frequency",
        7000.0,
        2000.0,
        16000.0,
        100.0,
        "Hz",
        "Detection",
    )
    .structural()
    .setup()
    .doc("Center frequency for sibilance detection"),
    ParamSpec::float("Q", "q", 1.5, 0.5, 5.0, 0.1, "", "Detection")
        .structural()
        .setup()
        .doc("Bandwidth of detection filter"),
    // Dynamics
    ParamSpec::float(
        "Threshold",
        "threshold",
        -20.0,
        -60.0,
        0.0,
        0.5,
        "dB",
        "Dynamics",
    )
    .doc("Sibilance detection threshold"),
    ParamSpec::float("Ratio", "ratio", 4.0, 1.0, 20.0, 0.1, ":1", "Dynamics")
        .doc("Compression ratio for sibilance"),
    ParamSpec::float("Attack", "attack", 0.5, 0.1, 10.0, 0.1, "ms", "Dynamics").doc("Attack time"),
    ParamSpec::float(
        "Release", "release", 20.0, 5.0, 200.0, 1.0, "ms", "Dynamics",
    )
    .doc("Release time"),
    // Mode
    ParamSpec::choice("Mode", "mode", 1, MODES, "Mode")
        .structural()
        .setup()
        .doc("Wideband reduces full signal; Split-band only reduces HF"),
    // Output
    ParamSpec::float("Mix", "mix", 1.0, 0.0, 1.0, 0.01, "%", "Output")
        .scaled(100.0)
        .output()
        .doc("Dry/wet mix"),
    // Append controls to preserve the indices used by older hosts and presets.
    ParamSpec::float("Range", "range_db", 60.0, 0.0, 60.0, 0.5, "dB", "Dynamics")
        .doc("Maximum gain reduction; zero disables reduction"),
    ParamSpec::float(
        "Stereo Link",
        "stereo_link",
        0.0,
        0.0,
        1.0,
        0.01,
        "%",
        "Detection",
    )
    .scaled(100.0)
    .doc("Link all channel gains to the strongest reduction to preserve the stereo image"),
];

// ============================================================================
// UI Layout
// ============================================================================

/// De-Esser controls preserve indices 0–7; range and stereo link use 8–9.
pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[
        ControlSpec::selector(6), // mode
    ],
    main: &[
        ControlGroup::new(
            "detection",
            "DETECTION",
            &[
                ControlSpec::slider(0), // frequency
                ControlSpec::slider(1), // q
                ControlSpec::slider(9), // stereo link
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.85)),
        ControlGroup::new(
            "dynamics",
            "DYNAMICS",
            &[
                ControlSpec::slider(2), // threshold
                ControlSpec::slider(3), // ratio
                ControlSpec::slider(4), // attack
                ControlSpec::slider(5), // release
                ControlSpec::slider(8), // range
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "output",
            "OUTPUT",
            &[ControlSpec::meter(-30.0, 0.0), ControlSpec::knob(7)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.9)),
    ],
    output: &[],
    tabs: &[],
    visualizations: &[],
    column_constraints: &[
        ColumnConstraint::config(100.0, 0.5),
        ColumnConstraint::main(300.0),
    ],
    dynamic_sections: &[],
};

// ============================================================================
// Serializable Parameter State
// ============================================================================

/// De-Esser plugin parameters.
///
/// All serde defaults are derived from PARAMS — adding a field here with
/// the correct default function is enough to support old presets that
/// don't have the new field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    #[serde(default = "d_frequency")]
    pub frequency: f64,
    #[serde(default = "d_q")]
    pub q: f64,
    #[serde(default = "d_threshold")]
    pub threshold: f64,
    #[serde(default = "d_ratio")]
    pub ratio: f64,
    #[serde(default = "d_attack")]
    pub attack: f64,
    #[serde(default = "d_release")]
    pub release: f64,
    #[serde(default = "d_mode")]
    pub mode: String,
    #[serde(default = "d_mix")]
    pub mix: f64,
    /// Maximum gain reduction in decibels.
    #[serde(default = "d_range_db")]
    pub range_db: f64,
    /// Channel linking from independent (zero) to fully linked (one).
    #[serde(default = "d_stereo_link")]
    pub stereo_link: f64,
}

fn d_frequency() -> f64 {
    pk(PARAMS, "frequency").default_f64()
}
fn d_q() -> f64 {
    pk(PARAMS, "q").default_f64()
}
fn d_threshold() -> f64 {
    pk(PARAMS, "threshold").default_f64()
}
fn d_ratio() -> f64 {
    pk(PARAMS, "ratio").default_f64()
}
fn d_attack() -> f64 {
    pk(PARAMS, "attack").default_f64()
}
fn d_release() -> f64 {
    pk(PARAMS, "release").default_f64()
}
fn d_mode() -> String {
    MODES[1].to_string()
}
fn d_mix() -> f64 {
    pk(PARAMS, "mix").default_f64()
}
fn d_range_db() -> f64 {
    pk(PARAMS, "range_db").default_f64()
}
fn d_stereo_link() -> f64 {
    pk(PARAMS, "stereo_link").default_f64()
}

/// Public default helpers used by `DeEsserPluginParams` so its serde defaults
/// come from the same `PARAMS` array used by `PluginParamDef`.
pub fn default_frequency() -> f32 {
    d_frequency() as f32
}
pub fn default_q() -> f32 {
    d_q() as f32
}
pub fn default_threshold() -> f32 {
    d_threshold() as f32
}
pub fn default_ratio() -> f32 {
    d_ratio() as f32
}
pub fn default_attack_ms() -> f32 {
    d_attack() as f32
}
pub fn default_release_ms() -> f32 {
    d_release() as f32
}
pub fn default_mode() -> String {
    d_mode()
}
pub fn default_mix() -> f32 {
    d_mix() as f32
}

/// Returns the maximum reduction used by presets without a range control.
pub fn default_range_db() -> f32 {
    d_range_db() as f32
}

/// Returns independent channel processing for presets without a link control.
pub fn default_stereo_link() -> f32 {
    d_stereo_link() as f32
}

impl Default for Params {
    fn default() -> Self {
        Self {
            frequency: d_frequency(),
            q: d_q(),
            threshold: d_threshold(),
            ratio: d_ratio(),
            attack: d_attack(),
            release: d_release(),
            mode: d_mode(),
            mix: d_mix(),
            range_db: d_range_db(),
            stereo_link: d_stereo_link(),
        }
    }
}

// ============================================================================
// PluginParamDef implementation
// ============================================================================

impl PluginParamDef for Params {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 1;
    const PLUGIN_TYPE_KEY: &'static str = "de_esser";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.frequency),
            1 => Some(self.q),
            2 => Some(self.threshold),
            3 => Some(self.ratio),
            4 => Some(self.attack),
            5 => Some(self.release),
            6 => Some(
                MODES
                    .iter()
                    .position(|&m| m.eq_ignore_ascii_case(&self.mode))
                    .unwrap_or(1) as f64,
            ),
            7 => Some(self.mix),
            8 => Some(self.range_db),
            9 => Some(self.stereo_link),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.frequency = PARAMS[0].clamp_f64(value),
            1 => self.q = PARAMS[1].clamp_f64(value),
            2 => self.threshold = PARAMS[2].clamp_f64(value),
            3 => self.ratio = PARAMS[3].clamp_f64(value),
            4 => self.attack = PARAMS[4].clamp_f64(value),
            5 => self.release = PARAMS[5].clamp_f64(value),
            6 => {
                let idx = value as usize;
                if let Some(&label) = MODES.get(idx) {
                    self.mode = label.to_string();
                }
            }
            7 => self.mix = PARAMS[7].clamp_f64(value),
            8 => self.range_db = PARAMS[8].clamp_f64(value),
            9 => self.stereo_link = PARAMS[9].clamp_f64(value),
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
        assert_eq!(original.frequency, restored.frequency);
        assert_eq!(original.q, restored.q);
        assert_eq!(original.threshold, restored.threshold);
        assert_eq!(original.ratio, restored.ratio);
        assert_eq!(original.attack, restored.attack);
        assert_eq!(original.release, restored.release);
        assert_eq!(original.mode, restored.mode);
        assert_eq!(original.mix, restored.mix);
        assert_eq!(original.range_db, restored.range_db);
        assert_eq!(original.stereo_link, restored.stereo_link);
    }

    #[test]
    fn deserialize_empty_json_uses_defaults() {
        let p: Params = serde_json::from_str("{}").unwrap();
        assert_eq!(p.frequency, pk(PARAMS, "frequency").default_f64());
        assert_eq!(p.q, pk(PARAMS, "q").default_f64());
        assert_eq!(p.threshold, pk(PARAMS, "threshold").default_f64());
        assert_eq!(p.ratio, pk(PARAMS, "ratio").default_f64());
        assert_eq!(p.attack, pk(PARAMS, "attack").default_f64());
        assert_eq!(p.release, pk(PARAMS, "release").default_f64());
        assert_eq!(p.mode, MODES[1]);
        assert_eq!(p.mix, pk(PARAMS, "mix").default_f64());
        assert_eq!(p.range_db, pk(PARAMS, "range_db").default_f64());
        assert_eq!(p.stereo_link, pk(PARAMS, "stereo_link").default_f64());
    }
}
