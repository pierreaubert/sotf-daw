//! Shared C/FFI integration regressions for the audited plugin changes.
//!
//! Every test drives the public C ABI (`plugin_create`, parameter
//! enumeration, `plugin_set_parameter`, `plugin_save_state`,
//! `plugin_load_state`, `plugin_process`) rather than calling plugin crates
//! directly. Audio assertions use real renders: nonzero multichannel
//! signals where the contract promises them, and failed restores must
//! retain the old audio path and configuration byte-for-byte.

use crate::*;
use std::ffi::{CStr, CString};

struct AbiHandle(*mut PluginHandle);

impl AbiHandle {
    fn create(kind: &str, config: &str, inputs: usize, outputs: usize) -> Self {
        let kind = CString::new(kind).unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(kind.as_ptr(), config.as_ptr(), 48_000, inputs, outputs);
        assert!(
            !handle.is_null(),
            "{kind:?} construction failed: {err}",
            kind = kind.to_str().unwrap(),
            err = abi_last_error()
        );
        Self(handle)
    }

    fn inner(&self) -> &PluginHandle {
        // SAFETY: This guard owns a live handle, accessed only on this thread.
        unsafe { &*self.0 }
    }

    fn param_count(&self) -> usize {
        let count = plugin_get_parameter_count(self.0);
        assert!(count >= 0);
        count as usize
    }

    fn param_id(&self, index: usize) -> String {
        let info = plugin_get_parameter_info(self.0, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: The info pointer borrows from this live handle.
        let id = unsafe { (*info).id };
        assert!(!id.is_null());
        // SAFETY: IDs are valid NUL-terminated strings while live.
        unsafe { CStr::from_ptr(id) }
            .to_string_lossy()
            .into_owned()
    }

    fn choice_label(&self, index: usize, choice: usize) -> Option<String> {
        let label = plugin_get_parameter_choice_label(self.0, index, choice);
        if label.is_null() {
            return None;
        }
        // SAFETY: Choice labels are process-static NUL-terminated strings.
        Some(unsafe { CStr::from_ptr(label) }.to_string_lossy().into_owned())
    }

    /// Full `ParameterInfo` tuple for golden comparisons.
    ///
    /// Returns (id, name, unit, min, max, default, steps, logarithmic).
    /// All strings are copied out while the handle is live.
    fn param_info(&self, index: usize) -> (String, String, String, f64, f64, f64, u32, bool) {
        let info = plugin_get_parameter_info(self.0, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: The info pointer borrows from this live handle; strings are
        // valid NUL-terminated UTF-8 while the handle (or its retired maps)
        // stay alive.
        unsafe {
            let info = &*info;
            let id = CStr::from_ptr(info.id).to_string_lossy().into_owned();
            let name = CStr::from_ptr(info.name).to_string_lossy().into_owned();
            let unit = CStr::from_ptr(info.unit).to_string_lossy().into_owned();
            (
                id,
                name,
                unit,
                info.min_value,
                info.max_value,
                info.default_value,
                info.steps,
                info.logarithmic,
            )
        }
    }

    fn set_normalized(&mut self, id: &str, value: f64) -> i32 {
        let id = CString::new(id).unwrap();
        plugin_set_parameter(self.0, id.as_ptr(), value)
    }

    fn get_normalized(&self, id: &str) -> f64 {
        let id = CString::new(id).unwrap();
        plugin_get_parameter(self.0, id.as_ptr())
    }

    fn save(&self) -> Vec<u8> {
        let mut len = 0;
        let state = plugin_save_state(self.0, &mut len);
        assert!(!state.is_null());
        // SAFETY: The FFI owns exactly len initialized bytes until freed below.
        let saved = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        plugin_free_state(state, len);
        saved
    }

    fn load(&mut self, state: &[u8]) -> i32 {
        plugin_load_state(self.0, state.as_ptr(), state.len())
    }

    fn reset(&mut self) {
        assert_eq!(plugin_reset(self.0), 0, "{}", abi_last_error());
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let frames = input.len() / self.inner().input_channels;
        let mut output = vec![f32::NAN; frames * self.inner().output_channels];
        assert_eq!(
            plugin_process(self.0, input.as_ptr(), output.as_mut_ptr(), frames),
            0,
            "{}",
            abi_last_error()
        );
        assert!(output.iter().all(|value| value.is_finite()));
        output
    }

    fn config_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.inner().config_json).expect("handle config must be JSON")
    }
}

impl Drop for AbiHandle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn abi_last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

/// Interleaved sine frame buffer (`frames` x `channels`, 0.5 peak).
fn sine_interleaved(frames: usize, channels: usize, freq: f32, sample_rate: f32) -> Vec<f32> {
    let mut buffer = vec![0.0; frames * channels];
    for frame in 0..frames {
        let sample =
            0.5 * (2.0 * std::f32::consts::PI * freq * frame as f32 / sample_rate).sin();
        for channel in 0..channels {
            buffer[frame * channels + channel] = sample;
        }
    }
    buffer
}

fn channel_rms(output: &[f32], channels: usize, channel: usize) -> f32 {
    let frames = output.len() / channels;
    let sum: f32 = (0..frames)
        .map(|frame| {
            let sample = output[frame * channels + channel];
            sample * sample
        })
        .sum();
    (sum / frames as f32).sqrt()
}

fn peak(output: &[f32]) -> f32 {
    output.iter().map(|sample| sample.abs()).fold(0.0, f32::max)
}

// ---------------------------------------------------------------------------
// EQ mapping: legacy 105 addresses stable, placement appended after.
// ---------------------------------------------------------------------------

#[test]
fn eq_legacy_105_addresses_stable_and_placement_appended() {
    let handle = AbiHandle::create("EQ", "{}", 2, 2);
    assert_eq!(handle.param_count(), 125);

    let globals = [
        "max_filters",
        "tdf2",
        "topology",
        "auto_gain_enabled",
        "oversampling",
    ];
    for (index, expected) in globals.iter().enumerate() {
        assert_eq!(handle.param_id(index), *expected, "global {index} moved");
    }
    let fields = ["freq", "q", "gain", "filter_type", "order"];
    for band in 0..20 {
        for (field, name) in fields.iter().enumerate() {
            let index = 5 + band * 5 + field;
            assert_eq!(
                handle.param_id(index),
                format!("band_{band}_{name}"),
                "legacy address {index} moved"
            );
        }
    }
    for slot in 0..20 {
        assert_eq!(
            handle.param_id(105 + slot),
            format!("filter_{slot}_placement"),
            "placement slot {slot} must follow the 105 legacy addresses"
        );
    }

    let labels = ["Legacy", "Stereo", "Left", "Right", "Mid", "Side"];
    for (choice, expected) in labels.iter().enumerate() {
        assert_eq!(
            handle.choice_label(105, choice).as_deref(),
            Some(*expected)
        );
    }
    assert_eq!(handle.choice_label(105, 6), None);
}

// ---------------------------------------------------------------------------
// EQ P0-2: full 105-entry metadata prefix against the frozen pre-edit capture,
// including the accepted oversampling choice-to-factor correction.
// ---------------------------------------------------------------------------

