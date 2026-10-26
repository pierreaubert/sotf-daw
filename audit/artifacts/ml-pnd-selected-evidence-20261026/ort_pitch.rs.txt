//! Optional actual-library preprocessing, worker and lifecycle qualification.
#![cfg(feature = "neural-pitch-ort")]

// Rust guideline compliant 2026-02-21
use sha2::{Digest, Sha256};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_pnd::neural_pitch::{CrepeModel, NeuralPitchAssistant};
use sotf_plugin_pnd::ort_pitch::{OrtPitchConfig, OrtPitchProvider};
use sotf_plugin_pnd::{NeuralPitchConfig, PndPlugin, PndPluginParams};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn config() -> NeuralPitchConfig {
    NeuralPitchConfig {
        model_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/crepe-tiny/crepe-tiny.onnx")
            .to_string_lossy()
            .into_owned(),
        sha256: "57209bf565bce9de445b31a175294dd2f297432864599c6aa237a895353918d3".into(),
        min_confidence: 0.8,
        max_age_ms: 150.0,
        max_deviation_cents: 100.0,
        source_channel: 0,
    }
}

fn runtime(provider: OrtPitchProvider) -> OrtPitchConfig {
    OrtPitchConfig {
        library_path: std::env::var_os("SOTF_ORT_DYLIB")
            .expect("explicit SOTF_ORT_DYLIB required")
            .into(),
        library_sha256: std::env::var("SOTF_ORT_SHA256")
            .expect("explicit SOTF_ORT_SHA256 required"),
        provider,
    }
}

fn tone(rate: f64, hz: f64, weak: bool, scale: f64) -> Vec<f32> {
    (0..(1024.0 * rate / 16000.0).ceil() as usize)
        .map(|i| {
            let p = std::f64::consts::TAU * hz * i as f64 / rate;
            (scale
                * if weak {
                    0.01 * p.sin() + 0.6 * (2.0 * p).sin() + 0.3 * (3.0 * p).sin()
                } else {
                    p.sin()
                }) as f32
        })
        .collect()
}

#[test]
fn invalid_resources_and_policy_fail_without_loading_a_runtime() {
    let absent = OrtPitchConfig {
        library_path: PathBuf::from("/sotf-missing-runtime/no-runtime.dylib"),
        library_sha256: "0".repeat(64),
        provider: OrtPitchProvider::Cpu,
    };
    assert!(CrepeModel::load_ort(&config(), 48000.0, &absent).is_err());
    assert!(CrepeModel::load_ort(&config(), 8000.0, &absent).is_err());
    let mut invalid = config();
    invalid.min_confidence = f32::NAN;
    assert!(NeuralPitchAssistant::new_ort(invalid, 48000.0, &absent).is_err());
    let mut plugin = PndPlugin::new(1);
    assert!(plugin.configure_qa_ort_pitch(absent).is_err());
    let mut unsupported = serde_json::json!({"library_path":"/runtime.dylib","library_sha256":"0".repeat(64),"provider":"coreml_gpu_allowed"});
    assert!(serde_json::from_value::<OrtPitchConfig>(unsupported.clone()).is_err());
    unsupported["provider"] = serde_json::json!("cpu");
    unsupported["implicit_download"] = serde_json::json!(true);
    assert!(serde_json::from_value::<OrtPitchConfig>(unsupported).is_err());
}

