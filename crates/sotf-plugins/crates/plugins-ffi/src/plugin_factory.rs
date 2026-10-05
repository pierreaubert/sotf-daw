// ============================================================================
// Plugin Factory - Delegates to plugins-bridge for all plugin creation
// ============================================================================

use sotf_host::AsyncTimelinePlugin;
use sotf_host::plugin::Plugin;

const LINEAR_PHASE_EQ_DEFAULT_FILTERS: usize = 5;
const LINEAR_PHASE_EQ_MAX_FILTERS: usize = 10;
pub(crate) const MAX_CALLBACK_FRAMES_CONFIG_KEY: &str = "_sotf_max_callback_frames";

pub(crate) fn max_callback_frames_from_config(config_json: &str) -> Result<usize, String> {
    let config: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|error| format!("Invalid plugin configuration JSON: {error}"))?;
    let Some(value) = config.get(MAX_CALLBACK_FRAMES_CONFIG_KEY) else {
        return Ok(super::consts::DEFAULT_MAX_CALLBACK_FRAMES);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            format!(
                "'{MAX_CALLBACK_FRAMES_CONFIG_KEY}' must be a positive integer no greater than {}",
                super::consts::MAX_CALLBACK_FRAMES
            )
        })?;
    if value == 0 || value > super::consts::MAX_CALLBACK_FRAMES {
        return Err(format!(
            "'{MAX_CALLBACK_FRAMES_CONFIG_KEY}' must be between 1 and {}",
            super::consts::MAX_CALLBACK_FRAMES
        ));
    }
    Ok(value)
}

fn plugin_config_without_ffi_metadata(config_json: &str) -> Result<String, String> {
    let mut config: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|error| format!("Invalid plugin configuration JSON: {error}"))?;
    if let Some(config) = config.as_object_mut() {
        config.remove(MAX_CALLBACK_FRAMES_CONFIG_KEY);
    }
    serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize plugin configuration JSON: {error}"))
}

fn state_integer(
    state: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<usize>, String> {
    state
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    format!("LinearPhaseEQ state '{key}' must be a non-negative integer")
                })
        })
        .transpose()
}

fn state_number(
    state: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, String> {
    state
        .get(key)
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("LinearPhaseEQ state '{key}' must be a finite number"))
        })
        .transpose()
}

fn state_bool(
    state: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    state
        .get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("LinearPhaseEQ state '{key}' must be a boolean"))
        })
        .transpose()
}

fn default_linear_phase_eq_band() -> serde_json::Value {
    serde_json::json!({
        "filter_type": "Peak",
        "frequency": 1000.0,
        "q": 1.0,
        "gain_db": 0.0,
        "active": true,
    })
}

/// Merge the flat parameter state used by plugin presets into the structured
/// constructor configuration required by LinearPhaseEQ.
pub(crate) fn merge_linear_phase_eq_state_into_config(
    config_json: &str,
    state_bytes: &[u8],
) -> Result<String, String> {
    let mut config: serde_json::Value =
        if config_json.trim().is_empty() || matches!(config_json.trim(), "null" | "{}") {
            serde_json::json!({})
        } else {
            serde_json::from_str(config_json)
                .map_err(|error| format!("Failed to parse saved LinearPhaseEQ config: {error}"))?
        };
    let config = config
        .as_object_mut()
        .ok_or_else(|| "LinearPhaseEQ constructor config must be a JSON object".to_string())?;
    let state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state_bytes)
        .map_err(|error| format!("Failed to parse LinearPhaseEQ state: {error}"))?;

    if let Some(index) = state_integer(&state, "fir_length")? {
        if index > 3 {
            return Err(format!(
                "LinearPhaseEQ fir_length index must be within [0, 3], got {index}"
            ));
        }
        config.insert("fir_length_index".to_string(), index.into());
    }
    if let Some(index) = state_integer(&state, "phase_mode")? {
        if index > 1 {
            return Err(format!(
                "LinearPhaseEQ phase_mode index must be within [0, 1], got {index}"
            ));
        }
        config.remove("phase_mode");
        config.insert("phase_mode_index".to_string(), index.into());
    }
    if let Some(auto_gain) = state_bool(&state, "auto_gain")? {
        config.insert("auto_gain".to_string(), auto_gain.into());
    }
    if let Some(mix) = state_number(&state, "mix")? {
        if !(0.0..=1.0).contains(&mix) {
            return Err(format!(
                "LinearPhaseEQ mix must be within [0, 1], got {mix}"
            ));
        }
        config.insert("mix".to_string(), mix.into());
    }

    let num_filters = state_integer(&state, "num_filters")?
        .or_else(|| {
            config
                .get("num_filters")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize)
        })
        .unwrap_or(LINEAR_PHASE_EQ_DEFAULT_FILTERS);
    if !(1..=LINEAR_PHASE_EQ_MAX_FILTERS).contains(&num_filters) {
        return Err(format!(
            "LinearPhaseEQ num_filters must be within [1, {LINEAR_PHASE_EQ_MAX_FILTERS}], got {num_filters}"
        ));
    }
    config.insert("num_filters".to_string(), num_filters.into());

    let mut filters = match config.remove("filters") {
        Some(serde_json::Value::Array(filters)) => filters,
        Some(_) => return Err("LinearPhaseEQ config 'filters' must be an array".to_string()),
        None => Vec::new(),
    };
    filters.resize_with(num_filters, default_linear_phase_eq_band);
    filters.truncate(num_filters);

    const FILTER_TYPES: [&str; 5] = ["Peak", "Lowshelf", "Highshelf", "Lowpass", "Highpass"];
    for (key, value) in &state {
        let Some(rest) = key.strip_prefix("band_") else {
            continue;
        };
        let Some((index, field)) = rest.split_once('_') else {
            continue;
        };
        let Ok(index) = index.parse::<usize>() else {
            continue;
        };
        if !matches!(
            field,
            "type" | "freq" | "q" | "gain" | "active" | "placement"
        ) {
            continue;
        }
        if index >= num_filters {
            return Err(format!(
                "LinearPhaseEQ state '{key}' targets band {index}, but num_filters is {num_filters}"
            ));
        }
        let band = filters[index]
            .as_object_mut()
            .ok_or_else(|| format!("LinearPhaseEQ config filter {index} must be an object"))?;
        match field {
            "type" => {
                let type_index = value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|index| *index < FILTER_TYPES.len())
                    .ok_or_else(|| {
                        format!("LinearPhaseEQ state '{key}' must be an integer within [0, 4]")
                    })?;
                band.insert("filter_type".to_string(), FILTER_TYPES[type_index].into());
            }
            "active" => {
                let active = value
                    .as_bool()
                    .ok_or_else(|| format!("LinearPhaseEQ state '{key}' must be a boolean"))?;
                band.insert("active".to_string(), active.into());
            }
            "placement" => {
                const PLACEMENT_LABELS: [&str; 5] = ["stereo", "left", "right", "mid", "side"];
                let placement_index = value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|index| *index <= PLACEMENT_LABELS.len())
                    .ok_or_else(|| {
                        format!("LinearPhaseEQ state '{key}' must be a placement index in 0..=5")
                    })?;
                // LinearPhaseEQ placement: 0 = legacy (absence of the key),
                // 1 = stereo .. 5 = side. Same 6-choice contract as EQ, unlike
                // DynamicEQ, where 0 is explicit Stereo with no Legacy.
                if placement_index == 0 {
                    band.remove("placement");
                } else {
                    band.insert(
                        "placement".to_string(),
                        PLACEMENT_LABELS[placement_index - 1].into(),
                    );
                }
            }
            numeric_field => {
                let number = value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| {
                        format!("LinearPhaseEQ state '{key}' must be a finite number")
                    })?;
                let config_field = match numeric_field {
                    "freq" => "frequency",
                    "gain" => "gain_db",
                    "q" => "q",
                    _ => unreachable!(),
                };
                band.insert(config_field.to_string(), number.into());
            }
        }
    }
    config.insert("filters".to_string(), filters.into());

    // Pair lists are structural saved state. When the incoming state carries
    // them they replace the config pairs; otherwise the construction config
    // pairs survive. Shape-checked here, geometry-checked by the plugin
    // constructor (disjoint, in-range, nonzero pair count).
    if let Some(pairs) = state.get("stereo_pairs") {
        match pairs {
            serde_json::Value::Null => {
                config.remove("stereo_pairs");
            }
            serde_json::Value::Array(pairs) => {
                for (pair_index, pair) in pairs.iter().enumerate() {
                    let valid = pair.as_array().is_some_and(|pair| {
                        pair.len() == 2
                            && pair.iter().all(|channel| {
                                channel
                                    .as_u64()
                                    .is_some_and(|channel| usize::try_from(channel).is_ok())
                            })
                    });
                    if !valid {
                        return Err(format!(
                            "LinearPhaseEQ state 'stereo_pairs[{pair_index}]' must be a [left, right] channel pair"
                        ));
                    }
                }
                config.insert(
                    "stereo_pairs".to_string(),
                    serde_json::Value::Array(pairs.clone()),
                );
            }
            _ => {
                return Err(
                    "LinearPhaseEQ state 'stereo_pairs' must be an array of [left, right] pairs or null"
                        .to_string(),
                );
            }
        }
    }

    serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize rebuilt LinearPhaseEQ config: {error}"))
}

