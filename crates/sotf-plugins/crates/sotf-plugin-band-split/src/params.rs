//! Band Split plugin parameter definitions — single source of truth.
//!
//! This file owns:
//! - Parameter specs (PARAMS array)
//! - UI layout (LAYOUT)
//! - Serializable state (Params struct with serde defaults)
//! - Index<->field mapping (PluginParamDef impl)
//!
//! Adding a parameter: add to PARAMS, add field to Params, add match arms.
//! Nothing else needs to change.

use crate::types::BandSplitRecombinationMode;
use serde::{Deserialize, Serialize};
use sotf_host::param_specs::{ParamSpec, find_by_key as pk};
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

// ============================================================================
// Parameter Specifications
// ============================================================================

pub const CROSSOVER_TYPES: &[&str] = &["LR24", "LR48"];
pub const BAND_COUNTS: &[&str] = &["2 Bands", "3 Bands", "4 Bands"];

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::float(
        "Frequency",
        "frequency",
        300.0,
        20.0,
        20000.0,
        10.0,
        "Hz",
        "General",
    )
    .doc("Log-smoothed crossover frequency with bounded-rate coefficient updates"),
    ParamSpec::choice("Type", "type", 0, CROSSOVER_TYPES, "General")
        .structural()
        .doc("Filter slope (24 or 48 dB/oct)"),
    ParamSpec::choice(
        "Recombination",
        "recombination_mode",
        1,
        BandSplitRecombinationMode::LABELS,
        "General",
    )
    .structural()
    .doc("Phase-compensated routing matches later crossover delays; legacy mode preserves prior cascades"),
    ParamSpec::choice("Bands", "num_bands", 0, BAND_COUNTS, "Routing")
        .structural()
        .doc("Number of independently routed output bands"),
    ParamSpec::float("Frequency 2", "frequency_2", 1_200.0, 20.0, 20_000.0, 10.0, "Hz", "Routing")
        .doc("Second cutoff; available in three- and four-band routing"),
    ParamSpec::float("Frequency 3", "frequency_3", 4_800.0, 20.0, 20_000.0, 10.0, "Hz", "Routing")
        .doc("Third cutoff; shown when four bands are selected"),
];

// ============================================================================
// UI Layout
// ============================================================================

pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[],
    main: &[
        ControlGroup::new(
            "CROSSOVER",
            "CROSSOVER",
            &[
                ControlSpec::knob(0),
                ControlSpec::button_set(1, CROSSOVER_TYPES),
                ControlSpec::button_set(2, BandSplitRecombinationMode::LABELS),
            ],
        )
        .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible()),
        ControlGroup::new(
            "ROUTING",
            "ROUTING",
            &[ControlSpec::button_set(3, BAND_COUNTS)],
        )
        .with_layout(GroupLayoutHints::inferred().priority(0.9).keep_visible()),
        // One cutoff is active with two bands. Show the second cutoff only
        // when the selected layout actually routes three or four bands.
        // ParamCondition intentionally supports choice equality rather than
        // ranges, so the mutually exclusive groups represent the same control
        // for the two valid multiband choices.
        ControlGroup::new(
            "CUTOFF_2_FOR_THREE_BANDS",
            "CUTOFF 2",
            &[ControlSpec::knob(4)],
        )
        .visible_when(ParamCondition::choice(3, 1))
        .with_layout(GroupLayoutHints::inferred().priority(0.7)),
        ControlGroup::new(
            "CUTOFF_2_FOR_FOUR_BANDS",
            "CUTOFF 2",
            &[ControlSpec::knob(4)],
        )
        .visible_when(ParamCondition::choice(3, 2))
        .with_layout(GroupLayoutHints::inferred().priority(0.7)),
        ControlGroup::new("CUTOFF 3", "CUTOFF 3", &[ControlSpec::knob(5)])
            .visible_when(ParamCondition::choice(3, 2))
            .with_layout(GroupLayoutHints::inferred().priority(0.6)),
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

/// Band Split plugin parameters.
///
/// All serde defaults are derived from PARAMS — adding a field here with
/// the correct default function is enough to support old presets that
/// don't have the new field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    #[serde(default = "d_frequency")]
    pub frequency: f64,
    #[serde(
        rename = "type",
        alias = "crossover_type",
        default = "d_crossover_type"
    )]
    pub crossover_type: String,
    /// Missing saved mode remains on the legacy cascade. New UI creation uses
    /// `Default::default()` and selects phase compensation explicitly.
    #[serde(default)]
    pub recombination_mode: BandSplitRecombinationMode,
    #[serde(default = "d_num_bands")]
    pub num_bands: usize,
    #[serde(default = "d_frequency_2")]
    pub frequency_2: f64,
    #[serde(default = "d_frequency_3")]
    pub frequency_3: f64,
}

