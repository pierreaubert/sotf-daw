//! Retain exact EQ targets until their source audio reaches the internal filters.
//!
//! Each band coalesces edits at the same source frame. The transport is fed no
//! more than one input chunk at a time, bounding pending source instants by one
//! chunk plus the upsampler delay. All history storage is prepared off-thread.

// Rust guideline compliant 2026-02-21
use super::types::BandTransition;
use math_audio_iir_fir::{Biquad, BiquadCoefficients};

struct BandHistory {
    timestamps: Vec<u64>,
    targets: Vec<BiquadCoefficients>,
    offsets: Vec<usize>,
    stride: usize,
    read: usize,
    count: usize,
    transition: BandTransition,
}

impl BandHistory {
    fn new(filters: &[Vec<Vec<Biquad>>], band: usize, capacity: usize) -> Self {
        let coefficients: Vec<Vec<_>> = filters
            .iter()
            .map(|channel| {
                channel
                    .get(band)
                    .into_iter()
                    .flatten()
                    .map(Biquad::coefficients)
                    .collect()
            })
            .collect();
        let mut offsets = Vec::with_capacity(coefficients.len());
        let mut stride = 0;
        for channel in &coefficients {
            offsets.push(stride);
            stride += channel.len();
        }
        let mut targets = Vec::with_capacity(stride * capacity);
        for _ in 0..capacity {
            for channel in &coefficients {
                targets.extend_from_slice(channel);
            }
        }
        Self {
            timestamps: vec![0; capacity],
            targets,
            offsets,
            stride,
            read: 0,
            count: 0,
            transition: BandTransition {
                old_coeffs_per_channel: coefficients.clone(),
                new_coeffs_per_channel: coefficients,
                samples_remaining: 0,
                total_samples: 0,
            },
        }
    }

    fn enqueue(&mut self, timestamp: u64, filters: &[Vec<Vec<Biquad>>], band: usize) {
        let capacity = self.timestamps.len();
        let last = (self.read + self.count + capacity - 1) % capacity;
        let slot = if self.count != 0 && self.timestamps[last] == timestamp {
            last
        } else {
            // At most one event per band per source frame; processing each input
            // chunk releases events older than the upsampler's fixed delay.
            assert!(
                self.count < capacity,
                "EQ automation history bound violated"
            );
            let slot = (self.read + self.count) % capacity;
            self.count += 1;
            self.timestamps[slot] = timestamp;
            slot
        };
        for (channel, filters) in filters.iter().enumerate() {
            for (stage, filter) in filters.get(band).into_iter().flatten().enumerate() {
                self.targets[slot * self.stride + self.offsets[channel] + stage] =
                    filter.coefficients();
            }
        }
    }

    fn begin_frame(&mut self, frame: u64, duration: usize) {
        while self.count != 0 && self.timestamps[self.read] <= frame {
            for channel in 0..self.transition.old_coeffs_per_channel.len() {
                for stage in 0..self.transition.old_coeffs_per_channel[channel].len() {
                    let running = self.transition.coefficients(channel, stage).unwrap();
                    self.transition.old_coeffs_per_channel[channel][stage] = running;
                    self.transition.new_coeffs_per_channel[channel][stage] =
                        self.targets[self.read * self.stride + self.offsets[channel] + stage];
                }
            }
            self.transition.total_samples = duration;
            self.transition.samples_remaining = duration;
            self.read = (self.read + 1) % self.timestamps.len();
            self.count -= 1;
        }
    }

    fn reset(&mut self, filters: &[Vec<Vec<Biquad>>], band: usize) {
        self.read = 0;
        self.count = 0;
        self.transition.samples_remaining = 0;
        for (channel, filters) in filters.iter().enumerate() {
            for (stage, filter) in filters.get(band).into_iter().flatten().enumerate() {
                self.transition.old_coeffs_per_channel[channel][stage] = filter.coefficients();
                self.transition.new_coeffs_per_channel[channel][stage] = filter.coefficients();
            }
        }
    }
}

pub(super) struct OversampledAutomation {
    source_frame: u64,
    processing_frame: u64,
    delay: u64,
    duration: usize,
    bands: Vec<BandHistory>,
}

impl OversampledAutomation {
    pub(super) fn new(
        filters: &[Vec<Vec<Biquad>>],
        bands: usize,
        capacity: usize,
        delay: usize,
        duration: usize,
        timeline: (u64, u64),
    ) -> Self {
        Self {
            source_frame: timeline.0,
            processing_frame: timeline.1,
            delay: delay as u64,
            duration,
            bands: (0..bands)
                .map(|band| BandHistory::new(filters, band, capacity))
                .collect(),
        }
    }

    pub(super) fn timeline(&self) -> (u64, u64) {
        (self.source_frame, self.processing_frame)
    }

    pub(super) fn enqueue(&mut self, filters: &[Vec<Vec<Biquad>>], band: usize) {
        self.bands[band].enqueue(self.source_frame.saturating_add(self.delay), filters, band);
    }

    pub(super) fn advance_source(&mut self, frames: usize) {
        self.source_frame = self.source_frame.saturating_add(frames as u64);
    }

    pub(super) fn begin_frame(&mut self) {
        for band in &mut self.bands {
            band.begin_frame(self.processing_frame, self.duration);
        }
    }

    pub(super) fn coefficients(
        &self,
        band: usize,
        channel: usize,
        stage: usize,
    ) -> Option<BiquadCoefficients> {
        self.bands
            .get(band)?
            .transition
            .coefficients(channel, stage)
    }

    pub(super) fn end_frame(&mut self) {
        for band in &mut self.bands {
            band.transition.samples_remaining = band.transition.samples_remaining.saturating_sub(1);
        }
        self.processing_frame = self.processing_frame.saturating_add(1);
    }

    pub(super) fn reset(&mut self, filters: &[Vec<Vec<Biquad>>], reset_timeline: bool) {
        if reset_timeline {
            self.source_frame = 0;
            self.processing_frame = 0;
        }
        for (band, history) in self.bands.iter_mut().enumerate() {
            history.reset(filters, band);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OversampledAutomation;
    use math_audio_iir_fir::{Biquad, BiquadFilterType};
    use sotf_host::oversampling::Oversampler;

    #[test]
    fn prepared_coefficient_history_has_a_finite_geometry_based_budget() {
        for factor in [2, 4] {
            let os = Oversampler::new(factor, 32).unwrap();
            let delay = os.upsampling_delay_frames();
            let capacity = os.input_chunk_frames() + delay.div_ceil(factor as usize) + 1;
            let stage = Biquad::new(
                BiquadFilterType::Peak,
                1000.0,
                48_000.0 * f64::from(factor),
                0.7,
                3.0,
            );
            let filters = vec![vec![vec![stage; 4]; 20]; 32];
            let history = OversampledAutomation::new(
                &filters,
                20,
                capacity,
                delay,
                240 * factor as usize,
                (0, 0),
            );
            let bytes: usize = history
                .bands
                .iter()
                .map(|band| {
                    band.targets.capacity()
                        * std::mem::size_of::<math_audio_iir_fir::BiquadCoefficients>()
                })
                .sum();
            // 20 bands x 32 channels x 4 stages; one chunk plus FIR delay.
            assert_eq!(
                bytes,
                20 * 32
                    * 4
                    * capacity
                    * std::mem::size_of::<math_audio_iir_fir::BiquadCoefficients>()
            );
            assert!(bytes < 100 * 1024 * 1024);
            println!(
                "EQ history factor={factor}, source_capacity={capacity}, upsampled_delay={delay}, coefficient_payload_bytes={bytes}"
            );
        }
    }
}
