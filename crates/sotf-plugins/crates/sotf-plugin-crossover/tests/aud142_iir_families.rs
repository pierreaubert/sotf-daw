//! Independent response, routing, and lifecycle checks for AUD142 IIR families.

// Rust guideline compliant 2026-02-21

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_crossover::{CrossoverPlugin, PerChannelOpMode, params::CROSSOVER_TYPES};
use std::f64::consts::TAU;

const INPUT_AMPLITUDE: f64 = 0.25;
const COMPLEX_ERROR_LIMIT: f64 = 0.002;
const SETTLED_RMS_ERROR_LIMIT: f64 = 0.002;
const CONVERGENCE_LIMIT: f64 = 0.00025;
const FAMILY_TYPES: &[&str] = &[
    "LR12", "LR48", "BW6", "BW12", "BW18", "BW24", "BW30", "BW36", "BW42", "BW48", "Bessel12",
];
const BLOCK_PATTERN: &[usize] = &[1, 7, 31, 127, 512, 19];

#[derive(Clone, Copy, Debug, Default)]
struct Complex64 {
    real: f64,
    imaginary: f64,
}

impl Complex64 {
    const ZERO: Self = Self {
        real: 0.0,
        imaginary: 0.0,
    };
    const ONE: Self = Self {
        real: 1.0,
        imaginary: 0.0,
    };

    fn new(real: f64, imaginary: f64) -> Self {
        Self { real, imaginary }
    }

    fn abs(self) -> f64 {
        self.real.hypot(self.imaginary)
    }

    fn reciprocal(self) -> Self {
        let denominator = self.real * self.real + self.imaginary * self.imaginary;
        Self::new(self.real / denominator, -self.imaginary / denominator)
    }

    fn scale(self, scale: f64) -> Self {
        Self::new(self.real * scale, self.imaginary * scale)
    }

    fn add(self, rhs: Self) -> Self {
        Self::new(self.real + rhs.real, self.imaginary + rhs.imaginary)
    }

    fn subtract(self, rhs: Self) -> Self {
        Self::new(self.real - rhs.real, self.imaginary - rhs.imaginary)
    }

    fn multiply(self, rhs: Self) -> Self {
        Self::new(
            self.real * rhs.real - self.imaginary * rhs.imaginary,
            self.real * rhs.imaginary + self.imaginary * rhs.real,
        )
    }

    fn divide(self, rhs: Self) -> Self {
        self.multiply(rhs.reciprocal())
    }
}

#[derive(Clone, Copy, Debug)]
enum Branch {
    Low,
    High,
}

fn order_and_repeats(kind: &str) -> Option<(usize, usize)> {
    match kind {
        "LR12" => Some((1, 2)),
        "LR48" => Some((4, 2)),
        "BW6" => Some((1, 1)),
        "BW12" => Some((2, 1)),
        "BW18" => Some((3, 1)),
        "BW24" => Some((4, 1)),
        "BW30" => Some((5, 1)),
        "BW36" => Some((6, 1)),
        "BW42" => Some((7, 1)),
        "BW48" => Some((8, 1)),
        _ => None,
    }
}

fn butterworth_poles(order: usize) -> Vec<Complex64> {
    (0..order)
        .map(|index| {
            let angle = std::f64::consts::PI * (2 * index + order + 1) as f64 / (2 * order) as f64;
            Complex64::new(angle.cos(), angle.sin())
        })
        .collect()
}

fn bessel_poles() -> Vec<Complex64> {
    let a = (3.0 * (5.0_f64.sqrt() - 1.0) / 2.0).sqrt();
    vec![
        Complex64::new(-3.0 / (2.0 * a), 3.0_f64.sqrt() / (2.0 * a)),
        Complex64::new(-3.0 / (2.0 * a), -3.0_f64.sqrt() / (2.0 * a)),
    ]
}

fn butterworth_lowpass(order: usize, normalized_frequency: Complex64) -> Complex64 {
    let mut numerator = Complex64::ONE;
    let mut denominator = Complex64::ONE;
    for pole in butterworth_poles(order) {
        numerator = numerator.multiply(pole.scale(-1.0));
        denominator = denominator.multiply(normalized_frequency.subtract(pole));
    }
    numerator.divide(denominator)
}

fn bessel_lowpass(normalized_frequency: Complex64) -> Complex64 {
    let a = (3.0 * (5.0_f64.sqrt() - 1.0) / 2.0).sqrt();
    let scaled = normalized_frequency.scale(a);
    Complex64::new(3.0, 0.0).divide(
        scaled
            .multiply(scaled)
            .add(scaled.scale(3.0))
            .add(Complex64::new(3.0, 0.0)),
    )
}

fn family_response_at_u(kind: &str, normalized_frequency: Complex64, branch: Branch) -> Complex64 {
    if kind == "Bessel12" {
        return match branch {
            Branch::Low => bessel_lowpass(normalized_frequency),
            Branch::High => bessel_lowpass(normalized_frequency.reciprocal()),
        };
    }

    let (order, repeats) = order_and_repeats(kind).expect("known IIR family");
    let argument = match branch {
        Branch::Low => normalized_frequency,
        Branch::High => normalized_frequency.reciprocal(),
    };
    let mut response = butterworth_lowpass(order, argument);
    for _ in 1..repeats {
        response = response.multiply(butterworth_lowpass(order, argument));
    }
    if kind == "LR12" && matches!(branch, Branch::High) {
        response.scale(-1.0)
    } else {
        response
    }
}

fn family_response(
    kind: &str,
    cutoff: f64,
    sample_rate: u32,
    frequency: f64,
    branch: Branch,
) -> Complex64 {
    if frequency == 0.0 {
        return match branch {
            Branch::Low => Complex64::ONE,
            Branch::High => Complex64::ZERO,
        };
    }
    if frequency == f64::from(sample_rate) * 0.5 {
        let high_gain = if kind == "LR12" { -1.0 } else { 1.0 };
        return match branch {
            Branch::Low => Complex64::ZERO,
            Branch::High => Complex64::new(high_gain, 0.0),
        };
    }
    let prewarp_cutoff = (std::f64::consts::PI * cutoff / f64::from(sample_rate)).tan();
    let prewarp_frequency = (std::f64::consts::PI * frequency / f64::from(sample_rate)).tan();
    family_response_at_u(
        kind,
        Complex64::new(0.0, prewarp_frequency / prewarp_cutoff),
        branch,
    )
}

