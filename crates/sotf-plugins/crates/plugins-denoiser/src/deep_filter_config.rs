//! Structural configuration for optional DeepFilterNet speech enhancement.

use serde::{Deserialize, Serialize};

/// Explicit local model identity and chronological worker playout budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeepFilterConfig {
    /// Local ONNX archive, loaded only during control-thread preparation.
    pub model_path: String,
    /// Expected lowercase SHA-256 of the complete model archive.
    pub sha256: String,
    /// Additional 10 ms packets reserved for worker turnaround; accepted range 2–16.
    #[serde(default = "default_playout_packets")]
    pub playout_packets: usize,
}

fn default_playout_packets() -> usize {
    4
}

impl DeepFilterConfig {
    /// Validate identity syntax and bounded playout before model loading.
    ///
    /// # Errors
    /// Rejects empty paths, invalid SHA-256, or an unsupported playout budget.
    pub fn validate(&self) -> Result<(), String> {
        if self.model_path.is_empty()
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !(2..=16).contains(&self.playout_packets)
        {
            return Err("Invalid DeepFilter model identity or playout budget".into());
        }
        Ok(())
    }
}
