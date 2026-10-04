//! Engine Hiss measured-profile persistence through settings and factory.
//!
//! Carries actual captured v1/v2 blobs through [`PluginSettings`]
//! serialization, [`PluginSettings::to_plugin_config`], and the hosted
//! factory, then renders nonzero audio before and after save/reload with
//! bit-exact process/drain output, contract latency, and full EOF delivery.

// Rust guideline compliant 2026-02-21

use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::ParametricInPlacePlugin;
use sotf_plugins::plugin_hiss_reducer::HissReducerPlugin;
use sotf_plugins::plugin_hiss_reducer::profile::{
    NoiseProfileData, PROFILE_FORMAT_VERSION, PROFILE_FORMAT_VERSION_V1,
};
use sotf_plugins::{ParameterId, ParameterValue, ProcessContext, create_plugin};

const RATE_48K: u32 = 48_000;
const RATE_96K: u32 = 96_000;
/// Spectral drain tail for 256-aligned input: `2 * 1024 - 256` frames.
/// Derived from the documented WOLA framing (N=1024, H=256); the Hiss
/// finite-stream suite pins the same endpoint across hop phases.
const SPECTRAL_DRAIN_TAIL_FRAMES: usize = 2 * 1024 - 256;

fn lcg_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

/// Two-pole lowpass (cascaded exact-mapped one-poles) for coloring.
fn lowpass_two_pole(signal: &[f32], cutoff_hz: f64, rate: f64) -> Vec<f32> {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / rate).exp();
    let mut first = 0.0;
    let mut second = 0.0;
    signal
        .iter()
        .map(|&sample| {
            first = alpha * f64::from(sample) + (1.0 - alpha) * first;
            second = alpha * first + (1.0 - alpha) * second;
            second as f32
        })
        .collect()
}

/// Independent f64 one-pole high-band power oracle.
fn oracle_high_band_power(signal: &[f32], cutoff_hz: f64, rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in signal {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let high = dry - low;
        sum += high * high;
    }
    sum / signal.len() as f64
}

fn scale_to_highband_rms(signal: &[f32], cutoff_hz: f64, rate: f64, target_rms: f64) -> Vec<f32> {
    let power = oracle_high_band_power(signal, cutoff_hz, rate);
    let gain = target_rms / power.sqrt();
    signal.iter().map(|s| (*s as f64 * gain) as f32).collect()
}

fn regional_mean(spectrum: &[f32], lo_bin: usize, hi_bin: usize) -> f64 {
    let slice = &spectrum[lo_bin..=hi_bin];
    slice.iter().map(|p| f64::from(*p)).sum::<f64>() / slice.len() as f64
}

/// Captures a genuinely nonflat v2 profile from colored noise.
///
/// Uses the live capture engine (never a spectrum synthesized from scalar
/// floors): lowpassed noise normalized to -36 dBFS high-band RMS, fed as a
/// full 1 s capture. Returns the validated exported blob.
fn capture_colored_v2() -> NoiseProfileData {
    let white = lcg_noise(RATE_48K as usize, 0.05, 0xc010);
    let low_raw = lowpass_two_pole(&white, 6000.0, f64::from(RATE_48K));
    let colored = scale_to_highband_rms(
        &low_raw,
        4000.0,
        f64::from(RATE_48K),
        10.0f64.powf(-36.0 / 20.0),
    );
    let mut profiler = HissReducerPlugin::new(1);
    profiler.initialize(f64::from(RATE_48K)).unwrap();
    profiler
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    assert!(profiler.is_capturing());
    let mut cursor = 0;
    while cursor < colored.len() {
        let count = 4096.min(colored.len() - cursor);
        let mut block = colored[cursor..cursor + count].to_vec();
        profiler
            .process_in_place(&mut block, &ProcessContext::new(RATE_48K, count))
            .unwrap();
        cursor += count;
    }
    assert!(!profiler.is_capturing());
    assert!(profiler.has_captured_profile());
    assert!(profiler.has_measured_spectrum());
    let profile = profiler
        .persisted_params()
        .captured_profile
        .expect("capture must export a profile");
    assert_eq!(profile.format_version, PROFILE_FORMAT_VERSION);
    profile.validate().unwrap();
    assert_eq!(
        profile.spectral.as_ref().unwrap().hops_analyzed,
        184,
        "1 s at 48 kHz yields (48000 - 1024) / 256 + 1 full windows"
    );
    // Nonflat fixture sanity, kept below the 4x Hiss design bound: the
    // lowpassed color dominates 4-7 kHz (bins 86-149) over 16-23 kHz
    // (bins 341-490).
    let powers = &profile.spectral.as_ref().unwrap().power_per_channel_bin;
    let ratio = regional_mean(powers, 86, 149) / regional_mean(powers, 341, 490);
    assert!(
        ratio > 2.0,
        "captured spectrum must be nonflat, low/high ratio {ratio:.2}"
    );
    profile
}

