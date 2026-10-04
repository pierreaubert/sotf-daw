//! Loaded CLAP/VST3 tests for the native Declick route.
//!
//! UNEXECUTED: requires fresh declick binaries plus
//! `SOTF_TEST_DECLICK_CLAP_PLUGIN` / `SOTF_TEST_DECLICK_VST3_PLUGIN`.
//! Two test functions (one per format); every subcase is embedded, so the
//! runner expects exactly 2 executed with `--ignored` (2 ignored without).
//! Reuses the proven `native_compressor_host` backend APIs: default loaded
//! processing vs a direct factory reference, saved-state control carriage,
//! save/reload continuation, seeded periodic/multiband/widened proof
//! (latency 11, regroup identity, repair engagement), invalid-restore
//! refusal with retained-history twins, and a zero-feed tail for exact
//! last samples (loaded hosts expose no drain call).

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
/// Rendered input blocks per leg (4 x 512 = 2048 frames; plans must fit).
const BLOCKS: usize = 4;
/// Widened repair span used by the seeded legs.
const WIDTH: usize = 3;
/// Owned-path latency derived from the known width (8 + repair_width).
const LATENCY: usize = 8 + WIDTH;
/// Legacy neutral-path latency (zero width, random mode, fullband).
const LEGACY_LATENCY: usize = 8;
/// Synthetic click amplitude added onto the fixture tones.
const CLICK_AMP: f32 = 3.0;
// VST3 hex of the `SotfDeclick00001` class id in plugins-nih/src/lib.rs.
const DECLICK_VST3_ID: &str = "536F74664465636C69636B3030303031";