fn d_frequency() -> f64 {
    pk(PARAMS, "frequency").default_f64()
}
fn d_crossover_type() -> String {
    CROSSOVER_TYPES[0].to_string()
}
fn d_num_bands() -> usize {
    2
}
fn d_frequency_2() -> f64 {
    1_200.0
}
fn d_frequency_3() -> f64 {
    4_800.0
}

impl Default for Params {
    fn default() -> Self {
        Self {
            frequency: d_frequency(),
            crossover_type: d_crossover_type(),
            recombination_mode: BandSplitRecombinationMode::PhaseCompensated,
            num_bands: d_num_bands(),
            frequency_2: d_frequency_2(),
            frequency_3: d_frequency_3(),
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
    const PLUGIN_TYPE_KEY: &'static str = "band_split";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.frequency),
            1 => {
                let idx = CROSSOVER_TYPES
                    .iter()
                    .position(|&t| t == self.crossover_type)
                    .unwrap_or(0);
                Some(idx as f64)
            }
            2 => Some(self.recombination_mode.index() as f64),
            3 => Some(self.num_bands.saturating_sub(2).min(2) as f64),
            4 => Some(self.frequency_2),
            5 => Some(self.frequency_3),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.frequency = PARAMS[0].clamp_f64(value),
            1 => {
                let idx = value.round().clamp(0.0, (CROSSOVER_TYPES.len() - 1) as f64) as usize;
                self.crossover_type = CROSSOVER_TYPES[idx].to_string();
            }
            2 => {
                self.recombination_mode =
                    BandSplitRecombinationMode::from_index(value.round().clamp(0.0, 1.0) as usize);
            }
            3 => self.num_bands = value.round().clamp(0.0, 2.0) as usize + 2,
            4 => self.frequency_2 = PARAMS[4].clamp_f64(value),
            5 => self.frequency_3 = PARAMS[5].clamp_f64(value),
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
        assert_eq!(original.crossover_type, restored.crossover_type);
        assert_eq!(original.recombination_mode, restored.recombination_mode);
        assert_eq!(original.num_bands, restored.num_bands);
        assert_eq!(original.frequency_2, restored.frequency_2);
        assert_eq!(original.frequency_3, restored.frequency_3);
    }

    #[test]
    fn deserialize_empty_json_uses_defaults() {
        let p: Params = serde_json::from_str("{}").unwrap();
        assert_eq!(p.frequency, pk(PARAMS, "frequency").default_f64());
        assert_eq!(p.crossover_type, CROSSOVER_TYPES[0]);
        assert_eq!(
            p.recombination_mode,
            BandSplitRecombinationMode::LegacyCascade
        );
    }

    #[test]
    fn update_modes_match_runtime_contract() {
        use sotf_host::param_specs::UpdateMode;

        assert_eq!(PARAMS[0].update_mode, UpdateMode::Realtime);
        assert_eq!(PARAMS[1].update_mode, UpdateMode::Structural);
        assert_eq!(PARAMS[2].update_mode, UpdateMode::Structural);
        assert_eq!(PARAMS[3].update_mode, UpdateMode::Structural);
        assert_eq!(PARAMS[4].update_mode, UpdateMode::Realtime);
        assert_eq!(PARAMS[5].update_mode, UpdateMode::Realtime);
    }

    #[test]
    fn newly_created_ui_state_and_layout_expose_phase_and_multiband_controls() {
        let params = Params::default();
        assert_eq!(
            params.recombination_mode,
            BandSplitRecombinationMode::PhaseCompensated
        );
        assert_eq!(params.num_bands, 2);
        assert_eq!(PARAMS.len(), 6);
        assert_eq!(LAYOUT.main.len(), 5);
        let frequency_2_groups = LAYOUT
            .main
            .iter()
            .filter(|group| group.controls[0].param_index == 4)
            .collect::<Vec<_>>();
        assert_eq!(frequency_2_groups.len(), 2);
        assert!(frequency_2_groups.iter().all(|group| {
            matches!(
                group.visible_when,
                Some(ParamCondition::Choice { param_index: 3, .. })
            )
        }));
        for (bands_choice, expected_visibility) in [(0.0, false), (1.0, true), (2.0, true)] {
            let values = [300.0, 0.0, 1.0, bands_choice, 1_200.0, 4_800.0];
            assert_eq!(
                frequency_2_groups
                    .iter()
                    .any(|group| group.is_visible(&values)),
                expected_visibility,
                "second cutoff visibility for band choice {bands_choice}"
            );
        }
    }

    #[test]
    fn strict_state_rejects_unknown_fields() {
        assert!(serde_json::from_str::<Params>(r#"{"frequency":300.0,"unknown":1}"#).is_err());
    }
}