/// Merge DynamicEQ's flat saved parameter map into its structured constructor
/// config. The band array always retains up to all eight slots so shrinking
/// `num_bands` does not discard dormant settings.
pub(crate) fn merge_dynamic_eq_state_into_config(
    config_json: &str,
    state_bytes: &[u8],
) -> Result<String, String> {
    const MAX_BANDS: usize = 8;
    const GLOBAL_KEYS: [&str; 8] = [
        "num_bands",
        "threshold",
        "ratio",
        "attack",
        "release",
        "knee",
        "link_channels",
        "mix",
    ];
    const BAND_FIELDS: [&str; 10] = [
        "frequency",
        "q",
        "gain",
        "band_threshold",
        "band_ratio",
        "active",
        "solo",
        "shape",
        "shelf_slope",
        "placement",
    ];

    let mut config: serde_json::Value =
        if config_json.trim().is_empty() || matches!(config_json.trim(), "null" | "{}") {
            serde_json::json!({})
        } else {
            serde_json::from_str(config_json)
                .map_err(|error| format!("Failed to parse saved DynamicEQ config: {error}"))?
        };
    let config = config
        .as_object_mut()
        .ok_or_else(|| "DynamicEQ constructor config must be a JSON object".to_string())?;
    let state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state_bytes)
        .map_err(|error| format!("Failed to parse DynamicEQ state: {error}"))?;

    for key in state.keys() {
        if GLOBAL_KEYS.contains(&key.as_str()) || key == "stereo_pairs" {
            continue;
        }
        let rest = key
            .strip_prefix("band_")
            .ok_or_else(|| format!("DynamicEQ state contains unsupported parameter '{key}'"))?;
        let (index_text, field) = rest
            .split_once('_')
            .ok_or_else(|| format!("DynamicEQ state parameter '{key}' is malformed"))?;
        let index = index_text
            .parse::<usize>()
            .ok()
            .filter(|index| *index < MAX_BANDS)
            .ok_or_else(|| {
                format!("DynamicEQ state parameter '{key}' has an invalid band index")
            })?;
        if index_text != index.to_string() {
            return Err(format!(
                "DynamicEQ state parameter '{key}' has a noncanonical band index"
            ));
        }
        if !BAND_FIELDS.contains(&field) {
            return Err(format!(
                "DynamicEQ state parameter '{key}' has an unsupported band field"
            ));
        }

        let value = &state[key];
        match field {
            "active" | "solo" => {
                if !value.is_boolean() {
                    return Err(format!("DynamicEQ state '{key}' must be a boolean"));
                }
            }
            "shape" => {
                if !value.as_i64().is_some_and(|index| (0..=3).contains(&index)) {
                    return Err(format!(
                        "DynamicEQ state '{key}' must be a choice index in 0..=3"
                    ));
                }
            }
            "placement" => {
                if !value.as_i64().is_some_and(|index| (0..=4).contains(&index)) {
                    return Err(format!(
                        "DynamicEQ state '{key}' must be a placement index in 0..=4"
                    ));
                }
            }
            _ => {
                if !value.as_f64().is_some_and(f64::is_finite) {
                    return Err(format!("DynamicEQ state '{key}' must be a finite number"));
                }
            }
        }
    }

    if let Some(value) = state.get("num_bands") {
        let num_bands = value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=MAX_BANDS).contains(value))
            .ok_or_else(|| "DynamicEQ state 'num_bands' must be in 1..=8".to_string())?;
        config.insert("num_bands".to_string(), num_bands.into());
    }
    for (state_key, config_key) in [
        ("threshold", "threshold"),
        ("ratio", "ratio"),
        ("attack", "attack_ms"),
        ("release", "release_ms"),
        ("knee", "knee"),
        ("link_channels", "link_channels"),
        ("mix", "mix"),
    ] {
        if let Some(value) = state.get(state_key) {
            let valid = if state_key == "link_channels" {
                value.is_boolean()
            } else {
                value.as_f64().is_some_and(f64::is_finite)
            };
            if !valid {
                return Err(format!(
                    "DynamicEQ state '{state_key}' has an invalid type/value"
                ));
            }
            config.insert(config_key.to_string(), value.clone());
        }
    }

    let mut bands = match config.remove("bands") {
        Some(serde_json::Value::Array(bands)) if bands.len() <= MAX_BANDS => bands,
        Some(serde_json::Value::Array(_)) => {
            return Err("DynamicEQ config cannot contain more than eight bands".into());
        }
        Some(_) => return Err("DynamicEQ config 'bands' must be an array".into()),
        None => Vec::new(),
    };
    bands.resize_with(MAX_BANDS, || serde_json::json!({}));

    for (band_index, band_value) in bands.iter_mut().enumerate().take(MAX_BANDS) {
        let band = band_value
            .as_object_mut()
            .ok_or_else(|| format!("DynamicEQ config band {band_index} must be an object"))?;
        for field in BAND_FIELDS {
            let key = format!("band_{band_index}_{field}");
            let Some(value) = state.get(&key) else {
                continue;
            };
            let config_value = if field == "shape" {
                // Shape indices match DSP `DynEqShape::CHOICE_LABELS` order:
                // 0=Peak, 1=Low Shelf, 2=High Shelf, 3=Tilt.
                match value.as_u64() {
                    Some(0) => serde_json::Value::String("peak".to_string()),
                    Some(1) => serde_json::Value::String("low_shelf".to_string()),
                    Some(2) => serde_json::Value::String("high_shelf".to_string()),
                    Some(3) => serde_json::Value::String("tilt".to_string()),
                    _ => {
                        return Err(format!(
                            "DynamicEQ state '{key}' has an invalid choice index"
                        ));
                    }
                }
            } else if field == "placement" {
                // DynamicEQ placement: 0 = explicit Stereo .. 4 = Side (5
                // choices, no Legacy). Unlike EQ/Linear, 0 does not remove the
                // key; it writes "stereo".
                match value.as_u64() {
                    Some(0) => serde_json::Value::String("stereo".to_string()),
                    Some(1) => serde_json::Value::String("left".to_string()),
                    Some(2) => serde_json::Value::String("right".to_string()),
                    Some(3) => serde_json::Value::String("mid".to_string()),
                    Some(4) => serde_json::Value::String("side".to_string()),
                    _ => {
                        return Err(format!(
                            "DynamicEQ state '{key}' has an invalid placement index"
                        ));
                    }
                }
            } else {
                value.clone()
            };
            band.insert(field.to_string(), config_value);
        }
    }
    config.insert("bands".to_string(), bands.into());

    // Pair lists are structural saved state, not realtime parameters. When the
    // incoming state carries them they replace the config pairs; otherwise the
    // construction config pairs survive. Shape-checked here, geometry-checked
    // by the plugin constructor (disjoint, in-range, pair-count limits).
    if let Some(pairs) = state.get("stereo_pairs") {
        match pairs {
            serde_json::Value::Null => {
                config.remove("stereo_pairs");
            }
            serde_json::Value::Array(pairs) => {
                for (pair_index, pair) in pairs.iter().enumerate() {
                    let valid = pair.as_array().is_some_and(|pair| {
                        pair.len() == 2
                            && pair.iter().all(|channel| {
                                channel
                                    .as_u64()
                                    .is_some_and(|channel| usize::try_from(channel).is_ok())
                            })
                    });
                    if !valid {
                        return Err(format!(
                            "DynamicEQ state 'stereo_pairs[{pair_index}]' must be a [left, right] channel pair"
                        ));
                    }
                }
                config.insert(
                    "stereo_pairs".to_string(),
                    serde_json::Value::Array(pairs.clone()),
                );
            }
            _ => {
                return Err(
                    "DynamicEQ state 'stereo_pairs' must be an array of [left, right] pairs or null"
                        .to_string(),
                );
            }
        }
    }

    serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize rebuilt DynamicEQ config: {error}"))
}

