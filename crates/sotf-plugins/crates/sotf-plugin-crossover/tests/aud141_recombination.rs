//! Public regression for the LR24 multiway all-pass decomposition.

use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_crossover::CrossoverPlugin;

const SAMPLE_RATE: u32 = 48_000;
const PROBE_HZ: f64 = 1_100.0;
const WARMUP_FRAMES: usize = 48_000;
const MEASURE_FRAMES: usize = 9_600;

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn reciprocal(self) -> Self {
        let norm_squared = self.re * self.re + self.im * self.im;
        Self::new(self.re / norm_squared, -self.im / norm_squared)
    }

    fn divided_by(self, rhs: Self) -> Self {
        self * rhs.reciprocal()
    }

    fn distance(self, rhs: Self) -> f64 {
        (self.re - rhs.re).hypot(self.im - rhs.im)
    }
}

impl std::ops::Add for Complex {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl std::ops::Mul for Complex {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

fn lr24_low_high(cutoff_hz: f64) -> (Complex, Complex) {
    lr24_low_high_at(PROBE_HZ, SAMPLE_RATE, cutoff_hz)
}

fn lr24_low_high_at(probe_hz: f64, sample_rate: u32, cutoff_hz: f64) -> (Complex, Complex) {
    // Independent analog LR4 prototypes evaluated with the prewarped
    // bilinear substitution. For s = j*t, both fourth-order numerators are
    // real, while the shared denominator is (1 - t^2 + j*sqrt(2)*t)^2.
    let sample_rate = f64::from(sample_rate);
    let t = (std::f64::consts::PI * probe_hz / sample_rate).tan()
        / (std::f64::consts::PI * cutoff_hz / sample_rate).tan();
    let denominator = Complex::new(1.0 - t * t, std::f64::consts::SQRT_2 * t);
    let common = (denominator * denominator).reciprocal();
    let low = common;
    let high = Complex::new(t.powi(4), 0.0) * common;
    (low, high)
}

fn project(signal: &[f32], start: usize, frames: usize) -> Complex {
    project_at(signal, start, frames, PROBE_HZ, SAMPLE_RATE)
}

fn project_at(
    signal: &[f32],
    start: usize,
    frames: usize,
    frequency_hz: f64,
    sample_rate: u32,
) -> Complex {
    let mut re = 0.0;
    let mut im = 0.0;
    let omega = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
    for (offset, &sample) in signal[start..start + frames].iter().enumerate() {
        let phase = omega * (start + offset) as f64;
        let sample = f64::from(sample);
        re += sample * phase.cos();
        im -= sample * phase.sin();
    }
    let scale = 2.0 / frames as f64;
    Complex::new(re * scale, im * scale)
}

fn expected_multiway_bands(sample_rate: u32, probe_hz: f64, cutoffs_hz: &[f64]) -> Vec<Complex> {
    let responses: Vec<_> = cutoffs_hz
        .iter()
        .map(|&cutoff| lr24_low_high_at(probe_hz, sample_rate, cutoff))
        .collect();
    let mut bands = Vec::with_capacity(cutoffs_hz.len() + 1);
    for band in 0..cutoffs_hz.len() {
        let mut response = Complex::new(1.0, 0.0);
        for &(_, high) in responses.iter().take(band) {
            response = response * high;
        }
        response = response * responses[band].0;
        for &(low, high) in responses.iter().skip(band + 1) {
            response = response * (low + high);
        }
        bands.push(response);
    }
    let mut final_high = Complex::new(1.0, 0.0);
    for &(_, high) in &responses {
        final_high = final_high * high;
    }
    bands.push(final_high);
    bands
}

fn coherent_probe_cycles(sample_rate: u32, cutoffs_hz: &[f64]) -> Vec<usize> {
    let mut cycles = std::collections::BTreeSet::new();
    let nearest = |frequency_hz: f64| {
        (frequency_hz * MEASURE_FRAMES as f64 / f64::from(sample_rate)).round() as usize
    };
    for (split, &cutoff_hz) in cutoffs_hz.iter().enumerate() {
        let center = nearest(cutoff_hz);
        cycles.insert(center);
        cycles.insert(center.saturating_sub(2));
        cycles.insert(center + 2);
        if split + 1 < cutoffs_hz.len() {
            cycles.insert(nearest((cutoff_hz + cutoffs_hz[split + 1]) * 0.5));
        }
    }
    cycles.insert(nearest(cutoffs_hz[0] * 0.5));
    cycles.insert(nearest(
        (cutoffs_hz[cutoffs_hz.len() - 1] + f64::from(sample_rate) * 0.5) * 0.5,
    ));
    cycles
        .into_iter()
        .filter(|&cycle| {
            let frequency_hz = cycle as f64 * f64::from(sample_rate) / MEASURE_FRAMES as f64;
            frequency_hz > 20.0 && frequency_hz < f64::from(sample_rate) * 0.49
        })
        .collect()
}

fn sine_stereo_input(sample_rate: u32, frames: usize, frequency_hz: f64) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(sample_rate);
            let phase = std::f64::consts::TAU * frequency_hz * time;
            [
                (0.31 * (phase + 0.15).sin()) as f32,
                (0.23 * (phase + 1.05).sin()) as f32,
            ]
        })
        .collect()
}

