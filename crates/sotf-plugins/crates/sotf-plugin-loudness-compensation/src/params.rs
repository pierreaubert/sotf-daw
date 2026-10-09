//! Loudness Compensation plugin parameter definitions — single source of truth.
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
// Parameter Specifications
// ============================================================================

/// Mode labels: index 0 = "Manual" (backward compat default), index 1 = "ISO 226", index 2 = "Auto".
pub const MODE_LABELS: &[&str] = &["Manual", "ISO 226", "Auto"];
pub const AUTO_GAIN_POSITION_LABELS: &[&str] = &["Disabled", "Pre", "Post"];

pub const PARAMS: &[ParamSpec] = &[
    // -- indices 0..10: existing parameters (order preserved for backward compat) --
    ParamSpec::float("Low Freq", "low_freq", 100.0, 20.0, 500.0, 5.0, "Hz", "Low")
        .doc("Low shelf center frequency"),
    ParamSpec::float("Low Gain", "low_gain", 6.0, -20.0, 20.0, 0.5, "dB", "Low")
        .doc("Low shelf boost/cut at low volume"),
    // ISO 226:2003: sensitivity drops above ~8 kHz (80-phon contour rises steeply)
    ParamSpec::float(
        "High Freq",
        "high_freq",
        8000.0,
        2000.0,
        20000.0,
        100.0,
        "Hz",
        "High",
    )
    .doc("High shelf center frequency"),
    ParamSpec::float(
        "High Gain",
        "high_gain",
        6.0,
        -20.0,
        20.0,
        0.5,
        "dB",
        "High",
    )
    .doc("High shelf boost/cut at low volume"),
    ParamSpec::bool_param("Mid Enabled", "mid_enabled", true, "Mid")
        .structural()
        .doc("Enable mid-range compensation band"),
    // ISO 226:2003: ear canal resonance creates max sensitivity at ~3.5 kHz
    ParamSpec::float(
        "Mid Freq", "mid_freq", 3500.0, 500.0, 8000.0, 50.0, "Hz", "Mid",
    )
    .doc("Mid peak center frequency"),
    ParamSpec::float("Mid Gain", "mid_gain", 3.0, -20.0, 20.0, 0.5, "dB", "Mid")
        .doc("Mid peak boost/cut at low volume"),
    ParamSpec::float("Mid Q", "mid_q", 0.707, 0.1, 5.0, 0.05, "", "Mid")
        .doc("Mid peak bandwidth (Q factor)"),
    ParamSpec::bool_param("Auto Gain", "auto_gain_enabled", false, "Auto Gain")
        .structural()
        .output()
        .doc("Auto-normalize output level"),
    ParamSpec::float(
        "Max Auto Gain",
        "auto_gain_max_db",
        12.0,
        0.0,
        24.0,
        1.0,
        "dB",
        "Auto Gain",
    )
    .structural()
    .output()
    .doc("Maximum auto gain correction"),
    ParamSpec::float(
        "Smoothing",
        "auto_gain_smoothing_ms",
        100.0,
        1.0,
        1000.0,
        5.0,
        "ms",
        "Auto Gain",
    )
    .structural()
    .output()
    .doc("Auto gain transition time"),
    // -- indices 11..13: new ISO 226 parameters --
    // Default mode_index = 0 = "Manual" for backward compatibility.
    ParamSpec::choice("Mode", "mode", 0, MODE_LABELS, "Compensation")
        .structural()
        .doc("Manual 3-band or ISO 226 automatic contour"),
    ParamSpec::float(
        "Playback Level",
        "playback_level_db",
        70.0,
        40.0,
        90.0,
        1.0,
        "dB SPL",
        "Compensation",
    )
    .doc("Current playback level — compensation adjusts for this level vs reference"),
    ParamSpec::float(
        "Reference Level",
        "reference_level_db",
        83.0,
        60.0,
        100.0,
        1.0,
        "dB SPL",
        "Compensation",
    )
    .doc("Reference listening level (no compensation applied at this level)"),
    // -- index 14: Auto mode parameter --
    ParamSpec::float(
        "Playback Volume",
        "playback_volume_db",
        0.0,
        -80.0,
        0.0,
        0.5,
        "dB",
        "Auto",
    )
    .setup()
    .doc("Engine playback volume (set automatically by the engine)"),
    ParamSpec::choice(
        "Auto Gain",
        "auto_gain_position",
        0,
        AUTO_GAIN_POSITION_LABELS,
        "Auto Gain",
    )
    .structural()
    .output()
    .doc("Canonical AutoGain state and position"),
    ParamSpec::bool_param(
        "Headroom Normalized",
        "headroom_normalized",
        false,
        "Compensation",
    )
    .doc("Attenuate by the realized positive cascade peak; changes the 1 kHz reference"),
    ParamSpec::bool_param("SPL Calibrated", "auto_calibrated", false, "Auto")
        .structural()
        .doc("Reference level is a measured SPL at digital volume 0 dB"),
];