/// Apply the legacy Peak/Stereo defaults that were implicit before DynamicEQ
/// shelf and routing controls existed. This is only for full preset
/// documents; partial state loads continue to merge omitted values from
/// the live plugin.
pub(crate) fn add_dynamic_eq_preset_shelf_defaults(state_bytes: &[u8]) -> Result<Vec<u8>, String> {
    const MAX_BANDS: usize = 8;

    let mut state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state_bytes)
        .map_err(|error| format!("Failed to parse DynamicEQ preset state: {error}"))?;
    let num_bands = state
        .get("num_bands")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| (1..=MAX_BANDS).contains(value))
        .ok_or_else(|| "DynamicEQ preset 'num_bands' must be in 1..=8".to_string())?;

    for band_index in 0..num_bands {
        state
            .entry(format!("band_{band_index}_shape"))
            .or_insert_with(|| serde_json::Value::from(0));
        state
            .entry(format!("band_{band_index}_shelf_slope"))
            .or_insert_with(|| serde_json::Value::from(1.0));
        state
            .entry(format!("band_{band_index}_placement"))
            .or_insert_with(|| serde_json::Value::from(0));
    }

    serde_json::to_vec(&serde_json::Value::Object(state))
        .map_err(|error| format!("Failed to serialize DynamicEQ preset defaults: {error}"))
}

