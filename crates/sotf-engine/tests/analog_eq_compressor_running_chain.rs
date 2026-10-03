//! Analog EQ/compressor through a real running engine: structural refusal,
//! chain reconstruction, and retained-chain history.
//!
//! Each test drives an actual public `AudioEngine` (manager thread,
//! processing thread, playback thread) on the named hardware-free ALSA
//! null backend and observes production state only, mirroring the limiter
//! structural leg (`analog_limiter_running_chain.rs`): a live
//! `analog_model` set is refused naming the rebuild contract, the
//! supported path rebuilds via `update_plugin_chain`, and a rejected
//! update retains the running chain and its history. Every configuration
//! is smoked end to end (fresh replay drained to EOF with a fresh
//! tracker); per-sample proof across live mutation instants lives in the
//! deterministic plugin-crate twin tests, not in meter snapshots.
//!
//! Contract assertions use each plugin's actual published behavior with
//! the measured fixture peak as oracle (no arbitrary ceilings):
//! - EQ runs flat (0 dB bands) with the color stage off, which the
//!   plugin contract proves transparent, so the latched peak must match
//!   the fixture peak within meter slack on every model.
//! - The compressor runs makeup-free (0 dB static, no auto makeup,
//!   full wet) with the color stage off, so its gain computer can only
//!   attenuate: the latched peak must never exceed the fixture peak,
//!   while the hot program keeps it well above the liveness floor.
//!
//! Both plugins publish zero added latency, pinned across every rebuild.
//!
//! Backend selection (root sets process env before running):
//!   ALSA_CONFIG_PATH=<repo>/audit/continuation-2026-10-01/hiss-async-capture/alsa-null.conf
//!   ALIM_E2E_DEVICE='SOTF Audit Null'
//! Without `ALIM_E2E_DEVICE` the tests print a loud skip and return; with
//! it set, a missing CPAL-visible device fails loudly (no silent skip).

// Rust guideline compliant 2026-02-21
mod common;

use common::null_backend::{
    PeakTracker, await_eof, await_latency_value, await_startup_latency, force_replay, log_phase,
    make_hot_source, null_device, require_device_visible, wav_peak_from_bytes,
};
use serial_test::serial;
use sotf_audio::engine::{AudioEngine, EngineConfig, PluginConfig};
use std::time::Instant;

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const DEVICE_VAR: &str = "ALIM_E2E_DEVICE";
/// Unity-gain EQ must neither boost nor swallow the program peak; the
/// 16-bit fixture quantizes near 3e-5, so this is pure meter slack.
const TRANSPARENCY_SLACK: f32 = 0.02;
const TRANSPARENCY_DROPOUT_SLACK: f32 = 0.08;
/// Makeup-free compressor gain never exceeds unity; float dust only.
const NO_BOOST_SLACK: f32 = 0.001;
/// Hot two-tone through mild 2:1 compression stays far above this.
const COMP_LIVE_FLOOR: f32 = 0.2;

fn eq_config(model: &str) -> PluginConfig {
    PluginConfig::new(
        "analog_eq",
        serde_json::json!({
            "low_freq": 100.0,
            "low_gain": 0.0,
            "mid1_freq": 800.0,
            "mid1_gain": 0.0,
            "mid1_q": 1.0,
            "mid2_freq": 3000.0,
            "mid2_gain": 0.0,
            "mid2_q": 1.0,
            "high_freq": 10000.0,
            "high_gain": 0.0,
            "analog_model": model,
            "analog_drive": 0.0,
            "analog_color": 0.0,
            "analog_character": 0.0,
            "analog_trim": 0.0,
        }),
    )
}

fn compressor_config(model: &str) -> PluginConfig {
    PluginConfig::new(
        "analog_compressor",
        serde_json::json!({
            "threshold": -12.0,
            "ratio": 2.0,
            "attack": 10.0,
            "release": 100.0,
            "knee": 6.0,
            "makeup": 0.0,
            "mix": 1.0,
            "auto_makeup": false,
            "analog_model": model,
            "analog_drive": 0.0,
            "analog_color": 0.0,
            "analog_character": 0.0,
            "analog_trim": 0.0,
            "range_db": 120.0,
            "hold_ms": 0.0,
        }),
    )
}

