//! Exercise surround negotiation and audio through the native plugin callbacks.

use super::*;
use crate::wrapper::native_upmixer_layout::{CLAP_LAYOUTS, SpeakerLayout, clap_channel_map, pair};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_input_events,
};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_plugin_audio_ports_config,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use clap_sys::ext::surround::{CLAP_EXT_SURROUND, clap_plugin_surround};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use nih_plug::wrapper::vst3::vst3_sys as vst3_com;
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use std::ffi::{CStr, c_char, c_void};
#[cfg(target_os = "linux")]
use vst3_com::base::IBStream;
use vst3_com::base::{kInvalidArgument, kResultFalse, kResultOk, tresult};
use vst3_com::utils::{StaticVstPtr, VstPtr};
#[cfg(target_os = "linux")]
use vst3_com::vst::IComponentHandler;
use vst3_com::vst::{IParamValueQueue, IParameterChanges};
use vst3_com::{ComInterface, VST3};

#[VST3(implements(IParamValueQueue))]
struct VstScalarQueue {
    id: u32,
    normalized: f64,
}

impl IParamValueQueue for VstScalarQueue {
    unsafe fn get_parameter_id(&self) -> u32 {
        self.id
    }

    unsafe fn get_point_count(&self) -> i32 {
        1
    }

    unsafe fn get_point(&self, index: i32, offset: *mut i32, value: *mut f64) -> tresult {
        if index != 0 || offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the synchronous host callback supplies valid result pointers.
        unsafe {
            *offset = 0;
            *value = self.normalized;
        }
        kResultOk
    }

    unsafe fn add_point(&self, _: i32, _: f64, _: *mut i32) -> tresult {
        kResultFalse
    }
}

#[VST3(implements(IParameterChanges))]
struct VstScalarChanges {
    queue: VstPtr<dyn IParamValueQueue>,
}

impl IParameterChanges for VstScalarChanges {
    unsafe fn get_parameter_count(&self) -> i32 {
        1
    }

    unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
        // SAFETY: the queue remains owned until after the synchronous process call.
        unsafe {
            if index == 0 {
                std::mem::transmute::<
                    *mut *mut <dyn IParamValueQueue as ComInterface>::VTable,
                    StaticVstPtr<dyn IParamValueQueue>,
                >(self.queue.as_ptr())
            } else {
                std::mem::zeroed()
            }
        }
    }

    unsafe fn add_parameter_data(
        &self,
        _: *const u32,
        _: *mut i32,
    ) -> StaticVstPtr<dyn IParamValueQueue> {
        // SAFETY: a null raw COM pointer represents an absent queue.
        unsafe { std::mem::zeroed() }
    }
}

fn vst_scalar_changes(id: u32, value: f64) -> VstPtr<dyn IParameterChanges> {
    let queue = VstScalarQueue::allocate(id, value);
    // SAFETY: each boxed COM object transfers its single owned interface once.
    let queue =
        unsafe { VstPtr::<dyn IParamValueQueue>::owned(Box::into_raw(queue).cast()).unwrap() };
    let changes = VstScalarChanges::allocate(queue);
    // SAFETY: the changes object owns its queue for the duration of processing.
    unsafe { VstPtr::<dyn IParameterChanges>::owned(Box::into_raw(changes).cast()).unwrap() }
}

#[cfg(target_os = "linux")]
#[VST3(implements(IComponentHandler))]
struct LinuxThreadCheckHandler {
    main_thread: std::thread::ThreadId,
    notifications: std::sync::mpsc::Sender<(i32, bool)>,
    deactivate_component: std::sync::Arc<std::sync::atomic::AtomicPtr<c_void>>,
    deactivated: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reject_next_refresh: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(target_os = "linux")]
impl IComponentHandler for LinuxThreadCheckHandler {
    unsafe fn begin_edit(&self, _: u32) -> tresult {
        kResultOk
    }
    unsafe fn perform_edit(&self, _: u32, _: f64) -> tresult {
        kResultOk
    }
    unsafe fn end_edit(&self, _: u32) -> tresult {
        kResultOk
    }
    unsafe fn restart_component(&self, flags: i32) -> tresult {
        use std::sync::atomic::Ordering;
        use vst3_com::vst::{IComponent, RestartFlags};

        let refresh = flags & RestartFlags::kParamValuesChanged as i32 != 0;
        if refresh {
            let address = self
                .deactivate_component
                .swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !address.is_null() {
                // SAFETY: the test keeps this component alive throughout the callback.
                let component =
                    unsafe { VstPtr::<dyn IComponent>::shared(address.cast()).unwrap() };
                // SAFETY: VST3 permits the host to enter the component from its UI callback.
                self.deactivated.store(
                    unsafe { component.set_active(0) } == kResultOk,
                    Ordering::Release,
                );
            }
        }
        let _ = self
            .notifications
            .send((flags, std::thread::current().id() == self.main_thread));
        if refresh && self.reject_next_refresh.swap(false, Ordering::AcqRel) {
            kResultFalse
        } else {
            kResultOk
        }
    }
}

