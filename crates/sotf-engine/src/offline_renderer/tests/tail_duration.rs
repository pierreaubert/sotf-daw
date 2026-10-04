//! Explicit export tails have a duration contract even for recursive effects.

use super::*;
use crate::offline_renderer::render_offline_with_tail;
use std::time::Duration;

fn read_float(path: &Path) -> (hound::WavSpec, Vec<f32>) {
    let reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    let samples = reader.into_samples::<f32>().map(Result::unwrap).collect();
    (spec, samples)
}

#[test]
fn explicit_tail_renders_analytic_feedback_echoes_and_preserves_zero_default() {
    let dir = std::env::temp_dir().join("sotf_offline_explicit_tail_echoes");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.wav");
    create_impulse_test_wav(&input, 73, &[72]);
    let mut config = OfflineRenderConfig::new(AudioSource::File(input), dir.join("output.wav"));
    config.plugins.push(PluginConfig {
        plugin_type: "delay".into(),
        parameters: serde_json::json!({"delay_ms":3.0,"mix":1.0,"feedback":0.5}),
    });
    for block in [1, 127, 1024] {
        config.frame_size = block;
        render_offline(&config, None).unwrap();
        let (_, original) = read_float(&config.output_path);
        assert_eq!(original.len(), 73);
        assert!(original.iter().all(|&x| x == 0.0));
        render_offline_with_tail(&config, Duration::ZERO, None).unwrap();
        assert_eq!(read_float(&config.output_path).1, original);
        let mut progress = Vec::new();
        render_offline_with_tail(
            &config,
            Duration::from_millis(30),
            Some(&mut |value: &RenderProgress| {
                progress.push((value.frames_processed, value.total_frames));
            }),
        )
        .unwrap();
        let (spec, samples) = read_float(&config.output_path);
        assert_eq!(spec.sample_rate, 48000);
        assert_eq!(samples.len(), 73 + 1440);
        assert_eq!(progress.last(), Some(&(1513, Some(1513))));
        assert!(progress.windows(2).all(|p| p[0].0 <= p[1].0));
        for (frame, &value) in samples.iter().enumerate() {
            let expected = if frame > 72 && (frame - 72).is_multiple_of(144) {
                0.5f32.powi(((frame - 72) / 144 - 1) as i32)
            } else {
                0.0
            };
            assert!(
                (value - expected).abs() < 2e-6,
                "block{block}, frame{frame}: {value} vs {expected}"
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn explicit_tail_uses_export_clock_through_source_and_chain_resampling() {
    let dir = std::env::temp_dir().join("sotf_offline_tail_export_clock");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.wav");
    create_test_wav(&input, 44100, 2, 997);
    let mut config = OfflineRenderConfig::new(AudioSource::File(input), dir.join("output.wav"));
    config.output_sample_rate = Some(48000);
    config.plugins.push(PluginConfig {
        plugin_type: "resampler".into(),
        parameters: serde_json::json!({"input_sample_rate":48000,"output_sample_rate":96000,"chunk_size":64}),
    });
    let (host, _) = crate::engine::build_plugin_host(&config.plugins, 48000, 2).unwrap();
    assert_eq!(host.output_sample_rate(48000).unwrap(), 96000);
    for block in [127, 1024] {
        config.frame_size = block;
        let mut final_progress = None;
        render_offline_with_tail(
            &config,
            Duration::from_nanos(10_000_001),
            Some(&mut |p: &RenderProgress| {
                final_progress = Some((p.frames_processed, p.total_frames));
            }),
        )
        .unwrap();
        let (spec, samples) = read_float(&config.output_path);
        let expected = (997usize * 48000).div_ceil(44100) + 481;
        assert_eq!(spec.sample_rate, 48000);
        assert_eq!(spec.channels, 2);
        assert_eq!(samples.len(), expected * 2);
        assert_eq!(
            final_progress,
            Some((expected as u64, Some(expected as u64)))
        );
        assert!(samples.iter().all(|x| x.is_finite()));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unrepresentable_tail_does_not_truncate_existing_output() {
    let dir = std::env::temp_dir().join("sotf_offline_tail_preflight");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.wav");
    create_constant_test_wav(&input, 48000, 1, 1, 0.25);
    let output = dir.join("output.wav");
    std::fs::write(&output, b"existing output must survive preflight").unwrap();
    let config = OfflineRenderConfig::new(AudioSource::File(input), &output);
    let error = render_offline_with_tail(&config, Duration::MAX, None).unwrap_err();
    assert!(error.contains("tail duration"), "{error}");
    assert_eq!(
        std::fs::read(output).unwrap(),
        b"existing output must survive preflight"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tail_preserves_source_converter_response_like_explicit_source_padding() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.wav");
    let padded = dir.path().join("padded.wav");
    for (path, frames) in [(&original, 441), (&padded, 882)] {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 44100,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for frame in 0..frames {
            writer
                .write_sample(if frame == 440 { 1.0f32 } else { 0.0 })
                .unwrap();
        }
        writer.finalize().unwrap();
    }
    for block in [127, 1024] {
        let mut candidate = OfflineRenderConfig::new(
            AudioSource::File(original.clone()),
            dir.path().join("candidate.wav"),
        );
        candidate.output_sample_rate = Some(48000);
        candidate.frame_size = block;
        let mut reference = candidate.clone();
        reference.source = AudioSource::File(padded.clone());
        reference.output_path = dir.path().join("reference.wav");
        render_offline_with_tail(&candidate, Duration::from_millis(10), None).unwrap();
        render_offline(&reference, None).unwrap();
        let actual = read_float(&candidate.output_path).1;
        let expected = read_float(&reference.output_path).1;
        assert_eq!(actual.len(), 960);
        assert_eq!(actual.len(), expected.len());
        for (index, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                (actual - expected).abs() < 2e-6,
                "block{block}, sample{index}: {actual} vs {expected}"
            );
        }
    }
}
