//! Saturation's preferred oversampling must suppress folded energy in real hosts.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, create_plugin};
use std::f64::consts::TAU;

const N: usize = 4096;
fn render(mode: &str, rate: u32, oversampling: &str, bin: usize, parts: &[usize]) -> Vec<f32> {
    let preset = serde_json::json!({"mode":mode,"drive":12.0,"tone":2.0,
        "exciter_freq":3000.0,"mix":1.0,"oversampling":oversampling,
        "use_adaa":false,"dc_blocker_enabled":false,"dynamic_amount":0.0});
    let mut host = DawHost::new(1, rate);
    host.set_max_block_frames(N).unwrap();
    host.add_plugin(create_plugin("saturation", &preset, 1, rate).unwrap())
        .unwrap();
    host.build().unwrap();
    if oversampling == "Off" {
        assert_eq!(host.total_latency_samples(), 0);
    } else {
        assert!(host.total_latency_samples() > 0);
    }
    let source: Vec<f32> = (0..N * 4)
        .map(|n| (0.7 * (TAU * bin as f64 * n as f64 / N as f64).sin()) as f32)
        .collect();
    let mut output = vec![0.0; source.len()];
    let mut pos = 0;
    let mut block = 0;
    while pos < source.len() {
        let n = parts[block % parts.len()].min(source.len() - pos);
        host.process(&source[pos..pos + n], &mut output[pos..pos + n])
            .unwrap();
        pos += n;
        block += 1;
    }
    output
}
fn alias_ratio(x: &[f32], bin: usize) -> f64 {
    // Project DC and every harmonic below the original Nyquist limit. The
    // remaining coherent-period energy is folded distortion, not passband
    // phase shift or attenuation from the host's anti-alias filters.
    let total = x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64;
    let mean = x.iter().map(|&v| f64::from(v)).sum::<f64>() / x.len() as f64;
    let mut legitimate = mean * mean;
    let mut fundamental = 0.0;
    for h in 1..=N / 2 / bin {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &v) in x.iter().enumerate() {
            let angle = TAU * (h * bin) as f64 * n as f64 / N as f64;
            re += f64::from(v) * angle.cos();
            im += f64::from(v) * angle.sin();
        }
        let power = 2.0 * (re * re + im * im) / (N * N) as f64;
        legitimate += power;
        if h == 1 {
            fundamental = power;
        }
    }
    assert!(
        fundamental > 1e-5,
        "a muted host must not pass the alias test"
    );
    (total - legitimate).max(0.0) / fundamental
}
#[test]
fn preferred_oversampling_reduces_folded_distortion_for_every_curve() {
    for mode in ["Soft Clip", "Tube", "Tape", "Exciter", "Asymmetric"] {
        for rate in [44_100, 48_000, 96_000] {
            for bin in [901, 1601] {
                let direct = render(mode, rate, "Off", bin, &[N]);
                let baseline = alias_ratio(&direct[3 * N..], bin);
                for factor in ["2x", "4x"] {
                    let oversampled = render(mode, rate, factor, bin, &[N]);
                    let measured = alias_ratio(&oversampled[3 * N..], bin);
                    assert!(
                        measured < baseline * 0.5,
                        "{mode} {rate} bin={bin} {factor}: alias/fundamental {measured:e} versus native {baseline:e}"
                    );
                }
            }
        }
    }
}
#[test]
fn oversampled_factory_paths_have_callback_independent_cold_history() {
    for mode in ["Soft Clip", "Tube", "Tape", "Exciter", "Asymmetric"] {
        for factor in ["Off", "2x", "4x"] {
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let a = render(mode, rate, factor, 311, &[N]);
                let b = render(mode, rate, factor, 311, &[1, 31, 257]);
                let peak = a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    peak < 2e-6,
                    "{mode} {rate} {factor}: callback-dependent audio {peak:e}"
                );
            }
        }
    }
}
