use std::ops::{Add, Mul};

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_band_split::{BandSplitPlugin, BandSplitRecombinationMode};

const SAMPLE_RATE: u32 = 48_000;
const WARMUP_FRAMES: usize = 12_000;
const MEASURE_FRAMES: usize = 24_000;
const TOTAL_FRAMES: usize = WARMUP_FRAMES + MEASURE_FRAMES;
const COMPLEX_RESPONSE_TOLERANCE: f64 = 0.002;
const MAGNITUDE_TOLERANCE: f64 = 0.005;
const WAVEFORM_RESIDUAL_TOLERANCE: f64 = 0.002;
const PROBE_OFFSETS_HZ: [f64; 5] = [-48.0, -24.0, 0.0, 24.0, 48.0];

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const ONE: Self = Self { re: 1.0, im: 0.0 };

    fn reciprocal(self) -> Self {
        let denominator = self.re * self.re + self.im * self.im;
        Self {
            re: self.re / denominator,
            im: -self.im / denominator,
        }
    }

    fn distance(self, other: Self) -> f64 {
        (self.re - other.re).hypot(self.im - other.im)
    }

    fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn exp(phase: f64) -> Self {
        Self {
            re: phase.cos(),
            im: phase.sin(),
        }
    }

    fn powu(self, power: u32) -> Self {
        (0..power).fold(Self::ONE, |product, _| product * self)
    }
}

impl Add for Complex {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            re: self.re + rhs.re,
            im: self.im + rhs.im,
        }
    }
}

impl Mul for Complex {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            re: self.re * rhs.re - self.im * rhs.im,
            im: self.re * rhs.im + self.im * rhs.re,
        }
    }
}

#[derive(Clone)]
struct Tone {
    frequency_hz: f64,
    amplitude: Vec<f64>,
    phase: Vec<f64>,
}

struct Case {
    name: &'static str,
    slope: &'static str,
    cutoffs_hz: &'static [f64],
    sample_rate: u32,
    channels: usize,
    include_scale_and_log_sweep: bool,
}

struct WaveformReference {
    channel_phasors: Vec<Complex>,
    oscillator: Complex,
    rotation: Complex,
}

fn low_high_response(
    slope: &str,
    probe_hz: f64,
    cutoff_hz: f64,
    sample_rate: u32,
) -> (Complex, Complex) {
    // Bilinear-transform prewarping gives s = j*tan(pi*f/fs)/tan(pi*fc/fs).
    let normalized = (std::f64::consts::PI * probe_hz / f64::from(sample_rate)).tan()
        / (std::f64::consts::PI * cutoff_hz / f64::from(sample_rate)).tan();
    let s = Complex {
        re: 0.0,
        im: normalized,
    };

    let (denominator, high_order) = match slope {
        "LR24" => {
            let butterworth_second_order = s * s
                + Complex { re: 1.0, im: 0.0 }
                + Complex {
                    re: 0.0,
                    im: std::f64::consts::SQRT_2 * normalized,
                };
            (butterworth_second_order * butterworth_second_order, 4)
        }
        "LR48" => {
            let q1 = 1.0 / (2.0 * (std::f64::consts::PI / 8.0).sin());
            let q2 = 1.0 / (2.0 * (3.0 * std::f64::consts::PI / 8.0).sin());
            let first_section = s * s
                + s * Complex {
                    re: 1.0 / q1,
                    im: 0.0,
                }
                + Complex::ONE;
            let second_section = s * s
                + s * Complex {
                    re: 1.0 / q2,
                    im: 0.0,
                }
                + Complex::ONE;
            let butterworth_fourth_order = first_section * second_section;
            (butterworth_fourth_order * butterworth_fourth_order, 8)
        }
        _ => panic!("unsupported slope {slope}"),
    };

    let low = denominator.reciprocal();
    let high = s.powu(high_order) * low;
    (low, high)
}

