//! Independent spectral reconstruction and finite-stream support oracles.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_multiband_expander::{MultibandExpanderPlugin, MultibandExpanderPluginParams};

const RATE: u32 = 48_000;
const WINDOW: usize = 1024;
const HOP: usize = 256;

fn make(channels: usize, bands: usize, unity: bool) -> MultibandExpanderPlugin {
    let mut params = MultibandExpanderPluginParams {
        num_bands: bands,
        processing_mode: "spectral".into(),
        mix: 1.0,
        ratio: if unity { 1.0 } else { 4.0 },
        threshold_db: -24.0,
        knee_db: 0.0,
        hold_ms: 0.0,
        hysteresis_db: 0.0,
        ..Default::default()
    };
    for band in &mut params.bands {
        band.ratio = Some(params.ratio);
        band.threshold_db = Some(params.threshold_db);
        band.knee_db = Some(0.0);
        band.hold_ms = Some(0.0);
        band.hysteresis_db = Some(0.0);
    }
    let mut plugin = MultibandExpanderPlugin::with_params(channels, params);
    plugin.initialize(f64::from(RATE)).unwrap();
    plugin
}

fn process(
    plugin: &mut MultibandExpanderPlugin,
    source: &[f32],
    channels: usize,
    pattern: &[usize],
) -> Vec<f32> {
    let mut output = source.to_vec();
    let mut offset = 0;
    let mut block = 0;
    while offset < source.len() / channels {
        let frames = pattern[block % pattern.len()].min(source.len() / channels - offset);
        plugin
            .process_in_place(
                &mut output[offset * channels..(offset + frames) * channels],
                &ProcessContext::new(RATE, frames),
            )
            .unwrap();
        offset += frames;
        block += 1;
    }
    output
}

#[test]
fn unity_startup_checkpoints_retain_full_amplitude_at_the_declared_delay() {
    let mut observed = Vec::new();
    for phase in [0, HOP, 2 * HOP, 3 * HOP] {
        let mut plugin = make(1, 3, true);
        let mut source = vec![0.0; 4 * WINDOW];
        source[phase] = 0.5;
        let output = process(&mut plugin, &source, 1, &[1, 17, 255, 7, 513]);
        observed.push(output[WINDOW + phase]);
        assert_eq!(plugin.latency_samples(), WINDOW);
    }
    for actual in &observed {
        assert!(
            (actual - 0.5).abs() < 2e-6,
            "unity onset checkpoint amplitudes {observed:?}"
        );
    }
}

#[test]
fn wet_spectral_drain_has_derived_support_and_preserves_the_final_marker() {
    let frames = WINDOW + 1;
    let mut plugin = make(1, 3, true);
    let mut source = vec![0.0; frames];
    source[frames - 1] = 0.5;
    let mut output = process(&mut plugin, &source, 1, &[137, 1, 511]);
    let expected_tail = 2 * WINDOW - HOP + (HOP - frames % HOP) % HOP;
    let mut tail = [0.0; HOP];
    let mut tail_frames = 0;
    for _ in 0..16 {
        let result = plugin
            .drain(&mut tail, &ProcessContext::new(RATE, 0))
            .unwrap();
        output.extend_from_slice(&tail[..result.frames]);
        tail_frames += result.frames;
        if result.complete {
            break;
        }
    }
    assert_eq!(tail_frames, expected_tail);
    assert!((output[frames - 1 + WINDOW] - 0.5).abs() < 2e-6);
}

fn tail(plugin: &mut MultibandExpanderPlugin, channels: usize, capacities: &[usize]) -> Vec<f32> {
    let mut rendered = Vec::new();
    for step in 0..4096 {
        let capacity = capacities[step % capacities.len()];
        let mut buffer = vec![123.0; capacity * channels + 3];
        let result = plugin
            .drain(
                &mut buffer[..capacity * channels],
                &ProcessContext::new(RATE, 0),
            )
            .unwrap();
        assert!(result.frames <= capacity && result.frames <= HOP);
        assert!(
            buffer[result.frames * channels..]
                .iter()
                .all(|&x| x == 123.0)
        );
        rendered.extend_from_slice(&buffer[..result.frames * channels]);
        if result.complete {
            return rendered;
        }
    }
    panic!("finite continuation did not finish")
}

fn assert_unity(output: &[f32], source: &[f32], channels: usize) {
    for (index, &actual) in output.iter().enumerate() {
        let expected = index
            .checked_sub(WINDOW * channels)
            .and_then(|source_index| source.get(source_index))
            .copied()
            .unwrap_or(0.0);
        assert!(
            (actual - expected).abs() < 2e-6,
            "sample={index}, actual={actual}, expected={expected}"
        );
    }
}

