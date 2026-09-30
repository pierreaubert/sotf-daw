use crate::params::CROSSOVER_TYPES;
use serde::{Deserialize, Serialize};
use sotf_host::define_choice_string_deserializer;

define_choice_string_deserializer!(deserialize_crossover_type, CROSSOVER_TYPES);

/// Selects which crossover configuration is active.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossoverTopology {
    /// Use the global frequency, output mode, and extra frequencies.
    #[default]
    Bands,
    /// Use one independent crossover configuration for each input channel.
    PerChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossoverPluginParams {
    #[serde(rename = "type", deserialize_with = "deserialize_crossover_type")]
    pub crossover_type: String,
    pub frequency: f64,
    pub output: String,
    /// Explicitly selects the active topology; `None` preserves legacy inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<CrossoverTopology>,
    /// Additional crossover frequencies for 3-way or 4-way mode.
    /// When provided, creates a multi-way crossover. The primary `frequency`
    /// becomes the first crossover point.
    #[serde(default)]
    pub extra_frequencies: Vec<f64>,
    /// FIR taps for linear-phase crossover mode. Even values are rounded up.
    #[serde(default)]
    pub fir_taps: Option<usize>,
    /// Per-channel crossover frequencies in Hz. When non-empty, switches the
    /// plugin into per-channel mode (one independent IIR crossover per
    /// channel) and the scalar `frequency` / `output` fields are ignored.
    #[serde(default)]
    pub channel_frequencies_hz: Vec<f32>,
    /// Per-channel modes: lowpass, highpass, mute, or passthrough.
    ///
    /// An omitted list retains the legacy scalar-output fallback. A supplied
    /// list in explicit per-channel topology must match the input width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_modes: Option<Vec<String>>,
}
