use super::default::default_crossover_type;
use super::default::default_frequency;
use super::default::default_num_bands;
use crate::params::CROSSOVER_TYPES;
use serde::{Deserialize, Serialize};
use sotf_host::define_choice_string_deserializer;

define_choice_string_deserializer!(deserialize_crossover_type, CROSSOVER_TYPES);

/// Determines whether multiband outputs preserve the existing independent
/// cascades or compensate each intermediate band through the later crossover
/// all-pass sections before it is routed onward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandSplitRecombinationMode {
    /// Preserve the historical multiband cascade and its phase response.
    #[default]
    LegacyCascade,
    /// Match the phase delay of later crossover stages before routing bands.
    PhaseCompensated,
}

impl BandSplitRecombinationMode {
    pub const LABELS: &'static [&'static str] = &["Legacy Cascade", "Phase Compensated"];

    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::PhaseCompensated,
            _ => Self::LegacyCascade,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::LegacyCascade => 0,
            Self::PhaseCompensated => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandSplitPluginParams {
    /// Crossover frequencies. Length determines the number of bands (len + 1).
    /// For backwards compatibility, a single frequency creates 2 bands.
    #[serde(default)]
    pub frequencies: Vec<f64>,

    /// New typed engine cutoff vector. `Some` takes precedence over both the
    /// legacy public vector and the scalar/count compatibility fields, even
    /// when empty; invalid explicit input must fail rather than fall back.
    #[serde(default)]
    pub explicit_frequencies: Option<Vec<f64>>,

    /// Inactive cutoff values are retained for the static host parameter IDs.
    #[serde(default)]
    pub frequency_2: Option<f64>,
    #[serde(default)]
    pub frequency_3: Option<f64>,

    /// Legacy single-frequency field (used when `frequencies` is empty).
    #[serde(default = "default_frequency")]
    pub frequency: f64,

    /// Number of bands (2-4). Used when `frequencies` is empty.
    #[serde(default = "default_num_bands")]
    pub num_bands: usize,

    /// Recombination behavior. Missing state stays on the historical cascade.
    #[serde(default)]
    pub recombination_mode: BandSplitRecombinationMode,

    #[serde(
        rename = "type",
        alias = "crossover_type",
        default = "default_crossover_type",
        deserialize_with = "deserialize_crossover_type"
    )]
    pub crossover_type: String,
}
