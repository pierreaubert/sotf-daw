//! C-callable plugin lifecycle and processing entry points.
//!
//! Every exported function follows the same ownership contract: pointers
//! received from C are borrowed for the duration of the call, pointers
//! returned as owned values must be freed with the matching `plugin_free_*`
//! function exactly once, and any pointer derived from a [`PluginHandle`] is
//! only valid while that handle remains alive and undestroyed.

use super::PluginError;
use super::PluginFfiCapabilities;
use super::PluginHandle;
use super::PluginMidiEvent;
use super::PluginNoteExpressionEvent;
use super::PluginPresetDocumentInfo;
use super::PluginSwiftPackageInfo;
use super::PluginVst3FfiDescriptor;
use super::consts::MAX_FFI_MIDI_EVENTS_PER_BLOCK;
use super::consts::MAX_FFI_OUTPUT_EVENTS_PER_BLOCK;
use super::consts::MAX_PRESET_JSON_IMPORT_BYTES;
use super::consts::MAX_PRESET_STATE_BYTES;
use super::consts::PRESET_FILE_EXTENSION;
use super::consts::PRESET_MIME_TYPE;
use super::consts::PRESET_UT_TYPE;
use super::consts::SOTF_PLUGIN_FFI_ABI_VERSION;
use super::consts::SWIFT_HEADER_NAME;
use super::consts::SWIFT_LIBRARY_NAME;
use super::consts::SWIFT_PACKAGE_NAME;
use super::consts::SWIFT_PRODUCT_NAME;
use super::consts::SWIFT_TARGET_NAME;
use super::consts::VST3_COMPONENT_NAME;
use super::consts::VST3_ENTRYPOINT;
use super::consts::VST3_SDK_VERSION;
use super::consts::VST3_VENDOR;
use super::copy::copy_bytes_to_ffi_buffer;
use super::copy::copy_midi_output_events;
use super::copy::copy_note_expression_output_events;
use super::host::host_kind_name;
use super::libc::libc_free;
use super::libc::libc_malloc;
use super::misc::sanitize_filename_component;
use super::misc::set_last_error;
use super::misc::set_last_error_static;
use super::parameter_map::SPEECH_MODEL_CHOICE_COUNT;
pub use super::parameter_map::{ParameterInfo, ParameterMap};
use super::process::process_impl;
use super::process::process_with_ffi_events_impl;
use super::process::process_with_full_events_impl;
use super::types::current_host_kind;
use super::{LAST_ERROR, LAST_STATIC_ERROR};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::ProcessContext;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::slice;

const PRESET_UT_TYPE_JSON: &str = "org.spinorama.sotf.plugin-preset";

fn load_changed_state(
    plugin: &mut dyn sotf_host::plugin::Plugin,
    state: &[u8],
    plugin_type: &str,
) -> Result<(), String> {
    let mut incoming: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse plugin state: {error}"))?;

    // Crossfeed's preset selector is an action that changes several other
    // parameters. Apply it before their explicit saved values, so replaying a
    // saved custom mode cannot be overwritten by its preset selection.
    if matches!(plugin_type, "Crossfeed" | "crossfeed")
        && let Some(preset) = incoming.remove("preset")
    {
        let preset = serde_json::to_vec(&serde_json::json!({"preset": preset}))
            .map_err(|error| format!("Failed to serialize preset selection: {error}"))?;
        load_changed_state(plugin, &preset, "")?;
    }

    let current: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(plugin))
            .map_err(|error| format!("Failed to capture plugin state: {error}"))?;
    // Unchanged setters may run preset/momentary actions or reload resources.
    // Invalid values still differ and reach the normal typed loader below.
    incoming.retain(|key, value| current.get(key) != Some(value));
    let changes = serde_json::to_vec(&incoming)
        .map_err(|error| format!("Failed to serialize plugin state: {error}"))?;
    plugins_bridge::state::load_state(plugin, &changes)
}

fn is_dynamic_eq_shelf_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(plugin_type, "DynamicEQ" | "dynamic_eq" | "dynamic-eq") {
        return false;
    }
    let Some((band, field)) = param_id
        .strip_prefix("band_")
        .and_then(|suffix| suffix.split_once('_'))
    else {
        return false;
    };
    let Ok(index) = band.parse::<usize>() else {
        return false;
    };
    // Canonical indices only (`band_01_shape` is not a structural address;
    // it falls through to "unknown parameter", matching the merge's
    // fail-closed noncanonical rejection).
    if !canonical_index(band) || index >= 8 {
        return false;
    }
    matches!(field, "shape" | "shelf_slope" | "placement")
}

/// EQ placement addresses use Structural/restart semantics: a placement edit
/// rebuilds the filter bank and resets DSP history, so it must go through
/// state restoration (transactional, control thread) rather than a live
/// setter call.
fn is_eq_placement_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(plugin_type, "EQ" | "eq") {
        return false;
    }
    let Some(index_text) = param_id
        .strip_prefix("filter_")
        .and_then(|rest| rest.strip_suffix("_placement"))
    else {
        return false;
    };
    let Ok(index) = index_text.parse::<usize>() else {
        return false;
    };
    // Canonical form only; `filter_01_placement` is unknown, matching the
    // merge's fail-closed rule and `eq_placement_index`.
    canonical_index(index_text) && index < 20
}

/// LinearPhaseEQ placement addresses are structural like EQ placement:
/// the FIR bank rebuilds, so live edits must go through state restoration.
fn is_linear_phase_eq_placement_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(
        plugin_type,
        "LinearPhaseEQ" | "linear_phase_eq" | "Linear-Phase-EQ"
    ) {
        return false;
    }
    let Some((band, field)) = param_id
        .strip_prefix("band_")
        .and_then(|suffix| suffix.split_once('_'))
    else {
        return false;
    };
    let Ok(index) = band.parse::<usize>() else {
        return false;
    };
    if !canonical_index(band) || index >= 10 {
        return false;
    }
    field == "placement"
}

/// Checks decimal address spelling without allocating on the render thread.
fn canonical_index(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
}

/// De-esser structural IDs per `sotf-plugin-de-esser` PARAMS.
///
/// `frequency`, `q`, `mode`, `lookahead_ms`, `split_topology`, and
/// `sidechain_external` all rebuild DSP state or bus layout. The live DSP
/// setters reject changes transactionally ("requires a host rebuild"); the
/// FFI guard converts that into the uniform C ABI contract ("structural,
/// requires state restoration") before any live mutation is attempted.
fn is_de_esser_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(plugin_type, "DeEsser" | "de_esser") {
        return false;
    }
    matches!(
        param_id,
        "frequency" | "q" | "mode" | "lookahead_ms" | "split_topology" | "sidechain_external"
    )
}

/// Speech Denoiser structural model selector.
///
/// `model` swaps the inference graph; the live plugin rejects post-init
/// changes with an allocating `Err(String)`, and the generic FFI error
/// path formats a second allocation, so without this guard every changed
/// write from render-thread automation allocates (measured 4 alloc/4 free
/// steady state). `strength` stays realtime. Control-thread state
/// restoration is unaffected: it rebuilds through
/// `replace_plugin_from_state`, never this setter.
fn is_speech_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(
        plugin_type,
        "SpeechDenoiser" | "speech_denoiser" | "RNNoise" | "rnnoise"
    ) {
        return false;
    }
    matches!(param_id, "model")
}

/// Quantize a normalized Speech model write to a choice index.
///
/// Mirrors `ParamBridge` Choice denormalization exactly (clamp, scale,
/// round, saturate) so the guard accepts precisely the writes the bridge
/// would map to the committed index. Pure float/integer math, so the
/// render-thread guard cannot allocate.
fn speech_model_index_from_normalized(normalized_value: f64) -> usize {
    let clamped = normalized_value.clamp(0.0, 1.0);
    let index = (clamped * (SPEECH_MODEL_CHOICE_COUNT - 1) as f64).round() as usize;
    index.min(SPEECH_MODEL_CHOICE_COUNT - 1)
}

/// Hiss structural IDs per `sotf-plugin-hiss-reducer` PARAMS.
///
/// `spectral_mode` swaps the DSP engine (IIR vs STFT) and the live plugin
/// rejects post-init flips with an allocating `Err(String)`; without this
/// guard that allocation is reachable from render-thread automation.
/// `learn_noise`/`clear_profile` are momentary capture commands that must
/// never fire from the render loop. All three route through state
/// restoration on the control thread instead. `transient_guard` is
/// deliberately not structural: it is a plain realtime bool accepted
/// post-init without allocation.
fn is_hiss_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(
        plugin_type,
        "HissReducer" | "hiss_reducer" | "Hiss" | "hiss"
    ) {
        return false;
    }
    matches!(param_id, "spectral_mode" | "learn_noise" | "clear_profile")
}

/// Returns true for Hiss Reducer plugin type names.
///
/// Same alias set as [`is_hiss_structural_id`]; routes state restoration
/// to the Hiss transactional path. Profile handling still identifies the
/// live instance through the `get_data` snapshot downcast.
fn is_hiss_plugin_type(plugin_type: &str) -> bool {
    matches!(
        plugin_type,
        "HissReducer" | "hiss_reducer" | "Hiss" | "hiss"
    )
}

/// Hiss capture action: cancel an active capture.
pub const HISS_CAPTURE_CANCEL: c_int = 0;
/// Hiss capture action: start a new 1 s capture.
pub const HISS_CAPTURE_START: c_int = 1;
/// Hiss capture action: discard the stored profile.
pub const HISS_CLEAR_PROFILE: c_int = 2;