fn family_band_responses(
    kind: &str,
    cutoffs: &[f64],
    sample_rate: u32,
    frequency: f64,
) -> Vec<Complex64> {
    let is_lr = kind.starts_with("LR");
    let responses: Vec<(Complex64, Complex64)> = cutoffs
        .iter()
        .map(|&cutoff| {
            (
                family_response(kind, cutoff, sample_rate, frequency, Branch::Low),
                family_response(kind, cutoff, sample_rate, frequency, Branch::High),
            )
        })
        .collect();
    let mut bands = Vec::with_capacity(cutoffs.len() + 1);
    for band in 0..cutoffs.len() {
        let mut response = Complex64::ONE;
        for (_, high) in responses.iter().take(band) {
            response = response.multiply(*high);
        }
        response = response.multiply(responses[band].0);
        if is_lr {
            for (low, high) in responses.iter().skip(band + 1) {
                response = response.multiply(low.add(*high));
            }
        }
        bands.push(response);
    }
    let mut final_band = Complex64::ONE;
    for (_, high) in responses {
        final_band = final_band.multiply(high);
    }
    bands.push(final_band);
    bands
}

fn family_analog_poles(kind: &str) -> (Vec<Complex64>, usize) {
    if kind == "Bessel12" {
        return (bessel_poles(), 1);
    }
    let (order, repeats) = order_and_repeats(kind).expect("known IIR family");
    (butterworth_poles(order), repeats)
}

/// Computes a bare-pole decay minimum; repeated-pole residues are checked by convergence windows.
fn settling_minimum_frames(kind: &str, cutoff: f64, sample_rate: u32) -> usize {
    let (lowpass_poles, repeats) = family_analog_poles(kind);
    let prewarp = (std::f64::consts::PI * cutoff / f64::from(sample_rate)).tan();
    let mut maximum_radius = 0.0_f64;
    for pole in lowpass_poles {
        for analog_pole in [pole, pole.reciprocal()] {
            let scaled_pole = analog_pole.scale(prewarp);
            let digital_pole = Complex64::ONE
                .add(scaled_pole)
                .divide(Complex64::ONE.subtract(scaled_pole));
            maximum_radius = maximum_radius.max(digital_pole.abs());
        }
    }
    assert!(
        maximum_radius < 1.0,
        "unstable independent oracle pole estimate"
    );
    let bare_decay = (1.0e-5_f64.ln() / maximum_radius.ln()).ceil() as usize;
    // LR squares the prototype. The 2x factor is conservative but not treated
    // as a residue bound: measured consecutive phasor windows must also converge.
    bare_decay
        .saturating_mul(repeats)
        .max(sample_rate as usize / 4)
}

fn make_plugin(kind: &str, sample_rate: u32, cutoff: f64, output: &str) -> CrossoverPlugin {
    let mut plugin = CrossoverPlugin::new(1, kind, cutoff, output).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    plugin
}

fn process_signal(
    plugin: &mut CrossoverPlugin,
    sample_rate: u32,
    start_frame: usize,
    frames: usize,
    frequency: f64,
    output_channels: usize,
    capture: bool,
) -> Vec<f64> {
    let mut captured = if capture {
        Vec::with_capacity(frames * output_channels)
    } else {
        Vec::new()
    };
    let mut processed = 0;
    let mut block_index = 0;
    while processed < frames {
        let count = BLOCK_PATTERN[block_index % BLOCK_PATTERN.len()].min(frames - processed);
        let input: Vec<f32> = (0..count)
            .map(|offset| {
                let sample = start_frame + processed + offset;
                (INPUT_AMPLITUDE * (TAU * frequency * sample as f64 / f64::from(sample_rate)).cos())
                    as f32
            })
            .collect();
        let mut output = vec![f32::NAN; count * output_channels];
        let processed_frames = plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        assert_eq!(processed_frames, count, "callback frame count changed");
        assert_eq!(output.len(), count * output_channels);
        assert!(output.iter().all(|sample| sample.is_finite()));
        if capture {
            captured.extend(output.into_iter().map(f64::from));
        }
        processed += count;
        block_index += 1;
    }
    captured
}

fn fit_channel(
    interleaved: &[f64],
    channels: usize,
    channel: usize,
    start_frame: usize,
    frequency: f64,
    sample_rate: u32,
) -> Complex64 {
    let frames = interleaved.len() / channels;
    let mut cos_cos = 0.0;
    let mut sin_sin = 0.0;
    let mut cos_sin = 0.0;
    let mut sample_cos = 0.0;
    let mut sample_sin = 0.0;
    for frame in 0..frames {
        let phase = TAU * frequency * (start_frame + frame) as f64 / f64::from(sample_rate);
        let cosine = phase.cos();
        let sine = phase.sin();
        let sample = interleaved[frame * channels + channel] / INPUT_AMPLITUDE;
        cos_cos += cosine * cosine;
        sin_sin += sine * sine;
        cos_sin += cosine * sine;
        sample_cos += sample * cosine;
        sample_sin += sample * sine;
    }
    let determinant = cos_cos * sin_sin - cos_sin * cos_sin;
    assert!(
        determinant > 0.0,
        "response window is too short to fit a sinusoid"
    );
    let real = (sample_cos * sin_sin - sample_sin * cos_sin) / determinant;
    let sine_coefficient = (sample_sin * cos_cos - sample_cos * cos_sin) / determinant;
    Complex64::new(real, -sine_coefficient)
}

fn assert_complex_close(label: &str, actual: Complex64, expected: Complex64) {
    let error = actual.subtract(expected).abs();
    assert!(
        error <= COMPLEX_ERROR_LIMIT,
        "{label}: actual={actual:?}, expected={expected:?}, complex_error={error}"
    );
}

fn expected_cosine_sample(response: Complex64, phase: f64) -> f64 {
    response.real * phase.cos() - response.imaginary * phase.sin()
}

fn assert_window_rms_error(
    label: &str,
    interleaved: &[f64],
    channels: usize,
    start_frame: usize,
    frequency: f64,
    sample_rate: u32,
    expected: &[Complex64],
) {
    assert_eq!(interleaved.len() % channels, 0);
    assert_eq!(expected.len(), channels);
    let frames = interleaved.len() / channels;
    let mut input_energy = 0.0;
    let mut error_energy = vec![0.0; channels];
    for frame in 0..frames {
        let phase = TAU * frequency * (start_frame + frame) as f64 / f64::from(sample_rate);
        let input = phase.cos();
        input_energy += input * input;
        for channel in 0..channels {
            let actual = interleaved[frame * channels + channel] / INPUT_AMPLITUDE;
            let error = actual - expected_cosine_sample(expected[channel], phase);
            error_energy[channel] += error * error;
        }
    }
    let input_rms = (input_energy / frames as f64).sqrt();
    assert!(input_rms > 0.0, "{label}: input window has zero RMS");
    for (channel, energy) in error_energy.into_iter().enumerate() {
        let normalized_rms = (energy / frames as f64).sqrt() / input_rms;
        assert!(
            normalized_rms <= SETTLED_RMS_ERROR_LIMIT,
            "{label} channel {channel}: input-RMS-normalized settled error {normalized_rms} exceeds {SETTLED_RMS_ERROR_LIMIT}"
        );
    }
}