/// Builds a valid v1 blob from measured floors (no spectrum).
fn v1_floors_from(v2: &NoiseProfileData) -> NoiseProfileData {
    let v1 = NoiseProfileData {
        format_version: PROFILE_FORMAT_VERSION_V1,
        sample_rate: v2.sample_rate,
        channels: v2.channels,
        measurement_cutoff_hz: v2.measurement_cutoff_hz,
        floor_db_per_channel: v2.floor_db_per_channel.clone(),
        frames_analyzed: v2.frames_analyzed,
        spectral: None,
    };
    v1.validate().unwrap();
    v1
}

fn hiss_settings_with(
    profile: &NoiseProfileData,
    spectral_mode: bool,
    use_profile: bool,
    strength: f64,
) -> PluginSettings {
    let mut settings = PluginSettings::default_for(&PluginType::HissReducer).unwrap();
    let PluginSettings::HissReducer {
        spectral_mode: spectral,
        use_captured_profile: use_flag,
        strength: strength_field,
        captured_profile: blob,
        ..
    } = &mut settings
    else {
        panic!("HissReducer default must have HissReducer settings");
    };
    *spectral = spectral_mode;
    *use_flag = use_profile;
    *strength_field = strength;
    *blob = Some(NoiseProfileData::clone(profile));
    settings
}

/// Renders through settings, converter, and the hosted factory.
///
/// Returns the full output (process frames plus complete EOF drain, with no
/// trimming or padding) and the contract latency.
fn render_hosted_through_settings(
    settings: &PluginSettings,
    channels: usize,
    rate: u32,
    input: &[f32],
) -> (Vec<f32>, usize) {
    let config = settings.to_plugin_config(f64::from(rate));
    let mut plugin =
        create_plugin(&config.plugin_type, &config.parameters, channels, rate).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let latency = plugin.latency_samples();
    let frames = input.len() / channels;
    let mut output = vec![f32::NAN; input.len()];
    let processed = plugin
        .process(input, &mut output, &ProcessContext::new(rate, frames))
        .unwrap();
    assert_eq!(processed, frames, "hosted process must preserve length");
    let mut full = output;
    for _ in 0..4096 {
        let mut block = vec![0.0; 256 * channels];
        let status = plugin
            .drain(&mut block, &ProcessContext::new(rate, 256))
            .unwrap();
        full.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            return (full, latency);
        }
    }
    panic!("drain did not complete");
}

fn render_process_only(
    plugin: &mut Box<dyn sotf_plugins::Plugin>,
    input: &[f32],
    rate: u32,
) -> Vec<f32> {
    let frames = input.len();
    let mut output = vec![f32::NAN; input.len()];
    let processed = plugin
        .process(input, &mut output, &ProcessContext::new(rate, frames))
        .unwrap();
    assert_eq!(processed, frames, "hosted process must preserve length");
    output
}

fn assert_nonzero_finite(output: &[f32]) {
    assert!(
        output.iter().all(|s| s.is_finite()),
        "output must be finite"
    );
    let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(peak > 1e-6, "output must be nonzero, peak {peak:.3e}");
}

