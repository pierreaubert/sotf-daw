//! Noise-profile capture, persistence, and threshold-following tests.
//!
//! Tolerances are fixed here before any candidate runs: floor agreement
//! 0.15 dB against an independent f64 one-pole oracle, suppression and
//! preservation bounds in dB per case, and bit-exact round-trip/state
//! assertions where the contract is exact.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_host::{CountingAlloc, assert_no_allocs};
use sotf_plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const RATE: u32 = 48_000;

fn ctx(frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(RATE, frames)
}

fn time_domain_plugin(channels: usize) -> HissReducerPlugin {
    let mut plugin = HissReducerPlugin::new(channels);
    plugin.initialize(RATE).unwrap();
    plugin
}

fn lcg_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

/// Independent f64 one-pole high-band floor oracle (same documented split).
fn oracle_high_band_floor_db(input: &[f32], cutoff_hz: f64, sample_rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / sample_rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in input {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let high = dry - low;
        sum += high * high;
    }
    10.0 * (sum / input.len() as f64).log10()
}

/// Independent f64 one-pole high-band power over a slice.
fn oracle_high_band_power(input: &[f32], cutoff_hz: f64, sample_rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / sample_rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in input {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let high = dry - low;
        sum += high * high;
    }
    sum / input.len() as f64
}

/// Independent f64 one-pole low-band power over a slice.
fn oracle_low_band_power(input: &[f32], cutoff_hz: f64, sample_rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / sample_rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in input {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        sum += low * low;
    }
    sum / input.len() as f64
}

fn process_all(
    plugin: &mut HissReducerPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    assert_eq!(input.len() % channels, 0);
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut pos = 0;
    let mut call = 0;
    while pos < frames {
        let count = blocks[call % blocks.len()].min(frames - pos);
        plugin
            .process_in_place(
                &mut output[pos * channels..(pos + count) * channels],
                &ctx(count),
            )
            .unwrap();
        pos += count;
        call += 1;
    }
    output
}

fn start_capture(plugin: &mut HissReducerPlugin) {
    plugin
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    assert!(plugin.is_capturing());
}

fn drain_all(plugin: &mut HissReducerPlugin, channels: usize) -> Vec<f32> {
    let mut output = Vec::new();
    for _ in 0..4096 {
        let mut block = vec![0.0; 256 * channels];
        let status = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 256))
            .unwrap();
        output.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            return output;
        }
    }
    panic!("drain did not complete");
}

fn spectral_plugin(channels: usize) -> HissReducerPlugin {
    let mut plugin = HissReducerPlugin::from_params(
        channels,
        HissReducerPluginParams {
            spectral_mode: true,
            ..HissReducerPluginParams::default()
        },
    );
    plugin.initialize(RATE).unwrap();
    plugin
}

#[test]
fn capture_measures_high_band_floor_accurately() {
    // Mono floor against the independent oracle.
    let mut plugin = time_domain_plugin(1);
    let noise = lcg_noise(RATE as usize, 0.05, 0x1234_5678);
    let expected = oracle_high_band_floor_db(&noise, 4000.0, f64::from(RATE));
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[1, 137, 4096]);
    assert!(!plugin.is_capturing(), "capture must complete after 1 s");
    assert!(plugin.has_captured_profile());
    let measured = f64::from(plugin.overall_profile_floor_db().unwrap());
    assert!(
        (measured - expected).abs() < 0.15,
        "floor {measured:.3} dB vs oracle {expected:.3} dB"
    );
    assert_eq!(
        plugin.profile_metadata(),
        Some((RATE, 4000.0, u64::from(RATE)))
    );

    // Stereo floors stay per-channel.
    let mut stereo = time_domain_plugin(2);
    let left = lcg_noise(RATE as usize, 0.05, 0x1);
    let right = lcg_noise(RATE as usize, 0.01, 0x2);
    let mut interleaved = Vec::with_capacity(2 * RATE as usize);
    for frame in 0..RATE as usize {
        interleaved.push(left[frame]);
        interleaved.push(right[frame]);
    }
    start_capture(&mut stereo);
    process_all(&mut stereo, &interleaved, 2, &[7, 511, 64]);
    let floors = stereo.persisted_params().captured_profile.unwrap();
    assert_eq!(floors.floor_db_per_channel.len(), 2);
    let expected_left = oracle_high_band_floor_db(&left, 4000.0, f64::from(RATE));
    let expected_right = oracle_high_band_floor_db(&right, 4000.0, f64::from(RATE));
    assert!(
        (f64::from(floors.floor_db_per_channel[0]) - expected_left).abs() < 0.15,
        "left floor {:?} vs oracle {expected_left:.3}",
        floors.floor_db_per_channel[0]
    );
    assert!(
        (f64::from(floors.floor_db_per_channel[1]) - expected_right).abs() < 0.15,
        "right floor {:?} vs oracle {expected_right:.3}",
        floors.floor_db_per_channel[1]
    );
    assert!(
        floors.floor_db_per_channel[0] > floors.floor_db_per_channel[1] + 6.0,
        "channels must keep independent floors: {:?}",
        floors.floor_db_per_channel
    );
}

