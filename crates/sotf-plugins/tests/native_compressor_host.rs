//! Loaded CLAP/VST3 tests for the native Compressor family.
//!
//! UNEXECUTED: requires fresh compressor binaries plus
//! `SOTF_TEST_COMPRESSOR_CLAP_PLUGIN` / `SOTF_TEST_COMPRESSOR_VST3_PLUGIN`
//! (broadband) and `SOTF_TEST_MULTIBAND_COMPRESSOR_CLAP_PLUGIN` /
//! `SOTF_TEST_MULTIBAND_COMPRESSOR_VST3_PLUGIN` (multiband). Four test
//! functions (one per format per kind); every subcase is embedded, so the
//! runner expects exactly 4 executed with `--ignored` (4 ignored without).
//! Reuses the proven `native_ambisonics_host_*` backend APIs: default
//! loaded processing vs a direct factory reference, save/reload
//! continuation, detector-flavored loaded proof via seeded state, and
//! invalid-restore refusal with retained-history twins.

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
// VST3 hex of the `SotfCmprssor0001` class id in plugins-nih/src/lib.rs.
const COMPRESSOR_VST3_ID: &str = "536F7466436D707273736F7230303031";
// VST3 hex of the `SotfMBComprss001` class id in plugins-nih/src/lib.rs.
const MULTIBAND_VST3_ID: &str = "536F74664D42436F6D70727373303031";