#[cfg(target_os = "linux")]
#[VST3(implements(IBStream))]
struct LinuxStateStream {
    bytes: Vec<u8>,
    position: std::cell::Cell<usize>,
}

#[cfg(target_os = "linux")]
impl IBStream for LinuxStateStream {
    unsafe fn read(&self, out: *mut c_void, size: i32, read: *mut i32) -> tresult {
        let Ok(size) = usize::try_from(size) else {
            return kInvalidArgument;
        };
        if size > 0 && out.is_null() {
            return kInvalidArgument;
        }
        let position = self.position.get();
        let count = size.min(self.bytes.len().saturating_sub(position));
        // SAFETY: the destination has at least size bytes and the source range is in bounds.
        unsafe {
            if count > 0 {
                std::ptr::copy_nonoverlapping(
                    self.bytes.as_ptr().add(position),
                    out.cast::<u8>(),
                    count,
                );
            }
            if !read.is_null() {
                *read = count as i32;
            }
        }
        self.position.set(position + count);
        kResultOk
    }

    unsafe fn write(&self, _: *const c_void, _: i32, _: *mut i32) -> tresult {
        kResultFalse
    }

    unsafe fn seek(&self, offset: i64, mode: i32, result: *mut i64) -> tresult {
        use vst3_com::base::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
        let base = match mode {
            value if value == kIBSeekSet => 0_i128,
            value if value == kIBSeekCur => self.position.get() as i128,
            value if value == kIBSeekEnd => self.bytes.len() as i128,
            _ => return kInvalidArgument,
        };
        let position = base + i128::from(offset);
        if position < 0 || position > self.bytes.len() as i128 {
            return kInvalidArgument;
        }
        self.position.set(position as usize);
        // SAFETY: the optional result pointer is supplied by the COM caller.
        if !result.is_null() {
            unsafe {
                *result = position as i64;
            }
        }
        kResultOk
    }

    unsafe fn tell(&self, result: *mut i64) -> tresult {
        if result.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a non-null COM result pointer references writable i64 storage.
        unsafe {
            *result = self.position.get() as i64;
        }
        kResultOk
    }
}

#[cfg(target_os = "linux")]
#[test]
fn vst3_linux_staged_state_refresh_runs_on_host_main_thread() {
    linux_staged_state_refresh(false, false, false);
}

#[cfg(target_os = "linux")]
#[test]
fn vst3_linux_staged_state_refresh_survives_late_handler() {
    linux_staged_state_refresh(true, false, false);
}

#[cfg(target_os = "linux")]
#[test]
fn vst3_linux_staged_state_refresh_allows_reentrant_deactivation() {
    linux_staged_state_refresh(false, true, false);
}

#[cfg(target_os = "linux")]
#[test]
fn vst3_linux_staged_state_refresh_retries_after_host_refusal() {
    linux_staged_state_refresh(false, false, true);
}

