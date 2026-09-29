//! Sample-clock AutoGain accuracy, lifecycle and callback ownership.
// Rust guideline compliant 2026-02-21
use crate::initialize_tests::MatrixFile;
use crate::realtime_tests::callback_counts;
use crate::{XtcData, XtcPlugin, XtcPluginParams};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};

fn make(rate: u32, n: usize) -> XtcPlugin {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size: n,
            auto_gain_enabled: true,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn render(plugin: &mut XtcPlugin, source: &[f32], pattern: &[usize]) -> Vec<f32> {
    let rate = plugin.fft.sample_rate;
    let mut output = vec![0.0; source.len()];
    let mut cursor = 0;
    for &size in pattern.iter().cycle() {
        let frames = size.min(source.len() / 2 - cursor);
        if frames == 0 {
            break;
        }
        plugin
            .process(
                &source[cursor * 2..(cursor + frames) * 2],
                &mut output[cursor * 2..(cursor + frames) * 2],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        cursor += frames;
    }
    output
}

fn diagonal(rate: u32, n: usize) -> XtcPlugin {
    let file = MatrixFile::new(rate, 2);
    std::fs::write(
        &file.0,
        serde_json::to_vec(&serde_json::json!({
            "sample_rate": rate, "speakers": ["s0", "s1"], "ears": ["e0", "e1"],
            "filters": [
                {"speaker":"s0", "target_ear":"e0", "taps":[0.5]},
                {"speaker":"s0", "target_ear":"e1", "taps":[0.0]},
                {"speaker":"s1", "target_ear":"e0", "taps":[0.0]},
                {"speaker":"s1", "target_ear":"e1", "taps":[0.5]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut params = file.params();
    params.fft_size = n;
    params.auto_gain_enabled = true;
    params.auto_gain_max_db = 12.0;
    let mut plugin = XtcPlugin::new(params, rate).unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

#[test]
fn fixed_diagonal_gain_matches_independent_amplitude_ratio_and_delayed_loudness() {
    for rate in [44100, 48000, 96000] {
        for n in [128, 2048] {
            let mut plugin = diagonal(rate, n);
            let source: Vec<_> = (0..rate as usize * 4)
                .flat_map(|i| {
                    let x = (std::f64::consts::TAU * 1000.0 * i as f64 / f64::from(rate)).sin();
                    [(0.04 * x) as f32, (-0.02 * x) as f32]
                })
                .collect();
            let output = render(&mut plugin, &source, &[8193, 137, 1]);
            let data = plugin.get_data().unwrap();
            let data = &data.downcast_ref::<XtcData>().unwrap().auto_gain;
            let expected_db = 20.0 * 2.0_f64.log10();
            assert!((data.input_lufs - data.output_lufs - expected_db).abs() < 1e-5);
            // f32 one-pole updates eventually reach a rounding fixed point;
            // allow 0.01 dB for that known finite-precision steady-state error.
            assert!(
                (f64::from(data.gain_db) - expected_db).abs() < 0.01,
                "rate={rate}, N={n}, gain={} expected={expected_db}",
                data.gain_db
            );
            let start = rate as usize * 3 * 2;
            let measured: f64 = output[start..].iter().map(|&v| f64::from(v).powi(2)).sum();
            let expected: f64 = source[start - 2 * n..source.len() - 2 * n]
                .iter()
                .map(|&v| f64::from(v).powi(2))
                .sum();
            assert!((10.0 * (measured / expected).log10()).abs() < 0.01);
        }
    }
}

#[test]
fn future_measurements_cannot_rewrite_audio_before_their_sample_boundary() {
    for rate in [44100, 48000, 96000] {
        for offset in [-1_isize, 0, 1] {
            let n = 128;
            let boundary = rate as usize / 10 * 5;
            let same_frames = boundary.checked_add_signed(offset).unwrap();
            let mut left = vec![0.03125; (same_frames + 4096) * 2];
            let mut right = left.clone();
            left[same_frames * 2..].fill(0.0625);
            right[same_frames * 2..].fill(-0.125);
            let mut a = diagonal(rate, n);
            let mut b = diagonal(rate, n);
            let a = render(&mut a, &left, &[left.len() / 2]);
            let b = render(&mut b, &right, &[17, 8193]);
            let causal_prefix = same_frames * 2;
            let first = a[..causal_prefix]
                .iter()
                .zip(&b[..causal_prefix])
                .position(|(a, b)| a != b);
            assert!(
                first.is_none(),
                "future input changed earlier output: rate={rate}, offset={offset}, first={first:?}"
            );
            // After the different input has been accepted, FFT roundoff in a
            // shared window can differ before the ideal one-tap delay arrives.
            // Keep the existing independent WOLA reconstruction tolerance.
            let ideal_prefix = (same_frames + n) * 2;
            let maximum = a[..ideal_prefix]
                .iter()
                .zip(&b[..ideal_prefix])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                maximum < 1.5e-6,
                "delayed-identity prefix error={maximum}, rate={rate}, offset={offset}"
            );
            assert!(a.iter().zip(&b).any(|(a, b)| a != b));
        }
    }
}

#[test]
fn parameter_snapshots_fresh_meter_and_reset_keep_explicit_sample_phase() {
    let mut plugin = make(48000, 128);
    render(&mut plugin, &[0.0625; 74], &[37]);
    assert_eq!(plugin.diagnostics.auto_gain_frames, 37);
    let enable = ParameterId::from("auto_gain_enabled");
    plugin
        .set_parameter(enable.clone(), ParameterValue::Bool(true))
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("auto_gain_max_db"),
            ParameterValue::Float(9.0),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("auto_gain_smoothing_ms"),
            ParameterValue::Float(200.0),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
        .unwrap();
    assert_eq!(plugin.diagnostics.auto_gain_frames, 37);
    plugin
        .set_parameter(enable.clone(), ParameterValue::Bool(false))
        .unwrap();
    render(&mut plugin, &[0.03125; 34], &[17]);
    assert_eq!(plugin.bypass.position, (37 + 17) * 2);
    plugin
        .set_parameter(enable, ParameterValue::Bool(true))
        .unwrap();
    assert_eq!(plugin.diagnostics.auto_gain_frames, 0);
    render(&mut plugin, &vec![0.03125; 4799 * 2], &[8193]);
    assert_eq!(plugin.diagnostics.auto_gain_frames, 4799);
    let mut tail = [987.0; 2];
    let result = plugin
        .drain(&mut tail, &ProcessContext::new(48000, 0))
        .unwrap();
    assert_eq!(result.frames, 1);
    // A one-frame destination owns one canonical 32-frame refill. It crosses
    // the refresh once; subsequent cached reads must not advance that clock.
    assert_eq!(plugin.diagnostics.auto_gain_frames, 31);
    let remaining = plugin.drain_state.remaining;
    plugin
        .drain(&mut tail, &ProcessContext::new(48000, 0))
        .unwrap();
    assert_eq!(plugin.diagnostics.auto_gain_frames, 31);
    assert_eq!(plugin.drain_state.remaining, remaining.map(|r| r - 1));
    plugin.reset();
    assert_eq!(plugin.diagnostics.auto_gain_frames, 0);
    assert_eq!(plugin.bypass.position, 0);
}

#[test]
fn first_refresh_active_gain_partial_eof_and_reset_neither_allocate_nor_free() {
    for rate in [44100, 48000, 192000] {
        for n in [128, 2048, 16384] {
            for enabled in [false, true] {
                let mut plugin = make(rate, n);
                plugin
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(enabled))
                    .unwrap();
                plugin.reset();
                let input = vec![0.03125; 8193 * 2];
                let mut output = vec![0.0; input.len()];
                let (plugin, counts) = std::thread::spawn(move || {
                    let counts = callback_counts(|| {
                        for _ in 0..2 {
                            // More than 400 ms at every selected rate exercises
                            // first publication and a nontrivial gain target.
                            for _ in 0..12 {
                                plugin
                                    .process(&input, &mut output, &ProcessContext::new(rate, 8193))
                                    .unwrap();
                            }
                            plugin
                                .drain(&mut output[..2], &ProcessContext::new(rate, 0))
                                .unwrap();
                            let bound = plugin.drain_call_bound().unwrap().get();
                            let mut calls = 0;
                            loop {
                                calls += 1;
                                if plugin
                                    .drain(&mut output[..n / 2], &ProcessContext::new(rate, 0))
                                    .unwrap()
                                    .complete
                                {
                                    break;
                                }
                            }
                            assert!(calls <= bound);
                            assert!(
                                plugin
                                    .dynamics
                                    .auto_gain
                                    .as_ref()
                                    .unwrap()
                                    .last_input_lufs()
                                    .is_finite()
                            );
                            plugin.reset();
                        }
                    });
                    (plugin, counts)
                })
                .join()
                .unwrap();
                assert_eq!(counts, (0, 0), "rate={rate}, N={n}, enabled={enabled}");
                drop(plugin);
            }
        }
    }
}
