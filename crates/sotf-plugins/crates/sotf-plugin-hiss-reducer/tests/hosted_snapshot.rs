//! Hosted profile snapshot export tests.
//!
//! Bounds are fixed here before any candidate runs: hosted capture through
//! the object-safe adapter publishes a generation-consistent snapshot;
//! exports are bit-exact against the concrete store and an independent
//! direct-DFT oracle (2% per bin); colored captures prove nonflat spectra
//! (low/high regional ratio below 0.5x); reconstruction renders and drains
//! bit-exactly; malformed restores keep generation, profile, and audio;
//! concurrent readers observe only coherent generations or Busy; and the
//! realtime query/publication paths show zero allocations and frees.

// Rust guideline compliant 2026-10-21
use sotf_host::CountingAlloc;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::{
    ParametricInPlacePlugin, ParametricInPlacePluginAdapter,
};
use sotf_host::plugin::{InPlacePlugin, ProcessContext, TailLength};
use sotf_host::test_utils::measure_heap_activity;
use sotf_plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugin_hiss_reducer::snapshot::{ProfileFallback, ProfileSnapshot};
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

type HostedHiss = ParametricInPlacePluginAdapter<HissReducerPlugin>;

const RATE: u32 = 48_000;
const NUM_BINS: usize = 513;
const PARTITIONS: [usize; 5] = [1, 64, 511, 73, 997];

fn ctx(frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(RATE, frames)
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

/// First-difference high-passed hiss, bit-identical to legacy fixtures.
fn first_difference_hiss(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let white = lcg_noise(frames, 1.0, seed);
    let mut previous = 0.0f32;
    white
        .iter()
        .map(|&sample| {
            let high_pass = amplitude * (sample - previous);
            previous = sample;
            high_pass
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

fn hosted_spectral() -> HostedHiss {
    let plugin = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            strength: 0.85,
            ..HissReducerPluginParams::default()
        },
    );
    let mut hosted = HostedHiss::new(plugin);
    hosted.initialize(RATE).unwrap();
    hosted
}

fn hosted_process(hosted: &mut HostedHiss, input: &[f32], blocks: &[usize]) -> Vec<f32> {
    let mut output = input.to_vec();
    let frames = input.len();
    let mut pos = 0;
    let mut call = 0;
    while pos < frames {
        let count = blocks[call % blocks.len()].min(frames - pos);
        hosted
            .process_in_place(&mut output[pos..pos + count], &ctx(count))
            .unwrap();
        pos += count;
        call += 1;
    }
    output
}

fn hosted_drain(hosted: &mut HostedHiss) -> Vec<f32> {
    let mut output = Vec::new();
    for _ in 0..4096 {
        let mut block = vec![0.0; 256];
        let status = hosted.drain(&mut block, &ctx(256)).unwrap();
        output.extend_from_slice(&block[..status.frames]);
        if status.complete {
            return output;
        }
    }
    panic!("drain did not complete");
}

fn hosted_learn(hosted: &mut HostedHiss, fire: bool) {
    hosted
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(fire))
        .unwrap();
}

fn hosted_snapshot(hosted: &HostedHiss) -> std::sync::Arc<ProfileSnapshot> {
    hosted
        .get_data()
        .expect("hosted Hiss must publish snapshot data")
        .downcast::<ProfileSnapshot>()
        .expect("snapshot must downcast to ProfileSnapshot")
}

fn regional_mean(spectrum: &[f32], lo_bin: usize, hi_bin: usize) -> f64 {
    let slice = &spectrum[lo_bin..=hi_bin];
    slice.iter().map(|p| f64::from(*p)).sum::<f64>() / slice.len() as f64
}

#[test]
fn hosted_capture_export_reconstruct_bitexact() {
    let mut hosted = hosted_spectral();
    // get_data is Some from construction, before any capture or init work
    // beyond hosting, so hosts cache availability once.
    assert!(hosted.get_data().is_some());
    let pre = hosted_snapshot(&hosted).try_export().unwrap();
    assert!(pre.is_none(), "fresh plugin exports no profile");

    // Capture 1 s of colored hiss through the public named control.
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xe940c);
    hosted_learn(&mut hosted, true);
    hosted_process(&mut hosted, &colored, &[4096]);
    let snapshot = hosted_snapshot(&hosted);
    let (active, progress) = snapshot.capture_state();
    assert!(!active && progress == 0.0);
    let export = snapshot.try_export().unwrap().expect("capture must export");
    assert_eq!(export.profile.format_version, 2);
    assert_eq!(export.profile.channels, 1);
    assert_eq!(export.profile.sample_rate, RATE);
    let spectral = export.profile.spectral.as_ref().expect("v2 spectrum");
    assert_eq!(spectral.power_per_channel_bin.len(), NUM_BINS);
    assert_eq!(spectral.hops_analyzed, 184);

    // Nonflat colored proof: rising first-difference hiss concentrates
    // power up high (low/high regional means far below unity).
    let low = regional_mean(&spectral.power_per_channel_bin, 86, 149);
    let high = regional_mean(&spectral.power_per_channel_bin, 341, 490);
    assert!(
        low / high < 0.5,
        "colored capture must be nonflat, ratio {:.3}",
        low / high
    );

    // Render and drain a nonzero mix through the hosted object, then
    // compare against the concrete store and a reconstruction. Reset first
    // so DSP state matches the fresh reconstruction (the profile and its
    // snapshot generation survive reset by contract).
    hosted
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    hosted.reset();
    let hiss = lcg_noise(16384, 0.04, 0xe9f1);
    let tone = sine_tone(16384, 0.06, 9984.375, RATE);
    let mix: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let mut hosted_out = hosted_process(&mut hosted, &mix, &[4096]);
    hosted_out.extend(hosted_drain(&mut hosted));

    // Same data through the concrete export path, bit-exact.
    let inner = hosted.into_inner();
    assert_eq!(
        inner.persisted_params().captured_profile.as_ref().unwrap(),
        &export.profile,
        "hosted export must equal the concrete store bit-exactly"
    );

    // Reconstruct from the export; nonzero render and drain match bit-exactly.
    let mut rebuilt = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            strength: 0.85,
            use_captured_profile: true,
            ..HissReducerPluginParams::default()
        },
    );
    rebuilt.initialize(RATE).unwrap();
    rebuilt.restore_profile(&export.profile).unwrap();
    let mut rebuilt_out = mix.clone();
    let mut pos = 0;
    while pos < rebuilt_out.len() {
        let count = 4096.min(rebuilt_out.len() - pos);
        rebuilt
            .process_in_place(&mut rebuilt_out[pos..pos + count], &ctx(count))
            .unwrap();
        pos += count;
    }
    for _ in 0..4096 {
        let mut block = vec![0.0; 256];
        let status = rebuilt.drain(&mut block, &ctx(256)).unwrap();
        rebuilt_out.extend_from_slice(&block[..status.frames]);
        if status.complete {
            break;
        }
    }
    assert_eq!(hosted_out, rebuilt_out);
    assert_ne!(
        hosted_out[1024..16384],
        mix[0..15360],
        "engaged reconstruction must process hosted audio, not pass through"
    );
}