fn manager_config(device: &str, plugins: Vec<PluginConfig>) -> EngineConfig {
    EngineConfig {
        output_sample_rate: RATE,
        output_channels: CHANNELS,
        input_channels: CHANNELS,
        frame_size: 512,
        buffer_ms: 100,
        allow_virtual_output: true,
        output_device: Some(device.to_string()),
        plugins,
        ..EngineConfig::default()
    }
}

fn require_null_backend() -> Option<String> {
    let device = null_device(DEVICE_VAR, "analog-eq-compressor running-chain")?;
    require_device_visible(&device);
    Some(device)
}

fn fixture_peak(wav_path: &std::path::Path) -> f32 {
    let bytes = std::fs::read(wav_path).expect("fixture must read");
    wav_peak_from_bytes(&bytes, RATE, CHANNELS)
}

/// Smoke one chain configuration end to end: fresh replay drained to EOF,
/// asserting the latched epoch peak sits in `(floor, ceiling]`, and return
/// the latched peak. Freshness is proven by frame advance past the
/// pre-Play baselines; EOF completes the observation.
fn smoke_whole_run(
    engine: &AudioEngine,
    wav_path: &std::path::Path,
    tag: &str,
    ceiling: f32,
    floor: f32,
) -> f32 {
    let epoch_start = Instant::now();
    let mut peaks = PeakTracker::new();
    let (baseline, started) = force_replay(engine, wav_path, &mut peaks);
    let final_state = await_eof(engine, &mut peaks, &started);
    assert_eq!(final_state.playback_epoch, started.playback_epoch);
    log_phase(tag, &peaks, &final_state, epoch_start.elapsed());
    let latched = final_state.playback_peak_max_linear;
    assert!(
        latched > floor,
        "{tag}: expected audio above {floor}, latched {latched}"
    );
    assert!(
        latched <= ceiling,
        "{tag}: latched peak {latched} exceeds {ceiling}"
    );
    assert!(
        final_state.playback_frames_written > baseline.playback_frames_written,
        "{tag}: no fresh frames (baseline {}, final {})",
        baseline.playback_frames_written,
        final_state.playback_frames_written
    );
    assert_eq!(final_state.last_error, None);
    // D3: absolute zero — a generation-ahead receipt is impossible by
    // construction; any occurrence fails loudly.
    assert_eq!(final_state.gen_ahead_events, 0);
    // P2: no worker died mid-epoch, so the record is complete.
    assert!(final_state.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_state.worker_death_poisoned);
    latched
}

#[test]
#[serial]
fn running_chain_eq_structural_model_change_reconstructs() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let (wav_path, _wav_temp) = make_hot_source(8.0, RATE, CHANNELS);
    let source_peak = fixture_peak(&wav_path);
    assert!(
        source_peak > 0.5,
        "hot fixture must peak hot: {source_peak}"
    );
    let engine = AudioEngine::new(manager_config(&device, vec![eq_config("Harmonics")]))
        .expect("engine must start");
    await_startup_latency(&engine, 0);

    // Flat EQ with the color stage off is transparent: the latched peak
    // matches the measured fixture peak within meter slack.
    smoke_whole_run(
        &engine,
        &wav_path,
        "eq-structural/smoke-harmonics",
        source_peak + TRANSPARENCY_SLACK,
        source_peak - TRANSPARENCY_DROPOUT_SLACK,
    );

    // A live structural model set is refused by the running chain (host
    // metadata gate plus owned refusal); the refusal must name the
    // rebuild contract and leave the chain untouched.
    let refusal = engine
        .set_plugin_parameter(0, "analog_model".to_string(), "Tape".to_string())
        .expect_err("live structural model change must fail");
    assert!(
        refusal.contains("rebuild"),
        "refusal must name reconstruction: {refusal}"
    );
    await_latency_value(&engine, &mut PeakTracker::new(), 0);
    assert_eq!(engine.get_state().last_error, None);
    // P3: steady state never poisons (false-death pin).
    assert!(!engine.get_state().worker_death_poisoned);

    // The supported path is chain reconstruction on the live chain: the
    // rebuilt chain adopts the requested model (transparency holds on
    // every model at zero color), latency stays zero, no error recorded.
    engine
        .update_plugin_chain(&[eq_config("Tape")])
        .expect("model chain update must apply");
    await_latency_value(&engine, &mut PeakTracker::new(), 0);
    assert_eq!(engine.get_state().last_error, None);
    assert!(!engine.get_state().worker_death_poisoned);
    smoke_whole_run(
        &engine,
        &wav_path,
        "eq-structural/smoke-tape",
        source_peak + TRANSPARENCY_SLACK,
        source_peak - TRANSPARENCY_DROPOUT_SLACK,
    );

    // A rejected chain update retains the running chain and its history:
    // the rejection names the cause, latency is unchanged, and the
    // retained chain still runs transparent end to end.
    let bad = engine
        .update_plugin_chain(&[PluginConfig::new("no_such_plugin", serde_json::json!({}))])
        .expect_err("unknown plugin type must fail validation");
    assert!(
        bad.contains("Unknown plugin type"),
        "unexpected rejection: {bad}"
    );
    await_latency_value(&engine, &mut PeakTracker::new(), 0);
    smoke_whole_run(
        &engine,
        &wav_path,
        "eq-structural/smoke-post-rejection",
        source_peak + TRANSPARENCY_SLACK,
        source_peak - TRANSPARENCY_DROPOUT_SLACK,
    );
}

