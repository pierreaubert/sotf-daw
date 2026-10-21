//! Exercise the feature-selected Upmixer factory, state streams and audio through CLAP callbacks.

// Rust guideline compliant 2026-02-21
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::ext::audio_ports::{
    CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency, clap_plugin_latency};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, clap_host_params, clap_param_clear_flags, clap_param_rescan_flags,
};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use nih_plug::wrapper::state::{ParamValue, PluginState};
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, ThreadId};

const MAX_FRAMES: usize = 257;
const ID: &CStr = c"org.spinorama.sotf.upmixer";

struct HostState {
    callbacks: AtomicUsize,
    restarts: AtomicUsize,
    latency_changes: AtomicUsize,
    rescans: AtomicUsize,
    wrong_thread: AtomicBool,
    main_thread: ThreadId,
}

unsafe fn host_state<'a>(host: *const clap_host) -> &'a HostState {
    // SAFETY: Native owns the boxed host/state until after plugin destroy; all
    // native calls are synchronous and every callback receives that host pointer.
    unsafe { &*(*host).host_data.cast::<HostState>() }
}
unsafe extern "C" fn extension(_: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return ptr::null();
    }
    // SAFETY: CLAP supplies a valid NUL-terminated extension name during this call.
    match unsafe { CStr::from_ptr(id) } {
        name if name == CLAP_EXT_LATENCY => (&HOST_LATENCY as *const clap_host_latency).cast(),
        name if name == CLAP_EXT_PARAMS => (&HOST_PARAMS as *const clap_host_params).cast(),
        _ => ptr::null(),
    }
}
unsafe extern "C" fn request_callback(host: *const clap_host) {
    // SAFETY: the callback's host data is retained by Native throughout its lifetime.
    unsafe { host_state(host) }
        .callbacks
        .fetch_add(1, Ordering::AcqRel);
}
unsafe extern "C" fn restart(host: *const clap_host) {
    // SAFETY: the callback's host data is retained by Native throughout its lifetime.
    let state = unsafe { host_state(host) };
    state.wrong_thread.fetch_or(
        thread::current().id() != state.main_thread,
        Ordering::AcqRel,
    );
    state.restarts.fetch_add(1, Ordering::AcqRel);
}
unsafe extern "C" fn latency_changed(host: *const clap_host) {
    // SAFETY: the callback's host data is retained by Native throughout its lifetime.
    let state = unsafe { host_state(host) };
    state.wrong_thread.fetch_or(
        thread::current().id() != state.main_thread,
        Ordering::AcqRel,
    );
    state.latency_changes.fetch_add(1, Ordering::AcqRel);
}
unsafe extern "C" fn rescan(host: *const clap_host, _: clap_param_rescan_flags) {
    // SAFETY: the callback's host data is retained by Native throughout its lifetime.
    unsafe { host_state(host) }
        .rescans
        .fetch_add(1, Ordering::AcqRel);
}
unsafe extern "C" fn clear(_: *const clap_host, _: u32, _: clap_param_clear_flags) {}
unsafe extern "C" fn no_request(_: *const clap_host) {}
static HOST_LATENCY: clap_host_latency = clap_host_latency {
    changed: Some(latency_changed),
};
static HOST_PARAMS: clap_host_params = clap_host_params {
    rescan: Some(rescan),
    clear: Some(clear),
    request_flush: Some(no_request),
};

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    max_read: usize,
}
unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let Ok(size) = usize::try_from(size) else {
        return -1;
    };
    // SAFETY: load() exclusively retains this reader during the synchronous callback.
    let reader = unsafe { &mut *(*stream).ctx.cast::<Reader<'_>>() };
    let count = size
        .min(reader.bytes.len() - reader.offset)
        .min(reader.max_read);
    // SAFETY: source bounds are checked above; CLAP provides size writable destination bytes.
    unsafe {
        ptr::copy_nonoverlapping(
            reader.bytes.as_ptr().add(reader.offset),
            buffer.cast::<u8>(),
            count,
        );
    }
    reader.offset += count;
    count as i64
}
unsafe extern "C" fn write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    if size == 0 {
        return 0;
    }
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let Ok(size) = usize::try_from(size) else {
        return -1;
    };
    // SAFETY: save() exclusively retains the output Vec; CLAP supplies size readable bytes.
    let output = unsafe { &mut *(*stream).ctx.cast::<Vec<u8>>() };
    // SAFETY: the stream caller supplies size initialized readable source bytes.
    output.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size) });
    size as i64
}

