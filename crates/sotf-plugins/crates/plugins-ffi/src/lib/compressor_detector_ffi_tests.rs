//! Compressor detector C ABI consumer proofs.
//!
//! Drives the public C API for the detector controls shared by the
//! broadband `Compressor` kind (PARAMS indices 9/10/11/18) and the
//! `MultibandCompressor` kind (GLOBAL_PARAMS indices 19/20/21/22):
//! descriptor stability, normalized setter mapping, default-audio
//! preservation, an independent gain-reduction separation oracle,
//! state/preset round-trips, and transactional refusal with twin
//! continuation. Detector fields are live-applied in place by the DSP
//! (coefficient refresh plus filter-state reset), so the C live setter
//! is the correct route; no FFI structural guard covers them.

// Rust guideline compliant 2026-02-21

use crate::*;
use std::ffi::{CStr, CString};

/// Mono sample rate used by every handle in this module.
const SAMPLE_RATE_HZ: f32 = 48_000.0;
/// Low probe tone: dominant driver when the detector HPF is off.
const LF_HZ: f32 = 50.0;
/// Low probe peak level in dBFS.
const LF_PEAK_DB: f32 = -3.0;
/// High probe tone: surviving driver when the detector HPF is on.
const HF_HZ: f32 = 1000.0;
/// High probe peak level in dBFS.
const HF_PEAK_DB: f32 = -24.0;
/// Oracle render length: 1 s settle plus 1 s measurement at 48 kHz.
const ORACLE_FRAMES: usize = 96_000;
/// Frames skipped before measurement (20 release constants at 50 ms).
const ORACLE_SETTLE_FRAMES: usize = 48_000;
/// Minimum output separation between HPF-off and HPF-on configs.
const MIN_GR_SEPARATION_DB: f32 = 6.0;

struct DetectorHandle {
    pointer: *mut PluginHandle,
}

impl DetectorHandle {
    fn create(kind: &str, config: &str) -> Self {
        let kind = CString::new(kind).unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(kind.as_ptr(), config.as_ptr(), 48_000, 2, 2);
        assert!(!handle.is_null(), "compressor construction failed: {}", detector_last_error());
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
        assert_eq!(plugin_reset(self.pointer), 0, "{}", detector_last_error());
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
                detector_last_error()
            );
        }
        assert!(output.iter().all(|value| value.is_finite()));
        output
    }
}

impl Drop for DetectorHandle {
    fn drop(&mut self) {
        plugin_destroy(self.pointer);
    }
}

fn detector_last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned()
}

/// Dual-mono probe: 50 Hz at -3 dB peak plus 1 kHz at -24 dB peak.
fn probe_signal(frames: usize) -> Vec<f32> {
    let lf = 10.0f32.powf(LF_PEAK_DB / 20.0);
    let hf = 10.0f32.powf(HF_PEAK_DB / 20.0);
    let mut buffer = vec![0.0; frames * 2];
    for frame in 0..frames {
        let sample = lf * (2.0 * std::f32::consts::PI * LF_HZ * frame as f32 / SAMPLE_RATE_HZ).sin()
            + hf * (2.0 * std::f32::consts::PI * HF_HZ * frame as f32 / SAMPLE_RATE_HZ).sin();
        buffer[frame * 2] = sample;
        buffer[frame * 2 + 1] = sample;
    }
    buffer
}

/// Settled output level in dBFS (channel 0 RMS past the settle window).
fn settled_output_db(output: &[f32]) -> f32 {
    let frames = output.len() / 2;
    assert!(frames > ORACLE_SETTLE_FRAMES);
    let mut sum = 0.0f64;
    // u32 counter: measured frames never exceed ORACLE_FRAMES (96_000).
    let mut count = 0u32;
    for frame in ORACLE_SETTLE_FRAMES..frames {
        let sample = f64::from(output[frame * 2]);
        sum += sample * sample;
        count += 1;
    }
    let rms = (sum / f64::from(count)).sqrt();
    (20.0 * rms.log10()) as f32
}