#[test]
fn capture_progress_tracks_accumulated_frames() {
    let mut plugin = time_domain_plugin(1);
    assert_eq!(plugin.capture_progress(), 0.0);
    start_capture(&mut plugin);
    let noise = lcg_noise(12_000, 0.05, 0x77);
    process_all(&mut plugin, &noise, 1, &[1000]);
    assert!((plugin.capture_progress() - 0.25).abs() < 1e-3);
    assert!(plugin.is_capturing());
    assert!(!plugin.has_captured_profile());
    process_all(&mut plugin, &noise, 1, &[1000]);
    assert!((plugin.capture_progress() - 0.5).abs() < 1e-3);
}

#[test]
fn capture_restart_and_cancel_semantics() {
    let mut plugin = time_domain_plugin(1);
    // Cancel discards the partial measurement and stores nothing.
    start_capture(&mut plugin);
    let noise = lcg_noise(1000, 0.05, 0x9);
    process_all(&mut plugin, &noise, 1, &[1000]);
    plugin
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(false))
        .unwrap();
    assert!(!plugin.is_capturing());
    assert_eq!(plugin.capture_progress(), 0.0);
    assert!(!plugin.has_captured_profile());

    // A full capture stores a profile.
    let full = lcg_noise(RATE as usize, 0.05, 0x11);
    start_capture(&mut plugin);
    process_all(&mut plugin, &full, 1, &[4096]);
    let before = plugin.persisted_params().captured_profile.unwrap();

    // Restarting then cancelling keeps the previous stored profile.
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[997]);
    assert!(plugin.is_capturing());
    plugin
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(false))
        .unwrap();
    let after = plugin.persisted_params().captured_profile.unwrap();
    assert_eq!(before, after);
}

#[test]
fn profile_persists_through_json_round_trip() {
    let mut plugin = time_domain_plugin(1);
    let noise = lcg_noise(RATE as usize, 0.05, 0x5eed);
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[2048]);
    plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("curve_low"), ParameterValue::Float(0.25))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("curve_mid"), ParameterValue::Float(0.5))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("curve_high"), ParameterValue::Float(0.75))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("link_mode"), ParameterValue::Int(1))
        .unwrap();

    let json = serde_json::to_string(&plugin.persisted_params()).unwrap();
    let restored: HissReducerPluginParams = serde_json::from_str(&json).unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(plugin.persisted_params()).unwrap()
    );
    let mut reloaded = HissReducerPlugin::from_params(1, restored);
    reloaded.initialize(RATE).unwrap();
    assert!(reloaded.has_captured_profile());
    assert_eq!(
        reloaded.persisted_params().captured_profile,
        plugin.persisted_params().captured_profile
    );
    assert_eq!(
        reloaded.get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(
        reloaded.get_parameter(&ParameterId::from("link_mode")),
        Some(ParameterValue::Int(1))
    );

    // Old five-field presets load with the new compatible defaults.
    let legacy: HissReducerPluginParams = serde_json::from_str(
        r#"{"enabled":true,"threshold_db":-30.0,"frequency_hz":4000.0,"strength":0.5,"spectral_mode":false}"#,
    )
    .unwrap();
    assert!(!legacy.use_captured_profile);
    assert_eq!((legacy.curve_low, legacy.curve_mid, legacy.curve_high), (1.0, 1.0, 1.0));
    assert_eq!(legacy.link_mode, 0);
    assert!(!legacy.transient_guard);
    assert!(legacy.captured_profile.is_none());
    let legacy_plugin = HissReducerPlugin::from_params(1, legacy);
    assert!(!legacy_plugin.has_captured_profile());
    assert_eq!(legacy_plugin.link_mode(), 0);
    assert!(legacy_plugin.reduction_curve().is_flat());
}

