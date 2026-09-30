//! Public C-ABI state replacement tests for Crossover topology and family.

// Rust guideline compliant 2026-02-21
use crate::{
    ParameterInfo, PluginError, PluginHandle, plugin_create, plugin_destroy,
    plugin_export_preset_json, plugin_free_state, plugin_get_last_error, plugin_get_parameter,
    plugin_get_parameter_count, plugin_get_parameter_info, plugin_import_preset_json,
    plugin_load_state, plugin_process, plugin_save_state,
};
use std::ffi::{CStr, CString};
use std::slice;

struct CrossoverHandle(*mut PluginHandle);

impl CrossoverHandle {
    fn new(config: &str, output_channels: usize) -> Self {
        let plugin_type = CString::new("Crossover").expect("static type has no NUL");
        let config = CString::new(config).expect("test config has no NUL");
        let handle = plugin_create(
            plugin_type.as_ptr(),
            config.as_ptr(),
            48_000,
            2,
            output_channels,
        );
        assert!(
            !handle.is_null(),
            "Crossover creation failed: {}",
            last_error()
        );
        Self(handle)
    }

    fn load(&mut self, state: &[u8]) -> i32 {
        plugin_load_state(self.0, state.as_ptr(), state.len())
    }

    fn save(&self) -> Vec<u8> {
        let mut length = 0;
        let state = plugin_save_state(self.0, &mut length);
        assert!(!state.is_null(), "Crossover state allocation failed");
        // SAFETY: `plugin_save_state` returns `length` initialized bytes owned by
        // this test until `plugin_free_state` is called below.
        let copy = unsafe { slice::from_raw_parts(state, length) }.to_vec();
        plugin_free_state(state, length);
        copy
    }

    fn export_preset(&self) -> Vec<u8> {
        let name = CString::new("AUD142 Crossover topology").expect("static name has no NUL");
        let mut length = 0;
        let preset = plugin_export_preset_json(self.0, name.as_ptr(), &mut length);
        assert!(!preset.is_null(), "Crossover preset export failed");
        // SAFETY: `plugin_export_preset_json` returns `length` initialized
        // bytes owned by this test until `plugin_free_state` is called below.
        let copy = unsafe { slice::from_raw_parts(preset, length) }.to_vec();
        plugin_free_state(preset, length);
        copy
    }

    fn import_preset(&mut self, preset: &[u8]) -> i32 {
        plugin_import_preset_json(self.0, preset.as_ptr(), preset.len())
    }

    fn normalized(&self, parameter_id: &str) -> f64 {
        let parameter_id = CString::new(parameter_id).expect("static ID has no NUL");
        plugin_get_parameter(self.0, parameter_id.as_ptr())
    }

    fn parameter_count(&self) -> usize {
        usize::try_from(plugin_get_parameter_count(self.0)).expect("parameter count is nonnegative")
    }

    fn parameter_info(&self, index: usize) -> *const ParameterInfo {
        plugin_get_parameter_info(self.0, index)
    }

    fn process(&mut self, input: &[f32], output_channels: usize) -> Vec<f32> {
        assert_eq!(input.len() % 2, 0, "test input is interleaved stereo");
        let frames = input.len() / 2;
        let mut output = vec![0.0; frames * output_channels];
        assert_eq!(
            plugin_process(self.0, input.as_ptr(), output.as_mut_ptr(), frames),
            PluginError::Success as i32,
            "Crossover processing failed: {}",
            last_error()
        );
        output
    }
}

impl Drop for CrossoverHandle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return "<no FFI error>".to_owned();
    }
    // SAFETY: The FFI error string remains valid until the next error update on
    // this thread. This helper reads it immediately after the failing call.
    unsafe { CStr::from_ptr(error) }
        .to_str()
        .expect("FFI error is UTF-8")
        .to_owned()
}

fn info_id(info: *const ParameterInfo) -> String {
    assert!(!info.is_null(), "parameter info is non-null");
    // SAFETY: The pointer is owned by a live handle; the test checks it both
    // before and after a structural restore while that handle remains alive.
    let id = unsafe { (*info).id };
    assert!(!id.is_null(), "parameter info ID is non-null");
    // SAFETY: The ID is a C string retained by the owning ParameterMap.
    unsafe { CStr::from_ptr(id) }
        .to_str()
        .expect("parameter ID is UTF-8")
        .to_owned()
}

fn signal(first_frame: usize, frames: usize) -> Vec<f32> {
    (first_frame * 2..(first_frame + frames) * 2)
        .map(|index| {
            let sample = index as f32;
            (sample * 0.071).sin() * 0.41 + (sample * 0.013).cos() * 0.17
        })
        .collect()
}

