use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const VOLUME_RAMP_MILLIS: u64 = 10;

/// Lock-free callback-owned gain state shared through the playback state.
///
/// Only the hardware callback mutates the ramp in production. Atomics let the
/// state remain behind a shared `Arc` without introducing a callback lock.
pub(super) struct VolumeRampState {
    current_bits: AtomicU32,
    target_bits: AtomicU32,
    step_bits: AtomicU32,
    frames_remaining: AtomicU64,
}

impl VolumeRampState {
    pub(super) fn new(initial_gain: f32) -> Self {
        let gain = sanitize_gain(initial_gain);
        Self {
            current_bits: AtomicU32::new(gain.to_bits()),
            target_bits: AtomicU32::new(gain.to_bits()),
            step_bits: AtomicU32::new(0.0f32.to_bits()),
            frames_remaining: AtomicU64::new(0),
        }
    }

    pub(super) fn snap_to(&self, gain: f32) {
        let gain = sanitize_gain(gain);
        self.current_bits.store(gain.to_bits(), Ordering::Relaxed);
        self.target_bits.store(gain.to_bits(), Ordering::Relaxed);
        self.step_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.frames_remaining.store(0, Ordering::Relaxed);
    }

    /// Apply a sample-rate-independent linear ramp, advancing once per frame
    /// so all channels in an interleaved frame receive exactly the same gain.
    #[inline(always)]
    pub(super) fn apply(
        &self,
        samples: &mut [f32],
        channels: usize,
        sample_rate: u32,
        target_gain: f32,
    ) {
        let channels = channels.max(1);
        let target = sanitize_gain(target_gain);
        let target_bits = target.to_bits();
        let mut current = f32::from_bits(self.current_bits.load(Ordering::Relaxed));
        let mut remaining = self.frames_remaining.load(Ordering::Relaxed);
        let mut step = f32::from_bits(self.step_bits.load(Ordering::Relaxed));

        if self.target_bits.load(Ordering::Relaxed) != target_bits {
            remaining = ramp_frames(sample_rate);
            step = (target - current) / remaining as f32;
            self.target_bits.store(target_bits, Ordering::Relaxed);
        }

        // Split the ramp head (gain advances per frame) from the steady tail
        // (constant gain). The tail is a plain multiply loop the optimizer can
        // vectorize; it dominates once the 10 ms ramp has settled, which is
        // the common case when the gain is static.
        let total_frames = samples.len().div_ceil(channels);
        let ramp_chunks = remaining.min(total_frames as u64) as usize;
        let head_len = (ramp_chunks * channels).min(samples.len());
        let (head, tail) = samples.split_at_mut(head_len);

        for frame in head.chunks_mut(channels) {
            if remaining > 0 {
                current += step;
                remaining -= 1;
                if remaining == 0 {
                    current = target;
                    step = 0.0;
                }
            }
            for sample in frame {
                *sample *= current;
            }
        }
        for sample in tail {
            *sample *= current;
        }

        self.current_bits
            .store(current.to_bits(), Ordering::Relaxed);
        self.step_bits.store(step.to_bits(), Ordering::Relaxed);
        self.frames_remaining.store(remaining, Ordering::Relaxed);
    }
}

fn sanitize_gain(gain: f32) -> f32 {
    if gain.is_finite() { gain.max(0.0) } else { 0.0 }
}

fn ramp_frames(sample_rate: u32) -> u64 {
    (u64::from(sample_rate) * VOLUME_RAMP_MILLIS / 1_000).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_reaches_target_after_ten_milliseconds() {
        let ramp = VolumeRampState::new(1.0);
        let mut samples = vec![1.0; 480 * 2];

        ramp.apply(&mut samples, 2, 48_000, 0.0);

        assert!((samples[0] - (479.0 / 480.0)).abs() < 1e-6);
        assert_eq!(samples[958], 0.0);
        assert_eq!(samples[959], 0.0);
    }

    #[test]
    fn ramp_gain_is_identical_across_each_interleaved_frame() {
        let ramp = VolumeRampState::new(0.0);
        let mut samples = vec![1.0; 12];

        ramp.apply(&mut samples, 3, 1_000, 1.0);

        for frame in samples.as_chunks::<3>().0 {
            assert_eq!(frame[0], frame[1]);
            assert_eq!(frame[1], frame[2]);
        }
        assert!(samples[0] < samples[3]);
    }

    #[test]
    fn target_change_continues_from_current_gain_without_a_jump() {
        let ramp = VolumeRampState::new(1.0);
        let mut first = vec![1.0; 240];
        ramp.apply(&mut first, 1, 48_000, 0.0);
        let before_change = first[239];

        let mut second = [1.0];
        ramp.apply(&mut second, 1, 48_000, 1.0);

        assert!(second[0] > before_change);
        assert!(second[0] - before_change < 0.01);
    }
}
