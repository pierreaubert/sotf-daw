//! Hardware-free running-engine harness on the audit ALSA null backend.
//!
//! Shared by engine integration tests that drive a real public
//! `AudioEngine` (manager thread, processing thread, playback thread)
//! without physical hardware: the null backend drains audio faster than
//! wall-clock test phases, so every helper polls production state and
//! gates audio claims on progression (acked commands plus advancing
//! frame/callback/position counters), never on elapsed wall time.
//!
//! Polling sleeps between state reads, and every wait carries a wall-clock
//! deadline — but the deadline is a liveness guardrail that fails loudly
//! on a stuck engine, not a timing assertion. Root exports the per-lane
//! device variable plus the shared config before running:
//! `ALSA_CONFIG_PATH=<repo>/audit/continuation-2026-10-01/hiss-async-capture/alsa-null.conf`
//! with e.g. `ALIM_E2E_DEVICE='SOTF Audit Null'`.

// Rust guideline compliant 2026-02-21
use sotf_audio::engine::{AudioEngine, AudioEngineState, PlaybackState};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Poll interval between state reads: sleeps pace observation only, and no
/// audio claim depends on elapsed time. Dense enough to oversample the
/// 100 ms meter snapshot cadence manyfold, but polls can still miss
/// drain-boundary windows — so poll maxima stay diagnostic only and
/// verdicts use the latched epoch peak instead.
pub const POLL_STEP: Duration = Duration::from_millis(2);
/// Liveness guardrail for every wait: a stuck engine fails loudly instead
/// of hanging the suite. Never an audio-timing assertion.
pub const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// Resolve the required null backend, or print a loud skip and return `None`.
pub fn null_device(var: &str, lane: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(name) if !name.trim().is_empty() => Some(name),
        _ => {
            eprintln!(
                "Skipping {lane} manager test: {var} is unset. Set {var}='SOTF Audit Null' \
                 with the audit ALSA null config to run the real-manager consumer gate."
            );
            None
        }
    }
}

/// Require the named device to be CPAL-visible; fail loudly otherwise.
pub fn require_device_visible(name: &str) {
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
        "device '{name}' is not CPAL-visible; found devices: {found:?}"
    );
}

/// Write a genuine HOT stereo fixture: 0.75 at 440 Hz plus 0.15 at 1 kHz
/// per channel, peaking near 0.9 so a -12 dB ceiling is really exercised.
pub fn write_hot_wav(path: &std::path::Path, frames: usize, rate: u32, channels: usize) {
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav writer must open");
    for frame in 0..frames {
        let time = frame as f32 / rate as f32;
        let left = (time * 440.0 * std::f32::consts::TAU).sin() * 0.75
            + (time * 1000.0 * std::f32::consts::TAU).sin() * 0.15;
        let right = (time * 660.0 * std::f32::consts::TAU).sin() * 0.75
            + (time * 1000.0 * std::f32::consts::TAU).sin() * 0.15;
        for sample in [left, right] {
            let quantized = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(quantized)
                .expect("wav sample must write");
        }
    }
    writer.finalize().expect("wav must finalize");
}

/// Read back the fixture and prove it is hot nonzero audio (a ceiling test
/// on a quiet fixture would be vacuous).
pub fn assert_hot_source(path: &std::path::Path, frames: usize, rate: u32, channels: usize) {
    let mut reader = hound::WavReader::open(path).expect("wav must reopen");
    let spec = reader.spec();
    assert_eq!(spec.channels, channels as u16);
    assert_eq!(spec.sample_rate, rate);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("wav sample must decode")) / f32::from(i16::MAX))
        .collect();
    assert_eq!(samples.len(), frames * channels);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        peak > 0.7,
        "fixture peak too low to exercise a ceiling: {peak}"
    );
}

pub fn make_hot_source(
    seconds: f32,
    rate: u32,
    channels: usize,
) -> (std::path::PathBuf, tempfile::NamedTempFile) {
    let temp = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .expect("temp wav must open");
    let path = temp.path().to_path_buf();
    let frames = (seconds * rate as f32) as usize;
    write_hot_wav(&path, frames, rate, channels);
    assert_hot_source(&path, frames, rate, channels);
    (path, temp)
}