fn expected_phase_compensated_responses(
    slope: &str,
    cutoffs_hz: &[f64],
    probe_hz: f64,
    sample_rate: u32,
) -> (Vec<Complex>, Complex) {
    let stages: Vec<(Complex, Complex)> = cutoffs_hz
        .iter()
        .map(|&cutoff| low_high_response(slope, probe_hz, cutoff, sample_rate))
        .collect();
    let allpass_product = stages
        .iter()
        .map(|(low, high)| *low + *high)
        .fold(Complex::ONE, |product, allpass| product * allpass);

    let mut carry = Complex::ONE;
    let mut bands = Vec::with_capacity(cutoffs_hz.len() + 1);
    for (stage_index, (low, high)) in stages.iter().copied().enumerate() {
        let later_allpasses = stages[stage_index + 1..]
            .iter()
            .map(|(later_low, later_high)| *later_low + *later_high)
            .fold(Complex::ONE, |product, allpass| product * allpass);
        bands.push(carry * low * later_allpasses);
        carry = carry * high;
    }
    bands.push(carry);
    (bands, allpass_product)
}

fn coherent_bin_frequency(target_hz: f64, sample_rate: u32) -> f64 {
    let bin = (target_hz * MEASURE_FRAMES as f64 / f64::from(sample_rate)).round();
    bin * f64::from(sample_rate) / MEASURE_FRAMES as f64
}

fn make_probe_frequencies(
    cutoffs_hz: &[f64],
    sample_rate: u32,
    include_scale_and_log_sweep: bool,
) -> Vec<f64> {
    let mut targets = Vec::new();
    for &cutoff in cutoffs_hz {
        targets.extend(PROBE_OFFSETS_HZ.map(|offset| cutoff + offset));
        if include_scale_and_log_sweep {
            targets.extend([0.5 * cutoff, cutoff, 2.0 * cutoff]);
        }
    }
    if include_scale_and_log_sweep {
        let minimum = 30.0_f64;
        let maximum = (18_000.0_f64).min(f64::from(sample_rate) * 0.4);
        let points = 48;
        for index in 0..points {
            let fraction = index as f64 / (points - 1) as f64;
            targets.push(minimum * (maximum / minimum).powf(fraction));
        }
    }

    let mut probes: Vec<f64> = targets
        .into_iter()
        .map(|frequency| coherent_bin_frequency(frequency, sample_rate))
        .collect();
    probes.sort_by(f64::total_cmp);
    probes.dedup_by(|left, right| (*left - *right).abs() < 1e-9);
    probes
}

fn make_tones(probe_frequencies: &[f64], channels: usize) -> Vec<Tone> {
    let tone_count = probe_frequencies.len() as f64;
    probe_frequencies
        .iter()
        .copied()
        .enumerate()
        .map(|(index, frequency_hz)| {
            let phase = index as f64 * 0.173 + 0.15;
            Tone {
                frequency_hz,
                amplitude: (0..channels)
                    .map(|channel| (0.31 - channel as f64 * 0.02) / tone_count)
                    .collect(),
                phase: (0..channels)
                    .map(|channel| phase + channel as f64 * 0.79 + index as f64 * 0.011)
                    .collect(),
            }
        })
        .collect()
}

fn make_input(tones: &[Tone], sample_rate: u32, channels: usize) -> Vec<f32> {
    let mut input = vec![0.0; TOTAL_FRAMES * channels];
    let mut states: Vec<Vec<Complex>> = tones
        .iter()
        .map(|tone| {
            tone.phase
                .iter()
                .map(|phase| Complex::exp(*phase))
                .collect()
        })
        .collect();
    let rotations: Vec<Complex> = tones
        .iter()
        .map(|tone| {
            Complex::exp(std::f64::consts::TAU * tone.frequency_hz / f64::from(sample_rate))
        })
        .collect();
    let mut samples = vec![0.0_f64; channels];
    for frame in 0..TOTAL_FRAMES {
        samples.fill(0.0);
        for (tone_index, tone) in tones.iter().enumerate() {
            for channel in 0..channels {
                let state = states[tone_index][channel];
                samples[channel] += tone.amplitude[channel] * state.im;
                states[tone_index][channel] = state * rotations[tone_index];
            }
        }
        for (channel, sample) in samples.iter().copied().enumerate() {
            input[frame * channels + channel] = sample as f32;
        }
    }
    input
}