/// Merge an EQ flat saved map into its structured constructor config.
///
/// Scalar band values, placements, and runtime globals (including the live
/// `auto_gain_enabled` switch, which is distinct from the constructor's
/// `auto_gain` measurement struct) stay in the flat map and are replayed
/// onto the rebuilt plugin through its own setters (which own the compacted
/// biquad index mapping, placement validation, and value ranges). Only
/// constructor-owned keys are folded into the config here: `stereo_pairs`
/// and the optional full-structural `filters` / `channel_filters` arrays
/// carried by new full presets. Unknown top-level keys are ignored so legacy
/// raw partial states keep their old semantics; malformed `band_` / `filter_`
/// addresses fail closed instead of silently landing on the wrong band.
pub(crate) fn merge_eq_state_into_config(
    config_json: &str,
    state_bytes: &[u8],
) -> Result<String, String> {
    const MAX_FILTERS: usize = 20;
    const GLOBAL_KEYS: [&str; 5] = [
        "max_filters",
        "tdf2",
        "topology",
        "auto_gain_enabled",
        "oversampling",
    ];
    const BAND_FIELDS: [&str; 5] = ["freq", "q", "gain", "filter_type", "order"];

    let mut config: serde_json::Value =
        if config_json.trim().is_empty() || matches!(config_json.trim(), "null" | "{}") {
            serde_json::json!({})
        } else {
            serde_json::from_str(config_json)
                .map_err(|error| format!("Failed to parse saved EQ config: {error}"))?
        };
    let config = config
        .as_object_mut()
        .ok_or_else(|| "EQ constructor config must be a JSON object".to_string())?;
    let state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state_bytes)
        .map_err(|error| format!("Failed to parse EQ state: {error}"))?;

    for key in state.keys() {
        if GLOBAL_KEYS.contains(&key.as_str())
            || matches!(key.as_str(), "stereo_pairs" | "filters" | "channel_filters")
        {
            continue;
        }
        if let Some(rest) = key.strip_prefix("band_") {
            let (index_text, field) = rest
                .split_once('_')
                .ok_or_else(|| format!("EQ state parameter '{key}' is malformed"))?;
            let index = index_text
                .parse::<usize>()
                .ok()
                .filter(|index| *index < MAX_FILTERS)
                .ok_or_else(|| format!("EQ state parameter '{key}' has an invalid band index"))?;
            if index_text != index.to_string() {
                return Err(format!(
                    "EQ state parameter '{key}' has a noncanonical band index"
                ));
            }
            if !BAND_FIELDS.contains(&field) {
                return Err(format!(
                    "EQ state parameter '{key}' has an unsupported band field"
                ));
            }
            continue;
        }
        if let Some(rest) = key.strip_prefix("filter_") {
            let index_text = rest.strip_suffix("_placement").ok_or_else(|| {
                format!("EQ state parameter '{key}' has an unsupported filter field")
            })?;
            let index = index_text
                .parse::<usize>()
                .ok()
                .filter(|index| *index < MAX_FILTERS)
                .ok_or_else(|| format!("EQ state parameter '{key}' has an invalid filter index"))?;
            if index_text != index.to_string() {
                return Err(format!(
                    "EQ state parameter '{key}' has a noncanonical filter index"
                ));
            }
            state[key]
                .as_i64()
                .filter(|placement| (0..=5).contains(placement))
                .ok_or_else(|| format!("EQ state '{key}' must be a placement index in 0..=5"))?;
            continue;
        }
        // Legacy tolerance: bridge state loads ignore unknown keys, so the
        // merge must not turn a foreign key into a hard failure.
    }

    // Constructor-owned keys. Band order and advanced/Kautz entries live in
    // `filters` and are preserved verbatim unless a full preset replaces them.
    if let Some(pairs) = state.get("stereo_pairs") {
        match pairs {
            serde_json::Value::Null => {
                config.remove("stereo_pairs");
            }
            serde_json::Value::Array(pairs) => {
                for (pair_index, pair) in pairs.iter().enumerate() {
                    let valid = pair.as_array().is_some_and(|pair| {
                        pair.len() == 2
                            && pair.iter().all(|channel| {
                                channel
                                    .as_u64()
                                    .is_some_and(|channel| usize::try_from(channel).is_ok())
                            })
                    });
                    if !valid {
                        return Err(format!(
                            "EQ state 'stereo_pairs[{pair_index}]' must be a [left, right] channel pair"
                        ));
                    }
                }
                config.insert(
                    "stereo_pairs".to_string(),
                    serde_json::Value::Array(pairs.clone()),
                );
            }
            _ => {
                return Err(
                    "EQ state 'stereo_pairs' must be an array of [left, right] pairs or null"
                        .to_string(),
                );
            }
        }
    }
    // Full structural presets replace the whole filter vector (band order,
    // per-filter placement, topology, lambda, Kautz sections). Element-level
    // validation belongs to the plugin constructor, which rejects unknown
    // filter types, out-of-range orders, and bad geometry transactionally.
    if let Some(filters) = state.get("filters") {
        match filters {
            serde_json::Value::Array(filters) => {
                if filters.len() > MAX_FILTERS {
                    return Err(format!(
                        "EQ state 'filters' holds {} entries, more than the {MAX_FILTERS} maximum",
                        filters.len()
                    ));
                }
                config.insert(
                    "filters".to_string(),
                    serde_json::Value::Array(filters.clone()),
                );
            }
            _ => return Err("EQ state 'filters' must be an array".to_string()),
        }
    }
    if let Some(channel_filters) = state.get("channel_filters") {
        match channel_filters {
            serde_json::Value::Null => {
                config.remove("channel_filters");
            }
            serde_json::Value::Array(channel_filters) => {
                config.insert(
                    "channel_filters".to_string(),
                    serde_json::Value::Array(channel_filters.clone()),
                );
            }
            _ => {
                return Err("EQ state 'channel_filters' must be an array or null".to_string());
            }
        }
    }

    serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize rebuilt EQ config: {error}"))
}

