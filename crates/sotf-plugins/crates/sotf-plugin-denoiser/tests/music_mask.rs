//! Full-stream QA-only musical mask fallback, endpoints and lifecycle contracts.

#![cfg(feature = "qa")]

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::music_mask::MusicMask;
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MODEL: &[u8] = include_bytes!("../../../models/music-noise-mask/model.synthetic.json");
const SHA: &str = "ab1a996543de22b77b38ea57363a57d23a65319161ba81f4ecce1d5b280fe0a1";
fn model() -> MusicMask {
    MusicMask::from_json(MODEL, SHA).unwrap()
}
fn plugin(channels: usize, rate: u32, low_latency: bool, zero: bool) -> DenoiserPlugin {
    let mut p = DenoiserPlugin::from_params(
        channels,
        DenoiserPluginParams {
            low_latency,
            reduction_db: if zero { 0.0 } else { 20.0 },
            ..Default::default()
        },
    );
    p.initialize(f64::from(rate)).unwrap();
    p
}
fn source(channels: usize) -> Vec<f32> {
    (0..8197 * channels)
        .map(|i| ((i * 13 % 97) as f32 - 48.0) / 1024.0)
        .collect()
}
fn render(
    p: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    rate: u32,
    callback: usize,
) -> Vec<f32> {
    let frames = input.len() / channels;
    let mut result = Vec::with_capacity(input.len());
    for start in (0..frames).step_by(callback) {
        let count = (frames - start).min(callback);
        let mut block = input[start * channels..(start + count) * channels].to_vec();
        assert_no_allocs_or_deallocs("music-mask callback", || {
            assert_eq!(
                p.process_in_place(&mut block, &ProcessContext::new(rate, count))
                    .unwrap(),
                count
            )
        });
        result.extend_from_slice(&block);
    }
    result
}

#[test]
fn missing_disabled_and_unsupported_modes_keep_exact_classical_audio() {
    for low in [false, true] {
        for channels in [1, 2, 4] {
            let x = source(channels);
            let expected = render(
                &mut plugin(channels, 48000, low, false),
                &x,
                channels,
                48000,
                193,
            );
            for present in [false, true] {
                let mut p = plugin(channels, 48000, low, false);
                p.install_qa_music_mask(present.then(model)).unwrap();
                p.set_qa_music_mask_enabled(!present).unwrap();
                assert_eq!(render(&mut p, &x, channels, 48000, 17), expected);
            }
            if channels > 2 {
                let mut p = plugin(channels, 48000, low, false);
                p.install_qa_music_mask(Some(model())).unwrap();
                p.set_qa_music_mask_enabled(true).unwrap();
                assert_eq!(render(&mut p, &x, channels, 48000, 193), expected);
                assert!(p.qa_music_mask_counters().unwrap().unavailable_bins > 0);
            }
        }
    }
}

#[test]
fn zero_depth_is_exact_delayed_input_with_active_neural_history() {
    for rate in [44100, 48000, 96000] {
        for low in [false, true] {
            let mut p = plugin(2, rate, low, true);
            p.install_qa_music_mask(Some(model())).unwrap();
            p.set_qa_music_mask_enabled(true).unwrap();
            let input = source(2);
            let latency = p.latency_samples();
            let mut x = input.clone();
            x.resize(x.len() + 2 * latency, 0.0);
            let y = render(&mut p, &x, 2, rate, 193);
            assert!(y[..latency * 2].iter().all(|&v| v == 0.0));
            for (a, b) in y[latency * 2..latency * 2 + input.len()].iter().zip(&input) {
                assert!((a - b).abs() < 3e-8);
            }
            assert!(p.qa_music_mask_counters().unwrap().neural_bins > 0);
        }
    }
}

