//! SOS implementations for the selectable non-legacy IIR crossover families.
//!
//! This module owns only the new LR12, LR48, Butterworth, and Bessel branches.
//! Existing LR24 processing remains in `PreciseLr4` so its established audio
//! and automation behavior stays unchanged. Coefficients and delay state use
//! `f64`; conversion to the plugin's `f32` audio boundary happens per sample.

// Rust guideline compliant 2026-02-21

use crate::crossover_kind::CrossoverKind;

const MAX_SECTIONS: usize = 4;

#[derive(Clone, Copy, Debug, Default)]
struct SosCoefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct SosState {
    z1: f64,
    z2: f64,
}

#[derive(Clone, Copy, Debug)]
enum SectionDesign {
    ButterworthFirstOrder,
    ButterworthSecondOrder { q: f64 },
    BesselMagnitude,
}

#[derive(Clone, Copy, Debug)]
enum Branch {
    Low,
    High,
}

#[derive(Debug)]
struct SosSection {
    design: SectionDesign,
    coefficients: SosCoefficients,
    states: Vec<SosState>,
}

impl SosSection {
    fn new(design: SectionDesign, branch: Branch, prewarp: f64, channels: usize) -> Self {
        Self {
            design,
            coefficients: design.coefficients(branch, prewarp),
            states: vec![SosState::default(); channels],
        }
    }

    fn set_frequency(&mut self, branch: Branch, prewarp: f64) {
        self.coefficients = self.design.coefficients(branch, prewarp);
    }

    fn process_sample(&mut self, input: f64, channel: usize) -> f64 {
        let coefficients = self.coefficients;
        let state = &mut self.states[channel];
        let output = coefficients.b0 * input + state.z1;
        state.z1 = coefficients.b1 * input - coefficients.a1 * output + state.z2;
        state.z2 = coefficients.b2 * input - coefficients.a2 * output;
        output
    }

    fn reset(&mut self) {
        self.states.fill(SosState::default());
    }
}

impl SectionDesign {
    fn coefficients(self, branch: Branch, prewarp: f64) -> SosCoefficients {
        match self {
            Self::ButterworthFirstOrder => first_order_coefficients(branch, prewarp),
            Self::ButterworthSecondOrder { q } => second_order_coefficients(branch, prewarp, q),
            Self::BesselMagnitude => bessel_magnitude_coefficients(branch, prewarp),
        }
    }
}

/// One independently stateful low/high crossover split.
///
/// The caller owns one split per literal branch or compensation pair, so
/// multiway LR processing never shares delay state between emitted bands.
#[derive(Debug)]
pub(super) struct IirSplit {
    low_sections: Vec<SosSection>,
    high_sections: Vec<SosSection>,
    high_polarity: f64,
    channels: usize,
    sample_rate: f64,
}

impl IirSplit {
    pub(super) fn new(
        kind: CrossoverKind,
        frequency: f32,
        sample_rate: f64,
        channels: usize,
    ) -> Result<Self, String> {
        let (designs, high_polarity) = section_designs(kind)?;
        let frequency = f64::from(frequency);
        let prewarp = prewarp(frequency, sample_rate);
        let mut low_sections = Vec::with_capacity(MAX_SECTIONS);
        let mut high_sections = Vec::with_capacity(MAX_SECTIONS);
        for design in designs {
            low_sections.push(SosSection::new(design, Branch::Low, prewarp, channels));
            high_sections.push(SosSection::new(design, Branch::High, prewarp, channels));
        }

        Ok(Self {
            low_sections,
            high_sections,
            high_polarity,
            channels,
            sample_rate,
        })
    }

    /// Updates coefficients while preserving the independent section histories.
    pub(super) fn set_frequency(&mut self, frequency: f32) {
        let frequency = f64::from(frequency);
        let prewarp = prewarp(frequency, self.sample_rate);
        for section in &mut self.low_sections {
            section.set_frequency(Branch::Low, prewarp);
        }
        for section in &mut self.high_sections {
            section.set_frequency(Branch::High, prewarp);
        }
    }

    /// Changes the sample rate and cutoff, preserving state for a live update.
    pub(super) fn reconfigure(&mut self, frequency: f32, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.set_frequency(frequency);
    }

    /// Processes one interleaved frame into its low and high branches.
    pub(super) fn process_frame(&mut self, input: &[f32], low: &mut [f32], high: &mut [f32]) {
        debug_assert_eq!(input.len(), self.channels);
        debug_assert_eq!(low.len(), self.channels);
        debug_assert_eq!(high.len(), self.channels);

        for channel in 0..self.channels {
            let sample = f64::from(input[channel]);
            let mut low_sample = sample;
            for section in &mut self.low_sections {
                low_sample = section.process_sample(low_sample, channel);
            }

            let mut high_sample = sample;
            for section in &mut self.high_sections {
                high_sample = section.process_sample(high_sample, channel);
            }

            low[channel] = low_sample as f32;
            high[channel] = (high_sample * self.high_polarity) as f32;
        }
    }

    pub(super) fn reset(&mut self) {
        for section in self.low_sections.iter_mut().chain(&mut self.high_sections) {
            section.reset();
        }
    }
}