fn assert_valid_nontrivial(output: &[f32]) {
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-5));
}

#[test]
fn family_state_restore_rebuilds_runtime_map_and_matches_full_audio() {
    for (family, normalized_type, is_fir) in [
        ("LR48", 3.0 / 12.0, false),
        ("Bessel12", 12.0 / 12.0, false),
        ("LinearPhase", 1.0 / 12.0, true),
    ] {
        let taps = if is_fir { 31 } else { 1_025 };
        let source_config =
            format!(r#"{{"type":"{family}","frequency":930.0,"output":"both","fir_taps":{taps}}}"#);
        let source = CrossoverHandle::new(&source_config, 4);
        let target_config = r#"{"type":"LR24","frequency":930.0,"output":"both"}"#;
        let mut target = CrossoverHandle::new(target_config, 4);
        let mut fresh_reference = CrossoverHandle::new(&source_config, 4);

        let old_info = target.parameter_info(0);
        assert_eq!(info_id(old_info), "type");
        // SAFETY: `old_info` points into the live target map, retained across
        // state replacement by PluginHandle::retired_parameter_maps.
        let old_default = unsafe { (*old_info).default_value };
        assert_eq!(old_default, 0.0);
        let state = source.save();
        assert_eq!(
            target.load(&state),
            PluginError::Success as i32,
            "restoring same-width {family} family: {}",
            last_error()
        );
        assert!((target.normalized("type") - normalized_type).abs() < 1.0e-12);
        assert_eq!(info_id(old_info), "type", "old info pointers remain valid");
        // SAFETY: The old map storage is still owned by the target handle.
        assert_eq!(unsafe { (*old_info).default_value }, old_default);
        let new_info = target.parameter_info(0);
        assert_eq!(info_id(new_info), "type");
        // SAFETY: The returned info is owned by the target's current map.
        let new_info = unsafe { &*new_info };
        assert_eq!(new_info.min_value, 0.0);
        assert_eq!(new_info.max_value, 12.0);
        assert_eq!(new_info.steps, 12);
        assert_eq!(new_info.default_value, normalized_type * 12.0);
        assert_eq!(
            target.parameter_count(),
            if is_fir { 4 } else { 3 },
            "runtime IDs follow the restored family's legacy layout"
        );

        let input = signal(0, 512);
        let restored_audio = target.process(&input, 4);
        let reference_audio = fresh_reference.process(&input, 4);
        assert_valid_nontrivial(&restored_audio);
        assert_eq!(restored_audio, reference_audio, "family {family}");
    }
}

#[test]
fn fir_alias_state_is_canonicalized_and_preserved_on_the_public_abi() {
    let mut target =
        CrossoverHandle::new(r#"{"type":"LR24","frequency":930.0,"output":"both"}"#, 4);
    let mut fresh_reference = CrossoverHandle::new(
        r#"{"type":"LinearPhase","frequency":930.0,"output":"both","fir_taps":31}"#,
        4,
    );

    let alias_state = br#"{"type":"FIR","fir_taps":31}"#;
    assert_eq!(
        target.load(alias_state),
        PluginError::Success as i32,
        "supported FIR alias is a same-topology family restore: {}",
        last_error()
    );
    assert!((target.normalized("type") - 1.0 / 12.0).abs() < 1.0e-12);
    assert_eq!(target.parameter_count(), 4);
    let saved: serde_json::Value = serde_json::from_slice(&target.save()).unwrap();
    assert_eq!(saved["type"], "LinearPhase", "saved state is canonical");
    assert_eq!(saved["fir_taps"], 31);

    let input = signal(0, 512);
    let restored_audio = target.process(&input, 4);
    let reference_audio = fresh_reference.process(&input, 4);
    assert_valid_nontrivial(&restored_audio);
    assert_eq!(restored_audio, reference_audio);
}

#[test]
fn per_channel_state_restore_keeps_dormant_global_both_and_matches_audio() {
    let source_config = r#"{"type":"Bessel12","frequency":870.0,"output":"both","channel_frequencies_hz":[720.0,1800.0],"channel_modes":["lowpass","highpass"]}"#;
    let target_config = r#"{"type":"LR24","frequency":870.0,"output":"both","channel_frequencies_hz":[720.0,1800.0],"channel_modes":["lowpass","highpass"]}"#;
    let source = CrossoverHandle::new(source_config, 2);
    let mut target = CrossoverHandle::new(target_config, 2);
    let mut fresh_reference = CrossoverHandle::new(source_config, 2);

    assert_eq!(
        target.load(&source.save()),
        PluginError::Success as i32,
        "complete explicit channel modes select the per-channel route: {}",
        last_error()
    );
    assert!((target.normalized("type") - 1.0).abs() < 1.0e-12);
    let input = signal(0, 512);
    let restored_audio = target.process(&input, 2);
    let reference_audio = fresh_reference.process(&input, 2);
    assert_valid_nontrivial(&restored_audio);
    assert_eq!(restored_audio, reference_audio);
}

#[test]
fn partial_state_merges_and_invalid_topology_or_width_preserves_audio() {
    let config = r#"{"type":"LR24","frequency":870.0,"output":"lowpass"}"#;
    let mut target = CrossoverHandle::new(config, 2);
    let mut twin = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":1260.0,"output":"lowpass"}"#,
        2,
    );

    let partial_state = br#"{"frequency":1260.0}"#;
    assert_eq!(target.load(partial_state), PluginError::Success as i32);
    assert_eq!(
        target.normalized("type"),
        0.0,
        "omitted family is preserved"
    );
    assert_eq!(target.normalized("mode"), 0.0, "omitted mode is preserved");
    assert!((target.normalized("frequency") - (1260.0 - 20.0) / 19_980.0).abs() < 1.0e-5);

    // Bring both live histories to the same populated point after the accepted
    // partial restore, then reject a state that introduces multiway IDs.
    let prefix = signal(0, 128);
    assert_eq!(target.process(&prefix, 2), twin.process(&prefix, 2));
    let state_before = target.save();

    let multiway = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":870.0,"output":"both","extra_frequencies":[2600.0,7800.0]}"#,
        8,
    );
    assert_eq!(
        target.load(&multiway.save()),
        PluginError::InvalidConfig as i32,
        "multiway runtime IDs cannot be applied to a two-way handle"
    );
    assert_eq!(
        target.save(),
        state_before,
        "topology rejection is transactional"
    );

    let per_channel = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":870.0,"output":"both","channel_frequencies_hz":[720.0,1800.0],"channel_modes":["lowpass","highpass"]}"#,
        2,
    );
    assert_eq!(
        target.load(&per_channel.save()),
        PluginError::InvalidConfig as i32,
        "per-channel IDs cannot be applied to a band-topology handle"
    );
    assert_eq!(
        target.save(),
        state_before,
        "topology rejection is transactional"
    );

    let both_width =
        CrossoverHandle::new(r#"{"type":"LR24","frequency":870.0,"output":"both"}"#, 4);
    assert_eq!(
        target.load(&both_width.save()),
        PluginError::InvalidConfig as i32,
        "Both changes the width of this scalar-output handle"
    );
    assert_eq!(
        target.save(),
        state_before,
        "width rejection is transactional"
    );

    let continuation = signal(128, 256);
    let target_audio = target.process(&continuation, 2);
    let twin_audio = twin.process(&continuation, 2);
    assert_valid_nontrivial(&target_audio);
    assert_eq!(
        target_audio, twin_audio,
        "rejected restores preserve live history"
    );
}

