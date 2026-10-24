//! Structural policy and diagnostics for explicit learned speech protection.

use crate::GateMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// SHA-256 of the verified fixed-shape Silero v6.2.1 model.
pub const SILERO_MODEL_SHA256: &str =
    "7d47a98acce5f4b688a7d541ee0d9061b64171fdb39581cbcf29fe72bfe57c51";

/// Explicit pretrained detector selection; load models on a control thread.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SpeechProtectionModel {
    /// Existing independent-window RNNoise detector.
    #[default]
    Rnnoise,
    /// Recurrent Silero detector, requiring the `silero-speech` build feature.
    Silero {
        /// Local copy of the pinned fixed-shape model.
        model_path: PathBuf,
        /// Expected digest; only the verified model identity is supported.
        sha256: String,
    },
}

/// Optional downward-gate speech protection; it never adds attenuation or boost.
///
/// Source-clock age includes a 100 ms analysis allowance. RNNoise uses a complete
/// independent window; Silero retains recurrent history and waits for four
/// contiguous 32 ms hops after restart. Scores are not calibrated probabilities.
/// Missing, uncertain, stale or unavailable observations retain the classical
/// gate, with its release smoothing after a previous protection decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeechProtectionConfig {
    /// Detector selection; omitted settings preserve RNNoise behavior.
    pub model: SpeechProtectionModel,
    /// Minimum raw detector speech output required to protect the program.
    pub min_probability: f32,
    /// Maximum endpoint age plus the 100 ms analysis allowance.
    pub max_age_ms: f32,
    /// Physical program channel analyzed; no phase-canceling downmix is used.
    pub source_channel: usize,
}

impl Default for SpeechProtectionConfig {
    fn default() -> Self {
        Self {
            model: SpeechProtectionModel::default(),
            min_probability: 0.8,
            max_age_ms: 150.0,
            source_channel: 0,
        }
    }
}

impl SpeechProtectionConfig {
    /// Validate finite confidence and bounded source-age policy.
    ///
    /// # Errors
    /// Rejects invalid confidence or a deadline shorter than the observation.
    pub fn validate(&self) -> Result<(), String> {
        if let SpeechProtectionModel::Silero { model_path, sha256 } = &self.model
            && (model_path.as_os_str().is_empty() || sha256 != SILERO_MODEL_SHA256)
        {
            return Err("Silero speech requires a path and the pinned model SHA-256".into());
        }
        if !self.min_probability.is_finite()
            || !(0.5..=1.0).contains(&self.min_probability)
            || !self.max_age_ms.is_finite()
            || !(120.0..=1000.0).contains(&self.max_age_ms)
        {
            return Err("Invalid Gate speech-protection confidence or age policy".into());
        }
        Ok(())
    }
}

/// Model-specific speech protection diagnostics, separate from the gate latch.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpeechProtectionData {
    /// True after sufficient contiguous model input since the most recent restart.
    pub warmup_complete: bool,
    /// Model frames since the most recent restart; RNNoise uses ten per window.
    pub model_frames: u64,
    /// Worker-detected stream gaps since reset; RNNoise windows use zero.
    pub stream_restarts: u64,
    /// Latest raw model score; not a statement of classification correctness.
    pub model_score: Option<f32>,
    /// Endpoint source-clock age plus 100 ms; recurrent history can be older.
    pub source_age_ms: Option<f32>,
    /// True while the latest observation passes confidence and age checks.
    pub observation_usable: bool,
    /// Smoothed fraction of attenuation removed, between zero and one.
    pub protection_mix: f32,
    /// Submitted source windows or streaming packets since reset.
    pub submitted_windows: u64,
    /// Windows or packets rejected by worker backpressure since reset.
    pub dropped_windows: u64,
    /// Distinct completed observations read since reset.
    pub observed_windows: u64,
}

pub(crate) fn validate_mode(
    config: Option<&SpeechProtectionConfig>,
    channels: usize,
    mode: GateMode,
    linked: bool,
) -> Result<(), String> {
    if let Some(config) = config {
        config.validate()?;
        #[cfg(not(feature = "silero-speech"))]
        if matches!(config.model, SpeechProtectionModel::Silero { .. }) {
            return Err("Silero speech protection requires the silero-speech build feature".into());
        }
        if config.source_channel >= channels || mode != GateMode::Downward || !linked {
            return Err(
                "Gate speech protection requires a valid source channel and linked Downward mode"
                    .into(),
            );
        }
        #[cfg(not(feature = "neural-speech"))]
        return Err("Gate speech protection requires the neural-speech build feature".into());
    }
    Ok(())
}

/// Remove attenuation without changing the independently computed baseline.
pub(crate) fn protected_gain(classical: f32, protection: f32) -> f32 {
    (classical + protection * (1.0 - classical)).clamp(classical, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_cannot_deepen_attenuation_or_boost_and_zero_is_exact() {
        for step in 0..=100 {
            let baseline = step as f32 / 100.0;
            assert_eq!(protected_gain(baseline, 0.0), baseline);
            for strength in 0..=100 {
                let result = protected_gain(baseline, strength as f32 / 100.0);
                assert!(result >= baseline && result <= 1.0);
            }
        }
    }
}
