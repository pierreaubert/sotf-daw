//! Hiss bridge/FFI captured-profile persistence tests.
//!
//! Drives the public C ABI (`plugin_create`, `plugin_hiss_capture_control`,
//! `plugin_process`, `plugin_save_state`, `plugin_load_state`) for actual
//! capture-to-save-to-fresh-load chains with nonzero audio and complete EOF.
//! Seeded blobs appear only in matrix legs; the capture legs use the live
//! 1 s engine. Bounds reuse documented framing (1024/256, 184 hops at
//! 48 kHz, 1792-frame aligned tail); no existing bound is weakened.

use crate::{
    HISS_CAPTURE_CANCEL, HISS_CAPTURE_START, HISS_CLEAR_PROFILE, PluginError, PluginHandle,
    plugin_create, plugin_destroy, plugin_export_preset_json, plugin_free_state,
    plugin_get_last_error, plugin_get_parameter, plugin_get_parameter_count,
    plugin_get_parameter_info, plugin_hiss_capture_control, plugin_import_preset_json,
    plugin_load_state, plugin_process, plugin_reset, plugin_save_state, plugin_set_parameter,
};
use sotf_host::plugin::{ProcessContext, TailLength};
use sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugins::plugin_hiss_reducer::snapshot::ProfileSnapshot;
use std::ffi::{CStr, CString};

const RATE_48K: u32 = 48_000;
const RATE_96K: u32 = 96_000;
const HISS_HOPS_48K: u64 = 184;
const SPECTRAL_TAIL_ALIGNED: usize = 2 * 1024 - 256;

struct HissHandle {
    pointer: *mut PluginHandle,
}

impl HissHandle {
    fn create(kind: &str, config: &str, rate: u32, inputs: usize, outputs: usize) -> Self {
        let kind = CString::new(kind).unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(kind.as_ptr(), config.as_ptr(), rate, inputs, outputs);
        assert!(!handle.is_null(), "construction failed: {}", last_error());
        Self { pointer: handle }
    }

    fn inner(&self) -> &PluginHandle {
        // SAFETY: Guard owns a live handle on this thread.
        unsafe { &*self.pointer }
    }

    fn inner_mut(&mut self) -> &mut PluginHandle {
        // SAFETY: Guard owns a live handle on this thread.
        unsafe { &mut *self.pointer }
    }

    fn param_count(&self) -> usize {
        plugin_get_parameter_count(self.pointer).max(0) as usize
    }