#[test]
fn hiss_legacy_settings_keep_profile_less_shape_and_defaults() {
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "HissReducer": {
            "enabled": true,
            "threshold_db": -30.0,
            "frequency_hz": 4000.0,
            "strength": 0.5
        }
    }))
    .unwrap();
    // Exhaustive: pins the owned settings shape including the new carrier.
    let PluginSettings::HissReducer {
        enabled,
        threshold_db,
        frequency_hz,
        strength,
        spectral_mode,
        learn_noise,
        use_captured_profile,
        clear_profile,
        curve_low,
        curve_mid,
        curve_high,
        link_mode,
        transient_guard,
        captured_profile,
    } = &legacy
    else {
        panic!("legacy preset must deserialize to HissReducer settings");
    };
    assert!(*enabled);
    assert_eq!(*threshold_db, -30.0);
    assert_eq!(*frequency_hz, 4000.0);
    assert_eq!(*strength, 0.5);
    assert!(!*spectral_mode);
    assert!(!*learn_noise);
    assert!(!*use_captured_profile);
    assert!(!*clear_profile);
    assert_eq!(*curve_low, 1.0);
    assert_eq!(*curve_mid, 1.0);
    assert_eq!(*curve_high, 1.0);
    assert_eq!(*link_mode, 0);
    assert!(!*transient_guard);
    assert!(captured_profile.is_none());
    // Legacy serialized shape carries no blob key.
    let saved = serde_json::to_value(&legacy).unwrap();
    assert!(
        saved["HissReducer"].get("captured_profile").is_none(),
        "legacy settings must serialize without a profile blob"
    );
    // Accessor contract unchanged: the carrier is out-of-band state.
    assert_eq!(legacy.param_specs().len(), 13);
    // Converter output has no blob or trigger keys for legacy settings.
    let config = legacy.to_plugin_config(48_000.0);
    assert_eq!(config.plugin_type, "hiss_reducer");
    for key in ["captured_profile", "learn_noise", "clear_profile"] {
        assert!(
            config.parameters.get(key).is_none(),
            "{key} must be absent from legacy construction JSON"
        );
    }
    assert_eq!(config.parameters["transient_guard"], false);
    assert_eq!(config.parameters["use_captured_profile"], false);
    // The hosted factory constructs legacy defaults, guard off.
    let plugin = create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("transient_guard")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(plugin.latency_samples(), 0);
}

#[test]
fn hiss_measured_profiles_roundtrip_through_settings_converter_and_factory() {
    let v2 = capture_colored_v2();
    let v1 = v1_floors_from(&v2);
    for (profile, version) in [
        (v2, PROFILE_FORMAT_VERSION),
        (v1, PROFILE_FORMAT_VERSION_V1),
    ] {
        let settings = hiss_settings_with(&profile, true, true, 0.85);
        let saved = serde_json::to_vec(&settings).unwrap();
        // The blob is present verbatim in settings JSON. Both sides use the
        // same decimal wire convention: the standalone original profile is
        // serialized and re-parsed as `Value`, so the comparison cannot
        // confuse direct f32-to-f64 `Value` expansion with the wire text the
        // settings actually persist.
        let saved_json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        let expected_json: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&profile).unwrap()).unwrap();
        assert_eq!(
            &saved_json["HissReducer"]["captured_profile"], &expected_json,
            "v{version} blob must persist verbatim in settings JSON"
        );
        let restored: PluginSettings = serde_json::from_slice(&saved).unwrap();
        let PluginSettings::HissReducer {
            captured_profile: restored_blob,
            ..
        } = &restored
        else {
            panic!("restored preset must deserialize to HissReducer settings");
        };
        assert_eq!(
            restored_blob.as_ref().unwrap(),
            &profile,
            "v{version} blob must survive settings save/reload bit-exactly"
        );
        // The converter forwards the blob verbatim and still drops triggers.
        let config = restored.to_plugin_config(48_000.0);
        assert!(config.parameters.get("learn_noise").is_none());
        assert!(config.parameters.get("clear_profile").is_none());
        let forwarded: NoiseProfileData =
            serde_json::from_value(config.parameters["captured_profile"].clone()).unwrap();
        assert_eq!(forwarded.format_version, version);
        assert_eq!(forwarded, profile);
        forwarded.validate().unwrap();
        // The hosted factory accepts the forwarded blob and exposes controls.
        let plugin = create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("use_captured_profile")),
            Some(ParameterValue::Bool(true))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("spectral_mode")),
            Some(ParameterValue::Bool(true))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("strength")),
            Some(ParameterValue::Float(0.85))
        );
    }
}

