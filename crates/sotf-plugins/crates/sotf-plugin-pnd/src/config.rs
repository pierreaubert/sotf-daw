// ============================================================================
// PND Plugin Configuration
// ============================================================================

use crate::params::PARAMS as PD;
use serde::{Deserialize, Serialize};
use sotf_host::param_specs::find_by_key as pk;

/// Structural opt-in for referenced, monophonic neural pitch assistance.
///
/// Models load on the control thread at initialization. The model must implement
/// the `crepe-tiny-v1` contract; its SHA-256 is mandatory. Low-confidence, late,
/// silent, or out-of-range observations leave the classical estimator in charge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeuralPitchConfig {
    /// Local ONNX model path; never fetched during audio processing.
    pub model_path: String,
    /// Expected lowercase SHA-256 of the ONNX bytes.
    pub sha256: String,
    /// Minimum activation confidence, in `(0, 1]`.
    pub min_confidence: f32,
    /// Maximum observation age in milliseconds, including the analysis window.
    pub max_age_ms: f32,
    /// Maximum accepted deviation from the explicit reference, in cents.
    pub max_deviation_cents: f32,
    /// Physical channel containing the monophonic reference source.
    pub source_channel: usize,
}

pub fn default_correction_strength() -> f32 {
    pk(PD, "correction_strength").default_f64() as f32
}

pub fn default_analysis_window_ms() -> f32 {
    pk(PD, "analysis_window_ms").default_f64() as f32
}

pub fn default_drift_smoothing() -> f32 {
    pk(PD, "drift_smoothing").default_f64() as f32
}

pub fn default_multi_channel_analysis() -> bool {
    pk(PD, "multi_channel_analysis").default_bool()
}

pub fn default_confidence_threshold() -> f32 {
    pk(PD, "confidence_threshold").default_f64() as f32
}

pub fn default_reference_frequency_hz() -> f32 {
    pk(PD, "reference_frequency_hz").default_f64() as f32
}

pub fn default_formant_preservation() -> bool {
    pk(PD, "formant_preservation").default_bool()
}

pub fn default_formant_strength() -> f32 {
    pk(PD, "formant_strength").default_f64() as f32
}

pub fn default_phase_vocoder() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PndPluginParams {
    /// Optional structural neural analysis. Legacy presets retain classical analysis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neural_pitch: Option<NeuralPitchConfig>,
    #[serde(default = "default_correction_strength")]
    pub correction_strength: f32,

    #[serde(default = "default_analysis_window_ms")]
    pub analysis_window_ms: f32,

    #[serde(default = "default_drift_smoothing")]
    pub drift_smoothing: f32,

    #[serde(default = "default_multi_channel_analysis")]
    pub multi_channel_analysis: bool,

    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f32,

    #[serde(default = "default_reference_frequency_hz")]
    pub reference_frequency_hz: f32,

    #[serde(default = "default_formant_preservation")]
    pub formant_preservation: bool,

    #[serde(default = "default_formant_strength")]
    pub formant_strength: f32,

    /// Legacy preset compatibility only. Both `false` and `true` select the
    /// duration-preserving engine; new serialization omits this field.
    #[serde(default = "default_phase_vocoder", skip_serializing)]
    pub phase_vocoder: bool,
}

impl Default for PndPluginParams {
    fn default() -> Self {
        Self {
            neural_pitch: None,
            correction_strength: default_correction_strength(),
            analysis_window_ms: default_analysis_window_ms(),
            drift_smoothing: default_drift_smoothing(),
            multi_channel_analysis: default_multi_channel_analysis(),
            confidence_threshold: default_confidence_threshold(),
            reference_frequency_hz: default_reference_frequency_hz(),
            formant_preservation: default_formant_preservation(),
            formant_strength: default_formant_strength(),
            phase_vocoder: default_phase_vocoder(),
        }
    }
}