/// Configure the static-law oracle: threshold -20, ratio 4, hard knee.
///
/// Normalized values invert the linear bridge mapping exactly:
/// threshold 40/60, ratio 3/19, knee 0.
fn configure_static_oracle(handle: &mut DetectorHandle, kind: &str) {
    assert_eq!(handle.set_normalized("threshold", 40.0 / 60.0), 0, "{kind}: {}", detector_last_error());
    assert_eq!(handle.set_normalized("ratio", 3.0 / 19.0), 0, "{kind}: {}", detector_last_error());
    assert_eq!(handle.set_normalized("knee", 0.0), 0, "{kind}: {}", detector_last_error());
}

/// Detector descriptor and defaults match on both compressor kinds.
///
/// Broadband exposes 19 PARAMS entries (detector at 9/10/11/18);
/// multiband exposes 23 globals plus 5 x 12 band entries (detector at
/// 19/20/21/22). Legacy defaults hold: 80 Hz, inactive HPF, 2nd order,
/// Peak detection.
#[test]
fn compressor_detector_metadata_and_defaults_both_kinds() {
    let broadband_ids = [
        "threshold",
        "ratio",
        "attack",
        "release",
        "knee",
        "makeup_gain",
        "mix",
        "auto_makeup",
        "link_channels",
        "sidechain_hpf_hz",
        "sidechain_hpf_order",
        "detection_mode",
        "lookahead_ms",
        "program_dependent_release",
        "measured_auto_makeup",
        "sidechain_external",
        "range_db",
        "hold_ms",
        "sidechain_hpf_enabled",
    ];
    let multiband_global_ids = [
        "num_bands",
        "crossover_preset",
        "crossover_freq_1",
        "crossover_freq_2",
        "crossover_freq_3",
        "crossover_freq_4",
        "threshold",
        "ratio",
        "attack",
        "release",
        "knee",
        "mix",
        "link_channels",
        "per_band_lookahead_ms",
        "ms_mode",
        "sidechain_tilt_db",
        "link_amount",
        "range_db",
        "hold_ms",
        "sidechain_hpf_hz",
        "sidechain_hpf_order",
        "detection_mode",
        "sidechain_hpf_enabled",
    ];
    // Detector addresses are non-contiguous on broadband (9/10/11/18)
    // and contiguous on multiband (19/20/21/22); index each explicitly.
    for (kind, count, globals, hz_idx, ord_idx, det_idx, en_idx) in [
        ("Compressor", 19usize, broadband_ids.as_slice(), 9usize, 10usize, 11usize, 18usize),
        (
            "MultibandCompressor",
            83usize,
            multiband_global_ids.as_slice(),
            19usize,
            20usize,
            21usize,
            22usize,
        ),
    ] {
        let handle = DetectorHandle::create(kind, "{}");
        assert_eq!(handle.param_count(), count, "{kind} count moved");
        for (index, id) in globals.iter().enumerate() {
            assert_eq!(handle.param_id(index), *id, "{kind} address {index} moved");
        }
        // HPF frequency: linear 0..=200 (min 0 selects linear scaling even
        // though the Hz unit sets the logarithmic metadata flag), 5 Hz steps.
        assert_eq!(
            handle.param_info(hz_idx),
            (
                "sidechain_hpf_hz".to_string(),
                "Sidechain HPF".to_string(),
                "Hz".to_string(),
                0.0,
                200.0,
                80.0,
                40,
                true
            ),
            "{kind} hpf hz metadata moved"
        );
        // Order and detection are 2-label choices reading as indices 0..=1.
        assert_eq!(
            handle.param_info(ord_idx),
            ("sidechain_hpf_order".to_string(), "Sidechain HPF Order".to_string(), String::new(), 0.0, 1.0, 0.0, 2, false),
            "{kind} hpf order metadata moved"
        );
        assert_eq!(
            handle.param_info(det_idx),
            ("detection_mode".to_string(), "Detection Mode".to_string(), String::new(), 0.0, 1.0, 0.0, 2, false),
            "{kind} detection metadata moved"
        );
        assert_eq!(
            handle.param_info(en_idx),
            ("sidechain_hpf_enabled".to_string(), "Sidechain HPF Enabled".to_string(), String::new(), 0.0, 1.0, 0.0, 1, false),
            "{kind} hpf enable metadata moved"
        );
        // Legacy defaults read back: 80 Hz, 2nd order, Peak, disabled.
        assert!((handle.get_normalized("sidechain_hpf_hz") - 0.4).abs() < 1e-12, "{kind} hpf default");
        assert!((handle.get_normalized("sidechain_hpf_order") - 0.0).abs() < 1e-12, "{kind} order default");
        assert!((handle.get_normalized("detection_mode") - 0.0).abs() < 1e-12, "{kind} detection default");
        assert!((handle.get_normalized("sidechain_hpf_enabled") - 0.0).abs() < 1e-12, "{kind} enable default");
    }
    // Multiband band block follows the 23 globals: 5 bands x 12 fields.
    let multiband = DetectorHandle::create("MultibandCompressor", "{}");
    assert_eq!(multiband.param_id(23), "band_0_solo");
    assert_eq!(multiband.param_id(82), "band_4_hold_ms");
}

