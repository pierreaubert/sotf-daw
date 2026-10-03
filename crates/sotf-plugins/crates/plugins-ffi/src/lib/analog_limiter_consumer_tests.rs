//! Analog limiter through the public C ABI: create, params, state, process.
//!
//! Every test drives the C entry points (`plugin_create`, parameter
//! enumeration, `plugin_set_parameter`, `plugin_save_state`,
//! `plugin_load_state`, `plugin_reset`, `plugin_process`) rather than calling
//! plugin crates directly. Audio assertions use real renders, and failed
//! restores must retain the old audio path and configuration byte-for-byte.

use crate::*;
use sotf_host::{ParameterId, ParameterValue};
use std::ffi::{CStr, CString};

struct AbiHandle {
    pointer: *mut PluginHandle,
}

impl AbiHandle {
    fn create(kind: &str, config: &str, rate: u32, channels: usize) -> Self {
        let kind_c = CString::new(kind).unwrap();
        let config_c = CString::new(config).unwrap();
        let handle = plugin_create(kind_c.as_ptr(), config_c.as_ptr(), rate, channels, channels);
        assert!(
            !handle.is_null(),
            "{kind} construction failed: {err}",
            err = abi_last_error()
        );
        Self { pointer: handle }
    }

    fn inner(&self) -> &PluginHandle {
        // SAFETY: This guard owns a live handle, accessed only on this thread.
        unsafe { &*self.pointer }
    }

    fn param_count(&self) -> usize {
        let count = plugin_get_parameter_count(self.pointer);
        assert!(count >= 0);
        count as usize
    }

    fn param_id(&self, index: usize) -> String {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: The info pointer borrows from this live handle.
        let id = unsafe { (*info).id };
        assert!(!id.is_null());
        // SAFETY: IDs are valid NUL-terminated strings while live.
        unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned()
    }

    fn param_min_max_default(&self, index: usize) -> (f64, f64, f64) {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: The info pointer borrows from this live handle.
        unsafe {
            let info = &*info;
            (info.min_value, info.max_value, info.default_value)
        }
    }

    fn param_steps(&self, index: usize) -> u32 {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: The info pointer borrows from this live handle.
        unsafe { (*info).steps }
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

    fn reset(&mut self) {
        assert_eq!(plugin_reset(self.pointer), 0, "{}", abi_last_error());
    }

    fn latency(&self) -> usize {
        self.inner().plugin.latency_samples()
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let channels = self.inner().input_channels;
        let bound = self.inner().max_callback_frames;
        let frames = input.len() / channels;
        assert_eq!(
            input.len() % channels,
            0,
            "complete interleaved frames required"
        );
        let mut output = vec![f32::NAN; input.len()];
        for start in (0..frames).step_by(bound.max(1)) {
            let count = bound.max(1).min(frames - start);
            assert_eq!(
                plugin_process(
                    self.pointer,
                    input[start * channels..].as_ptr(),
                    output[start * channels..].as_mut_ptr(),
                    count
                ),
                0,
                "{}",
                abi_last_error()
            );
        }
        assert!(output.iter().all(|value| value.is_finite()));
        output
    }
}

impl Drop for AbiHandle {
    fn drop(&mut self) {
        plugin_destroy(self.pointer);
    }
}

fn abi_last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

fn ceiling_f32(threshold_db: f32) -> f32 {
    10f32.powf(threshold_db / 20.0)
}

fn hot_interleaved(frames: usize, channels: usize, rate: u32, peak: f32) -> Vec<f32> {
    let mut buffer = vec![0.0; frames * channels];
    for frame in 0..frames {
        let sample = peak * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / rate as f32).sin();
        for channel in 0..channels {
            buffer[frame * channels + channel] = sample;
        }
    }
    buffer
}

