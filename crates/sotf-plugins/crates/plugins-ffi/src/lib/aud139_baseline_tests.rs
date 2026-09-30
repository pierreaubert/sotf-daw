//! Manual pre-edit snapshot of the public DynamicEQ C-ABI surface.
//!
//! This records the actual handle's parameter enumeration, state document,
//! and preset document before AUD139 changes the shape parameter surface.

// Rust guideline compliant 2026-02-21
use super::{
    ParameterInfo, plugin_create, plugin_destroy, plugin_export_preset_json, plugin_free_state,
    plugin_get_parameter_choice_label, plugin_get_parameter_count, plugin_get_parameter_info,
    plugin_import_preset_json, plugin_load_state, plugin_process, plugin_save_state,
    plugin_set_parameter,
};
use serde_json::json;
use std::ffi::{CStr, CString, c_char};
use std::fs;
use std::path::PathBuf;

fn c_string(pointer: *const c_char) -> String {
    assert!(!pointer.is_null(), "C ABI returned a null string pointer");
    // SAFETY: `pointer` is borrowed from ParameterInfo owned by the live handle;
    // its contract keeps these strings valid until plugin_destroy.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .expect("C ABI metadata is UTF-8")
        .to_owned()
}

fn copy_owned_bytes(pointer: *mut u8, length: usize) -> Vec<u8> {
    assert!(!pointer.is_null(), "C ABI state/preset allocation failed");
    // SAFETY: these functions return an allocation of `length` bytes owned by
    // the caller; copy it before releasing it through the matching C API.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
    plugin_free_state(pointer, length);
    bytes
}

fn create_dynamic_eq(config: &serde_json::Value) -> *mut super::PluginHandle {
    let plugin_type = CString::new("DynamicEQ").expect("static plugin type");
    let config = CString::new(config.to_string()).expect("JSON contains no interior NUL");
    let handle = plugin_create(plugin_type.as_ptr(), config.as_ptr(), 48_000, 2, 2);
    assert!(!handle.is_null(), "DynamicEQ C-ABI construction succeeds");
    handle
}

fn saved_dynamic_eq_state(handle: *const super::PluginHandle) -> serde_json::Value {
    let mut state_len = 0;
    let state_ptr = plugin_save_state(handle, &mut state_len);
    let state = copy_owned_bytes(state_ptr, state_len);
    serde_json::from_slice(&state).expect("saved DynamicEQ state is JSON")
}

fn process_dynamic_eq(handle: *mut super::PluginHandle, input: &[f32]) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin_process(handle, input.as_ptr(), output.as_mut_ptr(), input.len() / 2),
        0,
        "DynamicEQ C-ABI processing succeeds"
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

