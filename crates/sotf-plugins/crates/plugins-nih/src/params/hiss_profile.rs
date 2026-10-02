//! Hiss captured-profile field helpers for native state.
//!
//! Encodes the exact v1/v2 [`NoiseProfileData`] carrier used by the NIH
//! `serialize_fields`/`deserialize_fields` pair, validates incoming blobs
//! transactionally before any accepted state mutates, and exposes the
//! control-thread live capture route. All functions run on control threads
//! only; none is called from the realtime `process` callback.
//!
//! The field value is a JSON object with three keys: `version` (always 1),
//! `generation` (the source snapshot export generation for Busy coherence),
//! and `captured_profile` (the exact profile object or explicit null for a
//! pending clear). A missing field preserves the accepted profile; explicit
//! null clears it; malformed blobs reject before any mutation.
//!
//! Host capture actions ride the visible non-automatable `learn_noise` and
//! `clear_profile` controls. The audio thread edge-consumes their values
//! into DSP setters gated by the plugin immediate-momentary opt-in; the
//! latch below is the only edge state, preallocated with the wrapper.
//!
//! [`NoiseProfileData`]: sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData

// Rust guideline compliant 2026-02-21

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::Plugin;
use sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugins::plugin_hiss_reducer::snapshot::ProfileSnapshot;
use std::sync::Arc;

/// Native state field carrying the Hiss captured profile.
pub(crate) const HISS_PROFILE_STATE_FIELD: &str = "sotf_hiss_native_state";

/// Version of the native Hiss field envelope.
///
/// Only version 1 exists. Bumped only for an incompatible envelope change;
/// profile bytes stay exact v1/v2 [`NoiseProfileData`] in all versions.
pub(crate) const HISS_STATE_VERSION: u64 = 1;

/// Native Hiss channel width.
///
/// The NIH wrapper always constructs stereo Hiss. Channel agreement is
/// checked during restore; mismatched blobs reject transactionally.
pub(crate) const HISS_NATIVE_CHANNELS: usize = 2;

/// Momentary Hiss capture commands excluded from saved state.
pub(crate) const HISS_MOMENTARY_IDS: [&str; 2] = ["learn_noise", "clear_profile"];

/// Control-thread Hiss capture action.
///
/// Explicit wrapper control path only; never from render automation or
/// saved state. `Start` arms a 1 s capture, `Cancel` disarms without
/// touching the stored profile, `Clear` discards the stored profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HissCaptureAction {
    /// Cancel an active capture.
    Cancel,
    /// Start a new 1 s capture.
    Start,
    /// Discard the stored profile.
    Clear,
}

/// Returns true for momentary Hiss command identifiers.
///
/// Summary sentence covers the full contract; no further sections apply.
pub(crate) fn is_hiss_momentary_id(id: &str) -> bool {
    matches!(id, "learn_noise" | "clear_profile")
}

/// Encodes a generation-tagged Hiss profile field.
///
/// Serializes `profile` exactly (v1 floors-only or v2 with spectrum) with
/// its source `generation`. A `None` profile encodes explicit null for a
/// pending clear. Runs on control threads only; allocates the JSON string.
///
/// # Examples
///
/// ```ignore
/// let encoded = encode_hiss_field(42, None);
/// assert!(encoded.contains("\"captured_profile\":null"));
/// ```
pub(crate) fn encode_hiss_field(generation: u64, profile: Option<&NoiseProfileData>) -> String {
    let profile = match profile {
        Some(data) => serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "version": HISS_STATE_VERSION,
        "generation": generation,
        "captured_profile": profile,
    })
    .to_string()
}

/// Encodes an explicit Busy retry marker.
///
/// Emitted only when a live export contends and no previously validated
/// profile exists to carry. Decodes as an error (never as absence), so a
/// restore from this save rejects loudly and preserves the target instead
/// of silently dropping a newly captured profile. Control threads only.
///
/// # Examples
///
/// ```ignore
/// let encoded = encode_hiss_busy();
/// assert!(decode_hiss_field(&encoded, 2).is_err());
/// ```
pub(crate) fn encode_hiss_busy() -> String {
    serde_json::json!({
        "version": HISS_STATE_VERSION,
        "generation": 0,
        "captured_profile": "busy",
    })
    .to_string()
}

