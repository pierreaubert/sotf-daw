//! Optional DeepFilterNet2 host framing, actual serving, failure and heap checks.
#![cfg(feature = "deep-filter")]

use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_speech_denoiser::{
    DeepFilterConfig, DeepFilterData, SpeechDenoiserPlugin, SpeechDenoiserPluginParams,
};
use std::path::PathBuf;
use std::time::Duration;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn config(playout_packets: usize) -> DeepFilterConfig {
    DeepFilterConfig {
        model_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/deepfilternet2/DeepFilterNet2_onnx.tar.gz")
            .to_string_lossy()
            .into_owned(),
        sha256: "412e81d8d1779b8f925ac4b53472655a9f866c4af73dbf930ab587359f631628".into(),
        playout_packets,
    }
}

fn prepared(rate: f64, strength: f32, config: DeepFilterConfig) -> SpeechDenoiserPlugin {
    prepared_channels(rate, 1, strength, config)
}

fn prepared_channels(
    rate: f64,
    channels: usize,
    strength: f32,
    config: DeepFilterConfig,
) -> SpeechDenoiserPlugin {
    let mut plugin = SpeechDenoiserPlugin::try_from_params(
        channels,
        SpeechDenoiserPluginParams {
            deep_filter: Some(config),
            strength,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn signal(n: usize, rate: f64) -> f32 {
    let phase = std::f64::consts::TAU * n as f64 / rate;
    (0.2 * (193.0 * phase).sin() + 0.07 * (386.0 * phase).sin() + 0.025 * (7171.0 * phase).sin())
        as f32
}

fn process(plugin: &mut SpeechDenoiserPlugin, source: &[f32], rate: f64) -> Vec<f32> {
    let mut audio = source.to_vec();
    let channels = plugin.channels();
    for block in audio.chunks_mut(137 * channels) {
        let frames = block.len() / channels;
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                plugin
                    .process_in_place(block, &ProcessContext::new(rate, frames))
                    .unwrap(),
                frames
            );
        });
        assert_eq!(
            heap,
            (0, 0),
            "valid audio may not allocate or free model state"
        );
    }
    audio
}

#[test]
fn dry_endpoint_retains_host_bandwidth_latency_and_finite_stream_contract() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        let mut plugin = prepared(rate, 0.0, config(4));
        let latency = plugin.latency_samples();
        if rate == 48_000.0 {
            assert_eq!(latency, 4320);
        }
        let source: Vec<f32> = (0..3001).map(|n| signal(n, rate)).collect();
        let mut rendered = process(&mut plugin, &source, rate);
        let mut calls = plugin.drain_call_bound().unwrap().get();
        loop {
            assert!(calls > 0);
            calls -= 1;
            let mut buffer = vec![0.0; plugin.drain_output_frames_max()];
            let context = ProcessContext::new(rate, buffer.len());
            let result = plugin.drain(&mut buffer, &context).unwrap();
            rendered.extend_from_slice(&buffer[..result.frames]);
            if result.complete {
                break;
            }
        }
        let mut expected = vec![0.0; rendered.len()];
        expected[latency..latency + source.len()].copy_from_slice(&source);
        assert_eq!(rendered, expected);
        assert!(
            plugin
                .process_in_place(&mut [0.0], &ProcessContext::new(rate, 1))
                .is_err()
        );
        assert_eq!(
            sotf_host::test_utils::measure_heap_activity(|| plugin.reset()),
            (0, 0)
        );
    }
}

#[test]
fn actual_model_packets_reach_audio_and_publish_local_snr_without_inventing_vad() {
    // A larger explicit worker budget isolates functional serving from a
    // capacity claim about the default budget or a contended DAW workload.
    let mut plugin = prepared(48_000.0, 1.0, config(16));
    let latency = plugin.latency_samples();
    assert_eq!(latency, 10080);
    let mut output = Vec::new();
    let source: Vec<f32> = (0..48_000).map(|n| signal(n, 48_000.0)).collect();
    for packet in source.as_chunks::<480>().0 {
        let mut audio = packet.to_vec();
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            plugin
                .process_in_place(&mut audio, &ProcessContext::new(48_000, 480))
                .unwrap();
        });
        assert_eq!(heap, (0, 0));
        output.extend(audio);
        std::thread::sleep(Duration::from_millis(12));
    }
    let data = plugin
        .get_data()
        .unwrap()
        .downcast::<DeepFilterData>()
        .unwrap();
    assert!(
        data.timely_packets > 0,
        "the real model must be served rather than permanent fallback"
    );
    assert!(data.last_local_snr_db.unwrap().is_finite());
    assert_eq!(data.lifetime_worker_errors, 0);
    let changed = output[latency..]
        .iter()
        .zip(&source)
        .map(|(a, b)| f64::from(a - b).powi(2))
        .sum::<f64>();
    println!(
        "DeepFilter model packets={} fallback={} warmup={} dropped={} changed_energy={changed}",
        data.timely_packets, data.fallback_packets, data.warmup_packets, data.dropped_inputs
    );
    assert!(changed > 1e-6, "serving must produce real enhanced audio");
    assert!(
        plugin
            .get_data()
            .unwrap()
            .downcast::<sotf_plugin_speech_denoiser::SpeechDenoiserData>()
            .is_err()
    );
}