#[cfg(target_os = "linux")]
fn linux_staged_state_refresh(attach_handler_late: bool, reenter: bool, reject_refresh: bool) {
    use nih_plug::wrapper::state::{ParamValue, PluginState};
    use nih_plug::wrapper::vst3::Wrapper;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use vst3_com::base::{IPluginBase, IUnknown};
    use vst3_com::utils::SharedVstPtr;
    use vst3_com::vst::{
        IAudioProcessor, IComponent, IEditController, ProcessSetup, RestartFlags,
        SymbolicSampleSizes,
    };

    let (send, receive) = std::sync::mpsc::channel();
    let deactivate_component = std::sync::Arc::new(AtomicPtr::new(std::ptr::null_mut()));
    let deactivated = std::sync::Arc::new(AtomicBool::new(false));
    let reject_next_refresh = std::sync::Arc::new(AtomicBool::new(false));
    let handler = LinuxThreadCheckHandler::allocate(
        std::thread::current().id(),
        send,
        deactivate_component.clone(),
        deactivated.clone(),
        reject_next_refresh.clone(),
    );
    // SAFETY: the boxed handler's first COM interface is IComponentHandler.
    let handler =
        unsafe { VstPtr::<dyn IComponentHandler>::owned(Box::into_raw(handler).cast()).unwrap() };
    let wrapper = Wrapper::<UpmixerWrapper>::new();
    // SAFETY: the wrapper and handler outlive all COM references and callbacks.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        let controller = processor.cast::<dyn IEditController>().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
        let shared: SharedVstPtr<dyn IComponentHandler> = std::mem::transmute(handler.as_ptr());
        assert_eq!(controller.set_component_handler(shared), kResultOk);
        let setup = ProcessSetup {
            process_mode: 0,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            max_samples_per_block: 64,
            sample_rate: 48_000.0,
        };
        assert_eq!(processor.setup_processing(&setup), kResultOk);
        assert_eq!(component.set_active(1), kResultOk);
        let _ = receive.try_iter().count();
        if attach_handler_late {
            let absent: SharedVstPtr<dyn IComponentHandler> = std::mem::zeroed();
            assert_eq!(controller.set_component_handler(absent), kResultOk);
        }
        if reenter {
            deactivate_component.store(component.as_ptr().cast(), Ordering::Release);
        }
        reject_next_refresh.store(reject_refresh, Ordering::Release);
        let state = PluginState {
            version: String::new(),
            params: if reenter {
                [("low_latency".into(), ParamValue::Bool(true))].into()
            } else {
                [("height_gain".into(), ParamValue::F32(0.3))].into()
            },
            fields: Default::default(),
        };
        let bytes = serde_json::to_vec(&state).unwrap();
        let stream = LinuxStateStream::allocate(bytes, std::cell::Cell::new(0));
        let stream = VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap();
        let stream_ref: SharedVstPtr<dyn IBStream> = std::mem::transmute(stream.as_ptr());
        assert_eq!(component.set_state(stream_ref), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.set_active(1), kResultOk);
        if attach_handler_late {
            let shared: SharedVstPtr<dyn IComponentHandler> = std::mem::transmute(handler.as_ptr());
            assert_eq!(controller.set_component_handler(shared), kResultOk);
        }
        if reject_refresh {
            let (flags, on_main) = receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("initial VST3 parameter refresh");
            assert_ne!(flags & RestartFlags::kParamValuesChanged as i32, 0);
            assert!(on_main);
            let shared: SharedVstPtr<dyn IComponentHandler> = std::mem::transmute(handler.as_ptr());
            assert_eq!(controller.set_component_handler(shared), kResultOk);
        }
        if !reenter {
            assert_eq!(processor.get_latency_samples(), 2048);
        } else {
            assert!(deactivated.load(Ordering::Acquire));
        }
        let mut on_main = None;
        for _ in 0..8 {
            let (flags, is_main) = receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("VST3 parameter refresh notification");
            if flags & RestartFlags::kParamValuesChanged as i32 != 0 {
                on_main = Some(is_main);
                break;
            }
        }
        if !reenter {
            assert_eq!(component.set_active(0), kResultOk);
        }
        assert_eq!(component.terminate(), kResultOk);
        assert_eq!(
            on_main,
            Some(true),
            "parameter refresh must run on the host main thread"
        );
    }
}

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn no_request(_: *const clap_host) {}

unsafe extern "C" fn one_event(_: *const clap_input_events) -> u32 {
    1
}

unsafe extern "C" fn event_at(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    if index != 0 {
        return std::ptr::null();
    }
    // SAFETY: the flush call holds a live event in the list context.
    unsafe { &(*((*list).ctx.cast::<clap_event_param_value>())).header }
}

fn clap_param_id(plugin: *const clap_plugin, name: &[u8]) -> u32 {
    // SAFETY: the live plugin owns this extension and each initialized metadata record.
    unsafe {
        let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
            .cast::<clap_plugin_params>();
        assert!(!params.is_null());
        for index in 0..((*params).count.unwrap())(plugin) {
            let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
            assert!(((*params).get_info.unwrap())(
                plugin,
                index,
                info.as_mut_ptr()
            ));
            let info = info.assume_init();
            if CStr::from_ptr(info.name.as_ptr()).to_bytes() == name {
                return info.id;
            }
        }
    }
    panic!("missing CLAP parameter {name:?}");
}

fn set_clap_bool(plugin: *const clap_plugin, name: &[u8], enabled: bool) {
    let id = clap_param_id(plugin, name);
    // SAFETY: the live plugin owns this extension and the event remains live throughout flush.
    unsafe {
        let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
            .cast::<clap_plugin_params>();
        let event = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: f64::from(enabled),
        };
        let events = clap_input_events {
            ctx: (&event as *const clap_event_param_value).cast_mut().cast(),
            size: Some(one_event),
            get: Some(event_at),
        };
        ((*params).flush.unwrap())(plugin, &events, std::ptr::null());
    }
}

