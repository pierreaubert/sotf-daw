//! Routes surround bass through prepared Linkwitz-Riley crossovers.

// Rust guideline compliant 2026-10-09

use crate::surround_routing::SurroundRouting;
use sotf_host::lr4_crossover::Lr4Crossover;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BassMode {
    Off,
    Derived,
    Managed,
}

/// Construction settings for the prepared bass router.
pub(super) struct BassSettings {
    pub(super) mode: BassMode,
    pub(super) cutoff_hz: f64,
    pub(super) selected: Vec<bool>,
    pub(super) source_gains: Vec<f32>,
    pub(super) sub_gain: f32,
    pub(super) combine_input_lfe: bool,
}

/// Prepared bass routing for one input and output layout.
pub(super) struct SurroundBass {
    mode: BassMode,
    selected: Vec<bool>,
    source_gains: Vec<f32>,
    sub_gain: f32,
    cutoff_hz: f64,
    crossover: Lr4Crossover<f64>,
}

impl SurroundBass {
    /// Prepares filter state and validates the requested subwoofer routing.
    pub(super) fn new(
        routing: &SurroundRouting,
        sample_rate: f64,
        settings: BassSettings,
    ) -> Result<Self, String> {
        let BassSettings {
            mode,
            cutoff_hz,
            selected,
            source_gains,
            sub_gain,
            combine_input_lfe,
        } = settings;
        if !sample_rate.is_finite()
            || !cutoff_hz.is_finite()
            || sample_rate <= 0.0
            || !(20.0..=180.0).contains(&cutoff_hz)
            || cutoff_hz >= sample_rate * 0.495
        {
            return Err("bass crossover requires a valid sample rate and cutoff".into());
        }
        if selected.len() != routing.input_channels || source_gains.len() != routing.input_channels
        {
            return Err("bass routing requires one selection and gain per input channel".into());
        }
        if !sub_gain.is_finite()
            || sub_gain < 0.0
            || source_gains
                .iter()
                .any(|gain| !gain.is_finite() || *gain < 0.0)
        {
            return Err("bass gains must be finite and nonnegative".into());
        }
        if routing.input_lfe.is_some_and(|index| selected[index]) {
            return Err("the existing LFE cannot be a derived bass source".into());
        }
        if mode != BassMode::Off && routing.output_lfe.is_none() {
            return Err("bass routing requires a subwoofer output".into());
        }
        if mode != BassMode::Off && routing.input_lfe.is_some() && !combine_input_lfe {
            return Err(
                "combining redirected bass with existing LFE requires explicit consent".into(),
            );
        }

        Ok(Self {
            mode,
            selected,
            source_gains,
            sub_gain,
            cutoff_hz,
            crossover: Lr4Crossover::new(cutoff_hz, sample_rate, routing.input_channels),
        })
    }

    /// Clears crossover history without reallocating.
    pub(super) fn reset(&mut self) {
        self.crossover.reset();
    }

    /// Reports whether recursive crossover state can produce a tail.
    pub(super) fn has_recursive_tail(&self) -> bool {
        self.mode != BassMode::Off && self.selected.iter().any(|selected| *selected)
    }