fn project(
    samples: &[f32],
    stride: usize,
    offset: usize,
    frequency_hz: f64,
    sample_rate: u32,
) -> Complex {
    let omega = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
    let mut projection = Complex { re: 0.0, im: 0.0 };
    let mut oscillator = Complex::exp(-omega * WARMUP_FRAMES as f64);
    let rotation = Complex::exp(-omega);
    for frame in WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES {
        let sample = f64::from(samples[frame * stride + offset]);
        projection.re += sample * oscillator.re;
        projection.im += sample * oscillator.im;
        oscillator = oscillator * rotation;
    }
    let scale = 2.0 / MEASURE_FRAMES as f64;
    Complex {
        re: projection.re * scale,
        im: projection.im * scale,
    }
}

fn project_recombined(
    output: &[f32],
    bands: usize,
    channels: usize,
    channel: usize,
    frequency_hz: f64,
    sample_rate: u32,
) -> Complex {
    let omega = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
    let mut projection = Complex { re: 0.0, im: 0.0 };
    let mut oscillator = Complex::exp(-omega * WARMUP_FRAMES as f64);
    let rotation = Complex::exp(-omega);
    for frame in WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES {
        let frame_offset = frame * bands * channels;
        let sample = (0..bands)
            .map(|band| f64::from(output[frame_offset + band * channels + channel]))
            .sum::<f64>();
        projection.re += sample * oscillator.re;
        projection.im += sample * oscillator.im;
        oscillator = oscillator * rotation;
    }
    let scale = 2.0 / MEASURE_FRAMES as f64;
    Complex {
        re: projection.re * scale,
        im: projection.im * scale,
    }
}

fn input_phasor(tone: &Tone, channel: usize) -> Complex {
    // For A*sin(w*n + phase), the positive-frequency phasor is -i*A*exp(i*phase).
    Complex {
        re: tone.amplitude[channel] * tone.phase[channel].sin(),
        im: -tone.amplitude[channel] * tone.phase[channel].cos(),
    }
}

fn set_targets(plugin: &mut BandSplitPlugin, targets: [f64; 3]) {
    for (parameter, value) in [
        ("frequency", targets[0]),
        ("frequency_2", targets[1]),
        ("frequency_3", targets[2]),
    ] {
        plugin
            .set_parameter(
                ParameterId::from(parameter),
                ParameterValue::Float(value as f32),
            )
            .unwrap();
    }
}

fn process_range(
    plugin: &mut BandSplitPlugin,
    input: &[f32],
    output: &mut [f32],
    start_frame: usize,
    end_frame: usize,
    partitioned: bool,
) {
    const PARTITIONS: [usize; 8] = [1, 31, 7, 256, 3, 511, 17, 127];
    let mut frame = start_frame;
    let mut partition_index = 0;
    while frame < end_frame {
        let requested = if partitioned {
            PARTITIONS[partition_index % PARTITIONS.len()]
        } else {
            end_frame - frame
        };
        let next_frame = (frame + requested).min(end_frame);
        let input_start = frame * 2;
        let input_end = next_frame * 2;
        let output_start = frame * 8;
        let output_end = next_frame * 8;
        plugin
            .process(
                &input[input_start..input_end],
                &mut output[output_start..output_end],
                &ProcessContext::new(SAMPLE_RATE, next_frame - frame),
            )
            .unwrap();
        frame = next_frame;
        partition_index += 1;
    }
}