struct Native {
    plugin: *const clap_plugin,
    _host: Box<clap_host>,
    state: Box<HostState>,
    active: bool,
    processing: bool,
}
impl Native {
    fn new() -> Self {
        let state = Box::new(HostState {
            callbacks: AtomicUsize::new(0),
            restarts: AtomicUsize::new(0),
            latency_changes: AtomicUsize::new(0),
            rescans: AtomicUsize::new(0),
            wrong_thread: AtomicBool::new(false),
            main_thread: thread::current().id(),
        });
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: (&*state as *const HostState).cast_mut().cast(),
            name: c"Upmixer ABI host".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(extension),
            request_restart: Some(restart),
            request_process: Some(no_request),
            request_callback: Some(request_callback),
        });
        let entry = &crate::plugin::clap_entry;
        // SAFETY: exported entry/factory tables are static, strings and boxed host are
        // retained, and every live plugin is destroyed before host/entry teardown.
        let plugin = unsafe {
            assert!((entry.init.unwrap())(c"in-process-upmixer.clap".as_ptr()));
            let factory = (entry.get_factory.unwrap())(CLAP_PLUGIN_FACTORY_ID.as_ptr())
                .cast::<clap_plugin_factory>();
            assert!(!factory.is_null());
            assert_eq!(((*factory).get_plugin_count.unwrap())(factory), 1);
            let descriptor = ((*factory).get_plugin_descriptor.unwrap())(factory, 0);
            assert!(!descriptor.is_null());
            assert_eq!(CStr::from_ptr((*descriptor).id), ID);
            ((*factory).create_plugin.unwrap())(factory, &*host, ID.as_ptr())
        };
        assert!(!plugin.is_null());
        let native = Self {
            plugin,
            _host: host,
            state,
            active: false,
            processing: false,
        };
        // SAFETY: init is called exactly once on this live factory-created plugin.
        assert!(unsafe { ((*plugin).init.unwrap())(plugin) });
        native
    }
    fn pump(&self) {
        for _ in 0..32 {
            if self.state.callbacks.swap(0, Ordering::AcqRel) == 0 {
                return;
            }
            // SAFETY: invoked on the creating host thread, after request_callback.
            unsafe {
                ((*self.plugin).on_main_thread.unwrap())(self.plugin);
            }
        }
        panic!("native callback queue did not quiesce")
    }
    fn latency(&self) -> u32 {
        // SAFETY: extension table belongs to this live initialized plugin.
        unsafe {
            let ext =
                ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_LATENCY.as_ptr())
                    .cast::<clap_plugin_latency>();
            assert!(!ext.is_null());
            ((*ext).get.unwrap())(self.plugin)
        }
    }
    fn ports(&self) {
        // SAFETY: extension table and returned initialized entries live during each call.
        unsafe {
            let ext =
                ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_AUDIO_PORTS.as_ptr())
                    .cast::<clap_plugin_audio_ports>();
            assert!(!ext.is_null());
            for (is_input, channels) in [(true, 2), (false, 6)] {
                assert_eq!(((*ext).count.unwrap())(self.plugin, is_input), 1);
                let mut info = std::mem::MaybeUninit::<clap_audio_port_info>::uninit();
                assert!(((*ext).get.unwrap())(
                    self.plugin,
                    0,
                    is_input,
                    info.as_mut_ptr()
                ));
                assert_eq!(info.assume_init().channel_count, channels);
            }
        }
    }
    fn save_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let stream = clap_ostream {
            ctx: (&mut bytes as *mut Vec<u8>).cast(),
            write: Some(write),
        };
        // SAFETY: stream context and output Vec remain exclusive/live throughout save.
        unsafe {
            let ext = ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_STATE.as_ptr())
                .cast::<clap_plugin_state>();
            assert!(!ext.is_null());
            assert!(((*ext).save.unwrap())(self.plugin, &stream));
        }
        bytes
    }
    fn saved(&self) -> PluginState {
        let bytes = self.save_bytes();
        let len = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
        assert_eq!(len, bytes.len() - 8);
        serde_json::from_slice(&bytes[8..]).unwrap()
    }
    fn load_bytes(&self, bytes: &[u8], max_read: usize) -> bool {
        assert!(max_read > 0);
        let mut reader = Reader {
            bytes,
            offset: 0,
            max_read,
        };
        let stream = clap_istream {
            ctx: (&mut reader as *mut Reader<'_>).cast(),
            read: Some(read),
        };
        // SAFETY: retained plugin, borrowed input and exclusive stream context remain
        // live throughout this main-thread synchronous load; no audio call overlaps.
        unsafe {
            let ext = ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_STATE.as_ptr())
                .cast::<clap_plugin_state>();
            assert!(!ext.is_null());
            ((*ext).load.unwrap())(self.plugin, &stream)
        }
    }
    fn load(&self, state: &PluginState, max_read: usize) -> bool {
        let json = serde_json::to_vec(state).unwrap();
        let mut bytes = (json.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&json);
        self.load_bytes(&bytes, max_read)
    }
    fn activate(&mut self, rate: f64) {
        assert!(!self.active);
        // SAFETY: CLAP activation is on the owning host thread with valid prepared bounds.
        assert!(unsafe {
            ((*self.plugin).activate.unwrap())(self.plugin, rate, 1, MAX_FRAMES as u32)
        });
        self.active = true;
        self.pump();
        self.ports();
    }
    fn start(&mut self) {
        assert!(self.active && !self.processing);
        // SAFETY: paired synchronous start/process/stop session on this test's processing thread.
        assert!(unsafe { ((*self.plugin).start_processing.unwrap())(self.plugin) });
        self.processing = true;
    }
    fn deactivate(&mut self) {
        // SAFETY: calls match successful start/activate and finish before plugin destruction.
        unsafe {
            if self.processing {
                ((*self.plugin).stop_processing.unwrap())(self.plugin);
                self.processing = false;
            }
            if self.active {
                ((*self.plugin).deactivate.unwrap())(self.plugin);
                self.active = false;
            }
        }
        self.pump();
    }
    fn process(&mut self, input: &[[f32; 2]]) -> (i32, Vec<f32>) {
        assert!(self.processing && !input.is_empty() && input.len() <= MAX_FRAMES);
        let mut left: Vec<_> = input.iter().map(|s| s[0]).collect();
        let mut right: Vec<_> = input.iter().map(|s| s[1]).collect();
        let mut out = vec![vec![f32::NAN; input.len()]; 6];
        let mut ins = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut outs: Vec<_> = out.iter_mut().map(|s| s.as_mut_ptr()).collect();
        let input_bus = clap_audio_buffer {
            data32: ins.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_bus = clap_audio_buffer {
            data32: outs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 6,
            latency: 0,
            constant_mask: 0,
        };
        let process = clap_process {
            steady_time: -1,
            frames_count: input.len() as u32,
            transport: ptr::null(),
            audio_inputs: &input_bus,
            audio_outputs: &mut output_bus,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: ptr::null(),
            out_events: ptr::null(),
        };
        // SAFETY: channel-pointer arrays and disjoint planar buffers contain frames_count
        // samples and remain live for this exclusive synchronous audio callback.
        let status = unsafe { ((*self.plugin).process.unwrap())(self.plugin, &process) };
        let mut interleaved = Vec::with_capacity(input.len() * 6);
        for i in 0..input.len() {
            for channel in &out {
                interleaved.push(channel[i]);
            }
        }
        (status, interleaved)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.deactivate();
        // SAFETY: this is the sole factory reference; destroy releases it before
        // the boxed host/state are dropped, then deinit balances entry init.
        unsafe {
            ((*self.plugin).destroy.unwrap())(self.plugin);
            (crate::plugin::clap_entry.deinit.unwrap())();
        }
    }
}

