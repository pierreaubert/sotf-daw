//! Declick C ABI consumer tests: nine-control mapping, repaired and
//! residual audio, exact EOS through the exported drain, structural-vs-live
//! parameter sets, save/reload continuation, and preset documents.
//!
//! Every test drives the public C ABI exclusively: construction, parameter
//! enumeration, normalized and document state, chunked processing, and
//! finalization through the exported `plugin_drain` (added alongside these
//! tests; the C ABI previously had no drain operation). No test reaches the
//! underlying Rust plugin — not for audio, produced counts, completion
//! flags, latency, typed values, or partitioning metadata. Process
//! partitions are sized by adaptive public probing (oversized blocks fail
//! cleanly without consuming state); drain capacity is sized from the
//! exported capacity query; typed values are read back from saved state
//! documents. Poisoned buffers prove the implementation writes exactly the
//! produced prefix and nothing on failure paths.
//!
//! Oracles reuse the frozen declick contract accepted in
//! `declick-engine-integration/review-r3.md`: 5%-of-amplitude repair error
//! at click frames, 0.05 absolute damage outside the widened repair
//! footprint, recall/precision at least 0.95 over settled frames, exact
//! leading silence, 1e-5 residual regrouping, legacy bit identity, and
//! exact `(frames + produced) * channels` lengths. Expected latency is
//! derived from the known typed controls (8 + repair_width) and checked
//! against the advertised info value; drain capacity always comes from the
//! exported capacity query, never from Rust internals.
//!
//! Boundary-domain note (full-context convention, honestly sourced): the
//! detector post window is 8 frames, so inputs in the last 8 frames of a
//! finite stream see drain zeros as future context. The scoping precedent
//! is the clean-controls/bit-exact disposition (declick lane fix-r2,
//! failure 1: the shared legacy suite leaves the last 8 frames unpinned)
//! plus the all-combo flush-boundary regression — NOT the click-stream
//! suites: accepted A1 asserts 0.05 damage through input 1023 on its own
//! fixtures and passes. These legs therefore do not claim the boundary
//! exclusion is universally accepted; they rest it on mechanism identity
//! with failure 1 (a zero-dominated post window marginally triggers and
//! repairs toward the median baseline, fixture-phase-dependent), frozen
//! outputs (every sample still asserted, exact), and no fitted numbers.
//! Legs that assert interior bounds on every input render a natural
//! 8-frame tone continuation past the asserted span, so every asserted
//! frame enjoys full post context; continuation
//! and other click-free boundary outputs are pinned to the fixture
//! programme peak (0.25: dry tone and median baselines stay inside it by
//! construction); confinement legs prove pre-boundary output is
//! bit-identical with and without continuation; and true boundary clicks
//! (no continuation) pin spike removal plus a derived repair peak (half
//! the tone peak: the zero post-median halves the clean pre-median — the
//! shared legacy suppressor and the owned core use the identical emission
//! formula, so the derivation holds for both fullband routings).
//! Multiband boundary legs pin removal plus conservative magnitude peaks
//! (2-band 1.0, 3-band 2.0; see the `MULTIBAND_BOUNDARY_PEAK_*` constants)
//! as a blowup/NaN/wild-value exclusion alongside finiteness, geometry,
//! full-context damage, and click-free boundary peaks. Accuracy at the
//! contract-defined zero-continuation endpoint is proven separately by the
//! endpoint error oracle (`declick_ffi_multiband_eof_endpoint_error` and
//! the periodic lock leg): corrupted-vs-clean-plugin differential error
//! (<0.15 at clicks, <0.05 on clean outside the footprint), clean-plugin
//! vs analytical delayed-clean error (<0.05, the independent
//! false-positive measure), and corrupted vs analytical end-to-end error
//! (<0.15/<0.05). Continued legs restore full context and carry the frozen
//! 5%/0.05 bounds there. Every sample of every render is asserted;
//! nothing is deleted to pass.
//!
//! Multi-rate/channel legs assert routing, geometry, and structural
//! regrouping only; per-rate accuracy numbers stay with the DSP lane.

// Rust guideline compliant 2026-02-21

use crate::{
    PluginError, PluginHandle, plugin_create, plugin_destroy, plugin_drain,
    plugin_export_preset_json, plugin_free_state, plugin_free_string,
    plugin_get_drain_capacity_frames, plugin_get_info_json, plugin_get_last_error,
    plugin_get_parameter, plugin_get_parameter_choice_label, plugin_get_parameter_count,
    plugin_get_parameter_info, plugin_import_preset_json, plugin_load_state, plugin_process,
    plugin_reset, plugin_save_state, plugin_set_parameter,
};
use std::ffi::{CStr, CString};

/// C ABI fixture rate for full-oracle legs (DSP-proven territory).
const RATE_48K: u32 = 48_000;
/// Fixture frames per render (click plans below must fit).
const FRAMES: usize = 1024;
/// Stereo fixture width for full-oracle legs.
const CHANNELS: usize = 2;
/// Widened repair span used by the non-default legs.
const WIDTH: usize = 3;
/// Owned-path latency derived from the known width (8 + repair_width).
const LATENCY: usize = 8 + WIDTH;
/// Legacy neutral-path latency (zero width, random mode, fullband).
const LEGACY_LATENCY: usize = 8;
/// Synthetic click amplitude added onto the fixture tones.
const CLICK_AMP: f32 = 3.0;
/// Residual magnitude counted as a hot detection.
const HOT_RESIDUAL: f32 = 0.5;
/// First settled input frame for value assertions (startup context).
const SETTLED_FROM: usize = 32;
/// Fixture tone peak amplitude (see `tone`).
const TONE_PEAK: f32 = 0.25;
/// Flush-boundary depth: the detector post window. Inputs at or past
/// `frames - POST_CONTEXT` see drain zeros in their post window; the legs
/// assert this against the DSP `LOOKAHEAD_SAMPLES` const.
const POST_CONTEXT: usize = 8;
/// Derived repair peak for fullband boundary clicks: half the tone peak
/// plus float dust. The zero post-median halves the clean pre-median (see
/// module docs).
/// Bound derivation (fullband, either routing — the shared legacy
/// suppressor and the owned core use the identical formula): emission is
/// the baseline `0.5 * (pre_median + post_median)`. At the asserted click
/// frames the pre window holds at most one click sample and the post
/// window at most one (the first frame of the double-wide click sees the
/// second ahead in its post window; the second sees the first behind in
/// its pre window); a post window
/// of one click plus seven zeros still medians to exactly 0 (sorted middle
/// order statistics, indices 3 and 4 of 8, are both 0), and both pre middle
/// order statistics are clean tone within `[-TONE_PEAK, TONE_PEAK]`. Hence
/// `|repaired| <= TONE_PEAK / 2` up to float rounding.
const BOUNDARY_REPAIR_PEAK: f32 = TONE_PEAK / 2.0 + 1.0e-6;
/// Programme peak for click-free boundary outputs: the tone peak plus
/// float dust. Dry tone stays inside it, and so does any median baseline
/// over windows holding only tone, zeros, and (below) at most two clicks.
const BOUNDARY_PROGRAMME_PEAK: f32 = TONE_PEAK + 1.0e-6;
/// Blowup-exclusion peak for 2-band boundary outputs (click + click-free).
///
/// Conservative hull (measured worsts sit well inside); values inside prove
/// no wild extrapolation, but accuracy is proven by the endpoint error
/// oracle, not here. Removal (>1.0 from the 3.25 spike) is asserted
/// alongside.
const MULTIBAND_BOUNDARY_PEAK_2BAND: f32 = 1.0;
/// Blowup-exclusion peak for 3-band boundary outputs: 2.0.
///
/// Same role as the 2-band pin with headroom for the three-band cascade;
/// measured worsts sit well inside. Accuracy is proven by the endpoint
/// error oracle, not here. Removal still holds (3.25-2.0=1.25>1.0).
const MULTIBAND_BOUNDARY_PEAK_3BAND: f32 = 2.0;
/// First process stride tried by adaptive public probing.
const PROCESS_PROBE_START: usize = 256;

struct DeclickHandle {
    pointer: *mut PluginHandle,
    inputs: usize,
    outputs: usize,
}

impl DeclickHandle {
    fn create(config: &str, rate: u32, inputs: usize, outputs: usize) -> Self {
        Self::create_as("Declick", config, rate, inputs, outputs)
    }

    fn create_as(kind: &str, config: &str, rate: u32, inputs: usize, outputs: usize) -> Self {
        let kind = CString::new(kind).unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(kind.as_ptr(), config.as_ptr(), rate, inputs, outputs);
        assert!(!handle.is_null(), "construction failed: {}", last_error());
        Self {
            pointer: handle,
            inputs,
            outputs,
        }
    }

    fn param_count(&self) -> usize {
        plugin_get_parameter_count(self.pointer).max(0) as usize
    }

    fn param_id(&self, index: usize) -> String {
        let info = plugin_get_parameter_info(self.pointer, index);
        assert!(!info.is_null(), "parameter {index} info must exist");
        // SAFETY: Info borrows from this live handle.
        let id = unsafe { (*info).id };
        assert!(!id.is_null());
        // SAFETY: IDs are NUL-terminated while live.
        unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned()
    }

