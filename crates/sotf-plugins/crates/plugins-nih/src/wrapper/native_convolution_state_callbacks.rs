//! Exercise Convolution resource restoration through the native CLAP state callbacks.

use crate::wrapper::native_convolution_editor::Geometry;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_input_events,
};
#[cfg(target_os = "linux")]
use clap_sys::ext::gui::{
    CLAP_EXT_GUI, CLAP_WINDOW_API_X11, clap_plugin_gui, clap_window, clap_window_handle,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_plugin_tail};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use nih_plug::prelude::{
    AuxiliaryBuffers, Buffer, BufferConfig, ClapPlugin, Plugin as NihPlugin, ProcessMode,
    ProcessStatus,
};
use nih_plug::wrapper::clap::Wrapper;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use nih_plug::wrapper::vst3::{Wrapper as Vst3Wrapper, vst3_sys};
use std::cell::{Cell, RefCell};
#[cfg(target_os = "linux")]
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use vst3_sys as vst3_com;
use vst3_sys::base::{
    IBStream, IPluginBase, IUnknown, kIBSeekCur, kIBSeekEnd, kIBSeekSet, kInvalidArgument,
    kResultFalse, kResultOk, tresult,
};
#[cfg(target_os = "linux")]
use vst3_sys::gui::IPlugFrame;
#[cfg(target_os = "linux")]
use vst3_sys::gui::linux::{FileDescriptor, IEventHandler, IRunLoop, ITimerHandler, TimerInterval};
#[cfg(target_os = "linux")]
use vst3_sys::gui::{IPlugView, ViewRect};
#[cfg(target_os = "linux")]
use vst3_sys::utils::StaticVstPtr;
use vst3_sys::utils::{SharedVstPtr, VstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, IAudioProcessor, IComponent, IEditController, IParameterChanges, ProcessData,
    ProcessModes, ProcessSetup, SymbolicSampleSizes,
};
#[cfg(target_os = "linux")]
use vst3_sys::vst::{IComponentHandler, IParamValueQueue, RestartFlags};
use vst3_sys::{ComInterface, VST3};

const IR_RESOURCE_FIELD: &str = "sotf_convolution_ir_resource";
const SAMPLE_RATE: f64 = 48_000.0;
const MAX_FRAMES: usize = 128;
const NORMAL_LATENCY: usize = 1_024;

crate::sotf_nih_plugin!(
    NativeConvolutionStateProbe,
    plugin_type: "Convolution",
    name: "Convolution State Probe",
    clap_id: "org.sotf.convolution-state-probe",
    vst3_class_id: *b"SotfConvState001",
    channels: 2
);

unsafe extern "C" fn no_host_extension(
    _host: *const clap_host,
    _id: *const c_char,
) -> *const c_void {
    ptr::null()
}

struct NativeClapHostState {
    callback_requests: AtomicUsize,
    restart_requests: AtomicUsize,
    restart_on_main_thread: AtomicBool,
    main_thread: ThreadId,
}

unsafe fn clap_host_state<'a>(host: *const clap_host) -> &'a NativeClapHostState {
    // SAFETY: NativeClap stores this pointer in its boxed host and drops the wrapper first.
    let data = unsafe { (*host).host_data };
    assert!(!data.is_null());
    unsafe { &*data.cast::<NativeClapHostState>() }
}

