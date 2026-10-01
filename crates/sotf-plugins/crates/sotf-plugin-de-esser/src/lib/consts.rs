pub(super) const DB_CONVERSION_FACTOR: f32 = 20.0;

pub(super) const EPSILON: f32 = 1e-10;

pub(super) const FIXED_KNEE_DB: f32 = 6.0;

/// Maximum lookahead delay. Sized with the `lookahead_ms` parameter range so
/// every legal setting fits the prepared delay lines at any sample rate.
pub(super) const MAX_LOOKAHEAD_MS: f32 = 20.0;

/// Cap the work and prepared drain scratch for each end-of-stream callback.
/// Matches the gate/crossover bound so shared hosts can size one scratch.
pub(super) const MAX_DRAIN_FRAMES: usize = 256;

/// Linear-phase split-bank length. Shared ecosystem default (crossover,
/// linear-phase EQ); group delay is `(FIR_TAPS - 1) / 2 = 512` samples.
/// Fixed rather than parametric to keep the split contract explicit: the
/// reported latency never depends on a hidden tap count.
pub(super) const FIR_TAPS: usize = sotf_host::fir_crossover::DEFAULT_FIR_CROSSOVER_TAPS;
