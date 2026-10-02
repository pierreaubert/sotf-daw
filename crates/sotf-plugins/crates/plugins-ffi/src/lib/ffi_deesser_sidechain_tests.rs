//! De-esser external-key proofs through the public C ABI.
//!
//! Every test drives `plugin_create`, `plugin_set_parameter`,
//! `plugin_save_state`, `plugin_load_state`, `plugin_export_preset_json`,
//! `plugin_import_preset_json` and `plugin_process` on real handles.
//! Stimuli and bounds are fixed a priori: an 8 kHz program at 0.05 peak
//! with an independent 8 kHz key at 0.5 peak (hot) or 0.0 (silent),
//! Wideband mode, threshold -20 dB, ratio 8. A hot key must reduce the
//! program into (-20, -6) dB; silent and swapped keys must pass within
//! 1 dB. History-refusal tests never reset before comparing audio.

// Rust guideline compliant 2026-02-21

use crate::{
    PluginHandle, plugin_create, plugin_destroy, plugin_export_preset_json, plugin_free_state,
    plugin_get_parameter, plugin_import_preset_json, plugin_load_state, plugin_process,
    plugin_reset, plugin_save_state, plugin_set_parameter,
};
use std::ffi::{CStr, CString};

const SAMPLE_RATE: u32 = 48_000;
const PROGRAM_PEAK: f32 = 0.05;
const KEY_PEAK: f32 = 0.5;
const TONE_HZ: f32 = 8_000.0;

/// Minimal C-handle guard. The de-esser DSP is synchronous, so unlike
/// `ffi_integration_tests::AbiHandle` no async-timeline bookkeeping is
/// needed here.
struct KeyHandle {
    pointer: *mut PluginHandle,
}

impl KeyHandle {
    fn create(config: &str, inputs: usize, outputs: usize) -> Self {
        let kind = CString::new("DeEsser").unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(
            kind.as_ptr(),
            config.as_ptr(),
            SAMPLE_RATE,
            inputs,
            outputs,
        );
        assert!(!handle.is_null(), "DeEsser construction failed: {err}", err = last_error());
        Self { pointer: handle }
    }

    fn inner(&self) -> &PluginHandle {
        // SAFETY: This guard owns a live handle, accessed only on this thread.
        unsafe { &*self.pointer }
    }

    fn set_normalized(&mut self, id: &str, value: f64) -> i32 {
        let id = CString::new(id).unwrap();
        plugin_set_parameter(self.pointer, id.as_ptr(), value)
    }

    fn get_normalized(&self, id: &str) -> f64 {
        let id = CString::new(id).unwrap();
        plugin_get_parameter(self.pointer, id.as_ptr())
    }

    fn save(&self) -> Vec<u8> {
        let mut len = 0;
        let state = plugin_save_state(self.pointer, &mut len);
        assert!(!state.is_null());
        // SAFETY: The FFI owns exactly len initialized bytes until freed below.
        let saved = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        plugin_free_state(state, len);
        saved
    }

    fn load(&mut self, state: &[u8]) -> i32 {
        plugin_load_state(self.pointer, state.as_ptr(), state.len())
    }

    fn export_preset(&self, name: &str) -> Vec<u8> {
        let name = CString::new(name).unwrap();
        let mut len = 0;
        let document = plugin_export_preset_json(self.pointer, name.as_ptr(), &mut len);
        assert!(!document.is_null(), "preset export failed: {err}", err = last_error());
        // SAFETY: The FFI owns exactly len initialized bytes until freed below.
        let saved = unsafe { std::slice::from_raw_parts(document, len) }.to_vec();
        plugin_free_state(document, len);
        saved
    }

    fn import_preset(&mut self, document: &[u8]) -> i32 {
        plugin_import_preset_json(self.pointer, document.as_ptr(), document.len())
    }

    fn reset(&mut self) {
        assert_eq!(plugin_reset(self.pointer), 0, "{}", last_error());
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.process_partitioned(input, self.inner().max_callback_frames)
    }

    fn process_partitioned(&mut self, input: &[f32], callback_frames: usize) -> Vec<f32> {
        let inputs = self.inner().input_channels;
        let outputs = self.inner().output_channels;
        let frames = input.len() / inputs;
        assert_eq!(input.len() % inputs, 0, "complete interleaved frames required");
        assert!((1..=self.inner().max_callback_frames).contains(&callback_frames));
        // NaN poisoning proves the callback overwrites every output sample.
        let mut output = vec![f32::NAN; frames * outputs];
        for start in (0..frames).step_by(callback_frames) {
            let count = callback_frames.min(frames - start);
            assert_eq!(
                plugin_process(
                    self.pointer,
                    input[start * inputs..].as_ptr(),
                    output[start * outputs..].as_mut_ptr(),
                    count,
                ),
                0,
                "{}",
                last_error()
            );
        }
        assert!(output.iter().all(|value| value.is_finite()));
        output
    }