/// Detector live setters round-trip with correct C setter types.
///
/// HPF frequency 0.6 maps to exactly 120 Hz (linear, 5 Hz step);
/// order/detection 1.0 select 4th/RMS as integer indices; enable 1.0
/// latches true. The DSP applies these live (coefficient refresh plus
/// filter-state reset), so the normalized C setter is the right route.
#[test]
fn compressor_detector_live_setters_roundtrip_both_kinds() {
    for kind in ["Compressor", "MultibandCompressor"] {
        let mut handle = DetectorHandle::create(kind, "{}");
        assert_eq!(handle.set_normalized("sidechain_hpf_hz", 0.6), 0, "{kind}: {}", detector_last_error());
        assert!((handle.get_normalized("sidechain_hpf_hz") - 0.6).abs() < 1e-12, "{kind} hz must read 120 Hz");
        assert_eq!(handle.set_normalized("sidechain_hpf_order", 1.0), 0, "{kind}: {}", detector_last_error());
        assert!((handle.get_normalized("sidechain_hpf_order") - 1.0).abs() < 1e-12, "{kind} order must read 4th");
        assert_eq!(handle.set_normalized("detection_mode", 1.0), 0, "{kind}: {}", detector_last_error());
        assert!((handle.get_normalized("detection_mode") - 1.0).abs() < 1e-12, "{kind} detection must read RMS");
        assert_eq!(handle.set_normalized("sidechain_hpf_enabled", 1.0), 0, "{kind}: {}", detector_last_error());
        assert!((handle.get_normalized("sidechain_hpf_enabled") - 1.0).abs() < 1e-12, "{kind} enable must latch");
        assert_eq!(handle.set_normalized("sidechain_hpf_enabled", 0.0), 0, "{kind}: {}", detector_last_error());
        assert!((handle.get_normalized("sidechain_hpf_enabled") - 0.0).abs() < 1e-12, "{kind} enable must clear");
    }
}

/// Default audio is preserved and the HPF is inert while disabled.
///
/// Fresh default handles render identical nonzero audio; changing only
/// the HPF frequency while disabled (80 -> 0 Hz), or enabling with a
/// 0 Hz cutoff, renders bit-identically (active <=> enabled AND hz > 0).
#[test]
fn compressor_default_audio_preserved_and_hpf_inert_when_disabled() {
    for kind in ["Compressor", "MultibandCompressor"] {
        let input = probe_signal(4800);
        let mut default = DetectorHandle::create(kind, "{}");
        let default_out = default.process(&input);
        assert!(default_out.iter().any(|sample| sample.abs() > 0.01), "{kind} default must render nonzero audio");
        // Retuning the cutoff while disabled cannot change audio.
        let mut retuned = DetectorHandle::create(kind, "{}");
        assert_eq!(retuned.set_normalized("sidechain_hpf_hz", 0.0), 0, "{kind}: {}", detector_last_error());
        assert_eq!(retuned.process(&input), default_out, "{kind} hz must be inert while disabled");
        // Enabled with a 0 Hz cutoff stays inactive by the AND contract.
        let mut zero_cutoff = DetectorHandle::create(kind, "{}");
        assert_eq!(zero_cutoff.set_normalized("sidechain_hpf_hz", 0.0), 0, "{kind}: {}", detector_last_error());
        assert_eq!(zero_cutoff.set_normalized("sidechain_hpf_enabled", 1.0), 0, "{kind}: {}", detector_last_error());
        assert_eq!(zero_cutoff.process(&input), default_out, "{kind} 0 Hz cutoff must stay inactive");
    }
}

