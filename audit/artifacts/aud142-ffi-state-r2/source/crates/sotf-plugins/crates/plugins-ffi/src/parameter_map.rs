// ============================================================================
// Parameter Mapping System - Delegates to plugins-bridge ParamBridge
// ============================================================================
//
// Maps plugin parameters to a generic C-compatible parameter system for AU hosts.
// Uses ParamBridge from plugins-bridge for normalization and metadata.

use plugins_bridge::ParamBridge;
use sotf_host::param_specs::ParamType;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::Plugin;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

fn ffi_cstring(value: impl AsRef<str>) -> CString {
    match CString::new(value.as_ref()) {
        Ok(value) => value,
        Err(err) => {
            let mut bytes = err.into_vec();
            bytes.retain(|byte| *byte != 0);
            CString::new(bytes).unwrap_or_default()
        }
    }
}

fn ffi_cstring_ptr(value: impl AsRef<str>) -> *const c_char {
    ffi_cstring(value).into_raw() as *const c_char
}

/// Parameter information exposed to AU host
#[repr(C)]
#[derive(Debug, Clone)]
pub struct ParameterInfo {
    /// Unique parameter ID (e.g., "threshold_db")
    pub id: *const c_char,
    /// Human-readable name (e.g., "Threshold")
    pub name: *const c_char,
    /// Unit string (e.g., "Hz", "dB", "")
    pub unit: *const c_char,
    /// Minimum value
    pub min_value: f64,
    /// Maximum value
    pub max_value: f64,
    /// Default value
    pub default_value: f64,
    /// Number of steps (0 = continuous)
    pub steps: u32,
    /// Whether this parameter uses logarithmic scaling
    pub logarithmic: bool,
}

impl ParameterInfo {
    fn normalize(&self, raw: f64) -> f64 {
        if self.max_value <= self.min_value {
            return 0.0;
        }
        if self.logarithmic && self.min_value > 0.0 {
            let log_min = self.min_value.ln();
            ((raw.clamp(self.min_value, self.max_value).ln() - log_min)
                / (self.max_value.ln() - log_min))
                .clamp(0.0, 1.0)
        } else {
            ((raw - self.min_value) / (self.max_value - self.min_value)).clamp(0.0, 1.0)
        }
    }

    fn denormalize(&self, normalized: f64) -> f64 {
        let normalized = normalized.clamp(0.0, 1.0);
        if self.logarithmic && self.min_value > 0.0 {
            let log_min = self.min_value.ln();
            (log_min + normalized * (self.max_value.ln() - log_min)).exp()
        } else {
            self.min_value + normalized * (self.max_value - self.min_value)
        }
    }
}

/// Parameter mapping for a plugin, backed by plugins-bridge ParamBridge.
pub struct ParameterMap {
    bridge: ParamBridge,
    /// Cached C-compatible info structs (leaked CStrings for FFI safety).
    /// Stored as `ParameterInfo` directly so we can return stable pointers via `get_info()`.
    cached_infos: Vec<ParameterInfo>,
    cached_ids: Vec<ParameterId>,
    cached_kinds: Vec<FallbackKind>,
    plugin_type: String,
}

/// Prepared at construction, including inactive band slots from their schema.
enum FallbackKind {
    Float,
    Int,
    Bool,
    StringChoice(&'static [&'static str]),
    Unsupported,
}

const CROSSOVER_MODE_VALUES: &[&str] = &["lowpass", "highpass", "both"];
const CROSSOVER_CHANNEL_MODE_VALUES: &[&str] = &["lowpass", "highpass", "mute", "passthrough"];

const CROSSOVER_TYPE_LABELS: [&CStr; 13] = [
    c"LR24",
    c"LinearPhase",
    c"LR12",
    c"LR48",
    c"BW6",
    c"BW12",
    c"BW18",
    c"BW24",
    c"BW30",
    c"BW36",
    c"BW42",
    c"BW48",
    c"Bessel12",
];

const CROSSOVER_MODE_LABELS: [&CStr; 3] = [c"Lowpass", c"Highpass", c"Both"];
const CROSSOVER_CHANNEL_MODE_LABELS: [&CStr; 4] =
    [c"Lowpass", c"Highpass", c"Mute", c"Passthrough"];