/// Write a genuine SILENT stereo fixture: all zeros, so a direct-play
/// isolation leg contributes nothing of its own (any latched content is
/// provably pre-Flush tail, bounded by the previous chain's ceiling).
pub fn write_silent_wav(path: &std::path::Path, frames: usize, rate: u32, channels: usize) {
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav writer must open");
    for _ in 0..frames {
        for _ in 0..channels {
            writer.write_sample(0i16).expect("wav sample must write");
        }
    }
    writer.finalize().expect("wav must finalize");
}

/// Read back the fixture and prove it is exact digital silence.
pub fn assert_silent_source(path: &std::path::Path, frames: usize, rate: u32, channels: usize) {
    let mut reader = hound::WavReader::open(path).expect("wav must reopen");
    let spec = reader.spec();
    assert_eq!(spec.channels, channels as u16);
    assert_eq!(spec.sample_rate, rate);
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .map(|sample| sample.expect("wav sample must decode"))
        .collect();
    assert_eq!(samples.len(), frames * channels);
    assert!(samples.iter().all(|sample| *sample == 0));
}

pub fn make_silent_source(
    seconds: f32,
    rate: u32,
    channels: usize,
) -> (std::path::PathBuf, tempfile::NamedTempFile) {
    let temp = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .expect("temp wav must open");
    let path = temp.path().to_path_buf();
    let frames = (seconds * rate as f32) as usize;
    write_silent_wav(&path, frames, rate, channels);
    assert_silent_source(&path, frames, rate, channels);
    (path, temp)
}

/// Tracks the maximum output meter peak observed across polls.
///
/// Diagnostic only: polls can miss drain-boundary windows, so verdicts use
/// the latched epoch peak on the final state instead.
#[derive(Debug, Default)]
pub struct PeakTracker {
    pub max: f32,
}

impl PeakTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, engine: &AudioEngine) -> AudioEngineState {
        let state = engine.get_state();
        assert!(
            state.output_peak_linear.is_finite(),
            "meter must stay finite: {:?}",
            state.output_peak_linear
        );
        self.max = self.max.max(state.output_peak_linear);
        state
    }
}

/// Await real playback start: receipt of the target epoch.
///
/// The epoch is a LEVEL, not a transient flag: it persists from the acked
/// Play until the next Play, so awaiting it cannot miss a fast epoch the
/// way polling a transient Playing flag can. A full EOF before the first
/// poll still presents the target epoch and completes downstream. Fails
/// fast if the engine already moved past the target (caller bug).
///
/// Decide whether a polled state witnesses the target epoch started.
///
/// Pure decision table behind [`await_started`]: the epoch level persists
/// from the acked Play, so a first poll that already shows EOF still
/// witnesses the start. Tested deterministically in the running suite.
pub fn epoch_started(state: &AudioEngineState, target_epoch: u64) -> bool {
    state.playback_epoch == target_epoch
}

/// Decide whether a polled state completes the awaited epoch.
///
/// Pure decision table behind [`await_eof`]: the epoch must match (stale
/// epochs never complete) and the transport must read Stopped, so an
/// epoch that EOFs before the first poll completes immediately.
pub fn epoch_eof(state: &AudioEngineState, target_epoch: u64) -> bool {
    state.playback_epoch == target_epoch && state.playback_state == PlaybackState::Stopped
}