#[test]
#[ignore = "requires SOTF_TEST_COMPRESSOR_CLAP_PLUGIN to point to the exported Compressor CLAP library"]
fn exported_clap_compressor_audio_matches_direct_reference_and_saved_state() {
    verify_loaded_compressor_route(
        PluginFormat::Clap,
        "SOTF_TEST_COMPRESSOR_CLAP_PLUGIN",
        "org.spinorama.sotf.compressor",
        "SOTF: Compressor",
        "compressor",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_COMPRESSOR_VST3_PLUGIN to point to the exported Compressor VST3 library"]
fn exported_vst3_compressor_audio_matches_direct_reference_and_saved_state() {
    verify_loaded_compressor_route(
        PluginFormat::Vst3,
        "SOTF_TEST_COMPRESSOR_VST3_PLUGIN",
        COMPRESSOR_VST3_ID,
        "SOTF: Compressor",
        "compressor",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_MULTIBAND_COMPRESSOR_CLAP_PLUGIN to point to the exported MultibandCompressor CLAP library"]
fn exported_clap_multiband_compressor_audio_matches_direct_reference_and_saved_state() {
    verify_loaded_compressor_route(
        PluginFormat::Clap,
        "SOTF_TEST_MULTIBAND_COMPRESSOR_CLAP_PLUGIN",
        "org.spinorama.sotf.multiband-compressor",
        "SOTF: Multiband Compressor",
        "multiband_compressor",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_MULTIBAND_COMPRESSOR_VST3_PLUGIN to point to the exported MultibandCompressor VST3 library"]
fn exported_vst3_multiband_compressor_audio_matches_direct_reference_and_saved_state() {
    verify_loaded_compressor_route(
        PluginFormat::Vst3,
        "SOTF_TEST_MULTIBAND_COMPRESSOR_VST3_PLUGIN",
        MULTIBAND_VST3_ID,
        "SOTF: Multiband Compressor",
        "multiband_compressor",
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
        let sample = 0.5012 * (2.0 * std::f32::consts::PI * 50.0 * t).sin();
        block[frame * CHANNELS] = sample;
        block[frame * CHANNELS + 1] = sample * 0.5;
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

fn native_state_bool(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<bool> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("bool")?
        .as_bool()
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

fn direct_reference(factory_type: &str) -> Box<dyn Plugin> {
    let mut reference =
        sotf_plugins::create_plugin(factory_type, &serde_json::json!({}), CHANNELS, SAMPLE_RATE)
            .expect("direct factory reference builds");
    // Factory construction alone leaves DSP buffers empty; the loaded
    // binaries initialize through their wrapper lifecycle, so the direct
    // reference must be initialized explicitly before any process call.
    reference
        .initialize(SAMPLE_RATE)
        .expect("direct factory reference initializes");
    reference
}

fn verify_loaded_compressor_route(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    name: &str,
    factory_type: &str,
) {
    let instance = format!("{name} {format:?}");
    let descriptor = test_descriptor(format, library_env, plugin_id, name);

    // Default loaded instance processes finite nonzero compressed audio.
    let mut plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} load: {error}"));
    assert_eq!(plugin.input_channels(), CHANNELS);
    assert_eq!(plugin.output_channels(), CHANNELS);
    let mut reference = direct_reference(factory_type);
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
        let peak_in = input.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let peak_out = actual.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            peak_out < peak_in * 0.9,
            "{instance} block {block_index} not compressing: {peak_out:.4} vs {peak_in:.4}"
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

    // Saved state carries the detector controls at legacy defaults.
    let preset = plugin.serialize().expect("serialize loaded defaults");
    let saved_state = preset
        .external_plugin_state()
        .expect("read external state envelope")
        .expect("serialized external state");
    assert_eq!(saved_state.descriptor, descriptor);
    assert_eq!(
        native_state_float(&saved_state.opaque_state, format, "sidechain_hpf_hz"),
        Some(80.0),
        "{instance} saved HPF default"
    );
    assert_eq!(
        native_state_bool(&saved_state.opaque_state, format, "sidechain_hpf_enabled"),
        Some(false),
        "{instance} saved HPF enable default"
    );
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "sidechain_hpf_order"),
        Some(0),
        "{instance} saved HPF order default"
    );
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "detection_mode"),
        Some(0),
        "{instance} saved detection default"
    );

    // Restore continues like a fresh direct reference (both fresh DSP).
    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} restore: {error}"));
    let mut fresh_reference = direct_reference(factory_type);
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

    // Detector-flavored loaded proof: a seeded opt-in state compresses LF
    // audibly less than the default-loaded instance on the same program.
    let mut detector_state = saved_state.clone();
    set_native_state_param(
        &mut detector_state.opaque_state,
        format,
        "sidechain_hpf_hz",
        serde_json::json!({"f32": 120.0}),
    );
    set_native_state_param(
        &mut detector_state.opaque_state,
        format,
        "sidechain_hpf_order",
        serde_json::json!({"i32": 1}),
    );
    set_native_state_param(
        &mut detector_state.opaque_state,
        format,
        "sidechain_hpf_enabled",
        serde_json::json!({"bool": true}),
    );
    set_native_state_param(
        &mut detector_state.opaque_state,
        format,
        "detection_mode",
        serde_json::json!({"i32": 1}),
    );
    let mut detector = ExternalPlugin::from_placeholder_state(&detector_state, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} detector restore: {error}"));
    let tail_peak = |plugin: &mut ExternalPlugin| {
        let mut tail = 0.0f32;
        for block_index in 0..96 {
            let output = render_block(plugin, &probe_block(block_index * FRAMES));
            if block_index >= 80 {
                tail = tail.max(output.iter().map(|s| s.abs()).fold(0.0f32, f32::max));
            }
        }
        tail
    };
    let mut default_loaded =
        ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
            .expect("reload default twin");
    let default_tail = tail_peak(&mut default_loaded);
    let detector_tail = tail_peak(&mut detector);
    println!(
        "{instance} loaded LF tails: default={default_tail:.4} detector={detector_tail:.4}"
    );
    let separation_db =
        20.0 * (detector_tail / default_tail.max(1.0e-9)).max(1.0e-9).log10();
    assert!(
        separation_db > 3.0,
        "{instance} loaded detector separation too small: {separation_db:.2} dB"
    );

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
    // R1-1: multiband NIH specs carry no program/external keys, so the
    // invalid candidate branches by kind. Broadband keeps the
    // program-dependent refusal; multiband refuses an out-of-range
    // detector choice index (7 vs 0..2), which the DSP choice
    // deserializer rejects during the restore-time construction both
    // paths share. Garbage refusal is asserted for both kinds.
    if factory_type == "multiband_compressor" {
        let mut invalid_detector = saved_state.clone();
        set_native_state_param(
            &mut invalid_detector.opaque_state,
            format,
            "sidechain_hpf_order",
            serde_json::json!({"i32": 7}),
        );
        assert!(
            ExternalPlugin::from_placeholder_state(&invalid_detector, SAMPLE_RATE).is_err(),
            "{instance} placeholder must refuse out-of-range detector order"
        );
        assert!(
            restored
                .load_opaque_state(&invalid_detector.opaque_state)
                .is_err(),
            "{instance} live restore must refuse out-of-range detector order"
        );
    } else {
        let mut program_state = saved_state.clone();
        set_native_state_param(
            &mut program_state.opaque_state,
            format,
            "program_dependent_release",
            serde_json::json!({"bool": true}),
        );
        assert!(
            ExternalPlugin::from_placeholder_state(&program_state, SAMPLE_RATE).is_err(),
            "{instance} placeholder must refuse program-dependent state"
        );
        assert!(
            restored
                .load_opaque_state(&program_state.opaque_state)
                .is_err(),
            "{instance} live restore must refuse program-dependent state"
        );
    }
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
