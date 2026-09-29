//! Source-clock, delayed-reference and lifecycle oracles independent of AutoGain internals.
// Rust guideline compliant 2026-02-21
use crate::{UpmixerPlugin, UpmixerPluginParams};
use rustfft::num_complex::Complex;
use sotf_host::{ParameterValue, Plugin, ProcessContext};

fn plugin(rate: u32, fft_size: usize, preview: bool, hr: bool, enabled: bool) -> UpmixerPlugin {
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size": fft_size, "speaker_config": "5.1", "binaural_preview": preview,
        "enable_hr_direct": hr, "auto_gain_enabled": enabled,
        "auto_gain_max_db": 12.0, "auto_gain_smoothing_ms": 100.0,
        "safety_cap_db": -1.0
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    // Constructor JSON must synchronize the already prepared disabled helper.
    assert_eq!(
        plugin.safety.auto_gain.as_ref().unwrap().is_enabled(),
        enabled
    );
    plugin.initialize(rate).unwrap();
    plugin
}

fn input(frames: usize, offset: usize) -> Vec<f32> {
    (offset..offset + frames)
        .flat_map(|n| {
            let a = (n as f64 * 0.131).sin() as f32 * 0.003;
            let b = (n as f64 * 0.071).cos() as f32 * 0.001;
            [a, b]
        })
        .collect()
}

fn check_reference(plugin: &UpmixerPlugin, accepted: usize) {
    let reference_frames = plugin.safety.auto_gain_reference.len() / 2;
    assert_eq!(
        plugin.safety.auto_gain_reference_position,
        accepted % reference_frames
    );
    for age in 0..reference_frames {
        let stored = (plugin.safety.auto_gain_reference_position + age) % reference_frames;
        let expected = if accepted + age >= reference_frames {
            input(1, accepted + age - reference_frames)
        } else {
            vec![0.0, 0.0]
        };
        assert_eq!(
            plugin.safety.auto_gain_reference[stored * 2..stored * 2 + 2],
            expected
        );
    }
}

fn check_reference_matches_source(plugin: &UpmixerPlugin, source: &[f32], accepted: usize) {
    let reference_frames = plugin.safety.auto_gain_reference.len() / 2;
    assert_eq!(source.len(), accepted * 2);
    assert_eq!(
        plugin.safety.auto_gain_reference_position,
        accepted % reference_frames
    );
    for age in 0..reference_frames {
        let stored = (plugin.safety.auto_gain_reference_position + age) % reference_frames;
        let expected_frame = accepted + age;
        let expected = if expected_frame >= reference_frames {
            let source_frame = expected_frame - reference_frames;
            [source[source_frame * 2], source[source_frame * 2 + 1]]
        } else {
            [0.0, 0.0]
        };
        assert_eq!(
            plugin.safety.auto_gain_reference[stored * 2..stored * 2 + 2],
            expected,
            "delayed reference mismatch at age {age} after {accepted} accepted frames"
        );
    }
}

