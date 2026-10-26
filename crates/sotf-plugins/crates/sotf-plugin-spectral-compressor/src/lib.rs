#![allow(clippy::duplicate_mod)]
pub mod params;

#[path = "lib/misc.rs"]
mod misc;
#[path = "lib/spectral_compressor_plugin.rs"]
mod spectral_compressor_plugin;
#[path = "lib/spectral_compressor_plugin_params.rs"]
mod spectral_compressor_plugin_params;
#[path = "lib/stft_state.rs"]
mod stft_state;
#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;

pub use spectral_compressor_plugin::*;
pub use spectral_compressor_plugin_params::*;
/// Shared unadmitted model contract for explicit QA restoration selection.
#[cfg(feature = "qa")]
pub mod restoration {
    #[doc(inline)]
    pub use sotf_restoration_mask::{DenoisingCounters, DenoisingMask, features};
}
