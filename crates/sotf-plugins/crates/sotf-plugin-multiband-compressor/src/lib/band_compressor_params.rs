use crate::params::default_active;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandCompressorParams {
    pub threshold_db: Option<f32>,
    pub ratio: Option<f32>,
    pub attack_ms: Option<f32>,
    pub release_ms: Option<f32>,
    pub knee_db: Option<f32>,
    pub makeup_gain_db: f32,
    #[serde(default)]
    pub auto_makeup: bool,
    #[serde(default)]
    pub measured_auto_makeup: bool,
    #[serde(default = "default_active")]
    pub active: bool,
    pub solo: bool,
    pub bypass: bool,
    /// Overrides the global reduction limit; 120 dB disables the limit.
    #[serde(default)]
    pub range_db: Option<f32>,
    /// Overrides the global hold duration.
    #[serde(default)]
    pub hold_ms: Option<f32>,
}

impl Default for BandCompressorParams {
    fn default() -> Self {
        Self {
            threshold_db: None,
            ratio: None,
            attack_ms: None,
            release_ms: None,
            knee_db: None,
            makeup_gain_db: 0.0,
            auto_makeup: false,
            measured_auto_makeup: false,
            active: default_active(),
            solo: false,
            bypass: false,
            range_db: None,
            hold_ms: None,
        }
    }
}