/// Returns the witnessed target-epoch state for `await_eof` attribution
/// (receipt only). Advance baselines come from the pre-Play read (see
/// `force_replay`): the witnessed state may already show EOF.
pub fn await_started(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    target_epoch: u64,
) -> AudioEngineState {
    let start = Instant::now();
    loop {
        let state = peaks.observe(engine);
        assert!(
            state.playback_epoch <= target_epoch,
            "epoch {target_epoch} already passed (at {})",
            state.playback_epoch
        );
        if epoch_started(&state, target_epoch) {
            return state;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "playback never started epoch {target_epoch}: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Await the published chain latency at startup (engine still Stopped).
pub fn await_startup_latency(engine: &AudioEngine, latency_samples: usize) {
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

/// Await a chain latency value mid-run (any playback state), e.g. after a
/// reconstructing chain update.
pub fn await_latency_value(engine: &AudioEngine, peaks: &mut PeakTracker, latency_samples: usize) {
    let start = Instant::now();
    loop {
        let state = peaks.observe(engine);
        if state.plugin_latency_samples == latency_samples && state.last_error.is_none() {
            return;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "latency never settled at {latency_samples}: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

/// Replay the fixture from zero unconditionally, returning baseline and start.
///
/// Stops unless already stopped, plays, and waits for the new epoch
/// receipt (all transport commands are acked). Unconditional by design:
/// there is no check-then-act on playback state, so no race between the
/// check and the phase. Phases call this before every observation attempt.
/// Start polls feed `peaks` so the poll diagnostic covers the whole epoch.
///
/// Returns the post-stop/pre-play baseline plus the witnessed
/// target-epoch start: pass the start to `await_eof` for attribution and
/// assert counter advance of the EOF state past the baseline. The
/// baseline read sits after the acked stop, so even an epoch that EOFs
/// before the first poll shows advance past it.
///
/// # Panics
///
/// Panics on transport command errors or when the liveness deadline expires.
pub fn force_replay(
    engine: &AudioEngine,
    wav_path: &std::path::Path,
    peaks: &mut PeakTracker,
) -> (AudioEngineState, AudioEngineState) {
    let before = engine.get_state();
    if before.playback_state != PlaybackState::Stopped {
        engine.stop().expect("replay stop must succeed");
    }
    let baseline = engine.get_state();
    engine
        .play(wav_path.to_path_buf())
        .expect("replay must start");
    let started = await_started(engine, peaks, before.playback_epoch.wrapping_add(1));
    (baseline, started)
}

/// Print one attributed phase observation for the gate log: the epoch
/// (receipt evidence), the latched epoch peak (verdict source) plus the
/// poll maximum (diagnostic), frame counters, and the epoch wall time.
pub fn log_phase(tag: &str, peaks: &PeakTracker, state: &AudioEngineState, epoch_wall: Duration) {
    eprintln!(
        "phase {tag}: epoch={epoch} latch_max={latch:.8} peak_max={max:.8} chunks_written={chunks} callbacks={callbacks} position={position:.3}s latency={latency} bypassed={bypassed} error={error:?} epoch_wall_ms={wall}",
        epoch = state.playback_epoch,
        latch = state.playback_peak_max_linear,
        max = peaks.max,
        chunks = state.playback_frames_written,
        callbacks = state.playback_callback_count,
        position = state.position,
        latency = state.plugin_latency_samples,
        bypassed = state.processing_bypassed,
        error = state.last_error,
        wall = epoch_wall.as_millis(),
    );
}

/// Await natural end-of-stream for the epoch witnessed by `started`.
///
/// `started` must be the target-epoch state returned by the immediately
/// preceding start wait (`await_started`, directly or via `force_replay`).
/// An entry check rejects a stale `started` deterministically (the test is
/// single-threaded: nothing else advances the epoch), and the gate polls
/// a durable level (`epoch == target && Stopped`), so an epoch that EOFs
/// between the two waits still completes instead of spinning on Stopped
/// to the deadline. Freshness is enforced downstream by asserting counter
/// advance past the pre-Play baselines; a stale return shows zero advance
/// and fails loudly. Peak verdicts use the latched epoch peak on the
/// returned state; the tracker maximum stays diagnostic only.
pub fn await_eof(
    engine: &AudioEngine,
    peaks: &mut PeakTracker,
    started: &AudioEngineState,
) -> AudioEngineState {
    let target = started.playback_epoch;
    assert_eq!(
        engine.get_state().playback_epoch,
        target,
        "await_eof called for epoch {target} but the engine already moved on"
    );
    let start = Instant::now();
    loop {
        let state = peaks.observe(engine);
        if epoch_eof(&state, target) {
            return state;
        }
        assert!(
            start.elapsed() < POLL_DEADLINE,
            "natural EOF never reached for epoch {target}: {state:?}"
        );
        std::thread::sleep(POLL_STEP);
    }
}

// ---------------------------------------------------------------------------
// FIFO drip-feed barrier for deterministic mid-stream Stop legs.
// ---------------------------------------------------------------------------

/// Amplitude of the analytic stop-leg tone, linear.
///
/// 0.99 drives the post-color signal above the -12 dB ceiling within the
/// first periods (the same chain clamped the 0.9 two-tone program in the
/// R20 gates), so the latch gate fills fast; under 1.0 so pre-clamp
/// metering never counts a clip. The bypass path applies no gain, so no
/// bypassed sample can overshoot past this. Amplitude affects only gate
/// latency, never soundness: the gate observes an actual clamped/peak
/// sample in the latch before Stop, which is the proof — not the number.
pub const TONE_AMPLITUDE: f32 = 0.99;
/// Bytes per stereo 16-bit frame in the stop-leg fixture.
const TONE_BYTES_PER_FRAME: u64 = 4;
/// Header length of the hound-written PCM16 WAV fixture, verified by
/// assert on the RIFF/fmt/data markers at feeder start: hound layout
/// drift fails loud there instead of silently misaligning the feeder's
/// payload accounting.
const TONE_WAV_HEADER_LEN: usize = 44;
/// First drip after the header: 0.5 s of audio, satisfying the decoder
/// open/probe plus the first decode burst before play-ack returns.
const FEEDER_FIRST_DRIP_FRAMES: u64 = 24_000;
/// Steady drip: 0.1 s of audio per interval (~10x realtime supply against
/// a ~56x drain, so starvation gaps exist but the latch gate still fills
/// in well under a second).
const FEEDER_DRIP_FRAMES: u64 = 4_800;
/// Drip pacing: the decoder blocks in a symphonia read for at most about
/// one interval between drips, two orders of magnitude under the 1000 ms
/// decoder-command timeout the Stop path waits on — so Stop's decoder ack
/// always lands during the drip phase under any sane scheduling (a
/// pathological stall fails loud at the ack wait, never false-passes).
const FEEDER_DRIP_INTERVAL: Duration = Duration::from_millis(10);
/// Hard supply cap: 5 s of the 30 s program. Past the cap the feeder
/// holds the pipe open without writing, so supplied stays a deterministic
/// integer fact (<= cap < total) whatever the wall clock does — this cap,
/// not timing, is the mid-stream remainder proof.
const FEEDER_SUPPLY_CAP_FRAMES: u64 = 240_000;

/// Write an analytic stereo tone fixture: fs/4 sine per channel.
///
/// Left is `[0, +A, 0, -A]` repeating (sine phase) and right is
/// `[+A, 0, -A, 0]` (cosine phase) with `A = TONE_AMPLITUDE`: exactly
/// what a sample-rate/4 tone is under sampling, written bit-exactly with
/// no `sin()` calls, so every peak sample carries identical bits. Any
/// consumed prefix of two or more samples contains a peak sample, which
/// makes the byte oracle ([`wav_peak_from_bytes`]) exact for every Stop
/// position. Audible real audio (12 kHz at 48 kHz), hot enough to clamp.
///
/// # Panics
///
/// Panics when `channels` is not 2 (the stereo phase design is fixed) or
/// when the WAV writer fails.
pub fn write_tone_wav(path: &std::path::Path, frames: usize, rate: u32, channels: usize) {
    assert_eq!(channels, 2, "tone fixture is stereo by design");
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav writer must open");
    for frame in 0..frames {
        let left = match frame % 4 {
            0 => 0.0,
            1 => TONE_AMPLITUDE,
            2 => 0.0,
            _ => -TONE_AMPLITUDE,
        };
        let right = match frame % 4 {
            0 => TONE_AMPLITUDE,
            1 => 0.0,
            2 => -TONE_AMPLITUDE,
            _ => 0.0,
        };
        for sample in [left, right] {
            let quantized = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(quantized)
                .expect("wav sample must write");
        }
    }
    writer.finalize().expect("wav must finalize");
}

/// Build the analytic stop-leg tone program on disk.
///
/// Stereo PCM16 tone via [`write_tone_wav`] plus the hot check. The
/// engine never opens this file (it plays the FIFO); the file is only the
/// byte buffer the feeder drips and the oracle parses.
pub fn make_tone_source(
    seconds: f32,
    rate: u32,
    channels: usize,
) -> (std::path::PathBuf, tempfile::NamedTempFile) {
    let temp = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .expect("temp wav must open");
    let path = temp.path().to_path_buf();
    let frames = (seconds * rate as f32) as usize;
    write_tone_wav(&path, frames, rate, channels);
    assert_hot_source(&path, frames, rate, channels);
    (path, temp)
}

/// Peak oracle over raw WAV bytes, matching production decoding exactly.
///
/// Reopens `bytes` with hound, asserts the PCM16 spec, and returns
/// `max|i16| as f32 / 32768.0` — the same divisor the symphonia S16 arm
/// uses (`i as f32 / 32768.0`), NOT the 32767 of the test-side hot check
/// (a floor check, never an exact oracle). The engine path (symphonia
/// decode, bit-exact bypass copy, unity volume multiply, pre-clamp meter,
/// max-fold latch) reproduces these exact bits, so the oracle is the
/// independent upper bound AND — once the latch gate has observed it —
/// the exact expected terminal.
///
/// # Panics
///
/// Panics when the bytes do not parse as the expected WAV spec or contain
/// no samples.
pub fn wav_peak_from_bytes(bytes: &[u8], rate: u32, channels: usize) -> f32 {
    let cursor = std::io::Cursor::new(bytes);
    let mut reader = hound::WavReader::new(cursor).expect("wav bytes must parse");
    let spec = reader.spec();
    assert_eq!(spec.channels, channels as u16);
    assert_eq!(spec.sample_rate, rate);
    assert_eq!(spec.bits_per_sample, 16);
    let max_abs: i32 = reader
        .samples::<i16>()
        .map(|sample| i32::from(sample.expect("wav sample must decode")).abs())
        .max()
        .expect("wav must contain samples");
    max_abs as f32 / 32768.0
}

/// Final byte accounting from a released feeder.
#[derive(Debug)]
pub struct FeederReport {
    /// Payload frames written to the FIFO (header excluded).
    pub supplied_payload_frames: u64,
    /// Payload frames in the full program (header excluded).
    pub total_payload_frames: u64,
    /// The hard supply cap (strictly below the total for stop legs).
    pub cap_frames: u64,
    /// True when a FIFO write failed with a NON-EPIPE error (genuine I/O
    /// pathology — EPIPE is the expected teardown class and breaks the
    /// loop cleanly instead): the leg must fail, never trust the counts.
    pub write_failed: bool,
}

/// Byte-accounted FIFO drip-feed: the deterministic mid-stream barrier.
///
/// The engine plays the program through a named pipe the feeder drips at
/// ~10x realtime with a hard supply cap. Mid-stream is then a matter of
/// physics, not timing: the decoder cannot emit unsupplied bytes, EOF is
/// impossible before release (the write ends stay open), and the cap
/// makes supplied <= cap < total an integer fact — so an acked Stop with
/// `!write_failed` provably lands mid-stream. The decoder stays
/// responsive throughout: it blocks in a read for ~one drip interval at
/// most, far under the Stop path's decoder-ack timeout.
///
/// Lifecycle (termination-safe by structure, not by timing): the main
/// thread retains an O_RDWR read end until teardown, so writes never
/// EPIPE mid-leg (a reader always exists); the write end is nonblocking,
/// so the drip thread never blocks in a write under ANY reader behavior
/// — sleeping, WouldBlock, partial write, cap-idle, decoder-never-opened,
/// even a hung decoder holding the read end open — and exits within ~one
/// drip interval of the release signal on every path, including panics.
/// Release order is signal → drop the read end → join, which additionally
/// EPIPE-unblocks the racy post-signal write; EPIPE anywhere breaks the
/// loop cleanly (expected teardown class — readers gone by design), while
/// any other write error sets `write_failed` (genuine pathology, the leg
/// must fail). On panic paths [`Drop`] follows the same order without
/// propagating; closing the write ends EOF-unblocks a decoder stuck in a
/// FIFO read, so engine teardown cannot hang behind the feeder either.
///
/// Requires the WAV single-open path (channels in codec params — true
/// for PCM): the decoder is the sole consuming reader, opened once.
pub struct FifoFeeder {
    fifo_path: std::path::PathBuf,
    total_payload_frames: u64,
    supplied_payload_frames: Arc<AtomicU64>,
    write_failed: Arc<AtomicBool>,
    read_end: Option<std::fs::File>,
    ready_rx: mpsc::Receiver<()>,
    release_tx: Option<mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for FifoFeeder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FifoFeeder")
            .field("fifo_path", &self.fifo_path)
            .field(
                "supplied_payload_frames",
                &self.supplied_payload_frames.load(Ordering::Relaxed),
            )
            .field("total_payload_frames", &self.total_payload_frames)
            .finish_non_exhaustive()
    }
}

impl FifoFeeder {
    /// Start dripping `bytes` through a fresh FIFO.
    ///
    /// Creates the pipe, opens the write end (the open dance below never
    /// blocks), writes the 44-byte header (always fits the empty pipe),
    /// and spawns the drip thread; the first 0.5 s drip lands as the
    /// decoder opens and reads — that rendezvous is by design, paced by
    /// WouldBlock rather than blocking. The tempdir lives in the thread,
    /// so the FIFO cleans itself on exit.
    ///
    /// # Panics
    ///
    /// Panics when `file_name` lacks the `.wav` extension (the decoder
    /// routes by extension before probing), when the WAV layout is not
    /// the minimal 44-byte hound header, when the program does not exceed
    /// the supply cap, or when pipe/thread setup fails.
    pub fn start(bytes: Vec<u8>, file_name: &str) -> Self {
        assert!(
            file_name.ends_with(".wav"),
            "decoder routes by extension; FIFO must look like a WAV"
        );
        let tempdir = tempfile::tempdir().expect("feeder tempdir must open");
        let fifo_path = tempdir.path().join(file_name);
        create_fifo(&fifo_path);
        let header_len = wav_header_len(&bytes);
        let total_payload_frames = (bytes.len() - header_len) as u64 / TONE_BYTES_PER_FRAME;
        assert!(
            total_payload_frames > FEEDER_SUPPLY_CAP_FRAMES,
            "stop-leg program ({total_payload_frames} frames) must exceed the supply cap"
        );
        // FIFO open dance: O_RDWR never blocks; the O_WRONLY handle then
        // opens against our own read end. The RDWR handle is RETAINED in
        // the struct until teardown (dropping it here EPIPEs the very
        // first write — zero readers before the decoder opens — which is
        // exactly the R22 failure): its presence guarantees a reader
        // mid-leg, and dropping it at release unblocks the racy
        // post-signal write with EPIPE.
        let read_end = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo_path)
            .expect("fifo RDWR open must succeed");
        // Nonblocking from the first byte (`O_NONBLOCK` at open): the
        // drip thread must never block in a write under any reader
        // behavior (pipe full with a live-but-unreading reader would
        // otherwise hang the join). WouldBlock paces the loop; partial
        // writes advance exactly.
        let mut write = open_write_end(&fifo_path);
        write
            .write_all(&bytes[..header_len])
            .expect("fifo header must write");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let supplied_payload_frames = Arc::new(AtomicU64::new(0));
        let write_failed = Arc::new(AtomicBool::new(false));
        let handle = std::thread::Builder::new()
            .name("fifo-feeder".to_string())
            .spawn({
                let supplied_payload_frames = Arc::clone(&supplied_payload_frames);
                let write_failed = Arc::clone(&write_failed);
                move || {
                    // Tempdir owned here: the FIFO cleans itself on exit.
                    let _tempdir = tempdir;
                    feeder_loop(
                        write,
                        bytes,
                        header_len,
                        ready_tx,
                        release_rx,
                        supplied_payload_frames,
                        write_failed,
                    );
                }
            })
            .expect("feeder thread must spawn");
        Self {
            fifo_path,
            total_payload_frames,
            supplied_payload_frames,
            write_failed,
            read_end: Some(read_end),
            ready_rx,
            release_tx: Some(release_tx),
            handle: Some(handle),
        }
    }

    /// Path the engine plays (the FIFO).
    pub fn path(&self) -> &std::path::Path {
        &self.fifo_path
    }

    /// Whether a NON-EPIPE write error tripped (genuine I/O pathology).
    ///
    /// EPIPE — the expected class when readers are gone by design
    /// (post-stop release, decoder-never-opened teardown) — breaks the
    /// loop cleanly and never sets this, so both this pre-stop read and
    /// the post-release report assert are flake-free: only real
    /// pathology trips them, whenever in the leg it strikes.
    pub fn write_failed(&self) -> bool {
        self.write_failed.load(Ordering::Relaxed)
    }

    /// Wait for the feeder thread to come alive (fail-loud deadline).
    ///
    /// Ready means the header is already in the pipe and the thread is
    /// writing the first drip; the decoder rendezvous needs no further
    /// synchronization (the decoder's blocking reads pair up with the
    /// drip writes on their own).
    pub fn await_ready(&self) {
        self.ready_rx
            .recv_timeout(POLL_DEADLINE)
            .expect("feeder thread never signaled ready");
    }

    /// Release the feeder: close the pipe and join the thread.
    ///
    /// Order is signal → drop the read end → join: the signal breaks the
    /// loop at its next check, dropping the read end EPIPE-unblocks the
    /// racy post-signal write (decoder read end already closed by the
    /// acked Stop), and the join is bounded (~one drip interval — the
    /// nonblocking thread never blocks in a write under any reader
    /// behavior). Returns the final byte accounting.
    pub fn release(mut self) -> FeederReport {
        if let Some(tx) = self.release_tx.take() {
            tx.send(()).ok();
        }
        drop(self.read_end.take());
        if let Some(handle) = self.handle.take() {
            handle.join().expect("feeder thread panicked");
        }
        FeederReport {
            supplied_payload_frames: self.supplied_payload_frames.load(Ordering::Relaxed),
            total_payload_frames: self.total_payload_frames,
            cap_frames: FEEDER_SUPPLY_CAP_FRAMES,
            write_failed: self.write_failed.load(Ordering::Relaxed),
        }
    }
}

impl Drop for FifoFeeder {
    /// Best-effort release on panic paths: signal, drop the read end,
    /// and join without propagating (a `Drop` must never panic into an
    /// unwind). The join is bounded on every path — sleeping, WouldBlock,
    /// partial write, cap-idle, decoder-never-opened, even a hung decoder
    /// holding the read end open — because the thread never blocks in a
    /// write. Closing the write ends EOF-unblocks a decoder stuck in a
    /// FIFO read, so engine teardown cannot hang behind the feeder
    /// either.
    fn drop(&mut self) {
        if let Some(tx) = self.release_tx.take() {
            tx.send(()).ok();
        }
        drop(self.read_end.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Verify the hound PCM16 layout and return the header length.
///
/// Asserts the RIFF/WAVE/fmt/data markers of the minimal 44-byte header:
/// a layout drift fails loud here instead of silently misaligning the
/// feeder's payload accounting.
fn wav_header_len(bytes: &[u8]) -> usize {
    assert!(
        bytes.len() > TONE_WAV_HEADER_LEN,
        "wav bytes shorter than the header"
    );
    assert_eq!(&bytes[0..4], b"RIFF", "wav must start with RIFF");
    assert_eq!(&bytes[8..12], b"WAVE", "wav must declare WAVE");
    assert_eq!(&bytes[12..16], b"fmt ", "wav must declare fmt");
    assert_eq!(&bytes[36..40], b"data", "wav data chunk must sit at 36");
    TONE_WAV_HEADER_LEN
}

/// Drip the program into the pipe until released, capped, or failed.
///
/// The write end is nonblocking, so no iteration here can block: each
/// tick offers up to one drip, the kernel accepts what fits, and the
/// offset advances by exactly the accepted bytes — partial writes stay
/// byte-exact (no duplication, no skip; the undelivered remainder slides
/// into later ticks' offers by contiguity). WouldBlock (pipe full) simply
/// paces to the next tick, preserving decoder backpressure. The first
/// drip is large (decoder open/probe burst, spread over ticks as the pipe
/// drains); steady drips pace at the drip interval. Past the cap — or
/// past the program end, which is unreachable since the cap sits strictly
/// below the total — the loop holds the pipe open without writing (EOF
/// stays impossible). EPIPE breaks the loop cleanly (expected teardown
/// class — readers gone by design, e.g. the post-signal write after the
/// read end drops); any OTHER write error flags `write_failed` (genuine
/// pathology — the leg must fail, never trust the counts). The `write`
/// handle drops on exit, closing the write end so a still-reading
/// decoder observes EOF.
fn feeder_loop(
    mut write: std::fs::File,
    bytes: Vec<u8>,
    header_len: usize,
    ready_tx: mpsc::Sender<()>,
    release_rx: mpsc::Receiver<()>,
    supplied_payload_frames: Arc<AtomicU64>,
    write_failed: Arc<AtomicBool>,
) {
    use std::io::ErrorKind;
    ready_tx.send(()).ok();
    let cap_end = header_len + (FEEDER_SUPPLY_CAP_FRAMES * TONE_BYTES_PER_FRAME) as usize;
    let cap_end = cap_end.min(bytes.len());
    let mut offset = header_len;
    let mut next_drip = (FEEDER_FIRST_DRIP_FRAMES * TONE_BYTES_PER_FRAME) as usize;
    let steady_drip = (FEEDER_DRIP_FRAMES * TONE_BYTES_PER_FRAME) as usize;
    loop {
        match release_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if offset >= cap_end {
            std::thread::sleep(FEEDER_DRIP_INTERVAL);
            continue;
        }
        let end = (offset + next_drip).min(cap_end);
        next_drip = steady_drip;
        match write.write(&bytes[offset..end]) {
            Ok(accepted) => {
                offset += accepted;
                supplied_payload_frames.store(
                    (offset - header_len) as u64 / TONE_BYTES_PER_FRAME,
                    Ordering::Relaxed,
                );
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == ErrorKind::BrokenPipe => break,
            Err(_) => {
                write_failed.store(true, Ordering::Relaxed);
                break;
            }
        }
        std::thread::sleep(FEEDER_DRIP_INTERVAL);
    }
}

/// Create the FIFO at `path` (unix) or fail loud (non-unix).
///
/// The ALSA null gate runs on Linux, so the non-unix arm is unreachable
/// in practice (the device check returns or panics first); it exists so
/// the suite compiles everywhere.
#[cfg(unix)]
fn create_fifo(path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .expect("fifo path must not contain NUL");
    // SAFETY: `libc::mkfifo` with a valid NUL-terminated path pointer and
    // a plain mode argument performs no memory access beyond reading the
    // path; the pointer borrows `c_path`, alive for the call. The return
    // value is checked below; errno is read immediately on failure.
    let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644 as libc::mode_t) };
    assert_eq!(
        result,
        0,
        "mkfifo failed for {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

#[cfg(not(unix))]
fn create_fifo(_path: &std::path::Path) {
    panic!("FIFO-backed stop legs require a unix host");
}

/// Open the FIFO write end with `O_NONBLOCK` (unix) or fail loud.
///
/// `std::fs::File` has no `set_nonblocking` (sockets only), so the flag
/// rides `OpenOptionsExt::custom_flags` at open — ORed with the write
/// access mode by the standard library. Opening against our own retained
/// read end, so the open itself never blocks. Same reachability note as
/// [`create_fifo`]: the non-unix arm only keeps the suite compiling.
#[cfg(unix)]
fn open_write_end(path: &std::path::Path) -> std::fs::File {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .expect("fifo write open must succeed")
}

#[cfg(not(unix))]
fn open_write_end(_path: &std::path::Path) -> std::fs::File {
    panic!("FIFO-backed stop legs require a unix host");
}