#[test]
fn invalid_typed_values_are_transactional_for_populated_audio() {
    let config = r#"{"type":"LR24","frequency":870.0,"output":"lowpass"}"#;
    let mut target = CrossoverHandle::new(config, 2);
    let mut twin = CrossoverHandle::new(config, 2);
    let prefix = signal(0, 48);
    assert_eq!(target.process(&prefix, 2), twin.process(&prefix, 2));

    for invalid_state in [
        br#"{"type":17}"#.as_slice(),
        br#"{"type":"not-a-family"}"#.as_slice(),
        br#"{"mode":2}"#.as_slice(),
        br#"{"mode":"not-a-mode"}"#.as_slice(),
        br#"{"frequency":"not-numeric"}"#.as_slice(),
        br#"{"frequency":26000.0}"#.as_slice(),
    ] {
        let before = target.save();
        assert_eq!(
            target.load(invalid_state),
            PluginError::InvalidConfig as i32,
            "invalid Crossover state must be rejected"
        );
        assert_eq!(target.save(), before, "invalid values do not change state");
    }

    let continuation = signal(48, 256);
    let target_audio = target.process(&continuation, 2);
    let twin_audio = twin.process(&continuation, 2);
    assert_valid_nontrivial(&target_audio);
    assert_eq!(target_audio, twin_audio);

    let mut cold = CrossoverHandle::new(config, 2);
    let cold_audio = cold.process(&continuation, 2);
    let max_difference = target_audio
        .iter()
        .zip(&cold_audio)
        .map(|(populated, fresh)| (populated - fresh).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_difference > 1.0e-5,
        "continuation after the populated prefix must differ from a cold instance"
    );
}

