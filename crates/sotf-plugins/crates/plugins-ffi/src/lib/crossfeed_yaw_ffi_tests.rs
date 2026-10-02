//! Crossfeed yaw C ABI consumer proofs.
//!
//! Drives the public C API (`plugin_create`, parameter enumeration,
//! `plugin_set_parameter`, `plugin_save_state`, `plugin_load_state`,
//! `plugin_import_preset_json`, `plugin_process`) for `head_yaw_deg`
//! (index 17). Covers descriptor stability, normalized setter mapping,
//! differential-ITD audio direction, state/preset round-trips, preset
//! action semantics, and transactional refusal with twin continuation.

// Rust guideline compliant 2026-02-21

use crate::*;
use std::ffi::{CStr, CString};

/// Mono sample rate used by every handle in this module.
const SAMPLE_RATE_HZ: f32 = 48_000.0;
/// Frames of leading silence so the 10 ms yaw smoother fully settles.
const SMOOTHER_PREROLL_FRAMES: usize = 48_000;
/// First-sample threshold for cross-channel onset detection.
const ONSET_THRESHOLD: f32 = 1e-4;
/// Minimum R-onset shift between yaw +45 and -45, in samples.
const MIN_ONSET_SHIFT_SAMPLES: usize = 6;
/// Minimum cross-channel peak proving a nonzero yaw effect.
const MIN_CROSS_PEAK: f32 = 0.05;

struct YawHandle {
    pointer: *mut PluginHandle,
}

impl YawHandle {
    fn create(config: &str) -> Self {
        let kind = CString::new("Crossfeed").unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(kind.as_ptr(), config.as_ptr(), 48_000, 2, 2);
        assert!(!handle.is_null(), "Crossfeed construction failed: {}", yaw_last_error());
        Self { pointer: handle }
    }

    fn inner(&self) -> &PluginHandle {
        // SAFETY: This guard owns a live handle, accessed only on this thread.
        unsafe { &*self.pointer }
    }

    fn param_count(&self) -> usize {
        let count = plugin_get_parameter_count(self.pointer);
        assert!(count >= 0);
        count as usize
    }

    fn param_id(&self, index: usize) -> String {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: The info pointer borrows from this live handle.
        let id = unsafe { (*info).id };
        assert!(!id.is_null());
        // SAFETY: IDs are valid NUL-terminated strings while live.
        unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned()
    }

    /// Full `ParameterInfo` tuple: (id, name, unit, min, max, default, steps, logarithmic).
    fn param_info(&self, index: usize) -> (String, String, String, f64, f64, f64, u32, bool) {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: Info and its strings borrow from this live handle.
        unsafe {
            let info = &*info;
            let id = CStr::from_ptr(info.id).to_string_lossy().into_owned();
            let name = CStr::from_ptr(info.name).to_string_lossy().into_owned();
            let unit = CStr::from_ptr(info.unit).to_string_lossy().into_owned();
            (id, name, unit, info.min_value, info.max_value, info.default_value, info.steps, info.logarithmic)
        }
    }

    fn set_normalized(&mut self, id: &str, value: f64) -> i32 {
        let id = CString::new(id).unwrap();
        plugin_set_parameter(self.pointer, id.as_ptr(), value)
    }

    fn get_normalized(&self, id: &str) -> f64 {
        let id = CString::new(id).unwrap();
        plugin_get_parameter(self.pointer, id.as_ptr())
    }

    fn save(&self) -> Vec<u8> {
        let mut len = 0;
        let state = plugin_save_state(self.pointer, &mut len);
        assert!(!state.is_null());
        // SAFETY: The FFI owns exactly len initialized bytes until freed below.
        let saved = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        plugin_free_state(state, len);
        saved
    }

    fn load(&mut self, state: &[u8], document: bool) -> i32 {
        if document {
            let plugin_type = self.inner().plugin_type.clone();
            let bytes = serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "ut_type": "org.spinorama.sotf.plugin-preset",
                "plugin_type": plugin_type,
                "state": state,
            }))
            .unwrap();
            plugin_import_preset_json(self.pointer, bytes.as_ptr(), bytes.len())
        } else {
            plugin_load_state(self.pointer, state.as_ptr(), state.len())
        }
    }

    fn reset(&mut self) {
        assert_eq!(plugin_reset(self.pointer), 0, "{}", yaw_last_error());
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let inputs = self.inner().input_channels;
        let outputs = self.inner().output_channels;
        let frames = input.len() / inputs;
        assert_eq!(input.len() % inputs, 0, "complete interleaved frames required");
        let callback_frames = self.inner().max_callback_frames;
        let mut output = vec![f32::NAN; frames * outputs];
        for start in (0..frames).step_by(callback_frames) {
            let count = callback_frames.min(frames - start);
            assert_eq!(
                plugin_process(
                    self.pointer,
                    input[start * inputs..].as_ptr(),
                    output[start * outputs..].as_mut_ptr(),
                    count
                ),
                0,
                "{}",
                yaw_last_error()
            );
        }
        assert!(output.iter().all(|value| value.is_finite()));
        output
    }
}