#[test]
fn hiss_profile_audio_is_bit_exact_across_save_reload_with_full_eof() {
    let v2 = capture_colored_v2();
    let v1 = v1_floors_from(&v2);

    // Spectral leg: engaged v2 renders nonzero audio with a finite tail.
    let spectral = hiss_settings_with(&v2, true, true, 0.85);
    let input = lcg_noise(16384, 0.04, 0xd15c);
    let (before, latency_before) = render_hosted_through_settings(&spectral, 1, RATE_48K, &input);
    assert!(latency_before > 0, "spectral hiss has nonzero latency");
    assert_eq!(
        before.len(),
        16384 + SPECTRAL_DRAIN_TAIL_FRAMES,
        "process frames plus complete drain, no trimming or padding"
    );
    assert_nonzero_finite(&before);
    assert_ne!(
        &before[..16384],
        &input[..],
        "engaged spectral reduction must process the input"
    );
    let saved = serde_json::to_vec(&spectral).unwrap();
    let restored: PluginSettings = serde_json::from_slice(&saved).unwrap();
    let (after, latency_after) = render_hosted_through_settings(&restored, 1, RATE_48K, &input);
    assert_eq!(latency_before, latency_after);
    assert_eq!(
        before, after,
        "process plus full EOF drain must match bit-exactly across reload"
    );

    // Time-domain leg: v1 floors, zero latency, immediate EOF completion.
    let time_domain = hiss_settings_with(&v1, false, true, 0.5);
    let quiet = lcg_noise(16384, 0.02, 0x1d0e);
    let (td_before, td_latency_before) =
        render_hosted_through_settings(&time_domain, 1, RATE_48K, &quiet);
    assert_eq!(td_latency_before, 0);
    assert_eq!(
        td_before.len(),
        16384,
        "time-domain drain completes with zero tail frames"
    );
    assert_nonzero_finite(&td_before);
    assert_ne!(
        &td_before[..],
        &quiet[..],
        "engaged time-domain reduction must process the input"
    );
    let td_saved = serde_json::to_vec(&time_domain).unwrap();
    let td_restored: PluginSettings = serde_json::from_slice(&td_saved).unwrap();
    let (td_after, td_latency_after) =
        render_hosted_through_settings(&td_restored, 1, RATE_48K, &quiet);
    assert_eq!(td_latency_before, td_latency_after);
    assert_eq!(
        td_before, td_after,
        "time-domain output must match bit-exactly across reload"
    );
}

/// Returns a copy of `base` with `mutate` applied (malformed candidates).
fn mutated_blob(
    base: serde_json::Value,
    mutate: &dyn Fn(&mut serde_json::Value),
) -> serde_json::Value {
    let mut blob = base;
    mutate(&mut blob);
    blob
}

