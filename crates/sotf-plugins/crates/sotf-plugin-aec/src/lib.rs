pub mod params;
mod pbfdaf;
mod post_filter;
pub mod residual_config;
#[cfg(any(feature = "qa", feature = "neural-residual"))]
pub mod residual_features;
#[cfg(feature = "neural-residual")]
pub mod residual_model;
#[cfg(feature = "neural-residual")]
mod residual_stage;
mod two_path;

#[path = "lib/aec_plugin.rs"]
mod aec_plugin;
#[path = "lib/misc.rs"]
mod misc;
#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;

pub use aec_plugin::*;
pub use params::Params as AecPluginParams;

#[cfg(test)]
#[path = "lib/constructor_tests.rs"]
mod constructor_tests;
