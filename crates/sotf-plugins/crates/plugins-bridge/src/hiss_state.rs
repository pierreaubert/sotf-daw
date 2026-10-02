//! Hiss captured-profile state helpers.
//!
//! Identifies actual Hiss instances through the hosted
//! [`ProfileSnapshot`](sotf_plugin_hiss_reducer::snapshot::ProfileSnapshot)
//! behind `get_data`, never by type-name strings. Non-Hiss plugins keep the
//! scalar-only save behavior by construction: a failed downcast skips the
//! carrier without error.

use sotf_host::plugin::Plugin;
use sotf_plugin_hiss_reducer::snapshot::ProfileSnapshot;
use std::sync::Arc;

/// Canonical saved-state key for the Hiss noise profile.
///
/// The value is the exact persisted `NoiseProfileData` (format 1 floors-only
/// or format 2 with spectrum), shared with the engine settings carrier, the
/// factory constructor, and the FFI state surface.
pub const CAPTURED_PROFILE_KEY: &str = "captured_profile";

/// Momentary Hiss capture commands excluded from saved state.
///
/// These actions are control-thread intents, not restorable state. They never
/// appear in saves and are never replayed from legacy state.
pub const MOMENTARY_IDS: [&str; 2] = ["learn_noise", "clear_profile"];

/// Returns the Hiss snapshot when the plugin publishes one.
///
/// # Examples
///
/// ```ignore
/// if let Some(snapshot) = hiss_snapshot(plugin) {
///     let _ = snapshot.try_status();
/// }
/// ```
pub fn hiss_snapshot(plugin: &dyn Plugin) -> Option<Arc<ProfileSnapshot>> {
    plugin.get_data()?.downcast::<ProfileSnapshot>().ok()
}

/// Returns true for actual Hiss instances.
///
/// Detection uses the `get_data` downcast only. Hiss publishes `Some` from
/// construction, so absence means a non-Hiss plugin on this surface.
pub fn is_hiss(plugin: &dyn Plugin) -> bool {
    hiss_snapshot(plugin).is_some()
}

/// Returns true for momentary Hiss command identifiers.
pub fn is_momentary_id(id: &str) -> bool {
    matches!(id, "learn_noise" | "clear_profile")
}