/// Restores Hiss state transactionally with profile preservation.
///
/// Reconstructs a candidate constructor from the accepted config plus the
/// current live scalars/profile, then applies the incoming partial state:
/// omitted keys preserve live values, explicit null clears the profile,
/// and valid v1/v2 blobs install exactly. Momentary `learn_noise` and
/// `clear_profile` keys never replay. All fallible work (typed profile
/// validation with explicit channel agreement, construction, preparation,
/// initialization, layout checks) precedes the sole commit, so failures
/// retain live audio, history, metadata, and configuration. The parameter
/// map is intentionally preserved: Hiss keeps a fixed 13-parameter schema,
/// so foreign `ParameterInfo` pointers stay stable across restore.
fn replace_hiss_from_state(handle: &mut PluginHandle, state: &[u8]) -> Result<(), String> {
    let incoming_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(state)
            .map_err(|error| format!("Failed to parse HissReducer state: {error}"))?;
    // Current live snapshot carries scalars plus the stored profile when
    // present. Contention fails explicitly; never silently drop the blob.
    let current_bytes = plugins_bridge::state::try_save_state(&*handle.plugin)
        .map_err(|error| format!("Failed to capture current HissReducer state: {error}"))?;
    let current_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&current_bytes)
            .map_err(|error| format!("Failed to parse current HissReducer state: {error}"))?;

    let mut candidate: serde_json::Value = match handle.config_json.trim() {
        "" | "null" | "{}" => serde_json::json!({}),
        config => serde_json::from_str(config)
            .map_err(|error| format!("Failed to parse HissReducer config: {error}"))?,
    };
    let candidate_object = candidate
        .as_object_mut()
        .ok_or_else(|| "HissReducer config must be a JSON object".to_string())?;

    for (key, value) in &current_state {
        candidate_object.insert(key.clone(), value.clone());
    }
    if !current_state.contains_key(plugins_bridge::state::hiss_state::CAPTURED_PROFILE_KEY) {
        candidate_object.remove(plugins_bridge::state::hiss_state::CAPTURED_PROFILE_KEY);
    }
    candidate_object.remove("learn_noise");
    candidate_object.remove("clear_profile");

    for (key, value) in &incoming_state {
        if matches!(key.as_str(), "learn_noise" | "clear_profile") {
            continue;
        }
        if key == plugins_bridge::state::hiss_state::CAPTURED_PROFILE_KEY {
            if value.is_null() {
                candidate_object.remove(key);
            } else {
                let profile: sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData =
                    serde_json::from_value(value.clone()).map_err(|error| {
                        format!("HissReducer captured_profile is malformed: {error}")
                    })?;
                profile
                    .validate()
                    .map_err(|error| format!("HissReducer captured_profile invalid: {error}"))?;
                if profile.channels != handle.input_channels {
                    return Err(format!(
                        "HissReducer captured_profile has {} channels, handle has {}",
                        profile.channels, handle.input_channels
                    ));
                }
                candidate_object.insert(key.clone(), value.clone());
            }
            continue;
        }
        if matches!(
            key.as_str(),
            "enabled"
                | "threshold_db"
                | "frequency_hz"
                | "strength"
                | "spectral_mode"
                | "use_captured_profile"
                | "curve_low"
                | "curve_mid"
                | "curve_high"
                | "link_mode"
                | "transient_guard"
        ) {
            candidate_object.insert(key.clone(), value.clone());
        }
    }
    if let Some(carried) =
        candidate_object.get(plugins_bridge::state::hiss_state::CAPTURED_PROFILE_KEY)
    {
        let profile: sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData =
            serde_json::from_value(carried.clone())
                .map_err(|error| format!("HissReducer captured_profile is malformed: {error}"))?;
        profile
            .validate()
            .map_err(|error| format!("HissReducer captured_profile invalid: {error}"))?;
        if profile.channels != handle.input_channels {
            return Err(format!(
                "HissReducer captured_profile has {} channels, handle has {}",
                profile.channels, handle.input_channels
            ));
        }
    }

    let replacement_config = serde_json::to_string(&candidate)
        .map_err(|error| format!("Failed to serialize HissReducer config: {error}"))?;
    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored HissReducer channel layout differs from the handle layout".into());
    }

    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

/// Every Ambisonics parameter is structural (order, target, weighting,
/// dual-band, algorithm). Live DSP setters reject changes; the FFI guard
/// gives the uniform restoration error.
fn is_ambisonics_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(
        plugin_type,
        "AmbisonicsDecoder" | "ambisonics_decoder"
    ) {
        return false;
    }
    matches!(
        param_id,
        "order" | "target_layout" | "max_re_weighting" | "dual_band" | "algorithm"
    )
}

