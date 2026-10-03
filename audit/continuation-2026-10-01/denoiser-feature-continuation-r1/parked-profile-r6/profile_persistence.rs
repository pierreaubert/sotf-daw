//! Versioned captured-profile carrier: validation, import/export,
//! construction tiers, and failed-restore history retention.
//!
//! The carrier persists learned noise floors as live-unit per-bin powers.
//! Corrupt blobs fail transactionally; geometry/channel mismatches drop
//! at construction and reject on explicit import; rate mismatches drop
//! at `initialize`. Failed imports write nothing, so populated history
//! survives them bit-exactly.

// Rust guideline compliant 2026-02-21

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
struct TrackingAllocator;
fn record() {
    let _ = TRACKING.try_with(|tracking| {
        if tracking.get() {
            let _ = OPERATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}
// SAFETY: all memory operations delegate unchanged to System. The thread-local
// counters neither allocate nor inspect the allocated memory.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_denoiser::profile::{
    DENOISER_PROFILE_FORMAT_VERSION, DENOISER_PROFILE_WINDOW, NoiseProfileData,
};
use sotf_plugin_denoiser::{DenoiserData, DenoiserPlugin, DenoiserPluginParams};

const RATE: u32 = 48_000;

#[derive(Clone)]
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as f32 / 2_147_483_648.0 * 2.0 - 1.0
    }
}

fn white_noise(frames: usize, channels: usize, seed: u64, amplitude: f32) -> Vec<f32> {
    let mut rng = Lcg(seed);
    (0..frames * channels)
        .map(|_| rng.next() * amplitude)
        .collect()
}

fn process_all(
    plugin: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    rate: u32,
) -> Vec<f32> {
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut pos = 0;
    while pos < frames {
        let n = 1024.min(frames - pos);
        plugin
            .process_in_place(
                &mut output[pos * channels..(pos + n) * channels],
                &ProcessContext::new(rate, n),
            )
            .unwrap();
        pos += n;
    }
    output
}

fn plugin_data(plugin: &DenoiserPlugin) -> DenoiserData {
    (*plugin
        .get_data()
        .unwrap()
        .downcast::<DenoiserData>()
        .unwrap())
    .clone()
}

/// A valid carrier: mono, 2048/1024/sqrt-hann/48 kHz, flat 1e-6 powers.
fn valid_carrier() -> NoiseProfileData {
    NoiseProfileData {
        format_version: DENOISER_PROFILE_FORMAT_VERSION,
        fft_size: 2048,
        hop_size: 1024,
        window: DENOISER_PROFILE_WINDOW.to_string(),
        sample_rate: RATE,
        channels: 1,
        num_bins: 1025,
        power_per_channel_bin: vec![1e-6; 1025],
        hops_analyzed: 47,
    }
}

fn learn_profile(channels: usize, rate: u32, seed: u64) -> (DenoiserPlugin, NoiseProfileData) {
    let mut plugin = DenoiserPlugin::from_params(channels, DenoiserPluginParams::default());
    plugin.initialize(rate).unwrap();
    plugin
        .parametric_set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    // Two seconds of noise-only input (capture needs ~1 s at any rate).
    let noise = white_noise(2 * rate as usize, channels, seed, 0.05);
    let _ = process_all(&mut plugin, &noise, channels, rate);
    let data = plugin_data(&plugin);
    assert!(data.has_captured_profile, "capture must complete");
    assert_eq!(data.profile_generation, 1);
    let carrier = plugin.export_captured_profile().expect("export must exist");
    assert_eq!(carrier.hops_analyzed, (rate as usize / (2048 / 2)) as u64);
    (plugin, carrier)
}