fn configured(native: &Native, bypass: bool, low_latency: bool) -> PluginState {
    let mut state = native.saved();
    state
        .params
        .insert("bypass_all_processing".into(), ParamValue::Bool(bypass));
    state
        .params
        .insert("low_latency".into(), ParamValue::Bool(low_latency));
    state
}

#[test]
fn exported_clap_active_structural_load_cannot_change_latency_before_restart() {
    let mut subject = Native::new();
    subject.activate(48_000.0);
    assert_eq!(subject.latency(), 2048);
    subject.start();
    let input = vec![[0.03, -0.02]; MAX_FRAMES];
    assert_ne!(subject.process(&input).0, CLAP_PROCESS_ERROR);
    let before = subject.saved();
    let mut changed = configured(&subject, true, false);
    changed
        .params
        .insert("gain_front_direct".into(), ParamValue::F32(0.7));
    let restarts = subject.state.restarts.load(Ordering::Acquire);
    let loaded = subject.load(&changed, 3);
    subject.pump();
    println!(
        "active_load accepted={loaded} latency_before=2048 latency_after={} restart_delta={}",
        subject.latency(),
        subject.state.restarts.load(Ordering::Acquire) - restarts
    );
    assert!(
        !loaded,
        "active snapshot replaced DSP before host accepted restart"
    );
    assert_eq!(subject.latency(), 2048);
    assert_eq!(
        serde_json::to_value(subject.saved().params).unwrap(),
        serde_json::to_value(before.params).unwrap()
    );
}