fn rms(signal: &[f32]) -> f64 {
    (signal
        .iter()
        .map(|&sample| f64::from(sample).powi(2))
        .sum::<f64>()
        / signal.len() as f64)
        .sqrt()
}

fn projected_residual_ratio(
    signal: &[f32],
    start: usize,
    frames: usize,
    frequency_hz: f64,
    sample_rate: u32,
    input_rms: f64,
) -> f64 {
    let projection = project_at(signal, start, frames, frequency_hz, sample_rate);
    let omega = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
    let residual_energy = signal[start..start + frames]
        .iter()
        .enumerate()
        .map(|(offset, &sample)| {
            let phase = omega * (start + offset) as f64;
            let fitted = projection.re * phase.cos() - projection.im * phase.sin();
            (f64::from(sample) - fitted).powi(2)
        })
        .sum::<f64>();
    (residual_energy / frames as f64).sqrt() / input_rms
}

fn assert_multiway_frequency_response(sample_rate: u32, cutoffs_hz: &[f64], probe_hz: f64) {
    let total_frames = sample_rate as usize + MEASURE_FRAMES;
    let input = sine_stereo_input(sample_rate, total_frames, probe_hz);
    let mut plugin =
        CrossoverPlugin::new_multiway(2, "LR24", cutoffs_hz[0], "both", &cutoffs_hz[1..]).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let output_channels = plugin.output_channels();
    let mut output = vec![f32::NAN; total_frames * output_channels];
    assert_eq!(
        plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(sample_rate, total_frames),
            )
            .unwrap(),
        total_frames
    );

    let expected_bands = expected_multiway_bands(sample_rate, probe_hz, cutoffs_hz);
    let expected_sum = expected_bands
        .iter()
        .copied()
        .fold(Complex::new(0.0, 0.0), |sum, band| sum + band);
    let expected_allpass = cutoffs_hz
        .iter()
        .fold(Complex::new(1.0, 0.0), |product, &cutoff| {
            let (low, high) = lr24_low_high_at(probe_hz, sample_rate, cutoff);
            product * (low + high)
        });
    assert!(expected_sum.distance(expected_allpass) < 1.0e-12);
    assert!((expected_sum.norm() - 1.0).abs() < 1.0e-12);

    let mut max_branch_error = 0.0_f64;
    let mut max_branch_residual = 0.0_f64;
    let mut max_sum_error = 0.0_f64;
    let mut max_sum_residual = 0.0_f64;
    for channel in 0..2 {
        let input_channel: Vec<f32> = input.iter().skip(channel).step_by(2).copied().collect();
        let input_rms =
            rms(&input_channel[sample_rate as usize..sample_rate as usize + MEASURE_FRAMES]);
        assert!(
            input_rms > 0.1,
            "input channel {channel} has insufficient tone energy"
        );
        let input_projection = project_at(
            &input_channel,
            sample_rate as usize,
            MEASURE_FRAMES,
            probe_hz,
            sample_rate,
        );
        assert!(
            input_projection.norm() > 0.1,
            "input channel {channel} projection is too small: {input_projection:?}"
        );
        for (band, expected) in expected_bands.iter().copied().enumerate() {
            let output_channel: Vec<f32> = output
                .chunks_exact(output_channels)
                .map(|frame| frame[band * 2 + channel])
                .collect();
            let output_projection = project_at(
                &output_channel,
                sample_rate as usize,
                MEASURE_FRAMES,
                probe_hz,
                sample_rate,
            );
            let measured = output_projection.divided_by(input_projection);
            let error = measured.distance(expected);
            max_branch_error = max_branch_error.max(error);
            assert!(
                error <= 2.0e-3,
                "rate={sample_rate} cutoffs={cutoffs_hz:?} probe={probe_hz} channel={channel} band={band}: complex error={error}, measured={measured:?}, expected={expected:?}"
            );
            let residual_ratio = projected_residual_ratio(
                &output_channel,
                sample_rate as usize,
                MEASURE_FRAMES,
                probe_hz,
                sample_rate,
                input_rms,
            );
            max_branch_residual = max_branch_residual.max(residual_ratio);
            assert!(
                residual_ratio <= 0.01,
                "rate={sample_rate} cutoffs={cutoffs_hz:?} probe={probe_hz} channel={channel} band={band}: residual/input RMS={residual_ratio} exceeds 1%"
            );
        }

        let mut summed_channel = vec![0.0_f32; total_frames];
        for (frame_index, frame) in output.chunks_exact(output_channels).enumerate() {
            for band in 0..expected_bands.len() {
                summed_channel[frame_index] += frame[band * 2 + channel];
            }
        }
        let summed_projection = project_at(
            &summed_channel,
            sample_rate as usize,
            MEASURE_FRAMES,
            probe_hz,
            sample_rate,
        );
        let measured_sum = summed_projection.divided_by(input_projection);
        let sum_error = measured_sum.distance(expected_allpass);
        max_sum_error = max_sum_error.max(sum_error);
        assert!(
            sum_error <= 2.0e-3,
            "rate={sample_rate} cutoffs={cutoffs_hz:?} probe={probe_hz} channel={channel}: summed complex error={sum_error}, measured={measured_sum:?}, expected={expected_allpass:?}"
        );
        assert!(
            (measured_sum.norm() - 1.0).abs() <= 2.0e-3,
            "rate={sample_rate} cutoffs={cutoffs_hz:?} probe={probe_hz} channel={channel}: summed magnitude={} differs from unity",
            measured_sum.norm()
        );
        let sum_residual_ratio = projected_residual_ratio(
            &summed_channel,
            sample_rate as usize,
            MEASURE_FRAMES,
            probe_hz,
            sample_rate,
            input_rms,
        );
        max_sum_residual = max_sum_residual.max(sum_residual_ratio);
        assert!(
            sum_residual_ratio <= 0.01,
            "rate={sample_rate} cutoffs={cutoffs_hz:?} probe={probe_hz} channel={channel}: summed residual/input RMS={sum_residual_ratio} exceeds 1%"
        );
    }
    println!(
        "matrix rate={sample_rate} cuts={cutoffs_hz:?} probe={probe_hz}: max branch complex error={max_branch_error:.3e}, max summed complex error={max_sum_error:.3e}, max branch residual/input RMS={max_branch_residual:.3e}, max summed residual/input RMS={max_sum_residual:.3e}"
    );
}