/// Frozen EQ 105-address metadata prefix.
///
/// Golden source: `audit/artifacts/aud145-ffi-preedit-r1/metadata.json`
/// (frozen pre-edit C ABI capture; `current_value`/`choice_labels` live
/// fields stripped, only the 8 `ParameterInfo` fields kept). Cross-checked
/// by `audit/artifacts/aud145-ffi-oversampling-regression-r1/verify.py`,
/// which asserts `first105_metadata_unchanged` plus the 1/2/4 factor mapping
/// and nonzero/legacy-bit-exact audio. This test was authored from that
/// frozen file, never regenerated from the implementation under test.
/// Coordinator verifies the source hash; any golden update is a deliberate
/// contract change with justification, never a regeneration.
#[test]
fn eq_legacy_105_full_metadata_matches_frozen_capture_with_oversampling_correction() {
    let mut handle = AbiHandle::create("EQ", "{}", 2, 2);
    assert_eq!(handle.param_count(), 125);

    // (id, name, unit, min, max, default, steps, logarithmic)
    let expected_globals: [(&str, &str, &str, f64, f64, f64, u32, bool); 5] = [
        ("max_filters", "Max Filters", "", 1.0, 20.0, 20.0, 19, false),
        ("tdf2", "TDF-II", "", 0.0, 1.0, 0.0, 1, false),
        ("topology", "Topology", "", 0.0, 1.0, 0.0, 2, false),
        ("auto_gain_enabled", "Auto Gain", "", 0.0, 1.0, 0.0, 1, false),
        // Oversampling: 3 UI choices (Off/2x/4x) as indices 0..=2; steps is
        // the label count (3), matching the frozen capture.
        ("oversampling", "Oversampling", "", 0.0, 2.0, 0.0, 3, false),
    ];
    for (index, expected) in expected_globals.iter().enumerate() {
        let actual = handle.param_info(index);
        assert_eq!(
            actual,
            (
                expected.0.to_string(),
                expected.1.to_string(),
                expected.2.to_string(),
                expected.3,
                expected.4,
                expected.5,
                expected.6,
                expected.7,
            ),
            "legacy global {index} metadata drifted from frozen capture"
        );
    }

    // Per-band field templates from the same frozen capture. Q steps 797 is
    // the frozen f64-truncation value ((40.0-0.1)/0.05 as u32), not 798.
    struct BandField {
        key: &'static str,
        name: &'static str,
        unit: &'static str,
        min: f64,
        max: f64,
        default: f64,
        steps: u32,
        logarithmic: bool,
    }
    let fields = [
        BandField {
            key: "freq",
            name: "Frequency",
            unit: "Hz",
            min: 20.0,
            max: 20000.0,
            default: 1000.0,
            steps: 1998,
            logarithmic: true,
        },
        BandField {
            key: "q",
            name: "Q",
            unit: "",
            min: 0.1,
            max: 40.0,
            default: 1.0,
            steps: 797,
            logarithmic: false,
        },
        BandField {
            key: "gain",
            name: "Gain",
            unit: "dB",
            min: -24.0,
            max: 24.0,
            default: 0.0,
            steps: 96,
            logarithmic: false,
        },
        BandField {
            key: "filter_type",
            name: "Type",
            unit: "",
            min: 0.0,
            max: 7.0,
            default: 0.0,
            steps: 7,
            logarithmic: false,
        },
        BandField {
            key: "order",
            name: "Order",
            unit: "",
            min: 2.0,
            max: 8.0,
            default: 2.0,
            steps: 3,
            logarithmic: false,
        },
    ];
    for band in 0..20 {
        for (field_offset, field) in fields.iter().enumerate() {
            let index = 5 + band * 5 + field_offset;
            let actual = handle.param_info(index);
            assert_eq!(
                actual,
                (
                    format!("band_{band}_{}", field.key),
                    format!("Band {} {}", band + 1, field.name),
                    field.unit.to_string(),
                    field.min,
                    field.max,
                    field.default,
                    field.steps,
                    field.logarithmic,
                ),
                "legacy address {index} metadata drifted from frozen capture"
            );
        }
    }

    // Accepted oversampling correction: C ABI choice indices 0/1/2 select
    // DSP factors 1/2/4 (normalized 0.0/0.5/1.0). State serializes factors,
    // not indices; reload preserves the factor and the normalized reading.
    for (normalized, factor) in [(0.0, 1), (0.5, 2), (1.0, 4)] {
        assert_eq!(
            handle.set_normalized("oversampling", normalized),
            0,
            "{}",
            abi_last_error()
        );
        let saved: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&handle.save()).unwrap();
        assert_eq!(
            saved.get("oversampling"),
            Some(&serde_json::json!(factor)),
            "normalized {normalized} must save factor {factor}"
        );
        assert!(
            (handle.get_normalized("oversampling") - normalized).abs() < 1e-12,
            "oversampling must read back normalized {normalized}"
        );
    }
    // Default is Off (choice 0, factor 1).
    let fresh = AbiHandle::create("EQ", "{}", 2, 2);
    assert!(fresh.get_normalized("oversampling").abs() < 1e-12);
    let fresh_saved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&fresh.save()).unwrap();
    assert_eq!(
        fresh_saved.get("oversampling"),
        Some(&serde_json::json!(1)),
        "default oversampling must save factor 1"
    );
}

// ---------------------------------------------------------------------------
// EQ state: placement/pairs round-trip, partial merge, full presets.
// ---------------------------------------------------------------------------

#[test]
fn eq_placement_and_pairs_round_trip_through_c_abi() {
    let config = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 12.0, "placement": "left"}
        ],
        "stereo_pairs": [[0, 1]],
    })
    .to_string();
    let source = AbiHandle::create("EQ", &config, 2, 2);
    let saved = source.save();
    let saved_map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&saved).unwrap();
    assert_eq!(saved_map.get("filter_0_placement"), Some(&serde_json::json!(2)));
    assert_eq!(
        saved_map.get("stereo_pairs"),
        Some(&serde_json::json!([[0, 1]]))
    );

    let mut target = AbiHandle::create("EQ", &config, 2, 2);
    assert_eq!(target.load(&saved), 0, "{}", abi_last_error());
    assert_eq!(
        target.config_json()["stereo_pairs"],
        serde_json::json!([[0, 1]])
    );
    let resaved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&target.save()).unwrap();
    assert_eq!(resaved.get("filter_0_placement"), Some(&serde_json::json!(2)));

    // Left-only +12 dB peak: left RMS must clearly exceed right RMS.
    let input = sine_interleaved(4096, 2, 1000.0, 48_000.0);
    let output = target.process(&input);
    let left = channel_rms(&output, 2, 0);
    let right = channel_rms(&output, 2, 1);
    assert!(left > 0.3, "boosted left RMS too small: {left}");
    assert!(right > 0.05, "right channel must still pass audio: {right}");
    assert!(
        left > right * 2.0,
        "left-only placement must boost left over right: {left} vs {right}"
    );
}

#[test]
fn eq_legacy_partial_state_merges_with_live_values() {
    let config = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 2000.0, "q": 2.0, "db_gain": 0.0}
        ],
    })
    .to_string();
    let mut handle = AbiHandle::create("EQ", &config, 2, 2);
    let before: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();

    let partial = serde_json::json!({"band_0_gain": 6.0});
    let partial_bytes = serde_json::to_vec(&partial).unwrap();
    assert_eq!(handle.load(&partial_bytes), 0, "{}", abi_last_error());

    let after: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();
    assert_eq!(after.get("band_0_gain"), Some(&serde_json::json!(6.0)));
    for key in ["band_0_freq", "band_0_q", "band_0_filter_type", "band_0_order"] {
        assert_eq!(after.get(key), before.get(key), "{key} must be retained");
    }
    // Unknown foreign keys keep the legacy lenient behavior (ignored).
    let foreign = serde_json::json!({"band_0_gain": 3.0, "future_host_key": 1});
    let foreign_bytes = serde_json::to_vec(&foreign).unwrap();
    assert_eq!(handle.load(&foreign_bytes), 0, "{}", abi_last_error());
    let merged: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();
    assert_eq!(merged.get("band_0_gain"), Some(&serde_json::json!(3.0)));
}