    fn param_id(&self, index: usize) -> String {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null());
        // SAFETY: Info borrows from this live handle.
        let id = unsafe { (*info).id };
        // SAFETY: IDs are NUL-terminated while live.
        unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned()
    }

    fn info_ptr(&self, index: usize) -> *const crate::ParameterInfo {
        plugin_get_parameter_info(self.pointer, index)
    }

    fn set_normalized(&mut self, id: &str, value: f64) -> i32 {
        let id = CString::new(id).unwrap();
        plugin_set_parameter(self.pointer, id.as_ptr(), value)
    }

    fn get_normalized(&self, id: &str) -> f64 {
        let id = CString::new(id).unwrap();
        plugin_get_parameter(self.pointer, id.as_ptr())
    }

    fn capture(&mut self, action: i32) -> i32 {
        plugin_hiss_capture_control(self.pointer, action)
    }

    fn save(&self) -> Vec<u8> {
        let mut len = 0usize;
        let state = plugin_save_state(self.pointer, &mut len);
        assert!(!state.is_null(), "save failed: {}", last_error());
        // SAFETY: FFI owns exactly len bytes until freed below.
        let saved = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        plugin_free_state(state, len);
        saved
    }

    fn save_map(&self) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_slice(&self.save()).unwrap()
    }

    fn load(&mut self, state: &[u8]) -> i32 {
        plugin_load_state(self.pointer, state.as_ptr(), state.len())
    }

    fn reset(&mut self) {
        assert_eq!(plugin_reset(self.pointer), 0, "{}", last_error());
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let inputs = self.inner().input_channels;
        let outputs = self.inner().output_channels;
        let bound = self.inner().max_callback_frames;
        assert_eq!(input.len() % inputs, 0);
        let frames = input.len() / inputs;
        let mut output = vec![f32::NAN; frames * outputs];
        for start in (0..frames).step_by(bound) {
            let count = bound.min(frames - start);
            assert_eq!(
                plugin_process(
                    self.pointer,
                    input[start * inputs..].as_ptr(),
                    output[start * outputs..].as_mut_ptr(),
                    count,
                ),
                0,
                "{}",
                last_error()
            );
        }
        assert!(output.iter().all(|v| v.is_finite()));
        output
    }

    fn process_with_drain(&mut self, input: &[f32]) -> Vec<f32> {
        // The C ABI has no drain call; tests reach the owned plugin
        // directly on this thread (serialized with processing).
        let mut full = self.process(input);
        let channels = self.inner().output_channels;
        let rate = self.inner().sample_rate;
        for _ in 0..4096 {
            let mut block = vec![0.0f32; 256 * channels];
            let status = self
                .inner_mut()
                .plugin
                .drain(&mut block, &ProcessContext::new(rate, 256))
                .unwrap();
            full.extend_from_slice(&block[..status.frames * channels]);
            if status.complete {
                assert!(full.iter().all(|v| v.is_finite()));
                return full;
            }
        }
        panic!("drain did not complete");
    }

    fn snapshot(&self) -> std::sync::Arc<ProfileSnapshot> {
        self.inner()
            .plugin
            .get_data()
            .expect("Hiss must publish get_data")
            .downcast::<ProfileSnapshot>()
            .expect("get_data must downcast to ProfileSnapshot")
    }

    fn config(&self) -> serde_json::Value {
        serde_json::from_str(&self.inner().config_json).unwrap()
    }
}

impl Drop for HissHandle {
    fn drop(&mut self) {
        plugin_destroy(self.pointer);
    }
}

fn last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: Current thread owns this diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
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

fn first_difference_hiss(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let white = lcg_noise(frames, 1.0, seed);
    let mut previous = 0.0f32;
    white
        .iter()
        .map(|&sample| {
            let high = amplitude * (sample - previous);
            previous = sample;
            high
        })
        .collect()
}

fn sine_tone(frames: usize, amplitude: f32, freq_hz: f64, rate: u32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (f64::from(amplitude)
                * (2.0 * std::f64::consts::PI * freq_hz * i as f64 / f64::from(rate)).sin())
                as f32
        })
        .collect()
}

fn interleave_dual_mono(mono: &[f32]) -> Vec<f32> {
    let mut stereo = Vec::with_capacity(mono.len() * 2);
    for &sample in mono {
        stereo.push(sample);
        stereo.push(sample);
    }
    stereo
}

fn regional_mean(spectrum: &[f32], lo_bin: usize, hi_bin: usize) -> f64 {
    let slice = &spectrum[lo_bin..=hi_bin];
    slice.iter().map(|p| f64::from(*p)).sum::<f64>() / slice.len() as f64
}

fn peak(output: &[f32]) -> f32 {
    output.iter().map(|s| s.abs()).fold(0.0, f32::max)
}

fn seeded_v1(channels: usize, rate: u32) -> NoiseProfileData {
    let floors = (0..channels).map(|ch| -40.0 - ch as f32).collect();
    NoiseProfileData {
        format_version: 1,
        sample_rate: f64::from(rate),
        channels,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: floors,
        frames_analyzed: u64::from(rate),
        spectral: None,
    }
}

fn seeded_v2(channels: usize, rate: u32) -> NoiseProfileData {
    let powers = vec![1.0f32; channels * 513];
    NoiseProfileData {
        format_version: 2,
        sample_rate: f64::from(rate),
        channels,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: (0..channels).map(|ch| -40.0 - ch as f32).collect(),
        frames_analyzed: u64::from(rate),
        spectral: Some(
            sotf_plugins::plugin_hiss_reducer::profile::SpectralProfileData {
                fft_size: 1024,
                hop_size: 256,
                window: "hann-periodic".to_string(),
                sample_rate: f64::from(rate),
                channels,
                num_bins: 513,
                power_per_channel_bin: powers,
                hops_analyzed: HISS_HOPS_48K,
            },
        ),
    }
}