#[test]
fn carrier_validation_matrix() {
    valid_carrier().validate().unwrap();
    for (mutate, fragment) in [
        ("version", "format version"),
        ("fft", "FFT size"),
        ("hop", "hop size"),
        ("window", "window"),
        ("rate", "sample rate"),
        ("channels", "channels"),
        ("bins", "bins"),
        ("hops", "hops"),
        ("length", "powers"),
        ("nan", "finite"),
        ("negative", "nonnegative"),
    ] {
        let mut bad = valid_carrier();
        match mutate {
            "version" => bad.format_version = 99,
            "fft" => bad.fft_size = 1024,
            "hop" => bad.hop_size = 512,
            "window" => bad.window = "hann-periodic".to_string(),
            "rate" => bad.sample_rate = 0,
            "channels" => bad.channels = 0,
            "bins" => bad.num_bins = 513,
            "hops" => bad.hops_analyzed = 0,
            "length" => bad.power_per_channel_bin.pop(),
            "nan" => bad.power_per_channel_bin[0] = f32::NAN,
            _ => bad.power_per_channel_bin[500] = -1.0,
        }
        let error = bad.validate().err().unwrap_or_else(|| {
            panic!("{mutate} corruption must fail validation")
        });
        assert!(
            error.contains(fragment),
            "{mutate}: error must name the cause, got: {error}"
        );
    }
    // Oversized channel count is bounded even before the length check.
    let mut wide = valid_carrier();
    wide.channels = 65;
    assert!(wide.validate().is_err());
    // Unknown JSON keys are rejected (no silent schema drift).
    let mut json = serde_json::to_value(valid_carrier()).unwrap();
    json.as_object_mut().unwrap().insert(
        "bogus_future_key".to_string(),
        serde_json::json!(1),
    );
    assert!(serde_json::from_value::<NoiseProfileData>(json).is_err());
}

#[test]
fn carrier_agreement_matrix() {
    let carrier = valid_carrier();
    carrier.validate_against(2048, 1024, Some(RATE), 1).unwrap();
    // Pre-initialize construction skips the rate check (None).
    carrier.validate_against(2048, 1024, None, 1).unwrap();
    for (fft, hop, rate, channels, fragment) in [
        (512, 256, Some(RATE), 1, "FFT size"),
        (2048, 1024, Some(44_100), 1, "rate"),
        (2048, 1024, Some(RATE), 2, "channels"),
    ] {
        let error = carrier
            .validate_against(fft, hop, rate, channels)
            .err()
            .unwrap_or_else(|| panic!("{fragment} mismatch must fail agreement"));
        assert!(
            error.contains(fragment),
            "error must name the cause, got: {error}"
        );
    }
}

#[test]
fn learn_export_import_roundtrip_engages_dsp() {
    let (_learner, carrier) = learn_profile(2, RATE, 0xbeef);
    assert_eq!(carrier.channels, 2);
    assert_eq!(carrier.power_per_channel_bin.len(), 2 * 1025);
    // JSON file roundtrip preserves the carrier exactly.
    let encoded = serde_json::to_string(&carrier).unwrap();
    let decoded: NoiseProfileData = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, carrier);
    // Import into a fresh twin engages the identical floor.
    let mut importer = DenoiserPlugin::from_params(2, DenoiserPluginParams::default());
    importer.initialize(RATE).unwrap();
    assert!(importer.export_captured_profile().is_none());
    importer.import_captured_profile(&decoded).unwrap();
    assert_eq!(importer.export_captured_profile().as_ref(), Some(&carrier));
    // Explicit import never engages by itself.
    assert_eq!(
        importer.parametric_get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(false))
    );
    importer
        .parametric_set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mixture = white_noise(16_384, 2, 0xbeef, 0.05);
    let profiled = process_all(&mut importer, &mixture, 2, RATE);
    let mut unprofiled = DenoiserPlugin::from_params(2, DenoiserPluginParams::default());
    unprofiled.initialize(RATE).unwrap();
    let plain = process_all(&mut unprofiled, &mixture, 2, RATE);
    assert_ne!(profiled, plain, "imported profile must engage DSP");
    assert!(profiled.iter().all(|s| s.is_finite()));
    assert!(profiled.iter().any(|s| s.abs() > 1e-6));
    // Monitor publication carries the import generation.
    let _ = process_all(&mut importer, &white_noise(8192, 2, 0x1, 0.05), 2, RATE);
    assert_eq!(plugin_data(&importer).profile_generation, 1);
}