fn is_crossover_type(plugin_type: &str) -> bool {
    matches!(plugin_type, "Crossover" | "crossover")
}

fn crossover_string_choices(plugin_type: &str, param_id: &str) -> Option<&'static [&'static str]> {
    if !is_crossover_type(plugin_type) {
        return None;
    }

    match param_id {
        "type" => Some(sotf_plugins::param_specs::crossover::CROSSOVER_TYPES),
        "mode" => Some(CROSSOVER_MODE_VALUES),
        id if id.starts_with("channel_mode_") => Some(CROSSOVER_CHANNEL_MODE_VALUES),
        _ => None,
    }
}

fn crossover_choice_label(param_id: &str, choice_index: usize) -> Option<*const c_char> {
    let label = match param_id {
        "type" => CROSSOVER_TYPE_LABELS.get(choice_index),
        "mode" => CROSSOVER_MODE_LABELS.get(choice_index),
        id if id.starts_with("channel_mode_") => CROSSOVER_CHANNEL_MODE_LABELS.get(choice_index),
        _ => None,
    }?;
    Some(label.as_ptr())
}

/// Translate a pre-migration external parameter id to its canonical key.
///
/// Enumeration always exposes canonical ids, but hosts with persisted
/// sessions still address renamed choice parameters by their legacy ids.
/// Scoped per plugin type so identically-named parameters of untouched
/// plugins (e.g. the upmixer's own `fft_size`) are never hijacked.
fn canonical_param_id<'a>(plugin_type: &str, param_id: &'a str) -> std::borrow::Cow<'a, str> {
    // FFI callers use display names ("LinearPhaseEQ"), snake case, and
    // hyphenated aliases ("Linear-Phase-EQ") interchangeably; normalize all
    // three before matching so the legacy table cannot be bypassed by
    // spelling.
    let matches_family = |family: &str| {
        plugin_type
            .bytes()
            .filter(|byte| *byte != b'-' && *byte != b'_')
            .map(|byte| byte.to_ascii_lowercase())
            .eq(family.bytes())
    };
    let linear_phase_eq = matches_family("linearphaseeq") || matches_family("firdesigner");
    let crossfeed = matches_family("crossfeed");
    let spectral = matches_family("spectralcompressor");
    let band_split = matches_family("bandsplit");
    use std::borrow::Cow;
    match param_id {
        "fir_length" if linear_phase_eq => Cow::Borrowed("fir_length_index"),
        "phase_mode" if linear_phase_eq => Cow::Borrowed("phase_mode_index"),
        "crossfeed_mode" if crossfeed => Cow::Borrowed("mode"),
        "crossfeed_preset" if crossfeed => Cow::Borrowed("preset"),
        "fft_size" if spectral => Cow::Borrowed("fft_size_index"),
        "crossover_type" if band_split => Cow::Borrowed("type"),
        _ => Cow::Borrowed(param_id),
    }
}

impl ParameterMap {
    /// Create parameter map from a plugin using its ParamSpec definitions.
    pub fn from_plugin(plugin: &dyn Plugin, plugin_type: &str) -> Self {
        Self::from_specs(plugin, plugin_type, get_param_specs(plugin_type))
    }

