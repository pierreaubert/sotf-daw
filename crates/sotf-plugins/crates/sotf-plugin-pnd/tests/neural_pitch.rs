//! Actual pretrained-model parity, causal policy, lifecycle, and source-rate checks.
#![cfg(feature = "neural-pitch")]

use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_pnd::neural_pitch::{CrepeModel, NeuralPitchAssistant};
use sotf_plugin_pnd::{NeuralPitchConfig, PndPlugin, PndPluginParams};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[cfg(feature = "qa")]
#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

fn model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/crepe-tiny")
}

fn config() -> NeuralPitchConfig {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(model_dir().join("manifest.json")).unwrap()).unwrap();
    NeuralPitchConfig {
        model_path: model_dir()
            .join("crepe-tiny.onnx")
            .to_string_lossy()
            .into_owned(),
        sha256: manifest["sha256"].as_str().unwrap().into(),
        min_confidence: 0.8,
        max_age_ms: 150.0,
        max_deviation_cents: 100.0,
        source_channel: 0,
    }
}

fn tone(rate: f64, hz: f64, weak: bool, scale: f64) -> Vec<f32> {
    (0..(1024.0 * rate / 16000.0).ceil() as usize)
        .map(|i| {
            let phase = std::f64::consts::TAU * hz * i as f64 / rate;
            (scale
                * if weak {
                    0.01 * phase.sin() + 0.6 * (2.0 * phase).sin() + 0.3 * (3.0 * phase).sin()
                } else {
                    phase.sin()
                }) as f32
        })
        .collect()
}

#[test]
fn tract_matches_independent_pytorch_activations() {
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(model_dir().join("parity.json")).unwrap()).unwrap();
    let mut model = CrepeModel::load(&config(), 16_000.0).unwrap();
    let mut max_error = 0.0_f32;
    for (input, expected) in data["input"]
        .as_array()
        .unwrap()
        .iter()
        .zip(data["activation"].as_array().unwrap())
    {
        let frame: [f32; 1024] = input
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap() as f32)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let actual = model.activations(&frame).unwrap();
        for (a, b) in actual.iter().zip(expected.as_array().unwrap()) {
            max_error = max_error.max((a - b.as_f64().unwrap() as f32).abs());
        }
    }
    println!("crepe cross-runtime activation max_error={max_error}");
    assert!(max_error < 2e-5, "cross-runtime parity error {max_error}");
}

#[test]
fn model_hash_shape_and_policy_fail_before_audio() {
    let mut bad = config();
    bad.sha256 = "0".repeat(64);
    assert!(CrepeModel::load(&bad, 48_000.0).is_err());
    assert!(CrepeModel::load(&config(), 8000.0).is_err());
    let mut bad = config();
    bad.min_confidence = f32::NAN;
    assert!(NeuralPitchAssistant::new(bad, 48_000.0).is_err());
    let params = PndPluginParams {
        neural_pitch: Some(config()),
        ..Default::default()
    };
    assert!(
        PndPlugin::from_params(1, params).is_err(),
        "explicit reference required"
    );
    let old: PndPluginParams = serde_json::from_str("{}").unwrap();
    assert!(old.neural_pitch.is_none());
}