unsafe extern "C" fn request_clap_callback(host: *const clap_host) {
    let state = unsafe { clap_host_state(host) };
    state.callback_requests.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn request_clap_restart(host: *const clap_host) {
    let state = unsafe { clap_host_state(host) };
    state.restart_on_main_thread.store(
        thread::current().id() == state.main_thread,
        Ordering::Release,
    );
    state.restart_requests.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn no_host_process_request(_host: *const clap_host) {}

struct NativeClap {
    // Drop the wrapper before its raw host pointer owner.
    wrapper: Arc<Wrapper<NativeConvolutionStateProbe>>,
    _host: Box<clap_host>,
    _host_state: Box<NativeClapHostState>,
    active: bool,
}

impl NativeClap {
    fn new() -> Self {
        let host_state = Box::new(NativeClapHostState {
            callback_requests: AtomicUsize::new(0),
            restart_requests: AtomicUsize::new(0),
            restart_on_main_thread: AtomicBool::new(false),
            main_thread: thread::current().id(),
        });
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: (&*host_state as *const NativeClapHostState)
                .cast_mut()
                .cast(),
            name: c"Convolution state test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_host_extension),
            request_restart: Some(request_clap_restart),
            request_process: Some(no_host_process_request),
            request_callback: Some(request_clap_callback),
        });
        // SAFETY: the boxed host remains alive until after the wrapper is dropped.
        let wrapper = unsafe { Wrapper::<NativeConvolutionStateProbe>::new(&*host) };
        let instance = Self {
            wrapper,
            _host: host,
            _host_state: host_state,
            active: false,
        };
        let plugin = instance.plugin();
        // SAFETY: this is the generated CLAP lifecycle callback for a live wrapper.
        assert!(unsafe { ((*plugin).init.unwrap())(plugin) });
        instance
    }

    fn plugin(&self) -> *const clap_plugin {
        self.wrapper.clap_plugin.as_ptr()
    }

    #[cfg(target_os = "linux")]
    fn host_state(&self) -> &NativeClapHostState {
        &self._host_state
    }

    #[cfg(target_os = "linux")]
    fn service_main_thread_callback(&self) {
        // SAFETY: the CLAP host invokes this callback on its main thread after request_callback.
        unsafe { (self.plugin().as_ref().unwrap().on_main_thread.unwrap())(self.plugin()) };
    }

    #[cfg(target_os = "linux")]
    fn gui_extension(&self) -> *const clap_plugin_gui {
        let plugin = self.plugin();
        // SAFETY: get_extension returns the stable native GUI callback table.
        let extension = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_GUI.as_ptr())
                .cast::<clap_plugin_gui>()
        };
        assert!(
            !extension.is_null(),
            "Convolution did not export a native editor"
        );
        extension
    }

    #[cfg(target_os = "linux")]
    fn open_gui(&self, parent: &X11HostWindow) -> *const clap_plugin_gui {
        let gui = self.gui_extension();
        let plugin = self.plugin();
        // SAFETY: all callback pointers belong to the live CLAP wrapper and `parent` remains
        // mapped until `close_gui()` returns.
        unsafe {
            assert!((*gui).is_api_supported.unwrap()(
                plugin,
                CLAP_WINDOW_API_X11.as_ptr(),
                false
            ));
            assert!((*gui).create.unwrap()(
                plugin,
                CLAP_WINDOW_API_X11.as_ptr(),
                false
            ));
            let window = clap_window {
                api: CLAP_WINDOW_API_X11.as_ptr(),
                specific: clap_window_handle { x11: parent.window },
            };
            assert!((*gui).set_parent.unwrap()(plugin, &window));
        }
        assert!(self.show_gui(gui));
        thread::sleep(Duration::from_millis(500));
        gui
    }

    #[cfg(target_os = "linux")]
    fn show_gui(&self, gui: *const clap_plugin_gui) -> bool {
        // SAFETY: the GUI callback table belongs to this live wrapper and is called on the host's
        // main thread after successful create/set_parent.
        unsafe { ((*gui).show.unwrap())(self.plugin()) }
    }

    #[cfg(target_os = "linux")]
    fn hide_gui(&self, gui: *const clap_plugin_gui) -> bool {
        // SAFETY: the GUI callback table belongs to this live wrapper and is called on the host's
        // main thread while the editor handle is still attached.
        unsafe { ((*gui).hide.unwrap())(self.plugin()) }
    }

    #[cfg(target_os = "linux")]
    fn close_gui(&self, gui: *const clap_plugin_gui) {
        // SAFETY: the GUI was created by this wrapper and remains live until destroy.
        unsafe {
            assert!(self.hide_gui(gui));
            ((*gui).destroy.unwrap())(self.plugin());
        }
    }

    fn state_extension(&self) -> *const clap_plugin_state {
        let plugin = self.plugin();
        // SAFETY: get_extension returns the wrapper's stable state extension table.
        let extension = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_STATE.as_ptr())
                .cast::<clap_plugin_state>()
        };
        assert!(!extension.is_null());
        extension
    }

    fn latency_frames(&self) -> usize {
        let plugin = self.plugin();
        // SAFETY: get_extension returns the wrapper's stable latency extension table.
        let extension = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_LATENCY.as_ptr())
                .cast::<clap_plugin_latency>()
        };
        assert!(!extension.is_null());
        // SAFETY: the extension pointer remains valid for the wrapper's lifetime.
        unsafe { ((*extension).get.unwrap())(plugin) as usize }
    }

    fn tail_frames(&self) -> usize {
        let plugin = self.plugin();
        // SAFETY: get_extension returns the wrapper's stable tail extension table.
        let extension = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_TAIL.as_ptr())
                .cast::<clap_plugin_tail>()
        };
        assert!(!extension.is_null());
        // SAFETY: the extension pointer remains valid for the wrapper's lifetime.
        unsafe { ((*extension).get.unwrap())(plugin) as usize }
    }

    fn new_state(&self) -> PluginState {
        self.wrapper.get_state_object()
    }

    fn load_state(&self, state: &PluginState) -> bool {
        let payload = serde_json::to_vec(state).unwrap();
        let mut bytes = Vec::with_capacity(8 + payload.len());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&payload);
        self.load_stream(&bytes)
    }

    fn load_stream(&self, bytes: &[u8]) -> bool {
        let mut reader = StateReader { bytes, offset: 0 };
        let stream = clap_istream {
            ctx: (&mut reader as *mut StateReader<'_>).cast(),
            read: Some(read_state),
        };
        let plugin = self.plugin();
        // SAFETY: the stream and its borrowed context remain live for the synchronous callback.
        unsafe { ((*self.state_extension()).load.unwrap())(plugin, &stream) }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let stream = clap_ostream {
            ctx: (&mut bytes as *mut Vec<u8>).cast(),
            write: Some(write_state),
        };
        let plugin = self.plugin();
        // SAFETY: the stream and vector remain live for the synchronous callback.
        assert!(unsafe { ((*self.state_extension()).save.unwrap())(plugin, &stream) });
        bytes
    }

    fn try_activate(&mut self) -> bool {
        let plugin = self.plugin();
        // SAFETY: callbacks run on this test's control thread in CLAP lifecycle order.
        unsafe {
            if !((*plugin).activate.unwrap())(plugin, SAMPLE_RATE, 1, MAX_FRAMES as u32) {
                return false;
            }
            if !((*plugin).start_processing.unwrap())(plugin) {
                ((*plugin).deactivate.unwrap())(plugin);
                return false;
            }
        }
        self.active = true;
        true
    }

    fn activate(&mut self) {
        assert!(self.try_activate(), "native CLAP activation failed");
    }

    fn deactivate(&mut self) {
        if !self.active {
            return;
        }
        let plugin = self.plugin();
        // SAFETY: stop and deactivate pair with the successful activation above.
        unsafe {
            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).deactivate.unwrap())(plugin);
        }
        self.active = false;
    }

    fn process(&mut self, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
        self.process_with_events(input, None)
    }

    fn process_with_mix_event(&mut self, input: &[[f32; 2]], mix: f64) -> Vec<[f32; 2]> {
        let parameter_id = self.parameter_id("Mix");
        let event = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: parameter_id,
            cookie: ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: mix,
        };
        self.process_with_events(input, Some(std::slice::from_ref(&event)))
    }

    fn parameter_id(&self, name: &str) -> clap_id {
        let plugin = self.plugin();
        // SAFETY: get_extension returns the stable CLAP parameter table for this live plugin.
        let params = unsafe {
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                .cast::<clap_plugin_params>()
        };
        assert!(!params.is_null());
        // SAFETY: each info entry is initialized by the live plugin; the name field is a
        // NUL-terminated fixed-size CLAP string.
        unsafe {
            for index in 0..((*params).count.unwrap())(plugin) {
                let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
                assert!((*params).get_info.unwrap()(
                    plugin,
                    index,
                    info.as_mut_ptr()
                ));
                let info = info.assume_init();
                if CStr::from_ptr(info.name.as_ptr())
                    .to_bytes()
                    .eq_ignore_ascii_case(name.as_bytes())
                {
                    return info.id;
                }
            }
        }
        panic!("missing CLAP parameter {name}");
    }

    fn process_with_events(
        &mut self,
        input: &[[f32; 2]],
        events: Option<&[clap_event_param_value]>,
    ) -> Vec<[f32; 2]> {
        assert!(self.active);
        let mut output = Vec::with_capacity(input.len());
        let mut event_slice = events.unwrap_or(&[]);
        let event_list = clap_input_events {
            ctx: (&mut event_slice as *mut &[clap_event_param_value]).cast(),
            size: Some(native_param_event_count),
            get: Some(native_param_event_at),
        };
        for (block_index, block) in input.chunks(MAX_FRAMES).enumerate() {
            let mut left_input: Vec<f32> = block.iter().map(|frame| frame[0]).collect();
            let mut right_input: Vec<f32> = block.iter().map(|frame| frame[1]).collect();
            let mut left_output = vec![f32::NAN; block.len()];
            let mut right_output = vec![f32::NAN; block.len()];
            let mut input_channels = [left_input.as_mut_ptr(), right_input.as_mut_ptr()];
            let mut output_channels = [left_output.as_mut_ptr(), right_output.as_mut_ptr()];
            let input_bus = clap_audio_buffer {
                data32: input_channels.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let mut output_bus = clap_audio_buffer {
                data32: output_channels.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let process = clap_process {
                steady_time: -1,
                frames_count: block.len() as u32,
                transport: ptr::null(),
                audio_inputs: &input_bus,
                audio_outputs: &mut output_bus,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: if block_index == 0 && events.is_some() {
                    &event_list
                } else {
                    ptr::null()
                },
                out_events: ptr::null(),
            };
            let plugin = self.plugin();
            // SAFETY: every channel buffer is sized for the block and lives through the callback.
            let status = unsafe { ((*plugin).process.unwrap())(plugin, &process) };
            assert_ne!(status, CLAP_PROCESS_ERROR);
            output.extend(
                left_output
                    .into_iter()
                    .zip(right_output)
                    .map(|(left, right)| [left, right]),
            );
        }
        output
    }
}

unsafe extern "C" fn native_param_event_count(list: *const clap_input_events) -> u32 {
    // SAFETY: `ctx` points to the borrowed event slice kept alive by `process_with_events()`.
    let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
    u32::try_from(events.len()).expect("test event count fits in u32")
}

unsafe extern "C" fn native_param_event_at(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    // SAFETY: `ctx` points to the borrowed event slice kept alive by `process_with_events()`.
    let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
    events
        .get(index as usize)
        .map_or(ptr::null(), |event| &event.header)
}

impl Drop for NativeClap {
    fn drop(&mut self) {
        self.deactivate();
    }
}

#[cfg(target_os = "linux")]
struct X11HostWindow {
    display: *mut X11Display,
    screen: std::ffi::c_int,
    window: std::ffi::c_ulong,
    origin_x: i32,
    origin_y: i32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct X11Display {
    _private: [u8; 0],
}

#[cfg(target_os = "linux")]
#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(display_name: *const c_char) -> *mut X11Display;
    fn XDefaultScreen(display: *mut X11Display) -> std::ffi::c_int;
    fn XRootWindow(display: *mut X11Display, screen: std::ffi::c_int) -> std::ffi::c_ulong;
    fn XCreateSimpleWindow(
        display: *mut X11Display,
        parent: std::ffi::c_ulong,
        x: std::ffi::c_int,
        y: std::ffi::c_int,
        width: std::ffi::c_uint,
        height: std::ffi::c_uint,
        border_width: std::ffi::c_uint,
        border: std::ffi::c_ulong,
        background: std::ffi::c_ulong,
    ) -> std::ffi::c_ulong;
    fn XMapRaised(display: *mut X11Display, window: std::ffi::c_ulong) -> std::ffi::c_int;
    fn XDestroyWindow(display: *mut X11Display, window: std::ffi::c_ulong) -> std::ffi::c_int;
    fn XSync(display: *mut X11Display, discard: std::ffi::c_int) -> std::ffi::c_int;
    fn XCloseDisplay(display: *mut X11Display) -> std::ffi::c_int;
    fn XKeysymToKeycode(display: *mut X11Display, keysym: std::ffi::c_ulong) -> std::ffi::c_uchar;
}

#[cfg(target_os = "linux")]
#[link(name = "Xtst")]
unsafe extern "C" {
    fn XTestFakeMotionEvent(
        display: *mut X11Display,
        screen: std::ffi::c_int,
        x: std::ffi::c_int,
        y: std::ffi::c_int,
        delay: std::ffi::c_ulong,
    ) -> std::ffi::c_int;
    fn XTestFakeButtonEvent(
        display: *mut X11Display,
        button: std::ffi::c_uint,
        is_press: std::ffi::c_int,
        delay: std::ffi::c_ulong,
    ) -> std::ffi::c_int;
    fn XTestFakeKeyEvent(
        display: *mut X11Display,
        keycode: std::ffi::c_uint,
        is_press: std::ffi::c_int,
        delay: std::ffi::c_ulong,
    ) -> std::ffi::c_int;
}

#[cfg(target_os = "linux")]
impl X11HostWindow {
    fn new() -> Self {
        // SAFETY: XOpenDisplay returns an owned connection that this helper closes in Drop.
        let display = unsafe { XOpenDisplay(ptr::null()) };
        assert!(
            !display.is_null(),
            "DISPLAY must be set by the Xvfb test helper"
        );
        // SAFETY: display is live and Xlib owns the root/window IDs returned here.
        let screen = unsafe { XDefaultScreen(display) };
        let root = unsafe { XRootWindow(display, screen) };
        let origin_x = 40;
        let origin_y = 40;
        let window = unsafe {
            XCreateSimpleWindow(
                display,
                root,
                origin_x,
                origin_y,
                800,
                600,
                0,
                0,
                0x00ff_ffff,
            )
        };
        assert_ne!(window, 0, "X11 host parent window creation failed");
        unsafe {
            XMapRaised(display, window);
            XSync(display, 0);
        }
        thread::sleep(Duration::from_millis(100));
        Self {
            display,
            screen,
            window,
            origin_x,
            origin_y,
        }
    }

    fn click(&self, x: i32, y: i32) {
        // SAFETY: all calls target this live X11 connection and a mapped parent window.
        unsafe {
            assert_ne!(
                XTestFakeMotionEvent(
                    self.display,
                    self.screen,
                    self.origin_x + x,
                    self.origin_y + y,
                    0,
                ),
                0
            );
            assert_ne!(XTestFakeButtonEvent(self.display, 1, 1, 0), 0);
            assert_ne!(XTestFakeButtonEvent(self.display, 1, 0, 0), 0);
            XSync(self.display, 0);
        }
        thread::sleep(Duration::from_millis(120));
    }

    fn type_ascii(&self, text: &str) {
        const XK_CONTROL_L: std::ffi::c_ulong = 0xffe3;
        const XK_SHIFT_L: std::ffi::c_ulong = 0xffe1;
        let keycode =
            |keysym| unsafe { XKeysymToKeycode(self.display, keysym) as std::ffi::c_uint };
        let fake_key = |code, pressed| unsafe {
            assert_ne!(
                XTestFakeKeyEvent(self.display, code, if pressed { 1 } else { 0 }, 0),
                0,
                "XTest could not send a keyboard event"
            );
        };

        // Replace the folder field through ordinary key events so the test addresses its unique
        // temporary fixture path instead of relying on directory-list order or stale entries.
        let control = keycode(XK_CONTROL_L);
        let a = keycode(u64::from(b'a'));
        assert_ne!(control, 0);
        assert_ne!(a, 0);
        fake_key(control, true);
        fake_key(a, true);
        fake_key(a, false);
        fake_key(control, false);

        for byte in text.bytes() {
            assert!(byte.is_ascii(), "native fixture paths must be ASCII");
            let (keysym, shifted) = if byte == b'_' {
                (u64::from(b'-'), true)
            } else {
                (u64::from(byte), false)
            };
            let code = keycode(keysym);
            assert_ne!(code, 0, "X11 has no keycode for path byte {byte:?}");
            if shifted {
                fake_key(keycode(XK_SHIFT_L), true);
            }
            fake_key(code, true);
            fake_key(code, false);
            if shifted {
                fake_key(keycode(XK_SHIFT_L), false);
            }
        }
        // SAFETY: flush the queued XTest input on this live display connection.
        unsafe { XSync(self.display, 0) };
        thread::sleep(Duration::from_millis(200));
    }

    fn capture_if_requested(&self, variable: &str) {
        let Some(path) = std::env::var_os(variable) else {
            return;
        };
        // This test-only screenshot is optional. ImageMagick is available in the prepared Xvfb
        // environment, but the functional GUI regression does not depend on it.
        let status = Command::new("import")
            .arg("-window")
            .arg(format!("0x{:x}", self.window))
            .arg(path)
            .status()
            .expect("launch ImageMagick import for requested screenshot");
        assert!(status.success(), "failed to capture the visible editor");
    }
}

#[cfg(target_os = "linux")]
impl Drop for X11HostWindow {
    fn drop(&mut self) {
        // SAFETY: the parent window and display are owned by this helper.
        unsafe {
            XDestroyWindow(self.display, self.window);
            XSync(self.display, 0);
            XCloseDisplay(self.display);
        }
    }
}

#[cfg(target_os = "linux")]
fn wait_for_clap_restart(plugin: &NativeClap, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut serviced_callbacks = 0;
    while plugin.host_state().restart_requests.load(Ordering::Acquire) < expected
        && Instant::now() < deadline
    {
        let requested_callbacks = plugin
            .host_state()
            .callback_requests
            .load(Ordering::Acquire);
        if requested_callbacks > serviced_callbacks {
            plugin.service_main_thread_callback();
            serviced_callbacks = requested_callbacks;
        }
        thread::sleep(Duration::from_millis(8));
    }
    assert!(
        plugin.host_state().restart_requests.load(Ordering::Acquire) >= expected,
        "CLAP host did not receive restart request {expected}"
    );
    assert!(
        plugin
            .host_state()
            .restart_on_main_thread
            .load(Ordering::Acquire),
        "CLAP restart callback was not delivered on the host main thread"
    );
}

struct NativeVst3 {
    component: VstPtr<dyn IComponent>,
    processor: VstPtr<dyn IAudioProcessor>,
    controller: VstPtr<dyn IEditController>,
    _wrapper: Box<Vst3Wrapper<NativeConvolutionStateProbe>>,
    active: bool,
    processing: bool,
}

impl NativeVst3 {
    fn new() -> Self {
        let wrapper = Vst3Wrapper::<NativeConvolutionStateProbe>::new();
        // SAFETY: each queried interface owns a COM reference. `wrapper` remains alive for this
        // fixture's lifetime and the two owning VstPtrs are dropped before it.
        let (component, processor, controller) = unsafe {
            let mut pointer = ptr::null_mut();
            assert_eq!(
                wrapper.query_interface(&<dyn IComponent>::IID, &mut pointer),
                kResultOk
            );
            let component = VstPtr::<dyn IComponent>::owned(pointer.cast()).unwrap();
            let processor = component.cast::<dyn IAudioProcessor>().unwrap();
            let controller = component.cast::<dyn IEditController>().unwrap();
            assert_eq!(component.initialize(ptr::null_mut()), kResultOk);
            (component, processor, controller)
        };
        Self {
            component,
            processor,
            controller,
            _wrapper: wrapper,
            active: false,
            processing: false,
        }
    }

    fn state(&self) -> PluginState {
        serde_json::from_slice(&self.state_bytes()).expect("VST3 component state is valid JSON")
    }

    fn state_bytes(&self) -> Vec<u8> {
        let (result, bytes) = with_vst3_stream(&[], true, |stream| {
            // SAFETY: the borrowed stream remains owned and valid until this synchronous callback
            // returns.
            unsafe { self.component.get_state(stream) }
        });
        assert_eq!(result, kResultOk);
        bytes
    }

    fn load_state(&self, state: &PluginState) -> tresult {
        let bytes = serde_json::to_vec(state).unwrap();
        self.load_bytes(&bytes)
    }

    fn load_bytes(&self, bytes: &[u8]) -> tresult {
        with_vst3_stream(bytes, false, |stream| {
            // SAFETY: the borrowed stream remains owned and valid until this synchronous callback
            // returns.
            unsafe { self.component.set_state(stream) }
        })
        .0
    }

    #[cfg(target_os = "linux")]
    fn set_component_handler(&self, host: &NativeVst3Host) {
        let handler = host.component_handler();
        // SAFETY: the controller borrows this COM interface for the call and retains its own
        // reference; `handler` remains owned until the call returns.
        let shared: SharedVstPtr<dyn IComponentHandler> =
            unsafe { std::mem::transmute(handler.as_ptr()) };
        assert_eq!(
            unsafe { self.controller.set_component_handler(shared) },
            kResultOk
        );
    }

    #[cfg(target_os = "linux")]
    fn open_editor(&self, host: &NativeVst3Host, parent: &X11HostWindow) -> NativeVst3Editor {
        // SAFETY: create_view returns one owned IPlugView reference for the live controller.
        let raw_view = unsafe { self.controller.create_view(c"editor".as_ptr()) };
        assert!(
            !raw_view.is_null(),
            "VST3 Convolution editor was not created"
        );
        let view = unsafe { VstPtr::<dyn IPlugView>::owned(raw_view.cast()).unwrap() };
        // SAFETY: these calls follow the host IPlugView lifecycle on the test's UI thread. The
        // host window and frame outlive the view, and `set_frame` retains its own frame reference.
        unsafe {
            assert_eq!(
                view.is_platform_type_supported(c"X11EmbedWindowID".as_ptr()),
                kResultOk
            );
            let frame = host.plug_frame();
            assert_eq!(view.set_frame(frame.as_ptr().cast()), kResultOk);
            assert_eq!(
                view.attached(
                    parent.window as usize as *mut c_void,
                    c"X11EmbedWindowID".as_ptr()
                ),
                kResultOk
            );
            let mut size = ViewRect::default();
            assert_eq!(view.get_size(&mut size), kResultOk);
            assert!(size.right > size.left && size.bottom > size.top);
            assert_eq!(view.on_size(&mut size), kResultOk);
        }
        thread::sleep(Duration::from_millis(500));
        NativeVst3Editor { view: Some(view) }
    }

    fn setup(&self) {
        let setup = ProcessSetup {
            process_mode: ProcessModes::kRealtime as i32,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            max_samples_per_block: MAX_FRAMES as i32,
            sample_rate: SAMPLE_RATE,
        };
        // SAFETY: `setup` remains live for the synchronous ABI callback.
        assert_eq!(
            unsafe { self.processor.setup_processing(&setup) },
            kResultOk
        );
    }

    fn try_activate(&mut self) -> bool {
        // SAFETY: activation and processing follow the VST3 component lifecycle on this control
        // thread.
        unsafe {
            if self.component.set_active(1) != kResultOk {
                return false;
            }
            self.active = true;
            if self.processor.set_processing(1) != kResultOk {
                self.component.set_active(0);
                self.active = false;
                return false;
            }
        }
        self.processing = true;
        true
    }

    fn activate(&mut self) {
        self.setup();
        assert!(self.try_activate(), "native VST3 activation failed");
    }

    fn deactivate(&mut self) {
        // SAFETY: each callback is paired with the successful call recorded above.
        unsafe {
            if self.processing {
                assert_eq!(self.processor.set_processing(0), kResultOk);
                self.processing = false;
            }
            if self.active {
                assert_eq!(self.component.set_active(0), kResultOk);
                self.active = false;
            }
        }
    }

    fn process(&self, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
        self.process_with_parameter_changes(input, None)
    }

    #[cfg(target_os = "linux")]
    fn process_with_mix_event(&self, input: &[[f32; 2]], value: f64) -> Vec<[f32; 2]> {
        let id = vst3_parameter_id("mix");
        // SAFETY: this mirrors the host's controller conversion for the public mix parameter.
        let normalized = unsafe { self.controller.plain_param_to_normalized(id, value) };
        let changes = vst3_single_parameter_change(id, normalized);
        self.process_with_parameter_changes(input, Some(changes))
    }

    fn process_with_parameter_changes(
        &self,
        input: &[[f32; 2]],
        parameter_changes: Option<VstPtr<dyn IParameterChanges>>,
    ) -> Vec<[f32; 2]> {
        assert!(self.processing);
        let mut output = Vec::with_capacity(input.len());
        for (block_index, block) in input.chunks(MAX_FRAMES).enumerate() {
            let mut left_input: Vec<f32> = block.iter().map(|frame| frame[0]).collect();
            let mut right_input: Vec<f32> = block.iter().map(|frame| frame[1]).collect();
            let mut left_output = vec![f32::NAN; block.len()];
            let mut right_output = vec![f32::NAN; block.len()];
            let mut input_channels = [
                left_input.as_mut_ptr().cast(),
                right_input.as_mut_ptr().cast(),
            ];
            let mut output_channels = [
                left_output.as_mut_ptr().cast(),
                right_output.as_mut_ptr().cast(),
            ];
            let mut input_bus = AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                buffers: input_channels.as_mut_ptr(),
            };
            let mut output_bus = AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                buffers: output_channels.as_mut_ptr(),
            };
            // SAFETY: all channel pointers address `block.len()` initialized samples and remain
            // live for the synchronous process callback.
            let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
            data.process_mode = ProcessModes::kRealtime as i32;
            data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
            data.num_samples = block.len() as i32;
            data.num_inputs = 1;
            data.num_outputs = 1;
            data.inputs = &mut input_bus;
            data.outputs = &mut output_bus;
            if block_index == 0
                && let Some(changes) = &parameter_changes
            {
                // SAFETY: `changes` owns the input event graph through this synchronous process
                // callback, and the event queues retain all point storage for the same lifetime.
                data.input_param_changes = unsafe { std::mem::transmute(changes.as_ptr()) };
            }
            // SAFETY: bus pointers and backing buffers remain valid through this call.
            assert_eq!(unsafe { self.processor.process(&mut data) }, kResultOk);
            assert!(
                left_output
                    .iter()
                    .chain(&right_output)
                    .all(|sample| sample.is_finite())
            );
            output.extend(
                left_output
                    .into_iter()
                    .zip(right_output)
                    .map(|(left, right)| [left, right]),
            );
        }
        output
    }
}

