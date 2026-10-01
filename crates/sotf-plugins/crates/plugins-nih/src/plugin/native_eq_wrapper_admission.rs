//! EQ state admission through real CLAP wrapper callbacks.
//!
//! The vendor gate is covered by unit tests in `nih-plug/src/wrapper/state.rs`.
//! These tests drive the actual `Wrapper<SotfEQ>` paths: audio-thread refusal
//! via `set_state_inner(true)` preserves params, DSP history, and latency,
//! while control-thread retry via `set_state_inner(false)` applies the same
//! retained state object. Host-visible params are read back through the real
//! CLAP `params` extension, and audio is processed through the real `process`
//! callback.
//!
//! Allocating host setup (wrapper construction, activation, history audio,
//! state snapshots) stays outside the realtime guard. Only the refusal call
//! itself runs under `assert_no_alloc`, with its boolean asserted outside
//! so no assert formatting can allocate inside the guard. Heap-level
//! no-free is pinned by the vendor `CountingAllocator` gate test; here
//! pre/post snapshots plus live param/audio/latency equality prove the
//! refusal neither mutates nor drops retained or live state.

// Rust guideline compliant 2026-02-21
use super::SotfEQ;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_plugin_params};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use nih_plug::wrapper::clap::Wrapper;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::ThreadId;

const FRAMES: usize = 128;
const SAMPLE_RATE: f64 = 48_000.0;

static STAGE: AtomicUsize = AtomicUsize::new(0);

/// Record a harness stage without allocating.
///
/// Stores an atomic marker and writes a static line to stderr.
/// Callers invoke this only outside allocation guards and C callbacks.
fn mark_stage(stage: usize, line: &'static str) {
    STAGE.store(stage, Ordering::Release);
    let _ = std::io::Write::write_all(&mut std::io::stderr(), line.as_bytes());
}

struct HostState {
    callback_requests: AtomicUsize,
    restart_requests: AtomicUsize,
    restart_was_on_main_thread: AtomicBool,
    main_thread: ThreadId,
}

/// Read host state without panicking or allocating.
///
/// Returns `None` on null pointers instead of asserting: these
/// callbacks cross `extern "C"` (where unwinding is unsound) and may
/// run inside the global no-alloc guard on the audio path.
///
/// # Safety
///
/// A non-null `host_data` must point to a live boxed `HostState`; the
/// harness keeps each box alive until its wrapper is dropped.
unsafe fn state_from_host<'a>(host: *const clap_host) -> Option<&'a HostState> {
    if host.is_null() {
        return None;
    }
    // SAFETY: each test host points to a boxed HostState kept alive until its wrapper is dropped.
    let data = unsafe { (*host).host_data };
    if data.is_null() {
        return None;
    }
    Some(unsafe { &*data.cast::<HostState>() })
}