#[test]
fn failed_model_reprepare_preserves_nonzero_delayed_source_history() {
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let original = config(4);
    let path = std::env::temp_dir().join(format!(
        "sotf-deepfilter-{}-{}.tar.gz",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _cleanup = Cleanup(path.clone());
    std::fs::copy(&original.model_path, &path).unwrap();
    let mut config = original.clone();
    config.model_path = path.to_string_lossy().into_owned();
    let mut plugin = prepared(48_000.0, 0.0, config);
    let latency = plugin.latency_samples();
    let prefix: Vec<f32> = (0..337).map(|n| signal(n, 48_000.0)).collect();
    let mut rendered = process(&mut plugin, &prefix, 48_000.0);
    std::fs::write(&path, b"wrong model bytes").unwrap();
    assert!(plugin.initialize(48_000.0).is_err());
    assert_eq!(plugin.latency_samples(), latency);
    rendered.extend(process(&mut plugin, &vec![0.0; latency + 337], 48_000.0));
    let mut expected = vec![0.0; rendered.len()];
    expected[latency..latency + prefix.len()].copy_from_slice(&prefix);
    assert_eq!(rendered, expected);
}

#[test]
fn structural_mode_preserves_legacy_choices_and_rejects_wider_layouts() {
    assert_eq!(sotf_plugin_speech_denoiser::model::MODEL_LABELS.len(), 3);
    let old: SpeechDenoiserPluginParams = serde_json::from_str("{}").unwrap();
    assert!(old.deep_filter.is_none());
    let serialized = serde_json::to_value(old).unwrap();
    assert!(serialized.get("deep_filter").is_none());
    assert!(
        SpeechDenoiserPlugin::try_from_params(
            2,
            SpeechDenoiserPluginParams {
                deep_filter: Some(config(4)),
                ..Default::default()
            }
        )
        .is_ok()
    );
    assert!(
        SpeechDenoiserPlugin::try_from_params(
            3,
            SpeechDenoiserPluginParams {
                deep_filter: Some(config(4)),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn linked_stereo_dry_path_has_exact_delay_drain_and_zero_heap_activity() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        let mut plugin = prepared_channels(rate, 2, 0.0, config(4));
        let latency = plugin.latency_samples();
        let source: Vec<f32> = (0..3001)
            .flat_map(|n| [signal(n, rate), -0.25 * signal(n + 37, rate)])
            .collect();
        let mut rendered = process(&mut plugin, &source, rate);
        let mut calls = plugin.drain_call_bound().unwrap().get();
        loop {
            assert!(calls > 0);
            calls -= 1;
            let frames = plugin.drain_output_frames_max();
            let mut buffer = vec![0.0; frames * 2];
            let result = plugin
                .drain(&mut buffer, &ProcessContext::new(rate, frames))
                .unwrap();
            rendered.extend_from_slice(&buffer[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        let mut expected = vec![0.0; rendered.len()];
        expected[latency * 2..latency * 2 + source.len()].copy_from_slice(&source);
        assert_eq!(rendered, expected);
        assert_eq!(
            sotf_host::test_utils::measure_heap_activity(|| plugin.reset()),
            (0, 0)
        );
    }
}

#[test]
fn linked_stereo_real_packets_preserve_antiphase_image_without_callback_allocations() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        let mut plugin = prepared_channels(rate, 2, 1.0, config(16));
        let latency = plugin.latency_samples();
        let frames = (rate * 0.01).round() as usize;
        let mut max_image_error = 0f32;
        let mut output_energy = 0f64;
        let mut changed_energy = 0f64;
        for packet in 0..100 {
            let mut audio: Vec<f32> = (packet * frames..(packet + 1) * frames)
                .flat_map(|n| {
                    let left = signal(n, rate);
                    [left, -0.25 * left]
                })
                .collect();
            assert_eq!(
                sotf_host::test_utils::measure_heap_activity(|| {
                    plugin
                        .process_in_place(&mut audio, &ProcessContext::new(rate, frames))
                        .unwrap();
                }),
                (0, 0)
            );
            for (i, &[left, right]) in audio.as_chunks::<2>().0.iter().enumerate() {
                max_image_error = max_image_error.max((right + 0.25 * left).abs());
                output_energy += f64::from(left).powi(2);
                let n = packet * frames + i;
                if n >= latency {
                    changed_energy += f64::from(left - signal(n - latency, rate)).powi(2);
                }
            }
            std::thread::sleep(Duration::from_millis(12));
        }
        let data = plugin
            .get_data()
            .unwrap()
            .downcast::<DeepFilterData>()
            .unwrap();
        println!(
            "Linked stereo rate={rate} timely={} fallback={} dropped={} image_error={max_image_error} changed_energy={changed_energy}",
            data.timely_packets, data.fallback_packets, data.dropped_inputs
        );
        assert!(data.timely_packets > 0);
        assert_eq!(data.lifetime_worker_errors, 0);
        assert!(output_energy > 1e-6);
        assert!(changed_energy > 1e-6);
        assert!(max_image_error < 1e-6);
    }
}