fn assert_sum_window_rms_error(
    label: &str,
    interleaved: &[f64],
    channels: usize,
    start_frame: usize,
    frequency: f64,
    sample_rate: u32,
    expected_sum: Complex64,
) {
    assert_eq!(interleaved.len() % channels, 0);
    let frames = interleaved.len() / channels;
    let mut input_energy = 0.0;
    let mut error_energy = 0.0;
    for frame in 0..frames {
        let phase = TAU * frequency * (start_frame + frame) as f64 / f64::from(sample_rate);
        let input = phase.cos();
        input_energy += input * input;
        let actual_sum = interleaved[frame * channels..(frame + 1) * channels]
            .iter()
            .map(|sample| sample / INPUT_AMPLITUDE)
            .sum::<f64>();
        let error = actual_sum - expected_cosine_sample(expected_sum, phase);
        error_energy += error * error;
    }
    let input_rms = (input_energy / frames as f64).sqrt();
    assert!(input_rms > 0.0, "{label}: input window has zero RMS");
    let normalized_rms = (error_energy / frames as f64).sqrt() / input_rms;
    assert!(
        normalized_rms <= SETTLED_RMS_ERROR_LIMIT,
        "{label}: input-RMS-normalized settled sum error {normalized_rms} exceeds {SETTLED_RMS_ERROR_LIMIT}"
    );
}

fn measure_converged_two_way(
    plugin: &mut CrossoverPlugin,
    kind: &str,
    cutoff: f64,
    probe_frequency: f64,
    sample_rate: u32,
) -> [Complex64; 2] {
    let settling = settling_minimum_frames(kind, cutoff, sample_rate);
    process_signal(plugin, sample_rate, 0, settling, probe_frequency, 2, false);
    let window = (sample_rate as usize / 4).max(8_192);
    let mut previous: Option<[Complex64; 2]> = None;
    let mut converged = None;
    let mut consecutive_small_deltas = 0;
    for window_index in 0..6 {
        let start = settling + window_index * window;
        let samples = process_signal(plugin, sample_rate, start, window, probe_frequency, 2, true);
        let response = [
            fit_channel(&samples, 2, 0, start, probe_frequency, sample_rate),
            fit_channel(&samples, 2, 1, start, probe_frequency, sample_rate),
        ];
        if let Some(old) = previous {
            let low_delta = response[0].subtract(old[0]).abs();
            let high_delta = response[1].subtract(old[1]).abs();
            if low_delta <= CONVERGENCE_LIMIT && high_delta <= CONVERGENCE_LIMIT {
                consecutive_small_deltas += 1;
                if consecutive_small_deltas == 2 {
                    let expected = [
                        family_response(kind, cutoff, sample_rate, probe_frequency, Branch::Low),
                        family_response(kind, cutoff, sample_rate, probe_frequency, Branch::High),
                    ];
                    assert_window_rms_error(
                        kind,
                        &samples,
                        2,
                        start,
                        probe_frequency,
                        sample_rate,
                        &expected,
                    );
                    assert_sum_window_rms_error(
                        kind,
                        &samples,
                        2,
                        start,
                        probe_frequency,
                        sample_rate,
                        expected[0].add(expected[1]),
                    );
                    converged = Some(response);
                    break;
                }
            } else {
                consecutive_small_deltas = 0;
            }
        }
        previous = Some(response);
    }
    converged.expect("three consecutive output windows must converge after the pole minimum")
}

#[test]
fn canonical_choices_append_without_reindexing_the_two_existing_entries() {
    assert_eq!(
        CROSSOVER_TYPES,
        &[
            "LR24",
            "LinearPhase",
            "LR12",
            "LR48",
            "BW6",
            "BW12",
            "BW18",
            "BW24",
            "BW30",
            "BW36",
            "BW42",
            "BW48",
            "Bessel12",
        ]
    );
    for kind in FAMILY_TYPES {
        assert!(
            CrossoverPlugin::new(1, kind, 1_000.0, "both").is_ok(),
            "{kind}"
        );
    }
}