fn run_case(case: &Case) {
    let probes = make_probe_frequencies(
        case.cutoffs_hz,
        case.sample_rate,
        case.include_scale_and_log_sweep,
    );
    let tones = make_tones(&probes, case.channels);
    let input = make_input(&tones, case.sample_rate, case.channels);
    let num_bands = case.cutoffs_hz.len() + 1;
    let mut plugin = BandSplitPlugin::new_multiband_with_mode(
        case.channels,
        case.cutoffs_hz,
        case.slope,
        BandSplitRecombinationMode::PhaseCompensated,
    )
    .unwrap();
    plugin.initialize(f64::from(case.sample_rate)).unwrap();
    assert_eq!(plugin.output_channels(), num_bands * case.channels);

    let mut output = vec![0.0; TOTAL_FRAMES * num_bands * case.channels];
    plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(case.sample_rate, TOTAL_FRAMES),
        )
        .unwrap();
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{}",
        case.name
    );
    assert!(
        input.iter().all(|sample| sample.is_finite()),
        "{}",
        case.name
    );
    assert!(
        output.iter().any(|sample| sample.abs() > 1e-5),
        "{}",
        case.name
    );
    if case.channels > 1 {
        assert!(
            (0..TOTAL_FRAMES).any(|frame| {
                (1..case.channels).any(|channel| {
                    input[frame * case.channels] != input[frame * case.channels + channel]
                })
            }),
            "{} channels should be distinct",
            case.name
        );
    }

    for &probe_hz in &probes {
        let (expected_bands, expected_sum) = expected_phase_compensated_responses(
            case.slope,
            case.cutoffs_hz,
            probe_hz,
            case.sample_rate,
        );
        for channel in 0..case.channels {
            let input_response =
                project(&input, case.channels, channel, probe_hz, case.sample_rate);
            assert!(input_response.norm() > 1e-4, "{} {probe_hz} Hz", case.name);
            for (band, expected) in expected_bands.iter().copied().enumerate() {
                let actual = project(
                    &output,
                    num_bands * case.channels,
                    band * case.channels + channel,
                    probe_hz,
                    case.sample_rate,
                ) * input_response.reciprocal();
                let error = actual.distance(expected);
                assert!(
                    error <= COMPLEX_RESPONSE_TOLERANCE,
                    "{} {} band {band}, channel {channel}, {probe_hz} Hz: response error {error}",
                    case.name,
                    case.slope
                );
            }
            let actual_sum = project_recombined(
                &output,
                num_bands,
                case.channels,
                channel,
                probe_hz,
                case.sample_rate,
            ) * input_response.reciprocal();
            assert!(
                actual_sum.distance(expected_sum) <= COMPLEX_RESPONSE_TOLERANCE,
                "{} {} sum channel {channel}, {probe_hz} Hz: measured {:?}, expected {:?}",
                case.name,
                case.slope,
                actual_sum,
                expected_sum
            );
            let magnitude_error = (actual_sum.norm() - 1.0).abs();
            assert!(
                magnitude_error <= MAGNITUDE_TOLERANCE,
                "{} {} sum channel {channel}, {probe_hz} Hz: magnitude error {magnitude_error}",
                case.name,
                case.slope
            );
        }
    }

    let mut waveform_references: Vec<WaveformReference> = tones
        .iter()
        .map(|tone| {
            let (_, total) = expected_phase_compensated_responses(
                case.slope,
                case.cutoffs_hz,
                tone.frequency_hz,
                case.sample_rate,
            );
            let omega = std::f64::consts::TAU * tone.frequency_hz / f64::from(case.sample_rate);
            WaveformReference {
                channel_phasors: (0..case.channels)
                    .map(|channel| input_phasor(tone, channel) * total)
                    .collect(),
                oscillator: Complex::exp(omega * WARMUP_FRAMES as f64),
                rotation: Complex::exp(omega),
            }
        })
        .collect();
    let mut input_power = 0.0;
    let mut error_power = 0.0;
    let mut max_sample_residual = 0.0_f64;
    for frame in WARMUP_FRAMES..TOTAL_FRAMES {
        for channel in 0..case.channels {
            let mut actual = 0.0_f64;
            let mut expected = 0.0_f64;
            for band in 0..num_bands {
                actual += f64::from(
                    output[frame * num_bands * case.channels + band * case.channels + channel],
                );
            }
            for tone in &waveform_references {
                expected += (tone.channel_phasors[channel] * tone.oscillator).re;
            }
            let input_sample = f64::from(input[frame * case.channels + channel]);
            input_power += input_sample * input_sample;
            let error = (actual - expected).abs();
            error_power += error * error;
            max_sample_residual = max_sample_residual.max(error);
        }
        for tone in &mut waveform_references {
            tone.oscillator = tone.oscillator * tone.rotation;
        }
    }
    let normalized_waveform_residual = (error_power / input_power).sqrt();
    println!(
        "AUD143 settled waveform {} {}: input-normalized RMS={normalized_waveform_residual:.8e}, max-absolute={max_sample_residual:.8e}",
        case.name, case.slope
    );
    assert!(
        normalized_waveform_residual <= WAVEFORM_RESIDUAL_TOLERANCE,
        "{} {} settled recombined waveform residual {normalized_waveform_residual}",
        case.name,
        case.slope
    );
}

