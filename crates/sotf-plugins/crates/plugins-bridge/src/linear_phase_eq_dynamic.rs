//! Re-export of the shared linear-phase EQ dynamic host wrapper.
//!
//! The authoritative implementation lives in
//! `sotf_plugin_linear_phase_eq::dynamic_host` so the bridge and facade
//! factories share one realtime path without duplicating logic. This module
//! only re-exports the shared types for bridge callers and tests.

// Rust guideline compliant 2026-02-21

#[doc(inline)]
pub use sotf_plugin_linear_phase_eq::dynamic_host::{
    LinearPhaseEqControlHandle, LinearPhaseEqDynamicPlugin,
};
