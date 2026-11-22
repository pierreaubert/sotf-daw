pub use config::*;

mod bass;
mod config;
mod decorrelation;
mod detection;
mod drain;
mod fft;
#[cfg(all(test, debug_assertions))]
mod fft_scratch_tests;
mod frequency_domain;
mod gain_clock;
mod height;
mod hr_processing;
#[cfg(feature = "onnx")]
mod ml_features;
#[cfg(feature = "onnx")]
mod ml_inference;
mod output;
mod panning;
pub mod params;
mod process;
mod setup;
#[cfg(feature = "qa")]
pub mod source_mask;
#[cfg(test)]
#[path = "lib/stream_boundary_tests.rs"]
mod stream_boundary_tests;
mod surround_bass;
mod surround_processing;
mod surround_routing;
mod table_clock;
#[cfg(test)]
mod test;

#[path = "lib/misc.rs"]
mod misc;
#[path = "lib/types.rs"]
mod types;
#[path = "lib/upmixer_plugin.rs"]
mod upmixer_plugin;

pub use types::*;
pub use upmixer_plugin::*;

#[cfg(test)]
#[path = "lib/autogain_reference_tests.rs"]
mod autogain_reference_tests;
