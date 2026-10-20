use super::reflection_hrtf::ReflectionHrtf;

/// Represents a single reflection path from source to listener
#[derive(Debug, Clone)]
pub struct Reflection {
    /// Delay in samples
    pub delay_samples: usize,
    /// Linear gain (after absorption and distance attenuation)
    pub gain: f32,
    /// Left/right channel multipliers for asymmetric reflections
    pub left_gain: f32,
    pub right_gain: f32,
    /// World/capture-frame DOA azimuth in degrees (zero front, positive left).
    /// Convert to the current head frame before per-reflection HRTF lookup.
    pub azimuth_deg: f32,
    /// DOA elevation in degrees. Used for per-reflection HRTF lookup.
    pub elevation_deg: f32,
    /// Initial prepared broadband HRTF-derived ILD for this direction. Live
    /// head-relative gains belong to the renderer's immutable state. This does
    /// not claim reflection ITD or pinna-spectrum rendering; direct paths use
    /// the full linear HRTF convolution renderer.
    pub hrtf_filter: Option<ReflectionHrtf>,
}
