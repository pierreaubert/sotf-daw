//! Learned mask arithmetic, covariance and complete native streaming contracts.

#![cfg(feature = "qa")]

// Rust guideline compliant 2026-02-21
use nalgebra::Complex;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_beamformer::mvdr::MvdrBeamformer;
use sotf_plugin_beamformer::noise_mask::{NoiseMask, spatial_features};
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MODEL: &[u8] = include_bytes!("../../../models/beamformer-noise-mask/model.synthetic.json");
const SHA: &str = "5ed1fe74a6aa11d702c20469a87a209198cb742f3233903f32d12b11729998f7";

fn model() -> NoiseMask {
    NoiseMask::from_json(MODEL, SHA).unwrap()
}
fn plugin(mics: usize, rate: u32, spacing: f32) -> BeamformerPlugin {
    let mut p = BeamformerPlugin::from_params(
        rate,
        BeamformerPluginParams {
            num_mics: mics,
            mic_spacing_cm: spacing,
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}
fn input(mics: usize) -> Vec<f32> {
    (0..mics * 8197)
        .map(|i| {
            let mut v = (i as u32).wrapping_add(0x9e37_79b9);
            v = (v ^ (v >> 16)).wrapping_mul(0x85eb_ca6b);
            v = (v ^ (v >> 13)).wrapping_mul(0xc2b2_ae35);
            v ^= v >> 16;
            ((f64::from(v) / f64::from(u32::MAX) - 0.5) * 0.05) as f32
        })
        .collect()
}
fn process(
    p: &mut BeamformerPlugin,
    x: &[f32],
    mics: usize,
    rate: u32,
    callback: usize,
) -> Vec<f32> {
    let frames = x.len() / mics;
    let mut y = vec![f32::NAN; frames];
    for start in (0..frames).step_by(callback) {
        let count = (frames - start).min(callback);
        assert_no_allocs_or_deallocs("neural MVDR stream", || {
            assert_eq!(
                p.process(
                    &x[start * mics..(start + count) * mics],
                    &mut y[start..start + count],
                    &ProcessContext::new(rate, count)
                )
                .unwrap(),
                count
            );
        });
    }
    y
}

#[test]
fn native_features_and_network_match_frozen_numpy_reference() {
    let m = model();
    let rows: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../models/beamformer-noise-mask/parity.synthetic.json"
    ))
    .unwrap();
    let mut feature_error = 0.0_f64;
    let mut probability_error = 0.0_f64;
    for row in rows.as_array().unwrap() {
        let microphones = row["microphones"].as_u64().unwrap() as usize;
        let bin = row["bin"].as_u64().unwrap() as usize;
        let bins = row["bins"].as_u64().unwrap() as usize;
        let value = |v: &serde_json::Value| {
            Complex::new(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32)
        };
        let mut x = vec![vec![Complex::new(0.0, 0.0); bins]; microphones];
        let mut steering = Vec::new();
        for (i, channel) in x.iter_mut().enumerate() {
            channel[bin] = value(&row["input"][i]);
            steering.push(value(&row["steering"][i]));
        }
        let f = spatial_features(&x, &steering, bin, bin as f64 / (bins - 1) as f64).unwrap();
        for (i, &v) in f.iter().enumerate() {
            feature_error = feature_error.max((v - row["features"][i].as_f64().unwrap()).abs());
        }
        let mut p = None;
        assert_no_allocs_or_deallocs("mask scalar inference", || p = m.probability(&f));
        probability_error = probability_error
            .max((f64::from(p.unwrap()) - row["probability"].as_f64().unwrap()).abs());
    }
    assert!(feature_error < 1e-12);
    assert!(probability_error < 3e-8);
    println!("native mask parity: feature={feature_error}, probability={probability_error}");
    assert!(NoiseMask::from_json(MODEL, &"0".repeat(64)).is_err());
    assert!(m.probability(&[f64::NAN; 6]).is_none());
}