#[test]
fn malformed_profile_rejected_transactionally() {
    // Unsupported format version fails construction.
    let bad_version = serde_json::json!({
        "captured_profile": {
            "format_version": 99,
            "sample_rate": RATE,
            "channels": 1,
            "measurement_cutoff_hz": 4000.0,
            "floor_db_per_channel": [-40.0],
            "frames_analyzed": RATE,
        }
    });
    let params: HissReducerPluginParams = serde_json::from_value(bad_version).unwrap();
    let error = HissReducerPlugin::try_from_params(1, params)
        .err()
        .expect("unsupported profile version must fail construction");
    assert!(error.contains("version"), "unexpected error: {error}");

    // Inconsistent floor length fails construction.
    let bad_shape = serde_json::json!({
        "captured_profile": {
            "format_version": 1,
            "sample_rate": RATE,
            "channels": 2,
            "measurement_cutoff_hz": 4000.0,
            "floor_db_per_channel": [-40.0],
            "frames_analyzed": RATE,
        }
    });
    let params: HissReducerPluginParams = serde_json::from_value(bad_shape).unwrap();
    assert!(HissReducerPlugin::try_from_params(2, params).is_err());

    // Non-finite floors fail explicit restore and keep the old profile.
    let mut plugin = time_domain_plugin(1);
    let noise = lcg_noise(RATE as usize, 0.05, 0x31);
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[4096]);
    let before = plugin.persisted_params().captured_profile.unwrap();
    let corrupt = NoiseProfileData {
        format_version: 1,
        sample_rate: RATE,
        channels: 1,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: vec![f32::NAN],
        frames_analyzed: u64::from(RATE),
        spectral: None,
    };
    let error = plugin.restore_profile(&corrupt).unwrap_err();
    assert!(error.contains("range"), "unexpected error: {error}");
    assert_eq!(
        plugin.persisted_params().captured_profile.unwrap(),
        before,
        "failed restore must keep the accepted profile"
    );
}

#[test]
fn channel_mismatched_profile_handling() {
    let data = NoiseProfileData {
        format_version: 1,
        sample_rate: RATE,
        channels: 1,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: vec![-40.0],
        frames_analyzed: u64::from(RATE),
        spectral: None,
    };
    // Preset loading drops well-formed but inapplicable blobs.
    let plugin = HissReducerPlugin::from_params(
        2,
        HissReducerPluginParams {
            captured_profile: Some(data.clone()),
            ..HissReducerPluginParams::default()
        },
    );
    assert!(!plugin.has_captured_profile());

    // Explicit restore rejects the mismatch as an error.
    let mut mono = time_domain_plugin(1);
    let stereo_data = NoiseProfileData {
        channels: 2,
        floor_db_per_channel: vec![-40.0, -41.0],
        ..data
    };
    let error = mono.restore_profile(&stereo_data).unwrap_err();
    assert!(error.contains("channels"), "unexpected error: {error}");
    assert!(!mono.has_captured_profile());
}