    fn from_specs(
        plugin: &dyn Plugin,
        plugin_type: &str,
        specs: &[sotf_host::param_specs::ParamSpec],
    ) -> Self {
        let bridge = ParamBridge::new(specs);
        let runtime_parameters = plugin.parameters();

        // Pre-build cached C-compatible info structs from static ParamSpec
        let mut cached_infos = Vec::with_capacity(bridge.count());
        for i in 0..bridge.count() {
            if let Some(info) = bridge.info(i) {
                let id = ffi_cstring_ptr(info.id);
                let name = ffi_cstring_ptr(info.name);
                let unit = ffi_cstring_ptr(info.unit);

                cached_infos.push(ParameterInfo {
                    id,
                    name,
                    unit,
                    min_value: info.min_value,
                    max_value: info.max_value,
                    default_value: info.default_value,
                    steps: info.steps,
                    logarithmic: info.logarithmic,
                });
            }
        }

        // Expand per-band templates for plugins with dynamic bands (EQ, multiband, etc.)
        // Band parameters use "band_N_field" naming convention matching the Rust plugin's
        // set_parameter/get_parameter interface.
        if let Some((template, max_bands)) = get_band_template(plugin_type) {
            expand_band_params(&mut cached_infos, template, max_bands);
        }
        // DynamicEQ's new shelf controls are append-only. Keep its seven-field
        // legacy stride intact so every pre-existing numeric address survives.
        if let Some((template, max_bands)) = get_appended_band_template(plugin_type) {
            expand_band_params(&mut cached_infos, template, max_bands);
        }

        // Fallback: if no static specs produced params, use Plugin::parameters()
        if cached_infos.is_empty() {
            for param in &runtime_parameters {
                let (min, max, default, steps) =
                    match (&param.min_value, &param.max_value, &param.default_value) {
                        (
                            Some(sotf_host::parameters::ParameterValue::Float(min)),
                            Some(sotf_host::parameters::ParameterValue::Float(max)),
                            sotf_host::parameters::ParameterValue::Float(def),
                        ) => (*min as f64, *max as f64, *def as f64, 0),
                        (_, _, ParameterValue::Bool(default)) => (0.0, 1.0, f64::from(*default), 0),
                        (
                            Some(sotf_host::parameters::ParameterValue::Int(min)),
                            Some(sotf_host::parameters::ParameterValue::Int(max)),
                            sotf_host::parameters::ParameterValue::Int(def),
                        ) => (*min as f64, *max as f64, *def as f64, 0),
                        (_, _, ParameterValue::String(default)) => {
                            let choices = crossover_string_choices(plugin_type, &param.id.0);
                            match choices.and_then(|choices| {
                                choices
                                    .iter()
                                    .position(|choice| *choice == default)
                                    .map(|index| (choices, index))
                            }) {
                                Some((choices, index)) => {
                                    let max_index = choices.len().saturating_sub(1) as f64;
                                    (0.0, max_index, index as f64, max_index as u32)
                                }
                                None => (0.0, 1.0, 0.0, 0),
                            }
                        }
                        _ => (0.0, 1.0, 0.0, 0),
                    };

                let id = ffi_cstring_ptr(&param.id.0);
                let name = ffi_cstring_ptr(&param.name);
                let unit = ffi_cstring_ptr(&param.unit);

                cached_infos.push(ParameterInfo {
                    id,
                    name,
                    unit,
                    min_value: min,
                    max_value: max,
                    default_value: default,
                    steps,
                    logarithmic: param.logarithmic,
                });
            }
        }

        let cached_ids: Vec<ParameterId> = cached_infos
            .iter()
            .map(|info| {
                // SAFETY: These CStrings are owned by cached_infos and remain valid
                // until ParameterMap::drop; ParameterId copies the bytes here.
                let key = unsafe { std::ffi::CStr::from_ptr(info.id).to_str().unwrap_or("") };
                ParameterId::from(key)
            })
            .collect();
        let cached_kinds = cached_ids
            .iter()
            .map(|id| {
                let template_type =
                    id.0.strip_prefix("band_")
                        .and_then(|suffix| suffix.split_once('_'))
                        .and_then(|(_, field)| band_spec_for_field(plugin_type, field))
                        .map(|spec| spec.param_type);
                let runtime = runtime_parameters
                    .iter()
                    .find(|parameter| parameter.id == *id);
                match runtime.map(|parameter| &parameter.default_value) {
                    Some(ParameterValue::Float(_)) => FallbackKind::Float,
                    Some(ParameterValue::Int(_)) => FallbackKind::Int,
                    Some(ParameterValue::Bool(_)) => FallbackKind::Bool,
                    Some(ParameterValue::String(value)) => match template_type {
                        Some(ParamType::Choice { labels, .. }) => {
                            FallbackKind::StringChoice(labels)
                        }
                        _ => crossover_string_choices(plugin_type, &id.0)
                            .filter(|labels| labels.iter().any(|label| *label == value))
                            .map_or(FallbackKind::Unsupported, FallbackKind::StringChoice),
                    },
                    None => match template_type {
                        Some(ParamType::Float { .. }) => FallbackKind::Float,
                        Some(ParamType::Int { .. } | ParamType::Choice { .. }) => FallbackKind::Int,
                        Some(ParamType::Bool { .. }) => FallbackKind::Bool,
                        _ => FallbackKind::Unsupported,
                    },
                }
            })
            .collect();
        Self {
            bridge,
            cached_infos,
            cached_ids,
            cached_kinds,
            plugin_type: plugin_type.to_string(),
        }
    }

