//! Limiter plugin parameter definitions — single source of truth.
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

/// Labels accepted by constructor JSON and displayed by the quality selector.
pub const OVERSAMPLING_OPTIONS: &[&str] = &["1x", "2x", "4x"];

sotf_host::define_choice_index_deserializer!(deserialize_oversampling, OVERSAMPLING_OPTIONS);

// ============================================================================
// Parameter Specifications
// ============================================================================

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::float(
        "Threshold",
        "threshold",
        -0.1,
        -20.0,
        0.0,
        0.1,
        "dB",
        "Dynamics",
    )
    .doc("Ceiling level (max output)"),
    ParamSpec::float(
        "Release", "release", 50.0, 10.0, 1000.0, 5.0, "ms", "Timing",
    )
    .doc("Time to return to unity gain"),
    ParamSpec::float(
        "Lookahead",
        "lookahead",
        5.0,
        0.0,
        20.0,
        0.5,
        "ms",
        "Timing",
    )
    .structural()
    .setup()
    .doc("Graph latency / pre-delay for predictive peak catching"),
    ParamSpec::bool_labeled("Soft Knee", "soft", false, "Soft", "Hard", "Dynamics")
        .setup()
        .doc("One-dB gain-computer knee vs hard limiting onset"),
    ParamSpec::bool_labeled("True Peak", "true_peak", false, "On", "Off", "Detection")
        .setup()
        .doc("Rate-appropriate ITU-R BS.1770-compatible inter-sample peak detection"),
    ParamSpec::bool_labeled("ISP Limit", "isp_mode", false, "On", "Off", "Detection")
        .structural()
        .setup()
        .doc("Predictive output ISP correction adds rate-dependent latency; requires hard mode, 100% wet, and enough input lookahead to cover the detector delay"),
    ParamSpec::bool_labeled("Dual Release", "dual_release", false, "On", "Off", "Timing")
        .setup()
        .doc("Fast+slow release envelopes"),
    ParamSpec::float("Mix", "mix", 1.0, 0.0, 1.0, 0.05, "%", "Output")
        .scaled(100.0)
        .output()
        .doc("Dry/wet blend"),
    // --- Phase 3B: SOTA additions ---
    ParamSpec::float("Link", "link_amount", 1.0, 0.0, 1.0, 0.01, "%", "Detection")
        .scaled(100.0)
        .doc("Channel linking: 0%=independent, 100%=linked (all channels see max peak)"),
    ParamSpec::bool_labeled(
        "Feed Forward",
        "feed_forward",
        false,
        "On",
        "Off",
        "Detection",
    )
    .setup()
    .doc("Compatibility control; nonzero lookahead is always predictive"),
    ParamSpec::choice("Oversampling", "oversampling", 0, OVERSAMPLING_OPTIONS, "Quality")
        .structural()
        .setup()
        .doc("Prepared audio-rate oversampling with native output protection; changes latency"),
];

// ============================================================================
// UI Layout
// ============================================================================

/// Limiter: idx 0=threshold, 1=release, 2=lookahead, 3=soft_knee, 4=true_peak, 5=isp_mode, 6=dual_release, 7=mix, 8=link_amount, 9=feed_forward, 10=oversampling
///
/// ui.md Phase 3 pilot: the primary dynamics row (ceiling, release,
/// gain-reduction feedback, output mix) is pinned visible at every width so
/// narrow editors never hide output feedback while preserving threshold.
/// Timing (lookahead) and detector (link) detail discloses by name through
/// the generic overflow surface instead of an anonymous "More".
pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[],
    main: &[
        ControlGroup::new(
            "DYNAMICS",
            "DYNAMICS",
            &[
                ControlSpec::slider(0),         // threshold (ceiling)
                ControlSpec::slider(1),         // release
                ControlSpec::toggle(4),         // true-peak detection (always visible)
                ControlSpec::knob(7),           // mix
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "TIMING",
            "Timing & lookahead",
            &[
                ControlSpec::slider(2), // lookahead
                ControlSpec::toggle(6), // dual_release
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.25)),
        ControlGroup::new(
            "DETECTOR",
            "Knee & detection",
            &[
                ControlSpec::toggle(3),    // soft knee
                ControlSpec::toggle(5),    // ISP mode
                ControlSpec::toggle(9),    // feed-forward
                ControlSpec::slider(8),    // link_amount
                ControlSpec::selector(10), // oversampling
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.35)),
    ],
    output: &[ControlSpec::meter(-20.0, 0.0)], // compact GR feedback above controls
    tabs: &[],
    visualizations: &[VizSlot::TransferCurve {
        position: VizPosition::BelowGroup("DYNAMICS"),
    }],
    column_constraints: &[ColumnConstraint::main(300.0)],
    dynamic_sections: &[],
};

