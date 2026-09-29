//! Export endpoints preserve converter history and allow prepared chunks to finish.

// Rust guideline compliant 2026-02-21

use super::{
    AudioSource, OfflineRenderConfig, PluginConfig, create_constant_test_wav, render_offline,
};
use crate::offline_renderer::render_offline_with_tail;
use std::path::Path;
use std::time::Duration;

fn final_impulse(path: &Path, frames: usize) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
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

fn read_float(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn converter_response_survives_downstream_fir_until_the_compensated_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    let padded = dir.path().join("padded.wav");
    final_impulse(&source, 441);
    // Far beyond the converter and FIR EQ support: cropping this render cannot
    // share an early source-converter cutoff at the candidate's endpoint.
    final_impulse(&padded, 4410);
    let mut candidate =
        OfflineRenderConfig::new(AudioSource::File(source), dir.path().join("candidate.wav"));
    candidate.output_sample_rate = Some(48_000);
    candidate.frame_size = 127;
    candidate.plugins.push(PluginConfig {
        plugin_type: "linear_phase_eq".into(),
        parameters: serde_json::json!({
            "num_filters": 1,
            "fir_length_index": 0,
            "phase_mode_index": 0,
            "auto_gain": false,
            "mix": 1.0,
            "filters": [{
                "filter_type": "Peak", "frequency": 1300.0,
                "q": 0.8, "gain_db": 18.0, "active": true
            }]
        }),
    });
    let mut reference = candidate.clone();
    reference.source = AudioSource::File(padded);
    reference.output_path = dir.path().join("reference.wav");
    render_offline(&reference, None).unwrap();
    let expected = read_float(&reference.output_path);
    assert_eq!(expected.len(), 4800);
    assert!(expected.iter().any(|sample| sample.abs() > 0.1));

    for tail in [Duration::ZERO, Duration::from_millis(1)] {
        render_offline_with_tail(&candidate, tail, None).unwrap();
        let actual = read_float(&candidate.output_path);
        let frames = 480 + if tail.is_zero() { 0 } else { 48 };
        assert_eq!(actual.len(), frames);
        let (index, error) = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .map(|(index, (&actual, &expected))| (index, (actual - expected).abs()))
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap();
        assert!(
            error < 2e-6,
            "tail {tail:?}, sample {index}: {} vs {} (error {error})",
            actual[index],
            expected[index]
        );
    }
}

#[test]
fn endpoint_completion_allows_large_resampler_chunks_with_single_frame_callbacks() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    create_constant_test_wav(&source, 48_000, 1, 1, 0.25);
    let mut config =
        OfflineRenderConfig::new(AudioSource::File(source), dir.path().join("output.wav"));
    config.frame_size = 1;
    config.plugins.push(PluginConfig {
        plugin_type: "resampler".into(),
        parameters: serde_json::json!({
            "input_sample_rate": 48000, "output_sample_rate": 96000,
            "chunk_size": 4096
        }),
    });
    render_offline_with_tail(&config, Duration::from_millis(1), None).unwrap();
    let actual = read_float(&config.output_path);
    assert_eq!(actual.len(), 49);
    assert!(actual.iter().all(|sample| sample.is_finite()));
    assert!(actual.iter().any(|sample| sample.abs() > 0.01));
}
