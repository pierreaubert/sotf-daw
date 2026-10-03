//! Analog limiter through a real running engine: chain, manager, meters.
//!
//! Each test drives an actual public `AudioEngine` (manager thread,
//! processing thread, playback thread) on the named hardware-free ALSA
//! null backend and observes production state only: chain latency,
//! the latched epoch peak (lossless-complete at natural EOF via the
//! drained receipt's terminal record, and after an acked Stop via its
//! terminal record; partial only after a Stop-ack timeout, which sets
//! last_error), poll-maximum diagnostics, engine counters,
//! and natural end-of-stream. All
//! waits live in the shared `common::null_backend` harness, which polls
//! with sleeps and fails loudly on a liveness deadline; every audio claim
//! is gated on acked commands plus counters advancing past fresh-epoch
//! baselines, never on elapsed wall time. Fresh epochs are proven by epoch
//! receipt (a durable generation persisting until the next Play), never by
//! polling a transient flag or by cumulative counters alone. The null
//! backend drains faster
//! than the 100 ms meter cadence can sustain mid-playback windows, so this
//! suite proves whole-run behavior per configuration: every mutation runs
//! on the real live chain via the public commands, and each resulting
//! configuration is smoked end to end (fresh replay drained to EOF with a
//! fresh tracker). EOF completes an observation; it cannot interrupt one.
//! Per-sample proof across live mutation instants lives in the
//! deterministic `limiter_live_mutation` module tests (real commands, real
//! DSP, every sample); this suite is the running-engine smoke leg and
//! does not claim full sample coverage from meter snapshots.
//!
//! Mid-stream Stop legs (P2) use a FIFO drip-feed barrier instead of
//! frame-counter polling: `playback_frames_written` publishes on the 5 s
//! diagnostics cadence while the null backend drains a 30 s fixture in
//! ~0.5 s wall, so counters cannot observe mid-stream. The engine plays a
//! 30 s program through a named pipe the test drips at ~10x realtime with
//! a hard 5 s supply cap, so unconsumed program provably remains at Stop
//! (byte accounting, not wall timing) while the decoder stays responsive
//! for the Stop acks. Two stimuli, each justified: the bypassed leg plays
//! the analytic fs/4 tone (the bit-exact chain needs a phase-exact
//! fixture; the byte oracle is the exact terminal), while the limited
//! legs play the original musical two-tone (the final clamp engages only
//! when post-color program exceeds C — the core-first + Tape-ADAA chain
//! holds maximum-slew program below C, measured 0.24400914 twice
//! identically, while drive x2 + tanh pushes slow-slew program back above
//! C every carrier cycle; the offline clamp-engagement test proves early
//! and dense engagement on this exact program class). Peak verdicts gate on
//! the observed latch (100 ms meter cadence) before stopping, then assert
//! the terminal bit-exactly: ceiling legs observe the clamped ceiling,
//! the bypassed leg observes the byte-oracle peak.
//!
//! Backend selection (root sets process env before running):
//!   ALSA_CONFIG_PATH=<repo>/audit/continuation-2026-10-01/hiss-async-capture/alsa-null.conf
//!   ALIM_E2E_DEVICE='SOTF Audit Null'
//! Without `ALIM_E2E_DEVICE` the tests print a loud skip and return; with
//! it set, a missing CPAL-visible device fails loudly (no silent skip).

// Rust guideline compliant 2026-02-21
mod common;

use common::null_backend::{
    FeederReport, FifoFeeder, POLL_DEADLINE, POLL_STEP, PeakTracker, TONE_AMPLITUDE, await_eof,
    await_latency_value, await_started, await_startup_latency, epoch_eof, epoch_started,
    force_replay, log_phase, make_hot_source, make_silent_source, make_tone_source, null_device,
    require_device_visible, wav_peak_from_bytes,
};
use serial_test::serial;
use sotf_audio::engine::{
    AudioEngine, AudioEngineState, EngineConfig, PlaybackState, PluginConfig,
};
use sotf_audio::{PluginChain, PluginSettings, PluginType};
use std::time::Instant;

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const DEVICE_VAR: &str = "ALIM_E2E_DEVICE";
const LOOKAHEAD_5MS_LATENCY: usize = 240;
const LOOKAHEAD_10MS_LATENCY: usize = 480;
/// -12 dB ceiling (0.25119) with meter-window slack, and the hot floor
/// proving the program really exercised the limiter (source peaks ~0.9).
const CEILING_12DB: f32 = 0.26;
const HOT_FLOOR: f32 = 0.2;
/// -18 dB ceiling (0.12589) with slack for the automation leg.
const CEILING_18DB: f32 = 0.13;
/// Bypassed program must exceed this (unlimited ~0.9 peaks).
const BYPASSED_FLOOR: f32 = 0.5;
/// Liveness floor for bounded phases under the -18 dB ceiling: the fixture
/// is constant-amplitude hot program, so any live window reads well above this.
const LIVE_FLOOR_18DB: f32 = 0.05;
/// Bypassed program stays under this (fixture peaks ~0.9); guards against
/// bypass-path garbage while the floor proves the program is unlimited.
const BYPASSED_CEILING: f32 = 1.0;

