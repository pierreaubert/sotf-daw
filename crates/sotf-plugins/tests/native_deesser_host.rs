//! Loaded CLAP/VST3 tests for the native DeEsser sidechain route.
//!
//! UNEXECUTED: requires fresh de-esser binaries plus
//! `SOTF_TEST_DEESSER_CLAP_PLUGIN` / `SOTF_TEST_DEESSER_VST3_PLUGIN`.
//! Two test functions (one per format); every subcase is embedded, so the
//! runner expects exactly 2 executed with `--ignored` (2 ignored without).
//! Reuses the proven `native_compressor_host` backend APIs: sidechain
//! setup negotiation, fresh internal roundtrips (reload, self-restore,
//! rate reinit), hot/silent/swapped key routing, save/reload
//! continuation with external-config persistence, structural
//! restart/latency, refused detector flips in both directions with
//! retained-history twins, accepted latency renegotiation, valid
//! recovery, and preset-deserialize continuity (empty and dropped-setup
//! refusals with retained-history twins, matching recovery, internal
//! empty acceptance). Wrapper-to-DSP transparency itself is covered by the
//! `de_esser_routing` baseline; these tests prove the loaded host
//! routes the auxiliary key bus end to end.

// Rust guideline compliant 2026-02-21

#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

use sotf_host::external_plugin::{
    ExternalPlugin, ExternalPluginState, NativePluginAudioSetup, PluginDescriptor, PluginFormat,
    PluginScanStatus,
};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::serialization::{PluginPreset, SerializablePlugin};
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 512;
const INPUTS: usize = 4;
const OUTPUTS: usize = 2;
// VST3 hex of the `SotfDeEsser00001` class id in plugins-nih/src/lib.rs.
const DEESSER_VST3_ID: &str = "536F7466446545737365723030303031";

