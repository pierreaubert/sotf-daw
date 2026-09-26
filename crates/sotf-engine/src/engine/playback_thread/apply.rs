use super::playback_state::PlaybackState;
use std::sync::atomic::Ordering;

/// Apply volume and mute to f32 scratch buffer without clipping the float path.
///
/// Test-only: production integer paths go through `apply_volume_clamp`.
#[cfg(test)]
#[inline(always)]
pub(super) fn apply_volume(
    scratch: &mut [f32],
    state: &PlaybackState,
    channels: usize,
    sample_rate: u32,
) {
    apply_volume_ramp_only(scratch, state, channels, sample_rate);

    accumulate_output_meter(scratch, state);
}

/// Apply volume/mute and clamp for integer hardware formats.
///
/// Metering and clamping share one pass: the meter observes the same
/// post-volume, pre-clamp values `accumulate_output_meter` would see.
#[inline(always)]
pub(in crate::engine) fn apply_volume_clamp(
    scratch: &mut [f32],
    state: &PlaybackState,
    channels: usize,
    sample_rate: u32,
) {
    apply_volume_ramp_only(scratch, state, channels, sample_rate);
    accumulate_output_meter_and_clamp(scratch, state);
}

#[inline(always)]
fn apply_volume_ramp_only(
    scratch: &mut [f32],
    state: &PlaybackState,
    channels: usize,
    sample_rate: u32,
) {
    let volume = f32::from_bits(state.volume.load(Ordering::Relaxed));
    let muted = state.muted.load(Ordering::Relaxed);
    let target = if muted { 0.0 } else { volume };
    state
        .volume_ramp
        .apply(scratch, channels, sample_rate, target);
}

/// Accumulate post-volume, pre-clamp output telemetry without allocation or locking.
///
/// Test-only companion to `apply_volume`.
#[cfg(test)]
#[inline(always)]
fn accumulate_output_meter(samples: &[f32], state: &PlaybackState) {
    let (peak, clipped) = observe_output_meter(samples);

    // Positive finite f32 values preserve numeric ordering in their bit pattern.
    state
        .output_peak_bits
        .fetch_max(peak.to_bits(), Ordering::Relaxed);
    if clipped > 0 {
        state
            .clipped_sample_count
            .fetch_add(clipped, Ordering::Relaxed);
    }
}

/// Meter and clamp in a single pass for integer hardware formats.
///
/// Observes exactly what `accumulate_output_meter` would see, then clamps in
/// place, halving scratch-buffer traffic versus two separate passes.
#[inline(always)]
fn accumulate_output_meter_and_clamp(samples: &mut [f32], state: &PlaybackState) {
    let mut peak = 0.0f32;
    let mut clipped = 0u64;

    for sample in samples.iter_mut() {
        let (sample_peak, sample_clipped) = observe_sample(*sample);
        peak = peak.max(sample_peak);
        clipped += sample_clipped;
        *sample = sample.clamp(-1.0, 1.0);
    }

    state
        .output_peak_bits
        .fetch_max(peak.to_bits(), Ordering::Relaxed);
    if clipped > 0 {
        state
            .clipped_sample_count
            .fetch_add(clipped, Ordering::Relaxed);
    }
}

/// Fold one post-volume sample into `(peak, clipped)` telemetry.
#[inline(always)]
fn observe_sample(sample: f32) -> (f32, u64) {
    let magnitude = sample.abs();
    if magnitude.is_finite() {
        (magnitude, u64::from(magnitude > 1.0))
    } else {
        (0.0, 1)
    }
}

#[cfg(test)]
#[inline(always)]
fn observe_output_meter(samples: &[f32]) -> (f32, u64) {
    let mut peak = 0.0f32;
    let mut clipped = 0u64;

    for &sample in samples {
        let (sample_peak, sample_clipped) = observe_sample(sample);
        peak = peak.max(sample_peak);
        clipped += sample_clipped;
    }

    (peak, clipped)
}