    /// Get the number of parameters.
    pub fn count(&self) -> usize {
        self.cached_infos.len()
    }

    /// Get parameter info by index.
    ///
    /// Returns a reference to the cached info, which is valid for the lifetime of this ParameterMap.
    /// This is critical for FFI safety — callers can convert the reference to a raw pointer that
    /// remains valid as long as the PluginHandle (and thus this ParameterMap) is alive.
    pub fn get_info(&self, index: usize) -> Option<&ParameterInfo> {
        self.cached_infos.get(index)
    }

    /// Get the param_id string for a given index.
    pub fn param_id_at(&self, index: usize) -> Option<&str> {
        self.cached_infos.get(index).map(|info| {
            // SAFETY: id was created from CString::into_raw and is valid for the lifetime of self
            unsafe { std::ffi::CStr::from_ptr(info.id).to_str().unwrap_or("") }
        })
    }

    /// Return a static, NUL-terminated choice label for an enumerated
    /// parameter. DynamicEQ shelf-shape labels are part of the public C ABI;
    /// their storage does not depend on a handle or allocate on lookup.
    pub fn choice_label_at(&self, index: usize, choice_index: usize) -> Option<*const c_char> {
        let param_id = self.param_id_at(index)?;
        if is_crossover_type(&self.plugin_type) {
            return crossover_choice_label(param_id, choice_index);
        }
        if is_dynamic_eq_type(&self.plugin_type) && param_id.rsplit_once('_')?.1 == "shape" {
            return match choice_index {
                0 => Some(c"Peak".as_ptr()),
                1 => Some(c"Low Shelf".as_ptr()),
                2 => Some(c"High Shelf".as_ptr()),
                _ => None,
            };
        }
        None
    }

    /// Get denormalized parameter value by index.
    ///
    /// Returns the raw value in parameter units (Hz, dB, etc.).
    /// Uses the `ParamBridge` for correct scaling (log for Hz, linear for others).
    pub fn get_denormalized_by_index(&self, plugin: &dyn Plugin, index: usize) -> Option<f64> {
        let info = self.cached_infos.get(index)?;
        let param_id = unsafe { std::ffi::CStr::from_ptr(info.id).to_str().unwrap_or("") };
        let normalized = self.get_normalized(plugin, param_id)?;
        // Use bridge for correct denormalization (handles log scaling for Hz params)
        self.bridge
            .denormalize(index, normalized)
            .or_else(|| Some(info.denormalize(normalized)))
    }

    /// Set denormalized parameter value by index.
    ///
    /// Takes the raw value in parameter units (Hz, dB, etc.) and normalizes internally.
    /// Uses the `ParamBridge` for correct scaling (log for Hz, linear for others).
    pub fn set_denormalized_by_index(
        &self,
        plugin: &mut dyn Plugin,
        index: usize,
        value: f64,
    ) -> Result<(), String> {
        let info = self
            .cached_infos
            .get(index)
            .ok_or_else(|| format!("Parameter index {index} out of range"))?;
        let param_id = unsafe { std::ffi::CStr::from_ptr(info.id).to_str().unwrap_or("") };
        // Use bridge for correct normalization (handles log scaling for Hz params)
        let normalized = self
            .bridge
            .normalize(index, value)
            .unwrap_or_else(|| info.normalize(value));
        self.set_normalized(plugin, param_id, normalized)
    }