#[test]
fn clap_surround_layouts_negotiate_and_process_without_allocations() {
    const FRAMES: usize = 257;
    for (index, _) in CLAP_LAYOUTS.iter().enumerate().skip(1) {
        let pair = pair(index, PluginApi::Clap).unwrap();
        assert_ne!(pair.input, SpeakerLayout::Stereo);
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: std::ptr::null_mut(),
            name: c"SOTF surround callback test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_extension),
            request_restart: Some(no_request),
            request_process: Some(no_request),
            request_callback: Some(no_request),
        });
        // SAFETY: the boxed host and its callbacks live until the wrapper is dropped.
        let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<UpmixerWrapper>::new(&*host) };
        let plugin = wrapper.clap_plugin.as_ptr();
        // SAFETY: these extension and lifecycle callbacks belong to the live wrapper.
        unsafe {
            assert!(((*plugin).init.unwrap())(plugin));
            let configs =
                ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr())
                    .cast::<clap_plugin_audio_ports_config>();
            let surround = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_SURROUND.as_ptr())
                .cast::<clap_plugin_surround>();
            assert!(!configs.is_null() && !surround.is_null());
            assert_eq!(
                ((*configs).count.unwrap())(plugin) as usize,
                CLAP_LAYOUTS.len()
            );
            let mut config: clap_audio_ports_config = std::mem::zeroed();
            assert!(((*configs).get.unwrap())(plugin, index as u32, &mut config));
            assert_eq!(config.id, index as u32);
            assert_eq!(
                config.main_input_channel_count as usize,
                pair.input.channels()
            );
            assert_eq!(
                config.main_output_channel_count as usize,
                pair.output.channels()
            );
            assert!(((*configs).select.unwrap())(plugin, config.id));
            for is_input in [true, false] {
                let mut roles = [0_u8; 16];
                let count = ((*surround).get_channel_map.unwrap())(
                    plugin,
                    is_input,
                    0,
                    roles.as_mut_ptr(),
                    roles.len() as u32,
                ) as usize;
                assert_eq!(
                    &roles[..count],
                    clap_channel_map(index, is_input, 0).unwrap()
                );
            }
            set_clap_bool(plugin, b"Binaural Preview", true);
            set_clap_bool(plugin, b"Auto Gain", true);
            set_clap_bool(plugin, b"Subharmonic Synth", true);
            assert!(((*plugin).activate.unwrap())(
                plugin,
                48_000.0,
                1,
                FRAMES as u32
            ));
            assert!(((*plugin).start_processing.unwrap())(plugin));
        }

        let mut inputs: Vec<Vec<f32>> = (0..pair.input.channels())
            .map(|channel| vec![(channel + 1) as f32 / 16.0; FRAMES])
            .collect();
        let mut outputs = vec![vec![0.0_f32; FRAMES]; pair.output.channels()];
        let mut input_pointers: Vec<_> = inputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut output_pointers: Vec<_> = outputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let input_bus = clap_audio_buffer {
            data32: input_pointers.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: pair.input.channels() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_bus = clap_audio_buffer {
            data32: output_pointers.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: pair.output.channels() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut process = clap_process {
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
        for _ in 0..9 {
            // SAFETY: disjoint, prepared channel arrays and bus pointers remain live during the callback.
            let status = assert_no_alloc::assert_no_alloc(|| unsafe {
                ((*plugin).process.unwrap())(plugin, &process)
            });
            assert_ne!(status, CLAP_PROCESS_ERROR, "{pair:?}");
        }
        let input_layout =
            ChannelLayout::from_speaker_config(get_speaker_config(pair.input.id()).unwrap())
                .unwrap();
        let output_layout =
            ChannelLayout::from_speaker_config(get_speaker_config(pair.output.id()).unwrap())
                .unwrap();
        for (source, channel) in inputs.iter().enumerate() {
            let role = input_layout.role_at(source).unwrap();
            let destination = (0..pair.output.channels())
                .find(|index| output_layout.role_at(*index) == Some(role))
                .unwrap();
            assert_eq!(
                outputs[destination][FRAMES - 1].to_bits(),
                channel[FRAMES - 1].to_bits(),
                "{pair:?} {role:?}"
            );
        }
        assert!(outputs.iter().flatten().all(|sample| sample.is_finite()));
        for name in [
            b"Binaural Preview".as_slice(),
            b"Auto Gain".as_slice(),
            b"Subharmonic Synth".as_slice(),
        ] {
            for enabled in [false, true] {
                set_clap_bool(plugin, name, enabled);
                // SAFETY: the prepared disjoint audio buses remain live during the callback.
                let status = assert_no_alloc::assert_no_alloc(|| unsafe {
                    ((*plugin).process.unwrap())(plugin, &process)
                });
                assert_ne!(status, CLAP_PROCESS_ERROR, "{pair:?} dormant {name:?}");
            }
        }
        if pair.input == SpeakerLayout::FiveZero && pair.output == SpeakerLayout::FiveOneFour {
            let event = clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: clap_param_id(plugin, b"Bass Mode"),
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: 1.0,
            };
            let events = clap_input_events {
                ctx: (&event as *const clap_event_param_value).cast_mut().cast(),
                size: Some(one_event),
                get: Some(event_at),
            };
            process.in_events = &events;
            // SAFETY: the event and prepared audio buses remain live throughout processing.
            let status = assert_no_alloc::assert_no_alloc(|| unsafe {
                ((*plugin).process.unwrap())(plugin, &process)
            });
            assert_ne!(status, CLAP_PROCESS_ERROR, "{pair:?} pending bass restart");
            process.in_events = std::ptr::null();
            // SAFETY: processing continues with the old prepared DSP until host reactivation.
            let status = assert_no_alloc::assert_no_alloc(|| unsafe {
                ((*plugin).process.unwrap())(plugin, &process)
            });
            assert_ne!(status, CLAP_PROCESS_ERROR, "{pair:?} before reactivation");
        }
        set_clap_bool(plugin, b"Bypass Surround", true);
        for _ in 0..32 {
            // SAFETY: the prepared disjoint audio buses remain live during each callback.
            let status = assert_no_alloc::assert_no_alloc(|| unsafe {
                ((*plugin).process.unwrap())(plugin, &process)
            });
            assert_ne!(status, CLAP_PROCESS_ERROR, "{pair:?} bypass");
        }
        for (channel, samples) in outputs.iter().enumerate() {
            let role = output_layout.role_at(channel).unwrap();
            let expected = (0..pair.input.channels())
                .find(|source| input_layout.role_at(*source) == Some(role))
                .map_or(0.0, |source| inputs[source][FRAMES - 1]);
            assert!(
                (samples[FRAMES - 1] - expected).abs() < 1e-7,
                "{pair:?} {role:?}"
            );
        }
        // SAFETY: stop and deactivate in lifecycle order after the last callback.
        unsafe {
            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).deactivate.unwrap())(plugin);
        }
    }
}

