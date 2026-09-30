//! Exercise restart-required Dynamic EQ controls through the native CLAP callbacks.

use super::SotfDynamicEQ;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_input_events,
};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_plugin_params};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use nih_plug::wrapper::clap::Wrapper;
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_plugins::plugin_dynamic_eq::DynEqShape;
use sotf_plugins::{DynEqBandParams, DynamicEqPlugin, DynamicEqPluginParams, ProcessContext};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

const FRAMES: usize = 128;
const SAMPLE_RATE: f64 = 48_000.0;

struct HostState {
    callback_requests: AtomicUsize,
    restart_requests: AtomicUsize,
    restart_was_on_main_thread: AtomicBool,
    main_thread: ThreadId,
}

unsafe fn state_from_host<'a>(host: *const clap_host) -> &'a HostState {
    // SAFETY: each test host points to a boxed HostState kept alive until its wrapper is dropped.
    let data = unsafe { (*host).host_data };
    assert!(!data.is_null());
    unsafe { &*data.cast::<HostState>() }
}

unsafe extern "C" fn no_host_extension(
    _host: *const clap_host,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn request_restart(host: *const clap_host) {
    let state = unsafe { state_from_host(host) };
    state.restart_was_on_main_thread.store(
        thread::current().id() == state.main_thread,
        Ordering::Release,
    );
    state.restart_requests.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn request_callback(host: *const clap_host) {
    let state = unsafe { state_from_host(host) };
    state.callback_requests.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn request_process(_host: *const clap_host) {}

struct TestPlugin {
    // The wrapper contains a raw host pointer; drop it before freeing either boxed host object.
    wrapper: std::sync::Arc<Wrapper<SotfDynamicEQ>>,
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
            main_thread: thread::current().id(),
        });
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: (&*host_state as *const HostState).cast_mut().cast(),
            name: c"Dynamic EQ restart test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_host_extension),
            request_restart: Some(request_restart),
            request_process: Some(request_process),
            request_callback: Some(request_callback),
        });

        // SAFETY: both boxed host objects outlive the returned wrapper and all callback invocations.
        let wrapper = unsafe { Wrapper::<SotfDynamicEQ>::new(&*host) };
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

    fn set_param(&self, param_id: &str, value: f64) {
        let plugin = self.clap_plugin();
        let params = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                .cast::<clap_plugin_params>()
        };
        assert!(!params.is_null());

        let event = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: hash_param_id(param_id),
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        let events = [event];
        let mut event_slice: &[clap_event_param_value] = &events;
        let event_list = clap_input_events {
            ctx: (&mut event_slice as *mut &[clap_event_param_value]).cast(),
            size: Some(event_count),
            get: Some(event_at),
        };
        unsafe { ((*params).flush.unwrap())(plugin, &event_list, std::ptr::null()) };
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

    fn activate(&self) {
        let plugin = self.clap_plugin();
        unsafe {
            assert!(((*plugin).activate.unwrap())(
                plugin,
                SAMPLE_RATE,
                1,
                FRAMES as u32,
            ));
            assert!(((*plugin).start_processing.unwrap())(plugin));
        }
        self.active.set(true);
    }

    fn restart_lifecycle(&self) {
        let plugin = self.clap_plugin();
        unsafe {
            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).deactivate.unwrap())(plugin);
        }
        self.active.set(false);
        self.activate();
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

    fn service_main_thread_callback(&self) {
        unsafe {
            (self.clap_plugin().as_ref().unwrap().on_main_thread.unwrap())(self.clap_plugin())
        };
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

unsafe extern "C" fn event_count(list: *const clap_input_events) -> u32 {
    let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
    events.len() as u32
}

unsafe extern "C" fn event_at(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
    events
        .get(index as usize)
        .map_or(std::ptr::null(), |event| &event.header)
}

fn hash_param_id(id: &str) -> u32 {
    let mut hash = 0_u32;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    hash & !(1 << 31)
}

fn wait_for_callback(host_state: &HostState) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while host_state.callback_requests.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        host_state.callback_requests.load(Ordering::Acquire) > 0,
        "CLAP host callback was not requested"
    );
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
    let square_error: f64 = left
        .iter()
        .zip(right)
        .flat_map(|(left, right)| left.iter().zip(right))
        .map(|(left, right)| f64::from(*left - *right).powi(2))
        .sum();
    let sample_count = left[0].len() + left[1].len();
    assert!(sample_count > 0);
    let rms = (square_error / sample_count as f64).sqrt();
    assert!(rms > 1.0e-3, "shelf-vs-Peak RMS was only {rms}");
}