fn identity_plugin(rate: u32, fft_size: usize, auto_gain_enabled: bool) -> UpmixerPlugin {
    let params = serde_json::from_value(serde_json::json!({
        "fft_size": fft_size, "speaker_config": "2.0",
        "gain_front_direct": 1.0, "gain_front_ambient": 0.0,
        "gain_rear_ambient": 0.0, "stereo_width": 0.0,
        "enable_hr_direct": false, "enable_subharmonic_synth": false,
        "bypass_decorrelation": true, "auto_gain_enabled": auto_gain_enabled,
        "safety_cap_db": -1.0, "multi_source_extraction": false
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(rate).unwrap();
    // Identity transform path isolates the genuine WOLA scheduler; no bypass
    // skips analysis, synthesis or its transport delay.
    plugin
        .panning
        .panning_gains_left
        .copy_from_slice(&[1.0, 0.0]);
    plugin
        .panning
        .panning_gains_right
        .copy_from_slice(&[0.0, 1.0]);
    plugin
        .spectral
        .mains_high_gains
        .fill(Complex::new(1.0, 0.0));
    plugin.spectral.lfe_low_gains.fill(Complex::new(0.0, 0.0));
    plugin
}

fn identity_source(rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|i| {
            let level = [0.02, 0.2, 0.003][i / (rate as usize / 5) % 3];
            let sample = level * (std::f64::consts::TAU * 997.0 * i as f64 / f64::from(rate)).sin();
            [sample as f32, -0.5 * sample as f32]
        })
        .collect()
}

#[test]
fn prepared_scheduler_accepts_all_frames_and_reference_advances_while_disabled() {
    for rate in [44_100, 48_000, 96_000] {
        for fft_size in [256, 512, 1024, 2048, 4096, 8192] {
            for (preview, hr) in [(false, false), (false, true), (true, false), (true, true)] {
                let mut plugin = plugin(rate, fft_size, preview, hr, false);
                let mut accepted = 0;
                for frames in [1, 17, fft_size - 1, 8193, fft_size + 3] {
                    let data = input(frames, accepted);
                    let mut output = vec![f32::NAN; frames * plugin.output_channels()];
                    assert_eq!(
                        plugin
                            .process(&data, &mut output, &ProcessContext::new(rate, frames))
                            .unwrap(),
                        frames
                    );
                    assert!(output.iter().all(|v| v.is_finite()));
                    accepted += frames;
                    check_reference(&plugin, accepted);
                }
                plugin
                    .set_parameter("auto_gain_enabled".into(), ParameterValue::Bool(true))
                    .unwrap();
                let data = input(8193, accepted);
                let mut output = vec![f32::NAN; 8193 * plugin.output_channels()];
                assert_eq!(
                    plugin
                        .process(&data, &mut output, &ProcessContext::new(rate, 8193))
                        .unwrap(),
                    8193
                );
                accepted += 8193;
                check_reference(&plugin, accepted);
                let before = plugin.safety.auto_gain_reference.clone();
                let cursor = plugin.safety.auto_gain_reference_position;
                let mut canary = [777.0];
                assert!(
                    plugin
                        .process(&[0.0, 0.0], &mut canary, &ProcessContext::new(rate, 1))
                        .is_err()
                );
                assert_eq!(canary, [777.0]);
                assert_eq!(plugin.safety.auto_gain_reference, before);
                assert_eq!(plugin.safety.auto_gain_reference_position, cursor);
                assert_eq!(
                    plugin
                        .process(&[], &mut [], &ProcessContext::new(rate, 0))
                        .unwrap(),
                    0
                );
                assert_eq!(plugin.safety.auto_gain_reference, before);
                plugin.reset();
                assert!(plugin.safety.auto_gain_reference.iter().all(|v| *v == 0.0));
                assert_eq!(plugin.safety.auto_gain_reference_position, 0);
                plugin
                    .set_parameter("low_latency".into(), ParameterValue::Bool(true))
                    .unwrap();
                assert_eq!(plugin.core.fft_size, 1024);
                assert_eq!(plugin.safety.auto_gain_reference.len(), 2048);
                assert!(plugin.safety.auto_gain_reference.iter().all(|v| *v == 0.0));
            }
        }
    }
}

#[test]
fn delayed_identity_has_zero_compensation_through_startup_and_level_changes() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for n in [2, 64, 256, 512, 1024, 2048] {
            let mut plugin = identity_plugin(rate, n, true);
            let frames = rate as usize + n + 17;
            let latency = plugin.latency_samples();
            let source = identity_source(rate, frames);
            let mut result = vec![f32::NAN; frames * 2];
            let mut position = 0;
            for &requested in [1, 17, 137, 8193].iter().cycle() {
                if position == frames {
                    break;
                }
                let block = requested.min(frames - position);
                assert_eq!(
                    plugin
                        .process(
                            &source[position * 2..(position + block) * 2],
                            &mut result[position * 2..(position + block) * 2],
                            &ProcessContext::new(rate, block)
                        )
                        .unwrap(),
                    block
                );
                position += block;
            }
            let maximum = result
                .iter()
                .enumerate()
                .map(|(i, sample)| {
                    let expected = if i >= latency * 2 {
                        source[i - latency * 2]
                    } else {
                        0.0
                    };
                    (sample - expected).abs()
                })
                .fold(0.0, f32::max);
            assert!(
                maximum < 2.0e-6,
                "{rate}/{n}: delayed identity error {maximum}"
            );
            check_reference_matches_source(&plugin, &source, frames);
            let data = plugin.safety.auto_gain.as_ref().unwrap().data();
            assert!(
                data.gain_db.abs() < 0.0001,
                "{rate}/{n}: {} dB",
                data.gain_db
            );
            assert!((data.input_lufs - data.output_lufs).abs() < 0.0001);
        }
    }
}

