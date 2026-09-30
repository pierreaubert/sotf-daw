//! Preset envelope identity and transactional refusal regressions.

// Rust guideline compliant 2026-02-21
use crate::{
    PluginError, PluginHandle, plugin_create, plugin_destroy, plugin_export_preset_json,
    plugin_free_state, plugin_import_preset_json, plugin_process, plugin_save_state,
    plugin_set_parameter,
};
use serde_json::{Value, json};
use std::ffi::CString;

struct TestHandle(*mut PluginHandle);

impl TestHandle {
    fn new(plugin_type: &str) -> Self {
        let plugin_type = CString::new(plugin_type).unwrap();
        let config =
            if plugin_type.to_bytes() == b"DynamicEQ" || plugin_type.to_bytes() == b"dynamic_eq" {
                json!({
                    "num_bands": 4,
                    "threshold": -24.0,
                    "ratio": 3.0,
                    "attack_ms": 2.0,
                    "release_ms": 55.0,
                    "knee": 3.0,
                    "link_channels": true,
                    "mix": 1.0,
                    "bands": [{"gain": 6.0}],
                })
            } else {
                json!({})
            };
        let config = CString::new(config.to_string()).unwrap();
        let handle = plugin_create(plugin_type.as_ptr(), config.as_ptr(), 48_000, 2, 2);
        assert!(!handle.is_null(), "plugin creation succeeds");
        Self(handle)
    }

    fn state(&self) -> Vec<u8> {
        let mut length = 0;
        let pointer = plugin_save_state(self.0, &mut length);
        assert!(!pointer.is_null(), "state export succeeds");
        // SAFETY: plugin_save_state returns `length` initialized bytes owned by
        // this caller until they are released through plugin_free_state.
        let state = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
        plugin_free_state(pointer, length);
        state
    }

    fn exported_document(&self) -> Value {
        let mut length = 0;
        let pointer = plugin_export_preset_json(self.0, c"AUD144".as_ptr(), &mut length);
        assert!(!pointer.is_null(), "preset export succeeds");
        // SAFETY: plugin_export_preset_json returns `length` initialized bytes
        // owned by this caller until they are released through plugin_free_state.
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
        plugin_free_state(pointer, length);
        serde_json::from_slice(&bytes).unwrap()
    }

    fn import(&mut self, document: &Value) -> i32 {
        let bytes = serde_json::to_vec(document).unwrap();
        plugin_import_preset_json(self.0, bytes.as_ptr(), bytes.len())
    }

    fn set_normalized(&mut self, parameter: &str, value: f64) -> i32 {
        let parameter = CString::new(parameter).unwrap();
        plugin_set_parameter(self.0, parameter.as_ptr(), value)
    }

    fn process(&mut self, offset: usize, frames: usize) -> Vec<f32> {
        let mut input = Vec::with_capacity(frames * 2);
        for frame in offset..offset + frames {
            let time = frame as f64 / 48_000.0;
            input.push((0.3 * (std::f64::consts::TAU * 440.0 * time).sin()) as f32);
            input.push((0.17 * (std::f64::consts::TAU * 997.0 * time).cos()) as f32);
        }
        let mut output = vec![f32::NAN; input.len()];
        assert_eq!(
            plugin_process(self.0, input.as_ptr(), output.as_mut_ptr(), frames),
            PluginError::Success as i32
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        output
    }
}

impl Drop for TestHandle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn append_continuation(handle: &mut TestHandle) -> Vec<f32> {
    let mut output = Vec::new();
    for (offset, frames) in [(771, 127), (898, 509)] {
        output.extend(handle.process(offset, frames));
    }
    output
}

fn maximum_difference(left: &[f32], right: &[f32]) -> f32 {
    assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0, f32::max)
}

