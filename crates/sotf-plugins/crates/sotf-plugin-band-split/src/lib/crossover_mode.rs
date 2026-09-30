use super::types::BandSplitRecombinationMode;
use sotf_host::lr4_crossover::{Lr4Crossover, MultibandLr4Crossover};
use sotf_host::lr8_crossover::{Lr8Crossover, MultibandLr8Crossover};

enum MultibandCrossover {
    LR24(MultibandLr4Crossover<f32>),
    LR48(MultibandLr8Crossover<f32>),
}

enum AllpassCrossover {
    LR24(Lr4Crossover<f32>),
    LR48(Lr8Crossover<f32>),
}

impl AllpassCrossover {
    fn new(frequency: f32, sample_rate: u32, channels: usize, slope_index: usize) -> Self {
        match slope_index {
            1 => Self::LR48(Lr8Crossover::new(frequency, sample_rate as f32, channels)),
            _ => Self::LR24(Lr4Crossover::new(frequency, sample_rate as f32, channels)),
        }
    }

    fn set_frequency(&mut self, frequency: f32) {
        match self {
            Self::LR24(crossover) => crossover.set_frequency(frequency),
            Self::LR48(crossover) => crossover.set_frequency(frequency),
        }
    }

    #[inline]
    fn process_allpass(&mut self, sample: f32, channel: usize) -> f32 {
        let (low, high) = match self {
            Self::LR24(crossover) => crossover.process(sample, channel),
            Self::LR48(crossover) => crossover.process(sample, channel),
        };
        low + high
    }

    fn reset(&mut self) {
        match self {
            Self::LR24(crossover) => crossover.reset(),
            Self::LR48(crossover) => crossover.reset(),
        }
    }

    fn reset_at_frequency(&mut self, frequency: f32) {
        match self {
            Self::LR24(crossover) => crossover.reset_at_frequency(frequency),
            Self::LR48(crossover) => crossover.reset_at_frequency(frequency),
        }
    }
}

/// Shared crossover state for the historical split and optional per-band
/// all-pass compensation. The compensation sections are created only for the
/// phase-compensated mode and always have independent recursive state.
pub(super) struct CrossoverMode {
    crossover: MultibandCrossover,
    compensation: Vec<Vec<AllpassCrossover>>,
    recombination_mode: BandSplitRecombinationMode,
}

impl CrossoverMode {
    pub(super) fn new(
        frequencies: &[f32],
        sample_rate: u32,
        channels: usize,
        slope_index: usize,
        recombination_mode: BandSplitRecombinationMode,
    ) -> Self {
        let crossover = match slope_index {
            1 => MultibandCrossover::LR48(MultibandLr8Crossover::new(
                frequencies,
                sample_rate as f32,
                channels,
            )),
            _ => MultibandCrossover::LR24(MultibandLr4Crossover::new(
                frequencies,
                sample_rate as f32,
                channels,
            )),
        };

        let compensation = if recombination_mode == BandSplitRecombinationMode::PhaseCompensated {
            (0..frequencies.len())
                .map(|band_index| {
                    ((band_index + 1)..frequencies.len())
                        .map(|stage_index| {
                            AllpassCrossover::new(
                                frequencies[stage_index],
                                sample_rate,
                                channels,
                                slope_index,
                            )
                        })
                        .collect()
                })
                .collect()
        } else {
            Vec::new()
        };

        Self {
            crossover,
            compensation,
            recombination_mode,
        }
    }

    pub(super) fn set_frequency(&mut self, index: usize, frequency: f32) {
        match &mut self.crossover {
            MultibandCrossover::LR24(crossover) => crossover.set_frequency(index, frequency),
            MultibandCrossover::LR48(crossover) => crossover.set_frequency(index, frequency),
        }

        for (band_index, band_compensation) in self.compensation.iter_mut().enumerate() {
            if let Some(section) = index
                .checked_sub(band_index + 1)
                .and_then(|offset| band_compensation.get_mut(offset))
            {
                section.set_frequency(frequency);
            }
        }
    }

    pub(super) fn process_frame(&mut self, input: &[f32], bands: &mut [&mut [f32]]) {
        match &mut self.crossover {
            MultibandCrossover::LR24(crossover) => crossover.process_frame(input, bands),
            MultibandCrossover::LR48(crossover) => crossover.process_frame(input, bands),
        }
    }

    /// Apply all later crossover all-pass sections to each intermediate band.
    /// Band order and the existing band-major output layout are unchanged.
    #[inline]
    pub(super) fn compensate_intermediate_bands(
        &mut self,
        flat_bands: &mut [f32],
        num_bands: usize,
        channels: usize,
    ) {
        if self.recombination_mode != BandSplitRecombinationMode::PhaseCompensated {
            return;
        }

        for (band_index, compensation) in self.compensation.iter_mut().enumerate() {
            if band_index + 1 >= num_bands {
                break;
            }
            let band_offset = band_index * channels;
            for channel in 0..channels {
                let sample_index = band_offset + channel;
                let mut sample = flat_bands[sample_index];
                for section in compensation.iter_mut() {
                    sample = section.process_allpass(sample, channel);
                }
                flat_bands[sample_index] = sample;
            }
        }
    }

    pub(super) fn reset(&mut self, frequencies: &[f32]) {
        match &mut self.crossover {
            MultibandCrossover::LR24(crossover) => crossover.reset_at_frequencies(frequencies),
            MultibandCrossover::LR48(crossover) => crossover.reset_at_frequencies(frequencies),
        }
        for (band_index, band) in self.compensation.iter_mut().enumerate() {
            for (offset, section) in band.iter_mut().enumerate() {
                if let Some(&frequency) = frequencies.get(band_index + 1 + offset) {
                    section.reset_at_frequency(frequency);
                } else {
                    section.reset();
                }
            }
        }
    }

    pub(super) fn reinit(
        &mut self,
        frequencies: &[f32],
        sample_rate: u32,
        channels: usize,
        slope_index: usize,
        recombination_mode: BandSplitRecombinationMode,
    ) {
        *self = Self::new(
            frequencies,
            sample_rate,
            channels,
            slope_index,
            recombination_mode,
        );
    }
}
