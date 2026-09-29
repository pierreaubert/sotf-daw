//! Independent waveform, timing, lifecycle, and telemetry oracles for new modes.

// Rust guideline compliant 2026-02-21
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_gate::{GateData, GateMode, GatePlugin, GatePluginParams};

const RATE: u32 = 48_000;
const GAIN_ERROR_DB: f64 = 0.02;

fn amplitude(db: f64) -> f32 {
    10.0_f64.powf(db / 20.0) as f32
}

fn magnitude(level: f64, threshold: f64, ratio: f64, knee: f64, cap: f64) -> f64 {
    let x = level - threshold;
    let hinge = if knee < 0.1 {
        x.max(0.0)
    } else if x <= -knee / 2.0 {
        0.0
    } else if x >= knee / 2.0 {
        x
    } else {
        (x + knee / 2.0).powi(2) / (2.0 * knee)
    };
    ((ratio - 1.0) * hinge).min(cap)
}

fn configuration(mode: GateMode, linked: bool) -> GatePluginParams {
    GatePluginParams {
        mode,
        threshold_db: -20.0,
        ratio: 4.0,
        knee_db: 6.0,
        attack_ms: 1.0,
        hold_ms: 0.0,
        release_ms: 10.0,
        hysteresis_db: 0.0,
        range_db: 12.0,
        max_boost_db: 12.0,
        link_channels: linked,
        sidechain_external: true,
        ..GatePluginParams::default()
    }
}

fn make(channels: usize, rate: u32, params: GatePluginParams) -> GatePlugin {
    let mut plugin = GatePlugin::try_from_params(channels, params).unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn block(plugin: &mut GatePlugin, rate: u32, levels: &[f64], frames: usize) -> Vec<f32> {
    let channels = levels.len();
    let mut audio = Vec::with_capacity(2 * channels * frames);
    for _ in 0..frames {
        audio.extend((0..channels).map(|channel| if channel % 2 == 0 { 0.25 } else { -0.125 }));
        audio.extend(levels.iter().map(|&level| amplitude(level)));
    }
    plugin
        .process_in_place(&mut audio, &ProcessContext::new(rate, frames))
        .unwrap();
    for frame in audio.chunks_exact(channels * 2) {
        for (actual, &level) in frame[channels..].iter().zip(levels) {
            assert_eq!(
                *actual,
                amplitude(level),
                "external sidechain must remain unchanged"
            );
        }
    }
    audio
}

fn measured_gain(output: f32, input: f32) -> f64 {
    20.0 * (f64::from(output) / f64::from(input)).log10()
}

#[test]
fn independent_above_threshold_curves_cover_knees_caps_mix_and_linking() {
    for mode in [GateMode::Upward, GateMode::Duck] {
        for linked in [false, true] {
            for rate in [44_100, 48_000, 96_000] {
                for knee in [0.0, 6.0] {
                    for level in [
                        -26.0, -23.01, -23.0, -22.99, -20.01, -20.0, -19.99, -17.01, -17.0, -16.99,
                        -14.0,
                    ] {
                        let mut params = configuration(mode, linked);
                        params.knee_db = knee;
                        let mut plugin = make(2, rate, params);
                        let levels = [level, level - 4.0];
                        let audio = block(&mut plugin, rate, &levels, rate as usize / 4);
                        let last = &audio[audio.len() - 4..];
                        for channel in 0..2 {
                            let detector = if linked { level } else { levels[channel] };
                            let amount = magnitude(detector, -20.0, 4.0, f64::from(knee), 12.0);
                            let expected = if mode == GateMode::Upward {
                                amount
                            } else {
                                -amount
                            };
                            let input = if channel == 0 { 0.25 } else { -0.125 };
                            let actual = measured_gain(last[channel], input);
                            assert!(
                                (actual - expected).abs() < GAIN_ERROR_DB,
                                "{mode:?} linked={linked} rate={rate} K={knee} L={detector}: actual={actual} expected={expected}"
                            );
                        }
                    }
                }
            }
            for (ratio, cap, mix) in [
                (1.0, 12.0, 1.0),
                (4.0, 0.0, 1.0),
                (4.0, 3.0, 0.25),
                (4.0, 24.0, 1.0),
                (4.0, 12.0, 0.0),
            ] {
                let mut params = configuration(mode, linked);
                params.ratio = ratio;
                params.max_boost_db = cap;
                params.range_db = cap;
                params.mix = mix;
                let mut plugin = make(2, RATE, params);
                let audio = block(&mut plugin, RATE, &[-10.0; 2], 12_000);
                let effective_cap = if mode == GateMode::Duck && cap == 0.0 {
                    240.0
                } else {
                    f64::from(cap)
                };
                let amount = magnitude(-10.0, -20.0, f64::from(ratio), 6.0, effective_cap);
                let signed = if mode == GateMode::Upward {
                    amount
                } else {
                    -amount
                };
                let expected = 1.0 - f64::from(mix) + f64::from(mix) * 10.0_f64.powf(signed / 20.0);
                let actual = f64::from(audio[audio.len() - 4]) / 0.25;
                assert!((20.0 * (actual / expected).log10()).abs() < GAIN_ERROR_DB);
            }
        }
    }
}

#[test]
fn activation_hysteresis_hold_and_release_match_sample_timing_oracle() {
    for mode in [GateMode::Upward, GateMode::Duck] {
        for linked in [false, true] {
            let mut params = configuration(mode, linked);
            params.hysteresis_db = 4.0;
            params.hold_ms = 2.0;
            let mut plugin = make(2, RATE, params);
            let sign = if mode == GateMode::Upward { 1.0 } else { -1.0 };
            let quiet = block(&mut plugin, RATE, &[-26.0; 2], 64);
            assert!(
                quiet
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|frame| frame[0] == 0.25)
            );
            let rising = block(&mut plugin, RATE, &[-17.0; 2], 12_000);
            for (index, frame) in rising.as_chunks::<4>().0.iter().enumerate() {
                let expected = sign * 9.0 * (1.0 - (-(index as f64 + 1.0) / 48.0).exp());
                assert!((measured_gain(frame[0], 0.25) - expected).abs() < GAIN_ERROR_DB);
            }
            // Activation -23 dB, closing -27 dB. Keep the last 9 dB target in between.
            let latched = block(&mut plugin, RATE, &[-25.0; 2], 1024);
            assert!(latched.as_chunks::<4>().0.iter().all(|frame| (measured_gain(frame[0], 0.25) - sign * 9.0).abs() < GAIN_ERROR_DB));
            let falling = block(&mut plugin, RATE, &[-28.0; 2], 96 + 1440);
            for (index, frame) in falling.as_chunks::<4>().0.iter().enumerate() {
                let released = (index + 1).saturating_sub(96);
                let expected = sign * 9.0 * (-(released as f64) / 480.0).exp();
                assert!((measured_gain(frame[0], 0.25) - expected).abs() < GAIN_ERROR_DB);
                if released == 0 {
                    assert_eq!(frame[0], latched[0]);
                }
            }
        }
    }
}

