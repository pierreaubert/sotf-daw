use rustfft::num_complex::Complex;
use sotf_host::sofa::SofaFile;
use std::sync::Arc;

pub(super) struct BinauralState {
    pub(super) hrtf_filters_freq: Vec<Vec<Complex<f32>>>,
    // N-point preparation cache; audio spectra already contain this EQ.
    pub(super) design_diffuse_field_eq: Option<[Vec<Complex<f32>>; 2]>,
    pub(super) _hrtf_data: Option<Arc<SofaFile>>,
}
