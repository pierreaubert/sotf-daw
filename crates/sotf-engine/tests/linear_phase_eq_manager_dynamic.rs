// Real-manager consumer tests for linear-phase EQ dynamic updates.
//
// Each test drives an actual public `AudioEngine` (spawning the real manager
// thread, decoder, processing thread and playback thread) on the named
// hardware-free ALSA null backend. The manager thread snapshots the accepted
// base from the shared wrapper handle, prepares FIR design off audio, and
// queues the bounded payload; real audio commits the existing crossfade.
// Observations come from production state only: manager status round-trips,
// lock-free handle mirrors, engine state counters (frames, meter, latency,
// position) and natural end-of-stream. No helper loop is duplicated, no
// processing command is invoked on the test thread, and no wall-clock
// playback state is assumed across the compressed null device.
//
// Backend selection (root sets process env before running):
//   ALSA_CONFIG_PATH=<repo>/audit/continuation-2026-10-01/hiss-async-capture/alsa-null.conf
//   AEQ_E2E_DEVICE='SOTF Audit Null'
// Without `AEQ_E2E_DEVICE` the tests print a loud skip and return; with it
// set, a missing CPAL-visible device fails loudly (no silent skip).
//
// M1 accuracy (96 kHz / 1024 taps multiband, 0.05 dB) stays open separately;
// these tests use 48 kHz passing-case bands only.

use serial_test::serial;
use sotf_audio::engine::{
    AudioEngine, AudioEngineState, EngineConfig, PlaybackState, PluginConfig,
};
use sotf_plugins::plugin_linear_phase_eq::dynamic_host::{
    LinearPhaseEqControlHandle, LinearPhaseEqControlStatus,
};
use sotf_plugins::plugin_linear_phase_eq::{BandConfig, CommitRefusal, LinearPhaseEqPlugin};
use std::path::PathBuf;
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const FILE_SECONDS: f32 = 30.0;
/// Expected chain latency for the single-stage legacy path at 1024 taps:
/// 512 is half the even-tap FIR (the designer centers its impulse at N/2)
/// and 32 is one NUPC 32-sample head partition of streaming latency per
/// convolver stage (`latency_samples`: `fir_length()/2 + 32`, one stage).
const EXPECTED_LATENCY: usize = 512 + 32;
const EQ_INDEX: usize = 1;
const POLL_STEP: Duration = Duration::from_millis(10);
const POLL_DEADLINE: Duration = Duration::from_secs(30);
/// Minimum output meter peak proving genuine nonzero audio traversed the
/// full decode/process/playback chain (source peaks near 0.5).
const MIN_OUTPUT_PEAK: f32 = 0.1;

/// Resolve the required null backend, or print a loud skip and return `None`.
fn null_device() -> Option<String> {
    match std::env::var("AEQ_E2E_DEVICE") {
        Ok(name) if !name.trim().is_empty() => Some(name),
        _ => {
            eprintln!(
                "Skipping linear-phase-EQ manager test: AEQ_E2E_DEVICE is unset. \
                 Set AEQ_E2E_DEVICE='SOTF Audit Null' with the audit ALSA null \
                 config to run the real-manager consumer gate."
            );
            None
        }
    }
}