/// Merge a DeEsser flat saved map into its structured constructor config.
///
/// State keys use parameter IDs (`attack`, `release`); config keys use
/// `DeEsserPluginParams` field names (`attack_ms`, `release_ms`). The two
/// choice states accept both the label strings the plugin snapshot emits
/// (`"Split-Band"`) and integer indices, normalizing to canonical labels.
/// Structural values that the live setters reject (detection band, mode,
/// lookahead, split topology, sidechain route) are folded into the config so
/// full saved reloads rebuild instead of failing; layout-changing sidechain
/// flips still fail at bus validation with the live handle preserved.
pub(crate) fn merge_de_esser_state_into_config(
    config_json: &str,
    state_bytes: &[u8],
) -> Result<String, String> {
    use sotf_plugins::param_specs::de_esser::{MODES, SPLIT_TOPOLOGIES};

    const KNOWN_KEYS: [&str; 14] = [
        "frequency",
        "q",
        "threshold",
        "ratio",
        "attack",
        "release",
        "mode",
        "mix",
        "range_db",
        "stereo_link",
        "lookahead_ms",
        "split_topology",
        "ms_mode",
        "sidechain_external",
    ];

    let mut config: serde_json::Value =
        if config_json.trim().is_empty() || matches!(config_json.trim(), "null" | "{}") {
            serde_json::json!({})
        } else {
            serde_json::from_str(config_json)
                .map_err(|error| format!("Failed to parse saved DeEsser config: {error}"))?
        };
    let config = config
        .as_object_mut()
        .ok_or_else(|| "DeEsser constructor config must be a JSON object".to_string())?;
    let state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state_bytes)
        .map_err(|error| format!("Failed to parse DeEsser state: {error}"))?;

    for key in state.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "DeEsser state contains unsupported parameter '{key}'"
            ));
        }
    }

    for (state_key, config_key) in [
        ("frequency", "frequency"),
        ("q", "q"),
        ("threshold", "threshold"),
        ("ratio", "ratio"),
        ("attack", "attack_ms"),
        ("release", "release_ms"),
        ("mix", "mix"),
        ("range_db", "range_db"),
        ("stereo_link", "stereo_link"),
        ("lookahead_ms", "lookahead_ms"),
    ] {
        if let Some(value) = state.get(state_key) {
            if !value.as_f64().is_some_and(f64::is_finite) {
                return Err(format!(
                    "DeEsser state '{state_key}' must be a finite number"
                ));
            }
            config.insert(config_key.to_string(), value.clone());
        }
    }
    for key in ["ms_mode", "sidechain_external"] {
        if let Some(value) = state.get(key) {
            if !value.is_boolean() {
                return Err(format!("DeEsser state '{key}' must be a boolean"));
            }
            config.insert(key.to_string(), value.clone());
        }
    }
    // Choice states arrive either as the label strings the snapshot emits
    // or as integer indices from hand-authored documents; both normalize
    // to the canonical label the constructor deserializer expects.
    let choice_label = |key: &str, labels: &[&str]| -> Result<Option<String>, String> {
        let Some(value) = state.get(key) else {
            return Ok(None);
        };
        if let Some(index) = value
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < labels.len())
        {
            return Ok(Some(labels[index].to_string()));
        }
        if let Some(label) = value.as_str()
            && let Some(canonical) = labels
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(label))
        {
            return Ok(Some((*canonical).to_string()));
        }
        Err(format!(
            "DeEsser state '{key}' must be one of {} or a choice index in 0..={}",
            labels.join("/"),
            labels.len() - 1
        ))
    };
    if let Some(mode) = choice_label("mode", MODES)? {
        config.insert("mode".to_string(), mode.into());
    }
    if let Some(topology) = choice_label("split_topology", SPLIT_TOPOLOGIES)? {
        config.insert("split_topology".to_string(), topology.into());
    }

    serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize rebuilt DeEsser config: {error}"))
}

/// Merge an Ambisonics flat saved map into its structured constructor config.
///
/// Choice indices map back to layout/algorithm labels; the custom geometry
/// object rides in the construction config (it is not a flat parameter) and
/// is preserved across structural reloads. Selecting `custom` without
/// geometry fails closed and names the limitation instead of silently
/// building a named decoder.
pub(crate) fn merge_ambisonics_state_into_config(
    config_json: &str,
    state_bytes: &[u8],
) -> Result<String, String> {
    use sotf_plugin_ambisonics::custom_layout::CUSTOM_LAYOUT_KEY;
    use sotf_plugin_ambisonics::params::{ALGORITHMS, TARGET_LAYOUTS};

    const KNOWN_KEYS: [&str; 5] = [
        "order",
        "target_layout",
        "max_re_weighting",
        "dual_band",
        "algorithm",
    ];

    let mut config: serde_json::Value =
        if config_json.trim().is_empty() || matches!(config_json.trim(), "null" | "{}") {
            serde_json::json!({})
        } else {
            serde_json::from_str(config_json)
                .map_err(|error| format!("Failed to parse saved Ambisonics config: {error}"))?
        };
    let config = config
        .as_object_mut()
        .ok_or_else(|| "Ambisonics constructor config must be a JSON object".to_string())?;
    let state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state_bytes)
        .map_err(|error| format!("Failed to parse Ambisonics state: {error}"))?;

    for key in state.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "Ambisonics state contains unsupported parameter '{key}'"
            ));
        }
    }

    if let Some(value) = state.get("order") {
        let order = value
            .as_i64()
            .filter(|order| (1..=7).contains(order))
            .ok_or_else(|| "Ambisonics state 'order' must be an integer in 1..=7".to_string())?;
        config.insert("order".to_string(), order.into());
    }
    if let Some(value) = state.get("target_layout") {
        let index = value
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < TARGET_LAYOUTS.len())
            .ok_or_else(|| {
                format!(
                    "Ambisonics state 'target_layout' must be a choice index in 0..={}",
                    TARGET_LAYOUTS.len() - 1
                )
            })?;
        if TARGET_LAYOUTS[index] == CUSTOM_LAYOUT_KEY {
            if config.get("custom_layout").is_none() {
                return Err(
                    "Ambisonics state selects the custom target without custom geometry: the saved config has no \"custom_layout\" object"
                        .to_string(),
                );
            }
            config.insert("target_layout".to_string(), CUSTOM_LAYOUT_KEY.into());
        } else {
            config.insert("target_layout".to_string(), TARGET_LAYOUTS[index].into());
            config.remove("custom_layout");
        }
    }
    for key in ["max_re_weighting", "dual_band"] {
        if let Some(value) = state.get(key) {
            if !value.is_boolean() {
                return Err(format!("Ambisonics state '{key}' must be a boolean"));
            }
            config.insert(key.to_string(), value.clone());
        }
    }
    if let Some(value) = state.get("algorithm") {
        let index = value
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < ALGORITHMS.len())
            .ok_or_else(|| {
                "Ambisonics state 'algorithm' must be a choice index in 0..=1".to_string()
            })?;
        config.insert("algorithm".to_string(), ALGORITHMS[index].into());
    }

    serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize rebuilt Ambisonics config: {error}"))
}

