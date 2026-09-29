//! Restore construction settings on the control thread before starting audio.

use super::DynamicParams;
use serde_json::{Map, Value};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::Plugin;

/// Construct the DSP instance represented by persisted DAW parameter state.
///
/// This allocates and may rebuild filters. Call only from the host's activation
/// lifecycle, never from the audio callback.
///
/// # Errors
/// Returns an error when saved state cannot be restored or requires an
/// unsupported channel layout.
#[doc(hidden)]
pub fn create_plugin(
    name: &str,
    sample_rate: u32,
    params: &DynamicParams,
) -> Result<Box<dyn Plugin>, String> {
    let config = match name {
        "EQ" => crate::wrapper::eq_config_json(|id| params.value(id)),
        "LinearPhaseEQ" => params.linear_phase_eq_config_json()?,
        _ => constructor_config(name, params)?,
    };
    let factory_name = if name == "FletcherMunson" {
        // Both packaged variants expose the LoudnessCompensation parameter
        // schema, including its mode. The legacy JSON compatibility format
        // cannot represent that complete saved state.
        "LoudnessCompensation"
    } else {
        name
    };
    let mut plugin = plugins_bridge::create_plugin(
        factory_name,
        crate::wrapper::plugin_constructor_channels(name),
        sample_rate,
        &config,
    )?;
    if name == "EQ" {
        crate::wrapper::apply_eq_structural(plugin.as_mut(), |id| params.value(id))?;
    } else if name != "LinearPhaseEQ" {
        restore_structural_values(name, params, plugin.as_mut())?;
    }
    let expected_layout = crate::wrapper::plugin_io_channels(name);
    let actual_layout = (plugin.input_channels(), plugin.output_channels());
    let gate_external_layout = name == "Gate" && actual_layout == (4, 2);
    if actual_layout != expected_layout && !gate_external_layout {
        return Err(format!(
            "{name} saved state requires {} input/{} output channels; this wrapper supports {} input/{} output channels",
            actual_layout.0, actual_layout.1, expected_layout.0, expected_layout.1
        ));
    }
    Ok(plugin)
}

fn canonical_id<'a>(name: &str, id: &'a str) -> &'a str {
    match (name, id) {
        ("Crossfeed", "crossfeed_mode") => "mode",
        ("Crossfeed", "crossfeed_preset") => "preset",
        ("BandSplit", "crossover_type") => "type",
        ("SpectralCompressor", "fft_size") => "fft_size_index",
        _ => id,
    }
}

fn scalar_json(value: ParameterValue) -> Value {
    match value {
        ParameterValue::Float(value) => Value::from(value),
        ParameterValue::Int(value) => Value::from(value),
        ParameterValue::Bool(value) => Value::from(value),
        ParameterValue::String(value) => Value::from(value),
    }
}

fn unsupported_legacy_default(
    name: &str,
    id: &str,
    value: &ParameterValue,
) -> Result<bool, String> {
    if name != "Compressor"
        || !matches!(
            id,
            "sidechain_hpf_hz"
                | "sidechain_hpf_order"
                | "detection_mode"
                | "program_dependent_release"
                | "sidechain_external"
        )
    {
        return Ok(false);
    }
    let default = crate::wrapper::get_param_specs(name)
        .iter()
        .find(|spec| spec.engine_key == id)
        .expect("legacy compressor schema entry exists");
    let matches_default = match value {
        ParameterValue::Float(value) => *value == default.default_f64() as f32,
        ParameterValue::Int(value) => *value == default.default_f64() as i32,
        ParameterValue::Bool(value) => *value == default.default_bool(),
        ParameterValue::String(_) => false,
    };
    if !matches_default {
        return Err(format!(
            "{name}.{id} is not implemented by this DSP and cannot restore a nondefault value"
        ));
    }
    Ok(true)
}

