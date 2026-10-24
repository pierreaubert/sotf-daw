//! Compare existing noise estimates with an independent analytic PSD baseline.

// Rust guideline compliant 2026-02-21
use crate::{DenoiserPlugin, DenoiserPluginParams};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};

fn amplitude(frame: usize, channel: usize) -> f32 {
    if channel == 1 {
        0.01
    } else if frame > 12_000 && frame < 36_000 {
        0.1
    } else {
        0.001
    }
}

fn noise_source(seed: u32) -> Vec<f32> {
    let mut state = seed;
    let mut source = Vec::with_capacity(96_000);
    for frame in 0..48_000 {
        for channel in 0..2 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let uniform = (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32;
            source.push(uniform * amplitude(frame, channel));
        }
    }
    source
}

// This first moment needs only sample variances and the declared analysis
// window. It does not read production windows, FFTs, stored powers or gains.
fn expected_psd(end: usize, size: usize, channel: usize) -> f64 {
    (0..size)
        .map(|n| {
            let source = end as isize - size as isize + n as isize;
            if !(0..48_000).contains(&source) {
                return 0.0;
            }
            let a = f64::from(amplitude(source as usize, channel));
            let w = (std::f64::consts::PI * n as f64 / size as f64).sin();
            a * a / 3.0 * w * w
        })
        .sum()
}

#[test]
fn analytic_uniform_noise_bias_trace_preserves_known_source_variance() {
    let times = [0.2_f64, 0.3, 0.45, 0.6, 0.9];
    let mut records = 0;
    for ordinal in 1_u32..=64 {
        // Fixed ordinal permutation, declared before measuring any result.
        let seed = ordinal.wrapping_mul(0x9E37_79B9);
        let source = noise_source(seed);
        for low_latency in [false, true] {
            let mut plugin = DenoiserPlugin::from_params(
                2,
                DenoiserPluginParams {
                    low_latency,
                    spatial_denoise: false,
                    polyphonic_detection: false,
                    psychoacoustic_masking: false,
                    ..Default::default()
                },
            );
            plugin.initialize(48_000.0).unwrap();
            let size = plugin.config.fft_size;
            let hop = plugin.config.hop_size;
            let first = (100.0 * size as f64 / 48_000.0).ceil() as usize;
            let last = (4000.0 * size as f64 / 48_000.0).floor() as usize;
            let bins = last - first + 1;
            let mut end = 0;
            let mut next_time = 0;
            for input in source.chunks(hop * 2) {
                let mut block = input.to_vec();
                let frames = block.len() / 2;
                plugin
                    .process_in_place(&mut block, &ProcessContext::new(48_000, frames))
                    .unwrap();
                end += frames;
                let center = end.saturating_sub(hop) as f64 / 48_000.0;
                if frames != hop || next_time >= times.len() || center < times[next_time] {
                    continue;
                }
                assert!(plugin.mcra.frame_counter[0] >= plugin.mcra.bootstrap_frames);
                let requested = times[next_time];
                next_time += 1;
                for channel in 0..2 {
                    let expected = expected_psd(end, size, channel);
                    assert!(expected > 0.0);
                    let observed: f64 = (first..=last)
                        .map(|k| f64::from(plugin.decision_directed.prev_power[channel][k]))
                        .sum();
                    let estimated: f64 = (first..=last)
                        .map(|k| f64::from(plugin.mcra.noise_psd[channel][k]))
                        .sum();
                    let presence: f64 = (first..=last)
                        .map(|k| f64::from(plugin.mcra.speech_presence[channel][k]))
                        .sum();
                    let weighted: f64 = (first..=last)
                        .map(|k| {
                            f64::from(plugin.mcra.speech_presence[channel][k])
                                * f64::from(plugin.decision_directed.prev_power[channel][k])
                        })
                        .sum();
                    let mut ratios: Vec<f64> = (first..=last)
                        .map(|k| f64::from(plugin.mcra.noise_psd[channel][k]) / expected)
                        .collect();
                    ratios.sort_by(f64::total_cmp);
                    assert!(ratios.iter().all(|v| v.is_finite() && *v >= 0.0));
                    assert!((first..=last).all(|k|(0.0..=1.0).contains(&plugin.mcra.speech_presence[channel][k])));
                    println!(
                        "NOISE_BIAS {}",
                        serde_json::json!({"ordinal":ordinal,"seed":seed,"low_latency":low_latency,"fft_size":size,"channel":channel,"condition":if channel==0 {"pure_uniform_step"} else {"stationary_uniform"},"requested_source_time":requested,"source_window_center":center,"window_end_frame":end,"bins":bins,"expected_psd":expected,"observed_mean_psd":observed/bins as f64,"estimated_mean_psd":estimated/bins as f64,"observed_mean_ratio":observed/(bins as f64*expected),"estimated_mean_ratio":estimated/(bins as f64*expected),"estimated_ratio_order_stat_p10":ratios[bins/10],"estimated_ratio_order_stat_p50":ratios[bins/2],"estimated_ratio_order_stat_p90":ratios[bins*9/10],"mean_p":presence/bins as f64,"power_weighted_p":weighted/observed.max(1e-30)})
                    );
                    records += 1;
                }
            }
            assert_eq!(next_time, times.len());
        }
    }
    assert_eq!(records, 1280);
    println!(
        "analytic_noise_bias_summary seeds=64 fft_modes=2 channels=2 times=5 records={records} population_acceptance=false"
    );
}
