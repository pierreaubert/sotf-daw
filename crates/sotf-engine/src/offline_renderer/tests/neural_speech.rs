//! Offline renderer integration with actual speech-model inference.

use super::{AudioSource, OfflineRenderConfig, PluginConfig, render_offline};
use sotf_plugins::plugin_gate::{GatePlugin, GatePluginParams, SpeechProtectionConfig};
use sotf_plugins::{ParametricInPlacePlugin, ProcessingMode};

fn assert_same_audio(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "rendered sample count");
    let mismatch = actual
        .iter()
        .zip(expected)
        .enumerate()
        .find(|(_, (a, b))| a.to_bits() != b.to_bits())
        .map(|(index, (&actual, &expected))| (index, actual, expected));
    assert!(
        mismatch.is_none(),
        "first bitwise audio mismatch: {mismatch:?}"
    );
}

#[test]
fn offline_speech_matches_synchronous_reference_across_render_block_sizes() {
    let dir = std::env::temp_dir().join(format!("sotf_offline_speech_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("speech.wav");
    let bytes = include_bytes!(
        "../../../../sotf-plugins/crates/sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm"
    );
    let mut input: Vec<f32> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|x| {
            let value = f32::from(i16::from_le_bytes([x[0], x[1]])) / 32768.0;
            [value, -0.5 * value]
        })
        .collect();
    input.extend_from_slice(&[0.001; 26]);
    let mut writer = hound::WavWriter::create(
        &input_path,
        hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for &value in &input {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();

    #[cfg(feature = "silero-speech")]
    let models = [false, true];
    #[cfg(not(feature = "silero-speech"))]
    let models = [false];
    for silero in models {
        let mut policy = SpeechProtectionConfig::default();
        if silero {
            policy.model = sotf_plugins::plugin_gate::SpeechProtectionModel::Silero {
                model_path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../sotf-plugins/models/silero-vad/silero_vad_16k_batch1.onnx"),
                sha256: sotf_plugins::plugin_gate::SILERO_MODEL_SHA256.to_owned(),
            };
        }
        let settings = GatePluginParams {
            speech_protection: Some(policy),
            threshold_db: -10.0,
            attack_ms: 1.0,
            release_ms: 10.0,
            hold_ms: 0.0,
            range_db: 40.0,
            ..GatePluginParams::default()
        };
        let mut gate = GatePlugin::try_from_params(2, settings.clone()).unwrap();
        gate.initialize_with_mode(48_000.0, ProcessingMode::Offline)
            .unwrap();
        let mut expected = input.clone();
        for block in expected.chunks_mut(480 * 2) {
            let frames = block.len() / 2;
            gate.process_in_place(block, &sotf_plugins::ProcessContext::new(48_000, frames))
                .unwrap();
        }
        let mut classical_settings = settings.clone();
        classical_settings.speech_protection = None;
        let mut classical = GatePlugin::try_from_params(2, classical_settings).unwrap();
        classical.initialize(48_000.0).unwrap();
        let mut baseline = input.clone();
        classical
            .process_in_place(
                &mut baseline,
                &sotf_plugins::ProcessContext::new(48_000, input.len() / 2),
            )
            .unwrap();
        assert!(
            expected
                .iter()
                .zip(&baseline)
                .any(|(a, b)| (a - b).abs() > 1e-5),
            "reference must contain actual speech protection"
        );
        for frame_size in [127, 1024, 3001] {
            let output_path = dir.join(format!("output_{silero}_{frame_size}.wav"));
            let mut config =
                OfflineRenderConfig::new(AudioSource::File(input_path.clone()), &output_path);
            config.frame_size = frame_size;
            config.plugins = vec![PluginConfig {
                plugin_type: "gate".into(),
                parameters: serde_json::to_value(&settings).unwrap(),
            }];
            render_offline(&config, None).unwrap();
            let actual: Vec<f32> = hound::WavReader::open(&output_path)
                .unwrap()
                .into_samples::<f32>()
                .map(Result::unwrap)
                .collect();
            assert_same_audio(&actual, &expected);
            if frame_size == 3001 {
                config.output_path = dir.join(format!("parallel_{silero}.wav"));
                crate::offline_renderer::render_offline_parallel(&config, 3, None).unwrap();
                let parallel: Vec<f32> = hound::WavReader::open(&config.output_path)
                    .unwrap()
                    .into_samples::<f32>()
                    .map(Result::unwrap)
                    .collect();
                assert_same_audio(&parallel, &expected);
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