/// EQ-family constructor-only keys are structural saved state, not realtime
/// parameters. They have no live setters; addressing them via
/// `plugin_set_parameter` must fail with the restoration contract, not an
/// "unknown parameter" that could be mistaken for a typo.
fn is_eq_family_constructor_structural_id(plugin_type: &str, param_id: &str) -> bool {
    if !matches!(
        plugin_type,
        "EQ" | "eq"
            | "DynamicEQ"
            | "dynamic_eq"
            | "dynamic-eq"
            | "LinearPhaseEQ"
            | "linear_phase_eq"
            | "Linear-Phase-EQ"
    ) {
        return false;
    }
    matches!(
        param_id,
        "stereo_pairs" | "filters" | "channel_filters" | "bands"
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PluginStateRestoreKind {
    Partial,
    Preset,
}

fn replace_plugin_from_state(
    handle: &mut PluginHandle,
    state: &[u8],
    restore_kind: PluginStateRestoreKind,
) -> Result<(), String> {
    if matches!(
        handle.plugin_type.as_str(),
        "DynamicEQ" | "dynamic_eq" | "dynamic-eq"
    ) {
        return replace_dynamic_eq_from_state(handle, state);
    }
    if matches!(handle.plugin_type.as_str(), "LinearPhaseEQ" | "linear_phase_eq" | "Linear-Phase-EQ") {
        return replace_linear_phase_eq_from_state(handle, state);
    }
    if matches!(handle.plugin_type.as_str(), "EQ" | "eq") {
        return replace_eq_from_state(handle, state);
    }
    if matches!(handle.plugin_type.as_str(), "DeEsser" | "de_esser") {
        return replace_de_esser_from_state(handle, state);
    }
    if matches!(
        handle.plugin_type.as_str(),
        "AmbisonicsDecoder" | "ambisonics_decoder"
    ) {
        return replace_ambisonics_from_state(handle, state);
    }
    if matches!(handle.plugin_type.as_str(), "Crossover" | "crossover") {
        return replace_crossover_from_state(handle, state, restore_kind);
    }
    if matches!(
        handle.plugin_type.as_str(),
        "BandSplit" | "band_split" | "bandsplit"
    ) {
        return replace_band_split_from_state(handle, state);
    }
    if is_hiss_plugin_type(handle.plugin_type.as_str()) {
        return replace_hiss_from_state(handle, state);
    }

    let current = plugins_bridge::state::save_state(&*handle.plugin);
    let mut replacement_config = handle.config_json.clone();
    let (current_state, incoming_state) =
        if matches!(handle.plugin_type.as_str(), "Convolution" | "convolution") {
            let mut current_values: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(&current).map_err(|error| {
                    format!("Failed to capture current Convolution state: {error}")
                })?;
            let mut incoming_values: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(state)
                    .map_err(|error| format!("Failed to parse Convolution state: {error}"))?;

            let mut config: serde_json::Value = match handle.config_json.trim() {
                "" | "null" | "{}" => serde_json::json!({}),
                config => serde_json::from_str(config)
                    .map_err(|error| format!("Failed to parse Convolution config: {error}"))?,
            };
            let config_object = config
                .as_object_mut()
                .ok_or_else(|| "Convolution config must be a JSON object".to_string())?;
            let configured_mode = config_object
                .get("true_stereo")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let current_mode = match current_values.get("true_stereo") {
                Some(serde_json::Value::Bool(enabled)) => *enabled,
                Some(_) => {
                    return Err("Current Convolution true_stereo state is not a boolean".into());
                }
                None => configured_mode,
            };
            let next_mode = match incoming_values.get("true_stereo") {
                None => current_mode,
                Some(serde_json::Value::Bool(enabled)) => *enabled,
                Some(_) => return Err("Convolution true_stereo state must be a boolean".into()),
            };

            // Structural routing is constructor state: instantiate the replacement
            // in the requested mode before replaying ordinary partial parameters.
            // If the mode changes, apply an incoming IR path to construction too;
            // otherwise a legacy two-channel IR in the old config can reject a
            // valid four-channel preset before its saved IR path is restored.
            // Leave the original config bytes alone when routing is unchanged.
            if next_mode != current_mode {
                let incoming_ir_file = incoming_values
                    .get("ir_file")
                    .filter(|value| value.is_string())
                    .cloned();
                let current_ir_file = current_values
                    .get("ir_file")
                    .filter(|value| value.is_string())
                    .cloned();
                config_object.insert("true_stereo".into(), serde_json::Value::Bool(next_mode));
                if let Some(ir_file) = incoming_ir_file.clone().or(current_ir_file) {
                    config_object.insert("ir_file".into(), ir_file);
                    current_values.remove("ir_file");
                    if incoming_ir_file.is_some() {
                        incoming_values.remove("ir_file");
                    }
                }
                replacement_config = serde_json::to_string(&config)
                    .map_err(|error| format!("Failed to serialize Convolution config: {error}"))?;
            }
            current_values.remove("true_stereo");
            incoming_values.remove("true_stereo");
            (
                serde_json::to_vec(&current_values).map_err(|error| {
                    format!("Failed to serialize current Convolution state: {error}")
                })?,
                serde_json::to_vec(&incoming_values).map_err(|error| {
                    format!("Failed to serialize incoming Convolution state: {error}")
                })?,
            )
        } else {
            (current, state.to_vec())
        };

    // Construct from the original configuration: the flat parameter snapshot
    // does not contain every resource, routing matrix, or setup option.
    // All fallible work runs on a separate instance on the control thread.
    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    // Preserve live automation when a partial preset omits a parameter. Only
    // replay values that differ from the constructor, avoiding unnecessary
    // writes to setup parameters already represented by the original config.
    load_changed_state(&mut *replacement, &current_state, &handle.plugin_type)?;
    load_changed_state(&mut *replacement, &incoming_state, &handle.plugin_type)?;

    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;

    // A setup setter may change its bus widths. The host's buffers and stored
    // layout remain fixed for the lifetime of the handle.
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored plugin channel layout differs from the handle layout".into());
    }

    // Keep ParameterMap storage alive: foreign ParameterInfo pointers remain
    // valid until plugin_destroy. A rejected preset never touches live DSP.
    handle.plugin = replacement;
    handle.config_json = replacement_config;
    // The generic path reuses the same map across commits: refresh the Speech
    // model snapshot so the realtime guard tracks the committed choice. This
    // runs on the control thread after the commit; no-ops for other types.
    handle
        .parameter_map
        .refresh_speech_model_index(&*handle.plugin);
    Ok(())
}

fn is_crossover_runtime_id(id: &str) -> bool {
    matches!(id, "type" | "frequency" | "mode" | "fir_taps")
        || id
            .strip_prefix("frequency_")
            .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
        || id
            .strip_prefix("channel_frequency_")
            .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
        || id
            .strip_prefix("channel_mode_")
            .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
}

#[derive(Debug, PartialEq, Eq)]
enum CrossoverRuntimeLayout {
    Bands {
        has_second_split: bool,
        has_third_split: bool,
    },
    PerChannel {
        channels: usize,
    },
}

fn crossover_runtime_layout(
    state: &serde_json::Map<String, serde_json::Value>,
) -> Result<CrossoverRuntimeLayout, String> {
    let mut channel_frequencies = std::collections::BTreeSet::new();
    let mut channel_modes = std::collections::BTreeSet::new();
    for id in state.keys() {
        if let Some(suffix) = id.strip_prefix("channel_frequency_")
            && let Ok(channel) = suffix.parse::<usize>()
        {
            channel_frequencies.insert(channel);
        }
        if let Some(suffix) = id.strip_prefix("channel_mode_")
            && let Ok(channel) = suffix.parse::<usize>()
        {
            channel_modes.insert(channel);
        }
    }

    if !channel_frequencies.is_empty() || !channel_modes.is_empty() {
        if state.contains_key("frequency")
            || state.contains_key("mode")
            || state.contains_key("frequency_2")
            || state.contains_key("frequency_3")
        {
            return Err("Crossover preset mixes per-channel and band runtime controls".into());
        }
        let channel_count = channel_frequencies.len();
        let expected_channels: std::collections::BTreeSet<_> = (0..channel_count).collect();
        if channel_count == 0
            || channel_frequencies != expected_channels
            || channel_modes != expected_channels
        {
            return Err("Crossover preset has an incomplete per-channel runtime layout".into());
        }
        return Ok(CrossoverRuntimeLayout::PerChannel {
            channels: channel_count,
        });
    }

    let has_second_split = state.contains_key("frequency_2");
    let has_third_split = state.contains_key("frequency_3");
    if has_third_split && !has_second_split {
        return Err("Crossover preset has a third split without a second split".into());
    }
    Ok(CrossoverRuntimeLayout::Bands {
        has_second_split,
        has_third_split,
    })
}

fn replace_crossover_from_state(
    handle: &mut PluginHandle,
    state: &[u8],
    restore_kind: PluginStateRestoreKind,
) -> Result<(), String> {
    let current_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current Crossover state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse Crossover state: {error}"))?;
    if restore_kind == PluginStateRestoreKind::Preset {
        let current_layout = crossover_runtime_layout(&current_state)?;
        let preset_layout = crossover_runtime_layout(&incoming_state)?;
        if current_layout != preset_layout {
            return Err(format!(
                "Crossover preset topology {preset_layout:?} does not match handle topology {current_layout:?}"
            ));
        }
    }
    let current_type = current_state
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Current Crossover type state must be a string".to_string())?
        .to_owned();
    let current_type = sotf_plugins::plugin_crossover::canonical_crossover_type(&current_type)?;
    let requested_type = match incoming_state.get("type") {
        Some(serde_json::Value::String(value)) => {
            sotf_plugins::plugin_crossover::canonical_crossover_type(value)?
        }
        Some(_) => return Err("Crossover type state must be a string".into()),
        None => current_type,
    };
    let requested_fir = requested_type == "LinearPhase";

    // State import is a partial merge, matching the public FFI contract.
    // Known Crossover IDs absent from this instance identify a topology change;
    // inactive FIR taps and unknown keys retain the generic loader's ignore
    // behavior unless the requested family activates that FIR control.
    for key in incoming_state.keys() {
        if is_crossover_runtime_id(key) && !current_state.contains_key(key) && key != "fir_taps" {
            return Err(format!(
                "Crossover state parameter '{key}' changes the active topology; create a new handle"
            ));
        }
    }
    let mut merged_state = current_state;
    for (key, value) in incoming_state {
        if is_crossover_runtime_id(&key)
            && (key != "fir_taps" || requested_fir || current_type == "LinearPhase")
        {
            merged_state.insert(key, value);
        }
    }
    merged_state.insert(
        "type".into(),
        serde_json::Value::String(requested_type.to_owned()),
    );

    let is_per_channel = merged_state
        .keys()
        .any(|key| key.starts_with("channel_frequency_"));
    let crossover_type = merged_state
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Crossover type state must be a string".to_string())?;

    let mut config: serde_json::Value = match handle.config_json.trim() {
        "" | "null" | "{}" => serde_json::json!({}),
        config => serde_json::from_str(config)
            .map_err(|error| format!("Failed to parse Crossover config: {error}"))?,
    };
    let config_object = config
        .as_object_mut()
        .ok_or_else(|| "Crossover config must be a JSON object".to_string())?;
    config_object.insert(
        "type".into(),
        serde_json::Value::String(crossover_type.to_owned()),
    );

    if is_per_channel {
        let mut frequencies = Vec::new();
        let mut modes = Vec::new();
        loop {
            let channel = frequencies.len();
            let frequency_id = format!("channel_frequency_{channel}");
            let Some(frequency) = merged_state.get(&frequency_id) else {
                break;
            };
            let frequency = frequency
                .as_f64()
                .ok_or_else(|| format!("Crossover {frequency_id} must be numeric"))?;
            if !frequency.is_finite() {
                return Err(format!("Crossover {frequency_id} must be finite"));
            }
            let mode_id = format!("channel_mode_{channel}");
            let mode = merged_state
                .get(&mode_id)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("Crossover {mode_id} must be a string"))?;
            frequencies.push(serde_json::Value::from(frequency));
            modes.push(serde_json::Value::String(mode.to_owned()));
        }
        if frequencies.len() != handle.input_channels {
            return Err(format!(
                "Crossover per-channel state has {} cutoffs for {} input channels",
                frequencies.len(),
                handle.input_channels
            ));
        }
        // Keep dormant global frequency/mode and multiway values from the
        // constructor config. Complete explicit channel modes select the live
        // route, so dormant `output=both` remains valid.
        config_object.insert(
            "channel_frequencies_hz".into(),
            serde_json::Value::Array(frequencies),
        );
        config_object.insert("channel_modes".into(), serde_json::Value::Array(modes));
    } else {
        let frequency = merged_state
            .get("frequency")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| "Crossover frequency state must be numeric".to_string())?;
        if !frequency.is_finite() {
            return Err("Crossover frequency state must be finite".into());
        }
        let mode = merged_state
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Crossover mode state must be a string".to_string())?;
        let mut extra_frequencies = Vec::new();
        let mut ordered_frequencies = vec![frequency];
        for index in 2..=3 {
            let id = format!("frequency_{index}");
            let Some(value) = merged_state.get(&id) else {
                break;
            };
            let value = value
                .as_f64()
                .ok_or_else(|| format!("Crossover {id} state must be numeric"))?;
            if !value.is_finite() {
                return Err(format!("Crossover {id} state must be finite"));
            }
            ordered_frequencies.push(value);
            extra_frequencies.push(serde_json::Value::from(value));
        }
        if ordered_frequencies
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("Crossover cutoff frequencies must be strictly increasing".into());
        }
        config_object.insert("frequency".into(), serde_json::Value::from(frequency));
        config_object.insert("output".into(), serde_json::Value::String(mode.to_owned()));
        config_object.insert(
            "extra_frequencies".into(),
            serde_json::Value::Array(extra_frequencies),
        );
        if crossover_type == "LinearPhase"
            && let Some(fir_taps) = merged_state.get("fir_taps")
        {
            let fir_taps = fir_taps
                .as_i64()
                .ok_or_else(|| "Crossover fir_taps state must be an integer".to_string())?;
            config_object.insert("fir_taps".into(), serde_json::Value::from(fir_taps));
        }
    }

    let replacement_config = serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize Crossover config: {error}"))?;
    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored Crossover channel layout differs from the handle layout".into());
    }

    // The runtime map changes when a restore enters or leaves FIR mode, and
    // its default values track the committed state. Keep every old map alive
    // because C callers may retain any previously returned info pointer.
    let next_parameter_map = ParameterMap::from_plugin(&*replacement, &handle.plugin_type);
    handle.retired_parameter_maps.push(std::mem::replace(
        &mut handle.parameter_map,
        next_parameter_map,
    ));
    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

fn replace_dynamic_eq_from_state(handle: &mut PluginHandle, state: &[u8]) -> Result<(), String> {
    // Partial presets merge into the full live snapshot, which includes all
    // eight slots even when fewer bands currently process audio.
    let mut merged_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current DynamicEQ state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse DynamicEQ state: {error}"))?;
    merged_state.extend(incoming_state);
    let merged_state = serde_json::to_vec(&merged_state)
        .map_err(|error| format!("Failed to merge DynamicEQ state: {error}"))?;
    let replacement_config = super::plugin_factory::merge_dynamic_eq_state_into_config(
        &handle.config_json,
        &merged_state,
    )?;

    // Construct, prepare, and initialize a separate instance. The live
    // plugin, constructor config, and parameter-map pointers remain untouched
    // until every fallible operation and layout check has succeeded.
    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored DynamicEQ channel layout differs from the handle layout".into());
    }

    // Rebuild the map so post-restore info/values match the committed plugin;
    // retired maps stay alive for previously returned info pointers.
    let next_parameter_map = ParameterMap::from_plugin(&*replacement, &handle.plugin_type);
    handle.retired_parameter_maps.push(std::mem::replace(
        &mut handle.parameter_map,
        next_parameter_map,
    ));
    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

fn replace_eq_from_state(handle: &mut PluginHandle, state: &[u8]) -> Result<(), String> {
    // Legacy raw partial-state semantics: omitted values merge from the live
    // snapshot, which carries every current band, placement, and global.
    let mut merged_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current EQ state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse EQ state: {error}"))?;
    merged_state.extend(incoming_state.clone());
    let merged_bytes = serde_json::to_vec(&merged_state)
        .map_err(|error| format!("Failed to merge EQ state: {error}"))?;
    // Constructor-owned keys (pairs, full filter vectors) fold into the
    // config; band order and advanced/Kautz entries survive verbatim unless
    // a full preset replaces the filter vector. Runtime globals replay below.
    let replacement_config =
        super::plugin_factory::merge_eq_state_into_config(&handle.config_json, &merged_bytes)?;

    // Construct, replay scalars through the plugin's own setters (which own
    // the compacted biquad mapping and placement/pair validation), then
    // prepare and initialize. The bridge loader skips the structural
    // `stereo_pairs` / `filters` / `channel_filters` keys, which have no
    // parameter setters. Every fallible step precedes the commit, so a
    // failed restore retains the old audio path and configuration.
    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    // Placement metadata reserves twenty addresses, but a configured bank
    // can contain fewer filters. The generic bridge ignores unknown keys;
    // a requested structural placement must instead fail transactionally.
    for key in incoming_state.keys() {
        if is_eq_placement_structural_id(&handle.plugin_type, key)
            && replacement.get_parameter(&ParameterId::from(key.as_str())).is_none()
        {
            return Err(format!("EQ placement '{key}' targets a filter outside the configured bank"));
        }
    }
    load_changed_state(&mut *replacement, &merged_bytes, &handle.plugin_type)?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored EQ channel layout differs from the handle layout".into());
    }

    // Bank-length changes alter live band parameters; rebuild the map like
    // Crossover so post-restore reads match the committed bank. Retired maps
    // keep old info pointers valid.
    let next_parameter_map = ParameterMap::from_plugin(&*replacement, &handle.plugin_type);
    handle.retired_parameter_maps.push(std::mem::replace(
        &mut handle.parameter_map,
        next_parameter_map,
    ));
    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

