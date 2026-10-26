//! Actual native mask selection, frozen inference parity and independent fallback.
#![cfg(feature = "qa")]
// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;
use sha2::{Digest, Sha256};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_upmixer::source_mask::{SourceMaskModel, band_features};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MODEL: &[u8] = include_bytes!("../../../models/upmixer-source-mask/model.synthetic.json");

fn model() -> SourceMaskModel {
    SourceMaskModel::from_json(MODEL, &sha(MODEL)).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}

fn make(fft: usize, rate: u32, installed: bool) -> UpmixerPlugin {
    let mut params = UpmixerPluginParams::default();
    params.core.fft_size = fft;
    params.core.enable_hr_direct = false;
    let mut p = UpmixerPlugin::from_params(params);
    p.initialize(f64::from(rate)).unwrap();
    p.configure_source_masks(installed.then(model)).unwrap();
    p
}

fn input() -> Vec<f32> {
    (0..5101)
        .flat_map(|i| {
            let x = 0.03 * (i as f32 * 0.174).sin();
            let y = 0.015 * (i as f32 * 0.371).sin();
            [x + y, x - 0.7 * y]
        })
        .collect()
}

fn render(p: &mut UpmixerPlugin, input: &[f32], callback: usize, rate: u32) -> Vec<f32> {
    let channels = p.output_channels();
    let mut output = vec![0.0; callback * channels];
    let mut result = Vec::new();
    for block in input.chunks(callback * 2) {
        let frames = block.len() / 2;
        let ctx = ProcessContext::new(rate, frames);
        assert_no_allocs_or_deallocs("source-mask process", || {
            p.process(block, &mut output[..frames * channels], &ctx)
                .unwrap();
        });
        result.extend_from_slice(&output[..frames * channels]);
    }
    let before = p.source_mask_snapshot();
    for _ in 0..10000 {
        let mut drained = None;
        assert_no_allocs_or_deallocs("source-mask drain", || {
            drained = Some(
                p.drain(&mut output, &ProcessContext::new(rate, callback))
                    .unwrap(),
            );
        });
        let drained = drained.unwrap();
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            assert_eq!(
                p.source_mask_snapshot().analysis_frames,
                before.analysis_frames
            );
            assert_eq!(p.source_mask_snapshot().masks, before.masks);
            assert_eq!(p.source_mask_snapshot().band_power, before.band_power);
            return result;
        }
    }
    panic!("finite source-mask drain did not complete");
}

#[test]
fn frozen_native_probabilities_match_independent_python() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/upmixer-source-mask/parity.synthetic.json"
    ))
    .unwrap();
    let m = SourceMaskModel::from_json(MODEL, fixture["model_sha256"].as_str().unwrap()).unwrap();
    for point in fixture["points"].as_array().unwrap() {
        let x: [f64; 6] = serde_json::from_value(point["features"].clone()).unwrap();
        let expected = point["prediction"].as_f64().unwrap();
        assert!((f64::from(m.predict(&x).unwrap()) - expected).abs() < 2e-7);
    }
    let mut invalid = [0.5; 6];
    invalid[3] = f64::NAN;
    assert!(m.predict(&invalid).is_none());
}

#[test]
fn covariance_features_match_closed_form_and_native_headroom_compensation() {
    let left = vec![Complex::new(0.25, 0.0); 9];
    let right = vec![Complex::new(0.0, 0.25); 9];
    let f = band_features(&left, &right, 1, 9, 1.0).unwrap();
    assert_eq!(&f[..5], &[0.0, 0.5, 1.0, 1.0, 9.0 / 512.0]);
    assert!((f[5] - (10.0 * (1.0_f64 / 512.0).log10() + 100.0) / 100.0).abs() < 1e-15);
    let scaled = left
        .iter()
        .map(|x| *x * std::f32::consts::FRAC_1_SQRT_2)
        .collect::<Vec<_>>();
    let scaled_r = right
        .iter()
        .map(|x| *x * std::f32::consts::FRAC_1_SQRT_2)
        .collect::<Vec<_>>();
    let compensated = band_features(&scaled, &scaled_r, 1, 9, 2.0).unwrap();
    for (a, b) in f.iter().zip(compensated) {
        assert!((a - b).abs() < 2e-8);
    }
    assert!(band_features(&left, &right, 0, 9, 1.0).is_none());
    assert!(band_features(&left, &right, 1, 10, 1.0).is_none());
    assert!(band_features(&left, &right, 1, 9, f64::NAN).is_none());
}

