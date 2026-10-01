use super::offline_render_config::OfflineRenderConfig;
use super::render::render_offline;
use super::render::render_passthrough;
use super::render::render_timeline;
use super::render_progress::RenderProgress;
use super::types::OutputFormat;
use crate::decoder::source::AudioSource;
use crate::engine::PluginConfig;
use crate::plugins::{PluginSettings, PluginType};
use std::path::Path;

mod endpoint_composition;
mod signal_delay;
mod tail_duration;

fn create_test_wav(path: &Path, sample_rate: u32, channels: u16, num_frames: usize) {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in 0..num_frames {
        let val = (frame as f32 / num_frames as f32) * 2.0 - 1.0; // ramp -1..1
        for _ in 0..channels {
            writer.write_sample(val).unwrap();
        }
    }
    writer.finalize().unwrap();
}

fn create_constant_test_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    num_frames: usize,
    value: f32,
) {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..num_frames * usize::from(channels) {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

fn create_stereo_tone_test_wav(path: &Path, sample_rate: u32, num_frames: usize) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in 0..num_frames {
        let time = frame as f64 / f64::from(sample_rate);
        writer
            .write_sample((std::f64::consts::TAU * 2_000.0 * time).sin() as f32 * 0.5)
            .unwrap();
        writer
            .write_sample((std::f64::consts::TAU * 7_000.0 * time).cos() as f32 * 0.3)
            .unwrap();
    }
    writer.finalize().unwrap();
}

fn typed_band_split_config(
    frequencies: Option<Vec<f64>>,
    num_bands: usize,
    crossover_type: &str,
    phase_compensated: bool,
) -> PluginConfig {
    let mut settings = PluginSettings::default_for(&PluginType::BandSplit).unwrap();
    let primary_frequency = frequencies
        .as_ref()
        .and_then(|frequencies| frequencies.first())
        .copied()
        .unwrap_or(900.0);
    let second_frequency = frequencies
        .as_ref()
        .and_then(|frequencies| frequencies.get(1))
        .copied()
        .unwrap_or(4_000.0);
    let third_frequency = frequencies
        .as_ref()
        .and_then(|frequencies| frequencies.get(2))
        .copied()
        .unwrap_or(12_000.0);
    if let PluginSettings::BandSplit {
        channels,
        frequency,
        crossover_type: setting_type,
        frequencies: setting_frequencies,
        recombination_mode,
        num_bands: setting_bands,
        frequency_2,
        frequency_3,
    } = &mut settings
    {
        *channels = 2;
        *frequency = primary_frequency;
        *setting_type = crossover_type.to_string();
        *setting_frequencies = frequencies;
        *recombination_mode = serde_json::from_value(if phase_compensated {
            serde_json::json!("phase_compensated")
        } else {
            serde_json::json!("legacy_cascade")
        })
        .unwrap();
        *setting_bands = num_bands;
        *frequency_2 = second_frequency;
        *frequency_3 = third_frequency;
    } else {
        unreachable!("BandSplit default returned another plugin settings variant");
    }
    settings.to_plugin_config(48_000.0)
}

type BandSplitComplex = (f64, f64);

fn complex_add(left: BandSplitComplex, right: BandSplitComplex) -> BandSplitComplex {
    (left.0 + right.0, left.1 + right.1)
}

fn complex_mul(left: BandSplitComplex, right: BandSplitComplex) -> BandSplitComplex {
    (
        left.0 * right.0 - left.1 * right.1,
        left.0 * right.1 + left.1 * right.0,
    )
}

fn complex_reciprocal(value: BandSplitComplex) -> BandSplitComplex {
    let norm_squared = value.0 * value.0 + value.1 * value.1;
    (value.0 / norm_squared, -value.1 / norm_squared)
}

fn complex_power(value: BandSplitComplex, power: u32) -> BandSplitComplex {
    (0..power).fold((1.0, 0.0), |product, _| complex_mul(product, value))
}

fn independent_lr_allpass_response(
    slope: &str,
    probe_hz: f64,
    cutoff_hz: f64,
    sample_rate: u32,
) -> BandSplitComplex {
    let normalized = (std::f64::consts::PI * probe_hz / f64::from(sample_rate)).tan()
        / (std::f64::consts::PI * cutoff_hz / f64::from(sample_rate)).tan();
    let s = (0.0, normalized);
    let s_squared = complex_mul(s, s);
    let (denominator, high_order) = match slope {
        "LR24" => {
            let section = (
                s_squared.0 + 1.0,
                s_squared.1 + std::f64::consts::SQRT_2 * normalized,
            );
            (complex_mul(section, section), 4)
        }
        "LR48" => {
            let q1 = 1.0 / (2.0 * (std::f64::consts::PI / 8.0).sin());
            let q2 = 1.0 / (2.0 * (3.0 * std::f64::consts::PI / 8.0).sin());
            let first = (s_squared.0 + 1.0, s_squared.1 + normalized / q1);
            let second = (s_squared.0 + 1.0, s_squared.1 + normalized / q2);
            let butterworth_fourth = complex_mul(first, second);
            (complex_mul(butterworth_fourth, butterworth_fourth), 8)
        }
        _ => panic!("unsupported BandSplit slope {slope}"),
    };
    let low = complex_reciprocal(denominator);
    let high = complex_mul(complex_power(s, high_order), low);
    complex_add(low, high)
}

fn independent_split_merge_response(
    slope: &str,
    frequencies: &[f64],
    probe_hz: f64,
    sample_rate: u32,
) -> BandSplitComplex {
    frequencies.iter().fold((1.0, 0.0), |product, cutoff| {
        complex_mul(
            product,
            independent_lr_allpass_response(slope, probe_hz, *cutoff, sample_rate),
        )
    })
}

fn typed_band_merge_config(num_bands: usize) -> PluginConfig {
    let mut settings = PluginSettings::default_for(&PluginType::BandMerge).unwrap();
    if let PluginSettings::BandMerge { channels, bands } = &mut settings {
        *channels = 2;
        *bands = num_bands;
    } else {
        unreachable!("BandMerge default returned another plugin settings variant");
    }
    settings.to_plugin_config(48_000.0)
}

fn create_impulse_test_wav(path: &Path, num_frames: usize, impulse_frames: &[usize]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in 0..num_frames {
        writer
            .write_sample(if impulse_frames.contains(&frame) {
                1.0f32
            } else {
                0.0
            })
            .unwrap();
    }
    writer.finalize().unwrap();
}

fn convolution_plugin_config(ir_path: &Path) -> PluginConfig {
    PluginConfig {
        plugin_type: "convolution".into(),
        parameters: serde_json::json!({
            "ir_file": ir_path,
            "mix": 1.0,
            "gain_db": 0.0,
            "use_nupc": false,
            "zero_latency_head": false,
            "head_taps": 0
        }),
    }
}

#[test]
fn test_offline_render_passthrough() {
    let dir = std::env::temp_dir().join("sotf_test_offline");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let output_path = dir.join("output_passthrough.wav");

    let sr = 48000;
    let ch = 2;
    let frames = 4800; // 100ms
    create_test_wav(&input_path, sr, ch, frames);

    render_passthrough(&input_path, &output_path, 32).unwrap();

    // Verify output exists and has correct spec
    let reader = hound::WavReader::open(&output_path).unwrap();
    let out_spec = reader.spec();
    assert_eq!(out_spec.sample_rate, sr);
    assert_eq!(out_spec.channels, ch);

    // Verify sample count matches
    let out_samples: Vec<f32> = reader.into_samples::<f32>().map(|s| s.unwrap()).collect();
    assert_eq!(
        out_samples.len(),
        frames * ch as usize,
        "Output should have same number of samples as input"
    );

    // Clean up
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_offline_render_with_gain() {
    let dir = std::env::temp_dir().join("sotf_test_offline_gain");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let output_path = dir.join("output_gain.wav");

    let sr = 48000;
    let ch = 2;
    let frames = 480;
    create_test_wav(&input_path, sr, ch, frames);

    let config = OfflineRenderConfig {
        source: AudioSource::File(input_path.clone()),
        output_path: output_path.clone(),
        format: OutputFormat::Wav {
            bits_per_sample: 32,
        },
        plugins: vec![PluginConfig {
            plugin_type: "gain".to_string(),
            parameters: serde_json::json!({ "gain_db": -6.0 }),
        }],
        frame_size: 256,
        output_sample_rate: None,
    };

    render_offline(&config, None).unwrap();

    // Read input and output
    let in_reader = hound::WavReader::open(&input_path).unwrap();
    let in_samples: Vec<f32> = in_reader
        .into_samples::<f32>()
        .map(|s| s.unwrap())
        .collect();

    let out_reader = hound::WavReader::open(&output_path).unwrap();
    let out_samples: Vec<f32> = out_reader
        .into_samples::<f32>()
        .map(|s| s.unwrap())
        .collect();

    assert_eq!(in_samples.len(), out_samples.len());

    // -6dB ≈ 0.5012 gain. Output should be roughly half the input amplitude.
    // Skip first few samples (gain smoother ramp-up) and check the tail.
    let gain_linear = 10.0f32.powf(-6.0 / 20.0);
    let check_start = (frames * ch as usize) / 2; // check second half
    for i in check_start..in_samples.len() {
        let expected = in_samples[i] * gain_linear;
        assert!(
            (out_samples[i] - expected).abs() < 0.05,
            "Sample {i}: expected ~{expected:.4}, got {:.4}",
            out_samples[i]
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_offline_render_progress() {
    let dir = std::env::temp_dir().join("sotf_test_offline_progress");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let output_path = dir.join("output_progress.wav");

    create_test_wav(&input_path, 48000, 1, 9600);

    let config = OfflineRenderConfig::new(AudioSource::File(input_path), &output_path);

    let mut progress_calls = 0u32;
    let mut last_frames = 0u64;

    render_offline(
        &config,
        Some(&mut |p: &RenderProgress| {
            progress_calls += 1;
            assert!(
                p.frames_processed >= last_frames,
                "Progress should be monotonically increasing"
            );
            last_frames = p.frames_processed;
            assert!(p.total_frames.is_some());
        }),
    )
    .unwrap();

    assert!(
        progress_calls > 0,
        "Progress callback should have been called"
    );
    assert!(last_frames > 0, "Should have processed frames");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_offline_render_resamples_to_requested_rate_and_duration() {
    let dir = std::env::temp_dir().join("sotf_test_offline_resample");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let output_path = dir.join("output.wav");
    create_test_wav(&input_path, 48_000, 2, 4_800);

    let mut config = OfflineRenderConfig::new(AudioSource::File(input_path), &output_path);
    config.output_sample_rate = Some(44_100);
    config.frame_size = 257;
    render_offline(&config, None).unwrap();

    let reader = hound::WavReader::open(&output_path).unwrap();
    assert_eq!(reader.spec().sample_rate, 44_100);
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.duration(), 4_410);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rate_changing_chain_preserves_export_clock_pitch_and_duration() {
    let directory = tempfile::tempdir().unwrap();
    // A prime-sized partial source frame count exercises rational duration
    // rounding separately from the callback and resampler chunk sizes.
    for (source_rate, requested_rate, chain_rate) in [
        (48_000, None, 96_000),
        (96_000, None, 48_000),
        (44_100, Some(48_000), 44_100),
        (48_000, Some(44_100), 96_000),
    ] {
        let export_rate = requested_rate.unwrap_or(source_rate);
        let source_frames = source_rate as usize / 5 + 37;
        let expected_frames =
            (source_frames as u64 * u64::from(export_rate)).div_ceil(u64::from(source_rate));
        let input_path = directory.path().join("input.wav");
        let mut writer = hound::WavWriter::create(
            &input_path,
            hound::WavSpec {
                channels: 2,
                sample_rate: source_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for frame in 0..source_frames {
            let tone = (std::f64::consts::TAU * 1_000.0 * frame as f64 / f64::from(source_rate))
                .sin() as f32
                * 0.25;
            writer.write_sample(tone).unwrap();
            writer
                .write_sample(if frame == 0 || frame == source_frames - 1 {
                    1.0_f32
                } else {
                    0.0
                })
                .unwrap();
        }
        writer.finalize().unwrap();

        for frame_size in [127, 257, 1_024] {
            let output_path = directory.path().join("output.wav");
            let mut config =
                OfflineRenderConfig::new(AudioSource::File(input_path.clone()), &output_path);
            config.output_sample_rate = requested_rate;
            config.frame_size = frame_size;
            config.plugins.push(PluginConfig::new(
                "resampler",
                serde_json::json!({
                    "input_sample_rate": export_rate,
                    "output_sample_rate": chain_rate,
                    "chunk_size": 64
                }),
            ));
            let mut progress = Vec::new();
            render_offline(&config, Some(&mut |value| progress.push(value.clone()))).unwrap();
            let reader = hound::WavReader::open(&output_path).unwrap();
            let spec = reader.spec();
            assert_eq!(spec.sample_rate, export_rate);
            assert_eq!(spec.channels, 2);
            assert_eq!(
                u64::from(reader.duration()),
                expected_frames,
                "source={source_rate}, export={export_rate}, chain={chain_rate}, block={frame_size}"
            );
            for value in &progress {
                assert_eq!(value.total_frames, Some(expected_frames));
                assert!(value.frames_processed <= expected_frames);
            }
            assert_eq!(progress.last().unwrap().frames_processed, expected_frames);
            let samples: Vec<f32> = reader.into_samples::<f32>().map(Result::unwrap).collect();
            let (frames, remainder) = samples.as_chunks::<2>();
            assert!(remainder.is_empty());
            // Zero-crossing interpolation is independent of the production
            // resampler and checks the frequency implied by the file header.
            let stable_start = export_rate as usize / 20;
            let stable_end = export_rate as usize * 3 / 20;
            let crossings: Vec<f64> = (stable_start..stable_end)
                .filter_map(|frame| {
                    let a = f64::from(frames[frame][0]);
                    let b = f64::from(frames[frame + 1][0]);
                    (a <= 0.0 && b > 0.0).then(|| frame as f64 - a / (b - a))
                })
                .collect();
            assert!(crossings.len() > 50);
            let frequency = (crossings.len() - 1) as f64 * f64::from(spec.sample_rate)
                / (crossings.last().unwrap() - crossings[0]);
            assert!(
                (frequency - 1_000.0).abs() < 0.1,
                "source={source_rate}, export={export_rate}, chain={chain_rate}: {frequency} Hz"
            );
            // Conversion can distribute an impulse over neighboring samples,
            // but must preserve both program boundaries after delay removal.
            let first_peak = frames[..4]
                .iter()
                .map(|frame| frame[1].abs())
                .fold(0.0_f32, f32::max);
            let final_peak = frames[frames.len() - 4..]
                .iter()
                .map(|frame| frame[1].abs())
                .fold(0.0_f32, f32::max);
            assert!(
                first_peak > 0.2,
                "source={source_rate}, export={export_rate}, chain={chain_rate}, block={frame_size}: first impulse lost: {first_peak}, samples={:?}",
                &frames[..12]
            );
            assert!(
                final_peak > 0.2,
                "source={source_rate}, export={export_rate}, chain={chain_rate}, block={frame_size}: final impulse lost: {final_peak}"
            );
            let first_peak_frame = frames[..16]
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a[1].abs().total_cmp(&b[1].abs()))
                .unwrap()
                .0;
            let final_peak_frame = frames.len() - 16
                + frames[frames.len() - 16..]
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a[1].abs().total_cmp(&b[1].abs()))
                    .unwrap()
                    .0;
            let expected_final = ((source_frames - 1) as f64 * f64::from(export_rate)
                / f64::from(source_rate))
            .round() as usize;
            assert!(first_peak_frame <= 1, "first peak at {first_peak_frame}");
            assert!(
                final_peak_frame.abs_diff(expected_final) <= 1,
                "final peak at {final_peak_frame}, expected {expected_final}"
            );
        }
    }
}

#[test]
fn test_offline_render_supports_integer_wav_depths_with_dither() {
    for bits_per_sample in [16, 24] {
        let dir = std::env::temp_dir().join(format!("sotf_test_offline_integer_{bits_per_sample}"));
        std::fs::create_dir_all(&dir).unwrap();
        let input_path = dir.join("silence.wav");
        let output_path = dir.join("output.wav");
        create_constant_test_wav(&input_path, 48_000, 1, 4_096, 0.0);

        let mut config = OfflineRenderConfig::new(AudioSource::File(input_path), &output_path);
        config.format = OutputFormat::Wav { bits_per_sample };
        render_offline(&config, None).unwrap();

        let reader = hound::WavReader::open(&output_path).unwrap();
        assert_eq!(reader.spec().bits_per_sample, bits_per_sample);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        let codes: Vec<i32> = reader
            .into_samples::<i32>()
            .map(|sample| sample.unwrap())
            .collect();
        assert!(codes.iter().any(|&sample| sample < 0));
        assert!(codes.iter().any(|&sample| sample > 0));
        assert!(codes.iter().all(|&sample| (-1..=1).contains(&sample)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn test_offline_float_render_preserves_headroom() {
    let dir = std::env::temp_dir().join("sotf_test_offline_float_headroom");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let output_path = dir.join("output.wav");
    create_constant_test_wav(&input_path, 48_000, 1, 128, 1.25);

    render_passthrough(&input_path, &output_path, 32).unwrap();

    let reader = hound::WavReader::open(&output_path).unwrap();
    let peak = reader
        .into_samples::<f32>()
        .map(|sample| sample.unwrap().abs())
        .fold(0.0f32, f32::max);
    assert!(peak > 1.2, "float output clipped headroom to {peak}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_offline_render_compensates_and_drains_plugin_latency() {
    let dir = std::env::temp_dir().join("sotf_test_offline_plugin_latency");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let ir_path = dir.join("ir.wav");
    let output_path = dir.join("output.wav");
    let frames = 2_048;
    create_impulse_test_wav(&input_path, frames, &[0, frames - 1]);
    create_impulse_test_wav(&ir_path, 1, &[0]);

    let mut config = OfflineRenderConfig::new(AudioSource::File(input_path), &output_path);
    config.frame_size = 127;
    config.plugins.push(convolution_plugin_config(&ir_path));
    render_offline(&config, None).unwrap();

    let reader = hound::WavReader::open(&output_path).unwrap();
    assert_eq!(reader.duration(), frames as u32);
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .map(|sample| sample.unwrap())
        .collect();
    let strongest: Vec<(usize, f32)> = samples
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, sample)| sample.abs() > 0.5)
        .collect();
    assert!(
        samples[0] > 0.9,
        "first impulse shifted to {}; strongest samples: {strongest:?}",
        samples[0]
    );
    assert!(
        samples[frames - 1] > 0.9,
        "final impulse was truncated to {}; strongest samples: {strongest:?}",
        samples[frames - 1],
    );
    assert_eq!(strongest.len(), 2, "unexpected impulses: {strongest:?}");
    assert_eq!(strongest[0].0, 0);
    assert_eq!(strongest[1].0, frames - 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_offline_render_rejects_invalid_configuration() {
    let dir = std::env::temp_dir().join("sotf_test_offline_invalid");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    create_test_wav(&input_path, 48_000, 1, 16);

    let mut config =
        OfflineRenderConfig::new(AudioSource::File(input_path), dir.join("output.wav"));
    config.frame_size = 0;
    assert!(
        render_offline(&config, None)
            .unwrap_err()
            .contains("frame_size")
    );

    config.frame_size = 16;
    config.output_sample_rate = Some(0);
    assert!(
        render_offline(&config, None)
            .unwrap_err()
            .contains("sample rate")
    );

    config.output_sample_rate = None;
    config.format = OutputFormat::Wav {
        bits_per_sample: 20,
    };
    assert!(
        render_offline(&config, None)
            .unwrap_err()
            .contains("Unsupported WAV depth")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offline_typed_bandsplit_merge_preserves_phase_compensated_audio() {
    let dir = std::env::temp_dir().join(format!(
        "sotf_aud143_offline_bandsplit_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let sample_rate = 48_000;
    let frames = sample_rate as usize;
    create_stereo_tone_test_wav(&input_path, sample_rate, frames);

    let cases: [(usize, &[f64]); 3] = [
        (2, &[4_000.0]),
        (3, &[900.0, 4_000.0]),
        (4, &[900.0, 3_000.0, 6_500.0]),
    ];
    for slope in ["LR24", "LR48"] {
        for (bands, cutoffs_hz) in cases {
            let output_path = dir.join(format!("output-{slope}-{bands}.wav"));
            let mut config = OfflineRenderConfig::new(
                AudioSource::File(input_path.clone()),
                output_path.clone(),
            );
            config.frame_size = 512;
            config.plugins = vec![
                typed_band_split_config(Some(cutoffs_hz.to_vec()), bands, slope, true),
                typed_band_merge_config(bands),
            ];
            render_offline(&config, None).unwrap();

            let reader = hound::WavReader::open(&output_path).unwrap();
            assert_eq!(
                reader.spec().sample_rate,
                sample_rate,
                "{slope}, {bands} bands"
            );
            assert_eq!(reader.spec().channels, 2, "{slope}, {bands} bands");
            let output: Vec<f32> = reader.into_samples::<f32>().map(Result::unwrap).collect();
            assert_eq!(output.len(), frames * 2, "{slope}, {bands} bands");
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "{slope}, {bands} bands"
            );

            // Compare the complete settled output against an independently
            // derived bilinear LR24/LR48 all-pass product, not a second render
            // of the same implementation. Startup recursion is excluded.
            let responses = [
                independent_split_merge_response(slope, cutoffs_hz, 2_000.0, sample_rate),
                independent_split_merge_response(slope, cutoffs_hz, 7_000.0, sample_rate),
            ];
            for (channel, frequency) in [2_000.0, 7_000.0].into_iter().enumerate() {
                let response = responses[channel];
                let magnitude = response.0.hypot(response.1);
                assert!(
                    (magnitude - 1.0).abs() <= 0.005,
                    "{slope}, {bands} bands, {frequency} Hz independent all-pass magnitude {magnitude}"
                );
            }

            let start_frame = 12_000;
            let mut error_power = 0.0;
            let mut input_power = 0.0;
            let mut max_sample_residual = 0.0_f64;
            for frame in start_frame..frames {
                let time = frame as f64 / f64::from(sample_rate);
                let phases = [
                    std::f64::consts::TAU * 2_000.0 * time,
                    std::f64::consts::TAU * 7_000.0 * time + std::f64::consts::FRAC_PI_2,
                ];
                for channel in 0..2 {
                    let input_amplitude = if channel == 0 { 0.5 } else { 0.3 };
                    let response = responses[channel];
                    let expected = input_amplitude
                        * response.0.hypot(response.1)
                        * (phases[channel] + response.1.atan2(response.0)).sin();
                    let actual = f64::from(output[frame * 2 + channel]);
                    let residual = (actual - expected).abs();
                    error_power += residual * residual;
                    max_sample_residual = max_sample_residual.max(residual);
                    input_power += (input_amplitude * phases[channel].sin()).powi(2);
                }
            }
            let normalized_rms = (error_power / input_power).sqrt();
            assert!(
                normalized_rms <= 0.002,
                "{slope}, {bands} bands settled full-waveform residual {normalized_rms}, max={max_sample_residual}"
            );
            assert!(output.iter().any(|sample| sample.abs() > 0.1));
            let (frames, remainder) = output.as_chunks::<2>();
            assert!(remainder.is_empty());
            assert!(
                frames.iter().any(|frame| (frame[0] - frame[1]).abs() > 0.1),
                "{slope}, {bands} bands collapsed distinct channels"
            );
            println!(
                "AUD143 offline {slope} {bands}-band route: settled waveform normalized RMS={normalized_rms:.8e}, max-absolute={max_sample_residual:.8e}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offline_explicit_empty_bandsplit_does_not_truncate_destination() {
    let dir = std::env::temp_dir().join(format!(
        "sotf_aud143_offline_bandsplit_empty_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.wav");
    let output_path = dir.join("output.wav");
    create_stereo_tone_test_wav(&input_path, 48_000, 1024);
    let original = b"preserve existing offline destination";
    std::fs::write(&output_path, original).unwrap();

    let mut config = OfflineRenderConfig::new(AudioSource::File(input_path), output_path.clone());
    config.plugins = vec![typed_band_split_config(Some(Vec::new()), 2, "LR24", true)];
    let error = render_offline(&config, None).unwrap_err();
    assert!(
        error.contains("At least one crossover frequency is required"),
        "unexpected typed BandSplit factory error: {error}"
    );
    assert_eq!(std::fs::read(&output_path).unwrap(), original);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_render_timeline() {
    use crate::timeline::clip::{Clip, Region};
    use crate::timeline::timeline::Timeline;
    use crate::timeline::track::Track;

    let dir = std::env::temp_dir().join("sotf_test_render_timeline");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.wav");
    let out = dir.join("bounced.wav");
    create_test_wav(&src, 48000, 1, 4800);

    let mut tl = Timeline::new(1, 48000, 1024);
    let mut t = Track::new("T1", 1, 48000);
    t.add_region(Region::new(Clip::from_file(&src, 4800), 0));
    tl.add_track(t);
    tl.build().unwrap();

    let fmt = OutputFormat::Wav {
        bits_per_sample: 32,
    };
    render_timeline(&mut tl, &out, &fmt, None).unwrap();

    let reader = hound::WavReader::open(&out).unwrap();
    assert_eq!(reader.spec().sample_rate, 48000);
    let samples: Vec<f32> = reader.into_samples::<f32>().map(|s| s.unwrap()).collect();
    assert_eq!(samples.len(), 4800);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_render_timeline_compensates_and_drains_master_latency() {
    use crate::engine::build_plugin_host;
    use crate::timeline::clip::{Clip, Region};
    use crate::timeline::timeline::Timeline;
    use crate::timeline::track::Track;

    let dir = std::env::temp_dir().join("sotf_test_render_timeline_latency");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.wav");
    let ir = dir.join("ir.wav");
    let out = dir.join("bounced.wav");
    let frames = 2_048;
    create_impulse_test_wav(&src, frames, &[0, frames - 1]);
    create_impulse_test_wav(&ir, 1, &[0]);

    let plugin = convolution_plugin_config(&ir);
    let mut timeline = Timeline::new(1, 48_000, 127);
    let mut track = Track::new("T1", 1, 48_000);
    track.add_region(Region::new(Clip::from_file(&src, frames as u64), 0));
    timeline.add_track(track);
    timeline.master_chain = build_plugin_host(std::slice::from_ref(&plugin), 48_000, 1)
        .unwrap()
        .0;
    timeline.master_plugin_configs.push(plugin);
    timeline.build().unwrap();

    timeline.transport.play();
    let mut dirty_output = vec![0.0; timeline.frame_size];
    for _ in 0..5 {
        timeline.process(&mut dirty_output).unwrap();
    }

    render_timeline(
        &mut timeline,
        &out,
        &OutputFormat::Wav {
            bits_per_sample: 32,
        },
        None,
    )
    .unwrap();

    let reader = hound::WavReader::open(&out).unwrap();
    assert_eq!(reader.duration(), frames as u32);
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .map(|sample| sample.unwrap())
        .collect();
    let strongest: Vec<(usize, f32)> = samples
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, sample)| sample.abs() > 0.5)
        .collect();
    assert!(
        samples[0] > 0.9,
        "first impulse shifted to {}; strongest samples: {strongest:?}",
        samples[0]
    );
    assert!(
        samples[frames - 1] > 0.9,
        "final impulse was truncated to {}; strongest samples: {strongest:?}",
        samples[frames - 1],
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_render_timeline_aligns_parallel_track_latencies() {
    use crate::engine::build_plugin_host;
    use crate::timeline::clip::{Clip, Region};
    use crate::timeline::timeline::Timeline;
    use crate::timeline::track::Track;

    let dir = std::env::temp_dir().join("sotf_test_render_timeline_pdc");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.wav");
    let ir = dir.join("ir.wav");
    let out = dir.join("bounced.wav");
    let frames = 2_048;
    create_impulse_test_wav(&src, frames, &[0, frames - 1]);
    create_impulse_test_wav(&ir, 1, &[0]);

    let mut direct = Track::new("direct", 1, 48_000);
    direct.add_region(Region::new(Clip::from_file(&src, frames as u64), 0));

    let plugin = convolution_plugin_config(&ir);
    let mut latent = Track::new("latent", 1, 48_000);
    latent.add_region(Region::new(Clip::from_file(&src, frames as u64), 0));
    latent.chain = build_plugin_host(std::slice::from_ref(&plugin), 48_000, 1)
        .unwrap()
        .0;
    latent.plugin_configs.push(plugin);

    let mut timeline = Timeline::new(1, 48_000, 127);
    timeline.add_track(direct);
    timeline.add_track(latent);
    timeline.build().unwrap();

    render_timeline(
        &mut timeline,
        &out,
        &OutputFormat::Wav {
            bits_per_sample: 32,
        },
        None,
    )
    .unwrap();

    let reader = hound::WavReader::open(&out).unwrap();
    assert_eq!(reader.duration(), frames as u32);
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .map(|sample| sample.unwrap())
        .collect();
    assert!(
        (samples[0] - 2.0).abs() < 1e-4,
        "track impulses were not aligned at start: {}",
        samples[0]
    );
    assert!(
        (samples[frames - 1] - 2.0).abs() < 1e-4,
        "track impulses were not aligned at end: {}",
        samples[frames - 1]
    );
    assert!(
        samples[1..frames - 1]
            .iter()
            .all(|sample| sample.abs() < 1e-4),
        "parallel path compensation introduced shifted impulses"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_timeline_restores_loop_range_when_progress_panics() {
    use crate::timeline::clip::{Clip, Region};
    use crate::timeline::timeline::Timeline;
    use crate::timeline::track::Track;

    let dir = std::env::temp_dir().join("sotf_test_render_timeline_panic");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.wav");
    let out = dir.join("bounced.wav");
    create_test_wav(&src, 48000, 1, 4800);

    let mut tl = Timeline::new(1, 48000, 1024);
    let mut t = Track::new("T1", 1, 48000);
    t.add_region(Region::new(Clip::from_file(&src, 4800), 0));
    tl.add_track(t);
    tl.transport.loop_range = Some((128, 256));
    tl.build().unwrap();

    let fmt = OutputFormat::Wav {
        bits_per_sample: 32,
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_timeline(
            &mut tl,
            &out,
            &fmt,
            Some(&mut |_p: &RenderProgress| panic!("progress panic")),
        )
    }));

    assert!(result.is_err());
    assert_eq!(tl.transport.loop_range, Some((128, 256)));
    assert!(!tl.transport.playing);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offline_dynamic_eq_shelf_settings_match_separately_configured_core() {
    use sotf_plugins::{DynEqBandParams, ProcessContext, create_plugin};

    fn band(shape: &str, frequency: f32, gain: f32, shelf_slope: f32) -> DynEqBandParams {
        serde_json::from_value(serde_json::json!({
            "shape": shape,
            "shelf_slope": shelf_slope,
            "frequency": frequency,
            "q": 0.707,
            "gain": gain,
            "band_threshold": -48.0,
            "band_ratio": 4.0,
            "active": true,
            "solo": false,
        }))
        .expect("typed DynamicEQ shelf band deserializes")
    }

    let sample_rate = 48_000_u32;
    let source_frames = 4_099_usize;
    let directory = tempfile::tempdir().expect("create temporary render directory");
    let input_path = directory.path().join("dynamic-eq-input.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&input_path, spec).unwrap();
    let mut input = Vec::with_capacity(source_frames * 2);
    for frame in 0..source_frames {
        let time = frame as f64 / f64::from(sample_rate);
        let tone = |frequency: f64, phase: f64, amplitude: f64| {
            (amplitude * (std::f64::consts::TAU * frequency * time + phase).sin()) as f32
        };
        let left = tone(90.0, 0.0, 0.18) + tone(720.0, 0.3, 0.13) + tone(6_800.0, -0.2, 0.08);
        let right = tone(140.0, 0.2, 0.16) + tone(1_600.0, -0.4, 0.11) + tone(8_100.0, 0.5, 0.07);
        input.extend([left, right]);
        writer.write_sample(left).unwrap();
        writer.write_sample(right).unwrap();
    }
    writer.finalize().unwrap();

    let reference_parameters = serde_json::json!({
        "num_bands": 2,
        "threshold": -48.0,
        "ratio": 4.0,
        "attack_ms": 5.0,
        "release_ms": 50.0,
        "knee": 3.0,
        "link_channels": true,
        "mix": 1.0,
        "bands": [
            {
                "frequency": 250.0,
                "q": 0.707,
                "gain": 8.0,
                "band_threshold": -48.0,
                "band_ratio": 4.0,
                "active": true,
                "solo": false,
                "shape": "low_shelf",
                "shelf_slope": 0.7
            },
            {
                "frequency": 6_000.0,
                "q": 0.707,
                "gain": -7.0,
                "band_threshold": -48.0,
                "band_ratio": 4.0,
                "active": true,
                "solo": false,
                "shape": "high_shelf",
                "shelf_slope": 0.8
            }
        ]
    });

    let settings = PluginSettings::DynamicEq {
        num_bands: 2.0,
        threshold: -48.0,
        ratio: 4.0,
        attack: 5.0,
        release: 50.0,
        knee: 3.0,
        link_channels: true,
        mix: 1.0,
        bands: vec![
            band("low_shelf", 250.0, 8.0, 0.7),
            band("high_shelf", 6_000.0, -7.0, 0.8),
        ],
        stereo_pairs: None,
    };
    let plugin_config = settings.to_plugin_config(f64::from(sample_rate));
    assert_eq!(plugin_config.plugin_type, "dynamic_eq");
    assert_eq!(plugin_config.parameters["bands"][0]["shape"], "low_shelf");
    assert_eq!(plugin_config.parameters["bands"][1]["shape"], "high_shelf");
    assert_eq!(
        plugin_config.parameters["bands"][0]["shelf_slope"]
            .as_f64()
            .unwrap() as f32,
        0.7_f32
    );
    assert_eq!(
        plugin_config.parameters["bands"][1]["shelf_slope"]
            .as_f64()
            .unwrap() as f32,
        0.8_f32
    );

    let mut first_render: Option<Vec<f32>> = None;
    for frame_size in [127, 257, 1_024] {
        let output_path = directory
            .path()
            .join(format!("dynamic-eq-output-{frame_size}.wav"));
        let mut config =
            OfflineRenderConfig::new(AudioSource::File(input_path.clone()), output_path.clone());
        config.plugins = vec![plugin_config.clone()];
        config.frame_size = frame_size;
        render_offline(&config, None).expect("engine offline render succeeds");

        let rendered_reader = hound::WavReader::open(&output_path).unwrap();
        assert_eq!(rendered_reader.spec().channels, 2);
        assert_eq!(rendered_reader.spec().sample_rate, sample_rate);
        let rendered = rendered_reader
            .into_samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        // `render_offline` adds no post-program tail; its default export is
        // exactly the source duration after compensating chain latency.
        assert_eq!(rendered.len(), source_frames * 2);
        assert!(rendered.iter().all(|sample| sample.is_finite()));

        let mut reference = create_plugin("dynamic_eq", &reference_parameters, 2, sample_rate)
            .expect("independent explicit shelf config constructs the accepted core");
        reference.initialize(sample_rate).unwrap();
        let mut expected = vec![f32::NAN; input.len()];
        let mut start_frame = 0;
        while start_frame < source_frames {
            let block_frames = (source_frames - start_frame).min(frame_size);
            let sample_start = start_frame * 2;
            let sample_end = sample_start + block_frames * 2;
            let context = ProcessContext::new(sample_rate, block_frames);
            assert_eq!(
                reference
                    .process(
                        &input[sample_start..sample_end],
                        &mut expected[sample_start..sample_end],
                        &context,
                    )
                    .expect("reference core processes the same block"),
                block_frames
            );
            start_frame += block_frames;
        }
        assert!(expected.iter().all(|sample| sample.is_finite()));

        let max_error = rendered
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0_f32, f32::max);
        let rms_error = rendered
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| f64::from(actual - expected).powi(2))
            .sum::<f64>()
            / rendered.len() as f64;
        let rms_error = rms_error.sqrt();
        assert!(max_error <= 2.0e-6, "max sample error {max_error}");
        assert!(rms_error <= 2.0e-7, "RMS sample error {rms_error}");

        if let Some(baseline) = &first_render {
            let partition_error = rendered
                .iter()
                .zip(baseline)
                .map(|(actual, baseline)| f64::from(actual - baseline).powi(2))
                .sum::<f64>()
                / rendered.len() as f64;
            let partition_error = partition_error.sqrt();
            assert!(
                partition_error <= 2.0e-7,
                "frame_size {frame_size} partition RMS {partition_error}"
            );
        } else {
            first_render = Some(rendered);
        }
    }
}
