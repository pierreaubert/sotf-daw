//! State serialization for plugin presets (save/load).
//!
//! Converts a plugin's current parameter state to/from a JSON blob,
//! suitable for AU `fullState` and VST3 state chunks.
//!
//! Hiss Reducer carries its measured noise profile out of band under the
//! canonical `captured_profile` key (exact v1/v2 `NoiseProfileData`).
//! Momentary `learn_noise`/`clear_profile` commands are never saved and
//! never replayed from legacy state.

#[path = "hiss_state.rs"]
pub mod hiss_state;

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::Plugin;

/// Save plugin state, failing explicitly on Hiss contention.
///
/// Serializes named scalar parameters plus, for actual Hiss instances
/// identified by the `get_data` snapshot downcast, the exact stored
/// `captured_profile` blob when present. Momentary Hiss commands are
/// omitted. Non-Hiss plugins keep the scalar-only shape.
///
/// # Errors
///
/// Returns an error when a Hiss export contends (`SnapshotBusy`, distinct
/// from absence) or when serialization fails. Callers must treat `Err` as
/// a failed save: retry later, never persist a truncated blob.
///
/// # Panics
///
/// Never panics on valid plugin state.
pub fn try_save_state(plugin: &dyn Plugin) -> Result<Vec<u8>, String> {
    let snapshot = hiss_state::hiss_snapshot(plugin);
    let is_hiss_plugin = snapshot.is_some();
    let params = plugin.parameters();
    let mut map = serde_json::Map::new();

    for param in &params {
        if is_hiss_plugin && hiss_state::is_momentary_id(param.id.as_str()) {
            continue;
        }
        if let Some(value) = plugin.get_parameter(&param.id) {
            let json_val = match value {
                ParameterValue::Float(f) => serde_json::Value::from(f),
                ParameterValue::Int(i) => serde_json::Value::from(i),
                ParameterValue::Bool(b) => serde_json::Value::from(b),
                ParameterValue::String(s) => serde_json::Value::from(s),
            };
            map.insert(param.id.to_string(), json_val);
        }
    }

    if let Some(snapshot) = snapshot {
        match snapshot.try_export() {
            Ok(Some(export)) => {
                let profile = serde_json::to_value(&export.profile)
                    .map_err(|error| format!("Failed to serialize Hiss profile: {error}"))?;
                map.insert(hiss_state::CAPTURED_PROFILE_KEY.to_string(), profile);
            }
            Ok(None) => {}
            Err(_) => {
                return Err(
                    "Hiss profile snapshot busy: retry the save on a later control tick"
                        .to_string(),
                );
            }
        }
    }

    serde_json::to_vec(&serde_json::Value::Object(map))
        .map_err(|error| format!("Failed to serialize plugin state: {error}"))
}

/// Save all plugin parameters to a JSON byte vector.
///
/// Returns a JSON object mapping parameter ID → value, plus the Hiss
/// `captured_profile` carrier when present. This infallible wrapper exists
/// for legacy callers: on Hiss contention it returns an empty vector, which
/// fails parsing on load rather than yielding an apparently valid state
/// that silently omits a captured profile. Prefer [`try_save_state`] to
/// distinguish contention from absence.
pub fn save_state(plugin: &dyn Plugin) -> Vec<u8> {
    try_save_state(plugin).unwrap_or_default()
}

/// Load plugin parameters from a JSON byte slice.
///
/// The data should be a JSON object mapping parameter ID → value,
/// as produced by `save_state`.
/// Unchanged values are retained without invoking a setter. Changed structural
/// values still require reconstruction by the caller. This in-place helper is
/// not transactional: a later setter error can follow earlier successful writes.
/// Hiss momentary commands never route through setters here and are skipped.
/// A Hiss state carrying `captured_profile` (install or explicit null clear)
/// is rejected before any mutation: in-place load cannot install profiles,
/// so success would silently drop the blob. The FFI transactional restore
/// owns profile installation; native reconstruction stays open.
///
/// # Errors
///
/// Returns an error when the bytes are not a JSON object, when a known value
/// has the wrong type, when a setter fails, or when a Hiss state carries a
/// profile the in-place path cannot install.
pub fn load_state(plugin: &mut dyn Plugin, data: &[u8]) -> Result<(), String> {
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(data).map_err(|e| format!("Failed to parse state: {e}"))?;
    let is_hiss_plugin = hiss_state::is_hiss(&*plugin);
    if is_hiss_plugin && map.contains_key(hiss_state::CAPTURED_PROFILE_KEY) {
        return Err(
            "HissReducer captured_profile requires transactional profile restore; \
             bridge in-place load cannot install it"
                .to_string(),
        );
    }
    let params = plugin.parameters();

    for (key, json_val) in &map {
        if hiss_state::should_skip_hiss_load_key(is_hiss_plugin, key) {
            continue;
        }
        let Some(param) = params.iter().find(|param| param.id.as_str() == key) else {
            continue;
        };
        let value = value_from_json(key, json_val, &param.default_value)?;
        // Restoring an unchanged structural control must not require a rebuild
        // or reset DSP history. Changed values still obey the plugin lifecycle.
        if plugin.get_parameter(&param.id).as_ref() == Some(&value) {
            continue;
        }

        plugin
            .set_parameter(ParameterId::from(key.clone()), value)
            .map_err(|e| format!("Failed to load parameter '{key}': {e}"))?;
    }

    Ok(())
}

