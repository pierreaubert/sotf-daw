//! Limiter factor persistence through actual transactional C state entry points.

// Rust guideline compliant 2026-02-21
use crate::{
    PluginHandle, plugin_create, plugin_destroy, plugin_export_preset_json, plugin_free_state,
    plugin_free_string, plugin_get_info_json, plugin_get_last_error, plugin_get_parameter,
    plugin_import_preset_json, plugin_load_state, plugin_process, plugin_save_state,
    plugin_set_parameter,
};
use std::ffi::{CStr, CString};

struct Handle(*mut PluginHandle);
impl Handle {
    fn new(rate: u32, choice: Option<i32>) -> Self {
        let mut config = serde_json::json!({"threshold_db":-12.0,"lookahead_ms":2.0,"_sotf_max_callback_frames":257});
        if let Some(choice) = choice {
            config["oversampling"] = choice.into();
        }
        let config = CString::new(config.to_string()).unwrap();
        let pointer = plugin_create(c"Limiter".as_ptr(), config.as_ptr(), rate, 2, 2);
        assert!(!pointer.is_null(), "{}", error());
        Self(pointer)
    }
    fn state(&self, document: bool) -> Vec<u8> {
        let mut length = 0;
        let pointer = if document {
            plugin_export_preset_json(self.0, c"Oversampling".as_ptr(), &mut length)
        } else {
            plugin_save_state(self.0, &mut length)
        };
        assert!(!pointer.is_null(), "{}", error());
        // SAFETY: This live handle returned exactly length initialized bytes.
        // The Rust copy is independent before the C allocation is released.
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
        plugin_free_state(pointer, length);
        bytes
    }
    fn load(&mut self, bytes: &[u8], document: bool) -> i32 {
        if document {
            plugin_import_preset_json(self.0, bytes.as_ptr(), bytes.len())
        } else {
            plugin_load_state(self.0, bytes.as_ptr(), bytes.len())
        }
    }
    fn latency(&self) -> usize {
        let pointer = plugin_get_info_json(self.0);
        assert!(!pointer.is_null());
        // SAFETY: The C API returns an owned, null-terminated string.
        let value: serde_json::Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        plugin_free_string(pointer);
        value["latency_samples"].as_u64().unwrap() as usize
    }
    fn render(&mut self, input: &[f32]) -> Vec<f32> {
        let mut output = vec![f32::NAN; input.len()];
        for (input, output) in input.chunks(514).zip(output.chunks_mut(514)) {
            assert_eq!(
                plugin_process(self.0, input.as_ptr(), output.as_mut_ptr(), input.len() / 2),
                0,
                "{}",
                error()
            );
        }
        output
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}
fn error() -> String {
    let pointer = plugin_get_last_error();
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: The diagnostic belongs to this thread and is copied immediately.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}
fn markers() -> Vec<f32> {
    let mut input = vec![0.0; 8192];
    input[0] = 0.001;
    input[1] = -0.002;
    input[4094] = -0.001;
    input[4095] = 0.002;
    input
}

fn preset_with_state(state: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "ut_type": "org.spinorama.sotf.plugin-preset",
        "plugin_type": "Limiter",
        "state": state,
    }))
    .unwrap()
}

#[test]
fn limiter_ffi_factor_roundtrips_state_and_documents_before_preparation() {
    for rate in [44_100, 48_000, 96_000] {
        for document in [false, true] {
            let mut legacy = Handle::new(rate, None);
            let input = markers();
            let legacy_output = legacy.render(&input);
            for choice in 0..=2 {
                let source = Handle::new(rate, Some(choice));
                let saved = source.state(document);
                let mut restored = Handle::new(rate, None);
                assert_eq!(restored.load(&saved, document), 0, "{}", error());
                assert_eq!(
                    plugin_get_parameter(restored.0, c"oversampling".as_ptr()),
                    f64::from(choice) / 2.0
                );
                let delay =
                    (u64::from(rate) * 2 / 1000) as usize + if choice == 0 { 0 } else { 512 };
                assert_eq!(restored.latency(), delay);
                let mut fresh = Handle::new(rate, Some(choice));
                let output = restored.render(&input);
                assert!(output == fresh.render(&input));
                if choice == 0 {
                    assert_eq!(output, legacy_output);
                } else {
                    assert!(output != legacy_output);
                }
                for origin in [0, 2047] {
                    for channel in 0..2 {
                        let peak = (origin..origin + 1024)
                            .max_by(|&a, &b| {
                                output[a * 2 + channel]
                                    .abs()
                                    .total_cmp(&output[b * 2 + channel].abs())
                            })
                            .unwrap();
                        assert_eq!(peak, origin + delay);
                    }
                }
                // Omitted setup values in a partial preset preserve the factor.
                let partial = if document {
                    preset_with_state(b"{}")
                } else {
                    b"{}".to_vec()
                };
                assert_eq!(restored.load(&partial, document), 0, "{}", error());
                assert_eq!(restored.latency(), delay);
                assert_eq!(
                    plugin_get_parameter(restored.0, c"oversampling".as_ptr()),
                    f64::from(choice) / 2.0
                );
            }
        }
    }
}

#[test]
fn limiter_ffi_rejects_active_factor_changes_and_invalid_presets_transactionally() {
    for choice in 0..=2 {
        for document in [false, true] {
            for invalid in [
                serde_json::json!(3),
                serde_json::json!(-1),
                serde_json::json!(0.5),
                serde_json::json!("2x"),
            ] {
                let mut plugin = Handle::new(48_000, Some(choice));
                let mut reference = Handle::new(48_000, Some(choice));
                let warm: Vec<_> = (0..734)
                    .map(|index| (index as f32 * 0.071).sin() * 0.7)
                    .collect();
                assert_eq!(plugin.render(&warm), reference.render(&warm));
                let saved = plugin.state(false);
                let latency = plugin.latency();
                assert_eq!(
                    plugin_set_parameter(
                        plugin.0,
                        c"oversampling".as_ptr(),
                        f64::from(choice) / 2.0
                    ),
                    0
                );
                assert_ne!(
                    plugin_set_parameter(
                        plugin.0,
                        c"oversampling".as_ptr(),
                        f64::from((choice + 1) % 3) / 2.0
                    ),
                    0
                );
                let state = serde_json::to_vec(
                    &serde_json::json!({"lookahead":1.0,"oversampling":invalid}),
                )
                .unwrap();
                let payload = if document {
                    preset_with_state(&state)
                } else {
                    state
                };
                assert_ne!(plugin.load(&payload, document), 0);
                assert_eq!(plugin.state(false), saved);
                assert_eq!(plugin.latency(), latency);
                assert_eq!(
                    plugin.render(&markers()),
                    reference.render(&markers()),
                    "rejected state must retain delayed audio and gain history"
                );
            }
        }
    }
}
