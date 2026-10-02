//! Speech-denoiser model registry and selection type.
//!
//! This module names the inference models the plugin may use: the bundled
//! full-quality RNNoise voice model plus two staged legacy alternates whose
//! weights are parsed and validated by the checked `.rnnn` loader. The
//! registry is append-only so future models add labels without renumbering
//! index 0. Selection validates against these labels off the audio
//! callback, and the plugin keeps running the previously accepted model
//! whenever adoption of a new identity fails.

// Rust guideline compliant 2026-02-21

use plugins_denoiser::rnnoise::RnnoiseModelId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sotf_host::define_choice_string_deserializer;

/// Stable model identities in parameter-choice order.
///
/// Append-only: never rename, remove, or reorder entries. Index 0 is the
/// bundled full-quality RNNoise voice model (48 kHz, 22 suppression bands)
/// served by the shared `RnnoiseBackend`.
///
/// Indices 1-2 are real staged weights from
/// GregorR/rnnoise-models@3eee541 (see
/// `plugins-denoiser/models/legacy-rnnoise-nu/` and its source manifest):
/// index 1 serves `leavened-quisling-2018-08-31/lq.rnnn` (voice in a noisy
/// recording environment, sha256 `2782bbb3…`), index 2 serves
/// `somnolent-hogwash-2018-09-01/sh.rnnn` (speech in recording noise,
/// sha256 `de1392ba…`). Suite descriptions are provenance, not quality
/// claims; both files use `.rnnn` v1 with 42 features, 22 gain bands, and
/// Tanh VAD/denoise GRUs honored by the loader.
pub const MODEL_LABELS: &[&str] = &["RNNoise Full", "RNNoise Legacy LQ", "RNNoise Legacy SH"];

/// Default model index into [`MODEL_LABELS`].
pub const DEFAULT_MODEL_INDEX: usize = 0;

define_choice_string_deserializer!(deserialize_model_label, MODEL_LABELS);

/// Selects the inference model for denoising.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SpeechDenoiserModel {
    /// Bundled full-quality RNNoise voice model at 48 kHz.
    #[default]
    RnnoiseFull,
    /// Staged legacy `lq.rnnn` weights (2018-08-31 suite).
    RnnoiseLegacyLq,
    /// Staged legacy `sh.rnnn` weights (2018-09-01 suite).
    RnnoiseLegacySh,
}

impl SpeechDenoiserModel {
    /// Returns the stable parameter-choice index.
    pub const fn index(self) -> usize {
        match self {
            Self::RnnoiseFull => 0,
            Self::RnnoiseLegacyLq => 1,
            Self::RnnoiseLegacySh => 2,
        }
    }

    /// Returns the stable persisted label.
    pub fn label(self) -> &'static str {
        MODEL_LABELS[self.index()]
    }

    /// Returns the backend weight identity for this selection.
    pub fn backend_id(self) -> RnnoiseModelId {
        match self {
            Self::RnnoiseFull => RnnoiseModelId::BundledFull,
            Self::RnnoiseLegacyLq => RnnoiseModelId::LegacyLq,
            Self::RnnoiseLegacySh => RnnoiseModelId::LegacySh,
        }
    }

    /// Returns a model for valid choice indices, or `None` otherwise.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::RnnoiseFull),
            1 => Some(Self::RnnoiseLegacyLq),
            2 => Some(Self::RnnoiseLegacySh),
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
    fn registry_accepts_all_entries_and_rejects_unknown() {
        use SpeechDenoiserModel::{RnnoiseFull, RnnoiseLegacyLq, RnnoiseLegacySh};
        assert_eq!(SpeechDenoiserModel::default(), RnnoiseFull);
        assert_eq!(RnnoiseFull.index(), DEFAULT_MODEL_INDEX);
        assert_eq!(SpeechDenoiserModel::from_index(0), Some(RnnoiseFull));
        assert_eq!(SpeechDenoiserModel::from_index(1), Some(RnnoiseLegacyLq));
        assert_eq!(SpeechDenoiserModel::from_index(2), Some(RnnoiseLegacySh));
        assert_eq!(SpeechDenoiserModel::from_index(3), None);
        assert_eq!(SpeechDenoiserModel::from_label("RNNoise Full"), Some(RnnoiseFull));
        assert_eq!(
            SpeechDenoiserModel::from_label("RNNoise Legacy LQ"),
            Some(RnnoiseLegacyLq)
        );
        assert_eq!(
            SpeechDenoiserModel::from_label("RNNoise Legacy SH"),
            Some(RnnoiseLegacySh)
        );
        assert_eq!(
            SpeechDenoiserModel::from_label("rnnoise legacy lq"),
            Some(RnnoiseLegacyLq)
        );
        assert_eq!(SpeechDenoiserModel::from_label("RNNoise Light"), None);
        assert_eq!(SpeechDenoiserModel::from_label(""), None);
        assert_eq!(RnnoiseFull.backend_id(), RnnoiseModelId::BundledFull);
        assert_eq!(RnnoiseLegacyLq.backend_id(), RnnoiseModelId::LegacyLq);
        assert_eq!(RnnoiseLegacySh.backend_id(), RnnoiseModelId::LegacySh);
    }

    #[test]
    fn serde_roundtrips_every_label_and_accepts_indices() {
        use SpeechDenoiserModel::{RnnoiseFull, RnnoiseLegacyLq, RnnoiseLegacySh};
        for (model, label) in [
            (RnnoiseFull, "\"RNNoise Full\""),
            (RnnoiseLegacyLq, "\"RNNoise Legacy LQ\""),
            (RnnoiseLegacySh, "\"RNNoise Legacy SH\""),
        ] {
            assert_eq!(serde_json::to_string(&model).unwrap(), label);
            assert_eq!(serde_json::from_str::<SpeechDenoiserModel>(label).unwrap(), model);
        }
        for (accepted, expected) in [
            ("\"rnnoise full\"", RnnoiseFull),
            ("\"rnnoise legacy sh\"", RnnoiseLegacySh),
            ("0", RnnoiseFull),
            ("1", RnnoiseLegacyLq),
            ("2", RnnoiseLegacySh),
            ("0.0", RnnoiseFull),
        ] {
            assert_eq!(
                serde_json::from_str::<SpeechDenoiserModel>(accepted).unwrap(),
                expected,
                "{accepted} must select a model"
            );
        }
        for rejected in ["\"RNNoise Light\"", "3", "-1", "0.5", "true", "null"] {
            assert!(
                serde_json::from_str::<SpeechDenoiserModel>(rejected).is_err(),
                "{rejected} must not select a model"
            );
        }
    }
}