#[test]
fn reset_preserves_profile_and_matches_fresh_restore() {
    let mut plugin = time_domain_plugin(1);
    let noise = lcg_noise(RATE as usize, 0.05, 0x71);
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[4096]);
    plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let exported = plugin.persisted_params().captured_profile.unwrap();

    let segment: Vec<f32> = (0..8192)
        .map(|i| {
            0.1 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / RATE as f32).sin()
                + 0.03 * ((i * 7919 % 1021) as f32 / 510.5 - 1.0)
        })
        .collect();

    plugin.reset();
    assert!(plugin.has_captured_profile(), "reset keeps the profile");
    assert!(!plugin.is_capturing());
    let after_reset = process_all(&mut plugin, &segment, 1, &[997, 64]);

    let mut fresh = time_domain_plugin(1);
    fresh.restore_profile(&exported).unwrap();
    fresh
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let restored = process_all(&mut fresh, &segment, 1, &[997, 64]);
    assert_eq!(after_reset, restored);

    // Reset also discards a partial capture while keeping the profile.
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise[..4096], 1, &[997]);
    assert!(plugin.is_capturing());
    plugin.reset();
    assert!(!plugin.is_capturing());
    assert!(plugin.has_captured_profile());
}

#[test]
fn capture_frozen_during_drain_and_learn_rejected_after_drain() {
    let mut plugin = spectral_plugin(1);
    let prefix = vec![0.125; 1000];
    process_all(&mut plugin, &prefix, 1, &[1000]);
    start_capture(&mut plugin);
    let partial = lcg_noise(5000, 0.05, 0xD4);
    process_all(&mut plugin, &partial, 1, &[1000]);
    let progress = plugin.capture_progress();
    assert!(progress > 0.0 && progress < 1.0);

    let _ = drain_all(&mut plugin, 1);
    assert!(
        (plugin.capture_progress() - progress).abs() < f32::EPSILON,
        "drain must not accumulate capture"
    );
    assert!(plugin.is_capturing());
    assert!(!plugin.has_captured_profile());

    let error = plugin
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap_err();
    assert!(error.contains("drain"), "unexpected error: {error}");

    plugin.reset();
    assert!(!plugin.is_capturing());
    assert_eq!(plugin.capture_progress(), 0.0);
}

#[test]
fn profile_threshold_modulation_suppresses_loud_hiss() {
    // Hiss 6+ dB louder than the default -30 dBFS threshold passes the
    // unprofiled reducer untouched; the captured profile must engage it.
    let hiss_only = lcg_noise(RATE as usize * 2, 0.14, 0x7155);
    let strong_params = || HissReducerPluginParams {
        strength: 0.8,
        ..HissReducerPluginParams::default()
    };
    let mut profiler = HissReducerPlugin::from_params(1, strong_params());
    profiler.initialize(RATE).unwrap();
    start_capture(&mut profiler);
    process_all(&mut profiler, &hiss_only[..RATE as usize], 1, &[4096]);
    let floor = profiler.overall_profile_floor_db().unwrap();
    assert!(
        floor > -30.0,
        "fixture hiss must clear the default threshold, got {floor:.2} dB"
    );

    let frames = RATE as usize * 2;
    let mut mixed = Vec::with_capacity(frames);
    for (index, &hiss) in hiss_only.iter().enumerate() {
        let tone =
            0.12 * (2.0 * std::f32::consts::PI * 750.0 * index as f32 / RATE as f32).sin();
        mixed.push(tone + hiss);
    }

    let mut plain = HissReducerPlugin::from_params(1, strong_params());
    plain.initialize(RATE).unwrap();
    let plain_output = process_all(&mut plain, &mixed, 1, &[4096]);
    profiler
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let profiled_output = process_all(&mut profiler, &mixed, 1, &[4096]);

    // Steady region: skip the first second (envelopes, persistence, hold).
    let skip = RATE as usize;
    let input_high = oracle_high_band_power(&mixed[skip..], 4000.0, f64::from(RATE));
    let plain_high = oracle_high_band_power(&plain_output[skip..], 4000.0, f64::from(RATE));
    let profiled_high =
        oracle_high_band_power(&profiled_output[skip..], 4000.0, f64::from(RATE));
    let plain_db = 10.0 * (plain_high / input_high).log10();
    let profiled_db = 10.0 * (profiled_high / input_high).log10();
    assert!(
        plain_db > -1.5,
        "unprofiled loud hiss must pass nearly untouched, got {plain_db:.2} dB"
    );
    assert!(
        profiled_db < -3.0,
        "profiled hiss must drop 3+ dB, got {profiled_db:.2} dB"
    );

    // The wanted low-band tone survives in both paths.
    let input_low = oracle_low_band_power(&mixed[skip..], 4000.0, f64::from(RATE));
    let plain_low = oracle_low_band_power(&plain_output[skip..], 4000.0, f64::from(RATE));
    let profiled_low =
        oracle_low_band_power(&profiled_output[skip..], 4000.0, f64::from(RATE));
    assert!(
        (10.0 * (plain_low / input_low).log10()).abs() < 0.5,
        "plain tone changed"
    );
    assert!(
        (10.0 * (profiled_low / input_low).log10()).abs() < 0.5,
        "profiled tone changed"
    );
}