#[test]
fn eq_full_structured_preset_retains_band_order_and_advanced_config() {
    // Order matters: peak, warped shelf, Kautz section, with placements.
    let advanced = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 500.0, "q": 1.0, "db_gain": 3.0},
            {"filter_type": "highshelf", "freq": 8000.0, "q": 0.7, "db_gain": -2.0,
             "topology": "warped_biquad", "lambda": 0.4, "placement": "stereo"},
            {"filter_type": "peak", "freq": 120.0, "q": 4.0, "db_gain": 0.0,
             "topology": "kautz_filter",
             "kautz_sections": [{"pole_freq": 120.0, "q": 4.0, "gain": 1.0}]},
        ],
        "stereo_pairs": [[0, 1]],
    })
    .to_string();
    let mut plain = AbiHandle::create("EQ", "{}", 2, 2);
    // A full preset carries the structured filter vector plus scalars.
    let preset = serde_json::json!({
        "filters": serde_json::from_str::<serde_json::Value>(&advanced).unwrap()["filters"],
        "stereo_pairs": [[0, 1]],
        "band_0_gain": 3.0,
    });
    let preset_bytes = serde_json::to_vec(&preset).unwrap();
    assert_eq!(plain.load(&preset_bytes), 0, "{}", abi_last_error());

    let config = plain.config_json();
    let filters = config["filters"].as_array().unwrap();
    assert_eq!(filters.len(), 3, "band order must carry three entries");
    assert_eq!(filters[0]["filter_type"], serde_json::json!("peak"));
    assert_eq!(filters[1]["topology"], serde_json::json!("warped_biquad"));
    assert_eq!(filters[1]["lambda"], serde_json::json!(0.4));
    assert_eq!(filters[1]["placement"], serde_json::json!("stereo"));
    assert_eq!(filters[2]["topology"], serde_json::json!("kautz_filter"));
    assert_eq!(
        filters[2]["kautz_sections"],
        serde_json::json!([{"pole_freq": 120.0, "q": 4.0, "gain": 1.0}])
    );

    // Same-advanced reload on a matching handle renders identically.
    let mut reference = AbiHandle::create("EQ", &advanced, 2, 2);
    let input = sine_interleaved(2048, 2, 440.0, 48_000.0);
    reference.reset();
    let expected = reference.process(&input);
    plain.reset();
    let actual = plain.process(&input);
    assert_eq!(actual.len(), expected.len());
    for (index, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1e-5,
            "sample {index} diverged: {got} vs {want}"
        );
    }
}

#[test]
fn eq_placement_live_edit_rejected_and_failed_restore_rolls_back() {
    let config = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 6.0}
        ],
    })
    .to_string();
    let mut handle = AbiHandle::create("EQ", &config, 2, 2);

    let id = CString::new("filter_0_placement").unwrap();
    assert_ne!(
        plugin_set_parameter(handle.0, id.as_ptr(), 0.4),
        0,
        "live placement edits must require state restoration"
    );
    assert!(
        abi_last_error().contains("restoration"),
        "rejection must name restoration: {}",
        abi_last_error()
    );

    let input = sine_interleaved(1024, 2, 1000.0, 48_000.0);
    handle.reset();
    let before_audio = handle.process(&input);
    let before_state = handle.save();
    let before_config = handle.config_json();

    // Out-of-range placement index must fail without touching the live path.
    let bad = serde_json::json!({"filter_0_placement": 9});
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    assert_ne!(handle.load(&bad_bytes), 0);
    assert!(
        abi_last_error().contains("placement"),
        "error must name placement: {}",
        abi_last_error()
    );
    assert_eq!(handle.save(), before_state, "state must roll back");
    assert_eq!(handle.config_json(), before_config, "config must roll back");
    handle.reset();
    let after_audio = handle.process(&input);
    assert_eq!(after_audio, before_audio, "audio must roll back");
}

#[test]
fn eq_32_channel_route_renders_nonzero() {
    let config = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 6.0}
        ],
    })
    .to_string();
    let mut handle = AbiHandle::create("EQ", &config, 32, 32);
    assert_eq!(handle.param_count(), 125);
    let input = sine_interleaved(1024, 32, 1000.0, 48_000.0);
    let output = handle.process(&input);
    assert_eq!(output.len(), 1024 * 32);
    for channel in 0..32 {
        let rms = channel_rms(&output, 32, channel);
        assert!(
            rms > 0.4,
            "channel {channel} must render boosted audio, got RMS {rms}"
        );
    }
}

// ---------------------------------------------------------------------------
// DynamicEQ: Tilt, placement, pairs, preset defaults, rollback.
// ---------------------------------------------------------------------------

#[test]
fn dynamic_eq_routing_appended_after_shelf_block_with_tilt_label() {
    let handle = AbiHandle::create("DynamicEQ", "{}", 2, 2);
    assert_eq!(handle.param_count(), 8 + 8 * 7 + 8 * 2 + 8);
    assert_eq!(handle.param_id(8 + 8 * 7), "band_0_shape");
    let routing_start = 8 + 8 * 7 + 8 * 2;
    for band in 0..8 {
        assert_eq!(
            handle.param_id(routing_start + band),
            format!("band_{band}_placement")
        );
    }
    let shape_index = 8 + 8 * 7;
    assert_eq!(
        handle.choice_label(shape_index, 3).as_deref(),
        Some("Tilt")
    );
    assert_eq!(
        handle.choice_label(routing_start, 0).as_deref(),
        Some("Stereo")
    );
    assert_eq!(
        handle.choice_label(routing_start, 4).as_deref(),
        Some("Side")
    );
    assert_eq!(handle.choice_label(routing_start, 5), None);
}

#[test]
fn dynamic_eq_tilt_placement_and_pairs_restore_and_render() {
    let config = serde_json::json!({
        "num_bands": 2,
        "bands": [
            {"frequency": 100.0, "q": 1.0, "gain": -6.0},
            {"frequency": 5000.0, "q": 1.0, "gain": 6.0}
        ],
        "stereo_pairs": [[0, 1]],
    })
    .to_string();
    let mut handle = AbiHandle::create("DynamicEQ", &config, 2, 2);

    let update = serde_json::json!({
        "band_0_shape": 3,
        "band_0_placement": 1,
        "band_1_placement": 2,
    });
    let update_bytes = serde_json::to_vec(&update).unwrap();
    assert_eq!(handle.load(&update_bytes), 0, "{}", abi_last_error());

    let rebuilt = handle.config_json();
    assert_eq!(rebuilt["bands"][0]["shape"], serde_json::json!("tilt"));
    assert_eq!(rebuilt["bands"][0]["placement"], serde_json::json!("left"));
    assert_eq!(rebuilt["bands"][1]["placement"], serde_json::json!("right"));

    let saved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();
    assert_eq!(saved.get("band_0_shape"), Some(&serde_json::json!(3)));
    assert_eq!(saved.get("band_0_placement"), Some(&serde_json::json!(1)));
    assert_eq!(
        saved.get("stereo_pairs"),
        Some(&serde_json::json!([[0, 1]])),
        "saved presets must retain pair routing"
    );

    // Live structural edits stay rejected through the C setter.
    for id in ["band_0_shape", "band_0_shelf_slope", "band_0_placement"] {
        assert_ne!(
            handle.set_normalized(id, 0.5),
            0,
            "{id} must require state restoration"
        );
    }

    let input = sine_interleaved(2048, 2, 440.0, 48_000.0);
    let output = handle.process(&input);
    assert!(peak(&output) > 0.01, "dynamic bands must pass audio");
}

#[test]
fn dynamic_eq_legacy_preset_defaults_to_peak_stereo() {
    // Legacy documents predate shelf and routing controls entirely.
    let mut handle = AbiHandle::create("DynamicEQ", "{}", 2, 2);
    let legacy = serde_json::json!({
        "num_bands": 1,
        "threshold": -20.0,
        "ratio": 2.0,
        "attack": 5.0,
        "release": 50.0,
        "knee": 6.0,
        "link_channels": true,
        "mix": 1.0,
        "band_0_frequency": 1000.0,
        "band_0_q": 1.0,
        "band_0_gain": 3.0,
        "band_0_band_threshold": -20.0,
        "band_0_band_ratio": 2.0,
        "band_0_active": true,
        "band_0_solo": false,
    });
    let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
    // Preset documents store the state as raw bytes, like the exporter.
    let document = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "ut_type": "org.spinorama.sotf.plugin-preset",
        "plugin_type": "DynamicEQ",
        "state": legacy_bytes,
    }))
    .unwrap();
    assert_eq!(
        plugin_import_preset_json(handle.0, document.as_ptr(), document.len()),
        0,
        "{}",
        abi_last_error()
    );
    let rebuilt = handle.config_json();
    assert_eq!(rebuilt["bands"][0]["shape"], serde_json::json!("peak"));
    assert_eq!(
        rebuilt["bands"][0]["placement"],
        serde_json::json!("stereo")
    );
}