#[test]
#[ignore = "manual pre-edit AUD139 public C-ABI parameter/state artifact capture"]
fn capture_aud139_pre_edit_dynamic_eq_parameter_addresses_and_state() {
    let output_dir = PathBuf::from(
        std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
            .expect("set SOTF_AUDIT_BASELINE_DIR to the durable AUD139 baseline directory"),
    );
    fs::create_dir_all(&output_dir).expect("create AUD139 C-ABI baseline directory");

    let plugin_type = CString::new("DynamicEQ").expect("static plugin type");
    let config_json = json!({
        "num_bands": 4,
        "threshold": -24.0,
        "ratio": 4.0,
        "attack_ms": 2.0,
        "release_ms": 55.0,
        "knee": 3.0,
        "link_channels": true,
        "mix": 1.0,
        "bands": [
            {"frequency": 220.0, "q": 0.9, "gain": 9.0, "band_threshold": -24.0, "band_ratio": 4.0, "active": true, "solo": false},
            {"frequency": 980.0, "q": 1.1, "gain": -12.0, "band_threshold": -24.0, "band_ratio": 4.0, "active": true, "solo": false},
            {"frequency": 4_600.0, "q": 0.8, "gain": 6.0, "band_threshold": -24.0, "band_ratio": 4.0, "active": true, "solo": false},
            {"frequency": 9_000.0, "q": 1.3, "gain": 0.0, "band_threshold": -24.0, "band_ratio": 4.0, "active": false, "solo": false}
        ]
    });
    let config = CString::new(config_json.to_string()).expect("JSON contains no interior NUL");

    let handle = plugin_create(plugin_type.as_ptr(), config.as_ptr(), 48_000, 2, 2);
    assert!(
        !handle.is_null(),
        "DynamicEQ C-ABI instance creation failed"
    );
    let handle_const = handle.cast_const();

    let count = plugin_get_parameter_count(handle_const);
    assert_eq!(
        count, 64,
        "legacy DynamicEQ exposes 8 globals plus 8×7 band fields"
    );
    let mut entries = Vec::with_capacity(count as usize);
    let mut ids = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let pointer = plugin_get_parameter_info(handle_const, index);
        assert!(
            !pointer.is_null(),
            "missing legacy parameter metadata at {index}"
        );
        // SAFETY: the returned pointer belongs to `handle`, which stays alive
        // through the complete enumeration and state capture below.
        let info: &ParameterInfo = unsafe { &*pointer };
        let id = c_string(info.id);
        ids.push(id.clone());
        entries.push(json!({
            "index": index,
            "id": id,
            "name": c_string(info.name),
            "unit": c_string(info.unit),
            "min": info.min_value,
            "max": info.max_value,
            "default": info.default_value,
            "steps": info.steps,
            "logarithmic": info.logarithmic,
        }));
    }
    assert_eq!(ids.get(8).map(String::as_str), Some("band_0_frequency"));
    assert_eq!(ids.get(14).map(String::as_str), Some("band_0_solo"));
    assert_eq!(ids.get(15).map(String::as_str), Some("band_1_frequency"));
    assert_eq!(ids.get(63).map(String::as_str), Some("band_7_solo"));

    let mut state_len = 0;
    let state_ptr = plugin_save_state(handle_const, &mut state_len);
    let state = copy_owned_bytes(state_ptr, state_len);
    let state_json: serde_json::Value =
        serde_json::from_slice(&state).expect("saved C-ABI state is JSON");
    fs::write(output_dir.join("dynamic-eq-state.json"), &state)
        .expect("write actual C-ABI state document");

    let preset_name = CString::new("AUD139 pre-edit peak baseline").unwrap();
    let mut preset_len = 0;
    let preset_ptr = plugin_export_preset_json(handle_const, preset_name.as_ptr(), &mut preset_len);
    let preset = copy_owned_bytes(preset_ptr, preset_len);
    let preset_json: serde_json::Value =
        serde_json::from_slice(&preset).expect("exported C-ABI preset is JSON");
    fs::write(output_dir.join("dynamic-eq-preset.json"), &preset)
        .expect("write actual C-ABI preset document");

    let enumeration = json!({
        "plugin_type": "DynamicEQ",
        "sample_rate_hz": 48_000,
        "input_channels": 2,
        "output_channels": 2,
        "count": count,
        "entries": entries,
        "legacy_abi_layout": "8 globals, then 8 bands with 7 fields in template order",
        "legacy_index_15": ids[15],
        "state_top_level_keys": state_json.as_object().map(|object| object.keys().cloned().collect::<Vec<_>>()),
        "preset_plugin_type": preset_json.get("plugin_type").cloned(),
        "preset_state": preset_json.get("state").cloned(),
        "capture_scope": "in-process public C-ABI calls; does not claim an Audio Unit host run",
    });
    fs::write(
        output_dir.join("dynamic-eq-parameter-addresses.json"),
        serde_json::to_vec_pretty(&enumeration).expect("serialize parameter enumeration"),
    )
    .expect("write actual C-ABI parameter enumeration");

    plugin_destroy(handle);
}