#[test]
fn snapshot_v1_v2_payload_roundtrip_without_action_replay() {
    let mut hosted = hosted_spectral();
    // v1 floors-only blob exports exactly, with floor fallback engaged.
    let v1 = NoiseProfileData {
        format_version: 1,
        sample_rate: RATE,
        channels: 1,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: vec![-40.0],
        frames_analyzed: u64::from(RATE),
        spectral: None,
    };
    let mut inner = hosted.into_inner();
    inner.restore_profile(&v1).unwrap();
    inner
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    hosted = HostedHiss::new(inner);
    hosted.initialize(RATE).unwrap();
    let snapshot = hosted_snapshot(&hosted);
    let export = snapshot.try_export().unwrap().expect("v1 must export");
    assert_eq!(export.profile, v1);
    assert!(!export.engaged);
    let status = snapshot.try_status().unwrap();
    assert_eq!(status.fallback, ProfileFallback::Floor);

    // v2 JSON round-trips through construction without replaying actions.
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xa07c);
    let mut profiler = hosted_spectral();
    hosted_learn(&mut profiler, true);
    hosted_process(&mut profiler, &colored, &PARTITIONS);
    let v2 = hosted_snapshot(&profiler)
        .try_export()
        .unwrap()
        .expect("v2 must export")
        .profile;
    assert_eq!(v2.format_version, 2);
    let json = serde_json::to_string(&v2).unwrap();
    assert!(
        !json.contains("learn_noise") && !json.contains("clear_profile"),
        "exported blob must carry no momentary actions"
    );
    let reloaded: NoiseProfileData = serde_json::from_str(&json).unwrap();
    assert_eq!(reloaded, v2);
    let mut rebuilt = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            use_captured_profile: true,
            ..HissReducerPluginParams::default()
        },
    );
    rebuilt.initialize(RATE).unwrap();
    rebuilt.restore_profile(&reloaded).unwrap();
    assert!(!rebuilt.is_capturing(), "restore must not start capture");
    assert!(rebuilt.has_captured_profile());
}

