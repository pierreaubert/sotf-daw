//! Convert AutoEQ RoomEQ output into native SOTF plugin graphs.

mod build;
mod identity;
mod infer;
mod linear;
mod misc;
mod physical;
mod types;

pub use autoeq::roomeq::DspChainOutput;
pub use build::build_room_eq_plugin_graph_config;
