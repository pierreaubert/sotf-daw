//! Upmixer state restoration through the public C ABI.

// Rust guideline compliant 2026-10-09

use crate::{
    PLUGIN_PARAMETER_TRAIT_FLOAT, PLUGIN_PARAMETER_TRAIT_KNOWN, PLUGIN_PARAMETER_TRAIT_REALTIME,
    PluginHandle, plugin_create, plugin_destroy, plugin_free_state, plugin_get_parameter,
    plugin_get_parameter_automation_traits, plugin_get_parameter_count, plugin_get_parameter_info,
    plugin_import_preset_json, plugin_load_state, plugin_process, plugin_save_state,
    plugin_set_parameter, plugin_set_parameter_precise,
};
use std::ffi::{CStr, CString};

struct Handle(*mut PluginHandle);

impl Handle {
    fn new(config: &str, inputs: usize, outputs: usize) -> Self {
        let config = CString::new(config).unwrap();
        let handle = plugin_create(
            c"Upmixer".as_ptr(),
            config.as_ptr(),
            48_000,
            inputs,
            outputs,
        );
        assert!(!handle.is_null());
        Self(handle)
    }

    fn load(&mut self, json: &str) -> i32 {
        plugin_load_state(self.0, json.as_ptr(), json.len())
    }

    fn value(&self, id: &str) -> f64 {
        let id = CString::new(id).unwrap();
        plugin_get_parameter(self.0, id.as_ptr())
    }

    fn state(&self) -> Vec<u8> {
        let mut len = 0;
        let state = plugin_save_state(self.0, &mut len);
        assert!(!state.is_null());
        // SAFETY: The returned allocation contains len bytes until freed below.
        let bytes = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        plugin_free_state(state, len);
        bytes
    }