#[test]
fn dynamic_eq_failed_restore_retains_audio_and_config() {
    let mut handle = AbiHandle::create("DynamicEQ", "{}", 2, 2);
    let input = sine_interleaved(1024, 2, 440.0, 48_000.0);
    handle.reset();
    let before_audio = handle.process(&input);
    let before_state = handle.save();
    let before_config = handle.config_json();

    let bad = serde_json::json!({"band_0_shape": 9});
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    assert_ne!(handle.load(&bad_bytes), 0);
    assert!(
        abi_last_error().contains("choice index"),
        "error must name the bad choice: {}",
        abi_last_error()
    );
    assert_eq!(handle.save(), before_state);
    assert_eq!(handle.config_json(), before_config);
    handle.reset();
    assert_eq!(handle.process(&input), before_audio);
}

// ---------------------------------------------------------------------------
// LinearPhaseEQ: placement/pairs/complete state.
// ---------------------------------------------------------------------------

#[test]
fn linear_phase_eq_template_matches_plugin_ids_and_count() {
    let handle = AbiHandle::create("LinearPhaseEQ", "{}", 2, 2);
    assert_eq!(handle.param_count(), 5 + 10 * 6);
    let fields = ["type", "freq", "q", "gain", "active", "placement"];
    for band in 0..10 {
        for (field, name) in fields.iter().enumerate() {
            assert_eq!(
                handle.param_id(5 + band * 6 + field),
                format!("band_{band}_{name}")
            );
        }
    }
    assert_eq!(
        handle.choice_label(10, 0).as_deref(),
        Some("Legacy"),
        "band_0_placement labels must resolve"
    );
    assert_eq!(handle.choice_label(10, 5).as_deref(), Some("Side"));

    // Band edits (including placement) are structural in the plugin; the C
    // setter must surface that rejection instead of silently dropping it.
    let mut handle = handle;
    assert_ne!(handle.set_normalized("band_0_placement", 1.0), 0);
    assert!(
        abi_last_error().contains("structural"),
        "linear placement rejection must name structure: {}",
        abi_last_error()
    );
}

#[test]
fn linear_phase_eq_placement_and_pairs_restore_and_render() {
    let config = serde_json::json!({
        "num_filters": 1,
        "filters": [
            {"filter_type": "Peak", "frequency": 1000.0, "q": 1.0, "gain_db": 6.0}
        ],
        "stereo_pairs": [[0, 1]],
    })
    .to_string();
    let mut handle = AbiHandle::create("LinearPhaseEQ", &config, 2, 2);

    let update = serde_json::json!({"band_0_placement": 2});
    let update_bytes = serde_json::to_vec(&update).unwrap();
    assert_eq!(handle.load(&update_bytes), 0, "{}", abi_last_error());
    assert_eq!(
        handle.config_json()["filters"][0]["placement"],
        serde_json::json!("left")
    );
    let saved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();
    assert_eq!(saved.get("band_0_placement"), Some(&serde_json::json!(2)));
    assert_eq!(
        saved.get("stereo_pairs"),
        Some(&serde_json::json!([[0, 1]]))
    );

    // Legacy index 0 clears placement back to the inherited route.
    let clear = serde_json::json!({"band_0_placement": 0});
    let clear_bytes = serde_json::to_vec(&clear).unwrap();
    assert_eq!(handle.load(&clear_bytes), 0, "{}", abi_last_error());
    assert!(
        handle.config_json()["filters"][0]
            .get("placement")
            .is_none(),
        "legacy placement must remove the key"
    );

    // FIR latency delays the signal; assert on the settled tail.
    let input = sine_interleaved(8192, 2, 1000.0, 48_000.0);
    let output = handle.process(&input);
    let tail = &output[output.len() - 2048..];
    assert!(
        peak(tail) > 0.3,
        "settled linear-phase output must be nonzero"
    );
}

#[test]
fn linear_phase_eq_rejects_bad_placement_without_touching_live_state() {
    let mut handle = AbiHandle::create("LinearPhaseEQ", "{}", 2, 2);
    let before_state = handle.save();
    let before_config = handle.config_json();
    let bad = serde_json::json!({"band_0_placement": 9});
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    assert_ne!(handle.load(&bad_bytes), 0);
    assert!(
        abi_last_error().contains("placement"),
        "error must name placement: {}",
        abi_last_error()
    );
    assert_eq!(handle.save(), before_state);
    assert_eq!(handle.config_json(), before_config);
}

// ---------------------------------------------------------------------------
// De-esser: new controls, sidechain routes, structural reload.
// ---------------------------------------------------------------------------