#[test]
fn hiss_ffi_actual_capture_save_fresh_load_renders_bitexact_with_eof() {
    // Spectral stereo through the C ABI: capture, save, fresh load, audio.
    let mut source = HissHandle::create(
        "HissReducer",
        r#"{"spectral_mode": true, "strength": 0.85}"#,
        RATE_48K,
        2,
        2,
    );
    assert!(source.inner().plugin.get_data().is_some());
    assert!(source.snapshot().try_export().unwrap().is_none());

    let colored_mono = first_difference_hiss(RATE_48K as usize, 0.035, 0xe940c);
    let colored = interleave_dual_mono(&colored_mono);
    assert_eq!(source.capture(HISS_CAPTURE_START), 0, "{}", last_error());
    assert_eq!(source.get_normalized("learn_noise"), 1.0);
    source.process(&colored);
    assert_eq!(source.get_normalized("learn_noise"), 0.0);
    let export = source
        .snapshot()
        .try_export()
        .unwrap()
        .expect("capture must export");
    assert_eq!(export.profile.format_version, 2);
    assert_eq!(export.profile.channels, 2);
    assert_eq!(
        export.profile.spectral.as_ref().unwrap().hops_analyzed,
        HISS_HOPS_48K
    );
    let powers = &export
        .profile
        .spectral
        .as_ref()
        .unwrap()
        .power_per_channel_bin;
    // Dual-mono capture: both channels share the rising spectrum.
    for ch in 0..2 {
        let band = &powers[ch * 513..(ch + 1) * 513];
        let ratio = regional_mean(band, 86, 149) / regional_mean(band, 341, 490);
        assert!(ratio < 0.5, "channel {ch} must be nonflat: {ratio:.3}");
    }

    assert_eq!(source.set_normalized("use_captured_profile", 1.0), 0);
    let saved = source.save();
    let saved_map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&saved).unwrap();
    assert!(!saved_map.contains_key("learn_noise"));
    assert!(!saved_map.contains_key("clear_profile"));
    let blob: NoiseProfileData =
        serde_json::from_value(saved_map["captured_profile"].clone()).unwrap();
    assert_eq!(blob, export.profile);

    // Fresh handle loads the blob; nonzero mix renders bit-exactly with EOF.
    let mut fresh = HissHandle::create(
        "HissReducer",
        r#"{"spectral_mode": true, "strength": 0.85}"#,
        RATE_48K,
        2,
        2,
    );
    assert_eq!(fresh.load(&saved), 0, "{}", last_error());
    assert_eq!(fresh.get_normalized("use_captured_profile"), 1.0);
    assert_eq!(
        fresh
            .snapshot()
            .try_export()
            .unwrap()
            .expect("fresh must export")
            .profile,
        export.profile
    );

    let hiss = lcg_noise(16384, 0.04, 0xe9f1);
    let tone = sine_tone(16384, 0.06, 9984.375, RATE_48K);
    let mix_mono: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let mix = interleave_dual_mono(&mix_mono);
    source.reset();
    fresh.reset();
    let before = source.process_with_drain(&mix);
    let after = fresh.process_with_drain(&mix);
    assert_eq!(before.len(), 16384 * 2 + SPECTRAL_TAIL_ALIGNED * 2);
    assert!(peak(&before) > 1e-6);
    assert_ne!(&before[..16384 * 2], &mix[..]);
    assert_eq!(before, after);
    assert!(source.inner().plugin.get_data().is_some());
    assert!(fresh.inner().plugin.get_data().is_some());
    assert!(source.inner().plugin.latency_samples() > 0);
    assert_eq!(
        source.inner().plugin.latency_samples(),
        fresh.inner().plugin.latency_samples()
    );
    assert!(matches!(
        fresh.inner().plugin.tail_length(),
        TailLength::Finite(_)
    ));
}