fn settled_peak(output: &[f32], channels: usize) -> f32 {
    output[2048 * channels..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn ffi_analog_limiter_param_schema_order_and_defaults() {
    let handle = AbiHandle::create("AnalogLimiter", "{}", 48_000, 2);
    assert_eq!(handle.param_count(), 11);
    let ids: Vec<_> = (0..handle.param_count())
        .map(|index| handle.param_id(index))
        .collect();
    assert_eq!(
        ids,
        [
            "threshold",
            "release",
            "lookahead",
            "soft",
            "true_peak",
            "mix",
            "analog_model",
            "analog_drive",
            "analog_color",
            "analog_character",
            "analog_trim",
        ]
    );
    assert_eq!(handle.param_min_max_default(0), (-20.0, 0.0, -0.1));
    assert_eq!(handle.param_min_max_default(2), (0.0, 20.0, 5.0));
    // No choice-label assertion: `choice_label_at` serves labels only for
    // explicitly-wired families (speech model, declick, crossover,
    // dynamic-EQ, EQ placements) and returns None otherwise by contract
    // (`parameter_map.rs`). The retained API scope is enumerability: the
    // model exposes index range 0..=5 over six choices, and stays fully
    // controllable through the C ABI via normalized index, proven by
    // `ffi_analog_limiter_normalized_roundtrip_and_structural_model`.
    assert_eq!(handle.param_min_max_default(6), (0.0, 5.0, 0.0));
    assert_eq!(handle.param_steps(6), 6);
}

#[test]
fn ffi_analog_limiter_both_spellings_create_and_unknown_fails_null() {
    for spelling in ["AnalogLimiter", "analog_limiter"] {
        let handle = AbiHandle::create(spelling, r#"{"threshold": -12.0}"#, 48_000, 2);
        assert_eq!(handle.param_count(), 11);
        assert_eq!(handle.latency(), 240);
    }
    let kind = CString::new("analog_limiter_typo").unwrap();
    let config = CString::new("{}").unwrap();
    assert!(plugin_create(kind.as_ptr(), config.as_ptr(), 48_000, 2, 2).is_null());
}

#[test]
fn ffi_analog_limiter_normalized_roundtrip_and_structural_model() {
    let mut handle = AbiHandle::create("AnalogLimiter", "{}", 48_000, 2);
    assert_eq!(handle.set_normalized("threshold", 0.4), 0);
    assert!((handle.get_normalized("threshold") - 0.4).abs() < 1e-9);
    assert_eq!(
        handle
            .inner()
            .plugin
            .get_parameter(&ParameterId::from("threshold")),
        Some(ParameterValue::Float(-12.0))
    );
    // The model is structural: repeating the committed default (index 0)
    // is a no-op success, while switching to Tape (0.6) on the live
    // handle is refused without touching the committed selection.
    assert_eq!(handle.set_normalized("analog_model", 0.0), 0);
    assert!((handle.get_normalized("analog_model") - 0.0).abs() < 1e-9);
    assert_ne!(handle.set_normalized("analog_model", 0.6), 0);
    assert_eq!(
        handle
            .inner()
            .plugin
            .get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Harmonics".to_string()))
    );
    // Constructor adoption still selects Tape (index 3 of six choices).
    let created = AbiHandle::create("AnalogLimiter", r#"{"analog_model": "Tape"}"#, 48_000, 2);
    assert!((created.get_normalized("analog_model") - 0.6).abs() < 1e-9);
    assert_eq!(
        created
            .inner()
            .plugin
            .get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
}

#[test]
fn ffi_analog_limiter_hot_colored_render_respects_ceiling() {
    for channels in [1, 2, 6] {
        let mut handle = AbiHandle::create(
            "analog_limiter",
            r#"{"threshold": -12.0, "analog_model": "Tape", "analog_drive": 6.0, "analog_color": 0.5, "analog_character": 0.25}"#,
            48_000,
            channels,
        );
        let input = hot_interleaved(4096, channels, 48_000, 0.9);
        let output = handle.process(&input);
        let peak = settled_peak(&output, channels);
        let ceiling = ceiling_f32(-12.0);
        assert!(
            peak <= ceiling,
            "ch={channels}: peak {peak} exceeds ceiling {ceiling}"
        );
        assert!(
            peak > ceiling * 0.1,
            "ch={channels}: peak {peak} is suspiciously quiet"
        );
    }
}

#[test]
fn ffi_analog_limiter_state_save_mutate_load_roundtrip() {
    let mut handle = AbiHandle::create(
        "AnalogLimiter",
        r#"{"threshold": -12.0, "analog_model": "Tape", "analog_color": 0.5}"#,
        48_000,
        2,
    );
    let input = hot_interleaved(4096, 2, 48_000, 0.9);
    let before = handle.process(&input);
    handle.reset();
    let saved = handle.save();
    assert_eq!(handle.set_normalized("threshold", 0.1), 0);
    assert_eq!(handle.load(&saved), 0);
    assert!((handle.get_normalized("threshold") - 0.4).abs() < 1e-9);
    assert_eq!(
        handle
            .inner()
            .plugin
            .get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
    let after = handle.process(&input);
    assert_eq!(before, after);
}

#[test]
fn ffi_analog_limiter_failed_restore_retains_audio_and_config() {
    let mut handle = AbiHandle::create(
        "AnalogLimiter",
        r#"{"threshold": -12.0, "analog_model": "Tape", "analog_color": 0.5}"#,
        48_000,
        2,
    );
    let input = hot_interleaved(4096, 2, 48_000, 0.9);
    let before = handle.process(&input);
    handle.reset();
    assert_ne!(handle.load(b"\x00\x01not-json"), 0);
    assert!((handle.get_normalized("threshold") - 0.4).abs() < 1e-9);
    assert_eq!(
        handle
            .inner()
            .plugin
            .get_parameter(&ParameterId::from("analog_model")),
        Some(ParameterValue::String("Tape".to_string()))
    );
    let after = handle.process(&input);
    assert_eq!(before, after);
}

#[test]
fn ffi_analog_limiter_reset_reproduces_audio() {
    let mut handle = AbiHandle::create(
        "AnalogLimiter",
        r#"{"threshold": -12.0, "analog_model": "Tape", "analog_color": 0.5}"#,
        48_000,
        2,
    );
    let input = hot_interleaved(4096, 2, 48_000, 0.9);
    let first = handle.process(&input);
    handle.reset();
    let second = handle.process(&input);
    assert_eq!(first, second);
}
