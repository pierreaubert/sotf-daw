//! Bounded decoded sample accumulation before FFT resource admission.

// Rust guideline compliant 2026-02-21
use super::{MAX_IR_CHANNELS, MAX_IR_DECODE_BYTES, MAX_IR_SECONDS};
use symphonia::core::audio::{Audio, GenericAudioBufferRef};

pub(super) struct DecodedIr {
    samples: Vec<Vec<f32>>,
    sample_rate: u32,
    frame_limit: usize,
}

impl DecodedIr {
    pub(super) fn new(
        sample_rate: u32,
        channels: usize,
        known_frames: Option<u64>,
    ) -> Result<Self, String> {
        Self::with_byte_limit(sample_rate, channels, known_frames, MAX_IR_DECODE_BYTES)
    }

    fn with_byte_limit(
        sample_rate: u32,
        channels: usize,
        known_frames: Option<u64>,
        byte_limit: usize,
    ) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("IR file is missing a valid sample rate".into());
        }
        if channels == 0 || channels > MAX_IR_CHANNELS {
            return Err(format!(
                "IR has {channels} channels; supported range is 1..={MAX_IR_CHANNELS}"
            ));
        }
        let duration_limit = (sample_rate as usize)
            .checked_mul(MAX_IR_SECONDS)
            .ok_or("IR duration bound is unsupported")?;
        let bytes_per_frame = channels
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or("IR decoded channel size is unsupported")?;
        let frame_limit = duration_limit.min(byte_limit / bytes_per_frame);
        if known_frames.is_some_and(|frames| frames > frame_limit as u64) {
            return Err(Self::limit_error());
        }
        Ok(Self {
            samples: vec![Vec::new(); channels],
            sample_rate,
            frame_limit,
        })
    }

    fn limit_error() -> String {
        "IR decoding exceeds the 30-second duration or temporary sample memory limit".into()
    }

    pub(super) fn append(&mut self, decoded: &GenericAudioBufferRef<'_>) -> Result<(), String> {
        if decoded.spec().rate() != self.sample_rate || decoded.num_planes() != self.samples.len() {
            return Err("IR decoded sample rate or channel count changed".into());
        }
        if !matches!(
            decoded,
            GenericAudioBufferRef::F32(_)
                | GenericAudioBufferRef::S16(_)
                | GenericAudioBufferRef::S24(_)
                | GenericAudioBufferRef::S32(_)
        ) {
            return Err("Unsupported sample format in IR file".into());
        }
        let needed = self.samples[0]
            .len()
            .checked_add(decoded.frames())
            .filter(|&frames| frames <= self.frame_limit)
            .ok_or_else(Self::limit_error)?;
        // Bound capacities as well as lengths. Geometric growth amortizes
        // packet accumulation, while the final step stops exactly at the cap.
        let capacity = needed
            .checked_next_power_of_two()
            .unwrap_or(self.frame_limit)
            .min(self.frame_limit);
        for samples in &mut self.samples {
            if samples.capacity() < needed {
                samples
                    .try_reserve_exact(capacity - samples.len())
                    .map_err(|error| format!("IR decoded sample reservation failed: {error}"))?;
            }
        }
        match decoded {
            GenericAudioBufferRef::F32(buf) => {
                for (ch, samples) in self.samples.iter_mut().enumerate() {
                    samples.extend_from_slice(buf.plane(ch).ok_or("Missing IR channel")?);
                }
            }
            GenericAudioBufferRef::S16(buf) => {
                for (ch, samples) in self.samples.iter_mut().enumerate() {
                    samples.extend(
                        buf.plane(ch)
                            .ok_or("Missing IR channel")?
                            .iter()
                            .map(|&sample| sample as f32 * (1.0 / 32768.0)),
                    );
                }
            }
            GenericAudioBufferRef::S24(buf) => {
                for (ch, samples) in self.samples.iter_mut().enumerate() {
                    samples.extend(
                        buf.plane(ch)
                            .ok_or("Missing IR channel")?
                            .iter()
                            .map(|sample| sample.inner() as f32 * (1.0 / 8388608.0)),
                    );
                }
            }
            GenericAudioBufferRef::S32(buf) => {
                for (ch, samples) in self.samples.iter_mut().enumerate() {
                    samples.extend(
                        buf.plane(ch)
                            .ok_or("Missing IR channel")?
                            .iter()
                            .map(|&sample| sample as f32 * (1.0 / 2147483648.0)),
                    );
                }
            }
            _ => unreachable!("sample format checked before reserving storage"),
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Result<(Vec<Vec<f32>>, u32), String> {
        if self.samples[0].is_empty() {
            return Err("IR file contains no audio data".into());
        }
        Ok((self.samples, self.sample_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::audio::sample::{Sample, i24};
    use symphonia::core::audio::{AudioBuffer, AudioSpec, Channels};

    fn packet<S: Sample>(rate: u32, frame: &[S], frames: usize) -> AudioBuffer<S> {
        let mut buffer = AudioBuffer::new(
            AudioSpec::new(rate, Channels::Discrete(frame.len() as u16)),
            frames,
        );
        buffer.render(Some(frames), frame);
        buffer
    }

    #[test]
    fn metadata_refuses_bad_geometry_and_oversized_lengths_before_samples() {
        for (rate, channels) in [(0, 1), (1, 0), (48_000, 33)] {
            assert!(DecodedIr::new(rate, channels, None).is_err());
        }
        assert!(DecodedIr::new(1, 1, Some(31)).is_err());
        assert!(DecodedIr::new(48_000, 1, Some(u64::MAX)).is_err());
        let memory_frames = MAX_IR_DECODE_BYTES / (32 * std::mem::size_of::<f32>());
        assert!(DecodedIr::new(384_000, 32, Some(memory_frames as u64 + 1)).is_err());
        let exact = DecodedIr::new(384_000, 32, Some(memory_frames as u64)).unwrap();
        assert_eq!(exact.frame_limit, memory_frames);
        assert!(exact.samples.iter().all(|samples| samples.capacity() == 0));
    }

    #[test]
    fn unknown_and_understated_lengths_obey_capacity_and_sample_boundaries() {
        for known_frames in [None, Some(1)] {
            let mut decoded = DecodedIr::with_byte_limit(10, 2, known_frames, 40).unwrap();
            for frames in [2, 1, 2] {
                let buffer = packet(10, &[0.25_f32, -0.5], frames);
                decoded
                    .append(&GenericAudioBufferRef::F32(&buffer))
                    .unwrap();
                assert!(
                    decoded
                        .samples
                        .iter()
                        .map(|samples| samples.capacity() * 4)
                        .sum::<usize>()
                        <= 40
                );
            }
            assert_eq!(decoded.samples[0], [0.25; 5]);
            assert_eq!(decoded.samples[1], [-0.5; 5]);
            let capacities: Vec<_> = decoded.samples.iter().map(Vec::capacity).collect();
            let buffer = packet(10, &[0.75_f32, 0.875], 1);
            assert!(
                decoded
                    .append(&GenericAudioBufferRef::F32(&buffer))
                    .is_err()
            );
            assert_eq!(decoded.samples[0], [0.25; 5]);
            assert_eq!(decoded.samples[1], [-0.5; 5]);
            assert_eq!(
                decoded
                    .samples
                    .iter()
                    .map(Vec::capacity)
                    .collect::<Vec<_>>(),
                capacities
            );
            let (samples, rate) = decoded.finish().unwrap();
            assert_eq!(rate, 10);
            assert_eq!(samples.len(), 2);
        }
    }

    #[test]
    fn unknown_duration_keeps_last_allowed_frame_and_rejects_next_packet() {
        let mut decoded = DecodedIr::new(1, 1, None).unwrap();
        for frames in [17, 13] {
            decoded
                .append(&GenericAudioBufferRef::F32(&packet(
                    1,
                    &[0.125_f32],
                    frames,
                )))
                .unwrap();
        }
        assert_eq!(decoded.samples[0].capacity(), 30);
        assert!(
            decoded
                .append(&GenericAudioBufferRef::F32(&packet(1, &[0.5_f32], 1)))
                .is_err()
        );
        assert_eq!(decoded.finish().unwrap().0[0], [0.125; 30]);
    }

    #[test]
    fn packet_geometry_and_unsupported_format_refuse_before_growth() {
        let mut decoded = DecodedIr::new(48_000, 2, None).unwrap();
        for buffer in [
            packet(44_100, &[0.25_f32, -0.5], 1),
            packet(48_000, &[0.25_f32], 1),
        ] {
            assert!(
                decoded
                    .append(&GenericAudioBufferRef::F32(&buffer))
                    .is_err()
            );
            assert!(
                decoded
                    .samples
                    .iter()
                    .all(|samples| samples.capacity() == 0)
            );
        }
        let unsigned = packet(48_000, &[0_u8, 255], 1);
        assert!(
            decoded
                .append(&GenericAudioBufferRef::U8(&unsigned))
                .is_err()
        );
        assert!(
            decoded
                .samples
                .iter()
                .all(|samples| samples.capacity() == 0)
        );
        assert!(decoded.finish().is_err());
    }

    #[test]
    fn supported_pcm_formats_preserve_scaling_channels_and_packet_order() {
        let mut decoded = DecodedIr::new(48_000, 2, None).unwrap();
        let float = packet(48_000, &[0.25_f32, -0.5], 2);
        let short = packet(48_000, &[8192_i16, -16384], 3);
        let medium = packet(48_000, &[i24(2097152), i24(-4194304)], 1);
        let long = packet(48_000, &[536870912_i32, -1073741824], 4);
        for buffer in [
            GenericAudioBufferRef::F32(&float),
            GenericAudioBufferRef::S16(&short),
            GenericAudioBufferRef::S24(&medium),
            GenericAudioBufferRef::S32(&long),
        ] {
            decoded.append(&buffer).unwrap();
        }
        let (samples, rate) = decoded.finish().unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(samples[0], [0.25; 10]);
        assert_eq!(samples[1], [-0.5; 10]);
    }
}