impl Drop for YawHandle {
    fn drop(&mut self) {
        plugin_destroy(self.pointer);
    }
}

fn yaw_last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned()
}

/// First frame where `channel` exceeds `ONSET_THRESHOLD`, if any.
fn onset_frame(output: &[f32], channels: usize, channel: usize) -> Option<usize> {
    let frames = output.len() / channels;
    (0..frames).find(|frame| output[frame * channels + channel].abs() > ONSET_THRESHOLD)
}

fn channel_peak(output: &[f32], channels: usize, channel: usize) -> f32 {
    let frames = output.len() / channels;
    (0..frames)
        .map(|frame| output[frame * channels + channel].abs())
        .fold(0.0, f32::max)
}

/// Render an L-only (or R-only) impulse after smoother preroll.
///
/// Sets yaw on a fresh handle, renders silence so the 10 ms yaw smoother
/// settles from its initial value to the target, then renders one impulse
/// followed by zeros. Returns the impulse-segment output only.
fn render_single_sided_impulse(yaw_deg: f32, left_side: bool) -> Vec<f32> {
    let mut handle = YawHandle::create("{}");
    // FFI creation deserializes `{}` with the derived mode default (Off),
    // not the descriptor default (Multiband), so select the mode explicitly.
    assert_eq!(handle.set_normalized("mode", 3.0 / 4.0), 0, "{}", yaw_last_error());
    assert!((handle.get_normalized("mode") - 0.75).abs() < 1e-12, "mode must read Multiband");
    // Normalized yaw: (45 + 90) / 180 = 0.75, (-45 + 90) / 180 = 0.25.
    let normalized = (f64::from(yaw_deg) + 90.0) / 180.0;
    assert_eq!(handle.set_normalized("head_yaw_deg", normalized), 0, "{}", yaw_last_error());
    let silence = vec![0.0; SMOOTHER_PREROLL_FRAMES * 2];
    handle.process(&silence);
    let mut impulse = vec![0.0; 4096 * 2];
    impulse[usize::from(!left_side)] = 0.5;
    handle.process(&impulse)
}

/// Yaw descriptor is stable at index 17 with float metadata.
///
/// Contract: 18 addresses, frozen order 0..=16, appended `head_yaw_deg`
/// float in [-90, 90] defaulting to 0 with 1-degree steps.
#[test]
fn crossfeed_yaw_count_order_and_metadata_stable() {
    let handle = YawHandle::create("{}");
    assert_eq!(handle.param_count(), 18);
    let expected = [
        "mode",
        "preset",
        "enabled",
        "mix",
        "bauer_fcut_hz",
        "bauer_feed_db",
        "meier_level",
        "mb_low_freq_hz",
        "mb_mid_high_freq_hz",
        "mb_low_feed_db",
        "mb_mid_feed_db",
        "mb_high_feed_db",
        "itd_delay_ms",
        "autogain_enabled",
        "autogain_target_lufs",
        "autogain_max_gain_db",
        "autogain_smoothing_ms",
        "head_yaw_deg",
    ];
    for (index, id) in expected.iter().enumerate() {
        assert_eq!(handle.param_id(index), *id, "address {index} moved");
    }
    assert_eq!(
        handle.param_info(17),
        (
            "head_yaw_deg".to_string(),
            "Head Yaw".to_string(),
            "deg".to_string(),
            -90.0,
            90.0,
            0.0,
            180,
            false
        )
    );
}

/// Yaw normalized setter round-trips and clamps out-of-range input.
///
/// Mapping is linear over [-90, 90]: 0.75 reads back exactly 45 degrees.
/// Out-of-range normalized values clamp (accepted bridge contract).
#[test]
fn crossfeed_yaw_normalized_setter_roundtrip_and_clamp() {
    let mut handle = YawHandle::create("{}");
    assert!((handle.get_normalized("head_yaw_deg") - 0.5).abs() < 1e-12, "default yaw must read 0.5");
    assert_eq!(handle.set_normalized("head_yaw_deg", 0.75), 0, "{}", yaw_last_error());
    assert!((handle.get_normalized("head_yaw_deg") - 0.75).abs() < 1e-12, "yaw must read back 0.75");
    assert_eq!(handle.set_normalized("head_yaw_deg", 1.5), 0, "{}", yaw_last_error());
    assert!((handle.get_normalized("head_yaw_deg") - 1.0).abs() < 1e-12, "yaw must clamp to +90");
    assert_eq!(handle.set_normalized("head_yaw_deg", -2.0), 0, "{}", yaw_last_error());
    assert!((handle.get_normalized("head_yaw_deg") - 0.0).abs() < 1e-12, "yaw must clamp to -90");
}

