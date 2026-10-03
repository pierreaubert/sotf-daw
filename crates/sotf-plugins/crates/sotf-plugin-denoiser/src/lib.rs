pub use config::DenoiserPluginParams;
pub use reduction_curve::{DENOISER_CURVE_ANCHOR_HZ, ReductionCurve};

mod config;
mod fft;
mod masking;
mod mcra;
mod multi_resolution;
mod noise_profile;
pub mod params;
mod polyphonic;
mod reduction_curve;
mod spectral_sub;
mod wiener;

#[path = "lib/denoiser_data.rs"]
mod denoiser_data;
#[path = "lib/denoiser_plugin.rs"]
mod denoiser_plugin;
#[path = "lib/misc.rs"]
mod misc;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub use denoiser_data::*;
pub use denoiser_plugin::*;