#[test]
fn all_new_two_way_responses_match_independent_analog_pole_products() {
    for &kind in FAMILY_TYPES {
        for sample_rate in [44_100, 48_000, 96_000] {
            for cutoff in [20.0, 1_000.0, 18_000.0] {
                let nyquist = f64::from(sample_rate) * 0.5;
                let probes = [cutoff * 0.5, cutoff, (cutoff + nyquist) * 0.5];
                for probe_frequency in probes {
                    let mut plugin = make_plugin(kind, sample_rate, cutoff, "both");
                    let measured = measure_converged_two_way(
                        &mut plugin,
                        kind,
                        cutoff,
                        probe_frequency,
                        sample_rate,
                    );
                    let expected_low =
                        family_response(kind, cutoff, sample_rate, probe_frequency, Branch::Low);
                    let expected_high =
                        family_response(kind, cutoff, sample_rate, probe_frequency, Branch::High);
                    assert_complex_close(
                        &format!("{kind} low at fs={sample_rate} fc={cutoff} f={probe_frequency}"),
                        measured[0],
                        expected_low,
                    );
                    assert_complex_close(
                        &format!("{kind} high at fs={sample_rate} fc={cutoff} f={probe_frequency}"),
                        measured[1],
                        expected_high,
                    );
                    if probe_frequency == cutoff {
                        let cutoff_magnitude = if kind.starts_with("LR") {
                            0.5
                        } else {
                            std::f64::consts::FRAC_1_SQRT_2
                        };
                        assert!((expected_low.abs() - cutoff_magnitude).abs() < 1e-12);
                        assert!((expected_high.abs() - cutoff_magnitude).abs() < 1e-12);
                    } else if probe_frequency < cutoff {
                        assert!(
                            expected_low.abs() > expected_high.abs(),
                            "{kind} below cutoff"
                        );
                    } else {
                        assert!(
                            expected_high.abs() > expected_low.abs(),
                            "{kind} above cutoff"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn all_new_family_endpoints_and_high_band_polarities_are_explicit() {
    const SAMPLE_RATE: u32 = 48_000;
    const CUTOFF: f64 = 1_000.0;
    for &kind in FAMILY_TYPES {
        let mut plugin = make_plugin(kind, SAMPLE_RATE, CUTOFF, "both");
        let dc = process_signal(&mut plugin, SAMPLE_RATE, 0, 8_192, 0.0, 2, true);
        let dc_start = 8_192 - 1_024;
        let (dc_frames, dc_remainder) = dc.as_slice().as_chunks::<2>();
        assert!(dc_remainder.is_empty());
        let low_dc = dc_frames[dc_start..]
            .iter()
            .map(|frame| frame[0])
            .sum::<f64>()
            / 1_024.0
            / INPUT_AMPLITUDE;
        let high_dc = dc_frames[dc_start..]
            .iter()
            .map(|frame| frame[1])
            .sum::<f64>()
            / 1_024.0
            / INPUT_AMPLITUDE;
        assert!((low_dc - 1.0).abs() < 2e-4, "{kind} low DC gain {low_dc}");
        assert!(high_dc.abs() < 2e-4, "{kind} high DC gain {high_dc}");

        plugin.reset();
        let nyquist = process_signal(
            &mut plugin,
            SAMPLE_RATE,
            0,
            8_192,
            f64::from(SAMPLE_RATE) * 0.5,
            2,
            true,
        );
        let nyquist_start = 8_192 - 1_024;
        let mut low_gain = 0.0;
        let mut high_gain = 0.0;
        let (nyquist_frames, nyquist_remainder) = nyquist.as_slice().as_chunks::<2>();
        assert!(nyquist_remainder.is_empty());
        for (offset, frame) in nyquist_frames.iter().enumerate().skip(nyquist_start) {
            let input_sign = if offset % 2 == 0 { 1.0 } else { -1.0 };
            low_gain += frame[0] * input_sign;
            high_gain += frame[1] * input_sign;
        }
        low_gain /= 1_024.0 * INPUT_AMPLITUDE;
        high_gain /= 1_024.0 * INPUT_AMPLITUDE;
        let high_expected = if kind == "LR12" { -1.0 } else { 1.0 };
        assert!(low_gain.abs() < 2e-4, "{kind} low Nyquist gain {low_gain}");
        assert!(
            (high_gain - high_expected).abs() < 2e-4,
            "{kind} high Nyquist gain {high_gain}"
        );
    }
}

#[test]
fn multiway_lr_compensation_and_serial_non_lr_bands_match_their_products() {
    for kind in ["LR12", "LR48", "BW48", "Bessel12"] {
        const SAMPLE_RATE: u32 = 48_000;
        const CUTOFFS: [f64; 3] = [300.0, 2_500.0, 9_000.0];
        const PROBE: f64 = 1_200.0;
        let extra = [CUTOFFS[1], CUTOFFS[2]];
        let mut plugin =
            CrossoverPlugin::new_multiway(1, kind, CUTOFFS[0], "both", &extra).unwrap();
        plugin.initialize(SAMPLE_RATE).unwrap();
        let settling = CUTOFFS
            .iter()
            .map(|&cutoff| settling_minimum_frames(kind, cutoff, SAMPLE_RATE))
            .max()
            .unwrap();
        process_signal(&mut plugin, SAMPLE_RATE, 0, settling, PROBE, 4, false);
        let window = SAMPLE_RATE as usize / 4;
        let mut previous: Option<Vec<Complex64>> = None;
        let mut converged = None;
        let mut consecutive_small_deltas = 0;
        for window_index in 0..6 {
            let start = settling + window_index * window;
            let samples = process_signal(&mut plugin, SAMPLE_RATE, start, window, PROBE, 4, true);
            let response: Vec<Complex64> = (0..4)
                .map(|band| fit_channel(&samples, 4, band, start, PROBE, SAMPLE_RATE))
                .collect();
            if let Some(old) = previous.as_ref() {
                let max_delta = response
                    .iter()
                    .zip(old)
                    .map(|(new, previous)| new.subtract(*previous).abs())
                    .fold(0.0_f64, f64::max);
                if max_delta <= CONVERGENCE_LIMIT {
                    consecutive_small_deltas += 1;
                    if consecutive_small_deltas == 2 {
                        let expected = family_band_responses(kind, &CUTOFFS, SAMPLE_RATE, PROBE);
                        assert_window_rms_error(
                            kind,
                            &samples,
                            4,
                            start,
                            PROBE,
                            SAMPLE_RATE,
                            &expected,
                        );
                        let expected_sum = expected
                            .iter()
                            .copied()
                            .fold(Complex64::ZERO, Complex64::add);
                        assert_sum_window_rms_error(
                            kind,
                            &samples,
                            4,
                            start,
                            PROBE,
                            SAMPLE_RATE,
                            expected_sum,
                        );
                        converged = Some(response);
                        break;
                    }
                } else {
                    consecutive_small_deltas = 0;
                }
            }
            previous = Some(response);
        }
        let measured = converged.expect(
            "three consecutive multiway phasor windows must converge after the pole minimum",
        );
        let expected = family_band_responses(kind, &CUTOFFS, SAMPLE_RATE, PROBE);
        for band in 0..4 {
            assert_complex_close(
                &format!("{kind} multiway band {band}"),
                measured[band],
                expected[band],
            );
        }
        let measured_sum = measured
            .iter()
            .copied()
            .fold(Complex64::ZERO, Complex64::add);
        let expected_sum = expected
            .iter()
            .copied()
            .fold(Complex64::ZERO, Complex64::add);
        assert_complex_close(
            &format!("{kind} actual multiband sum"),
            measured_sum,
            expected_sum,
        );
        if kind.starts_with("LR") {
            let allpass_sum = CUTOFFS
                .iter()
                .map(|&cutoff| {
                    family_response(kind, cutoff, SAMPLE_RATE, PROBE, Branch::Low).add(
                        family_response(kind, cutoff, SAMPLE_RATE, PROBE, Branch::High),
                    )
                })
                .fold(Complex64::ONE, Complex64::multiply);
            assert!((measured_sum.abs() - 1.0).abs() <= COMPLEX_ERROR_LIMIT);
            assert_complex_close(
                &format!("{kind} LR signed allpass sum"),
                measured_sum,
                allpass_sum,
            );
        }
    }
}

#[test]
fn new_family_modes_keep_width_and_per_channel_modes_keep_channel_order() {
    const SAMPLE_RATE: u32 = 48_000;
    const CUTS: [f64; 3] = [250.0, 2_000.0, 8_000.0];
    const FRAMES: usize = 512;
    let input: Vec<f32> = (0..FRAMES)
        .map(|frame| (0.3 * (TAU * 743.0 * frame as f64 / f64::from(SAMPLE_RATE)).cos()) as f32)
        .collect();

    for &kind in FAMILY_TYPES {
        let mut both =
            CrossoverPlugin::new_multiway(1, kind, CUTS[0], "both", &[CUTS[1], CUTS[2]]).unwrap();
        let mut low =
            CrossoverPlugin::new_multiway(1, kind, CUTS[0], "low", &[CUTS[1], CUTS[2]]).unwrap();
        let mut high =
            CrossoverPlugin::new_multiway(1, kind, CUTS[0], "high", &[CUTS[1], CUTS[2]]).unwrap();
        both.initialize(SAMPLE_RATE).unwrap();
        low.initialize(SAMPLE_RATE).unwrap();
        high.initialize(SAMPLE_RATE).unwrap();
        assert_eq!(both.output_channels(), 4, "{kind} Both width");
        assert_eq!(low.output_channels(), 1, "{kind} Low width");
        assert_eq!(high.output_channels(), 1, "{kind} High width");

        let mut both_output = vec![0.0; FRAMES * 4];
        let mut low_output = vec![0.0; FRAMES];
        let mut high_output = vec![0.0; FRAMES];
        let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
        assert_eq!(
            both.process(&input, &mut both_output, &context).unwrap(),
            FRAMES
        );
        assert_eq!(
            low.process(&input, &mut low_output, &context).unwrap(),
            FRAMES
        );
        assert_eq!(
            high.process(&input, &mut high_output, &context).unwrap(),
            FRAMES
        );
        assert!(both_output.iter().all(|sample| sample.is_finite()));
        assert!(low_output.iter().all(|sample| sample.is_finite()));
        assert!(high_output.iter().all(|sample| sample.is_finite()));
        for frame in 0..FRAMES {
            assert_eq!(low_output[frame], both_output[frame * 4]);
            assert_eq!(high_output[frame], both_output[frame * 4 + 3]);
        }

        let mut per_channel = CrossoverPlugin::new_per_channel(
            kind,
            vec![500.0, 750.0, 1_000.0, 1_250.0],
            vec![
                PerChannelOpMode::Lowpass,
                PerChannelOpMode::Highpass,
                PerChannelOpMode::Mute,
                PerChannelOpMode::Passthrough,
            ],
        )
        .unwrap();
        per_channel.initialize(SAMPLE_RATE).unwrap();
        assert_eq!(per_channel.output_channels(), 4);
        let interleaved_input: Vec<f32> = (0..FRAMES)
            .flat_map(|frame| {
                let sample =
                    (0.3 * (TAU * 743.0 * frame as f64 / f64::from(SAMPLE_RATE)).cos()) as f32;
                [sample, -sample, sample * 0.5, sample * 0.25]
            })
            .collect();
        let mut per_channel_output = vec![0.0; FRAMES * 4];
        assert_eq!(
            per_channel
                .process(
                    &interleaved_input,
                    &mut per_channel_output,
                    &ProcessContext::new(SAMPLE_RATE, FRAMES),
                )
                .unwrap(),
            FRAMES
        );
        assert!(per_channel_output.iter().all(|sample| sample.is_finite()));

        let (interleaved_frames, interleaved_remainder) =
            interleaved_input.as_slice().as_chunks::<4>();
        assert!(interleaved_remainder.is_empty());
        let low_input: Vec<f32> = interleaved_frames.iter().map(|frame| frame[0]).collect();
        let high_input: Vec<f32> = interleaved_frames.iter().map(|frame| frame[1]).collect();
        let mut low_reference = make_plugin(kind, SAMPLE_RATE, 500.0, "low");
        let mut high_reference = make_plugin(kind, SAMPLE_RATE, 750.0, "high");
        let mut low_reference_output = vec![0.0; FRAMES];
        let mut high_reference_output = vec![0.0; FRAMES];
        let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
        assert_eq!(
            low_reference
                .process(&low_input, &mut low_reference_output, &context)
                .unwrap(),
            FRAMES
        );
        assert_eq!(
            high_reference
                .process(&high_input, &mut high_reference_output, &context)
                .unwrap(),
            FRAMES
        );
        assert!(low_reference_output.iter().all(|sample| sample.is_finite()));
        assert!(
            high_reference_output
                .iter()
                .all(|sample| sample.is_finite())
        );
        for frame in 0..FRAMES {
            assert_eq!(
                per_channel_output[frame * 4],
                low_reference_output[frame],
                "{kind} per-channel low route must match a 500 Hz low-only instance"
            );
            assert_eq!(
                per_channel_output[frame * 4 + 1],
                high_reference_output[frame],
                "{kind} per-channel high route must match a 750 Hz high-only instance"
            );
            assert_eq!(per_channel_output[frame * 4 + 2], 0.0);
            assert_eq!(
                per_channel_output[frame * 4 + 3],
                interleaved_input[frame * 4 + 3]
            );
        }
    }
}

#[test]
fn cutoff_validation_is_atomic_and_valid_reinitialization_matches_a_fresh_instance() {
    let mut test = make_plugin("LR48", 48_000, 4_800.0, "both");
    let mut twin = make_plugin("LR48", 48_000, 4_800.0, "both");
    let warm = vec![0.2_f32; 2_048];
    let mut test_warm = vec![0.0; warm.len() * 2];
    let mut twin_warm = vec![0.0; warm.len() * 2];
    let warm_context = ProcessContext::new(48_000, warm.len());
    assert_eq!(
        test.process(&warm, &mut test_warm, &warm_context).unwrap(),
        warm.len()
    );
    assert_eq!(
        twin.process(&warm, &mut twin_warm, &warm_context).unwrap(),
        warm.len()
    );

    assert!(test.initialize(8_000).is_err());
    let continuation = vec![0.17_f32; 1_024];
    let mut test_output = vec![0.0; continuation.len() * 2];
    let mut twin_output = vec![0.0; continuation.len() * 2];
    let context = ProcessContext::new(48_000, continuation.len());
    assert_eq!(
        test.process(&continuation, &mut test_output, &context)
            .unwrap(),
        continuation.len()
    );
    assert_eq!(
        twin.process(&continuation, &mut twin_output, &context)
            .unwrap(),
        continuation.len()
    );
    assert!(test_output.iter().all(|sample| sample.is_finite()));
    assert!(twin_output.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        test_output, twin_output,
        "rejected sample rate must preserve state"
    );

    assert!(CrossoverPlugin::new(1, "BW24", 19.0, "both").is_err());
    assert!(CrossoverPlugin::new(1, "BW24", 20_001.0, "both").is_err());
    assert!(CrossoverPlugin::new_multiway(1, "BW24", 1_000.0, "both", &[500.0]).is_err());
    assert!(CrossoverPlugin::new_multiway(1, "BW24", 500.0, "both", &[500.0]).is_err());

    let mut populated = make_plugin("Bessel12", 48_000, 3_000.0, "both");
    let prior = vec![0.12_f32; 512];
    let mut prior_out = vec![0.0; prior.len() * 2];
    assert_eq!(
        populated
            .process(
                &prior,
                &mut prior_out,
                &ProcessContext::new(48_000, prior.len())
            )
            .unwrap(),
        prior.len()
    );
    assert!(prior_out.iter().all(|sample| sample.is_finite()));
    populated.initialize(44_100).unwrap();
    let mut fresh = make_plugin("Bessel12", 44_100, 3_000.0, "both");
    let next = vec![0.19_f32; 2_048];
    let mut populated_output = vec![0.0; next.len() * 2];
    let mut fresh_output = vec![0.0; next.len() * 2];
    let context = ProcessContext::new(44_100, next.len());
    assert_eq!(
        populated
            .process(&next, &mut populated_output, &context)
            .unwrap(),
        next.len()
    );
    assert_eq!(
        fresh.process(&next, &mut fresh_output, &context).unwrap(),
        next.len()
    );
    assert!(populated_output.iter().all(|sample| sample.is_finite()));
    assert!(fresh_output.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        populated_output, fresh_output,
        "valid reinit must reset to target state"
    );

    let old_frequency = populated.get_parameter(&ParameterId::from("frequency"));
    assert!(
        populated
            .set_parameter(ParameterId::from("frequency"), ParameterValue::Float(10.0),)
            .is_err()
    );
    assert_eq!(
        populated.get_parameter(&ParameterId::from("frequency")),
        old_frequency
    );
}

#[test]
fn automation_is_partition_invariant_finite_and_structural_mode_is_rejected() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 24_000;
    const CHANGE_AT: usize = 4_093;
    let input: Vec<f32> = (0..FRAMES)
        .map(|frame| {
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            (0.35 * (TAU * 997.0 * time).sin() + 0.12 * (TAU * 8_201.0 * time).cos()) as f32
        })
        .collect();
    let mut regular = make_plugin("LR48", SAMPLE_RATE, 900.0, "both");
    let mut irregular = make_plugin("LR48", SAMPLE_RATE, 900.0, "both");
    let regular_output = render_automated(
        &mut regular,
        &input,
        SAMPLE_RATE,
        &[256, 512, 1_024],
        CHANGE_AT,
    );
    let irregular_output = render_automated(
        &mut irregular,
        &input,
        SAMPLE_RATE,
        &[1, 7, 31, 127, 513],
        CHANGE_AT,
    );
    let input_peak = input
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let output_peak = regular_output
        .iter()
        .chain(&irregular_output)
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(regular_output.iter().all(|sample| sample.is_finite()));
    assert!(irregular_output.iter().all(|sample| sample.is_finite()));
    assert!(
        f64::from(output_peak) <= 8.0 * f64::from(input_peak) + 1e-6,
        "automation peak {output_peak} exceeds input-relative bound for input peak {input_peak}"
    );
    let normalized_partition_error = regular_output
        .iter()
        .zip(&irregular_output)
        .map(|(left, right)| f64::from((left - right).abs()) / f64::from(input_peak.max(1e-12)))
        .fold(0.0_f64, f64::max);
    assert!(
        normalized_partition_error <= 2e-5,
        "partition error {normalized_partition_error} exceeds 2e-5 input-peak normalization"
    );

    let mut structural = make_plugin("BW24", SAMPLE_RATE, 1_000.0, "low");
    assert!(
        structural
            .set_parameter(
                ParameterId::from("mode"),
                ParameterValue::String("both".to_string()),
            )
            .is_err()
    );
    assert_eq!(
        structural.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::String("lowpass".to_string()))
    );
}

fn render_automated(
    plugin: &mut CrossoverPlugin,
    input: &[f32],
    sample_rate: u32,
    block_pattern: &[usize],
    change_at: usize,
) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len() * 2];
    let mut frame = 0;
    let mut block_index = 0;
    let mut changed = false;
    while frame < input.len() {
        if frame == change_at && !changed {
            plugin
                .set_parameter(
                    ParameterId::from("frequency"),
                    ParameterValue::Float(19_500.0),
                )
                .unwrap();
            changed = true;
        }
        let mut count = block_pattern[block_index % block_pattern.len()].min(input.len() - frame);
        if !changed {
            count = count.min(change_at - frame);
        }
        let input_start = frame;
        let input_end = frame + count;
        let processed_frames = plugin
            .process(
                &input[input_start..input_end],
                &mut output[input_start * 2..input_end * 2],
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        assert_eq!(processed_frames, count);
        frame = input_end;
        block_index += 1;
    }
    output
}

fn make_multiway_plugin(kind: &str, sample_rate: u32, cutoffs: &[f64]) -> CrossoverPlugin {
    assert!(!cutoffs.is_empty());
    let mut plugin = if cutoffs.len() == 1 {
        CrossoverPlugin::new(2, kind, cutoffs[0], "both").unwrap()
    } else {
        CrossoverPlugin::new_multiway(2, kind, cutoffs[0], "both", &cutoffs[1..]).unwrap()
    };
    plugin.initialize(f64::from(sample_rate)).unwrap();
    plugin
}

fn make_stereo_input(frames: usize, sample_rate: u32) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(sample_rate);
            [
                (0.31 * (TAU * 997.0 * time).sin() + 0.14 * (TAU * 8_201.0 * time).cos()) as f32,
                (0.23 * (TAU * 241.0 * time).cos() + 0.19 * (TAU * 1_901.0 * time).sin()) as f32,
            ]
        })
        .collect()
}

fn render_partitioned(
    plugin: &mut CrossoverPlugin,
    input: &[f32],
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
    block_pattern: &[usize],
) -> Vec<f32> {
    assert!(!block_pattern.is_empty());
    assert_eq!(input.len() % input_channels, 0);
    let frames = input.len() / input_channels;
    let mut output = vec![f32::NAN; frames * output_channels];
    let mut frame = 0;
    let mut block_index = 0;
    while frame < frames {
        let count = block_pattern[block_index % block_pattern.len()].min(frames - frame);
        let input_start = frame * input_channels;
        let input_end = input_start + count * input_channels;
        let output_start = frame * output_channels;
        let output_end = output_start + count * output_channels;
        let processed = plugin
            .process(
                &input[input_start..input_end],
                &mut output[output_start..output_end],
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        assert_eq!(processed, count, "callback frame count changed");
        frame += count;
        block_index += 1;
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn render_with_scalar_change(
    plugin: &mut CrossoverPlugin,
    input: &[f32],
    channel_counts: (usize, usize),
    sample_rate: u32,
    block_pattern: &[usize],
    change_at: usize,
    updates: &[(&str, f32)],
) -> Vec<f32> {
    let (input_channels, output_channels) = channel_counts;
    assert!(!block_pattern.is_empty());
    assert_eq!(input.len() % input_channels, 0);
    let frames = input.len() / input_channels;
    assert!(change_at < frames);
    let mut output = vec![f32::NAN; frames * output_channels];
    let mut frame = 0;
    let mut block_index = 0;
    let mut changed = false;
    while frame < frames {
        if frame == change_at && !changed {
            for &(id, value) in updates {
                plugin
                    .set_parameter(ParameterId::from(id), ParameterValue::Float(value))
                    .unwrap_or_else(|error| panic!("setting {id} to {value} failed: {error}"));
            }
            changed = true;
        }
        let mut count = block_pattern[block_index % block_pattern.len()].min(frames - frame);
        if !changed {
            count = count.min(change_at - frame);
        }
        let input_start = frame * input_channels;
        let input_end = input_start + count * input_channels;
        let output_start = frame * output_channels;
        let output_end = output_start + count * output_channels;
        let processed = plugin
            .process(
                &input[input_start..input_end],
                &mut output[output_start..output_end],
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        assert_eq!(processed, count, "callback frame count changed");
        frame += count;
        block_index += 1;
    }
    assert!(changed, "automation event was not applied");
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn assert_automation_limits(input: &[f32], regular: &[f32], irregular: &[f32], label: &str) {
    assert_eq!(
        regular.len(),
        irregular.len(),
        "{label}: output width changed"
    );
    assert!(regular.iter().all(|sample| sample.is_finite()), "{label}");
    assert!(irregular.iter().all(|sample| sample.is_finite()), "{label}");
    let input_peak = input
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let output_peak = regular
        .iter()
        .chain(irregular)
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(
        f64::from(output_peak) <= 8.0 * f64::from(input_peak) + 1e-6,
        "{label}: peak {output_peak} exceeds input-relative bound for input peak {input_peak}"
    );
    let partition_error = regular
        .iter()
        .zip(irregular)
        .map(|(left, right)| f64::from((left - right).abs()) / f64::from(input_peak.max(1e-12)))
        .fold(0.0_f64, f64::max);
    assert!(
        partition_error <= 2e-5,
        "{label}: maximum input-peak-normalized partition error {partition_error} exceeds 2e-5"
    );
}

fn assert_reset_matches_fresh(
    plugin: &mut CrossoverPlugin,
    kind: &str,
    sample_rate: u32,
    cutoffs: &[f64],
) {
    const FRAMES: usize = 2_049;
    let input = make_stereo_input(FRAMES, sample_rate);
    let mut fresh = make_multiway_plugin(kind, sample_rate, cutoffs);
    plugin.reset();
    fresh.reset();
    let reset_output = render_partitioned(
        plugin,
        &input,
        2,
        2 * (cutoffs.len() + 1),
        sample_rate,
        &[1, 7, 31, 127, 513],
    );
    let fresh_output = render_partitioned(
        &mut fresh,
        &input,
        2,
        2 * (cutoffs.len() + 1),
        sample_rate,
        &[1, 7, 31, 127, 513],
    );
    assert_eq!(reset_output, fresh_output, "{kind} reset target mismatch");
}

#[test]
fn admitted_upper_cutoffs_match_independent_oracles_and_reject_just_outside() {
    const BOUNDARY_FAMILIES: &[&str] = &["LR48", "BW42", "BW48", "Bessel12"];
    // The declared sample-rate bound is strict. Use the nearest f32 value
    // below 0.495*8 kHz because native controls are represented as f32.
    let rate_limited_cutoff = f64::from(f32::from_bits(3_960.0_f32.to_bits() - 1));
    for &kind in BOUNDARY_FAMILIES {
        for (sample_rate, cutoff) in [(48_000, 20_000.0), (8_000, rate_limited_cutoff)] {
            let mut plugin = make_plugin(kind, sample_rate, cutoff, "both");
            assert_eq!(plugin.output_channels(), 2, "{kind} output width");
            for probe_frequency in [cutoff * 0.5, cutoff] {
                let measured = measure_converged_two_way(
                    &mut plugin,
                    kind,
                    cutoff,
                    probe_frequency,
                    sample_rate,
                );
                for (branch, actual) in [Branch::Low, Branch::High].into_iter().zip(measured) {
                    let expected =
                        family_response(kind, cutoff, sample_rate, probe_frequency, branch);
                    assert_complex_close(
                        &format!(
                            "{kind} upper boundary fs={sample_rate} fc={cutoff} f={probe_frequency}"
                        ),
                        actual,
                        expected,
                    );
                }
            }
        }
    }

    let exact_limit = 3_960.0_f32;
    let above_limit = f32::from_bits(exact_limit.to_bits() + 1);
    for rejected_cutoff in [f64::from(exact_limit), f64::from(above_limit)] {
        let mut rejected = make_plugin("LR48", 48_000, rejected_cutoff, "both");
        let mut twin = make_plugin("LR48", 48_000, rejected_cutoff, "both");
        process_signal(&mut rejected, 48_000, 0, 4_096, 1_231.0, 2, false);
        process_signal(&mut twin, 48_000, 0, 4_096, 1_231.0, 2, false);
        assert!(
            rejected.initialize(8_000).is_err(),
            "cutoff {rejected_cutoff} must be inadmissible at the strict 0.495*Fs boundary"
        );
        let rejected_continuation =
            process_signal(&mut rejected, 48_000, 4_096, 2_049, 1_231.0, 2, true);
        let twin_continuation = process_signal(&mut twin, 48_000, 4_096, 2_049, 1_231.0, 2, true);
        assert_eq!(
            rejected_continuation, twin_continuation,
            "refused 8 kHz reinit at {rejected_cutoff} Hz must preserve populated audio history"
        );
    }
}

#[test]
fn bw42_and_compensated_lr_multiway_scalar_automation_meet_peak_and_partition_bounds() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 24_000;
    const CHANGE_AT: usize = 4_093;
    let input = make_stereo_input(FRAMES, SAMPLE_RATE);
    let regular_blocks = [256, 512, 1_024];
    let irregular_blocks = [1, 7, 31, 127, 513];

    for (initial, target, label) in [
        (
            [1_000.0],
            [20_000.0],
            "BW42 two-way low-to-high endpoint automation",
        ),
        (
            [20_000.0],
            [1_000.0],
            "BW42 two-way high-to-low endpoint automation",
        ),
    ] {
        let mut regular = make_multiway_plugin("BW42", SAMPLE_RATE, &initial);
        let mut irregular = make_multiway_plugin("BW42", SAMPLE_RATE, &initial);
        let updates = [("frequency", target[0] as f32)];
        let regular_output = render_with_scalar_change(
            &mut regular,
            &input,
            (2, 4),
            SAMPLE_RATE,
            &regular_blocks,
            CHANGE_AT,
            &updates,
        );
        let irregular_output = render_with_scalar_change(
            &mut irregular,
            &input,
            (2, 4),
            SAMPLE_RATE,
            &irregular_blocks,
            CHANGE_AT,
            &updates,
        );
        assert_automation_limits(&input, &regular_output, &irregular_output, label);
        assert_reset_matches_fresh(&mut regular, "BW42", SAMPLE_RATE, &target);
        assert_reset_matches_fresh(&mut irregular, "BW42", SAMPLE_RATE, &target);
    }

    for (initial, target, label) in [
        (
            [300.0, 2_500.0, 9_000.0],
            [800.0, 6_000.0, 19_500.0],
            "LR48 compensated multiway low-to-high",
        ),
        (
            [800.0, 6_000.0, 19_500.0],
            [300.0, 2_500.0, 9_000.0],
            "LR48 compensated multiway high-to-low",
        ),
    ] {
        let mut regular = make_multiway_plugin("LR48", SAMPLE_RATE, &initial);
        let mut irregular = make_multiway_plugin("LR48", SAMPLE_RATE, &initial);
        // Update the upper cutoffs first on a rising sweep and the lower first
        // on a falling sweep so every intermediate scalar tuple stays ordered.
        let updates = if target[0] > initial[0] {
            [
                ("frequency_3", target[2] as f32),
                ("frequency_2", target[1] as f32),
                ("frequency", target[0] as f32),
            ]
        } else {
            [
                ("frequency", target[0] as f32),
                ("frequency_2", target[1] as f32),
                ("frequency_3", target[2] as f32),
            ]
        };
        let regular_output = render_with_scalar_change(
            &mut regular,
            &input,
            (2, 8),
            SAMPLE_RATE,
            &regular_blocks,
            CHANGE_AT,
            &updates,
        );
        let irregular_output = render_with_scalar_change(
            &mut irregular,
            &input,
            (2, 8),
            SAMPLE_RATE,
            &irregular_blocks,
            CHANGE_AT,
            &updates,
        );
        assert_automation_limits(&input, &regular_output, &irregular_output, label);
        assert_reset_matches_fresh(&mut regular, "LR48", SAMPLE_RATE, &target);
        assert_reset_matches_fresh(&mut irregular, "LR48", SAMPLE_RATE, &target);
    }
}

fn make_per_channel_plugin(
    sample_rate: u32,
    frequencies: [f32; 2],
    modes: [PerChannelOpMode; 2],
) -> CrossoverPlugin {
    let mut plugin =
        CrossoverPlugin::new_per_channel("LR48", frequencies.to_vec(), modes.to_vec()).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    plugin
}

#[test]
fn per_channel_public_lifecycle_refuses_live_edits_and_reinitializes_transactionally() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 2_049;
    let input = make_stereo_input(FRAMES, SAMPLE_RATE);
    let configurations = [
        (
            [20.0, 20_000.0],
            [PerChannelOpMode::Lowpass, PerChannelOpMode::Highpass],
        ),
        (
            [20_000.0, 20.0],
            [PerChannelOpMode::Highpass, PerChannelOpMode::Lowpass],
        ),
    ];
    let regular_blocks = [256, 512, 1_024];
    let irregular_blocks = [1, 7, 31, 127, 513];

    for (frequencies, modes) in configurations {
        let mut populated = make_per_channel_plugin(SAMPLE_RATE, frequencies, modes);
        let mut twin = make_per_channel_plugin(SAMPLE_RATE, frequencies, modes);
        let mut regular = make_per_channel_plugin(SAMPLE_RATE, frequencies, modes);
        assert_eq!(
            (populated.input_channels(), populated.output_channels()),
            (2, 2)
        );
        let regular_output =
            render_partitioned(&mut regular, &input, 2, 2, SAMPLE_RATE, &regular_blocks);
        let initial =
            render_partitioned(&mut populated, &input, 2, 2, SAMPLE_RATE, &irregular_blocks);
        let twin_initial =
            render_partitioned(&mut twin, &input, 2, 2, SAMPLE_RATE, &irregular_blocks);
        assert_eq!(initial, twin_initial);
        assert!(initial.iter().any(|sample| sample.abs() > 1e-5));
        assert_automation_limits(
            &input,
            &regular_output,
            &initial,
            "per-channel opposite extreme configuration partition invariance",
        );

        let old_frequency_0 = populated.get_parameter(&ParameterId::from("channel_frequency_0"));
        let old_mode_1 = populated.get_parameter(&ParameterId::from("channel_mode_1"));
        assert!(
            populated
                .set_parameter(
                    ParameterId::from("channel_frequency_0"),
                    ParameterValue::Float(if frequencies[0] == 20.0 {
                        21.0
                    } else {
                        19_999.0
                    }),
                )
                .is_err(),
            "initialized per-channel cutoff edits are structural"
        );
        assert!(
            populated
                .set_parameter(
                    ParameterId::from("channel_mode_1"),
                    ParameterValue::String("mute".to_string()),
                )
                .is_err(),
            "initialized per-channel mode edits are structural"
        );
        assert_eq!(
            populated.get_parameter(&ParameterId::from("channel_frequency_0")),
            old_frequency_0
        );
        assert_eq!(
            populated.get_parameter(&ParameterId::from("channel_mode_1")),
            old_mode_1
        );

        assert!(
            populated.initialize(8_000).is_err(),
            "20,000 Hz per-channel cutoff must be rejected at 8 kHz"
        );
        let continuation = make_stereo_input(1_027, SAMPLE_RATE);
        let continued = render_partitioned(
            &mut populated,
            &continuation,
            2,
            2,
            SAMPLE_RATE,
            &irregular_blocks,
        );
        let twin_continued = render_partitioned(
            &mut twin,
            &continuation,
            2,
            2,
            SAMPLE_RATE,
            &irregular_blocks,
        );
        assert_eq!(
            continued, twin_continued,
            "rejected per-channel sample-rate change must preserve populated audio"
        );

        populated.initialize(44_100).unwrap();
        let mut fresh = make_per_channel_plugin(44_100, frequencies, modes);
        let reinit_input = make_stereo_input(1_541, 44_100);
        let reinitialized = render_partitioned(
            &mut populated,
            &reinit_input,
            2,
            2,
            44_100,
            &irregular_blocks,
        );
        let fresh_output =
            render_partitioned(&mut fresh, &reinit_input, 2, 2, 44_100, &irregular_blocks);
        assert_eq!(
            reinitialized, fresh_output,
            "valid per-channel reinit target"
        );
        assert_reset_matches_per_channel(
            &mut populated,
            44_100,
            frequencies,
            modes,
            &irregular_blocks,
        );
    }
}

fn assert_reset_matches_per_channel(
    populated: &mut CrossoverPlugin,
    sample_rate: u32,
    frequencies: [f32; 2],
    modes: [PerChannelOpMode; 2],
    block_pattern: &[usize],
) {
    let input = make_stereo_input(2_053, sample_rate);
    populated.reset();
    let mut fresh = make_per_channel_plugin(sample_rate, frequencies, modes);
    let reset_output = render_partitioned(populated, &input, 2, 2, sample_rate, block_pattern);
    let fresh_output = render_partitioned(&mut fresh, &input, 2, 2, sample_rate, block_pattern);
    assert_eq!(reset_output, fresh_output, "per-channel reset target");
}