    fn choice_label(&self, index: usize, choice: usize) -> Option<String> {
        let label = plugin_get_parameter_choice_label(self.pointer, index, choice);
        if label.is_null() {
            return None;
        }
        // SAFETY: Choice labels are process-static NUL-terminated strings.
        Some(
            unsafe { CStr::from_ptr(label) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn set_normalized(&mut self, id: &str, value: f64) -> i32 {
        let id = CString::new(id).unwrap();
        plugin_set_parameter(self.pointer, id.as_ptr(), value)
    }

    fn get_normalized(&self, id: &str) -> f64 {
        let id = CString::new(id).unwrap();
        plugin_get_parameter(self.pointer, id.as_ptr())
    }

    fn info_latency(&self) -> usize {
        let info_pointer = plugin_get_info_json(self.pointer);
        assert!(!info_pointer.is_null());
        // SAFETY: The C API owns a valid NUL-terminated string until freed below.
        let info: serde_json::Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(info_pointer) }.to_bytes()).unwrap();
        plugin_free_string(info_pointer);
        info["latency_samples"].as_u64().unwrap() as usize
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
        assert_eq!(input.len() % self.inputs, 0, "complete frames required");
        let frames = input.len() / self.inputs;
        let mut output = vec![f32::NAN; frames * self.outputs];
        let mut start = 0;
        // Adaptive public probing: oversized blocks fail cleanly without
        // consuming state (the ABI silences and reports BufferTooSmall),
        // so halve until the negotiated bound accepts. No private maximum
        // is consulted anywhere in this module.
        let mut stride = PROCESS_PROBE_START;
        while start < frames {
            let count = stride.min(frames - start);
            let code = plugin_process(
                self.pointer,
                input[start * self.inputs..].as_ptr(),
                output[start * self.outputs..].as_mut_ptr(),
                count,
            );
            if code == 0 {
                start += count;
                stride = PROCESS_PROBE_START;
            } else if code == PluginError::BufferTooSmall as i32 && stride > 1 {
                stride /= 2;
            } else {
                panic!("process failed: {code} {}", last_error());
            }
        }
        assert!(output.iter().all(|v| v.is_finite()));
        output
    }

    fn process_strided(&mut self, input: &[f32], stride: usize) -> Vec<f32> {
        assert!(stride > 0, "positive stride required");
        assert_eq!(input.len() % self.inputs, 0, "complete frames required");
        let frames = input.len() / self.inputs;
        let mut output = vec![f32::NAN; frames * self.outputs];
        for start in (0..frames).step_by(stride) {
            let count = stride.min(frames - start);
            let code = plugin_process(
                self.pointer,
                input[start * self.inputs..].as_ptr(),
                output[start * self.outputs..].as_mut_ptr(),
                count,
            );
            assert_eq!(code, 0, "stride {stride}: {}", last_error());
        }
        assert!(output.iter().all(|v| v.is_finite()));
        output
    }

    /// One exported drain call. Returns (status, buffer, produced, complete).
    /// The buffer stays poisoned past the produced prefix on success so
    /// tests can prove only valid frames were written; out-pointers are
    /// poisoned to prove the call writes them on every writable path.
    fn drain_capacity(&self) -> usize {
        let mut capacity = usize::MAX;
        // SAFETY: The handle is live and exclusively held; `capacity` is a
        // valid local out-pointer that outlives the call.
        let code = unsafe { plugin_get_drain_capacity_frames(self.pointer, &mut capacity) };
        assert_eq!(code, 0, "capacity query failed: {}", last_error());
        assert_ne!(capacity, usize::MAX, "capacity unwritten");
        capacity
    }

    fn drain_once(&mut self, capacity_frames: usize) -> (i32, Vec<f32>, usize, bool) {
        let mut output = vec![f32::NAN; capacity_frames * self.outputs];
        let mut produced = usize::MAX;
        let mut complete: std::os::raw::c_int = -1;
        // SAFETY: The handle is live and exclusively held; the buffer holds
        // exactly `capacity_frames * outputs` samples and the locals are
        // valid out-pointers, all disjoint and outliving the call.
        let code = unsafe {
            plugin_drain(
                self.pointer,
                output.as_mut_ptr(),
                capacity_frames,
                &mut produced,
                &mut complete,
            )
        };
        assert_ne!(produced, usize::MAX, "produced unwritten");
        assert_ne!(complete, -1, "complete unwritten");
        assert!(
            produced <= capacity_frames,
            "produced {produced} exceeds capacity {capacity_frames}"
        );
        (code, output, produced, complete != 0)
    }

    /// Drain through the exported API to completion, collecting exactly the
    /// produced frames. Capacity is the caller's explicit sizing (legs pass
    /// the advertised info latency); Declick's declared call bound is one,
    /// so exactly one successful call must complete any drain started here.
    fn drain_to_eof(&mut self, capacity_frames: usize) -> Vec<f32> {
        let mut tail = Vec::new();
        let mut calls = 0;
        for _ in 0..8 {
            let (code, buffer, produced, complete) = self.drain_once(capacity_frames);
            assert_eq!(code, 0, "drain failed: {}", last_error());
            calls += 1;
            // Only the produced prefix is valid audio; the poisoned
            // remainder proves the implementation wrote exactly that much.
            assert!(
                buffer[produced * self.outputs..].iter().all(|v| v.is_nan()),
                "drain wrote past the produced prefix"
            );
            assert!(
                buffer[..produced * self.outputs]
                    .iter()
                    .all(|v| v.is_finite())
            );
            tail.extend_from_slice(&buffer[..produced * self.outputs]);
            if complete {
                assert_eq!(calls, 1, "Declick declares a drain call bound of one");
                return tail;
            }
        }
        panic!("drain did not complete");
    }

    fn process_with_drain(&mut self, input: &[f32]) -> Vec<f32> {
        // Drain capacity always comes from the exported capacity query —
        // the honest C-client sizing — never from Rust internals. (Oracle
        // delays in the legs still use the advertised info latency.)
        let capacity = self.drain_capacity();
        let mut full = self.process(input);
        full.extend_from_slice(&self.drain_to_eof(capacity));
        full
    }
}

impl Drop for DeclickHandle {
    fn drop(&mut self) {
        plugin_destroy(self.pointer);
    }
}

fn last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

/// Largest of {256, .., 1} the public over-max contract accepts, probed on
/// throwaway handles through `plugin_process` alone.
fn discover_stride() -> usize {
    for stride in [256, 128, 64, 32, 16, 8, 4, 2, 1] {
        let probe = DeclickHandle::create(&neutral_config(), RATE_48K, 2, 2);
        let input = vec![0.0; stride * 2];
        let mut output = vec![0.0; stride * 2];
        let code = plugin_process(probe.pointer, input.as_ptr(), output.as_mut_ptr(), stride);
        if code == 0 {
            return stride;
        }
        assert_eq!(
            code,
            PluginError::BufferTooSmall as i32,
            "stride probe: {}",
            last_error()
        );
    }
    panic!("even single-frame blocks refused");
}

/// Non-default nine-control config exercising every appended control.
fn nondefault_config(audition: bool) -> String {
    serde_json::json!({
        "enabled": true,
        "sensitivity": 2.0,
        "link_channels": true,
        "mode": 1,
        "bands": 2,
        "crossover_hz": 8000.0,
        "frequency_skew": 0.5,
        "repair_width": WIDTH,
        "audition_residual": audition,
    })
    .to_string()
}

/// Explicit neutral nine-control config (legacy-equivalent path).
fn neutral_config() -> String {
    serde_json::json!({
        "enabled": true,
        "sensitivity": 2.0,
        "link_channels": true,
        "mode": 0,
        "bands": 0,
        "crossover_hz": 4000.0,
        "frequency_skew": 0.0,
        "repair_width": 0,
        "audition_residual": false,
    })
    .to_string()
}

/// Error-oracle config: random multiband with neutral skew/link.
fn error_config(bands: usize, width: usize, crossover_hz: f32, audition: bool) -> String {
    serde_json::json!({
        "enabled": true,
        "sensitivity": 2.0,
        "link_channels": true,
        "mode": 0,
        "bands": bands,
        "crossover_hz": crossover_hz,
        "frequency_skew": 0.0,
        "repair_width": width,
        "audition_residual": audition,
    })
    .to_string()
}

/// Fixture tone: 0.25-amplitude sine at `freq` Hz.
fn tone(frame: usize, freq: f32, rate: u32) -> f32 {
    (frame as f32 * freq / rate as f32 * std::f32::consts::TAU).sin() * 0.25
}

/// Corrupted multichannel input, per-channel clean references, click flags.
///
/// Channel 0 carries 440 Hz with its own click plan, channel 1 carries
/// 660 Hz with an offset plan, so swaps or cross-talk fail loudly.
fn click_fixture(
    channels: usize,
    frames: usize,
    rate: u32,
) -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    assert!((1..=2).contains(&channels), "fixture supports mono/stereo");
    let freqs = [440.0, 660.0];
    let plans: [Vec<(usize, usize, f32)>; 2] = [
        vec![(100, 1, 1.0), (300, 3, -1.0), (700, 1, 1.0)],
        vec![(150, 1, -1.0), (500, 3, 1.0), (900, 1, -1.0)],
    ];
    build_fixture(channels, frames, rate, &freqs, &plans)
}

/// End-of-stream stress fixture: clicks only at the final input frames
/// (channel 0 single at the last frame, channel 1 double-wide straddling
/// the last two), so repair runs on one-sided lookahead during the drain.
fn last_frame_fixture(
    channels: usize,
    frames: usize,
    rate: u32,
) -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    continued_last_frame_fixture(channels, frames, 0, rate)
}

/// Last-frame clicks anchored at `anchor - 1` / `anchor - 2` with the
/// stream continuing `extra` click-free tone frames past the anchor, so
/// the anchored clicks gain full post context while the stream still ends.
fn continued_last_frame_fixture(
    channels: usize,
    anchor: usize,
    extra: usize,
    rate: u32,
) -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    assert!((1..=2).contains(&channels), "fixture supports mono/stereo");
    assert!(anchor >= 64, "fixture needs settled context");
    let freqs = [440.0, 660.0];
    let plans: [Vec<(usize, usize, f32)>; 2] =
        [vec![(anchor - 1, 1, 1.0)], vec![(anchor - 2, 2, -1.0)]];
    build_fixture(channels, anchor + extra, rate, &freqs, &plans)
}

/// W-wide EOF click fixture for the error oracle: ch0 carries a W-wide
/// loud click at the stream end, all other channels stay clean (exposes
/// link coupling on stereo). Returns (corrupted, clean, is_click).
fn eof_wide_fixture(
    channels: usize,
    frames: usize,
    rate: u32,
    click_w: usize,
) -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    assert!((1..=2).contains(&channels), "fixture supports mono/stereo");
    assert!((1..=3).contains(&click_w), "click widths 1-3");
    assert!(frames >= 64, "fixture needs settled context");
    let freqs = [440.0, 660.0];
    let mut plans: [Vec<(usize, usize, f32)>; 2] = [Vec::new(), Vec::new()];
    plans[0].push((frames - click_w, click_w, 1.0));
    build_fixture(channels, frames, rate, &freqs, &plans)
}

fn build_fixture(
    channels: usize,
    frames: usize,
    rate: u32,
    freqs: &[f32; 2],
    plans: &[Vec<(usize, usize, f32)>; 2],
) -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    let mut corrupted = vec![0.0; frames * channels];
    let mut clean = vec![0.0; frames * channels];
    let mut is_click = vec![vec![false; frames]; channels];
    for ch in 0..channels {
        for frame in 0..frames {
            let sample = tone(frame, freqs[ch], rate);
            corrupted[frame * channels + ch] = sample;
            clean[frame * channels + ch] = sample;
        }
        for &(start, width, sign) in &plans[ch] {
            for offset in 0..width {
                corrupted[(start + offset) * channels + ch] += sign * CLICK_AMP;
                is_click[ch][start + offset] = true;
            }
        }
    }
    (corrupted, clean, is_click)
}

/// Independent oracle: `signal` delayed by `latency` frames over the full
/// rendered span (input frames plus latency tail), zero-padded past the end.
fn manual_delayed(signal: &[f32], channels: usize, latency: usize) -> Vec<f32> {
    let frames = signal.len() / channels;
    let mut delayed = vec![0.0; (frames + latency) * channels];
    for frame in 0..frames {
        for ch in 0..channels {
            delayed[(frame + latency) * channels + ch] = signal[frame * channels + ch];
        }
    }
    delayed
}