fn state_value(state: &PluginState) -> serde_json::Value {
    serde_json::to_value(state).unwrap()
}

fn program(position: usize, frames: usize) -> Vec<[f32; 2]> {
    (position..position + frames)
        .map(|i| {
            if i >= 4099 {
                [0.0; 2]
            } else {
                let phase = i as f32;
                [0.03 * (phase * 0.013).sin(), 0.02 * (phase * 0.021).cos()]
            }
        })
        .collect()
}

#[test]
fn exported_clap_inactive_state_latency_and_audio_match_direct_dsp() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for bypass in [false, true] {
            for low_latency in [false, true] {
                let mut native = Native::new();
                let state = configured(&native, bypass, low_latency);
                assert!(native.load(&state, 1));
                native.activate(f64::from(rate));
                let latency = if bypass {
                    0
                } else if low_latency {
                    1024
                } else {
                    2048
                };
                assert_eq!(native.latency(), latency);
                if !bypass {
                    assert!(native.state.latency_changes.load(Ordering::Acquire) > 0);
                }
                assert!(!native.state.wrong_thread.load(Ordering::Acquire));
                assert_eq!(native.state.restarts.load(Ordering::Acquire), 0);
                // Separate constructor with explicit physical FFT/bus settings: no
                // native state parser, migration, parameter mapper or wrapper path.
                let config = serde_json::json!({
                    "speaker_config": "5.1",
                    "bypass_all_processing": bypass,
                    "low_latency": low_latency,
                    "fft_size": if low_latency { 1024 } else { 2048 },
                })
                .to_string();
                let mut direct =
                    plugins_bridge::create_plugin("Upmixer", 2, f64::from(rate), &config).unwrap();
                direct.initialize(f64::from(rate)).unwrap();
                assert_eq!(direct.latency_samples(), latency as usize);
                native.start();
                let mut position = 0;
                let mut maximum_error = 0.0_f32;
                let mut energy = 0.0_f64;
                for requested in [1, 17, 257, 63, 2, 191, 256, 7].into_iter().cycle() {
                    let frames = requested.min(12_291 - position);
                    let input = program(position, frames);
                    let flat: Vec<_> = input.iter().flatten().copied().collect();
                    let mut expected = vec![f32::NAN; frames * 6];
                    assert_eq!(
                        direct
                            .process(
                                &flat,
                                &mut expected,
                                &sotf_host::plugin::ProcessContext::new(rate, frames)
                            )
                            .unwrap(),
                        frames
                    );
                    let (status, actual) = native.process(&input);
                    assert_ne!(status, CLAP_PROCESS_ERROR);
                    for (index, (&observed, &reference)) in actual.iter().zip(&expected).enumerate()
                    {
                        assert!(observed.is_finite() && reference.is_finite());
                        maximum_error = maximum_error.max((observed - reference).abs());
                        energy += f64::from(observed).powi(2);
                        let frame = position + index / 6;
                        if frame < latency as usize {
                            assert_eq!(observed, 0.0, "fresh latency at {frame}");
                        }
                        if bypass {
                            let channel = index % 6;
                            assert_eq!(
                                observed,
                                if channel < 2 {
                                    input[index / 6][channel]
                                } else {
                                    0.0
                                }
                            );
                        }
                    }
                    position += frames;
                    if position == 12_291 {
                        break;
                    }
                }
                assert!(energy > 0.001, "audio reference must be nonvacuous");
                assert!(
                    maximum_error <= 2e-6,
                    "rate={rate} bypass={bypass} low={low_latency} error={maximum_error}"
                );
                println!(
                    "exported_clap_audio rate={rate} bypass={bypass} low={low_latency} latency={latency} frames={position} maximum_error={maximum_error}"
                );
            }
        }
    }
}