    /// Set parameter value (normalized 0.0-1.0).
    pub fn set_normalized(
        &self,
        plugin: &mut dyn Plugin,
        param_id: &str,
        normalized_value: f64,
    ) -> Result<(), String> {
        let param_id: &str = &canonical_param_id(&self.plugin_type, param_id);
        // Try ParamBridge first
        if let Some(index) = self.bridge.find_index(param_id) {
            return self.bridge.set_normalized(plugin, index, normalized_value);
        }

        // Fallback: direct set using raw parameter system
        // Denormalize using cached info (log scaling for Hz params, linear for others)
        if let Some(pos) = self.cached_infos.iter().position(|info| {
            let id = unsafe { std::ffi::CStr::from_ptr(info.id).to_str().unwrap_or("") };
            id == param_id
        }) {
            let info = &self.cached_infos[pos];
            let normalized_value = normalized_value.clamp(0.0, 1.0);
            let raw = info.denormalize(normalized_value);
            let value = match &self.cached_kinds[pos] {
                FallbackKind::Float => ParameterValue::Float(raw as f32),
                FallbackKind::Int => {
                    let raw = if info.steps > 0 && info.max_value > info.min_value {
                        let step = (info.max_value - info.min_value) / f64::from(info.steps);
                        info.min_value + ((raw - info.min_value) / step).round() * step
                    } else {
                        raw
                    };
                    ParameterValue::Int(raw.round() as i32)
                }
                FallbackKind::Bool => ParameterValue::Bool(normalized_value >= 0.5),
                FallbackKind::StringChoice(labels) => {
                    let label = labels
                        .get(raw.round() as usize)
                        .ok_or_else(|| format!("Invalid choice for {param_id}"))?;
                    ParameterValue::String((*label).to_owned())
                }
                FallbackKind::Unsupported => {
                    return Err(format!("{param_id} requires a string value"));
                }
            };
            plugin.set_parameter(self.cached_ids[pos].clone(), value)
        } else {
            Err(format!("Unknown parameter: {param_id}"))
        }
    }

    /// Get parameter value (normalized 0.0-1.0).
    pub fn get_normalized(&self, plugin: &dyn Plugin, param_id: &str) -> Option<f64> {
        let param_id: &str = &canonical_param_id(&self.plugin_type, param_id);
        // Try ParamBridge first
        if let Some(index) = self.bridge.find_index(param_id) {
            return self.bridge.get_normalized(plugin, index);
        }

        // Fallback: direct get using raw parameter system
        let pos = self.cached_infos.iter().position(|info| {
            let id = unsafe { std::ffi::CStr::from_ptr(info.id).to_str().unwrap_or("") };
            id == param_id
        })?;

        let info = &self.cached_infos[pos];
        let value = plugin.get_parameter(&self.cached_ids[pos])?;
        let raw = match value {
            sotf_host::parameters::ParameterValue::Float(f) => f as f64,
            sotf_host::parameters::ParameterValue::Int(i) => i as f64,
            sotf_host::parameters::ParameterValue::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            ParameterValue::String(value) => match &self.cached_kinds[pos] {
                FallbackKind::StringChoice(labels) => {
                    labels.iter().position(|label| *label == value)? as f64
                }
                _ => return None,
            },
        };
        Some(info.normalize(raw))
    }
}

impl Drop for ParameterMap {
    fn drop(&mut self) {
        // Reclaim the leaked CStrings
        for info in &self.cached_infos {
            unsafe {
                if !info.id.is_null() {
                    drop(CString::from_raw(info.id as *mut c_char));
                }
                if !info.name.is_null() {
                    drop(CString::from_raw(info.name as *mut c_char));
                }
                if !info.unit.is_null() {
                    drop(CString::from_raw(info.unit as *mut c_char));
                }
            }
        }
    }
}

/// Get the band template and max band count for plugins with per-band parameters.
/// Returns None for plugins without dynamic bands.
fn get_band_template(
    plugin_type: &str,
) -> Option<(&'static [sotf_host::param_specs::ParamSpec], usize)> {
    use sotf_plugins::param_specs::*;

    match plugin_type {
        "EQ" | "eq" => Some((eq::BAND_TEMPLATE, 20)),
        "MultibandCompressor" | "multiband_compressor" => {
            Some((multiband_compressor::BAND_TEMPLATE, 5))
        }
        "MultibandExpander" | "multiband_expander" => Some((multiband_expander::BAND_TEMPLATE, 5)),
        "DynamicEQ" | "dynamic_eq" => Some((dynamic_eq::BAND_PARAMS, 8)),
        "LinearPhaseEQ" | "linear_phase_eq" => {
            Some((linear_phase_eq::BAND_TEMPLATE, linear_phase_eq::MAX_FILTERS))
        }
        _ => None,
    }
}