#[test]
fn stereo_threshold_following_uses_loudest_channel_floor() {
    // The single backend threshold follows the maximum floor across
    // channels: on split material the quiet channel inherits the loud
    // channel's threshold. This pins that documented behavior, so a
    // future min/mean change must update the README contract as well.
    let stereo_params = || HissReducerPluginParams {
        threshold_db: -55.0,
        strength: 0.8,
        ..HissReducerPluginParams::default()
    };

    // Capture 1 s of split stereo noise: left 20 dB louder than right.
    let left = lcg_noise(RATE as usize, 0.05, 0x1e);
    let right = lcg_noise(RATE as usize, 0.005, 0xf7);
    let mut capture = Vec::with_capacity(RATE as usize * 2);
    for (&left_sample, &right_sample) in left.iter().zip(right.iter()) {
        capture.push(left_sample);
        capture.push(right_sample);
    }
    let mut profiler = HissReducerPlugin::from_params(2, stereo_params());
    profiler.initialize(RATE).unwrap();
    start_capture(&mut profiler);
    process_all(&mut profiler, &capture, 2, &[4096]);
    assert!(profiler.has_captured_profile());
    let floors = profiler
        .persisted_params()
        .captured_profile
        .expect("profile persists")
        .floor_db_per_channel;
    assert!(
        floors[0] > floors[1] + 10.0,
        "split capture must separate floors, got {floors:?}"
    );
    profiler
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();

    // Right-channel program far above its own floor but ~10 dB below the
    // inherited loud-channel threshold (~-26 dBFS): reduced under
    // max-following, preserved under min-following.
    let frames = RATE as usize * 2;
    let mut mixed = Vec::with_capacity(frames * 2);
    for index in 0..frames {
        let tone =
            0.022 * (2.0 * std::f32::consts::PI * 8000.0 * index as f32 / RATE as f32).sin();
        mixed.push(0.0);
        mixed.push(tone);
    }
    let profiled = process_all(&mut profiler, &mixed, 2, &[4096]);

    let right_in: Vec<f32> = mixed.iter().skip(1).step_by(2).copied().collect();
    let right_out: Vec<f32> = profiled.iter().skip(1).step_by(2).copied().collect();
    let skip = RATE as usize;
    let ratio = oracle_high_band_power(&right_out[skip..], 4000.0, f64::from(RATE))
        / oracle_high_band_power(&right_in[skip..], 4000.0, f64::from(RATE));
    let change_db = 10.0 * ratio.log10();
    assert!(
        change_db < -3.0,
        "quiet channel must inherit the loud threshold, got {change_db:.2} dB"
    );

    // Control: the same -55 dBFS user threshold without a profile leaves
    // the right program untouched, proving the reduction above comes from
    // the inherited floor rather than the user setting.
    let mut plain = HissReducerPlugin::from_params(2, stereo_params());
    plain.initialize(RATE).unwrap();
    let plain_out = process_all(&mut plain, &mixed, 2, &[4096]);
    let plain_right: Vec<f32> = plain_out.iter().skip(1).step_by(2).copied().collect();
    let plain_ratio = oracle_high_band_power(&plain_right[skip..], 4000.0, f64::from(RATE))
        / oracle_high_band_power(&right_in[skip..], 4000.0, f64::from(RATE));
    let plain_db = 10.0 * plain_ratio.log10();
    assert!(
        plain_db.abs() < 1.0,
        "unprofiled control must preserve the tone, got {plain_db:.2} dB"
    );
}

