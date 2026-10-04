//! Behavioral accuracy tests for reduction range and channel linking.

// Rust guideline compliant 2026-02-21

use sotf_host::plugin::ProcessContext;
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};
use sotf_plugin_de_esser::{DeEsserPlugin, DeEsserPluginParams};

fn plugin(mode: &str, range_db: f32, stereo_link: f32, sample_rate: u32) -> DeEsserPlugin {
    let params = DeEsserPluginParams {
        frequency: 8_000.0,
        threshold: -40.0,
        ratio: 20.0,
        attack_ms: 0.1,
        mode: mode.into(),
        range_db,
        stereo_link,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::from_params(2, params).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    plugin
}

fn stereo_tone(sample_rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let strong =
                0.8 * (std::f32::consts::TAU * 8_000.0 * frame as f32 / sample_rate as f32).sin();
            [strong, strong * 0.005]
        })
        .collect()
}

fn channel_rms(samples: &[f32], channel: usize) -> f32 {
    (samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|frame| frame[channel].powi(2))
        .sum::<f32>()
        / (samples.len() / 2) as f32)
        .sqrt()
}

fn process(plugin: &mut DeEsserPlugin, signal: &mut [f32], sample_rate: u32, chunks: &[usize]) {
    let mut offset = 0;
    for &frames in chunks.iter().cycle() {
        if offset == signal.len() {
            break;
        }
        let end = (offset + frames * 2).min(signal.len());
        plugin
            .process_in_place(
                &mut signal[offset..end],
                &ProcessContext::new(sample_rate, (end - offset) / 2),
            )
            .unwrap();
        offset = end;
    }
}

#[test]
fn range_matches_the_analytic_gain_in_both_modes() {
    for sample_rate in [44_100, 48_000, 96_000] {
        let input = stereo_tone(sample_rate, sample_rate as usize / 2);
        for mode in ["Wideband", "Split-Band"] {
            for range_db in [0.0, 3.0, 6.0, 12.0] {
                let mut processor = plugin(mode, range_db, 1.0, sample_rate);
                let mut output = input.clone();
                process(&mut processor, &mut output, sample_rate, &[1, 7, 257, 64]);
                let tail = input.len() / 2;
                let gain = 10.0_f32.powf(-range_db / 20.0);
                // At the LR4 crossover, low and high have matching phase and
                // magnitude 0.5, so the summed gain is (1 + HF gain) / 2.
                let expected = if mode == "Wideband" {
                    gain
                } else {
                    (1.0 + gain) * 0.5
                };
                let actual = channel_rms(&output[tail..], 0) / channel_rms(&input[tail..], 0);
                assert!(
                    (actual - expected).abs() < 0.002,
                    "{mode}, {sample_rate} Hz, range {range_db}: got {actual}, expected {expected}"
                );
            }
        }
    }
}

#[test]
fn fully_linked_processing_preserves_the_channel_ratio() {
    for mode in ["Wideband", "Split-Band"] {
        let input = stereo_tone(48_000, 24_000);
        let mut linked = input.clone();
        process(
            &mut plugin(mode, 60.0, 1.0, 48_000),
            &mut linked,
            48_000,
            &[257],
        );
        for frame in linked.as_chunks::<2>().0 {
            assert!(
                (frame[1] - frame[0] * 0.005).abs() < 2e-7,
                "{mode} changed the channel ratio: {frame:?}"
            );
        }
        let mut independent = input.clone();
        process(
            &mut plugin(mode, 60.0, 0.0, 48_000),
            &mut independent,
            48_000,
            &[257],
        );
        let tail = input.len() / 2;
        let weak_gain = channel_rms(&independent[tail..], 1) / channel_rms(&input[tail..], 1);
        assert!(
            (weak_gain - 1.0).abs() < 0.001,
            "{mode}: independent weak channel gain {weak_gain}"
        );
        assert!(channel_rms(&linked[tail..], 1) < channel_rms(&independent[tail..], 1) * 0.7);
    }
}