#[test]
fn snapshot_lifecycle_states_and_progress() {
    let mut hosted = hosted_spectral();
    let snapshot = hosted_snapshot(&hosted);
    // Uncaptured: consistent absence with a stable generation.
    let gen_empty = snapshot.try_status().unwrap().generation;
    assert!(snapshot.try_export().unwrap().is_none());
    let status = snapshot.try_status().unwrap();
    assert!(!status.present && !status.spectral && !status.engaged);
    assert_eq!(status.fallback, ProfileFallback::Disabled);
    assert_eq!(snapshot.capture_state(), (false, 0.0));

    // Capturing: progress advances, export stays absent, generation holds.
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xca97u32);
    hosted_learn(&mut hosted, true);
    assert_eq!(snapshot.capture_state(), (true, 0.0));
    let partial: Vec<f32> = colored[..12288].to_vec();
    hosted_process(&mut hosted, &partial, &[4096]);
    let (active, progress) = snapshot.capture_state();
    assert!(active);
    assert_eq!(progress, 12288f32 / 48000f32);
    assert!(snapshot.try_export().unwrap().is_none());
    assert_eq!(snapshot.try_status().unwrap().generation, gen_empty);

    // Cancel keeps absence and the generation; restart completes.
    hosted_learn(&mut hosted, false);
    assert_eq!(snapshot.capture_state(), (false, 0.0));
    assert_eq!(snapshot.try_status().unwrap().generation, gen_empty);
    hosted_learn(&mut hosted, true);
    hosted_process(&mut hosted, &colored, &PARTITIONS);
    assert_eq!(snapshot.capture_state(), (false, 0.0));
    let done = snapshot.try_export().unwrap().expect("capture must export");
    assert!(done.generation > gen_empty);
    assert_eq!(done.profile.format_version, 2);
    assert_eq!(done.profile.frames_analyzed, u64::from(RATE));
    let done_status = snapshot.try_status().unwrap();
    assert_eq!(done_status.generation, done.generation);
    assert!(done_status.present && done_status.spectral);

    // Callback partitions never change the published payload.
    let mut other = hosted_spectral();
    hosted_learn(&mut other, true);
    hosted_process(&mut other, &colored, &[RATE as usize]);
    let other_export = hosted_snapshot(&other).try_export().unwrap().unwrap();
    assert_eq!(other_export.profile, done.profile);

    // Clear returns to consistent absence with a fresh generation.
    hosted
        .set_parameter(
            ParameterId::from("clear_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert!(snapshot.try_export().unwrap().is_none());
    assert!(snapshot.try_status().unwrap().generation > done.generation);
    assert_eq!(snapshot.capture_state(), (false, 0.0));
}

#[test]
fn snapshot_reset_retains_crossrate_falls_back_use_toggles() {
    let mut hosted = hosted_spectral();
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xbe51);
    hosted_learn(&mut hosted, true);
    hosted_process(&mut hosted, &colored, &[4096]);
    hosted
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let snapshot = hosted_snapshot(&hosted);
    let before = snapshot.try_export().unwrap().expect("capture must export");
    assert!(before.engaged);
    assert_eq!(
        snapshot.try_status().unwrap().fallback,
        ProfileFallback::Measured
    );

    // Reset retains the stored profile and its generation bit-exactly.
    hosted.reset();
    let kept = snapshot.try_export().unwrap().expect("reset retains");
    assert_eq!(kept.generation, before.generation);
    assert_eq!(kept.profile, before.profile);
    assert!(kept.engaged);

    // Cross-rate: payload keeps capture metadata while engagement falls
    // back to floors; nothing is silently remapped.
    hosted.initialize(96_000).unwrap();
    let crossed = snapshot.try_export().unwrap().expect("cross-rate keeps");
    assert_eq!(crossed.profile, before.profile);
    assert_eq!(crossed.profile.sample_rate, RATE);
    assert_eq!(crossed.processing_rate, 96_000);
    assert!(!crossed.engaged);
    assert_eq!(
        snapshot.try_status().unwrap().fallback,
        ProfileFallback::Floor
    );
    assert!(crossed.generation > before.generation);

    // Use flag off disables engagement without touching the payload.
    hosted
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    let disabled = snapshot.try_export().unwrap().expect("payload survives");
    assert_eq!(disabled.profile, before.profile);
    assert!(!disabled.use_flag && !disabled.engaged);
    assert_eq!(
        snapshot.try_status().unwrap().fallback,
        ProfileFallback::Disabled
    );
}

