//! Independent filter-response quantification for every quality preset.
//!
//! Reference: an f64 analytic DTFT of the declared periodic squared
//! four-term Blackman-Harris windowed sinc (window definition per
//! scipy.signal.windows.blackmanharris; table layout per SOTF_FORK.md),
//! transcribed independently of production taps. A second, algorithmically
//! independent f64 direct-convolution reference (512-tap single
//! Blackman-Harris, 2-8x longer than production) cross-checks end-to-end
//! amplitude on 44.1/48 kHz ratios. No production coefficient, window, or
//! FFT routine is used. Cutoff values use the shared
//! `rubato::calculate_cutoff` (also used by production); window, sinc,
//! table layout, and DTFT are independently transcribed, and the cutoff
//! itself is pinned by the existing `test_f_cutoff_is_quality_dependent`
//! unit test. Measured values print as RESAMPLER-FILTER lines.
//!
//! Bounds are pre-declared: passband ripple below 0.01 dB (the established
//! flat-band gain bound), transition-width narrowing Fast > Medium > High,
//! measured-vs-analytic agreement within 0.5 dB or 2e-6 absolute amplitude
//! (the established alias-amplitude bound), and reference amplitude
//! agreement within 0.05 dB. Existing tolerances are untouched.
//!
//! Error budgets (a-priori, P1-2): 0.5 dB = 0.01 dB flat-band ripple +
//! 0.04 dB coherent-fit measurement (200 ms window, 5 Hz-coherent tones, f32
//! output) + 0.45 dB conservative margin for transition-band modeling
//! (single-phase displacement, f32 cutoff rounding, above-Nyquist alias
//! sign); 2e-6 is the established alias-amplitude bound. 0.05 dB = 0.01 dB
//! production ripple + 0.01 dB reference ripple (512-tap single window) +
//! 0.02 dB cutoff-definition delta + 0.01 dB fit uncertainty.

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

fn specs(quality: ResamplerQuality) -> (usize, usize, f64) {
    // Taps, table phases, and tested flat-band fraction per the README.
    match quality {
        ResamplerQuality::Fast => (64, 128, 0.2),
        ResamplerQuality::Medium => (128, 256, 0.5),
        ResamplerQuality::High => (256, 256, 0.75),
    }
}