#[cfg(target_os = "linux")]
type NativeVst3ParameterPoints = Rc<RefCell<Vec<(i32, f64)>>>;

#[cfg(target_os = "linux")]
#[VST3(implements(IParamValueQueue))]
struct NativeVst3ParameterQueue {
    id: u32,
    points: NativeVst3ParameterPoints,
}

#[cfg(target_os = "linux")]
impl IParamValueQueue for NativeVst3ParameterQueue {
    unsafe fn get_parameter_id(&self) -> u32 {
        self.id
    }

    unsafe fn get_point_count(&self) -> i32 {
        self.points.borrow().len() as i32
    }

    unsafe fn get_point(&self, index: i32, sample_offset: *mut i32, value: *mut f64) -> tresult {
        let Ok(index) = usize::try_from(index) else {
            return kInvalidArgument;
        };
        if sample_offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        let points = self.points.borrow();
        let Some((offset, point)) = points.get(index).copied() else {
            return kInvalidArgument;
        };
        // SAFETY: the required output pointers were checked above.
        unsafe {
            *sample_offset = offset;
            *value = point;
        }
        kResultOk
    }

    unsafe fn add_point(&self, sample_offset: i32, value: f64, index: *mut i32) -> tresult {
        if index.is_null() {
            return kInvalidArgument;
        }
        let mut points = self.points.borrow_mut();
        let point_index = points.len();
        points.push((sample_offset, value));
        // SAFETY: the required output pointer was checked above.
        unsafe { *index = point_index as i32 };
        kResultOk
    }
}

#[cfg(target_os = "linux")]
#[VST3(implements(IParameterChanges))]
struct NativeVst3ParameterChanges {
    queues: Vec<VstPtr<dyn IParamValueQueue>>,
    points: Vec<NativeVst3ParameterPoints>,
}

#[cfg(target_os = "linux")]
impl IParameterChanges for NativeVst3ParameterChanges {
    unsafe fn get_parameter_count(&self) -> i32 {
        self.points
            .iter()
            .filter(|points| !points.borrow().is_empty())
            .count() as i32
    }

    unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
        let Ok(index) = usize::try_from(index) else {
            // SAFETY: a zeroed StaticVstPtr is VST3's null queue sentinel.
            return unsafe { std::mem::zeroed() };
        };
        self.queues
            .iter()
            .zip(&self.points)
            .filter(|(_, points)| !points.borrow().is_empty())
            .nth(index)
            .map_or_else(
                || {
                    // SAFETY: a zeroed StaticVstPtr is VST3's null queue sentinel.
                    unsafe { std::mem::zeroed() }
                },
                |(queue, _)| {
                    // SAFETY: this object owns the queue throughout the synchronous process call.
                    unsafe { std::mem::transmute(queue.as_ptr()) }
                },
            )
    }

    unsafe fn add_parameter_data(
        &self,
        _id: *const u32,
        _index: *mut i32,
    ) -> StaticVstPtr<dyn IParamValueQueue> {
        // SAFETY: input parameter changes are immutable to the plugin.
        unsafe { std::mem::zeroed() }
    }
}

#[cfg(target_os = "linux")]
fn vst3_single_parameter_change(id: u32, value: f64) -> VstPtr<dyn IParameterChanges> {
    let points = Rc::new(RefCell::new(vec![(0, value)]));
    let queue = NativeVst3ParameterQueue::allocate(id, Rc::clone(&points));
    // SAFETY: ownership of the generated queue object is transferred to its owning VstPtr.
    let queue = unsafe {
        VstPtr::<dyn IParamValueQueue>::owned(Box::into_raw(queue).cast())
            .expect("generated VST3 parameter queue is non-null")
    };
    let changes = NativeVst3ParameterChanges::allocate(vec![queue], vec![points]);
    // SAFETY: ownership of the generated changes object is transferred to its owning VstPtr.
    unsafe {
        VstPtr::<dyn IParameterChanges>::owned(Box::into_raw(changes).cast())
            .expect("generated VST3 parameter changes is non-null")
    }
}

impl Drop for NativeVst3 {
    fn drop(&mut self) {
        self.deactivate();
    }
}

#[cfg(target_os = "linux")]
struct NativeVst3Editor {
    view: Option<VstPtr<dyn IPlugView>>,
}

