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
        "Crossover" => crossover_constructor_config(params)?,
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
    let constructor_channels = if name == "AmbisonicsDecoder" {
        let order = params
            .value("order")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "Ambisonics order is missing or invalid".to_string())?;
        let target_layout = params
            .value("target_layout")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "Ambisonics target layout is missing or invalid".to_string())?;
        crate::wrapper::ambisonics_io_channels(order, target_layout)
            .ok_or_else(|| "Ambisonics layout is outside the supported range".to_string())?
            .0
    } else {
        crate::wrapper::plugin_constructor_channels(name)
    };
    let mut plugin =
        plugins_bridge::create_plugin(factory_name, constructor_channels, sample_rate, &config)?;
    if name == "Crossover" {
        // The fixed native schema is converted to a complete typed config
        // before construction. Runtime IDs differ by topology, so replaying
        // its structural choices through legacy scalar setters is incorrect.
    } else if name == "EQ" {
        crate::wrapper::apply_eq_structural(plugin.as_mut(), |id| params.value(id))?;
    } else if name != "LinearPhaseEQ" {
        restore_structural_values(name, params, plugin.as_mut())?;
    }
    if name == "Crossover" {
        let actual = (plugin.input_channels(), plugin.output_channels());
        if actual.0 != crate::wrapper::plugin_constructor_channels(name)
            || !matches!(actual.1, 2 | 4 | 6 | 8)
        {
            return Err(format!(
                "Crossover native settings produced unsupported {} input/{} output channels",
                actual.0, actual.1
            ));
        }
        return Ok(plugin);
    }
    let expected_layout = if name == "AmbisonicsDecoder" {
        let order = params
            .value("order")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "Ambisonics order is missing or invalid".to_string())?;
        let target_layout = params
            .value("target_layout")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "Ambisonics target layout is missing or invalid".to_string())?;
        crate::wrapper::ambisonics_io_channels(order, target_layout)
            .ok_or_else(|| "Ambisonics layout is outside the supported range".to_string())?
    } else if name == "BandSplit" {
        let index = params
            .value("num_bands")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok())
            .filter(|index| *index < 3)
            .ok_or_else(|| "BandSplit band-count choice is missing or invalid".to_string())?;
        (2, (index + 2) * 2)
    } else {
        crate::wrapper::plugin_io_channels(name)
    };
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

fn crossover_constructor_config(params: &DynamicParams) -> Result<String, String> {
    use sotf_plugins::param_specs::crossover::CROSSOVER_TYPES;

    let int_value = |id: &str| match params.value(id) {
        Some(ParameterValue::Int(value)) => Ok(value),
        _ => Err(format!(
            "Crossover integer parameter '{id}' is missing or invalid"
        )),
    };
    let float_value = |id: &str| match params.value(id) {
        Some(ParameterValue::Float(value)) => Ok(value),
        _ => Err(format!(
            "Crossover float parameter '{id}' is missing or invalid"
        )),
    };
    let family_index = usize::try_from(int_value("family")?)
        .map_err(|_| "Crossover family choice is negative".to_string())?;
    let family = CROSSOVER_TYPES
        .get(family_index)
        .ok_or_else(|| format!("Crossover family choice {family_index} is out of range"))?;
    let output = match int_value("mode")? {
        0 => "lowpass",
        1 => "highpass",
        2 => "both",
        index => return Err(format!("Crossover output choice {index} is out of range")),
    };
    let topology = match int_value("topology")? {
        0 => "bands",
        1 => "per_channel",
        index => return Err(format!("Crossover topology choice {index} is out of range")),
    };
    let band_count_index = usize::try_from(int_value("band_count")?)
        .map_err(|_| "Crossover band-count choice is negative".to_string())?;
    if band_count_index > 2 {
        return Err(format!(
            "Crossover band-count choice {band_count_index} is out of range"
        ));
    }
    let fir_taps = usize::try_from(int_value("fir_taps")?)
        .map_err(|_| "Crossover FIR tap count is negative".to_string())?;
    if !(31..=16_385).contains(&fir_taps) {
        return Err(format!(
            "Crossover FIR tap count {fir_taps} is outside 31..=16385"
        ));
    }

    let mut extra_frequencies = Vec::with_capacity(band_count_index);
    if band_count_index >= 1 {
        extra_frequencies.push(float_value("frequency_2")?);
    }
    if band_count_index >= 2 {
        extra_frequencies.push(float_value("frequency_3")?);
    }
    let channel_modes = ["channel_mode_0", "channel_mode_1"]
        .into_iter()
        .map(|id| match int_value(id)? {
            0 => Ok("lowpass"),
            1 => Ok("highpass"),
            2 => Ok("mute"),
            3 => Ok("passthrough"),
            index => Err(format!("Crossover {id} choice {index} is out of range")),
        })
        .collect::<Result<Vec<_>, String>>()?;

    serde_json::to_string(&serde_json::json!({
        "type": family,
        "frequency": float_value("frequency")?,
        "output": output,
        "fir_taps": fir_taps,
        "topology": topology,
        "extra_frequencies": extra_frequencies,
        "channel_frequencies_hz": [
            float_value("channel_frequency_0")?,
            float_value("channel_frequency_1")?,
        ],
        "channel_modes": channel_modes,
    }))
    .map_err(|error| format!("Crossover configuration: {error}"))
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
                config.insert(
                    field.to_string(),
                    dynamic_eq_constructor_value(field, value)?,
                );
            }
            // The host retains inactive band IDs for future band-count changes.
            continue;
        }
        let key = match (name, id) {
            ("Limiter", "lookahead") => "lookahead_ms",
            _ => id,
        };
        let value = if name == "BandSplit" {
            band_split_constructor_value(id, value)?
        } else {
            scalar_json(value)
        };
        config.insert(key.to_string(), value);
    }
    if name == "DynamicEQ" {
        config.insert(
            "bands".to_string(),
            Value::Array(dynamic_bands.into_iter().map(Value::Object).collect()),
        );
    }
    serde_json::to_string(&config).map_err(|error| format!("{name} configuration: {error}"))
}