fn normalized_type_is(plugin_type: &str, expected: &str) -> bool {
    plugin_type
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase())
        .eq(expected.bytes())
}

pub(crate) fn canonical_direct_plugin_type(plugin_type: &str) -> &str {
    if normalized_type_is(plugin_type, "linearphaseeq") {
        "LinearPhaseEQ"
    } else if normalized_type_is(plugin_type, "resampler") {
        "Resampler"
    } else {
        plugin_type
    }
}

/// Check whether a saved preset's plugin type is valid for the import target.
///
/// Factory aliases match in either direction. The legacy `FletcherMunson`
/// document migrates only into a `LoudnessCompensation` target; that directed
/// compatibility rule does not imply the reverse migration.
pub(crate) fn preset_import_type_matches_target(
    target_plugin_type: &str,
    saved_plugin_type: &str,
) -> bool {
    fn family(plugin_type: &str) -> &str {
        match canonical_direct_plugin_type(plugin_type) {
            "EQ" | "eq" => "EQ",
            "Compressor" | "compressor" => "Compressor",
            "Limiter" | "limiter" => "Limiter",
            "Gate" | "gate" => "Gate",
            "Gain" | "gain" => "Gain",
            "Delay" | "delay" => "Delay",
            "Expander" | "expander" => "Expander",
            "DeEsser" | "de_esser" => "DeEsser",
            "DynamicEQ" | "dynamic_eq" => "DynamicEQ",
            "Crossfeed" | "crossfeed" => "Crossfeed",
            "MultibandCompressor" | "multiband_compressor" => "MultibandCompressor",
            "MultibandExpander" | "multiband_expander" => "MultibandExpander",
            "Convolution" | "convolution" => "Convolution",
            "FletcherMunson" | "fletcher_munson" => "FletcherMunson",
            "LoudnessCompensation" | "loudness_compensation" => "LoudnessCompensation",
            "ChannelMuteSolo" | "channel_mute_solo" => "ChannelMuteSolo",
            "Downmix" | "downmix" => "Downmix",
            "Upmixer" | "upmixer" => "Upmixer",
            "AAE" | "aae" | "active_acoustic_enhancement" => "AAE",
            "XTC" | "xtc" => "XTC",
            "Binaural" | "binaural" => "Binaural",
            "Matrix" | "matrix" => "Matrix",
            "MonoToStereo" | "mono_to_stereo" => "MonoToStereo",
            "PND" | "pnd" => "PND",
            "Denoiser" | "denoiser" => "Denoiser",
            "SpeechDenoiser" | "speech_denoiser" | "RNNoise" | "rnnoise" => "SpeechDenoiser",
            "HissReducer" | "hiss_reducer" | "Hiss" | "hiss" => "HissReducer",
            "Declick" | "declick" | "TransientRepair" | "transient_repair" => "Declick",
            "ABCompare" | "ab_compare" => "ABCompare",
            "Crossover" | "crossover" => "Crossover",
            "StereoImager" | "stereo_imager" => "StereoImager",
            "TransientShaper" | "transient_shaper" => "TransientShaper",
            "Saturation" | "saturation" => "Saturation",
            "AnalogCompressor" | "analog_compressor" => "AnalogCompressor",
            "AnalogEQ" | "analog_eq" => "AnalogEQ",
            "AnalogLimiter" | "analog_limiter" => "AnalogLimiter",
            "LinearPhaseEQ" => "LinearPhaseEQ",
            "SpectralCompressor" | "spectral_compressor" => "SpectralCompressor",
            "Dither" | "dither" => "Dither",
            "AmbisonicsDecoder" | "ambisonics_decoder" => "AmbisonicsDecoder",
            "BandSplit" | "band_split" => "BandSplit",
            "BandMerge" | "band_merge" => "BandMerge",
            "AEC" | "aec" => "AEC",
            "Beamformer" | "beamformer" => "Beamformer",
            "SpectrumAnalyzer" | "spectrum_analyzer" => "SpectrumAnalyzer",
            "LoudnessMonitor" | "loudness_monitor" => "LoudnessMonitor",
            "Resampler" => "Resampler",
            other => other,
        }
    }

    let target_family = family(target_plugin_type);
    let saved_family = family(saved_plugin_type);

    target_family == saved_family
        // `FletcherMunson` is a supported legacy spelling of an older preset
        // family. The engine's backward-compatible route converts that saved
        // plugin to `LoudnessCompensation`, so importing old presets into the
        // replacement is valid. Keep this migration directional: a preset
        // exported by the replacement is not an old FletcherMunson document.
        || (target_family == "LoudnessCompensation" && saved_family == "FletcherMunson")
}

/// Create a plugin instance from plugin type and JSON config string.
///
/// Delegates to `plugins_bridge::create_plugin()` which supports all plugin types.
#[cfg(test)]
pub fn create_plugin(
    plugin_type: &str,
    config_json: &str,
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin_with_max_callback(
        plugin_type,
        config_json,
        input_channels,
        output_channels,
        sample_rate,
        super::consts::DEFAULT_MAX_CALLBACK_FRAMES,
    )
}

