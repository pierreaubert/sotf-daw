//! Independent output-meter oracles for native and oversampled paths.
//!
//! Output peak uses direct f32 max-abs over emitted samples. Output true peak
//! uses an independent f64 Hann-sinc reconstruction, never production helpers.
//! Bounds are fixed before measurement: 0.1 dB for meters (matching the
//! accepted Pro-L2 meter-difference note and existing telemetry tolerance).

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterData, LimiterPlugin, LimiterPluginParams};
use std::f64::consts::{PI, TAU};
use std::sync::Arc;

const METER_TOLERANCE_DB: f64 = 0.1;
const RATES: [u32; 4] = [44_100, 48_000, 96_000, 192_000];

fn make(
    rate: u32,
    channels: usize,
    oversampling: usize,
    threshold_db: f32,
    lookahead_ms: f32,
    isp: bool,
    mix: f32,
) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": threshold_db,
        "release_ms": 50.0,
        "lookahead_ms": lookahead_ms,
        "soft": false,
        "true_peak": isp,
        "isp_mode": isp,
        "dual_release": false,
        "mix": mix,
        "feed_forward": false,
        "link_amount": 1.0,
        "oversampling": oversampling,
    }))
    .unwrap();
    let mut plugin = LimiterPlugin::from_params(channels, params);
    plugin.initialize(rate).unwrap();
    plugin
}

fn meter(plugin: &LimiterPlugin) -> Arc<LimiterData> {
    plugin
        .get_data()
        .unwrap()
        .downcast::<LimiterData>()
        .unwrap()
}

// Independent f64 Hann-sinc reconstruction peak for one channel stream.
fn reconstructed_peak(samples: &[f32], rate: u32) -> f64 {
    let factor = if rate < 96_000 {
        4
    } else if rate < 192_000 {
        2
    } else {
        return samples
            .iter()
            .map(|s| f64::from(*s).abs())
            .fold(0.0, f64::max);
    };
    let kernels: [[f64; 25]; 4] = std::array::from_fn(|phase| {
        std::array::from_fn(|past| {
            let j = factor * past + phase;
            if j > 48 {
                return 0.0;
            }
            let offset = j as f64 - 24.0;
            let angle = PI * offset / factor as f64;
            let sinc = if offset == 0.0 {
                1.0
            } else {
                angle.sin() / angle
            };
            sinc * 0.5 * (1.0 - (TAU * j as f64 / 48.0).cos())
        })
    });
    let mut history = [0.0; 25];
    let mut peak: f64 = 0.0;
    for sample in samples
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0.0, 25))
    {
        history.copy_within(..24, 1);
        history[0] = f64::from(sample);
        for kernel in &kernels[..factor] {
            let value: f64 = history.iter().zip(kernel).map(|(x, h)| x * h).sum();
            peak = peak.max(value.abs());
        }
    }
    peak
}

fn db(linear: f64) -> f64 {
    20.0 * linear.max(1.0e-12).log10()
}