#[test]
fn hiss_malformed_profile_rejected_by_factory_and_accepted_state_retained() {
    let v2 = capture_colored_v2();
    let settings = hiss_settings_with(&v2, true, true, 0.85);
    let mut config = settings.to_plugin_config(f64::from(RATE_48K));
    // Accepted construction renders the reference before any rejection.
    let mut accepted = create_plugin(&config.plugin_type, &config.parameters, 1, RATE_48K).unwrap();
    accepted.initialize(f64::from(RATE_48K)).unwrap();
    let input = lcg_noise(8192, 0.04, 0x77aa);
    let reference = render_process_only(&mut accepted, &input, RATE_48K);

    let base = config.parameters["captured_profile"].clone();
    let candidates: [(&str, serde_json::Value); 6] = [
        (
            "bad-fft",
            mutated_blob(base.clone(), &|blob| {
                blob["spectral"]["fft_size"] = serde_json::json!(2048);
            }),
        ),
        (
            "bad-length",
            mutated_blob(base.clone(), &|blob| {
                blob["spectral"]["power_per_channel_bin"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }),
        ),
        (
            "negative-power",
            mutated_blob(base.clone(), &|blob| {
                blob["spectral"]["power_per_channel_bin"][11] = serde_json::json!(-1.0);
            }),
        ),
        (
            "v1-with-spectral",
            mutated_blob(base.clone(), &|blob| {
                blob["format_version"] = serde_json::json!(1);
            }),
        ),
        (
            "v2-without-spectral",
            mutated_blob(base.clone(), &|blob| {
                blob.as_object_mut().unwrap().remove("spectral");
            }),
        ),
        (
            "bad-version",
            mutated_blob(base.clone(), &|blob| {
                blob["format_version"] = serde_json::json!(99);
            }),
        ),
    ];
    for (label, blob) in &candidates {
        config.parameters["captured_profile"] = blob.clone();
        let error = create_plugin(&config.plugin_type, &config.parameters, 1, RATE_48K)
            .err()
            .unwrap_or_else(|| panic!("{label} must fail factory construction"));
        assert!(
            error.to_lowercase().contains("profile"),
            "{label} must explain the profile rejection: {error}"
        );
    }

    // The accepted instance is untouched by the failed constructions: after
    // reset it renders the reference bit-exactly. This claims only the
    // exercised path (construction rejection plus re-render), not a general
    // live-restore rollback, which stays Hiss-owner scope.
    accepted.reset();
    let after = render_process_only(&mut accepted, &input, RATE_48K);
    assert_eq!(
        reference, after,
        "rejected candidates must retain accepted audio"
    );
}

#[test]
fn hiss_cross_rate_v2_matches_explicit_v1_fallback() {
    let v2 = capture_colored_v2();
    let v1 = v1_floors_from(&v2);
    let settings_v2 = hiss_settings_with(&v2, true, true, 0.85);
    let settings_v1 = hiss_settings_with(&v1, true, true, 0.85);

    // At 96 kHz the 48 kHz spectrum stays stored but disengaged: audio
    // matches the explicit v1 floors-only fallback bit-exactly, proving no
    // implicit cross-rate spectra through the settings/converter path.
    let input_96 = lcg_noise(16384, 0.04, 0x96c0);
    let (out_v2_96, latency_v2_96) =
        render_hosted_through_settings(&settings_v2, 1, RATE_96K, &input_96);
    let (out_v1_96, latency_v1_96) =
        render_hosted_through_settings(&settings_v1, 1, RATE_96K, &input_96);
    assert_eq!(latency_v2_96, latency_v1_96);
    assert_eq!(
        out_v2_96, out_v1_96,
        "cross-rate v2 must equal the explicit v1 white-spread fallback"
    );
    assert_nonzero_finite(&out_v2_96);

    // At the 48 kHz capture rate the carried spectrum engages: v2 differs
    // from the v1 fallback, proving the blob reaches spectral DSP.
    let input_48 = lcg_noise(16384, 0.04, 0xc470);
    let (out_v2_48, _) = render_hosted_through_settings(&settings_v2, 1, RATE_48K, &input_48);
    let (out_v1_48, _) = render_hosted_through_settings(&settings_v1, 1, RATE_48K, &input_48);
    assert_ne!(
        out_v2_48, out_v1_48,
        "engaged spectrum must differ from white-spread at the capture rate"
    );
    assert_nonzero_finite(&out_v2_48);
    assert_nonzero_finite(&out_v1_48);
}

#[test]
fn hiss_momentary_capture_actions_never_replay_on_load() {
    let v2 = capture_colored_v2();
    // A preset with both triggers armed plus a carried blob.
    let mut saved = serde_json::to_value(hiss_settings_with(&v2, false, true, 0.5)).unwrap();
    saved["HissReducer"]["learn_noise"] = serde_json::json!(true);
    saved["HissReducer"]["clear_profile"] = serde_json::json!(true);
    let loaded: PluginSettings = serde_json::from_value(saved).unwrap();
    // Settings preserve the momentary UI state, but the constructor JSON
    // must never carry the triggers.
    let PluginSettings::HissReducer {
        learn_noise,
        clear_profile,
        ..
    } = &loaded
    else {
        panic!("preset must deserialize to HissReducer settings");
    };
    assert!(*learn_noise);
    assert!(*clear_profile);
    let config = loaded.to_plugin_config(48_000.0);
    assert!(config.parameters.get("learn_noise").is_none());
    assert!(config.parameters.get("clear_profile").is_none());
    assert!(config.parameters.get("captured_profile").is_some());
    // Construction succeeds under deny_unknown_fields and starts idle with
    // the profile flag applied.
    let plugin = create_plugin(&config.plugin_type, &config.parameters, 1, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("clear_profile")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(true))
    );
}