#[test]
fn overlapping_multiway_lr24_bands_match_allpass_decomposition() {
    let (low0, high0) = lr24_low_high(1_000.0);
    let (low1, high1) = lr24_low_high(1_200.0);
    let expected_bands = [low0 * (low1 + high1), high0 * low1, high0 * high1];
    let expected_sum = expected_bands
        .iter()
        .copied()
        .fold(Complex::new(0.0, 0.0), |sum, band| sum + band);
    let expected_allpass = (low0 + high0) * (low1 + high1);
    assert!((expected_sum.re - expected_allpass.re).abs() < 1.0e-12);
    assert!((expected_sum.im - expected_allpass.im).abs() < 1.0e-12);
    assert!((expected_allpass.norm() - 1.0).abs() < 1.0e-12);

    let total_frames = WARMUP_FRAMES + MEASURE_FRAMES;
    let input: Vec<f32> = (0..total_frames)
        .map(|frame| {
            (std::f64::consts::TAU * PROBE_HZ * frame as f64 / f64::from(SAMPLE_RATE)).sin() as f32
                * 0.25
        })
        .collect();
    let mut plugin = CrossoverPlugin::new_multiway(1, "LR24", 1_000.0, "both", &[1_200.0]).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    let mut output = vec![0.0_f32; total_frames * 3];
    plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, total_frames),
        )
        .unwrap();

    let input_response = project(&input, WARMUP_FRAMES, MEASURE_FRAMES);
    assert!(
        input_response.norm() > 0.1,
        "coherent input tone projection is too small to normalize residuals: {input_response:?}"
    );
    let actual_bands: [Complex; 3] = std::array::from_fn(|band| {
        let channel: Vec<f32> = output[band..].iter().step_by(3).copied().collect();
        project(&channel, WARMUP_FRAMES, MEASURE_FRAMES)
    });
    let actual_transfer: [Complex; 3] = std::array::from_fn(|band| {
        let re =
            actual_bands[band].re * input_response.re + actual_bands[band].im * input_response.im;
        let im =
            actual_bands[band].im * input_response.re - actual_bands[band].re * input_response.im;
        let denom = input_response.re * input_response.re + input_response.im * input_response.im;
        Complex::new(re / denom, im / denom)
    });
    let actual_sum = actual_transfer
        .iter()
        .copied()
        .fold(Complex::new(0.0, 0.0), |sum, band| sum + band);
    let normalized_residual = actual_transfer
        .iter()
        .zip(expected_bands)
        .map(|(actual, expected)| {
            (actual.re - expected.re).powi(2) + (actual.im - expected.im).powi(2)
        })
        .sum::<f64>()
        .sqrt();

    println!("input projection: {input_response:?}");
    println!("actual band transfers: {actual_transfer:?}");
    println!("expected band transfers: {expected_bands:?}");
    println!("input-normalized branch residual: {normalized_residual}");
    println!(
        "actual summed transfer: {actual_sum:?}, magnitude={}",
        actual_sum.norm()
    );
    println!("expected all-pass transfer: {expected_allpass:?}");

    for (band, (actual, expected)) in actual_transfer.iter().zip(expected_bands).enumerate() {
        let error = (actual.re - expected.re).hypot(actual.im - expected.im);
        assert!(
            error < 2.0e-3,
            "band {band} complex error {error}: actual={actual:?}, expected={expected:?}"
        );
    }
    assert!(
        normalized_residual < 2.0e-3,
        "input-normalized branch residual {normalized_residual} exceeds 0.002"
    );
    let sum_error =
        (actual_sum.re - expected_allpass.re).hypot(actual_sum.im - expected_allpass.im);
    assert!(
        sum_error < 2.0e-3,
        "summed complex error {sum_error}: actual={actual_sum:?}, expected={expected_allpass:?}"
    );
}