fn value_from_json(
    key: &str,
    json_val: &serde_json::Value,
    default_value: &ParameterValue,
) -> Result<ParameterValue, String> {
    match (json_val, default_value) {
        (serde_json::Value::Number(n), ParameterValue::Float(_)) => n
            .as_f64()
            .map(|f| ParameterValue::Float(f as f32))
            .ok_or_else(|| format!("Invalid numeric value for parameter '{key}'")),
        (serde_json::Value::Number(n), ParameterValue::Int(_)) => {
            let i = n
                .as_i64()
                .ok_or_else(|| format!("Invalid integer value for parameter '{key}'"))?;
            let i = i32::try_from(i)
                .map_err(|_| format!("Integer value out of range for parameter '{key}'"))?;
            Ok(ParameterValue::Int(i))
        }
        (serde_json::Value::Bool(b), ParameterValue::Bool(_)) => Ok(ParameterValue::Bool(*b)),
        (serde_json::Value::String(s), ParameterValue::String(_)) => {
            Ok(ParameterValue::String(s.clone()))
        }
        (_, expected) => Err(format!(
            "Invalid state value for parameter '{key}': expected {}",
            parameter_type_name(expected)
        )),
    }
}

fn parameter_type_name(value: &ParameterValue) -> &'static str {
    match value {
        ParameterValue::Float(_) => "float",
        ParameterValue::Int(_) => "integer",
        ParameterValue::Bool(_) => "bool",
        ParameterValue::String(_) => "string",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::create_plugin;
    use sotf_host::parameters::Parameter;
    use sotf_host::plugin::{PluginInfo, ProcessContext};
    use std::collections::HashMap;

    #[test]
    fn test_save_load_roundtrip() {
        let mut plugin = create_plugin("Gain", 2, 48000.0, "{}").unwrap();
        plugin.initialize(48000.0).unwrap();

        // Set a parameter
        let id = ParameterId::from("gain_db");
        plugin
            .set_parameter(id.clone(), ParameterValue::Float(3.0))
            .unwrap();

        // Save state
        let state = save_state(&*plugin);
        assert!(!state.is_empty());

        // Create a fresh plugin and load state
        let mut plugin2 = create_plugin("Gain", 2, 48000.0, "{}").unwrap();
        plugin2.initialize(48000.0).unwrap();
        load_state(&mut *plugin2, &state).unwrap();

        // Verify parameter was restored
        let value = plugin2.get_parameter(&id);
        match value {
            Some(ParameterValue::Float(f)) => {
                assert!((f - 3.0).abs() < 0.01, "Expected gain_db=3.0, got {f}");
            }
            other => panic!("Expected Float(3.0), got {other:?}"),
        }
    }

    #[test]
    fn test_load_invalid_json() {
        let mut plugin = create_plugin("Gain", 2, 48000.0, "{}").unwrap();
        let result = load_state(&mut *plugin, b"not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_unknown_params_ignored() {
        let mut plugin = create_plugin("Gain", 2, 48000.0, "{}").unwrap();
        let data = br#"{"unknown_param": 42.0, "gain_db": 5.0}"#;
        let result = load_state(&mut *plugin, data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_unknown_param_does_not_call_setter() {
        let mut plugin = RecordingPlugin::default();
        let data = br#"{"unknown_param": 42.0, "known": 0.5}"#;

        load_state(&mut plugin, data).unwrap();

        assert_eq!(
            plugin.values.get("known"),
            Some(&ParameterValue::Float(0.5))
        );
        assert!(plugin.set_calls.contains(&"known".to_string()));
        assert!(!plugin.set_calls.contains(&"unknown_param".to_string()));
    }

    #[test]
    fn test_load_reports_known_parameter_error() {
        let mut plugin = RecordingPlugin::default();
        let data = br#"{"known": 2.0}"#;

        let err = load_state(&mut plugin, data).unwrap_err();

        assert!(
            err.contains("known"),
            "error should identify the failing parameter: {err}"
        );
        assert!(
            err.contains("invalid known value"),
            "setter error should be preserved: {err}"
        );
    }

    #[derive(Default)]
    struct RecordingPlugin {
        values: HashMap<String, ParameterValue>,
        set_calls: Vec<String>,
    }

    impl Plugin for RecordingPlugin {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Recording", "0.0.0", "SOTF")
        }

        fn input_channels(&self) -> usize {
            2
        }

        fn output_channels(&self) -> usize {
            2
        }

        fn parameters(&self) -> Vec<Parameter> {
            vec![Parameter::new_float("known", "Known", 0.0, -1.0, 1.0)]
        }

        fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
            self.set_calls.push(id.to_string());
            match (id.as_str(), value) {
                ("known", ParameterValue::Float(v)) if (-1.0..=1.0).contains(&v) => {
                    self.values.insert(id.to_string(), ParameterValue::Float(v));
                    Ok(())
                }
                ("known", _) => Err("invalid known value".to_string()),
                _ => Err(format!("Unknown parameter: {id}")),
            }
        }

        fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
            self.values.get(id.as_str()).cloned()
        }

        fn process(
            &mut self,
            _input: &[f32],
            _output: &mut [f32],
            context: &ProcessContext,
        ) -> Result<usize, String> {
            Ok(context.num_frames)
        }
    }
}