#[test]
fn partial_link_interpolates_reduction_in_decibels() {
    let input = stereo_tone(48_000, 8_000);
    let results: Vec<_> = [0.0, 0.5, 1.0]
        .into_iter()
        .map(|link| {
            let mut output = input.clone();
            process(
                &mut plugin("Wideband", 12.0, link, 48_000),
                &mut output,
                48_000,
                &[64],
            );
            output
        })
        .collect();
    for frame in 0..input.len() / 2 {
        let index = 2 * frame + 1;
        // Half the dB reduction is the geometric mean of the endpoint gains.
        if input[index].abs() > 1e-4 {
            let error_db = 20.0
                * (results[1][index].powi(2) / (results[0][index] * results[2][index])).log10();
            assert!(
                error_db.abs() < 0.002,
                "frame {frame}: interpolation error {error_db} dB"
            );
        }
    }
}

#[test]
fn control_automation_is_independent_of_callback_size_and_reset_is_deterministic() {
    for mode in ["Wideband", "Split-Band"] {
        let input = stereo_tone(48_000, 12_000);
        let mut reference = plugin(mode, 24.0, 0.0, 48_000);
        let mut chunked = plugin(mode, 24.0, 0.0, 48_000);
        let mut a = input.clone();
        let mut b = input.clone();
        process(&mut reference, &mut a[..8_000], 48_000, &[4_000]);
        process(&mut chunked, &mut b[..8_000], 48_000, &[1, 17, 257]);
        for processor in [&mut reference, &mut chunked] {
            for (key, value) in [("stereo_link", 1.0), ("range_db", 3.0)] {
                processor
                    .parametric_set_parameter(ParameterId::from(key), ParameterValue::Float(value))
                    .unwrap();
            }
        }
        process(&mut reference, &mut a[8_000..], 48_000, &[8_000]);
        process(&mut chunked, &mut b[8_000..], 48_000, &[3, 64, 511]);
        assert_eq!(a, b, "{mode} depends on callback size");

        reference.reset();
        let mut fresh = plugin(mode, 3.0, 1.0, 48_000);
        a.copy_from_slice(&input);
        b.copy_from_slice(&input);
        process(&mut reference, &mut a, 48_000, &[73]);
        process(&mut fresh, &mut b, 48_000, &[73]);
        assert_eq!(a, b, "{mode} reset retained envelope or control history");
    }
}

#[test]
fn new_controls_roundtrip_and_reject_invalid_values() {
    let mut processor = plugin("Wideband", 60.0, 0.0, 48_000);
    for (key, value, invalid) in [("range_db", 6.0, 61.0), ("stereo_link", 0.75, 1.1)] {
        let id = ParameterId::from(key);
        processor
            .parametric_set_parameter(id.clone(), ParameterValue::Float(value))
            .unwrap();
        assert_eq!(
            processor.parametric_get_parameter(&id),
            Some(ParameterValue::Float(value))
        );
        for bad in [invalid, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                processor
                    .parametric_set_parameter(id.clone(), ParameterValue::Float(bad))
                    .is_err()
            );
            assert_eq!(
                processor.parametric_get_parameter(&id),
                Some(ParameterValue::Float(value))
            );
        }
    }
    let legacy: DeEsserPluginParams = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.range_db, 60.0);
    assert_eq!(legacy.stereo_link, 0.0);
    let state = DeEsserPluginParams {
        range_db: 6.0,
        stereo_link: 0.75,
        ..legacy
    };
    let restored: DeEsserPluginParams =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(restored.range_db, 6.0);
    assert_eq!(restored.stereo_link, 0.75);
    for invalid in [f32::NAN, -1.0, 61.0] {
        let state = DeEsserPluginParams {
            range_db: invalid,
            ..Default::default()
        };
        assert!(DeEsserPlugin::from_params(2, state).is_err());
    }
}