#[test]
fn weighted_covariance_matches_independent_recurrence_and_retains_native_weight_bounds() {
    let mut q = MvdrBeamformer::new(2, 3);
    let x = vec![
        vec![Complex::new(0.3, 0.1); 3],
        vec![Complex::new(-0.2, 0.4); 3],
    ];
    let masks = [1.0, 0.5, 0.0];
    let alpha = 0.95_f32;
    let mut expected = vec![Complex::new(0.0_f64, 0.0); 12];
    for k in 0..3 {
        expected[k * 4] = Complex::new(1.0, 0.0);
        expected[k * 4 + 3] = Complex::new(1.0, 0.0);
    }
    for _ in 0..33 {
        assert_no_allocs_or_deallocs("weighted noise covariance", || {
            assert!(q.update_noise_covariance_masked(&x, &masks))
        });
        for k in 0..3 {
            if masks[k] == 0.0 {
                continue;
            }
            let beta = (1.0 - alpha) * masks[k];
            let keep = if masks[k] == 1.0 { alpha } else { 1.0 - beta };
            for i in 0..2 {
                for j in 0..2 {
                    let wide = |v: Complex<f32>| Complex::new(f64::from(v.re), f64::from(v.im));
                    let index = k * 4 + i * 2 + j;
                    expected[index] = expected[index] * f64::from(keep)
                        + wide(x[i][k]) * wide(x[j][k]).conj() * f64::from(beta);
                }
            }
        }
    }
    for (a, b) in q.noise_cov_snapshot().iter().zip(expected) {
        assert!((Complex::new(f64::from(a.re), f64::from(a.im)) - b).norm() < 1e-6);
    }
    let steering = vec![vec![Complex::new(1.0, 0.0); 2]; 3];
    let weights = q.compute_weights(&steering);
    for w in weights {
        let response: Complex<f32> = w.iter().map(|v| v.conj()).sum();
        assert!((response - Complex::new(1.0, 0.0)).norm() < 1e-5);
        assert!(w.iter().map(|v| v.norm_sqr()).sum::<f32>() <= 1.00001);
    }
    let before = q.noise_cov_snapshot().to_vec();
    assert!(!q.update_noise_covariance_masked(&x, &[1.0, f32::NAN, 0.0]));
    assert_eq!(q.noise_cov_snapshot(), before);
    let mut bad = x.clone();
    bad[0][0].re = f32::INFINITY;
    assert!(!q.update_noise_covariance_masked(&bad, &masks));
    assert_eq!(q.noise_cov_snapshot(), before);
}

#[test]
fn missing_disabled_and_unsupported_context_use_exact_native_streams() {
    for rate in [16000, 48000, 96000] {
        for microphones in [2, 4, 8] {
            let x = input(microphones);
            let expected = process(
                &mut plugin(microphones, rate, 5.0),
                &x,
                microphones,
                rate,
                193,
            );
            for present in [false, true] {
                let mut p = plugin(microphones, rate, 5.0);
                p.install_qa_noise_mask(present.then(model)).unwrap();
                p.set_qa_noise_mask_enabled(!present).unwrap();
                assert_eq!(process(&mut p, &x, microphones, rate, 17), expected);
            }
            let mut p = plugin(microphones, rate, 25.0);
            p.install_qa_noise_mask(Some(model())).unwrap();
            p.set_qa_noise_mask_enabled(true).unwrap();
            let expected = process(
                &mut plugin(microphones, rate, 25.0),
                &x,
                microphones,
                rate,
                193,
            );
            assert_eq!(process(&mut p, &x, microphones, rate, 4096), expected);
            assert!(p.qa_noise_mask_counters().unwrap().unavailable_frames > 0);
        }
    }
}

#[test]
fn enabled_callbacks_reset_disable_and_drain_preserve_native_contracts() {
    let x = input(4);
    let mut a = plugin(4, 48000, 5.0);
    a.install_qa_noise_mask(Some(model())).unwrap();
    a.set_qa_noise_mask_enabled(true).unwrap();
    let mut b = plugin(4, 48000, 5.0);
    b.install_qa_noise_mask(Some(model())).unwrap();
    b.set_qa_noise_mask_enabled(true).unwrap();
    let actual = process(&mut a, &x, 4, 48000, 17);
    let expected = process(&mut b, &x, 4, 48000, 4096);
    assert_eq!(actual, expected);
    assert_eq!(a.qa_noise_mask_counters(), b.qa_noise_mask_counters());
    let counts = a.qa_noise_mask_counters().unwrap();
    assert!(counts.noise_bins > 0 && counts.uncertain_bins > 0);
    assert!(actual.iter().all(|v| v.is_finite()));
    assert_eq!(a.latency_samples(), 512);
    let mut buffer = [0.0; 257];
    let bound = a.drain_call_bound().unwrap().get();
    let mut completed = false;
    for _ in 0..bound {
        let mut done = false;
        assert_no_allocs_or_deallocs("masked MVDR drain", || {
            done = a
                .drain(&mut buffer, &ProcessContext::new(48000, 257))
                .unwrap()
                .complete
        });
        if done {
            completed = true;
            break;
        }
    }
    assert!(completed);
    assert_eq!(a.qa_noise_mask_counters().unwrap(), counts);
    assert_no_allocs_or_deallocs("masked MVDR reset", || a.reset());
    assert_eq!(a.qa_noise_mask_counters().unwrap(), Default::default());
    assert_eq!(process(&mut a, &x, 4, 48000, 193), actual);
    a.set_qa_noise_mask_enabled(false).unwrap();
    let mut reference = plugin(4, 48000, 5.0);
    process(&mut reference, &x, 4, 48000, 193);
    let expected = process(&mut reference, &x, 4, 48000, 193);
    let actual = process(&mut a, &x, 4, 48000, 193);
    // Previously synthesized learned frames can occupy at most two FFT windows.
    assert_eq!(&actual[1024..], &expected[1024..]);
}

