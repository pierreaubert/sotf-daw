use serde::{Deserialize, Serialize};

/// Reference path named by an ABX answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbxPath {
    /// Reference A.
    A,
    /// Reference B.
    B,
}

/// Audible reference selected during a blinded trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbxMonitor {
    /// Listen to reference A.
    A,
    /// Listen to reference B.
    B,
    /// Listen to the trial's hidden reference.
    X,
}

/// Trial assignment revealed only after submitting an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbxResult {
    /// Listener's submitted answer.
    pub guess: AbxPath,
    /// Reference assigned to X for the completed trial.
    pub reference: AbxPath,
    /// Whether the listener identified X correctly.
    pub correct: bool,
}

/// Data exposed by the A/B comparison plugin for monitoring
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ABCompareData {
    pub loudness_a_lufs: f64,
    pub loudness_b_lufs: f64,
    pub auto_gain_db: f32,
    pub peak_a: f64,
    pub peak_b: f64,
    /// Crossfade position; redacted to zero throughout an active ABX trial.
    pub current_mix: f32,
    pub bypass_active: bool,
}