// ============================================================================
// Serializable Parameter State
// ============================================================================

/// Limiter plugin parameters.
///
/// All serde defaults are derived from PARAMS — adding a field here with
/// the correct default function is enough to support old presets that
/// don't have the new field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    #[serde(default = "d_threshold")]
    pub threshold: f64,
    #[serde(default = "d_release")]
    pub release: f64,
    #[serde(default = "d_lookahead")]
    pub lookahead: f64,
    #[serde(default = "d_soft")]
    pub soft: bool,
    #[serde(default = "d_true_peak")]
    pub true_peak: bool,
    #[serde(default = "d_isp_mode")]
    pub isp_mode: bool,
    #[serde(default = "d_dual_release")]
    pub dual_release: bool,
    #[serde(default = "d_mix")]
    pub mix: f64,
    #[serde(default = "d_link_amount")]
    pub link_amount: f64,
    #[serde(default = "d_feed_forward")]
    pub feed_forward: bool,
    /// Choice index: 0 = 1x, 1 = 2x, 2 = 4x.
    #[serde(
        default = "default_oversampling",
        deserialize_with = "deserialize_oversampling"
    )]
    pub oversampling: usize,
}

fn d_link_amount() -> f64 {
    pk(PARAMS, "link_amount").default_f64()
}
fn d_feed_forward() -> bool {
    pk(PARAMS, "feed_forward").default_bool()
}
fn d_threshold() -> f64 {
    pk(PARAMS, "threshold").default_f64()
}
fn d_release() -> f64 {
    pk(PARAMS, "release").default_f64()
}
fn d_lookahead() -> f64 {
    pk(PARAMS, "lookahead").default_f64()
}
fn d_soft() -> bool {
    pk(PARAMS, "soft").default_bool()
}
fn d_true_peak() -> bool {
    pk(PARAMS, "true_peak").default_bool()
}
fn d_isp_mode() -> bool {
    pk(PARAMS, "isp_mode").default_bool()
}
fn d_dual_release() -> bool {
    pk(PARAMS, "dual_release").default_bool()
}
fn d_mix() -> f64 {
    pk(PARAMS, "mix").default_f64()
}

/// Public default helpers used by `LimiterPluginParams` so its serde defaults
/// come from the same `PARAMS` array used by `PluginParamDef`.
pub fn default_threshold_db() -> f32 {
    d_threshold() as f32
}
pub fn default_release_ms() -> f32 {
    d_release() as f32
}
pub fn default_lookahead_ms() -> f32 {
    d_lookahead() as f32
}
pub fn default_soft() -> bool {
    d_soft()
}
pub fn default_true_peak() -> bool {
    d_true_peak()
}
pub fn default_isp_mode() -> bool {
    d_isp_mode()
}
pub fn default_dual_release() -> bool {
    d_dual_release()
}
pub fn default_mix() -> f32 {
    d_mix() as f32
}
pub fn default_link_amount() -> f32 {
    d_link_amount() as f32
}
pub fn default_feed_forward() -> bool {
    d_feed_forward()
}

pub fn default_oversampling() -> usize {
    pk(PARAMS, "oversampling").default_usize()
}