#[test]
fn actual_selection_is_callback_independent_and_reset_reproducible_without_heap() {
    let x = input();
    let mut one = make(512, 48000, true);
    let mut odd = make(512, 48000, true);
    let latency = one.latency_samples();
    assert_eq!(latency, make(512, 48000, false).latency_samples());
    let a = render(&mut one, &x, 1, 48000);
    let b = render(&mut odd, &x, 257, 48000);
    assert_eq!(a, b);
    assert!(a.iter().all(|v| v.is_finite()));
    assert!(
        a[..latency * one.output_channels()]
            .iter()
            .all(|v| *v == 0.0)
    );
    assert!(one.source_mask_snapshot().selected_bands > 0);
    assert_eq!(
        one.source_mask_snapshot().masks,
        odd.source_mask_snapshot().masks
    );
    assert_eq!(
        one.source_mask_snapshot().band_power,
        odd.source_mask_snapshot().band_power
    );
    assert!(
        one.source_mask_snapshot()
            .band_power
            .iter()
            .flatten()
            .all(|power| power.is_finite() && *power >= 0.0)
    );
    assert!(one.configure_source_masks(None).is_err());
    assert_no_allocs_or_deallocs("source-mask reset", || one.reset());
    assert_eq!(one.source_mask_snapshot().analysis_frames, 0);
    assert!(
        one.source_mask_snapshot()
            .band_power
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(a, render(&mut one, &x, 113, 48000));
    let mut native = make(512, 48000, false);
    assert_ne!(a, render(&mut native, &x, 257, 48000));
}

#[test]
fn unsupported_geometry_and_silence_preserve_independent_native_output() {
    let x = input();
    for (fft, rate) in [(1024, 48000), (512, 44100), (512, 96000)] {
        let mut native = make(fft, rate, false);
        let mut selected = make(fft, rate, true);
        assert_eq!(
            render(&mut native, &x, 257, rate),
            render(&mut selected, &x, 257, rate)
        );
        assert_eq!(selected.source_mask_snapshot().selected_bands, 0);
        assert!(
            selected
                .source_mask_snapshot()
                .band_power
                .iter()
                .all(Option::is_none)
        );
    }
    let mut silent = make(512, 48000, true);
    assert!(
        render(&mut silent, &vec![0.0; 2049 * 2], 257, 48000)
            .iter()
            .all(|x| *x == 0.0)
    );
    assert_eq!(silent.source_mask_snapshot().selected_bands, 0);
    assert_eq!(silent.source_mask_snapshot().band_power, [Some(0.0); 32]);
}

#[test]
fn malformed_or_unadmitted_contracts_do_not_load() {
    for key in ["deployment_admitted", "format", "policy", "coefficients"] {
        let mut doc: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
        doc[key] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&doc).unwrap();
        assert!(SourceMaskModel::from_json(&bytes, &sha(&bytes)).is_err());
    }
    assert!(SourceMaskModel::from_json(MODEL, &"0".repeat(64)).is_err());
}

#[test]
fn v2_contract_requires_exact_excitation_and_native_training_provenance() {
    let mut doc: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
    doc["format"] = serde_json::json!("sotf-upmixer-direct-ambient-mask-v2");
    doc["policy"]["fallback"] = serde_json::json!("ordinary_native_decomposition");
    doc["policy"]["excitation"] = serde_json::json!({
        "band_power_interval_exclusive": [1e-16, 1e12],
        "relative_to_peak_band_min": 1e-6,
        "power_scale": "reference_fft", "dc_nyquist": "excluded"
    });
    doc["provenance"]["training_features"] = serde_json::json!("native_f32_fft_f64_covariance");
    let bytes = serde_json::to_vec(&doc).unwrap();
    assert!(SourceMaskModel::from_json(&bytes, &sha(&bytes)).is_ok());
    for changed in [serde_json::json!(1e-5), serde_json::json!(true)] {
        let mut invalid = doc.clone();
        invalid["policy"]["excitation"]["relative_to_peak_band_min"] = changed;
        let bytes = serde_json::to_vec(&invalid).unwrap();
        assert!(SourceMaskModel::from_json(&bytes, &sha(&bytes)).is_err());
    }
    doc["provenance"]
        .as_object_mut()
        .unwrap()
        .remove("training_features");
    let bytes = serde_json::to_vec(&doc).unwrap();
    assert!(SourceMaskModel::from_json(&bytes, &sha(&bytes)).is_err());
}

#[test]
fn impulse_measures_no_added_audio_latency_and_preserves_native_tail_bound() {
    for source in [0, 1] {
        let mut x = vec![0.0; 2049 * 2];
        x[source] = 0.1;
        let mut native = make(512, 48000, false);
        let mut selected = make(512, 48000, true);
        assert_eq!(native.tail_length(), selected.tail_length());
        let channels = native.output_channels();
        let a = render(&mut native, &x, 113, 48000);
        let b = render(&mut selected, &x, 113, 48000);
        let first = |audio: &[f32]| {
            audio
                .chunks_exact(channels)
                .position(|frame| frame.iter().any(|v| v.abs() > 1e-9))
                .unwrap()
        };
        println!(
            "source={source}, native onset={}, selected onset={}, declared={}",
            first(&a),
            first(&b),
            native.latency_samples()
        );
        assert_eq!(first(&a), native.latency_samples());
        assert_eq!(first(&b), first(&a));
        assert_eq!(a.len(), b.len());
    }
}

#[test]
fn partial_source_prevents_replacement_and_hr_or_scalar_modes_fall_back() {
    let x = input();
    let mut partial = make(512, 48000, true);
    let mut out = vec![0.0; partial.output_channels()];
    partial
        .process(&x[..2], &mut out, &ProcessContext::new(48000, 1))
        .unwrap();
    assert!(partial.configure_source_masks(None).is_err());
    partial.reset();
    partial.configure_source_masks(None).unwrap();
    for key in ["enable_hr_direct", "enable_ml_detection"] {
        let mut a = make(512, 48000, false);
        let mut b = make(512, 48000, true);
        for p in [&mut a, &mut b] {
            p.set_parameter(key.into(), sotf_host::ParameterValue::Bool(true))
                .unwrap();
        }
        assert_eq!(
            render(&mut a, &x, 257, 48000),
            render(&mut b, &x, 257, 48000)
        );
        assert_eq!(b.source_mask_snapshot().selected_bands, 0);
    }
}