#[test]
fn uncertain_network_is_exact_native_fallback_and_loader_rejects_changed_policy() {
    use sha2::{Digest, Sha256};
    let mut value: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
    value["weights"] = serde_json::json!(vec![vec![0.0; 8]; 6]);
    value["bias"] = serde_json::json!(vec![0.0; 8]);
    value["output_weights"] = serde_json::json!(vec![0.0; 8]);
    value["output_bias"] = serde_json::json!(0.0);
    let load = |v: &serde_json::Value| {
        let raw = serde_json::to_vec(v).unwrap();
        let hash: String = Sha256::digest(&raw)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        NoiseMask::from_json(&raw, &hash)
    };
    let mut p = plugin(4, 48000, 5.0);
    p.install_qa_noise_mask(Some(load(&value).unwrap()))
        .unwrap();
    p.set_qa_noise_mask_enabled(true).unwrap();
    let x = input(4);
    let expected = process(&mut plugin(4, 48000, 5.0), &x, 4, 48000, 193);
    assert_eq!(process(&mut p, &x, 4, 48000, 17), expected);
    assert!(p.qa_noise_mask_counters().unwrap().uncertain_bins > 0);
    value["policy"]["confidence"] = serde_json::json!(0.5);
    assert!(load(&value).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
    value["deployment_admitted"] = serde_json::json!(true);
    assert!(load(&value).is_err());
}

#[test]
fn wrong_rate_and_live_replacement_reject_before_state_or_output_changes() {
    let x = input(4);
    let mut a = plugin(4, 48000, 5.0);
    a.install_qa_noise_mask(Some(model())).unwrap();
    a.set_qa_noise_mask_enabled(true).unwrap();
    let mut b = plugin(4, 48000, 5.0);
    b.install_qa_noise_mask(Some(model())).unwrap();
    b.set_qa_noise_mask_enabled(true).unwrap();
    let mut output = [1.0; 17];
    assert!(
        a.process(&x[..68], &mut output, &ProcessContext::new(96000, 17))
            .is_err()
    );
    assert_eq!(output, [1.0; 17]);
    assert_eq!(
        process(&mut a, &x, 4, 48000, 193),
        process(&mut b, &x, 4, 48000, 193)
    );
    assert!(a.install_qa_noise_mask(Some(model())).is_err());
    assert_eq!(
        process(&mut a, &x, 4, 48000, 193),
        process(&mut b, &x, 4, 48000, 193)
    );
}

#[test]
#[ignore = "Local deadline evidence; run alone with one test thread"]
fn measure_mask_callback_budget() {
    use std::time::Instant;
    let mut rows = Vec::new();
    let callback = 193;
    for rate in [16000, 48000, 96000] {
        for mode in ["production", "disabled", "neural"] {
            let mut p = plugin(8, rate, 5.0);
            if mode != "production" {
                p.install_qa_noise_mask(Some(model())).unwrap();
                p.set_qa_noise_mask_enabled(mode == "neural").unwrap();
            }
            let x = input(8);
            let mut y = vec![0.0; callback];
            let context = ProcessContext::new(rate, callback);
            for _ in 0..8 {
                p.process(&x[..callback * 8], &mut y, &context).unwrap();
            }
            let mut times = Vec::with_capacity(200);
            for _ in 0..200 {
                let start = Instant::now();
                p.process(&x[..callback * 8], &mut y, &context).unwrap();
                times.push(start.elapsed().as_secs_f64());
            }
            times.sort_by(f64::total_cmp);
            let deadline = callback as f64 / f64::from(rate);
            rows.push(serde_json::json!({"mode":mode,"sample_rate":rate,"microphones":8,"callback_frames":callback,"callbacks":200,
            "p50_seconds":times[100],"p95_seconds":times[190],"p99_seconds":times[198],"max_seconds":times[199],
            "callback_deadline_seconds":deadline,"declared_local_budget_seconds":0.8*deadline,"local_budget_pass":times[199]<=0.8*deadline,
            "latency_samples":p.latency_samples(),"added_model_lookahead_samples":0,"counts":p.qa_noise_mask_counters()}));
        }
    }
    println!(
        "{}",
        serde_json::json!({"cases":rows,"deployment_admitted":false,"scope":"Local wall-clock samples including active native core and mask work; not platform or portable realtime admission. Budget is80 percent of the callback duration, fixed before measurement."})
    );
}