// ============================================================================
// UI Layout
// ============================================================================

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[
        ControlSpec::button_set(11, MODE_LABELS),
        ControlSpec::button_set(15, AUTO_GAIN_POSITION_LABELS),
        ControlSpec::toggle(17),
        ControlSpec::toggle(16),
        ControlSpec::toggle(4).enabled_when(ParamCondition::choice(11, 0)),
        ControlSpec::toggle(8).hide(), // Legacy alias of the canonical position control.
    ],
    main: &[
        ControlGroup::new("reference-level", "", &[ControlSpec::slider(13)])
            .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new("ISO 226", "", &[ControlSpec::slider(12)])
            .visible_when(ParamCondition::choice(11, 1))
            .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new("AUTO", "", &[ControlSpec::label(14)])
            .visible_when(ParamCondition::choice(11, 2))
            .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "TONE",
            "",
            &[
                ControlSpec::slider(1),
                ControlSpec::slider(3),
                ControlSpec::slider(6).enabled_when(ParamCondition::bool(4, true)),
            ],
        )
        .visible_when(ParamCondition::choice(11, 0))
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "TONE FREQUENCIES",
            "TONE FREQUENCIES",
            &[ControlSpec::slider(0), ControlSpec::slider(2)],
        )
        .visible_when(ParamCondition::choice(11, 0))
        .with_layout(GroupLayoutHints::inferred().priority(0.4)),
        ControlGroup::new(
            "MID",
            "MID",
            &[
                ControlSpec::slider(5).enabled_when(ParamCondition::bool(4, true)),
                ControlSpec::slider(7).enabled_when(ParamCondition::bool(4, true)),
            ],
        )
        .visible_when(ParamCondition::choice(11, 0))
        .with_layout(GroupLayoutHints::inferred().priority(0.4)),
        ControlGroup::new(
            "AUTO GAIN",
            "AUTO GAIN",
            &[
                ControlSpec::slider(9).enabled_when(ParamCondition::bool(8, true)),
                ControlSpec::slider(10).enabled_when(ParamCondition::bool(8, true)),
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

// ============================================================================
// Serializable Parameter State
// ============================================================================

/// Loudness Compensation plugin parameters.
///
/// All serde defaults are derived from PARAMS — adding a field here with
/// the correct default function is enough to support old presets that
/// don't have the new field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    #[serde(default = "d_low_freq")]
    pub low_freq: f64,
    #[serde(default = "d_low_gain")]
    pub low_gain: f64,
    #[serde(default = "d_high_freq")]
    pub high_freq: f64,
    #[serde(default = "d_high_gain")]
    pub high_gain: f64,
    #[serde(default = "d_mid_enabled")]
    pub mid_enabled: bool,
    #[serde(default = "d_mid_freq")]
    pub mid_freq: f64,
    #[serde(default = "d_mid_gain")]
    pub mid_gain: f64,
    #[serde(default = "d_mid_q")]
    pub mid_q: f64,
    #[serde(default = "d_auto_gain_enabled")]
    pub auto_gain_enabled: bool,
    #[serde(default = "d_auto_gain_max_db")]
    pub auto_gain_max_db: f64,
    #[serde(default = "d_auto_gain_smoothing_ms")]
    pub auto_gain_smoothing_ms: f64,
    /// 0 = Manual, 1 = ISO 226, 2 = Auto
    #[serde(default = "d_mode")]
    pub mode: usize,
    #[serde(default = "d_playback_level_db")]
    pub playback_level_db: f64,
    #[serde(default = "d_reference_level_db")]
    pub reference_level_db: f64,
    /// Engine playback volume in dB (set externally, used in Auto mode)
    #[serde(default = "d_playback_volume_db")]
    pub playback_volume_db: f64,
    #[serde(default = "d_auto_gain_position")]
    pub auto_gain_position: usize,
    #[serde(default = "d_headroom_normalized")]
    pub headroom_normalized: bool,
    #[serde(default = "d_auto_calibrated")]
    pub auto_calibrated: bool,
}

fn d_low_freq() -> f64 {
    pk(PARAMS, "low_freq").default_f64()
}
fn d_low_gain() -> f64 {
    pk(PARAMS, "low_gain").default_f64()
}
fn d_high_freq() -> f64 {
    pk(PARAMS, "high_freq").default_f64()
}
fn d_high_gain() -> f64 {
    pk(PARAMS, "high_gain").default_f64()
}
fn d_mid_enabled() -> bool {
    pk(PARAMS, "mid_enabled").default_bool()
}
fn d_mid_freq() -> f64 {
    pk(PARAMS, "mid_freq").default_f64()
}
fn d_mid_gain() -> f64 {
    pk(PARAMS, "mid_gain").default_f64()
}
fn d_mid_q() -> f64 {
    pk(PARAMS, "mid_q").default_f64()
}
fn d_auto_gain_enabled() -> bool {
    pk(PARAMS, "auto_gain_enabled").default_bool()
}
fn d_auto_gain_max_db() -> f64 {
    pk(PARAMS, "auto_gain_max_db").default_f64()
}
fn d_auto_gain_smoothing_ms() -> f64 {
    pk(PARAMS, "auto_gain_smoothing_ms").default_f64()
}
fn d_mode() -> usize {
    pk(PARAMS, "mode").default_usize()
}
fn d_playback_level_db() -> f64 {
    pk(PARAMS, "playback_level_db").default_f64()
}
fn d_reference_level_db() -> f64 {
    pk(PARAMS, "reference_level_db").default_f64()
}
fn d_playback_volume_db() -> f64 {
    pk(PARAMS, "playback_volume_db").default_f64()
}
fn d_auto_gain_position() -> usize {
    pk(PARAMS, "auto_gain_position").default_usize()
}
fn d_headroom_normalized() -> bool {
    pk(PARAMS, "headroom_normalized").default_bool()
}
fn d_auto_calibrated() -> bool {
    pk(PARAMS, "auto_calibrated").default_bool()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            low_freq: d_low_freq(),
            low_gain: d_low_gain(),
            high_freq: d_high_freq(),
            high_gain: d_high_gain(),
            mid_enabled: d_mid_enabled(),
            mid_freq: d_mid_freq(),
            mid_gain: d_mid_gain(),
            mid_q: d_mid_q(),
            auto_gain_enabled: d_auto_gain_enabled(),
            auto_gain_max_db: d_auto_gain_max_db(),
            auto_gain_smoothing_ms: d_auto_gain_smoothing_ms(),
            mode: d_mode(),
            playback_level_db: d_playback_level_db(),
            reference_level_db: d_reference_level_db(),
            playback_volume_db: d_playback_volume_db(),
            auto_gain_position: d_auto_gain_position(),
            headroom_normalized: d_headroom_normalized(),
            auto_calibrated: d_auto_calibrated(),
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
    const PLUGIN_TYPE_KEY: &'static str = "loudness_compensation";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.low_freq),
            1 => Some(self.low_gain),
            2 => Some(self.high_freq),
            3 => Some(self.high_gain),
            4 => Some(if self.mid_enabled { 1.0 } else { 0.0 }),
            5 => Some(self.mid_freq),
            6 => Some(self.mid_gain),
            7 => Some(self.mid_q),
            8 => Some(if self.auto_gain_position != 0 {
                1.0
            } else {
                0.0
            }),
            9 => Some(self.auto_gain_max_db),
            10 => Some(self.auto_gain_smoothing_ms),
            11 => Some(self.mode as f64),
            12 => Some(self.playback_level_db),
            13 => Some(self.reference_level_db),
            14 => Some(self.playback_volume_db),
            15 => Some(self.auto_gain_position as f64),
            16 => Some(if self.headroom_normalized { 1.0 } else { 0.0 }),
            17 => Some(if self.auto_calibrated { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.low_freq = PARAMS[0].clamp_f64(value),
            1 => self.low_gain = PARAMS[1].clamp_f64(value),
            2 => self.high_freq = PARAMS[2].clamp_f64(value),
            3 => self.high_gain = PARAMS[3].clamp_f64(value),
            4 => self.mid_enabled = PARAMS[4].clamp_f64(value) > 0.5,
            5 => self.mid_freq = PARAMS[5].clamp_f64(value),
            6 => self.mid_gain = PARAMS[6].clamp_f64(value),
            7 => self.mid_q = PARAMS[7].clamp_f64(value),
            8 => {
                self.auto_gain_enabled = value > 0.5;
                self.auto_gain_position = if self.auto_gain_enabled { 2 } else { 0 };
            }
            9 => self.auto_gain_max_db = PARAMS[9].clamp_f64(value),
            10 => self.auto_gain_smoothing_ms = PARAMS[10].clamp_f64(value),
            11 => self.mode = PARAMS[11].clamp_f64(value) as usize,
            12 => self.playback_level_db = PARAMS[12].clamp_f64(value),
            13 => self.reference_level_db = PARAMS[13].clamp_f64(value),
            14 => self.playback_volume_db = PARAMS[14].clamp_f64(value),
            15 => {
                self.auto_gain_position = value as usize;
                self.auto_gain_enabled = self.auto_gain_position != 0;
            }
            16 => self.headroom_normalized = PARAMS[16].clamp_f64(value) > 0.5,
            17 => self.auto_calibrated = PARAMS[17].clamp_f64(value) > 0.5,
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
    fn each_mode_keeps_its_primary_controls_visible_at_narrow_width() {
        for (mode, required) in [
            (0.0, vec!["reference-level", "TONE"]),
            (1.0, vec!["reference-level", "ISO 226"]),
            (2.0, vec!["reference-level", "AUTO"]),
        ] {
            let mut values: Vec<_> = PARAMS.iter().map(|param| param.default_f64()).collect();
            values[11] = mode;
            let groups: Vec<_> = LAYOUT
                .main
                .iter()
                .filter(|group| group.is_visible(&values))
                .collect();
            let solved = sotf_host::layout_solver::solve_control_groups(&groups, 320.0).unwrap();
            for id in required {
                assert!(
                    solved.find(id).unwrap().visible(),
                    "mode {mode}: {id} collapsed"
                );
            }
        }
    }

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
        assert_eq!(original.low_freq, restored.low_freq);
        assert_eq!(original.low_gain, restored.low_gain);
        assert_eq!(original.high_freq, restored.high_freq);
        assert_eq!(original.high_gain, restored.high_gain);
        assert_eq!(original.mid_enabled, restored.mid_enabled);
        assert_eq!(original.mid_freq, restored.mid_freq);
        assert_eq!(original.mid_gain, restored.mid_gain);
        assert_eq!(original.mid_q, restored.mid_q);
        assert_eq!(original.auto_gain_enabled, restored.auto_gain_enabled);
        assert_eq!(original.auto_gain_max_db, restored.auto_gain_max_db);
        assert_eq!(
            original.auto_gain_smoothing_ms,
            restored.auto_gain_smoothing_ms
        );
        assert_eq!(original.mode, restored.mode);
        assert_eq!(original.playback_level_db, restored.playback_level_db);
        assert_eq!(original.reference_level_db, restored.reference_level_db);
        assert_eq!(original.playback_volume_db, restored.playback_volume_db);
        assert_eq!(original.auto_gain_position, restored.auto_gain_position);
        assert_eq!(original.headroom_normalized, restored.headroom_normalized);
        assert_eq!(original.auto_calibrated, restored.auto_calibrated);
    }

    #[test]
    fn deserialize_empty_json_uses_defaults() {
        let p: Params = serde_json::from_str("{}").unwrap();
        assert_eq!(p.low_freq, pk(PARAMS, "low_freq").default_f64());
        assert_eq!(p.low_gain, pk(PARAMS, "low_gain").default_f64());
        assert_eq!(p.high_freq, pk(PARAMS, "high_freq").default_f64());
        assert_eq!(p.high_gain, pk(PARAMS, "high_gain").default_f64());
        assert_eq!(p.mid_enabled, pk(PARAMS, "mid_enabled").default_bool());
        assert_eq!(p.mid_freq, pk(PARAMS, "mid_freq").default_f64());
        assert_eq!(p.mid_gain, pk(PARAMS, "mid_gain").default_f64());
        assert_eq!(p.mid_q, pk(PARAMS, "mid_q").default_f64());
        assert_eq!(
            p.auto_gain_enabled,
            pk(PARAMS, "auto_gain_enabled").default_bool()
        );
        assert_eq!(
            p.auto_gain_max_db,
            pk(PARAMS, "auto_gain_max_db").default_f64()
        );
        assert_eq!(
            p.auto_gain_smoothing_ms,
            pk(PARAMS, "auto_gain_smoothing_ms").default_f64()
        );
        assert_eq!(p.mode, pk(PARAMS, "mode").default_usize());
        assert_eq!(
            p.playback_level_db,
            pk(PARAMS, "playback_level_db").default_f64()
        );
        assert_eq!(
            p.reference_level_db,
            pk(PARAMS, "reference_level_db").default_f64()
        );
        assert_eq!(
            p.playback_volume_db,
            pk(PARAMS, "playback_volume_db").default_f64()
        );
        assert_eq!(p.auto_gain_position, 0);
        assert!(!p.headroom_normalized);
        assert!(!p.auto_calibrated);
    }

    #[test]
    fn default_mode_is_manual() {
        let p = Params::default();
        assert_eq!(p.mode, 0, "Default mode should be Manual (0)");
    }
}