fn report(label: &str, worst: f32) {
    eprintln!("[declick-ffi] {label} worst={worst:.6}");
}

/// Repair-footprint guard: click frames ± widened emission plus the ±2
/// detection slop carried from the accepted width-0 suite.
fn near_click(is_click: &[bool], frame: usize, width: usize) -> bool {
    let guard = width + 2;
    let start = frame.saturating_sub(guard);
    let end = (frame + guard + 1).min(is_click.len());
    is_click[start..end].contains(&true)
}

/// Check a full cleaned render against the frozen accuracy contract.
///
/// Repair error within 5% of click amplitude at click frames, absolute
/// damage below 0.05 on settled frames outside the repair footprint, exact
/// leading silence, and exact full-span length. Output frame `F` carries
/// input frame `F - latency`.
#[allow(
    clippy::too_many_arguments,
    reason = "test oracle: one argument per fixture stream and geometry bound"
)]
fn check_cleaned(
    label: &str,
    output: &[f32],
    clean: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    frames: usize,
    latency: usize,
    width: usize,
) {
    assert_eq!(
        output.len(),
        (frames + latency) * channels,
        "{label}: length"
    );
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{label}: finite"
    );
    for (index, sample) in output.iter().take(latency * channels).enumerate() {
        assert_eq!(*sample, 0.0, "{label}: leading silence sample={index}");
    }
    let clean_delayed = manual_delayed(clean, channels, latency);
    let mut worst_repair = 0.0_f32;
    let mut worst_damage = 0.0_f32;
    for frame in latency..frames + latency {
        let input = frame - latency;
        if input < SETTLED_FROM {
            continue;
        }
        for ch in 0..channels {
            let clicks = &is_click[ch];
            let actual = output[frame * channels + ch];
            let expected = clean_delayed[frame * channels + ch];
            if clicks[input] {
                let error = (actual - expected).abs();
                worst_repair = worst_repair.max(error);
                assert!(
                    error < CLICK_AMP * 0.05,
                    "{label}: repair input={input} ch={ch} error={error}"
                );
            } else if !near_click(clicks, input, width) {
                let damage = (actual - expected).abs();
                worst_damage = worst_damage.max(damage);
                assert!(
                    damage < 0.05,
                    "{label}: damage input={input} ch={ch} damage={damage}"
                );
            }
        }
    }
    report(&format!("{label} repair-error"), worst_repair);
    report(&format!("{label} damage"), worst_damage);
}

/// Exact length plus finiteness for a full render (inputs plus tail).
fn check_exact_finite_length(
    label: &str,
    output: &[f32],
    channels: usize,
    frames: usize,
    latency: usize,
) {
    assert_eq!(
        output.len(),
        (frames + latency) * channels,
        "{label}: length"
    );
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{label}: finite"
    );
}

/// Exact leading silence over the latency pre-roll.
fn check_leading_silence(label: &str, output: &[f32], channels: usize, latency: usize) {
    for (index, sample) in output.iter().take(latency * channels).enumerate() {
        assert_eq!(*sample, 0.0, "{label}: leading silence sample={index}");
    }
}

/// Pin click-free boundary outputs over an input range to the fixture
/// programme peak (dry tone and median baselines stay inside it).
fn check_boundary_peak(
    label: &str,
    output: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    latency: usize,
    first: usize,
    end: usize,
) {
    let mut worst = 0.0_f32;
    for input in first..end {
        for ch in 0..channels {
            assert!(
                !is_click[ch][input],
                "{label}: input={input} ch={ch} must be click-free"
            );
            let actual = output[(input + latency) * channels + ch];
            worst = worst.max(actual.abs());
            assert!(
                actual.abs() <= BOUNDARY_PROGRAMME_PEAK,
                "{label}: boundary programme input={input} ch={ch} value={actual}"
            );
        }
    }
    report(&format!("{label} boundary-peak"), worst);
}

/// Check a continued render: frozen interior bounds on every asserted
/// input (which all enjoy full post context through the continuation),
/// click-free continuation outputs pinned to the caller-supplied endpoint
/// peak (`Some` always; fullband uses the programme peak, multiband uses
/// its derived endpoint peak), and exact geometry. Output frame `F`
/// carries input frame `F - latency`.
#[allow(
    clippy::too_many_arguments,
    reason = "test oracle: one argument per fixture stream and geometry bound"
)]
fn check_continued(
    label: &str,
    output: &[f32],
    clean: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    frames: usize,
    assert_frames: usize,
    latency: usize,
    width: usize,
    continuation_peak: Option<f32>,
) {
    check_exact_finite_length(label, output, channels, frames, latency);
    check_leading_silence(label, output, channels, latency);
    let clean_delayed = manual_delayed(clean, channels, latency);
    let mut worst_repair = 0.0_f32;
    let mut worst_damage = 0.0_f32;
    for frame in latency..frames + latency {
        let input = frame - latency;
        for ch in 0..channels {
            let actual = output[frame * channels + ch];
            if input >= assert_frames {
                assert!(
                    !is_click[ch][input],
                    "{label}: continuation must be click-free"
                );
                if let Some(peak) = continuation_peak {
                    assert!(
                        actual.abs() <= peak,
                        "{label}: continuation input={input} ch={ch} value={actual}"
                    );
                }
                continue;
            }
            if input < SETTLED_FROM {
                continue;
            }
            let clicks = &is_click[ch];
            let expected = clean_delayed[frame * channels + ch];
            if clicks[input] {
                let error = (actual - expected).abs();
                worst_repair = worst_repair.max(error);
                assert!(
                    error < CLICK_AMP * 0.05,
                    "{label}: repair input={input} ch={ch} error={error}"
                );
            } else if !near_click(clicks, input, width) {
                let damage = (actual - expected).abs();
                worst_damage = worst_damage.max(damage);
                assert!(
                    damage < 0.05,
                    "{label}: damage input={input} ch={ch} damage={damage}"
                );
            }
        }
    }
    report(&format!("{label} repair-error"), worst_repair);
    report(&format!("{label} damage"), worst_damage);
}

/// Check a true-boundary fullband render: frozen geometry and damage on
/// full-context frames (inputs whose 8-frame post window holds only real
/// signal), spike removal plus the derived repair peak at click frames,
/// and programme-peak pins on click-free boundary frames.
#[allow(
    clippy::too_many_arguments,
    reason = "test oracle: one argument per fixture stream and geometry bound"
)]
fn check_boundary_clicks(
    label: &str,
    output: &[f32],
    clean: &[f32],
    corrupted: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    frames: usize,
    latency: usize,
) {
    check_exact_finite_length(label, output, channels, frames, latency);
    check_leading_silence(label, output, channels, latency);
    let clean_delayed = manual_delayed(clean, channels, latency);
    let context_end = frames - POST_CONTEXT;
    let mut worst_damage = 0.0_f32;
    let mut worst_peak = 0.0_f32;
    for frame in latency..frames + latency {
        let input = frame - latency;
        for ch in 0..channels {
            let clicks = &is_click[ch];
            let actual = output[frame * channels + ch];
            if clicks[input] {
                let spike = corrupted[input * channels + ch];
                let removal = (actual - spike).abs();
                assert!(
                    removal > 1.0,
                    "{label}: ch{ch} input={input} spike passed through"
                );
                worst_peak = worst_peak.max(actual.abs());
                assert!(
                    actual.abs() <= BOUNDARY_REPAIR_PEAK,
                    "{label}: boundary repair input={input} ch={ch} value={actual}"
                );
            } else if input >= SETTLED_FROM && input < context_end && !near_click(clicks, input, 0)
            {
                let expected = clean_delayed[frame * channels + ch];
                let damage = (actual - expected).abs();
                worst_damage = worst_damage.max(damage);
                assert!(
                    damage < 0.05,
                    "{label}: damage input={input} ch={ch} damage={damage}"
                );
            } else if input >= context_end {
                worst_peak = worst_peak.max(actual.abs());
                assert!(
                    actual.abs() <= BOUNDARY_PROGRAMME_PEAK,
                    "{label}: boundary programme input={input} ch={ch} value={actual}"
                );
            }
        }
    }
    report(&format!("{label} damage"), worst_damage);
    report(&format!("{label} boundary-peak"), worst_peak);
    for ch in 0..channels {
        let last_out = (frames + latency - 1) * channels;
        eprintln!(
            "[declick-ffi] {label} ch{ch} last repaired={:.6}",
            output[last_out + ch]
        );
    }
}

/// Check a true-boundary multiband render: same geometry, removal, and
/// full-context damage as the fullband leg, plus the derived endpoint peak
/// at click frames and on click-free boundary frames. `bands` selects the
/// peak (2 or 3); any other count is a test bug.
#[allow(
    clippy::too_many_arguments,
    reason = "test oracle: one argument per fixture stream and geometry bound"
)]
fn check_boundary_clicks_multiband(
    label: &str,
    output: &[f32],
    clean: &[f32],
    corrupted: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    frames: usize,
    latency: usize,
    width: usize,
    bands: usize,
) {
    let peak = match bands {
        2 => MULTIBAND_BOUNDARY_PEAK_2BAND,
        3 => MULTIBAND_BOUNDARY_PEAK_3BAND,
        _ => panic!("{label}: multiband endpoint peak needs 2 or 3 bands, got {bands}"),
    };
    check_exact_finite_length(label, output, channels, frames, latency);
    check_leading_silence(label, output, channels, latency);
    let clean_delayed = manual_delayed(clean, channels, latency);
    let context_end = frames - POST_CONTEXT;
    let mut worst_damage = 0.0_f32;
    let mut worst_peak = 0.0_f32;
    for frame in latency..frames + latency {
        let input = frame - latency;
        for ch in 0..channels {
            let clicks = &is_click[ch];
            let actual = output[frame * channels + ch];
            if clicks[input] {
                let spike = corrupted[input * channels + ch];
                let removal = (actual - spike).abs();
                assert!(
                    removal > 1.0,
                    "{label}: ch{ch} input={input} spike passed through"
                );
                worst_peak = worst_peak.max(actual.abs());
                assert!(
                    actual.abs() <= peak,
                    "{label}: boundary repair input={input} ch={ch} value={actual}"
                );
            } else if input >= SETTLED_FROM
                && input < context_end
                && !near_click(clicks, input, width)
            {
                let expected = clean_delayed[frame * channels + ch];
                let damage = (actual - expected).abs();
                worst_damage = worst_damage.max(damage);
                assert!(
                    damage < 0.05,
                    "{label}: damage input={input} ch={ch} damage={damage}"
                );
            } else if input >= context_end {
                worst_peak = worst_peak.max(actual.abs());
                assert!(
                    actual.abs() <= peak,
                    "{label}: boundary programme input={input} ch={ch} value={actual}"
                );
            }
        }
    }
    report(&format!("{label} damage"), worst_damage);
    report(&format!("{label} boundary-peak"), worst_peak);
    for ch in 0..channels {
        let last_out = (frames + latency - 1) * channels;
        eprintln!(
            "[declick-ffi] {label} ch{ch} last repaired={:.6}",
            output[last_out + ch]
        );
    }
}