#[test]
fn aud139_legacy_cabi_parameter_table_replays_from_tracked_fixture() {
    const BASELINE: &str =
        include_str!("../../tests/data/aud139_pre_edit/cabi/dynamic-eq-parameter-addresses.json");

    let baseline: serde_json::Value =
        serde_json::from_str(BASELINE).expect("tracked pre-edit parameter table is valid JSON");
    let expected_entries = baseline["entries"]
        .as_array()
        .expect("baseline has an entries array");
    assert_eq!(baseline["count"].as_u64(), Some(64));
    assert_eq!(expected_entries.len(), 64);

    let plugin_type = CString::new("DynamicEQ").expect("static plugin type");
    let config = CString::new(
        r#"{"num_bands":4,"threshold":-24.0,"ratio":4.0,"attack_ms":2.0,"release_ms":55.0,"knee":3.0,"link_channels":true,"mix":1.0,"bands":[{"frequency":220.0,"q":0.9,"gain":9.0,"band_threshold":-24.0,"band_ratio":4.0,"active":true,"solo":false},{"frequency":980.0,"q":1.1,"gain":-12.0,"band_threshold":-24.0,"band_ratio":4.0,"active":true,"solo":false},{"frequency":4600.0,"q":0.8,"gain":6.0,"band_threshold":-24.0,"band_ratio":4.0,"active":true,"solo":false},{"frequency":9000.0,"q":1.3,"gain":0.0,"band_threshold":-24.0,"band_ratio":4.0,"active":false,"solo":false}]}"#,
    )
    .expect("static JSON config");
    let handle = plugin_create(plugin_type.as_ptr(), config.as_ptr(), 48_000, 2, 2);
    assert!(!handle.is_null(), "DynamicEQ C-ABI construction succeeds");
    let handle_const = handle.cast_const();

    let actual_count = plugin_get_parameter_count(handle_const);
    assert!(
        actual_count >= 64,
        "legacy parameter prefix remains present"
    );
    for (index, expected) in expected_entries.iter().enumerate().take(64usize) {
        let pointer = plugin_get_parameter_info(handle_const, index);
        assert!(!pointer.is_null(), "missing parameter info at {index}");
        // SAFETY: the borrowed descriptor remains owned by `handle`, which is
        // live until after all 64 entries have been checked.
        let info: &ParameterInfo = unsafe { &*pointer };
        assert_eq!(expected["index"].as_u64(), Some(index as u64));
        assert_eq!(expected["id"].as_str(), Some(c_string(info.id).as_str()));
        assert_eq!(
            expected["name"].as_str(),
            Some(c_string(info.name).as_str())
        );
        assert_eq!(
            expected["unit"].as_str(),
            Some(c_string(info.unit).as_str())
        );
        assert_eq!(expected["min"].as_f64(), Some(info.min_value));
        assert_eq!(expected["max"].as_f64(), Some(info.max_value));
        assert_eq!(expected["default"].as_f64(), Some(info.default_value));
        assert_eq!(expected["steps"].as_u64(), Some(info.steps as u64));
        assert_eq!(expected["logarithmic"].as_bool(), Some(info.logarithmic));
    }

    plugin_destroy(handle);
}

#[test]
fn aud139_shelf_parameter_descriptors_append_after_legacy_addresses() {
    let handle = create_dynamic_eq(&json!({"num_bands": 4}));
    let handle_const = handle.cast_const();

    assert_eq!(
        plugin_get_parameter_count(handle_const),
        80,
        "the 64-entry Peak prefix is followed by shape/slope for all eight bands"
    );
    for band in 0..8 {
        for (offset, field) in [(0, "shape"), (1, "shelf_slope")] {
            let index = 64 + band * 2 + offset;
            let info_ptr = plugin_get_parameter_info(handle_const, index);
            assert!(!info_ptr.is_null(), "missing appended descriptor {index}");
            // SAFETY: metadata remains owned by the live handle until destruction.
            let info = unsafe { &*info_ptr };
            assert_eq!(
                c_string(info.id),
                format!("band_{band}_{field}"),
                "appended descriptor order at index {index}"
            );
        }
        let shape_index = 64 + band * 2;
        for (choice_index, expected) in ["Peak", "Low Shelf", "High Shelf"].into_iter().enumerate()
        {
            let label = plugin_get_parameter_choice_label(handle_const, shape_index, choice_index);
            assert_eq!(c_string(label), expected);
        }
        assert!(plugin_get_parameter_choice_label(handle_const, shape_index, 3).is_null());
        assert!(plugin_get_parameter_choice_label(handle_const, shape_index + 1, 0).is_null());
    }

    plugin_destroy(handle);
}