#[test]
fn every_initial_phase_retains_first_and_final_impulses_through_eos() {
    let mut fixtures = 0;
    for channels in [1, 2] {
        for phase in 0..WINDOW {
            let mut plugin = make(channels, 3, true);
            let frames = phase + 1;
            let mut source = vec![0.0; frames * channels];
            for channel in 0..channels {
                source[channel] = 0.125 * (channel + 1) as f32;
                source[phase * channels + channel] += 0.25;
            }
            let mut output = process(
                &mut plugin,
                &source,
                channels,
                if phase % 2 == 0 {
                    &[1]
                } else {
                    &[17, 255, 1, 513]
                },
            );
            output.extend(tail(&mut plugin, channels, &[1, 17, 255, 256]));
            let end = 2 * WINDOW + ((frames - 1) / HOP) * HOP;
            assert_eq!(output.len(), end * channels);
            assert_unity(&output, &source, channels);
            fixtures += 1;
        }
    }
    assert_eq!(fixtures, 2048);
}

#[test]
fn dense_unity_ring_wrap_and_reset_match_independent_source_delay() {
    for channels in [1, 2] {
        for bands in [1, 2, 3, 5] {
            let source: Vec<_> = (0..(17 * WINDOW + 13) * channels)
                .map(|i| ((i * 29 % 127) as f32 - 63.0) / 256.0)
                .collect();
            let mut baseline = None;
            for pattern in [&[1][..], &[255, 257, 7][..], &[8193, 17, 1][..]] {
                let mut plugin = make(channels, bands, true);
                for epoch in 0..3 {
                    if epoch == 1 {
                        plugin.reset();
                    }
                    if epoch == 2 {
                        plugin.initialize(f64::from(RATE)).unwrap();
                    }
                    let mut output = process(&mut plugin, &source, channels, pattern);
                    output.extend(tail(&mut plugin, channels, &[17, 256, 1]));
                    assert_unity(&output, &source, channels);
                    if let Some(expected) = &baseline {
                        assert_eq!(&output, expected);
                    } else {
                        baseline = Some(output);
                    }
                }
            }
        }
    }
}

#[test]
fn nonlinear_spectral_tail_matches_independent_zero_continuation_for_all_hop_boundaries() {
    use sotf_host::{ParameterId, ParameterValue};
    for channels in [1, 2] {
        for bands in [1, 2, 3, 5] {
            for variant in 0..4 {
                for frames in [
                    1,
                    HOP - 1,
                    HOP,
                    HOP + 1,
                    WINDOW - 1,
                    WINDOW,
                    WINDOW + 1,
                    4 * WINDOW + 73,
                ] {
                    let mut actual = make(channels, bands, false);
                    let mut reference = make(channels, bands, false);
                    for plugin in [&mut actual, &mut reference] {
                        plugin
                            .set_parameter(
                                ParameterId::from("band_0_hold"),
                                ParameterValue::Float(7.0),
                            )
                            .unwrap();
                        plugin
                            .set_parameter(
                                ParameterId::from("band_0_knee"),
                                ParameterValue::Float(6.0),
                            )
                            .unwrap();
                        if variant == 1 {
                            plugin
                                .set_parameter(
                                    ParameterId::from("band_0_solo"),
                                    ParameterValue::Bool(true),
                                )
                                .unwrap();
                        }
                        if variant == 2 {
                            plugin
                                .set_parameter(
                                    ParameterId::from("band_0_bypass"),
                                    ParameterValue::Bool(true),
                                )
                                .unwrap();
                        }
                    }
                    let source: Vec<_> = (0..frames * channels)
                        .map(|i| ((i * 19 % 113) as f32 - 56.0) / 2048.0)
                        .collect();
                    let got = process(&mut actual, &source, channels, &[17, 1, 511]);
                    let expected = process(&mut reference, &source, channels, &[8193]);
                    assert_eq!(got, expected);
                    if variant == 3 {
                        for plugin in [&mut actual, &mut reference] {
                            plugin
                                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                                .unwrap();
                        }
                    }
                    let remaining = 2 * WINDOW + ((frames - 1) / HOP) * HOP - frames;
                    let expected = process(
                        &mut reference,
                        &vec![0.0; (remaining + 2 * WINDOW) * channels],
                        channels,
                        &[1, 37, 255, 513],
                    );
                    let got = tail(&mut actual, channels, &[1, 17, 255, 256]);
                    assert_eq!(got.len(), remaining * channels);
                    assert_eq!(
                        got,
                        &expected[..got.len()],
                        "channels={channels},bands={bands},variant={variant},frames={frames}"
                    );
                    assert!(expected[got.len()..].iter().all(|&x| x == 0.0));
                }
            }
        }
    }
}