/// Analytic linear magnitude of one prepared table at one DTFT frequency.
///
/// Integer source-anchor steps select phase zero of the polyphase table;
/// agreement tests therefore use integer-step ratios (0.5, 0.25), where no
/// inter-phase interpolation occurs.
fn analytic_gain(taps: usize, phases: usize, ratio_scale: f64, freq_hz: f64, input_rate: u32) -> f64 {
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
    // The table layout reverses phase indices and normalizes the sum over
    // all prepared phases; phase zero carries a (P-1)/P displacement.
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
    plugin.initialize(input_rate).unwrap();
    let mut output = Vec::new();
    for block in input.chunks(CHUNK * channels) {
        let block_frames = block.len() / channels;
        let mut cell = vec![f32::NAN; plugin.output_frames_for_input(block_frames) * channels];
        let written = plugin
            .process(block, &mut cell, &ProcessContext::new(input_rate, block_frames))
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

/// Coherent least-squares tone fit: gain, residual, and total energy in dB.
/// Same windowing as spectral_accuracy.rs: 200 ms well inside the stream.
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
fn analytic_model_quantifies_transition_width_ripple_and_stopband() {
    // Transition width is defined from the analytic magnitude curve: the
    // span between the -3 dB and -60 dB crossings, as a fraction of output
    // Nyquist. Ripple is the peak absolute deviation over the tested
    // flat band. Stopband quality is the attenuation at 1.5x output
    // Nyquist. All values print for the published per-preset table.
    for ratio in [1.0, 0.5, 0.25] {
        let mut widths = Vec::new();
        for quality in QUALITIES {
            let (taps, phases, flat) = specs(quality);
            let input_rate = 48_000;
            let out_nyquist = ratio * f64::from(input_rate) / 2.0;
            let cutoff = f64::from(calculate_cutoff::<f32>(
                taps,
                WindowFunction::BlackmanHarris2,
            ));
            let mut ripple: f64 = 0.0;
            for point in 0..24 {
                let fraction = 0.05 * (flat / 0.05).powf(point as f64 / 23.0);
                let gain =
                    analytic_db(taps, phases, ratio.min(1.0), fraction * out_nyquist, input_rate);
                ripple = ripple.max(gain.abs());
            }
            let steps = 4000;
            let mut cross3: Option<f64> = None;
            let mut cross60: Option<f64> = None;
            let mut previous =
                analytic_db(taps, phases, ratio.min(1.0), 0.0, input_rate);
            for step in 1..=steps {
                let freq = f64::from(input_rate) / 2.0 * step as f64 / steps as f64;
                let gain = analytic_db(taps, phases, ratio.min(1.0), freq, input_rate);
                // Linear interpolation between 6 Hz scan samples.
                if cross3.is_none() && gain < -3.0 {
                    let base = freq - f64::from(input_rate) / 2.0 / steps as f64;
                    let weight = (-3.0 - previous) / (gain - previous);
                    cross3 = Some(base + weight * (freq - base));
                }
                if cross60.is_none() && gain < -60.0 {
                    let base = freq - f64::from(input_rate) / 2.0 / steps as f64;
                    let weight = (-60.0 - previous) / (gain - previous);
                    cross60 = Some(base + weight * (freq - base));
                }
                previous = gain;
            }
            let f3 = cross3.expect("passband edge must exist");
            let f60 = cross60.expect("stopband floor must be reached");
            let width = (f60 - f3) / out_nyquist;
            widths.push(width);
            let deep = analytic_db(taps, phases, ratio.min(1.0), 1.5 * out_nyquist, input_rate);
            eprintln!(
                "RESAMPLER-FILTER {quality:?} ratio={ratio}: f_cutoff={cutoff:.4} f-3dB={f3:.0}Hz f-60dB={f60:.0}Hz width={width:.4}Nyquist ripple={ripple:.6}dB deep150={deep:.1}dB"
            );
            assert!(
                ripple < 0.01,
                "{quality:?} ratio={ratio}: analytic ripple {ripple} dB exceeds the flat-band bound"
            );
        }
        assert!(
            widths[0] > widths[1] && widths[1] > widths[2],
            "ratio={ratio}: transition width must narrow Fast->Medium->High, got {widths:?}"
        );
    }
}

#[test]
fn measured_transition_sweep_matches_analytic_model() {
    // Static 48->24 kHz takes integer source steps, so single-phase
    // analysis applies. The flat point extends the 0.01 dB / -90 dB
    // flat-band bound to a new ratio; transition and deep-stop points
    // must track the analytic curve (composite bound: 0.5 dB, or 2e-6
    // absolute where measurement noise dominates deep attenuation).
    // Above-Nyquist points (12.6/14.4/18 kHz at a 24 kHz output clock)
    // alias identically in the plugin output and the fitted reference:
    // the sin/cos references at the nominal frequency equal the aliased
    // tone up to sign (f aliases to 24k - f), so the fitted magnitude
    // tracks the leakage amplitude, which the filter magnitude sets.
    // Every sweep frequency completes integer cycles in the 200 ms fit
    // window (all are multiples of 5 Hz), so no windowing bias leaks
    // between the fitted tone and the residual.
    let nyquist = 12_000.0;
    let fractions = [0.1, 0.9, 1.05, 1.2, 1.5];
    let frequencies: Vec<f64> = fractions.iter().map(|f| f * nyquist).collect();
    for quality in QUALITIES {
        let (taps, phases, _) = specs(quality);
        let input = tones(&frequencies, 48_000);
        let output = render_static(&input, frequencies.len(), 48_000, 24_000, quality);
        for (channel, &frequency) in frequencies.iter().enumerate() {
            let (gain_db, residual_db, _) =
                measure(&output, frequencies.len(), channel, 24_000, frequency);
            let expected = analytic_db(taps, phases, 0.5, frequency, 48_000);
            let amplitude = 10.0_f64.powf(gain_db / 20.0) * AMPLITUDE;
            let reference = 10.0_f64.powf(expected / 20.0) * AMPLITUDE;
            eprintln!(
                "RESAMPLER-FILTER {quality:?} 48->24 {frequency:.0}Hz: measured={gain_db:.3}dB analytic={expected:.3}dB residual={residual_db:.1}dB"
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

/// Algorithmically independent f64 resampling reference: direct
/// convolution with a 512-tap single Blackman-Harris windowed sinc
/// evaluated at exact fractional offsets, normalized per output phase.
/// Only fully supported outputs are returned. The single (not squared)
/// window, 2-8x length, direct fractional evaluation, and per-phase
/// normalization keep this reference independent of production's short
/// squared-window polyphase tables.
fn direct_reference(input: &[f32], ratio: f64) -> Vec<f64> {
    const TAPS: f64 = 512.0;
    // Passband covers the new Nyquist with margin.
    let cutoff = 0.95 * ratio.min(1.0) * 0.5;
    let tap = |offset: f64| {
        let angle = TAU * (offset + TAPS / 2.0) / TAPS;
        let window = 0.35875 - 0.48829 * angle.cos() + 0.14128 * (2.0 * angle).cos()
            - 0.01168 * (3.0 * angle).cos();
        let argument = 2.0 * cutoff * offset;
        let sinc = if argument == 0.0 {
            1.0
        } else {
            (std::f64::consts::PI * argument).sin() / (std::f64::consts::PI * argument)
        };
        window * sinc
    };
    let start = ((TAPS / 2.0 * ratio).ceil() as usize).max(1);
    let end = (((input.len() as f64 - 1.0 - TAPS / 2.0) * ratio).floor() as usize).max(start);
    (start..end)
        .map(|m| {
            let position = m as f64 / ratio;
            let first = (position - TAPS / 2.0).ceil() as isize;
            let last = (position + TAPS / 2.0).floor() as isize;
            let mut acc = 0.0;
            let mut gain = 0.0;
            for index in first..=last {
                if index >= 0 && (index as usize) < input.len() {
                    let weight = tap(index as f64 - position);
                    acc += f64::from(input[index as usize]) * weight;
                    gain += weight;
                }
            }
            acc / gain
        })
        .collect()
}

fn fit_amplitude_db(output: &[f64], rate: u32, frequency: f64) -> f64 {
    // Central 200 ms coherent fit; delay-independent amplitude comparison.
    let count = rate as usize / 5;
    let offset = output.len() / 2 - count / 2;
    let (sin_sum, cos_sum) = (0..count).fold((0.0, 0.0), |(s, c), frame| {
        let value = output[offset + frame];
        let phase = TAU * frequency * frame as f64 / f64::from(rate);
        (s + value * phase.sin(), c + value * phase.cos())
    });
    let amplitude = (2.0 * sin_sum / count as f64).hypot(2.0 * cos_sum / count as f64);
    20.0 * (amplitude / AMPLITUDE).log10()
}

#[test]
fn independent_direct_sinc_reference_agrees_on_rational_ratios() {
    // End-to-end amplitude agreement between production and the
    // independent f64 reference at 44.1<->48 kHz. Delay-independent:
    // only fitted tone amplitudes are compared, so no timing fit can
    // hide errors. Fast is excluded by its narrower documented flat
    // band and stays covered by the spectral suite.
    for (input_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
        let ratio = output_rate as f64 / input_rate as f64;
        // Report-only near-cutoff sweep point, beyond flat-band claims.
        let edge = f64::from(input_rate.min(output_rate)) / 2.0 * 0.9;
        let frequencies = [1_000.0, 5_000.0, edge];
        for quality in [ResamplerQuality::Medium, ResamplerQuality::High] {
            let input = tones(&frequencies, input_rate);
            let output = render_static(&input, frequencies.len(), input_rate, output_rate, quality);
            for (channel, &frequency) in frequencies.iter().enumerate() {
                let (gain_db, residual_db, _) =
                    measure(&output, frequencies.len(), channel, output_rate, frequency);
                let channel_input: Vec<f32> = input
                    .iter()
                    .skip(channel)
                    .step_by(frequencies.len())
                    .copied()
                    .collect();
                let reference = direct_reference(&channel_input, ratio);
                let expected = fit_amplitude_db(&reference, output_rate, frequency);
                eprintln!(
                    "RESAMPLER-FILTER {quality:?} {input_rate}->{output_rate} {frequency:.0}Hz: plugin={gain_db:.4}dB residual={residual_db:.1}dB reference={expected:.4}dB"
                );
                if channel < 2 {
                    assert!(
                        (gain_db - expected).abs() < 0.05,
                        "{quality:?} {input_rate}->{output_rate} {frequency:.0}Hz: {gain_db:.4} vs {expected:.4} dB"
                    );
                }
            }
        }
    }
}