#[test]
fn de_esser_maps_14_params_with_new_controls_last() {
    let handle = AbiHandle::create("DeEsser", "{}", 2, 2);
    assert_eq!(handle.param_count(), 14);
    let expected = [
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
    for (index, id) in expected.iter().enumerate() {
        assert_eq!(handle.param_id(index), *id);
    }
    // M/S mode is realtime and round-trips through the normalized C ABI;
    // lookahead is structural and its live edit must be rejected.
    let mut handle = handle;
    assert_eq!(handle.set_normalized("ms_mode", 1.0), 0);
    assert_eq!(handle.get_normalized("ms_mode"), 1.0);
    assert_ne!(handle.set_normalized("lookahead_ms", 0.5), 0);
    assert!(
        abi_last_error().contains("structural"),
        "lookahead rejection must name structure: {}",
        abi_last_error()
    );
}

#[test]
fn de_esser_internal_and_external_routes_render() {
    let mut internal = AbiHandle::create("DeEsser", "{}", 2, 2);
    let input = sine_interleaved(2048, 2, 1000.0, 48_000.0);
    let output = internal.process(&input);
    assert!(peak(&output) > 0.1, "internal route must pass audio");

    let external_config = serde_json::json!({"sidechain_external": true}).to_string();
    let mut external = AbiHandle::create("DeEsser", &external_config, 4, 2);
    // Program sine plus a silent key bus.
    let mut keyed = vec![0.0; 2048 * 4];
    for frame in 0..2048 {
        let sample = input[frame * 2];
        keyed[frame * 4] = sample;
        keyed[frame * 4 + 1] = sample;
    }
    let keyed_out = external.process(&keyed);
    assert_eq!(keyed_out.len(), 2048 * 2);
    assert!(peak(&keyed_out) > 0.1, "external route must pass program");
}

#[test]
fn de_esser_structural_reload_rebuilds_and_sidechain_flip_rolls_back() {
    let mut handle = AbiHandle::create("DeEsser", "{}", 2, 2);
    let topology = serde_json::json!({"lookahead_ms": 2.0, "split_topology": 1});
    let topology_bytes = serde_json::to_vec(&topology).unwrap();
    assert_eq!(handle.load(&topology_bytes), 0, "{}", abi_last_error());
    let rebuilt = handle.config_json();
    assert_eq!(rebuilt["lookahead_ms"], serde_json::json!(2.0));
    assert_eq!(
        rebuilt["split_topology"],
        serde_json::json!("Linear-Phase")
    );
    let input = sine_interleaved(1024, 2, 6000.0, 48_000.0);
    assert!(peak(&handle.process(&input)) > 0.01);

    // Flipping the key route changes the bus layout, which a live handle
    // cannot adopt: the restore must fail and preserve everything.
    let before_state = handle.save();
    let before_config = handle.config_json();
    handle.reset();
    let before_audio = handle.process(&input);
    let flip = serde_json::json!({"sidechain_external": true});
    let flip_bytes = serde_json::to_vec(&flip).unwrap();
    assert_ne!(handle.load(&flip_bytes), 0);
    assert!(
        abi_last_error().contains("input channels"),
        "error must name the bus mismatch: {}",
        abi_last_error()
    );
    assert_eq!(handle.save(), before_state);
    assert_eq!(handle.config_json(), before_config);
    handle.reset();
    assert_eq!(handle.process(&input), before_audio);
}

// ---------------------------------------------------------------------------
// Ambisonics: named/custom construction, truthful rejection, reload.
// ---------------------------------------------------------------------------

const CUSTOM_STEREO_LAYOUT: &str = r#"{
    "name": "stereo",
    "speakers": [
        {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
        {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
    ]
}"#;

fn custom_ambisonics_config() -> String {
    serde_json::json!({
        "order": 1,
        "target_layout": "custom",
        "custom_layout": serde_json::from_str::<serde_json::Value>(CUSTOM_STEREO_LAYOUT).unwrap(),
    })
    .to_string()
}

#[test]
fn ambisonics_maps_5_params_with_custom_index_last() {
    let handle = AbiHandle::create(
        "AmbisonicsDecoder",
        r#"{"order": 1, "target_layout": "5.1"}"#,
        4,
        6,
    );
    assert_eq!(handle.param_count(), 5);
    let expected = [
        "order",
        "target_layout",
        "max_re_weighting",
        "dual_band",
        "algorithm",
    ];
    for (index, id) in expected.iter().enumerate() {
        assert_eq!(handle.param_id(index), *id);
    }
    // Custom is appended after the eight named layouts: index 8.
    let custom = AbiHandle::create("AmbisonicsDecoder", &custom_ambisonics_config(), 4, 2);
    let saved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&custom.save()).unwrap();
    assert_eq!(saved.get("target_layout"), Some(&serde_json::json!(8)));
}

#[test]
fn ambisonics_named_and_custom_routes_render_nonzero() {
    // Order-1 W-channel energy must reach every output speaker.
    let w_only = |frames: usize| {
        let mut input = vec![0.0; frames * 4];
        for frame in 0..frames {
            input[frame * 4] = 0.5;
        }
        input
    };

    let mut named = AbiHandle::create(
        "AmbisonicsDecoder",
        r#"{"order": 1, "target_layout": "5.1"}"#,
        4,
        6,
    );
    let named_out = named.process(&w_only(512));
    assert_eq!(named_out.len(), 512 * 6);
    assert!(
        peak(&named_out) > 0.01,
        "named decode must spread W energy"
    );

    let mut custom = AbiHandle::create("AmbisonicsDecoder", &custom_ambisonics_config(), 4, 2);
    let custom_out = custom.process(&w_only(512));
    assert_eq!(custom_out.len(), 512 * 2);
    assert!(
        channel_rms(&custom_out, 2, 0) > 0.01 && channel_rms(&custom_out, 2, 1) > 0.01,
        "custom stereo decode must drive both speakers"
    );
}

#[test]
fn ambisonics_bare_custom_rejected_and_geometry_survives_reload() {
    // Bare "custom" without geometry fails closed and names the limitation.
    let kind = CString::new("AmbisonicsDecoder").unwrap();
    let bare = CString::new(r#"{"order": 1, "target_layout": "custom"}"#).unwrap();
    let failed = plugin_create(kind.as_ptr(), bare.as_ptr(), 48_000, 4, 2);
    assert!(failed.is_null(), "bare custom must not construct");
    assert!(
        abi_last_error().contains("custom_layout"),
        "rejection must name the missing geometry: {}",
        abi_last_error()
    );

    // Structural reloads preserve custom geometry across the rebuild.
    let mut custom = AbiHandle::create("AmbisonicsDecoder", &custom_ambisonics_config(), 4, 2);
    let update = serde_json::json!({"dual_band": true});
    let update_bytes = serde_json::to_vec(&update).unwrap();
    assert_eq!(custom.load(&update_bytes), 0, "{}", abi_last_error());
    let rebuilt = custom.config_json();
    assert_eq!(rebuilt["target_layout"], serde_json::json!("custom"));
    assert_eq!(
        rebuilt["custom_layout"]["name"],
        serde_json::json!("stereo")
    );
    assert_eq!(rebuilt["dual_band"], serde_json::json!(true));

    // Geometry survives a full save/load cycle through the same handle.
    let saved = custom.save();
    assert_eq!(custom.load(&saved), 0, "{}", abi_last_error());
    assert_eq!(
        custom.config_json()["custom_layout"]["name"],
        serde_json::json!("stereo")
    );
}

#[test]
fn ambisonics_layout_changing_retarget_fails_with_rollback() {
    let mut custom = AbiHandle::create("AmbisonicsDecoder", &custom_ambisonics_config(), 4, 2);
    let before_state = custom.save();
    let before_config = custom.config_json();
    // "5.1" needs 6 outputs; this handle owns 2.
    let retarget = serde_json::json!({"target_layout": 1});
    let retarget_bytes = serde_json::to_vec(&retarget).unwrap();
    assert_ne!(custom.load(&retarget_bytes), 0);
    assert!(
        abi_last_error().contains("output channels"),
        "error must name the bus mismatch: {}",
        abi_last_error()
    );
    assert_eq!(custom.save(), before_state);
    assert_eq!(custom.config_json(), before_config);
}

// ---------------------------------------------------------------------------
// Restoration and speech controls flow through the generic contracts.
// ---------------------------------------------------------------------------

#[test]
fn restoration_and_speech_mapping_counts_and_new_controls() {
    let hiss = AbiHandle::create("HissReducer", "{}", 2, 2);
    assert_eq!(hiss.param_count(), 13);
    let hiss_ids: Vec<String> = (0..13).map(|index| hiss.param_id(index)).collect();
    for id in [
        "enabled",
        "strength",
        "spectral_mode",
        "link_mode",
        "transient_guard",
    ] {
        assert!(hiss_ids.contains(&id.to_string()), "hiss lacks {id}");
    }
    // The guard is appended last; legacy numeric addresses never move.
    assert_eq!(hiss.param_id(12), "transient_guard");

    let declick = AbiHandle::create("Declick", "{}", 2, 2);
    assert_eq!(declick.param_count(), 9);
    let declick_ids: Vec<String> = (0..9).map(|index| declick.param_id(index)).collect();
    for id in [
        "enabled",
        "mode",
        "bands",
        "audition_residual",
    ] {
        assert!(declick_ids.contains(&id.to_string()), "declick lacks {id}");
    }

    let speech = AbiHandle::create("SpeechDenoiser", "{}", 2, 2);
    assert_eq!(speech.param_count(), 3);
    assert_eq!(speech.param_id(0), "enabled");
    assert_eq!(speech.param_id(1), "strength");
    assert_eq!(speech.param_id(2), "model");

    let denoiser = AbiHandle::create("Denoiser", "{}", 2, 2);
    assert!(denoiser.param_count() > 20, "denoiser PARAMS must map");
    let denoiser_ids: Vec<String> = (0..denoiser.param_count())
        .map(|index| denoiser.param_id(index))
        .collect();
    for id in ["reduction_db", "use_captured_profile", "formant_preservation"] {
        assert!(denoiser_ids.contains(&id.to_string()), "denoiser lacks {id}");
    }
}

#[test]
fn restoration_state_round_trips_and_renders() {
    let mut hiss = AbiHandle::create("HissReducer", "{}", 2, 2);
    let hiss_state = serde_json::json!({"strength": 0.75});
    let hiss_bytes = serde_json::to_vec(&hiss_state).unwrap();
    assert_eq!(hiss.load(&hiss_bytes), 0, "{}", abi_last_error());
    let resaved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&hiss.save()).unwrap();
    assert_eq!(resaved.get("strength"), Some(&serde_json::json!(0.75)));
    let input = sine_interleaved(2048, 2, 440.0, 48_000.0);
    let hissed = hiss.process(&input);
    assert!(peak(&hissed) > 0.05, "hiss reducer must pass tone");

    let mut declick = AbiHandle::create("Declick", "{}", 2, 2);
    let declick_state = serde_json::json!({"mode": 1});
    let declick_bytes = serde_json::to_vec(&declick_state).unwrap();
    assert_eq!(declick.load(&declick_bytes), 0, "{}", abi_last_error());
    let declicked = declick.process(&input);
    assert!(peak(&declicked) > 0.05, "declicker must pass tone");

    // The plugin contract clamps out-of-range choice writes; the FFI
    // propagates that behavior instead of inventing its own rejection.
    let clamped = serde_json::json!({"mode": 99});
    let clamped_bytes = serde_json::to_vec(&clamped).unwrap();
    assert_eq!(declick.load(&clamped_bytes), 0, "{}", abi_last_error());
    let resaved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&declick.save()).unwrap();
    assert_eq!(resaved.get("mode"), Some(&serde_json::json!(1)));

    // Failed restores keep the generic transactional guarantee too.
    let before = declick.save();
    let bad = serde_json::json!({"sensitivity": "not-a-number"});
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    assert_ne!(declick.load(&bad_bytes), 0);
    assert_eq!(declick.save(), before);
}