fn replace_de_esser_from_state(handle: &mut PluginHandle, state: &[u8]) -> Result<(), String> {
    // Partial loads merge omitted values from the live snapshot; structural
    // values the live setters reject (lookahead, split topology, sidechain
    // route) rebuild through the constructor config instead.
    let mut merged_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current DeEsser state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse DeEsser state: {error}"))?;
    merged_state.extend(incoming_state);
    let merged_bytes = serde_json::to_vec(&merged_state)
        .map_err(|error| format!("Failed to merge DeEsser state: {error}"))?;
    let replacement_config = super::plugin_factory::merge_de_esser_state_into_config(
        &handle.config_json,
        &merged_bytes,
    )?;

    // A sidechain-mode flip changes the bus layout (program + key versus
    // program only) and fails bus validation below with the live handle
    // preserved; the host must recreate the handle for the new layout.
    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored DeEsser channel layout differs from the handle layout".into());
    }

    // Rebuild the map so post-restore info/values match the committed plugin;
    // retired maps stay alive for previously returned info pointers.
    let next_parameter_map = ParameterMap::from_plugin(&*replacement, &handle.plugin_type);
    handle.retired_parameter_maps.push(std::mem::replace(
        &mut handle.parameter_map,
        next_parameter_map,
    ));
    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

fn replace_ambisonics_from_state(handle: &mut PluginHandle, state: &[u8]) -> Result<(), String> {
    // Every Ambisonics parameter is structural, so any changed value rebuilds
    // through the constructor config. Custom geometry rides in the config and
    // survives reloads; selecting custom without geometry fails closed.
    let mut merged_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current Ambisonics state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse Ambisonics state: {error}"))?;
    merged_state.extend(incoming_state);
    let merged_bytes = serde_json::to_vec(&merged_state)
        .map_err(|error| format!("Failed to merge Ambisonics state: {error}"))?;
    let replacement_config = super::plugin_factory::merge_ambisonics_state_into_config(
        &handle.config_json,
        &merged_bytes,
    )?;

    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;
    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored Ambisonics channel layout differs from the handle layout".into());
    }

    // Rebuild the map so post-restore info/values match the committed plugin;
    // retired maps stay alive for previously returned info pointers.
    let next_parameter_map = ParameterMap::from_plugin(&*replacement, &handle.plugin_type);
    handle.retired_parameter_maps.push(std::mem::replace(
        &mut handle.parameter_map,
        next_parameter_map,
    ));
    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

fn replace_band_split_from_state(handle: &mut PluginHandle, state: &[u8]) -> Result<(), String> {
    let mut merged_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current BandSplit state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse BandSplit state: {error}"))?;
    merged_state.extend(incoming_state);

    let choice = |key: &str| -> Result<usize, String> {
        let value = merged_state
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| format!("BandSplit {key} must be a choice index"))?;
        usize::try_from(value).map_err(|_| format!("BandSplit {key} choice is out of range"))
    };
    let cutoff = |key: &str| -> Result<f64, String> {
        let value = merged_state
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| format!("BandSplit {key} must be numeric"))?;
        if !value.is_finite() {
            return Err(format!("BandSplit {key} must be finite"));
        }
        Ok(value)
    };

    // These values are choice indices in the public parameter state and
    // constructor values in plugin configuration. Build the full active
    // cutoff vector before applying any setters, so a coordinated shift such
    // as [1000, 2000, 3000] -> [2000, 3000, 4000] never passes through an
    // invalid intermediate ordering.
    let band_count = choice("num_bands")?
        .checked_add(2)
        .filter(|count| (2..=4).contains(count))
        .ok_or_else(|| "BandSplit num_bands choice is out of range".to_string())?;
    let expected_outputs = handle
        .input_channels
        .checked_mul(band_count)
        .ok_or_else(|| "BandSplit output channel count overflow".to_string())?;
    if expected_outputs != handle.output_channels {
        return Err(format!(
            "BandSplit restore changes output geometry from {} to {}; create a new handle",
            handle.output_channels, expected_outputs
        ));
    }

    let crossover_type = match choice("type")? {
        0 => "LR24",
        1 => "LR48",
        _ => return Err("BandSplit type choice is out of range".into()),
    };
    let recombination_mode = match choice("recombination_mode")? {
        0 => "legacy_cascade",
        1 => "phase_compensated",
        _ => return Err("BandSplit recombination_mode choice is out of range".into()),
    };
    let mut frequencies = Vec::with_capacity(band_count - 1);
    for key in ["frequency", "frequency_2", "frequency_3"]
        .into_iter()
        .take(band_count - 1)
    {
        frequencies.push(cutoff(key)?);
    }

    let mut config: serde_json::Value = match handle.config_json.trim() {
        "" | "null" | "{}" => serde_json::json!({}),
        config => serde_json::from_str(config)
            .map_err(|error| format!("Failed to parse BandSplit config: {error}"))?,
    };
    let config_object = config
        .as_object_mut()
        .ok_or_else(|| "BandSplit config must be a JSON object".to_string())?;
    // `crossover_type` is a supported legacy alias for the canonical `type`
    // constructor field. Remove it before inserting the new canonical value,
    // otherwise serde sees both names for one field and rejects the rebuild.
    config_object.remove("crossover_type");
    config_object.insert(
        "frequencies".into(),
        serde_json::to_value(&frequencies)
            .map_err(|error| format!("Failed to serialize BandSplit cutoffs: {error}"))?,
    );
    config_object.insert(
        "explicit_frequencies".into(),
        serde_json::to_value(&frequencies)
            .map_err(|error| format!("Failed to serialize BandSplit explicit cutoffs: {error}"))?,
    );
    config_object.insert("num_bands".into(), serde_json::json!(band_count));
    config_object.insert("type".into(), serde_json::json!(crossover_type));
    config_object.insert(
        "recombination_mode".into(),
        serde_json::json!(recombination_mode),
    );
    // Inactive cutoffs remain visible and serializable when this same-width
    // handle later reopens a wider band configuration.
    for key in ["frequency", "frequency_2", "frequency_3"] {
        config_object.insert(key.into(), serde_json::json!(cutoff(key)?));
    }
    let replacement_config = serde_json::to_string(&config)
        .map_err(|error| format!("Failed to serialize BandSplit config: {error}"))?;

    let mut replacement = super::plugin_factory::create_unprepared_plugin(
        &handle.plugin_type,
        &replacement_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
    )?;
    // Reapply non-constructor controls such as band gains. Structural and
    // cutoff values already match the replacement constructor, so the
    // changed-only loader will not replay them in an unsafe order.
    let merged_state = serde_json::to_vec(&merged_state)
        .map_err(|error| format!("Failed to serialize merged BandSplit state: {error}"))?;
    load_changed_state(&mut *replacement, &merged_state, &handle.plugin_type)?;
    replacement =
        plugins_bridge::prepare_standalone_plugin(replacement, handle.max_callback_frames)?;
    replacement.initialize(handle.sample_rate)?;

    if replacement.input_channels() != handle.input_channels
        || replacement.output_channels() != handle.output_channels
    {
        return Err("Restored plugin channel layout differs from the handle layout".into());
    }

    handle.plugin = replacement;
    handle.config_json = replacement_config;
    Ok(())
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;

#[cfg(test)]
#[path = "ffi_integration_tests.rs"]
mod ffi_integration_tests;

#[cfg(test)]
#[path = "hiss_state_tests.rs"]
mod hiss_state_tests;

#[cfg(test)]
#[path = "crossfeed_yaw_ffi_tests.rs"]
mod crossfeed_yaw_ffi_tests;

#[cfg(test)]
#[path = "compressor_detector_ffi_tests.rs"]
mod compressor_detector_ffi_tests;

fn replace_linear_phase_eq_from_state(
    handle: &mut PluginHandle,
    state: &[u8],
) -> Result<(), String> {
    // Match the generic state loader's partial-update contract: values omitted
    // from the incoming object retain their current live values, including
    // automation applied after the last structural reconstruction.
    let mut merged_state: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plugins_bridge::state::save_state(&*handle.plugin))
            .map_err(|error| format!("Failed to capture current LinearPhaseEQ state: {error}"))?;
    let incoming_state: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(state)
        .map_err(|error| format!("Failed to parse LinearPhaseEQ state: {error}"))?;
    if let Some(num_filters) = incoming_state
        .get("num_filters")
        .and_then(serde_json::Value::as_u64)
    {
        // A smaller incoming structure replaces, rather than preserves, the
        // now-out-of-range live bands. Explicit incoming out-of-range band
        // fields remain in the overlay and are rejected by constructor merge.
        merged_state.retain(|key, _| {
            key.strip_prefix("band_")
                .and_then(|rest| rest.split_once('_'))
                .and_then(|(index, _)| index.parse::<u64>().ok())
                .is_none_or(|index| index < num_filters)
        });
    }
    merged_state.extend(incoming_state);
    let merged_state = serde_json::to_vec(&merged_state)
        .map_err(|error| format!("Failed to merge LinearPhaseEQ state: {error}"))?;
    let rebuilt_config = super::plugin_factory::merge_linear_phase_eq_state_into_config(
        &handle.config_json,
        &merged_state,
    )?;
    let mut replacement = super::plugin_factory::create_plugin_with_max_callback(
        &handle.plugin_type,
        &rebuilt_config,
        handle.input_channels,
        handle.output_channels,
        handle.sample_rate,
        handle.max_callback_frames,
    )?;
    replacement.initialize(handle.sample_rate)?;

    // LinearPhaseEQ always exposes the same ten-band schema, including
    // dormant slots. Keep its map and borrowed metadata pointers stable;
    // parameter reads use the replacement DSP instance below. All fallible
    // preparation precedes the commit.
    handle.plugin = replacement;
    handle.config_json = rebuilt_config;
    Ok(())
}