fn direct_shelf_reference(first_frame: usize, block_count: usize) -> [Vec<f32>; 2] {
    let linear =
        |minimum: f32, maximum: f32, normalized: f32| minimum + normalized * (maximum - minimum);
    let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(
        2,
        DynamicEqPluginParams {
            num_bands: 1,
            threshold: -20.0,
            ratio: 2.0,
            attack_ms: 5.0,
            release_ms: 50.0,
            knee: 6.0,
            link_channels: true,
            mix: 1.0,
            bands: vec![DynEqBandParams {
                shape: DynEqShape::LowShelf,
                shelf_slope: linear(0.1, 1.0, 0.75),
                frequency: linear(20.0, 20_000.0, 0.5),
                q: 1.0,
                gain: linear(-24.0, 24.0, 0.75),
                band_threshold: linear(-60.0, 0.0, 0.25),
                band_ratio: 2.0,
                active: true,
                solo: false,
            }],
        },
        SAMPLE_RATE as u32,
    )
    .unwrap();
    plugin.initialize(SAMPLE_RATE as u32).unwrap();

    let sample_count = block_count * FRAMES;
    let mut output = [
        Vec::with_capacity(sample_count),
        Vec::with_capacity(sample_count),
    ];
    for block in 0..block_count {
        let block_start = first_frame + block * FRAMES;
        let mut interleaved = Vec::with_capacity(FRAMES * 2);
        for frame in 0..FRAMES {
            let time = (block_start + frame) as f32 / SAMPLE_RATE as f32;
            let sample = (std::f32::consts::TAU * 120.0 * time).sin() * 0.1;
            interleaved.push(sample);
            interleaved.push(sample * 0.7);
        }
        assert_eq!(
            plugin
                .process_in_place(&mut interleaved, &ProcessContext::new(48_000, FRAMES))
                .unwrap(),
            FRAMES
        );
        for frame in 0..FRAMES {
            output[0].push(interleaved[frame * 2]);
            output[1].push(interleaved[frame * 2 + 1]);
        }
    }
    output
}

#[test]
fn clap_defers_restart_required_shelf_until_host_reactivation() {
    let subject = TestPlugin::new();
    let peak_control = TestPlugin::new();
    let shelf_reference = TestPlugin::new();
    for plugin in [&subject, &peak_control, &shelf_reference] {
        plugin.set_param("band_0_gain", 0.75);
        plugin.set_param("band_0_frequency", 0.5);
        plugin.set_param("band_0_band_threshold", 0.35);
    }
    shelf_reference.set_param("band_0_shape", 1.0);
    shelf_reference.set_param("band_0_shelf_slope", 0.75);
    shelf_reference.set_param("band_0_band_threshold", 0.25);
    assert_eq!(shelf_reference.get_param("band_0_shape"), 1.0);
    assert_eq!(
        subject.get_param("band_0_shape"),
        peak_control.get_param("band_0_shape")
    );
    subject.activate();
    peak_control.activate();
    shelf_reference.activate();

    // Build identical detector/filter history before the structural update.
    assert_same_audio(
        &subject.process_blocks(0, 8),
        &peak_control.process_blocks(0, 8),
    );

    // Low shelf is the appended choice with plain value 1. It is stored immediately, but it
    // must not rebuild or alter the prepared Peak processor on the CLAP parameter callback.
    subject.set_param("band_0_shape", 1.0);
    subject.set_param("band_0_shelf_slope", 0.75);
    assert_eq!(subject.state().restart_requests.load(Ordering::Acquire), 0);
    // A real active scalar event must also cross the callback path without allocating.
    subject.set_param("band_0_band_threshold", 0.25);
    peak_control.set_param("band_0_band_threshold", 0.25);
    let before_host_service = subject.process_blocks(8 * FRAMES, 8);
    let old_shape_control = peak_control.process_blocks(8 * FRAMES, 8);
    assert_same_audio(&before_host_service, &old_shape_control);

    // The worker thread asks CLAP for a main-thread callback; only that callback may issue the
    // actual request_restart call to the host.
    wait_for_callback(subject.state());
    assert_eq!(subject.state().restart_requests.load(Ordering::Acquire), 0);
    subject.service_main_thread_callback();
    assert_eq!(subject.state().restart_requests.load(Ordering::Acquire), 1);
    assert!(
        subject
            .state()
            .restart_was_on_main_thread
            .load(Ordering::Acquire)
    );

    // Simulate CLAP's stop/deactivate/activate/start response. A fresh reset applies the stored
    // shelf choice. The unchanged Peak twin provides the independent control signal.
    subject.restart_lifecycle();
    peak_control.restart_lifecycle();
    let first_post_restart_frame = 16 * FRAMES;
    let shelf_audio = subject.process_blocks(first_post_restart_frame, 32);
    let peak_audio = peak_control.process_blocks(first_post_restart_frame, 32);
    let fresh_shelf_audio = shelf_reference.process_blocks(first_post_restart_frame, 32);
    assert_audio_differs(&shelf_audio, &peak_audio);
    assert_same_audio(&shelf_audio, &fresh_shelf_audio);
    assert_eq!(
        subject.get_param("band_0_shape"),
        shelf_reference.get_param("band_0_shape")
    );
    assert_eq!(
        subject.get_param("band_0_shelf_slope"),
        shelf_reference.get_param("band_0_shelf_slope")
    );
    assert_eq!(
        subject.get_param("band_0_band_threshold"),
        shelf_reference.get_param("band_0_band_threshold")
    );
    assert_same_audio(
        &shelf_audio,
        &direct_shelf_reference(first_post_restart_frame, 32),
    );

    // Successful reactivation clears the pending latch; a same-value host echo must not trigger
    // a second, spurious restart after the shelf is already active.
    subject.set_param("band_0_shape", 1.0);
    thread::sleep(Duration::from_millis(10));
    subject.service_main_thread_callback();
    assert_eq!(subject.state().restart_requests.load(Ordering::Acquire), 1);
}