#[test]
fn actual_model_cross_rate_pitch_and_weak_fundamental_characterization() {
    let mut rows = Vec::new();
    for rate in [16_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let mut model = CrepeModel::load(&config(), rate).unwrap();
        for hz in [82.41, 130.81, 220.0, 440.0, 659.25, 880.0, 1318.51] {
            for weak in [false, true] {
                let samples = tone(rate, hz, weak, 0.3);
                let start = Instant::now();
                let observation = model
                    .analyze_window(&samples, samples.len() as u64)
                    .unwrap();
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                let cents = 1200.0 * (f64::from(observation.frequency_hz) / hz).log2();
                rows.push(
                    serde_json::json!({"rate":rate,"hz":hz,"weak_fundamental":weak,
                    "cents_error":cents,"confidence":observation.confidence,"inference_ms":ms}),
                );
            }
        }
        for scale in [0.0, 0.00001] {
            let samples = tone(rate, 440.0, false, scale);
            let p = model
                .analyze_window(&samples, samples.len() as u64)
                .unwrap();
            assert_eq!(
                p.confidence, 0.0,
                "silence must not be normalized into confident pitch"
            );
        }
    }
    let mut errors: Vec<f64> = rows
        .iter()
        .filter(|r| r["weak_fundamental"] == false)
        .map(|r| r["cents_error"].as_f64().unwrap().abs())
        .collect();
    errors.sort_by(f64::total_cmp);
    println!(
        "crepe clean median_cents={} max_cents={} rows={}",
        errors[errors.len() / 2],
        errors.last().unwrap(),
        rows.len()
    );
    // This synthetic smoke gate is deliberately distinct from held-out quality acceptance.
    assert!(errors[errors.len() / 2] < 20.0);
    assert!(*errors.last().unwrap() < 60.0);
    if let Ok(path) = std::env::var("SOTF_ML_QA_REPORT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "kind":"synthetic_characterization_not_held_out_acceptance", "rows":rows
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn worker_reset_discards_old_source_and_publishes_real_pitch() {
    let mut assistant = NeuralPitchAssistant::new(config(), 48_000.0).unwrap();
    for sample in tone(48_000.0, 440.0, false, 0.3) {
        assistant.push(sample);
    }
    let start = Instant::now();
    let observation = loop {
        if let Some(p) = assistant.estimate_against_reference(440.0) {
            break p;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no valid real-model observation"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!((1200.0 * observation.0.log2()).abs() < 30.0);
    assistant.reset();
    assert!(assistant.estimate_against_reference(440.0).is_none());
    std::thread::sleep(Duration::from_millis(20));
    assert!(assistant.estimate_against_reference(440.0).is_none());
}

#[test]
fn plugin_neural_silence_fallback_matches_classical_and_preserves_latency() {
    let params = PndPluginParams {
        reference_frequency_hz: 440.0,
        ..Default::default()
    };
    let mut classical = PndPlugin::from_params(1, params.clone()).unwrap();
    let mut neural = PndPlugin::from_params(
        1,
        PndPluginParams {
            neural_pitch: Some(config()),
            ..params
        },
    )
    .unwrap();
    classical.initialize(48_000.0).unwrap();
    neural.initialize(48_000.0).unwrap();
    assert_eq!(classical.latency_samples(), neural.latency_samples());
    // Nonzero below the neural silence floor still exercises classical pitch analysis.
    let input = tone(48_000.0, 444.0, false, 0.00001);
    let mut a = vec![0.0; input.len()];
    let mut b = a.clone();
    for _ in 0..8 {
        let context = ProcessContext::new(48_000, input.len());
        classical.process(&input, &mut a, &context).unwrap();
        #[cfg(feature = "qa")]
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            neural.process(&input, &mut b, &context).unwrap();
        });
        #[cfg(not(feature = "qa"))]
        neural.process(&input, &mut b, &context).unwrap();
        #[cfg(feature = "qa")]
        assert_eq!(
            heap,
            (0, 0),
            "audio side may not allocate or free model work"
        );
        assert_eq!(a, b);
        std::thread::sleep(Duration::from_millis(10));
    }
    #[cfg(feature = "qa")]
    assert_eq!(
        sotf_host::test_utils::measure_heap_activity(|| neural.reset()),
        (0, 0)
    );
    #[cfg(not(feature = "qa"))]
    neural.reset();
}

#[test]
fn plugin_adopts_actual_neural_observations_and_freezes_at_eof() {
    let mut plugin = PndPlugin::from_params(
        1,
        PndPluginParams {
            reference_frequency_hz: 440.0,
            neural_pitch: Some(config()),
            confidence_threshold: 0.5,
            drift_smoothing: 0.001,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(48_000.0).unwrap();
    let mut seen_neural = false;
    let mut output = vec![0.0; 480];
    for block in 0..50 {
        let input: Vec<f32> = (0..480)
            .map(|i| {
                (0.3 * (std::f64::consts::TAU * 444.4 * (block * 480 + i) as f64 / 48_000.0).sin())
                    as f32
            })
            .collect();
        let context = ProcessContext::new(48_000, 480);
        #[cfg(feature = "qa")]
        assert_eq!(
            sotf_host::test_utils::measure_heap_activity(|| {
                plugin.process(&input, &mut output, &context).unwrap();
            }),
            (0, 0)
        );
        #[cfg(not(feature = "qa"))]
        plugin.process(&input, &mut output, &context).unwrap();
        let data = plugin
            .get_data()
            .unwrap()
            .downcast::<sotf_plugin_pnd::PndData>()
            .unwrap();
        if data.neural_pitch_used {
            seen_neural = true;
            assert!((data.drift_ratio - 1.0).abs() < 0.06);
            assert!(data.confidence >= 0.8);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        seen_neural,
        "real model results must reach the correction path"
    );
    let before = plugin
        .get_data()
        .unwrap()
        .downcast::<sotf_plugin_pnd::PndData>()
        .unwrap();
    let mut remaining_calls = plugin.drain_call_bound().unwrap().get();
    loop {
        assert!(remaining_calls > 0);
        remaining_calls -= 1;
        if plugin
            .drain(&mut output, &ProcessContext::new(48_000, 480))
            .unwrap()
            .complete
        {
            break;
        }
    }
    let after = plugin
        .get_data()
        .unwrap()
        .downcast::<sotf_plugin_pnd::PndData>()
        .unwrap();
    assert_eq!(before.correction_ratio, after.correction_ratio);
    plugin.reset();
    assert!(
        !plugin
            .get_data()
            .unwrap()
            .downcast::<sotf_plugin_pnd::PndData>()
            .unwrap()
            .neural_pitch_used
    );
}
