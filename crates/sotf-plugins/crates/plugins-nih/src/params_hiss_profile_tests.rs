//! Hiss native captured-profile persistence tests.
//!
//! Drives actual `DynamicParams` plus bridge Hiss plugins through live
//! capture, control-thread export, save, fresh restore, and bit-exact
//! render with EOF. Seeded blobs appear only in matrix legs; capture legs
//! use the live 1 s engine. No fixtures are generated.

// Rust guideline compliant 2026-02-21
use super::hiss_profile::{
    HISS_NATIVE_CHANNELS, HISS_PROFILE_STATE_FIELD, HissCaptureAction, decode_hiss_field,
    encode_hiss_busy, encode_hiss_field, hiss_cancel_capture, hiss_capture_control,
    hiss_clear_profile, hiss_snapshot, hiss_start_capture,
};
use super::{DynamicParams, HissProfileRestoreAttempt, ParamKind};
use nih_plug::prelude::{Param, Params};
use nih_plug::wrapper::state::{ParamValue, PluginState};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData;
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE_48K: u32 = 48_000;
const RATE_96K: u32 = 96_000;
const HISS_HOPS_48K: u64 = 184;
const SPECTRAL_TAIL_ALIGNED: usize = 2 * 1024 - 256;

fn hiss_infos() -> Vec<BridgedParamInfo> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("HissReducer"));
    (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect()
}

fn hiss_params() -> Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("HissReducer", &hiss_infos())
}

fn current_state(params: &DynamicParams) -> PluginState {
    let mut values = params
        .sync_entries
        .iter()
        .map(|entry| {
            let value = match entry.kind {
                ParamKind::Float => {
                    ParamValue::F32(params.float_params[entry.index].unmodulated_plain_value())
                }
                ParamKind::Int => {
                    ParamValue::I32(params.int_params[entry.index].unmodulated_plain_value())
                }
                ParamKind::Bool => {
                    ParamValue::Bool(params.bool_params[entry.index].unmodulated_plain_value())
                }
            };
            (entry.id.as_str().to_owned(), value)
        })
        .collect::<BTreeMap<_, _>>();
    values.extend(params.serialize_parameter_overrides());
    PluginState {
        version: "hiss-native-test-state".to_owned(),
        params: values,
        fields: params.serialize_fields(),
    }
}

fn apply_incoming_params(params: &DynamicParams, incoming: &BTreeMap<String, ParamValue>) {
    for (id, value) in incoming {
        let Some(entry) = params.param_map.get(id) else {
            continue;
        };
        match (entry.kind, value) {
            (ParamKind::Float, ParamValue::F32(v)) => {
                params.float_params[entry.index].set_plain_value_for_initialization(*v);
            }
            (ParamKind::Bool, ParamValue::Bool(v)) => {
                params.bool_params[entry.index].set_plain_value_for_initialization(*v);
            }
            (ParamKind::Int, ParamValue::I32(v)) => {
                params.int_params[entry.index].set_plain_value_for_initialization(*v);
            }
            _ => {}
        }
    }
}

fn lcg_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

fn first_difference_hiss(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let white = lcg_noise(frames, 1.0, seed);
    let mut previous = 0.0f32;
    white
        .iter()
        .map(|&sample| {
            let high = amplitude * (sample - previous);
            previous = sample;
            high
        })
        .collect()
}

fn sine_tone(frames: usize, amplitude: f32, freq_hz: f64, rate: u32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (f64::from(amplitude)
                * (2.0 * std::f64::consts::PI * freq_hz * i as f64 / f64::from(rate)).sin())
                as f32
        })
        .collect()
}

fn interleave_dual_mono(mono: &[f32]) -> Vec<f32> {
    let mut stereo = Vec::with_capacity(mono.len() * 2);
    for &sample in mono {
        stereo.push(sample);
        stereo.push(sample);
    }
    stereo
}

fn regional_mean(spectrum: &[f32], lo_bin: usize, hi_bin: usize) -> f64 {
    let slice = &spectrum[lo_bin..=hi_bin];
    slice.iter().map(|p| f64::from(*p)).sum::<f64>() / slice.len() as f64
}

fn peak(output: &[f32]) -> f32 {
    output.iter().map(|s| s.abs()).fold(0.0, f32::max)
}

fn seeded_v1(channels: usize, rate: u32) -> NoiseProfileData {
    let floors = (0..channels).map(|ch| -40.0 - ch as f32).collect();
    NoiseProfileData {
        format_version: 1,
        sample_rate: rate,
        channels,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: floors,
        frames_analyzed: u64::from(rate),
        spectral: None,
    }
}

fn seeded_v2(channels: usize, rate: u32) -> NoiseProfileData {
    let powers = vec![1.0f32; channels * 513];
    NoiseProfileData {
        format_version: 2,
        sample_rate: rate,
        channels,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: (0..channels).map(|ch| -40.0 - ch as f32).collect(),
        frames_analyzed: u64::from(rate),
        spectral: Some(
            sotf_plugins::plugin_hiss_reducer::profile::SpectralProfileData {
                fft_size: 1024,
                hop_size: 256,
                window: "hann-periodic".to_string(),
                sample_rate: rate,
                channels,
                num_bins: 513,
                power_per_channel_bin: powers,
                hops_analyzed: HISS_HOPS_48K,
            },
        ),
    }
}

fn process_blocks(
    plugin: &mut dyn Plugin,
    input: &[f32],
    rate: u32,
    block_frames: usize,
) -> Vec<f32> {
    let inputs = plugin.input_channels();
    let outputs = plugin.output_channels();
    assert_eq!(input.len() % inputs, 0);
    let frames = input.len() / inputs;
    let mut output = vec![f32::NAN; frames * outputs];
    for start in (0..frames).step_by(block_frames) {
        let count = block_frames.min(frames - start);
        let context = ProcessContext::new(rate, count);
        let produced = plugin
            .process(
                &input[start * inputs..(start + count) * inputs],
                &mut output[start * outputs..(start + count) * outputs],
                &context,
            )
            .unwrap();
        assert_eq!(produced, count);
    }
    assert!(output.iter().all(|v| v.is_finite()));
    output
}

fn process_with_drain(plugin: &mut dyn Plugin, input: &[f32], rate: u32) -> Vec<f32> {
    let mut full = process_blocks(plugin, input, rate, 256);
    let channels = plugin.output_channels();
    for _ in 0..4096 {
        let mut block = vec![0.0f32; 256 * channels];
        let status = plugin
            .drain(&mut block, &ProcessContext::new(rate, 256))
            .unwrap();
        full.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            assert!(full.iter().all(|v| v.is_finite()));
            return full;
        }
    }
    panic!("drain did not complete");
}

fn install_snapshot(params: &DynamicParams, plugin: &dyn Plugin) {
    let snapshot = hiss_snapshot(plugin).expect("Hiss must publish get_data");
    params.install_hiss_snapshot(snapshot);
}