/// Template appended after all legacy DynamicEQ band fields.
fn get_appended_band_template(
    plugin_type: &str,
) -> Option<(&'static [sotf_host::param_specs::ParamSpec], usize)> {
    use sotf_plugins::param_specs::dynamic_eq;

    is_dynamic_eq_type(plugin_type).then_some((dynamic_eq::SHELF_PARAMS, 8))
}

fn is_dynamic_eq_type(plugin_type: &str) -> bool {
    matches!(plugin_type, "DynamicEQ" | "dynamic_eq" | "dynamic-eq")
}

fn band_spec_for_field(
    plugin_type: &str,
    field: &str,
) -> Option<&'static sotf_host::param_specs::ParamSpec> {
    get_band_template(plugin_type)
        .into_iter()
        .chain(get_appended_band_template(plugin_type))
        .find_map(|(template, _)| template.iter().find(|spec| spec.engine_key == field))
}

/// Expand a per-band ParamSpec template into concrete ParameterInfo entries.
///
/// For each band 0..max_bands, creates parameters with IDs like "band_0_frequency",
/// "band_1_q", etc. — matching the naming convention used by the Rust plugins'
/// set_parameter/get_parameter implementations.
fn expand_band_params(
    cached_infos: &mut Vec<ParameterInfo>,
    template: &[sotf_host::param_specs::ParamSpec],
    max_bands: usize,
) {
    use sotf_host::param_specs::ParamType;

    for band_idx in 0..max_bands {
        for spec in template {
            let band_id = format!("band_{}_{}", band_idx, spec.engine_key);
            let band_name = format!("Band {} {}", band_idx + 1, spec.name);

            let (min, max, default, steps, logarithmic) = match spec.param_type {
                ParamType::Float {
                    default,
                    min,
                    max,
                    step,
                } => {
                    let steps = if step > 0.0 {
                        ((max - min) / step) as u32
                    } else {
                        0
                    };
                    // Hz params with positive min use logarithmic scaling
                    let is_log = spec.unit == "Hz" && min > 0.0;
                    (min, max, default, steps, is_log)
                }
                ParamType::Int {
                    default,
                    min,
                    max,
                    step,
                } => (
                    min as f64,
                    max as f64,
                    default as f64,
                    ((max - min) / step) as u32,
                    false,
                ),
                ParamType::Bool { default, .. } => {
                    (0.0, 1.0, if default { 1.0 } else { 0.0 }, 1, false)
                }
                ParamType::Choice {
                    default_index,
                    labels,
                } => (
                    0.0,
                    (labels.len().saturating_sub(1)) as f64,
                    default_index as f64,
                    labels.len().saturating_sub(1) as u32,
                    false,
                ),
                ParamType::FilePath => continue, // skip file paths for AU
            };

            let id = ffi_cstring_ptr(&band_id);
            let name = ffi_cstring_ptr(&band_name);
            let unit = ffi_cstring_ptr(spec.unit);

            cached_infos.push(ParameterInfo {
                id,
                name,
                unit,
                min_value: min,
                max_value: max,
                default_value: default,
                steps,
                logarithmic,
            });
        }
    }
}