    /// Rebuilds crossover state for a new sample rate.
    pub(super) fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 || self.cutoff_hz >= sample_rate * 0.495 {
            return Err("bass crossover requires a valid sample rate".into());
        }
        self.crossover
            .reinit(self.cutoff_hz, sample_rate, self.selected.len());
        Ok(())
    }

    /// Writes the bed and optional low-frequency sum into one output frame.
    #[inline]
    pub(super) fn process_frame(
        &mut self,
        routing: &SurroundRouting,
        input: &[f32],
        output: &mut [f32],
    ) {
        let mut bass = 0.0_f64;
        for &(source, destination) in &routing.bed {
            let sample = input[source];
            output[destination] = if self.mode != BassMode::Off && self.selected[source] {
                let (low, high) = self.crossover.process(f64::from(sample), source);
                bass += low * f64::from(self.source_gains[source]);
                if self.mode == BassMode::Managed {
                    high as f32
                } else {
                    sample
                }
            } else {
                sample
            };
        }
        if self.mode != BassMode::Off {
            let destination = routing
                .output_lfe
                .expect("prepared bass route has an LFE output");
            output[destination] += (bass * f64::from(self.sub_gain)) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};

    fn routing(input: &str, output: &str) -> SurroundRouting {
        let input = ChannelLayout::from_speaker_config(get_speaker_config(input).unwrap()).unwrap();
        let output =
            ChannelLayout::from_speaker_config(get_speaker_config(output).unwrap()).unwrap();
        SurroundRouting::new(&input, &output).unwrap()
    }

    fn bass(routing: &SurroundRouting, mode: BassMode, selected: Vec<bool>) -> SurroundBass {
        SurroundBass::new(
            routing,
            48_000.0,
            BassSettings {
                mode,
                cutoff_hz: 80.0,
                selected,
                source_gains: vec![1.0; routing.input_channels],
                sub_gain: 1.0,
                combine_input_lfe: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn derived_bass_preserves_the_bed_and_excludes_unselected_channels() {
        let route = routing("5.0", "7.1.4");
        let mut derived = bass(
            &route,
            BassMode::Derived,
            vec![true, false, true, false, false],
        );
        let mut output = [0.0; 12];
        for _ in 0..48_000 {
            output.fill(0.0);
            derived.process_frame(&route, &[0.25, 0.75, 0.5, 0.6, 0.7], &mut output);
        }
        assert_eq!(&output[..3], &[0.25, 0.75, 0.5]);
        assert_eq!(&output[4..8], &[0.6, 0.7, 0.0, 0.0]);
        assert!((output[3] - 0.75).abs() < 1e-5);
    }

    #[test]
    fn managed_crossover_splits_selected_channels_and_resets() {
        let route = routing("5.0", "5.1");
        let mut managed = bass(
            &route,
            BassMode::Managed,
            vec![true, false, false, false, false],
        );
        let mut output = [0.0; 6];
        let mut low_energy = 0.0_f64;
        let mut high_energy = 0.0_f64;
        let mut sum_energy = 0.0_f64;
        for frame in 0..96_000 {
            let signal = (std::f64::consts::TAU * 80.0 * frame as f64 / 48_000.0).sin() as f32;
            output.fill(0.0);
            managed.process_frame(&route, &[signal, 0.25, 0.0, 0.0, 0.0], &mut output);
            assert_eq!(output[1], 0.25);
            if frame >= 48_000 {
                low_energy += f64::from(output[3]).powi(2);
                high_energy += f64::from(output[0]).powi(2);
                sum_energy += f64::from(output[0] + output[3]).powi(2);
            }
        }
        let samples = 48_000.0;
        assert!(((low_energy / samples).sqrt() - 0.5 / 2.0_f64.sqrt()).abs() < 1e-4);
        assert!(((high_energy / samples).sqrt() - 0.5 / 2.0_f64.sqrt()).abs() < 1e-4);
        assert!(((sum_energy / samples).sqrt() - 1.0 / 2.0_f64.sqrt()).abs() < 1e-4);
        managed.reset();
        output.fill(0.0);
        managed.process_frame(&route, &[0.0; 5], &mut output);
        assert_eq!(output, [0.0; 6]);
    }

    #[test]
    fn managed_crossover_matches_independent_complex_lr4_response() {
        let route = routing("5.0", "5.1");
        for sample_rate in [44_100.0_f64, 48_000.0, 96_000.0] {
            for cutoff in [40.0_f64, 80.0, 160.0] {
                let mut managed = SurroundBass::new(
                    &route,
                    sample_rate,
                    BassSettings {
                        mode: BassMode::Managed,
                        cutoff_hz: cutoff,
                        selected: vec![true, false, false, false, false],
                        source_gains: vec![1.0; 5],
                        sub_gain: 1.0,
                        combine_input_lfe: false,
                    },
                )
                .unwrap();
                let frequencies = [
                    cutoff / 4.0,
                    cutoff / 2.0,
                    cutoff,
                    cutoff * 2.0,
                    cutoff * 4.0,
                ];
                let mut measured = [[(0.0_f64, 0.0_f64); 2]; 5];
                let mut output = [0.0; 6];
                for n in 0..(sample_rate as usize * 2) {
                    output.fill(0.0);
                    let input = [if n == 0 { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0];
                    managed.process_frame(&route, &input, &mut output);
                    for (index, frequency) in frequencies.iter().enumerate() {
                        let angle = std::f64::consts::TAU * frequency * n as f64 / sample_rate;
                        for (band, value) in [output[3], output[0]].iter().enumerate() {
                            measured[index][band].0 += f64::from(*value) * angle.cos();
                            measured[index][band].1 -= f64::from(*value) * angle.sin();
                        }
                    }
                }
                for (index, frequency) in frequencies.iter().enumerate() {
                    // Bilinear-transform the analog LR4 transfer functions directly.
                    let r = (std::f64::consts::PI * frequency / sample_rate).tan()
                        / (std::f64::consts::PI * cutoff / sample_rate).tan();
                    let real = 1.0 - r * r;
                    let imag = std::f64::consts::SQRT_2 * r;
                    let squared_real = real * real - imag * imag;
                    let squared_imag = 2.0 * real * imag;
                    let norm = squared_real * squared_real + squared_imag * squared_imag;
                    let low = (squared_real / norm, -squared_imag / norm);
                    let high = (low.0 * r.powi(4), low.1 * r.powi(4));
                    for (actual, expected) in measured[index].iter().zip([low, high]) {
                        let error = ((actual.0 - expected.0).powi(2)
                            + (actual.1 - expected.1).powi(2))
                        .sqrt();
                        assert!(
                            error < 2e-5,
                            "complex LR4 error {error} at {sample_rate} Hz, cutoff {cutoff} Hz, signal {frequency} Hz"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn correlated_bass_sum_retains_its_true_peak_headroom() {
        let route = routing("5.0", "5.1");
        let mut derived = bass(&route, BassMode::Derived, vec![true; 5]);
        let mut meter = sotf_host::TruePeakDetector::new();
        let mut sample_peak = 0.0_f32;
        let mut true_peak = 0.0_f32;
        let mut output = [0.0; 6];
        let frequency = 37.0_f64;
        let sample_rate = 48_000.0_f64;
        for frame in 0..96_000 {
            let sample = (0.8
                * (std::f64::consts::TAU * frequency * frame as f64 / sample_rate).sin())
                as f32;
            output.fill(0.0);
            derived.process_frame(&route, &[sample; 5], &mut output);
            assert_eq!(
                [output[0], output[1], output[2], output[4], output[5]],
                [sample; 5]
            );
            let interval_peak = meter.process_linear(output[3]);
            if frame >= 24_000 {
                sample_peak = sample_peak.max(output[3].abs());
                true_peak = true_peak.max(interval_peak);
            }
        }
        let ratio = (std::f64::consts::PI * frequency / sample_rate).tan()
            / (std::f64::consts::PI * 80.0 / sample_rate).tan();
        let expected = (4.0 / (1.0 + ratio.powi(4))) as f32;
        assert!(true_peak >= sample_peak);
        assert!(
            (true_peak - expected).abs() < 0.02,
            "true peak {true_peak}, expected {expected}"
        );
        assert!(true_peak > 1.0);
    }

    #[test]
    fn opposite_polarity_sources_cancel_in_the_generated_subwoofer_feed() {
        let route = routing("5.0", "5.1");
        for mode in [BassMode::Derived, BassMode::Managed] {
            let mut bass = bass(&route, mode, vec![true, true, false, false, false]);
            let mut output = [0.0; 6];
            for frame in 0..48_000 {
                let sample =
                    (0.5 * (std::f64::consts::TAU * 80.0 * frame as f64 / 48_000.0).sin()) as f32;
                output.fill(0.0);
                bass.process_frame(&route, &[sample, -sample, 0.0, 0.0, 0.0], &mut output);
                assert!(output[3].abs() < 1e-7);
                if mode == BassMode::Derived {
                    assert_eq!(output[0], sample);
                    assert_eq!(output[1], -sample);
                }
            }
        }
    }

    #[test]
    fn existing_lfe_requires_explicit_combining_and_is_never_a_source() {
        let route = routing("5.1", "7.1");
        let mut selected = vec![false; 6];
        selected[0] = true;
        let gains = vec![1.0; 6];
        assert!(
            SurroundBass::new(
                &route,
                48_000.0,
                BassSettings {
                    mode: BassMode::Derived,
                    cutoff_hz: 80.0,
                    selected: selected.clone(),
                    source_gains: gains.clone(),
                    sub_gain: 1.0,
                    combine_input_lfe: false,
                },
            )
            .is_err()
        );
        selected[route.input_lfe.unwrap()] = true;
        assert!(
            SurroundBass::new(
                &route,
                48_000.0,
                BassSettings {
                    mode: BassMode::Managed,
                    cutoff_hz: 80.0,
                    selected,
                    source_gains: gains,
                    sub_gain: 1.0,
                    combine_input_lfe: true,
                },
            )
            .is_err()
        );
        let mut off = bass(&route, BassMode::Off, vec![false; 6]);
        let mut output = [0.0; 8];
        off.process_frame(&route, &[0.0, 0.0, 0.0, 0.75, 0.0, 0.0], &mut output);
        assert_eq!(output[3], 0.75);
    }
}
