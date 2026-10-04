//! Loaded CLAP/VST3 tests for the native AnalogLimiter route.
//!
//! UNEXECUTED: requires fresh analog-limiter binaries plus
//! `SOTF_TEST_ANALOG_LIMITER_CLAP_PLUGIN` /
//! `SOTF_TEST_ANALOG_LIMITER_VST3_PLUGIN`. Two test functions (one per
//! format); every subcase is embedded, so the runner expects exactly 2
//! executed with `--ignored` (2 ignored without). Reuses the proven
//! `native_compressor_host` backend APIs: default loaded processing vs a
//! direct factory reference, save/reload continuation, a threshold-seeded
//! ceiling proof, and invalid-restore refusal with retained-history twins.
//! Wrapper-to-DSP transparency itself is covered by the proposed
//! `analog_limiter_routing` baseline; these tests prove the loaded host
//! routes the analog limiter end to end, including the emitted ceiling.

// Rust guideline compliant 2026-02-21

#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

use sotf_host::external_plugin::{
    ExternalPlugin, PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::serialization::SerializablePlugin;
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 512;
const CHANNELS: usize = 2;
// VST3 hex of the proposed `SotfAnalogLim001` class id (see
// integration-r1-shared-patches.md P1; 16 bytes, no collision with any
// existing export).
const ANALOG_LIMITER_VST3_ID: &str = "536F7466416E616C6F674C696D303031";

#[test]
#[ignore = "requires SOTF_TEST_ANALOG_LIMITER_CLAP_PLUGIN to point to the exported AnalogLimiter CLAP library"]
fn exported_clap_analog_limiter_route_matches_contract_and_ceiling() {
    verify_loaded_analog_limiter_route(
        PluginFormat::Clap,
        "SOTF_TEST_ANALOG_LIMITER_CLAP_PLUGIN",
        "org.spinorama.sotf.analog-limiter",
        "SOTF: Analog Limiter",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_ANALOG_LIMITER_VST3_PLUGIN to point to the exported AnalogLimiter VST3 library"]
fn exported_vst3_analog_limiter_route_matches_contract_and_ceiling() {
    verify_loaded_analog_limiter_route(
        PluginFormat::Vst3,
        "SOTF_TEST_ANALOG_LIMITER_VST3_PLUGIN",
        ANALOG_LIMITER_VST3_ID,
        "SOTF: Analog Limiter",
    );
}

fn test_descriptor(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    name: &str,
) -> PluginDescriptor {
    let library_path = PathBuf::from(
        std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
    );
    PluginDescriptor {
        id: plugin_id.into(),
        name: name.into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: library_path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

fn probe_block(start_frame: usize) -> Vec<f32> {
    let mut block = vec![0.0; FRAMES * CHANNELS];
    for frame in 0..FRAMES {
        let t = (start_frame + frame) as f32 / SAMPLE_RATE as f32;
        let sample = 0.9 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
        block[frame * CHANNELS] = sample;
        block[frame * CHANNELS + 1] = sample;
    }
    block
}

fn render_block(plugin: &mut dyn Plugin, input: &[f32]) -> Vec<f32> {
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
    let mut output = vec![f32::NAN; FRAMES * CHANNELS];
    assert_eq!(
        plugin.process(input, &mut output, &context).unwrap(),
        FRAMES
    );
    output
}

fn block_peak(block: &[f32]) -> f32 {
    block.iter().map(|s| s.abs()).fold(0.0f32, f32::max)
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

fn native_state_float(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<f64> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("f32")?
        .as_f64()
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

fn direct_reference() -> Box<dyn Plugin> {
    let mut reference = sotf_plugins::create_plugin(
        "analog_limiter",
        &serde_json::json!({}),
        CHANNELS,
        SAMPLE_RATE,
    )
    .expect("direct factory reference builds");
    // Factory construction alone leaves DSP buffers empty; the loaded
    // binaries initialize through their wrapper lifecycle, so the direct
    // reference must be initialized explicitly before any process call.
    reference
        .initialize(f64::from(SAMPLE_RATE))
        .expect("direct factory reference initializes");
    reference
}

fn verify_loaded_analog_limiter_route(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    name: &str,
) {
    let instance = format!("{name} {format:?}");
    let descriptor = test_descriptor(format, library_env, plugin_id, name);

    // Default loaded instance is transparent on hot-but-below-ceiling
    // program and matches the direct factory reference sample-closely.
    let mut plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} load: {error}"));
    assert_eq!(plugin.input_channels(), CHANNELS);
    assert_eq!(plugin.output_channels(), CHANNELS);
    let mut reference = direct_reference();
    for block_index in 0..4 {
        let input = probe_block(block_index * FRAMES);
        let actual = render_block(&mut plugin, &input);
        let expected = render_block(reference.as_mut(), &input);
        assert!(
            actual.iter().all(|sample| sample.is_finite()),
            "{instance} block {block_index} non-finite"
        );
        assert!(
            actual.iter().any(|sample| sample.abs() > 1.0e-3),
            "{instance} block {block_index} silent"
        );
        let peak_in = block_peak(&input);
        let peak_out = block_peak(&actual);
        // Default threshold is -0.1 dB (ceiling ~0.989): a 0.9-peak signal
        // passes untouched, so any attenuation here is a wrapper defect.
        assert!(
            (peak_out - peak_in).abs() < 1.0e-3,
            "{instance} block {block_index} attenuated: {peak_out:.4} vs {peak_in:.4}"
        );
        let worst = actual
            .iter()
            .zip(expected.iter())
            .map(|(a, e)| (a - e).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 1.0e-4,
            "{instance} block {block_index} differs from direct reference by {worst:.2e}"
        );
    }

    // Saved state carries the owned defaults.
    let preset = plugin.serialize().expect("serialize loaded defaults");
    let saved_state = preset
        .external_plugin_state()
        .expect("read external state envelope")
        .expect("serialized external state");
    assert_eq!(saved_state.descriptor, descriptor);
    let saved_threshold = native_state_float(&saved_state.opaque_state, format, "threshold")
        .unwrap_or_else(|| panic!("{instance} saved threshold present"));
    assert!(
        (saved_threshold + 0.1).abs() < 1.0e-7,
        "{instance} saved threshold default: {saved_threshold}"
    );
    assert_eq!(
        native_state_float(&saved_state.opaque_state, format, "mix"),
        Some(1.0),
        "{instance} saved mix default"
    );
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "analog_model"),
        Some(0),
        "{instance} saved model default"
    );

    // Restore continues like a fresh direct reference (both fresh DSP).
    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} restore: {error}"));
    let mut fresh_reference = direct_reference();
    let input = probe_block(4 * FRAMES);
    let restored_output = render_block(&mut restored, &input);
    let expected_output = render_block(fresh_reference.as_mut(), &input);
    let worst = restored_output
        .iter()
        .zip(expected_output.iter())
        .map(|(a, e)| (a - e).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1.0e-4,
        "{instance} restored continuation differs by {worst:.2e}"
    );

    // Model selection + ceiling proof through seeded states: every one of
    // the six models restores, renders finite audio, and caps the hot
    // program at the emitted ceiling with threshold -12 dB and color.
    let ceiling = 10f32.powf(-12.0 / 20.0);
    for (index, label) in sotf_plugins::plugin_analog_common::MODEL_NAMES
        .iter()
        .enumerate()
    {
        let mut seeded = saved_state.clone();
        set_native_state_param(
            &mut seeded.opaque_state,
            format,
            "threshold",
            serde_json::json!({"f32": -12.0}),
        );
        set_native_state_param(
            &mut seeded.opaque_state,
            format,
            "analog_model",
            serde_json::json!({"i32": index as i32}),
        );
        set_native_state_param(
            &mut seeded.opaque_state,
            format,
            "analog_color",
            serde_json::json!({"f32": 0.5}),
        );
        let mut limited = ExternalPlugin::from_placeholder_state(&seeded, SAMPLE_RATE)
            .unwrap_or_else(|error| panic!("{instance} seeded restore {label}: {error}"));
        let mut worst_peak = 0.0f32;
        for block_index in 0..8 {
            let output = render_block(&mut limited, &probe_block(block_index * FRAMES));
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "{instance} model {label} block {block_index} non-finite"
            );
            worst_peak = worst_peak.max(block_peak(&output));
        }
        println!("{instance} loaded {label}: peak={worst_peak:.6} ceiling={ceiling:.6}");
        assert!(
            worst_peak <= ceiling,
            "{instance} model {label} peak {worst_peak:.6} exceeds ceiling {ceiling:.6}"
        );
        assert!(
            worst_peak > ceiling * 0.1,
            "{instance} model {label} peak {worst_peak:.6} is suspiciously quiet"
        );
    }

    // Invalid restores refuse with the running instance untouched.
    let mut twin = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("load retained-history twin");
    // Synchronize DSP history: `restored` already rendered block 4 in the
    // restore-continuation section, so the fresh twin must render the
    // identical block before any bit-exact comparison is meaningful.
    render_block(&mut twin, &probe_block(4 * FRAMES));
    let history = probe_block(5 * FRAMES);
    assert_eq!(
        render_block(&mut restored, &history),
        render_block(&mut twin, &history),
        "{instance} twin history must start bit-exact"
    );
    // Out-of-range model index (99 vs 0..5): the DSP choice deserializer
    // rejects it during the restore-time construction both paths share.
    let mut invalid_model = saved_state.clone();
    set_native_state_param(
        &mut invalid_model.opaque_state,
        format,
        "analog_model",
        serde_json::json!({"i32": 99}),
    );
    assert!(
        ExternalPlugin::from_placeholder_state(&invalid_model, SAMPLE_RATE).is_err(),
        "{instance} placeholder must refuse out-of-range model index"
    );
    assert!(
        restored
            .load_opaque_state(&invalid_model.opaque_state)
            .is_err(),
        "{instance} live restore must refuse out-of-range model index"
    );
    assert!(
        restored.load_opaque_state(&[0u8; 64]).is_err(),
        "{instance} live restore must refuse garbage state"
    );
    let next = probe_block(6 * FRAMES);
    assert_eq!(
        render_block(&mut restored, &next),
        render_block(&mut twin, &next),
        "{instance} rejected restores must leave populated history unchanged"
    );
}