/// Get the ParamSpec array for a given plugin type.
fn get_param_specs(plugin_type: &str) -> &'static [sotf_host::param_specs::ParamSpec] {
    use sotf_plugins::param_specs::*;

    match plugin_type {
        // EQ has GLOBAL_PARAMS + per-band BAND_TEMPLATE (dynamic bands)
        // Expose global params; band params come from Plugin::parameters() fallback
        "EQ" | "eq" => eq::GLOBAL_PARAMS,
        "Compressor" | "compressor" => compressor::PARAMS,
        "Limiter" | "limiter" => limiter::PARAMS,
        "Gate" | "gate" => gate::PARAMS,
        "Gain" | "gain" => gain::PARAMS,
        "Expander" | "expander" => expander::PARAMS,
        "Crossfeed" | "crossfeed" => crossfeed::PARAMS,
        "FletcherMunson" | "fletcher_munson" => loudness_compensation::PARAMS,
        "LoudnessCompensation" | "loudness_compensation" => loudness_compensation::PARAMS,
        // Multiband plugins have GLOBAL_PARAMS + per-band params (dynamic)
        "MultibandCompressor" | "multiband_compressor" => multiband_compressor::GLOBAL_PARAMS,
        "MultibandExpander" | "multiband_expander" => multiband_expander::GLOBAL_PARAMS,
        "Upmixer" | "upmixer" => upmixer::PARAMS,
        "AAE" | "aae" => aae::PARAMS,
        "XTC" | "xtc" => xtc::PARAMS,
        "Binaural" | "binaural" => binaural::PARAMS,
        "ChannelMuteSolo" | "channel_mute_solo" => channel_mute_solo::PARAMS,
        "Convolution" | "convolution" => convolution::PARAMS,
        "ABCompare" | "ab_compare" => ab_compare::PARAMS,
        "MonoToStereo" | "mono_to_stereo" => mono_to_stereo::PARAMS,
        "PND" | "pnd" => pnd::PARAMS,
        "Denoiser" | "denoiser" => denoiser::PARAMS,
        "SpeechDenoiser" | "speech_denoiser" | "RNNoise" | "rnnoise" => speech_denoiser::PARAMS,
        "HissReducer" | "hiss_reducer" | "Hiss" | "hiss" => hiss_reducer::PARAMS,
        "Declick" | "declick" | "TransientRepair" | "transient_repair" => declick::PARAMS,
        "Downmix" | "downmix" => downmix::PARAMS,
        "Saturation" | "saturation" => saturation::PARAMS,
        "AnalogEQ" | "analog_eq" => analog_eq::PARAMS,
        "AnalogLimiter" | "analog_limiter" => analog_limiter::PARAMS,
        "AnalogCompressor" | "analog_compressor" => analog_compressor::PARAMS,
        "StereoImager" | "stereo_imager" => stereo_imager::PARAMS,
        "TransientShaper" | "transient_shaper" => transient_shaper::PARAMS,
        "DeEsser" | "de_esser" => de_esser::PARAMS,
        "DynamicEQ" | "dynamic_eq" => dynamic_eq::PARAMS,
        "LinearPhaseEQ" | "linear_phase_eq" => linear_phase_eq::PARAMS,
        "Dither" | "dither" => dither::PARAMS,
        "BandSplit" | "band_split" => band_split::PARAMS,
        "BandMerge" | "band_merge" => band_merge::PARAMS,
        "AEC" | "aec" => aec::PARAMS,
        "Beamformer" | "beamformer" => beamformer::PARAMS,
        "SpectralCompressor" | "spectral_compressor" => spectral_compressor::PARAMS,
        "AmbisonicsDecoder" | "ambisonics_decoder" => ambisonics::PARAMS,
        // Plugins without param_specs entries fall back to Plugin::parameters() in from_plugin()
        "Delay" | "delay" | "Matrix" | "matrix" | "Crossover" | "crossover" | "LoudnessMonitor"
        | "loudness_monitor" | "SpectrumAnalyzer" | "spectrum_analyzer" => &[],
        other => {
            panic!("get_param_specs: unknown plugin type \"{other}\" — add it to the match arm")
        }
    }
}

/// Get the global (non-band) ParamSpec array for a plugin type.
/// Used by `AuHostState` to determine the global param offset for band-based plugins.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn global_param_specs(plugin_type: &str) -> &'static [sotf_host::param_specs::ParamSpec] {
    get_param_specs(plugin_type)
}