    fn config_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.inner().config_json).expect("handle config must be JSON")
    }
}

impl Drop for KeyHandle {
    fn drop(&mut self) {
        plugin_destroy(self.pointer);
    }
}

fn last_error() -> String {
    let error = crate::plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned()
}

/// 4-channel interleaved input: 8 kHz program on [0, 1], independent
/// 8 kHz key at `key_peak` on [2, 3].
fn keyed_input(frames: usize, program_peak: f32, key_peak: f32) -> Vec<f32> {
    let mut input = vec![0.0f32; frames * 4];
    for frame in 0..frames {
        let t = frame as f32 / SAMPLE_RATE as f32;
        let program = program_peak * (std::f32::consts::TAU * TONE_HZ * t).sin();
        let key = key_peak * (std::f32::consts::TAU * TONE_HZ * t).sin();
        input[frame * 4] = program;
        input[frame * 4 + 1] = program;
        input[frame * 4 + 2] = key;
        input[frame * 4 + 3] = key;
    }
    input
}

fn channel_rms(interleaved: &[f32], width: usize, channel: usize, from_frame: usize) -> f32 {
    let frames = interleaved.len() / width;
    let sum: f32 = (from_frame..frames)
        .map(|frame| {
            let sample = interleaved[frame * width + channel];
            sample * sample
        })
        .sum();
    (sum / (frames - from_frame) as f32).sqrt()
}