#[test]
fn hiss_ffi_time_domain_v1_capture_roundtrip_zero_tail() {
    // Time-domain mono: actual capture floors carried as v1.
    let mut profiler = HissHandle::create("HissReducer", "{}", RATE_48K, 1, 1);
    let colored = first_difference_hiss(RATE_48K as usize, 0.035, 0xa07c);
    assert_eq!(profiler.capture(HISS_CAPTURE_START), 0, "{}", last_error());
    profiler.process(&colored);
    let v2 = profiler
        .snapshot()
        .try_export()
        .unwrap()
        .expect("capture must export")
        .profile;
    let v1 = NoiseProfileData {
        format_version: 1,
        sample_rate: v2.sample_rate,
        channels: v2.channels,
        measurement_cutoff_hz: v2.measurement_cutoff_hz,
        floor_db_per_channel: v2.floor_db_per_channel.clone(),
        frames_analyzed: v2.frames_analyzed,
        spectral: None,
    };
    v1.validate().unwrap();

    let mut source = HissHandle::create("HissReducer", "{}", RATE_48K, 1, 1);
    let state = serde_json::json!({
        "use_captured_profile": true,
        "captured_profile": v1,
    });
    assert_eq!(
        source.load(&serde_json::to_vec(&state).unwrap()),
        0,
        "{}",
        last_error()
    );
    let saved = source.save_map();
    let back: NoiseProfileData = serde_json::from_value(saved["captured_profile"].clone()).unwrap();
    assert_eq!(back, v1);

    let mut fresh = HissHandle::create("HissReducer", "{}", RATE_48K, 1, 1);
    assert_eq!(
        fresh.load(&serde_json::to_vec(&state).unwrap()),
        0,
        "{}",
        last_error()
    );
    let input = lcg_noise(16384, 0.02, 0x1d0e);
    source.reset();
    fresh.reset();
    let before = source.process_with_drain(&input);
    let after = fresh.process_with_drain(&input);
    assert_eq!(before.len(), 16384);
    assert_eq!(source.inner().plugin.latency_samples(), 0);
    assert!(peak(&before) > 1e-6);
    assert_ne!(&before[..], &input[..]);
    assert_eq!(before, after);
}

#[test]
fn hiss_ffi_partial_omission_null_clear_and_use_off_retain() {
    // Seeded matrix legs for omission/null/use semantics (mono).
    let v2 = seeded_v2(1, RATE_48K);
    v2.validate().unwrap();
    let v1 = seeded_v1(1, RATE_48K);
    v1.validate().unwrap();

    let mut handle =
        HissHandle::create("HissReducer", r#"{"spectral_mode": true}"#, RATE_48K, 1, 1);
    let full = serde_json::json!({
        "use_captured_profile": true,
        "strength": 0.85,
        "captured_profile": v2,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&full).unwrap()),
        0,
        "{}",
        last_error()
    );

    // Partial omission preserves the accepted profile.
    let partial = serde_json::json!({"strength": 0.6});
    assert_eq!(
        handle.load(&serde_json::to_vec(&partial).unwrap()),
        0,
        "{}",
        last_error()
    );
    let kept: NoiseProfileData =
        serde_json::from_value(handle.save_map()["captured_profile"].clone()).unwrap();
    assert_eq!(kept, v2);

    // Explicit null clears without touching scalars.
    let clear = serde_json::json!({"captured_profile": null});
    assert_eq!(
        handle.load(&serde_json::to_vec(&clear).unwrap()),
        0,
        "{}",
        last_error()
    );
    let cleared = handle.save_map();
    assert!(!cleared.contains_key("captured_profile"));
    assert!(handle.snapshot().try_export().unwrap().is_none());

    // v1 installs exactly; use-off retains the payload but disengages.
    let v1_state = serde_json::json!({
        "use_captured_profile": true,
        "captured_profile": v1,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&v1_state).unwrap()),
        0,
        "{}",
        last_error()
    );
    assert_eq!(handle.set_normalized("use_captured_profile", 0.0), 0);
    let retained: NoiseProfileData =
        serde_json::from_value(handle.save_map()["captured_profile"].clone()).unwrap();
    assert_eq!(retained, v1);
    let export = handle
        .snapshot()
        .try_export()
        .unwrap()
        .expect("payload survives use-off");
    assert_eq!(export.profile, v1);
    assert!(!export.use_flag && !export.engaged);
}

