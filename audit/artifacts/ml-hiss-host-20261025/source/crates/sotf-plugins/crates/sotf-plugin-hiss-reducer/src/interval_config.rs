//! Explicit structural configuration and preparation diagnostics for interval review.

// Rust guideline compliant 2026-02-21
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Explicit local model selection; omission preserves classical Hiss processing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntervalReviewConfig {
    /// Caller-selected local model; no model is downloaded or discovered.
    pub model_path: PathBuf,
    /// Required lowercase SHA256 of the exact model bytes.
    pub sha256: String,
    /// Recording chain identity required to match the model context.
    pub capture_setup_id: String,
    /// Maximum source frames after an analyzed interval's endpoint.
    pub maximum_endpoint_age_frames: u64,
}

impl IntervalReviewConfig {
    /// Checks bounded explicit identity and source-age settings.
    ///
    /// # Errors
    /// Rejects an empty path, invalid digest/setup identifier or age above ten seconds.
    pub fn validate(&self, sample_rate: f64) -> Result<(), String> {
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || self.model_path.as_os_str().is_empty()
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
            || !(1..=128).contains(&self.capture_setup_id.chars().count())
            || self.maximum_endpoint_age_frames as f64 > sample_rate * 10.0
        {
            return Err("Invalid explicit Hiss interval review identity or age".into());
        }
        Ok(())
    }

    #[cfg(feature = "interval-proposals")]
    pub(crate) fn load(
        &self,
        rate: f64,
        channels: usize,
        cutoff: f32,
    ) -> Result<crate::interval_proposals::IntervalModel, String> {
        use crate::interval_proposals::{IntervalContext, IntervalModel};
        use std::io::Read;
        if !rate.is_finite() || rate.fract() != 0.0 || !(44100.0..=192000.0).contains(&rate) {
            return Err("Hiss interval review requires an integer rate from44.1 to192kHz".into());
        }
        let context = IntervalContext {
            sample_rate: rate as usize,
            channels,
            frequency_hz: f64::from(cutoff),
            capture_setup_id: self.capture_setup_id.clone(),
            noise_definition: "unwanted_stationary_hiss".into(),
        };
        context.validate().map_err(|error| error.to_string())?;
        // Open once and cap the read at1MiB plus one sentinel byte. The same
        // bytes are hashed and parsed without a metadata/read race.
        let file = std::fs::File::open(&self.model_path)
            .map_err(|error| format!("Hiss interval model unavailable: {error}"))?;
        let mut raw = Vec::new();
        file.take(1_048_577)
            .read_to_end(&mut raw)
            .map_err(|error| format!("Hiss interval model read failed: {error}"))?;
        IntervalModel::from_bytes(&raw, &self.sha256, &context).map_err(|error| error.to_string())
    }
}

/// Control-thread preparation outcome exposed through the existing profile snapshot.
#[derive(Debug, Clone)]
pub enum IntervalReviewPreparation {
    /// No review configuration was requested.
    Disabled,
    /// A configured model could not be prepared; classical audio remains operational.
    Unavailable {
        /// Preparation diagnostic, stored on the control thread.
        reason: String,
    },
    /// Prepared read-only diagnostics, including results after the final callback.
    #[cfg(feature = "interval-proposals")]
    Ready(std::sync::Arc<crate::interval_worker::IntervalReviewReader>),
}