/// Check an EOF error-oracle render triple with three stated estimands, no
/// boundary exclusion, original bounds only: (D) differential
/// |corrupted_plugin - clean_plugin| (<0.15 at clicks, <0.05 on clean
/// outside the footprint; useful control, NOT independent); (I)
/// independent |clean_plugin - analytical| (<0.05 on clean outside the
/// footprint, where analytical is the aligned clean tone with no plugin;
/// the independent false-positive measure); (E) end-to-end
/// |corrupted_plugin - analytical| (<0.15 at clicks, <0.05 on clean).
/// Plus removal >1.0, PR >=0.95 (hot>0.5 over settled frames, no
/// exclusion), regroup <1e-5 vs the delayed corrupted input, and exact
/// geometry. Output frame `F` carries input frame `F - latency`.
#[allow(
    clippy::too_many_arguments,
    reason = "test oracle: one argument per fixture stream and geometry bound"
)]
fn check_eof_error(
    label: &str,
    repaired: &[f32],
    clean_out: &[f32],
    residual: &[f32],
    clean: &[f32],
    corrupted: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    frames: usize,
    latency: usize,
    width: usize,
    linked: bool,
) {
    for (name, output) in [
        ("repaired", repaired),
        ("clean", clean_out),
        ("residual", residual),
    ] {
        check_exact_finite_length(
            &format!("{label} {name}"),
            output,
            channels,
            frames,
            latency,
        );
    }
    check_leading_silence(label, repaired, channels, latency);
    let clean_delayed = manual_delayed(clean, channels, latency);
    let mut worst_diff_repair = 0.0_f32;
    let mut worst_direct_repair = 0.0_f32;
    let mut worst_clean_at_click = 0.0_f32;
    let mut worst_diff_damage = 0.0_f32;
    let mut worst_indep_damage = 0.0_f32;
    let mut worst_direct_damage = 0.0_f32;
    for frame in latency..frames + latency {
        let input = frame - latency;
        if input < SETTLED_FROM {
            continue;
        }
        for ch in 0..channels {
            let clicks = &is_click[ch];
            let out_idx = frame * channels + ch;
            let actual = repaired[out_idx];
            let clean_ref = clean_out[out_idx];
            let analytical = clean_delayed[out_idx];
            if clicks[input] {
                let spike = corrupted[input * channels + ch];
                assert!(
                    (actual - spike).abs() > 1.0,
                    "{label}: ch{ch} input={input} spike passed through"
                );
                let diff = (actual - clean_ref).abs();
                worst_diff_repair = worst_diff_repair.max(diff);
                assert!(
                    diff < 0.15,
                    "{label}: differential ch{ch} input={input} error={diff}"
                );
                let direct = (actual - analytical).abs();
                worst_direct_repair = worst_direct_repair.max(direct);
                assert!(
                    direct < 0.15,
                    "{label}: direct ch{ch} input={input} error={direct}"
                );
                let clean_err = (clean_ref - analytical).abs();
                worst_clean_at_click = worst_clean_at_click.max(clean_err);
                assert!(
                    clean_err < 0.05,
                    "{label}: clean-at-click ch{ch} input={input} error={clean_err}"
                );
            } else if !near_click(clicks, input, width) {
                // Link-coupled clean frames (partner clicks at the same
                // frame while linked) are intentional pair repairs: repair
                // bound for differential/direct, 0.05 for independent
                // (clean render has no coupling).
                let coupled = linked && channels > 1 && is_click[1 - ch][input];
                let pair_bound = if coupled { 0.15 } else { 0.05 };
                let diff = (actual - clean_ref).abs();
                worst_diff_damage = worst_diff_damage.max(diff);
                assert!(
                    diff < pair_bound,
                    "{label}: differential damage ch{ch} input={input} coupled={coupled} error={diff}"
                );
                let indep = (clean_ref - analytical).abs();
                worst_indep_damage = worst_indep_damage.max(indep);
                assert!(
                    indep < 0.05,
                    "{label}: independent damage ch{ch} input={input} error={indep}"
                );
                let direct = (actual - analytical).abs();
                worst_direct_damage = worst_direct_damage.max(direct);
                assert!(
                    direct < pair_bound,
                    "{label}: direct damage ch{ch} input={input} coupled={coupled} error={direct}"
                );
            }
        }
    }
    report(&format!("{label} diff-repair"), worst_diff_repair);
    report(&format!("{label} direct-repair"), worst_direct_repair);
    report(&format!("{label} clean-at-click"), worst_clean_at_click);
    report(&format!("{label} diff-damage"), worst_diff_damage);
    report(&format!("{label} indep-damage"), worst_indep_damage);
    report(&format!("{label} direct-damage"), worst_direct_damage);
    check_residual(&ResidualOracle {
        label,
        residual,
        repaired,
        corrupted,
        is_click,
        channels,
        frames,
        latency,
        width,
    });
}

/// Fixture streams and geometry for the residual regroup oracle: the
/// repaired render, the residual render, and the corrupted input they must
/// regroup to, plus the click map and render geometry.
struct ResidualOracle<'a> {
    label: &'a str,
    residual: &'a [f32],
    repaired: &'a [f32],
    corrupted: &'a [f32],
    is_click: &'a [Vec<bool>],
    channels: usize,
    frames: usize,
    latency: usize,
    width: usize,
}

/// Check a residual render: `repaired + residual` must regroup to the
/// independently delayed corrupted input within float regrouping, and the
/// residual must run hot at click frames (recall/precision at least 0.95
/// over settled frames; the widened footprint is excluded from false-hot
/// counting because intentional skirt repairs run hot by design).
fn check_residual(oracle: &ResidualOracle<'_>) {
    let label = oracle.label;
    let (residual, repaired) = (oracle.residual, oracle.repaired);
    let (channels, frames, latency, width) =
        (oracle.channels, oracle.frames, oracle.latency, oracle.width);
    assert_eq!(residual.len(), repaired.len(), "{label}: length");
    assert_eq!(
        residual.len(),
        (frames + latency) * channels,
        "{label}: length"
    );
    let dry = manual_delayed(oracle.corrupted, channels, latency);
    let mut worst = 0.0_f32;
    for i in 0..dry.len() {
        worst = worst.max((repaired[i] + residual[i] - dry[i]).abs());
    }
    assert!(worst < 1.0e-5, "{label}: regroup drift {worst}");
    report(&format!("{label} regroup"), worst);
    let mut true_hot = 0;
    let mut false_hot = 0;
    let mut missed = 0;
    for frame in SETTLED_FROM..frames {
        for ch in 0..channels {
            let clicks = &oracle.is_click[ch];
            if !clicks[frame] && near_click(clicks, frame, width) {
                continue;
            }
            let hot = residual[(frame + latency) * channels + ch].abs() > HOT_RESIDUAL;
            match (clicks[frame], hot) {
                (true, true) => true_hot += 1,
                (false, true) => false_hot += 1,
                (true, false) => missed += 1,
                (false, false) => {}
            }
        }
    }
    let total = true_hot + missed;
    let recall = true_hot as f32 / total as f32;
    let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
    assert!(
        recall >= 0.95,
        "{label}: recall={recall} ({true_hot}/{total})"
    );
    assert!(
        precision >= 0.95,
        "{label}: precision={precision} false_hot={false_hot}"
    );
    eprintln!("[declick-ffi] {label} recall={recall:.4} precision={precision:.4}");
}

fn peak(output: &[f32]) -> f32 {
    output.iter().map(|sample| sample.abs()).fold(0.0, f32::max)
}

#[test]
fn declick_ffi_maps_nine_controls_in_param_order() {
    let handle = DeclickHandle::create("{}", RATE_48K, 2, 2);
    assert_eq!(handle.param_count(), 9);
    let expected = [
        "enabled",
        "sensitivity",
        "link_channels",
        "mode",
        "bands",
        "crossover_hz",
        "frequency_skew",
        "repair_width",
        "audition_residual",
    ];
    for (index, id) in expected.iter().enumerate() {
        assert_eq!(handle.param_id(index), *id, "address {index} moved");
    }
    // Typed live defaults for every control, read back from the saved
    // state document (exported API, not Rust internals).
    let map = handle.save_map();
    for (id, expected) in [
        ("enabled", serde_json::json!(true)),
        ("sensitivity", serde_json::json!(10.0)),
        ("link_channels", serde_json::json!(true)),
        ("mode", serde_json::json!(0)),
        ("bands", serde_json::json!(0)),
        ("crossover_hz", serde_json::json!(4000.0)),
        ("frequency_skew", serde_json::json!(0.0)),
        ("repair_width", serde_json::json!(0)),
        ("audition_residual", serde_json::json!(false)),
    ] {
        assert_eq!(map[id], expected, "default {id}");
    }
    // Choice labels in registry order; non-choice and out-of-range are NULL.
    assert_eq!(handle.choice_label(3, 0).as_deref(), Some("Random"));
    assert_eq!(handle.choice_label(3, 1).as_deref(), Some("Periodic"));
    assert_eq!(handle.choice_label(4, 0).as_deref(), Some("Fullband"));
    assert_eq!(handle.choice_label(4, 1).as_deref(), Some("2-band"));
    assert_eq!(handle.choice_label(4, 2).as_deref(), Some("3-band"));
    assert_eq!(handle.choice_label(3, 2), None);
    assert_eq!(handle.choice_label(1, 0), None);
    // The default config takes the legacy path: eight samples, advertised
    // through the C ABI info document.
    assert_eq!(handle.info_latency(), LEGACY_LATENCY);
    // Declick tails are latency-bounded: the capacity query agrees.
    assert_eq!(handle.drain_capacity(), LEGACY_LATENCY);
}