fn construction_config(
    params: &DynamicParams,
    profile: Option<&NoiseProfileData>,
) -> String {
    let bool_value = |id: &str| match params.value(id) {
        Some(ParameterValue::Bool(v)) => v,
        _ => panic!("missing Hiss bool {id}"),
    };
    let float_value = |id: &str| match params.value(id) {
        Some(ParameterValue::Float(v)) => v,
        _ => panic!("missing Hiss float {id}"),
    };
    let int_value = |id: &str| match params.value(id) {
        Some(ParameterValue::Int(v)) => v,
        _ => panic!("missing Hiss int {id}"),
    };
    let mut config = serde_json::json!({
        "enabled": bool_value("enabled"),
        "threshold_db": float_value("threshold_db"),
        "frequency_hz": float_value("frequency_hz"),
        "strength": float_value("strength"),
        "spectral_mode": bool_value("spectral_mode"),
        "use_captured_profile": bool_value("use_captured_profile"),
        "curve_low": float_value("curve_low"),
        "curve_mid": float_value("curve_mid"),
        "curve_high": float_value("curve_high"),
        "link_mode": int_value("link_mode"),
        "transient_guard": bool_value("transient_guard"),
    });
    if let Some(profile) = profile {
        config["captured_profile"] = serde_json::to_value(profile).unwrap();
    }
    serde_json::to_string(&config).unwrap()
}

#[test]
fn hiss_native_actual_capture_save_fresh_load_renders_bitexact_with_eof() {
    let source_params = hiss_params();
    let mut source =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, r#"{"spectral_mode": true, "strength": 0.85}"#)
            .unwrap();
    source.initialize(RATE_48K).unwrap();
    apply_incoming_params(
        &source_params,
        &BTreeMap::from([
            ("spectral_mode".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
    );
    assert!(source.get_data().is_some());
    install_snapshot(&source_params, &*source);
    assert!(
        hiss_snapshot(&*source)
            .unwrap()
            .try_export()
            .unwrap()
            .is_none()
    );

    let colored_mono = first_difference_hiss(RATE_48K as usize, 0.035, 0xe940c);
    let colored = interleave_dual_mono(&colored_mono);
    hiss_start_capture(source.as_mut()).unwrap();
    assert_eq!(
        source.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(true))
    );
    process_blocks(source.as_mut(), &colored, RATE_48K, 256);
    assert_eq!(
        source.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(false))
    );
    let export = hiss_snapshot(&*source)
        .unwrap()
        .try_export()
        .unwrap()
        .expect("capture must export");
    assert_eq!(export.profile.format_version, 2);
    assert_eq!(export.profile.channels, 2);
    assert_eq!(
        export.profile.spectral.as_ref().unwrap().hops_analyzed,
        HISS_HOPS_48K
    );
    let powers = &export
        .profile
        .spectral
        .as_ref()
        .unwrap()
        .power_per_channel_bin;
    for ch in 0..2 {
        let band = &powers[ch * 513..(ch + 1) * 513];
        let ratio = regional_mean(band, 86, 149) / regional_mean(band, 341, 490);
        assert!(ratio < 0.5, "channel {ch} must be nonflat: {ratio:.3}");
    }

    source
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    apply_incoming_params(
        &source_params,
        &BTreeMap::from([(
            "use_captured_profile".to_string(),
            ParamValue::Bool(true),
        )]),
    );
    let saved = current_state(&source_params);
    assert!(
        matches!(
            saved.params.get("learn_noise"),
            Some(ParamValue::Bool(false))
        ),
        "saved learn_noise must be forced false"
    );
    assert!(
        matches!(
            saved.params.get("clear_profile"),
            Some(ParamValue::Bool(false))
        ),
        "saved clear_profile must be forced false"
    );
    let encoded = saved
        .fields
        .get(HISS_PROFILE_STATE_FIELD)
        .expect("Hiss field must save");
    let (generation, blob) =
        decode_hiss_field(encoded, HISS_NATIVE_CHANNELS).unwrap();
    assert_eq!(generation % 2, 0);
    let blob = blob.expect("capture must save a profile");
    assert_eq!(blob, export.profile);

    let fresh_params = hiss_params();
    assert!(fresh_params.validate_state(&saved, false, false, Some(48_000.0)));
    apply_incoming_params(&fresh_params, &saved.params);
    fresh_params.deserialize_fields(&saved.fields);
    let carried = fresh_params
        .hiss_profile_for_construction()
        .unwrap()
        .expect("fresh must carry a profile");
    assert_eq!(carried, export.profile);
    let config = construction_config(&fresh_params, Some(&carried));
    let mut fresh_attempt = HissProfileRestoreAttempt::new(fresh_params.clone());
    let mut fresh = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    fresh.initialize(RATE_48K).unwrap();
    fresh_params.complete_hiss_profile_restore();
    fresh_attempt.commit();
    install_snapshot(&fresh_params, &*fresh);
    assert_eq!(
        fresh.get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(
        hiss_snapshot(&*fresh)
            .unwrap()
            .try_export()
            .unwrap()
            .expect("fresh must export")
            .profile,
        export.profile
    );

    let hiss = lcg_noise(16384, 0.04, 0xe9f1);
    let tone = sine_tone(16384, 0.06, 9984.375, RATE_48K);
    let mix_mono: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let mix = interleave_dual_mono(&mix_mono);
    source.reset();
    fresh.reset();
    let before = process_with_drain(source.as_mut(), &mix, RATE_48K);
    let after = process_with_drain(fresh.as_mut(), &mix, RATE_48K);
    assert_eq!(before.len(), 16384 * 2 + SPECTRAL_TAIL_ALIGNED * 2);
    assert!(peak(&before) > 1e-6);
    assert_ne!(&before[..16384 * 2], &mix[..]);
    assert_eq!(before, after);
    assert!(source.get_data().is_some());
    assert!(fresh.get_data().is_some());
    assert!(source.latency_samples() > 0);
    assert_eq!(source.latency_samples(), fresh.latency_samples());
    assert!(matches!(fresh.tail_length(), TailLength::Finite(_)));
}

#[test]
fn hiss_native_time_domain_v1_capture_roundtrip_zero_tail() {
    let mut profiler =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, "{}").unwrap();
    profiler.initialize(RATE_48K).unwrap();
    let colored_mono = first_difference_hiss(RATE_48K as usize, 0.035, 0xa07c);
    let colored = interleave_dual_mono(&colored_mono);
    hiss_start_capture(profiler.as_mut()).unwrap();
    process_blocks(profiler.as_mut(), &colored, RATE_48K, 256);
    let v2 = hiss_snapshot(&*profiler)
        .unwrap()
        .try_export()
        .unwrap()
        .expect("capture must export")
        .profile;
    let v1 = NoiseProfileData {
        format_version: 1,
        sample_rate: v2.sample_rate,
        channels: v2.channels,
        measurement_cutoff_hz: v2.measurement_cutoff_hz,
        floor_db_per_channel: v2.floor_db_per_channel.clone(),
        frames_analyzed: v2.frames_analyzed,
        spectral: None,
    };
    v1.validate().unwrap();

    let source_params = hiss_params();
    let mut source =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, "{}").unwrap();
    source.initialize(RATE_48K).unwrap();
    install_snapshot(&source_params, &*source);
    let encoded = encode_hiss_field(7, Some(&v1));
    let incoming = PluginState {
        version: "hiss-v1".to_owned(),
        params: BTreeMap::from([(
            "use_captured_profile".to_string(),
            ParamValue::Bool(true),
        )]),
        fields: BTreeMap::from([(HISS_PROFILE_STATE_FIELD.to_string(), encoded)]),
    };
    assert!(source_params.validate_state(&incoming, false, false, Some(48_000.0)));
    apply_incoming_params(&source_params, &incoming.params);
    source_params.deserialize_fields(&incoming.fields);
    let carried = source_params
        .hiss_profile_for_construction()
        .unwrap()
        .expect("v1 must carry");
    assert_eq!(carried, v1);
    let config = construction_config(&source_params, Some(&carried));
    let mut attempt = HissProfileRestoreAttempt::new(source_params.clone());
    source = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    source.initialize(RATE_48K).unwrap();
    source_params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&source_params, &*source);
    let saved = current_state(&source_params);
    let (_, back) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(back.unwrap(), v1);

    let fresh_params = hiss_params();
    assert!(fresh_params.validate_state(&saved, false, false, Some(48_000.0)));
    apply_incoming_params(&fresh_params, &saved.params);
    fresh_params.deserialize_fields(&saved.fields);
    let carried = fresh_params
        .hiss_profile_for_construction()
        .unwrap()
        .expect("fresh v1 must carry");
    let config = construction_config(&fresh_params, Some(&carried));
    let mut fresh_attempt = HissProfileRestoreAttempt::new(fresh_params.clone());
    let mut fresh = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    fresh.initialize(RATE_48K).unwrap();
    fresh_params.complete_hiss_profile_restore();
    fresh_attempt.commit();

    let input = interleave_dual_mono(&lcg_noise(16384, 0.02, 0x1d0e));
    source.reset();
    fresh.reset();
    let before = process_with_drain(source.as_mut(), &input, RATE_48K);
    let after = process_with_drain(fresh.as_mut(), &input, RATE_48K);
    assert_eq!(before.len(), 16384 * 2);
    assert_eq!(source.latency_samples(), 0);
    assert!(peak(&before) > 1e-6);
    assert_ne!(&before[..], &input[..]);
    assert_eq!(before, after);
}