/// Returns true when a saved-state key must be skipped on Hiss loads.
///
/// Momentary commands and the profile carrier never route through scalar
/// setters. Unknown keys keep the generic loader ignore behavior; this
/// helper only names the Hiss savings explicitly.
pub fn should_skip_hiss_load_key(is_hiss_plugin: bool, key: &str) -> bool {
    is_hiss_plugin && (key == CAPTURED_PROFILE_KEY || is_momentary_id(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::create_plugin;
    use crate::state::{load_state, save_state, try_save_state};
    use sotf_host::parameters::{ParameterId, ParameterValue};
    use sotf_host::plugin::ProcessContext;

    fn hiss_params(channels: usize) -> Box<dyn Plugin> {
        let mut plugin = create_plugin("HissReducer", channels, 48_000, "{}").unwrap();
        plugin.initialize(48_000).unwrap();
        plugin
    }

    #[test]
    fn detects_actual_hiss_and_ignores_others() {
        let hiss = hiss_params(1);
        assert!(is_hiss(&*hiss));
        assert!(hiss_snapshot(&*hiss).is_some());

        let gain = create_plugin("Gain", 2, 48_000, "{}").unwrap();
        assert!(!is_hiss(&*gain));
        assert!(hiss_snapshot(&*gain).is_none());
    }

    #[test]
    fn momentary_ids_are_named_exactly() {
        assert_eq!(MOMENTARY_IDS, ["learn_noise", "clear_profile"]);
        assert!(is_momentary_id("learn_noise"));
        assert!(is_momentary_id("clear_profile"));
        assert!(!is_momentary_id("use_captured_profile"));
        assert!(!is_momentary_id("captured_profile"));
        assert!(!is_momentary_id("spectral_mode"));
    }

    #[test]
    fn load_skip_names_hiss_keys_only() {
        assert!(should_skip_hiss_load_key(true, "learn_noise"));
        assert!(should_skip_hiss_load_key(true, "clear_profile"));
        assert!(should_skip_hiss_load_key(true, CAPTURED_PROFILE_KEY));
        assert!(!should_skip_hiss_load_key(true, "strength"));
        assert!(!should_skip_hiss_load_key(false, "learn_noise"));
        assert!(!should_skip_hiss_load_key(false, CAPTURED_PROFILE_KEY));
    }

    #[test]
    fn hiss_save_omits_momentary_without_profile() {
        let plugin = hiss_params(1);
        let bytes = try_save_state(&*plugin).unwrap();
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes).unwrap();
        assert!(!map.contains_key("learn_noise"));
        assert!(!map.contains_key("clear_profile"));
        assert!(!map.contains_key(CAPTURED_PROFILE_KEY));
        assert_eq!(
            map.get("use_captured_profile"),
            Some(&serde_json::json!(false))
        );
        // The infallible wrapper agrees when no contention exists.
        assert_eq!(save_state(&*plugin), bytes);
    }

    #[test]
    fn non_hiss_save_has_no_carrier_and_matches_wrapper() {
        let plugin = create_plugin("Gain", 2, 48_000, "{}").unwrap();
        let bytes = try_save_state(&*plugin).unwrap();
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes).unwrap();
        assert!(!map.contains_key(CAPTURED_PROFILE_KEY));
        assert_eq!(save_state(&*plugin), bytes);
    }

    #[test]
    fn legacy_momentary_keys_never_replay_on_hiss_load() {
        let mut plugin = hiss_params(1);
        // Legacy blob with both triggers armed plus a scalar change.
        let legacy = br#"{"learn_noise": true, "clear_profile": true, "strength": 0.75}"#;
        load_state(&mut *plugin, legacy).unwrap();
        // Scalar applies; triggers do not fire (idle, no profile).
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("strength")),
            Some(ParameterValue::Float(0.75))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("learn_noise")),
            Some(ParameterValue::Bool(false))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("clear_profile")),
            Some(ParameterValue::Bool(false))
        );
        // Resave carries no momentary keys and no invented profile.
        let resaved: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&try_save_state(&*plugin).unwrap()).unwrap();
        assert!(!resaved.contains_key("learn_noise"));
        assert!(!resaved.contains_key("clear_profile"));
        assert!(!resaved.contains_key(CAPTURED_PROFILE_KEY));
    }

    fn lcg_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..frames)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn profiled_hiss_state_rejected_before_any_mutation() {
        // Capture a real v2 profile through the object-safe Plugin surface.
        let mut profiler = create_plugin("HissReducer", 1, 48_000, "{}").unwrap();
        profiler.initialize(48_000).unwrap();
        profiler
            .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
            .unwrap();
        let noise = lcg_noise(48_000, 0.035, 0xe940c);
        let mut cursor = 0;
        while cursor < noise.len() {
            let count = 4096.min(noise.len() - cursor);
            let mut output = vec![0.0f32; count];
            profiler
                .process(
                    &noise[cursor..cursor + count],
                    &mut output,
                    &ProcessContext::new(48_000, count),
                )
                .unwrap();
            cursor += count;
        }
        assert_eq!(
            profiler.get_parameter(&ParameterId::from("learn_noise")),
            Some(ParameterValue::Bool(false))
        );
        let saved = try_save_state(&*profiler).unwrap();
        let saved_map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            saved_map[CAPTURED_PROFILE_KEY]["format_version"],
            serde_json::json!(2)
        );

        // A profiled blob is rejected before any setter runs: the target
        // keeps its accepted configuration and renders like a control.
        let mut target = create_plugin("HissReducer", 1, 48_000, "{}").unwrap();
        target.initialize(48_000).unwrap();
        let mut control = create_plugin("HissReducer", 1, 48_000, "{}").unwrap();
        control.initialize(48_000).unwrap();
        let error = load_state(&mut *target, &saved).unwrap_err();
        assert!(
            error.contains(CAPTURED_PROFILE_KEY),
            "rejection must name the carrier: {error}"
        );
        let resaved: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&try_save_state(&*target).unwrap()).unwrap();
        assert!(!resaved.contains_key(CAPTURED_PROFILE_KEY));
        let input = lcg_noise(2048, 0.25, 0x440);
        let mut target_out = vec![0.0f32; 2048];
        let mut control_out = vec![0.0f32; 2048];
        target
            .process(&input, &mut target_out, &ProcessContext::new(48_000, 2048))
            .unwrap();
        control
            .process(&input, &mut control_out, &ProcessContext::new(48_000, 2048))
            .unwrap();
        assert_eq!(target_out, control_out);

        // An explicit null clear is likewise unsupported in place: reject
        // without applying the accompanying scalar.
        let null_clear = br#"{"captured_profile": null, "strength": 0.75}"#;
        let error = load_state(&mut *target, null_clear).unwrap_err();
        assert!(error.contains(CAPTURED_PROFILE_KEY));
        let strength = ParameterId::from("strength");
        assert_eq!(
            target.get_parameter(&strength),
            control.get_parameter(&strength)
        );
    }
}