#[test]
fn use_profile_without_capture_is_armed_but_inert() {
    let input: Vec<f32> = (0..16_384)
        .map(|i| {
            0.1 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / RATE as f32).sin()
                + 0.02 * ((i * 7919 % 1021) as f32 / 510.5 - 1.0)
        })
        .collect();
    for spectral in [false, true] {
        let mut armed = HissReducerPlugin::from_params(
            1,
            HissReducerPluginParams {
                spectral_mode: spectral,
                use_captured_profile: true,
                ..HissReducerPluginParams::default()
            },
        );
        armed.initialize(RATE).unwrap();
        let mut plain = HissReducerPlugin::from_params(
            1,
            HissReducerPluginParams {
                spectral_mode: spectral,
                ..HissReducerPluginParams::default()
            },
        );
        plain.initialize(RATE).unwrap();
        let armed_output = process_all(&mut armed, &input, 1, &[1, 64, 511, 997]);
        let plain_output = process_all(&mut plain, &input, 1, &[1, 64, 511, 997]);
        assert_eq!(
            armed_output, plain_output,
            "spectral={spectral}: use flag without a profile must be inert"
        );
    }
}

#[test]
fn capture_completion_transition_is_click_free() {
    // A steady 8 kHz tone sits above the default threshold but below the
    // profile-followed one, so completion engages reduction smoothly.
    let tone: Vec<f32> = (0..RATE as usize + 8192)
        .map(|i| 0.09 * (2.0 * std::f32::consts::PI * 8000.0 * i as f32 / RATE as f32).sin())
        .collect();
    let mut plugin = time_domain_plugin(1);
    plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    start_capture(&mut plugin);
    let output = process_all(&mut plugin, &tone, 1, &[4096]);
    assert!(plugin.has_captured_profile());

    // The tone's own maximum sample step is A*2*pi*f/sr = 0.0942; any gain
    // click at the completion frame (48000) would exceed 0.11.
    let mut maximum_step = 0.0f32;
    for pair in output[RATE as usize - 256..RATE as usize + 256].windows(2) {
        maximum_step = maximum_step.max((pair[1] - pair[0]).abs());
    }
    assert!(
        maximum_step < 0.11,
        "completion click: maximum step {maximum_step}"
    );

    // Reduction actually engaged after completion. The after-window sits
    // fully past the 30 ms persistence flip (+1440 frames) plus attack
    // slew, where the settled gain is ~0.625 in power ~0.39, so the
    // pre-declared 0.8 bound holds with large margin. The windows have
    // different lengths (2400 vs 4096 samples), so the comparison uses
    // mean-square power: raw sums would conflate the 4096/2400 length
    // ratio with gain.
    let before_window = &output[RATE as usize - 4800..RATE as usize - 2400];
    let after_window = &output[RATE as usize + 2048..RATE as usize + 6144];
    let before_sum: f32 = before_window.iter().map(|s| s * s).sum();
    let after_sum: f32 = after_window.iter().map(|s| s * s).sum();
    let before = before_sum / before_window.len() as f32;
    let after = after_sum / after_window.len() as f32;
    // Measurement-chain regression: the pre-completion window holds the
    // unreduced tone, so its mean power must equal the theoretical tone
    // power 0.09^2 / 2 = 0.00405 (2% covers f32 summation). This pins the
    // oracle itself, so a future window edit cannot silently reintroduce
    // an unnormalized comparison.
    assert!(
        (before - 0.00405).abs() < 0.00405 * 0.02,
        "oracle check failed: before-mean power {before}, expected 0.00405"
    );
    assert!(
        after < before * 0.8,
        "reduction did not engage: before={before} after={after}"
    );
}

#[test]
fn triggers_never_fire_from_batch_updates() {
    use sotf_host::parametric_plugin::ParameterSet;

    let mut plugin = time_domain_plugin(1);
    let mut values = ParameterSet::new();
    values.insert(ParameterId::from("learn_noise"), ParameterValue::Bool(true));
    plugin.apply_values(values).unwrap();
    assert!(
        !plugin.is_capturing(),
        "batch learn_noise must never fire capture"
    );

    let noise = lcg_noise(RATE as usize, 0.05, 0xc1);
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[4096]);
    assert!(plugin.has_captured_profile());
    let mut values = ParameterSet::new();
    values.insert(
        ParameterId::from("clear_profile"),
        ParameterValue::Bool(true),
    );
    plugin.apply_values(values).unwrap();
    assert!(
        plugin.has_captured_profile(),
        "batch clear_profile must be a no-op"
    );

    // Named setters fire; clear reads back false and learn reads active.
    plugin
        .set_parameter(ParameterId::from("clear_profile"), ParameterValue::Bool(true))
        .unwrap();
    assert!(!plugin.has_captured_profile());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("clear_profile")),
        Some(ParameterValue::Bool(false))
    );
    start_capture(&mut plugin);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(plugin.current_values().len(), 13);
}