/// Engaged 120 Hz 4th-order HPF separates LF gain reduction on both kinds.
///
/// Static law (hard knee, threshold -20, ratio 4): GR = (level + 20) * 0.75
/// for levels above threshold, else exactly 0. A 4th-order Butterworth at
/// 120 Hz passes 1 kHz and cuts 50 Hz by (50/120)^4 = 0.0301 (-30.4 dB).
/// Predictions for the settled output: Peak/No-HPF ~-18.6 dB (17 dB
/// over); Peak/HPF ~-6.0 dB on BOTH kinds (detector -24 dB sits 4 dB
/// BELOW threshold, so GR vanishes entirely); RMS/No-HPF ~-16.4 dB (sine
/// RMS sits 3 dB under peak); RMS/HPF ~-6.0 dB (detector -26.9 dB, below
/// threshold). Windows below carry >=2 dB of margin for envelope ripple
/// and ballistics; separation needs >=6 dB against ~10-12.75 dB
/// predicted. RMS-vs-Peak with the HPF off proves RMS is an independent
/// working path, measured with release at 1000 ms to isolate the static
/// law from ballistics: inter-peak droop over 480 samples is
/// exp(-480/(r*48000)), i.e. -1.74 dB at the default 50 ms but -0.087 dB
/// at 1000 ms, so the gap reads the static peak/RMS sine difference
/// (~2.25 broadband, ~2.03 multiband after exact band-power dilution)
/// within [1.5,2.5]. The 10 ms RMS window spans exactly one 50 Hz
/// sine-square period, so the RMS side is ripple-free in both fixtures.
#[test]
fn compressor_hpf_engaged_produces_lf_gr_separation_both_kinds() {
    for kind in ["Compressor", "MultibandCompressor"] {
        let input = probe_signal(ORACLE_FRAMES);
        let render = |hpf_enabled: f64, detection: f64| -> f32 {
            let mut handle = DetectorHandle::create(kind, "{}");
            configure_static_oracle(&mut handle, kind);
            assert_eq!(handle.set_normalized("sidechain_hpf_hz", 0.6), 0, "{kind}: {}", detector_last_error());
            assert_eq!(handle.set_normalized("sidechain_hpf_order", 1.0), 0, "{kind}: {}", detector_last_error());
            assert_eq!(handle.set_normalized("detection_mode", detection), 0, "{kind}: {}", detector_last_error());
            assert_eq!(handle.set_normalized("sidechain_hpf_enabled", hpf_enabled), 0, "{kind}: {}", detector_last_error());
            settled_output_db(&handle.process(&input))
        };
        let peak_off = render(0.0, 0.0);
        let peak_on = render(1.0, 0.0);
        let rms_off = render(0.0, 1.0);
        let rms_on = render(1.0, 1.0);
        // Ripple-free gap pair: release at maximum (normalized 1.0 = 1000 ms
        // exactly) kills inter-peak droop, so the gap reads the static law.
        // Stationary tone settles via the fast attack path; the 1 s measure
        // window starts after 1 s of settle.
        let render_gap = |detection: f64| -> f32 {
            let mut handle = DetectorHandle::create(kind, "{}");
            configure_static_oracle(&mut handle, kind);
            assert_eq!(handle.set_normalized("release", 1.0), 0, "{kind}: {}", detector_last_error());
            assert_eq!(handle.set_normalized("detection_mode", detection), 0, "{kind}: {}", detector_last_error());
            settled_output_db(&handle.process(&input))
        };
        let peak_gap = render_gap(0.0);
        let rms_gap = render_gap(1.0);
        println!(
            "[detector-gap] {kind} default-ballistics gap {:.2} dB, ripple-free gap {:.2} dB",
            rms_off - peak_off,
            rms_gap - peak_gap
        );
        assert!((-22.0..=-15.0).contains(&peak_off), "{kind} Peak/HPF-off {peak_off:.2} dB outside [-22,-15]");
        assert!((-20.0..=-13.0).contains(&rms_off), "{kind} RMS/HPF-off {rms_off:.2} dB outside [-20,-13]");
        assert!((-8.0..=-4.0).contains(&rms_on), "{kind} RMS/HPF-on {rms_on:.2} dB outside [-8,-4]");
        // Both kinds: the -24 dB detector sits below the -20 threshold, so
        // gain reduction vanishes and output equals the -6 dB input.
        assert!((-8.0..=-4.0).contains(&peak_on), "{kind} Peak/HPF-on {peak_on:.2} dB outside [-8,-4]");
        assert!(
            peak_on - peak_off >= MIN_GR_SEPARATION_DB,
            "{kind} Peak separation below 6 dB (on={peak_on:.2}, off={peak_off:.2})"
        );
        assert!(
            rms_on - rms_off >= MIN_GR_SEPARATION_DB,
            "{kind} RMS separation below 6 dB (on={rms_on:.2}, off={rms_off:.2})"
        );
        let rms_peak_gap = rms_gap - peak_gap;
        assert!(
            (1.5..=2.5).contains(&rms_peak_gap),
            "{kind} RMS-vs-Peak gap {rms_peak_gap:.2} dB outside [1.5,2.5]"
        );
    }
}