/// Get the last error message.
///
/// # Returns
/// * Pointer to a null-terminated C string, or `NULL` if no error has been set.
/// * The pointer points to thread-local storage that remains valid until the
///   next FFI call on the same thread that may set an error. Copy the contents
///   if you need it to outlive the next call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_last_error() -> *const c_char {
    let static_error = LAST_STATIC_ERROR.with(std::cell::Cell::get);
    if !static_error.is_null() {
        return static_error;
    }
    LAST_ERROR.with(|e| {
        e.borrow()
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(ptr::null())
    })
}

/// Get the stable ABI version for this FFI surface.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_ffi_abi_version() -> u32 {
    SOTF_PLUGIN_FFI_ABI_VERSION
}

/// Get runtime capabilities for the current target.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_ffi_capabilities() -> PluginFfiCapabilities {
    PluginFfiCapabilities {
        abi_version: SOTF_PLUGIN_FFI_ABI_VERSION,
        host_kind: current_host_kind(),
        supports_audio: true,
        supports_parameters: true,
        supports_state: true,
        supports_midi_input: true,
        supports_midi_output: true,
        supports_note_expression: true,
        supports_apple_au_v3: cfg!(any(target_os = "macos", target_os = "ios")),
        supports_ios_au_v3: cfg!(target_os = "ios"),
        supports_windows_vst3: cfg!(target_os = "windows"),
        supports_swift_package: cfg!(any(target_os = "macos", target_os = "ios")),
        supports_preset_documents: true,
    }
}

/// Get machine-readable platform and capability metadata as JSON.
///
/// # Returns
/// * JSON string owned by the caller. It must be released with
///   [`plugin_free_string`] when no longer needed.
/// * `NULL` on error.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_ffi_platform_info_json() -> *mut c_char {
    let caps = plugin_ffi_capabilities();
    let json = serde_json::json!({
        "abi_version": caps.abi_version,
        "target_os": std::env::consts::OS,
        "target_arch": std::env::consts::ARCH,
        "host_kind": host_kind_name(caps.host_kind),
        "supports_audio": caps.supports_audio,
        "supports_parameters": caps.supports_parameters,
        "supports_state": caps.supports_state,
        "supports_midi_input": caps.supports_midi_input,
        "supports_midi_output": caps.supports_midi_output,
        "supports_note_expression": caps.supports_note_expression,
        "supports_apple_au_v3": caps.supports_apple_au_v3,
        "supports_ios_au_v3": caps.supports_ios_au_v3,
        "supports_windows_vst3": caps.supports_windows_vst3,
        "supports_swift_package": caps.supports_swift_package,
        "supports_preset_documents": caps.supports_preset_documents,
    });

    match CString::new(json.to_string()) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Get preset document metadata for host file dialogs and AUv3 document state.
///
/// The returned string pointers reference static, null-terminated C strings
/// with program lifetime. They must not be freed.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_preset_document_info() -> PluginPresetDocumentInfo {
    PluginPresetDocumentInfo {
        schema_version: 1,
        ut_type: PRESET_UT_TYPE.as_ptr().cast(),
        file_extension: PRESET_FILE_EXTENSION.as_ptr().cast(),
        mime_type: PRESET_MIME_TYPE.as_ptr().cast(),
        supports_full_state_for_document: cfg!(any(target_os = "macos", target_os = "ios")),
        supports_security_scoped_bookmarks: cfg!(target_os = "macos"),
    }
}

/// Get Windows/VST3 FFI metadata for native language bindings.
///
/// The returned string pointers reference static, null-terminated C strings
/// with program lifetime. They must not be freed.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_vst3_ffi_descriptor() -> PluginVst3FfiDescriptor {
    PluginVst3FfiDescriptor {
        abi_version: SOTF_PLUGIN_FFI_ABI_VERSION,
        // Stable namespace UUID: "SOTF-FFI-VST3-01" bytes.
        class_id: *b"SOTF-FFI-VST3-01",
        component_name: VST3_COMPONENT_NAME.as_ptr().cast(),
        vendor: VST3_VENDOR.as_ptr().cast(),
        sdk_version: VST3_SDK_VERSION.as_ptr().cast(),
        entrypoint: VST3_ENTRYPOINT.as_ptr().cast(),
        supports_com_factory: false,
        supports_audio_effects: true,
        supports_instruments: true,
        supports_midi_output: true,
        supports_note_expression: true,
    }
}

/// Get Swift Package metadata for Xcode/SwiftPM integrations.
///
/// The returned string pointers reference static, null-terminated C strings
/// with program lifetime. They must not be freed.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_swift_package_info() -> PluginSwiftPackageInfo {
    PluginSwiftPackageInfo {
        package_name: SWIFT_PACKAGE_NAME.as_ptr().cast(),
        product_name: SWIFT_PRODUCT_NAME.as_ptr().cast(),
        target_name: SWIFT_TARGET_NAME.as_ptr().cast(),
        library_name: SWIFT_LIBRARY_NAME.as_ptr().cast(),
        umbrella_header: SWIFT_HEADER_NAME.as_ptr().cast(),
        supports_staticlib: true,
        supports_xcframework: true,
    }
}

/// Create a new plugin instance
///
/// # Arguments
/// * `plugin_type` - Plugin type name (e.g., "EQ", "Compressor")
/// * `config_json` - JSON configuration string
/// * `sample_rate` - Sample rate in Hz
/// * `input_channels` - Number of input channels
/// * `output_channels` - Number of output channels
///
/// # Returns
/// * Opaque plugin handle on success
/// * NULL on failure (check plugin_get_last_error())
///
/// # Safety
/// * `plugin_type` and `config_json` must be valid, null-terminated UTF-8 C strings
///   that remain readable for the duration of this call.
/// * The returned handle is owned by the caller and must be released with
///   [`plugin_destroy`] exactly once. The pointer is invalid after that call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_create(
    plugin_type: *const c_char,
    config_json: *const c_char,
    sample_rate: u32,
    input_channels: usize,
    output_channels: usize,
) -> *mut PluginHandle {
    // Catch panics and return NULL
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // Validate pointers
        if plugin_type.is_null() || config_json.is_null() {
            set_last_error("NULL pointer passed to plugin_create");
            return ptr::null_mut();
        }

        // Convert C strings to Rust
        let plugin_type_str = unsafe {
            match CStr::from_ptr(plugin_type).to_str() {
                Ok(s) => s,
                Err(_) => {
                    set_last_error("Invalid UTF-8 in plugin_type");
                    return ptr::null_mut();
                }
            }
        };

        let config_str = unsafe {
            match CStr::from_ptr(config_json).to_str() {
                Ok(s) => s,
                Err(_) => {
                    set_last_error("Invalid UTF-8 in config_json");
                    return ptr::null_mut();
                }
            }
        };

        // Canonicalize direct-format aliases once so factory policy,
        // parameter metadata, serialized identity, and adapter selection
        // cannot disagree.
        let plugin_type_str = super::plugin_factory::canonical_direct_plugin_type(plugin_type_str);

        let max_callback_frames =
            match super::plugin_factory::max_callback_frames_from_config(config_str) {
                Ok(value) => value,
                Err(error) => {
                    set_last_error(&error);
                    return ptr::null_mut();
                }
            };

        // Create plugin
        let mut plugin = match super::plugin_factory::create_plugin_with_max_callback(
            plugin_type_str,
            config_str,
            input_channels,
            output_channels,
            sample_rate,
            max_callback_frames,
        ) {
            Ok(p) => p,
            Err(e) => {
                set_last_error(&format!("Failed to create plugin: {}", e));
                return ptr::null_mut();
            }
        };

        // Initialize plugin
        if let Err(e) = plugin.initialize(sample_rate) {
            set_last_error(&format!("Failed to initialize plugin: {}", e));
            return ptr::null_mut();
        }

        // Build parameter map
        let parameter_map = ParameterMap::from_plugin(&*plugin, plugin_type_str);

        // Create handle
        let handle = Box::new(PluginHandle {
            plugin,
            plugin_type: plugin_type_str.to_string(),
            config_json: config_str.to_string(),
            parameter_map,
            retired_parameter_maps: Vec::new(),
            sample_rate,
            max_callback_frames,
            input_channels,
            output_channels,
            midi_output_events: Vec::with_capacity(MAX_FFI_OUTPUT_EVENTS_PER_BLOCK),
            note_expression_output_events: Vec::with_capacity(MAX_FFI_OUTPUT_EVENTS_PER_BLOCK),
        });

        Box::into_raw(handle)
    }));

    result.unwrap_or_else(|_| {
        set_last_error("Panic occurred in plugin_create");
        ptr::null_mut()
    })
}

/// Destroy a plugin instance
///
/// # Safety
/// * handle must be a valid pointer returned by plugin_create()
/// * handle must not be used after this call
#[unsafe(no_mangle)]
pub extern "C" fn plugin_destroy(handle: *mut PluginHandle) {
    if !handle.is_null() {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(handle));
        }));
    }
}

/// Reset plugin state (clear buffers, reset filters)
///
/// # Safety
/// * handle must be a valid pointer returned by plugin_create()
#[unsafe(no_mangle)]
pub extern "C" fn plugin_reset(handle: *mut PluginHandle) -> c_int {
    if handle.is_null() {
        set_last_error("NULL handle in plugin_reset");
        return PluginError::InvalidHandle.into();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        handle_ref.plugin.reset();
        PluginError::Success
    }));

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_reset");
            PluginError::UnknownError
        })
        .into()
}

/// Process audio samples
///
/// # Arguments
/// * `handle` - Plugin handle
/// * `input` - Interleaved input samples [C0_F0, C1_F0, ..., C0_F1, C1_F1, ...]
/// * `output` - Interleaved output buffer (will be filled)
/// * `num_frames` - Number of frames to process
///
/// # Returns
/// * 0 on success
/// * Error code on failure
///
/// # Safety
/// * `handle` must be a valid plugin handle returned by [`plugin_create`] that
///   has not been destroyed.
/// * `input` and `output` must be valid, properly aligned, non-overlapping
///   buffers that remain valid for the duration of this call.
/// * `input` must contain at least `num_frames * input_channels` samples and
///   `output` must have space for at least `num_frames * output_channels`
///   samples, where `input_channels` and `output_channels` are the values
///   passed to [`plugin_create`].
/// * This function is designed to be real-time safe (no allocations).
#[unsafe(no_mangle)]
pub extern "C" fn plugin_process(
    handle: *mut PluginHandle,
    input: *const f32,
    output: *mut f32,
    num_frames: usize,
) -> c_int {
    if handle.is_null() || input.is_null() || output.is_null() {
        return PluginError::NullPointer.into();
    }

    // Real-time processing: avoid panic catching overhead in release builds
    #[cfg(debug_assertions)]
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let context = ProcessContext::new((*handle).sample_rate, num_frames);
        process_impl(handle, input, output, num_frames, &context)
    }));

    #[cfg(not(debug_assertions))]
    let result: Result<PluginError, ()> = Ok(unsafe {
        let context = ProcessContext::new((*handle).sample_rate, num_frames);
        process_impl(handle, input, output, num_frames, &context)
    });

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_process");
            PluginError::ProcessingFailed
        })
        .into()
}