#[test]
fn hiss_ffi_crossrate_v2_matches_explicit_v1_fallback() {
    // 48 kHz blobs stay stored at 96 kHz but render as floors-only.
    let v2_48 = seeded_v2(1, RATE_48K);
    let v1_48 = seeded_v1(1, RATE_48K);
    let input = lcg_noise(16384, 0.04, 0x96c0);

    let mut with_v2 =
        HissHandle::create("HissReducer", r#"{"spectral_mode": true}"#, RATE_96K, 1, 1);
    let state_v2 = serde_json::json!({
        "use_captured_profile": true,
        "strength": 0.85,
        "captured_profile": v2_48,
    });
    assert_eq!(
        with_v2.load(&serde_json::to_vec(&state_v2).unwrap()),
        0,
        "{}",
        last_error()
    );
    let kept: NoiseProfileData =
        serde_json::from_value(with_v2.save_map()["captured_profile"].clone()).unwrap();
    assert_eq!(kept.sample_rate, f64::from(RATE_48K));
    assert!(!with_v2.snapshot().try_export().unwrap().unwrap().engaged);

    let mut with_v1 =
        HissHandle::create("HissReducer", r#"{"spectral_mode": true}"#, RATE_96K, 1, 1);
    let state_v1 = serde_json::json!({
        "use_captured_profile": true,
        "strength": 0.85,
        "captured_profile": v1_48,
    });
    assert_eq!(
        with_v1.load(&serde_json::to_vec(&state_v1).unwrap()),
        0,
        "{}",
        last_error()
    );

    with_v2.reset();
    with_v1.reset();
    let out_v2 = with_v2.process_with_drain(&input);
    let out_v1 = with_v1.process_with_drain(&input);
    assert_eq!(out_v2, out_v1);
    assert!(peak(&out_v2) > 1e-6);
}

#[test]
fn hiss_ffi_malformed_rejected_live_retained() {
    let v2 = seeded_v2(1, RATE_48K);
    let mut handle =
        HissHandle::create("HissReducer", r#"{"spectral_mode": true}"#, RATE_48K, 1, 1);
    let good = serde_json::json!({
        "use_captured_profile": true,
        "strength": 0.85,
        "captured_profile": v2,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&good).unwrap()),
        0,
        "{}",
        last_error()
    );
    handle.reset();
    let input = lcg_noise(8192, 0.04, 0x77aa);
    let reference = handle.process_with_drain(&input);
    let before_state = handle.save();
    let before_config = handle.config();

    let base = serde_json::to_value(&v2).unwrap();
    let mut bad_version = base.clone();
    bad_version["format_version"] = serde_json::json!(99);
    let mut bad_fft = base.clone();
    bad_fft["spectral"]["fft_size"] = serde_json::json!(2048);
    let mut bad_len = base.clone();
    bad_len["spectral"]["power_per_channel_bin"]
        .as_array_mut()
        .unwrap()
        .pop();
    let mut bad_power = base.clone();
    bad_power["spectral"]["power_per_channel_bin"][11] = serde_json::json!(-1.0);
    let mut v1_with_spectral = base.clone();
    v1_with_spectral["format_version"] = serde_json::json!(1);
    let mut v2_without_spectral = base.clone();
    v2_without_spectral
        .as_object_mut()
        .unwrap()
        .remove("spectral");
    let bad_channels = serde_json::to_value(seeded_v2(2, RATE_48K)).unwrap();

    for (label, blob) in [
        ("bad-version", bad_version),
        ("bad-fft", bad_fft),
        ("bad-length", bad_len),
        ("negative-power", bad_power),
        ("v1-with-spectral", v1_with_spectral),
        ("v2-without-spectral", v2_without_spectral),
        ("bad-channels", bad_channels),
        ("non-object", serde_json::json!("nope")),
    ] {
        let candidate = serde_json::json!({"captured_profile": blob});
        assert_ne!(
            handle.load(&serde_json::to_vec(&candidate).unwrap()),
            0,
            "{label} must fail"
        );
        let error = last_error().to_lowercase();
        assert!(
            error.contains("profile") || error.contains("hiss"),
            "{label} must name the profile: {error}"
        );
    }

    assert_eq!(handle.save(), before_state);
    assert_eq!(handle.config(), before_config);
    assert_eq!(
        handle
            .snapshot()
            .try_export()
            .unwrap()
            .expect("live profile retained")
            .profile,
        v2
    );
    handle.reset();
    assert_eq!(handle.process_with_drain(&input), reference);
}

#[test]
fn hiss_ffi_busy_never_drops_blob() {
    // Contended saves either carry the profile or fail as busy, never as
    // a successful absence. Publisher churns the generation from another
    // thread while this thread saves through the C ABI.
    let mut handle =
        HissHandle::create("HissReducer", r#"{"spectral_mode": true}"#, RATE_48K, 1, 1);
    let v2 = seeded_v2(1, RATE_48K);
    let good = serde_json::json!({
        "use_captured_profile": true,
        "captured_profile": v2,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&good).unwrap()),
        0,
        "{}",
        last_error()
    );

    let snapshot = handle.snapshot();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn_stop = stop.clone();
    let churn = std::thread::spawn(move || {
        let mut flag = false;
        while !churn_stop.load(std::sync::atomic::Ordering::Relaxed) {
            flag = !flag;
            snapshot.publish_live_flags(flag, f64::from(RATE_48K));
        }
    });

    let mut saw_ok = false;
    let mut saw_busy = false;
    for _ in 0..200 {
        let mut len = 0usize;
        let state = plugin_save_state(handle.pointer, &mut len);
        if state.is_null() {
            assert_eq!(len, 0);
            let error = last_error().to_lowercase();
            assert!(
                error.contains("busy"),
                "null save must diagnose contention: {error}"
            );
            saw_busy = true;
        } else {
            // SAFETY: FFI owns exactly len bytes until freed below.
            let saved = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
            plugin_free_state(state, len);
            let map: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(&saved).unwrap();
            let blob: NoiseProfileData = serde_json::from_value(map["captured_profile"].clone())
                .expect("successful save must carry the blob");
            assert_eq!(blob, v2);
            saw_ok = true;
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    churn.join().unwrap();
    assert!(saw_ok, "churn must still permit successful saves");
    // Busy may or may not trigger under this churn; the load-bearing claim
    // is that no success ever omits the blob. When busy occurs, it is loud.
    let _ = saw_busy;
}

#[test]
fn hiss_ffi_no_action_replay_and_pointers_stable() {
    let v2 = seeded_v2(2, RATE_48K);
    let mut handle = HissHandle::create("HissReducer", "{}", RATE_48K, 2, 2);
    let good = serde_json::json!({
        "use_captured_profile": true,
        "strength": 0.5,
        "captured_profile": v2,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&good).unwrap()),
        0,
        "{}",
        last_error()
    );
    assert_eq!(handle.param_count(), 13);
    let before_ids: Vec<String> = (0..13).map(|i| handle.param_id(i)).collect();
    let before_ptrs: Vec<*const crate::ParameterInfo> =
        (0..13).map(|i| handle.info_ptr(i)).collect();

    // Legacy triggers must not fire; the scalar applies and the blob stays.
    let legacy = serde_json::json!({
        "learn_noise": true,
        "clear_profile": true,
        "strength": 0.75,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&legacy).unwrap()),
        0,
        "{}",
        last_error()
    );
    assert_eq!(handle.get_normalized("learn_noise"), 0.0);
    let kept: NoiseProfileData =
        serde_json::from_value(handle.save_map()["captured_profile"].clone()).unwrap();
    assert_eq!(kept, v2);

    // Foreign metadata pointers stay stable across the Hiss restore.
    assert_eq!(handle.param_count(), 13);
    for i in 0..13 {
        assert_eq!(handle.param_id(i), before_ids[i]);
        assert_eq!(handle.info_ptr(i), before_ptrs[i]);
    }

    // The realtime refusal contract is unchanged by the new control API.
    assert_ne!(handle.set_normalized("learn_noise", 1.0), 0);
    assert!(last_error().contains("restoration"));
    assert_ne!(handle.set_normalized("clear_profile", 1.0), 0);
    assert!(last_error().contains("restoration"));
    assert_ne!(handle.set_normalized("spectral_mode", 1.0), 0);

    let input = interleave_dual_mono(&lcg_noise(2048, 0.25, 0x440));
    handle.reset();
    assert!(peak(&handle.process(&input)) > 0.05);
}

#[test]
fn hiss_ffi_capture_control_validates_and_never_replays() {
    let mut handle = HissHandle::create("HissReducer", "{}", RATE_48K, 1, 1);
    // Handle/type/action validation without touching DSP.
    assert_eq!(
        plugin_hiss_capture_control(std::ptr::null_mut(), HISS_CAPTURE_START),
        PluginError::NullPointer as i32
    );
    let mut gain = HissHandle::create("Gain", "{}", RATE_48K, 2, 2);
    assert_eq!(
        gain.capture(HISS_CAPTURE_START),
        PluginError::UnsupportedFeature as i32
    );
    assert_eq!(handle.capture(99), PluginError::InvalidParameter as i32);

    // Start/cancel/clear round-trip through the explicit control.
    assert_eq!(handle.capture(HISS_CAPTURE_START), 0, "{}", last_error());
    assert_eq!(handle.get_normalized("learn_noise"), 1.0);
    assert_eq!(handle.capture(HISS_CAPTURE_CANCEL), 0, "{}", last_error());
    assert_eq!(handle.get_normalized("learn_noise"), 0.0);
    assert_eq!(handle.capture(HISS_CLEAR_PROFILE), 0, "{}", last_error());
    assert!(handle.snapshot().try_export().unwrap().is_none());

    // Restoration discards an active capture without starting a new one.
    assert_eq!(handle.capture(HISS_CAPTURE_START), 0, "{}", last_error());
    let partial = serde_json::json!({"strength": 0.7});
    assert_eq!(
        handle.load(&serde_json::to_vec(&partial).unwrap()),
        0,
        "{}",
        last_error()
    );
    assert_eq!(handle.get_normalized("learn_noise"), 0.0);
    assert!(handle.snapshot().try_export().unwrap().is_none());

    // Preset documents carry the blob through the same transactional path.
    let v1 = seeded_v1(1, RATE_48K);
    let with_profile = serde_json::json!({
        "use_captured_profile": true,
        "captured_profile": v1,
    });
    assert_eq!(
        handle.load(&serde_json::to_vec(&with_profile).unwrap()),
        0,
        "{}",
        last_error()
    );
    let mut len = 0usize;
    let name = CString::new("hiss-v1").unwrap();
    let document = plugin_export_preset_json(handle.pointer, name.as_ptr(), &mut len);
    assert!(!document.is_null(), "{}", last_error());
    // SAFETY: FFI owns exactly len bytes until freed below.
    let bytes = unsafe { std::slice::from_raw_parts(document, len) }.to_vec();
    plugin_free_state(document, len);
    let fresh = HissHandle::create("HissReducer", "{}", RATE_48K, 1, 1);
    assert_eq!(
        plugin_import_preset_json(fresh.pointer, bytes.as_ptr(), bytes.len()),
        0,
        "{}",
        last_error()
    );
    let back: NoiseProfileData =
        serde_json::from_value(fresh.save_map()["captured_profile"].clone()).unwrap();
    assert_eq!(back, v1);
}
