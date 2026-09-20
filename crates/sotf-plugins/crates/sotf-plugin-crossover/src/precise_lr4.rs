//! f32 host boundary with double-precision LR24 coefficients and delay state.
//! Conversion is scalar and allocation-free during processing.

pub(super) struct PreciseLr4(sotf_host::lr4_crossover::Lr4Crossover<f64>);

impl PreciseLr4 {
    pub(super) fn new(frequency: f32, sample_rate: f32, channels: usize) -> Self {
        Self(sotf_host::lr4_crossover::Lr4Crossover::new(
            f64::from(frequency),
            f64::from(sample_rate),
            channels,
        ))
    }

    pub(super) fn reinit(&mut self, frequency: f32, sample_rate: f32, channels: usize) {
        self.0
            .reinit(f64::from(frequency), f64::from(sample_rate), channels);
    }

    pub(super) fn set_frequency(&mut self, frequency: f32) {
        self.0.set_frequency(f64::from(frequency));
    }

    pub(super) fn reset(&mut self) {
        self.0.reset();
    }

    #[inline]
    pub(super) fn process(&mut self, sample: f32, channel: usize) -> (f32, f32) {
        let (low, high) = self.0.process(f64::from(sample), channel);
        (low as f32, high as f32)
    }

    pub(super) fn process_frame(&mut self, input: &[f32], low: &mut [f32], high: &mut [f32]) {
        for (channel, &sample) in input.iter().enumerate() {
            (low[channel], high[channel]) = self.process(sample, channel);
        }
    }
}