fn limiter_config(model: &str, threshold_db: f64, lookahead_ms: f64) -> PluginConfig {
    PluginConfig::new(
        "analog_limiter",
        serde_json::json!({
            "threshold": threshold_db,
            "release": 50.0,
            "lookahead": lookahead_ms,
            "soft": false,
            "true_peak": false,
            "mix": 1.0,
            "analog_model": model,
            "analog_drive": 6.0,
            "analog_color": 0.5,
            "analog_character": 0.25,
            "analog_trim": 0.0,
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
    let device = null_device(DEVICE_VAR, "analog-limiter running-chain")?;
    require_device_visible(&device);
    Some(device)
}

/// Smoke one chain configuration end to end: fresh replay drained to EOF,
/// asserting latched nonzero audio under `ceiling`, and return the latched
/// epoch peak.
///
/// No window counting, so EOF completes the observation instead of
/// interrupting it; the only wall clock is the liveness deadline. The
/// chain persists across transport restarts, so each smoke exercises the
/// configuration left by the preceding live mutation. Freshness is proven
/// by frame advance past the pre-Play baselines, not by cumulative
/// counters alone.
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
    // D3: absolute zero, not baseline-relative — a generation-ahead
    // receipt is impossible by construction; any occurrence fails loudly.
    assert_eq!(final_state.gen_ahead_events, 0);
    // P2: no worker died mid-epoch, so the record is complete.
    assert!(final_state.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_state.worker_death_poisoned);
    latched
}

#[test]
#[serial]
fn running_chain_limits_hot_program_reports_latency_and_reaches_eof() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let (wav_path, _wav_temp) = make_hot_source(10.0, RATE, CHANNELS);
    let engine = AudioEngine::new(manager_config(
        &device,
        vec![limiter_config("Tape", -12.0, 5.0)],
    ))
    .expect("engine must start");
    await_startup_latency(&engine, LOOKAHEAD_5MS_LATENCY);

    let epoch_start = Instant::now();
    // Pre-Play baseline: the fresh engine is Stopped, so counters are frozen.
    let baseline = engine.get_state();
    let target_epoch = baseline.playback_epoch.wrapping_add(1);
    engine.play(wav_path.clone()).expect("play must succeed");
    let mut peaks = PeakTracker::new();
    let started = await_started(&engine, &mut peaks, target_epoch);
    let final_state = await_eof(&engine, &mut peaks, &started);
    assert_eq!(final_state.playback_epoch, started.playback_epoch);
    log_phase(
        "limits-hot-program/full-run",
        &peaks,
        &final_state,
        epoch_start.elapsed(),
    );

    // Nonzero bounded emitted audio across the whole run (latched verdict).
    let latched = final_state.playback_peak_max_linear;
    assert!(
        latched > HOT_FLOOR,
        "program never exercised the limiter: {latched}"
    );
    assert!(
        latched <= CEILING_12DB,
        "emitted peak {latched} exceeds the -12 dB ceiling"
    );
    // Clock/width/counters/latency contracts (fresh advance past the
    // pre-Play baseline; the stats channel is epoch-tagged). Position
    // stays diagnostic (decoder-fed, untagged): logged, never a proof.
    assert!(
        final_state.playback_frames_written > baseline.playback_frames_written,
        "no fresh frames (baseline {}, final {})",
        baseline.playback_frames_written,
        final_state.playback_frames_written
    );
    assert!(
        final_state.playback_callback_count > baseline.playback_callback_count,
        "no fresh callbacks (baseline {}, final {})",
        baseline.playback_callback_count,
        final_state.playback_callback_count
    );
    assert_eq!(final_state.sample_rate, RATE);
    assert_eq!(final_state.num_channels, CHANNELS);
    assert_eq!(final_state.plugin_latency_samples, LOOKAHEAD_5MS_LATENCY);
    assert_eq!(final_state.last_error, None);
    // D3: absolute zero — the drain-generation backstop must never fire.
    assert_eq!(final_state.gen_ahead_events, 0);
    // P2: no worker died mid-epoch, so the record is complete.
    assert!(final_state.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_state.worker_death_poisoned);
}

#[test]
#[serial]
fn running_chain_structural_model_change_reconstructs_and_latency_tracks_lookahead() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let (wav_path, _wav_temp) = make_hot_source(8.0, RATE, CHANNELS);
    let engine = AudioEngine::new(manager_config(
        &device,
        vec![limiter_config("Harmonics", -12.0, 5.0)],
    ))
    .expect("engine must start");
    await_startup_latency(&engine, LOOKAHEAD_5MS_LATENCY);

    // Config A smoke: the Harmonics chain runs bounded end to end.
    smoke_whole_run(
        &engine,
        &wav_path,
        "structural/smoke-harmonics",
        CEILING_12DB,
        HOT_FLOOR,
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
    await_latency_value(&engine, &mut PeakTracker::new(), LOOKAHEAD_5MS_LATENCY);
    assert_eq!(engine.get_state().last_error, None);
    // P3: steady state never poisons (false-death pin).
    assert!(!engine.get_state().worker_death_poisoned);

    // The supported path is chain reconstruction on the live chain: same
    // latency (the model adds none), no error recorded. The smoke proves
    // the rebuilt chain runs bounded; per-sample transition proof lives
    // in the deterministic module tests.
    engine
        .update_plugin_chain(&[limiter_config("Tape", -12.0, 5.0)])
        .expect("model chain update must apply");
    await_latency_value(&engine, &mut PeakTracker::new(), LOOKAHEAD_5MS_LATENCY);
    assert_eq!(engine.get_state().last_error, None);
    // P3: steady state never poisons (false-death pin).
    assert!(!engine.get_state().worker_death_poisoned);
    smoke_whole_run(
        &engine,
        &wav_path,
        "structural/smoke-tape",
        CEILING_12DB,
        HOT_FLOOR,
    );

    // A lookahead change proves the update really rebuilds: the reported
    // chain latency tracks the new value, and the chain still runs bounded.
    engine
        .update_plugin_chain(&[limiter_config("Tape", -12.0, 10.0)])
        .expect("lookahead chain update must apply");
    await_latency_value(&engine, &mut PeakTracker::new(), LOOKAHEAD_10MS_LATENCY);
    smoke_whole_run(
        &engine,
        &wav_path,
        "structural/smoke-lookahead-10ms",
        CEILING_12DB,
        HOT_FLOOR,
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
    await_latency_value(&engine, &mut PeakTracker::new(), LOOKAHEAD_10MS_LATENCY);
    smoke_whole_run(
        &engine,
        &wav_path,
        "structural/smoke-post-rejection",
        CEILING_12DB,
        HOT_FLOOR,
    );
}

#[test]
#[serial]
fn running_chain_automation_bypass_and_transport_restart() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let (wav_path, _wav_temp) = make_hot_source(8.0, RATE, CHANNELS);
    let engine = AudioEngine::new(manager_config(
        &device,
        vec![limiter_config("Tape", -12.0, 5.0)],
    ))
    .expect("engine must start");
    await_startup_latency(&engine, LOOKAHEAD_5MS_LATENCY);

    // Config smoke at -12 dB before any mutation.
    smoke_whole_run(
        &engine,
        &wav_path,
        "bypass/smoke-12db",
        CEILING_12DB,
        HOT_FLOOR,
    );

    // Live automation on the running chain tightens the emitted bound to
    // the new ceiling; the smoke proves the automated chain end to end.
    engine
        .set_plugin_parameter(0, "threshold".to_string(), "-18.0".to_string())
        .expect("threshold automation must apply");
    smoke_whole_run(
        &engine,
        &wav_path,
        "bypass/smoke-18db",
        CEILING_18DB,
        LIVE_FLOOR_18DB,
    );

    // Bypass routes the unlimited program: the smoke must reach the floor
    // while staying under the sanity ceiling.
    engine.set_bypass(true).expect("bypass must apply");
    assert!(engine.get_state().processing_bypassed);
    let bypassed_latch = smoke_whole_run(
        &engine,
        &wav_path,
        "bypass/smoke-bypassed",
        BYPASSED_CEILING,
        BYPASSED_FLOOR,
    );

    // Re-engagement restores the bound on the live chain.
    engine.set_bypass(false).expect("un-bypass must apply");
    assert!(!engine.get_state().processing_bypassed);
    let reengaged_latch = smoke_whole_run(
        &engine,
        &wav_path,
        "bypass/smoke-re-engaged",
        CEILING_18DB,
        LIVE_FLOOR_18DB,
    );
    // The re-engaged epoch starts from a cleared latch: without the
    // Play-time reset, the hot bypassed peak would persist into this epoch.
    assert!(
        reengaged_latch < bypassed_latch,
        "latch never cleared across replay: bypassed {bypassed_latch}, re-engaged {reengaged_latch}"
    );

    // A second bypassed epoch sets up the PlayAt clear proof: hot again.
    engine.set_bypass(true).expect("bypass must apply");
    let bypassed_latch_2 = smoke_whole_run(
        &engine,
        &wav_path,
        "bypass/smoke-bypassed-2",
        BYPASSED_CEILING,
        BYPASSED_FLOOR,
    );

    // PlayAt starts a new epoch exactly like Play: the re-engaged limited
    // chain clears the hot bypassed latch, positions at 4.0s, and runs
    // bounded to EOF.
    engine.set_bypass(false).expect("un-bypass must apply");
    let play_at_start = Instant::now();
    // Pre-Play baseline: the previous smoke EOF'd, so the engine is Stopped
    // with frozen counters.
    let play_at_baseline = engine.get_state();
    let play_at_target = play_at_baseline.playback_epoch.wrapping_add(1);
    engine
        .play_at(wav_path.clone(), 4.0)
        .expect("play_at must succeed");
    let mut play_at_peaks = PeakTracker::new();
    let play_at_started = await_started(&engine, &mut play_at_peaks, play_at_target);
    assert_eq!(play_at_started.playback_epoch, play_at_target);
    let play_at_final = await_eof(&engine, &mut play_at_peaks, &play_at_started);
    assert_eq!(play_at_final.playback_epoch, play_at_started.playback_epoch);
    log_phase(
        "bypass/play-at",
        &play_at_peaks,
        &play_at_final,
        play_at_start.elapsed(),
    );
    let play_at_latch = play_at_final.playback_peak_max_linear;
    assert!(
        play_at_latch > LIVE_FLOOR_18DB,
        "play_at emitted no audio: {play_at_latch}"
    );
    assert!(
        play_at_latch <= CEILING_18DB,
        "play_at peak {play_at_latch} exceeds the -18 dB ceiling"
    );
    assert!(
        play_at_latch < bypassed_latch_2,
        "latch never cleared across play_at: bypassed {bypassed_latch_2}, play_at {play_at_latch}"
    );
    // Positioning is manager-set (deterministic); position *advance* is
    // decoder-fed and untagged, so it stays diagnostic (logged only).
    assert!(
        play_at_started.position >= 4.0,
        "play_at did not position at 4.0s: {}",
        play_at_started.position
    );
    assert!(
        play_at_final.playback_frames_written > play_at_baseline.playback_frames_written,
        "play_at produced no fresh frames (baseline {}, final {})",
        play_at_baseline.playback_frames_written,
        play_at_final.playback_frames_written
    );
    assert_eq!(play_at_final.last_error, None);
    // P3: steady state never poisons (false-death pin).
    assert!(!play_at_final.worker_death_poisoned);

    // Transport restart resets the stream: fresh playback runs to EOF with
    // the chain bound intact (whichever threshold the restart carries).
    engine.stop().expect("stop must succeed");
    let epoch_start = Instant::now();
    // Post-stop baseline: the acked stop settled the transport, so the
    // counters below exclude the previous epoch.
    let baseline = engine.get_state();
    let target_epoch = baseline.playback_epoch.wrapping_add(1);
    engine.play(wav_path.clone()).expect("replay must succeed");
    let mut replay_peaks = PeakTracker::new();
    let started = await_started(&engine, &mut replay_peaks, target_epoch);
    let final_state = await_eof(&engine, &mut replay_peaks, &started);
    assert_eq!(final_state.playback_epoch, started.playback_epoch);
    log_phase(
        "bypass/replay",
        &replay_peaks,
        &final_state,
        epoch_start.elapsed(),
    );
    let latched = final_state.playback_peak_max_linear;
    assert!(latched > 0.1, "replay emitted no audio: {latched}");
    assert!(
        latched <= CEILING_12DB,
        "replay peak {latched} exceeds the -12 dB ceiling"
    );
    assert!(
        final_state.playback_frames_written > baseline.playback_frames_written,
        "replay produced no fresh frames (baseline {}, final {})",
        baseline.playback_frames_written,
        final_state.playback_frames_written
    );
    assert_eq!(final_state.last_error, None);
    // D3: absolute zero — the drain-generation backstop must never fire.
    assert_eq!(final_state.gen_ahead_events, 0);
    // P2: no worker died mid-epoch, so the record is complete.
    assert!(final_state.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_state.worker_death_poisoned);
}

#[test]
#[serial]
fn running_chain_settings_save_reload_roundtrip() {
    let Some(device) = require_null_backend() else {
        return;
    };
    // Author the chain through public settings, persist, and reload.
    let mut chain = PluginChain::new();
    let index = chain
        .add_plugin(&PluginType::AnalogLimiter)
        .expect("analog limiter must add to the chain");
    {
        let plugin = chain.get_plugin_mut(index).expect("added plugin");
        let PluginSettings::AnalogLimiter {
            threshold,
            release,
            lookahead,
            mix,
            analog_model,
            analog_drive,
            analog_color,
            analog_character,
            analog_trim,
            ..
        } = &mut plugin.settings
        else {
            panic!("AnalogLimiter entry must carry AnalogLimiter settings");
        };
        *threshold = -12.0;
        *release = 50.0;
        *lookahead = 5.0;
        *mix = 1.0;
        *analog_model = 3.0;
        *analog_drive = 6.0;
        *analog_color = 0.5;
        *analog_character = 0.25;
        *analog_trim = 0.0;
    }
    // A freshly authored chain carries only the user plugin; the default
    // rack (monitors, matrix) is added by the loader, not the editor.
    assert_eq!(chain.len(), 1);
    let presets = tempfile::tempdir().expect("presets dir must open");
    chain
        .save_to_file(presets.path(), "analog_limiter_e2e")
        .expect("chain must save");
    let mut reloaded = PluginChain::new();
    let warnings = reloaded
        .load_from_file(presets.path(), "analog_limiter_e2e")
        .expect("chain must reload");
    assert!(warnings.is_empty(), "reload warnings: {warnings:?}");

    // The loader wraps user plugins in the documented default rack
    // (`ensure_default_rack` + `to_plugin_configs`): input monitor, user
    // processing, matrix, output monitor. The disabled replay gain stays
    // out of the configs while disabled. The supported oracle compares the
    // logical limiter entry and its preserved settings — not the raw
    // config arrays, which legitimately differ by the generated rack.
    let sample_rate = f64::from(RATE);
    let reloaded_configs = reloaded.to_plugin_configs(sample_rate);
    let reloaded_types: Vec<&str> = reloaded_configs
        .iter()
        .map(|config| config.plugin_type.as_str())
        .collect();
    assert_eq!(
        reloaded_types,
        [
            "loudness_monitor",
            "analog_limiter",
            "matrix",
            "loudness_monitor"
        ]
    );
    let authored_configs = chain.to_plugin_configs(sample_rate);
    assert_eq!(authored_configs.len(), 1);
    assert_eq!(authored_configs[0].plugin_type, "analog_limiter");
    let reloaded_limiters: Vec<&PluginConfig> = reloaded_configs
        .iter()
        .filter(|config| config.plugin_type == "analog_limiter")
        .collect();
    assert_eq!(reloaded_limiters.len(), 1);
    assert_eq!(
        authored_configs[0].parameters,
        reloaded_limiters[0].parameters
    );
    let authored_settings =
        serde_json::to_value(&chain.get_plugin(index).expect("authored limiter").settings).unwrap();
    let reloaded_limiter = reloaded
        .plugins()
        .iter()
        .find(|plugin| matches!(plugin.plugin_type(), PluginType::AnalogLimiter))
        .expect("reloaded limiter");
    assert_eq!(
        authored_settings,
        serde_json::to_value(&reloaded_limiter.settings).unwrap()
    );

    // The reloaded chain runs the limiter end to end with the bound intact.
    let (wav_path, _wav_temp) = make_hot_source(8.0, RATE, CHANNELS);
    let engine = AudioEngine::new(manager_config(
        &device,
        reloaded.to_plugin_configs(f64::from(RATE)),
    ))
    .expect("engine must start");
    await_startup_latency(&engine, LOOKAHEAD_5MS_LATENCY);
    let epoch_start = Instant::now();
    // Pre-Play baseline: the fresh engine is Stopped, so counters are frozen.
    let baseline = engine.get_state();
    let target_epoch = baseline.playback_epoch.wrapping_add(1);
    engine.play(wav_path.clone()).expect("play must succeed");
    let mut peaks = PeakTracker::new();
    let started = await_started(&engine, &mut peaks, target_epoch);
    let final_state = await_eof(&engine, &mut peaks, &started);
    assert_eq!(final_state.playback_epoch, started.playback_epoch);
    log_phase(
        "preset/reloaded-run",
        &peaks,
        &final_state,
        epoch_start.elapsed(),
    );
    let latched = final_state.playback_peak_max_linear;
    assert!(
        latched > HOT_FLOOR,
        "reloaded chain never exercised the limiter: {latched}"
    );
    assert!(
        latched <= CEILING_12DB,
        "reloaded peak {latched} exceeds the -12 dB ceiling"
    );
    assert!(
        final_state.playback_frames_written > baseline.playback_frames_written,
        "reloaded run produced no fresh frames (baseline {}, final {})",
        baseline.playback_frames_written,
        final_state.playback_frames_written
    );
    assert_eq!(final_state.last_error, None);
    // D3: absolute zero — the drain-generation backstop must never fire.
    assert_eq!(final_state.gen_ahead_events, 0);
    // P2: no worker died mid-epoch, so the record is complete.
    assert!(final_state.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_state.worker_death_poisoned);
}

#[test]
#[serial]
fn running_chain_direct_play_adopts_new_epoch_and_stays_bounded() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let (wav_path, _wav_temp) = make_hot_source(8.0, RATE, CHANNELS);
    let (silent_path, _silent_temp) = make_silent_source(8.0, RATE, CHANNELS);
    let engine = AudioEngine::new(manager_config(
        &device,
        vec![limiter_config("Tape", -12.0, 5.0)],
    ))
    .expect("engine must start");
    await_startup_latency(&engine, LOOKAHEAD_5MS_LATENCY);

    // Epoch A starts hot-limited; it is abandoned mid-stream (no EOF await).
    let target_a = engine.get_state().playback_epoch.wrapping_add(1);
    engine.play(wav_path).expect("play A must succeed");
    let started_a = await_started(&engine, &mut PeakTracker::new(), target_a);
    assert_eq!(started_a.playback_epoch, target_a);

    // Direct Play B with no intervening Stop: the fence adopts the new
    // epoch, discards A's unreported residual, and clears A's EOS flags.
    // B is silent, so any latched content is provably pre-Flush tail of
    // the limited A program — bounded by A's ceiling, never amplified.
    // (Isolation-by-value is impossible here — tail and leak share the
    // magnitude — so this leg guards wiring/completion; the isolation
    // delta is proven by the deterministic stale-rejection + fence unit
    // tests. No floor: silence promises no emission.)
    //
    // Exact sample boundary: callbacks render A-samples until the first
    // callback observing flush_requested starts discarding; the pending
    // pre-fence window is discarded with A's abandoned record (no verdict
    // covers A), while post-fence A-samples fold into B's record under
    // the honest wall-clock rule. The terminal record is lossless, so a
    // 0.0 latch proves B emitted silence; the bound below tolerates the
    // honest tail.
    let epoch_start = Instant::now();
    // Pre-Play baseline, read while A is still live: it may include up to
    // a millisecond of A-tail progress. Sound anyway: B cannot reach EOF
    // without emitting its own frames, so zero B-progress fails loudly at
    // the EOF wait instead of passing on A-tail advance.
    let baseline_b = engine.get_state();
    let target_b = started_a.playback_epoch.wrapping_add(1);
    engine
        .play(silent_path)
        .expect("direct play B must succeed");
    let mut peaks_b = PeakTracker::new();
    let started_b = await_started(&engine, &mut peaks_b, target_b);
    assert_eq!(started_b.playback_epoch, target_b);
    let final_b = await_eof(&engine, &mut peaks_b, &started_b);
    assert_eq!(final_b.playback_epoch, started_b.playback_epoch);
    log_phase(
        "direct-play/silent-run",
        &peaks_b,
        &final_b,
        epoch_start.elapsed(),
    );
    let latched_b = final_b.playback_peak_max_linear;
    assert!(
        latched_b <= CEILING_12DB,
        "direct-play peak {latched_b} exceeds A's -12 dB ceiling"
    );
    assert!(
        final_b.playback_frames_written > baseline_b.playback_frames_written,
        "no fresh frames (baseline {}, final {})",
        baseline_b.playback_frames_written,
        final_b.playback_frames_written
    );
    assert_eq!(final_b.last_error, None);
    // D3: absolute zero — the drain-generation backstop must never fire.
    assert_eq!(final_b.gen_ahead_events, 0);
    // P2: no worker died mid-epoch, so the record is complete.
    assert!(final_b.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_b.worker_death_poisoned);
}

