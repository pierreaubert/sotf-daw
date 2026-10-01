#![allow(clippy::duplicate_mod)]
pub mod params;

#[path = "lib/default.rs"]
mod default;
#[cfg(test)]
#[path = "lib/drain_tests.rs"]
mod drain_tests;
#[cfg(test)]
#[path = "lib/dynamic_tests.rs"]
mod dynamic_tests;
#[path = "lib/eq_band.rs"]
mod eq_band;
#[path = "lib/linear_phase_eq_plugin.rs"]
mod linear_phase_eq_plugin;
#[path = "lib/misc.rs"]
mod misc;
#[path = "lib/ordered.rs"]
mod ordered;
#[cfg(test)]
#[path = "lib/placement_tests.rs"]
mod placement_tests;
#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
#[path = "lib/types.rs"]
mod types;

pub use linear_phase_eq_plugin::*;
pub use types::*;
