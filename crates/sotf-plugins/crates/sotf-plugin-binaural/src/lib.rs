pub use self::config::BinauralDecoderParams;
pub use self::error::BinauralError;
pub use self::room::{Reflection, ReflectionHrtf, RoomModel};

pub mod config;
pub mod error;
pub mod filter;
pub mod hrtf;
pub mod hrtf_database;
pub mod params;
pub mod room;

#[path = "lib/angle_smoothing.rs"]
mod angle_smoothing;
#[path = "lib/binaural_decoder_plugin.rs"]
mod binaural_decoder_plugin;
#[cfg(test)]
#[path = "lib/drain_tests.rs"]
mod drain_tests;
#[cfg(test)]
#[path = "lib/head_cue_tests.rs"]
mod head_cue_tests;
#[path = "lib/head_tracking.rs"]
mod head_tracking;
#[cfg(test)]
#[path = "lib/head_worker_tests.rs"]
mod head_worker_tests;
#[cfg(test)]
#[path = "lib/realtime_tests.rs"]
mod realtime_tests;
#[path = "lib/state.rs"]
mod state;
#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
#[path = "lib/types.rs"]
mod types;

pub use binaural_decoder_plugin::*;

#[cfg(test)]
#[path = "lib/sofa_delay_tests.rs"]
mod sofa_delay_tests;

#[cfg(test)]
#[path = "lib/head_clock_tests.rs"]
mod head_clock_tests;

#[cfg(test)]
#[path = "lib/fade_cue_tests.rs"]
mod fade_cue_tests;

#[cfg(test)]
#[path = "lib/head_storage_tests.rs"]
mod head_storage_tests;

#[cfg(test)]
#[path = "lib/linear_support_tests.rs"]
mod linear_support_tests;

#[cfg(test)]
#[path = "lib/reflection_head_tests.rs"]
mod reflection_head_tests;

#[path = "lib/reflection_tracking.rs"]
mod reflection_tracking;
