//! Independent coherent-tone measurements of the public streaming resampler.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::f64::consts::TAU;

const AMPLITUDE: f64 = 0.5;

fn render(
    input: &[f32],
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    quality: ResamplerQuality,
    partitions: &[usize],
    relative_changes: &[f64],
) -> (Vec<f32>, usize) {
    let mut plugin =
        ResamplerPlugin::with_quality(channels, input_rate, output_rate, 256, quality).unwrap();
    plugin.initialize(f64::from(input_rate)).unwrap();
    if !relative_changes.is_empty() {
        plugin
            .set_parameter(
                ParameterId::from("dynamic_ratio"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        for &factor in relative_changes {
            plugin.set_ratio_relative(factor, false).unwrap();
        }
    }
    let delay = plugin.output_delay_frames();
    let mut output = Vec::new();
    let mut offset = 0;
    let mut partition = 0;
    while offset < input.len() / channels {
        let frames = partitions[partition % partitions.len()].min(input.len() / channels - offset);
        let mut block = vec![f32::NAN; plugin.output_frames_for_input(frames) * channels];
        let written = plugin
            .process(
                &input[offset * channels..(offset + frames) * channels],
                &mut block,
                &ProcessContext::new(input_rate, frames),
            )
            .unwrap();
        assert!(written * channels <= block.len());
        output.extend_from_slice(&block[..written * channels]);
        offset += frames;
        partition += 1;
    }
    let mut complete = false;
    for _ in 0..16 {
        let mut block = vec![f32::NAN; plugin.drain_output_frames_max() * channels];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(input_rate, 0))
            .unwrap();
        output.extend_from_slice(&block[..result.frames * channels]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete, "drain must terminate");
    let ratio = output_rate as f64 / input_rate as f64 * relative_changes.iter().product::<f64>();
    let expected_frames = (input.len() as f64 / channels as f64 * ratio).ceil() as usize + delay;
    assert_eq!(output.len(), expected_frames * channels);
    assert!(output.iter().all(|sample| sample.is_finite()));
    (output, delay)
}

fn tones(frequencies: &[f64], rate: u32) -> Vec<f32> {
    // An extra 37 frames exercises a non-full final chunk and complete drain.
    (0..rate as usize / 2 + 37)
        .flat_map(|frame| {
            frequencies.iter().map(move |frequency| {
                (AMPLITUDE * (TAU * frequency * frame as f64 / rate as f64).sin()) as f32
            })
        })
        .collect()
}

fn coherent_frequency(frequency: f64) -> f64 {
    // Five-hertz spacing completes an integer number of cycles in 200 ms.
    (frequency / 5.0).round() * 5.0
}

fn measure(
    output: &[f32],
    channels: usize,
    channel: usize,
    rate: u32,
    frequency: f64,
) -> (f64, f64, f64) {
    // The measurement lies well inside the stream, beyond startup and before
    // the ending transient. Coherence eliminates rectangular-window leakage.
    let offset = rate as usize / 10;
    let count = rate as usize / 5;
    let samples = (0..count).map(|frame| output[(offset + frame) * channels + channel] as f64);
    let (sin_sum, cos_sum) =
        samples
            .clone()
            .enumerate()
            .fold((0.0, 0.0), |(s, c), (frame, value)| {
                let phase = TAU * frequency * frame as f64 / rate as f64;
                (s + value * phase.sin(), c + value * phase.cos())
            });
    let sin_gain = 2.0 * sin_sum / count as f64;
    let cos_gain = 2.0 * cos_sum / count as f64;
    let fitted_amplitude = sin_gain.hypot(cos_gain);
    let (energy, residual) =
        samples
            .enumerate()
            .fold((0.0, 0.0), |(energy, residual), (frame, value)| {
                let phase = TAU * frequency * frame as f64 / rate as f64;
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
fn fixed_ratios_preserve_coherent_tones_and_reject_stopband_energy() {
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        // Conservative flat-band coverage across every tested ratio, including
        // 4:1 decimation. Filter lengths stay fixed in input samples, so the
        // usable fraction of output Nyquist depends on the quality preset.
        let flat_band_fraction = match quality {
            ResamplerQuality::Fast => 0.2,
            ResamplerQuality::Medium => 0.5,
            ResamplerQuality::High => 0.75,
        };
        for (input_rate, output_rate) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (48_000, 96_000),
            (96_000, 48_000),
            (96_000, 24_000),
        ] {
            let nyquist = input_rate.min(output_rate) as f64 / 2.0;
            let mut frequencies: Vec<f64> = [0.05, 0.2, 0.4, 0.5, 0.75, 0.9]
                .iter()
                .map(|fraction| coherent_frequency(nyquist * fraction))
                .collect();
            if input_rate > output_rate {
                let stopband_width = (input_rate - output_rate) as f64 / 2.0;
                for fraction in [0.01, 0.1, 0.5] {
                    frequencies.push(coherent_frequency(nyquist + stopband_width * fraction));
                }
            }
            let channels = frequencies.len();
            let input = tones(&frequencies, input_rate);
            let (output, delay) = render(
                &input,
                channels,
                input_rate,
                output_rate,
                quality,
                &[1, 7, 131, 509, 2, 1023],
                &[],
            );
            let (regular, _) = render(
                &input,
                channels,
                input_rate,
                output_rate,
                quality,
                &[256],
                &[],
            );
            assert!(
                output == regular,
                "{quality:?} {input_rate}->{output_rate}: callback partition changes audio"
            );
            eprintln!("{quality:?} {input_rate}->{output_rate} delay={delay}");
            for (channel, &frequency) in frequencies.iter().enumerate() {
                let (gain_db, residual_db, energy_db) =
                    measure(&output, channels, channel, output_rate, frequency);
                eprintln!(
                    "  {frequency:8.1} Hz gain={gain_db:8.3} dB residual={residual_db:8.2} dB energy={energy_db:8.2} dB"
                );
                // The independently generated tone must retain its amplitude;
                // total energy alone would miss interpolation images/distortion.
                if frequency <= nyquist * (flat_band_fraction + 0.001) {
                    assert!(
                        gain_db.abs() < 0.01,
                        "{quality:?} {input_rate}->{output_rate} {frequency} Hz gain {gain_db}"
                    );
                    assert!(
                        residual_db < -90.0,
                        "{quality:?} {input_rate}->{output_rate} {frequency} Hz residual {residual_db}"
                    );
                }
                // Midway between output and input Nyquist is beyond the
                // transition band for these presets and representative ratios.
                if input_rate > output_rate && channel == channels - 1 {
                    assert!(
                        energy_db < -60.0,
                        "{quality:?} {input_rate}->{output_rate} {frequency} Hz stopband {energy_db}"
                    );
                }
            }
        }
    }
}

#[test]
fn measure_large_dynamic_downsampling_alias() {
    let input = tones(&[18_000.0], 48_000);
    let (output, _) = render(
        &input,
        1,
        48_000,
        48_000,
        ResamplerQuality::High,
        &[127, 509, 3],
        &[0.5],
    );
    let (_, _, energy_db) = measure(&output, 1, 0, 24_000, 6_000.0);
    eprintln!("48k nominal / dynamic ratio0.5:18kHz aliases to6kHz at {energy_db:.3} dB");
    assert!(
        energy_db < -60.0,
        "large downward ratio changes must suppress aliases: {energy_db}"
    );
}

#[test]
fn cumulative_relative_ratio_matches_the_actual_audio_clock() {
    let input = tones(&[1_200.0], 48_000);
    let (output, _) = render(
        &input,
        1,
        48_000,
        48_000,
        ResamplerQuality::High,
        &[127, 509, 3],
        &[1.1, 1.1],
    );
    let (single_change, _) = render(
        &input,
        1,
        48_000,
        48_000,
        ResamplerQuality::High,
        &[256],
        &[1.1 * 1.1],
    );
    assert!(
        output == single_change,
        "two relative updates must equal their cumulative product"
    );
    // The target clock is 48,000 * 1.1 * 1.1 = 58,080 Hz. Fitting a 1,200 Hz
    // reference at that clock checks the rendered audio, not just the getter.
    let (gain_db, residual_db, _) = measure(&output, 1, 0, 58_080, 1_200.0);
    assert!(
        gain_db.abs() < 0.01,
        "relative ratio frequency error: {gain_db} dB"
    );
    assert!(
        residual_db < -75.0,
        "relative ratio spurious energy: {residual_db} dB"
    );
}

#[test]
fn rejected_relative_ratio_updates_preserve_processing_state() {
    let create = || {
        let mut plugin = ResamplerPlugin::new(1, 48_000, 48_000, 256).unwrap();
        plugin.initialize(48_000.0).unwrap();
        plugin
            .set_parameter(
                ParameterId::from("dynamic_ratio"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        plugin.set_ratio(1.1, false).unwrap();
        plugin
    };
    let mut updated = create();
    let mut unchanged = create();
    for factor in [2.0, 0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            updated.set_ratio_relative(factor, true).is_err(),
            "factor {factor}"
        );
        assert_eq!(updated.current_ratio(), 1.1);
    }
    let input = tones(&[1_200.0], 48_000);
    let mut actual = vec![0.0; updated.output_frames_for_input(input.len())];
    let mut expected = vec![0.0; unchanged.output_frames_for_input(input.len())];
    let context = ProcessContext::new(48_000, input.len());
    let actual_frames = updated.process(&input, &mut actual, &context).unwrap();
    let expected_frames = unchanged.process(&input, &mut expected, &context).unwrap();
    assert_eq!(actual_frames, expected_frames);
    assert!(
        actual == expected,
        "rejected updates must leave the backend unchanged"
    );
}