#[test]
fn mode_and_boost_contracts_validate_before_mutation_and_preserve_old_json() {
    use sotf_host::param_specs::UpdateMode;
    use sotf_host::plugin_params::PluginParamDef;
    use sotf_plugin_gate::params::{PARAMS, Params};

    let keys = [
        "threshold",
        "ratio",
        "attack",
        "hold",
        "release",
        "mix",
        "link_channels",
        "sidechain_hpf_hz",
        "sidechain_hpf_order",
        "detection_mode",
        "sidechain_external",
        "range_db",
        "hysteresis_db",
        "knee_db",
        "lookahead_ms",
        "mode",
        "max_boost_db",
    ];
    assert_eq!(
        PARAMS
            .iter()
            .map(|spec| spec.engine_key)
            .collect::<Vec<_>>(),
        keys
    );
    assert_eq!(PARAMS[15].update_mode, UpdateMode::Structural);
    assert_eq!(PARAMS[16].update_mode, UpdateMode::Realtime);
    let state: Params = serde_json::from_str("{}").unwrap();
    assert_eq!(state.param_value(15), Some(0.0));
    assert_eq!(state.param_value(16), Some(12.0));
    let old: GatePluginParams = serde_json::from_str("{}").unwrap();
    assert_eq!(old.mode, GateMode::Downward);
    assert_eq!(old.max_boost_db, 12.0);
    for (json, mode) in [
        ("\"upward\"", GateMode::Upward),
        ("1.0", GateMode::Upward),
        ("2", GateMode::Duck),
        ("\"DOWNWARD\"", GateMode::Downward),
    ] {
        let params: GatePluginParams =
            serde_json::from_str(&format!("{{\"mode\":{json}}}")).unwrap();
        let state: Params = serde_json::from_str(&format!("{{\"mode\":{json}}}")).unwrap();
        assert_eq!(state.mode, mode);
        assert_eq!(params.mode, mode);
        assert_eq!(
            serde_json::from_str::<GatePluginParams>(&serde_json::to_string(&params).unwrap())
                .unwrap()
                .mode,
            mode
        );
    }
    for json in ["3", "-1", "0.5", "null", "true", "\"unknown\""] {
        assert!(serde_json::from_str::<GatePluginParams>(&format!("{{\"mode\":{json}}}")).is_err());
    }
    for cap in [f32::NAN, f32::INFINITY, -0.1, 24.1] {
        let mut params = configuration(GateMode::Upward, true);
        params.max_boost_db = cap;
        assert!(GatePlugin::try_from_params(2, params).is_err());
    }
    let mut plugin = make(2, RATE, configuration(GateMode::Upward, true));
    let mut control = make(2, RATE, configuration(GateMode::Upward, true));
    assert_eq!(
        block(&mut plugin, RATE, &[-17.0; 2], 71),
        block(&mut control, RATE, &[-17.0; 2], 71)
    );
    let before = plugin.current_values();
    plugin
        .parametric_set_parameter(ParameterId::from("mode"), ParameterValue::Int(1))
        .unwrap();
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("mode"), ParameterValue::Int(2))
            .is_err()
    );
    let mut batch = ParameterSet::new();
    batch.insert(
        ParameterId::from("max_boost_db"),
        ParameterValue::Float(3.0),
    );
    batch.insert(ParameterId::from("mode"), ParameterValue::Int(0));
    assert!(plugin.apply_values(batch).is_err());
    assert_eq!(plugin.current_values(), before);
    assert_eq!(
        block(&mut plugin, RATE, &[-17.0; 2], 321),
        block(&mut control, RATE, &[-17.0; 2], 321)
    );
    plugin
        .parametric_set_parameter(
            ParameterId::from("max_boost_db"),
            ParameterValue::Float(0.0),
        )
        .unwrap();
    let disabled = block(&mut plugin, RATE, &[-17.0; 2], 1);
    assert_eq!(disabled[0], 0.25);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("max_boost_db")),
        Some(ParameterValue::Float(0.0))
    );
}