#[test]
fn declick_ffi_choice_labels_match_canonical_options_for_all_aliases() {
    use sotf_plugins::plugin_declick::params::{BANDS_OPTIONS, MODE_OPTIONS};

    // The FFI tables are fixed process-static copies: pin the canonical
    // lengths so a DSP option addition fails here instead of silently
    // desynchronizing.
    assert_eq!(MODE_OPTIONS.len(), 2, "FFI mode table holds 2 entries");
    assert_eq!(BANDS_OPTIONS.len(), 3, "FFI bands table holds 3 entries");

    for alias in ["Declick", "declick", "TransientRepair", "transient_repair"] {
        let handle = DeclickHandle::create_as(alias, "{}", RATE_48K, 2, 2);
        assert_eq!(handle.param_count(), 9, "{alias} must expose 9 controls");
        assert_eq!(handle.param_id(3), "mode", "{alias} address 3 moved");
        assert_eq!(handle.param_id(4), "bands", "{alias} address 4 moved");
        // Every canonical label resolves at its stable index.
        for (choice, expected) in MODE_OPTIONS.iter().enumerate() {
            assert_eq!(
                handle.choice_label(3, choice).as_deref(),
                Some(*expected),
                "{alias} mode choice {choice}"
            );
        }
        for (choice, expected) in BANDS_OPTIONS.iter().enumerate() {
            assert_eq!(
                handle.choice_label(4, choice).as_deref(),
                Some(*expected),
                "{alias} bands choice {choice}"
            );
        }
        // Fencepost and huge indexes are a documented NULL failure.
        assert_eq!(handle.choice_label(3, MODE_OPTIONS.len()), None);
        assert_eq!(handle.choice_label(4, BANDS_OPTIONS.len()), None);
        assert_eq!(handle.choice_label(3, usize::MAX), None);
        assert_eq!(handle.choice_label(4, usize::MAX), None);
        // Non-choice controls have no labels.
        assert_eq!(handle.choice_label(1, 0), None, "{alias} sensitivity");
        assert_eq!(handle.choice_label(8, 0), None, "{alias} audition");
    }
}

#[test]
fn declick_ffi_legacy_configs_render_neutral_eight_sample() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let mut defaulted = DeclickHandle::create("{}", RATE_48K, CHANNELS, CHANNELS);
    let mut neutral = DeclickHandle::create(&neutral_config(), RATE_48K, CHANNELS, CHANNELS);
    let legacy3 = r#"{"enabled": true, "sensitivity": 2.0, "link_channels": true}"#;
    let mut legacy = DeclickHandle::create(legacy3, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(defaulted.info_latency(), LEGACY_LATENCY);
    assert_eq!(neutral.info_latency(), LEGACY_LATENCY);
    assert_eq!(legacy.info_latency(), LEGACY_LATENCY);
    let defaulted_out = defaulted.process_with_drain(&corrupted);
    let neutral_out = neutral.process_with_drain(&corrupted);
    let legacy_out = legacy.process_with_drain(&corrupted);
    for (label, output) in [
        ("default", &defaulted_out),
        ("neutral", &neutral_out),
        ("legacy3", &legacy_out),
    ] {
        assert_eq!(
            output.len(),
            (FRAMES + LEGACY_LATENCY) * CHANNELS,
            "{label}: length"
        );
        for (index, sample) in output.iter().take(LEGACY_LATENCY * CHANNELS).enumerate() {
            assert_eq!(*sample, 0.0, "{label}: leading silence {index}");
        }
        assert!(peak(output) > 0.05, "{label}: repaired must stay nonzero");
    }
    // The old three-key state renders bit-identically to the explicit
    // neutral nine-key state: appended defaults are neutral.
    assert_eq!(legacy_out, neutral_out);
}