#[test]
fn aud139_cabi_scalar_shape_and_slope_changes_are_refused_without_mutation() {
    use sotf_host::test_utils::{assert_no_allocs_or_deallocs, measure_heap_activity};
    use std::alloc::{Layout, alloc, dealloc};

    let handle = create_dynamic_eq(&json!({"num_bands": 4}));
    let shape_id = CString::new("band_0_shape").unwrap();
    let slope_id = CString::new("band_0_shelf_slope").unwrap();
    let before = saved_dynamic_eq_state(handle.cast_const());

    let (allocations, deallocations) = measure_heap_activity(|| {
        let layout = Layout::from_size_align(64, 8).expect("valid allocator probe layout");
        // SAFETY: the allocation is checked for null and freed with the same layout below.
        let pointer = unsafe { alloc(layout) };
        if !pointer.is_null() {
            // Volatile access and black_box keep this allocation observable to the optimizer.
            unsafe {
                pointer.write_volatile(0xa5);
                assert_eq!(pointer.read_volatile(), 0xa5);
                dealloc(pointer, layout);
            }
        }
        std::hint::black_box(pointer);
    });
    assert!(
        allocations > 0,
        "allocator probe must observe a global allocation from this test binary"
    );
    assert!(
        deallocations > 0,
        "allocator probe must observe a global deallocation from this test binary"
    );

    let mut result = 0;
    assert_no_allocs_or_deallocs("first DynamicEQ structural setter refusal", || {
        result = plugin_set_parameter(handle, shape_id.as_ptr(), 0.5);
    });
    assert_eq!(
        result, -2,
        "scalar parameter writes must not rebuild structural state on callback routes"
    );

    assert_no_allocs_or_deallocs("warmed DynamicEQ structural setter refusal", || {
        result = plugin_set_parameter(handle, shape_id.as_ptr(), 0.5);
    });
    assert_eq!(result, -2);
    let slope_normalized = (0.4_f64 - 0.1) / (1.0 - 0.1);
    assert_no_allocs_or_deallocs("DynamicEQ shelf-slope setter refusal", || {
        result = plugin_set_parameter(handle, slope_id.as_ptr(), slope_normalized);
    });
    assert_eq!(
        result, -2,
        "slope changes use transactional state restoration"
    );
    assert_eq!(saved_dynamic_eq_state(handle.cast_const()), before);
    plugin_destroy(handle);
}

#[test]
fn aud139_state_reconfiguration_preserves_dormant_band_values() {
    let bands: Vec<_> = (0..8)
        .map(|band| {
            json!({
                "frequency": 120.0 + band as f64 * 70.0,
                "q": 0.5 + band as f64 * 0.25,
                "gain": if band % 2 == 0 { 6.0 } else { -4.0 },
                "band_threshold": -24.0,
                "band_ratio": 3.0,
                "active": true,
                "solo": false,
                "shape": if band % 2 == 0 { "low_shelf" } else { "high_shelf" },
                "shelf_slope": 0.2 + band as f64 * 0.08
            })
        })
        .collect();
    let handle = create_dynamic_eq(&json!({"num_bands": 8, "bands": bands.clone()}));

    let shrink = serde_json::to_vec(&json!({"num_bands": 4})).unwrap();
    assert_eq!(
        plugin_load_state(handle, shrink.as_ptr(), shrink.len()),
        0,
        "a valid structural state restores transactionally"
    );
    let shrunk = saved_dynamic_eq_state(handle.cast_const());
    assert_eq!(shrunk["num_bands"], 4);
    assert_eq!(shrunk["band_7_q"], 2.25);
    assert_eq!(shrunk["band_7_shape"], 2);
    assert!((shrunk["band_7_shelf_slope"].as_f64().unwrap() - 0.76).abs() < 1.0e-6);

    let expand = serde_json::to_vec(&json!({"num_bands": 8})).unwrap();
    assert_eq!(plugin_load_state(handle, expand.as_ptr(), expand.len()), 0);
    let expanded = saved_dynamic_eq_state(handle.cast_const());
    for (band, expected_band) in bands.iter().enumerate().take(8) {
        let q_key = format!("band_{band}_q");
        let shape_key = format!("band_{band}_shape");
        let slope_key = format!("band_{band}_shelf_slope");
        assert_eq!(expanded[&q_key], expected_band["q"]);
        assert_eq!(expanded[&shape_key], if band % 2 == 0 { 1 } else { 2 });
        assert_eq!(
            expanded[&slope_key].as_f64().unwrap() as f32,
            expected_band["shelf_slope"].as_f64().unwrap() as f32,
            "serialized slope preserves its stored f32 value for band {band}"
        );
    }

    let fresh = create_dynamic_eq(&json!({"num_bands": 4}));
    let full_state = serde_json::to_vec(&expanded).unwrap();
    assert_eq!(
        plugin_load_state(fresh, full_state.as_ptr(), full_state.len()),
        0,
        "a fresh handle can restore the complete eight-band state"
    );
    let restored = saved_dynamic_eq_state(fresh.cast_const());
    assert_eq!(restored, expanded);

    let preset_name = CString::new("AUD139 shelf state").unwrap();
    let mut preset_len = 0;
    let preset_ptr =
        plugin_export_preset_json(handle.cast_const(), preset_name.as_ptr(), &mut preset_len);
    let preset = copy_owned_bytes(preset_ptr, preset_len);
    assert_eq!(
        plugin_import_preset_json(fresh, preset.as_ptr(), preset.len()),
        0
    );
    assert_eq!(saved_dynamic_eq_state(fresh.cast_const()), expanded);

    plugin_destroy(fresh);
    plugin_destroy(handle);
}