    fn render_constant(&mut self, input_frame: &[f32], output_channels: usize) -> Vec<f32> {
        let frames = 4096;
        let input = input_frame.repeat(frames);
        let mut output = vec![0.0; frames * output_channels];
        for _ in 0..6 {
            assert_eq!(
                plugin_process(self.0, input.as_ptr(), output.as_mut_ptr(), frames),
                0
            );
        }
        output
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn config(with_lfe: bool) -> String {
    let mut roles = vec!["front_left", "front_right", "front_center"];
    if with_lfe {
        roles.push("lfe");
    }
    roles.extend(["side_left", "side_right"]);
    let channels: Vec<_> = roles
        .into_iter()
        .enumerate()
        .map(|(index, role)| serde_json::json!({ "index": index, "role": role }))
        .collect();
    serde_json::json!({
        "speaker_config": "5.1.4",
        "input_layout": { "channels": channels }
    })
    .to_string()
}

#[test]
fn rejected_surround_updates_do_not_allocate() {
    use sotf_host::test_utils::assert_no_allocs_or_deallocs;

    let unit = Handle::new(&config(false), 5, 10);
    let speaker_value = unit.value("speaker_config");
    let other_speaker = if speaker_value < 0.5 { 1.0 } else { 0.0 };
    let cases = [
        (
            c"speaker_config",
            other_speaker,
            c"Failed to set parameter: changing the surround output layout requires a new plugin instance",
        ),
        (
            c"low_latency",
            1.0,
            c"Failed to set parameter: changing surround latency requires a new plugin instance",
        ),
        (
            c"binaural_preview",
            1.0,
            c"Failed to set parameter: binaural preview is not supported for surround input",
        ),
        (
            c"auto_gain_enabled",
            1.0,
            c"Failed to set parameter: this stereo-only control is not supported for surround input",
        ),
        (
            c"enable_subharmonic_synth",
            1.0,
            c"Failed to set parameter: this stereo-only control is not supported for surround input",
        ),
    ];
    for (id, value, error) in cases {
        assert_ne!(plugin_set_parameter(unit.0, id.as_ptr(), value), 0);
        assert_no_allocs_or_deallocs("rejected surround parameter", || {
            for _ in 0..32 {
                assert_ne!(plugin_set_parameter(unit.0, id.as_ptr(), value), 0);
            }
        });
        // SAFETY: the FFI returns a valid error string until the next error.
        assert_eq!(
            unsafe { CStr::from_ptr(crate::plugin_get_last_error()) },
            error
        );
    }
    assert_eq!(unit.value("speaker_config"), speaker_value);
    for id in [
        "low_latency",
        "binaural_preview",
        "auto_gain_enabled",
        "enable_subharmonic_synth",
    ] {
        assert_eq!(unit.value(id), 0.0);
    }
}

#[test]
fn surround_rejection_preserves_drain_and_structural_noop_behavior() {
    use sotf_host::test_utils::assert_no_allocs_or_deallocs;

    let unit = Handle::new(&config(false), 5, 10);
    let speaker = unit.value("speaker_config");
    let input = vec![0.03; 128 * 5];
    let mut output = vec![0.0; 128 * 10];
    assert_eq!(
        plugin_process(unit.0, input.as_ptr(), output.as_mut_ptr(), 128),
        0
    );
    let mut capacity = 0;
    // SAFETY: all pointers are valid and hold exclusive access to this handle.
    assert_eq!(
        unsafe { crate::plugin_get_drain_capacity_frames(unit.0, &mut capacity) },
        0
    );
    let mut tail = vec![0.0; capacity * 10];
    let mut produced = 0;
    let mut complete = 0;
    // SAFETY: the output holds the reported capacity and the out-pointers are valid.
    assert_eq!(
        unsafe {
            crate::plugin_drain(
                unit.0,
                tail.as_mut_ptr(),
                capacity,
                &mut produced,
                &mut complete,
            )
        },
        0
    );

    assert_no_allocs_or_deallocs("surround controls during drain", || {
        for (id, value) in [
            (c"speaker_config", speaker),
            (c"low_latency", 0.0),
            (c"binaural_preview", 0.0),
        ] {
            assert_eq!(plugin_set_parameter(unit.0, id.as_ptr(), value), 0);
        }
        for id in [c"auto_gain_enabled", c"enable_subharmonic_synth"] {
            for value in [0.0, 1.0] {
                assert_ne!(plugin_set_parameter(unit.0, id.as_ptr(), value), 0);
            }
        }
    });
    // SAFETY: the FFI returns a valid error string until the next error.
    assert_eq!(
        unsafe { CStr::from_ptr(crate::plugin_get_last_error()) },
        c"Failed to set parameter: Upmixer: reset before changing parameters after drain"
    );
    assert_eq!(crate::plugin_reset(unit.0), 0);
    assert_eq!(
        plugin_set_parameter(unit.0, c"auto_gain_enabled".as_ptr(), 0.0),
        0
    );
}

#[test]
fn automation_traits_distinguish_type_from_ui_steps() {
    let mut unit = Handle::new(&config(false), 5, 10);
    let lookup = |unit: &Handle, name: &str| {
        (0..plugin_get_parameter_count(unit.0) as usize)
            .find(|&index| {
                let info = plugin_get_parameter_info(unit.0, index);
                assert!(!info.is_null());
                // SAFETY: parameter info and its id remain valid while the handle is alive.
                unsafe { CStr::from_ptr((*info).id) }.to_str().unwrap() == name
            })
            .unwrap()
    };
    let cases = [
        (
            "height_gain",
            PLUGIN_PARAMETER_TRAIT_KNOWN
                | PLUGIN_PARAMETER_TRAIT_FLOAT
                | PLUGIN_PARAMETER_TRAIT_REALTIME,
        ),
        (
            "bass_cutoff_hz",
            PLUGIN_PARAMETER_TRAIT_KNOWN | PLUGIN_PARAMETER_TRAIT_FLOAT,
        ),
        ("bass_mode", PLUGIN_PARAMETER_TRAIT_KNOWN),
        (
            "bypass_heights",
            PLUGIN_PARAMETER_TRAIT_KNOWN | PLUGIN_PARAMETER_TRAIT_REALTIME,
        ),
    ];
    for (id, expected) in cases {
        let index = lookup(&unit, id);
        assert_eq!(
            plugin_get_parameter_automation_traits(unit.0, index),
            expected,
            "{id}"
        );
    }
    for index in 0..plugin_get_parameter_count(unit.0) as usize {
        assert_ne!(
            plugin_get_parameter_automation_traits(unit.0, index) & PLUGIN_PARAMETER_TRAIT_KNOWN,
            0,
            "missing static specification for Upmixer parameter {index}"
        );
    }
    assert_eq!(
        plugin_get_parameter_automation_traits(std::ptr::null(), 0),
        0
    );
    assert_eq!(
        plugin_get_parameter_automation_traits(unit.0, usize::MAX),
        0
    );
    assert_eq!(unit.load(r#"{"bass_mode":1}"#), 0);
    for (id, expected) in cases {
        let index = lookup(&unit, id);
        assert_eq!(
            plugin_get_parameter_automation_traits(unit.0, index),
            expected,
            "{id}"
        );
    }
}

#[test]
fn height_gain_ramps_keep_fractional_values_without_allocating() {
    use sotf_host::test_utils::assert_no_allocs_or_deallocs;

    for (configuration, inputs, outputs) in [("{}".to_string(), 2, 6), (config(false), 5, 10)] {
        let unit = Handle::new(&configuration, inputs, outputs);
        assert_eq!(
            plugin_set_parameter(unit.0, c"height_gain".as_ptr(), 0.123),
            0
        );
        assert!((unit.value("height_gain") - 0.125).abs() < 1e-6);
        assert_no_allocs_or_deallocs("height gain ramp FFI", || {
            for index in 0..=1_000 {
                let requested = f64::from(index) / 1_000.0;
                assert_eq!(
                    plugin_set_parameter_precise(unit.0, c"height_gain".as_ptr(), requested),
                    0
                );
                let accepted = plugin_get_parameter(unit.0, c"height_gain".as_ptr());
                assert!(
                    (accepted - requested).abs() < 1e-6,
                    "{requested} → {accepted}"
                );
            }
        });
        let before = unit.value("height_gain");
        assert_ne!(
            plugin_set_parameter_precise(unit.0, c"height_gain".as_ptr(), f64::NAN),
            0
        );
        assert_ne!(
            plugin_set_parameter_precise(unit.0, c"bass_mode".as_ptr(), 0.5),
            0
        );
        assert_eq!(unit.value("height_gain"), before);
        assert_eq!(
            plugin_set_parameter_precise(unit.0, c"height_gain".as_ptr(), -1.0),
            0
        );
        assert_eq!(unit.value("height_gain"), 0.0);
        assert_eq!(
            plugin_set_parameter_precise(unit.0, c"height_gain".as_ptr(), 2.0),
            0
        );
        assert_eq!(unit.value("height_gain"), 1.0);
        assert_ne!(
            plugin_set_parameter_precise(std::ptr::null_mut(), c"height_gain".as_ptr(), 0.5),
            0
        );
        assert_ne!(
            plugin_set_parameter_precise(unit.0, std::ptr::null(), 0.5),
            0
        );
        assert_eq!(
            plugin_set_parameter_precise(unit.0, c"height_gain".as_ptr(), 0.617),
            0
        );
        let saved = unit.state();
        let restored = Handle::new(&configuration, inputs, outputs);
        assert_eq!(
            plugin_load_state(restored.0, saved.as_ptr(), saved.len()),
            0
        );
        assert!((restored.value("height_gain") - 0.617).abs() < 1e-6);
        let document = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "ut_type": "org.spinorama.sotf.plugin-preset",
            "plugin_type": "Upmixer",
            "state": saved,
        }))
        .unwrap();
        let imported = Handle::new(&configuration, inputs, outputs);
        assert_eq!(
            plugin_import_preset_json(imported.0, document.as_ptr(), document.len()),
            0
        );
        assert!((imported.value("height_gain") - 0.617).abs() < 1e-6);
    }
}

#[test]
fn height_gain_ramp_ffi_matches_direct_dsp_audio() {
    use sotf_host::parameters::{ParameterId, ParameterValue};
    use sotf_host::{Plugin, ProcessContext};
    use sotf_plugins::{UpmixerPlugin, UpmixerPluginParams};

    let mut derived: serde_json::Value = serde_json::from_str(&config(false)).unwrap();
    derived["bass_mode"] = "derived".into();
    let mut managed = derived.clone();
    managed["bass_mode"] = "managed".into();
    let mut existing_lfe: serde_json::Value = serde_json::from_str(&config(true)).unwrap();
    existing_lfe["bass_mode"] = "managed".into();
    existing_lfe["combine_input_lfe"] = true.into();
    let seven = serde_json::json!({
        "speaker_config": "7.1.4", "bass_mode": "managed", "combine_input_lfe": true,
        "input_layout": {"channels": [
            {"index":0,"role":"front_left"}, {"index":1,"role":"front_right"},
            {"index":2,"role":"front_center"}, {"index":3,"role":"lfe"},
            {"index":4,"role":"side_left"}, {"index":5,"role":"side_right"},
            {"index":6,"role":"back_left"}, {"index":7,"role":"back_right"}
        ]}
    });
    for (configuration, inputs, outputs, height_start) in [
        (r#"{"speaker_config":"5.1.4"}"#.to_string(), 2, 10, 6),
        (config(false), 5, 10, 6),
        (derived.to_string(), 5, 10, 6),
        (managed.to_string(), 5, 10, 6),
        (existing_lfe.to_string(), 6, 10, 6),
        (seven.to_string(), 8, 12, 8),
    ] {
        let params: UpmixerPluginParams = serde_json::from_str(&configuration).unwrap();
        let mut direct = UpmixerPlugin::try_from_params(params).unwrap();
        direct.initialize(48_000.0).unwrap();
        let unit = Handle::new(&configuration, inputs, outputs);
        let id = ParameterId::from("height_gain");
        let mut input = vec![0.0_f32; 16 * inputs];
        let mut expected = vec![0.0_f32; 16 * outputs];
        let mut actual = vec![0.0_f32; 16 * outputs];
        let mut seed = 0x72ac_8117_u32;
        let mut height_energy = 0.0_f64;
        for index in 0..1_024 {
            let normalized = f64::from(index % 257) / 256.0;
            assert_eq!(
                plugin_set_parameter_precise(unit.0, c"height_gain".as_ptr(), normalized),
                0
            );
            direct
                .set_parameter(id.clone(), ParameterValue::Float((normalized * 2.0) as f32))
                .unwrap();
            for sample in &mut input {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *sample = ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.2;
            }
            assert_eq!(
                plugin_process(unit.0, input.as_ptr(), actual.as_mut_ptr(), 16),
                0
            );
            direct
                .process(&input, &mut expected, &ProcessContext::new(48_000, 16))
                .unwrap();
            assert_eq!(actual, expected, "inputs={inputs}, block={index}");
            height_energy += actual
                .chunks_exact(outputs)
                .flat_map(|frame| &frame[height_start..])
                .map(|v| f64::from(v.abs()))
                .sum::<f64>();
        }
        assert!(
            height_energy > 0.01,
            "test did not exercise height output for {inputs} inputs"
        );
    }
}

#[test]
fn sparse_bass_state_reconstructs_and_round_trips() {
    for (mode, expected_main) in [(1, 0.1), (2, 0.0)] {
        let mut unit = Handle::new(&config(false), 5, 10);
        assert_eq!(
            plugin_set_parameter(unit.0, c"height_gain".as_ptr(), 0.25),
            0
        );
        assert_eq!(unit.load(&format!(r#"{{"bass_mode":{mode}}}"#)), 0);
        assert_eq!(unit.value("bass_mode"), f64::from(mode) / 2.0);
        assert_eq!(unit.value("height_gain"), 0.25);
        let output = unit.render_constant(&[0.1; 5], 10);
        for frame in output.chunks_exact(10).skip(2048) {
            assert!((frame[0] - expected_main).abs() < 1e-4);
            assert!((frame[3] - 0.5).abs() < 1e-4);
        }
        let saved = unit.state();
        let restored = Handle::new(&config(false), 5, 10);
        assert_eq!(
            plugin_load_state(restored.0, saved.as_ptr(), saved.len()),
            0
        );
        assert_eq!(restored.state(), saved);
        let document = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "ut_type": "org.spinorama.sotf.plugin-preset",
            "plugin_type": "Upmixer",
            "state": saved,
        }))
        .unwrap();
        let imported = Handle::new(&config(false), 5, 10);
        assert_eq!(
            plugin_import_preset_json(imported.0, document.as_ptr(), document.len()),
            0
        );
        assert_eq!(imported.state(), restored.state());
    }
}

#[test]
fn invalid_bass_or_layout_restores_are_transactional() {
    let mut unit = Handle::new(&config(true), 6, 10);
    let before = unit.state();
    for invalid in [
        r#"{"bass_mode":1}"#,
        r#"{"bass_mode":3}"#,
        r#"{"bass_cutoff_hz":181}"#,
        r#"{"bass_gain_db":13}"#,
        r#"{"bass_source_front_left":7}"#,
        r#"{"input_format":1}"#,
        r#"{"speaker_config":3}"#,
    ] {
        assert_ne!(unit.load(invalid), 0, "{invalid}");
        assert_eq!(unit.state(), before, "{invalid}");
    }
    assert_eq!(unit.load(r#"{"bass_mode":1,"combine_input_lfe":true}"#), 0);
    let output = unit.render_constant(&[0.1, 0.1, 0.1, 0.03, 0.1, 0.1], 10);
    for frame in output.chunks_exact(10).skip(2048) {
        assert!((frame[3] - 0.53).abs() < 1e-4);
    }
}

#[test]
fn stereo_default_and_partial_state_still_restore() {
    for config in ["null", "{}"] {
        let mut unit = Handle::new(config, 2, 6);
        assert_eq!(unit.load(r#"{"height_gain":0.5}"#), 0);
        assert!((unit.value("height_gain") - 0.25).abs() < 1e-6);
    }
}

#[test]
fn restored_bass_preserves_explicit_input_order_and_source_mask() {
    let mut value: serde_json::Value = serde_json::from_str(&config(false)).unwrap();
    let channels = value["input_layout"]["channels"].as_array_mut().unwrap();
    for assignment in channels {
        let index = assignment["index"].as_u64().unwrap();
        assignment["index"] = (4 - index).into();
    }
    let mut unit = Handle::new(&value.to_string(), 5, 10);
    assert_eq!(
        unit.load(
            r#"{"bass_mode":1,"bass_source_front_right":false,"bass_source_center":false,"bass_source_side_left":false,"bass_source_side_right":false,"bass_source_back_left":false,"bass_source_back_right":false}"#
        ),
        0
    );
    let output = unit.render_constant(&[0.05, 0.04, 0.03, 0.02, 0.1], 10);
    for frame in output.chunks_exact(10).skip(2048) {
        assert!((frame[0] - 0.1).abs() < 1e-6);
        assert!((frame[1] - 0.02).abs() < 1e-6);
        assert!((frame[3] - 0.1).abs() < 1e-4);
    }
}

#[test]
fn bass_restore_respects_the_negotiated_output_layout() {
    let mut value: serde_json::Value = serde_json::from_str(&config(false)).unwrap();
    value["speaker_config"] = "5.1.6".into();
    let mut height_output = Handle::new(&value.to_string(), 5, 12);
    assert_eq!(height_output.load(r#"{"bass_mode":1}"#), 0);
    assert_eq!(height_output.value("speaker_config"), 1.0);
    let output = height_output.render_constant(&[0.1; 5], 12);
    for frame in output.chunks_exact(12).skip(2048) {
        assert!((frame[3] - 0.5).abs() < 1e-4);
    }

    value["speaker_config"] = "5.0".into();
    let mut no_sub = Handle::new(&value.to_string(), 5, 5);
    let before = no_sub.state();
    assert_ne!(no_sub.load(r#"{"bass_mode":1}"#), 0);
    assert_eq!(no_sub.state(), before);
}