impl Default for Params {
    fn default() -> Self {
        Self {
            threshold: d_threshold(),
            release: d_release(),
            lookahead: d_lookahead(),
            soft: d_soft(),
            true_peak: d_true_peak(),
            isp_mode: d_isp_mode(),
            dual_release: d_dual_release(),
            mix: d_mix(),
            link_amount: d_link_amount(),
            feed_forward: d_feed_forward(),
            oversampling: default_oversampling(),
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
    const PLUGIN_TYPE_KEY: &'static str = "limiter";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.threshold),
            1 => Some(self.release),
            2 => Some(self.lookahead),
            3 => Some(if self.soft { 1.0 } else { 0.0 }),
            4 => Some(if self.true_peak { 1.0 } else { 0.0 }),
            5 => Some(if self.isp_mode { 1.0 } else { 0.0 }),
            6 => Some(if self.dual_release { 1.0 } else { 0.0 }),
            7 => Some(self.mix),
            8 => Some(self.link_amount),
            9 => Some(if self.feed_forward { 1.0 } else { 0.0 }),
            10 => Some(self.oversampling as f64),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.threshold = PARAMS[0].clamp_f64(value),
            1 => self.release = PARAMS[1].clamp_f64(value),
            2 => self.lookahead = PARAMS[2].clamp_f64(value),
            3 => self.soft = PARAMS[3].clamp_f64(value) > 0.5,
            4 => self.true_peak = PARAMS[4].clamp_f64(value) > 0.5,
            5 => self.isp_mode = PARAMS[5].clamp_f64(value) > 0.5,
            6 => self.dual_release = PARAMS[6].clamp_f64(value) > 0.5,
            7 => self.mix = PARAMS[7].clamp_f64(value),
            8 => self.link_amount = PARAMS[8].clamp_f64(value),
            9 => self.feed_forward = PARAMS[9].clamp_f64(value) > 0.5,
            10 => self.oversampling = PARAMS[10].clamp_f64(value).round() as usize,
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
        assert_eq!(original.threshold, restored.threshold);
        assert_eq!(original.release, restored.release);
        assert_eq!(original.lookahead, restored.lookahead);
        assert_eq!(original.soft, restored.soft);
        assert_eq!(original.true_peak, restored.true_peak);
        assert_eq!(original.isp_mode, restored.isp_mode);
        assert_eq!(original.dual_release, restored.dual_release);
        assert_eq!(original.mix, restored.mix);
    }

    #[test]
    fn deserialize_empty_json_uses_defaults() {
        let p: Params = serde_json::from_str("{}").unwrap();
        assert_eq!(p.threshold, pk(PARAMS, "threshold").default_f64());
        assert_eq!(p.release, pk(PARAMS, "release").default_f64());
        assert_eq!(p.lookahead, pk(PARAMS, "lookahead").default_f64());
        assert_eq!(p.soft, pk(PARAMS, "soft").default_bool());
        assert_eq!(p.true_peak, pk(PARAMS, "true_peak").default_bool());
        assert_eq!(p.isp_mode, pk(PARAMS, "isp_mode").default_bool());
        assert_eq!(p.dual_release, pk(PARAMS, "dual_release").default_bool());
        assert_eq!(p.mix, pk(PARAMS, "mix").default_f64());
    }

    #[test]
    fn dynamics_row_is_pinned_and_secondary_groups_disclose_by_name() {
        use sotf_host::layout_solver::solve_control_groups;
        use sotf_host::plugin_layout::GroupOverflow;

        // Semantic priorities: primary dynamics first, detector before timing.
        let dynamics = LAYOUT.main.iter().find(|g| g.id == "DYNAMICS").unwrap();
        let timing = LAYOUT.main.iter().find(|g| g.id == "TIMING").unwrap();
        let detector = LAYOUT.main.iter().find(|g| g.id == "DETECTOR").unwrap();
        assert_eq!(dynamics.layout.overflow, GroupOverflow::KeepVisible);
        assert_eq!(dynamics.layout.collapse_priority, 1.0);
        assert!(detector.layout.collapse_priority > timing.layout.collapse_priority);
        assert!(timing.layout.collapse_priority < dynamics.layout.collapse_priority);

        // Ceiling, release, gain reduction, and mix stay in the pinned group.
        let dynamics_params: Vec<usize> = dynamics
            .controls
            .iter()
            .filter(|c| c.param_index != usize::MAX)
            .map(|c| c.param_index)
            .collect();
        assert!(dynamics_params.contains(&0)); // threshold (ceiling)
        assert!(dynamics_params.contains(&1)); // release
        assert!(dynamics_params.contains(&4)); // true peak
        assert!(!LAYOUT.config.iter().any(|control| control.param_index == 4));
        assert!(dynamics_params.contains(&7)); // mix
        assert!(
            dynamics.controls.iter().any(|c| matches!(
                c.control_type,
                sotf_host::plugin_layout::ControlType::BarMeter { .. }
            )),
            "gain-reduction meter must ride with the primary dynamics row"
        );

        // Narrow editor: primary row stays, secondary groups overflow with
        // non-empty labels for the named disclosure.
        let groups: Vec<_> = LAYOUT.main.iter().collect();
        let solved = solve_control_groups(&groups, 320.0).unwrap();
        assert!(solved.find("DYNAMICS").unwrap().visible());
        let mut overflow: Vec<_> = solved.collapsed_slots().map(|slot| slot.id).collect();
        overflow.sort_unstable();
        assert_eq!(overflow, vec!["DETECTOR", "TIMING"]);
        for slot in solved.collapsed_slots() {
            assert!(
                !slot.label.is_empty(),
                "overflow of {} must be named",
                slot.id
            );
        }

        // Wide editor: everything fits, no overflow.
        let solved = solve_control_groups(&groups, 1000.0).unwrap();
        assert!(solved.collapsed_slots().next().is_none());
    }
}