#[test]
fn callback_reset_and_frozen_drain_are_deterministic_without_heap_work() {
    for low in [false, true] {
        let mut a = plugin(2, 48000, low, false);
        a.install_qa_music_mask(Some(model())).unwrap();
        a.set_qa_music_mask_enabled(true).unwrap();
        let mut b = plugin(2, 48000, low, false);
        b.install_qa_music_mask(Some(model())).unwrap();
        b.set_qa_music_mask_enabled(true).unwrap();
        let x = source(2);
        let y = render(&mut a, &x, 2, 48000, 17);
        assert_eq!(y, render(&mut b, &x, 2, 48000, 193));
        assert!(y.iter().all(|v| v.is_finite()));
        let counts = a.qa_music_mask_counters();
        let bound = a.drain_call_bound().unwrap().get();
        let mut buffer = vec![0.0; a.drain_output_frames_max() * 2];
        let mut complete = false;
        for _ in 0..bound {
            assert_no_allocs_or_deallocs("music-mask drain", || {
                complete = a
                    .drain(&mut buffer, &ProcessContext::new(48000, 257))
                    .unwrap()
                    .complete
            });
            if complete {
                break;
            }
        }
        assert!(complete);
        assert_eq!(a.qa_music_mask_counters(), counts);
        assert_no_allocs_or_deallocs("music-mask populated reset", || a.reset());
        assert_eq!(a.qa_music_mask_counters().unwrap(), Default::default());
        assert_eq!(render(&mut a, &x, 2, 48000, 193), y);
        a.set_qa_music_mask_enabled(false).unwrap();
        let mut native = plugin(2, 48000, low, false);
        render(&mut native, &x, 2, 48000, 193);
        let actual = render(&mut a, &x, 2, 48000, 193);
        let expected = render(&mut native, &x, 2, 48000, 193);
        let settled = 2 * a.latency_samples() * 2;
        assert_eq!(&actual[settled..], &expected[settled..]);
    }
}

#[test]
fn rejected_rate_and_live_replacement_preserve_audio_state() {
    let mut a = plugin(2, 48000, true, false);
    a.install_qa_music_mask(Some(model())).unwrap();
    a.set_qa_music_mask_enabled(true).unwrap();
    let mut b = plugin(2, 48000, true, false);
    b.install_qa_music_mask(Some(model())).unwrap();
    b.set_qa_music_mask_enabled(true).unwrap();
    let mut invalid = [0.01; 34];
    assert!(
        a.process_in_place(&mut invalid, &ProcessContext::new(96000, 17))
            .is_err()
    );
    assert_eq!(invalid, [0.01; 34]);
    let x = source(2);
    assert_eq!(
        render(&mut a, &x, 2, 48000, 193),
        render(&mut b, &x, 2, 48000, 193)
    );
    assert!(a.install_qa_music_mask(Some(model())).is_err());
    assert_eq!(
        render(&mut a, &x, 2, 48000, 193),
        render(&mut b, &x, 2, 48000, 193)
    );
}

#[test]
#[ignore = "Local deadline evidence; run alone with one test thread"]
fn measure_music_mask_callback_budget() {
    use std::time::Instant;
    let mut rows = Vec::new();
    let callback = 193;
    for rate in [44100, 48000, 96000] {
        for low in [false, true] {
            for mode in ["production", "disabled", "neural"] {
                let mut p = plugin(2, rate, low, false);
                if mode != "production" {
                    p.install_qa_music_mask(Some(model())).unwrap();
                    p.set_qa_music_mask_enabled(mode == "neural").unwrap();
                }
                let input = source(2);
                let mut block = vec![0.0; callback * 2];
                let context = ProcessContext::new(rate, callback);
                for _ in 0..64 {
                    block.copy_from_slice(&input[..callback * 2]);
                    p.process_in_place(&mut block, &context).unwrap();
                }
                let mut times = Vec::with_capacity(200);
                for _ in 0..200 {
                    block.copy_from_slice(&input[..callback * 2]);
                    let start = Instant::now();
                    p.process_in_place(&mut block, &context).unwrap();
                    times.push(start.elapsed().as_secs_f64());
                }
                times.sort_by(f64::total_cmp);
                let deadline = callback as f64 / f64::from(rate);
                rows.push(serde_json::json!({"sample_rate":rate,"fft_size":p.latency_samples(),"mode":mode,"channels":2,
            "callback_frames":callback,"callbacks":200,"warmup_callbacks":64,"p50_seconds":times[100],"p95_seconds":times[190],"p99_seconds":times[198],"max_seconds":times[199],
            "callback_deadline_seconds":deadline,"declared_local_budget_seconds":0.8*deadline,"local_budget_pass":times[199]<=0.8*deadline,
            "latency_samples":p.latency_samples(),"added_model_lookahead_samples":0,"counts":p.qa_music_mask_counters()}));
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"cases":rows,"deployment_admitted":false,"scope":"Local native stereo callback timings with active classical estimation. Disabled QA retains warm neural history and inference. Each budget is80 percent of the callback duration, fixed before measurement. This is not portable platform or complete memory admission."})
    );
}