#[test]
fn aud139_dynamic_eq_state_restore_is_transactional_and_rebuilds_shelves() {
    let base_config = json!({
        "num_bands": 4,
        "threshold": -24.0,
        "ratio": 3.0,
        "attack_ms": 2.0,
        "release_ms": 55.0,
        "knee": 3.0,
        "link_channels": true,
        "mix": 1.0,
        "bands": [{"gain": 6.0}]
    });
    let handle = create_dynamic_eq(&base_config);
    let twin = create_dynamic_eq(&base_config);
    let input = (0..256)
        .flat_map(|frame| {
            let sample = (std::f32::consts::TAU * 440.0 * frame as f32 / 48_000.0).sin() * 0.2;
            [sample, sample * 0.7]
        })
        .collect::<Vec<_>>();

    assert_eq!(
        process_dynamic_eq(handle, &input),
        process_dynamic_eq(twin, &input),
        "populated handles start from identical audio history"
    );
    let state_before = saved_dynamic_eq_state(handle.cast_const());
    for invalid in [
        json!({"band_0_shape": "high_shelf"}),
        json!({"band_00_shape": 1}),
        json!({"band_8_shape": 1}),
        json!({"band_0_shelf_slope": 1.1}),
        json!({"band_0_frequency": 20_001.0}),
    ] {
        let bytes = serde_json::to_vec(&invalid).unwrap();
        assert_eq!(
            plugin_load_state(handle, bytes.as_ptr(), bytes.len()),
            -8,
            "invalid type, slot, slope, and frequency are rejected"
        );
        assert_eq!(saved_dynamic_eq_state(handle.cast_const()), state_before);
        assert_eq!(
            process_dynamic_eq(handle, &input),
            process_dynamic_eq(twin, &input),
            "rejected restore preserves populated audio history"
        );
    }

    let update = serde_json::to_vec(&json!({
        "band_0_shape": 1,
        "band_0_shelf_slope": 0.4
    }))
    .unwrap();
    assert_eq!(plugin_load_state(handle, update.as_ptr(), update.len()), 0);
    let shelf_config = json!({
        "num_bands": 4,
        "threshold": -24.0,
        "ratio": 3.0,
        "attack_ms": 2.0,
        "release_ms": 55.0,
        "knee": 3.0,
        "link_channels": true,
        "mix": 1.0,
        "bands": [{"gain": 6.0, "shape": "low_shelf", "shelf_slope": 0.4}]
    });
    let fresh_shelf = create_dynamic_eq(&shelf_config);
    let fresh_peak = create_dynamic_eq(&base_config);
    let restored_output = process_dynamic_eq(handle, &input);
    let fresh_shelf_output = process_dynamic_eq(fresh_shelf, &input);
    let peak_output = process_dynamic_eq(fresh_peak, &input);
    assert_eq!(restored_output, fresh_shelf_output);
    assert_ne!(restored_output, peak_output, "restored shelf is audible");

    let partial = serde_json::to_vec(&json!({"band_0_gain": 4.0})).unwrap();
    assert_eq!(
        plugin_load_state(handle, partial.as_ptr(), partial.len()),
        0
    );
    let partial_state = saved_dynamic_eq_state(handle.cast_const());
    assert_eq!(partial_state["band_0_shape"], 1);
    assert_eq!(
        partial_state["band_0_shelf_slope"]
            .as_f64()
            .map(|value| value as f32),
        Some(0.4_f32),
        "shelf slope state round-trips at its stored f32 precision"
    );

    plugin_destroy(fresh_peak);
    plugin_destroy(fresh_shelf);
    plugin_destroy(twin);
    plugin_destroy(handle);
}

