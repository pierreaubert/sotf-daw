//! Private, platform-independent ownership of authenticated record leftovers.
//! The caller holds the transport read-commit guard for all identity/copy calls.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReaderIdentity {
    pub(crate) sample_rate: u32,
    pub(crate) channel_count: u32,
    pub(crate) buffer_frames: u32,
    pub(crate) encrypted: bool,
    pub(crate) key_fingerprint: [u8; 8],
}

impl ReaderIdentity {
    pub(crate) fn valid(self) -> bool {
        self.sample_rate != 0 && self.channel_count != 0 && self.buffer_frames != 0
    }
}

#[derive(Default)]
pub(crate) struct StagedPlaintext {
    samples: Vec<f32>,
    offset: usize,
    identity: Option<ReaderIdentity>,
}

impl StagedPlaintext {
    /// Setup only. Reads retain this capacity and never grow it.
    pub(crate) fn new(sample_capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(sample_capacity),
            ..Self::default()
        }
    }

    /// Also call before every key reload attempt and on transport replacement.
    pub(crate) fn invalidate(&mut self) {
        self.samples.clear();
        self.offset = 0;
        self.identity = None;
    }

    pub(crate) fn observe(&mut self, identity: ReaderIdentity) {
        if !identity.valid() || !identity.encrypted || self.identity != Some(identity) {
            self.invalidate();
        }
    }

    pub(crate) fn remaining_frames(&self, identity: ReaderIdentity) -> usize {
        if !identity.valid() || !identity.encrypted || self.identity != Some(identity) {
            return 0;
        }
        (self.samples.len() - self.offset) / identity.channel_count as usize
    }

    /// Copy complete frames only. Preserve destination samples beyond the result.
    pub(crate) fn copy_into(&mut self, identity: ReaderIdentity, output: &mut [f32]) -> usize {
        self.observe(identity);
        if self.identity != Some(identity) {
            return 0;
        }
        let channels = identity.channel_count as usize;
        let count = (self.samples.len() - self.offset).min(output.len()) / channels * channels;
        output[..count].copy_from_slice(&self.samples[self.offset..self.offset + count]);
        self.offset += count;
        if self.offset == self.samples.len() {
            self.invalidate();
        }
        count
    }

    /// Store a complete authenticated suffix. Reject oversized/misaligned input.
    pub(crate) fn stage(&mut self, identity: ReaderIdentity, samples: &[f32]) -> bool {
        self.invalidate();
        if !identity.valid()
            || !identity.encrypted
            || !samples
                .len()
                .is_multiple_of(identity.channel_count as usize)
            || samples.len() > self.samples.capacity()
        {
            return false;
        }
        self.samples.extend_from_slice(samples);
        self.identity = Some(identity);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{ReaderIdentity, StagedPlaintext};

    fn stereo() -> ReaderIdentity {
        ReaderIdentity {
            sample_rate: 48_000,
            channel_count: 2,
            buffer_frames: 512,
            encrypted: true,
            key_fingerprint: [0x31; 8],
        }
    }

    #[test]
    fn exact_resume_preserves_bits_and_destination_tail() {
        let identity = stereo();
        let samples = [0.0, -0.0, f32::from_bits(0x7fc00123), 3.0, 4.0, 5.0];
        let mut staged = StagedPlaintext::new(6);
        assert!(staged.stage(identity, &samples));
        let mut head = [99.0; 3];
        assert_eq!(staged.copy_into(identity, &mut head), 2);
        assert_eq!(head[0].to_bits(), samples[0].to_bits());
        assert_eq!(head[1].to_bits(), samples[1].to_bits());
        assert_eq!(head[2], 99.0);
        assert_eq!(staged.remaining_frames(identity), 2);
        let mut tail = [99.0; 7];
        assert_eq!(staged.copy_into(identity, &mut tail), 4);
        for (actual, expected) in tail[..4].iter().zip(&samples[2..]) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        assert_eq!(&tail[4..], &[99.0; 3]);
        assert_eq!(staged.remaining_frames(identity), 0);
    }

    #[test]
    fn stereo_leftovers_cannot_become_three_channel_frames() {
        let identity = stereo();
        let mut staged = StagedPlaintext::new(8);
        assert!(staged.stage(identity, &[1., 2., 3., 4., 5., 6., 7., 8.]));
        assert_eq!(staged.copy_into(identity, &mut [0.; 2]), 2);
        let next = ReaderIdentity {
            channel_count: 3,
            ..identity
        };
        assert_eq!(staged.remaining_frames(next), 0);
        let mut output = [77.; 6];
        assert_eq!(staged.copy_into(next, &mut output), 0);
        assert_eq!(output, [77.; 6]);
        // Once observed, the old identity cannot resurrect its staged samples.
        assert_eq!(staged.copy_into(identity, &mut output), 0);
    }

    #[test]
    fn every_identity_field_invalidates_leftovers() {
        let identity = stereo();
        for next in [
            ReaderIdentity {
                sample_rate: 96_000,
                ..identity
            },
            ReaderIdentity {
                channel_count: 1,
                ..identity
            },
            ReaderIdentity {
                buffer_frames: 128,
                ..identity
            },
            ReaderIdentity {
                encrypted: false,
                ..identity
            },
            ReaderIdentity {
                key_fingerprint: [0x32; 8],
                ..identity
            },
        ] {
            let mut staged = StagedPlaintext::new(4);
            assert!(staged.stage(identity, &[1.; 4]));
            assert_eq!(staged.remaining_frames(next), 0);
            staged.observe(next);
            assert_eq!(staged.remaining_frames(identity), 0);
        }
    }

    #[test]
    fn owner_invalidation_discards_same_identity_reload_or_reconnect() {
        let identity = stereo();
        let mut staged = StagedPlaintext::new(4);
        for _ in 0..3 {
            assert!(staged.stage(identity, &[1.; 4]));
            staged.invalidate();
            assert_eq!(staged.remaining_frames(identity), 0);
            assert_eq!(staged.copy_into(identity, &mut [0.; 4]), 0);
        }
    }

    #[test]
    fn invalid_geometry_plaintext_and_unaligned_records_fail_closed() {
        let identity = stereo();
        let mut staged = StagedPlaintext::new(4);
        for invalid in [
            ReaderIdentity {
                sample_rate: 0,
                ..identity
            },
            ReaderIdentity {
                channel_count: 0,
                ..identity
            },
            ReaderIdentity {
                buffer_frames: 0,
                ..identity
            },
            ReaderIdentity {
                encrypted: false,
                ..identity
            },
        ] {
            assert!(staged.stage(identity, &[1.; 4]));
            assert!(!staged.stage(invalid, &[2.; 4]));
            assert_eq!(staged.remaining_frames(identity), 0);
        }
        assert!(!staged.stage(identity, &[1.; 3]));
        assert_eq!(staged.remaining_frames(identity), 0);
    }

    #[test]
    fn capacity_failure_invalidates_without_growing() {
        let identity = stereo();
        let mut staged = StagedPlaintext::new(4);
        let pointer = staged.samples.as_ptr();
        let capacity = staged.samples.capacity();
        assert!(staged.stage(identity, &[1.; 4]));
        assert!(!staged.stage(identity, &[2.; 6]));
        assert_eq!(staged.remaining_frames(identity), 0);
        assert_eq!(staged.samples.capacity(), capacity);
        assert_eq!(staged.samples.as_ptr(), pointer);
    }

    #[test]
    fn repeated_stage_consume_and_invalidate_reuse_allocation() {
        let identity = stereo();
        let mut staged = StagedPlaintext::new(8);
        let pointer = staged.samples.as_ptr();
        let capacity = staged.samples.capacity();
        for _ in 0..1000 {
            assert!(staged.stage(identity, &[1.; 8]));
            assert_eq!(staged.copy_into(identity, &mut [0.; 2]), 2);
            staged.invalidate();
            assert_eq!(staged.samples.capacity(), capacity);
            assert_eq!(staged.samples.as_ptr(), pointer);
        }
    }

    #[test]
    fn read_smaller_than_one_frame_keeps_complete_cached_frame() {
        let identity = stereo();
        let mut staged = StagedPlaintext::new(2);
        assert!(staged.stage(identity, &[1., 2.]));
        assert_eq!(staged.copy_into(identity, &mut [99.]), 0);
        assert_eq!(staged.remaining_frames(identity), 1);
        assert_eq!(staged.copy_into(identity, &mut [0.; 2]), 2);
    }
}