#[test]
fn epoch_wait_gates_follow_the_decision_table() {
    let polled = |epoch: u64, playback_state: PlaybackState| AudioEngineState {
        playback_epoch: epoch,
        playback_state,
        ..AudioEngineState::default()
    };
    // Start gate: any target-epoch poll witnesses the start, including a
    // first poll that already shows EOF. Stale epochs never witness.
    assert!(epoch_started(&polled(5, PlaybackState::Playing), 5));
    assert!(epoch_started(&polled(5, PlaybackState::Stopped), 5));
    assert!(!epoch_started(&polled(4, PlaybackState::Playing), 5));
    assert!(!epoch_started(&polled(6, PlaybackState::Playing), 5));
    // EOF gate: target epoch plus Stopped; live or stale never completes.
    assert!(epoch_eof(&polled(5, PlaybackState::Stopped), 5));
    assert!(!epoch_eof(&polled(5, PlaybackState::Playing), 5));
    assert!(!epoch_eof(&polled(4, PlaybackState::Stopped), 5));
    assert!(!epoch_eof(&polled(6, PlaybackState::Stopped), 5));
}

// ---------------------------------------------------------------------------
// Mid-stream Stop leg (P2): FIFO drip-feed barrier + latch-gated terminal.
// ---------------------------------------------------------------------------
//
// Why a FIFO: `playback_frames_written` publishes on the 5 s diagnostics
// cadence while the null backend drains a 30 s fixture in ~0.5 s wall, so
// counter polling cannot observe mid-stream — the R20 legs stopped
// post-EOF (2813/2813 chunks in all three). The feeder instead supplies
// the program through a pipe with a hard cap: whatever the wall clock
// does, supplied <= cap < total is an integer fact, the decoder cannot
// emit unsupplied bytes, and EOF is impossible before release (write end
// open). Stop therefore provably lands mid-stream; the report counts are
// the remainder proof.
//
// Why latch-gating: the stop position inside the supplied prefix is still
// timing-dependent, so a stopped prefix peak cannot be predicted — but it
// need not be. The latch is monotonic and the meter cadence is fast
// (100 ms), so observing the target IN the latch before stopping pins the
// terminal from below; the ceiling (limited) or the byte oracle
// (bypassed) pins it from above. Exact equality both sides, no window
// derivation, no cross-fixture reference.
//
// Why two stimuli: the gate target must be reachable. The bypassed leg's
// bit-exact chain reproduces any fixture peak, so the phase-exact fs/4
// tone + exact oracle is ideal there — but the LIMITED legs need the
// final clamp to engage, and the core-first + Tape-ADAA chain holds the
// fs/4 tone's post-color max at 0.24400914 < C (R24, twice identical):
// hot input is not a clamp guarantee. The musical two-tone instead gets
// its core-limited C peaks pushed back above C by the 6 dB drive + tanh
// on every carrier cycle (slow slew: ADAA ~= static tanh), so the clamp
// re-engages within ms and recurs ~1000x/s — proven offline by the
// clamp-engagement test on this exact program class, re-verified online
// by the gate itself. The limited legs' byte oracle is therefore a
// hotness band (the FIFO'd bytes are the intended hot driver), while C
// stays the exact terminal target — never lowered to 0.24400914, which
// was the other stimulus's unclamped post-color max, not a ceiling.