#[test]
fn aud139_captured_legacy_peak_preset_resets_active_shelves_but_keeps_dormant_slots() {
    let preset_json = include_str!("aud139_legacy_peak_preset.json");
    let preset: serde_json::Value = serde_json::from_str(preset_json).unwrap();
    let legacy_state_bytes = preset["state"]
        .as_array()
        .unwrap()
        .iter()
        .map(|byte| byte.as_u64().unwrap() as u8)
        .collect::<Vec<_>>();
    let legacy_state: serde_json::Value = serde_json::from_slice(&legacy_state_bytes).unwrap();
    assert_eq!(legacy_state["num_bands"], 4);
    assert!(legacy_state.get("band_0_shape").is_none());

    let shelf_bands = (0..8)
        .map(|band| {
            json!({
                "frequency": 200.0 + band as f64 * 250.0,
                "q": 0.707,
                "gain": if band % 2 == 0 { 5.0 } else { -4.0 },
                "band_threshold": -24.0,
                "band_ratio": 4.0,
                "active": band < 4,
                "solo": false,
                "shape": if band % 2 == 0 { "low_shelf" } else { "high_shelf" },
                "shelf_slope": 0.3 + band as f64 * 0.05
            })
        })
        .collect::<Vec<_>>();
    let populated = create_dynamic_eq(&json!({
        "num_bands": 4,
        "threshold": -30.0,
        "ratio": 3.0,
        "attack_ms": 2.0,
        "release_ms": 40.0,
        "knee": 2.0,
        "link_channels": true,
        "mix": 1.0,
        "bands": shelf_bands
    }));
    let input = (0..1024)
        .flat_map(|frame| {
            let phase = std::f32::consts::TAU * 440.0 * frame as f32 / 48_000.0;
            let sample = 0.2 * phase.sin();
            [sample, sample * 0.8]
        })
        .collect::<Vec<_>>();
    process_dynamic_eq(populated, &input);
    let before = saved_dynamic_eq_state(populated.cast_const());

    assert_eq!(
        plugin_import_preset_json(populated, preset_json.as_ptr(), preset_json.len()),
        0,
        "the actual captured pre-shelf preset imports into a live shelf instance"
    );
    let restored = saved_dynamic_eq_state(populated.cast_const());
    for band in 0..4 {
        assert_eq!(restored[format!("band_{band}_shape")], 0);
        assert_eq!(restored[format!("band_{band}_shelf_slope")], 1.0);
        assert_eq!(
            restored[format!("band_{band}_frequency")],
            legacy_state[format!("band_{band}_frequency")]
        );
    }
    for band in 4..8 {
        assert_eq!(
            restored[format!("band_{band}_shape")],
            before[format!("band_{band}_shape")],
            "legacy preset leaves non-active dormant band {band} intact"
        );
        assert_eq!(
            restored[format!("band_{band}_shelf_slope")],
            before[format!("band_{band}_shelf_slope")],
            "legacy preset leaves dormant slope {band} intact"
        );
    }

    let legacy_bands = (0..4)
        .map(|band| {
            json!({
                "frequency": legacy_state[format!("band_{band}_frequency")],
                "q": legacy_state[format!("band_{band}_q")],
                "gain": legacy_state[format!("band_{band}_gain")],
                "band_threshold": legacy_state[format!("band_{band}_band_threshold")],
                "band_ratio": legacy_state[format!("band_{band}_band_ratio")],
                "active": legacy_state[format!("band_{band}_active")],
                "solo": legacy_state[format!("band_{band}_solo")],
                "shape": "peak",
                "shelf_slope": 1.0
            })
        })
        .collect::<Vec<_>>();
    let fresh_legacy = create_dynamic_eq(&json!({
        "num_bands": legacy_state["num_bands"],
        "threshold": legacy_state["threshold"],
        "ratio": legacy_state["ratio"],
        "attack_ms": legacy_state["attack"],
        "release_ms": legacy_state["release"],
        "knee": legacy_state["knee"],
        "link_channels": legacy_state["link_channels"],
        "mix": legacy_state["mix"],
        "bands": legacy_bands
    }));
    assert_eq!(
        process_dynamic_eq(populated, &input),
        process_dynamic_eq(fresh_legacy, &input),
        "legacy preset processing matches a fresh Peak/default-slope instance"
    );

    plugin_destroy(fresh_legacy);
    plugin_destroy(populated);
}
