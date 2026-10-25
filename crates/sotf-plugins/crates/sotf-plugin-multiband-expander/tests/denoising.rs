//! Explicit denoising must preserve an independent classical fallback.
#![cfg(feature = "qa")]
// Rust guideline compliant 2026-02-21
use sha2::{Digest, Sha256};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_multiband_expander::denoising::{DenoisingMask, features};
use sotf_plugin_multiband_expander::{MultibandExpanderPlugin, MultibandExpanderPluginParams};

fn model(bias: f64) -> DenoisingMask {
    let value = serde_json::json!({"format":"sotf-expander-denoising-mask-v1",
        "weights":vec![[0.0;8];6],"bias":vec![0.0;8],"output_weights":vec![0.0;8],
        "output_bias":bias,"provenance":"project-generated music/noise; GPL-3.0-or-later; no external audio",
        "deployment_admitted":false});
    let bytes = serde_json::to_vec(&value).unwrap();
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    DenoisingMask::from_json(&bytes, &sha).unwrap()
}

fn make(channels: usize, rate: u32) -> MultibandExpanderPlugin {
    let params = MultibandExpanderPluginParams {
        processing_mode: "spectral".into(),
        threshold_db: -24.0,
        ratio: 4.0,
        hold_ms: 0.0,
        ..Default::default()
    };
    let mut p = MultibandExpanderPlugin::with_params(channels, params);
    p.initialize(f64::from(rate)).unwrap();
    p
}

fn source(channels: usize) -> Vec<f32> {
    (0..5101 * channels)
        .map(|i| ((i * 17 % 103) as f32 - 51.0) * 0.001)
        .collect()
}

fn render(
    p: &mut MultibandExpanderPlugin,
    source: &[f32],
    channels: usize,
    rate: u32,
    block: usize,
    drain: usize,
) -> Vec<f32> {
    let mut out = source.to_vec();
    for chunk in out.chunks_mut(block * channels) {
        p.process_in_place(chunk, &ProcessContext::new(rate, chunk.len() / channels))
            .unwrap();
    }
    let counters = p.denoising_counters();
    let mut tail = vec![0.0; drain * channels];
    let mut complete = false;
    for _ in 0..4096 {
        let done = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        out.extend_from_slice(&tail[..done.frames * channels]);
        if done.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(
        p.denoising_counters(),
        counters,
        "EOF must not classify padding"
    );
    out
}

#[test]
fn absent_uncertain_and_unsupported_models_retain_exact_classical_stream() {
    for channels in [1, 2] {
        for rate in [44100, 48000, 96000, 192000] {
            let x = source(channels);
            let expected = render(&mut make(channels, rate), &x, channels, rate, 193, 256);
            for mask in [None, Some(model(0.0))] {
                let mut candidate = make(channels, rate);
                candidate.configure_denoising(mask).unwrap();
                assert_eq!(render(&mut candidate, &x, channels, rate, 17, 1), expected);
                assert_eq!(candidate.denoising_counters().unwrap().selected_bins, 0);
            }
            if rate == 192000 {
                let mut candidate = make(channels, rate);
                candidate.configure_denoising(Some(model(-8.0))).unwrap();
                assert_eq!(
                    render(&mut candidate, &x, channels, rate, 193, 17),
                    expected
                );
            }
        }
    }
}

#[test]
fn selected_synthesis_keeps_classical_state_callback_reset_and_latency_contracts() {
    for channels in [1, 2] {
        for rate in [44100, 48000, 96000] {
            let x = source(channels);
            let mut a = make(channels, rate);
            let mut b = make(channels, rate);
            let mut classical = make(channels, rate);
            a.configure_denoising(Some(model(-8.0))).unwrap();
            b.configure_denoising(Some(model(-8.0))).unwrap();
            let expected = render(&mut a, &x, channels, rate, 193, 256);
            assert_eq!(render(&mut b, &x, channels, rate, 17, 1), expected);
            let baseline = render(&mut classical, &x, channels, rate, 193, 17);
            assert!(expected.iter().all(|x| x.is_finite()));
            assert!(
                expected
                    .iter()
                    .zip(&baseline)
                    .any(|(a, b)| (a - b).abs() > 1e-5)
            );
            assert_eq!(
                a.classical_spectral_envelopes()
                    .unwrap()
                    .collect::<Vec<_>>(),
                classical
                    .classical_spectral_envelopes()
                    .unwrap()
                    .collect::<Vec<_>>()
            );
            let count = a.denoising_counters().unwrap();
            assert!(count.selected_bins > 0);
            assert_eq!(
                count.selected_bins + count.fallback_bins,
                count.source_hops * 513
            );
            assert_eq!(a.latency_samples(), 1024);
            assert!(a.configure_denoising(None).is_err());
            a.reset();
            assert_eq!(render(&mut a, &x, channels, rate, 137, 3), expected);
            a.initialize(f64::from(rate)).unwrap();
            assert_eq!(render(&mut a, &x, channels, rate, 137, 3), expected);
        }
    }
}

#[test]
fn feature_and_model_domains_are_explicit() {
    let p = vec![0.01; 513];
    let prior = vec![0.01; 513];
    let row = features(&p, &prior, 256).unwrap();
    for (a, b) in row.iter().zip([0.8, 0.5, 1.0, 0.5, 0.5, 0.8]) {
        assert!((a - b).abs() < 1e-12);
    }
    assert_eq!(model(0.0).predict(&row), Some(0.5));
    assert!(model(0.0).predict(&[f64::NAN; 6]).is_none());
    assert!(features(&vec![0.0; 513], &prior, 1).is_none());
    assert!(features(&vec![4097.0; 513], &prior, 1).is_none());
    assert!(DenoisingMask::from_json(b"{}", &"0".repeat(64)).is_err());
    let mut time = MultibandExpanderPlugin::new(1);
    time.initialize(48000.0).unwrap();
    assert!(time.configure_denoising(Some(model(-8.0))).is_err());
}

#[test]
fn frozen_trained_candidate_matches_python_features_and_network() {
    let bytes = include_bytes!("../../../models/expander-denoising-mask/model.synthetic.json");
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/expander-denoising-mask/parity.synthetic.json"
    ))
    .unwrap();
    let sha = fixture["model_sha256"].as_str().unwrap();
    let network = DenoisingMask::from_json(bytes, sha).unwrap();
    let mut feature_error = 0.0_f64;
    let mut prediction_error = 0.0_f64;
    for case in fixture["cases"].as_array().unwrap() {
        let values = |key: &str| {
            case[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>()
        };
        let power = values("power");
        let previous = values("previous");
        for point in case["points"].as_array().unwrap() {
            let bin = point["bin"].as_u64().unwrap() as usize;
            let actual = features(&power, &previous, bin).unwrap();
            for (a, b) in actual.iter().zip(point["features"].as_array().unwrap()) {
                feature_error = feature_error.max((a - b.as_f64().unwrap()).abs());
            }
            prediction_error = prediction_error.max(
                (f64::from(network.predict(&actual).unwrap())
                    - point["prediction"].as_f64().unwrap())
                .abs(),
            );
        }
    }
    println!("frozen expander parity feature={feature_error}, prediction={prediction_error}");
    assert!(feature_error < 1e-12);
    assert!(prediction_error < 2e-7);
}