pub(crate) fn create_plugin_with_max_callback(
    plugin_type: &str,
    config_json: &str,
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
    max_callback_frames: usize,
) -> Result<Box<dyn Plugin>, String> {
    let plugin = create_unprepared_plugin(
        plugin_type,
        config_json,
        input_channels,
        output_channels,
        sample_rate,
    )?;
    if canonical_direct_plugin_type(plugin_type) == "LinearPhaseEQ" {
        Ok(Box::new(AsyncTimelinePlugin::new(
            plugin,
            f64::from(sample_rate),
            max_callback_frames,
        )?))
    } else {
        plugins_bridge::prepare_standalone_plugin(plugin, max_callback_frames)
    }
}

/// Report whether a DeEsser construction config requests the external key bus.
///
/// Unparseable configs return false so the bridge parse error (not a width
/// guess) surfaces to the caller.
fn de_esser_config_is_external_sidechain(plugin_config: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(plugin_config)
        .ok()
        .and_then(|config| config.get("sidechain_external").cloned())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Report whether an Ambisonics construction config selects the custom target.
fn ambisonics_config_selects_custom(plugin_config: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(plugin_config)
        .ok()
        .and_then(|config| config.get("target_layout").cloned())
        .and_then(|value| value.as_str().map(str::to_owned))
        .is_some_and(|layout| layout == sotf_plugin_ambisonics::custom_layout::CUSTOM_LAYOUT_KEY)
}

/// Construct a custom-geometry Ambisonics decoder via the direct DSP route.
///
/// Bare `"custom"` without a `custom_layout` object fails closed with the
/// limitation named; geometry itself is validated by the plugin constructor
/// (bounded speaker count, finite angles, single LFE, well-conditioned
/// solve), never defaulted.
///
/// The bridge (`plugins-bridge/src/factory.rs`) and facade
/// (`sotf-plugins/src/factory/create.rs`) now have their own custom routes.
/// The FFI keeps this direct route rather than delegating: it validates both
/// the input bus (order-derived) and the output bus (layout-derived) against
/// the requested FFI layout before returning, emits the FFI-specific
/// fail-closed message naming `custom_layout`, and serves state restores that
/// must preserve `custom_layout` across rebuilds. Triple-route equivalence
/// (FFI/bridge/facade channel counts plus bit-identical renders) is pinned by
/// `ambisonics_triple_route_equivalence_ffi_bridge_facade`.
fn create_custom_ambisonics_plugin(
    plugin_config: &str,
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
) -> Result<Box<dyn Plugin>, String> {
    use sotf_plugin_ambisonics::custom_layout::{CUSTOM_LAYOUT_KEY, CustomDecoderConfig};

    let raw: serde_json::Value = serde_json::from_str(plugin_config)
        .map_err(|error| format!("Invalid Ambisonics custom configuration JSON: {error}"))?;
    if raw.get("custom_layout").is_none() {
        return Err(format!(
            "Ambisonics target \"{CUSTOM_LAYOUT_KEY}\" requires a \"custom_layout\" geometry object; the FFI cannot build a custom decoder without speaker positions"
        ));
    }
    let config: CustomDecoderConfig = serde_json::from_value(raw)
        .map_err(|error| format!("Invalid Ambisonics custom configuration: {error}"))?;
    let mut plugin = sotf_plugin_ambisonics::AmbisonicsDecoderPlugin::new_custom(&config)?;
    if plugin.input_channels() != input_channels {
        return Err(format!(
            "Custom Ambisonics order-{} decoder has {} input channels, requested {input_channels}",
            config.params.order,
            plugin.input_channels()
        ));
    }
    if plugin.output_channels() != output_channels {
        return Err(format!(
            "Custom Ambisonics layout '{}' has {} output channels, requested {output_channels}",
            config.custom_layout.name,
            plugin.output_channels()
        ));
    }
    // Mirror the bridge named route, which initializes eagerly.
    plugin.initialize(f64::from(sample_rate))?;
    Ok(Box::new(plugin))
}

// State restoration must apply structural settings before selecting native
// processing adapters (in particular, the oversampling factor).
pub(crate) fn create_unprepared_plugin(
    plugin_type: &str,
    config_json: &str,
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
) -> Result<Box<dyn Plugin>, String> {
    let plugin_type = canonical_direct_plugin_type(plugin_type);
    if plugin_type == "Resampler" {
        return Err(
            "Resampler is unavailable through the fixed-rate plugin FFI: its input and output frame counts differ; use the engine/catalog variable-rate path instead"
                .to_string(),
        );
    }

    // FFI construction metadata belongs to the facade, not the plugin schema.
    // Consume it before deserializing strict `deny_unknown_fields` configs.
    let plugin_config = plugin_config_without_ffi_metadata(config_json)?;
    // Custom Ambisonics geometry uses the direct DSP route (see
    // `create_custom_ambisonics_plugin` for why it does not delegate to the
    // bridge custom route); bare "custom" without geometry fails closed.
    if matches!(plugin_type, "AmbisonicsDecoder" | "ambisonics_decoder")
        && ambisonics_config_selects_custom(&plugin_config)
    {
        return create_custom_ambisonics_plugin(
            &plugin_config,
            input_channels,
            output_channels,
            sample_rate,
        );
    }
    // BandMerge's legacy bridge constructor takes the merged output width;
    // Gate also takes program/output width, as does an externally-keyed
    // DeEsser (its input bus is program + key). Other routes take input width.
    // Validate both actual buses so inconsistent sidechain/band counts cannot
    // bypass the requested FFI layout.
    let constructor_channels =
        if matches!(plugin_type, "BandMerge" | "band_merge" | "Gate" | "gate")
            || (matches!(plugin_type, "DeEsser" | "de_esser")
                && de_esser_config_is_external_sidechain(&plugin_config))
        {
            output_channels
        } else {
            input_channels
        };
    let plugin = plugins_bridge::create_plugin(
        plugin_type,
        constructor_channels,
        f64::from(sample_rate),
        &plugin_config,
    )?;
    if plugin.input_channels() != input_channels {
        return Err(format!(
            "Plugin {plugin_type} created with {} input channels, requested {input_channels}",
            plugin.input_channels()
        ));
    }
    if plugin.output_channels() != output_channels {
        return Err(format!(
            "Plugin {plugin_type} created with {} output channels, requested {output_channels}",
            plugin.output_channels()
        ));
    }
    Ok(plugin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_eq_plugin() {
        let config_json = r#"{
            "filters": [
                {"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 3.0}
            ]
        }"#;

        let plugin = create_plugin("EQ", config_json, 2, 2, 48000).unwrap();

        assert_eq!(plugin.input_channels(), 2);
        assert_eq!(plugin.output_channels(), 2);
    }

    #[test]
    fn test_create_compressor() {
        let plugin = create_plugin("Compressor", "{}", 2, 2, 48000).unwrap();
        assert_eq!(plugin.input_channels(), 2);
    }

    #[test]
    fn test_eq_channel_mismatch() {
        // EQ requires input_channels == output_channels (handled by EQ plugin itself)
        let config_json = r#"{"filters": []}"#;
        let result = create_plugin("EQ", config_json, 2, 2, 48000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_downmix_uses_independent_input_and_output_widths() {
        let plugin = create_plugin("Downmix", "{}", 6, 2, 48_000).unwrap();
        assert_eq!(plugin.input_channels(), 6);
        assert_eq!(plugin.output_channels(), 2);
        assert!(create_plugin("Downmix", "{}", 6, 6, 48_000).is_err());
    }

    #[test]
    fn band_merge_translates_legacy_constructor_width_and_validates_both_buses() {
        for alias in ["BandMerge", "band_merge"] {
            for (config, input_channels, output_channels) in [
                ("{}", 4, 2),
                (r#"{"bands":3}"#, 6, 2),
                (r#"{"bands":4}"#, 4, 1),
            ] {
                let plugin =
                    create_plugin(alias, config, input_channels, output_channels, 48_000).unwrap();
                assert_eq!(plugin.input_channels(), input_channels);
                assert_eq!(plugin.output_channels(), output_channels);
            }
            assert!(create_plugin(alias, "{}", 6, 2, 48_000).is_err());
            assert!(create_plugin(alias, r#"{"bands":3}"#, 4, 2, 48_000).is_err());
        }
    }

    #[test]
    fn fixed_rate_factory_rejects_resampler_without_removing_it_from_catalog() {
        let config = r#"{"input_sample_rate":48000,"output_sample_rate":44100}"#;
        let error = create_plugin("Resampler", config, 2, 2, 48_000)
            .err()
            .expect("fixed-rate facade must reject variable-rate processing");
        assert!(error.contains("fixed-rate plugin FFI"));
        assert!(
            sotf_plugins::supported_plugin_types().any(|plugin_type| plugin_type == "resampler")
        );
        assert!(create_plugin("re-sampler", config, 2, 2, 48_000).is_err());
    }

    #[test]
    fn direct_format_type_normalization_is_alias_stable() {
        assert_eq!(
            canonical_direct_plugin_type("linear_phase_eq"),
            "LinearPhaseEQ"
        );
        assert_eq!(
            canonical_direct_plugin_type("Linear-Phase-EQ"),
            "LinearPhaseEQ"
        );
        assert_eq!(canonical_direct_plugin_type("re_sampler"), "Resampler");
        assert_eq!(canonical_direct_plugin_type("Gain"), "Gain");
    }

    #[test]
    fn preset_import_identity_matches_supported_aliases_and_directional_migration() {
        for alias_group in [
            &["DynamicEQ", "dynamic_eq"][..],
            &["LinearPhaseEQ", "linear_phase_eq", "Linear-Phase-EQ"],
            &["AAE", "aae", "active_acoustic_enhancement"],
            &["SpeechDenoiser", "speech_denoiser", "RNNoise", "rnnoise"],
            &["HissReducer", "hiss_reducer", "Hiss", "hiss"],
            &["Declick", "declick", "TransientRepair", "transient_repair"],
        ] {
            for left in alias_group {
                for right in alias_group {
                    assert!(
                        preset_import_type_matches_target(left, right),
                        "target {left} vs saved {right}"
                    );
                }
            }
        }

        assert!(!preset_import_type_matches_target(
            "Compressor",
            "MultibandCompressor"
        ));
        assert!(preset_import_type_matches_target(
            "LoudnessCompensation",
            "FletcherMunson"
        ));
        assert!(preset_import_type_matches_target(
            "loudness_compensation",
            "fletcher_munson"
        ));
        assert!(!preset_import_type_matches_target(
            "FletcherMunson",
            "LoudnessCompensation"
        ));
        assert!(!preset_import_type_matches_target(
            "DynamicEQ",
            "dynamic-eq"
        ));
    }

    #[test]
    fn linear_phase_eq_rebuilt_fir_length_updates_latency() {
        let short_config = r#"{"num_filters":1,"fir_length_index":0,"filters":[]}"#;
        let long_state = br#"{
            "num_filters":1,
            "fir_length":3,
            "phase_mode":0,
            "auto_gain":false,
            "mix":1.0,
            "band_0_type":0,
            "band_0_freq":1000.0,
            "band_0_q":1.0,
            "band_0_gain":0.0,
            "band_0_active":true
        }"#;
        let long_config =
            merge_linear_phase_eq_state_into_config(short_config, long_state).unwrap();
        let mut short = create_plugin("LinearPhaseEQ", short_config, 2, 2, 48_000).unwrap();
        let mut long = create_plugin("LinearPhaseEQ", &long_config, 2, 2, 48_000).unwrap();
        short.initialize(48_000.0).unwrap();
        long.initialize(48_000.0).unwrap();
        assert!(long.latency_samples() > short.latency_samples());
    }

    #[test]
    fn linear_phase_eq_adapter_latency_uses_negotiated_callback_quantum() {
        let config = r#"{"num_filters":1,"fir_length_index":0,"filters":[]}"#;
        let mut inner =
            plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000.0, config).unwrap();
        inner.initialize(48_000.0).unwrap();
        let inner_latency = inner.latency_samples();

        let mut direct =
            create_plugin_with_max_callback("LinearPhaseEQ", config, 2, 2, 48_000, 257).unwrap();
        // The facade's normal post-factory initialize call is adapter-idempotent.
        direct.initialize(48_000.0).unwrap();
        assert_eq!(direct.realtime_quantum_frames(), 1);
        assert_eq!(direct.latency_samples(), inner_latency + 2 * 257);
    }
}