#[test]
#[ignore = "requires SOTF_TEST_DEESSER_CLAP_PLUGIN to point to the exported DeEsser CLAP library"]
fn exported_clap_deesser_sidechain_route_matches_keyed_contract() {
    verify_loaded_deesser_sidechain_route(
        PluginFormat::Clap,
        "SOTF_TEST_DEESSER_CLAP_PLUGIN",
        "org.spinorama.sotf.de-esser",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_DEESSER_VST3_PLUGIN to point to the exported DeEsser VST3 library"]
fn exported_vst3_deesser_sidechain_route_matches_keyed_contract() {
    verify_loaded_deesser_sidechain_route(
        PluginFormat::Vst3,
        "SOTF_TEST_DEESSER_VST3_PLUGIN",
        DEESSER_VST3_ID,
    );
}

fn sidechain_setup() -> NativePluginAudioSetup {
    NativePluginAudioSetup::Sidechain {
        main_channels: 2,
        key_channels: 2,
    }
}

fn test_descriptor(format: PluginFormat, library_env: &str, plugin_id: &str) -> PluginDescriptor {
    let library_path = PathBuf::from(
        std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
    );
    PluginDescriptor {
        id: plugin_id.into(),
        name: "SOTF: DeEsser".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: library_path,
        audio_inputs: INPUTS,
        audio_outputs: OUTPUTS,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

/// 4-channel interleaved probe: 8 kHz program on [0, 1], independent
/// 8 kHz key on [2, 3].
fn keyed_probe_block(start_frame: usize, program_peak: f32, key_peak: f32) -> Vec<f32> {
    keyed_probe_block_at(start_frame, program_peak, key_peak, SAMPLE_RATE)
}

fn keyed_probe_block_at(
    start_frame: usize,
    program_peak: f32,
    key_peak: f32,
    sample_rate: u32,
) -> Vec<f32> {
    let mut block = vec![0.0; FRAMES * INPUTS];
    for frame in 0..FRAMES {
        let t = (start_frame + frame) as f32 / sample_rate as f32;
        let program = program_peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        let key = key_peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        block[frame * INPUTS] = program;
        block[frame * INPUTS + 1] = program;
        block[frame * INPUTS + 2] = key;
        block[frame * INPUTS + 3] = key;
    }
    block
}

fn render_block(plugin: &mut ExternalPlugin, input: &[f32]) -> Vec<f32> {
    render_block_at(plugin, input, SAMPLE_RATE)
}

fn render_block_at(plugin: &mut ExternalPlugin, input: &[f32], sample_rate: u32) -> Vec<f32> {
    assert_eq!(input.len(), FRAMES * INPUTS);
    let context = ProcessContext::new(sample_rate, FRAMES);
    let mut output = vec![f32::NAN; FRAMES * OUTPUTS];
    assert_eq!(
        plugin.process(input, &mut output, &context).unwrap(),
        FRAMES
    );
    assert_eq!(output.len(), FRAMES * OUTPUTS);
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "loaded route rendered non-finite audio"
    );
    output
}

fn channel_rms(interleaved: &[f32], width: usize, channel: usize) -> f32 {
    let frames = interleaved.len() / width;
    let sum: f32 = (0..frames)
        .map(|frame| {
            let sample = interleaved[frame * width + channel];
            sample * sample
        })
        .sum();
    (sum / frames as f32).sqrt()
}

fn native_state_json(opaque_state: &[u8], format: PluginFormat) -> Option<serde_json::Value> {
    let payload = match format {
        PluginFormat::Clap => {
            let length_bytes: [u8; 8] = opaque_state.get(..8)?.try_into().ok()?;
            let length = usize::try_from(u64::from_le_bytes(length_bytes)).ok()?;
            let payload = opaque_state.get(8..)?;
            if payload.len() != length {
                return None;
            }
            payload
        }
        PluginFormat::Vst3 => opaque_state,
        PluginFormat::AudioUnit => return None,
    };
    serde_json::from_slice(payload).ok()
}

fn native_state_bool(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<bool> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("bool")?
        .as_bool()
}

fn native_state_int(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<i64> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("i32")?
        .as_i64()
}

fn set_native_state_param(
    opaque_state: &mut Vec<u8>,
    format: PluginFormat,
    id: &str,
    value: serde_json::Value,
) {
    let payload = match format {
        PluginFormat::Clap => {
            let length_bytes: [u8; 8] = opaque_state
                .get(..8)
                .expect("CLAP state includes its length prefix")
                .try_into()
                .expect("CLAP length prefix is eight bytes");
            let length = usize::try_from(u64::from_le_bytes(length_bytes))
                .expect("CLAP state length fits usize");
            let payload = opaque_state
                .get(8..)
                .expect("CLAP state includes its serialized body");
            assert_eq!(payload.len(), length);
            payload
        }
        PluginFormat::Vst3 => opaque_state.as_slice(),
        PluginFormat::AudioUnit => panic!("fixture is only for CLAP/VST3 state formats"),
    };
    let mut state: serde_json::Value =
        serde_json::from_slice(payload).expect("native state is NIH-plug JSON");
    let parameter = state
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|params| params.get_mut(id))
        .expect("serialized state contains requested parameter");
    *parameter = value;
    let serialized = serde_json::to_vec(&state).expect("serialize mutated native state");
    opaque_state.clear();
    if format == PluginFormat::Clap {
        opaque_state.extend_from_slice(&(serialized.len() as u64).to_le_bytes());
    }
    opaque_state.extend_from_slice(&serialized);
}

fn external_state_of(plugin: &ExternalPlugin, instance: &str) -> ExternalPluginState {
    plugin
        .serialize()
        .unwrap_or_else(|error| panic!("{instance} serialize: {error}"))
        .external_plugin_state()
        .unwrap_or_else(|error| panic!("{instance} read state envelope: {error}"))
        .unwrap_or_else(|| panic!("{instance} serialized external state"))
}

/// Wraps a (possibly mutated) state envelope back into a preset for
/// `deserialize`, keeping the live route's preset identity.
fn preset_from_state(
    plugin: &ExternalPlugin,
    instance: &str,
    state: &ExternalPluginState,
) -> PluginPreset {
    let mut preset = plugin
        .serialize()
        .unwrap_or_else(|error| panic!("{instance} serialize for preset: {error}"));
    preset
        .set_external_plugin_state(state)
        .unwrap_or_else(|error| panic!("{instance} embed preset state: {error}"));
    preset
}

/// Seeds the Wideband detector configuration into saved state and
/// rebuilds an instance from it; `external` selects the detector source.
fn keyed_instance(
    format: PluginFormat,
    instance: &str,
    seed: &ExternalPluginState,
    external: bool,
) -> ExternalPlugin {
    let mut keyed = seed.clone();
    for (id, value) in [
        ("sidechain_external", serde_json::json!({"bool": external})),
        ("mode", serde_json::json!({"i32": 0})),
        ("threshold", serde_json::json!({"f32": -20.0})),
        ("ratio", serde_json::json!({"f32": 8.0})),
        ("attack", serde_json::json!({"f32": 0.5})),
        ("release", serde_json::json!({"f32": 200.0})),
        ("mix", serde_json::json!({"f32": 1.0})),
        ("range_db", serde_json::json!({"f32": 60.0})),
        ("stereo_link", serde_json::json!({"f32": 0.0})),
        ("lookahead_ms", serde_json::json!({"f32": 0.0})),
    ] {
        set_native_state_param(&mut keyed.opaque_state, format, id, value);
    }
    let plugin = ExternalPlugin::from_placeholder_state(&keyed, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} keyed rebuild: {error}"));
    assert_eq!(plugin.input_channels(), INPUTS);
    assert_eq!(plugin.output_channels(), OUTPUTS);
    plugin
}

fn verify_loaded_deesser_sidechain_route(format: PluginFormat, library_env: &str, plugin_id: &str) {
    let instance = format!("DeEsser {format:?}");
    let descriptor = test_descriptor(format, library_env, plugin_id);

    // The sidechain setup negotiates the auxiliary key bus: 4 packed
    // inputs (program then key) and 2 program outputs.
    let mut default =
        ExternalPlugin::new_with_audio_setup(&descriptor, sidechain_setup(), SAMPLE_RATE)
            .unwrap_or_else(|error| panic!("{instance} load: {error}"));
    assert_eq!(default.input_channels(), INPUTS);
    assert_eq!(default.output_channels(), OUTPUTS);
    assert_eq!(default.latency_samples(), 0);
    let seed = external_state_of(&default, &instance);
    assert_eq!(seed.audio_setup, Some(sidechain_setup()));

    // Fresh internal key-capable route: its own saved state roundtrips
    // through placeholder reload, live self-restore, and sample-rate
    // reinit; the quiet program passes with the hot key ignored.
    let internal_saved = external_state_of(&default, &instance);
    assert_eq!(
        native_state_bool(&internal_saved.opaque_state, format, "sidechain_external"),
        Some(false),
        "{instance} fresh route must save internal detection"
    );
    let mut internal_reloaded =
        ExternalPlugin::from_placeholder_state(&internal_saved, SAMPLE_RATE)
            .unwrap_or_else(|error| panic!("{instance} internal reload: {error}"));
    assert_eq!(internal_reloaded.input_channels(), INPUTS);
    assert_eq!(internal_reloaded.output_channels(), OUTPUTS);
    let mut internal_program = Vec::new();
    let mut internal_output = Vec::new();
    for block_index in 0..4 {
        let input = keyed_probe_block(block_index * FRAMES, 0.05, 0.5);
        internal_program.extend_from_slice(&input);
        internal_output.extend_from_slice(&render_block(&mut internal_reloaded, &input));
    }
    let internal_db = 20.0
        * (channel_rms(&internal_output, OUTPUTS, 0) / channel_rms(&internal_program, INPUTS, 0))
            .log10();
    assert!(
        internal_db.abs() < 1.0,
        "{instance} internal route must pass within 1 dB, got {internal_db:.2} dB"
    );
    default
        .load_opaque_state(&internal_saved.opaque_state)
        .unwrap_or_else(|error| panic!("{instance} internal self-roundtrip: {error}"));
    default
        .initialize(44100.0)
        .unwrap_or_else(|error| panic!("{instance} reinit to 44100: {error:?}"));
    assert_eq!(default.input_channels(), INPUTS);
    assert_eq!(default.output_channels(), OUTPUTS);
    let mut reinit_program = Vec::new();
    let mut reinit_output = Vec::new();
    for block_index in 0..4 {
        let input = keyed_probe_block_at(block_index * FRAMES, 0.05, 0.5, 44100);
        reinit_program.extend_from_slice(&input);
        reinit_output.extend_from_slice(&render_block_at(&mut default, &input, 44100));
    }
    let reinit_db = 20.0
        * (channel_rms(&reinit_output, OUTPUTS, 0) / channel_rms(&reinit_program, INPUTS, 0))
            .log10();
    assert!(
        reinit_db.abs() < 1.0,
        "{instance} reinit route must pass within 1 dB, got {reinit_db:.2} dB"
    );
    default
        .initialize(f64::from(SAMPLE_RATE))
        .unwrap_or_else(|error| panic!("{instance} reinit to 48000: {error:?}"));

    // Hot key: live + twin populate engaged detectors and agree
    // bitwise; the settled program reduces without muting.
    let mut live = keyed_instance(format, &instance, &seed, true);
    let mut twin = keyed_instance(format, &instance, &seed, true);
    let mut hot_program = Vec::new();
    let mut hot_output = Vec::new();
    for block_index in 0..8 {
        let input = keyed_probe_block(block_index * FRAMES, 0.05, 0.5);
        let live_out = render_block(&mut live, &input);
        let twin_out = render_block(&mut twin, &input);
        assert_eq!(
            live_out, twin_out,
            "{instance} hot block {block_index} twin mismatch"
        );
        if block_index >= 4 {
            hot_program.extend_from_slice(&input);
            hot_output.extend_from_slice(&live_out);
        }
    }
    assert!(
        hot_output.iter().any(|sample| sample.abs() > 1.0e-3),
        "{instance} hot route silent"
    );
    let hot_db = 20.0
        * (channel_rms(&hot_output, OUTPUTS, 0) / channel_rms(&hot_program, INPUTS, 0)).log10();
    assert!(
        hot_db < -6.0,
        "{instance} hot key must reduce past -6 dB, got {hot_db:.2} dB"
    );
    assert!(
        hot_db > -20.0,
        "{instance} hot key must not mute, got {hot_db:.2} dB"
    );

    // Silent key passes the quiet program on a fresh keyed instance.
    let mut silent = keyed_instance(format, &instance, &seed, true);
    let mut silent_program = Vec::new();
    let mut silent_output = Vec::new();
    for block_index in 0..4 {
        let input = keyed_probe_block(block_index * FRAMES, 0.05, 0.0);
        silent_program.extend_from_slice(&input);
        silent_output.extend_from_slice(&render_block(&mut silent, &input));
    }
    let silent_db = 20.0
        * (channel_rms(&silent_output, OUTPUTS, 0) / channel_rms(&silent_program, INPUTS, 0))
            .log10();
    assert!(
        silent_db.abs() < 1.0,
        "{instance} silent key must pass within 1 dB, got {silent_db:.2} dB"
    );

    // Swapped buses pass the hot program: no key leak, no self-trigger.
    let mut swapped = keyed_instance(format, &instance, &seed, true);
    let mut swapped_program = Vec::new();
    let mut swapped_output = Vec::new();
    for block_index in 0..4 {
        let input = keyed_probe_block(block_index * FRAMES, 0.5, 0.05);
        swapped_program.extend_from_slice(&input);
        swapped_output.extend_from_slice(&render_block(&mut swapped, &input));
    }
    let swapped_db = 20.0
        * (channel_rms(&swapped_output, OUTPUTS, 0) / channel_rms(&swapped_program, INPUTS, 0))
            .log10();
    assert!(
        swapped_db.abs() < 1.0,
        "{instance} swapped buses must pass within 1 dB, got {swapped_db:.2} dB"
    );

    // Save/reload persists the external configuration: the rebuilt
    // instance renegotiates the key bus and renders bit-identical to
    // a fresh keyed instance (saved state carries parameters, while
    // detector history stays with the live route).
    let saved = external_state_of(&live, &instance);
    assert_eq!(saved.audio_setup, Some(sidechain_setup()));
    assert_eq!(
        native_state_bool(&saved.opaque_state, format, "sidechain_external"),
        Some(true),
        "{instance} external toggle must persist"
    );
    assert_eq!(
        native_state_int(&saved.opaque_state, format, "mode"),
        Some(0),
        "{instance} Wideband mode must persist"
    );
    let mut reloaded = ExternalPlugin::from_placeholder_state(&saved, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} reload: {error}"));
    assert_eq!(reloaded.input_channels(), INPUTS);
    assert_eq!(reloaded.output_channels(), OUTPUTS);
    let mut fresh = keyed_instance(format, &instance, &seed, true);
    let next = keyed_probe_block(8 * FRAMES, 0.05, 0.5);
    assert_eq!(
        render_block(&mut reloaded, &next),
        render_block(&mut fresh, &next),
        "{instance} reload must render like a fresh keyed instance"
    );

    // Structural restart: a 2 ms lookahead rebuild reports 96 samples
    // of latency and renders.
    let mut lookahead = saved.clone();
    set_native_state_param(
        &mut lookahead.opaque_state,
        format,
        "lookahead_ms",
        serde_json::json!({"f32": 2.0}),
    );
    let mut restarted = ExternalPlugin::from_placeholder_state(&lookahead, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} lookahead restart: {error}"));
    assert_eq!(restarted.latency_samples(), 96);
    assert!(
        render_block(&mut restarted, &next)
            .iter()
            .any(|sample| sample.abs() > 1.0e-3),
        "{instance} restarted route silent"
    );

    // A key-bus flip contradicts the negotiated route, so the live
    // instance refuses it with populated history untouched.
    let mut flipped = saved.clone();
    set_native_state_param(
        &mut flipped.opaque_state,
        format,
        "sidechain_external",
        serde_json::json!({"bool": false}),
    );
    assert_eq!(
        native_state_bool(&flipped.opaque_state, format, "sidechain_external"),
        Some(false),
        "{instance} flip candidate must carry internal detection"
    );
    let flip_refusal = match live.load_opaque_state(&flipped.opaque_state) {
        Ok(()) => panic!("{instance} live restore must refuse the key-bus flip"),
        Err(error) => error,
    };
    assert!(
        flip_refusal.contains("recreate"),
        "{instance} flip refusal must name recreation, got: {flip_refusal}"
    );
    let continued_flip = keyed_probe_block(9 * FRAMES, 0.05, 0.5);
    assert_eq!(
        render_block(&mut live, &continued_flip),
        render_block(&mut twin, &continued_flip),
        "{instance} refused flip must leave populated history unchanged"
    );

    // Garbage is refused the same way.
    assert!(
        live.load_opaque_state(&[0u8; 64]).is_err(),
        "{instance} live restore must refuse garbage state"
    );
    let continued_garbage = keyed_probe_block(10 * FRAMES, 0.05, 0.5);
    assert_eq!(
        render_block(&mut live, &continued_garbage),
        render_block(&mut twin, &continued_garbage),
        "{instance} refused garbage must leave populated history unchanged"
    );

    // Internal populated pair: hot program engages internal detection
    // (proving engaged history), quiet program passes with the hot key
    // ignored (proving program-driven detection); arming the key bus
    // live is refused with history kept.
    let mut internal_live = keyed_instance(format, &instance, &seed, false);
    let mut internal_twin = keyed_instance(format, &instance, &seed, false);
    let mut engaged_program = Vec::new();
    let mut engaged_output = Vec::new();
    for block_index in 0..4 {
        let input = keyed_probe_block(block_index * FRAMES, 0.5, 0.5);
        let live_out = render_block(&mut internal_live, &input);
        let twin_out = render_block(&mut internal_twin, &input);
        assert_eq!(
            live_out, twin_out,
            "{instance} internal block {block_index} twin mismatch"
        );
        engaged_program.extend_from_slice(&input);
        engaged_output.extend_from_slice(&live_out);
    }
    let engaged_db = 20.0
        * (channel_rms(&engaged_output, OUTPUTS, 0) / channel_rms(&engaged_program, INPUTS, 0))
            .log10();
    assert!(
        engaged_db < -6.0,
        "{instance} internal hot program must reduce past -6 dB, got {engaged_db:.2} dB"
    );
    assert!(
        engaged_db > -20.0,
        "{instance} internal hot program must not mute, got {engaged_db:.2} dB"
    );
    // A fresh internal instance proves the key is ignored (the engaged
    // pair above carries a 200 ms release tail, so it cannot measure a
    // pass leg without settling first).
    let mut ignored = keyed_instance(format, &instance, &seed, false);
    let mut ignored_program = Vec::new();
    let mut ignored_output = Vec::new();
    for block_index in 0..4 {
        let input = keyed_probe_block(block_index * FRAMES, 0.05, 0.5);
        ignored_program.extend_from_slice(&input);
        ignored_output.extend_from_slice(&render_block(&mut ignored, &input));
    }
    let ignored_db = 20.0
        * (channel_rms(&ignored_output, OUTPUTS, 0) / channel_rms(&ignored_program, INPUTS, 0))
            .log10();
    assert!(
        ignored_db.abs() < 1.0,
        "{instance} internal route must ignore the hot key within 1 dB, got {ignored_db:.2} dB"
    );
    let internal_save = external_state_of(&internal_live, &instance);
    assert_eq!(
        native_state_bool(&internal_save.opaque_state, format, "sidechain_external"),
        Some(false),
        "{instance} internal route must save internal detection"
    );
    let mut armed = internal_save.clone();
    set_native_state_param(
        &mut armed.opaque_state,
        format,
        "sidechain_external",
        serde_json::json!({"bool": true}),
    );
    assert_eq!(
        native_state_bool(&armed.opaque_state, format, "sidechain_external"),
        Some(true),
        "{instance} arming candidate must carry external detection"
    );
    let arm_refusal = match internal_live.load_opaque_state(&armed.opaque_state) {
        Ok(()) => panic!("{instance} live restore must refuse arming the key bus"),
        Err(error) => error,
    };
    assert!(
        arm_refusal.contains("recreate"),
        "{instance} arming refusal must name recreation, got: {arm_refusal}"
    );
    let continued_arm = keyed_probe_block(4 * FRAMES, 0.5, 0.5);
    assert_eq!(
        render_block(&mut internal_live, &continued_arm),
        render_block(&mut internal_twin, &continued_arm),
        "{instance} refused arming must leave populated history unchanged"
    );

    // Accepted latency changes renegotiate: the lookahead restore
    // commits a fresh backend, reports the new latency, keeps the
    // negotiated widths, and renders exactly like a fresh rebuild
    // from the same bytes — while audibly differing from the stale
    // populated twin it replaced.
    assert_eq!(
        native_state_bool(&lookahead.opaque_state, format, "sidechain_external"),
        Some(true),
        "{instance} lookahead candidate must keep external detection"
    );
    live.load_opaque_state(&lookahead.opaque_state)
        .unwrap_or_else(|error| panic!("{instance} lookahead live restore: {error}"));
    assert_eq!(live.latency_samples(), 96);
    assert_eq!(live.input_channels(), INPUTS);
    assert_eq!(live.output_channels(), OUTPUTS);
    let mut fresh_lookahead = ExternalPlugin::from_placeholder_state(&lookahead, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} fresh lookahead twin: {error}"));
    let adopted = keyed_probe_block(11 * FRAMES, 0.05, 0.5);
    let adopted_live = render_block(&mut live, &adopted);
    assert_eq!(
        adopted_live,
        render_block(&mut fresh_lookahead, &adopted),
        "{instance} committed lookahead restore must render like a fresh rebuild"
    );
    let stale_twin = render_block(&mut twin, &adopted);
    let adoption = adopted_live
        .iter()
        .zip(stale_twin.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        adoption > 1.0e-4,
        "{instance} lookahead restore did not audibly adopt"
    );

    // Valid realtime recovery on top still applies and adopts exactly.
    let mut recovery = lookahead.clone();
    set_native_state_param(
        &mut recovery.opaque_state,
        format,
        "threshold",
        serde_json::json!({"f32": -6.0}),
    );
    assert_eq!(
        native_state_bool(&recovery.opaque_state, format, "sidechain_external"),
        Some(true),
        "{instance} recovery candidate must keep external detection"
    );
    live.load_opaque_state(&recovery.opaque_state)
        .unwrap_or_else(|error| panic!("{instance} realtime recovery: {error}"));
    assert_eq!(live.latency_samples(), 96);
    let mut fresh_recovery = ExternalPlugin::from_placeholder_state(&recovery, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} fresh recovery twin: {error}"));
    let recovered = keyed_probe_block(12 * FRAMES, 0.05, 0.5);
    let recovered_live = render_block(&mut live, &recovered);
    assert!(
        recovered_live.iter().any(|sample| sample.abs() > 1.0e-3),
        "{instance} recovered route silent"
    );
    assert_eq!(
        recovered_live,
        render_block(&mut fresh_recovery, &recovered),
        "{instance} recovery must render like a fresh rebuild"
    );

    // Bus-count changes recreate; only the identical setup no-ops.
    live.reconfigure_audio_setup(sidechain_setup())
        .unwrap_or_else(|error| panic!("{instance} identical reconfigure: {error}"));
    let narrower = NativePluginAudioSetup::Sidechain {
        main_channels: 2,
        key_channels: 1,
    };
    let refusal = match live.reconfigure_audio_setup(narrower) {
        Ok(()) => panic!("{instance} bus-count reconfigure must be refused"),
        Err(error) => error,
    };
    assert!(
        refusal.contains("recreate"),
        "{instance} refusal must name recreation, got: {refusal}"
    );

    // Preset restores honor the same detector continuity as opaque
    // restores. A dedicated populated external pair proves the
    // invalid candidates refuse with history kept and a matching
    // preset recovers.
    let mut preset_live = keyed_instance(format, &instance, &seed, true);
    let mut preset_twin = keyed_instance(format, &instance, &seed, true);
    for block_index in 0..8 {
        let input = keyed_probe_block(block_index * FRAMES, 0.05, 0.5);
        let live_out = render_block(&mut preset_live, &input);
        let twin_out = render_block(&mut preset_twin, &input);
        assert_eq!(
            live_out, twin_out,
            "{instance} preset block {block_index} twin mismatch"
        );
    }
    let preset_saved = external_state_of(&preset_live, &instance);

    // An empty Sidechain preset carries no detector proof, so the
    // populated external route refuses it instead of silently
    // resetting to internal detection.
    let mut empty_sidechain = preset_saved.clone();
    empty_sidechain.opaque_state.clear();
    assert_eq!(empty_sidechain.audio_setup, Some(sidechain_setup()));
    let empty_preset = preset_from_state(&preset_live, &instance, &empty_sidechain);
    let empty_refusal = match preset_live.deserialize(&empty_preset) {
        Ok(()) => panic!("{instance} preset restore must refuse the empty candidate"),
        Err(error) => error.to_string(),
    };
    assert!(
        empty_refusal.contains("recreate"),
        "{instance} empty-preset refusal must name recreation, got: {empty_refusal}"
    );
    let continued_empty = keyed_probe_block(8 * FRAMES, 0.05, 0.5);
    assert_eq!(
        render_block(&mut preset_live, &continued_empty),
        render_block(&mut preset_twin, &continued_empty),
        "{instance} refused empty preset must leave populated history unchanged"
    );

    // A preset that drops the Sidechain setup is refused outright
    // instead of collapsing the key bus — with external bytes ...
    let mut dropped_external = preset_saved.clone();
    dropped_external.audio_setup = None;
    let dropped_external_preset = preset_from_state(&preset_live, &instance, &dropped_external);
    let dropped_refusal = match preset_live.deserialize(&dropped_external_preset) {
        Ok(()) => panic!("{instance} preset restore must refuse the dropped setup"),
        Err(error) => error.to_string(),
    };
    assert!(
        dropped_refusal.contains("recreate"),
        "{instance} dropped-setup refusal must name recreation, got: {dropped_refusal}"
    );
    let continued_dropped = keyed_probe_block(9 * FRAMES, 0.05, 0.5);
    assert_eq!(
        render_block(&mut preset_live, &continued_dropped),
        render_block(&mut preset_twin, &continued_dropped),
        "{instance} refused dropped setup must leave populated history unchanged"
    );

    // ... and internal bytes alike: the setup mismatch refuses
    // before any detector comparison.
    let mut dropped_internal = dropped_external.clone();
    set_native_state_param(
        &mut dropped_internal.opaque_state,
        format,
        "sidechain_external",
        serde_json::json!({"bool": false}),
    );
    assert_eq!(
        native_state_bool(&dropped_internal.opaque_state, format, "sidechain_external"),
        Some(false),
        "{instance} dropped-setup candidate must carry internal detection"
    );
    let dropped_internal_preset = preset_from_state(&preset_live, &instance, &dropped_internal);
    let dropped_internal_refusal = match preset_live.deserialize(&dropped_internal_preset) {
        Ok(()) => panic!("{instance} preset restore must refuse the internal dropped setup"),
        Err(error) => error.to_string(),
    };
    assert!(
        dropped_internal_refusal.contains("recreate"),
        "{instance} internal dropped-setup refusal must name recreation, got: {dropped_internal_refusal}"
    );
    let continued_internal_dropped = keyed_probe_block(10 * FRAMES, 0.05, 0.5);
    assert_eq!(
        render_block(&mut preset_live, &continued_internal_dropped),
        render_block(&mut preset_twin, &continued_internal_dropped),
        "{instance} refused internal dropped setup must leave populated history unchanged"
    );

    // A matching preset recovers: widths and the external toggle
    // persist, the committed fresh backend renders like a fresh
    // rebuild from the same bytes, and audibly differs from the
    // stale populated twin it replaced.
    let matching_preset = preset_from_state(&preset_live, &instance, &preset_saved);
    preset_live
        .deserialize(&matching_preset)
        .unwrap_or_else(|error| panic!("{instance} matching preset recovery: {error}"));
    assert_eq!(preset_live.input_channels(), INPUTS);
    assert_eq!(preset_live.output_channels(), OUTPUTS);
    let recovered_saved = external_state_of(&preset_live, &instance);
    assert_eq!(recovered_saved.audio_setup, Some(sidechain_setup()));
    assert_eq!(
        native_state_bool(&recovered_saved.opaque_state, format, "sidechain_external"),
        Some(true),
        "{instance} recovered route must save external detection"
    );
    let mut fresh_preset = ExternalPlugin::from_placeholder_state(&preset_saved, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} fresh preset twin: {error}"));
    let adopted_preset = keyed_probe_block(11 * FRAMES, 0.05, 0.5);
    let adopted_preset_live = render_block(&mut preset_live, &adopted_preset);
    assert_eq!(
        adopted_preset_live,
        render_block(&mut fresh_preset, &adopted_preset),
        "{instance} committed preset recovery must render like a fresh rebuild"
    );
    let stale_preset_twin = render_block(&mut preset_twin, &adopted_preset);
    let preset_adoption = adopted_preset_live
        .iter()
        .zip(stale_preset_twin.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        preset_adoption > 1.0e-4,
        "{instance} preset recovery did not audibly adopt"
    );

    // The empty Sidechain preset is valid on a populated internal
    // route, where the fresh default matches the live source.
    let internal_preset_saved = external_state_of(&internal_live, &instance);
    let mut internal_empty = internal_preset_saved.clone();
    internal_empty.opaque_state.clear();
    assert_eq!(internal_empty.audio_setup, Some(sidechain_setup()));
    let internal_empty_preset = preset_from_state(&internal_live, &instance, &internal_empty);
    internal_live
        .deserialize(&internal_empty_preset)
        .unwrap_or_else(|error| panic!("{instance} internal empty preset: {error}"));
    assert_eq!(internal_live.input_channels(), INPUTS);
    assert_eq!(internal_live.output_channels(), OUTPUTS);
    let internal_adopted = keyed_probe_block(5 * FRAMES, 0.5, 0.5);
    let internal_adopted_live = render_block(&mut internal_live, &internal_adopted);
    assert!(
        internal_adopted_live
            .iter()
            .any(|sample| sample.abs() > 1.0e-3),
        "{instance} internal preset route silent"
    );
    let stale_internal_twin = render_block(&mut internal_twin, &internal_adopted);
    let internal_adoption = internal_adopted_live
        .iter()
        .zip(stale_internal_twin.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        internal_adoption > 1.0e-4,
        "{instance} internal empty preset did not audibly adopt"
    );
}
