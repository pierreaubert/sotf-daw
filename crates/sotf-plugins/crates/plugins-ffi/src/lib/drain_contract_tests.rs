//! Generic C ABI drain-contract tests: capacity discovery, bound
//! enforcement, repeated-call completion, and zero-bound tails.
//!
//! Every test drives the public C ABI exclusively (construction, info,
//! processing, the capacity query, and the fused drain) on non-Declick
//! plugins, so the drain contract is proven generic rather than
//! Declick-shaped. Assertions stay behavioral — boundary pairs around the
//! queried bound, produced-prefix discipline, completion flags — and pin
//! no plugin-internal chunk or ring constants.
//!
//! Delay carries a finite non-latency tail (zero reported latency with a
//! positive drain bound); Gain carries no tail at all (zero bound), which
//! pins zero-capacity semantics including a null output buffer; A/B
//! Compare (dual delay paths, plus a gain-bearing companion now that
//! Gain declares identity geometry) pins the zero-frame drain context
//! both its preparation and its drain step require, with a real
//! multi-call tail through the mixer and positioned echo content pins
//! on the gain-bearing companion.

// Rust guideline compliant 2026-02-21

use crate::{
    PluginError, PluginHandle, plugin_create, plugin_destroy, plugin_drain, plugin_free_string,
    plugin_get_drain_capacity_frames, plugin_get_info_json, plugin_get_last_error, plugin_process,
};
use std::ffi::{CStr, CString};

/// C ABI fixture rate.
const RATE_48K: u32 = 48_000;
/// First process stride tried by adaptive public probing.
const PROCESS_PROBE_START: usize = 256;
/// Hang guard for repeated-call drain loops (far above any finite tail
/// exercised here, small enough to fail fast).
const DRAIN_CALL_CAP: usize = 4096;

struct DrainHandle {
    pointer: *mut PluginHandle,
    inputs: usize,
    outputs: usize,
}