// Process `frames` of deterministic dense audio, returning emitted samples and
// per-publication output-peak observations checked against the meter.
#[allow(clippy::too_many_arguments)]
fn render_and_check_output_peak(
    rate: u32,
    channels: usize,
    oversampling: usize,
    isp: bool,
    mix: f32,
    worst: &mut f64,
    worst_case: &mut String,
) {
    let lookahead_ms = if isp { 5.0 } else { 0.0 };
    let mut plugin = make(rate, channels, oversampling, -6.0, lookahead_ms, isp, mix);
    let interval = (rate as usize / 10).max(1);
    let frames = interval * 3 + 17;
    let mut input = vec![0.0; frames * channels];
    for frame in 0..frames {
        let t = frame as f64 / f64::from(rate);
        let sample = (0.9 * (TAU * 1000.0 * t).sin()
            + 0.4 * (TAU * 3333.0 * t + 0.7).sin()) as f32;
        for ch in 0..channels {
            let scale = if ch % 2 == 0 { 1.0 } else { 0.5 };
            input[frame * channels + ch] = sample * scale;
        }
    }
    // Dense burst near the second interval to stress peak tracking.
    for frame in interval..interval + 13 {
        for ch in 0..channels {
            input[frame * channels + ch] = if frame % 2 == 0 { 1.2 } else { -1.2 };
        }
    }
    let mut output = input.clone();
    let mut position = 0;
    let mut call = 0;
    let mut interval_peak = 0.0f64;
    let mut interval_frames = 0usize;
    let mut publications = 0;
    while position < frames {
        let count = [1, 127, 257][call % 3].min(frames - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + count) * channels],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        for frame in position..position + count {
            for ch in 0..channels {
                interval_peak = interval_peak.max(f64::from(output[frame * channels + ch]).abs());
            }
            interval_frames += 1;
            if interval_frames >= interval {
                publications += 1;
                let data = meter(&plugin);
                let expected = if interval_peak <= 1.0e-10 {
                    -100.0
                } else {
                    20.0 * interval_peak.log10()
                };
                let error = (f64::from(data.output_peak_db) - expected).abs();
                if error > *worst {
                    *worst = error;
                    *worst_case = format!(
                        "rate={rate} ch={channels} os={oversampling} isp={isp} mix={mix} pub={publications}"
                    );
                }
                assert!(
                    error <= METER_TOLERANCE_DB,
                    "output peak meter error {error:.6} dB exceeds {METER_TOLERANCE_DB} dB \
                     (expected {expected:.4}, got {:.4}) at {worst_case}",
                    f64::from(data.output_peak_db),
                );
                // Legacy input-peak field must remain populated and sane.
                assert!(data.peak_db.is_finite());
                interval_peak = 0.0;
                interval_frames = 0;
            }
        }
        position += count;
        call += 1;
    }
    assert!(publications >= 3, "expected 3+ publications, got {publications}");
}

#[test]
fn output_peak_matches_emitted_samples_across_paths() {
    let mut worst = 0.0;
    let mut worst_case = String::from("none");
    for rate in RATES {
        for channels in [1, 2, 6] {
            for oversampling in [0, 1, 2] {
                for isp in [false, true] {
                    // ISP requires hard mode, full wet, and covering lookahead.
                    let mixes: &[f32] = if isp { &[1.0] } else { &[1.0, 0.5] };
                    for &mix in mixes {
                        render_and_check_output_peak(
                            rate,
                            channels,
                            oversampling,
                            isp,
                            mix,
                            &mut worst,
                            &mut worst_case,
                        );
                    }
                }
            }
        }
    }
    eprintln!("worst output-peak meter error: {worst:.6} dB at {worst_case}");
    assert!(worst <= METER_TOLERANCE_DB);
}

#[test]
fn output_true_peak_matches_independent_reconstruction() {
    let mut worst = 0.0;
    let mut worst_case = String::from("none");
    // Check final-publication output TP against an independent oracle over the
    // emitted stream. Oracle runs per channel over the full render; the meter
    // reports per-interval maxima, so compare the last full interval only.
    for rate in RATES {
        for channels in [1, 2] {
            for oversampling in [0, 2] {
                let mut plugin = make(rate, channels, oversampling, -6.0, 5.0, true, 1.0);
                let interval = (rate as usize / 10).max(1);
                let frames = interval * 2;
                let mut input = vec![0.0; frames * channels];
                for frame in 0..frames {
                    let t = frame as f64 / f64::from(rate);
                    let sample =
                        (1.1 * (TAU * 7000.0 * t).sin() + 0.3 * (TAU * 11000.0 * t).sin()) as f32;
                    for ch in 0..channels {
                        input[frame * channels + ch] = sample;
                    }
                }
                let mut output = input.clone();
                plugin
                    .process_in_place(&mut output, &ProcessContext::new(rate, frames))
                    .unwrap();
                let data = meter(&plugin);
                assert_eq!(data.output_isp_dbtp.len(), channels);
                // Last interval emitted samples per channel.
                for ch in 0..channels {
                    let emitted: Vec<f32> = (interval..frames)
                        .map(|frame| output[frame * channels + ch])
                        .collect();
                    let expected = db(reconstructed_peak(&emitted, rate));
                    // Meter floor for near-silence is -120 dBTP.
                    let expected = if expected < -119.0 { -120.0 } else { expected };
                    let got = f64::from(data.output_isp_dbtp[ch]);
                    let error = (got - expected).abs();
                    if error > worst {
                        worst = error;
                        worst_case =
                            format!("rate={rate} ch={channels}/{ch} os={oversampling}");
                    }
                    assert!(
                        error <= METER_TOLERANCE_DB,
                        "output TP error {error:.4} dB exceeds bound (expected {expected:.4}, got {got:.4}) at {worst_case}"
                    );
                }
                // Oversampled legacy field already reports output; new field mirrors it.
                if oversampling != 0 {
                    assert_eq!(data.isp_dbtp, data.output_isp_dbtp);
                }
            }
        }
    }
    eprintln!("worst output-TP meter error: {worst:.6} dB at {worst_case}");
    assert!(worst <= METER_TOLERANCE_DB);
}

