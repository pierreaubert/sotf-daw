//! Bundled speech-denoiser model registry and selection type.
//!
//! This module names the inference models the plugin may use. Only the
//! bundled full-quality RNNoise voice model ships today; the registry is
//! append-only so future models add labels without renumbering index 0.
//! Selection validates against these labels off the audio callback, and the
//! plugin keeps running the previously accepted model whenever adoption of a
//! new identity fails.

// Rust guideline compliant 2026-02-21

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sotf_host::define_choice_string_deserializer;

/// Stable model identities in parameter-choice order.
///
/// Append-only: never rename, remove, or reorder entries. Index 0 is the
/// bundled full-quality RNNoise voice model (48 kHz, 22 suppression bands)
/// served by the shared `RnnoiseBackend`.
pub const MODEL_LABELS: &[&str] = &["RNNoise Full"];

/// Default model index into [`MODEL_LABELS`].
pub const DEFAULT_MODEL_INDEX: usize = 0;

define_choice_string_deserializer!(deserialize_model_label, MODEL_LABELS);

/// Selects the bundled inference model for denoising.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SpeechDenoiserModel {
    /// Bundled full-quality RNNoise voice model at 48 kHz.
    #[default]
    RnnoiseFull,
}

impl SpeechDenoiserModel {
    /// Returns the stable parameter-choice index.
    pub const fn index(self) -> usize {
        match self {
            Self::RnnoiseFull => 0,
        }
    }

    /// Returns the stable persisted label.
    pub fn label(self) -> &'static str {
        MODEL_LABELS[self.index()]
    }

    /// Returns a model for valid choice indices, or `None` otherwise.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::RnnoiseFull),
            _ => None,
        }
    }

    /// Returns a model for known labels, or `None` otherwise.
    ///
    /// Matching is exact first with an ASCII case-insensitive fallback, so
    /// benign UI/preset case drift does not fail adoption.
    pub fn from_label(label: &str) -> Option<Self> {
        sotf_host::param_specs::choice_index_from_label(MODEL_LABELS, label).and_then(Self::from_index)
    }
}

impl Serialize for SpeechDenoiserModel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for SpeechDenoiserModel {
    /// Deserializes a label or choice index.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown labels and out-of-range indices; callers
    /// keep the previously accepted model whenever this fails.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let label = deserialize_model_label(deserializer)?;
        Self::from_label(&label)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown speech denoiser model: {label}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_accepts_label_and_index_and_rejects_unknown() {
        assert_eq!(SpeechDenoiserModel::default().index(), DEFAULT_MODEL_INDEX);
        assert_eq!(SpeechDenoiserModel::from_index(0), Some(SpeechDenoiserModel::RnnoiseFull));
        assert_eq!(SpeechDenoiserModel::from_index(1), None);
        assert_eq!(
            SpeechDenoiserModel::from_label("RNNoise Full"),
            Some(SpeechDenoiserModel::RnnoiseFull)
        );
        assert_eq!(
            SpeechDenoiserModel::from_label("rnnoise full"),
            Some(SpeechDenoiserModel::RnnoiseFull)
        );
        assert_eq!(SpeechDenoiserModel::from_label("RNNoise Light"), None);
        assert_eq!(SpeechDenoiserModel::from_label(""), None);
    }

    #[test]
    fn serde_roundtrips_label_and_accepts_index() {
        let json = serde_json::to_string(&SpeechDenoiserModel::RnnoiseFull).unwrap();
        assert_eq!(json, "\"RNNoise Full\"");
        for accepted in ["\"RNNoise Full\"", "\"rnnoise full\"", "0", "0.0"] {
            assert_eq!(
                serde_json::from_str::<SpeechDenoiserModel>(accepted).unwrap(),
                SpeechDenoiserModel::RnnoiseFull,
            );
        }
        for rejected in ["\"RNNoise Light\"", "1", "-1", "0.5", "true", "null"] {
            assert!(
                serde_json::from_str::<SpeechDenoiserModel>(rejected).is_err(),
                "{rejected} must not select a model"
            );
        }
    }
}
