//! Prepared measurements used by gain compensation.
// Rust guideline compliant 2026-02-21
use super::AutoGainLoudnessType;
use math_audio_dsp::ebur128::{EbuR128, Mode};

/// Retain the generic monitor's positional channel map and M/S arithmetic.
/// Both windows stay warm across selection changes. Analyzer-only integrated
/// history, true peaks, correlation and display arrays are not needed here.
pub(super) struct GainMeter {
    channels: u32,
    meter: EbuR128,
}

impl GainMeter {
    pub(super) fn new(channels: u32, sample_rate: u32) -> Result<Self, String> {
        if channels == 0 {
            return Err("loudness monitor requires at least one channel".to_string());
        }
        if sample_rate < 10 {
            return Err("loudness monitor sample rate must be at least 10 Hz".to_string());
        }
        Ok(Self {
            channels,
            meter: EbuR128::new(channels, sample_rate, Mode::M | Mode::S | Mode::SAMPLE_PEAK)
                .map_err(|error| format!("{error:?}"))?,
        })
    }

    pub(super) fn add_frames(&mut self, samples: &[f32]) -> Result<(), String> {
        if !samples.len().is_multiple_of(self.channels as usize) {
            return Err(format!(
                "loudness input has {} samples, not a whole number of {}-channel frames",
                samples.len(),
                self.channels
            ));
        }
        // EbuR128 already splits on its persistent 100 ms boundaries. The
        // generic monitor's outer chunks do not alter sample order or sums.
        self.meter
            .add_frames_f32(samples)
            .map_err(|error| format!("EBU R128 add_frames failed: {error:?}"))
    }

    pub(super) fn measurement(&mut self, kind: AutoGainLoudnessType) -> (f64, f64) {
        let loudness = match kind {
            AutoGainLoudnessType::Momentary => self.meter.loudness_momentary(),
            AutoGainLoudnessType::ShortTerm => self.meter.loudness_shortterm(),
        }
        .unwrap_or(f64::NEG_INFINITY);
        // Query every channel, including channels with zero loudness weight.
        // Each query consumes that channel's peak since its previous refresh.
        let peak = (0..self.channels)
            .map(|channel| self.meter.prev_sample_peak(channel).unwrap_or(0.0))
            .fold(0.0, f64::max);
        (loudness, peak)
    }

    pub(super) fn reset(&mut self) {
        self.meter.reset();
    }
}
