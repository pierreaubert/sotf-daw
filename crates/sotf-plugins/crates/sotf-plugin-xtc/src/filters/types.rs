// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;

/// Pre-computed HRTF transfer functions for the XTC plant matrix.
///
/// When a SOFA/HRTF file is loaded, these frequency-domain transfer functions
/// replace the Woodworth analytical model for computing the crosstalk matrix C(f).
#[derive(Clone)]
pub(crate) struct HrtfTransferFunctions {
    /// Speaker L -> Left ear (ipsilateral)
    pub h_ll: Vec<Complex<f32>>,
    /// Speaker R -> Left ear (contralateral)
    pub h_lr: Vec<Complex<f32>>,
    /// Speaker L -> Right ear (contralateral)
    pub h_rl: Vec<Complex<f32>>,
    /// Speaker R -> Right ear (ipsilateral)
    pub h_rr: Vec<Complex<f32>>,
    /// Common causal SOFA offset represented by the source impulse responses.
    pub delay_rebase_seconds: f64,
}

/// Cached geometry values to avoid repeated computation in the hot loop.
///
/// Optimization 3: Pre-compute all geometry-dependent values that don't change
/// per frequency bin, avoiding redundant sqrt/trig operations.
pub(crate) struct GeometryCache {
    pub freq_per_bin: f32,
    pub sources: [SourceGeometry; 2],
    pub is_symmetric: bool,
}

/// One source's continuous antipodal-ear geometry, prepared off the audio thread.
pub(crate) struct SourceGeometry {
    pub near_angle: f32,
    pub ear_angles: [f32; 2],
    pub amplitudes: [f32; 2],
    pub delays: [f32; 2],
}