#[test]
fn clap_surround_96k_reports_latency_and_switches_pending_fft_without_allocations() {
    const FRAMES: usize = 64;
    let index = (0..CLAP_LAYOUTS.len())
        .find(|&index| {
            let layout = pair(index, PluginApi::Clap).unwrap();
            layout.input == SpeakerLayout::FiveZero && layout.output == SpeakerLayout::FiveOneFour
        })
        .unwrap();
    let layout = pair(index, PluginApi::Clap).unwrap();
    let host = Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF scheduled surround callback test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_request),
        request_process: Some(no_request),
        request_callback: Some(no_request),
    });
    // SAFETY: the boxed host and its callbacks outlive the native wrapper.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<UpmixerWrapper>::new(&*host) };
    let plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: these synchronous callbacks belong to the live wrapper.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let configs =
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr())
                .cast::<clap_plugin_audio_ports_config>();
        assert!(!configs.is_null());
        assert!(((*configs).select.unwrap())(plugin, index as u32));
        assert!(((*plugin).activate.unwrap())(
            plugin,
            96_000.0,
            1,
            FRAMES as u32
        ));
        let latency = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_LATENCY.as_ptr())
            .cast::<clap_plugin_latency>();
        assert!(!latency.is_null());
        assert_eq!(((*latency).get.unwrap())(plugin), 2176);
        assert!(((*plugin).start_processing.unwrap())(plugin));
    }
    let mut inputs: Vec<Vec<f32>> = (0..layout.input.channels())
        .map(|channel| vec![(channel + 1) as f32 / 16.0; FRAMES])
        .collect();
    let mut outputs = vec![vec![0.0_f32; FRAMES]; layout.output.channels()];
    let mut input_pointers: Vec<_> = inputs.iter_mut().map(Vec::as_mut_ptr).collect();
    let mut output_pointers: Vec<_> = outputs.iter_mut().map(Vec::as_mut_ptr).collect();
    let input_bus = clap_audio_buffer {
        data32: input_pointers.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: layout.input.channels() as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut output_bus = clap_audio_buffer {
        data32: output_pointers.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: layout.output.channels() as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut process = clap_process {
        steady_time: -1,
        frames_count: FRAMES as u32,
        transport: std::ptr::null(),
        audio_inputs: &input_bus,
        audio_outputs: &mut output_bus,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events: std::ptr::null(),
        out_events: std::ptr::null_mut(),
    };
    for _ in 0..17 {
        // SAFETY: all channel arrays and bus pointers are live and disjoint.
        let status = assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        });
        assert_ne!(status, CLAP_PROCESS_ERROR);
    }
    let event = clap_event_param_value {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_value>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: clap_param_id(plugin, b"Height Gain"),
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value: 0.25,
    };
    let events = clap_input_events {
        ctx: (&event as *const clap_event_param_value).cast_mut().cast(),
        size: Some(one_event),
        get: Some(event_at),
    };
    process.frames_count = 1;
    process.in_events = &events;
    // SAFETY: the one-frame buses and parameter event remain live during the callback.
    let status = assert_no_alloc::assert_no_alloc(|| unsafe {
        ((*plugin).process.unwrap())(plugin, &process)
    });
    assert_ne!(status, CLAP_PROCESS_ERROR);
    process.frames_count = FRAMES as u32;
    process.in_events = std::ptr::null();
    for _ in 0..64 {
        // SAFETY: the prepared audio buses remain live throughout processing.
        let status = assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        });
        assert_ne!(status, CLAP_PROCESS_ERROR);
    }
    let input_layout =
        ChannelLayout::from_speaker_config(get_speaker_config(layout.input.id()).unwrap()).unwrap();
    let output_layout =
        ChannelLayout::from_speaker_config(get_speaker_config(layout.output.id()).unwrap())
            .unwrap();
    for (source, channel) in inputs.iter().enumerate() {
        let role = input_layout.role_at(source).unwrap();
        let destination = (0..layout.output.channels())
            .find(|&target| output_layout.role_at(target) == Some(role))
            .unwrap();
        assert_eq!(
            outputs[destination][FRAMES - 1].to_bits(),
            channel[FRAMES - 1].to_bits(),
            "{role:?}"
        );
    }
    assert!(outputs.iter().flatten().all(|sample| sample.is_finite()));
    // SAFETY: deactivate in lifecycle order after the final callback.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
    }
}

