//! Asserted 96->24 near-cutoff sweep with a-priori error budgets.
//!
//! ADDITIVE coverage for review P1-3 (hardest decimation ratio) and P1-2
//! (bound provenance); the existing 48->24 sweep in `filter_response.rs`
//! is untouched. Same integer-step single-phase method: 96/24 = 4 takes
//! integer source steps, so no inter-phase interpolation occurs and the
//! phase-zero analytic DTFT applies.
//!
//! Error budgets (a-priori, also documenting the existing 48->24 bounds):
//! - Flat point (0.1x Nyquist): gain within 0.01 dB and residual below
//!   -90 dB, the established flat-band contract (spectral_accuracy).
//! - Transition/deep points: composite 0.5 dB or 2e-6 absolute amplitude.
//!   2e-6 inherits the established alias-amplitude bound (dynamic_cutoff:
//!   alias amplitude differs from the analytic FIR response by < 2e-6).
//!   0.5 dB = 0.01 dB flat-band ripple + 0.04 dB coherent-fit measurement
//!   (200 ms window, 5 Hz-coherent tones, f32 output quantization) +
//!   0.45 dB conservative margin for transition-band modeling
//!   (single-phase (P-1)/P displacement, f32 cutoff rounding, and the
//!   above-Nyquist alias-sign identity documented in filter_response).
//! - Direct-sinc 0.05 dB (existing 44.1<->48 cross-check, documented here
//!   for provenance): 0.01 dB production ripple + 0.01 dB reference ripple
//!   (512-tap single window, 2-8x longer than production, hence at most the
//!   production ripple) + 0.02 dB cutoff-definition delta (rubato
//!   calculate_cutoff vs 0.95 * min(r,1) * 0.5, both flat at 1/5 kHz) +
//!   0.01 dB delay-independent fit uncertainty.
//!
//! Above-Nyquist points alias identically in the plugin output and the
//! fitted reference (f aliases to 24k - f, up to sign), so the fitted
//! magnitude tracks the leakage amplitude set by the filter magnitude.
//! Every sweep frequency is a multiple of 5 Hz (integer cycles in 200 ms).
//!
//! Tag: `RESAMPLER-FILTER` (same published table; 96->24 lines join the
//! existing 48->24 lines).

// Rust guideline compliant 2026-02-21
use rubato::{WindowFunction, calculate_cutoff};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::f64::consts::TAU;

const AMPLITUDE: f64 = 0.5;
const CHUNK: usize = 256;
const QUALITIES: [ResamplerQuality; 3] = [
    ResamplerQuality::Fast,
    ResamplerQuality::Medium,
    ResamplerQuality::High,
];

fn specs(quality: ResamplerQuality) -> (usize, usize) {
    match quality {
        ResamplerQuality::Fast => (64, 128),
        ResamplerQuality::Medium => (128, 256),
        ResamplerQuality::High => (256, 256),
    }
}

fn analytic_gain(
    taps: usize,
    phases: usize,
    ratio_scale: f64,
    freq_hz: f64,
    input_rate: u32,
) -> f64 {
    let cutoff = f64::from(
        calculate_cutoff::<f32>(taps, WindowFunction::BlackmanHarris2) * ratio_scale as f32,
    );
    let prototype = |position: f64| {
        let angle = TAU * position / taps as f64;
        let window = 0.35875 - 0.48829 * angle.cos() + 0.14128 * (2.0 * angle).cos()
            - 0.01168 * (3.0 * angle).cos();
        let argument = std::f64::consts::PI * cutoff * (position - taps as f64 / 2.0);
        let sinc = if argument == 0.0 {
            1.0
        } else {
            argument.sin() / argument
        };
        window * window * sinc
    };
    let normalization = (0..taps * phases)
        .map(|index| prototype(index as f64 / phases as f64))
        .sum::<f64>()
        / phases as f64;
    let (real, imaginary) = (0..taps).fold((0.0, 0.0), |(real, imaginary), tap| {
        let coefficient =
            prototype(tap as f64 + (phases - 1) as f64 / phases as f64) / normalization;
        let angle = TAU * freq_hz * tap as f64 / f64::from(input_rate);
        (
            real + coefficient * angle.cos(),
            imaginary - coefficient * angle.sin(),
        )
    });
    real.hypot(imaginary)
}

fn analytic_db(taps: usize, phases: usize, ratio_scale: f64, freq_hz: f64, input_rate: u32) -> f64 {
    20.0 * analytic_gain(taps, phases, ratio_scale, freq_hz, input_rate).log10()
}

fn tones(frequencies: &[f64], rate: u32) -> Vec<f32> {
    (0..rate as usize / 2 + 37)
        .flat_map(|frame| {
            frequencies.iter().map(move |frequency| {
                (AMPLITUDE * (TAU * frequency * frame as f64 / f64::from(rate)).sin()) as f32
            })
        })
        .collect()
}