#[test]
fn validation_controls_completion_and_partial_cache_bounds_preserve_history() {
    use sotf_host::{ParameterId, ParameterValue};
    for channels in [1, 2] {
        for phase in 0..HOP {
            let mut actual = make(channels, 3, false);
            let mut reference = make(channels, 3, false);
            assert_eq!(actual.drain_output_frames_max(), HOP);
            assert!(
                actual
                    .drain(&mut [], &ProcessContext::new(RATE, 0))
                    .unwrap()
                    .complete
            );
            let source = vec![0.125; (WINDOW + phase + 1) * channels];
            assert_eq!(
                process(&mut actual, &source, channels, &[17]),
                process(&mut reference, &source, channels, &[17])
            );
            let mut canary = vec![123.0; HOP * channels + 1];
            assert!(
                actual
                    .drain(&mut canary, &ProcessContext::new(44100, 0))
                    .is_err()
            );
            assert!(
                actual
                    .drain(&mut [], &ProcessContext::new(RATE, 0))
                    .is_err()
            );
            if channels == 2 {
                assert!(
                    actual
                        .drain(&mut canary, &ProcessContext::new(RATE, 0))
                        .is_err()
                );
            }
            assert!(canary.iter().all(|&x| x == 123.0));
            // Invalid calls must not latch EOS; this recognized change remains legal.
            for plugin in [&mut actual, &mut reference] {
                plugin
                    .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-27.0))
                    .unwrap();
            }
            let expected = tail(&mut reference, channels, &[HOP]);
            let mut first = vec![0.0; channels];
            assert_eq!(
                actual
                    .drain(&mut first, &ProcessContext::new(RATE, 0))
                    .unwrap()
                    .frames,
                1
            );
            assert_eq!(first, &expected[..channels]);
            let snapshot = actual.current_values();
            actual.apply_values(snapshot.clone()).unwrap();
            assert!(
                actual
                    .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.5))
                    .is_err()
            );
            let mut changed = snapshot.clone();
            changed.insert(ParameterId::from("threshold"), ParameterValue::Float(-30.0));
            assert!(actual.apply_values(changed).is_err());
            assert_eq!(actual.current_values(), snapshot);
            assert!(
                actual
                    .process_in_place(&mut first, &ProcessContext::new(RATE, 1))
                    .is_err()
            );
            let bound = actual.drain_call_bound().unwrap().get();
            let mut got = Vec::new();
            let mut calls = 0;
            loop {
                let result = actual
                    .drain(&mut canary[..HOP * channels], &ProcessContext::new(RATE, 0))
                    .unwrap();
                calls += 1;
                assert!(calls <= bound);
                got.extend_from_slice(&canary[..result.frames * channels]);
                if result.complete {
                    break;
                }
            }
            assert_eq!(calls, bound);
            assert_eq!(got, &expected[channels..]);
            assert_eq!(actual.drain_call_bound().unwrap().get(), 1);
            assert!(
                actual
                    .drain(&mut [], &ProcessContext::new(RATE, 0))
                    .unwrap()
                    .complete
            );
            assert_eq!(actual.drain_output_frames_max(), HOP);
        }
    }
}

#[test]
fn full_spectral_tail_metadata_survives_a_nearly_settled_fade_and_cached_output() {
    use sotf_host::{ParameterId, ParameterValue, TailLength};
    let mut actual = make(2, 3, false);
    let mut reference = make(2, 3, false);
    for plugin in [&mut actual, &mut reference] {
        plugin
            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
            .unwrap();
    }
    // A 20 ms one-pole fades below the 1e-5 snap threshold after roughly
    // 960*ln(1e5)=11052 samples. EOF just before this point makes the first
    // canonical refill finish the fade while almost the entire tail is unread.
    let source = vec![0.125; 11_000 * 2];
    assert_eq!(
        process(&mut actual, &source, 2, &[17, 513]),
        process(&mut reference, &source, 2, &[8193])
    );
    assert_eq!(actual.tail_length(), TailLength::Finite(2047));
    process(&mut reference, &vec![0.0; HOP * 2], 2, &[HOP]);
    assert_eq!(reference.tail_length(), TailLength::Finite(1024));
    let mut frame = [0.0; 2];
    assert_eq!(
        actual
            .drain(&mut frame, &ProcessContext::new(RATE, 0))
            .unwrap()
            .frames,
        1
    );
    assert_eq!(actual.tail_length(), TailLength::Finite(2047));
    tail(&mut actual, 2, &[1, 256]);
    assert_eq!(actual.tail_length(), TailLength::Finite(2047));
    actual.reset();
    assert_eq!(actual.tail_length(), TailLength::Finite(1024));
}