#[test]
fn vst3_surround_layouts_negotiate_and_process_without_allocations() {
    check_vst3_surround_layouts::<257>(48_000.0, 2048, 32);
}

#[test]
fn vst3_surround_layouts_at_96k_schedule_without_allocations() {
    check_vst3_surround_layouts::<64>(96_000.0, 2176, 160);
}

fn check_vst3_surround_layouts<const FRAMES: usize>(
    sample_rate: f64,
    expected_height_latency: usize,
    bypass_blocks: usize,
) {
    use crate::wrapper::native_upmixer_layout::VST3_LAYOUTS;
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::ComInterface;
    use vst3_sys::base::{IPluginBase, IUnknown, kResultOk};
    use vst3_sys::utils::VstPtr;
    use vst3_sys::vst::{
        AudioBusBuffers, BusDirections, IAudioProcessor, IComponent, IEditController, MediaTypes,
        ParameterInfo, ProcessData, ProcessSetup, SymbolicSampleSizes,
    };

    for (index, _) in VST3_LAYOUTS.iter().enumerate().skip(1) {
        let pair = pair(index, PluginApi::Vst3).unwrap();
        let wrapper = Wrapper::<UpmixerWrapper>::new();
        // SAFETY: the wrapper outlives the owned COM references and every synchronous call.
        let (processor, component, controller, bypass_id, bass_mode_id, dormant_ids) = unsafe {
            let mut pointer = std::ptr::null_mut();
            assert_eq!(
                wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
                kResultOk
            );
            let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
            let component = processor.cast::<dyn IComponent>().unwrap();
            assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
            let controller = processor.cast::<dyn IEditController>().unwrap();
            let mut ids = [None; 5];
            let names = [
                "Bypass Surround",
                "Binaural Preview",
                "Auto Gain",
                "Subharmonic Synth",
                "Bass Mode",
            ];
            for parameter in 0..controller.get_parameter_count() {
                let mut info = std::mem::zeroed::<ParameterInfo>();
                assert_eq!(
                    controller.get_parameter_info(parameter, &mut info),
                    kResultOk
                );
                let length = info
                    .title
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(info.title.len());
                for (slot, name) in ids.iter_mut().zip(names) {
                    if info.title[..length]
                        .iter()
                        .copied()
                        .eq(name.encode_utf16().map(|value| value as i16))
                    {
                        *slot = Some(info.id);
                    }
                }
            }
            let [
                bypass_id,
                preview_id,
                auto_gain_id,
                subharmonic_id,
                bass_mode_id,
            ] = ids.map(|id| id.expect("missing VST3 upmixer control"));
            for id in [preview_id, auto_gain_id, subharmonic_id] {
                assert_eq!(controller.set_param_normalized(id, 1.0), kResultOk);
            }
            let mut input_arrangement = pair.input.vst3_arrangement();
            let mut output_arrangement = pair.output.vst3_arrangement();
            assert_eq!(
                processor.set_bus_arrangements(
                    &mut input_arrangement,
                    1,
                    &mut output_arrangement,
                    1
                ),
                kResultOk,
                "{pair:?}"
            );
            for (direction, expected) in [
                (BusDirections::kInput as i32, input_arrangement),
                (BusDirections::kOutput as i32, output_arrangement),
            ] {
                assert_eq!(
                    component.activate_bus(MediaTypes::kAudio as i32, direction, 0, 1),
                    kResultOk
                );
                let mut actual = 0;
                assert_eq!(
                    processor.get_bus_arrangement(direction, 0, &mut actual),
                    kResultOk
                );
                assert_eq!(actual, expected);
            }
            let setup = ProcessSetup {
                process_mode: 0,
                symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
                max_samples_per_block: FRAMES as i32,
                sample_rate,
            };
            assert_eq!(processor.setup_processing(&setup), kResultOk);
            assert_eq!(component.set_active(1), kResultOk);
            assert_eq!(processor.set_processing(1), kResultOk);
            (
                processor,
                component,
                controller,
                bypass_id,
                bass_mode_id,
                [preview_id, auto_gain_id, subharmonic_id],
            )
        };
        // SAFETY: the initialized processor owns a live latency callback.
        let latency = unsafe { processor.get_latency_samples() as usize };
        let has_heights = get_speaker_config(pair.output.id())
            .unwrap()
            .speakers
            .iter()
            .any(|speaker| speaker.elevation > 10.0);
        assert_eq!(
            latency,
            if has_heights {
                expected_height_latency
            } else {
                0
            },
            "{pair:?}"
        );
        let mut inputs: Vec<Vec<f32>> = (0..pair.input.channels())
            .map(|channel| {
                let mut state = 0x9e37_79b9_u32 ^ channel as u32;
                (0..FRAMES)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        ((state >> 8) as f32 / 16_777_216.0 - 0.5) * 0.2
                    })
                    .collect()
            })
            .collect();
        let mut outputs = vec![vec![0.0_f32; FRAMES]; pair.output.channels()];
        let mut input_pointers: Vec<*mut c_void> =
            inputs.iter_mut().map(|v| v.as_mut_ptr().cast()).collect();
        let mut output_pointers: Vec<*mut c_void> =
            outputs.iter_mut().map(|v| v.as_mut_ptr().cast()).collect();
        let mut input_bus = AudioBusBuffers {
            num_channels: pair.input.channels() as i32,
            silence_flags: 0,
            buffers: input_pointers.as_mut_ptr(),
        };
        let mut output_bus = AudioBusBuffers {
            num_channels: pair.output.channels() as i32,
            silence_flags: 0,
            buffers: output_pointers.as_mut_ptr(),
        };
        // SAFETY: nullable VST3 event, parameter, and context fields are zero-initialized.
        let mut process = unsafe { std::mem::zeroed::<ProcessData>() };
        process.process_mode = 0;
        process.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
        process.num_samples = FRAMES as i32;
        process.num_inputs = 1;
        process.num_outputs = 1;
        process.inputs = &mut input_bus;
        process.outputs = &mut output_bus;
        for _ in 0..(latency / FRAMES + 2) {
            // SAFETY: all channel arrays and bus pointers are disjoint and outlive the callback.
            let status =
                assert_no_alloc::assert_no_alloc(|| unsafe { processor.process(&mut process) });
            assert_eq!(status, kResultOk, "{pair:?}");
        }
        let input_layout =
            ChannelLayout::from_speaker_config(get_speaker_config(pair.input.id()).unwrap())
                .unwrap();
        let output_layout =
            ChannelLayout::from_speaker_config(get_speaker_config(pair.output.id()).unwrap())
                .unwrap();
        for (source, samples) in inputs.iter().enumerate() {
            let role = input_layout
                .role_at(pair.input.host_to_sotf(PluginApi::Vst3, source).unwrap())
                .unwrap();
            let destination = (0..pair.output.channels())
                .find(|channel| {
                    output_layout
                        .role_at(pair.output.host_to_sotf(PluginApi::Vst3, *channel).unwrap())
                        == Some(role)
                })
                .unwrap();
            assert_eq!(
                outputs[destination][FRAMES - 1].to_bits(),
                samples[(2 * FRAMES - 1 - latency % FRAMES) % FRAMES].to_bits(),
                "{pair:?} {role:?}"
            );
        }
        assert!(outputs.iter().flatten().all(|sample| sample.is_finite()));
        let height_channels: Vec<_> = (0..pair.output.channels())
            .filter(|channel| {
                use sotf_host::speaker_config::ChannelRole::*;
                matches!(
                    output_layout
                        .role_at(pair.output.host_to_sotf(PluginApi::Vst3, *channel).unwrap()),
                    Some(
                        TopFrontLeft
                            | TopFrontRight
                            | TopMiddleLeft
                            | TopMiddleRight
                            | TopBackLeft
                            | TopBackRight
                    )
                )
            })
            .collect();
        if !height_channels.is_empty() {
            assert!(
                height_channels
                    .iter()
                    .any(|channel| outputs[*channel].iter().any(|sample| sample.abs() > 1e-8)),
                "{pair:?} produced no height signal before bypass"
            );
        }
        for id in dormant_ids {
            for value in [0.0, 1.0] {
                let changes = vst_scalar_changes(id, value);
                // SAFETY: StaticVstPtr is a transparent raw pointer retained by `changes`.
                process.input_param_changes = unsafe {
                    std::mem::transmute::<
                        *mut *mut <dyn IParameterChanges as ComInterface>::VTable,
                        StaticVstPtr<dyn IParameterChanges>,
                    >(changes.as_ptr())
                };
                // SAFETY: the audio buses and parameter queue remain live during processing.
                let status =
                    assert_no_alloc::assert_no_alloc(|| unsafe { processor.process(&mut process) });
                assert_eq!(status, kResultOk, "{pair:?} dormant control {id}");
                // SAFETY: the null interface denotes no more parameter changes.
                process.input_param_changes = unsafe { std::mem::zeroed() };
            }
        }
        if pair.input == SpeakerLayout::FiveZero && pair.output == SpeakerLayout::FiveOneFour {
            let changes = vst_scalar_changes(bass_mode_id, 0.5);
            // SAFETY: the changes object owns the queue throughout the synchronous callback.
            process.input_param_changes = unsafe {
                std::mem::transmute::<
                    *mut *mut <dyn IParameterChanges as ComInterface>::VTable,
                    StaticVstPtr<dyn IParameterChanges>,
                >(changes.as_ptr())
            };
            // SAFETY: the prepared buses and parameter queue remain live during processing.
            let status =
                assert_no_alloc::assert_no_alloc(|| unsafe { processor.process(&mut process) });
            assert_eq!(status, kResultOk, "{pair:?} pending bass restart");
            // SAFETY: the null interface denotes no further parameter changes.
            process.input_param_changes = unsafe { std::mem::zeroed() };
            // SAFETY: the old prepared DSP remains active until host reactivation.
            let status =
                assert_no_alloc::assert_no_alloc(|| unsafe { processor.process(&mut process) });
            assert_eq!(status, kResultOk, "{pair:?} before reactivation");
        }
        let changes = vst_scalar_changes(bypass_id, 1.0);
        // SAFETY: StaticVstPtr is a transparent raw pointer retained by `changes`.
        process.input_param_changes = unsafe {
            std::mem::transmute::<
                *mut *mut <dyn IParameterChanges as ComInterface>::VTable,
                StaticVstPtr<dyn IParameterChanges>,
            >(changes.as_ptr())
        };
        for block in 0..bypass_blocks {
            // SAFETY: the audio buses and optional parameter queue remain live.
            let status =
                assert_no_alloc::assert_no_alloc(|| unsafe { processor.process(&mut process) });
            assert_eq!(status, kResultOk, "{pair:?} bypass");
            if block == 0 {
                // SAFETY: the null interface denotes no more parameter changes.
                process.input_param_changes = unsafe { std::mem::zeroed() };
            }
        }
        // SAFETY: the processor remains active and its reported latency must stay stable.
        assert_eq!(unsafe { processor.get_latency_samples() as usize }, latency);
        // SAFETY: the controller and ID belong to the active plugin instance.
        assert_eq!(unsafe { controller.get_param_normalized(bypass_id) }, 1.0);
        for (channel, samples) in outputs.iter().enumerate() {
            let role = output_layout
                .role_at(pair.output.host_to_sotf(PluginApi::Vst3, channel).unwrap())
                .unwrap();
            let source = (0..pair.input.channels()).find(|source| {
                input_layout.role_at(pair.input.host_to_sotf(PluginApi::Vst3, *source).unwrap())
                    == Some(role)
            });
            for (frame, sample) in samples.iter().enumerate() {
                let expected = source.map_or(0.0, |source| {
                    inputs[source][(frame + FRAMES - latency % FRAMES) % FRAMES]
                });
                assert!((*sample - expected).abs() < 1e-7, "{pair:?} {role:?}");
            }
        }
        // SAFETY: finish the active VST3 lifecycle before releasing the interfaces.
        unsafe {
            assert_eq!(processor.set_processing(0), kResultOk);
            assert_eq!(component.set_active(0), kResultOk);
            assert_eq!(component.terminate(), kResultOk);
        }
    }
}