#[test]
fn malformed_restore_keeps_generation_profile_and_audio() {
    let mut hosted = hosted_spectral();
    let colored = first_difference_hiss(RATE as usize, 0.035, 0x9a10);
    hosted_learn(&mut hosted, true);
    hosted_process(&mut hosted, &colored, &[4096]);
    hosted
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    hosted.reset();
    let snapshot = hosted_snapshot(&hosted);
    let good = snapshot.try_export().unwrap().expect("capture must export");

    let segment = lcg_noise(8192, 0.04, 0x77aa);
    let reference = hosted_process(&mut hosted, &segment, &[997, 64]);

    let mut bad_version = good.profile.clone();
    bad_version.format_version = 99;
    bad_version.spectral = None;
    let mut bad_floor = good.profile.clone();
    bad_floor.floor_db_per_channel[0] = f32::NAN;
    let mut bad_spectrum = good.profile.clone();
    bad_spectrum
        .spectral
        .as_mut()
        .unwrap()
        .power_per_channel_bin
        .pop();
    let bad_channels = NoiseProfileData {
        format_version: 1,
        sample_rate: RATE,
        channels: 2,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: vec![-40.0, -41.0],
        frames_analyzed: u64::from(RATE),
        spectral: None,
    };
    let mut inner = hosted.into_inner();
    for (label, candidate) in [
        ("bad-version", &bad_version),
        ("bad-floor", &bad_floor),
        ("bad-spectrum", &bad_spectrum),
        ("bad-channels", &bad_channels),
    ] {
        let error = inner.restore_profile(candidate).unwrap_err();
        assert!(!error.is_empty(), "{label} must explain the rejection");
    }
    // Re-hosting moves the same plugin (and its snapshot Arc) without
    // reinitializing, so the generation must be exactly preserved.
    let mut hosted = HostedHiss::new(inner);
    let kept = hosted_snapshot(&hosted).try_export().unwrap().unwrap();
    assert_eq!(kept.generation, good.generation);
    assert_eq!(kept.profile, good.profile);
    hosted.reset();
    let after = hosted_process(&mut hosted, &segment, &[997, 64]);
    assert_eq!(
        reference, after,
        "rejected restores must retain accepted audio history"
    );
}

#[test]
fn concurrent_retained_readers_coherent_or_busy() {
    // Two distinct stored profiles; every successful concurrent export
    // must equal one of them exactly, never a mixture.
    let low_base = lcg_noise(RATE as usize, 0.05, 0xc010);
    let mut low_pass = 0.0f64;
    let low: Vec<f32> = low_base
        .iter()
        .map(|&s| {
            low_pass = 0.5 * f64::from(s) + 0.5 * low_pass;
            low_pass as f32
        })
        .collect();
    let high: Vec<f32> = first_difference_hiss(RATE as usize, 0.035, 0xc010);
    let capture_blob = |noise: &[f32]| {
        let mut profiler = hosted_spectral();
        hosted_learn(&mut profiler, true);
        hosted_process(&mut profiler, noise, &[4096]);
        hosted_snapshot(&profiler)
            .try_export()
            .unwrap()
            .expect("capture must export")
            .profile
    };
    let blob_a = capture_blob(&low);
    let blob_b = capture_blob(&high);
    assert_ne!(blob_a, blob_b);

    let mut hosted = hosted_spectral();
    hosted
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let shared = hosted_snapshot(&hosted);
    let retained = shared.clone();
    let mut inner = hosted.into_inner();
    // Seed before spawning: the profile must be present throughout so a
    // vanishing export is a real failure, not a startup race.
    inner.restore_profile(&blob_a).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..3 {
            let reader = shared.clone();
            let expect_a = &blob_a;
            let expect_b = &blob_b;
            scope.spawn(move || {
                for _ in 0..2000 {
                    match reader.try_export() {
                        Ok(Some(export)) => {
                            assert!(
                                export.profile == *expect_a || export.profile == *expect_b,
                                "concurrent export mixed generations"
                            );
                            // A concurrent publication may still overlap the
                            // follow-up status read; Busy stays legitimate.
                            if let Ok(status) = reader.try_status() {
                                assert_eq!(status.generation % 2, 0);
                            }
                        }
                        Ok(None) => panic!("profile present throughout; no export may vanish"),
                        Err(_) => {}
                    }
                }
            });
        }
        for round in 0..200 {
            let blob = if round % 2 == 0 { &blob_a } else { &blob_b };
            inner.restore_profile(blob).unwrap();
        }
    });
    // Retained readers keep the payload alive and observe the newest
    // consistent publication once the producer settles.
    let settled = retained.try_export().unwrap().expect("retained export");
    assert_eq!(settled.profile, blob_b);
    drop(inner);
    let after_drop = retained.try_export().unwrap().expect("kept alive");
    assert_eq!(after_drop.profile, blob_b);
}

