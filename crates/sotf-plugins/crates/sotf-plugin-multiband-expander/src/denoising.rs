//! Compatibility exports for the shared experimental restoration evaluator.

// Rust guideline compliant 2026-02-21
pub(crate) use sotf_restoration_mask::DenoisingState;
#[doc(inline)]
pub use sotf_restoration_mask::{DenoisingCounters, DenoisingMask, features};
