//! Typed dynamics modes with stable choice indices and backward-compatible JSON.

// Rust guideline compliant 2026-02-21
use serde::{Deserialize, Deserializer, Serialize};
use sotf_host::define_choice_string_deserializer;

/// Available gate and expansion modes, in parameter-choice order.
pub const MODES: &[&str] = &["Downward", "Upward", "Duck"];

define_choice_string_deserializer!(deserialize_mode_label, MODES);

/// Selects below-threshold attenuation, above-threshold boost, or sidechain ducking.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum GateMode {
    /// Attenuate levels below threshold using the original gate behavior.
    #[default]
    Downward,
    /// Boost levels above threshold, bounded by the maximum boost control.
    Upward,
    /// Attenuate program audio when its detector exceeds threshold.
    Duck,
}

impl GateMode {
    /// Returns the stable parameter-choice index.
    pub const fn index(self) -> usize {
        match self {
            Self::Downward => 0,
            Self::Upward => 1,
            Self::Duck => 2,
        }
    }

    /// Returns a mode for valid choice indices, or `None` otherwise.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Downward),
            1 => Some(Self::Upward),
            2 => Some(Self::Duck),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for GateMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let label = deserialize_mode_label(deserializer)?;
        MODES
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(&label))
            .and_then(Self::from_index)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown Gate mode: {label}")))
    }
}