/// Stop-leg program length: the feeder caps supply at 5 s, so 30 s leaves
/// a 25 s deterministic remainder reservoir (integer frames, not timing).
const STOP_LEG_SECONDS: f32 = 30.0;
/// FIFO file name: the `.wav` extension routes the decoder (extension
/// check precedes probing); the engine opens this path exactly once.
const STOP_LEG_FIFO_NAME: &str = "stop-leg.wav";
/// -12 dB ceiling linear, bit-exact (f32(10^-0.6)): every clamped sample
/// equals these exact bits (min(x, C) == C for all x >= C), so observing
/// the ceiling in the latch before Stop pins the terminal exactly.
const LIMITED_CEILING_EXACT: f32 = 0.25118864;
/// Hot-program driver band for the LIMITED legs' byte oracle: the FIFO'd
/// two-tone must peak here (measured ~0.8998; input peaks 0.9 >> C, so the
/// core limits every carrier cycle and the Tape drive pushes post-color
/// back above C). This band proves the bytes are the intended hot driver
/// — never a clamp precondition, since clamping needs no rare alignment
/// (every carrier cycle qualifies, ~1000x/s).
const HOT_PROGRAM_PEAK_FLOOR: f32 = 0.85;
/// Upper end of the driver band: the generator clamps to +-1.0 and i16
/// quantizes below it, so anything above is generator/parser drift.
const HOT_PROGRAM_PEAK_CEILING: f32 = 1.0;