impl DrainHandle {
    fn create(kind: &str, config: &str, rate: u32, inputs: usize, outputs: usize) -> Self {
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

    fn info_latency(&self) -> usize {
        let info_pointer = plugin_get_info_json(self.pointer);
        assert!(!info_pointer.is_null());
        // SAFETY: The C API owns a valid NUL-terminated string until freed below.
        let info: serde_json::Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(info_pointer) }.to_bytes()).unwrap();
        plugin_free_string(info_pointer);
        info["latency_samples"].as_u64().unwrap() as usize
    }

    fn drain_capacity(&self) -> usize {
        let mut capacity = usize::MAX;
        // SAFETY: The handle is live and exclusively held; `capacity` is a
        // valid local out-pointer that outlives the call.
        let code = unsafe { plugin_get_drain_capacity_frames(self.pointer, &mut capacity) };
        assert_eq!(code, 0, "capacity query failed: {}", last_error());
        assert_ne!(capacity, usize::MAX, "capacity unwritten");
        capacity
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        assert_eq!(input.len() % self.inputs, 0, "complete frames required");
        let frames = input.len() / self.inputs;
        let mut output = vec![f32::NAN; frames * self.outputs];
        let mut start = 0;
        // Adaptive public probing: oversized blocks fail cleanly without
        // consuming state, so halve until the negotiated bound accepts.
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

    /// One exported drain call with an oversized poisoned destination.
    /// Returns (status, buffer, produced, complete).
    fn drain_once(
        &mut self,
        capacity_frames: usize,
        extra_frames: usize,
    ) -> (i32, Vec<f32>, usize, bool) {
        let mut output = vec![f32::NAN; (capacity_frames + extra_frames) * self.outputs];
        let mut produced = usize::MAX;
        let mut complete: std::os::raw::c_int = -1;
        // SAFETY: The handle is live and exclusively held; the buffer holds
        // more than `capacity_frames * outputs` samples and the locals are
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

    /// Drain to completion through the exported API, collecting exactly
    /// the produced frames. Returns (tail, calls). Every call proves its
    /// produced prefix is finite and its poisoned suffix untouched.
    fn drain_full(&mut self, capacity_frames: usize) -> (Vec<f32>, usize) {
        let mut tail = Vec::new();
        let mut calls = 0;
        for _ in 0..DRAIN_CALL_CAP {
            let (code, buffer, produced, complete) = self.drain_once(capacity_frames, 64);
            assert_eq!(code, 0, "drain failed: {}", last_error());
            calls += 1;
            assert!(
                buffer[..produced * self.outputs]
                    .iter()
                    .all(|v| v.is_finite()),
                "non-finite tail audio"
            );
            assert!(
                buffer[produced * self.outputs..].iter().all(|v| v.is_nan()),
                "drain wrote past the produced prefix"
            );
            if !complete {
                assert_eq!(
                    produced, capacity_frames,
                    "non-final full-capacity call must fill its destination"
                );
            }
            tail.extend_from_slice(&buffer[..produced * self.outputs]);
            if complete {
                return (tail, calls);
            }
        }
        panic!("drain did not complete within {DRAIN_CALL_CAP} calls");
    }
}

impl Drop for DrainHandle {
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

#[test]
fn drain_capacity_query_differs_from_latency_on_finite_tail() {
    // A 10 ms feedback-free delay reports zero latency but drains a
    // positive finite tail: the capacity query must disagree with latency
    // and pin the enforced bound behaviorally (one below fails, the bound
    // itself succeeds) without consuming state on failure.
    let config = r#"{"delay_ms": 10.0, "feedback": 0.0, "mix": 1.0}"#;
    let mut handle = DrainHandle::create("Delay", config, RATE_48K, 2, 2);
    assert_eq!(handle.info_latency(), 0);
    let bound = handle.drain_capacity();
    assert!(bound > 0, "finite tail must report a positive bound");
    assert_eq!(handle.drain_capacity(), bound, "query must repeat stably");
    // Feed an impulse so the tail is non-degenerate, then prove the
    // boundary pair: bound - 1 fails cleanly, bound succeeds.
    let mut impulse = vec![0.0; 512 * 2];
    impulse[0] = 1.0;
    impulse[1] = 1.0;
    let _ = handle.process(&impulse);
    assert_eq!(handle.drain_capacity(), bound, "query stable after input");
    let mut poisoned = vec![f32::NAN; (bound - 1) * 2];
    let mut produced = usize::MAX;
    let mut complete: std::os::raw::c_int = -1;
    // SAFETY: Live exclusive handle; the buffer holds exactly the passed
    // capacity in samples; locals are valid disjoint out-pointers.
    let code = unsafe {
        plugin_drain(
            handle.pointer,
            poisoned.as_mut_ptr(),
            bound - 1,
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
    let (code, _, produced, _) = handle.drain_once(bound, 0);
    assert_eq!(code, 0, "bound capacity must succeed: {}", last_error());
    assert_eq!(produced, bound, "non-final call must fill its destination");
}

#[test]
fn drain_delay_tail_completes_over_repeated_calls() {
    // The full delay tail drains over repeated full-capacity calls to a
    // stable completion, identically on twin handles; oversized poisoned
    // destinations prove the produced prefix discipline on every call.
    let config = r#"{"delay_ms": 10.0, "feedback": 0.0, "mix": 1.0}"#;
    let mut handle = DrainHandle::create("Delay", config, RATE_48K, 2, 2);
    let mut twin = DrainHandle::create("Delay", config, RATE_48K, 2, 2);
    let bound = handle.drain_capacity();
    assert_eq!(twin.drain_capacity(), bound);
    let mut impulse = vec![0.0; 512 * 2];
    impulse[0] = 1.0;
    impulse[1] = 1.0;
    let processed = handle.process(&impulse);
    assert_eq!(twin.process(&impulse), processed);
    let (tail, calls) = handle.drain_full(bound);
    let (twin_tail, twin_calls) = twin.drain_full(bound);
    assert!(calls > 1, "finite tail must need repeated calls");
    assert_eq!(calls, twin_calls, "drain schedule must be deterministic");
    assert!(
        tail.len() > bound * 2,
        "tail must exceed one full-capacity call"
    );
    assert_eq!(tail, twin_tail, "twin tails must agree bit-exactly");
    // Completion is stable: a further call produces nothing and writes
    // nothing.
    let (code, buffer, produced, complete) = handle.drain_once(bound, 64);
    assert_eq!(code, 0);
    assert_eq!(produced, 0);
    assert!(complete);
    assert!(
        buffer.iter().all(|v| v.is_nan()),
        "completed drain must not write"
    );
}

#[test]
fn drain_ab_compare_with_delay_path_completes_through_fused_call() {
    // A/B Compare rejects any nonzero-frame drain context in BOTH
    // begin_drain and drain, so this fused C call only succeeds with the
    // zero-frame host convention (the pre-fix capacity-sized context
    // failed here with ProcessingFailed). Both paths use delay children,
    // which declare the identity frame geometry drain preparation
    // requires of every active child. (Gain opts in since r7 — see the
    // gain-bearing companion below — so this regression now pins the
    // zero-frame context while the companion pins the opt-in
    // integration; both stand.) The delay tails give the drain a real
    // multi-call tail to traverse through the mixer.
    let config = r#"{
        "path_a": {"type": "Plugin", "plugin_type": "delay", "parameters": {"delay_ms": 5.0, "feedback": 0.0, "mix": 1.0}},
        "path_b": {"type": "Plugin", "plugin_type": "delay", "parameters": {"delay_ms": 10.0, "feedback": 0.0, "mix": 1.0}},
        "auto_gain_enabled": false
    }"#;
    let mut handle = DrainHandle::create("ABCompare", config, RATE_48K, 2, 2);
    let bound = handle.drain_capacity();
    assert!(bound > 0, "delay path must report a positive bound");
    let mut impulse = vec![0.0; 512 * 2];
    impulse[0] = 1.0;
    impulse[1] = 1.0;
    let processed = handle.process(&impulse);
    assert!(processed.iter().all(|v| v.is_finite()));
    // The mixer may report zero-frame non-complete steps while pumping
    // children, so this loop pins prefix discipline and finiteness per
    // call without demanding full destinations before completion.
    let mut tail = Vec::new();
    let mut calls = 0;
    let mut completed = false;
    for _ in 0..DRAIN_CALL_CAP {
        let (code, buffer, produced, complete) = handle.drain_once(bound, 64);
        assert_eq!(code, 0, "context-checking drain failed: {}", last_error());
        calls += 1;
        let produced_samples = produced * handle.outputs;
        assert!(buffer[..produced_samples].iter().all(|v| v.is_finite()));
        assert!(
            buffer[produced_samples..].iter().all(|v| v.is_nan()),
            "drain wrote past the produced prefix"
        );
        tail.extend_from_slice(&buffer[..produced_samples]);
        if complete {
            completed = true;
            break;
        }
    }
    assert!(
        completed,
        "drain did not complete within {DRAIN_CALL_CAP} calls"
    );
    assert!(calls > 1, "delay tail must need repeated calls");
    assert!(!tail.is_empty(), "delay tail must produce audio");
}

#[test]
fn drain_ab_compare_with_gain_path_completes_through_fused_call() {
    // Gain-bearing companion to the dual-delay regression: Gain now
    // declares the identity frame geometry A/B Compare drain
    // preparation requires, so the gain-vs-delay config drains to
    // completion through the fused C call. Completion mechanics plus
    // positioned echo pins (no natural-tail-length claim: the contract
    // does not define it; produced counts delimit the tail).
    //
    // Fixture derivation, all from public config (48 kHz, 10 ms delay =
    // 480 samples exact, feedback-free single echoes, wet-only mix,
    // unity impulse, pure-B mix, auto-gain off): a 512-frame stream
    // with unit impulses at frames 0 and 511 puts the first echo at
    // stream frame 480 (inside processed audio) and the second at
    // stream frame 991, i.e. drain frame 479. Nominal echo value 1.0
    // (impulse × integer-delay tap × wet-only × mixer unity); the ±1e-3
    // admits float plumbing and settled-smoother dust only, while any
    // structural weight error (halved, zeroed, misplaced echo) misses
    // by 100× or more. The 0.5 coarse bound sits at half nominal and
    // catches silent/truncated/degraded tails.
    let config = r#"{
        "path_a": {"type": "Plugin", "plugin_type": "gain", "parameters": {"gain_db": 0.0}},
        "path_b": {"type": "Plugin", "plugin_type": "delay", "parameters": {"delay_ms": 10.0, "feedback": 0.0, "mix": 1.0}},
        "mix": 1.0,
        "auto_gain_enabled": false
    }"#;
    let mut handle = DrainHandle::create("ABCompare", config, RATE_48K, 2, 2);
    let bound = handle.drain_capacity();
    assert!(bound > 0, "delay path must report a positive bound");
    let mut impulse = vec![0.0; 512 * 2];
    impulse[0] = 1.0;
    impulse[1] = 1.0;
    impulse[511 * 2] = 1.0;
    impulse[511 * 2 + 1] = 1.0;
    let processed = handle.process(&impulse);
    assert!(processed.iter().all(|v| v.is_finite()));
    for ch in 0..2 {
        let echo = processed[480 * 2 + ch];
        assert!(
            (echo - 1.0).abs() < 1.0e-3,
            "processed echo ch{ch} must be unity, got {echo}"
        );
    }
    let mut tail = Vec::new();
    let mut calls = 0;
    let mut completed = false;
    for _ in 0..DRAIN_CALL_CAP {
        let (code, buffer, produced, complete) = handle.drain_once(bound, 64);
        assert_eq!(code, 0, "gain-bearing drain failed: {}", last_error());
        calls += 1;
        let produced_samples = produced * handle.outputs;
        assert!(buffer[..produced_samples].iter().all(|v| v.is_finite()));
        assert!(
            buffer[produced_samples..].iter().all(|v| v.is_nan()),
            "drain wrote past the produced prefix"
        );
        tail.extend_from_slice(&buffer[..produced_samples]);
        if complete {
            completed = true;
            break;
        }
    }
    assert!(
        completed,
        "drain did not complete within {DRAIN_CALL_CAP} calls"
    );
    assert!(calls > 1, "delay tail must need repeated calls");
    assert!(!tail.is_empty(), "delay tail must produce audio");
    // Second echo lands at drain frame 479 on both channels; a silent
    // or truncated tail fails here and on the coarse peak below.
    for ch in 0..2 {
        let echo = tail[479 * handle.outputs + ch];
        assert!(
            (echo - 1.0).abs() < 1.0e-3,
            "drain echo ch{ch} must be unity, got {echo}"
        );
    }
    let peak = tail.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
    assert!(peak > 0.5, "tail peak {peak} must exceed half nominal");
}

#[test]
fn drain_zero_bound_plugin_completes_immediately() {
    // Gain has no tail: the query reports zero, and the fused drain
    // completes at once — including with a null output at zero capacity.
    let handle = DrainHandle::create("Gain", "{}", RATE_48K, 2, 2);
    assert_eq!(handle.info_latency(), 0);
    assert_eq!(handle.drain_capacity(), 0);
    let mut produced = usize::MAX;
    let mut complete: std::os::raw::c_int = -1;
    // SAFETY: Live exclusive handle; null output is allowed at zero
    // capacity; locals are valid disjoint out-pointers.
    let code = unsafe {
        plugin_drain(
            handle.pointer,
            std::ptr::null_mut(),
            0,
            &mut produced,
            &mut complete,
        )
    };
    assert_eq!(
        code,
        0,
        "zero-capacity drain must succeed: {}",
        last_error()
    );
    assert_eq!(produced, 0);
    assert_eq!(complete, 1);
    // An oversized destination also completes at once, writing nothing.
    let mut poisoned = vec![f32::NAN; 8 * 2];
    // SAFETY: Live exclusive handle; the buffer exceeds the passed
    // capacity in samples; locals are valid disjoint out-pointers.
    let code = unsafe {
        plugin_drain(
            handle.pointer,
            poisoned.as_mut_ptr(),
            8,
            &mut produced,
            &mut complete,
        )
    };
    assert_eq!(code, 0);
    assert_eq!(produced, 0);
    assert_eq!(complete, 1);
    assert!(
        poisoned.iter().all(|v| v.is_nan()),
        "tailless drain must not write"
    );
}

#[test]
fn drain_capacity_query_rejects_nulls() {
    // Null handles and out-pointers fail distinctly without writing.
    let handle = DrainHandle::create("Gain", "{}", RATE_48K, 2, 2);
    let mut capacity = usize::MAX;
    // SAFETY: Null handle exercises the checked rejection; the local is a
    // valid out-pointer.
    let code = unsafe { plugin_get_drain_capacity_frames(std::ptr::null_mut(), &mut capacity) };
    assert_eq!(code, PluginError::NullPointer as i32);
    assert_eq!(capacity, usize::MAX);
    // SAFETY: Null out-pointer exercises the checked rejection; the handle
    // is live and exclusively held.
    let code = unsafe { plugin_get_drain_capacity_frames(handle.pointer, std::ptr::null_mut()) };
    assert_eq!(code, PluginError::NullPointer as i32);
}