#[test]
fn hiss_native_partial_omission_null_clear_and_use_off_retain() {
    let v2 = seeded_v2(2, RATE_48K);
    v2.validate().unwrap();
    let v1 = seeded_v1(2, RATE_48K);
    v1.validate().unwrap();

    let params = hiss_params();
    let mut plugin = plugins_bridge::create_plugin(
        "HissReducer",
        2,
        RATE_48K,
        r#"{"spectral_mode": true}"#,
    )
    .unwrap();
    plugin.initialize(RATE_48K).unwrap();
    install_snapshot(&params, &*plugin);
    apply_incoming_params(
        &params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    let full = PluginState {
        version: "hiss-full".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(11, Some(&v2)),
        )]),
    };
    assert!(params.validate_state(&full, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &full.params);
    params.deserialize_fields(&full.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let carried = params.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params, Some(&carried));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);

    let partial = PluginState {
        version: "hiss-partial".to_owned(),
        params: BTreeMap::from([("strength".to_string(), ParamValue::F32(0.6))]),
        fields: BTreeMap::new(),
    };
    assert!(params.validate_state(&partial, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &partial.params);
    params.deserialize_fields(&partial.fields);
    let kept = params.hiss_profile_for_construction().unwrap().unwrap();
    assert_eq!(kept, v2);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let config = construction_config(&params, Some(&kept));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);
    let saved = current_state(&params);
    let (_, kept) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(kept.unwrap(), v2);

    let clear = PluginState {
        version: "hiss-clear".to_owned(),
        params: BTreeMap::new(),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(13, None),
        )]),
    };
    assert!(params.validate_state(&clear, false, false, Some(48_000.0)));
    params.deserialize_fields(&clear.fields);
    assert!(params.hiss_profile_for_construction().unwrap().is_none());
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let config = construction_config(&params, None);
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);
    let cleared = current_state(&params);
    let (_, cleared_profile) = decode_hiss_field(
        &cleared.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert!(cleared_profile.is_none());
    assert!(
        hiss_snapshot(&*plugin)
            .unwrap()
            .try_export()
            .unwrap()
            .is_none()
    );

    let v1_state = PluginState {
        version: "hiss-v1".to_owned(),
        params: BTreeMap::from([(
            "use_captured_profile".to_string(),
            ParamValue::Bool(true),
        )]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(17, Some(&v1)),
        )]),
    };
    assert!(params.validate_state(&v1_state, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &v1_state.params);
    params.deserialize_fields(&v1_state.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let carried = params.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params, Some(&carried));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);
    plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    apply_incoming_params(
        &params,
        &BTreeMap::from([(
            "use_captured_profile".to_string(),
            ParamValue::Bool(false),
        )]),
    );
    let saved = current_state(&params);
    let (_, retained) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(retained.unwrap(), v1);
    let export = hiss_snapshot(&*plugin)
        .unwrap()
        .try_export()
        .unwrap()
        .expect("payload survives use-off");
    assert_eq!(export.profile, v1);
    assert!(!export.use_flag && !export.engaged);
}

#[test]
fn hiss_native_crossrate_v2_matches_explicit_v1_fallback() {
    let v2_48 = seeded_v2(2, RATE_48K);
    let v1_48 = seeded_v1(2, RATE_48K);
    let input = interleave_dual_mono(&lcg_noise(16384, 0.04, 0x96c0));

    let params_v2 = hiss_params();
    let mut with_v2 =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_96K, r#"{"spectral_mode": true}"#)
            .unwrap();
    with_v2.initialize(RATE_96K).unwrap();
    install_snapshot(&params_v2, &*with_v2);
    apply_incoming_params(
        &params_v2,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    let state_v2 = PluginState {
        version: "hiss-xrate-v2".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(23, Some(&v2_48)),
        )]),
    };
    assert!(params_v2.validate_state(&state_v2, false, false, Some(96_000.0)));
    apply_incoming_params(&params_v2, &state_v2.params);
    params_v2.deserialize_fields(&state_v2.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params_v2.clone());
    let carried = params_v2.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params_v2, Some(&carried));
    with_v2 = plugins_bridge::create_plugin("HissReducer", 2, RATE_96K, &config).unwrap();
    with_v2.initialize(RATE_96K).unwrap();
    params_v2.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params_v2, &*with_v2);
    let saved = current_state(&params_v2);
    let (_, kept) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    let kept = kept.unwrap();
    assert_eq!(kept.sample_rate, RATE_48K);
    assert_eq!(kept, v2_48);
    assert!(
        !hiss_snapshot(&*with_v2)
            .unwrap()
            .try_export()
            .unwrap()
            .unwrap()
            .engaged
    );

    let params_v1 = hiss_params();
    let mut with_v1 =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_96K, r#"{"spectral_mode": true}"#)
            .unwrap();
    with_v1.initialize(RATE_96K).unwrap();
    install_snapshot(&params_v1, &*with_v1);
    apply_incoming_params(
        &params_v1,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    let state_v1 = PluginState {
        version: "hiss-xrate-v1".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(29, Some(&v1_48)),
        )]),
    };
    assert!(params_v1.validate_state(&state_v1, false, false, Some(96_000.0)));
    apply_incoming_params(&params_v1, &state_v1.params);
    params_v1.deserialize_fields(&state_v1.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params_v1.clone());
    let carried = params_v1.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params_v1, Some(&carried));
    with_v1 = plugins_bridge::create_plugin("HissReducer", 2, RATE_96K, &config).unwrap();
    with_v1.initialize(RATE_96K).unwrap();
    params_v1.complete_hiss_profile_restore();
    attempt.commit();

    with_v2.reset();
    with_v1.reset();
    let out_v2 = process_with_drain(with_v2.as_mut(), &input, RATE_96K);
    let out_v1 = process_with_drain(with_v1.as_mut(), &input, RATE_96K);
    assert_eq!(out_v2, out_v1);
    assert!(peak(&out_v2) > 1e-6);
}