#[test]
fn signed_telemetry_preserves_nonnegative_attenuation_and_held_snapshots() {
    for mode in [GateMode::Upward, GateMode::Duck] {
        let mut plugin = make(2, RATE, configuration(mode, true));
        block(&mut plugin, RATE, &[-17.0, -30.0], 12_000);
        let held = plugin.get_data().unwrap().downcast::<GateData>().unwrap();
        let expected = if mode == GateMode::Upward { 9.0 } else { -9.0 };
        for &gain in held.gain_db.iter() {
            assert!((f64::from(gain) - expected).abs() < GAIN_ERROR_DB);
        }
        let attenuation = if mode == GateMode::Upward { 0.0 } else { 9.0 };
        for &value in held.attenuation_db.iter() {
            assert!((f64::from(value) - attenuation).abs() < GAIN_ERROR_DB);
        }
        assert_eq!(held.mode, mode);
        assert!(held.effect_active && held.gate_open);
        assert_eq!(held.is_open, mode == GateMode::Upward);
        let signed = held.gain_db.clone();
        let copy = signed.as_ref().clone();
        block(&mut plugin, RATE, &[-40.0; 2], 12_000);
        assert_eq!(*signed, copy);
        let now = plugin.get_data().unwrap().downcast::<GateData>().unwrap();
        assert!(!now.effect_active && !now.gate_open && now.is_open);
    }
}

fn render_partitioned(plugin: &mut GatePlugin, frames: usize, partitions: &[usize]) -> Vec<f32> {
    let mut rendered = Vec::with_capacity(frames * 4);
    let mut frame = 0;
    for &capacity in partitions.iter().cycle() {
        if frame == frames {
            break;
        }
        let count = capacity.min(frames - frame);
        let mut buffer = Vec::with_capacity(count * 4);
        for index in frame..frame + count {
            let signal = (index as f64 * 0.13).sin() as f32 * 0.4;
            let detector_db = [-30.0, -19.0, -15.0, -25.0][index / 71 % 4];
            buffer.extend([
                signal,
                signal * -0.7,
                amplitude(detector_db),
                amplitude(detector_db - 3.0),
            ]);
        }
        plugin
            .process_in_place(&mut buffer, &ProcessContext::new(RATE, count))
            .unwrap();
        rendered.extend(buffer);
        frame += count;
    }
    rendered
}