/// Detector configuration survives state and preset-JSON reload.
///
/// Saved JSON carries typed detector values (float Hz, integer order /
/// detection indices, boolean enable); both restore entry points adopt
/// them byte-identically and render identically from reset with nonzero
/// measurable audio.
#[test]
fn compressor_detector_state_and_preset_json_roundtrip_both_kinds() {
    for kind in ["Compressor", "MultibandCompressor"] {
        for document in [false, true] {
            let mut source = DetectorHandle::create(kind, "{}");
            assert_eq!(source.set_normalized("sidechain_hpf_hz", 0.6), 0, "{kind}: {}", detector_last_error());
            assert_eq!(source.set_normalized("sidechain_hpf_order", 1.0), 0, "{kind}: {}", detector_last_error());
            assert_eq!(source.set_normalized("detection_mode", 1.0), 0, "{kind}: {}", detector_last_error());
            assert_eq!(source.set_normalized("sidechain_hpf_enabled", 1.0), 0, "{kind}: {}", detector_last_error());
            assert_eq!(source.set_normalized("threshold", 0.5), 0, "{kind}: {}", detector_last_error());
            let saved = source.save();
            let json: serde_json::Value = serde_json::from_slice(&saved).expect("state must be JSON");
            assert_eq!(json["sidechain_hpf_hz"], serde_json::json!(120.0), "{kind} hz must persist typed");
            assert_eq!(json["sidechain_hpf_order"], serde_json::json!(1), "{kind} order must persist typed");
            assert_eq!(json["detection_mode"], serde_json::json!(1), "{kind} detection must persist typed");
            assert_eq!(json["sidechain_hpf_enabled"], serde_json::json!(true), "{kind} enable must persist typed");
            let mut target = DetectorHandle::create(kind, "{}");
            assert_eq!(target.load(&saved, document), 0, "{kind} document={document}: {}", detector_last_error());
            assert_eq!(target.save(), saved, "{kind} document={document} state must round-trip");
            assert!((target.get_normalized("sidechain_hpf_hz") - 0.6).abs() < 1e-12);
            assert!((target.get_normalized("sidechain_hpf_order") - 1.0).abs() < 1e-12);
            assert!((target.get_normalized("detection_mode") - 1.0).abs() < 1e-12);
            assert!((target.get_normalized("sidechain_hpf_enabled") - 1.0).abs() < 1e-12);
            source.reset();
            target.reset();
            let input = probe_signal(8192);
            let output = target.process(&input);
            assert_eq!(output, source.process(&input), "{kind} document={document} audio must match");
            assert!(output.iter().any(|sample| sample.abs() > 0.01), "{kind} restored audio must be nonzero");
        }
    }
}