#[test]
fn multiway_partial_merge_preserves_order_and_rejects_crossed_cutoff() {
    let mut target = CrossoverHandle::new(
        r#"{"type":"LR48","frequency":600.0,"output":"both","extra_frequencies":[1500.0,4000.0]}"#,
        8,
    );
    let partial = br#"{"frequency_2":2200.0}"#;
    assert_eq!(target.load(partial), PluginError::Success as i32);
    assert!((target.normalized("type") - 3.0 / 12.0).abs() < 1.0e-12);
    assert_eq!(target.normalized("mode"), 1.0);
    assert!((target.normalized("frequency") - (600.0 - 20.0) / 19_980.0).abs() < 1.0e-5);
    assert!((target.normalized("frequency_2") - (2200.0 - 20.0) / 19_980.0).abs() < 1.0e-5);
    assert!((target.normalized("frequency_3") - (4000.0 - 20.0) / 19_980.0).abs() < 1.0e-5);

    let mut twin = CrossoverHandle::new(
        r#"{"type":"LR48","frequency":600.0,"output":"both","extra_frequencies":[2200.0,4000.0]}"#,
        8,
    );
    let input = signal(0, 64);
    assert_eq!(target.process(&input, 8), twin.process(&input, 8));
    let before = target.save();
    assert_eq!(
        target.load(br#"{"frequency_2":5000.0}"#),
        PluginError::InvalidConfig as i32,
        "cutoffs crossing frequency_3 must be rejected"
    );
    assert_eq!(target.save(), before);

    let continuation = signal(64, 256);
    let target_audio = target.process(&continuation, 8);
    let twin_audio = twin.process(&continuation, 8);
    assert_valid_nontrivial(&target_audio);
    assert_eq!(target_audio, twin_audio);
}

#[test]
fn full_two_way_preset_refuses_to_retain_multiway_target_splits_but_raw_state_stays_partial() {
    let mut source =
        CrossoverHandle::new(r#"{"type":"LR24","frequency":900.0,"output":"lowpass"}"#, 2);
    let mut same_layout_target = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":1200.0,"output":"lowpass"}"#,
        2,
    );
    assert_eq!(
        same_layout_target.import_preset(&source.export_preset()),
        PluginError::Success as i32,
        "full preset is accepted for the matching runtime layout"
    );
    let same_layout_input = signal(0, 64);
    assert_eq!(
        same_layout_target.process(&same_layout_input, 2),
        source.process(&same_layout_input, 2)
    );

    let four_way_config = r#"{"type":"LR24","frequency":600.0,"output":"lowpass","extra_frequencies":[1500.0,4000.0]}"#;
    let mut preset_target = CrossoverHandle::new(four_way_config, 2);
    let mut preset_twin = CrossoverHandle::new(four_way_config, 2);
    let prefix = signal(0, 96);
    assert_eq!(
        preset_target.process(&prefix, 2),
        preset_twin.process(&prefix, 2)
    );
    let before_rejected_preset = preset_target.save();
    assert_eq!(
        preset_target.import_preset(&source.export_preset()),
        PluginError::InvalidConfig as i32,
        "full two-way preset topology must not silently inherit target splits"
    );
    assert_eq!(preset_target.save(), before_rejected_preset);
    let continuation = signal(96, 256);
    assert_eq!(
        preset_target.process(&continuation, 2),
        preset_twin.process(&continuation, 2),
        "rejected full preset preserves populated target history"
    );

    let mut partial_target = CrossoverHandle::new(four_way_config, 2);
    assert_eq!(
        partial_target.load(&source.save()),
        PluginError::Success as i32,
        "raw state remains a partial merge"
    );
    let merged_state: serde_json::Value = serde_json::from_slice(&partial_target.save()).unwrap();
    assert_eq!(merged_state["frequency_2"], 1500.0);
    assert_eq!(merged_state["frequency_3"], 4000.0);
    let mut partial_reference = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":900.0,"output":"lowpass","extra_frequencies":[1500.0,4000.0]}"#,
        2,
    );
    let input = signal(0, 256);
    let partial_audio = partial_target.process(&input, 2);
    let reference_audio = partial_reference.process(&input, 2);
    assert_valid_nontrivial(&partial_audio);
    assert_eq!(partial_audio, reference_audio);
}