unsafe extern "C" fn no_host_extension(
    _host: *const clap_host,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn request_restart(host: *const clap_host) {
    // SAFETY: nulls return early inside; live hosts outlive the wrapper.
    let state = unsafe { state_from_host(host) };
    let Some(state) = state else {
        return;
    };
    state.restart_was_on_main_thread.store(
        std::thread::current().id() == state.main_thread,
        Ordering::Release,
    );
    state.restart_requests.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn request_callback(host: *const clap_host) {
    // SAFETY: nulls return early inside; live hosts outlive the wrapper.
    let state = unsafe { state_from_host(host) };
    let Some(state) = state else {
        return;
    };
    state.callback_requests.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn request_process(_host: *const clap_host) {}

struct TestPlugin {
    // The wrapper contains a raw host pointer; drop it before freeing either boxed host object.
    wrapper: std::sync::Arc<Wrapper<SotfEQ>>,
    _host: Box<clap_host>,
    _host_state: Box<HostState>,
    active: Cell<bool>,
}

impl TestPlugin {
    fn new() -> Self {
        let host_state = Box::new(HostState {
            callback_requests: AtomicUsize::new(0),
            restart_requests: AtomicUsize::new(0),
            restart_was_on_main_thread: AtomicBool::new(false),
            main_thread: std::thread::current().id(),
        });
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: (&*host_state as *const HostState).cast_mut().cast(),
            name: c"EQ admission test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_host_extension),
            request_restart: Some(request_restart),
            request_process: Some(request_process),
            request_callback: Some(request_callback),
        });

        // SAFETY: both boxed host objects outlive the returned wrapper and all callback invocations.
        let wrapper = unsafe { Wrapper::<SotfEQ>::new(&*host) };
        let test_plugin = Self {
            wrapper,
            _host: host,
            _host_state: host_state,
            active: Cell::new(false),
        };
        let plugin = test_plugin.clap_plugin();
        unsafe {
            assert!(((*plugin).init.unwrap())(plugin));
        };
        test_plugin
    }

    fn clap_plugin(&self) -> *const clap_plugin {
        self.wrapper.clap_plugin.as_ptr()
    }

    fn state(&self) -> &HostState {
        &self._host_state
    }

    fn get_param(&self, param_id: &str) -> f64 {
        let plugin = self.clap_plugin();
        let params = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                .cast::<clap_plugin_params>()
        };
        assert!(!params.is_null());
        let mut value = f64::NAN;
        assert!(unsafe {
            ((*params).get_value.unwrap())(plugin, hash_param_id(param_id), &mut value)
        });
        value
    }

    fn get_state(&self) -> PluginState {
        self.wrapper.get_state_object()
    }

    fn set_state_audio(&self, state: &mut PluginState) -> bool {
        self.wrapper.set_state_inner(state, true)
    }

    fn set_state_control(&self, state: &mut PluginState) -> bool {
        self.wrapper.set_state_inner(state, false)
    }

    fn latency(&self) -> u32 {
        self.wrapper.current_latency.load(Ordering::Acquire)
    }

    fn activate(&self) {
        let plugin = self.clap_plugin();
        let activated =
            unsafe { ((*plugin).activate.unwrap())(plugin, SAMPLE_RATE, 1, FRAMES as u32) };
        assert!(activated);
        unsafe {
            assert!(((*plugin).start_processing.unwrap())(plugin));
        }
        self.active.set(true);
    }

    fn process_block(&self, first_frame: usize) -> [Vec<f32>; 2] {
        let mut left_input = vec![0.0_f32; FRAMES];
        let mut right_input = vec![0.0_f32; FRAMES];
        for frame in 0..FRAMES {
            let time = (first_frame + frame) as f32 / SAMPLE_RATE as f32;
            let sample = (std::f32::consts::TAU * 120.0 * time).sin() * 0.1;
            left_input[frame] = sample;
            right_input[frame] = sample * 0.7;
        }
        let mut left_output = vec![f32::NAN; FRAMES];
        let mut right_output = vec![f32::NAN; FRAMES];
        let mut input_channels = [left_input.as_mut_ptr(), right_input.as_mut_ptr()];
        let mut output_channels = [left_output.as_mut_ptr(), right_output.as_mut_ptr()];
        let input_bus = clap_audio_buffer {
            data32: input_channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_bus = clap_audio_buffer {
            data32: output_channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let process = clap_process {
            steady_time: -1,
            frames_count: FRAMES as u32,
            transport: std::ptr::null(),
            audio_inputs: &input_bus,
            audio_outputs: &mut output_bus,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: std::ptr::null(),
            out_events: std::ptr::null(),
        };
        let status =
            unsafe { ((*self.clap_plugin()).process.unwrap())(self.clap_plugin(), &process) };
        assert_ne!(status, CLAP_PROCESS_ERROR);
        assert!(
            left_output
                .iter()
                .chain(&right_output)
                .all(|sample| sample.is_finite())
        );
        [left_output, right_output]
    }

    fn process_blocks(&self, first_frame: usize, block_count: usize) -> [Vec<f32>; 2] {
        let mut output = [
            Vec::with_capacity(block_count * FRAMES),
            Vec::with_capacity(block_count * FRAMES),
        ];
        for block in 0..block_count {
            let current = self.process_block(first_frame + block * FRAMES);
            output[0].extend_from_slice(&current[0]);
            output[1].extend_from_slice(&current[1]);
        }
        output
    }
}

impl Drop for TestPlugin {
    fn drop(&mut self) {
        if self.active.get() {
            let plugin = self.clap_plugin();
            // SAFETY: wrapper, host, and callback state are all alive until after this Drop body.
            unsafe {
                ((*plugin).stop_processing.unwrap())(plugin);
                ((*plugin).deactivate.unwrap())(plugin);
            }
            self.active.set(false);
        }
    }
}

fn hash_param_id(id: &str) -> u32 {
    let mut hash = 0_u32;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    hash & !(1 << 31)
}

fn assert_same_audio(left: &[Vec<f32>; 2], right: &[Vec<f32>; 2]) {
    assert_eq!(left[0].len(), right[0].len());
    assert_eq!(left[1].len(), right[1].len());
    for channel in 0..2 {
        for frame in 0..left[channel].len() {
            assert!((left[channel][frame] - right[channel][frame]).abs() < 1.0e-7);
        }
    }
}

fn assert_audio_differs(left: &[Vec<f32>; 2], right: &[Vec<f32>; 2]) {
    assert_eq!(left[0].len(), right[0].len());
    let mut max_diff = 0.0_f32;
    for channel in 0..2 {
        for frame in 0..left[channel].len() {
            max_diff = max_diff.max((left[channel][frame] - right[channel][frame]).abs());
        }
    }
    assert!(
        max_diff > 1.0e-5,
        "expected audible gain change, max diff {max_diff}"
    );
}

/// Assert a float param holds an exact plain value.
///
/// Matches the `ParamValue` variant directly because the vendor type
/// intentionally omits `PartialEq`; exact equality is correct here since
/// 0 dB and 6 dB are exactly representable.
fn assert_param_f32(state: &PluginState, key: &str, expected: f32, message: &str) {
    match state.params.get(key) {
        Some(ParamValue::F32(actual)) => {
            assert!(*actual == expected, "{message}: expected {expected}, got {actual}")
        }
        other => panic!("{message}: expected F32({expected}), got {other:?}"),
    }
}

/// Canonical snapshot of a populated state object.
///
/// `PluginState` intentionally omits `PartialEq`; serializing the ordered
/// maps yields a byte comparison that proves refusal left the retained
/// object untouched. Callers take snapshots outside allocation guards.
fn state_snapshot(state: &PluginState) -> Vec<u8> {
    serde_json::to_vec(state).expect("populated EQ state serializes")
}

#[test]
fn clap_eq_audio_refusal_preserves_state_and_control_retry_applies() {
    let subject = TestPlugin::new();
    let control = TestPlugin::new();
    subject.activate();
    control.activate();
    mark_stage(1, "EQ-ADMISSION-STAGE 1: setup\n");

    // Build identical history before the restore attempt.
    assert_same_audio(&subject.process_blocks(0, 8), &control.process_blocks(0, 8));
    let latency_before = subject.latency();
    // CLAP reports continuous floats as normalized 0..1; gain 0 dB is 0.5.
    let gain_before = subject.get_param("band_0_gain");
    assert!((gain_before - 0.5).abs() < 1.0e-6);
    mark_stage(2, "EQ-ADMISSION-STAGE 2: history\n");

    // Capture a populated state and stage a realtime gain change (plain 6 dB).
    let mut retained = subject.get_state();
    assert_param_f32(
        &retained,
        "band_0_gain",
        0.0,
        "populated EQ state carries band gains",
    );
    retained
        .params
        .insert("band_0_gain".to_string(), ParamValue::F32(6.0));
    let snapshot_before = state_snapshot(&retained);
    mark_stage(3, "EQ-ADMISSION-STAGE 3: staged\n");

    // Audio-thread handoff refuses without parse, migration, or frees.
    // Prior params, DSP history, and latency stay untouched. Only the
    // refusal runs under the no-alloc guard; its boolean is asserted
    // outside so no assert formatting can allocate inside the guard.
    mark_stage(4, "EQ-ADMISSION-STAGE 4: guard-enter\n");
    let refused = assert_no_alloc::assert_no_alloc(|| !subject.set_state_audio(&mut retained));
    mark_stage(5, "EQ-ADMISSION-STAGE 5: guard-exit\n");
    assert!(refused, "EQ audio-thread restore must refuse");
    // The gated refusal returns before GUI scheduling, so it must not
    // have poked the host for callbacks or restarts.
    assert_eq!(
        subject.state().callback_requests.load(Ordering::Acquire),
        0,
        "refusal must not schedule host callbacks"
    );
    assert_eq!(
        subject.state().restart_requests.load(Ordering::Acquire),
        0,
        "refusal must not request host restart"
    );
    assert!(
        !subject
            .state()
            .restart_was_on_main_thread
            .load(Ordering::Acquire),
        "no restart ran, so no restart thread was recorded"
    );
    let snapshot_after = state_snapshot(&retained);
    assert_eq!(
        snapshot_after, snapshot_before,
        "refusal must not mutate the retained state object"
    );
    assert_eq!(subject.get_param("band_0_gain"), gain_before);
    assert_param_f32(
        &subject.get_state(),
        "band_0_gain",
        0.0,
        "live plain gain stays 0 dB after refusal",
    );
    assert_eq!(subject.latency(), latency_before);
    assert_same_audio(
        &subject.process_blocks(8 * FRAMES, 8),
        &control.process_blocks(8 * FRAMES, 8),
    );
    mark_stage(6, "EQ-ADMISSION-STAGE 6: refused-checks\n");

    // The retry runs on the test main thread, which is the captured
    // control thread for this harness.
    assert_eq!(
        std::thread::current().id(),
        subject.state().main_thread,
        "control-thread retry must run on the main thread"
    );
    // Main-thread retry applies the same retained object (plain 6 dB,
    // normalized (6+24)/48 = 0.625).
    assert!(
        subject.set_state_control(&mut retained),
        "control-thread retry must apply the retained EQ state"
    );
    assert!((subject.get_param("band_0_gain") - 0.625).abs() < 1.0e-6);
    assert_param_f32(
        &subject.get_state(),
        "band_0_gain",
        6.0,
        "live plain gain is 6 dB after retry",
    );
    assert_eq!(subject.latency(), latency_before);
    assert_audio_differs(
        &subject.process_blocks(16 * FRAMES, 8),
        &control.process_blocks(16 * FRAMES, 8),
    );
    mark_stage(7, "EQ-ADMISSION-STAGE 7: retried\n");
}