#[test]
fn hiss_transient_guard_forwards_through_c_abi_and_survives_restore() {
    // The guard is a plain realtime bool: live C ABI sets are accepted
    // post-init (allocation-free flag store, no structural rebuild), read
    // back exactly, and survive save/load with the graph populated.
    let mut hiss = AbiHandle::create("HissReducer", "{}", 2, 2);
    assert_eq!(hiss.get_normalized("transient_guard"), 0.0);
    assert_eq!(hiss.set_normalized("transient_guard", 1.0), 0);
    assert_eq!(hiss.get_normalized("transient_guard"), 1.0);

    let input = sine_interleaved(2048, 2, 440.0, 48_000.0);
    hiss.reset();
    let first = hiss.process(&input);
    assert!(peak(&first) > 0.05, "guard-on hiss must pass tone");

    let saved = hiss.save();
    let resaved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&saved).unwrap();
    assert_eq!(
        resaved.get("transient_guard"),
        Some(&serde_json::json!(true))
    );

    // Restore onto a fresh handle, then prove the live bool path still
    // forwards after restoration with a populated graph.
    let mut restored = AbiHandle::create("HissReducer", "{}", 2, 2);
    assert_eq!(restored.load(&saved), 0, "{}", abi_last_error());
    assert_eq!(restored.get_normalized("transient_guard"), 1.0);
    restored.reset();
    let revived = restored.process(&input);
    assert!(peak(&revived) > 0.05, "restored hiss must pass tone");
    assert_eq!(restored.set_normalized("transient_guard", 0.0), 0);
    assert_eq!(restored.get_normalized("transient_guard"), 0.0);
    restored.reset();
    let disengaged = restored.process(&input);
    assert!(peak(&disengaged) > 0.05, "guard-off hiss must pass tone");
}

#[test]
fn hiss_structural_triggers_require_restoration_and_preserve_audio() {
    // `spectral_mode` flips the DSP engine and the plugin rejects live
    // flips with an allocating `Err(String)`; the FFI guard converts that
    // into the uniform restoration refusal before any render-thread
    // allocation can happen. The capture triggers are momentary control
    // commands with the same routing. Post-drain `Err(String)` failures
    // stay control-thread-only by the same contract: the render loop sees
    // only accepted realtime values, never a trigger call.
    let mut hiss = AbiHandle::create("HissReducer", "{}", 2, 2);
    let input = sine_interleaved(2048, 2, 440.0, 48_000.0);
    for id in ["spectral_mode", "learn_noise", "clear_profile"] {
        assert_structural_refusal_preserves(&mut hiss, id, &input);
    }
    // The guard itself stays live while the triggers are refused.
    assert_eq!(hiss.set_normalized("transient_guard", 1.0), 0);
    assert_eq!(hiss.get_normalized("transient_guard"), 1.0);
}

#[test]
fn speech_state_round_trips_and_silence_stays_finite() {
    let mut speech = AbiHandle::create("SpeechDenoiser", "{}", 2, 2);
    let state = serde_json::json!({"strength": 0.5, "model": 0});
    let state_bytes = serde_json::to_vec(&state).unwrap();
    assert_eq!(speech.load(&state_bytes), 0, "{}", abi_last_error());
    let resaved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&speech.save()).unwrap();
    assert_eq!(resaved.get("strength"), Some(&serde_json::json!(0.5)));

    // The model path must stay finite on digital silence.
    let silence = vec![0.0; 512 * 2];
    let output = speech.process(&silence);
    assert_eq!(output.len(), 512 * 2);
}

// ---------------------------------------------------------------------------
// P0-3: structural live-edit guards refuse with restoration contract.
// ---------------------------------------------------------------------------

/// Asserts a structural live-edit refusal preserves audio/state/config.
fn assert_structural_refusal_preserves(
    handle: &mut AbiHandle,
    param_id: &str,
    input: &[f32],
) {
    handle.reset();
    let before_audio = handle.process(input);
    assert!(
        peak(&before_audio) > 0.0,
        "{param_id} refusal test needs nonzero pre-audio"
    );
    let before_state = handle.save();
    let before_config = handle.config_json();

    assert_ne!(
        handle.set_normalized(param_id, 0.75),
        0,
        "{param_id} live edit must require state restoration"
    );
    let error = abi_last_error();
    assert!(
        error.contains("restoration"),
        "{param_id} rejection must name state restoration, got: {error}"
    );
    assert!(
        error.contains("structural"),
        "{param_id} rejection must name structural semantics, got: {error}"
    );

    assert_eq!(handle.save(), before_state, "{param_id} state must roll back");
    assert_eq!(
        handle.config_json(),
        before_config,
        "{param_id} config must roll back"
    );
    handle.reset();
    assert_eq!(
        handle.process(input),
        before_audio,
        "{param_id} audio must roll back"
    );
}

#[test]
fn structural_live_edits_require_restoration_across_families() {
    // LinearPhaseEQ placement (structural FIR rebuild).
    let mut linear = AbiHandle::create("LinearPhaseEQ", "{}", 2, 2);
    let linear_input = sine_interleaved(2048, 2, 440.0, 48_000.0);
    assert_structural_refusal_preserves(&mut linear, "band_3_placement", &linear_input);

    // De-esser structural IDs (detection band, mode, lookahead, topology,
    // sidechain route). Realtime controls (e.g. ms_mode) stay live.
    let mut de_esser = AbiHandle::create("DeEsser", "{}", 2, 2);
    let de_esser_input = sine_interleaved(2048, 2, 6000.0, 48_000.0);
    for id in [
        "frequency",
        "q",
        "mode",
        "lookahead_ms",
        "split_topology",
        "sidechain_external",
    ] {
        assert_structural_refusal_preserves(&mut de_esser, id, &de_esser_input);
    }
    // Realtime control still round-trips.
    assert_eq!(de_esser.set_normalized("ms_mode", 1.0), 0);
    assert!(
        (de_esser.get_normalized("ms_mode") - 1.0).abs() < 1e-12,
        "ms_mode must read back 1.0"
    );

    // Ambisonics: every parameter is structural.
    let mut ambisonics = AbiHandle::create(
        "AmbisonicsDecoder",
        r#"{"order": 1, "target_layout": "5.1"}"#,
        4,
        6,
    );
    let mut ambisonics_input = vec![0.0; 512 * 4];
    for frame in 0..512 {
        ambisonics_input[frame * 4] = 0.5;
    }
    for id in [
        "order",
        "target_layout",
        "max_re_weighting",
        "dual_band",
        "algorithm",
    ] {
        assert_structural_refusal_preserves(&mut ambisonics, id, &ambisonics_input);
    }

    // EQ-family constructor-only keys have no live setters.
    let mut eq = AbiHandle::create("EQ", "{}", 2, 2);
    let eq_input = sine_interleaved(1024, 2, 1000.0, 48_000.0);
    for id in ["stereo_pairs", "filters", "channel_filters"] {
        assert_structural_refusal_preserves(&mut eq, id, &eq_input);
    }
    let mut dynamic = AbiHandle::create("DynamicEQ", "{}", 2, 2);
    let dynamic_input = sine_interleaved(1024, 2, 440.0, 48_000.0);
    for id in ["stereo_pairs", "bands"] {
        assert_structural_refusal_preserves(&mut dynamic, id, &dynamic_input);
    }
}

