//! Separate learned synthesis must preserve native compression history and fallback.
#![cfg(feature = "qa")]
// Rust guideline compliant 2026-02-21
use sha2::{Digest, Sha256};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_spectral_compressor::restoration::DenoisingMask;
use sotf_plugin_spectral_compressor::{SpectralCompressorPlugin, SpectralCompressorPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn model(bias: f64) -> DenoisingMask {
    let value = serde_json::json!({"format":"sotf-expander-denoising-mask-v1","weights":vec![[0.0;8];6],
        "bias":vec![0.0;8],"output_weights":vec![0.0;8],"output_bias":bias,
        "provenance":"project-generated music/noise; GPL-3.0-or-later; no external audio","deployment_admitted":false});
    let bytes = serde_json::to_vec(&value).unwrap();
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect();
    DenoisingMask::from_json(&bytes, &sha).unwrap()
}

fn make(channels: usize, rate: u32, target: usize, adaptive: bool) -> SpectralCompressorPlugin {
    let params = SpectralCompressorPluginParams {
        fft_size_index: 0,
        threshold_db: -40.0,
        ratio: 4.0,
        target_mode: target,
        adaptive_threshold: adaptive,
        channel_link: 1.0,
        mix: 1.0,
        ..Default::default()
    };
    let mut p = SpectralCompressorPlugin::try_from_params(channels, params).unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}

fn input(channels: usize) -> Vec<f32> {
    (0..5101 * channels)
        .map(|i| ((i * 13 % 97) as f32 - 48.0) / 1024.0)
        .collect()
}

fn render(
    p: &mut SpectralCompressorPlugin,
    input: &[f32],
    channels: usize,
    rate: u32,
    block: usize,
    capacity: usize,
) -> Vec<f32> {
    let mut out = input.to_vec();
    for x in out.chunks_mut(block * channels) {
        let context = ProcessContext::new(rate, x.len() / channels);
        assert_no_allocs_or_deallocs("restoration process", || {
            p.process_in_place(x, &context).unwrap();
        });
    }
    let counters = p.restoration_counters();
    let mut tail = vec![0.0; capacity * channels];
    let mut complete = false;
    for _ in 0..4096 {
        let mut result = Ok(sotf_host::plugin::PluginDrainResult::COMPLETE);
        assert_no_allocs_or_deallocs("restoration drain", || {
            result = p.drain(&mut tail, &ProcessContext::new(rate, 0))
        });
        let done = result.unwrap();
        out.extend_from_slice(&tail[..done.frames * channels]);
        if done.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(
        p.restoration_counters(),
        counters,
        "EOF must not classify padding"
    );
    let frames = input.len() / channels;
    assert_eq!(
        out.len() / channels,
        frames + 1792 + (256 - frames % 256) % 256
    );
    out
}

#[test]
fn missing_uncertain_and_unsupported_clock_keep_exact_classical_output() {
    for rate in [44100, 48000, 96000, 192000] {
        for channels in [1, 2] {
            let x = input(channels);
            let expected = render(
                &mut make(channels, rate, 0, false),
                &x,
                channels,
                rate,
                193,
                256,
            );
            for network in [None, Some(model(0.0))] {
                let mut candidate = make(channels, rate, 0, false);
                candidate.configure_restoration(network).unwrap();
                assert_eq!(render(&mut candidate, &x, channels, rate, 17, 1), expected);
                assert_eq!(candidate.restoration_counters().unwrap().selected_bins, 0);
            }
            if rate == 192000 {
                let mut candidate = make(channels, rate, 0, false);
                candidate.configure_restoration(Some(model(-8.0))).unwrap();
                assert_eq!(render(&mut candidate, &x, channels, rate, 17, 1), expected);
            }
        }
    }
}

#[test]
fn learned_synthesis_preserves_classical_histories_latency_reset_and_energy_bound() {
    for rate in [44100, 48000, 96000] {
        for channels in [1, 2] {
            for target in [0, 1, 2] {
                let x = input(channels);
                let mut candidate = make(channels, rate, target, true);
                candidate.configure_restoration(Some(model(-8.0))).unwrap();
                let learned = render(&mut candidate, &x, channels, rate, 17, 1);
                let mut classical = make(channels, rate, target, true);
                let original = render(&mut classical, &x, channels, rate, 193, 256);
                assert_ne!(learned, original);
                assert_eq!(
                    candidate.classical_restoration_state().collect::<Vec<_>>(),
                    classical.classical_restoration_state().collect::<Vec<_>>()
                );
                let count = candidate.restoration_counters().unwrap();
                assert!(count.selected_bins > 0);
                assert_eq!(
                    count.selected_bins + count.fallback_bins,
                    count.source_hops * 513
                );
                assert_eq!(candidate.latency_samples(), 1024);
                let energy = |s: &[f32]| s.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
                assert!(energy(&learned) <= energy(&x) * (1.0 + 1e-5));
                assert_no_allocs_or_deallocs("restoration reset", || candidate.reset());
                assert_eq!(render(&mut candidate, &x, channels, rate, 193, 17), learned);
                candidate.initialize(f64::from(rate)).unwrap();
                assert_eq!(render(&mut candidate, &x, channels, rate, 137, 3), learned);
            }
        }
    }
}

#[test]
fn configuration_and_invalid_source_validation_are_transactional() {
    let mut unprepared = SpectralCompressorPlugin::try_from_params(
        1,
        SpectralCompressorPluginParams {
            fft_size_index: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(unprepared.configure_restoration(None).is_err());
    let mut wide = make(3, 48000, 0, false);
    assert!(wide.configure_restoration(None).is_err());
    let mut long = SpectralCompressorPlugin::try_from_params(
        1,
        SpectralCompressorPluginParams {
            fft_size_index: 1,
            ..Default::default()
        },
    )
    .unwrap();
    long.initialize(48000.0).unwrap();
    assert!(long.configure_restoration(None).is_err());
    let mut a = make(1, 48000, 0, false);
    a.configure_restoration(Some(model(-8.0))).unwrap();
    let mut b = make(1, 48000, 0, false);
    b.configure_restoration(Some(model(-8.0))).unwrap();
    let mut invalid = [f32::NAN];
    assert!(
        a.process_in_place(&mut invalid, &ProcessContext::new(48000, 1))
            .is_err()
    );
    let x = input(1);
    assert_eq!(
        render(&mut a, &x, 1, 48000, 17, 1),
        render(&mut b, &x, 1, 48000, 193, 256)
    );
    assert!(a.configure_restoration(None).is_err());
}

#[test]
#[ignore = "Local callback evidence; run alone with one test thread"]
fn measure_restoration_callback_budget() {
    use std::time::Instant;
    let bytes = include_bytes!("../../../models/expander-denoising-mask/model.synthetic.json");
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/expander-denoising-mask/parity.synthetic.json"
    ))
    .unwrap();
    let sha = fixture["model_sha256"].as_str().unwrap();
    let mut cases = Vec::new();
    const CALLBACK: usize = 193;
    for rate in [44100, 48000, 96000] {
        for channels in [1, 2] {
            for mode in ["classical", "missing_model", "learned"] {
                let params = SpectralCompressorPluginParams {
                    fft_size_index: 0,
                    threshold_db: -40.0,
                    ratio: 4.0,
                    attack_ms: 5.0,
                    release_ms: 80.0,
                    knee_db: 6.0,
                    spectral_smoothing: 0.3,
                    mix: 1.0,
                    channel_link: 1.0,
                    ..Default::default()
                };
                let mut plugin =
                    SpectralCompressorPlugin::try_from_params(channels, params).unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                if mode != "classical" {
                    let model = if mode == "learned" {
                        Some(DenoisingMask::from_json(bytes, sha).unwrap())
                    } else {
                        None
                    };
                    plugin.configure_restoration(model).unwrap();
                }
                let input: Vec<_> = (0..CALLBACK * channels)
                    .map(|i| ((i * 13 % 97) as f32 - 48.0) / 1024.0)
                    .collect();
                let mut buffer = input.clone();
                let context = ProcessContext::new(rate, CALLBACK);
                for _ in 0..16 {
                    buffer.copy_from_slice(&input);
                    plugin.process_in_place(&mut buffer, &context).unwrap();
                }
                let mut times = Vec::with_capacity(300);
                for _ in 0..300 {
                    buffer.copy_from_slice(&input);
                    let start = Instant::now();
                    plugin.process_in_place(&mut buffer, &context).unwrap();
                    times.push(start.elapsed().as_secs_f64());
                }
                times.sort_by(f64::total_cmp);
                let deadline = CALLBACK as f64 / f64::from(rate);
                cases.push(serde_json::json!({"mode":mode,"sample_rate":rate,"channels":channels,"callback_frames":CALLBACK,"callbacks":300,
                    "p50_seconds":times[150],"p95_seconds":times[285],"p99_seconds":times[297],"maximum_seconds":times[299],
                    "deadline_seconds":deadline,"declared_local_budget_seconds":0.8*deadline,"local_budget_pass":times[299]<=0.8*deadline,
                    "latency_samples":plugin.latency_samples(),"added_model_lookahead_samples":0,"selection_counters":plugin.restoration_counters()}));
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"cases":cases,"deployment_admitted":false,"scope":"Local native callback wall time with fixed 80% budget, 16 warmups and 300 callbacks. This does not prove cross-platform host deadlines or full heap memory."})
    );
}