#[test]
fn constant_mask_matches_independent_full_complex_wola_and_energy_bound() {
    use rustfft::{FftPlanner, num_complex::Complex};
    const N: usize = 1024;
    const H: usize = 256;
    let x = source(1);
    let mut params = MultibandExpanderPluginParams {
        processing_mode: "spectral".into(),
        ratio: 1.0,
        ..Default::default()
    };
    for band in &mut params.bands {
        band.ratio = Some(1.0);
    }
    let mut candidate = MultibandExpanderPlugin::with_params(1, params);
    candidate.initialize(48000.0).unwrap();
    candidate.configure_denoising(Some(model(-8.0))).unwrap();
    let actual = render(&mut candidate, &x, 1, 48000, 193, 17);
    let window: Vec<_> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();
    let mut planner = FftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(N);
    let inverse = planner.plan_fft_inverse(N);
    let mut expected = vec![0.0; actual.len()];
    let source_hops = x.len() / H;
    let attenuation = (-(H as f64) / (0.005 * 48000.0)).exp();
    let mut gain = 1.0;
    let mut frame = 0;
    let mut origin = -((N - H) as isize);
    while origin < x.len() as isize {
        if frame < source_hops {
            gain = 0.1 + attenuation * (gain - 0.1);
        }
        let mut spectrum: Vec<_> = (0..N)
            .map(|i| {
                let at = origin + i as isize;
                let value = if at >= 0 && (at as usize) < x.len() {
                    f64::from(x[at as usize])
                } else {
                    0.0
                };
                Complex::new(value * window[i], 0.0)
            })
            .collect();
        forward.process(&mut spectrum);
        for (k, bin) in spectrum.iter_mut().enumerate() {
            if k != 0 && k != N / 2 {
                *bin *= gain;
            }
        }
        inverse.process(&mut spectrum);
        for i in 0..N {
            let source_time = origin + i as isize;
            let output_time = source_time + N as isize;
            if source_time >= 0 && (output_time as usize) < expected.len() {
                expected[output_time as usize] += spectrum[i].re * window[i] / (1.5 * N as f64);
            }
        }
        origin += H as isize;
        frame += 1;
    }
    let error = actual
        .iter()
        .zip(&expected)
        .map(|(a, b)| (f64::from(*a) - b).abs())
        .fold(0.0, f64::max);
    assert!(error < 2e-7, "independent complete WOLA error {error}");
    let input_energy = x.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
    let output_energy = actual.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
    assert!(output_energy <= input_energy * (1.0 + 1e-5));
}