/// Process audio samples with incoming MIDI events.
///
/// MIDI events are copied into a fixed stack buffer, then borrowed by
/// ProcessContext. No heap allocation occurs on the render path.
///
/// # Safety
/// * `handle`, `input`, and `output` must satisfy the same contract as
///   [`plugin_process`].
/// * `midi_events` must point to `midi_event_count` valid [`PluginMidiEvent`]
///   structs and remain readable for the duration of this call. It may be
///   `NULL` only when `midi_event_count` is `0`.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_process_with_midi(
    handle: *mut PluginHandle,
    input: *const f32,
    output: *mut f32,
    num_frames: usize,
    midi_events: *const PluginMidiEvent,
    midi_event_count: usize,
) -> c_int {
    if handle.is_null()
        || input.is_null()
        || output.is_null()
        || (midi_events.is_null() && midi_event_count > 0)
    {
        return PluginError::NullPointer.into();
    }

    #[cfg(debug_assertions)]
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        process_with_ffi_events_impl(
            handle,
            input,
            output,
            num_frames,
            midi_events,
            midi_event_count,
        )
    }));

    #[cfg(not(debug_assertions))]
    let result: Result<PluginError, ()> = Ok(unsafe {
        process_with_ffi_events_impl(
            handle,
            input,
            output,
            num_frames,
            midi_events,
            midi_event_count,
        )
    });

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_process_with_midi");
            PluginError::ProcessingFailed
        })
        .into()
}

/// Process audio with MIDI/note-expression input and output ABI slots.
///
/// Incoming MIDI is bridged into ProcessContext. Queued MIDI output and Note
/// Expression events are copied into host-provided buffers without allocating
/// on the render path.
///
/// # Safety
/// * `handle`, `input`, and `output` must satisfy the same contract as
///   [`plugin_process`].
/// * `midi_input` must point to `midi_input_count` valid [`PluginMidiEvent`]
///   structs and remain readable for the duration of this call. It may be
///   `NULL` only when `midi_input_count` is `0`.
/// * `note_expression_input` must point to `note_expression_input_count` valid
///   [`PluginNoteExpressionEvent`] structs and remain readable for the duration
///   of this call. It may be `NULL` only when `note_expression_input_count` is `0`.
/// * If queued output events exist, `midi_output` and `note_expression_output`
///   must be valid writable buffers with capacities matching the supplied
///   `_capacity` values and remain writable for the duration of this call.
/// * `midi_output_count` and `note_expression_output_count` must be valid
///   pointers to `usize` values that this call may write.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_process_with_events(
    handle: *mut PluginHandle,
    input: *const f32,
    output: *mut f32,
    num_frames: usize,
    midi_input: *const PluginMidiEvent,
    midi_input_count: usize,
    note_expression_input: *const PluginNoteExpressionEvent,
    note_expression_input_count: usize,
    _midi_output: *mut PluginMidiEvent,
    _midi_output_capacity: usize,
    midi_output_count: *mut usize,
    _note_expression_output: *mut PluginNoteExpressionEvent,
    _note_expression_output_capacity: usize,
    note_expression_output_count: *mut usize,
) -> c_int {
    if !midi_output_count.is_null() {
        unsafe {
            *midi_output_count = 0;
        }
    }
    if !note_expression_output_count.is_null() {
        unsafe {
            *note_expression_output_count = 0;
        }
    }

    if note_expression_input.is_null() && note_expression_input_count > 0 {
        return PluginError::NullPointer.into();
    }
    if note_expression_input_count > MAX_FFI_MIDI_EVENTS_PER_BLOCK {
        set_last_error("Too many Note Expression events for one FFI processing block");
        return PluginError::BufferTooSmall.into();
    }

    if handle.is_null()
        || input.is_null()
        || output.is_null()
        || (midi_input.is_null() && midi_input_count > 0)
    {
        return PluginError::NullPointer.into();
    }

    #[cfg(debug_assertions)]
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        process_with_full_events_impl(
            handle,
            input,
            output,
            num_frames,
            midi_input,
            midi_input_count,
            note_expression_input,
            note_expression_input_count,
            _midi_output,
            _midi_output_capacity,
            midi_output_count,
            _note_expression_output,
            _note_expression_output_capacity,
            note_expression_output_count,
        )
    }));

    #[cfg(not(debug_assertions))]
    let result: Result<PluginError, ()> = Ok(unsafe {
        process_with_full_events_impl(
            handle,
            input,
            output,
            num_frames,
            midi_input,
            midi_input_count,
            note_expression_input,
            note_expression_input_count,
            _midi_output,
            _midi_output_capacity,
            midi_output_count,
            _note_expression_output,
            _note_expression_output_capacity,
            note_expression_output_count,
        )
    });

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_process_with_events");
            PluginError::ProcessingFailed
        })
        .into()
}

/// Clear queued MIDI and Note Expression output events.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_clear_output_events(handle: *mut PluginHandle) -> c_int {
    if handle.is_null() {
        return PluginError::InvalidHandle.into();
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        handle_ref.midi_output_events.clear();
        handle_ref.note_expression_output_events.clear();
        PluginError::Success
    }));
    result.unwrap_or(PluginError::UnknownError).into()
}

/// Queue one outgoing MIDI event for the next event-aware process call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_enqueue_midi_output_event(
    handle: *mut PluginHandle,
    event: PluginMidiEvent,
) -> c_int {
    if handle.is_null() {
        return PluginError::InvalidHandle.into();
    }
    if event.len > 3 {
        set_last_error("Invalid outgoing MIDI event length");
        return PluginError::InvalidConfig.into();
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        if handle_ref.midi_output_events.len() >= MAX_FFI_OUTPUT_EVENTS_PER_BLOCK {
            set_last_error("MIDI output queue is full");
            return PluginError::BufferTooSmall;
        }
        handle_ref.midi_output_events.push(event);
        PluginError::Success
    }));
    result.unwrap_or(PluginError::UnknownError).into()
}

/// Queue one outgoing Note Expression event for the next event-aware process call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_enqueue_note_expression_output_event(
    handle: *mut PluginHandle,
    event: PluginNoteExpressionEvent,
) -> c_int {
    if handle.is_null() {
        return PluginError::InvalidHandle.into();
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        if handle_ref.note_expression_output_events.len() >= MAX_FFI_OUTPUT_EVENTS_PER_BLOCK {
            set_last_error("Note Expression output queue is full");
            return PluginError::BufferTooSmall;
        }
        handle_ref.note_expression_output_events.push(event);
        PluginError::Success
    }));
    result.unwrap_or(PluginError::UnknownError).into()
}

/// Copy queued outgoing MIDI events without processing audio.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_midi_output_events(
    handle: *mut PluginHandle,
    out: *mut PluginMidiEvent,
    capacity: usize,
    out_count: *mut usize,
) -> c_int {
    if handle.is_null() {
        return PluginError::InvalidHandle.into();
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        copy_midi_output_events(&mut handle_ref.midi_output_events, out, capacity, out_count)
    }));
    result.unwrap_or(PluginError::UnknownError).into()
}

/// Copy queued outgoing Note Expression events without processing audio.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_note_expression_output_events(
    handle: *mut PluginHandle,
    out: *mut PluginNoteExpressionEvent,
    capacity: usize,
    out_count: *mut usize,
) -> c_int {
    if handle.is_null() {
        return PluginError::InvalidHandle.into();
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        copy_note_expression_output_events(
            &mut handle_ref.note_expression_output_events,
            out,
            capacity,
            out_count,
        )
    }));
    result.unwrap_or(PluginError::UnknownError).into()
}

/// Get the number of parameters
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_parameter_count(handle: *const PluginHandle) -> c_int {
    if handle.is_null() {
        return 0;
    }

    unsafe {
        let handle_ref = &*handle;
        handle_ref.parameter_map.count() as c_int
    }
}

/// Get parameter info by index.
///
/// # Returns
/// * Pointer to parameter info. The pointer is valid only while the
///   `PluginHandle` used to obtain it remains alive and has not been destroyed.
/// * `NULL` if the handle is invalid or the index is out of bounds.
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_parameter_info(
    handle: *const PluginHandle,
    index: usize,
) -> *const ParameterInfo {
    if handle.is_null() {
        return ptr::null();
    }

    unsafe {
        let handle_ref = &*handle;
        handle_ref
            .parameter_map
            .get_info(index)
            .map(|info| info as *const ParameterInfo)
            .unwrap_or(ptr::null())
    }
}

/// Get one choice label for a parameter in its enumerated ABI position.
///
/// The returned pointer references a static NUL-terminated string and is
/// valid for the process lifetime. It is `NULL` when the index does not
/// identify a supported choice parameter or when `choice_index` is invalid.
///
/// Placement index semantics differ by family under the same `_placement`
/// suffix: EQ/Linear use 0=Legacy/inherit, 1=Stereo, 2=Left, 3=Right, 4=Mid,
/// 5=Side (6 labels); DynamicEQ uses 0=Stereo, 1=Left, 2=Right, 3=Mid, 4=Side
/// (5 labels, no Legacy). A generic host must branch on the plugin family
/// before interpreting index 0. DynamicEQ shape index 3 is Tilt, matching
/// the DSP `DynEqShape` order.
///
/// Speech Denoiser `model` exposes 3 labels in registry order: 0=`RNNoise
/// Full`, 1=`RNNoise Legacy LQ`, 2=`RNNoise Legacy SH`, matching the DSP
/// `MODEL_LABELS` and the 0..=2 parameter range. Any other `choice_index`
/// returns `NULL`, as do the non-choice `enabled`/`strength` parameters.
///
/// # Safety
/// * `handle` must be `NULL` or a live plugin handle.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_parameter_choice_label(
    handle: *const PluginHandle,
    parameter_index: usize,
    choice_index: usize,
) -> *const c_char {
    if handle.is_null() {
        return ptr::null();
    }

    // SAFETY: the caller promises a live handle; the method only reads its
    // immutable parameter map and returns a process-static string pointer.
    unsafe {
        (&*handle)
            .parameter_map
            .choice_label_at(parameter_index, choice_index)
            .unwrap_or(ptr::null())
    }
}

