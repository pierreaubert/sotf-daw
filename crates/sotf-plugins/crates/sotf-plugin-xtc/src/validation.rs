//! XTC analytical diagnostics and externally supplied transfer-matrix evaluation.
//!
//! Analytical predictions share the filter design's model and are not measured
//! acoustic validation. They retain signed degradation, use the selected FFT bin,
//! and expose intended gain alongside raw and normalized crosstalk change.
//! [`evaluate_cancellation`] accepts independently obtained plant/filter matrices.
//! Their acquisition, timing, calibration and provenance remain the caller's
//! responsibility. [`run_validation`] compares model values with historical
//! reference targets; inspect every result and [`ValidationReport::all_passed`].
//! A passing report-structure or stability test does not certify every acoustic
//! metric, and the default high-frequency ILD discrepancy remains unresolved.

mod matrix;
mod measure;
mod misc;
mod reference;
mod run;
#[cfg(test)]
mod tests;
mod types;
mod validation_report;
mod validation_result;

pub use matrix::{CancellationMetrics, ComplexTransferMatrix, evaluate_cancellation};
pub use measure::*;
pub use misc::*;
pub use reference::*;
pub use run::*;
pub use types::*;
pub use validation_report::*;
pub use validation_result::*;

#[cfg(test)]
mod diagnostic_audit_tests;