fn band_split_constructor_value(id: &str, value: ParameterValue) -> Result<Value, String> {
    match (id, value) {
        ("type", ParameterValue::Int(index)) => match index {
            0 => Ok(Value::from("LR24")),
            1 => Ok(Value::from("LR48")),
            _ => Err(format!("BandSplit type choice {index} is outside 0..=1")),
        },
        ("recombination_mode", ParameterValue::Int(index)) => match index {
            0 => Ok(Value::from("legacy_cascade")),
            1 => Ok(Value::from("phase_compensated")),
            _ => Err(format!(
                "BandSplit recombination mode choice {index} is outside 0..=1"
            )),
        },
        ("num_bands", ParameterValue::Int(index)) => match index {
            0..=2 => Ok(Value::from(index + 2)),
            _ => Err(format!(
                "BandSplit band-count choice {index} is outside 0..=2"
            )),
        },
        (_, value) => Ok(scalar_json(value)),
    }
}

fn dynamic_eq_constructor_value(field: &str, value: ParameterValue) -> Result<Value, String> {
    if field != "shape" {
        return Ok(scalar_json(value));
    }

    match value {
        ParameterValue::Int(0) => Ok(Value::from("peak")),
        ParameterValue::Int(1) => Ok(Value::from("low_shelf")),
        ParameterValue::Int(2) => Ok(Value::from("high_shelf")),
        ParameterValue::Int(index) => {
            Err(format!("DynamicEQ shape choice {index} is outside 0..=2"))
        }
        _ => Err("DynamicEQ shape choice must be an integer".to_string()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn band_split_params(overrides: &[(&str, f64)]) -> std::sync::Arc<DynamicParams> {
        let bridge = plugins_bridge::param_bridge::ParamBridge::new(
            crate::wrapper::get_param_specs("BandSplit"),
        );
        let mut infos = (0..bridge.count())
            .filter_map(|index| bridge.info(index))
            .collect::<Vec<_>>();
        for info in &mut infos {
            info.id = crate::wrapper::legacy_external_param_id("BandSplit", &info.id).into_owned();
            if let Some((_, value)) = overrides.iter().find(|(id, _)| *id == info.id) {
                info.default_value = *value;
            }
        }
        DynamicParams::from_infos(&infos)
    }

    #[test]
    fn bandsplit_constructor_translates_choices_and_builds_selected_widths() {
        for (band_index, num_bands) in [(0, 2), (1, 3), (2, 4)] {
            for (type_index, crossover_type) in [(0, "LR24"), (1, "LR48")] {
                for (mode_index, mode) in [(0, "legacy_cascade"), (1, "phase_compensated")] {
                    let params = band_split_params(&[
                        ("crossover_type", type_index as f64),
                        ("recombination_mode", mode_index as f64),
                        ("num_bands", band_index as f64),
                    ]);
                    let config: Value =
                        serde_json::from_str(&constructor_config("BandSplit", &params).unwrap())
                            .unwrap();
                    assert_eq!(config["type"], crossover_type);
                    assert_eq!(config["recombination_mode"], mode);
                    assert_eq!(config["num_bands"], num_bands);

                    let plugin = create_plugin("BandSplit", 48_000, &params).unwrap();
                    assert_eq!(
                        (plugin.input_channels(), plugin.output_channels()),
                        (2, num_bands * 2)
                    );
                    assert_eq!(
                        plugin.get_parameter(&ParameterId::from("num_bands")),
                        Some(ParameterValue::Int(band_index))
                    );
                    assert_eq!(
                        plugin.get_parameter(&ParameterId::from("recombination_mode")),
                        Some(ParameterValue::Int(mode_index))
                    );
                    assert_eq!(
                        plugin.get_parameter(&ParameterId::from("type")),
                        Some(ParameterValue::Int(type_index))
                    );
                }
            }
        }
    }

    fn dynamic_eq_params(
        num_bands: usize,
        shape_indices: &[i32],
        shelf_slopes: &[f64],
    ) -> std::sync::Arc<DynamicParams> {
        assert_eq!(shape_indices.len(), 8);
        assert_eq!(shelf_slopes.len(), 8);
        let config = if num_bands == 4 {
            "{}".to_string()
        } else {
            serde_json::json!({ "num_bands": num_bands }).to_string()
        };
        let mut plugin = plugins_bridge::create_plugin("DynamicEQ", 2, 48_000, &config).unwrap();
        plugin.initialize(48_000).unwrap();
        let mut infos = plugin
            .parameters()
            .iter()
            .filter_map(crate::wrapper::bridged_info_from_parameter)
            .collect::<Vec<_>>();
        for info in &mut infos {
            info.id = crate::wrapper::legacy_external_param_id("DynamicEQ", &info.id).into_owned();
            if info.id == "num_bands" {
                info.default_value = num_bands as f64;
            } else if let Some((band, field)) = dynamic_band_field(&info.id) {
                if field == "shape" {
                    info.default_value = f64::from(shape_indices[band]);
                } else if field == "shelf_slope" {
                    info.default_value = shelf_slopes[band];
                }
            }
        }
        DynamicParams::from_infos(&infos)
    }

    #[test]
    fn dynamiceq_constructor_serializes_all_shapes_and_late_band_slots() {
        let default_params = dynamic_eq_params(4, &[0; 8], &[1.0; 8]);
        let default_config = constructor_config("DynamicEQ", &default_params).unwrap();
        println!("AUD139 DynamicEQ default constructor JSON: {default_config}");
        let parsed_default: Value = serde_json::from_str(&default_config).unwrap();
        assert_eq!(parsed_default["bands"].as_array().unwrap().len(), 4);
        let mut integer_shape_config = parsed_default.clone();
        integer_shape_config["bands"][0]["shape"] = Value::from(0);
        let integer_shape_json = serde_json::to_string(&integer_shape_config).unwrap();
        let integer_shape_error =
            match plugins_bridge::create_plugin("DynamicEQ", 2, 48_000, &integer_shape_json) {
                Ok(_) => panic!("integer DynamicEQ shape unexpectedly parsed"),
                Err(error) => error,
            };
        println!(
            "AUD139 pre-fix DynamicEQ constructor JSON: {integer_shape_json}; parse error: {integer_shape_error}"
        );
        assert!(integer_shape_error.to_string().contains("expected value"));
        let default_plugin = create_plugin("DynamicEQ", 48_000, &default_params).unwrap();
        assert_eq!(default_plugin.output_channels(), 2);

        let shape_indices = [0, 1, 2, 0, 1, 2, 0, 2];
        let shelf_slopes = [0.12, 0.23, 0.34, 0.45, 0.56, 0.67, 0.78, 0.89];
        let params = dynamic_eq_params(8, &shape_indices, &shelf_slopes);
        let config = constructor_config("DynamicEQ", &params).unwrap();
        let parsed: Value = serde_json::from_str(&config).unwrap();
        let bands = parsed["bands"].as_array().unwrap();
        assert_eq!(bands.len(), 8);
        for (index, band) in bands.iter().enumerate() {
            let shape = match shape_indices[index] {
                0 => "peak",
                1 => "low_shelf",
                2 => "high_shelf",
                _ => unreachable!(),
            };
            let stored_slope = shelf_slopes[index] as f32;
            assert_eq!(band["shape"], shape, "band {index} shape encoding");
            assert_eq!(
                band["shelf_slope"].as_f64().unwrap() as f32,
                stored_slope,
                "band {index} slope encoding"
            );
        }

        let restored = create_plugin("DynamicEQ", 48_000, &params).unwrap();
        assert_eq!(
            restored.get_parameter(&ParameterId::from("band_7_shape")),
            Some(ParameterValue::Int(2))
        );
        assert_eq!(
            restored.get_parameter(&ParameterId::from("band_7_shelf_slope")),
            Some(ParameterValue::Float(0.89_f32))
        );
        assert!(dynamic_eq_constructor_value("shape", ParameterValue::Int(3)).is_err());
        assert!(dynamic_eq_constructor_value("shape", ParameterValue::Bool(true)).is_err());
    }
}