/// Poll until the latched epoch peak reads exactly `target` bits.
///
/// Bitwise (`to_bits`) comparison: exact, no float-tolerance lint, and
/// stronger than `==` (distinct NaN/-0 encodings cannot slip through; the
/// latch is finite-positive here by construction). Fail-loud deadline;
/// every poll observes through the peak tracker like the other waits, so
/// the finiteness assert covers the whole leg.
fn await_latch_exact(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    target: f32,
) -> AudioEngineState {
    let start = Instant::now();
    loop {
        let state = peaks.observe(engine);
        if state.playback_peak_max_linear.to_bits() == target.to_bits() {
            return state;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "latch never read {target:.8} bits: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Outcome of one deterministic mid-stream Stop leg.
struct StopLegOutcome {
    epoch: u64,
    terminal: AudioEngineState,
    feeder: FeederReport,
}

/// Play the tone program through a FIFO feeder, gate on the latch, stop.
///
/// Starts the feeder (header written before play, first drip on the way),
/// replays from zero, waits for the latch to read exactly `target`, then
/// issues the acked Stop and releases the feeder. Returns the epoch
/// receipt, the sync-complete terminal state, and the feeder byte
/// accounting; the caller asserts verdicts (terminal peak, remainder,
/// pins).
fn run_fifo_stop_leg(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    tone_wav_bytes: Vec<u8>,
    latch_target: f32,
) -> StopLegOutcome {
    let feeder = FifoFeeder::start(tone_wav_bytes, STOP_LEG_FIFO_NAME);
    feeder.await_ready();
    let (_baseline, started) = force_replay(engine, feeder.path(), peaks);
    let epoch = started.playback_epoch;
    await_latch_exact(engine, peaks, latch_target);
    // Mid-leg write health: no genuine I/O pathology struck while the
    // decoder was reading (EPIPE never sets this — it is the expected
    // post-stop teardown class, so this check cannot flake on timing).
    assert!(
        !feeder.write_failed(),
        "FIFO feeder write failed mid-leg; byte counts untrusted"
    );
    engine.stop().expect("mid-stream stop must succeed");
    // Sync-complete stop: the terminal record folded before Ok (and
    // last_error None proves it was the sync path, not the timeout),
    // so an immediate read is deterministic — no poll needed.
    let terminal = engine.get_state();
    // P3: steady state never poisons (false-death pin, shared by all legs).
    assert!(!terminal.worker_death_poisoned);
    let report = feeder.release();
    StopLegOutcome {
        epoch,
        terminal,
        feeder: report,
    }
}

/// Build the 30 s analytic tone program and its byte oracle.
///
/// Writes the tone WAV to a temp file (proven writer plus hot check),
/// reads the bytes back for the FIFO feeder, and derives the expected
/// bypassed peak from those same bytes with the production decode
/// divisor — then cross-checks the parsed peak against the quantization
/// formula, so fixture-gen drift fails loud here, never in the engine
/// verdict. Returns the bytes plus the oracle peak.
fn tone_program_bytes_and_oracle() -> (Vec<u8>, f32) {
    let (path, _tone) = make_tone_source(STOP_LEG_SECONDS, RATE, CHANNELS);
    let bytes = std::fs::read(&path).expect("tone wav must read back");
    let oracle = wav_peak_from_bytes(&bytes, RATE, CHANNELS);
    let formula = f32::from((TONE_AMPLITUDE * f32::from(i16::MAX)) as i16) / 32768.0;
    assert_eq!(
        oracle.to_bits(),
        formula.to_bits(),
        "tone peak oracle drifted from the quantization formula"
    );
    (bytes, oracle)
}

/// Build the 30 s musical two-tone program for the LIMITED legs.
///
/// Writes the hot WAV to a temp file (proven writer plus hot check),
/// reads the bytes back for the FIFO feeder, and proves with the byte
/// oracle that the fed program peaks inside the driver band — the fed
/// bytes are the intended clamp driver, not a quiet/drifted fixture.
/// Returns the bytes; the LIMITED gate target stays the ceiling const
/// (clamp-exact + gate-pinned), never derived from these bytes.
fn hot_program_bytes() -> Vec<u8> {
    let (path, _hot) = make_hot_source(STOP_LEG_SECONDS, RATE, CHANNELS);
    let bytes = std::fs::read(&path).expect("hot wav must read back");
    let peak = wav_peak_from_bytes(&bytes, RATE, CHANNELS);
    assert!(
        (HOT_PROGRAM_PEAK_FLOOR..=HOT_PROGRAM_PEAK_CEILING).contains(&peak),
        "hot program outside the driver band: peak {peak} not in [{HOT_PROGRAM_PEAK_FLOOR}, {HOT_PROGRAM_PEAK_CEILING}]"
    );
    bytes
}

/// Fresh engine with the Tape -12 dB chain settled, bypassed on request.
///
/// Bypass applies before play (and is read back), matching the prior leg.
fn stop_leg_engine(device: &str, bypassed: bool) -> AudioEngine {
    let plugins = vec![limiter_config("Tape", -12.0, 5.0)];
    let engine = AudioEngine::new(manager_config(device, plugins)).expect("engine must start");
    await_startup_latency(&engine, LOOKAHEAD_5MS_LATENCY);
    if bypassed {
        engine.set_bypass(true).expect("bypass must apply");
        assert!(engine.get_state().processing_bypassed);
    }
    engine
}

/// Assert the deterministic remainder: no genuine write pathology,
/// supplied bounded by the cap, cap strictly below the program total
/// (all integer frames — no timing anywhere in this proof). The
/// `write_failed` assert stays post-release DELIBERATELY (review r8 §6.3
/// proposed removing it): EPIPE breaks the loop cleanly without setting
/// the flag, so only real pathology trips this — keeping it closes the
/// stop-window gap with zero flake risk.
fn assert_fifo_remainder(tag: &str, report: &FeederReport) {
    assert!(
        !report.write_failed,
        "{tag}: feeder write failed; byte counts untrusted"
    );
    assert!(
        report.supplied_payload_frames <= report.cap_frames,
        "{tag}: feeder exceeded its cap: {} > {}",
        report.supplied_payload_frames,
        report.cap_frames
    );
    assert!(
        report.cap_frames < report.total_payload_frames,
        "{tag}: cap covers the program: {} >= {}",
        report.cap_frames,
        report.total_payload_frames
    );
}

/// Log the feeder byte accounting for the gate log (the remainder proof
/// in numbers) plus the callback underrun count (starvation-gap witness,
/// diagnostic only — callback timing, never a verdict).
fn log_feeder(tag: &str, report: &FeederReport, underruns: u64) {
    eprintln!(
        "feeder {tag}: supplied_frames={} cap_frames={} total_frames={} write_failed={} underruns={underruns}",
        report.supplied_payload_frames,
        report.cap_frames,
        report.total_payload_frames,
        report.write_failed
    );
}

#[test]
#[serial]
fn running_chain_midstream_stop_reports_exact_ceiling_via_terminal() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let epoch_start = Instant::now();
    let engine = stop_leg_engine(&device, false);
    let bytes = hot_program_bytes();
    let mut peaks = PeakTracker::new();
    let outcome = run_fifo_stop_leg(&engine, &mut peaks, bytes, LIMITED_CEILING_EXACT);
    let terminal = outcome.terminal;
    assert_eq!(terminal.playback_state, PlaybackState::Stopped);
    assert_eq!(terminal.playback_epoch, outcome.epoch);
    assert_eq!(terminal.last_error, None);
    log_phase("stop/ceiling", &peaks, &terminal, epoch_start.elapsed());
    log_feeder("stop/ceiling", &outcome.feeder, terminal.underruns);
    // Two-sided exactness: the gate observed the ceiling in the latch
    // before Stop (a clamped sample was metered — lower pin), and the R1
    // contract forbids any emitted sample above it (upper pin).
    assert_eq!(
        terminal.playback_peak_max_linear.to_bits(),
        LIMITED_CEILING_EXACT.to_bits(),
        "mid-stream Stop terminal must carry the exact ceiling"
    );
    assert_fifo_remainder("stop/ceiling", &outcome.feeder);
    assert_eq!(terminal.gen_ahead_events, 0);
    assert!(terminal.peak_record_complete);
}

#[test]
#[serial]
fn running_chain_midstream_stop_reports_honest_bypassed_peak() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let epoch_start = Instant::now();
    let engine = stop_leg_engine(&device, true);
    let (bytes, oracle) = tone_program_bytes_and_oracle();
    let mut peaks = PeakTracker::new();
    let outcome = run_fifo_stop_leg(&engine, &mut peaks, bytes, oracle);
    let terminal = outcome.terminal;
    assert_eq!(terminal.playback_state, PlaybackState::Stopped);
    assert_eq!(terminal.playback_epoch, outcome.epoch);
    assert_eq!(terminal.last_error, None);
    log_phase("stop/bypassed", &peaks, &terminal, epoch_start.elapsed());
    log_feeder("stop/bypassed", &outcome.feeder, terminal.underruns);
    // Two-sided exactness: the gate observed the oracle peak in the latch
    // before Stop (a peak sample was metered — lower pin), and the
    // bit-exact bypass chain (f32 decode/copy/ring, unity volume,
    // pre-clamp meter) cannot emit above the fixture max (upper pin).
    assert_eq!(
        terminal.playback_peak_max_linear.to_bits(),
        oracle.to_bits(),
        "mid-stream bypassed terminal must carry the exact oracle peak"
    );
    assert_fifo_remainder("stop/bypassed", &outcome.feeder);
    assert_eq!(terminal.gen_ahead_events, 0);
    assert!(terminal.peak_record_complete);
}

#[test]
#[serial]
fn running_chain_stop_then_immediate_replay_keeps_fence_pure() {
    let Some(device) = require_null_backend() else {
        return;
    };
    let epoch_start = Instant::now();
    let engine = stop_leg_engine(&device, false);
    let bytes = hot_program_bytes();
    let (silent_path, _silent) = make_silent_source(8.0, RATE, CHANNELS);
    let mut peaks = PeakTracker::new();
    // A: hot two-tone program, stopped mid-stream behind the feeder
    // barrier. Its terminal carries the exact ceiling (convergent
    // replication of the ceiling leg) with the byte-counted remainder
    // to prove mid-stream.
    let outcome_a = run_fifo_stop_leg(&engine, &mut peaks, bytes, LIMITED_CEILING_EXACT);
    let stopped_a = outcome_a.terminal;
    assert_eq!(stopped_a.playback_state, PlaybackState::Stopped);
    assert_eq!(stopped_a.last_error, None);
    assert_eq!(
        stopped_a.playback_peak_max_linear.to_bits(),
        LIMITED_CEILING_EXACT.to_bits(),
        "stopped-A terminal must carry the exact ceiling"
    );
    assert_fifo_remainder("stop/fence-a", &outcome_a.feeder);
    log_phase("stop/fence-a", &peaks, &stopped_a, epoch_start.elapsed());
    log_feeder("stop/fence-a", &outcome_a.feeder, stopped_a.underruns);
    // B: immediate replay (no sleep, no EOF wait on A) of silence to
    // EOF. The fence cleared A's record; any A-tail leak into B would
    // read nonzero — exact 0.0 is the sharp purity proof.
    let mut peaks_b = PeakTracker::new();
    let (_baseline_b, started_b) = force_replay(&engine, &silent_path, &mut peaks_b);
    assert_eq!(started_b.playback_epoch, outcome_a.epoch + 1);
    let final_b = await_eof(&engine, &mut peaks_b, &started_b);
    assert_eq!(final_b.last_error, None);
    log_phase("stop/fence-b", &peaks_b, &final_b, epoch_start.elapsed());
    assert_eq!(
        final_b.playback_peak_max_linear, 0.0,
        "silent replay after Stop must latch exactly 0.0 (fence purity)"
    );
    assert_eq!(final_b.gen_ahead_events, 0);
    assert!(final_b.peak_record_complete);
    // P3: steady state never poisons (false-death pin).
    assert!(!final_b.worker_death_poisoned);
}