#[test]
fn phase_compensated_responses_and_settled_waveforms_match_independent_reference() {
    let cases = [
        ("two-band", &[1_000.0][..]),
        ("three-band-close", &[1_000.0, 1_200.0][..]),
        ("three-band-wide", &[500.0, 2_000.0][..]),
        ("four-band-close", &[1_000.0, 1_100.0, 1_200.0][..]),
        ("four-band-wide", &[250.0, 1_000.0, 4_000.0][..]),
    ];

    for slope in ["LR24", "LR48"] {
        for (name, cutoffs_hz) in cases {
            run_case(&Case {
                name,
                slope,
                cutoffs_hz,
                sample_rate: SAMPLE_RATE,
                channels: 2,
                include_scale_and_log_sweep: true,
            });
        }
    }
}

#[test]
fn phase_compensated_log_sweep_covers_supported_rates_and_channel_counts() {
    let cases = [
        ("44k-mono-wide", 44_100, 1, &[250.0, 1_000.0, 4_000.0][..]),
        (
            "96k-six-channel-close",
            96_000,
            6,
            &[1_000.0, 1_100.0, 1_200.0][..],
        ),
        ("96k-eight-channel-wide", 96_000, 8, &[500.0, 2_000.0][..]),
    ];
    for slope in ["LR24", "LR48"] {
        for (name, sample_rate, channels, cutoffs_hz) in cases {
            run_case(&Case {
                name,
                slope,
                cutoffs_hz,
                sample_rate,
                channels,
                include_scale_and_log_sweep: true,
            });
        }
    }
}

#[test]
fn phase_compensated_automation_is_finite_bounded_and_partition_invariant() {
    let frames = 4_096;
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let time = frame as f32 / SAMPLE_RATE as f32;
            [
                0.12 * (std::f32::consts::TAU * 731.0 * time).sin()
                    + 0.03 * (std::f32::consts::TAU * 5_300.0 * time).cos(),
                0.09 * (std::f32::consts::TAU * 1_117.0 * time + 0.4).cos()
                    - 0.02 * (std::f32::consts::TAU * 7_100.0 * time).sin(),
            ]
        })
        .collect();
    let target_frequencies = [550.0, 2_300.0, 7_200.0];

    for slope in ["LR24", "LR48"] {
        let mut contiguous = BandSplitPlugin::new_multiband_with_mode(
            2,
            &[500.0, 2_000.0, 8_000.0],
            slope,
            BandSplitRecombinationMode::PhaseCompensated,
        )
        .unwrap();
        let mut partitioned = BandSplitPlugin::new_multiband_with_mode(
            2,
            &[500.0, 2_000.0, 8_000.0],
            slope,
            BandSplitRecombinationMode::PhaseCompensated,
        )
        .unwrap();
        contiguous.initialize(f64::from(SAMPLE_RATE)).unwrap();
        partitioned.initialize(f64::from(SAMPLE_RATE)).unwrap();
        for (index, frequency) in target_frequencies.into_iter().enumerate() {
            let parameter = if index == 0 {
                "frequency".to_string()
            } else {
                format!("frequency_{}", index + 1)
            };
            for plugin in [&mut contiguous, &mut partitioned] {
                plugin
                    .set_parameter(
                        ParameterId::from(parameter.as_str()),
                        ParameterValue::Float(frequency),
                    )
                    .unwrap();
            }
        }

        let output_channels = 8;
        let mut expected = vec![0.0; frames * output_channels];
        contiguous
            .process(
                &input,
                &mut expected,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .unwrap();
        let mut actual = vec![0.0; frames * output_channels];
        let mut offset = 0;
        for block in [1, 31, 7, 256, 3, 511, 17, 1_270, 2_000] {
            let end = offset + block;
            partitioned
                .process(
                    &input[offset * 2..end * 2],
                    &mut actual[offset * output_channels..end * output_channels],
                    &ProcessContext::new(SAMPLE_RATE, block),
                )
                .unwrap();
            offset = end;
        }
        assert_eq!(offset, frames);
        assert!(actual.iter().all(|sample| sample.is_finite()), "{slope}");
        assert!(expected.iter().all(|sample| sample.is_finite()), "{slope}");
        assert!(actual.iter().all(|sample| sample.abs() <= 1.0), "{slope}");

        let input_power = input
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>();
        let error_power = actual
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| (f64::from(*actual) - f64::from(*expected)).powi(2))
            .sum::<f64>();
        let partition_residual = (error_power / input_power).sqrt();
        let max_sample_residual = actual
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| (f64::from(*actual) - f64::from(*expected)).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            partition_residual <= 2e-5,
            "{slope} phase-compensated automation partition residual {partition_residual}"
        );
        assert!(
            max_sample_residual <= 2e-5,
            "{slope} phase-compensated automation maximum sample residual {max_sample_residual}"
        );
        println!(
            "AUD143 automation {slope}: input-normalized RMS={partition_residual:.8e}, max-absolute={max_sample_residual:.8e}"
        );
    }
}