#[cfg(target_os = "linux")]
impl Drop for NativeVst3Editor {
    fn drop(&mut self) {
        if let Some(view) = self.view.take() {
            // SAFETY: this guard owns an attached IPlugView and is dropped before its VST3 host,
            // plugin wrapper, and X11 parent. The VST3 host lifecycle removes the view before
            // detaching its frame, which unregisters the host run-loop callback.
            unsafe {
                let _ = view.removed();
                let _ = view.set_frame(ptr::null_mut());
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
struct Vst3RestartCall {
    flags: i32,
    thread: ThreadId,
    result: tresult,
}

#[cfg(target_os = "linux")]
struct NativeVst3HostState {
    restart_calls: Mutex<Vec<Vst3RestartCall>>,
    restart_results: Mutex<VecDeque<tresult>>,
    registered_handlers: Mutex<Vec<(usize, FileDescriptor)>>,
    register_calls: AtomicUsize,
    unregister_calls: AtomicUsize,
    resize_calls: AtomicUsize,
    main_thread: ThreadId,
}

#[cfg(target_os = "linux")]
impl NativeVst3HostState {
    fn pump(&self) {
        let handlers = self.registered_handlers.lock().unwrap().clone();
        for (address, fd) in handlers {
            // SAFETY: the wrapper unregisters its handler before destroying it, and pump runs on
            // this host's UI thread. The test serializes pumping with view removal.
            let handler = unsafe {
                VstPtr::<dyn IEventHandler>::shared(address as *mut _)
                    .expect("registered VST3 GUI event handler stays alive")
            };
            unsafe { handler.on_fd_is_set(fd) };
        }
    }

    fn reload_call_count(&self) -> usize {
        self.restart_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.flags & RestartFlags::kReloadComponent as i32 != 0)
            .count()
    }

    fn pump_for(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.pump();
            thread::sleep(Duration::from_millis(5));
        }
        self.pump();
    }
}

/// A minimal VST3 host object. The same COM object supplies the component restart handler, plug
/// frame, and Linux run loop, so `IPlugView::set_frame()` exercises the real wrapper registration
/// and deferred callback path.
#[cfg(target_os = "linux")]
#[VST3(implements(IComponentHandler, IPlugFrame, IRunLoop))]
struct NativeVst3Host {
    state: Arc<NativeVst3HostState>,
    run_loop_result: tresult,
}

#[cfg(target_os = "linux")]
impl NativeVst3Host {
    fn new(
        restart_results: impl IntoIterator<Item = tresult>,
    ) -> (Box<Self>, Arc<NativeVst3HostState>) {
        let state = Arc::new(NativeVst3HostState {
            restart_calls: Mutex::new(Vec::new()),
            restart_results: Mutex::new(restart_results.into_iter().collect()),
            registered_handlers: Mutex::new(Vec::new()),
            register_calls: AtomicUsize::new(0),
            unregister_calls: AtomicUsize::new(0),
            resize_calls: AtomicUsize::new(0),
            main_thread: thread::current().id(),
        });
        (Self::allocate(Arc::clone(&state), kResultOk), state)
    }

    fn queue_restart_results(&self, results: impl IntoIterator<Item = tresult>) {
        self.state.restart_results.lock().unwrap().extend(results);
    }

    fn component_handler(&self) -> VstPtr<dyn IComponentHandler> {
        let mut raw = ptr::null_mut::<c_void>();
        // SAFETY: QueryInterface returns one owned COM reference, released with the VstPtr.
        assert_eq!(
            unsafe { self.query_interface(&<dyn IComponentHandler>::IID, &mut raw) },
            kResultOk
        );
        unsafe { VstPtr::<dyn IComponentHandler>::owned(raw.cast()).unwrap() }
    }

    fn plug_frame(&self) -> VstPtr<dyn IPlugFrame> {
        let mut raw = ptr::null_mut::<c_void>();
        // SAFETY: QueryInterface returns one owned COM reference, released with the VstPtr.
        assert_eq!(
            unsafe { self.query_interface(&<dyn IPlugFrame>::IID, &mut raw) },
            kResultOk
        );
        unsafe { VstPtr::<dyn IPlugFrame>::owned(raw.cast()).unwrap() }
    }
}

#[cfg(target_os = "linux")]
impl IComponentHandler for NativeVst3Host {
    unsafe fn begin_edit(&self, _id: u32) -> tresult {
        kResultOk
    }

    unsafe fn perform_edit(&self, _id: u32, _value_normalized: f64) -> tresult {
        kResultOk
    }

    unsafe fn end_edit(&self, _id: u32) -> tresult {
        kResultOk
    }

    unsafe fn restart_component(&self, flags: i32) -> tresult {
        let result = if flags & RestartFlags::kReloadComponent as i32 != 0 {
            self.state
                .restart_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(kResultOk)
        } else {
            // Ordinary startup parameter/latency notifications are not reload requests and must
            // not consume the scripted refusal/retry responses for the editor action.
            kResultOk
        };
        self.state
            .restart_calls
            .lock()
            .unwrap()
            .push(Vst3RestartCall {
                flags,
                thread: thread::current().id(),
                result,
            });
        result
    }
}

#[cfg(target_os = "linux")]
impl IPlugFrame for NativeVst3Host {
    unsafe fn resize_view(
        &self,
        _view: SharedVstPtr<dyn IPlugView>,
        new_size: *mut ViewRect,
    ) -> tresult {
        if new_size.is_null() {
            return kInvalidArgument;
        }
        if unsafe { (*new_size).right <= (*new_size).left || (*new_size).bottom <= (*new_size).top }
        {
            return kInvalidArgument;
        }
        self.state.resize_calls.fetch_add(1, Ordering::AcqRel);
        kResultOk
    }
}

#[cfg(target_os = "linux")]
impl IRunLoop for NativeVst3Host {
    unsafe fn register_event_handler(
        &self,
        mut handler: SharedVstPtr<dyn IEventHandler>,
        fd: FileDescriptor,
    ) -> tresult {
        self.state.register_calls.fetch_add(1, Ordering::AcqRel);
        if self.run_loop_result == kResultOk {
            self.state
                .registered_handlers
                .lock()
                .unwrap()
                .push((handler.as_ptr() as usize, fd));
        }
        self.run_loop_result
    }

    unsafe fn unregister_event_handler(
        &self,
        mut handler: SharedVstPtr<dyn IEventHandler>,
    ) -> tresult {
        self.state.unregister_calls.fetch_add(1, Ordering::AcqRel);
        let address = handler.as_ptr() as usize;
        self.state
            .registered_handlers
            .lock()
            .unwrap()
            .retain(|(registered, _)| *registered != address);
        kResultOk
    }

    unsafe fn register_timer(
        &self,
        _handler: SharedVstPtr<dyn ITimerHandler>,
        _interval: TimerInterval,
    ) -> tresult {
        kResultFalse
    }

    unsafe fn unregister_timer(&self, _handler: SharedVstPtr<dyn ITimerHandler>) -> tresult {
        kResultFalse
    }
}

#[cfg(target_os = "linux")]
fn wait_for_vst3_restart(
    host: &NativeVst3HostState,
    expected_count: usize,
    expected_result: tresult,
) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while host.reload_call_count() < expected_count && Instant::now() < deadline {
        host.pump();
        thread::sleep(Duration::from_millis(5));
    }
    let calls = host.restart_calls.lock().unwrap();
    let reload_calls: Vec<_> = calls
        .iter()
        .filter(|call| call.flags & RestartFlags::kReloadComponent as i32 != 0)
        .collect();
    assert!(
        reload_calls.len() >= expected_count,
        "VST3 reload request timed out; observed flags {:?}",
        calls.iter().map(|call| call.flags).collect::<Vec<_>>()
    );
    let call = reload_calls[expected_count - 1];
    assert_eq!(call.flags, RestartFlags::kReloadComponent as i32);
    assert_eq!(
        call.thread, host.main_thread,
        "restart must run on the host UI thread"
    );
    assert_eq!(call.result, expected_result);
}

#[cfg(target_os = "linux")]
fn vst3_parameter_id(id: &str) -> u32 {
    id.bytes().fold(0_u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    }) & !(1 << 31)
}

#[VST3(implements(IBStream))]
struct Vst3MemoryStream {
    bytes: Rc<RefCell<Vec<u8>>>,
    cursor: Cell<usize>,
    writable: bool,
}

impl Vst3MemoryStream {
    fn new(initial: &[u8], writable: bool) -> (Box<Self>, Rc<RefCell<Vec<u8>>>) {
        let bytes = Rc::new(RefCell::new(initial.to_vec()));
        (
            Self::allocate(Rc::clone(&bytes), Cell::new(0), writable),
            bytes,
        )
    }
}

impl IBStream for Vst3MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> tresult {
        let Ok(requested) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if requested > 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let cursor = self.cursor.get();
        let count = requested.min(bytes.len().saturating_sub(cursor));
        if count > 0 {
            // SAFETY: `count` fits the source and caller-provided buffer capacity.
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), buffer.cast(), count) };
        }
        self.cursor.set(cursor + count);
        if !num_bytes_read.is_null() {
            // SAFETY: the ABI supplies a writable output pointer when it is non-null.
            unsafe { *num_bytes_read = count as i32 };
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *const c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> tresult {
        let Ok(count) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if !self.writable || (count > 0 && buffer.is_null()) {
            return kResultFalse;
        }
        let cursor = self.cursor.get();
        let Some(end) = cursor.checked_add(count) else {
            return kInvalidArgument;
        };
        let mut bytes = self.bytes.borrow_mut();
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        if count > 0 {
            // SAFETY: `count` readable bytes are supplied by the ABI and the destination was
            // resized to cover the complete write.
            unsafe {
                ptr::copy_nonoverlapping(buffer.cast(), bytes.as_mut_ptr().add(cursor), count)
            };
        }
        self.cursor.set(end);
        if !num_bytes_written.is_null() {
            // SAFETY: the ABI supplies a writable output pointer when it is non-null.
            unsafe { *num_bytes_written = count as i32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let base = match mode {
            value if value == kIBSeekSet => 0_i128,
            value if value == kIBSeekCur => self.cursor.get() as i128,
            value if value == kIBSeekEnd => self.bytes.borrow().len() as i128,
            _ => return kInvalidArgument,
        };
        let Ok(target) = usize::try_from(base + i128::from(pos)) else {
            return kInvalidArgument;
        };
        self.cursor.set(target);
        if !result.is_null() {
            // SAFETY: the ABI supplies a writable output pointer when it is non-null.
            unsafe { *result = target as i64 };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: VST3 requires a writable output pointer for `tell()`.
        unsafe { *pos = self.cursor.get() as i64 };
        kResultOk
    }
}

fn with_vst3_stream(
    bytes: &[u8],
    writable: bool,
    callback: impl FnOnce(SharedVstPtr<dyn IBStream>) -> tresult,
) -> (tresult, Vec<u8>) {
    let (stream, contents) = Vst3MemoryStream::new(bytes, writable);
    // SAFETY: ownership transfers once. The synchronous callback borrows this pointer while the
    // owning VstPtr remains alive.
    let stream = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap() };
    // SAFETY: SharedVstPtr is a borrowed ABI pointer and `stream` remains alive for the callback.
    let shared: SharedVstPtr<dyn IBStream> = unsafe { std::mem::transmute(stream.as_ptr()) };
    let result = callback(shared);
    (result, contents.borrow().clone())
}

struct StateReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

unsafe extern "C" fn read_state(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let Ok(size) = usize::try_from(size) else {
        return -1;
    };
    // SAFETY: ctx points to the StateReader borrowed for this synchronous callback.
    let reader = unsafe { &mut *(*stream).ctx.cast::<StateReader<'_>>() };
    let count = size.min(reader.bytes.len().saturating_sub(reader.offset));
    // SAFETY: the stream supplied a writable buffer and the source range is bounded above.
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

unsafe extern "C" fn write_state(
    stream: *const clap_ostream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let Ok(size) = usize::try_from(size) else {
        return -1;
    };
    // SAFETY: ctx points to the output vector borrowed for this synchronous callback.
    let bytes = unsafe { &mut *(*stream).ctx.cast::<Vec<u8>>() };
    // SAFETY: the plugin supplies `size` readable bytes until this callback returns.
    let source = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size) };
    bytes.extend_from_slice(source);
    size as i64
}

struct ImpulseFile(PathBuf);

impl ImpulseFile {
    fn write_true_stereo(samples: [[i16; 4]; 3]) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sotf-native-convolution-state-{}-{unique}.wav",
            std::process::id()
        ));
        write_pcm16_wav(&path, &samples);
        Self(path)
    }

    fn absolute_path(&self) -> String {
        self.0
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn remove(&self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

impl Drop for ImpulseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_pcm16_wav(path: &Path, samples: &[[i16; 4]]) {
    const CHANNELS: u16 = 4;
    const SAMPLE_RATE_HZ: u32 = 48_000;
    let data_bytes = u32::try_from(samples.len() * usize::from(CHANNELS) * 2).unwrap();
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE_HZ * u32::from(CHANNELS) * 2).to_le_bytes());
    bytes.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in samples {
        for sample in frame {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).unwrap();
}

#[cfg(target_os = "linux")]
fn write_pcm16_stereo_wav(path: &Path, samples: &[[i16; 2]]) {
    const CHANNELS: u16 = 2;
    const SAMPLE_RATE_HZ: u32 = 48_000;
    let data_bytes = u32::try_from(samples.len() * usize::from(CHANNELS) * 2).unwrap();
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE_HZ * u32::from(CHANNELS) * 2).to_le_bytes());
    bytes.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in samples {
        for sample in frame {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).unwrap();
}

#[cfg(target_os = "linux")]
struct EditorImpulseFiles {
    directory: PathBuf,
    old_impulse: [[i16; 2]; 3],
    new_impulse: [[i16; 4]; 3],
}