#[test]
fn automation_partition_reset_and_rms_lookahead_state_remain_deterministic() {
    for mode in [GateMode::Upward, GateMode::Duck] {
        for linked in [false, true] {
            for filtered in [false, true] {
                let mut params = configuration(mode, linked);
                params.hold_ms = 2.0;
                params.hysteresis_db = 2.0;
                params.lookahead_ms = 1.0;
                if filtered {
                    params.detection_mode = "rms".into();
                    params.sidechain_hpf_hz = 120.0;
                    params.sidechain_hpf_order = "4th".into();
                }
                let mut whole = make(2, RATE, params.clone());
                let mut split = make(2, RATE, params.clone());
                for (threshold, knee, boost) in [
                    (-20.0, 6.0, 12.0),
                    (-24.0, 12.0, 3.0),
                    (-10.0, 0.0, 0.0),
                    (-20.0, 6.0, 24.0),
                ] {
                    for plugin in [&mut whole, &mut split] {
                        for (key, value) in [
                            ("threshold", threshold),
                            ("knee_db", knee),
                            ("max_boost_db", boost),
                        ] {
                            plugin
                                .parametric_set_parameter(
                                    ParameterId::from(key),
                                    ParameterValue::Float(value),
                                )
                                .unwrap();
                        }
                    }
                    assert_eq!(
                        render_partitioned(&mut whole, 2048, &[2048]),
                        render_partitioned(&mut split, 2048, &[1, 7, 31, 256])
                    );
                }
                whole.reset();
                params.max_boost_db = 24.0;
                let mut fresh = make(2, RATE, params);
                assert_eq!(
                    render_partitioned(&mut whole, 2048, &[11, 61]),
                    render_partitioned(&mut fresh, 2048, &[2048])
                );
            }
        }
    }
}

#[test]
fn lookahead_delays_program_while_detector_and_gain_advance_at_current_time() {
    for mode in [GateMode::Upward, GateMode::Duck] {
        let mut params = configuration(mode, false);
        params.lookahead_ms = 1.0;
        let mut plugin = make(1, RATE, params);
        assert_eq!(plugin.latency_samples(), 48);
        let mut audio = vec![0.0; 256 * 2];
        audio[0] = 0.25;
        for frame in audio.as_chunks_mut::<2>().0 {
            frame[1] = amplitude(-17.0);
        }
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(RATE, 256))
            .unwrap();
        let amount = 9.0 * (1.0 - (-49.0_f64 / 48.0).exp());
        let expected = if mode == GateMode::Upward {
            amount
        } else {
            -amount
        };
        for (frame, samples) in audio.as_chunks::<2>().0.iter().enumerate() {
            if frame == 48 {
                assert!((measured_gain(samples[0], 0.25) - expected).abs() < GAIN_ERROR_DB);
            } else {
                assert_eq!(samples[0], 0.0);
            }
        }
    }
}

#[test]
fn finite_peak_extremes_and_nonfinite_samples_cannot_escape_the_gain_bound() {
    for mode in [GateMode::Upward, GateMode::Duck] {
        for linked in [false, true] {
            let mut params = configuration(mode, linked);
            params.max_boost_db = 24.0;
            let mut plugin = make(2, RATE, params);
            block(&mut plugin, RATE, &[-10.0; 2], 12_000);
            let mut extremes = vec![f32::MAX, -f32::MAX, f32::MAX, -f32::MAX];
            plugin
                .process_in_place(&mut extremes, &ProcessContext::new(RATE, 1))
                .unwrap();
            assert!(extremes.iter().all(|value| value.is_finite()));
            assert!(extremes[0] > 0.0 && extremes[1] < 0.0);
            if mode == GateMode::Upward {
                assert_eq!(extremes[0], f32::MAX);
                assert_eq!(extremes[1], -f32::MAX);
            }
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut invalid_program = [invalid, invalid, 0.1, 0.1];
                plugin
                    .process_in_place(&mut invalid_program, &ProcessContext::new(RATE, 1))
                    .unwrap();
                assert_eq!(invalid_program[0], 0.0);
                assert_eq!(invalid_program[1], 0.0);
                let mut invalid_detector = [0.25, -0.125, invalid, invalid];
                plugin
                    .process_in_place(&mut invalid_detector, &ProcessContext::new(RATE, 1))
                    .unwrap();
                assert!(invalid_detector[0].is_finite() && invalid_detector[1].is_finite());
                assert_eq!(invalid_detector[2].to_bits(), invalid.to_bits());
            }
            let recovered = block(&mut plugin, RATE, &[-120.0; 2], 12_000);
            assert!((measured_gain(recovered[recovered.len() - 4], 0.25)).abs() < GAIN_ERROR_DB);
        }
    }
}

#[test]
fn old_json_and_explicit_downward_mode_produce_identical_audio() {
    let old: GatePluginParams = serde_json::from_str(
        r#"{"threshold_db":-20.0,"ratio":4.0,"knee_db":6.0,"sidechain_external":true}"#,
    )
    .unwrap();
    let mut explicit = old.clone();
    explicit.mode = GateMode::Downward;
    explicit.max_boost_db = 0.0;
    let mut implicit = make(2, RATE, old);
    let mut explicit = make(2, RATE, explicit);
    assert_eq!(
        render_partitioned(&mut implicit, 24_000, &[256]),
        render_partitioned(&mut explicit, 24_000, &[1, 61, 1024])
    );
}