#[test]
fn disabled_true_peak_reports_silence_floor_and_reset_clears_output_meters() {
    for oversampling in [0, 1, 2] {
        let rate = 48_000;
        let mut plugin = make(rate, 2, oversampling, -6.0, 5.0, false, 1.0);
        let interval = (rate as usize / 10).max(1);
        let mut loud = vec![0.9; interval * 2 * 2];
        plugin
            .process_in_place(&mut loud, &ProcessContext::new(rate, interval * 2))
            .unwrap();
        let data = meter(&plugin);
        assert_eq!(data.output_isp_dbtp, vec![-120.0; 2]);
        // Output sample peak still tracks loud audio while TP is disabled.
        assert!(data.output_peak_db > -20.0, "got {}", data.output_peak_db);
        // Reset then silence publishes the silence floors.
        plugin.reset();
        let mut silence = vec![0.0; interval * 2 * 2];
        plugin
            .process_in_place(&mut silence, &ProcessContext::new(rate, interval * 2))
            .unwrap();
        let data = meter(&plugin);
        assert_eq!(data.output_isp_dbtp, vec![-120.0; 2]);
        // After reset, delayed program is cleared; silence stays silent.
        assert!(
            (f64::from(data.output_peak_db) + 100.0).abs() < 1.0,
            "silence output peak {}, expected near -100",
            data.output_peak_db
        );
    }
}

#[test]
fn threshold_automation_keeps_output_meters_consistent() {
    let rate = 48_000;
    let channels = 2;
    let mut plugin = make(rate, channels, 0, -6.0, 5.0, true, 1.0);
    let interval = (rate as usize / 10).max(1);
    let frames = interval * 4;
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let t = frame as f64 / f64::from(rate);
            let sample = (0.8 * (TAU * 2000.0 * t).sin()) as f32;
            [sample, sample * 0.5]
        })
        .collect();
    let mut output = input.clone();
    let mut position = 0;
    while position < frames {
        if position == interval {
            plugin
                .parametric_set_parameter(
                    ParameterId::from("threshold"),
                    ParameterValue::Float(-18.0),
                )
                .unwrap();
        }
        // Split the callback at the automation event so a 257-frame quantum
        // cannot skip the requested sample position.
        let next_boundary = if position < interval {
            interval
        } else {
            frames
        };
        let count = 257.min(next_boundary - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + count) * channels],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
    }
    let data = meter(&plugin);
    // Final interval is limited to the tighter -18 dB ceiling (plus smoothing).
    let tail: Vec<f32> = output[(frames - interval) * channels..].to_vec();
    let measured = tail
        .iter()
        .map(|s| f64::from(*s).abs())
        .fold(0.0, f64::max);
    let measured_db = 20.0 * measured.max(1.0e-10).log10();
    assert!(
        measured_db <= -18.0 + 0.5,
        "automation tail {measured_db:.3} dB exceeds settled ceiling"
    );
    assert!(
        (f64::from(data.output_peak_db) - measured_db).abs() <= METER_TOLERANCE_DB,
        "meter {:.3} vs measured {measured_db:.3}",
        data.output_peak_db
    );
    assert!(data.output_isp_dbtp.iter().all(|v| v.is_finite()));
}