#[test]
fn above512_source_tagged_hr_keeps_autogain_reference_on_the_main_clock() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 24_000;

    let source: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let level = if frame < 8_000 {
                0.035
            } else if frame < 16_000 {
                0.11
            } else {
                0.06
            };
            let phase = std::f32::consts::TAU * 7_000.0 * frame as f32 / RATE as f32;
            let burst = if [0, 4_001, 12_003, 20_007].contains(&frame) {
                0.18
            } else {
                0.0
            };
            let sample = level * phase.sin() + burst;
            [sample, -0.7 * sample]
        })
        .collect();

    for fft_size in [1_024_usize, 2_048] {
        for auto_gain_enabled in [false, true] {
            let mut with_hr = plugin(RATE, fft_size, false, true, auto_gain_enabled);
            let mut main_only = plugin(RATE, fft_size, false, false, auto_gain_enabled);
            with_hr.hr_state.hr_direct_envelope = 1.0;
            with_hr.hr_state.hr_transient_env = 1.0;
            assert!(with_hr.uses_hr_source_tags());
            assert_eq!(with_hr.latency_samples(), fft_size);

            let channels = with_hr.output_channels();
            let mut hr_output = vec![f32::NAN; INPUT_FRAMES * channels];
            let mut main_output = vec![f32::NAN; INPUT_FRAMES * channels];
            let mut accepted = 0;
            let partitions = [1, 17, 137, 512, 8_193];
            let mut partition = 0;
            while accepted < INPUT_FRAMES {
                let frames = partitions[partition % partitions.len()].min(INPUT_FRAMES - accepted);
                let start = accepted * 2;
                let end = (accepted + frames) * 2;
                assert_eq!(
                    with_hr
                        .process(
                            &source[start..end],
                            &mut hr_output[accepted * channels..(accepted + frames) * channels],
                            &ProcessContext::new(RATE, frames),
                        )
                        .unwrap(),
                    frames
                );
                assert_eq!(
                    main_only
                        .process(
                            &source[start..end],
                            &mut main_output[accepted * channels..(accepted + frames) * channels],
                            &ProcessContext::new(RATE, frames),
                        )
                        .unwrap(),
                    frames
                );
                accepted += frames;
                partition += 1;
                check_reference_matches_source(&with_hr, &source[..accepted * 2], accepted);
                check_reference_matches_source(&main_only, &source[..accepted * 2], accepted);
            }

            assert!(hr_output.iter().all(|sample| sample.is_finite()));
            assert!(main_output.iter().all(|sample| sample.is_finite()));
            let maximum_hr_difference = hr_output
                .iter()
                .zip(&main_output)
                .map(|(with_hr, main)| (with_hr - main).abs())
                .fold(0.0_f32, f32::max);
            assert!(maximum_hr_difference > 1.0e-8);
            assert_eq!(
                with_hr.safety.auto_gain_reference_position,
                accepted % (with_hr.safety.auto_gain_reference.len() / 2)
            );
            if auto_gain_enabled {
                let data = with_hr.safety.auto_gain.as_ref().unwrap().data();
                assert!(data.gain_db.is_finite());
                assert!(data.input_lufs.is_finite());
                assert!(data.output_lufs.is_finite());
            }
            eprintln!(
                "AUD132 AutoGain HR clock N={fft_size} enabled={auto_gain_enabled} accepted={accepted} latency={} reference_cursor={} HR_difference={maximum_hr_difference:.9} output_digest={:016x}",
                with_hr.latency_samples(),
                with_hr.safety.auto_gain_reference_position,
                hr_output
                    .iter()
                    .fold(0xcbf29ce484222325_u64, |digest, sample| {
                        digest.wrapping_mul(0x100000001b3) ^ u64::from(sample.to_bits())
                    }),
            );
        }
    }
}

