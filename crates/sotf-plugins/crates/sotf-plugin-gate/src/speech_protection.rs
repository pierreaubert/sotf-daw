//! Structural policy and diagnostics for explicit learned speech protection.

use crate::GateMode;
use serde::{Deserialize, Serialize};

/// Optional downward-gate speech protection; it never adds attenuation or boost.
///
/// A 100 ms causal RNNoise observation is accepted only within the configured
/// source-clock age. Model output is not a calibrated correctness probability.
/// Missing, uncertain, stale or unavailable observations retain the classical
/// gate, with its release smoothing after a previous protection decision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeechProtectionConfig {
    /// Minimum raw RNNoise speech output required to protect the program.
    pub min_probability: f32,
    /// Maximum age including the complete 100 ms source window.
    pub max_age_ms: f32,
    /// Physical program channel analyzed; no phase-canceling downmix is used.
    pub source_channel: usize,
}

impl Default for SpeechProtectionConfig {
    fn default() -> Self {
        Self {
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
    /// Latest raw model score; not a statement of classification correctness.
    pub model_score: Option<f32>,
    /// Current source-clock age, including the 100 ms observation window.
    pub source_age_ms: Option<f32>,
    /// True while the latest observation passes confidence and age checks.
    pub observation_usable: bool,
    /// Smoothed fraction of attenuation removed, between zero and one.
    pub protection_mix: f32,
    /// Submitted source windows since reset.
    pub submitted_windows: u64,
    /// Source windows rejected by bounded worker backpressure since reset.
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