/// Get band template info for a plugin type: `(params_per_band, max_bands)`.
/// Returns `None` for plugins without dynamic bands.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn band_template_info(plugin_type: &str) -> Option<(usize, usize)> {
    get_band_template(plugin_type).map(|(template, max_bands)| (template.len(), max_bands))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffi_cstring_strips_interior_nul_without_panicking() {
        let value = ffi_cstring("gain\0db");
        assert_eq!(value.to_str().unwrap(), "gaindb");
    }

    #[test]
    fn test_parameter_map_eq() {
        let plugin = plugins_bridge::create_plugin("EQ", 2, 48000, "{}").unwrap();
        let param_map = ParameterMap::from_plugin(&*plugin, "EQ");
        // Five global controls plus 20 bands × five params.
        assert_eq!(param_map.count(), 5 + 20 * 5);
    }

    #[test]
    fn runtime_metadata_fallback_preserves_bool_and_integer_types() {
        let config = r#"{"filters":[{"filter_type":"peak","freq":1000.0,"q":1.5,"db_gain":3.0}]}"#;
        let mut plugin = plugins_bridge::create_plugin("EQ", 2, 48_000, config).unwrap();
        let map = ParameterMap::from_specs(&*plugin, "RuntimeMetadataOnly", &[]);
        for enabled in [true, false] {
            map.set_normalized(&mut *plugin, "tdf2", f64::from(enabled))
                .unwrap();
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("tdf2")),
                Some(ParameterValue::Bool(enabled))
            );
        }
        for order in [4, 6, 8, 2] {
            map.set_normalized(&mut *plugin, "band_0_order", f64::from(order - 2) / 6.0)
                .unwrap();
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("band_0_order")),
                Some(ParameterValue::Int(order))
            );
        }
    }

    #[test]
    fn expanded_frequency_raw_interface_matches_normalized_interface() {
        let config = r#"{"filters":[{"filter_type":"peak","freq":1000.0,"q":1.5,"db_gain":3.0}]}"#;
        let mut plugin = plugins_bridge::create_plugin("EQ", 2, 48_000, config).unwrap();
        let map = ParameterMap::from_plugin(&*plugin, "EQ");
        let index = (0..map.count())
            .find(|index| map.param_id_at(*index) == Some("band_0_freq"))
            .unwrap();
        assert!((map.get_denormalized_by_index(&*plugin, index).unwrap() - 1000.0).abs() < 1e-3);
        for frequency in [20.0, 160.0, 1000.0, 6400.0, 20000.0] {
            map.set_denormalized_by_index(&mut *plugin, index, frequency)
                .unwrap();
            let actual = plugin
                .get_parameter(&ParameterId::from("band_0_freq"))
                .unwrap()
                .as_float()
                .unwrap();
            assert!((f64::from(actual) - frequency).abs() < 1e-3);
            assert!(
                (map.get_denormalized_by_index(&*plugin, index).unwrap() - frequency).abs() < 1e-3
            );
        }
    }

    #[test]
    fn test_parameter_map_compressor() {
        let plugin = plugins_bridge::create_plugin("Compressor", 2, 48000, "{}").unwrap();
        let param_map = ParameterMap::from_plugin(&*plugin, "Compressor");
        assert!(param_map.count() > 0);

        // Check we can get info
        let info = param_map.get_info(0).unwrap();
        assert!(!info.id.is_null());
        assert!(!info.name.is_null());
    }

    #[test]
    fn test_parameter_map_linear_phase_eq_matches_dsp_band_ids_and_limit() {
        assert_eq!(band_template_info("LinearPhaseEQ"), Some((5, 10)));
        let plugin = plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, "{}").unwrap();
        let param_map = ParameterMap::from_plugin(&*plugin, "LinearPhaseEQ");
        assert_eq!(param_map.count(), 5 + 10 * 5);
    }

    #[test]
    fn spectral_compressor_target_choice_roundtrips_raw_and_normalized() {
        let mut plugin =
            plugins_bridge::create_plugin("SpectralCompressor", 2, 48_000, "{}").unwrap();
        let param_map = ParameterMap::from_plugin(&*plugin, "SpectralCompressor");
        let index = (0..param_map.count())
            .find(|index| param_map.param_id_at(*index) == Some("target_mode"))
            .expect("target_mode must be exported");

        for (raw, normalized) in [(0.0, 0.0), (1.0, 0.5), (2.0, 1.0)] {
            param_map
                .set_denormalized_by_index(&mut *plugin, index, raw)
                .unwrap();
            assert_eq!(
                param_map.get_denormalized_by_index(&*plugin, index),
                Some(raw)
            );
            assert_eq!(
                param_map.get_normalized(&*plugin, "target_mode"),
                Some(normalized)
            );
        }
    }
}