/// Set a parameter value (normalized 0.0-1.0).
///
/// # Arguments
/// * `handle` - Plugin handle
/// * `param_id` - Parameter ID string
/// * `normalized_value` - Normalized value (0.0 = min, 1.0 = max)
///
/// # Returns
/// * 0 on success
/// * Error code on failure
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `param_id` must be a valid, null-terminated UTF-8 C string that remains
///   readable for the duration of this call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_set_parameter(
    handle: *mut PluginHandle,
    param_id: *const c_char,
    normalized_value: c_double,
) -> c_int {
    if handle.is_null() || param_id.is_null() {
        set_last_error("NULL pointer in plugin_set_parameter");
        return PluginError::NullPointer.into();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;

        let param_id_str = match CStr::from_ptr(param_id).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error("Invalid UTF-8 in param_id");
                return PluginError::InvalidUtf8;
            }
        };

        if is_dynamic_eq_shelf_structural_id(&handle_ref.plugin_type, param_id_str) {
            set_last_error_static(
                c"Dynamic EQ structural shelf shape, slope, and placement require state restoration",
            );
            return PluginError::InvalidParameter;
        }
        if is_eq_placement_structural_id(&handle_ref.plugin_type, param_id_str) {
            set_last_error_static(c"EQ structural filter placement requires state restoration");
            return PluginError::InvalidParameter;
        }
        if is_linear_phase_eq_placement_structural_id(&handle_ref.plugin_type, param_id_str) {
            set_last_error_static(
                c"LinearPhaseEQ structural band placement requires state restoration",
            );
            return PluginError::InvalidParameter;
        }
        if is_de_esser_structural_id(&handle_ref.plugin_type, param_id_str) {
            // Repeating an already committed structural choice is a no-op,
            // matching legacy C/AU callers that synchronize all defaults.
            // The committed values were snapshotted on the control thread at
            // construction/restore; this probe never queries the live DSP, so
            // the `String` choices (mode, split_topology) stay
            // allocation-free here while keeping their no-op contract.
            if handle_ref.parameter_map.de_esser_structural_normalized(param_id_str)
                == Some(normalized_value)
            {
                return PluginError::Success;
            }
            set_last_error_static(
                c"DeEsser structural parameters require state restoration",
            );
            return PluginError::InvalidParameter;
        }
        if is_speech_structural_id(&handle_ref.plugin_type, param_id_str) {
            // Repeating the committed model is a no-op, matching hosts that
            // synchronize every parameter; anything else refuses here, before
            // the generic path can format its allocating error. The committed
            // index was snapshotted on the control thread at construction and
            // refreshed on every restore commit, so this probe never queries
            // the live DSP and cannot allocate.
            if handle_ref.parameter_map.speech_model_index()
                == Some(speech_model_index_from_normalized(normalized_value))
            {
                return PluginError::Success;
            }
            set_last_error_static(c"Speech Denoiser model changes require state restoration");
            return PluginError::InvalidParameter;
        }
        if is_hiss_structural_id(&handle_ref.plugin_type, param_id_str) {
            set_last_error_static(
                c"HissReducer structural parameters require state restoration",
            );
            return PluginError::InvalidParameter;
        }
        if is_ambisonics_structural_id(&handle_ref.plugin_type, param_id_str) {
            set_last_error_static(
                c"Ambisonics structural parameters require state restoration",
            );
            return PluginError::InvalidParameter;
        }
        if is_eq_family_constructor_structural_id(&handle_ref.plugin_type, param_id_str) {
            set_last_error_static(
                c"EQ-family structural constructor state requires state restoration",
            );
            return PluginError::InvalidParameter;
        }

        match handle_ref.parameter_map.set_normalized(
            &mut *handle_ref.plugin,
            param_id_str,
            normalized_value,
        ) {
            Ok(_) => PluginError::Success,
            Err(e) => {
                set_last_error(&format!("Failed to set parameter: {}", e));
                PluginError::InvalidParameter
            }
        }
    }));

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_set_parameter");
            PluginError::UnknownError
        })
        .into()
}

/// Get a parameter value (normalized 0.0-1.0).
///
/// # Returns
/// * Normalized value on success
/// * -1.0 on error
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `param_id` must be a valid, null-terminated UTF-8 C string that remains
///   readable for the duration of this call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_parameter(
    handle: *const PluginHandle,
    param_id: *const c_char,
) -> c_double {
    if handle.is_null() || param_id.is_null() {
        return -1.0;
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &*handle;

        let param_id_str = match CStr::from_ptr(param_id).to_str() {
            Ok(s) => s,
            Err(_) => return -1.0,
        };

        handle_ref
            .parameter_map
            .get_normalized(&*handle_ref.plugin, param_id_str)
            .unwrap_or(-1.0)
    }));

    result.unwrap_or(-1.0)
}

/// Get plugin information as a JSON string.
///
/// # Returns
/// * JSON string owned by the caller. It must be released with
///   [`plugin_free_string`] when no longer needed.
/// * `NULL` on error.
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_get_info_json(handle: *const PluginHandle) -> *mut c_char {
    if handle.is_null() {
        return ptr::null_mut();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &*handle;
        let info = handle_ref.plugin.info();

        let json = serde_json::json!({
            "name": info.name,
            "version": info.version,
            "author": info.author,
            "description": info.description,
            "input_channels": handle_ref.input_channels,
            "output_channels": handle_ref.output_channels,
            "latency_samples": handle_ref.plugin.latency_samples(),
        });

        match CString::new(json.to_string()) {
            Ok(s) => s.into_raw(),
            Err(_) => ptr::null_mut(),
        }
    }));

    result.unwrap_or(ptr::null_mut())
}

/// Free a string returned by the plugin.
///
/// # Safety
/// * `s` must be either `NULL` or a pointer previously returned by a function
///   documented as returning an owned string (for example [`plugin_get_info_json`],
///   [`plugin_ffi_platform_info_json`], or [`plugin_suggest_preset_filename`]).
/// * The pointer must not be freed more than once.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

/// Save plugin state to a JSON byte buffer.
///
/// Hiss saves include the exact `captured_profile` blob when present and
/// fail explicitly on snapshot contention: contention returns `NULL` with
/// a busy diagnostic and `out_len` set to 0, never a truncated success.
///
/// # Returns
/// * Pointer to an allocated buffer owned by the caller on success. It must be
///   released with [`plugin_free_state`] when no longer needed.
/// * `NULL` on error, with `out_len` set to 0 when it is writable.
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `out_len` must be a valid pointer to `usize` that this call may write.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_save_state(handle: *const PluginHandle, out_len: *mut usize) -> *mut u8 {
    if handle.is_null() || out_len.is_null() {
        return ptr::null_mut();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &*handle;
        let state = match save_state_with_eq_family_pairs(handle_ref) {
            Ok(state) => state,
            Err(error) => {
                set_last_error(&error);
                *out_len = 0;
                return ptr::null_mut();
            }
        };
        let len = state.len();
        let ptr = state.as_ptr();

        // Allocate and copy to a buffer the caller can free
        let buf = libc_malloc(len);
        if buf.is_null() {
            *out_len = 0;
            return ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(ptr, buf, len);
        *out_len = len;
        buf
    }));

    match result {
        Ok(buf) => buf,
        Err(_) => {
            set_last_error("Panic in plugin_save_state");
            unsafe {
                *out_len = 0;
            }
            ptr::null_mut()
        }
    }
}

/// Save plugin state, injecting the constructor-held pair list for EQ-family
/// plugins (`EQ`, `DynamicEQ`, `LinearPhaseEQ`).
///
/// Pair lists are structural saved state, not realtime parameters, so the
/// flat parameter snapshot cannot carry them. The handle config tracks the
/// current pairs across restores; injecting them here makes saved presets
/// retain pair routing. Handles without explicit pairs export no key, which
/// the merge functions treat as "retain current". Injection never fails the
/// save: an unparseable config or snapshot falls back to the plain snapshot
/// and emits a `log::warn!` naming the plugin type, so a corrupt handle
/// cannot silently shed pair routing across many saves.
///
/// Hiss contention fails explicitly: a busy snapshot returns `Err` so the
/// C callers return null with a busy diagnostic instead of persisting an
/// apparently valid state that omits a captured profile.
fn save_state_with_eq_family_pairs(handle: &PluginHandle) -> Result<Vec<u8>, String> {
    let state = plugins_bridge::state::try_save_state(&*handle.plugin)?;
    if !matches!(
        handle.plugin_type.as_str(),
        "EQ" | "eq" | "DynamicEQ" | "dynamic_eq" | "dynamic-eq"
            | "LinearPhaseEQ" | "linear_phase_eq" | "Linear-Phase-EQ"
    ) {
        return Ok(state);
    }
    let parsed_config = serde_json::from_str::<serde_json::Value>(&handle.config_json);
    let pairs = parsed_config
        .as_ref()
        .ok()
        .and_then(|config| config.get("stereo_pairs").cloned());
    if parsed_config.is_err() {
        log::warn!(
            "FFI save_state: {} handle has unparseable config_json; exporting plain snapshot without pair routing",
            handle.plugin_type
        );
        return Ok(state);
    }
    let Some(pairs) = pairs else {
        return Ok(state);
    };
    let Ok(mut map) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&state)
    else {
        log::warn!(
            "FFI save_state: {} snapshot is not a JSON object; exporting without injected pair routing",
            handle.plugin_type
        );
        return Ok(state);
    };
    map.insert("stereo_pairs".to_string(), pairs);
    Ok(
        serde_json::to_vec(&serde_json::Value::Object(map)).unwrap_or_else(|_| {
            log::warn!(
                "FFI save_state: {} pair injection serialization failed; exporting plain snapshot",
                handle.plugin_type
            );
            state
        }),
    )
}