// ---------------------------------------------------------------------------
// P1-2: beyond-bank placement slots and canonical index consistency.
// ---------------------------------------------------------------------------

#[test]
fn eq_beyond_bank_placement_fails_truthfully_and_state_rejected_with_rollback() {
    // 1-filter bank: slots 1..20 exist statically but have no live filter.
    let config = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 6.0}
        ],
    })
    .to_string();
    let mut handle = AbiHandle::create("EQ", &config, 2, 2);

    // Get on a beyond-bank slot returns the -1.0 sentinel (DSP has no value).
    assert!(
        (handle.get_normalized("filter_19_placement") + 1.0).abs() < f64::EPSILON,
        "beyond-bank get must return the error sentinel"
    );
    // Live set is rejected by the structural guard (all placement edits go
    // through state restoration, including beyond-bank addresses).
    assert_ne!(handle.set_normalized("filter_19_placement", 0.4), 0);
    assert!(
        abi_last_error().contains("restoration"),
        "beyond-bank set must name restoration: {}",
        abi_last_error()
    );

    // State carrying a beyond-bank key is rejected transactionally.
    let input = sine_interleaved(1024, 2, 1000.0, 48_000.0);
    handle.reset();
    let before_audio = handle.process(&input);
    let before_state = handle.save();
    let before_config = handle.config_json();
    let bad = serde_json::json!({"filter_19_placement": 2});
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    assert_ne!(handle.load(&bad_bytes), 0);
    assert_eq!(handle.save(), before_state, "state must roll back");
    assert_eq!(handle.config_json(), before_config, "config must roll back");
    handle.reset();
    assert_eq!(handle.process(&input), before_audio, "audio must roll back");
}

#[test]
fn eq_noncanonical_placement_indices_fail_closed_everywhere() {
    let mut handle = AbiHandle::create("EQ", "{}", 2, 2);
    // Live addressing: noncanonical IDs are unknown (guard + map both fail
    // closed; no silent normalization to the canonical slot).
    assert_ne!(handle.set_normalized("filter_01_placement", 0.5), 0);
    assert!(
        abi_last_error().contains("Unknown parameter"),
        "noncanonical set must report unknown parameter: {}",
        abi_last_error()
    );
    assert!(
        (handle.get_normalized("filter_01_placement") + 1.0).abs() < f64::EPSILON,
        "noncanonical get must return the error sentinel"
    );

    // State merge: noncanonical indices are malformed and rejected.
    let before_state = handle.save();
    let before_config = handle.config_json();
    let bad = serde_json::json!({"filter_01_placement": 1});
    let bad_bytes = serde_json::to_vec(&bad).unwrap();
    assert_ne!(handle.load(&bad_bytes), 0);
    assert!(
        abi_last_error().contains("noncanonical"),
        "noncanonical state must name the violation: {}",
        abi_last_error()
    );
    assert_eq!(handle.save(), before_state);
    assert_eq!(handle.config_json(), before_config);
}

// ---------------------------------------------------------------------------
// P1-3: ParameterMap rebuilt on structural restore, old pointers stable.
// ---------------------------------------------------------------------------

#[test]
fn structural_restore_rebuilds_map_and_keeps_old_info_pointers_valid() {
    // EQ bank-length change via full structural preset.
    let mut eq = AbiHandle::create("EQ", "{}", 2, 2);
    let old_info = plugin_get_parameter_info(eq.0, 105);
    assert!(!old_info.is_null());
    // SAFETY: Borrowed from the live handle; copied out before restore.
    let old_id = unsafe { CStr::from_ptr((*old_info).id) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(old_id, "filter_0_placement");

    let preset = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 500.0, "q": 1.0, "db_gain": 3.0},
            {"filter_type": "peak", "freq": 2000.0, "q": 1.0, "db_gain": -3.0},
        ],
        "stereo_pairs": [[0, 1]],
    });
    let preset_bytes = serde_json::to_vec(&preset).unwrap();
    assert_eq!(eq.load(&preset_bytes), 0, "{}", abi_last_error());

    // Old pointer still valid (retired map kept alive) with the same ID.
    // SAFETY: The retired map outlives the handle; the pointer was never freed.
    let retired_id = unsafe { CStr::from_ptr((*old_info).id) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(retired_id, "filter_0_placement");
    // New map serves the committed state.
    assert_eq!(eq.param_count(), 125);
    assert_eq!(eq.param_id(105), "filter_0_placement");
    let saved: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&eq.save()).unwrap();
    assert_eq!(
        saved.get("stereo_pairs"),
        Some(&serde_json::json!([[0, 1]]))
    );

    // De-esser structural restore also retires the map.
    let mut de_esser = AbiHandle::create("DeEsser", "{}", 2, 2);
    let old_de_esser = plugin_get_parameter_info(de_esser.0, 10);
    assert!(!old_de_esser.is_null());
    let update = serde_json::json!({"lookahead_ms": 5.0});
    let update_bytes = serde_json::to_vec(&update).unwrap();
    assert_eq!(de_esser.load(&update_bytes), 0, "{}", abi_last_error());
    // SAFETY: Same retired-map guarantee as above.
    let retired_de_esser = unsafe { CStr::from_ptr((*old_de_esser).id) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(retired_de_esser, "lookahead_ms");
    assert_eq!(de_esser.param_id(10), "lookahead_ms");
}

// ---------------------------------------------------------------------------
// P1-7: per-family placement index semantics under one suffix.
// ---------------------------------------------------------------------------

#[test]
fn placement_index_zero_means_legacy_for_eq_linear_but_stereo_for_dynamic() {
    // EQ: placement 0 removes the key (legacy/inherit).
    let mut eq = AbiHandle::create(
        "EQ",
        r#"{"filters": [{"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 0.0}]}"#,
        2,
        2,
    );
    let eq_clear = serde_json::json!({"filter_0_placement": 0});
    let eq_bytes = serde_json::to_vec(&eq_clear).unwrap();
    assert_eq!(eq.load(&eq_bytes), 0, "{}", abi_last_error());
    assert!(
        eq.config_json()["filters"][0].get("placement").is_none(),
        "EQ placement 0 must omit the key, got: {}",
        eq.config_json()["filters"][0]
    );

    // LinearPhaseEQ: placement 0 also removes the key.
    let mut linear = AbiHandle::create("LinearPhaseEQ", "{}", 2, 2);
    let linear_clear = serde_json::json!({"band_0_placement": 0});
    let linear_bytes = serde_json::to_vec(&linear_clear).unwrap();
    assert_eq!(linear.load(&linear_bytes), 0, "{}", abi_last_error());
    assert!(
        linear.config_json()["filters"][0]
            .get("placement")
            .is_none(),
        "Linear placement 0 must omit the key"
    );

    // DynamicEQ: placement 0 is explicit Stereo (no Legacy choice exists).
    let mut dynamic = AbiHandle::create("DynamicEQ", "{}", 2, 2);
    let dynamic_stereo = serde_json::json!({"band_0_placement": 0});
    let dynamic_bytes = serde_json::to_vec(&dynamic_stereo).unwrap();
    assert_eq!(dynamic.load(&dynamic_bytes), 0, "{}", abi_last_error());
    assert_eq!(
        dynamic.config_json()["bands"][0]["placement"],
        serde_json::json!("stereo"),
        "DynamicEQ placement 0 must be explicit Stereo, not Legacy"
    );

    // Tilt shape index 3 matches the DSP `DynEqShape` order.
    let tilt = serde_json::json!({"band_1_shape": 3});
    let tilt_bytes = serde_json::to_vec(&tilt).unwrap();
    assert_eq!(dynamic.load(&tilt_bytes), 0, "{}", abi_last_error());
    assert_eq!(
        dynamic.config_json()["bands"][1]["shape"],
        serde_json::json!("tilt")
    );
}

// ---------------------------------------------------------------------------
// P2-1: C ABI edge sentinels and choice-label bounds.
// ---------------------------------------------------------------------------

