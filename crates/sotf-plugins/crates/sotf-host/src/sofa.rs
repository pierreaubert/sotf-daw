//! Compatibility re-exports for SOFA/HRTF loading.
//!
//! The dependency types remain available unchanged. [`load_sofa`] additionally
//! materializes exact integer SOFA delays before control-side DSP preparation.

pub use sofa_reader::{CoordinateSystem, HrtfData, SofaFile, SourcePosition};

mod load;
#[doc(inline)]
pub use load::{LoadedSofa, load_sofa};