fn constructor_config(name: &str, params: &DynamicParams) -> Result<String, String> {
    let mut config = Map::new();
    let mut dynamic_bands = if name == "DynamicEQ" {
        let count = params
            .value("num_bands")
            .and_then(|value| value.as_int())
            .unwrap_or(4);
        vec![
            Map::new();
            usize::try_from(count).map_err(|_| "DynamicEQ band count must be positive")?
        ]
    } else {
        Vec::new()
    };
    for entry in params.sync_entries.iter().filter(|entry| !entry.realtime) {
        let id = canonical_id(name, entry.id.as_str());
        let value = params
            .value(entry.id.as_str())
            .expect("owned parameter exists");
        if unsupported_legacy_default(name, id, &value)? {
            continue;
        }
        if name == "DynamicEQ"
            && let Some((band, field)) = dynamic_band_field(id)
        {
            if let Some(config) = dynamic_bands.get_mut(band) {
                config.insert(field.to_string(), scalar_json(value));
            }
            // The host retains inactive band IDs for future band-count changes.
            continue;
        }
        let key = match (name, id) {
            ("Limiter", "lookahead") => "lookahead_ms",
            _ => id,
        };
        config.insert(key.to_string(), scalar_json(value));
    }
    if name == "DynamicEQ" {
        config.insert(
            "bands".to_string(),
            Value::Array(dynamic_bands.into_iter().map(Value::Object).collect()),
        );
    }
    serde_json::to_string(&config).map_err(|error| format!("{name} configuration: {error}"))
}

fn dynamic_band_field(id: &str) -> Option<(usize, &str)> {
    let (band, field) = id.strip_prefix("band_")?.split_once('_')?;
    Some((band.parse().ok()?, field))
}

fn restore_structural_values(
    name: &str,
    params: &DynamicParams,
    plugin: &mut dyn Plugin,
) -> Result<(), String> {
    for entry in params.sync_entries.iter().filter(|entry| !entry.realtime) {
        let id = canonical_id(name, entry.id.as_str());
        let mut expected = params
            .value(entry.id.as_str())
            .expect("owned parameter exists");
        if unsupported_legacy_default(name, id, &expected)? {
            continue;
        }
        if name == "DynamicEQ"
            && let Some((band, _)) = dynamic_band_field(id)
            && band
                >= params
                    .value("num_bands")
                    .and_then(|value| value.as_int())
                    .unwrap_or(4) as usize
        {
            continue;
        }
        let parameter_id = ParameterId::from(id);
        // Some constructors accept choice indices while their runtime getter
        // exposes canonical strings. Translation happens only during activation.
        if matches!(
            plugin.get_parameter(&parameter_id),
            Some(ParameterValue::String(_))
        ) && let ParameterValue::Int(index) = expected
        {
            let label = crate::wrapper::get_param_specs(name)
                .iter()
                .find_map(|spec| {
                    if spec.engine_key != id {
                        return None;
                    }
                    match spec.param_type {
                        sotf_host::param_specs::ParamType::Choice { labels, .. } => {
                            usize::try_from(index)
                                .ok()
                                .and_then(|index| labels.get(index))
                                .copied()
                        }
                        _ => None,
                    }
                })
                .ok_or_else(|| format!("{name}.{id} has no label for choice index {index}"))?;
            expected = ParameterValue::String(label.to_string());
        }
        if plugin.get_parameter(&parameter_id).as_ref() != Some(&expected) {
            plugin
                .set_parameter(parameter_id.clone(), expected.clone())
                .map_err(|error| format!("Cannot restore {name}.{id}: {error}"))?;
        }
        if name == "Denoiser" && id == "clear_profile" {
            // This is a momentary command: its getter returns false after the
            // setter has executed it, so true is never a retained DSP state.
            expected = ParameterValue::Bool(false);
        }
        if plugin.get_parameter(&parameter_id).as_ref() != Some(&expected) {
            return Err(format!(
                "{name}.{id} did not retain its saved structural value"
            ));
        }
    }
    Ok(())
}