/// Require the named device to be CPAL-visible; fail loudly otherwise.
fn require_device_visible(name: &str) {
    use cpal::traits::{DeviceTrait, HostTrait};
    let found: Vec<String> = cpal::default_host()
        .output_devices()
        .map(|devices| {
            devices
                .filter_map(|device| {
                    device
                        .description()
                        .ok()
                        .map(|description| description.name().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        found.iter().any(|device| device == name),
        "AEQ_E2E_DEVICE='{name}' is not CPAL-visible; found devices: {found:?}"
    );
}

fn band(filter_type: &str, frequency: f64, q: f64, gain_db: f64) -> BandConfig {
    BandConfig {
        filter_type: filter_type.to_string(),
        frequency,
        q,
        gain_db,
        active: true,
        placement: None,
    }
}

fn eq_parameters(bands: &[BandConfig]) -> serde_json::Value {
    let filters: Vec<serde_json::Value> = bands
        .iter()
        .map(|b| {
            serde_json::json!({
                "filter_type": b.filter_type,
                "frequency": b.frequency,
                "q": b.q,
                "gain_db": b.gain_db,
                "active": b.active,
            })
        })
        .collect();
    serde_json::json!({
        "num_filters": bands.len(),
        "fir_length_index": 0,
        "phase_mode_index": 0,
        "auto_gain": false,
        "mix": 1.0,
        "filters": filters,
    })
}

fn manager_config(device: &str, bands: &[BandConfig]) -> EngineConfig {
    EngineConfig {
        output_sample_rate: RATE,
        output_channels: CHANNELS,
        input_channels: CHANNELS,
        frame_size: 512,
        buffer_ms: 100,
        allow_virtual_output: true,
        output_device: Some(device.to_string()),
        plugins: vec![
            PluginConfig::new("gain", serde_json::json!({ "gain_db": 0.0 })),
            PluginConfig::new("linear_phase_eq", eq_parameters(bands)),
        ],
        ..EngineConfig::default()
    }
}

/// Write a genuine nonzero stereo fixture: 440 Hz at 0.35 plus 1 kHz at
/// 0.15 per channel, plus a deterministic low-level dither so no long
/// silence hides a stalled chain.
fn write_source_wav(path: &std::path::Path, frames: usize) {
    let spec = hound::WavSpec {
        channels: CHANNELS as u16,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav writer must open");
    for frame in 0..frames {
        let time = frame as f32 / RATE as f32;
        let dither = ((frame * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.02;
        let left = (time * 440.0 * std::f32::consts::TAU).sin() * 0.35
            + (time * 1000.0 * std::f32::consts::TAU).sin() * 0.15
            + dither;
        let right = (time * 660.0 * std::f32::consts::TAU).sin() * 0.35
            + (time * 1000.0 * std::f32::consts::TAU).sin() * 0.15
            - dither;
        for sample in [left, right] {
            let quantized = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(quantized)
                .expect("wav sample must write");
        }
    }
    writer.finalize().expect("wav must finalize");
}

/// Read back the fixture and prove it is genuine nonzero audio.
fn assert_source_nonzero(path: &std::path::Path, frames: usize) {
    let mut reader = hound::WavReader::open(path).expect("wav must reopen");
    let spec = reader.spec();
    assert_eq!(spec.channels, CHANNELS as u16);
    assert_eq!(spec.sample_rate, RATE);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("wav sample must decode")) / f32::from(i16::MAX))
        .collect();
    assert_eq!(samples.len(), frames * CHANNELS);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    let rms = (sum / samples.len() as f64).sqrt() as f32;
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(rms > 0.2, "fixture RMS too low: {rms}");
    assert!(peak > 0.3, "fixture peak too low: {peak}");
}

/// Tracks the maximum output meter peak observed across polls.
struct PeakTracker {
    max: f32,
}

impl PeakTracker {
    fn new() -> Self {
        Self { max: 0.0 }
    }

    fn observe(&mut self, engine: &AudioEngine) -> AudioEngineState {
        let state = engine.get_state();
        if state.output_peak_linear.is_finite() {
            self.max = self.max.max(state.output_peak_linear);
        }
        state
    }
}

/// Await real playback start: Playing state, consumed frames, or advanced
/// position. Never assumes a wall-clock state across the null device.
fn await_started(engine: &AudioEngine, peaks: &mut PeakTracker) -> AudioEngineState {
    let start = Instant::now();
    loop {
        let state = peaks.observe(engine);
        // Progress counters (not just Playing) prove real playback: the null
        // device may drain the whole file before the first poll observes it.
        if state.playback_state == PlaybackState::Playing
            || state.playback_frames_written > 0
            || state.position > 0.0
        {
            return state;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "playback never started: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Fetch the detached handle, retrying across engine startup.
fn await_handle(engine: &AudioEngine, plugin_index: usize) -> Arc<LinearPhaseEqControlHandle> {
    let start = Instant::now();
    loop {
        match engine.linear_phase_eq_handle(plugin_index) {
            Ok(handle) => return handle,
            Err(reason) => {
                assert!(
                    start.elapsed() < POLL_DEADLINE,
                    "detached handle never fetched: {reason}"
                );
                std::thread::sleep(POLL_STEP);
            }
        }
    }
}

/// Await the published chain latency, proving startup settled.
fn await_latency(engine: &AudioEngine, latency_samples: usize) {
    let start = Instant::now();
    loop {
        let state = engine.get_state();
        if state.playback_state == PlaybackState::Stopped
            && state.plugin_latency_samples == latency_samples
            && state.last_error.is_none()
        {
            return;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "startup never settled with latency {latency_samples}: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Poll one status round-trip, tolerating transient queue timeouts.
///
/// A `None` return means the round-trip itself failed (startup, rebuild, or
/// load); the caller keeps polling until the deadline and reports the last
/// error alongside the last observed status.
fn poll_status(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    plugin_index: usize,
    last_error: &mut Option<String>,
) -> Option<LinearPhaseEqControlStatus> {
    peaks.observe(engine);
    match engine.linear_phase_eq_status(plugin_index) {
        Ok(status) => Some(status),
        Err(reason) => {
            *last_error = Some(reason);
            None
        }
    }
}

/// Await one accepted generation through the manager status command.
fn await_generation(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    plugin_index: usize,
    generation: u64,
) -> LinearPhaseEqControlStatus {
    let start = Instant::now();
    let mut last_error = None;
    let mut last_status = None;
    loop {
        if let Some(status) = poll_status(engine, peaks, plugin_index, &mut last_error) {
            if status.accepted_generation == generation {
                return status;
            }
            last_status = Some(status);
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "accepted generation {generation} never observed; last status: {last_status:?}; \
             last round-trip error: {last_error:?}; engine state: {:?}",
            engine.get_state()
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Await one refusal mirror through the manager status command.
fn await_refusal(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    plugin_index: usize,
    refusal: CommitRefusal,
) -> LinearPhaseEqControlStatus {
    let start = Instant::now();
    let mut last_error = None;
    let mut last_status = None;
    loop {
        if let Some(status) = poll_status(engine, peaks, plugin_index, &mut last_error) {
            if status.last_refusal == Some(refusal) {
                return status;
            }
            last_status = Some(status);
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "refusal {refusal:?} never observed; last status: {last_status:?}; \
             last round-trip error: {last_error:?}; engine state: {:?}",
            engine.get_state()
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Await an idle queue with a cleared refusal through the status command.
fn await_queue_idle(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    plugin_index: usize,
) -> LinearPhaseEqControlStatus {
    let start = Instant::now();
    let mut last_error = None;
    let mut last_status = None;
    loop {
        if let Some(status) = poll_status(engine, peaks, plugin_index, &mut last_error) {
            if status.retained_queued == 0 && status.last_refusal.is_none() {
                return status;
            }
            last_status = Some(status);
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "queue never idled; last status: {last_status:?}; \
             last round-trip error: {last_error:?}; engine state: {:?}",
            engine.get_state()
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Await a nonzero output peak through the meter flow.
///
/// Call while audio is flowing, or paused with in-flight audio already
/// through the ring: the periodic meter reports the callback-observed
/// residual at least every 100 ms, and pausing freezes decoder progress so
/// the observation window cannot race to EOF. Either a poll during flow or
/// the first emission after the pause carries the nonzero residual.
fn await_peak_nonzero(engine: &AudioEngine, peaks: &mut PeakTracker) {
    let start = Instant::now();
    loop {
        peaks.observe(engine);
        if peaks.max > MIN_OUTPUT_PEAK {
            return;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "output meter never saw nonzero audio: last state: {:?}",
            engine.get_state()
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Await natural end-of-stream: the engine reaches Stopped by itself after
/// real playback progress. The entry assert rejects calls before playback.
fn await_eof(engine: &AudioEngine, peaks: &mut PeakTracker) -> AudioEngineState {
    let entry = engine.get_state();
    assert!(
        entry.playback_frames_written > 0 || entry.position > 0.0,
        "await_eof called before playback: {entry:?}"
    );
    let start = Instant::now();
    loop {
        let state = peaks.observe(engine);
        if state.playback_state == PlaybackState::Stopped {
            return state;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "natural EOF never reached: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

fn make_source() -> (PathBuf, tempfile::NamedTempFile) {
    make_source_with_seconds(FILE_SECONDS)
}

fn make_source_with_seconds(seconds: f32) -> (PathBuf, tempfile::NamedTempFile) {
    let temp = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .expect("temp wav must open");
    let path = temp.path().to_path_buf();
    let frames = (seconds * RATE as f32) as usize;
    write_source_wav(&path, frames);
    assert_source_nonzero(&path, frames);
    (path, temp)
}

fn initial_bands() -> Vec<BandConfig> {
    vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ]
}

#[test]
#[serial]
fn manager_request_commits_during_playback_and_reaches_eof() {
    let Some(device) = null_device() else {
        return;
    };
    require_device_visible(&device);
    let (wav_path, _wav_temp) = make_source();

    let engine =
        AudioEngine::new(manager_config(&device, &initial_bands())).expect("engine must start");
    await_latency(&engine, EXPECTED_LATENCY);

    // Detached handle fetch traverses the real manager queue; a non-EQ
    // index must fail loudly instead of returning a wrong handle.
    let handle: Arc<LinearPhaseEqControlHandle> = await_handle(&engine, EQ_INDEX);
    assert!(engine.linear_phase_eq_handle(0).is_err());
    assert!(
        engine
            .linear_phase_eq_request(0, 0, band("Peak", 1000.0, 1.0, 6.0))
            .is_err()
    );
    assert_eq!(handle.accepted_generation(), 0);

    // Queue before play so the first commit cannot race the null drain; the
    // commit itself still happens inside real playback of nonzero audio.
    engine
        .linear_phase_eq_request(EQ_INDEX, 0, band("Peak", 1000.0, 1.0, 9.0))
        .expect("manager request must queue");
    // Queued is not accepted: the generation holds at zero before audio.
    assert_eq!(
        engine
            .linear_phase_eq_status(EQ_INDEX)
            .expect("status must succeed")
            .accepted_generation,
        0
    );

    engine.play(wav_path.clone()).expect("play must succeed");
    let mut peaks = PeakTracker::new();
    await_started(&engine, &mut peaks);
    let status = await_generation(&engine, &mut peaks, EQ_INDEX, 1);
    assert_eq!(status.last_refusal, None);
    // The accepted snapshot behind the detached handle carries the edit.
    let accepted = handle
        .try_accepted_snapshot()
        .expect("accepted snapshot must read");
    assert_eq!(accepted.generation, 1);
    assert_eq!(accepted.snapshot.bands[0].gain_db, 9.0);
    assert_eq!(accepted.snapshot.bands[1].gain_db, 0.0);
    // Direct atomic reads agree with the queued command round-trip.
    let direct = handle.control_status();
    assert_eq!(direct.accepted_generation, status.accepted_generation);
    assert_eq!(direct.last_refusal, status.last_refusal);
    // Fixed latency survives the dynamic commit.
    assert_eq!(engine.get_state().plugin_latency_samples, EXPECTED_LATENCY);

    // Pause mid-stream for a deterministic nonzero-peak observation window,
    // then resume to natural EOF.
    engine.pause().expect("pause must succeed");
    await_peak_nonzero(&engine, &mut peaks);
    engine.resume().expect("resume must succeed");

    // Natural EOF with genuine nonzero output observed along the way.
    let final_state = await_eof(&engine, &mut peaks);
    assert!(final_state.playback_frames_written > 0);
    assert!(final_state.playback_callback_count > 0);
    assert!(
        peaks.max > MIN_OUTPUT_PEAK,
        "output meter never saw nonzero audio: {}",
        peaks.max
    );
    assert!(
        final_state.position > 20.0,
        "incomplete EOF drain: {}",
        final_state.position
    );
    if let Some(duration) = final_state.duration {
        assert!(
            (duration - FILE_SECONDS as f64).abs() < 0.5,
            "duration: {duration}"
        );
    }
    assert!(final_state.last_error.is_none());
    engine.shutdown().expect("shutdown must succeed");
}

#[test]
#[serial]
fn manager_stale_refusal_cancel_and_recovery() {
    let Some(device) = null_device() else {
        return;
    };
    require_device_visible(&device);
    let (wav_path, _wav_temp) = make_source();

    let engine =
        AudioEngine::new(manager_config(&device, &initial_bands())).expect("engine must start");
    await_latency(&engine, EXPECTED_LATENCY);
    let handle: Arc<LinearPhaseEqControlHandle> = await_handle(&engine, EQ_INDEX);
    let stale_base = handle
        .try_accepted_snapshot()
        .expect("initial accepted base must read");
    assert_eq!(stale_base.generation, 0);

    engine
        .linear_phase_eq_request(EQ_INDEX, 0, band("Peak", 1000.0, 1.0, 6.0))
        .expect("first request must queue");
    engine.play(wav_path.clone()).expect("play must succeed");
    let mut peaks = PeakTracker::new();
    await_started(&engine, &mut peaks);
    await_generation(&engine, &mut peaks, EQ_INDEX, 1);

    // Stage a stale edit while paused: a worker prepares from the
    // generation-0 base and submits through the detached handle. The pause
    // also opens the deterministic nonzero-peak observation window.
    engine.pause().expect("pause must succeed");
    await_peak_nonzero(&engine, &mut peaks);
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &stale_base.snapshot,
                1,
                band("Peak", 3000.0, 1.0, -6.0),
            )
            .expect("stale prepare must succeed");
            tx.send(prepared).expect("worker ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("stale must queue");
        barrier.wait();
    });
    engine.resume().expect("resume must succeed");
    // The stale head refuses observably; acceptance stays retained.
    let status = await_refusal(&engine, &mut peaks, EQ_INDEX, CommitRefusal::StaleBase);
    assert_eq!(status.accepted_generation, 1);
    let accepted = handle
        .try_accepted_snapshot()
        .expect("accepted snapshot must read");
    assert_eq!(accepted.snapshot.bands[0].gain_db, 6.0);
    assert_eq!(accepted.snapshot.bands[1].gain_db, 0.0);

    // Cancel the wedged head while paused, then observe the idle queue.
    engine.pause().expect("pause must succeed");
    engine
        .linear_phase_eq_cancel(EQ_INDEX)
        .expect("cancel must succeed");
    engine.resume().expect("resume must succeed");
    await_queue_idle(&engine, &mut peaks, EQ_INDEX);
    assert_eq!(handle.try_reclaim_cancelled(), 1);

    // Fresh manager-prepared edit recovers and commits.
    engine.pause().expect("pause must succeed");
    engine
        .linear_phase_eq_request(EQ_INDEX, 1, band("Peak", 3000.0, 1.0, -6.0))
        .expect("recovery request must queue");
    engine.resume().expect("resume must succeed");
    let status = await_generation(&engine, &mut peaks, EQ_INDEX, 2);
    assert_eq!(status.last_refusal, None);
    let accepted = handle
        .try_accepted_snapshot()
        .expect("accepted snapshot must read");
    assert_eq!(accepted.snapshot.bands[0].gain_db, 6.0);
    assert_eq!(accepted.snapshot.bands[1].gain_db, -6.0);

    let final_state = await_eof(&engine, &mut peaks);
    assert!(final_state.playback_frames_written > 0);
    assert!(
        peaks.max > MIN_OUTPUT_PEAK,
        "output meter never saw nonzero audio: {}",
        peaks.max
    );
    assert!(
        final_state.position > 20.0,
        "incomplete EOF drain: {}",
        final_state.position
    );
    assert!(final_state.last_error.is_none());
    engine.shutdown().expect("shutdown must succeed");
}

#[test]
#[serial]
fn manager_rebuild_orphans_old_handle() {
    let Some(device) = null_device() else {
        return;
    };
    require_device_visible(&device);
    let (wav_path, _wav_temp) = make_source();
    let bands = initial_bands();

    let engine = AudioEngine::new(manager_config(&device, &bands)).expect("engine must start");
    await_latency(&engine, EXPECTED_LATENCY);
    let old_handle: Arc<LinearPhaseEqControlHandle> = await_handle(&engine, EQ_INDEX);
    engine
        .linear_phase_eq_request(EQ_INDEX, 0, band("Peak", 1000.0, 1.0, 6.0))
        .expect("first request must queue");
    engine.play(wav_path.clone()).expect("play must succeed");
    let mut peaks = PeakTracker::new();
    await_started(&engine, &mut peaks);
    await_generation(&engine, &mut peaks, EQ_INDEX, 1);
    let frozen_old = old_handle
        .try_accepted_snapshot()
        .expect("old snapshot must read")
        .snapshot;

    // Hot chain rebuild replaces the wrapper; the old handle orphans.
    // The pause also opens the deterministic nonzero-peak observation window.
    engine.pause().expect("pause must succeed");
    await_peak_nonzero(&engine, &mut peaks);
    engine
        .update_plugin_chain(&manager_config(&device, &bands).plugins)
        .expect("rebuild must succeed");
    engine.resume().expect("resume must succeed");
    let new_handle: Arc<LinearPhaseEqControlHandle> = await_handle(&engine, EQ_INDEX);
    assert!(!Arc::ptr_eq(&old_handle, &new_handle));
    assert_eq!(new_handle.accepted_generation(), 0);

    // A submission through the orphaned handle cannot touch the new graph,
    // whether the orphaned mailbox reports full or accepts into the void.
    // The payload is prepared from the new graph's initial base, so it is
    // stale for the old graph too: even if the retired old host still
    // renders its crossfade tail, the old acceptance stays frozen.
    let new_base = new_handle
        .try_accepted_snapshot()
        .expect("new base must read");
    let prepared = LinearPhaseEqPlugin::prepare_band_update(
        &new_base.snapshot,
        0,
        band("Peak", 1000.0, 1.0, 12.0),
    )
    .expect("orphan prepare must succeed");
    // Both outcomes are safe by design; log which one occurred for future
    // orphan-endpoint diagnostics.
    match old_handle.try_submit(prepared) {
        Ok(()) => eprintln!("orphan submit outcome: queued into orphaned mailbox"),
        Err(reason) => eprintln!("orphan submit outcome: refused ({reason})"),
    }
    // Observe real audio progress while the new graph stays untouched.
    // Position is the frequent flow signal here (frame counters publish on
    // the slow diagnostics cadence or at terminal drain, so they cannot pace
    // a mid-stream observation): decode tracks playback within the bounded
    // pipeline lead (~10 blocks per queue plus the ring at 100 ms buffer),
    // so one second of position advance proves genuine audio rendered
    // through the rebuilt graph while its generation holds at zero.
    let flow_start_pos = engine.get_state().position;
    let flow_deadline = Instant::now() + POLL_DEADLINE;
    loop {
        peaks.observe(&engine);
        let state = engine.get_state();
        assert_eq!(
            new_handle.accepted_generation(),
            0,
            "orphan submission reached the new graph"
        );
        if state.position > flow_start_pos + 1.0 {
            break;
        }
        assert!(
            Instant::now() < flow_deadline,
            "audio never flowed after rebuild: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
    let new_accepted = new_handle
        .try_accepted_snapshot()
        .expect("new snapshot must read");
    assert_eq!(new_accepted.snapshot.bands[0].gain_db, 0.0);
    // The orphan keeps reporting its own frozen graph state.
    assert_eq!(old_handle.accepted_generation(), 1);
    assert_eq!(
        old_handle
            .try_accepted_snapshot()
            .expect("orphan must stay readable")
            .snapshot,
        frozen_old
    );

    // The rebuilt graph accepts fresh manager edits. The fresh edit is
    // staged while paused: pausing freezes decoder and playback progress,
    // so the synchronous manager-side FIR preparation cannot race the null
    // device to EOF. The queued edit then commits on real resumed audio.
    engine.pause().expect("pause must succeed");
    engine
        .linear_phase_eq_request(EQ_INDEX, 1, band("Peak", 3000.0, 1.0, -4.0))
        .expect("new request must queue");
    engine.resume().expect("resume must succeed");
    let status = await_generation(&engine, &mut peaks, EQ_INDEX, 1);
    assert_eq!(status.last_refusal, None);

    let final_state = await_eof(&engine, &mut peaks);
    assert!(final_state.playback_frames_written > 0);
    assert!(
        peaks.max > MIN_OUTPUT_PEAK,
        "output meter never saw nonzero audio: {}",
        peaks.max
    );
    assert!(
        final_state.position > 20.0,
        "incomplete EOF drain: {}",
        final_state.position
    );
    assert!(final_state.last_error.is_none());
    engine.shutdown().expect("shutdown must succeed");
}

#[test]
#[serial]
fn manager_queue_full_and_idle_cancel_without_playback() {
    let Some(device) = null_device() else {
        return;
    };
    require_device_visible(&device);

    // No playback is started: no audio quantum ever pops the mailbox, so the
    // two-slot control queue fills deterministically from manager requests.
    let engine =
        AudioEngine::new(manager_config(&device, &initial_bands())).expect("engine must start");
    await_latency(&engine, EXPECTED_LATENCY);
    let handle: Arc<LinearPhaseEqControlHandle> = await_handle(&engine, EQ_INDEX);

    // Idle cancel with nothing anywhere is a successful no-op.
    engine
        .linear_phase_eq_cancel(EQ_INDEX)
        .expect("idle cancel must succeed");
    let idle = engine
        .linear_phase_eq_status(EQ_INDEX)
        .expect("status must succeed");
    assert_eq!(idle.accepted_generation, 0);
    assert_eq!(idle.retained_queued, 0);
    assert_eq!(idle.last_refusal, None);

    // Two manager requests queue; the third fails loudly through the
    // manager error transport with live state untouched.
    engine
        .linear_phase_eq_request(EQ_INDEX, 0, band("Peak", 1000.0, 1.0, 4.0))
        .expect("first request must queue");
    engine
        .linear_phase_eq_request(EQ_INDEX, 1, band("Peak", 3000.0, 1.0, 5.0))
        .expect("second request must queue");
    let full = engine
        .linear_phase_eq_request(EQ_INDEX, 0, band("Peak", 1000.0, 1.0, 6.0))
        .expect_err("third request without playback must report full");
    assert!(full.contains("full"), "unexpected full error: {full}");
    let status = engine
        .linear_phase_eq_status(EQ_INDEX)
        .expect("status must succeed");
    assert_eq!(status.accepted_generation, 0);
    assert_eq!(status.retained_queued, 0);
    assert_eq!(status.last_refusal, None);
    let accepted = handle
        .try_accepted_snapshot()
        .expect("accepted snapshot must read");
    assert_eq!(accepted.snapshot.bands[0].gain_db, 0.0);
    assert_eq!(accepted.snapshot.bands[1].gain_db, 0.0);

    // Cancel still succeeds with a mailboxed (never popped) queue; the
    // queued payloads drop with the wrapper on manager shutdown.
    engine
        .linear_phase_eq_cancel(EQ_INDEX)
        .expect("cancel must succeed");
    engine.shutdown().expect("shutdown must succeed");
}

#[test]
#[serial]
fn terminal_stats_publish_before_drained_for_short_stream() {
    let test_start = Instant::now();
    let Some(device) = null_device() else {
        return;
    };
    require_device_visible(&device);
    // Thirty seconds of audio draining in well under a second of wall
    // time: the short-stream premise is wall-clock under the five-second
    // diagnostics cadence, not audio length, and the established
    // thirty-second fixture sustains flowing playback across several
    // meter windows. The premise assert below verifies the wall bound
    // every run. No pause: pausing after the single sub-100 ms burst can
    // freeze the pipeline with the residual peak lost to one dropped
    // meter event (bounded-256 event channel, nonblocking sends, manager
    // ACK-blocked), hanging peak observation with nothing left to
    // re-prime it. Sustained flow plus the terminal meter backstop
    // observes deterministically instead.
    let (wav_path, _wav_temp) = make_source();

    let engine =
        AudioEngine::new(manager_config(&device, &initial_bands())).expect("engine must start");
    await_latency(&engine, EXPECTED_LATENCY);
    engine.play(wav_path.clone()).expect("play must succeed");

    // Observe genuine nonzero audio during sustained flow: the meter
    // reports every 100 ms with a freely draining manager, so an early
    // emission carries the burst peak, and the max-hold atom plus the
    // terminal meter backstop keep it observable through the drain.
    let mut peaks = PeakTracker::new();
    await_peak_nonzero(&engine, &mut peaks);

    // Tight-poll for the FIRST Stopped observation: with the terminal
    // snapshot emitted before the drained receipt on the same event
    // channel, final counters are already present — no later periodic can
    // be credited, because the premise assert below proves none could run.
    let first_stopped = loop {
        let state = peaks.observe(&engine);
        if state.playback_state == PlaybackState::Stopped {
            break state;
        }
        assert!(
            test_start.elapsed() < POLL_DEADLINE,
            "natural EOF never reached: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(
        test_start.elapsed() < Duration::from_secs(5),
        "short-stream premise violated: EOF took {:?}, a periodic snapshot may have fired",
        test_start.elapsed()
    );
    assert!(
        first_stopped.playback_frames_written > 0,
        "terminal counters missing at first Stopped: {first_stopped:?}"
    );
    assert!(
        first_stopped.playback_frames_received > 0,
        "terminal counters missing at first Stopped: {first_stopped:?}"
    );
    assert!(
        first_stopped.playback_callback_count > 0,
        "terminal counters missing at first Stopped: {first_stopped:?}"
    );
    assert!(
        first_stopped.position > 20.0,
        "incomplete EOF drain: {first_stopped:?}"
    );
    assert!(
        peaks.max > MIN_OUTPUT_PEAK,
        "output meter never saw nonzero audio: {}",
        peaks.max
    );
    assert!(first_stopped.last_error.is_none());
    assert_eq!(
        first_stopped.plugin_latency_samples, EXPECTED_LATENCY,
        "chain latency must hold through terminal EOF"
    );
    engine.shutdown().expect("shutdown must succeed");
}