#[test]
fn hiss_native_malformed_rejected_live_retained() {
    let v2 = seeded_v2(2, RATE_48K);
    let params = hiss_params();
    let mut plugin = plugins_bridge::create_plugin(
        "HissReducer",
        2,
        RATE_48K,
        r#"{"spectral_mode": true}"#,
    )
    .unwrap();
    plugin.initialize(RATE_48K).unwrap();
    install_snapshot(&params, &*plugin);
    apply_incoming_params(
        &params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    let good = PluginState {
        version: "hiss-good".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(31, Some(&v2)),
        )]),
    };
    assert!(params.validate_state(&good, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &good.params);
    params.deserialize_fields(&good.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let carried = params.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params, Some(&carried));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);

    plugin.reset();
    let input = interleave_dual_mono(&lcg_noise(8192, 0.04, 0x77aa));
    let reference = process_with_drain(plugin.as_mut(), &input, RATE_48K);
    let before_state = current_state(&params);
    let before_saved = serde_json::to_value(&before_state).unwrap();

    let base = serde_json::to_value(&v2).unwrap();
    let mut bad_version = base.clone();
    bad_version["format_version"] = serde_json::json!(99);
    let mut bad_fft = base.clone();
    bad_fft["spectral"]["fft_size"] = serde_json::json!(2048);
    let mut bad_len = base.clone();
    bad_len["spectral"]["power_per_channel_bin"]
        .as_array_mut()
        .unwrap()
        .pop();
    let mut bad_power = base.clone();
    bad_power["spectral"]["power_per_channel_bin"][11] = serde_json::json!(-1.0);
    let mut v1_with_spectral = base.clone();
    v1_with_spectral["format_version"] = serde_json::json!(1);
    let mut v2_without_spectral = base.clone();
    v2_without_spectral
        .as_object_mut()
        .unwrap()
        .remove("spectral");
    let bad_channels = serde_json::to_value(seeded_v2(1, RATE_48K)).unwrap();

    for (label, blob) in [
        ("bad-version", bad_version),
        ("bad-fft", bad_fft),
        ("bad-length", bad_len),
        ("negative-power", bad_power),
        ("v1-with-spectral", v1_with_spectral),
        ("v2-without-spectral", v2_without_spectral),
        ("bad-channels", bad_channels),
        ("non-object", serde_json::json!("nope")),
    ] {
        let encoded = serde_json::json!({
            "version": 1,
            "generation": 37,
            "captured_profile": blob,
        })
        .to_string();
        let candidate = PluginState {
            version: format!("hiss-bad-{label}"),
            params: BTreeMap::new(),
            fields: BTreeMap::from([(HISS_PROFILE_STATE_FIELD.to_string(), encoded)]),
        };
        assert!(
            !params.validate_state(&candidate, false, false, Some(48_000.0)),
            "{label} must fail validation"
        );
        assert!(
            decode_hiss_field(
                &candidate.fields[HISS_PROFILE_STATE_FIELD],
                HISS_NATIVE_CHANNELS
            )
            .is_err(),
            "{label} must fail decode"
        );
    }

    assert_eq!(serde_json::to_value(current_state(&params)).unwrap(), before_saved);
    assert_eq!(
        params.hiss_profile_for_construction().unwrap().unwrap(),
        v2
    );
    assert_eq!(
        hiss_snapshot(&*plugin)
            .unwrap()
            .try_export()
            .unwrap()
            .expect("live profile retained")
            .profile,
        v2
    );
    plugin.reset();
    assert_eq!(
        process_with_drain(plugin.as_mut(), &input, RATE_48K),
        reference
    );
}

#[test]
fn hiss_native_no_action_replay_and_momentaries_excluded() {
    use nih_plug::prelude::ParamFlags;

    let v2 = seeded_v2(2, RATE_48K);
    let params = hiss_params();
    for id in ["learn_noise", "clear_profile"] {
        let entry = params.param_map.get(id).unwrap();
        assert!(!entry.realtime, "{id} must not sync into running DSP");
        let flags = params.bool_params[entry.index].flags();
        assert!(flags.contains(ParamFlags::NON_AUTOMATABLE), "{id}");
        assert!(
            !flags.contains(ParamFlags::HIDDEN),
            "{id} must stay visible as a host action"
        );
    }
    let before_fingerprint = params.structural_fingerprint();
    let before_non_restartable = params.non_restartable_structural_fingerprint();
    for id in ["learn_noise", "clear_profile"] {
        let entry = params.param_map.get(id).unwrap();
        params.bool_params[entry.index].set_plain_value_for_initialization(true);
    }
    assert_eq!(params.structural_fingerprint(), before_fingerprint);
    assert_eq!(
        params.non_restartable_structural_fingerprint(),
        before_non_restartable
    );
    for id in ["learn_noise", "clear_profile"] {
        let entry = params.param_map.get(id).unwrap();
        params.bool_params[entry.index].set_plain_value_for_initialization(false);
    }

    let mut plugin =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, "{}").unwrap();
    plugin.initialize(RATE_48K).unwrap();
    install_snapshot(&params, &*plugin);
    let good = PluginState {
        version: "hiss-good".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.5)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(41, Some(&v2)),
        )]),
    };
    assert!(params.validate_state(&good, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &good.params);
    params.deserialize_fields(&good.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let carried = params.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params, Some(&carried));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);

    let legacy = PluginState {
        version: "hiss-legacy".to_owned(),
        params: BTreeMap::from([
            ("learn_noise".to_string(), ParamValue::Bool(true)),
            ("clear_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.75)),
        ]),
        fields: BTreeMap::new(),
    };
    assert!(params.validate_state(&legacy, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &legacy.params);
    params.deserialize_fields(&legacy.fields);
    assert_eq!(
        params.value("learn_noise"),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        params.value("clear_profile"),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        params.value("strength"),
        Some(ParameterValue::Float(0.75))
    );
    let kept = params.hiss_profile_for_construction().unwrap().unwrap();
    assert_eq!(kept, v2);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let config = construction_config(&params, Some(&kept));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(false))
    );
    let saved = current_state(&params);
    assert!(
        matches!(
            saved.params.get("learn_noise"),
            Some(ParamValue::Bool(false))
        ),
        "saved learn_noise must be forced false"
    );
    assert!(
        matches!(
            saved.params.get("clear_profile"),
            Some(ParamValue::Bool(false))
        ),
        "saved clear_profile must be forced false"
    );
    let (_, kept) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(kept.unwrap(), v2);

    params.sync_to_plugin(plugin.as_mut()).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(false))
    );

    assert!(hiss_start_capture(plugin.as_mut()).is_ok());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(true))
    );
    assert!(hiss_cancel_capture(plugin.as_mut()).is_ok());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(false))
    );
    assert!(hiss_clear_profile(plugin.as_mut()).is_ok());
    assert!(
        hiss_snapshot(&*plugin)
            .unwrap()
            .try_export()
            .unwrap()
            .is_none()
    );

    let input = interleave_dual_mono(&lcg_noise(2048, 0.25, 0x440));
    plugin.reset();
    assert!(peak(&process_blocks(plugin.as_mut(), &input, RATE_48K, 256)) > 0.05);
}

