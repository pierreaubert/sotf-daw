#![cfg(target_os = "macos")]
//! Integration tests for HAL driver audio passthrough
//!
//! These tests verify that audio data passes through the HAL shared memory
//! interface and plugin processing without modification when configured
//! for passthrough (e.g., EQ with zero gain filters).

#[path = "hal_passthrough_tests/misc.rs"]
mod misc;
#[cfg(test)]
#[path = "hal_passthrough_tests/tests.rs"]
mod tests;
#[path = "hal_passthrough_tests/types.rs"]
mod types;