#[test]
#[ignore = "requires SOTF_TEST_DECLICK_CLAP_PLUGIN to point to the exported Declick CLAP library"]
fn exported_clap_declick_audio_matches_direct_reference_and_saved_state() {
    verify_loaded_declick_route(
        PluginFormat::Clap,
        "SOTF_TEST_DECLICK_CLAP_PLUGIN",
        "org.spinorama.sotf.declick",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_DECLICK_VST3_PLUGIN to point to the exported Declick VST3 library"]
fn exported_vst3_declick_audio_matches_direct_reference_and_saved_state() {
    verify_loaded_declick_route(
        PluginFormat::Vst3,
        "SOTF_TEST_DECLICK_VST3_PLUGIN",
        DECLICK_VST3_ID,
    );
}

fn test_descriptor(format: PluginFormat, library_env: &str, plugin_id: &str) -> PluginDescriptor {
    let library_path = PathBuf::from(
        std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
    );
    PluginDescriptor {
        id: plugin_id.into(),
        name: "SOTF: Declick".into(),
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

/// Click-corrupted stereo probe block starting at global `start_frame`.
///
/// Tones (440/660 Hz, 0.25 peak) plus sparse deterministic clicks, all at
/// settled global frames: ch0 600 (+), 1300-1302 (-); ch1 900 (-), 1700 (+).
fn probe_block(start_frame: usize) -> Vec<f32> {
    let mut block = vec![0.0; FRAMES * CHANNELS];
    for frame in 0..FRAMES {
        let global = start_frame + frame;
        let time = global as f32 / SAMPLE_RATE as f32;
        let mut left = 0.25 * (std::f32::consts::TAU * 440.0 * time).sin();
        let mut right = 0.25 * (std::f32::consts::TAU * 660.0 * time).sin();
        if global == 600 {
            left += CLICK_AMP;
        }
        if (1300..1303).contains(&global) {
            left -= CLICK_AMP;
        }
        if global == 900 {
            right -= CLICK_AMP;
        }
        if global == 1700 {
            right += CLICK_AMP;
        }
        block[frame * CHANNELS] = left;
        block[frame * CHANNELS + 1] = right;
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

fn direct_reference() -> Box<dyn Plugin> {
    let mut reference =
        sotf_plugins::create_plugin("declick", &serde_json::json!({}), CHANNELS, SAMPLE_RATE)
            .expect("direct factory reference builds");
    // Factory construction alone leaves DSP buffers empty; the loaded
    // binaries initialize through their wrapper lifecycle, so the direct
    // reference must be initialized explicitly before any process call.
    reference
        .initialize(f64::from(SAMPLE_RATE))
        .expect("direct factory reference initializes");
    reference
}

/// Independent oracle: `signal` delayed by `latency` frames over the full
/// rendered span (input frames plus latency tail), zero-padded past the end.
fn manual_delayed(signal: &[f32], latency: usize) -> Vec<f32> {
    let frames = signal.len() / CHANNELS;
    let mut delayed = vec![0.0; (frames + latency) * CHANNELS];
    for frame in 0..frames {
        for ch in 0..CHANNELS {
            delayed[(frame + latency) * CHANNELS + ch] = signal[frame * CHANNELS + ch];
        }
    }
    delayed
}

fn verify_loaded_declick_route(format: PluginFormat, library_env: &str, plugin_id: &str) {
    let instance = format!("Declick {format:?}");
    let descriptor = test_descriptor(format, library_env, plugin_id);

    // Default loaded instance processes finite nonzero audio like the
    // direct factory reference, with legacy latency and exact leading zeros.
    let mut plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} load: {error}"));
    assert_eq!(plugin.input_channels(), CHANNELS);
    assert_eq!(plugin.output_channels(), CHANNELS);
    assert_eq!(
        plugin.latency_samples(),
        LEGACY_LATENCY,
        "{instance} latency"
    );
    let mut reference = direct_reference();
    for block_index in 0..BLOCKS {
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
        if block_index == 0 {
            for (index, sample) in actual.iter().take(LEGACY_LATENCY * CHANNELS).enumerate() {
                assert_eq!(*sample, 0.0, "{instance} leading silence {index}");
            }
        }
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

    // Saved state carries all nine controls at legacy defaults.
    let preset = plugin.serialize().expect("serialize loaded defaults");
    let saved_state = preset
        .external_plugin_state()
        .expect("read external state envelope")
        .expect("serialized external state");
    assert_eq!(saved_state.descriptor, descriptor);
    for (id, expected) in [
        ("enabled", Some(true)),
        ("link_channels", Some(true)),
        ("audition_residual", Some(false)),
    ] {
        assert_eq!(
            native_state_bool(&saved_state.opaque_state, format, id),
            expected,
            "{instance} saved {id}"
        );
    }
    for (id, expected) in [
        ("sensitivity", Some(10.0)),
        ("crossover_hz", Some(4000.0)),
        ("frequency_skew", Some(0.0)),
    ] {
        assert_eq!(
            native_state_float(&saved_state.opaque_state, format, id),
            expected,
            "{instance} saved {id}"
        );
    }
    for (id, expected) in [
        ("mode", Some(0)),
        ("bands", Some(0)),
        ("repair_width", Some(0)),
    ] {
        assert_eq!(
            native_state_int(&saved_state.opaque_state, format, id),
            expected,
            "{instance} saved {id}"
        );
    }

    // Restore continues like a fresh direct reference (both fresh DSP).
    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} restore: {error}"));
    let mut fresh_reference = direct_reference();
    let input = probe_block(BLOCKS * FRAMES);
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

    // Seeded periodic/multiband/widened state: the loaded instances report
    // the new latency, repair audibly, and regroup cleaned + residual to
    // the independently delayed input.
    let mut seeded = saved_state.clone();
    for (id, value) in [
        ("sensitivity", serde_json::json!({"f32": 2.0})),
        ("mode", serde_json::json!({"i32": 1})),
        ("bands", serde_json::json!({"i32": 2})),
        ("crossover_hz", serde_json::json!({"f32": 8000.0})),
        ("frequency_skew", serde_json::json!({"f32": 0.5})),
        ("repair_width", serde_json::json!({"i32": 3})),
    ] {
        set_native_state_param(&mut seeded.opaque_state, format, id, value);
    }
    let mut cleaned = ExternalPlugin::from_placeholder_state(&seeded, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} seeded restore: {error}"));
    assert_eq!(
        cleaned.latency_samples(),
        LATENCY,
        "{instance} seeded latency"
    );
    let mut residual_seed = seeded.clone();
    set_native_state_param(
        &mut residual_seed.opaque_state,
        format,
        "audition_residual",
        serde_json::json!({"bool": true}),
    );
    let mut residual = ExternalPlugin::from_placeholder_state(&residual_seed, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("{instance} residual restore: {error}"));
    let mut full_input = Vec::new();
    let mut repaired = Vec::new();
    let mut residual_out = Vec::new();
    for block_index in 0..BLOCKS {
        let input = probe_block(block_index * FRAMES);
        repaired.extend_from_slice(&render_block(&mut cleaned, &input));
        residual_out.extend_from_slice(&render_block(&mut residual, &input));
        full_input.extend_from_slice(&input);
    }
    // Zero-feed tail: loaded hosts expose no drain call, so the exact
    // tail arrives as latency frames of zeros through the process path.
    // The returned produced count (asserted == LATENCY below), not the
    // buffer capacity, defines the valid tail.
    let tail_context = ProcessContext::new(SAMPLE_RATE, LATENCY);
    let tail_zeros = vec![0.0; LATENCY * CHANNELS];
    let mut cleaned_tail = vec![f32::NAN; LATENCY * CHANNELS];
    let mut residual_tail = vec![f32::NAN; LATENCY * CHANNELS];
    assert_eq!(
        cleaned
            .process(&tail_zeros, &mut cleaned_tail, &tail_context)
            .unwrap(),
        LATENCY
    );
    assert_eq!(
        residual
            .process(&tail_zeros, &mut residual_tail, &tail_context)
            .unwrap(),
        LATENCY
    );
    repaired.extend_from_slice(&cleaned_tail);
    residual_out.extend_from_slice(&residual_tail);
    assert_eq!(repaired.len(), full_input.len() + LATENCY * CHANNELS);
    assert_eq!(residual_out.len(), repaired.len());
    let dry = manual_delayed(&full_input, LATENCY);
    let mut worst = 0.0f32;
    for i in 0..dry.len() {
        worst = worst.max((repaired[i] + residual_out[i] - dry[i]).abs());
    }
    assert!(worst < 1.0e-5, "{instance} regroup drift {worst:.2e}");
    // Repair engagement at known click frames (output frame = input + 11):
    // the seeded instance removes nearly the full click amplitude.
    for (input_frame, ch) in [(600, 0), (900, 1)] {
        let out = (input_frame + LATENCY) * CHANNELS + ch;
        let corrupted = full_input[input_frame * CHANNELS + ch];
        let engagement = (repaired[out] - corrupted).abs();
        assert!(
            engagement > 1.0,
            "{instance} click {input_frame} ch{ch} not repaired (diff {engagement:.3})"
        );
    }
    // Exact last samples: the tail ends on the delayed input end within
    // the frozen damage bound (no new tight bound invented).
    let last = repaired.len() - CHANNELS;
    for ch in 0..CHANNELS {
        let error = (repaired[last + ch] - dry[last + ch]).abs();
        assert!(error < 0.05, "{instance} last sample ch{ch} error={error}");
    }

    // Invalid restores refuse with the running instance untouched.
    let mut twin = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("load retained-history twin");
    // Synchronize DSP history: `restored` already rendered block 4 in the
    // restore-continuation section, so the fresh twin must render the
    // identical block before any bit-exact comparison is meaningful.
    render_block(&mut twin, &probe_block(BLOCKS * FRAMES));
    let history = probe_block((BLOCKS + 1) * FRAMES);
    assert_eq!(
        render_block(&mut restored, &history),
        render_block(&mut twin, &history),
        "{instance} twin history must start bit-exact"
    );
    // Out-of-range structural choice (mode 7 vs 0..1): the DSP choice
    // deserializer rejects it during the restore-time construction both
    // paths share. Garbage refusal is asserted as well.
    let mut invalid_mode = saved_state.clone();
    set_native_state_param(
        &mut invalid_mode.opaque_state,
        format,
        "mode",
        serde_json::json!({"i32": 7}),
    );
    assert!(
        ExternalPlugin::from_placeholder_state(&invalid_mode, SAMPLE_RATE).is_err(),
        "{instance} placeholder must refuse out-of-range mode"
    );
    assert!(
        restored
            .load_opaque_state(&invalid_mode.opaque_state)
            .is_err(),
        "{instance} live restore must refuse out-of-range mode"
    );
    assert!(
        restored.load_opaque_state(&[0u8; 64]).is_err(),
        "{instance} live restore must refuse garbage state"
    );
    let next = probe_block((BLOCKS + 2) * FRAMES);
    assert_eq!(
        render_block(&mut restored, &next),
        render_block(&mut twin, &next),
        "{instance} rejected restores must leave populated history unchanged"
    );
}