#[test]
fn lr24_multiway_complex_response_matrix_matches_independent_reference() {
    let cutoffs = [
        vec![1_000.0, 1_200.0],
        vec![500.0, 8_000.0],
        vec![1_000.0, 1_100.0, 1_200.0],
        vec![500.0, 2_000.0, 8_000.0],
    ];
    for sample_rate in [44_100, 48_000, 96_000] {
        for cutoffs_hz in &cutoffs {
            for cycle in coherent_probe_cycles(sample_rate, cutoffs_hz) {
                let probe_hz = cycle as f64 * f64::from(sample_rate) / MEASURE_FRAMES as f64;
                assert_multiway_frequency_response(sample_rate, cutoffs_hz, probe_hz);
            }
        }
    }
}

#[test]
fn multiway_low_and_high_modes_select_the_named_bands() {
    let frames = 2_113;
    let input = sine_stereo_input(SAMPLE_RATE, frames, 1_100.0);
    let render = |mode: &str| {
        let mut plugin =
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, mode, &[1_800.0, 5_000.0]).unwrap();
        plugin.initialize(SAMPLE_RATE).unwrap();
        let mut output = vec![0.0; frames * plugin.output_channels()];
        plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .unwrap();
        output
    };
    let both = render("both");
    let low = render("low");
    let high = render("high");
    for frame in 0..frames {
        for channel in 0..2 {
            assert_eq!(low[frame * 2 + channel], both[frame * 8 + channel]);
            assert_eq!(high[frame * 2 + channel], both[frame * 8 + 6 + channel]);
        }
    }
}