/// External-key handle with a deterministic Wideband configuration
/// committed through the transactional state-load path.
fn wideband_key_handle(release_ms: f64) -> KeyHandle {
    let mut handle = KeyHandle::create(r#"{"sidechain_external": true}"#, 4, 2);
    assert_eq!(handle.inner().input_channels, 4);
    assert_eq!(handle.inner().output_channels, 2);
    let state = serde_json::json!({
        "mode": 0,
        "threshold": -20.0,
        "ratio": 8.0,
        "attack": 0.5,
        "release": release_ms,
        "mix": 1.0,
        "range_db": 60.0,
        "stereo_link": 0.0,
    });
    let state_bytes = serde_json::to_vec(&state).unwrap();
    assert_eq!(handle.load(&state_bytes), 0, "{}", last_error());
    let config = handle.config_json();
    assert_eq!(config["mode"], serde_json::json!("Wideband"));
    assert_eq!(config["threshold"], serde_json::json!(-20.0));
    handle
}

#[test]
fn ffi_deesser_external_hot_key_reduces_nonzero_program() {
    // Hot independent key drives the detector; the program bus carries
    // a quiet in-band tone that must reduce without muting. The input
    // buffer must survive the C call bit-identical (program/key
    // preservation) and the output width must be exactly 2ch.
    let frames = 8_192usize;
    let mut handle = wideband_key_handle(20.0);
    let input = keyed_input(frames, PROGRAM_PEAK, KEY_PEAK);
    let input_snapshot = input.clone();
    let output = handle.process(&input);
    assert_eq!(output.len(), frames * 2, "external output must stay 2ch");
    assert_eq!(input, input_snapshot, "C process must not mutate its input");
    let program_rms = channel_rms(&input, 4, 0, frames / 2);
    let hot_rms = channel_rms(&output, 2, 0, frames / 2);
    let hot_db = 20.0 * (hot_rms / program_rms).log10();
    assert!(
        hot_db < -6.0,
        "hot key must reduce the program past -6 dB, got {hot_db:.2} dB"
    );
    assert!(
        hot_db > -20.0,
        "hot key must not mute the program, got {hot_db:.2} dB"
    );
}

#[test]
fn ffi_deesser_external_silent_key_passes_program() {
    // Same route, silent key: with no key energy the quiet program
    // passes. Together with the hot leg this differential proves the
    // detector reads the key bus, not the program.
    let frames = 8_192usize;
    let mut handle = wideband_key_handle(20.0);
    // Realtime controls stay live on the external route.
    assert_eq!(handle.set_normalized("mix", 0.5), 0);
    assert_eq!(handle.get_normalized("mix"), 0.5);
    assert_eq!(handle.set_normalized("mix", 1.0), 0);
    assert_eq!(handle.get_normalized("mix"), 1.0);
    let input = keyed_input(frames, PROGRAM_PEAK, 0.0);
    let output = handle.process(&input);
    assert_eq!(output.len(), frames * 2);
    let program_rms = channel_rms(&input, 4, 0, frames / 2);
    let silent_rms = channel_rms(&output, 2, 0, frames / 2);
    let silent_db = 20.0 * (silent_rms / program_rms).log10();
    assert!(
        silent_db.abs() < 1.0,
        "silent key must pass the program within 1 dB, got {silent_db:.2} dB"
    );
}

#[test]
fn ffi_deesser_external_swapped_key_passes_hot_program() {
    // Hot tone on the program bus, quiet tone on the key bus: the
    // detector sees quiet and the hot program passes. A duplicated or
    // swapped key bus, or self-triggering on the program, would reduce
    // it instead. This is the no-key-leakage proof.
    let frames = 8_192usize;
    let mut handle = wideband_key_handle(20.0);
    let input = keyed_input(frames, KEY_PEAK, PROGRAM_PEAK);
    let output = handle.process(&input);
    assert_eq!(output.len(), frames * 2);
    let hot_program_rms = channel_rms(&input, 4, 0, frames / 2);
    let out_rms = channel_rms(&output, 2, 0, frames / 2);
    let ratio_db = 20.0 * (out_rms / hot_program_rms).log10();
    assert!(
        ratio_db.abs() < 1.0,
        "quiet key must pass the hot program within 1 dB, got {ratio_db:.2} dB"
    );
}

#[test]
fn ffi_deesser_partitioned_callbacks_render_bitwise() {
    // Irregular C callback partitions of one hot-key stream render
    // bit-identical audio through the real handle path.
    let frames = 2_048usize;
    let input = keyed_input(frames, PROGRAM_PEAK, KEY_PEAK);
    let mut full = wideband_key_handle(20.0);
    let max = full.inner().max_callback_frames;
    assert!(max >= 257, "partition test needs headroom, got {max}");
    let once = full.process_partitioned(&input, max);
    let mut split = wideband_key_handle(20.0);
    let twice = split.process_partitioned(&input, 257);
    let mut ragged = wideband_key_handle(20.0);
    let odd = ragged.process_partitioned(&input, 63);
    assert_eq!(once, twice, "257-frame partitions must match one block");
    assert_eq!(once, odd, "63-frame partitions must match one block");
}

#[test]
fn ffi_deesser_key_flip_refusal_preserves_engaged_history() {
    // Live + synchronized twin + cold control, all external-key with
    // engaged detectors. Flipping the key route changes the bus layout
    // and must fail WITHOUT a reset: state, config and the next
    // nonzero output agree with the untouched twin, while the cold
    // control proves the continuation carries retained history.
    const FRAMES: usize = 1_024;
    const BLOCKS: usize = 8;
    let mut live = wideband_key_handle(200.0);
    let mut twin = wideband_key_handle(200.0);
    let mut cold = wideband_key_handle(200.0);
    for _ in 0..BLOCKS {
        let input = keyed_input(FRAMES, PROGRAM_PEAK, KEY_PEAK);
        let live_out = live.process(&input);
        let twin_out = twin.process(&input);
        assert_eq!(live_out, twin_out, "populated history must agree");
    }
    assert!(
        live.process(&keyed_input(FRAMES, PROGRAM_PEAK, KEY_PEAK))
            .iter()
            .any(|sample| sample.abs() > 1.0e-3),
        "populated route must stay nonzero"
    );
    // Re-sync the twin: the extra live sanity block above advanced
    // only the live detector.
    twin.process(&keyed_input(FRAMES, PROGRAM_PEAK, KEY_PEAK));

    let before_state = live.save();
    let before_config = live.config_json();
    let flip = serde_json::to_vec(&serde_json::json!({"sidechain_external": false})).unwrap();
    assert_ne!(
        live.load(&flip),
        0,
        "bus-changing key flip must be refused"
    );
    let error = last_error();
    assert!(
        error.to_ascii_lowercase().contains("layout")
            || error.to_ascii_lowercase().contains("channel"),
        "refusal must name the bus mismatch, got: {error}"
    );
    assert_eq!(live.save(), before_state, "refusal preserves state");
    assert_eq!(live.config_json(), before_config, "refusal preserves config");

    // Malformed state is refused with the same preservation.
    assert_ne!(live.load(b"{not json"), 0, "garbage state must fail");
    assert_eq!(live.save(), before_state, "garbage refusal preserves state");

    // Continuation agrees with the untouched twin and stays live.
    let continued_input = keyed_input(FRAMES, PROGRAM_PEAK, KEY_PEAK);
    let continued_live = live.process(&continued_input);
    let continued_twin = twin.process(&continued_input);
    assert_eq!(continued_live, continued_twin, "refusal must not disturb history");
    assert!(
        continued_live.iter().any(|sample| sample.abs() > 1.0e-3),
        "rejected restore must leave the populated route processing"
    );
    // The cold control proves retained detector history rather than a
    // fresh attack transient: identical input, audibly different due
    // to the engaged envelope the twin pair shares.
    let cold_out = cold.process(&continued_input);
    let max_difference = continued_live
        .iter()
        .zip(cold_out.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_difference > 1.0e-4,
        "live continuation matches a cold handle; no retained history"
    );
}

#[test]
fn ffi_deesser_valid_same_width_restore_adopts_and_recovers() {
    // Internal 2ch route: a same-width structural restore commits
    // (lookahead adopted, audio renders with the new latency); a later
    // bus-changing flip is refused with preservation; a valid recovery
    // restore then still applies.
    let mut handle = KeyHandle::create("{}", 2, 2);
    let input = keyed_input(1_024, PROGRAM_PEAK, 0.0);
    let program: Vec<f32> = input
        .chunks(4)
        .flat_map(|frame| [frame[0], frame[1]])
        .collect();
    let restore = serde_json::to_vec(&serde_json::json!({
        "mode": 0,
        "lookahead_ms": 2.0,
    }))
    .unwrap();
    assert_eq!(handle.load(&restore), 0, "{}", last_error());
    assert_eq!(handle.config_json()["lookahead_ms"], serde_json::json!(2.0));
    assert_eq!(handle.config_json()["mode"], serde_json::json!("Wideband"));
    let adopted = handle.process(&program);
    assert!(adopted.iter().any(|sample| sample.abs() > 1.0e-3));

    let before_state = handle.save();
    let flip = serde_json::to_vec(&serde_json::json!({"sidechain_external": true})).unwrap();
    assert_ne!(handle.load(&flip), 0, "bus-changing flip must be refused");
    assert_eq!(handle.save(), before_state, "refusal preserves state");

    let recovery = serde_json::to_vec(&serde_json::json!({"lookahead_ms": 0.0})).unwrap();
    assert_eq!(handle.load(&recovery), 0, "{}", last_error());
    assert_eq!(handle.config_json()["lookahead_ms"], serde_json::json!(0.0));
    assert!(
        handle
            .process(&program)
            .iter()
            .any(|sample| sample.abs() > 1.0e-3),
        "recovery restore must render"
    );
}

#[test]
fn ffi_deesser_preset_format_round_trips_external_config() {
    // The preset C API format carries the external-key configuration:
    // export names the document, import into a fresh same-layout
    // handle adopts every committed value, and both handles render
    // bit-identical audio from reset. A tampered bus flip inside the
    // preset is refused with the live handle preserved.
    let mut source = wideband_key_handle(20.0);
    let document = source.export_preset("keyed-wideband");
    let parsed: serde_json::Value = serde_json::from_slice(&document).unwrap();
    assert_eq!(parsed["schema_version"], serde_json::json!(1));
    assert_eq!(parsed["plugin_type"], serde_json::json!("DeEsser"));
    assert_eq!(parsed["preset_name"], serde_json::json!("keyed-wideband"));

    let mut fresh = KeyHandle::create(r#"{"sidechain_external": true}"#, 4, 2);
    assert_eq!(fresh.import_preset(&document), 0, "{}", last_error());
    let imported = fresh.config_json();
    for (key, expected) in [
        ("mode", serde_json::json!("Wideband")),
        ("threshold", serde_json::json!(-20.0)),
        ("ratio", serde_json::json!(8.0)),
        ("attack_ms", serde_json::json!(0.5)),
        ("release_ms", serde_json::json!(20.0)),
        ("mix", serde_json::json!(1.0)),
        ("range_db", serde_json::json!(60.0)),
        ("stereo_link", serde_json::json!(0.0)),
        ("sidechain_external", serde_json::json!(true)),
    ] {
        assert_eq!(imported[key], expected, "preset must persist {key}");
    }
    source.reset();
    fresh.reset();
    let input = keyed_input(2_048, PROGRAM_PEAK, KEY_PEAK);
    assert_eq!(
        source.process(&input),
        fresh.process(&input),
        "preset import must render identically from reset"
    );

    // Tamper the inner state bytes: flip the key route off. Import
    // must refuse and leave the live handle untouched.
    let before_state = fresh.save();
    let mut tampered = parsed.clone();
    let state_bytes: Vec<u8> = serde_json::from_value(tampered["state"].clone()).unwrap();
    let mut state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&state_bytes).unwrap();
    state.insert("sidechain_external".to_string(), serde_json::json!(false));
    let tampered_bytes = serde_json::to_vec(&state).unwrap();
    tampered["state"] = serde_json::to_value(tampered_bytes).unwrap();
    let tampered_document = serde_json::to_vec(&tampered).unwrap();
    assert_ne!(
        fresh.import_preset(&tampered_document),
        0,
        "tampered bus flip must be refused"
    );
    assert_eq!(fresh.save(), before_state, "tampered import preserves state");
}