#[test]
fn small_fft_autogain_disabled_warming_toggle_preserves_causal_identity() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 24_000;
    const WARM_FRAMES: usize = 6_000;

    for fft_size in [2, 256] {
        let source = identity_source(RATE, INPUT_FRAMES);
        let mut plugin = identity_plugin(RATE, fft_size, false);
        assert_eq!(plugin.latency_samples(), 512);
        assert_eq!(plugin.safety.auto_gain_reference.len(), 512 * 2);

        let mut output = vec![f32::NAN; INPUT_FRAMES * 2];
        let mut accepted = 0;
        for enabled in [false, true] {
            if enabled {
                plugin
                    .set_parameter("auto_gain_enabled".into(), ParameterValue::Bool(true))
                    .unwrap();
            }
            let end = if enabled { INPUT_FRAMES } else { WARM_FRAMES };
            for &requested in [1, 17, 137, 512].iter().cycle() {
                if accepted == end {
                    break;
                }
                let block = requested.min(end - accepted);
                assert_eq!(
                    plugin
                        .process(
                            &source[accepted * 2..(accepted + block) * 2],
                            &mut output[accepted * 2..(accepted + block) * 2],
                            &ProcessContext::new(RATE, block)
                        )
                        .unwrap(),
                    block
                );
                accepted += block;
                check_reference_matches_source(&plugin, &source[..accepted * 2], accepted);
            }
        }

        let maximum = output
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let source_index = index as isize - 512 * 2;
                let expected = if source_index >= 0 {
                    source[source_index as usize]
                } else {
                    0.0
                };
                (sample - expected).abs()
            })
            .fold(0.0, f32::max);
        assert!(
            maximum < 2.0e-6,
            "N={fft_size}: disabled-warm/toggled AutoGain delayed identity error {maximum}"
        );
        let data = plugin.safety.auto_gain.as_ref().unwrap().data();
        assert!(
            data.gain_db.abs() < 0.0001,
            "N={fft_size}: {} dB",
            data.gain_db
        );
        assert!(
            (data.input_lufs - data.output_lufs).abs() < 0.0001,
            "N={fft_size}: input/output loudness diverged"
        );
    }
}

#[test]
fn bypass_uses_current_source_and_mode_changes_clear_reference() {
    let mut plugin = plugin(48000, 2048, false, true, true);
    let source = input(8193, 0);
    let mut output = vec![0.0; 8193 * plugin.output_channels()];
    plugin
        .process(&source, &mut output, &ProcessContext::new(48000, 8193))
        .unwrap();
    plugin
        .set_parameter("bypass_all_processing".into(), ParameterValue::Bool(true))
        .unwrap();
    assert_eq!(plugin.latency_samples(), 0);
    assert!(plugin.safety.auto_gain_reference.iter().all(|v| *v == 0.0));
    plugin
        .process(&source, &mut output, &ProcessContext::new(48000, 8193))
        .unwrap();
    for (frame, input) in output
        .chunks_exact(plugin.output_channels())
        .zip(source.as_chunks::<2>().0)
    {
        assert_eq!(frame[..2], *input);
        assert!(frame[2..].iter().all(|v| *v == 0.0));
    }
    assert_eq!(plugin.safety.auto_gain_reference_position, 0);
    plugin
        .set_parameter("bypass_all_processing".into(), ParameterValue::Bool(false))
        .unwrap();
    assert_eq!(plugin.latency_samples(), 2048);
    assert_eq!(plugin.safety.auto_gain_reference_position, 0);
    assert!(plugin.safety.auto_gain_reference.iter().all(|v| *v == 0.0));
}