#[test]
#[serial]
fn running_chain_compressor_structural_model_change_reconstructs() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let (wav_path, _wav_temp) = make_hot_source(8.0, RATE, CHANNELS);
    let source_peak = fixture_peak(&wav_path);
    assert!(
        source_peak > 0.5,
        "hot fixture must peak hot: {source_peak}"
    );
    let engine = AudioEngine::new(manager_config(
        &device,
        vec![compressor_config("Harmonics")],
    ))
    .expect("engine must start");
    await_startup_latency(&engine, 0);

    // Makeup-free compression only attenuates: the latched peak never
    // exceeds the measured fixture peak, while the hot program keeps it
    // far above the liveness floor.
    smoke_whole_run(
        &engine,
        &wav_path,
        "comp-structural/smoke-harmonics",
        source_peak + NO_BOOST_SLACK,
        COMP_LIVE_FLOOR,
    );

    // A live structural model set is refused by the running chain (host
    // metadata gate plus owned refusal); the refusal must name the
    // rebuild contract and leave the chain untouched.
    let refusal = engine
        .set_plugin_parameter(0, "analog_model".to_string(), "Tape".to_string())
        .expect_err("live structural model change must fail");
    assert!(
        refusal.contains("rebuild"),
        "refusal must name reconstruction: {refusal}"
    );
    await_latency_value(&engine, &mut PeakTracker::new(), 0);
    assert_eq!(engine.get_state().last_error, None);
    // P3: steady state never poisons (false-death pin).
    assert!(!engine.get_state().worker_death_poisoned);

    // The supported path is chain reconstruction on the live chain: the
    // rebuilt chain adopts the requested model, latency stays zero, no
    // error recorded, and the attenuate-only bound still holds.
    engine
        .update_plugin_chain(&[compressor_config("Tape")])
        .expect("model chain update must apply");
    await_latency_value(&engine, &mut PeakTracker::new(), 0);
    assert_eq!(engine.get_state().last_error, None);
    assert!(!engine.get_state().worker_death_poisoned);
    smoke_whole_run(
        &engine,
        &wav_path,
        "comp-structural/smoke-tape",
        source_peak + NO_BOOST_SLACK,
        COMP_LIVE_FLOOR,
    );

    // A rejected chain update retains the running chain and its history:
    // the rejection names the cause, latency is unchanged, and the
    // retained chain still runs bounded end to end.
    let bad = engine
        .update_plugin_chain(&[PluginConfig::new("no_such_plugin", serde_json::json!({}))])
        .expect_err("unknown plugin type must fail validation");
    assert!(
        bad.contains("Unknown plugin type"),
        "unexpected rejection: {bad}"
    );
    await_latency_value(&engine, &mut PeakTracker::new(), 0);
    smoke_whole_run(
        &engine,
        &wav_path,
        "comp-structural/smoke-post-rejection",
        source_peak + NO_BOOST_SLACK,
        COMP_LIVE_FLOOR,
    );
}