/// Decodes and validates a Hiss profile field.
///
/// Checks the three-key envelope, version, generation presence, and the
/// exact [`NoiseProfileData`] shape (including channel agreement with
/// `expected_channels`). Explicit null decodes to `None` for a pending
/// clear. A `"busy"` marker decodes as contention, never as absence.
/// Runs on control threads only; allocates while parsing.
///
/// # Examples
///
/// ```ignore
/// let (generation, profile) = decode_hiss_field(&encoded, 2).unwrap();
/// ```
///
/// # Errors
///
/// Returns a message when the envelope is malformed, the version or
/// generation is missing, the save was contended (`"busy"`, retry later),
/// the profile fails [`NoiseProfileData::validate`], or channels disagree.
/// Callers must preserve accepted state on error.
pub(crate) fn decode_hiss_field(
    encoded: &str,
    expected_channels: usize,
) -> Result<(u64, Option<NoiseProfileData>), String> {
    let value: serde_json::Value =
        serde_json::from_str(encoded).map_err(|error| format!("Hiss profile field malformed: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "Hiss profile field must be an object".to_string())?;
    if object.len() != 3 {
        return Err("Hiss profile field must carry version, generation, and captured_profile".to_string());
    }
    if object.get("version").and_then(serde_json::Value::as_u64) != Some(HISS_STATE_VERSION) {
        return Err(format!(
            "Hiss profile field version must be {HISS_STATE_VERSION}"
        ));
    }
    let generation = object
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "Hiss profile field generation must be a u64".to_string())?;
    let profile_value = object
        .get("captured_profile")
        .ok_or_else(|| "Hiss profile field is missing captured_profile".to_string())?;
    if profile_value.is_null() {
        return Ok((generation, None));
    }
    if profile_value == "busy" {
        return Err(
            "Hiss profile snapshot busy: retry the save on a later control tick".to_string(),
        );
    }
    let profile: NoiseProfileData = serde_json::from_value(profile_value.clone())
        .map_err(|error| format!("HissReducer captured_profile is malformed: {error}"))?;
    profile
        .validate()
        .map_err(|error| format!("HissReducer captured_profile invalid: {error}"))?;
    if profile.channels != expected_channels {
        return Err(format!(
            "HissReducer captured_profile has {} channels, native expects {}",
            profile.channels, expected_channels
        ));
    }
    Ok((generation, Some(profile)))
}

/// Returns the Hiss snapshot when the plugin publishes one.
///
/// Clones the stable [`ProfileSnapshot`] Arc behind `get_data` and
/// downcasts pointer-only. Returns `None` for non-Hiss plugins. Safe on
/// any thread; performs no allocation beyond the Arc clone.
///
/// # Examples
///
/// ```ignore
/// if let Some(snapshot) = hiss_snapshot(plugin) {
///     let _ = snapshot.try_status();
/// }
/// ```
pub(crate) fn hiss_snapshot(plugin: &dyn Plugin) -> Option<Arc<ProfileSnapshot>> {
    plugin.get_data()?.downcast::<ProfileSnapshot>().ok()
}

/// Starts a live Hiss capture on the control thread.
///
/// Fires the momentary `learn_noise` command via the named setter. Never
/// called from the realtime callback; structural momentaries never route
/// through render automation.
///
/// # Examples
///
/// ```ignore
/// hiss_start_capture(plugin.as_mut()).unwrap();
/// ```
///
/// # Errors
///
/// Returns the plugin error when capture cannot arm.
pub fn hiss_start_capture(plugin: &mut dyn Plugin) -> Result<(), String> {
    plugin.set_parameter(
        ParameterId::from("learn_noise"),
        ParameterValue::Bool(true),
    )
}

/// Cancels a live Hiss capture on the control thread.
///
/// Clears the `learn_noise` command via the named setter. Control thread
/// only; never from the realtime callback.
///
/// # Errors
///
/// Returns the plugin error when the cancel is rejected.
pub fn hiss_cancel_capture(plugin: &mut dyn Plugin) -> Result<(), String> {
    plugin.set_parameter(
        ParameterId::from("learn_noise"),
        ParameterValue::Bool(false),
    )
}

/// Discards the stored Hiss profile on the control thread.
///
/// Fires the momentary `clear_profile` command via the named setter.
/// Control thread only; never from the realtime callback.
///
/// # Errors
///
/// Returns the plugin error when the clear is rejected.
pub fn hiss_clear_profile(plugin: &mut dyn Plugin) -> Result<(), String> {
    plugin.set_parameter(
        ParameterId::from("clear_profile"),
        ParameterValue::Bool(true),
    )
}

/// Dispatches a wrapper capture action on the control thread.
///
/// Routes [`HissCaptureAction`] to the named momentary setter. Control
/// thread only; never from the realtime callback or saved state.
///
/// # Errors
///
/// Returns the plugin error when the action is rejected.
pub fn hiss_capture_control(
    plugin: &mut dyn Plugin,
    action: HissCaptureAction,
) -> Result<(), String> {
    match action {
        HissCaptureAction::Cancel => hiss_cancel_capture(plugin),
        HissCaptureAction::Start => hiss_start_capture(plugin),
        HissCaptureAction::Clear => hiss_clear_profile(plugin),
    }
}

/// Audio-thread edge state for host capture actions.
///
/// `learn` and `clear` mirror the last consumed host control values; the
/// `*_immediate` flags cache the live DSP immediate-momentary opt-in,
/// refreshed at every successful initialization. Plain data owned by the
/// wrapper: mutated on the audio thread, written on the control thread
/// only while no audio thread runs. Default is all false.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HissMomentaryLatch {
    /// Last consumed `learn_noise` host value.
    pub(crate) learn: bool,
    /// Last consumed `clear_profile` host value.
    pub(crate) clear: bool,
    /// Live DSP admits immediate `learn_noise` control.
    pub(crate) learn_immediate: bool,
    /// Live DSP admits immediate `clear_profile` control.
    pub(crate) clear_immediate: bool,
}