#[test]
fn construction_carrier_tiers() {
    // (a) Valid carrier + use flag: adopted and engaged from the start.
    let mut carrier = valid_carrier();
    for power in carrier.power_per_channel_bin.iter_mut() {
        *power = 1e-3;
    }
    let mut plugin = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            use_captured_profile: true,
            captured_profile: Some(carrier.clone()),
            ..Default::default()
        },
    );
    plugin.initialize(RATE).unwrap();
    assert_eq!(plugin.export_captured_profile().as_ref(), Some(&carrier));
    let mixture = white_noise(16_384, 1, 0xca, 0.05);
    let profiled = process_all(&mut plugin, &mixture, 1, RATE);
    let mut plain_plugin = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    plain_plugin.initialize(RATE).unwrap();
    let plain = process_all(&mut plain_plugin, &mixture, 1, RATE);
    assert_ne!(profiled, plain);
    // (b) Corrupt blob fails construction transactionally.
    let mut corrupt = valid_carrier();
    corrupt.format_version = 99;
    let error = DenoiserPlugin::try_from_params(
        1,
        DenoiserPluginParams {
            captured_profile: Some(corrupt),
            ..Default::default()
        },
    )
    .err()
    .expect("corrupt carrier must fail construction");
    assert!(error.contains("captured_profile"), "got: {error}");
    // (c) Geometry/channel mismatches are dropped, not fatal.
    let mut wide = valid_carrier();
    wide.channels = 2;
    wide.power_per_channel_bin = vec![1e-6; 2 * 1025];
    let mut dropped = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            use_captured_profile: true,
            captured_profile: Some(wide),
            ..Default::default()
        },
    );
    dropped.initialize(RATE).unwrap();
    assert!(dropped.export_captured_profile().is_none());
    let mut small_fft = valid_carrier();
    small_fft.fft_size = 512;
    small_fft.hop_size = 256;
    small_fft.num_bins = 257;
    small_fft.power_per_channel_bin = vec![1e-6; 257];
    let mut dropped_fft = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            captured_profile: Some(small_fft),
            ..Default::default()
        },
    );
    dropped_fft.initialize(RATE).unwrap();
    assert!(dropped_fft.export_captured_profile().is_none());
    // (d) Rate defers to initialize: match keeps, mismatch drops.
    let mut off_rate = valid_carrier();
    off_rate.sample_rate = 44_100;
    let mut kept = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            use_captured_profile: true,
            captured_profile: Some(off_rate.clone()),
            ..Default::default()
        },
    );
    kept.initialize(44_100).unwrap();
    assert!(kept.export_captured_profile().is_some());
    let mut stale = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            use_captured_profile: true,
            captured_profile: Some(off_rate),
            ..Default::default()
        },
    );
    stale.initialize(RATE).unwrap();
    assert!(stale.export_captured_profile().is_none());
    assert_eq!(
        stale.parametric_get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(false)),
        "dropped use flag must read back false"
    );
}

#[test]
fn failed_import_retains_populated_history() {
    let (mut live, carrier) = learn_profile(1, RATE, 0x71);
    let mixture = white_noise(32_768, 1, 0xbeef, 0.05);
    let prefix = process_all(&mut live, &mixture[..16_384], 1, RATE);
    // Uninterrupted twin: identical learn, identical prefix.
    let (mut twin, _) = learn_profile(1, RATE, 0x71);
    let twin_prefix = process_all(&mut twin, &mixture[..16_384], 1, RATE);
    assert_eq!(prefix, twin_prefix);
    // Every corruption and incompatibility class fails loudly.
    let mut bad_version = carrier.clone();
    bad_version.format_version = 0;
    let mut bad_power = carrier.clone();
    bad_power.power_per_channel_bin[7] = f32::NEG_INFINITY;
    let mut bad_len = carrier.clone();
    bad_len.power_per_channel_bin.push(1e-6);
    let mut bad_rate = carrier.clone();
    bad_rate.sample_rate = 96_000;
    let mut bad_channels = carrier.clone();
    bad_channels.channels = 2;
    bad_channels.power_per_channel_bin = vec![1e-6; 2 * 1025];
    let mut bad_fft = carrier.clone();
    bad_fft.fft_size = 512;
    bad_fft.hop_size = 256;
    bad_fft.num_bins = 257;
    bad_fft.power_per_channel_bin = vec![1e-6; 257];
    for (bad, fragment) in [
        (bad_version, "format version"),
        (bad_power, "finite"),
        (bad_len, "powers"),
        (bad_rate, "rate"),
        (bad_channels, "channels"),
        (bad_fft, "FFT size"),
    ] {
        let error = live
            .import_captured_profile(&bad)
            .err()
            .unwrap_or_else(|| panic!("{fragment} import must fail"));
        assert!(error.contains(fragment), "got: {error}");
    }
    // Storage, flags, rate, and generation are exactly untouched.
    assert_eq!(live.export_captured_profile().as_ref(), Some(&carrier));
    let _ = process_all(&mut live, &white_noise(8192, 1, 0x1, 0.05), 1, RATE);
    let _ = process_all(&mut twin, &white_noise(8192, 1, 0x1, 0.05), 1, RATE);
    let data = plugin_data(&live);
    assert!(data.has_captured_profile && data.using_captured_profile);
    assert_eq!(data.profile_generation, 1);
    // Continued processing matches the uninterrupted twin bit-exactly.
    let live_rest = process_all(&mut live, &mixture[16_384..], 1, RATE);
    let twin_rest = process_all(&mut twin, &mixture[16_384..], 1, RATE);
    assert_eq!(live_rest, twin_rest);
}