fn render_automated(partitions: &[usize]) -> Vec<f32> {
    const FRAMES: usize = 12_287;
    const EVENTS: [(usize, &str, f32); 2] = [
        (4_096, "frequency_2", 2_000.0),
        (8_192, "frequency", 1_000.0),
    ];
    let input = sine_stereo_input(SAMPLE_RATE, FRAMES, 1_347.0);
    let mut plugin =
        CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    let output_channels = plugin.output_channels();
    let mut output = vec![0.0; FRAMES * output_channels];
    let mut cursor = 0;
    let mut part_index = 0;
    let mut event_index = 0;
    while cursor < FRAMES {
        if let Some(&(event_frame, parameter, value)) = EVENTS.get(event_index)
            && event_frame == cursor
        {
            plugin
                .set_parameter(
                    sotf_host::parameters::ParameterId::from(parameter),
                    sotf_host::parameters::ParameterValue::Float(value),
                )
                .unwrap();
            event_index += 1;
        }
        let mut count = partitions[part_index % partitions.len()].min(FRAMES - cursor);
        if let Some(&(event_frame, _, _)) = EVENTS.get(event_index) {
            count = count.min(event_frame - cursor);
        }
        plugin
            .process(
                &input[cursor * 2..(cursor + count) * 2],
                &mut output[cursor * output_channels..(cursor + count) * output_channels],
                &ProcessContext::new(SAMPLE_RATE, count),
            )
            .unwrap();
        cursor += count;
        part_index += 1;
    }
    output
}

#[test]
fn frequency_automation_uses_absolute_event_frames_across_partitions() {
    let large_partitions = render_automated(&[4_096, 4_096, 4_095]);
    let irregular_partitions = render_automated(&[1, 7, 31, 128, 509, 17, 1_003]);
    // The two input channels peak at 0.31 and 0.23. This ceiling is over six
    // times the larger input amplitude, leaving broad headroom for bounded
    // transients as the smoothed cutoffs change while rejecting runaway output.
    const AUTOMATION_OUTPUT_PEAK_LIMIT: f32 = 2.0;
    assert_eq!(large_partitions.len(), irregular_partitions.len());
    for (name, rendered) in [
        ("large partitions", large_partitions.as_slice()),
        ("irregular partitions", irregular_partitions.as_slice()),
    ] {
        assert!(
            rendered.iter().all(|sample| sample.is_finite()),
            "{name} automation output contains a non-finite sample"
        );
        let peak = rendered
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            peak <= AUTOMATION_OUTPUT_PEAK_LIMIT,
            "{name} automation output peak {peak} exceeds the predeclared bound {AUTOMATION_OUTPUT_PEAK_LIMIT}"
        );
    }
    let max_difference = large_partitions
        .iter()
        .zip(&irregular_partitions)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_difference <= 1.0e-6,
        "absolute-frame automation differs by {max_difference} across callback partitions"
    );
}