fn render_static(
    input: &[f32],
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    quality: ResamplerQuality,
) -> Vec<f32> {
    let mut plugin =
        ResamplerPlugin::with_quality(channels, input_rate, output_rate, CHUNK, quality).unwrap();
    plugin.initialize(f64::from(input_rate)).unwrap();
    let mut output = Vec::new();
    for block in input.chunks(CHUNK * channels) {
        let block_frames = block.len() / channels;
        let mut cell = vec![f32::NAN; plugin.output_frames_for_input(block_frames) * channels];
        let written = plugin
            .process(
                block,
                &mut cell,
                &ProcessContext::new(input_rate, block_frames),
            )
            .unwrap();
        output.extend_from_slice(&cell[..written * channels]);
        assert!(cell[written * channels..].iter().all(|x| x.is_nan()));
    }
    for _ in 0..64 {
        let mut cell = vec![f32::NAN; plugin.drain_output_frames_max() * channels];
        let result = plugin
            .drain(&mut cell, &ProcessContext::new(input_rate, 0))
            .unwrap();
        output.extend_from_slice(&cell[..result.frames * channels]);
        assert!(cell[result.frames * channels..].iter().all(|x| x.is_nan()));
        if result.complete {
            assert!(output.iter().all(|sample| sample.is_finite()));
            return output;
        }
    }
    panic!("drain did not complete");
}

fn measure(
    output: &[f32],
    channels: usize,
    channel: usize,
    rate: u32,
    frequency: f64,
) -> (f64, f64, f64) {
    let offset = rate as usize / 10;
    let count = rate as usize / 5;
    let samples = (0..count).map(|frame| output[(offset + frame) * channels + channel] as f64);
    let (sin_sum, cos_sum) =
        samples
            .clone()
            .enumerate()
            .fold((0.0, 0.0), |(s, c), (frame, value)| {
                let phase = TAU * frequency * frame as f64 / f64::from(rate);
                (s + value * phase.sin(), c + value * phase.cos())
            });
    let sin_gain = 2.0 * sin_sum / count as f64;
    let cos_gain = 2.0 * cos_sum / count as f64;
    let fitted_amplitude = sin_gain.hypot(cos_gain);
    let (energy, residual) =
        samples
            .enumerate()
            .fold((0.0, 0.0), |(energy, residual), (frame, value)| {
                let phase = TAU * frequency * frame as f64 / f64::from(rate);
                let error = value - sin_gain * phase.sin() - cos_gain * phase.cos();
                (energy + value * value, residual + error * error)
            });
    let input_rms = AMPLITUDE / 2.0_f64.sqrt();
    (
        20.0 * (fitted_amplitude / AMPLITUDE).log10(),
        20.0 * ((residual / count as f64).sqrt() / input_rms).log10(),
        20.0 * ((energy / count as f64).sqrt() / input_rms).log10(),
    )
}

#[test]
fn measured_ninety_six_to_twenty_four_sweep_matches_analytic() {
    // Known-weak large-decimation case (README documents -19/-22/-27 dB at
    // 12.36 kHz); this sweep turns characterization into acceptance with
    // the same pre-declared composite bound as the 48->24 sweep.
    let nyquist = 12_000.0;
    let fractions = [0.1, 0.9, 1.05, 1.2, 1.5];
    let frequencies: Vec<f64> = fractions.iter().map(|f| f * nyquist).collect();
    for quality in QUALITIES {
        let (taps, phases) = specs(quality);
        let input = tones(&frequencies, 96_000);
        let output = render_static(&input, frequencies.len(), 96_000, 24_000, quality);
        for (channel, &frequency) in frequencies.iter().enumerate() {
            let (gain_db, residual_db, _) =
                measure(&output, frequencies.len(), channel, 24_000, frequency);
            let expected = analytic_db(taps, phases, 0.25, frequency, 96_000);
            let amplitude = 10.0_f64.powf(gain_db / 20.0) * AMPLITUDE;
            let reference = 10.0_f64.powf(expected / 20.0) * AMPLITUDE;
            eprintln!(
                "RESAMPLER-FILTER {quality:?} 96->24 {frequency:.0}Hz: measured={gain_db:.3}dB analytic={expected:.3}dB residual={residual_db:.1}dB"
            );
            if channel == 0 {
                assert!(
                    gain_db.abs() < 0.01,
                    "{quality:?} flat gain {gain_db} dB exceeds 0.01 dB"
                );
                assert!(
                    residual_db < -90.0,
                    "{quality:?} flat residual {residual_db} dB exceeds -90 dB"
                );
            }
            assert!(
                (gain_db - expected).abs() < 0.5 || (amplitude - reference).abs() < 2e-6,
                "{quality:?} {frequency:.0}Hz: measured {gain_db:.3} dB vs analytic {expected:.3} dB"
            );
        }
    }
}