#[test]
fn snapshot_paths_do_not_allocate_or_free() {
    let mut plugin = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            ..HissReducerPluginParams::default()
        },
    );
    plugin.initialize(RATE).unwrap();
    let warm = vec![0.01; 4096];
    let mut warm_block = warm.clone();
    plugin
        .process_in_place(&mut warm_block, &ctx(4096))
        .unwrap();

    // Cold get_data plus downcast: Arc clone and pointer read only.
    let (allocs, frees) = measure_heap_activity(|| {
        let data = plugin.get_data().expect("snapshot from construction");
        let _snapshot = data.downcast::<ProfileSnapshot>().unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "cold get_data heap activity");

    let learn_id = ParameterId::from("learn_noise");
    let use_id = ParameterId::from("use_captured_profile");
    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .parametric_set_parameter(learn_id.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "learn trigger heap activity");

    // Full 1 s capture block including spectral FFTs, completion, and the
    // bounded snapshot publication of floors plus 513 bin powers.
    let mut block = lcg_noise(RATE as usize, 0.05, 0xa110c);
    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .process_in_place(&mut block, &ctx(RATE as usize))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "capture process heap activity");
    assert!(plugin.has_captured_profile());

    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .parametric_set_parameter(use_id.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "use-profile toggle heap activity");

    // Steady processing with an engaged profile (no publication).
    let mut steady = lcg_noise(4096, 0.04, 0x57ea);
    let (allocs, frees) = measure_heap_activity(|| {
        plugin.process_in_place(&mut steady, &ctx(4096)).unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "steady process heap activity");

    let data = plugin.persisted_params().captured_profile.unwrap();
    assert_eq!(data.format_version, 2);
    let (allocs, frees) = measure_heap_activity(|| {
        plugin.restore_profile(&data).unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "v2 profile restore heap activity");

    let (allocs, frees) = measure_heap_activity(|| {
        plugin.clear_captured_profile();
    });
    assert_eq!((allocs, frees), (0, 0), "profile clear heap activity");

    plugin.restore_profile(&data).unwrap();
    let (allocs, frees) = measure_heap_activity(|| {
        plugin.reset();
    });
    assert_eq!((allocs, frees), (0, 0), "reset heap activity");

    // Profileless reinitialization publishes only live flags (the stored
    // carrier clone on profile-carrying reinit stays control-thread).
    plugin.clear_captured_profile();
    let (allocs, frees) = measure_heap_activity(|| {
        plugin.initialize(96_000).unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "rate init heap activity");

    // The original ID stays alive outside the tracked closure (as in the
    // learn/use legs): moving the last `Arc<str>` owner into the setter
    // would count its test-owned final drop as a free.
    let frequency_id = ParameterId::from("frequency_hz");
    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .parametric_set_parameter(frequency_id.clone(), ParameterValue::Float(8000.0))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "cutoff control heap activity");
}

#[test]
fn hosted_nonzero_audio_still_processed_with_data() {
    // Publishing get_data must not reclassify the audible effect into a
    // passthrough analyzer: latency/tail stay effect-shaped and hosted
    // nonzero audio is still processed (never bit-identical passthrough).
    let mut hosted = hosted_spectral();
    assert_eq!(hosted.latency_samples(), 1024);
    assert!(matches!(hosted.tail_length(), TailLength::Finite(_)));
    assert!(hosted.get_data().is_some());
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xdada);
    hosted_learn(&mut hosted, true);
    hosted_process(&mut hosted, &colored, &[4096]);
    hosted
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    hosted.reset();
    assert!(hosted.get_data().is_some());
    let hiss = lcg_noise(16384, 0.04, 0x9a12);
    let tone = sine_tone(16384, 0.06, 9984.375, RATE);
    let mix: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let output = hosted_process(&mut hosted, &mix, &PARTITIONS);
    assert!(output.iter().all(|s| s.is_finite()));
    assert_ne!(
        output[1024..16384],
        mix[0..15360],
        "hosted engaged render must process audio with data present"
    );
    assert!(hosted.get_data().is_some());
}