#[test]
fn rejected_frequency_update_preserves_the_following_audio_state() {
    let frames = 4_096;
    let split_frame = 1_537;
    let input = sine_stereo_input(SAMPLE_RATE, frames, 1_347.0);
    let mut tested =
        CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap();
    let mut control =
        CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap();
    tested.initialize(SAMPLE_RATE).unwrap();
    control.initialize(SAMPLE_RATE).unwrap();
    let output_channels = tested.output_channels();
    let mut tested_output = vec![0.0; frames * output_channels];
    let mut control_output = vec![0.0; frames * output_channels];
    tested
        .process(
            &input[..split_frame * 2],
            &mut tested_output[..split_frame * output_channels],
            &ProcessContext::new(SAMPLE_RATE, split_frame),
        )
        .unwrap();
    control
        .process(
            &input[..split_frame * 2],
            &mut control_output[..split_frame * output_channels],
            &ProcessContext::new(SAMPLE_RATE, split_frame),
        )
        .unwrap();

    let rejected = tested.set_parameter(
        sotf_host::parameters::ParameterId::from("frequency_2"),
        sotf_host::parameters::ParameterValue::Float(500.0),
    );
    assert!(rejected.is_err());
    tested
        .process(
            &input[split_frame * 2..],
            &mut tested_output[split_frame * output_channels..],
            &ProcessContext::new(SAMPLE_RATE, frames - split_frame),
        )
        .unwrap();
    control
        .process(
            &input[split_frame * 2..],
            &mut control_output[split_frame * output_channels..],
            &ProcessContext::new(SAMPLE_RATE, frames - split_frame),
        )
        .unwrap();
    assert_eq!(tested_output, control_output);
}

#[test]
fn reset_and_reinitialize_match_fresh_multiway_instances() {
    let warmup = sine_stereo_input(48_000, 2_047, 1_347.0);
    let input_48k = sine_stereo_input(48_000, 4_096, 1_347.0);
    let input_44k = sine_stereo_input(44_100, 4_096, 1_347.0);
    let mut reused =
        CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap();
    reused.initialize(48_000).unwrap();
    let mut warm_output = vec![0.0; warmup.len() * 4];
    reused
        .process(
            &warmup,
            &mut warm_output,
            &ProcessContext::new(48_000, warmup.len() / 2),
        )
        .unwrap();

    reused.reset();
    let mut fresh_48k =
        CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap();
    fresh_48k.initialize(48_000).unwrap();
    let mut reused_output = vec![0.0; input_48k.len() * 4];
    let mut fresh_output = vec![0.0; input_48k.len() * 4];
    reused
        .process(
            &input_48k,
            &mut reused_output,
            &ProcessContext::new(48_000, input_48k.len() / 2),
        )
        .unwrap();
    fresh_48k
        .process(
            &input_48k,
            &mut fresh_output,
            &ProcessContext::new(48_000, input_48k.len() / 2),
        )
        .unwrap();
    assert_eq!(reused_output, fresh_output);

    reused.initialize(44_100).unwrap();
    let mut fresh_44k =
        CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap();
    fresh_44k.initialize(44_100).unwrap();
    let mut reused_output = vec![0.0; input_44k.len() * 4];
    let mut fresh_output = vec![0.0; input_44k.len() * 4];
    let context = ProcessContext::new(44_100, input_44k.len() / 2);
    reused
        .process(&input_44k, &mut reused_output, &context)
        .unwrap();
    fresh_44k
        .process(&input_44k, &mut fresh_output, &context)
        .unwrap();
    assert_eq!(reused_output, fresh_output);
}
