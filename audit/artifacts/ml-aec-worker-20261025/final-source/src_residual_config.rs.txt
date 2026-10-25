//! Explicit structural configuration for experimental learned residual proposals.

// Rust guideline compliant 2026-02-21
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Verified initial model identity; later models require independent validation.
pub const RESIDUAL_MODEL_SHA256: &str =
    "2c39c23a5015cecbde5ea950381f80c18907558e897ce2a93ad4df8f72b42752";

/// Opt-in residual proposal policy, prepared outside the audio callback.
///
/// The existing classical post-filter remains active. Learned proposals can add
/// bounded attenuation; they cannot relax its cuts or override near-end protection.
/// Missing builds/files/models/results retain classical behavior. Model loading,
/// worker construction and destruction belong on a control thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidualSuppressionConfig {
    /// Local JSON weights, read with a 256 KiB bound during initialization.
    pub model_path: PathBuf,
    /// Exact supported model identity, not a hash inferred from the input file.
    pub sha256: String,
    /// Maximum source-clock age since the observation endpoint, in milliseconds.
    ///
    /// This excludes the 256-sample observation interval and 50 ms causal EMA
    /// history. A shorter deadline may make every realtime result unavailable.
    #[serde(default = "default_age")]
    pub max_age_ms: f32,
    /// Maximum additional spectral attenuation beyond the classical post-filter.
    #[serde(default = "default_extra")]
    pub max_extra_attenuation_db: f32,
}

fn default_age() -> f32 {
    40.0
}
fn default_extra() -> f32 {
    6.0
}

impl ResidualSuppressionConfig {
    /// Validate the pinned identity and finite bounded structural policy.
    ///
    /// # Errors
    /// Rejects an empty path, unsupported identity, invalid age or extra cut.
    pub fn validate(&self) -> Result<(), String> {
        if self.model_path.as_os_str().is_empty()
            || self.sha256 != RESIDUAL_MODEL_SHA256
            || !self.max_age_ms.is_finite()
            || !(1.0..=100.0).contains(&self.max_age_ms)
            || !self.max_extra_attenuation_db.is_finite()
            || !(0.0..=12.0).contains(&self.max_extra_attenuation_db)
        {
            return Err(
                "Invalid AEC residual model identity, path, age or attenuation bound".into(),
            );
        }
        Ok(())
    }
}

/// Availability of the explicit learned stage; classical processing stays usable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualAvailability {
    /// No learned stage was requested.
    #[default]
    Disabled,
    /// This build lacks the optional model/worker feature.
    FeatureUnavailable,
    /// Preparation failed; inspect the control-thread load error.
    ModelUnavailable,
    /// Prepared successfully, including warmup and freshness checks.
    Prepared,
}

/// Copy-only diagnostics; neither raw features nor owned model buffers escape.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ResidualSuppressionData {
    /// Preparation status; prepared does not imply an accepted observation.
    pub availability: ResidualAvailability,
    /// True only for explicitly prepared deterministic offline execution.
    pub offline: bool,
    /// Completed native blocks since reset/source discontinuity.
    pub native_blocks: u64,
    /// Successfully enqueued or synchronously inferred blocks.
    pub submitted_blocks: u64,
    /// Full-queue rejections; the affected block retains classical processing.
    pub dropped_blocks: u64,
    /// Unique valid observation endpoints accepted since reset.
    pub accepted_observations: u64,
    /// Blocks where at least one additional cut was applied.
    pub applied_blocks: u64,
    /// Live blocks retaining the exact classical spectral output.
    pub fallback_blocks: u64,
    /// Latest accepted endpoint age in source samples, if one is available.
    pub observation_age_samples: Option<u64>,
    /// Current causal warmup has completed; this is not model confidence.
    pub warmup_complete: bool,
}
