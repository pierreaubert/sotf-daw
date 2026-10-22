#![allow(dead_code)]
pub(super) const PARTITION_SIZE: usize = 1024;

pub(super) const FFT_SIZE: usize = PARTITION_SIZE * 2;

/// Nonnegative-frequency bins of the real UPC transform.
pub(super) const FFT_BINS: usize = FFT_SIZE / 2 + 1;