/// Yaw steers differential ITD direction on single-sided impulses.
///
/// Independent law: dynamic = 0.0875 * sin(yaw) / 343 * 1000 ms, L-to-R
/// path delay = clamp(base + dynamic) with base = static / 2. At static
/// ITD 0 and yaw +/-45, the L-to-R path carries 0.18038 ms (8.658
/// samples at 48 kHz) versus 0 ms, so the R-channel onset of an L-only
/// impulse shifts by 8.658 samples; the delay applies after the cross
/// sum, so both renders share one waveshape and only the shift varies.
/// Bound: onset(+45) - onset(-45) >= 6 samples (2.6 samples of margin
/// for fractional-interpolation threshold effects). R-only input at -45
/// must mirror L-only input at +45 through the symmetric path.
#[test]
fn crossfeed_yaw_steers_itd_direction_on_single_sided_impulse() {
    let plus = render_single_sided_impulse(45.0, true);
    let minus = render_single_sided_impulse(-45.0, true);
    assert!(channel_peak(&plus, 2, 1) > MIN_CROSS_PEAK, "yaw +45 needs nonzero R bleed");
    assert!(channel_peak(&minus, 2, 1) > MIN_CROSS_PEAK, "yaw -45 needs nonzero R bleed");
    let onset_plus = onset_frame(&plus, 2, 1).expect("R onset must exist at +45");
    let onset_minus = onset_frame(&minus, 2, 1).expect("R onset must exist at -45");
    println!(
        "[yaw-itd] plus45 R peak {:.4} onset {onset_plus}, minus45 R peak {:.4} onset {onset_minus}",
        channel_peak(&plus, 2, 1),
        channel_peak(&minus, 2, 1)
    );
    assert!(
        onset_plus >= onset_minus + MIN_ONSET_SHIFT_SAMPLES,
        "R onset must shift later at +45 (plus={onset_plus}, minus={onset_minus})"
    );
    // Mirror symmetry: R-only at -45 drives L through the same 0.18038 ms path.
    let mirror = render_single_sided_impulse(-45.0, false);
    let worst = plus
        .as_chunks::<2>()
        .0
        .iter()
        .zip(mirror.as_chunks::<2>().0.iter())
        .map(|(plus_frame, mirror_frame)| (plus_frame[1] - mirror_frame[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-6, "mirrored cross paths must match, worst {worst:.3e}");
}

/// Saved yaw, mix, and ITD survive state and preset-JSON reload.
///
/// Uses live C setters for every value, then checks both restore entry
/// points adopt them byte-identically and render identically from reset.
#[test]
fn crossfeed_yaw_mix_itd_survive_state_and_preset_json_reload() {
    for document in [false, true] {
        let mut source = YawHandle::create("{}");
        // Preset first: a preset selection is a full DSP reset, so custom
        // values must follow it (mirrors the preset-first replay order).
        assert_eq!(source.set_normalized("preset", 3.0 / 5.0), 0, "{}", yaw_last_error());
        assert_eq!(source.set_normalized("head_yaw_deg", 0.75), 0, "{}", yaw_last_error());
        assert_eq!(source.set_normalized("mix", 0.5), 0, "{}", yaw_last_error());
        assert_eq!(source.set_normalized("itd_delay_ms", 0.3), 0, "{}", yaw_last_error());
        let saved = source.save();
        let mut target = YawHandle::create("{}");
        assert_eq!(target.load(&saved, document), 0, "{}", yaw_last_error());
        assert_eq!(target.save(), saved, "document={document} state must round-trip");
        assert!((target.get_normalized("head_yaw_deg") - 0.75).abs() < 1e-12);
        assert!((target.get_normalized("mix") - 0.5).abs() < 1e-12);
        // 0.3 is not dyadic: |0.3f32 - 0.3| = 1.19e-8, so the tolerance must
        // exceed the f32 quantum (1e-9 was provably unsatisfiable); 1e-7 keeps
        // 8x margin over representation error with a lossless JSON round-trip.
        assert!((target.get_normalized("itd_delay_ms") - 0.3).abs() < 1e-7);
        // Identical configuration renders identically from reset.
        source.reset();
        target.reset();
        let mut impulse = vec![0.0; 2048 * 2];
        impulse[0] = 0.5;
        assert_eq!(target.process(&impulse), source.process(&impulse), "document={document} audio must match");
    }
}

/// Preset action resets yaw; explicit saved yaw overrides the preset.
///
/// DSP-internal preset reset (yaw back to 0) is the accepted contract;
/// the FFI applies the preset action first so explicit saved values in
/// the same document are never masked by it.
#[test]
fn crossfeed_preset_action_resets_yaw_but_explicit_yaw_wins() {
    for document in [false, true] {
        // Preset-only selection resets yaw to the preset default (0).
        let mut handle = YawHandle::create("{}");
        assert_eq!(handle.set_normalized("head_yaw_deg", 0.75), 0, "{}", yaw_last_error());
        assert_eq!(handle.load(br#"{"preset":5}"#, document), 0, "{}", yaw_last_error());
        assert!(
            (handle.get_normalized("head_yaw_deg") - 0.5).abs() < 1e-12,
            "document={document} preset action must reset yaw to 0"
        );
        // Explicit saved yaw in the same document overrides the preset.
        let mut explicit = YawHandle::create("{}");
        assert_eq!(
            explicit.load(br#"{"preset":5,"head_yaw_deg":45.0,"mix":0.5}"#, document),
            0,
            "{}",
            yaw_last_error()
        );
        assert!(
            (explicit.get_normalized("head_yaw_deg") - 0.75).abs() < 1e-12,
            "document={document} explicit yaw must survive the preset action"
        );
        assert!((explicit.get_normalized("mix") - 0.5).abs() < 1e-12);
    }
}

/// Invalid crossfeed state refuses with an engaged twin continuing identically.
///
/// Refusal cases: mistyped yaw (null), mistyped mix (string), invalid
/// preset index, and malformed JSON. Each must return nonzero, leave the
/// saved bytes untouched, and leave DSP history bit-identical to a twin
/// that never saw the refusal. Out-of-range finite yaw clamps to +90 by
/// the accepted contract instead of refusing. A subsequent valid restore
/// adopts cleanly and renders nonzero audio.
#[test]
fn crossfeed_invalid_state_refused_with_engaged_twin_continuation() {
    for document in [false, true] {
        let mut handle = YawHandle::create("{}");
        let mut twin = YawHandle::create("{}");
        for twin_handle in [&mut handle, &mut twin] {
            // Multiband mode: FFI creation defaults to Off, which would pass
            // dry audio and leave crossfeed history unpopulated.
            assert_eq!(twin_handle.set_normalized("mode", 3.0 / 4.0), 0, "{}", yaw_last_error());
            assert_eq!(twin_handle.set_normalized("head_yaw_deg", 0.75), 0, "{}", yaw_last_error());
        }
        // Engage DSP history identically on both handles.
        let engage: Vec<f32> = (0..8192)
            .flat_map(|frame| {
                let sample = 0.4 * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / SAMPLE_RATE_HZ).sin();
                [sample, 0.25 * sample]
            })
            .collect();
        assert_eq!(twin.process(&engage), handle.process(&engage));
        let saved = handle.save();
        assert_eq!(twin.save(), saved);

        for bad in [
            br#"{"head_yaw_deg":null}"#.as_slice(),
            br#"{"mix":"loud"}"#.as_slice(),
            br#"{"preset":99}"#.as_slice(),
            br#"{"head_yaw_deg":45.0"#.as_slice(),
        ] {
            assert_ne!(handle.load(bad, document), 0, "document={document} must refuse {bad:?}");
            assert_eq!(handle.save(), saved, "document={document} refused state must roll back");
        }
        // Continued renders stay bit-identical to the untouched twin.
        assert_eq!(twin.process(&engage), handle.process(&engage), "document={document} history must survive refusal");

        // Finite out-of-range yaw clamps (accepted contract), not refusal.
        assert_eq!(handle.load(br#"{"head_yaw_deg":999.0}"#, document), 0, "{}", yaw_last_error());
        assert!((handle.get_normalized("head_yaw_deg") - 1.0).abs() < 1e-12);

        // Valid restore after refusal adopts and renders nonzero audio.
        assert_eq!(handle.load(&saved, document), 0, "{}", yaw_last_error());
        assert_eq!(handle.save(), saved);
        assert!((handle.get_normalized("head_yaw_deg") - 0.75).abs() < 1e-12);
        let output = handle.process(&engage);
        assert!(output.iter().any(|sample| sample.abs() > 0.01), "restored handle must render nonzero audio");
    }
}