#[test]
fn hiss_native_busy_never_drops_blob() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let v2 = seeded_v2(2, RATE_48K);
    let params = hiss_params();
    let mut plugin = plugins_bridge::create_plugin(
        "HissReducer",
        2,
        RATE_48K,
        r#"{"spectral_mode": true}"#,
    )
    .unwrap();
    plugin.initialize(RATE_48K).unwrap();
    install_snapshot(&params, &*plugin);
    apply_incoming_params(
        &params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    let good = PluginState {
        version: "hiss-good".to_owned(),
        params: BTreeMap::from([(
            "use_captured_profile".to_string(),
            ParamValue::Bool(true),
        )]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(43, Some(&v2)),
        )]),
    };
    assert!(params.validate_state(&good, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &good.params);
    params.deserialize_fields(&good.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let carried = params.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params, Some(&carried));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);
    let prime = current_state(&params);
    let (_, prime_blob) = decode_hiss_field(
        &prime.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(prime_blob.unwrap(), v2);

    let snapshot = hiss_snapshot(&*plugin).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let churn_stop = stop.clone();
    let churn = std::thread::spawn(move || {
        let mut flag = false;
        while !churn_stop.load(Ordering::Relaxed) {
            flag = !flag;
            snapshot.publish_live_flags(flag, RATE_48K);
        }
    });

    for _ in 0..200 {
        let fields = params.serialize_fields();
        let encoded = fields
            .get(HISS_PROFILE_STATE_FIELD)
            .expect("Busy must still carry the field");
        let (generation, blob) =
            decode_hiss_field(encoded, HISS_NATIVE_CHANNELS).expect("field must decode");
        assert_eq!(generation % 2, 0);
        assert_eq!(blob.expect("Busy must never drop the blob"), v2);
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
}

#[test]
fn hiss_native_process_path_allocates_and_locks_nothing() {
    let params = hiss_params();
    let mut plugin =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, r#"{"spectral_mode": true}"#)
            .unwrap();
    plugin.initialize(RATE_48K).unwrap();
    install_snapshot(&params, &*plugin);
    let v2 = seeded_v2(2, RATE_48K);
    let incoming = PluginState {
        version: "hiss-process".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.7)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(47, Some(&v2)),
        )]),
    };
    assert!(params.validate_state(&incoming, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &incoming.params);
    params.deserialize_fields(&incoming.fields);
    let mut attempt = HissProfileRestoreAttempt::new(params.clone());
    let carried = params.hiss_profile_for_construction().unwrap().unwrap();
    let config = construction_config(&params, Some(&carried));
    plugin = plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, &config).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    params.complete_hiss_profile_restore();
    attempt.commit();
    install_snapshot(&params, &*plugin);

    let input = interleave_dual_mono(&lcg_noise(512, 0.03, 0xbeef));
    let mut output = vec![0.0f32; 512 * 2];
    let context = ProcessContext::new(RATE_48K, 512);
    assert_no_alloc::assert_no_alloc(|| {
        params.sync_to_plugin(plugin.as_mut()).unwrap();
        let _ = params.structural_fingerprint();
        let _ = params.non_restartable_structural_fingerprint();
        let produced = plugin.process(&input, &mut output, &context).unwrap();
        assert_eq!(produced, 512);
    });
    assert!(output.iter().all(|v| v.is_finite()));
    assert!(peak(&output) > 1e-6);
    assert_eq!(
        params.hiss_profile_for_construction().unwrap().unwrap(),
        v2
    );
}

crate::sotf_nih_plugin!(
    HissWiringWrapper,
    plugin_type: "HissReducer",
    name: "Hiss Wiring Test",
    clap_id: "org.sotf.test.hiss-wiring",
    vst3_class_id: *b"SotfTestHiss0001",
    channels: 2
);

struct HissWiringInitContext;

impl nih_plug::prelude::InitContext<HissWiringWrapper> for HissWiringInitContext {
    fn plugin_api(&self) -> nih_plug::context::PluginApi {
        nih_plug::context::PluginApi::Clap
    }
    fn execute(&self, _: ()) {}
    fn set_latency_samples(&self, _: u32) {}
    fn set_current_voice_capacity(&self, _: u32) {}
}

fn hiss_wiring_initialize(wrapper: &mut HissWiringWrapper, rate: f32) {
    let layout = <HissWiringWrapper as nih_plug::prelude::Plugin>::AUDIO_IO_LAYOUTS[0];
    let config = nih_plug::prelude::BufferConfig {
        sample_rate: rate,
        min_buffer_size: Some(1),
        max_buffer_size: 512,
        process_mode: nih_plug::prelude::ProcessMode::Realtime,
    };
    assert!(
        <HissWiringWrapper as nih_plug::prelude::Plugin>::initialize(
            wrapper,
            &layout,
            &config,
            &mut HissWiringInitContext
        ),
        "Hiss wrapper must initialize at {rate} Hz"
    );
}