#[test]
fn absolute_timed_cutoff_events_are_partition_invariant_in_both_recombination_modes() {
    let frames = 5_000;
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            [
                (0.11 * (std::f64::consts::TAU * 613.0 * time).sin()
                    + 0.025 * (std::f64::consts::TAU * 4_300.0 * time).cos())
                    as f32,
                (0.08 * (std::f64::consts::TAU * 1_073.0 * time + 0.31).cos()
                    - 0.02 * (std::f64::consts::TAU * 7_700.0 * time).sin()) as f32,
            ]
        })
        .collect();
    let events = [
        (0, [550.0, 2_300.0, 7_200.0]),
        (731, [680.0, 3_100.0, 8_100.0]),
        (1_903, [420.0, 1_800.0, 6_500.0]),
        (3_701, [600.0, 2_600.0, 7_900.0]),
        (4_800, [500.0, 2_000.0, 8_000.0]),
    ];

    for slope in ["LR24", "LR48"] {
        for mode in [
            BandSplitRecombinationMode::LegacyCascade,
            BandSplitRecombinationMode::PhaseCompensated,
        ] {
            let mut contiguous = BandSplitPlugin::new_multiband_with_mode(
                2,
                &[500.0, 2_000.0, 8_000.0],
                slope,
                mode,
            )
            .unwrap();
            let mut partitioned = BandSplitPlugin::new_multiband_with_mode(
                2,
                &[500.0, 2_000.0, 8_000.0],
                slope,
                mode,
            )
            .unwrap();
            contiguous.initialize(f64::from(SAMPLE_RATE)).unwrap();
            partitioned.initialize(f64::from(SAMPLE_RATE)).unwrap();

            let mut expected = vec![0.0; frames * 8];
            let mut actual = vec![0.0; frames * 8];
            for (event_index, (event_frame, targets)) in events.iter().copied().enumerate() {
                set_targets(&mut contiguous, targets);
                set_targets(&mut partitioned, targets);
                let end_frame = events
                    .get(event_index + 1)
                    .map_or(frames, |(next_frame, _)| *next_frame);
                process_range(
                    &mut contiguous,
                    &input,
                    &mut expected,
                    event_frame,
                    end_frame,
                    false,
                );
                process_range(
                    &mut partitioned,
                    &input,
                    &mut actual,
                    event_frame,
                    end_frame,
                    true,
                );
            }

            assert!(
                actual.iter().all(|sample| sample.is_finite()),
                "{slope} {mode:?}"
            );
            assert!(
                expected.iter().all(|sample| sample.is_finite()),
                "{slope} {mode:?}"
            );
            assert!(
                actual.iter().all(|sample| sample.abs() <= 1.0),
                "{slope} {mode:?}"
            );
            let input_power = input
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>();
            let mut error_power = 0.0;
            let mut max_sample_residual = 0.0_f64;
            for (actual, expected) in actual.iter().zip(&expected) {
                let error = (f64::from(*actual) - f64::from(*expected)).abs();
                error_power += error * error;
                max_sample_residual = max_sample_residual.max(error);
            }
            let normalized_residual = (error_power / input_power).sqrt();
            assert!(
                normalized_residual <= 2e-5,
                "{slope} {mode:?} timed-event partition RMS residual {normalized_residual}"
            );
            assert!(
                max_sample_residual <= 2e-5,
                "{slope} {mode:?} timed-event max sample residual {max_sample_residual}"
            );
            println!(
                "AUD143 timed automation {slope} {mode:?}: input-normalized RMS={normalized_residual:.8e}, max-absolute={max_sample_residual:.8e}"
            );
        }
    }
}