#[cfg(target_os = "linux")]
impl EditorImpulseFiles {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::current_dir().unwrap().join(format!(
            "000-aud134-native-gui-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let old_impulse = [[18_000, 0], [0, 8_000], [2_000, -1_000]];
        let new_impulse = [
            [14_000, 2_000, -3_000, 10_000],
            [3_000, -2_000, 4_000, 1_000],
            [-1_000, 500, 2_000, -1_500],
        ];
        write_pcm16_stereo_wav(&directory.join("a-old.wav"), &old_impulse);
        write_pcm16_wav(&directory.join("z-new.wav"), &new_impulse);
        Self {
            directory,
            old_impulse,
            new_impulse,
        }
    }

    fn old_path(&self) -> String {
        self.directory
            .join("a-old.wav")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn new_path(&self) -> String {
        self.directory
            .join("z-new.wav")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "linux")]
impl Drop for EditorImpulseFiles {
    fn drop(&mut self) {
        // Remove only this test's unique fixture directory, including on assertion failure.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn ir_state(plugin: &NativeClap, path: &str) -> PluginState {
    ir_state_from(plugin.new_state(), path)
}

fn ir_state_from(mut state: PluginState, path: &str) -> PluginState {
    state.params.insert("mix".into(), ParamValue::F32(0.65));
    state.params.insert("gain_db".into(), ParamValue::F32(0.0));
    state
        .params
        .insert("use_nupc".into(), ParamValue::Bool(true));
    state
        .params
        .insert("zero_latency_head".into(), ParamValue::Bool(false));
    state
        .params
        .insert("head_taps".into(), ParamValue::I32(128));
    state
        .params
        .insert("true_stereo".into(), ParamValue::Bool(true));
    state.fields.insert(
        IR_RESOURCE_FIELD.into(),
        serde_json::json!({"version": 1, "path": path}).to_string(),
    );
    state
}

fn decoded_state(bytes: &[u8]) -> PluginState {
    assert!(bytes.len() >= 8);
    let payload_len = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
    assert_eq!(bytes.len() - 8, payload_len);
    serde_json::from_slice(&bytes[8..]).unwrap()
}

fn assert_state_resource(bytes: &[u8], path: &str, mix: f32) {
    let state = decoded_state(bytes);
    assert_state_resource_object(&state, path, mix, true);
}

fn assert_state_resource_object(state: &PluginState, path: &str, mix: f32, true_stereo: bool) {
    let serialized = state.fields.get(IR_RESOURCE_FIELD).unwrap();
    let resource: serde_json::Value = serde_json::from_str(serialized).unwrap();
    assert_eq!(resource["version"], 1);
    assert_eq!(resource["path"], path);
    let actual_mix = state.params.get("mix");
    let actual_mix_bits = match actual_mix {
        Some(ParamValue::F32(value)) => Some(value.to_bits()),
        _ => None,
    };
    assert!(
        matches!(actual_mix, Some(ParamValue::F32(value)) if *value == mix),
        "saved mix {actual_mix:?} (bits {actual_mix_bits:?}) did not match expected f32 {mix:?} (bits {:08x})",
        mix.to_bits()
    );
    assert!(
        matches!(state.params.get("true_stereo"), Some(ParamValue::Bool(value)) if *value == true_stereo)
    );
}

fn input_sequence(frames: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|frame| {
            let t = frame as f32;
            [0.13 * (0.17 * t).sin(), 0.09 * (0.11 * t).cos()]
        })
        .collect()
}

fn direct_true_stereo(input: &[[f32; 2]], impulse: &[[i16; 4]], mix: f32) -> Vec<[f64; 2]> {
    let mut output = vec![[0.0_f64; 2]; input.len() + impulse.len() - 1];
    for (input_frame, [left, right]) in input.iter().copied().enumerate() {
        for (tap, [ll, lr, rl, rr]) in impulse.iter().copied().enumerate() {
            let frame = input_frame + tap;
            output[frame][0] += f64::from(left) * f64::from(ll) / 32_768.0;
            output[frame][0] += f64::from(right) * f64::from(rl) / 32_768.0;
            output[frame][1] += f64::from(left) * f64::from(lr) / 32_768.0;
            output[frame][1] += f64::from(right) * f64::from(rr) / 32_768.0;
        }
    }
    for (frame, output_frame) in output.iter_mut().enumerate() {
        let dry = input.get(frame).copied().unwrap_or([0.0; 2]);
        for channel in 0..2 {
            output_frame[channel] = f64::from(dry[channel]) * (1.0 - f64::from(mix))
                + output_frame[channel] * f64::from(mix);
        }
    }
    output
}

fn render_complete(plugin: &mut NativeClap, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let tail_frames = plugin.tail_frames();
    let total_frames = input.len() + tail_frames;
    let mut padded = vec![[0.0_f32; 2]; total_frames];
    padded[..input.len()].copy_from_slice(input);
    plugin.process(&padded)
}

fn render_complete_vst3(
    plugin: &NativeVst3,
    input: &[[f32; 2]],
    tail_frames: usize,
) -> Vec<[f32; 2]> {
    let mut padded = vec![[0.0_f32; 2]; input.len() + tail_frames];
    padded[..input.len()].copy_from_slice(input);
    plugin.process(&padded)
}

fn assert_waveform_matches_direct(
    output: &[[f32; 2]],
    input: &[[f32; 2]],
    impulse: &[[i16; 4]],
    latency: usize,
) {
    assert_waveform_matches_direct_with_mix(output, input, impulse, latency, 0.65);
}

fn assert_waveform_matches_direct_with_mix(
    output: &[[f32; 2]],
    input: &[[f32; 2]],
    impulse: &[[i16; 4]],
    latency: usize,
    mix: f32,
) {
    let reference = direct_true_stereo(input, impulse, mix);
    assert_eq!(output.len(), latency + reference.len());
    let mut max_error = 0.0_f64;
    let mut square_error = 0.0_f64;
    for (index, actual) in output.iter().enumerate() {
        for channel in 0..2 {
            assert!(
                actual[channel].is_finite(),
                "sample {index}, channel {channel}"
            );
            let expected = if (latency..latency + reference.len()).contains(&index) {
                reference[index - latency][channel]
            } else {
                0.0
            };
            let error = (f64::from(actual[channel]) - expected).abs();
            max_error = max_error.max(error);
            square_error += error * error;
        }
    }
    let rms_error = (square_error / (output.len() * 2) as f64).sqrt();
    assert!(max_error <= 1.0e-5, "peak error {max_error:e}");
    assert!(rms_error <= 1.0e-6, "RMS error {rms_error:e}");
}

#[cfg(target_os = "linux")]
fn assert_waveform_matches_stereo_direct(
    output: &[[f32; 2]],
    input: &[[f32; 2]],
    impulse: &[[i16; 2]],
    latency: usize,
    mix: f32,
) {
    let mut reference = vec![[0.0_f64; 2]; input.len() + impulse.len() - 1];
    for (input_frame, [left, right]) in input.iter().copied().enumerate() {
        for (tap, [left_gain, right_gain]) in impulse.iter().copied().enumerate() {
            let frame = input_frame + tap;
            reference[frame][0] += f64::from(left) * f64::from(left_gain) / 32_768.0;
            reference[frame][1] += f64::from(right) * f64::from(right_gain) / 32_768.0;
        }
    }
    for (frame, output_frame) in reference.iter_mut().enumerate() {
        let dry = input.get(frame).copied().unwrap_or([0.0; 2]);
        for channel in 0..2 {
            output_frame[channel] = f64::from(dry[channel]) * (1.0 - f64::from(mix))
                + output_frame[channel] * f64::from(mix);
        }
    }
    assert_eq!(output.len(), latency + reference.len());
    let mut max_error = 0.0_f64;
    let mut square_error = 0.0_f64;
    for (index, actual) in output.iter().enumerate() {
        for channel in 0..2 {
            assert!(actual[channel].is_finite());
            let expected = if index >= latency && index - latency < reference.len() {
                reference[index - latency][channel]
            } else {
                0.0
            };
            let error = (f64::from(actual[channel]) - expected).abs();
            max_error = max_error.max(error);
            square_error += error * error;
        }
    }
    let rms_error = (square_error / (output.len() * 2) as f64).sqrt();
    assert!(max_error <= 1.0e-5, "peak error {max_error:e}");
    assert!(rms_error <= 1.0e-6, "RMS error {rms_error:e}");
}

fn process_generated_block(
    plugin: &mut NativeConvolutionStateProbe,
    input: &[[f32; 2]],
) -> Vec<[f32; 2]> {
    let mut channels = [
        input.iter().map(|frame| frame[0]).collect::<Vec<_>>(),
        input.iter().map(|frame| frame[1]).collect::<Vec<_>>(),
    ];
    let mut buffer = Buffer::default();
    // SAFETY: both planar channels have the same initialized frame count and remain
    // alive until the generated NIH process callback returns.
    unsafe {
        buffer.set_slices(input.len(), |slices| {
            slices.extend(channels.iter_mut().map(Vec::as_mut_slice));
        });
    }
    let mut auxiliary = AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    let status = plugin.process_with_transport(&mut buffer, &mut auxiliary, Default::default());
    assert!(
        !matches!(status, ProcessStatus::Error(_)),
        "generated Convolution process failed: {status:?}"
    );
    let output = buffer.as_slice_immutable();
    assert_eq!(output.len(), 2);
    (0..input.len())
        .map(|frame| [output[0][frame], output[1][frame]])
        .collect()
}

fn process_generated_sequence(
    plugin: &mut NativeConvolutionStateProbe,
    input: &[[f32; 2]],
    block_size: usize,
) -> Vec<[f32; 2]> {
    assert!(block_size > 0);
    input
        .chunks(block_size)
        .flat_map(|block| process_generated_block(plugin, block))
        .collect()
}

fn initialize_generated_with_max_frames(
    plugin: &mut NativeConvolutionStateProbe,
    max_frames: usize,
) {
    let layout = <NativeConvolutionStateProbe as ClapPlugin>::clap_audio_io_layouts()
        .into_iter()
        .next()
        .expect("generated Convolution CLAP layout");
    let config = BufferConfig {
        sample_rate: SAMPLE_RATE as f32,
        min_buffer_size: Some(1),
        max_buffer_size: u32::try_from(max_frames).expect("test maximum fits in u32"),
        process_mode: ProcessMode::Realtime,
    };
    let mut context = super::TestContext;
    assert!(NihPlugin::initialize(
        plugin,
        &layout,
        &config,
        &mut context
    ));
}

fn render_generated_complete(
    plugin: &mut NativeConvolutionStateProbe,
    input: &[[f32; 2]],
    max_block_frames: usize,
) -> Vec<[f32; 2]> {
    assert!(max_block_frames > 0);
    let tail_frames =
        NihPlugin::tail_length(plugin).expect("initialized Convolution tail") as usize;
    let mut padded = vec![[0.0; 2]; input.len() + tail_frames];
    padded[..input.len()].copy_from_slice(input);

    let block_sizes = [13, 127, 1, 257, 64];
    let mut position = 0;
    let mut output = Vec::with_capacity(padded.len());
    for frames in block_sizes.into_iter().cycle() {
        if position == padded.len() {
            break;
        }
        let end = (position + frames.min(max_block_frames)).min(padded.len());
        output.extend(process_generated_block(plugin, &padded[position..end]));
        position = end;
    }
    assert_eq!(output.len(), input.len() + tail_frames);
    output
}

fn select_and_reactivate_generated(
    plugin: &mut NativeConvolutionStateProbe,
    path: Option<PathBuf>,
    true_stereo: bool,
    host_accepts_restart: bool,
) -> u64 {
    let params = Arc::clone(&plugin.params);
    let service = Arc::clone(&plugin.convolution_editor_service);
    let topology = params.convolution_editor_topology_fingerprint();
    let task = service
        .begin_prepare(path, true_stereo, topology)
        .expect("start off-thread Convolution preparation");
    let generation = match &task {
        crate::wrapper::native_convolution_editor::BackgroundTask::Prepare {
            generation, ..
        } => *generation,
    };
    let execute_background = NihPlugin::task_executor(plugin);
    execute_background(task);

    let request = service
        .ready_to_stage(topology)
        .expect("background candidate is ready to stage");
    let staged_fingerprint =
        params.convolution_editor_staged_structural_fingerprint(request.true_stereo);
    assert!(
        service.stage_selection(request.generation, topology, staged_fingerprint, || {
            if !params.stage_convolution_editor_selection(
                request.generation,
                request.path.clone(),
                request.true_stereo,
            ) {
                return false;
            }
            let Some(parameter) = params.native_bool_param("true_stereo") else {
                return false;
            };
            parameter.value() == request.true_stereo
                || parameter.set_plain_value_for_initialization(request.true_stereo)
        })
    );
    service.set_restart_result(request.generation, host_accepts_restart);

    if host_accepts_restart {
        super::initialize(plugin);
        NihPlugin::reset(plugin);
    }
    generation
}

#[test]
fn generated_editor_candidate_reactivation_preserves_pending_audio_and_latest_mix() {
    let old_impulse = [
        [21_504, -6_144, 4_608, 17_408],
        [5_120, 2_560, -4_096, 1_792],
        [-1_280, 768, 1_280, -512],
    ];
    let replacement_impulse = [
        [18_432, -9_216, 7_168, 15_360],
        [3_584, 10_240, -6_656, 3_072],
        [-4_608, 2_304, 8_192, -2_048],
    ];
    let old_file = ImpulseFile::write_true_stereo(old_impulse);
    let replacement_file = ImpulseFile::write_true_stereo(replacement_impulse);

    let mut subject = NativeConvolutionStateProbe::default();
    let mut old_control = NativeConvolutionStateProbe::default();
    super::initialize(&mut subject);
    super::initialize(&mut old_control);
    NihPlugin::reset(&mut subject);
    NihPlugin::reset(&mut old_control);

    for plugin in [&subject, &old_control] {
        plugin
            .params
            .native_float_param("mix")
            .expect("native Convolution wet control")
            .set_plain_value_for_initialization(0.65);
    }

    select_and_reactivate_generated(&mut subject, Some(old_file.0.clone()), false, true);
    select_and_reactivate_generated(&mut old_control, Some(old_file.0.clone()), false, true);

    let warm = input_sequence(1_307);
    assert_eq!(
        (0..warm.len())
            .step_by(127)
            .flat_map(|start| {
                let end = (start + 127).min(warm.len());
                process_generated_block(&mut subject, &warm[start..end])
            })
            .collect::<Vec<_>>(),
        (0..warm.len())
            .step_by(127)
            .flat_map(|start| {
                let end = (start + 127).min(warm.len());
                process_generated_block(&mut old_control, &warm[start..end])
            })
            .collect::<Vec<_>>(),
        "identically initialized old IR instances establish equal history"
    );

    let params = Arc::clone(&subject.params);
    let service = Arc::clone(&subject.convolution_editor_service);
    let topology = params.convolution_editor_topology_fingerprint();
    let task = service
        .begin_prepare(Some(replacement_file.0.clone()), true, topology)
        .expect("begin replacement IR preparation");
    let generation = match &task {
        crate::wrapper::native_convolution_editor::BackgroundTask::Prepare {
            generation, ..
        } => *generation,
    };

    let during_prepare = input_sequence(193);
    assert_eq!(
        process_generated_block(&mut subject, &during_prepare),
        process_generated_block(&mut old_control, &during_prepare),
        "old prepared DSP continues while the replacement is being prepared"
    );

    let execute_background = NihPlugin::task_executor(&mut subject);
    execute_background(task);
    let request = service
        .ready_to_stage(topology)
        .expect("replacement IR candidate prepared");
    let staged_fingerprint = params.convolution_editor_staged_structural_fingerprint(true);
    assert!(
        service.stage_selection(generation, topology, staged_fingerprint, || {
            params.stage_convolution_editor_selection(generation, request.path.clone(), true)
                && params
                    .native_bool_param("true_stereo")
                    .is_some_and(|parameter| parameter.set_plain_value_for_initialization(true))
        })
    );
    service.set_restart_result(generation, false);

    let pending = input_sequence(257);
    assert_eq!(
        process_generated_block(&mut subject, &pending),
        process_generated_block(&mut old_control, &pending),
        "host deferral leaves the complete old DSP route and history in place"
    );

    // The candidate was built with mix=.65; automation arriving while the host
    // defers reactivation must remain the value applied to the committed IR.
    const LATEST_MIX: f32 = 0.37;
    for plugin in [&subject, &old_control] {
        assert!(
            plugin
                .params
                .native_float_param("mix")
                .expect("native Convolution wet control")
                .set_plain_value_for_initialization(LATEST_MIX)
        );
    }
    let continued = input_sequence(311);
    assert_eq!(
        process_generated_sequence(&mut subject, &continued, 127),
        process_generated_sequence(&mut old_control, &continued, 127),
        "the old route continues while wet automation changes"
    );

    service.set_restart_result(generation, true);
    super::initialize(&mut subject);
    NihPlugin::reset(&mut subject);
    assert_eq!(
        params.value("true_stereo"),
        Some(sotf_host::parameters::ParameterValue::Bool(true))
    );
    assert_eq!(
        params.convolution_ir_path_for_initialization(),
        Some(replacement_file.0.clone())
    );

    let source = input_sequence(1_307);
    let output = render_generated_complete(&mut subject, &source, 257);
    let latency = NORMAL_LATENCY;
    assert_eq!(
        NihPlugin::tail_length(&subject),
        Some((latency + replacement_impulse.len() - 1) as u32)
    );
    assert_waveform_matches_direct_with_mix(
        &output,
        &source,
        &replacement_impulse,
        latency,
        LATEST_MIX,
    );

    let previous_same_route = direct_true_stereo(&source, &old_impulse, LATEST_MIX);
    assert!(previous_same_route.iter().enumerate().any(|(frame, old)| {
        let new = output[latency + frame];
        (f64::from(new[0]) - old[0]).abs() > 1.0e-3 || (f64::from(new[1]) - old[1]).abs() > 1.0e-3
    }));
}

#[test]
fn generated_editor_discards_geometry_and_out_of_order_generations_before_retry() {
    let old_impulse = [
        [20_480, -5_120, 4_096, 15_360],
        [4_096, 3_584, -3_072, 1_536],
        [-1_536, 1_024, 2_048, -768],
    ];
    let geometry_stale_impulse = [
        [16_384, -10_240, 6_144, 14_336],
        [2_560, 8_192, -5_120, 2_048],
        [-3_072, 1_536, 6_144, -1_024],
    ];
    let fresh_impulse = [
        [18_944, -7_680, 5_632, 17_920],
        [5_632, 9_216, -7_168, 3_072],
        [-4_096, 2_048, 7_680, -1_792],
    ];
    let old_file = ImpulseFile::write_true_stereo(old_impulse);
    let geometry_stale_file = ImpulseFile::write_true_stereo(geometry_stale_impulse);
    let fresh_file = ImpulseFile::write_true_stereo(fresh_impulse);

    let mut subject = NativeConvolutionStateProbe::default();
    let mut old_control = NativeConvolutionStateProbe::default();
    super::initialize(&mut subject);
    super::initialize(&mut old_control);
    NihPlugin::reset(&mut subject);
    NihPlugin::reset(&mut old_control);
    for plugin in [&subject, &old_control] {
        assert!(
            plugin
                .params
                .native_float_param("mix")
                .expect("native Convolution wet control")
                .set_plain_value_for_initialization(0.42)
        );
    }
    select_and_reactivate_generated(&mut subject, Some(old_file.0.clone()), false, true);
    select_and_reactivate_generated(&mut old_control, Some(old_file.0.clone()), false, true);

    let params = Arc::clone(&subject.params);
    let service = Arc::clone(&subject.convolution_editor_service);
    let topology = params.convolution_editor_topology_fingerprint();
    let old_geometry = Geometry {
        sample_rate: SAMPLE_RATE as u32,
        max_frames: 257,
        input_channels: 2,
        output_channels: 2,
    };
    let stale_prepared_task = service
        .begin_prepare(Some(geometry_stale_file.0.clone()), true, topology)
        .expect("start candidate under the original geometry");
    let stale_prepared_generation = match &stale_prepared_task {
        crate::wrapper::native_convolution_editor::BackgroundTask::Prepare {
            generation,
            sample_rate,
            max_frames,
            ..
        } => {
            assert_eq!(*sample_rate, old_geometry.sample_rate);
            assert_eq!(*max_frames, old_geometry.max_frames);
            *generation
        }
    };
    let execute_old_candidate = NihPlugin::task_executor(&mut subject);
    execute_old_candidate(stale_prepared_task);
    let prepared = service
        .ready_to_stage(topology)
        .expect("old-geometry background work produced a real candidate");
    assert_eq!(prepared.generation, stale_prepared_generation);
    assert_eq!(prepared.geometry, old_geometry);
    assert!(!service.allows_old_prepared_audio_for(params.structural_fingerprint()));

    // Reinitialize through the generated Plugin API with a genuinely different
    // host callback maximum. This updates the service's negotiated geometry.
    const RETRY_MAX_FRAMES: usize = 64;
    initialize_generated_with_max_frames(&mut subject, RETRY_MAX_FRAMES);
    initialize_generated_with_max_frames(&mut old_control, RETRY_MAX_FRAMES);
    NihPlugin::reset(&mut subject);
    NihPlugin::reset(&mut old_control);
    let retry_geometry = Geometry {
        sample_rate: SAMPLE_RATE as u32,
        max_frames: RETRY_MAX_FRAMES,
        input_channels: 2,
        output_channels: 2,
    };
    assert_ne!(retry_geometry, old_geometry);
    assert!(service.ready_to_stage(topology).is_none());
    assert!(service.status().contains("reconfigured"));
    assert!(!service.allows_old_prepared_audio_for(params.structural_fingerprint()));

    // Keep one generation in flight, replace it with a newer request, then
    // deliver the older completion late. It must not overwrite the new request.
    let older_completion = service
        .begin_prepare(Some(geometry_stale_file.0.clone()), false, topology)
        .expect("start an old-generation task at the new geometry");
    let older_generation = match &older_completion {
        crate::wrapper::native_convolution_editor::BackgroundTask::Prepare {
            generation,
            max_frames,
            ..
        } => {
            assert_eq!(*max_frames, RETRY_MAX_FRAMES);
            *generation
        }
    };
    let fresh_task = service
        .begin_prepare(Some(fresh_file.0.clone()), true, topology)
        .expect("newer geometry-matched request supersedes unprepared work");
    let fresh_generation = match &fresh_task {
        crate::wrapper::native_convolution_editor::BackgroundTask::Prepare {
            generation,
            sample_rate,
            max_frames,
            ..
        } => {
            assert_eq!(*sample_rate, retry_geometry.sample_rate);
            assert_eq!(*max_frames, retry_geometry.max_frames);
            *generation
        }
    };
    assert!(fresh_generation > older_generation);

    let execute_reordered_tasks = NihPlugin::task_executor(&mut subject);
    execute_reordered_tasks(older_completion);
    assert!(service.ready_to_stage(topology).is_none());
    assert!(service.status().contains("Preparing"));
    assert!(!service.allows_old_prepared_audio_for(params.structural_fingerprint()));

    let pending_audio = input_sequence(149);
    assert_eq!(
        process_generated_sequence(&mut subject, &pending_audio, 31),
        process_generated_sequence(&mut old_control, &pending_audio, 31),
        "old prepared audio remains available while the fresh request is pending"
    );

    execute_reordered_tasks(fresh_task);
    let fresh_request = service
        .ready_to_stage(topology)
        .expect("only the fresh request may publish a candidate");
    assert_eq!(fresh_request.generation, fresh_generation);
    assert_eq!(fresh_request.geometry, retry_geometry);
    assert_eq!(fresh_request.path, Some(fresh_file.0.clone()));
    assert!(!service.allows_old_prepared_audio_for(params.structural_fingerprint()));

    let staged_fingerprint = params.convolution_editor_staged_structural_fingerprint(true);
    assert!(
        service.stage_selection(fresh_generation, topology, staged_fingerprint, || {
            params.stage_convolution_editor_selection(
                fresh_generation,
                fresh_request.path.clone(),
                true,
            ) && params
                .native_bool_param("true_stereo")
                .is_some_and(|parameter| parameter.set_plain_value_for_initialization(true))
        })
    );
    assert!(service.allows_old_prepared_audio_for(staged_fingerprint));
    service.set_restart_result(fresh_generation, true);
    initialize_generated_with_max_frames(&mut subject, RETRY_MAX_FRAMES);
    NihPlugin::reset(&mut subject);
    assert_eq!(
        params.value("true_stereo"),
        Some(sotf_host::parameters::ParameterValue::Bool(true))
    );
    assert_eq!(
        params.convolution_ir_path_for_initialization(),
        Some(fresh_file.0.clone())
    );
    assert!(!service.allows_old_prepared_audio_for(params.structural_fingerprint()));

    let source = input_sequence(389);
    let output = render_generated_complete(&mut subject, &source, RETRY_MAX_FRAMES);
    let tail_frames = NihPlugin::tail_length(&subject).unwrap() as usize;
    assert_eq!(tail_frames, NORMAL_LATENCY + fresh_impulse.len() - 1);
    assert_waveform_matches_direct_with_mix(&output, &source, &fresh_impulse, NORMAL_LATENCY, 0.42);
}

fn assert_delayed_dry(output: &[[f32; 2]], input: &[[f32; 2]], latency: usize) {
    assert_eq!(output.len(), input.len() + latency);
    for (frame, output_frame) in output.iter().enumerate() {
        let expected = if frame >= latency && frame - latency < input.len() {
            input[frame - latency]
        } else {
            [0.0; 2]
        };
        assert_eq!(*output_frame, expected, "dry sample {frame}");
    }
}

#[test]
fn clap_state_restore_stages_true_stereo_resource_and_survives_rejections() {
    let impulse = [
        [24_576, -8_192, 4_096, 16_384],
        [6_144, 3_072, -5_120, 2_048],
        [-1_024, 512, 1_536, -768],
    ];
    let impulse_file = ImpulseFile::write_true_stereo(impulse);
    let path = impulse_file.absolute_path();
    let missing_path = format!("{path}.missing");
    let malformed_path = format!("{path}.malformed.wav");
    std::fs::write(&malformed_path, b"not a wave file").unwrap();

    let mut subject = NativeClap::new();
    let accepted_state = ir_state(&subject, &path);
    assert!(subject.load_state(&accepted_state), "fresh inactive load");
    let accepted_stream = subject.save_state();
    assert_state_resource(&accepted_stream, &path, 0.65);

    let missing_state = ir_state(&subject, &missing_path);
    assert!(
        !subject.load_state(&missing_state),
        "missing IR must refuse"
    );
    let malformed_state = ir_state(&subject, &malformed_path);
    assert!(
        !subject.load_state(&malformed_state),
        "malformed IR must refuse"
    );
    assert_state_resource(&subject.save_state(), &path, 0.65);

    let mut control = NativeClap::new();
    assert!(
        control.load_stream(&accepted_stream),
        "restore saved native bytes"
    );
    subject.activate();
    control.activate();

    let latency = subject.latency_frames();
    assert_eq!(latency, NORMAL_LATENCY);
    assert_eq!(control.latency_frames(), latency);
    assert_eq!(subject.tail_frames(), latency + impulse.len() - 1);

    let prefix = input_sequence(NORMAL_LATENCY + 37);
    assert_eq!(subject.process(&prefix), control.process(&prefix));
    assert!(
        !subject.load_state(&missing_state),
        "active state restore must refuse"
    );
    assert_state_resource(&subject.save_state(), &path, 0.65);

    let continuation = input_sequence(257);
    assert_eq!(
        subject.process(&continuation),
        control.process(&continuation),
        "failed native restore must preserve prepared DSP history"
    );

    subject.deactivate();
    let replacement_impulse = [
        [16_384, 4_096, -12_288, 8_192],
        [-4_096, 2_048, 6_144, -1_024],
        [2_048, -3_072, 5_120, 256],
    ];
    let replacement = ImpulseFile::write_true_stereo(replacement_impulse);
    let replacement_path = replacement.absolute_path();
    let replacement_state = ir_state(&subject, &replacement_path);
    assert!(
        subject.load_state(&replacement_state),
        "deactivated instance accepts a fully validated resource"
    );
    let replacement_stream = subject.save_state();
    assert_state_resource(&replacement_stream, &replacement_path, 0.65);
    let mut fresh_replacement = NativeClap::new();
    assert!(fresh_replacement.load_stream(&replacement_stream));
    subject.activate();
    fresh_replacement.activate();

    let source = input_sequence(73);
    let replacement_latency = subject.latency_frames();
    let replacement_tail = subject.tail_frames();
    assert_eq!(replacement_latency, latency);
    assert_eq!(replacement_tail, latency + replacement_impulse.len() - 1);
    assert_eq!(fresh_replacement.tail_frames(), replacement_tail);
    let subject_output = render_complete(&mut subject, &source);
    let fresh_output = render_complete(&mut fresh_replacement, &source);
    assert_eq!(subject_output, fresh_output);
    assert_waveform_matches_direct(
        &subject_output,
        &source,
        &replacement_impulse,
        replacement_latency,
    );
    let previous_ir_output = direct_true_stereo(&source, &impulse, 0.65);
    let mut old_and_new_differ = false;
    for (frame, expected_old) in previous_ir_output.iter().enumerate() {
        let actual_new = subject_output[replacement_latency + frame];
        old_and_new_differ |= (f64::from(actual_new[0]) - expected_old[0]).abs() > 1.0e-3
            || (f64::from(actual_new[1]) - expected_old[1]).abs() > 1.0e-3;
    }
    assert!(
        old_and_new_differ,
        "replacement IR must change the full output"
    );

    impulse_file.remove();
    let deleted_file_restore = NativeClap::new();
    assert!(
        !deleted_file_restore.load_state(&accepted_state),
        "deleted resource must not be restored as identity or dry state"
    );

    let mut legacy = NativeClap::new();
    let mut legacy_state = legacy.new_state();
    legacy_state.fields.remove(IR_RESOURCE_FIELD);
    assert!(
        legacy.load_state(&legacy_state),
        "legacy state migrates to dry"
    );
    legacy.activate();
    let legacy_latency = legacy.latency_frames();
    let legacy_tail = legacy.tail_frames();
    assert_eq!(legacy_latency, NORMAL_LATENCY);
    assert_eq!(legacy_tail, legacy_latency);
    let dry_source = input_sequence(1307);
    let dry_output = render_complete(&mut legacy, &dry_source);
    assert_eq!(dry_output.len(), dry_source.len() + legacy_tail);
    for (frame, output) in dry_output.iter().enumerate() {
        let expected = if frame >= legacy_latency && frame - legacy_latency < dry_source.len() {
            dry_source[frame - legacy_latency]
        } else {
            [0.0; 2]
        };
        assert_eq!(*output, expected, "legacy dry sample {frame}");
    }

    let pending_resource = ImpulseFile::write_true_stereo(impulse);
    let mut disappearing_resource = NativeClap::new();
    let pending_path = pending_resource.absolute_path();
    let pending_state = ir_state(&disappearing_resource, &pending_path);
    assert!(disappearing_resource.load_state(&pending_state));
    assert_state_resource(&disappearing_resource.save_state(), &pending_path, 0.65);
    pending_resource.remove();
    assert!(
        !disappearing_resource.try_activate(),
        "initialization must fail if the staged resource disappears"
    );
    let after_failed_init = decoded_state(&disappearing_resource.save_state());
    let resource_after_failed_init: serde_json::Value =
        serde_json::from_str(after_failed_init.fields.get(IR_RESOURCE_FIELD).unwrap()).unwrap();
    assert_eq!(resource_after_failed_init["path"], "");
    assert!(matches!(
        after_failed_init.params.get("mix"),
        Some(ParamValue::F32(value)) if *value == 1.0
    ));
    assert!(matches!(
        after_failed_init.params.get("true_stereo"),
        Some(ParamValue::Bool(false))
    ));
    disappearing_resource.activate();
    let after_ordinary_retry = decoded_state(&disappearing_resource.save_state());
    let resource_after_retry: serde_json::Value =
        serde_json::from_str(after_ordinary_retry.fields.get(IR_RESOURCE_FIELD).unwrap()).unwrap();
    assert_eq!(resource_after_retry["path"], "");
    assert!(matches!(
        after_ordinary_retry.params.get("mix"),
        Some(ParamValue::F32(value)) if *value == 1.0
    ));
    assert!(matches!(
        after_ordinary_retry.params.get("true_stereo"),
        Some(ParamValue::Bool(false))
    ));
    let retry_source = input_sequence(64);
    let retry_latency = disappearing_resource.latency_frames();
    let retry_output = render_complete(&mut disappearing_resource, &retry_source);
    assert_eq!(retry_output.len(), retry_source.len() + retry_latency);
    for (frame, output) in retry_output.iter().enumerate() {
        let expected = if frame >= retry_latency && frame - retry_latency < retry_source.len() {
            retry_source[frame - retry_latency]
        } else {
            [0.0; 2]
        };
        assert_eq!(*output, expected, "retry dry sample {frame}");
    }

    let _ = std::fs::remove_file(malformed_path);
}

#[test]
fn vst3_component_state_restore_stages_true_stereo_resource_and_preserves_audio() {
    let impulse = [
        [24_576, -8_192, 4_096, 16_384],
        [6_144, 3_072, -5_120, 2_048],
        [-1_024, 512, 1_536, -768],
    ];
    let impulse_file = ImpulseFile::write_true_stereo(impulse);
    let path = impulse_file.absolute_path();
    let missing_path = format!("{path}.missing");

    let mut subject = NativeVst3::new();
    let accepted_state = ir_state_from(subject.state(), &path);
    assert_eq!(subject.load_state(&accepted_state), kResultOk);
    let accepted_stream = subject.state_bytes();
    let accepted_state_from_stream: PluginState =
        serde_json::from_slice(&accepted_stream).expect("VST3 get_state stream JSON");
    assert_state_resource_object(&accepted_state_from_stream, &path, 0.65, true);

    let mut control = NativeVst3::new();
    assert_eq!(control.load_bytes(&accepted_stream), kResultOk);
    subject.activate();
    control.activate();
    // SAFETY: the initialized VST3 processor is queried synchronously while its component lives.
    let latency = unsafe { subject.processor.get_latency_samples() as usize };
    // SAFETY: the initialized VST3 processor is queried synchronously while its component lives.
    let tail = unsafe { subject.processor.get_tail_samples() as usize };
    assert_eq!(latency, NORMAL_LATENCY);
    assert_eq!(tail, latency + impulse.len() - 1);

    let prefix = input_sequence(latency + 37);
    assert_eq!(subject.process(&prefix), control.process(&prefix));
    let missing_state = ir_state_from(subject.state(), &missing_path);
    // SAFETY: VST3 state restoration is invoked on the active lifecycle/thread and must reject
    // before touching the prepared plugin.
    assert_eq!(subject.load_state(&missing_state), kResultFalse);
    let preserved: PluginState =
        serde_json::from_slice(&subject.state_bytes()).expect("preserved VST3 state");
    assert_state_resource_object(&preserved, &path, 0.65, true);
    let continuation = input_sequence(257);
    assert_eq!(
        subject.process(&continuation),
        control.process(&continuation),
        "rejected active VST3 state must preserve prepared DSP history"
    );

    subject.deactivate();
    let replacement_impulse = [
        [16_384, 4_096, -12_288, 8_192],
        [-4_096, 2_048, 6_144, -1_024],
        [2_048, -3_072, 5_120, 256],
    ];
    let replacement = ImpulseFile::write_true_stereo(replacement_impulse);
    let replacement_path = replacement.absolute_path();
    let replacement_state = ir_state_from(subject.state(), &replacement_path);
    assert_eq!(subject.load_state(&replacement_state), kResultOk);
    let replacement_stream = subject.state_bytes();
    let replacement_state_from_stream: PluginState =
        serde_json::from_slice(&replacement_stream).expect("replacement VST3 state JSON");
    assert_state_resource_object(
        &replacement_state_from_stream,
        &replacement_path,
        0.65,
        true,
    );

    let mut fresh_replacement = NativeVst3::new();
    assert_eq!(fresh_replacement.load_bytes(&replacement_stream), kResultOk);
    subject.activate();
    fresh_replacement.activate();
    // SAFETY: the processor is active and reports the prepared tail for this restored IR.
    let replacement_tail = unsafe { subject.processor.get_tail_samples() as usize };
    assert_eq!(replacement_tail, latency + replacement_impulse.len() - 1);
    // SAFETY: same active sample-rate/configuration on the fresh instance.
    assert_eq!(
        unsafe { fresh_replacement.processor.get_tail_samples() as usize },
        replacement_tail
    );
    let source = input_sequence(1_307);
    let subject_output = render_complete_vst3(&subject, &source, replacement_tail);
    let fresh_output = render_complete_vst3(&fresh_replacement, &source, replacement_tail);
    assert_eq!(subject_output, fresh_output);
    assert_waveform_matches_direct(&subject_output, &source, &replacement_impulse, latency);

    let previous_ir_output = direct_true_stereo(&source, &impulse, 0.65);
    assert!(
        previous_ir_output
            .iter()
            .enumerate()
            .any(|(frame, expected)| {
                let actual = subject_output[latency + frame];
                (f64::from(actual[0]) - expected[0]).abs() > 1.0e-3
                    || (f64::from(actual[1]) - expected[1]).abs() > 1.0e-3
            })
    );

    replacement.remove();
    let deleted_resource = NativeVst3::new();
    assert_eq!(
        deleted_resource.load_bytes(&replacement_stream),
        kResultFalse,
        "a saved external resource that was deleted must refuse VST3 restore"
    );

    subject.deactivate();
    let mut clear_state = subject.state();
    clear_state
        .params
        .insert("true_stereo".into(), ParamValue::Bool(false));
    clear_state.fields.insert(
        IR_RESOURCE_FIELD.into(),
        serde_json::json!({"version": 1, "path": ""}).to_string(),
    );
    assert_eq!(subject.load_state(&clear_state), kResultOk);
    let cleared_state: PluginState =
        serde_json::from_slice(&subject.state_bytes()).expect("cleared VST3 state");
    assert_state_resource_object(&cleared_state, "", 0.65, false);
    assert!(matches!(
        cleared_state.params.get("true_stereo"),
        Some(ParamValue::Bool(false))
    ));
    subject.activate();
    // SAFETY: the active processor reports the dry path's latency-aligned tail.
    let dry_tail = unsafe { subject.processor.get_tail_samples() as usize };
    assert_eq!(dry_tail, latency);
    let dry_source = input_sequence(1_307);
    let dry_output = render_complete_vst3(&subject, &dry_source, dry_tail);
    assert_delayed_dry(&dry_output, &dry_source, latency);
    subject.deactivate();

    let mut legacy = NativeVst3::new();
    let mut legacy_state = legacy.state();
    legacy_state.fields.remove(IR_RESOURCE_FIELD);
    assert_eq!(legacy.load_state(&legacy_state), kResultOk);
    legacy.activate();
    // SAFETY: the active legacy-restored processor reports its dry tail.
    let legacy_tail = unsafe { legacy.processor.get_tail_samples() as usize };
    assert_eq!(legacy_tail, latency);
    let legacy_output = render_complete_vst3(&legacy, &dry_source, legacy_tail);
    assert_delayed_dry(&legacy_output, &dry_source, latency);
    legacy.deactivate();

    let pending_file = ImpulseFile::write_true_stereo(impulse);
    let mut pending = NativeVst3::new();
    let pending_state = ir_state_from(pending.state(), &pending_file.absolute_path());
    assert_eq!(pending.load_state(&pending_state), kResultOk);
    let staged_bytes = pending.state_bytes();
    let staged_state: PluginState =
        serde_json::from_slice(&staged_bytes).expect("staged VST3 state");
    assert_state_resource_object(&staged_state, &pending_file.absolute_path(), 0.65, true);
    pending_file.remove();
    pending.setup();
    assert!(
        !pending.try_activate(),
        "VST3 activation must fail if the staged file disappears before initialization"
    );
    let rolled_back: PluginState =
        serde_json::from_slice(&pending.state_bytes()).expect("rolled-back VST3 state");
    assert_state_resource_object(&rolled_back, "", 1.0, false);
    assert!(matches!(
        rolled_back.params.get("true_stereo"),
        Some(ParamValue::Bool(false))
    ));
    pending.activate();
    let retried: PluginState =
        serde_json::from_slice(&pending.state_bytes()).expect("retried VST3 state");
    assert_state_resource_object(&retried, "", 1.0, false);
    assert!(matches!(
        retried.params.get("true_stereo"),
        Some(ParamValue::Bool(false))
    ));
    let retry_output = render_complete_vst3(&pending, &dry_source, legacy_tail);
    assert_delayed_dry(&retry_output, &dry_source, latency);

    impulse_file.remove();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires X11/Xvfb; run with the native GUI test helper"]
fn clap_embedded_convolution_editor_requests_host_restart_and_loads_selected_ir() {
    let files = EditorImpulseFiles::new();
    let parent = X11HostWindow::new();
    let mut subject = NativeClap::new();
    let mut old_control = NativeClap::new();

    let mut old_state = ir_state_from(subject.new_state(), &files.old_path());
    old_state
        .params
        .insert("true_stereo".into(), ParamValue::Bool(false));
    assert!(subject.load_state(&old_state));
    assert!(old_control.load_state(&old_state));
    subject.activate();
    old_control.activate();

    // Begin from the same populated two-channel IR graph so old audio can be compared while the
    // host intentionally defers the editor's restart request.
    let warmup = input_sequence(2_048);
    assert_eq!(subject.process(&warmup), old_control.process(&warmup));

    let gui = subject.open_gui(&parent);
    // Select the true-stereo option and open the temporary folder shown in the browser.
    parent.click(15, 62);
    parent.click(120, 234);
    thread::sleep(Duration::from_millis(250));
    parent.capture_if_requested("SOTF_NATIVE_GUI_SCREENSHOT_FOLDER");
    // The fixture folder has a-old.wav first and z-new.wav second. The browser uses a compact
    // selectable-label hit target around the filename, so click inside the short filename itself.
    parent.click(45, 151);
    assert!(subject.hide_gui(gui), "CLAP embedded editor hide callback");
    thread::sleep(Duration::from_millis(150));
    assert!(
        subject.show_gui(gui),
        "CLAP embedded editor reshow callback"
    );
    thread::sleep(Duration::from_millis(300));
    parent.capture_if_requested("SOTF_NATIVE_GUI_SCREENSHOT_SELECTED");
    parent.click(205, 171);
    wait_for_clap_restart(&subject, 1);
    parent.capture_if_requested("SOTF_NATIVE_GUI_SCREENSHOT_LOADED");

    let new_path = files.new_path();
    assert_state_resource(&subject.save_state(), &new_path, 0.65);
    assert!(matches!(
        decoded_state(&subject.save_state())
            .params
            .get("true_stereo"),
        Some(ParamValue::Bool(true))
    ));

    // CLAP's request_restart callback has no response. The host deliberately leaves the
    // component active to model a deferred/ignored request; the prepared old graph must continue
    // producing the same samples as its twin.
    let deferred_input = input_sequence(1_536);
    assert_eq!(
        subject.process_with_mix_event(&deferred_input, 0.37),
        old_control.process_with_mix_event(&deferred_input, 0.37),
        "the staged IR must not replace the active DSP before host reactivation"
    );
    assert_state_resource(&subject.save_state(), &new_path, 0.37);

    // The editor exposes an explicit retry action. Service the resulting native host callback,
    // then perform the CLAP stop/deactivate/activate sequence the host is expected to use.
    parent.click(410, 171);
    wait_for_clap_restart(&subject, 2);
    subject.deactivate();
    old_control.deactivate();
    subject.activate();
    old_control.activate();

    let source = input_sequence(2_053);
    let new_output = render_complete(&mut subject, &source);
    let old_output = render_complete(&mut old_control, &source);
    assert_waveform_matches_direct_with_mix(
        &new_output,
        &source,
        &files.new_impulse,
        NORMAL_LATENCY,
        0.37,
    );
    let square_error: f64 = new_output
        .iter()
        .zip(&old_output)
        .flat_map(|(new, old)| {
            new.iter()
                .zip(old)
                .map(|(new, old)| f64::from(*new - *old).powi(2))
        })
        .sum();
    let rms_difference = (square_error / (new_output.len() * 2) as f64).sqrt();
    assert!(
        rms_difference > 1.0e-3,
        "selected true-stereo IR did not change the prepared output: RMS {rms_difference}"
    );
    assert_waveform_matches_stereo_direct(
        &old_output,
        &source,
        &files.old_impulse,
        NORMAL_LATENCY,
        0.37,
    );
    subject.close_gui(gui);
    subject.deactivate();
    old_control.deactivate();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires X11/Xvfb; run with the native GUI test helper"]
fn vst3_embedded_convolution_editor_defers_retries_and_reloads_selected_ir() {
    let files = EditorImpulseFiles::new();
    let parent = X11HostWindow::new();
    let (host, host_state) = NativeVst3Host::new([]);
    let mut subject = NativeVst3::new();
    let mut old_control = NativeVst3::new();

    let mut old_state = ir_state_from(subject.state(), &files.old_path());
    old_state
        .params
        .insert("true_stereo".into(), ParamValue::Bool(false));
    assert_eq!(subject.load_state(&old_state), kResultOk);
    assert_eq!(old_control.load_state(&old_state), kResultOk);
    subject.set_component_handler(&host);
    subject.activate();
    old_control.activate();

    let warmup = input_sequence(2_048);
    assert_eq!(subject.process(&warmup), old_control.process(&warmup));
    let editor = subject.open_editor(&host, &parent);
    assert_eq!(host_state.register_calls.load(Ordering::Acquire), 1);
    // VST3 can send generic parameter/latency notifications while its host attaches a component
    // and view. Record them separately; they are not the reload request caused by IR selection.
    host_state.pump_for(Duration::from_millis(150));
    let startup_flags: Vec<_> = host_state
        .restart_calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| call.flags)
        .collect();
    eprintln!("VST3 startup restart flags before IR selection: {startup_flags:?}");
    let startup_reloads = host_state.reload_call_count();
    host.queue_restart_results([kResultFalse, kResultOk]);

    // Drive the exported editor itself: enable four-path routing, browse to the fixture folder,
    // select z-new.wav, and ask the host to load it. The response to the first actual
    // IComponentHandler::restartComponent(kReloadComponent) is deliberately refused.
    parent.click(15, 62);
    parent.click(250, 92);
    parent.type_ascii(&files.directory.display().to_string());
    parent.click(505, 92);
    thread::sleep(Duration::from_millis(250));
    parent.capture_if_requested("SOTF_NATIVE_VST3_GUI_SCREENSHOT_FOLDER");
    parent.click(45, 151);
    thread::sleep(Duration::from_millis(250));
    parent.capture_if_requested("SOTF_NATIVE_VST3_GUI_SCREENSHOT_SELECTED");
    parent.click(205, 171);
    wait_for_vst3_restart(&host_state, startup_reloads + 1, kResultFalse);
    let new_path = files.new_path();
    assert_state_resource_object(&subject.state(), &new_path, 0.65, true);
    parent.capture_if_requested("SOTF_NATIVE_VST3_GUI_SCREENSHOT_REFUSED");

    // A processing VST3 host delivers automation through ProcessData's IParameterChanges. The
    // controller setter is intentionally not used here: NIH-Plug ignores it while processing,
    // because the host is expected to supply this event queue on the audio call.
    let deferred_input = input_sequence(1_536);
    let subject_deferred = subject.process_with_mix_event(&deferred_input, 0.37);
    let control_deferred = old_control.process_with_mix_event(&deferred_input, 0.37);
    assert_state_resource_object(&subject.state(), &new_path, 0.37, true);
    assert_eq!(
        subject_deferred, control_deferred,
        "a refused VST3 reload must keep rendering the old prepared IR"
    );

    // Retry through the editor's action and service the second request on the registered host
    // run loop. The callback must be delivered on the same thread that attached IPlugView.
    parent.click(410, 171);
    wait_for_vst3_restart(&host_state, startup_reloads + 2, kResultOk);
    parent.capture_if_requested("SOTF_NATIVE_VST3_GUI_SCREENSHOT_RETRY_ACCEPTED");
    assert_eq!(
        host_state.registered_handlers.lock().unwrap().len(),
        1,
        "the embedded view must retain its host run-loop registration until removed"
    );

    // kReloadComponent asks the host to unload/recreate the controller/component. Preserve the
    // exact accepted state stream, remove the view through IPlugView::removed/set_frame(null),
    // then restore that stream into a fresh wrapper before reactivation.
    let saved = subject.state_bytes();
    drop(editor);
    assert!(host_state.registered_handlers.lock().unwrap().is_empty());
    assert_eq!(
        host_state.unregister_calls.load(Ordering::Acquire),
        host_state.register_calls.load(Ordering::Acquire)
    );
    subject.deactivate();
    old_control.deactivate();
    drop(subject);

    let mut reloaded = NativeVst3::new();
    assert_eq!(reloaded.load_bytes(&saved), kResultOk);
    assert_state_resource_object(&reloaded.state(), &new_path, 0.37, true);
    reloaded.activate();
    old_control.activate();

    // Compare the complete native render, including the declared tail, with the independent
    // LL/LR/RL/RR convolution oracle. The same mix and selected resource survive the host reload.
    // SAFETY: the active components report their latencies and tails through the VST3 processor.
    let latency = unsafe { reloaded.processor.get_latency_samples() as usize };
    // SAFETY: the active component reports the tail for the restored four-path resource.
    let tail = unsafe { reloaded.processor.get_tail_samples() as usize };
    assert_eq!(latency, NORMAL_LATENCY);
    assert_eq!(tail, latency + files.new_impulse.len() - 1);
    let source = input_sequence(2_053);
    let output = render_complete_vst3(&reloaded, &source, tail);
    // The old-IR control receives the same genuine VST3 process automation and is rendered to its
    // own declared tail, so the direct oracle checks its mix/path independently of the new graph.
    // SAFETY: the reactivated control component reports its latency and tail through VST3.
    let old_latency = unsafe { old_control.processor.get_latency_samples() as usize };
    // SAFETY: the active control component reports its own old-IR tail.
    let old_tail = unsafe { old_control.processor.get_tail_samples() as usize };
    assert_eq!(old_latency, NORMAL_LATENCY);
    assert_eq!(old_tail, old_latency + files.old_impulse.len() - 1);
    let old_output = render_complete_vst3(&old_control, &source, old_tail);
    assert_waveform_matches_direct_with_mix(&output, &source, &files.new_impulse, latency, 0.37);
    assert_waveform_matches_stereo_direct(
        &old_output,
        &source,
        &files.old_impulse,
        old_latency,
        0.37,
    );
    let square_error: f64 = output
        .iter()
        .zip(&old_output)
        .flat_map(|(new, old)| {
            new.iter()
                .zip(old)
                .map(|(new, old)| f64::from(*new - *old).powi(2))
        })
        .sum();
    let rms_difference = (square_error / (output.len() * 2) as f64).sqrt();
    assert!(
        rms_difference > 1.0e-3,
        "VST3 reloaded IR did not change output: RMS {rms_difference}"
    );
    old_control.deactivate();
    reloaded.deactivate();
}