#[test]
fn ffi_edge_cases_return_sentinels_without_crashing() {
    // Null handle returns sentinels/nulls.
    assert!(
        (plugin_get_parameter(std::ptr::null(), std::ptr::null()) + 1.0).abs() < f64::EPSILON,
        "null handle get must return -1.0"
    );
    assert!(plugin_get_parameter_info(std::ptr::null(), 0).is_null());
    assert!(plugin_get_parameter_choice_label(std::ptr::null(), 0, 0).is_null());
    assert_eq!(plugin_get_parameter_count(std::ptr::null()), 0);

    let mut handle = AbiHandle::create("EQ", "{}", 2, 2);
    // Unknown IDs fail truthfully.
    assert!(
        (handle.get_normalized("no_such_parameter") + 1.0).abs() < f64::EPSILON,
        "unknown get must return -1.0"
    );
    assert_ne!(handle.set_normalized("no_such_parameter", 0.5), 0);
    // Null param_id pointer fails without crashing.
    assert_ne!(
        plugin_set_parameter(handle.0, std::ptr::null(), 0.5),
        0
    );
    assert!(
        (plugin_get_parameter(handle.0, std::ptr::null()) + 1.0).abs() < f64::EPSILON,
        "null ID get must return -1.0"
    );

    // Out-of-range parameter indices return null/None.
    assert!(plugin_get_parameter_info(handle.0, 9999).is_null());
    assert_eq!(handle.choice_label(9999, 0), None);
    // Out-of-range choice indices return None for every placement family.
    let eq_placement = 105;
    assert_eq!(handle.choice_label(eq_placement, 6), None);
    let linear = AbiHandle::create("LinearPhaseEQ", "{}", 2, 2);
    assert_eq!(linear.choice_label(10, 6), None);
    let dynamic = AbiHandle::create("DynamicEQ", "{}", 2, 2);
    let routing_start = 8 + 8 * 7 + 8 * 2;
    assert_eq!(dynamic.choice_label(routing_start, 5), None);
    // Non-choice parameters have no labels.
    assert_eq!(handle.choice_label(0, 0), None);
}

// ---------------------------------------------------------------------------
// P2-2: pair-injection fallback on corrupt config never fails save.
// ---------------------------------------------------------------------------

#[test]
fn corrupt_config_save_succeeds_without_pair_routing() {
    let config = serde_json::json!({
        "filters": [
            {"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 6.0}
        ],
        "stereo_pairs": [[0, 1]],
    })
    .to_string();
    let mut handle = AbiHandle::create("EQ", &config, 2, 2);
    let healthy: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();
    assert_eq!(
        healthy.get("stereo_pairs"),
        Some(&serde_json::json!([[0, 1]])),
        "healthy save must inject pairs"
    );

    // Corrupt the handle config (white-box test of the never-fail-save path).
    // SAFETY: Single-threaded test; no concurrent handle access.
    unsafe {
        (*handle.0).config_json = "not json{{".to_string();
    }
    // Save still succeeds (fallback is logged via `log::warn!`), but the
    // corrupt handle cannot shed routing quietly: the exported preset omits
    // the pair key instead of inventing one.
    let fallback: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&handle.save()).unwrap();
    assert!(
        fallback.get("stereo_pairs").is_none(),
        "corrupt-config save must omit pairs, got: {fallback:?}"
    );
    // Audio path unaffected by the corrupt config string.
    let input = sine_interleaved(1024, 2, 1000.0, 48_000.0);
    assert!(peak(&handle.process(&input)) > 0.1);
}

// ---------------------------------------------------------------------------
// P2-3: de-esser choice normalization input matrix.
// ---------------------------------------------------------------------------

#[test]
fn de_esser_choice_merge_accepts_labels_and_indices() {
    use crate::plugin_factory::merge_de_esser_state_into_config;

    // (state key, input JSON, expected canonical config label or None for Err)
    let cases: &[(&str, serde_json::Value, Option<&str>)] = &[
        ("mode", serde_json::json!("Wideband"), Some("Wideband")),
        ("mode", serde_json::json!("wideband"), Some("Wideband")),
        ("mode", serde_json::json!("Split-Band"), Some("Split-Band")),
        ("mode", serde_json::json!("split-band"), Some("Split-Band")),
        ("mode", serde_json::json!(0), Some("Wideband")),
        ("mode", serde_json::json!(1), Some("Split-Band")),
        ("mode", serde_json::json!("Bogus"), None),
        ("mode", serde_json::json!(2), None),
        (
            "split_topology",
            serde_json::json!("Minimum-Phase"),
            Some("Minimum-Phase"),
        ),
        (
            "split_topology",
            serde_json::json!("minimum-phase"),
            Some("Minimum-Phase"),
        ),
        (
            "split_topology",
            serde_json::json!("Linear-Phase"),
            Some("Linear-Phase"),
        ),
        (
            "split_topology",
            serde_json::json!("linear-phase"),
            Some("Linear-Phase"),
        ),
        ("split_topology", serde_json::json!(0), Some("Minimum-Phase")),
        ("split_topology", serde_json::json!(1), Some("Linear-Phase")),
        ("split_topology", serde_json::json!("Bogus"), None),
        ("split_topology", serde_json::json!(2), None),
    ];
    for (key, input, expected) in cases {
        let state = serde_json::json!({ *key: input });
        let state_bytes = serde_json::to_vec(&state).unwrap();
        let merged = merge_de_esser_state_into_config("{}", &state_bytes);
        match expected {
            Some(label) => {
                let config: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&merged.unwrap_or_else(|error| {
                        panic!("{key}={input} must merge: {error}")
                    }))
                    .unwrap();
                assert_eq!(
                    config.get(*key),
                    Some(&serde_json::json!(label)),
                    "{key}={input} must normalize to {label}"
                );
            }
            None => {
                assert!(
                    merged.is_err(),
                    "{key}={input} must be rejected, got: {merged:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// P2-4: custom Ambisonics triple-route equivalence (FFI/bridge/facade).
// ---------------------------------------------------------------------------

#[test]
fn ambisonics_triple_route_equivalence_ffi_bridge_facade() {
    use sotf_host::plugin::Plugin as _;

    let config = custom_ambisonics_config();
    let params: serde_json::Value = serde_json::from_str(&config).unwrap();

    // FFI route (direct DSP construction + output-width validation).
    let mut ffi = AbiHandle::create("AmbisonicsDecoder", &config, 4, 2);
    assert_eq!(ffi.inner().input_channels, 4);
    assert_eq!(ffi.inner().output_channels, 2);

    // Bridge route (shared-lane custom constructor).
    let mut bridge = plugins_bridge::create_plugin("AmbisonicsDecoder", 4, 48_000, &config)
        .unwrap_or_else(|error| panic!("bridge custom route failed: {error}"));
    assert_eq!(bridge.input_channels(), 4);
    assert_eq!(bridge.output_channels(), 2);

    // Facade route (shared-lane custom constructor).
    let mut facade =
        sotf_plugins::factory::create_plugin("ambisonics_decoder", &params, 4, 48_000)
            .unwrap_or_else(|error| panic!("facade custom route failed: {error}"));
    assert_eq!(facade.input_channels(), 4);
    assert_eq!(facade.output_channels(), 2);

    // Bit-identical short renders across all three routes.
    let frames = 64;
    let mut input = vec![0.0; frames * 4];
    for frame in 0..frames {
        input[frame * 4] = 0.5;
    }
    let ffi_out = ffi.process(&input);
    assert_eq!(ffi_out.len(), frames * 2);
    assert!(peak(&ffi_out) > 0.01, "FFI custom route must render");

    let context = sotf_host::plugin::ProcessContext::new(48_000, frames);
    let mut bridge_out = vec![f32::NAN; frames * 2];
    assert_eq!(
        bridge.process(&input, &mut bridge_out, &context).unwrap(),
        frames
    );
    let mut facade_out = vec![f32::NAN; frames * 2];
    assert_eq!(
        facade.process(&input, &mut facade_out, &context).unwrap(),
        frames
    );
    assert_eq!(bridge_out, ffi_out, "bridge must match FFI bit-exactly");
    assert_eq!(facade_out, ffi_out, "facade must match FFI bit-exactly");
}