#[test]
fn realtime_capture_paths_do_not_allocate() {
    let mut plugin = time_domain_plugin(1);
    // Warm up once so lazy initialization cannot pollute the measurement.
    let warm = vec![0.01; 4096];
    process_all(&mut plugin, &warm, 1, &[4096]);

    // Identifiers are built outside: Arc construction allocates, cloning is
    // a refcount bump (mirrors the existing realtime parameter test).
    let learn_id = ParameterId::from("learn_noise");
    let use_id = ParameterId::from("use_captured_profile");
    assert_no_allocs("hiss learn trigger", || {
        plugin
            .parametric_set_parameter(learn_id.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
    let mut block = lcg_noise(RATE as usize, 0.05, 0xa110);
    assert_no_allocs("hiss capture process incl. completion", || {
        plugin
            .process_in_place(&mut block, &ctx(RATE as usize))
            .unwrap();
    });
    assert!(plugin.has_captured_profile());
    assert_no_allocs("hiss use-profile toggle", || {
        plugin
            .parametric_set_parameter(use_id.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
    let data = plugin.persisted_params().captured_profile.unwrap();
    assert_no_allocs("hiss profile restore", || {
        plugin.restore_profile(&data).unwrap();
    });
    assert_no_allocs("hiss profile clear", || {
        plugin.clear_captured_profile();
    });
    assert_no_allocs("hiss reset with profile", || {
        plugin.reset();
    });
}

#[test]
fn spectral_eof_with_profile_matches_derived_endpoint() {
    fn end(frames: usize) -> usize {
        2048 + ((frames - 1) / 256) * 256
    }
    // Unity (strength 0) isolates EOF accounting under profile state.
    for use_profile in [false, true] {
        for phase in [0, 1, 127, 255] {
            let mut plugin = HissReducerPlugin::from_params(
                1,
                HissReducerPluginParams {
                    spectral_mode: true,
                    strength: 0.0,
                    use_captured_profile: use_profile,
                    ..HissReducerPluginParams::default()
                },
            );
            plugin.initialize(RATE).unwrap();
            let noise = lcg_noise(RATE as usize, 0.05, 0xe0f);
            start_capture(&mut plugin);
            process_all(&mut plugin, &noise, 1, &[4096]);
            assert!(plugin.has_captured_profile());
            // Reset clears the capture pass from the phase and delay-line
            // state while preserving the stored profile, the use flag, and
            // settings, so the render below starts from a clean stream
            // with profile state armed. This also exercises spectral
            // reset-with-profile.
            plugin.reset();
            assert!(plugin.has_captured_profile());

            let marker_frames = phase + 1;
            let mut marker = vec![0.0; marker_frames];
            marker[0] = 0.25;
            marker[marker_frames - 1] -= 0.5;
            let mut output = process_all(&mut plugin, &noise, 1, &[4096]);
            output.extend(process_all(&mut plugin, &marker, 1, &[7, 137, 1]));
            output.extend(drain_all(&mut plugin, 1));
            let total = RATE as usize + marker_frames;
            assert_eq!(output.len(), end(total));
            let mut input = noise.clone();
            input.extend_from_slice(&marker);
            for (index, &actual) in output.iter().enumerate() {
                let expected = index
                    .checked_sub(1024)
                    .and_then(|i| input.get(i))
                    .copied()
                    .unwrap_or(0.0);
                assert!(
                    (actual - expected).abs() < 2e-6,
                    "use={use_profile} phase={phase} i={index}: {actual} vs {expected}"
                );
            }
        }
    }
}