fn section_designs(kind: CrossoverKind) -> Result<(Vec<SectionDesign>, f64), String> {
    let (order, repeats, high_polarity) = match kind {
        CrossoverKind::Lr12 => (1, 2, -1.0),
        CrossoverKind::Lr48 => (4, 2, 1.0),
        CrossoverKind::Butterworth6 => (1, 1, 1.0),
        CrossoverKind::Butterworth12 => (2, 1, 1.0),
        CrossoverKind::Butterworth18 => (3, 1, 1.0),
        CrossoverKind::Butterworth24 => (4, 1, 1.0),
        CrossoverKind::Butterworth30 => (5, 1, 1.0),
        CrossoverKind::Butterworth36 => (6, 1, 1.0),
        CrossoverKind::Butterworth42 => (7, 1, 1.0),
        CrossoverKind::Butterworth48 => (8, 1, 1.0),
        CrossoverKind::Bessel12 => {
            return Ok((vec![SectionDesign::BesselMagnitude], 1.0));
        }
        CrossoverKind::Lr24 | CrossoverKind::LinearPhase => {
            return Err(format!(
                "{} does not use the selectable IIR section implementation",
                kind.as_str()
            ));
        }
    };

    let mut designs = butterworth_section_designs(order);
    if repeats == 2 {
        let first = designs.clone();
        designs.extend(first);
    }
    Ok((designs, high_polarity))
}

fn butterworth_section_designs(order: usize) -> Vec<SectionDesign> {
    let mut designs = Vec::with_capacity(MAX_SECTIONS);
    if order % 2 == 1 {
        designs.push(SectionDesign::ButterworthFirstOrder);
    }
    let pairs = order / 2;
    for pair in (0..pairs).rev() {
        // Butterworth pole pairs are ordered from lower Q to higher Q so the
        // cascade grows from the well-damped sections toward the resonant ones.
        let k = pair as f64;
        let n = order as f64;
        let q = 1.0 / (2.0 * ((2.0 * k + 1.0) * std::f64::consts::PI / (2.0 * n)).sin());
        designs.push(SectionDesign::ButterworthSecondOrder { q });
    }
    designs
}

fn prewarp(frequency: f64, sample_rate: f64) -> f64 {
    (std::f64::consts::PI * frequency / sample_rate).tan()
}

fn first_order_coefficients(branch: Branch, prewarp: f64) -> SosCoefficients {
    let denominator = 1.0 + prewarp;
    let a1 = (prewarp - 1.0) / denominator;
    match branch {
        Branch::Low => SosCoefficients {
            b0: prewarp / denominator,
            b1: prewarp / denominator,
            b2: 0.0,
            a1,
            a2: 0.0,
        },
        Branch::High => SosCoefficients {
            b0: 1.0 / denominator,
            b1: -1.0 / denominator,
            b2: 0.0,
            a1,
            a2: 0.0,
        },
    }
}

fn second_order_coefficients(branch: Branch, prewarp: f64, q: f64) -> SosCoefficients {
    let prewarp_squared = prewarp * prewarp;
    let denominator = 1.0 + prewarp / q + prewarp_squared;
    let a1 = 2.0 * (prewarp_squared - 1.0) / denominator;
    let a2 = (1.0 - prewarp / q + prewarp_squared) / denominator;
    match branch {
        Branch::Low => SosCoefficients {
            b0: prewarp_squared / denominator,
            b1: 2.0 * prewarp_squared / denominator,
            b2: prewarp_squared / denominator,
            a1,
            a2,
        },
        Branch::High => SosCoefficients {
            b0: 1.0 / denominator,
            b1: -2.0 / denominator,
            b2: 1.0 / denominator,
            a1,
            a2,
        },
    }
}

fn bessel_magnitude_coefficients(branch: Branch, prewarp: f64) -> SosCoefficients {
    let a = (3.0 * (5.0_f64.sqrt() - 1.0) / 2.0).sqrt();
    let a_squared = a * a;
    match branch {
        Branch::Low => {
            let denominator_0 = a_squared + 3.0 * a * prewarp + 3.0 * prewarp * prewarp;
            let denominator_1 = -2.0 * a_squared + 6.0 * prewarp * prewarp;
            let denominator_2 = a_squared - 3.0 * a * prewarp + 3.0 * prewarp * prewarp;
            SosCoefficients {
                b0: 3.0 * prewarp * prewarp / denominator_0,
                b1: 6.0 * prewarp * prewarp / denominator_0,
                b2: 3.0 * prewarp * prewarp / denominator_0,
                a1: denominator_1 / denominator_0,
                a2: denominator_2 / denominator_0,
            }
        }
        Branch::High => {
            let prewarp_squared = prewarp * prewarp;
            let denominator_0 = a_squared * prewarp_squared + 3.0 * a * prewarp + 3.0;
            let denominator_1 = 2.0 * a_squared * prewarp_squared - 6.0;
            let denominator_2 = a_squared * prewarp_squared - 3.0 * a * prewarp + 3.0;
            SosCoefficients {
                b0: 3.0 / denominator_0,
                b1: -6.0 / denominator_0,
                b2: 3.0 / denominator_0,
                a1: denominator_1 / denominator_0,
                a2: denominator_2 / denominator_0,
            }
        }
    }
}