fn hiss_wiring_process_one(wrapper: &mut HissWiringWrapper, input: &[f32]) -> Vec<f32> {
    assert_eq!(input.len() % 2, 0);
    let frames = input.len() / 2;
    let mut main = vec![vec![0.0f32; frames]; 2];
    for ch in 0..2 {
        for frame in 0..frames {
            main[ch][frame] = input[frame * 2 + ch];
        }
    }
    let mut buffer = nih_plug::prelude::Buffer::default();
    // SAFETY: slices have `frames` samples and live through the callback.
    unsafe {
        buffer.set_slices(frames, |slices| {
            slices.extend(main.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let mut auxiliary = nih_plug::prelude::AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    let status = wrapper.process_with_transport(&mut buffer, &mut auxiliary, Default::default());
    assert!(
        !matches!(status, nih_plug::prelude::ProcessStatus::Error(_)),
        "Hiss wrapper process failed: {status:?}"
    );
    let mut output = vec![0.0f32; frames * 2];
    for (ch, samples) in buffer.as_slice_immutable().iter().enumerate() {
        for (frame, sample) in samples.iter().enumerate() {
            output[frame * 2 + ch] = *sample;
        }
    }
    assert!(output.iter().all(|v| v.is_finite()));
    output
}

fn hiss_wiring_process_blocks(
    wrapper: &mut HissWiringWrapper,
    input: &[f32],
    block_frames: usize,
) -> Vec<f32> {
    assert_eq!(input.len() % 2, 0);
    let frames = input.len() / 2;
    let mut output = Vec::with_capacity(input.len());
    for start in (0..frames).step_by(block_frames) {
        let count = block_frames.min(frames - start);
        output.extend(hiss_wiring_process_one(
            wrapper,
            &input[start * 2..(start + count) * 2],
        ));
    }
    output
}

#[test]
fn hiss_native_busy_with_no_known_profile_emits_retry_marker() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let params = hiss_params();
    let mut plugin =
        plugins_bridge::create_plugin("HissReducer", 2, RATE_48K, r#"{"spectral_mode": true}"#)
            .unwrap();
    plugin.initialize(RATE_48K).unwrap();
    install_snapshot(&params, &*plugin);
    let prime = current_state(&params);
    let (_, prime_blob) = decode_hiss_field(
        &prime.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert!(prime_blob.is_none());

    let snapshot = hiss_snapshot(&*plugin).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let churn_stop = stop.clone();
    let churn = std::thread::spawn(move || {
        let mut flag = false;
        while !churn_stop.load(Ordering::Relaxed) {
            flag = !flag;
            snapshot.publish_live_flags(flag, RATE_48K);
        }
    });

    let mut saw_null = false;
    let mut saw_busy = false;
    for _ in 0..200 {
        let fields = params.serialize_fields();
        let encoded = fields.get(HISS_PROFILE_STATE_FIELD).expect("field must save");
        match decode_hiss_field(encoded, HISS_NATIVE_CHANNELS) {
            Ok((_, None)) => saw_null = true,
            Ok((_, Some(_))) => panic!("no blob may appear from absence, even under churn"),
            Err(error) => {
                assert!(error.contains("busy"), "unknown error must name contention: {error}");
                saw_busy = true;
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
    assert!(saw_null, "churn must still permit confirmed-absence saves");
    let _ = saw_busy;

    let busy = PluginState {
        version: "hiss-busy".to_owned(),
        params: BTreeMap::new(),
        fields: BTreeMap::from([(HISS_PROFILE_STATE_FIELD.to_string(), encode_hiss_busy())]),
    };
    assert!(!params.validate_state(&busy, false, false, Some(48_000.0)));
    assert!(params.hiss_profile_for_construction().unwrap().is_none());
    let after = current_state(&params);
    match decode_hiss_field(&after.fields[HISS_PROFILE_STATE_FIELD], HISS_NATIVE_CHANNELS) {
        Ok((_, None)) => {}
        Ok((_, Some(_))) => panic!("busy restore must not invent a blob"),
        Err(error) => assert!(error.contains("busy"), "{error}"),
    }

    let colored = interleave_dual_mono(&first_difference_hiss(
        RATE_48K as usize,
        0.035,
        0xb5e,
    ));
    hiss_start_capture(plugin.as_mut()).unwrap();
    process_blocks(plugin.as_mut(), &colored, RATE_48K, 256);
    let captured = current_state(&params);
    let (_, captured_blob) = decode_hiss_field(
        &captured.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert!(captured_blob.is_some());
    hiss_clear_profile(plugin.as_mut()).unwrap();
    let cleared = current_state(&params);
    let (_, cleared_blob) = decode_hiss_field(
        &cleared.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert!(cleared_blob.is_none());

    let snapshot = hiss_snapshot(&*plugin).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let churn_stop = stop.clone();
    let churn = std::thread::spawn(move || {
        let mut flag = false;
        while !churn_stop.load(Ordering::Relaxed) {
            flag = !flag;
            snapshot.publish_live_flags(flag, RATE_48K);
        }
    });
    for _ in 0..100 {
        let fields = params.serialize_fields();
        match decode_hiss_field(&fields[HISS_PROFILE_STATE_FIELD], HISS_NATIVE_CHANNELS) {
            Ok((_, None)) => {}
            Ok((_, Some(_))) => panic!("cleared absence must not resurrect a blob under churn"),
            Err(error) => assert!(error.contains("busy"), "{error}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
}

#[test]
fn hiss_native_wrapper_capture_save_restore_renders_bitexact() {
    let mut source = HissWiringWrapper::default();
    apply_incoming_params(
        &source.params,
        &BTreeMap::from([
            ("spectral_mode".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
    );
    hiss_wiring_initialize(&mut source, 48_000.0);
    let live_inner = source.inner.as_mut().expect("wrapper must hold live DSP");
    hiss_capture_control(live_inner.as_mut(), HissCaptureAction::Start).unwrap();
    let colored = interleave_dual_mono(&first_difference_hiss(
        RATE_48K as usize,
        0.035,
        0xe940c,
    ));
    hiss_wiring_process_blocks(&mut source, &colored, 256);
    let entry = source.params.param_map.get("use_captured_profile").unwrap();
    source.params.bool_params[entry.index].set_plain_value_for_initialization(true);
    let saved = current_state(&source.params);
    let (generation, blob) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(generation % 2, 0);
    let profile = blob.expect("wrapper capture must save a profile");
    assert_eq!(profile.format_version, 2);
    assert_eq!(profile.channels, 2);

    let mut fresh = HissWiringWrapper::default();
    let fresh_params = fresh.params.clone();
    assert!(fresh_params.validate_state(&saved, false, false, Some(48_000.0)));
    apply_incoming_params(&fresh_params, &saved.params);
    fresh_params.deserialize_fields(&saved.fields);
    hiss_wiring_initialize(&mut fresh, 48_000.0);
    let fresh_saved = current_state(&fresh.params);
    let (_, fresh_blob) = decode_hiss_field(
        &fresh_saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(fresh_blob.unwrap(), profile);

    let hiss = lcg_noise(16384, 0.04, 0xe9f1);
    let tone = sine_tone(16384, 0.06, 9984.375, RATE_48K);
    let mix_mono: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let mix = interleave_dual_mono(&mix_mono);
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut source);
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut fresh);
    let before = hiss_wiring_process_blocks(&mut source, &mix, 256);
    let after = hiss_wiring_process_blocks(&mut fresh, &mix, 256);
    assert_eq!(before.len(), 16384 * 2);
    assert!(peak(&before) > 1e-6);
    assert_ne!(&before[..], &mix[..]);
    assert_eq!(before, after);
}

#[test]
fn hiss_native_wrapper_malformed_preserves_history() {
    let v2 = seeded_v2(2, RATE_48K);
    let mut wrapper = HissWiringWrapper::default();
    apply_incoming_params(
        &wrapper.params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    hiss_wiring_initialize(&mut wrapper, 48_000.0);
    let good = PluginState {
        version: "hiss-good".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(53, Some(&v2)),
        )]),
    };
    let params = wrapper.params.clone();
    assert!(params.validate_state(&good, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &good.params);
    params.deserialize_fields(&good.fields);
    hiss_wiring_initialize(&mut wrapper, 48_000.0);

    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut wrapper);
    let input = interleave_dual_mono(&lcg_noise(8192, 0.04, 0x77aa));
    let reference = hiss_wiring_process_blocks(&mut wrapper, &input, 256);
    let before_saved = serde_json::to_value(current_state(&params)).unwrap();

    let mut bad = serde_json::to_value(&v2).unwrap();
    bad["format_version"] = serde_json::json!(99);
    let encoded = serde_json::json!({
        "version": 1,
        "generation": 59,
        "captured_profile": bad,
    })
    .to_string();
    let candidate = PluginState {
        version: "hiss-bad".to_owned(),
        params: BTreeMap::new(),
        fields: BTreeMap::from([(HISS_PROFILE_STATE_FIELD.to_string(), encoded)]),
    };
    assert!(!params.validate_state(&candidate, false, false, Some(48_000.0)));
    assert_eq!(
        serde_json::to_value(current_state(&params)).unwrap(),
        before_saved
    );
    assert_eq!(params.hiss_profile_for_construction().unwrap().unwrap(), v2);
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut wrapper);
    assert_eq!(hiss_wiring_process_blocks(&mut wrapper, &input, 256), reference);
}

#[test]
fn hiss_native_legacy_generic_construction_carries_no_profile() {
    let params = DynamicParams::from_infos(&hiss_infos());
    assert!(params.hiss_profile_for_construction().unwrap().is_none());
    let mut plugin =
        super::configuration::create_plugin("HissReducer", RATE_48K, &params).unwrap();
    plugin.initialize(RATE_48K).unwrap();
    assert!(
        hiss_snapshot(&*plugin)
            .unwrap()
            .try_export()
            .unwrap()
            .is_none()
    );
}

/// Writes a Hiss host-action control as a host generic UI would.
///
/// Hosts mutate the same NIH bool through normalized setters; for these
/// unsmoothed bools that converges to the identical atomic state the
/// audio thread edge-consumes. Test-only driver for host-role writes.
fn set_hiss_toggle(params: &DynamicParams, id: &str, value: bool) {
    let entry = params.param_map.get(id).unwrap();
    assert!(matches!(entry.kind, ParamKind::Bool), "{id}");
    params.bool_params[entry.index].set_plain_value_for_initialization(value);
}

/// Drains the tail from a wrapper-owned live DSP instance.
///
/// Uses the production drain API on the instance the wrapper prepared,
/// initialized, and processed; hosts consume the same tail through
/// continued process calls after a tail status.
fn hiss_wiring_drain(wrapper: &mut HissWiringWrapper, rate: u32) -> Vec<f32> {
    let inner = wrapper.inner.as_mut().expect("live DSP");
    let channels = inner.output_channels();
    let mut tail = Vec::new();
    for _ in 0..4096 {
        let mut block = vec![0.0f32; 256 * channels];
        let status = inner
            .drain(&mut block, &ProcessContext::new(rate, 256))
            .unwrap();
        tail.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            assert!(tail.iter().all(|v| v.is_finite()));
            return tail;
        }
    }
    panic!("drain did not complete");
}

#[test]
fn hiss_native_host_action_capture_save_restore_bitexact_full_eof() {
    let mut source = HissWiringWrapper::default();
    apply_incoming_params(
        &source.params,
        &BTreeMap::from([
            ("spectral_mode".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
    );
    hiss_wiring_initialize(&mut source, 48_000.0);
    set_hiss_toggle(&source.params, "learn_noise", true);
    let colored = interleave_dual_mono(&first_difference_hiss(
        RATE_48K as usize,
        0.035,
        0xe940c,
    ));
    let half = colored.len() / 2;
    hiss_wiring_process_blocks(&mut source, &colored[..half], 256);
    let snapshot =
        hiss_snapshot(source.inner.as_ref().expect("live DSP").as_ref()).unwrap();
    let (active, progress) = snapshot.capture_state();
    assert!(active, "capture must run after the host checks Learn");
    assert!(
        (0.4f32..0.6).contains(&progress),
        "half the noise must read near half progress, got {progress}"
    );
    hiss_wiring_process_blocks(&mut source, &colored[half..], 256);
    let (active, progress) = snapshot.capture_state();
    assert!(
        !active && progress == 0.0,
        "completed capture must idle with zero progress"
    );
    let entry = source.params.param_map.get("use_captured_profile").unwrap();
    source.params.bool_params[entry.index].set_plain_value_for_initialization(true);
    let saved = current_state(&source.params);
    let (generation, blob) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(generation % 2, 0);
    let profile = blob.expect("host capture must save a measured profile");
    assert_eq!(profile.format_version, 2);
    assert_eq!(profile.channels, 2);
    assert_eq!(
        profile.spectral.as_ref().unwrap().hops_analyzed,
        HISS_HOPS_48K
    );
    let powers = &profile
        .spectral
        .as_ref()
        .unwrap()
        .power_per_channel_bin;
    for ch in 0..2 {
        let band = &powers[ch * 513..(ch + 1) * 513];
        let ratio = regional_mean(band, 86, 149) / regional_mean(band, 341, 490);
        assert!(ratio < 0.5, "channel {ch} must be nonflat: {ratio:.3}");
    }

    let mut fresh = HissWiringWrapper::default();
    let fresh_params = fresh.params.clone();
    assert!(fresh_params.validate_state(&saved, false, false, Some(48_000.0)));
    apply_incoming_params(&fresh_params, &saved.params);
    fresh_params.deserialize_fields(&saved.fields);
    hiss_wiring_initialize(&mut fresh, 48_000.0);
    let fresh_saved = current_state(&fresh.params);
    let (_, fresh_blob) = decode_hiss_field(
        &fresh_saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(fresh_blob.unwrap(), profile);

    let hiss = lcg_noise(16384, 0.04, 0xe9f1);
    let tone = sine_tone(16384, 0.06, 9984.375, RATE_48K);
    let mix_mono: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let mix = interleave_dual_mono(&mix_mono);
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut source);
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut fresh);
    let mut before = hiss_wiring_process_blocks(&mut source, &mix, 256);
    let mut after = hiss_wiring_process_blocks(&mut fresh, &mix, 256);
    before.extend(hiss_wiring_drain(&mut source, RATE_48K));
    after.extend(hiss_wiring_drain(&mut fresh, RATE_48K));
    assert_eq!(before.len(), 16384 * 2 + SPECTRAL_TAIL_ALIGNED * 2);
    assert!(peak(&before) > 1e-6);
    assert_ne!(&before[..16384 * 2], &mix[..]);
    assert_eq!(before, after);
    let source_inner = source.inner.as_ref().expect("live DSP");
    let fresh_inner = fresh.inner.as_ref().expect("live DSP");
    assert!(source_inner.get_data().is_some());
    assert!(fresh_inner.get_data().is_some());
    assert!(source_inner.latency_samples() > 0);
    assert_eq!(
        source_inner.latency_samples(),
        fresh_inner.latency_samples()
    );
    assert!(matches!(fresh_inner.tail_length(), TailLength::Finite(_)));
}

#[test]
fn hiss_native_host_action_cancel_preserves_accepted_profile() {
    let v2 = seeded_v2(2, RATE_48K);
    let mut wrapper = HissWiringWrapper::default();
    apply_incoming_params(
        &wrapper.params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    hiss_wiring_initialize(&mut wrapper, 48_000.0);
    let good = PluginState {
        version: "hiss-good".to_owned(),
        params: BTreeMap::from([
            ("use_captured_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.85)),
        ]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(67, Some(&v2)),
        )]),
    };
    let params = wrapper.params.clone();
    assert!(params.validate_state(&good, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &good.params);
    params.deserialize_fields(&good.fields);
    hiss_wiring_initialize(&mut wrapper, 48_000.0);

    let input = interleave_dual_mono(&lcg_noise(8192, 0.04, 0x77aa));
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut wrapper);
    let mut reference = hiss_wiring_process_blocks(&mut wrapper, &input, 256);
    reference.extend(hiss_wiring_drain(&mut wrapper, RATE_48K));

    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut wrapper);
    set_hiss_toggle(&params, "learn_noise", true);
    let partial = interleave_dual_mono(&first_difference_hiss(12000, 0.035, 0xca4ce1));
    hiss_wiring_process_blocks(&mut wrapper, &partial, 256);
    let snapshot = hiss_snapshot(wrapper.inner.as_ref().expect("live DSP").as_ref()).unwrap();
    let (active, progress) = snapshot.capture_state();
    assert!(active, "capture must run after the host checks Learn");
    assert!(
        (0.1f32..0.4).contains(&progress),
        "partial noise must read partial progress, got {progress}"
    );
    set_hiss_toggle(&params, "learn_noise", false);
    let tick = interleave_dual_mono(&lcg_noise(256, 0.01, 0x7eed));
    hiss_wiring_process_blocks(&mut wrapper, &tick, 256);
    let (active, progress) = snapshot.capture_state();
    assert!(!active && progress == 0.0, "cancel must idle the capture");
    let saved = current_state(&params);
    let (_, kept) = decode_hiss_field(
        &saved.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert_eq!(kept.unwrap(), v2);

    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut wrapper);
    let mut final_render = hiss_wiring_process_blocks(&mut wrapper, &input, 256);
    final_render.extend(hiss_wiring_drain(&mut wrapper, RATE_48K));
    assert_eq!(final_render, reference);
}

#[test]
fn hiss_native_host_action_clear_reports_explicit_null() {
    let v2 = seeded_v2(2, RATE_48K);
    let mut source = HissWiringWrapper::default();
    apply_incoming_params(
        &source.params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    hiss_wiring_initialize(&mut source, 48_000.0);
    let good = PluginState {
        version: "hiss-good".to_owned(),
        params: BTreeMap::from([(
            "use_captured_profile".to_string(),
            ParamValue::Bool(true),
        )]),
        fields: BTreeMap::from([(
            HISS_PROFILE_STATE_FIELD.to_string(),
            encode_hiss_field(71, Some(&v2)),
        )]),
    };
    let params = source.params.clone();
    assert!(params.validate_state(&good, false, false, Some(48_000.0)));
    apply_incoming_params(&params, &good.params);
    params.deserialize_fields(&good.fields);
    hiss_wiring_initialize(&mut source, 48_000.0);

    set_hiss_toggle(&params, "clear_profile", true);
    let tick = interleave_dual_mono(&lcg_noise(256, 0.01, 0xc1ea5));
    hiss_wiring_process_blocks(&mut source, &tick, 256);
    let cleared = current_state(&params);
    let (_, cleared_blob) = decode_hiss_field(
        &cleared.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert!(cleared_blob.is_none());
    let snapshot = hiss_snapshot(source.inner.as_ref().expect("live DSP").as_ref()).unwrap();
    assert!(
        snapshot.try_export().unwrap().is_none(),
        "cleared store must export nothing"
    );
    set_hiss_toggle(&params, "clear_profile", false);
    hiss_wiring_process_blocks(&mut source, &tick, 256);
    let rearmed = current_state(&params);
    let (_, rearmed_blob) = decode_hiss_field(
        &rearmed.fields[HISS_PROFILE_STATE_FIELD],
        HISS_NATIVE_CHANNELS,
    )
    .unwrap();
    assert!(rearmed_blob.is_none());

    let mut fresh = HissWiringWrapper::default();
    let fresh_params = fresh.params.clone();
    assert!(fresh_params.validate_state(&cleared, false, false, Some(48_000.0)));
    apply_incoming_params(&fresh_params, &cleared.params);
    fresh_params.deserialize_fields(&cleared.fields);
    hiss_wiring_initialize(&mut fresh, 48_000.0);
    let mix = interleave_dual_mono(&lcg_noise(4096, 0.04, 0xd0ff));
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut source);
    <HissWiringWrapper as nih_plug::prelude::Plugin>::reset(&mut fresh);
    let mut before = hiss_wiring_process_blocks(&mut source, &mix, 256);
    let mut after = hiss_wiring_process_blocks(&mut fresh, &mix, 256);
    before.extend(hiss_wiring_drain(&mut source, RATE_48K));
    after.extend(hiss_wiring_drain(&mut fresh, RATE_48K));
    assert!(peak(&before) > 1e-6);
    assert_eq!(before, after);
}

#[test]
fn hiss_native_host_action_momentaries_never_replay_or_rebuild() {
    let mut legacy = PluginState {
        version: "hiss-legacy".to_owned(),
        params: BTreeMap::from([
            ("learn_noise".to_string(), ParamValue::Bool(true)),
            ("clear_profile".to_string(), ParamValue::Bool(true)),
            ("strength".to_string(), ParamValue::F32(0.75)),
        ]),
        fields: BTreeMap::new(),
    };
    <HissWiringWrapper as nih_plug::prelude::Plugin>::filter_state(&mut legacy);
    assert!(matches!(
        legacy.params.get("learn_noise"),
        Some(ParamValue::Bool(false))
    ));
    assert!(matches!(
        legacy.params.get("clear_profile"),
        Some(ParamValue::Bool(false))
    ));
    assert!(matches!(
        legacy.params.get("strength"),
        Some(ParamValue::F32(0.75))
    ));

    let params = hiss_params();
    let racy = PluginState {
        version: "hiss-racy".to_owned(),
        params: BTreeMap::from([("learn_noise".to_string(), ParamValue::Bool(true))]),
        fields: BTreeMap::new(),
    };
    assert!(!params.validate_state(&racy, true, false, Some(48_000.0)));
    assert!(!params.validate_state(&racy, false, true, Some(48_000.0)));
    assert!(params.validate_state(&racy, false, false, Some(48_000.0)));

    let mut wrapper = HissWiringWrapper::default();
    hiss_wiring_initialize(&mut wrapper, 48_000.0);
    let fingerprint = wrapper.params.structural_fingerprint();
    let non_restartable = wrapper.params.non_restartable_structural_fingerprint();
    let silence = vec![0.0f32; 256 * 2];
    for value in [true, false, true, false] {
        set_hiss_toggle(&wrapper.params, "learn_noise", value);
        set_hiss_toggle(&wrapper.params, "clear_profile", value);
        let out = hiss_wiring_process_one(&mut wrapper, &silence);
        assert!(out.iter().all(|v| v.is_finite()));
    }
    assert_eq!(wrapper.params.structural_fingerprint(), fingerprint);
    assert_eq!(
        wrapper.params.non_restartable_structural_fingerprint(),
        non_restartable
    );
    let snapshot =
        hiss_snapshot(wrapper.inner.as_ref().expect("live DSP").as_ref()).unwrap();
    let (active, _) = snapshot.capture_state();
    assert!(!active, "toggles left off must idle the capture");
    assert!(snapshot.try_export().unwrap().is_none());
}

#[test]
fn hiss_native_host_action_edges_allocate_nothing() {
    let mut wrapper = HissWiringWrapper::default();
    apply_incoming_params(
        &wrapper.params,
        &BTreeMap::from([("spectral_mode".to_string(), ParamValue::Bool(true))]),
    );
    hiss_wiring_initialize(&mut wrapper, 48_000.0);
    let input = interleave_dual_mono(&lcg_noise(512, 0.03, 0xbeef));
    hiss_wiring_process_blocks(&mut wrapper, &input, 256);
    hiss_wiring_process_blocks(&mut wrapper, &input, 256);

    set_hiss_toggle(&wrapper.params, "learn_noise", true);
    assert_no_alloc::assert_no_alloc(|| {
        wrapper
            .params
            .forward_hiss_momentary_edges(
                wrapper.inner.as_mut().expect("live DSP").as_mut(),
                &mut wrapper.hiss_momentary,
            )
            .unwrap();
    });
    assert_eq!(
        wrapper
            .inner
            .as_ref()
            .expect("live DSP")
            .get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(true))
    );
    assert_no_alloc::assert_no_alloc(|| {
        wrapper
            .params
            .forward_hiss_momentary_edges(
                wrapper.inner.as_mut().expect("live DSP").as_mut(),
                &mut wrapper.hiss_momentary,
            )
            .unwrap();
    });
    set_hiss_toggle(&wrapper.params, "clear_profile", true);
    assert_no_alloc::assert_no_alloc(|| {
        wrapper
            .params
            .forward_hiss_momentary_edges(
                wrapper.inner.as_mut().expect("live DSP").as_mut(),
                &mut wrapper.hiss_momentary,
            )
            .unwrap();
    });
    let snapshot =
        hiss_snapshot(wrapper.inner.as_ref().expect("live DSP").as_ref()).unwrap();
    assert!(snapshot.try_export().unwrap().is_none());
    set_hiss_toggle(&wrapper.params, "learn_noise", false);
    assert_no_alloc::assert_no_alloc(|| {
        wrapper
            .params
            .forward_hiss_momentary_edges(
                wrapper.inner.as_mut().expect("live DSP").as_mut(),
                &mut wrapper.hiss_momentary,
            )
            .unwrap();
    });
    assert_eq!(
        wrapper
            .inner
            .as_ref()
            .expect("live DSP")
            .get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(false))
    );
}