/// Bad detector configs refuse with an engaged twin continuing identically.
///
/// State-load refusals: mistyped cutoff (null), mistyped order (string),
/// non-integer order (float for Int), malformed JSON, and a preset
/// document naming another plugin family. Finite out-of-range cutoff
/// refuses too (the validating trait route range-checks before the
/// inherent clamping setter). Broadband additionally refuses live writes
/// to the unimplemented legacy controls (`sidechain_external`,
/// `program_dependent_release`), naming the key. Each refusal returns
/// nonzero, leaves saved bytes untouched, and leaves DSP history
/// bit-identical to a twin that never saw the refusal. A subsequent
/// valid restore adopts and renders nonzero.
#[test]
fn compressor_bad_detector_config_refused_with_twin_continuation() {
    for kind in ["Compressor", "MultibandCompressor"] {
        for document in [false, true] {
            let mut handle = DetectorHandle::create(kind, "{}");
            let mut twin = DetectorHandle::create(kind, "{}");
            for twin_handle in [&mut handle, &mut twin] {
                configure_static_oracle(twin_handle, kind);
                assert_eq!(twin_handle.set_normalized("sidechain_hpf_hz", 0.6), 0, "{kind}: {}", detector_last_error());
                assert_eq!(twin_handle.set_normalized("sidechain_hpf_enabled", 1.0), 0, "{kind}: {}", detector_last_error());
            }
            // Engage detector/envelope history identically on both handles.
            let engage = probe_signal(16384);
            assert_eq!(twin.process(&engage), handle.process(&engage), "{kind} twins must start identical");
            let saved = handle.save();
            assert_eq!(twin.save(), saved);

            // State-load refusals: mistyped cutoff (null), mistyped order
            // (string), non-integer order (float for an Int field), and
            // malformed JSON. Unknown keys are skipped by the generic bridge
            // loader, so unsupported legacy controls refuse on the live
            // setter path below instead.
            let bad_cases: Vec<&[u8]> = vec![
                br#"{"sidechain_hpf_hz":null}"#,
                br#"{"sidechain_hpf_order":"4th"}"#,
                br#"{"sidechain_hpf_order":1.5}"#,
                br#"{"sidechain_hpf_hz":120.0"#, // malformed
            ];
            for bad in bad_cases {
                assert_ne!(handle.load(bad, document), 0, "{kind} document={document} must refuse {bad:?}");
                assert_eq!(handle.save(), saved, "{kind} document={document} refused state must roll back");
            }
            if kind == "Compressor" {
                // Broadband advertises the legacy sidechain controls but the
                // FFI route validates against the cached schema first, so the
                // refusal names the key ("Unknown parameter: {key}") rather
                // than the inherent DSP path's "unsupported legacy" wording
                // (which the DSP-crate tests pin). Either way the write is
                // refused before any mutation; read diagnostics immediately.
                for legacy in ["sidechain_external", "program_dependent_release"] {
                    assert_ne!(
                        handle.set_normalized(legacy, 1.0),
                        0,
                        "{kind} live set of {legacy} must refuse"
                    );
                    let error = detector_last_error();
                    assert!(error.contains(legacy), "{kind} {legacy} refusal must name the refused key: {error}");
                    assert_eq!(handle.save(), saved, "{kind} {legacy} refusal must roll back");
                }
            }
            // A preset document for another family refuses at the envelope.
            if document {
                let foreign = serde_json::to_vec(&serde_json::json!({
                    "schema_version": 1,
                    "ut_type": "org.spinorama.sotf.plugin-preset",
                    "plugin_type": "Gain",
                    "state": saved.as_slice(),
                }))
                .unwrap();
                assert_ne!(
                    plugin_import_preset_json(handle.pointer, foreign.as_ptr(), foreign.len()),
                    0,
                    "{kind} foreign preset family must refuse"
                );
                assert_eq!(handle.save(), saved, "{kind} foreign preset must roll back");
            }
            // Continued renders stay bit-identical to the untouched twin.
            assert_eq!(twin.process(&engage), handle.process(&engage), "{kind} document={document} history must survive refusal");

            // Finite out-of-range cutoff refuses on the state path: the
            // validating trait route range-checks before the inherent
            // clamping setter is reachable (COMMON rejected-change contract).
            assert_ne!(
                handle.load(br#"{"sidechain_hpf_hz":999.0}"#, document),
                0,
                "{kind} document={document} must refuse out-of-range cutoff"
            );
            assert_eq!(handle.save(), saved, "{kind} document={document} out-of-range refusal must roll back");
            assert_eq!(twin.process(&engage), handle.process(&engage), "{kind} document={document} history must survive range refusal");

            // Valid restore after refusal adopts and renders nonzero audio.
            assert_eq!(handle.load(&saved, document), 0, "{kind}: {}", detector_last_error());
            assert_eq!(handle.save(), saved);
            assert!((handle.get_normalized("sidechain_hpf_hz") - 0.6).abs() < 1e-12);
            let output = handle.process(&engage);
            assert!(output.iter().any(|sample| sample.abs() > 0.01), "{kind} restored handle must render nonzero audio");
        }
    }
}