/// Load plugin state from a JSON byte buffer.
///
/// Restoration is transactional: failure leaves the live instance unchanged.
/// Success replaces the DSP instance, restarting its processing history while
/// retaining constructor configuration and parameter values omitted by `data`.
/// This operation may allocate, load resources, and stop worker threads.
///
/// # Returns
/// * 0 on success
/// * Error code on failure
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `data` must point to `len` bytes of valid JSON that remain readable for
///   the duration of this call.
/// * Call on a control thread with no concurrent access to `handle`, including
///   audio processing or parameter access.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_load_state(
    handle: *mut PluginHandle,
    data: *const u8,
    len: usize,
) -> c_int {
    if handle.is_null() || data.is_null() {
        set_last_error("NULL pointer in plugin_load_state");
        return PluginError::NullPointer.into();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        let slice = slice::from_raw_parts(data, len);

        let load_result =
            replace_plugin_from_state(handle_ref, slice, PluginStateRestoreKind::Partial);
        match load_result {
            Ok(_) => PluginError::Success,
            Err(e) => {
                set_last_error(&format!("Failed to load state: {e}"));
                PluginError::InvalidConfig
            }
        }
    }));

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_load_state");
            PluginError::UnknownError
        })
        .into()
}

/// Control Hiss capture from the control thread.
///
/// Starts, cancels, or clears a noise-profile capture on a Hiss handle.
/// `action` must be [`HISS_CAPTURE_CANCEL`], [`HISS_CAPTURE_START`], or
/// [`HISS_CLEAR_PROFILE`]. State restoration never triggers these commands;
/// this explicit control distinguishes user intent from preset recall.
/// The realtime [`plugin_set_parameter`] refusal for `learn_noise` and
/// `clear_profile` is unchanged.
///
/// # Returns
/// * 0 on success
/// * Error code on failure
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * Call on a control thread with no concurrent access to `handle`,
///   including audio processing or parameter access.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_hiss_capture_control(
    handle: *mut PluginHandle,
    action: c_int,
) -> c_int {
    if handle.is_null() {
        set_last_error("NULL handle in plugin_hiss_capture_control");
        return PluginError::NullPointer.into();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        if !is_hiss_plugin_type(&handle_ref.plugin_type) {
            set_last_error("Hiss capture control requires a HissReducer handle");
            return PluginError::UnsupportedFeature;
        }
        let (id, value) = match action {
            HISS_CAPTURE_CANCEL => ("learn_noise", false),
            HISS_CAPTURE_START => ("learn_noise", true),
            HISS_CLEAR_PROFILE => ("clear_profile", true),
            _ => {
                set_last_error("Invalid Hiss capture action");
                return PluginError::InvalidParameter;
            }
        };
        match handle_ref.plugin.set_parameter(
            ParameterId::from(id),
            ParameterValue::Bool(value),
        ) {
            Ok(()) => PluginError::Success,
            Err(error) => {
                set_last_error(&format!("Hiss capture control failed: {error}"));
                PluginError::InvalidParameter
            }
        }
    }));

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_hiss_capture_control");
            PluginError::UnknownError
        })
        .into()
}

/// Free a state buffer returned by [`plugin_save_state`] or
/// [`plugin_export_preset_json`].
///
/// # Safety
/// * `data` must be either `NULL` or a pointer previously returned by a
///   function documented as returning an owned byte buffer, with the same
///   `len` value that was written to the associated `out_len` pointer.
/// * The pointer must not be freed more than once.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_free_state(data: *mut u8, len: usize) {
    if !data.is_null() && len > 0 {
        libc_free(data, len);
    }
}

/// Export a named preset document as JSON bytes.
///
/// The returned buffer uses the preset document schema advertised by
/// [`plugin_preset_document_info`] and must be freed with [`plugin_free_state`].
/// Hiss contention fails explicitly like [`plugin_save_state`]: `NULL` with
/// a busy diagnostic and `out_len` set to 0, never a truncated document.
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `preset_name` may be `NULL` or must be a valid, null-terminated UTF-8 C
///   string that remains readable for the duration of this call.
/// * `out_len` must be a valid pointer to `usize` that this call may write.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_export_preset_json(
    handle: *const PluginHandle,
    preset_name: *const c_char,
    out_len: *mut usize,
) -> *mut u8 {
    if handle.is_null() || out_len.is_null() {
        return ptr::null_mut();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &*handle;
        let name = if preset_name.is_null() {
            "Untitled"
        } else {
            CStr::from_ptr(preset_name).to_str().unwrap_or("Untitled")
        };
        let state = match save_state_with_eq_family_pairs(handle_ref) {
            Ok(state) => state,
            Err(error) => {
                set_last_error(&error);
                *out_len = 0;
                return ptr::null_mut();
            }
        };
        let info = handle_ref.plugin.info();
        let document = serde_json::json!({
            "schema_version": 1,
            "ut_type": PRESET_UT_TYPE_JSON,
            "file_extension": "sotfpreset",
            "preset_name": name,
            "plugin_type": handle_ref.plugin_type,
            "plugin_name": info.name,
            "plugin_version": info.version,
            "state": state,
        });
        let bytes = match serde_json::to_vec(&document) {
            Ok(bytes) => bytes,
            Err(err) => {
                set_last_error(&format!("Failed to serialize preset document: {err}"));
                *out_len = 0;
                return ptr::null_mut();
            }
        };
        copy_bytes_to_ffi_buffer(&bytes, out_len)
    }));

    match result {
        Ok(buf) => buf,
        Err(_) => {
            set_last_error("Panic in plugin_export_preset_json");
            unsafe {
                *out_len = 0;
            }
            ptr::null_mut()
        }
    }
}

/// Import a JSON preset document created by [`plugin_export_preset_json`].
///
/// Uses the transactional, control-thread restoration behavior documented by
/// [`plugin_load_state`].
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `data` must point to `len` bytes of valid preset JSON that remain readable
///   for the duration of this call.
/// * Call on a control thread with no concurrent access to `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_import_preset_json(
    handle: *mut PluginHandle,
    data: *const u8,
    len: usize,
) -> c_int {
    if handle.is_null() || data.is_null() {
        set_last_error("NULL pointer in plugin_import_preset_json");
        return PluginError::NullPointer.into();
    }
    if len > MAX_PRESET_JSON_IMPORT_BYTES {
        set_last_error(&format!(
            "Preset JSON exceeds {} byte import limit",
            MAX_PRESET_JSON_IMPORT_BYTES
        ));
        return PluginError::InvalidConfig.into();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &mut *handle;
        let slice = slice::from_raw_parts(data, len);
        let document: serde_json::Value = match serde_json::from_slice(slice) {
            Ok(document) => document,
            Err(err) => {
                set_last_error(&format!("Invalid preset JSON: {err}"));
                return PluginError::InvalidConfig;
            }
        };

        if document
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            set_last_error("Preset JSON schema_version must be integer 1");
            return PluginError::InvalidConfig;
        }
        if document.get("ut_type").and_then(serde_json::Value::as_str) != Some(PRESET_UT_TYPE_JSON)
        {
            set_last_error("Preset JSON has an unsupported ut_type");
            return PluginError::InvalidConfig;
        }
        let Some(preset_plugin_type) = document
            .get("plugin_type")
            .and_then(serde_json::Value::as_str)
        else {
            set_last_error("Preset JSON is missing a string plugin_type");
            return PluginError::InvalidConfig;
        };
        if !super::plugin_factory::preset_import_type_matches_target(
            &handle_ref.plugin_type,
            preset_plugin_type,
        ) {
            set_last_error("Preset plugin_type does not match the current plugin family");
            return PluginError::InvalidConfig;
        }

        let Some(state_values) = document.get("state").and_then(|state| state.as_array()) else {
            set_last_error("Preset JSON is missing a state byte array");
            return PluginError::InvalidConfig;
        };
        if state_values.len() > MAX_PRESET_STATE_BYTES {
            set_last_error(&format!(
                "Preset state exceeds {} byte limit",
                MAX_PRESET_STATE_BYTES
            ));
            return PluginError::InvalidConfig;
        }

        let mut state = Vec::with_capacity(state_values.len());
        for value in state_values {
            let Some(byte) = value.as_u64().and_then(|v| u8::try_from(v).ok()) else {
                set_last_error("Preset state contains a non-byte value");
                return PluginError::InvalidConfig;
            };
            state.push(byte);
        }

        let state = if matches!(
            handle_ref.plugin_type.as_str(),
            "DynamicEQ" | "dynamic_eq" | "dynamic-eq"
        ) {
            match super::plugin_factory::add_dynamic_eq_preset_shelf_defaults(&state) {
                Ok(state) => state,
                Err(error) => {
                    set_last_error(&format!("Invalid DynamicEQ preset state: {error}"));
                    return PluginError::InvalidConfig;
                }
            }
        } else {
            state
        };

        let load_result =
            replace_plugin_from_state(handle_ref, &state, PluginStateRestoreKind::Preset);
        match load_result {
            Ok(_) => PluginError::Success,
            Err(e) => {
                set_last_error(&format!("Failed to import preset state: {e}"));
                PluginError::InvalidConfig
            }
        }
    }));

    result
        .unwrap_or_else(|_| {
            set_last_error("Panic in plugin_import_preset_json");
            PluginError::UnknownError
        })
        .into()
}

/// Suggest a filesystem-safe preset filename.
///
/// # Returns
/// * Owned string that the caller must release with [`plugin_free_string`].
/// * `NULL` on error.
///
/// # Safety
/// * `handle` must be a valid plugin handle that has not been destroyed.
/// * `preset_name` may be `NULL` or must be a valid, null-terminated UTF-8 C
///   string that remains readable for the duration of this call.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_suggest_preset_filename(
    handle: *const PluginHandle,
    preset_name: *const c_char,
) -> *mut c_char {
    if handle.is_null() {
        return ptr::null_mut();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let handle_ref = &*handle;
        let name = if preset_name.is_null() {
            "Untitled"
        } else {
            CStr::from_ptr(preset_name).to_str().unwrap_or("Untitled")
        };
        let plugin = sanitize_filename_component(&handle_ref.plugin_type);
        let preset = sanitize_filename_component(name);
        CString::new(format!("{plugin}-{preset}.sotfpreset"))
            .map(CString::into_raw)
            .unwrap_or(ptr::null_mut())
    }));

    result.unwrap_or(ptr::null_mut())
}

/// Get the list of available plugin types as a JSON array string.
///
/// # Returns
/// * JSON array string owned by the caller. It must be released with
///   [`plugin_free_string`] when no longer needed.
/// * `NULL` on error.
#[unsafe(no_mangle)]
pub extern "C" fn plugin_available_types() -> *mut c_char {
    let types = plugins_bridge::factory::available_plugin_types();
    let json = serde_json::json!(types);
    match CString::new(json.to_string()) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}