#[test]
fn aud144_invalid_envelopes_are_refused_before_live_state_or_history_changes() {
    let mutations: [(&str, Option<(&str, Value)>); 12] = [
        ("wrong_plugin", Some(("plugin_type", json!("Gain")))),
        ("missing_plugin", Some(("plugin_type", Value::Null))),
        ("typed_plugin", Some(("plugin_type", json!(42)))),
        ("future_schema", Some(("schema_version", json!(2)))),
        ("zero_schema", Some(("schema_version", json!(0)))),
        ("missing_schema", Some(("schema_version", Value::Null))),
        ("typed_schema", Some(("schema_version", json!("1")))),
        ("float_schema", Some(("schema_version", json!(1.0)))),
        (
            "wrong_ut_type",
            Some(("ut_type", json!("org.example.unrelated"))),
        ),
        ("missing_ut_type", Some(("ut_type", Value::Null))),
        ("typed_ut_type", Some(("ut_type", json!(false)))),
        ("unknown_alias", Some(("plugin_type", json!("dynamic-eq")))),
    ];

    // This is a real, different-family factory handle, not an arbitrary name.
    let _different_family = TestHandle::new("Gain");

    for (case, mutation) in mutations {
        let mut live = TestHandle::new("DynamicEQ");
        let mut twin = TestHandle::new("DynamicEQ");
        let mut cold = TestHandle::new("DynamicEQ");
        for block in 0..3 {
            let offset = block * 257;
            assert_eq!(live.process(offset, 257), twin.process(offset, 257));
        }

        let saved_before = live.state();
        let mut document = live.exported_document();
        if let Some((field, value)) = mutation {
            if value.is_null() {
                document.as_object_mut().unwrap().remove(field);
            } else {
                document[field] = value;
            }
        }
        assert_eq!(
            live.import(&document),
            PluginError::InvalidConfig as i32,
            "{case} must be rejected"
        );
        assert_eq!(
            live.state(),
            saved_before,
            "{case} changed serialized state"
        );

        let live_tail = append_continuation(&mut live);
        let twin_tail = append_continuation(&mut twin);
        let cold_tail = append_continuation(&mut cold);
        assert_eq!(live_tail, twin_tail, "{case} changed processing history");
        assert!(
            maximum_difference(&cold_tail, &twin_tail) > 1.0e-6,
            "{case} continuation must distinguish populated from fresh DSP"
        );
    }
}

#[test]
fn aud144_valid_alias_and_editable_name_imports_are_accepted() {
    let mut source = TestHandle::new("DynamicEQ");
    let mut target = TestHandle::new("dynamic_eq");
    let mut control = TestHandle::new("dynamic_eq");
    for block in 0..3 {
        let offset = block * 257;
        source.process(offset, 257);
    }

    let state = source.state();
    let mut document = source.exported_document();
    document["plugin_type"] = json!("DynamicEQ");
    document["preset_name"] = json!("renamed without changing identity");
    assert_eq!(
        target.import(&document),
        PluginError::Success as i32,
        "same-family factory alias must import"
    );
    assert_eq!(target.state(), state);
    assert_eq!(
        append_continuation(&mut target),
        append_continuation(&mut control)
    );
}

#[test]
fn aud144_legacy_fletcher_munson_preset_migrates_to_loudness_compensation() {
    let mut source = TestHandle::new("FletcherMunson");
    for (parameter, value) in [("low_gain", 0.85), ("high_gain", 0.2), ("mid_gain", 0.75)] {
        assert_eq!(
            source.set_normalized(parameter, value),
            PluginError::Success as i32,
            "set legacy {parameter} through the C API"
        );
    }
    for block in 0..3 {
        source.process(block * 257, 257);
    }

    let expected_state = source.state();
    let document = source.exported_document();
    assert_eq!(document["plugin_type"], "FletcherMunson");
    assert_eq!(document["schema_version"], 1);

    let mut legacy_control = TestHandle::new("FletcherMunson");
    let mut migrated = TestHandle::new("LoudnessCompensation");
    assert_eq!(
        legacy_control.import(&document),
        PluginError::Success as i32,
        "the legacy family continues to import its own preset"
    );
    assert_eq!(
        migrated.import(&document),
        PluginError::Success as i32,
        "LoudnessCompensation accepts genuine legacy FletcherMunson presets"
    );
    assert_eq!(migrated.state(), expected_state);
    assert_eq!(legacy_control.state(), expected_state);

    let migrated_tail = append_continuation(&mut migrated);
    let legacy_tail = append_continuation(&mut legacy_control);
    assert_eq!(migrated_tail, legacy_tail);
    let mut default_loudness = TestHandle::new("LoudnessCompensation");
    assert!(
        maximum_difference(&migrated_tail, &append_continuation(&mut default_loudness)) > 1.0e-6,
        "the imported legacy settings must affect the output"
    );

    let replacement_preset = migrated.exported_document();
    assert_eq!(replacement_preset["plugin_type"], "LoudnessCompensation");
    let mut legacy_target = TestHandle::new("FletcherMunson");
    assert_eq!(
        legacy_target.import(&replacement_preset),
        PluginError::InvalidConfig as i32,
        "the legacy preset migration is one-way"
    );
}