#[test]
fn clear_bumps_generation_and_falls_back() {
    let (mut plugin, carrier) = learn_profile(1, RATE, 0xca);
    plugin
        .parametric_set_parameter(
            ParameterId::from("clear_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let _ = process_all(&mut plugin, &white_noise(8192, 1, 0x1, 0.05), 1, RATE);
    let data = plugin_data(&plugin);
    assert!(!data.has_captured_profile && !data.using_captured_profile);
    assert_eq!(data.profile_generation, 2);
    // Cleared audio equals a fresh unprofiled twin bit-exactly.
    let mixture = white_noise(16_384, 1, 0xbeef, 0.05);
    let cleared = process_all(&mut plugin, &mixture, 1, RATE);
    let mut fresh = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    fresh.initialize(RATE).unwrap();
    let plain = process_all(&mut fresh, &mixture, 1, RATE);
    assert_eq!(cleared, plain);
    // Re-import engages again at a new generation.
    plugin.import_captured_profile(&carrier).unwrap();
    plugin
        .parametric_set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let _ = process_all(&mut plugin, &white_noise(8192, 1, 0x1, 0.05), 1, RATE);
    assert_eq!(plugin_data(&plugin).profile_generation, 3);
    let mut fresh2 = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    fresh2.initialize(RATE).unwrap();
    plugin.reset();
    let reengaged = process_all(&mut plugin, &mixture, 1, RATE);
    let plain2 = process_all(&mut fresh2, &mixture, 1, RATE);
    assert_ne!(reengaged, plain2);
}

#[test]
fn reset_preserves_imported_profile() {
    let mut plugin = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    plugin.initialize(RATE).unwrap();
    plugin.import_captured_profile(&valid_carrier()).unwrap();
    plugin.reset();
    assert_eq!(
        plugin.export_captured_profile().as_ref(),
        Some(&valid_carrier())
    );
}

#[test]
fn profile_use_renders_without_callback_allocation() {
    let mut plugin = DenoiserPlugin::from_params(
        2,
        DenoiserPluginParams {
            use_captured_profile: true,
            captured_profile: Some({
                let mut carrier = valid_carrier();
                carrier.channels = 2;
                carrier.power_per_channel_bin = vec![1e-6; 2 * 1025];
                carrier
            }),
            ..Default::default()
        },
    );
    plugin.initialize(RATE).unwrap();
    assert!(plugin.export_captured_profile().is_some());
    std::thread::spawn(move || {
        let mut audio = vec![0.0; 4096 * 2];
        for (i, sample) in audio.iter_mut().enumerate() {
            *sample = (i as f32 * 0.131).sin() * 0.1;
        }
        let mut tail = vec![0.0; 1024 * 2];
        OPERATIONS.set(0);
        TRACKING.set(true);
        // Cold process with the profile engaged, drain, reset, reuse.
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(RATE, 4096))
            .unwrap();
        let mut drained = 0;
        for _ in 0..16 {
            let status = plugin
                .drain(&mut tail, &ProcessContext::new(RATE, 1024))
                .unwrap();
            drained += status.frames;
            if status.complete {
                break;
            }
        }
        assert!(drained > 0);
        plugin.reset();
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(RATE, 4096))
            .unwrap();
        TRACKING.set(false);
        assert_eq!(OPERATIONS.get(), 0, "profile-use heap operations");
    })
    .join()
    .unwrap();
}