#[test]
#[ignore = "requires explicit SOTF_ORT_DYLIB and SOTF_ORT_SHA256; runs actual library"]
fn actual_runtime_parity_workers_fallback_and_transactional_lifecycle() {
    let mut providers = vec![OrtPitchProvider::Cpu];
    if cfg!(target_vendor = "apple") {
        providers.push(OrtPitchProvider::CoreMlCpuOnly);
    }
    let mut records = Vec::new();
    let mut maximum_confidence_error = 0.0_f32;
    let mut maximum_pitch_error = 0.0_f64;
    let mut maximum_activation_error = 0.0_f32;
    let mut placements = Vec::new();
    let first = runtime(OrtPitchProvider::Cpu);
    let mut bad = first.clone();
    bad.library_sha256 = "0".repeat(64);
    assert!(CrepeModel::load_ort(&config(), 48000.0, &bad).is_err());
    for provider in providers {
        let runtime = runtime(provider);
        let directory = std::env::var_os("SOTF_ORT_QA_REPORT")
            .and_then(|p| PathBuf::from(p).parent().map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        let prefix = directory.join(format!("native-ort-{}-{provider:?}", std::process::id()));
        let profile =
            sotf_plugin_pnd::ort_pitch::profile_execution(&config(), &runtime, &prefix).unwrap();
        let raw = std::fs::read(&profile).unwrap();
        let events: Vec<serde_json::Value> = serde_json::from_slice(&raw).unwrap();
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for event in events {
            if event["cat"] == "Node"
                && let Some(name) = event["args"]["provider"].as_str()
            {
                *counts.entry(name.into()).or_default() += 1;
            }
        }
        let expected = match provider {
            OrtPitchProvider::Cpu => "CPUExecutionProvider",
            OrtPitchProvider::CoreMlCpuOnly => "CoreMLExecutionProvider",
        };
        assert!(counts.get(expected).is_some_and(|n| *n > 0));
        let profile_sha256: String = Sha256::digest(&raw)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        placements.push(serde_json::json!({"provider":provider,"path":profile,"sha256":profile_sha256,"node_kernel_events_by_provider":counts}));
        let corpus_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../audit/artifacts/ml-crepe-runtime-20261026/corpus.f32le");
        let bytes = std::fs::read(corpus_path).unwrap();
        assert_eq!(bytes.len(), 32 * 1024 * 4);
        let mut reference = CrepeModel::load(&config(), 16000.0).unwrap();
        let mut candidate = CrepeModel::load_ort(&config(), 16000.0, &runtime).unwrap();
        for frame in bytes.as_chunks::<4096>().0 {
            let x = std::array::from_fn(|i| {
                f32::from_le_bytes(frame[i * 4..i * 4 + 4].try_into().unwrap())
            });
            let a = reference.activations(&x).unwrap();
            let b = candidate.activations(&x).unwrap();
            for (a, b) in a.iter().zip(b) {
                maximum_activation_error = maximum_activation_error.max((*a - b).abs());
            }
        }
        assert!(maximum_activation_error <= 2e-5);
        for rate in [16000.0, 44100.0, 48000.0, 96000.0, 192000.0] {
            let mut tract = CrepeModel::load(&config(), rate).unwrap();
            let mut alternative = CrepeModel::load_ort(&config(), rate, &runtime).unwrap();
            assert_eq!(tract.backend_name(), "tract");
            assert_eq!(
                alternative.backend_name(),
                match provider {
                    OrtPitchProvider::Cpu => "ort_cpu",
                    OrtPitchProvider::CoreMlCpuOnly => "ort_coreml_cpu_only",
                }
            );
            for hz in [82.41, 220.0, 440.0, 880.0, 1318.51] {
                for weak in [false, true] {
                    let input = tone(rate, hz, weak, 0.2);
                    let a = tract.analyze_window(&input, input.len() as u64).unwrap();
                    let b = alternative
                        .analyze_window(&input, input.len() as u64)
                        .unwrap();
                    let cents = (1200.0
                        * (f64::from(a.frequency_hz) / f64::from(b.frequency_hz)).log2())
                    .abs();
                    let confidence = (a.confidence - b.confidence).abs();
                    maximum_pitch_error = maximum_pitch_error.max(cents);
                    maximum_confidence_error = maximum_confidence_error.max(confidence);
                    assert!(cents <= 0.1 && confidence <= 2e-5);
                    records.push(serde_json::json!({"provider":provider,"rate":rate,"hz":hz,"weak":weak,"pitch_difference_cents":cents,"confidence_difference":confidence}));
                }
            }
            let silent = tone(rate, 440.0, false, 1e-5);
            assert_eq!(
                alternative
                    .analyze_window(&silent, silent.len() as u64)
                    .unwrap()
                    .confidence,
                0.0
            );
        }
        let mut assistant = NeuralPitchAssistant::new_ort(config(), 48000.0, &runtime).unwrap();
        for x in tone(48000.0, 440.0, false, 0.2) {
            assistant.push(x);
        }
        let wait_start = Instant::now();
        while assistant.estimate_against_reference(440.0).is_none() {
            assert!(wait_start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
        // All non-silent old windows are now older than the source-age policy.
        assert_no_allocs_or_deallocs("ORT source producer", || {
            for _ in 0..7200 {
                assistant.push(0.0);
            }
        });
        assert!(assistant.estimate_against_reference(440.0).is_none());
        assert_no_allocs_or_deallocs("ORT source reset", || assistant.reset());
        assert!(assistant.estimate_against_reference(440.0).is_none());
        std::thread::sleep(Duration::from_millis(20));
        assert!(assistant.estimate_against_reference(440.0).is_none());

        let params = PndPluginParams {
            reference_frequency_hz: 440.0,
            neural_pitch: Some(config()),
            ..PndPluginParams::default()
        };
        let mut plugin = PndPlugin::from_params(1, params.clone()).unwrap();
        plugin.configure_qa_ort_pitch(runtime.clone()).unwrap();
        plugin.initialize(48000.0).unwrap();
        assert!(plugin.configure_qa_ort_pitch(runtime.clone()).is_err());
        let mut baseline = PndPlugin::from_params(1, params).unwrap();
        baseline.initialize(48000.0).unwrap();
        assert_eq!(plugin.latency_samples(), 2047);
        let input = tone(48000.0, 444.0, false, 1e-5);
        let mut a = vec![0.0; input.len()];
        let mut b = a.clone();
        for _ in 0..8 {
            assert_no_allocs_or_deallocs("ORT PND process", || {
                plugin
                    .process(&input, &mut a, &ProcessContext::new(48000, input.len()))
                    .unwrap();
                baseline
                    .process(&input, &mut b, &ProcessContext::new(48000, input.len()))
                    .unwrap();
            });
            assert_eq!(a, b);
        }
        let before = plugin.pitch_decision_snapshot();
        assert!(plugin.initialize(8000.0).is_err());
        assert_eq!(before, plugin.pitch_decision_snapshot());
        assert_no_allocs_or_deallocs("ORT PND reset", || plugin.reset());
        assert_eq!(plugin.pitch_decision_snapshot().analysis_generation, 0);
        let mut active = PndPlugin::from_params(
            1,
            PndPluginParams {
                reference_frequency_hz: 440.0,
                neural_pitch: Some(config()),
                ..PndPluginParams::default()
            },
        )
        .unwrap();
        active.configure_qa_ort_pitch(runtime.clone()).unwrap();
        active.initialize(48000.0).unwrap();
        let source = tone(48000.0, 440.0, false, 0.2);
        let mut destination = vec![0.0; source.len()];
        active
            .process(
                &source,
                &mut destination,
                &ProcessContext::new(48000, source.len()),
            )
            .unwrap();
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(2));
            active
                .process(
                    &source[..512],
                    &mut destination[..512],
                    &ProcessContext::new(48000, 512),
                )
                .unwrap();
            if active.pitch_decision_snapshot().neural_pitch_used {
                break;
            }
        }
        assert!(active.pitch_decision_snapshot().neural_pitch_used);
        let frozen = active.pitch_decision_snapshot();
        let evidence = frozen.neural_decision.unwrap();
        assert_eq!(evidence.window_samples, 3072);
        assert!(evidence.observation_end_sample <= evidence.decision_source_sample);
        let age = evidence.decision_source_sample - evidence.observation_end_sample
            + evidence.window_samples;
        assert!(age <= 7200);
        assert!(evidence.confidence >= config().min_confidence);
        assert!((1200.0 * (evidence.frequency_hz / 440.0).log2()).abs() <= 100.0);
        // Publication can continue while the source is stationary. Snapshots
        // must retain the evidence used at selection, not query a newer result.
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(active.pitch_decision_snapshot(), frozen);
        let mut completed = false;
        for _ in 0..128 {
            let mut result = None;
            assert_no_allocs_or_deallocs("ORT active PND drain", || {
                result = Some(
                    active
                        .drain(&mut destination[..128], &ProcessContext::new(48000, 128))
                        .unwrap(),
                );
            });
            if result.unwrap().complete {
                completed = true;
                break;
            }
        }
        assert!(completed);
        assert_eq!(active.pitch_decision_snapshot(), frozen);
        assert_no_allocs_or_deallocs("ORT selected-evidence reset", || active.reset());
        let reset = active.pitch_decision_snapshot();
        assert!(reset.neural_decision.is_none());
        assert!(!reset.neural_pitch_used);
        assert_eq!(reset.neural_backend, frozen.neural_backend);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(active.pitch_decision_snapshot(), reset);
    }
    // A different canonical path is refused even when its bytes are identical.
    let path = std::env::temp_dir().join(format!("sotf-ort-binding-{}.dylib", std::process::id()));
    std::fs::copy(&first.library_path, &path).unwrap();
    let mut changed = first.clone();
    changed.library_path = path.clone();
    assert!(CrepeModel::load_ort(&config(), 48000.0, &changed).is_err());
    std::fs::remove_file(path).unwrap();
    let library_sha: String = Sha256::digest(std::fs::read(&first.library_path).unwrap())
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    assert_eq!(library_sha, first.library_sha256);
    let report = serde_json::json!({"library_sha256": library_sha,"rows": records,"placement":placements,"activation_frames_per_provider":32,"maximum_activation_difference":maximum_activation_error,"maximum_pitch_difference_cents": maximum_pitch_error,"maximum_confidence_difference": maximum_confidence_error,"scope":"same preprocessing/decoder, actual model, CPU providers; controlled worker completion is not a paced deadline budget"});
    if let Ok(path) = std::env::var("SOTF_ORT_QA_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