#[test]
fn declick_ffi_repairs_clicks_and_drains_exact_eof() {
    let (corrupted, clean, is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let mut handle = DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(handle.info_latency(), LATENCY);
    assert_eq!(handle.drain_capacity(), LATENCY);
    let repaired = handle.process_with_drain(&corrupted);
    check_cleaned(
        "ffi cleaned",
        &repaired,
        &clean,
        &is_click,
        CHANNELS,
        FRAMES,
        LATENCY,
        WIDTH,
    );
    assert!(peak(&repaired) > 0.05, "repaired must stay nonzero");
    // Exact last samples: the drain tail carries the delayed input end
    // within the frozen damage bound (no new tight bound invented).
    let oracle = manual_delayed(&clean, CHANNELS, LATENCY);
    let last = (FRAMES + LATENCY - 1) * CHANNELS;
    for ch in 0..CHANNELS {
        let error = (repaired[last + ch] - oracle[last + ch]).abs();
        assert!(error < 0.05, "last sample ch={ch} error={error}");
        eprintln!(
            "[declick-ffi] last-sample ch={ch} value={} oracle={}",
            repaired[last + ch],
            oracle[last + ch]
        );
    }
}

#[test]
fn declick_ffi_residual_regroups_to_delayed_input() {
    let (corrupted, _clean, is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let mut cleaned =
        DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
    let mut residual =
        DeclickHandle::create(&nondefault_config(true), RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(residual.info_latency(), LATENCY);
    let repaired = cleaned.process_with_drain(&corrupted);
    let residual_out = residual.process_with_drain(&corrupted);
    check_residual(&ResidualOracle {
        label: "ffi residual",
        residual: &residual_out,
        repaired: &repaired,
        corrupted: &corrupted,
        is_click: &is_click,
        channels: CHANNELS,
        frames: FRAMES,
        latency: LATENCY,
        width: WIDTH,
    });
    assert!(
        peak(&residual_out) > HOT_RESIDUAL,
        "residual must run hot at repairs"
    );
}

#[test]
fn declick_ffi_invalid_restore_keeps_populated_history() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let config = nondefault_config(false);
    let mut handle = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    let mut twin = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    // Populate both histories identically (600 frames incl. settled clicks).
    let prefix = 600 * CHANNELS;
    assert_eq!(
        handle.process(&corrupted[..prefix]),
        twin.process(&corrupted[..prefix])
    );
    let before_save = handle.save();
    // Proven-invalid candidates: unparseable bytes and a mistyped value.
    assert_ne!(handle.load(b"not json"), 0);
    let bad = serde_json::json!({"sensitivity": "not-a-number"});
    assert_ne!(handle.load(&serde_json::to_vec(&bad).unwrap()), 0);
    assert_eq!(handle.save(), before_save);
    // Continuation is bit-identical to the uninterrupted twin, through EOF.
    assert_eq!(
        handle.process(&corrupted[prefix..]),
        twin.process(&corrupted[prefix..])
    );
    let latency = handle.info_latency();
    let drained = handle.drain_to_eof(latency);
    let twin_drained = twin.drain_to_eof(latency);
    assert_eq!(drained.len(), LATENCY * CHANNELS);
    assert_eq!(drained, twin_drained);
}

#[test]
fn declick_ffi_structural_and_live_sets_behave() {
    // (a) All nine controls round-trip through normalized sets: saved
    // state carries the exact typed values, normalized readbacks agree
    // within float dust. Everything crosses the exported boundary.
    let mut handle = DeclickHandle::create("{}", RATE_48K, CHANNELS, CHANNELS);
    for (id, normalized, typed) in [
        ("enabled", 0.0, serde_json::json!(false)),
        ("enabled", 1.0, serde_json::json!(true)),
        ("sensitivity", 0.0, serde_json::json!(1.0)),
        ("sensitivity", 1.0, serde_json::json!(100.0)),
        ("sensitivity", 9.0 / 99.0, serde_json::json!(10.0)),
        ("link_channels", 0.0, serde_json::json!(false)),
        ("link_channels", 1.0, serde_json::json!(true)),
        ("mode", 1.0, serde_json::json!(1)),
        ("mode", 0.0, serde_json::json!(0)),
        ("bands", 0.5, serde_json::json!(1)),
        ("bands", 1.0, serde_json::json!(2)),
        ("bands", 0.0, serde_json::json!(0)),
        ("crossover_hz", 0.0, serde_json::json!(80.0)),
        ("crossover_hz", 1.0, serde_json::json!(12000.0)),
        ("frequency_skew", 0.75, serde_json::json!(0.5)),
        ("frequency_skew", 0.5, serde_json::json!(0.0)),
        ("repair_width", 3.0 / 8.0, serde_json::json!(3)),
        ("repair_width", 0.0, serde_json::json!(0)),
        ("audition_residual", 1.0, serde_json::json!(true)),
        ("audition_residual", 0.0, serde_json::json!(false)),
    ] {
        assert_eq!(
            handle.set_normalized(id, normalized),
            0,
            "{id} set: {}",
            last_error()
        );
        assert_eq!(handle.save_map()[id], typed, "{id} typed");
        let readback = handle.get_normalized(id);
        assert!(
            (readback - normalized).abs() < 1e-12,
            "{id} roundtrip: {readback} vs {normalized}"
        );
    }

    let (corrupted, _clean, is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    // (b) Structural width set succeeds, retunes latency, resets history:
    // after a reset the stream renders exactly like a freshly constructed
    // width-3 instance.
    let mut widened = DeclickHandle::create(&neutral_config(), RATE_48K, CHANNELS, CHANNELS);
    let _ = widened.process(&corrupted[..600 * CHANNELS]);
    assert_eq!(
        widened.set_normalized("repair_width", 3.0 / 8.0),
        0,
        "{}",
        last_error()
    );
    assert_eq!(widened.save_map()["repair_width"], serde_json::json!(3));
    assert_eq!(widened.info_latency(), LATENCY);
    widened.reset();
    let width3 = serde_json::json!({
        "enabled": true,
        "sensitivity": 2.0,
        "link_channels": true,
        "mode": 0,
        "bands": 0,
        "crossover_hz": 4000.0,
        "frequency_skew": 0.0,
        "repair_width": WIDTH,
        "audition_residual": false,
    })
    .to_string();
    let mut fresh = DeclickHandle::create(&width3, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(
        widened.process_with_drain(&corrupted),
        fresh.process_with_drain(&corrupted)
    );

    // (c) Live sensitivity set preserves latency and keeps rendering.
    let mut live = DeclickHandle::create(&neutral_config(), RATE_48K, CHANNELS, CHANNELS);
    let _ = live.process(&corrupted[..600 * CHANNELS]);
    assert_eq!(
        live.set_normalized("sensitivity", 9.0 / 99.0),
        0,
        "{}",
        last_error()
    );
    assert_eq!(live.save_map()["sensitivity"], serde_json::json!(10.0));
    assert_eq!(live.info_latency(), LEGACY_LATENCY);
    let continued = live.process(&corrupted[600 * CHANNELS..]);
    assert!(peak(&continued) > 0.05, "live set must keep audio flowing");

    // (d) Periodic mode engages through the set path. A set-path render
    // equals a freshly constructed periodic twin bit-exactly (structural
    // commit check). Interior bounds apply on full-context frames only
    // (accepted convention; the last 8 inputs see drain zeros ahead), so
    // the continued render — a natural 8-frame tone past the asserted
    // span — carries the frozen width-0 oracle on every asserted input,
    // with programme-peak pins on the continuation itself. Confinement
    // proves the flush stays inside the last 8 inputs; exact length holds
    // on both renders.
    assert_eq!(
        sotf_plugins::plugin_declick::repair::LOOKAHEAD_SAMPLES,
        POST_CONTEXT,
        "boundary depth must track the DSP post window"
    );
    let mut periodic = DeclickHandle::create(&neutral_config(), RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(periodic.set_normalized("mode", 1.0), 0, "{}", last_error());
    assert_eq!(periodic.save_map()["mode"], serde_json::json!(1));
    assert_eq!(periodic.info_latency(), LEGACY_LATENCY);
    let periodic_config = serde_json::json!({
        "enabled": true,
        "sensitivity": 2.0,
        "link_channels": true,
        "mode": 1,
        "bands": 0,
        "crossover_hz": 4000.0,
        "frequency_skew": 0.0,
        "repair_width": 0,
        "audition_residual": false,
    })
    .to_string();
    let mut fresh = DeclickHandle::create(&periodic_config, RATE_48K, CHANNELS, CHANNELS);
    let rendered = periodic.process_with_drain(&corrupted);
    assert_eq!(
        rendered,
        fresh.process_with_drain(&corrupted),
        "set-path must commit like fresh construction"
    );
    check_exact_finite_length(
        "ffi periodic short",
        &rendered,
        CHANNELS,
        FRAMES,
        LEGACY_LATENCY,
    );
    check_leading_silence("ffi periodic short", &rendered, CHANNELS, LEGACY_LATENCY);
    check_boundary_peak(
        "ffi periodic short",
        &rendered,
        &is_click,
        CHANNELS,
        LEGACY_LATENCY,
        FRAMES - POST_CONTEXT,
        FRAMES,
    );
    let (continued_input, continued_clean, continued_clicks) =
        click_fixture(CHANNELS, FRAMES + POST_CONTEXT, RATE_48K);
    let mut continued = DeclickHandle::create(&periodic_config, RATE_48K, CHANNELS, CHANNELS);
    let continued_rendered = continued.process_with_drain(&continued_input);
    check_continued(
        "ffi periodic continued",
        &continued_rendered,
        &continued_clean,
        &continued_clicks,
        CHANNELS,
        FRAMES + POST_CONTEXT,
        FRAMES,
        LEGACY_LATENCY,
        0,
        Some(BOUNDARY_PROGRAMME_PEAK),
    );
    let confined = (FRAMES - POST_CONTEXT + LEGACY_LATENCY) * CHANNELS;
    assert_eq!(
        rendered[..confined],
        continued_rendered[..confined],
        "flush must stay inside the last 8 inputs"
    );
}

#[test]
fn declick_ffi_save_reload_continues_audio() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let config = nondefault_config(false);
    let mut handle = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    let _ = handle.process(&corrupted[..600 * CHANNELS]);
    let saved = handle.save();
    // All nine controls persist with exact values.
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&saved).unwrap();
    assert_eq!(map["enabled"], serde_json::json!(true));
    assert_eq!(map["sensitivity"], serde_json::json!(2.0));
    assert_eq!(map["link_channels"], serde_json::json!(true));
    assert_eq!(map["mode"], serde_json::json!(1));
    assert_eq!(map["bands"], serde_json::json!(2));
    assert_eq!(map["crossover_hz"], serde_json::json!(8000.0));
    assert_eq!(map["frequency_skew"], serde_json::json!(0.5));
    assert_eq!(map["repair_width"], serde_json::json!(WIDTH));
    assert_eq!(map["audition_residual"], serde_json::json!(false));
    // Reload commits (fresh history): the reloaded handle saves identically
    // and renders exactly like a freshly constructed twin.
    let mut reloaded = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(reloaded.load(&saved), 0, "{}", last_error());
    assert_eq!(reloaded.save(), saved);
    assert_eq!(reloaded.info_latency(), LATENCY);
    let mut fresh = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(
        reloaded.process_with_drain(&corrupted),
        fresh.process_with_drain(&corrupted)
    );
}

#[test]
fn declick_ffi_preset_document_round_trips_nine_controls() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let config = nondefault_config(false);
    let handle = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    let mut len = 0usize;
    let name = CString::new("declick-nondefault").unwrap();
    let document = plugin_export_preset_json(handle.pointer, name.as_ptr(), &mut len);
    assert!(!document.is_null(), "{}", last_error());
    // SAFETY: FFI owns exactly len bytes until freed below.
    let bytes = unsafe { std::slice::from_raw_parts(document, len) }.to_vec();
    plugin_free_state(document, len);
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["preset_name"], "declick-nondefault");
    assert_eq!(doc["plugin_type"], "Declick");
    let state_bytes: Vec<u8> = serde_json::from_value(doc["state"].clone()).unwrap();
    let state: serde_json::Value = serde_json::from_slice(&state_bytes).unwrap();
    for (key, expected) in [
        ("enabled", serde_json::json!(true)),
        ("sensitivity", serde_json::json!(2.0)),
        ("link_channels", serde_json::json!(true)),
        ("mode", serde_json::json!(1)),
        ("bands", serde_json::json!(2)),
        ("crossover_hz", serde_json::json!(8000.0)),
        ("frequency_skew", serde_json::json!(0.5)),
        ("repair_width", serde_json::json!(WIDTH)),
        ("audition_residual", serde_json::json!(false)),
    ] {
        assert_eq!(state[key], expected, "preset state {key}");
    }
    // Import into a differently configured handle: all nine controls land
    // and audio matches a freshly constructed non-default twin.
    let mut imported = DeclickHandle::create("{}", RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(
        plugin_import_preset_json(imported.pointer, bytes.as_ptr(), bytes.len()),
        0,
        "{}",
        last_error()
    );
    let map = imported.save_map();
    assert_eq!(map["mode"], serde_json::json!(1));
    assert_eq!(map["bands"], serde_json::json!(2));
    assert_eq!(map["repair_width"], serde_json::json!(WIDTH));
    let mut twin = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(
        imported.process_with_drain(&corrupted),
        twin.process_with_drain(&corrupted)
    );
    // A garbage document refuses with the handle untouched.
    let before = imported.save();
    assert_ne!(
        plugin_import_preset_json(imported.pointer, b"not json".as_ptr(), 8),
        0
    );
    assert_eq!(imported.save(), before);
}

#[test]
fn declick_ffi_channels_and_rates_matrix() {
    // Routing, geometry, structural regrouping, and the multiband endpoint
    // peak across layouts/rates. Per-rate interior accuracy numbers stay
    // with the DSP lane (see module docs); the last 8 inputs here are
    // click-free tone (fixture clicks end at frame 900), so they pin the
    // 3-band endpoint peak on every rate/channel combination.
    for channels in [1, 2] {
        for rate in [44_100, 48_000, 96_000] {
            let tag = format!("matrix {channels}ch {rate}Hz");
            let (corrupted, _clean, is_click) = click_fixture(channels, FRAMES, rate);
            let mut cleaned =
                DeclickHandle::create(&nondefault_config(false), rate, channels, channels);
            let mut residual =
                DeclickHandle::create(&nondefault_config(true), rate, channels, channels);
            assert_eq!(cleaned.param_count(), 9, "{tag}");
            assert_eq!(cleaned.info_latency(), LATENCY, "{tag}");
            let repaired = cleaned.process_with_drain(&corrupted);
            let residual_out = residual.process_with_drain(&corrupted);
            assert_eq!(
                repaired.len(),
                (FRAMES + LATENCY) * channels,
                "{tag}: length"
            );
            assert_eq!(residual_out.len(), repaired.len(), "{tag}: residual length");
            for (index, sample) in repaired.iter().take(LATENCY * channels).enumerate() {
                assert_eq!(*sample, 0.0, "{tag}: leading silence {index}");
            }
            assert!(peak(&repaired) > 0.05, "{tag}: repaired nonzero");
            let dry = manual_delayed(&corrupted, channels, LATENCY);
            let mut worst = 0.0f32;
            for i in 0..dry.len() {
                worst = worst.max((repaired[i] + residual_out[i] - dry[i]).abs());
            }
            assert!(worst < 1.0e-5, "{tag}: regroup drift {worst}");
            eprintln!("[declick-ffi] {tag} regroup worst={worst:.6}");
            let mut worst_peak = 0.0f32;
            for input in FRAMES - POST_CONTEXT..FRAMES {
                for ch in 0..channels {
                    assert!(
                        !is_click[ch][input],
                        "{tag}: input={input} ch={ch} must be click-free"
                    );
                    let actual = repaired[(input + LATENCY) * channels + ch];
                    worst_peak = worst_peak.max(actual.abs());
                    assert!(
                        actual.abs() <= MULTIBAND_BOUNDARY_PEAK_3BAND,
                        "{tag}: boundary programme input={input} ch={ch} value={actual}"
                    );
                }
            }
            eprintln!("[declick-ffi] {tag} boundary-peak worst={worst_peak:.6}");
        }
    }
}

#[test]
fn declick_ffi_last_frame_click_flushes_through_lookahead() {
    // Clicks on the final input frames repair on one-sided lookahead while
    // the exported drain flushes them. The fullband leg pins removal plus
    // the derived repair peak; the multiband leg pins removal, geometry,
    // full-context damage, and the derived endpoint peak (2.0 for the
    // 3-band non-default config) on click and click-free boundary frames;
    // the continued leg gives the same clicks full post context, so the
    // interior 5% repair bound applies to them there.
    assert_eq!(
        sotf_plugins::plugin_declick::repair::LOOKAHEAD_SAMPLES,
        POST_CONTEXT,
        "boundary depth must track the DSP post window"
    );
    let (corrupted, clean, is_click) = last_frame_fixture(CHANNELS, FRAMES, RATE_48K);
    let mut handle = DeclickHandle::create(&neutral_config(), RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(handle.info_latency(), LEGACY_LATENCY);
    let rendered = handle.process_with_drain(&corrupted);
    check_boundary_clicks(
        "ffi last-frame fullband",
        &rendered,
        &clean,
        &corrupted,
        &is_click,
        CHANNELS,
        FRAMES,
        LEGACY_LATENCY,
    );
    assert!(peak(&rendered) > 0.05, "repaired must stay nonzero");
    let mut wide = DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(wide.info_latency(), LATENCY);
    let rendered_wide = wide.process_with_drain(&corrupted);
    check_boundary_clicks_multiband(
        "ffi last-frame multiband",
        &rendered_wide,
        &clean,
        &corrupted,
        &is_click,
        CHANNELS,
        FRAMES,
        LATENCY,
        WIDTH,
        3,
    );
    let (continued_input, continued_clean, continued_clicks) =
        continued_last_frame_fixture(CHANNELS, FRAMES, POST_CONTEXT, RATE_48K);
    let mut continued =
        DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
    let continued_rendered = continued.process_with_drain(&continued_input);
    check_continued(
        "ffi last-frame continued",
        &continued_rendered,
        &continued_clean,
        &continued_clicks,
        CHANNELS,
        FRAMES + POST_CONTEXT,
        FRAMES,
        LATENCY,
        WIDTH,
        Some(MULTIBAND_BOUNDARY_PEAK_3BAND),
    );
    let confined = (FRAMES - POST_CONTEXT + LATENCY) * CHANNELS;
    assert_eq!(
        rendered_wide[..confined],
        continued_rendered[..confined],
        "multiband flush must stay inside the last 8 inputs"
    );
}

#[test]
fn declick_ffi_multiband_eof_endpoint_error() {
    // C ABI endpoint ERROR oracle for multiband EOF: the same three
    // estimands as the DSP oracle (differential <0.15 at clicks,
    // independent clean <0.05, end-to-end <0.15/<0.05, all via
    // `check_eof_error` with no boundary exclusion) driven exclusively
    // through the exported C ABI. Matrix: 2/3-band x widths 0/3/8 x
    // crossovers 80/4k/12kHz x 1/2/3-wide EOF clicks (ch0 W-wide, ch1
    // clean for link coupling) at 48k stereo linked, plus split-link and
    // skew-extreme spots. Mono, remaining rates, and split-link breadth
    // live in the DSP oracle (fast direct path) and the FFI peak matrix;
    // this leg proves the exported path delivers identical accuracy on its
    // subset. Expected latency derives from the known width (8 + width)
    // and is checked against the advertised info value.
    for bands in [1, 2] {
        let band_count = bands + 1;
        for width in [0, 3, 8] {
            let latency = POST_CONTEXT + width;
            for crossover_hz in [80.0, 4000.0, 12_000.0] {
                for click_w in [1, 2, 3] {
                    let tag = format!(
                        "ffi eof-error bands={band_count} width={width} xover={crossover_hz} clickw={click_w}"
                    );
                    let (corrupted, clean, is_click) =
                        eof_wide_fixture(CHANNELS, FRAMES, RATE_48K, click_w);
                    let mut repaired_handle = DeclickHandle::create(
                        &error_config(bands, width, crossover_hz, false),
                        RATE_48K,
                        CHANNELS,
                        CHANNELS,
                    );
                    assert_eq!(repaired_handle.info_latency(), latency, "{tag}");
                    assert_eq!(repaired_handle.drain_capacity(), latency, "{tag}");
                    let repaired = repaired_handle.process_with_drain(&corrupted);
                    let mut clean_handle = DeclickHandle::create(
                        &error_config(bands, width, crossover_hz, false),
                        RATE_48K,
                        CHANNELS,
                        CHANNELS,
                    );
                    let clean_out = clean_handle.process_with_drain(&clean);
                    let mut residual_handle = DeclickHandle::create(
                        &error_config(bands, width, crossover_hz, true),
                        RATE_48K,
                        CHANNELS,
                        CHANNELS,
                    );
                    let residual = residual_handle.process_with_drain(&corrupted);
                    check_eof_error(
                        &tag, &repaired, &clean_out, &residual, &clean, &corrupted, &is_click,
                        CHANNELS, FRAMES, latency, width, true,
                    );
                }
            }
        }
    }
    // Spots: split link + skew extremes (3-band, width 0, 4kHz, 3-wide).
    let make = |link: bool, skew: f32, audition: bool| {
        serde_json::json!({
            "enabled": true,
            "sensitivity": 2.0,
            "link_channels": link,
            "mode": 0,
            "bands": 2,
            "crossover_hz": 4000.0,
            "frequency_skew": skew,
            "repair_width": 0,
            "audition_residual": audition,
        })
        .to_string()
    };
    for (name, link, skew) in [
        ("split", false, 0.0),
        ("skew-neg", true, -1.0),
        ("skew-pos", true, 1.0),
    ] {
        let tag = format!("ffi eof-error spot {name}");
        let (corrupted, clean, is_click) = eof_wide_fixture(CHANNELS, FRAMES, RATE_48K, 3);
        let mut repaired_handle =
            DeclickHandle::create(&make(link, skew, false), RATE_48K, CHANNELS, CHANNELS);
        assert_eq!(repaired_handle.info_latency(), LEGACY_LATENCY, "{tag}");
        let repaired = repaired_handle.process_with_drain(&corrupted);
        let mut clean_handle =
            DeclickHandle::create(&make(link, skew, false), RATE_48K, CHANNELS, CHANNELS);
        let clean_out = clean_handle.process_with_drain(&clean);
        let mut residual_handle =
            DeclickHandle::create(&make(link, skew, true), RATE_48K, CHANNELS, CHANNELS);
        let residual = residual_handle.process_with_drain(&corrupted);
        check_eof_error(
            &tag,
            &repaired,
            &clean_out,
            &residual,
            &clean,
            &corrupted,
            &is_click,
            CHANNELS,
            FRAMES,
            LEGACY_LATENCY,
            0,
            link,
        );
    }
}

#[test]
fn declick_ffi_periodic_eof_locks_and_repairs() {
    // C ABI periodic lock + EOF error leg (single config: 3-band, width 0,
    // 4kHz, 48k stereo linked, 1441 frames, grid period 100 with a gap at
    // 1240 filled by the quiet on-phase probe). The FFI cannot white-box
    // tracker state, so the lock is proven behaviorally exactly as in the
    // DSP leg: quiet on-phase (1240, sensitive) must repair (<0.15 direct
    // error) while quiet off-phase (1290, guard) must miss (>0.3 error) —
    // a split impossible unless locked. Loud 3-wide EOF clicks (1438-1440)
    // must meet differential + direct <0.15; clean damage (independent +
    // direct <0.05, repair bound on link-coupled frames) covers every clean
    // frame outside the footprint including boundary clean; PR (>=0.95,
    // hot>0.5) counts loud clicks only since quiet probes are gate probes
    // with sub-hot residuals by design; regroup <1e-5 holds over the full
    // render. Quiet probe frames are skipped in damage/PR counting (two
    // stated gate-probe frames, not a boundary exclusion: every boundary
    // frame is asserted).
    let frames = 1441;
    let freqs = [440.0, 660.0];
    let quiet_on = 1240;
    let quiet_off = 1290;
    let mut grid_plan = Vec::new();
    let mut start = 140;
    while start < 1341 {
        if start != quiet_on {
            grid_plan.push((start, 1, 1.0));
        }
        start += 100;
    }
    grid_plan.push((frames - 3, 3, 1.0));
    let plans = [grid_plan, Vec::new()];
    let (mut corrupted, clean, is_loud) = build_fixture(CHANNELS, frames, RATE_48K, &freqs, &plans);
    corrupted[quiet_on * CHANNELS] += 0.5;
    corrupted[quiet_off * CHANNELS] += 0.5;
    let make = |audition: bool| {
        serde_json::json!({
            "enabled": true,
            "sensitivity": 5.0,
            "link_channels": true,
            "mode": 1,
            "bands": 2,
            "crossover_hz": 4000.0,
            "frequency_skew": 0.0,
            "repair_width": 0,
            "audition_residual": audition,
        })
        .to_string()
    };
    let label = "ffi periodic eof";
    let mut repaired_handle = DeclickHandle::create(&make(false), RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(repaired_handle.info_latency(), LEGACY_LATENCY);
    assert_eq!(repaired_handle.drain_capacity(), LEGACY_LATENCY);
    let repaired = repaired_handle.process_with_drain(&corrupted);
    let mut clean_handle = DeclickHandle::create(&make(false), RATE_48K, CHANNELS, CHANNELS);
    let clean_out = clean_handle.process_with_drain(&clean);
    let mut residual_handle = DeclickHandle::create(&make(true), RATE_48K, CHANNELS, CHANNELS);
    let residual = residual_handle.process_with_drain(&corrupted);
    check_exact_finite_length(label, &repaired, CHANNELS, frames, LEGACY_LATENCY);
    check_leading_silence(label, &repaired, CHANNELS, LEGACY_LATENCY);
    let clean_delayed = manual_delayed(&clean, CHANNELS, LEGACY_LATENCY);
    let on_err = (repaired[(quiet_on + LEGACY_LATENCY) * CHANNELS]
        - clean_delayed[(quiet_on + LEGACY_LATENCY) * CHANNELS])
        .abs();
    assert!(
        on_err < 0.15,
        "{label}: quiet on-phase must repair, error={on_err}"
    );
    let off_err = (repaired[(quiet_off + LEGACY_LATENCY) * CHANNELS]
        - clean_delayed[(quiet_off + LEGACY_LATENCY) * CHANNELS])
        .abs();
    assert!(
        off_err > 0.3,
        "{label}: quiet off-phase must miss (guard), error={off_err}"
    );
    report(&format!("{label} quiet-on"), on_err);
    report(&format!("{label} quiet-off"), off_err);
    let mut worst_diff = 0.0_f32;
    let mut worst_direct = 0.0_f32;
    for at in frames - 3..frames {
        let out_idx = (at + LEGACY_LATENCY) * CHANNELS;
        let diff = (repaired[out_idx] - clean_out[out_idx]).abs();
        worst_diff = worst_diff.max(diff);
        assert!(
            diff < 0.15,
            "{label}: EOF differential at={at} error={diff}"
        );
        let direct = (repaired[out_idx] - clean_delayed[out_idx]).abs();
        worst_direct = worst_direct.max(direct);
        assert!(direct < 0.15, "{label}: EOF direct at={at} error={direct}");
    }
    report(&format!("{label} eof-diff"), worst_diff);
    report(&format!("{label} eof-direct"), worst_direct);
    let mut loud_or_probe = is_loud.clone();
    loud_or_probe[0][quiet_on] = true;
    loud_or_probe[0][quiet_off] = true;
    let mut worst_indep = 0.0_f32;
    let mut worst_direct_clean = 0.0_f32;
    for frame in LEGACY_LATENCY..frames + LEGACY_LATENCY {
        let input = frame - LEGACY_LATENCY;
        if input < SETTLED_FROM {
            continue;
        }
        for ch in 0..CHANNELS {
            if is_loud[ch][input] {
                continue;
            }
            if ch == 0 && (input == quiet_on || input == quiet_off) {
                continue;
            }
            if near_click(&loud_or_probe[ch], input, 0) {
                continue;
            }
            let out_idx = frame * CHANNELS + ch;
            let coupled = loud_or_probe[1 - ch][input];
            let pair_bound = if coupled { 0.15 } else { 0.05 };
            let indep = (clean_out[out_idx] - clean_delayed[out_idx]).abs();
            worst_indep = worst_indep.max(indep);
            assert!(
                indep < 0.05,
                "{label}: indep ch{ch} input={input} error={indep}"
            );
            let direct = (repaired[out_idx] - clean_delayed[out_idx]).abs();
            worst_direct_clean = worst_direct_clean.max(direct);
            assert!(
                direct < pair_bound,
                "{label}: direct ch{ch} input={input} coupled={coupled} error={direct}"
            );
        }
    }
    report(&format!("{label} indep-damage"), worst_indep);
    report(&format!("{label} direct-clean"), worst_direct_clean);
    // PR over loud only: quiet probes stay cold (sub-hot residuals by
    // design), so the frozen residual oracle counts them as neither hot
    // nor missed-loud; regroup holds over the full render.
    check_residual(&ResidualOracle {
        label,
        residual: &residual,
        repaired: &repaired,
        corrupted: &corrupted,
        is_click: &is_loud,
        channels: CHANNELS,
        frames,
        latency: LEGACY_LATENCY,
        width: 0,
    });
}

#[test]
fn declick_ffi_controls_dc_silence_stay_clean() {
    // C ABI controls spot: DC constant and tone-to-silence transition stay
    // transparent (1e-5 vs analytical, no repairs) through the exported
    // path, including the EOF edge with drain zeros (bridge vetoes the
    // edges; runs stay exact). Single config (3-band, width 0, 4kHz, 48k
    // stereo linked); full signal-type breadth lives in the DSP controls
    // leg. Proves the C ABI delivers identical control behavior.
    let label = "ffi controls";
    let mut dc_handle = DeclickHandle::create(
        &error_config(2, 0, 4000.0, false),
        RATE_48K,
        CHANNELS,
        CHANNELS,
    );
    assert_eq!(dc_handle.info_latency(), LEGACY_LATENCY);
    let dc = vec![0.3; FRAMES * CHANNELS];
    let dc_out = dc_handle.process_with_drain(&dc);
    check_exact_finite_length(label, &dc_out, CHANNELS, FRAMES, LEGACY_LATENCY);
    let mut worst_dc = 0.0_f32;
    for input in SETTLED_FROM..FRAMES {
        for ch in 0..CHANNELS {
            let error = (dc_out[(input + LEGACY_LATENCY) * CHANNELS + ch] - 0.3).abs();
            worst_dc = worst_dc.max(error);
            assert!(
                error < 1.0e-5,
                "{label}: dc ch{ch} input={input} error={error}"
            );
        }
    }
    report(&format!("{label} dc"), worst_dc);
    let freqs = [440.0, 660.0];
    let empty: [Vec<(usize, usize, f32)>; 2] = [Vec::new(), Vec::new()];
    let (_, mut transition, _) = build_fixture(CHANNELS, FRAMES, RATE_48K, &freqs, &empty);
    for frame in FRAMES / 2..FRAMES {
        for ch in 0..CHANNELS {
            transition[frame * CHANNELS + ch] = 0.0;
        }
    }
    let mut silence_handle = DeclickHandle::create(
        &error_config(2, 0, 4000.0, false),
        RATE_48K,
        CHANNELS,
        CHANNELS,
    );
    let silence_out = silence_handle.process_with_drain(&transition);
    check_exact_finite_length(label, &silence_out, CHANNELS, FRAMES, LEGACY_LATENCY);
    let mut worst_silence = 0.0_f32;
    for input in SETTLED_FROM..FRAMES {
        for ch in 0..CHANNELS {
            let error = (silence_out[(input + LEGACY_LATENCY) * CHANNELS + ch]
                - transition[input * CHANNELS + ch])
                .abs();
            worst_silence = worst_silence.max(error);
            assert!(
                error < 1.0e-5,
                "{label}: silence ch{ch} input={input} error={error}"
            );
        }
    }
    report(&format!("{label} silence"), worst_silence);
}

#[test]
fn declick_ffi_partitions_agree_bit_exact() {
    // Discovered public strides (single-frame up to the negotiated bound)
    // plus an oversized drain capacity all deliver the identical stream:
    // produced counts, never capacity, define the tail.
    let big = discover_stride();
    let mut strides = vec![1, 7.min(big), big];
    strides.sort_unstable();
    strides.dedup();
    let (corrupted, clean, is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let mut renders = Vec::new();
    for stride in strides {
        let mut handle =
            DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
        let mut full = handle.process_strided(&corrupted, stride);
        let capacity = handle.drain_capacity();
        full.extend_from_slice(&handle.drain_to_eof(capacity));
        renders.push(full);
    }
    let mut wide = DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
    let mut wide_full = wide.process(&corrupted);
    let capacity = wide.drain_capacity();
    wide_full.extend_from_slice(&wide.drain_to_eof(capacity + 5));
    renders.push(wide_full);
    for (index, render) in renders.iter().enumerate().skip(1) {
        assert_eq!(*render, renders[0], "partition set {index} differs");
    }
    check_cleaned(
        "ffi partitions",
        &renders[0],
        &clean,
        &is_click,
        CHANNELS,
        FRAMES,
        LATENCY,
        WIDTH,
    );
}

#[test]
fn declick_ffi_capacity_failure_retries_without_consuming() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let config = nondefault_config(false);
    let mut handle = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    let mut twin = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(handle.process(&corrupted), twin.process(&corrupted));
    // Undersized capacity fails without touching state or the buffer: the
    // poisoned destination must survive, with zeroed out-params.
    let mut poisoned = vec![f32::NAN; (LATENCY - 1) * CHANNELS];
    let mut produced = usize::MAX;
    let mut complete: std::os::raw::c_int = -1;
    // SAFETY: Live exclusive handle; the buffer holds exactly the passed
    // capacity in samples; locals are valid disjoint out-pointers.
    let code = unsafe {
        plugin_drain(
            handle.pointer,
            poisoned.as_mut_ptr(),
            LATENCY - 1,
            &mut produced,
            &mut complete,
        )
    };
    assert_eq!(code, PluginError::BufferTooSmall as i32);
    assert_eq!(produced, 0);
    assert_eq!(complete, 0);
    assert!(
        poisoned.iter().all(|v| v.is_nan()),
        "failed drain touched the buffer"
    );
    assert!(!last_error().is_empty(), "capacity failure must diagnose");
    // Null pointers fail distinctly, without writing anywhere. Each null
    // below exercises a checked rejection (no dereference happens); every
    // other pointer is live, valid, and disjoint.
    let mut sink = vec![0.0; LATENCY * CHANNELS];
    let mut sink_produced = 0usize;
    let mut sink_complete: std::os::raw::c_int = 0;
    // SAFETY: Null produced-pointer leg; handle, buffer, and complete are valid.
    assert_eq!(
        unsafe {
            plugin_drain(
                handle.pointer,
                sink.as_mut_ptr(),
                LATENCY,
                std::ptr::null_mut(),
                &mut sink_complete,
            )
        },
        PluginError::NullPointer as i32
    );
    // SAFETY: Null complete leg; handle, buffer, and produced are valid.
    assert_eq!(
        unsafe {
            plugin_drain(
                handle.pointer,
                sink.as_mut_ptr(),
                LATENCY,
                &mut sink_produced,
                std::ptr::null_mut(),
            )
        },
        PluginError::NullPointer as i32
    );
    // SAFETY: Null output with positive capacity; handle and out-pointers valid.
    assert_eq!(
        unsafe {
            plugin_drain(
                handle.pointer,
                std::ptr::null_mut(),
                LATENCY,
                &mut sink_produced,
                &mut sink_complete,
            )
        },
        PluginError::NullPointer as i32
    );
    sink_produced = usize::MAX;
    sink_complete = -1;
    // SAFETY: Null handle leg; buffer and out-pointers are valid.
    assert_eq!(
        unsafe {
            plugin_drain(
                std::ptr::null_mut(),
                sink.as_mut_ptr(),
                LATENCY,
                &mut sink_produced,
                &mut sink_complete,
            )
        },
        PluginError::NullPointer as i32
    );
    assert_eq!(sink_produced, usize::MAX);
    assert_eq!(sink_complete, -1);
    // Retry with adequate capacity delivers the full tail, identical to the
    // engaged twin that never saw a failure — nothing was consumed.
    let retried = handle.drain_to_eof(LATENCY);
    let twinned = twin.drain_to_eof(LATENCY);
    assert_eq!(retried.len(), LATENCY * CHANNELS);
    assert_eq!(retried, twinned);
}

#[test]
fn declick_ffi_repeated_complete_and_reset_reuse() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let mut handle = DeclickHandle::create(&nondefault_config(false), RATE_48K, CHANNELS, CHANNELS);
    let first = handle.process_with_drain(&corrupted);
    // Completed drains stay complete with zero production and write nothing.
    let capacity = handle.drain_capacity();
    for round in 0..3 {
        let (code, buffer, produced, complete) = handle.drain_once(capacity);
        assert_eq!(code, 0, "round {round}");
        assert_eq!(produced, 0, "round {round}");
        assert!(complete, "round {round}");
        assert!(
            buffer.iter().all(|v| v.is_nan()),
            "round {round}: completed drain must not write"
        );
    }
    // Reset returns to normal processing: re-render equals first render.
    handle.reset();
    assert_eq!(handle.process_with_drain(&corrupted), first);
}

#[test]
fn declick_ffi_raw_and_preset_restore_drain_exact_eof() {
    let (corrupted, _clean, _is_click) = click_fixture(CHANNELS, FRAMES, RATE_48K);
    let config = nondefault_config(false);
    let mut source = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    let _ = source.process(&corrupted[..512 * CHANNELS]);
    let saved = source.save();
    // Raw state into a differently configured handle, then C drain to EOF.
    let mut raw = DeclickHandle::create("{}", RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(raw.load(&saved), 0, "{}", last_error());
    assert_eq!(raw.info_latency(), LATENCY);
    let mut twin = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    let restored = raw.process_with_drain(&corrupted);
    let expected = twin.process_with_drain(&corrupted);
    assert_eq!(restored.len(), (FRAMES + LATENCY) * CHANNELS);
    assert_eq!(restored, expected);
    // Preset document into a differently configured handle, ditto.
    let mut len = 0usize;
    let name = CString::new("declick-eof").unwrap();
    let document = plugin_export_preset_json(source.pointer, name.as_ptr(), &mut len);
    assert!(!document.is_null(), "{}", last_error());
    // SAFETY: FFI owns exactly len bytes until freed below.
    let bytes = unsafe { std::slice::from_raw_parts(document, len) }.to_vec();
    plugin_free_state(document, len);
    let mut preset = DeclickHandle::create("{}", RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(
        plugin_import_preset_json(preset.pointer, bytes.as_ptr(), bytes.len()),
        0,
        "{}",
        last_error()
    );
    assert_eq!(preset.info_latency(), LATENCY);
    let mut twin2 = DeclickHandle::create(&config, RATE_48K, CHANNELS, CHANNELS);
    assert_eq!(
        preset.process_with_drain(&corrupted),
        twin2.process_with_drain(&corrupted)
    );
}