#[test]
fn exported_clap_rejected_snapshots_preserve_history_then_allow_inactive_retry() {
    for processing in [false, true] {
        let mut subject = Native::new();
        let mut control = Native::new();
        for native in [&mut subject, &mut control] {
            native.activate(48_000.0);
            native.start();
            for position in (0..4096).step_by(256) {
                assert_ne!(
                    native.process(&program(position, 256)).0,
                    CLAP_PROCESS_ERROR
                );
            }
        }
        // Stop processing without deactivation: active snapshots remain unsafe
        // even if the callback is temporarily suspended.
        if !processing {
            for native in [&mut subject, &mut control] {
                // SAFETY: paired with the successful start above; no processing overlaps.
                unsafe {
                    ((*native.plugin).stop_processing.unwrap())(native.plugin);
                }
                native.processing = false;
            }
        }
        subject.pump();
        let before = subject.saved();
        let before_json = state_value(&before);
        let counts = (
            subject.state.restarts.load(Ordering::Acquire),
            subject.state.latency_changes.load(Ordering::Acquire),
            subject.state.rescans.load(Ordering::Acquire),
        );
        let mut structural = configured(&subject, true, true);
        structural
            .params
            .insert("gain_front_direct".into(), ParamValue::F32(0.7));
        let mut scalar = subject.saved();
        scalar
            .params
            .insert("gain_front_direct".into(), ParamValue::F32(0.7));
        let mut sparse = subject.saved();
        sparse.params.clear();
        sparse
            .params
            .insert("gain_front_direct".into(), ParamValue::F32(0.7));
        for state in [&structural, &scalar, &before, &sparse] {
            assert!(!subject.load(state, 3));
            subject.pump();
            assert_eq!(state_value(&subject.saved()), before_json);
            assert_eq!(subject.latency(), 2048);
            assert_eq!(
                (
                    subject.state.restarts.load(Ordering::Acquire),
                    subject.state.latency_changes.load(Ordering::Acquire),
                    subject.state.rescans.load(Ordering::Acquire),
                ),
                counts
            );
        }
        if !processing {
            subject.start();
            control.start();
        }
        let mut error = 0.0_f32;
        for position in (4096..12_288).step_by(256) {
            let input = program(position, 256);
            let (status, actual) = subject.process(&input);
            let (other_status, expected) = control.process(&input);
            assert_ne!(status, CLAP_PROCESS_ERROR);
            assert_ne!(other_status, CLAP_PROCESS_ERROR);
            for (a, b) in actual.iter().zip(expected) {
                assert!(a.is_finite() && b.is_finite());
                error = error.max((a - b).abs());
            }
        }
        assert_eq!(
            error, 0.0,
            "failed loads must retain the entire DSP history"
        );
        subject.deactivate();
        // Retry the exact rejected snapshot, including its changed scalar value.
        let retry = structural;
        assert!(subject.load(&retry, usize::MAX));
        subject.pump();
        assert_eq!(state_value(&subject.saved()), state_value(&retry));
        assert_eq!(subject.latency(), 0);
        assert_eq!(
            subject.state.latency_changes.load(Ordering::Acquire),
            counts.1 + 1
        );
        assert_eq!(subject.state.restarts.load(Ordering::Acquire), counts.0);
        subject.activate(96_000.0);
        assert_eq!(subject.latency(), 0);
        subject.start();
        let input = program(0, 17);
        let (status, actual) = subject.process(&input);
        assert_ne!(status, CLAP_PROCESS_ERROR);
        for (frame, source) in actual.as_chunks::<6>().0.iter().zip(input) {
            assert_eq!(frame, &[source[0], source[1], 0.0, 0.0, 0.0, 0.0]);
        }
        assert!(!subject.state.wrong_thread.load(Ordering::Acquire));
        println!(
            "exported_clap_reject processing={processing} snapshots=4 history_error={error} retry_latency=0"
        );
    }
}

#[test]
fn exported_clap_truncated_or_invalid_streams_preserve_state() {
    let subject = Native::new();
    let before = state_value(&subject.saved());
    let bytes = subject.save_bytes();
    let invalid = [1_u64.to_le_bytes().as_slice(), b"{"].concat();
    for input in [
        &bytes[..0],
        &bytes[..7],
        &bytes[..bytes.len() - 1],
        &invalid,
    ] {
        assert!(!subject.load_bytes(input, 1));
        assert_eq!(state_value(&subject.saved()), before);
    }
}

#[test]
fn upmixer_snapshot_policy_rejects_active_and_audio_thread_before_mutation() {
    use nih_plug::prelude::Plugin;
    let native = Native::new();
    let snapshot = configured(&native, true, true);
    let plugin = crate::plugin::SotfUpmixer::default();
    let params = plugin.params();
    for active in [false, true] {
        for audio in [false, true] {
            assert_eq!(
                params.validate_state(&snapshot, active, audio, Some(48_000.0)),
                !active && !audio
            );
        }
    }
    assert!(!crate::plugin::SotfUpmixer::state_restore_allows_audio_thread(&snapshot));
    let mut sparse = snapshot;
    sparse.params.clear();
    assert!(!crate::plugin::SotfUpmixer::state_restore_allows_audio_thread(&sparse));
}
