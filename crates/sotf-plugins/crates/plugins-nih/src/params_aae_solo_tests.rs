//! AAE solo-pair admission, cached metadata, history, and native lifecycle regressions.

use super::DynamicParams;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE, clap_event_header,
    clap_event_param_mod, clap_event_param_value, clap_input_events,
};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use nih_plug::wrapper::clap::Wrapper;
use plugins_bridge::param_bridge::ParamBridge;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugins::plugin_aae::{AaePlugin, params::AaePluginParams};
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::Arc;

const SAMPLE_RATE: u32 = 44_100;
const FRAMES: usize = 128;

fn parameters(plugin_type: &str) -> Arc<DynamicParams> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("AAE"));
    let infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();
    DynamicParams::from_infos_for_plugin(plugin_type, &infos)
}

fn write_pair(params: &DynamicParams, early: bool, late: bool) {
    for (id, value) in [("solo_early", early), ("solo_late", late)] {
        let entry = &params.param_map[id];
        params.bool_params[entry.index].set_plain_value_for_initialization(value);
    }
}

fn dsp(params: &DynamicParams) -> AaePlugin {
    let mut plugin = AaePlugin::try_from_params(AaePluginParams::default()).unwrap();
    plugin.initialize(SAMPLE_RATE as f64).unwrap();
    params.sync_to_plugin(&mut plugin).unwrap();
    plugin
}

fn assert_pair(plugin: &AaePlugin, early: bool, late: bool) {
    for (id, value) in [("solo_early", early), ("solo_late", late)] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(id)),
            Some(ParameterValue::Bool(value))
        );
        let cached = plugin
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id.as_str() == id)
            .unwrap();
        assert_eq!(cached.default_value, ParameterValue::Bool(value));
    }
}

fn render(plugin: &mut dyn Plugin, input: &[f32]) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len() / 2 * plugin.output_channels()];
    plugin
        .process(
            input,
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, input.len() / 2),
        )
        .unwrap();
    output
}

#[test]
fn aae_solo_valid_pair_transitions_update_cached_metadata_and_retries() {
    let params = parameters("AAE");
    let mut plugin = dsp(&params);
    for (early, late) in [
        (false, true),
        (true, false),
        (false, true),
        (false, false),
        (true, false),
        (false, false),
    ] {
        write_pair(&params, early, late);
        params.sync_to_plugin(&mut plugin).unwrap();
        assert_pair(&plugin, early, late);
        params.sync_to_plugin(&mut plugin).unwrap();
        assert_pair(&plugin, early, late);
        assert_eq!(
            params.value("solo_early"),
            Some(ParameterValue::Bool(early))
        );
        assert_eq!(params.value("solo_late"), Some(ParameterValue::Bool(late)));
    }
}

#[test]
fn aae_solo_invalid_final_tuple_preserves_cache_and_bit_exact_history() {
    let params = parameters("AAE");
    let mut subject = dsp(&params);
    let mut control = dsp(&params);
    let mut impulse = vec![0.0; FRAMES * 2];
    impulse[0] = 0.5;
    impulse[1] = -0.25;
    assert_eq!(
        render(&mut subject, &impulse),
        render(&mut control, &impulse)
    );
    for pair in [(false, false), (true, false), (false, true)] {
        write_pair(&params, pair.0, pair.1);
        params.sync_to_plugin(&mut subject).unwrap();
        params.sync_to_plugin(&mut control).unwrap();
        write_pair(&params, true, true);
        let error = params.sync_to_plugin(&mut subject).unwrap_err();
        assert!(error.contains("cannot both be enabled"));
        assert_pair(&subject, pair.0, pair.1);
        assert_eq!(
            render(&mut subject, &vec![0.0; FRAMES * 2]),
            render(&mut control, &vec![0.0; FRAMES * 2])
        );
    }
}

#[test]
fn aae_solo_valid_toggles_do_not_reset_reverb_history() {
    let params = parameters("AAE");
    let mut subject = dsp(&params);
    let mut control = dsp(&params);
    let mut impulse = vec![0.0; FRAMES * 2];
    impulse[0] = 0.5;
    impulse[1] = 0.25;
    assert_eq!(
        render(&mut subject, &impulse),
        render(&mut control, &impulse)
    );
    let silence = vec![0.0; FRAMES * 2];
    for _ in 0..80 {
        assert_eq!(
            render(&mut subject, &silence),
            render(&mut control, &silence)
        );
    }
    // Solo modes intentionally gate branch advancement during processing. Exercise
    // admission without intervening processing to isolate setter history preservation.
    for (early, late) in [(false, true), (true, false), (false, true), (false, false)] {
        write_pair(&params, early, late);
        params.sync_to_plugin(&mut subject).unwrap();
        assert_pair(&subject, early, late);
    }
    let actual = render(&mut subject, &silence);
    let expected = render(&mut control, &silence);
    assert!(
        expected.iter().any(|sample| sample.abs() > 1e-8),
        "history oracle must contain a retained tail"
    );
    assert_eq!(actual, expected);
}

#[test]
fn aae_solo_pair_admission_is_not_applied_to_other_plugin_schemas() {
    let params = parameters("Unrelated");
    let mut plugin = dsp(&params);
    write_pair(&params, false, true);
    params.sync_to_plugin(&mut plugin).unwrap();
    write_pair(&params, true, false);
    assert!(
        params.sync_to_plugin(&mut plugin).is_err(),
        "ordinary declaration-order setter behavior must remain unchanged"
    );
}

unsafe extern "C" fn no_extension(_host: *const clap_host, _id: *const c_char) -> *const c_void {
    ptr::null()
}
unsafe extern "C" fn no_request(_host: *const clap_host) {}

enum NativeParamEvent {
    Value(clap_event_param_value),
    Modulation(clap_event_param_mod),
}

struct NativeAae {
    // The wrapper is dropped before its boxed raw host owner.
    wrapper: Arc<Wrapper<crate::plugin::SotfAAE>>,
    _host: Box<clap_host>,
    _refresh_state: Option<Arc<ClapRefreshState>>,
    active: bool,
}

impl NativeAae {
    fn new() -> Self {
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: c"AAE solo test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_extension),
            request_restart: Some(no_request),
            request_process: Some(no_request),
            request_callback: Some(no_request),
        });
        // SAFETY: the boxed host outlives the wrapper and all scoped audio callbacks.
        let wrapper = unsafe { Wrapper::<crate::plugin::SotfAAE>::new(&*host) };
        let instance = Self {
            wrapper,
            _host: host,
            _refresh_state: None,
            active: false,
        };
        let plugin = instance.plugin();
        // SAFETY: the live generated callback is invoked on this instance's owning main thread.
        assert!(unsafe { ((*plugin).init.unwrap())(plugin) });
        instance
    }
    fn new_with_refresh() -> Self {
        let state = Arc::new(ClapRefreshState {
            calls: std::sync::atomic::AtomicUsize::new(0),
            callbacks: std::sync::atomic::AtomicUsize::new(0),
            plugin: std::sync::atomic::AtomicUsize::new(0),
            main_thread: std::thread::current().id(),
        });
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: Arc::as_ptr(&state).cast_mut().cast(),
            name: c"AAE refresh test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(refresh_extension),
            request_restart: Some(no_request),
            request_process: Some(no_request),
            request_callback: Some(refresh_request_callback),
        });
        // SAFETY: both retained host objects outlive the wrapper and callbacks.
        let wrapper = unsafe { Wrapper::<crate::plugin::SotfAAE>::new(&*host) };
        let instance = Self {
            wrapper,
            _host: host,
            _refresh_state: Some(state),
            active: false,
        };
        let plugin = instance.plugin();
        instance
            ._refresh_state
            .as_ref()
            .unwrap()
            .plugin
            .store(plugin as usize, std::sync::atomic::Ordering::Release);
        // SAFETY: owning main-thread init on the live instance.
        assert!(unsafe { ((*plugin).init.unwrap())(plugin) });
        instance
    }

    fn plugin(&self) -> *const clap_plugin {
        self.wrapper.clap_plugin.as_ptr()
    }
    fn activate(&mut self) {
        let plugin = self.plugin();
        // SAFETY: inactive live instance; main-thread activation with supported stereo/surround geometry.
        assert!(unsafe {
            ((*plugin).activate.unwrap())(plugin, SAMPLE_RATE as f64, 1, FRAMES as u32)
        });
        self.active = true;
    }
    fn deactivate(&mut self) {
        if self.active {
            // SAFETY: all scoped audio callbacks have joined and stopped processing.
            unsafe { ((*self.plugin()).deactivate.unwrap())(self.plugin()) };
            self.active = false;
        }
    }
    fn params(&self) -> *const clap_plugin_params {
        // SAFETY: stable extension table belongs to the live plugin.
        let params = unsafe {
            ((*self.plugin()).get_extension.unwrap())(self.plugin(), CLAP_EXT_PARAMS.as_ptr())
        }
        .cast::<clap_plugin_params>();
        assert!(!params.is_null());
        params
    }
    fn id(&self, name: &str) -> u32 {
        // SAFETY: each info entry is initialized by its callback and has a NUL-terminated name.
        unsafe {
            for index in 0..((*self.params()).count.unwrap())(self.plugin()) {
                let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
                assert!(((*self.params()).get_info.unwrap())(
                    self.plugin(),
                    index,
                    info.as_mut_ptr()
                ));
                let info = info.assume_init();
                if CStr::from_ptr(info.name.as_ptr()).to_bytes() == name.as_bytes() {
                    return info.id;
                }
            }
        }
        panic!("missing parameter {name}");
    }
    fn pair(&self) -> [f64; 2] {
        let mut pair = [0.0; 2];
        for (index, name) in ["Solo Early", "Solo Late"].iter().enumerate() {
            // SAFETY: live parameter table writes exactly one f64 into the provided location.
            assert!(unsafe {
                ((*self.params()).get_value.unwrap())(
                    self.plugin(),
                    self.id(name),
                    &mut pair[index],
                )
            });
        }
        pair
    }
    fn load_state(&self, state: &nih_plug::wrapper::state::PluginState) -> bool {
        use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
        use clap_sys::stream::clap_istream;
        struct Input {
            bytes: Vec<u8>,
            position: usize,
        }
        unsafe extern "C" fn read(
            stream: *const clap_istream,
            buffer: *mut c_void,
            size: u64,
        ) -> i64 {
            // SAFETY: ctx exclusively owns this retained input for the synchronous
            // load. NIH supplies a writable destination with the requested capacity.
            let input = unsafe { &mut *(*stream).ctx.cast::<Input>() };
            let count = usize::try_from(size)
                .unwrap()
                .min(input.bytes.len() - input.position);
            unsafe {
                ptr::copy_nonoverlapping(
                    input.bytes.as_ptr().add(input.position),
                    buffer.cast::<u8>(),
                    count,
                )
            };
            input.position += count;
            count as i64
        }
        let serialized = serde_json::to_vec(state).unwrap();
        let mut input = Input {
            bytes: (serialized.len() as u64).to_le_bytes().to_vec(),
            position: 0,
        };
        input.bytes.extend_from_slice(&serialized);
        let stream = clap_istream {
            ctx: (&mut input as *mut Input).cast(),
            read: Some(read),
        };
        // SAFETY: owning main-thread load with retained plugin and stream, after
        // the exclusive processing session has joined.
        let extension = unsafe {
            ((*self.plugin()).get_extension.unwrap())(self.plugin(), CLAP_EXT_STATE.as_ptr())
        }
        .cast::<clap_plugin_state>();
        assert!(!extension.is_null());
        unsafe { ((*extension).load.unwrap())(self.plugin(), &stream) }
    }

    fn saved_state(&self) -> nih_plug::wrapper::state::PluginState {
        use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
        use clap_sys::stream::clap_ostream;
        unsafe extern "C" fn write(
            stream: *const clap_ostream,
            buffer: *const c_void,
            size: u64,
        ) -> i64 {
            let size = usize::try_from(size).unwrap();
            // SAFETY: NIH supplies a readable buffer for this synchronous write;
            // ctx points to the exclusive Vec retained by saved_state().
            let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size) };
            let output = unsafe { &mut *(*stream).ctx.cast::<Vec<u8>>() };
            output.extend_from_slice(bytes);
            size as i64
        }
        let mut output = Vec::new();
        let stream = clap_ostream {
            ctx: (&mut output as *mut Vec<u8>).cast(),
            write: Some(write),
        };
        // SAFETY: the initialized plugin and stream live throughout the owning
        // main-thread save; its exclusive processing session has already joined.
        let extension = unsafe {
            ((*self.plugin()).get_extension.unwrap())(self.plugin(), CLAP_EXT_STATE.as_ptr())
        }
        .cast::<clap_plugin_state>();
        assert!(!extension.is_null());
        assert!(unsafe { ((*extension).save.unwrap())(self.plugin(), &stream) });
        let length = u64::from_le_bytes(output[..8].try_into().unwrap());
        assert_eq!(usize::try_from(length).unwrap(), output.len() - 8);
        serde_json::from_slice(&output[8..]).unwrap()
    }

    fn process(&mut self, input: &[[f32; 2]; FRAMES], pair: Option<[bool; 2]>) -> (i32, Vec<f32>) {
        self.process_sequence(&[(*input, pair)]).pop().unwrap()
    }
    fn process_sequence(
        &mut self,
        blocks: &[([[f32; 2]; FRAMES], Option<[bool; 2]>)],
    ) -> Vec<(i32, Vec<f32>)> {
        assert!(self.active);
        let events = blocks
            .iter()
            .map(|(_, pair)| {
                pair.map(|pair| {
                    ["Solo Early", "Solo Late"]
                        .iter()
                        .zip(pair)
                        .map(|(name, value)| clap_event_param_value {
                            header: clap_event_header {
                                size: std::mem::size_of::<clap_event_param_value>() as u32,
                                time: 0,
                                space_id: CLAP_CORE_EVENT_SPACE_ID,
                                type_: CLAP_EVENT_PARAM_VALUE,
                                flags: 0,
                            },
                            param_id: self.id(name),
                            cookie: ptr::null_mut(),
                            note_id: -1,
                            port_index: -1,
                            channel: -1,
                            key: -1,
                            value: f64::from(u8::from(value)),
                        })
                        .map(NativeParamEvent::Value)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        self.process_prepared_events(blocks, events)
    }

    fn process_prepared_events(
        &mut self,
        blocks: &[([[f32; 2]; FRAMES], Option<[bool; 2]>)],
        events: Vec<Vec<NativeParamEvent>>,
    ) -> Vec<(i32, Vec<f32>)> {
        assert!(self.active);
        assert_eq!(blocks.len(), events.len());
        let address = self.plugin() as usize;
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    // The upstream debug wrapper locks its parking_lot mutex inside
                    // the guarded start callback. Initialize this same-version
                    // deadlock-detection TLS before entering any guarded callback;
                    // this is audio-thread setup, not plugin processing permission.
                    // start, process, and stop retain their original allocation guards.
                    let thread_setup = parking_lot::Mutex::new(());
                    drop(thread_setup.lock());
                    let plugin = address as *const clap_plugin;
                    // SAFETY: activation preceded this exclusive audio session. The
                    // owning main thread waits for its join before touching the instance.
                    assert!(unsafe { ((*plugin).start_processing.unwrap())(plugin) });
                    let mut results = Vec::with_capacity(blocks.len());
                    for ((input, _), events) in blocks.iter().zip(&events) {
                        let mut event_slice = events.as_slice();
                        let event_list = clap_input_events {
                            ctx: (&mut event_slice as *mut &[NativeParamEvent]).cast(),
                            size: Some(event_count),
                            get: Some(event_at),
                        };
                        let mut inputs = [input.map(|frame| frame[0]), input.map(|frame| frame[1])];
                        let mut outputs = [[f32::NAN; FRAMES]; 6];
                        let mut in_channels = inputs
                            .iter_mut()
                            .map(|channel| channel.as_mut_ptr())
                            .collect::<Vec<_>>();
                        let mut out_channels = outputs
                            .iter_mut()
                            .map(|channel| channel.as_mut_ptr())
                            .collect::<Vec<_>>();
                        let in_bus = clap_audio_buffer {
                            data32: in_channels.as_mut_ptr(),
                            data64: ptr::null_mut(),
                            channel_count: 2,
                            latency: 0,
                            constant_mask: 0,
                        };
                        let mut out_bus = clap_audio_buffer {
                            data32: out_channels.as_mut_ptr(),
                            data64: ptr::null_mut(),
                            channel_count: 6,
                            latency: 0,
                            constant_mask: 0,
                        };
                        let process = clap_process {
                            steady_time: -1,
                            frames_count: FRAMES as u32,
                            transport: ptr::null(),
                            audio_inputs: &in_bus,
                            audio_outputs: &mut out_bus,
                            audio_inputs_count: 1,
                            audio_outputs_count: 1,
                            in_events: &event_list,
                            out_events: ptr::null(),
                        };
                        // SAFETY: main-thread activation completed before this exclusive scoped audio
                        // thread; host, plugin, event list and all planar buffers outlive the callbacks.
                        // Joining the thread precedes any main-thread reactivation/destruction.
                        let status = unsafe { ((*plugin).process.unwrap())(plugin, &process) };
                        results.push((
                            status,
                            (0..FRAMES)
                                .flat_map(|frame| outputs.iter().map(move |channel| channel[frame]))
                                .collect(),
                        ));
                    }
                    // SAFETY: this session started processing once and every callback
                    // completed before stopping on the same exclusive audio thread.
                    unsafe { ((*plugin).stop_processing.unwrap())(plugin) };
                    results
                })
                .join()
                .unwrap()
        })
    }
}
impl Drop for NativeAae {
    fn drop(&mut self) {
        self.deactivate();
    }
}
unsafe extern "C" fn event_count(list: *const clap_input_events) -> u32 {
    // SAFETY: ctx is the event slice reference that outlives this synchronous process call.
    let events = unsafe { (*list).ctx.cast::<&[NativeParamEvent]>().read() };
    events.len() as u32
}
unsafe extern "C" fn event_at(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    // SAFETY: ctx is the retained event slice; bounds checked indexing prevents invalid pointers.
    let events = unsafe { (*list).ctx.cast::<&[NativeParamEvent]>().read() };
    events
        .get(index as usize)
        .map_or(ptr::null(), |event| match event {
            NativeParamEvent::Value(event) => &event.header,
            NativeParamEvent::Modulation(event) => &event.header,
        })
}

#[test]
fn aae_solo_native_clap_valid_swap_survives_reactivation_reverse_and_off() {
    let mut plugin = NativeAae::new();
    plugin.activate();
    for pair in [[false, true], [true, false], [false, true], [false, false]] {
        assert_ne!(
            plugin.process(&[[0.0; 2]; FRAMES], Some(pair)).0,
            CLAP_PROCESS_ERROR
        );
        assert_eq!(plugin.pair(), pair.map(|value| f64::from(u8::from(value))));
        plugin.deactivate();
        plugin.activate();
        assert_ne!(
            plugin.process(&[[0.0; 2]; FRAMES], None).0,
            CLAP_PROCESS_ERROR
        );
        assert_eq!(plugin.pair(), pair.map(|value| f64::from(u8::from(value))));
    }
}

#[test]
fn aae_solo_native_clap_valid_automation_preserves_bit_exact_tail_history() {
    let mut subject = NativeAae::new();
    subject.activate();
    let params = parameters("AAE");
    let control = super::configuration::create_plugin("AAE", SAMPLE_RATE as f64, &params).unwrap();
    let mut control = plugins_bridge::prepare_standalone_plugin(control, FRAMES).unwrap();
    control.initialize(SAMPLE_RATE as f64).unwrap();
    params.sync_to_plugin(control.as_mut()).unwrap();
    control.reset();
    assert_eq!(control.input_channels(), 2);
    assert_eq!(control.output_channels(), 6);

    let mut impulse = [[0.0; 2]; FRAMES];
    impulse[0] = [0.5, 0.25];
    let silence = [[0.0; 2]; FRAMES];
    let mut blocks = vec![(impulse, None)];
    for _ in 0..80 {
        blocks.push((silence, None));
    }
    for pair in [[false, true], [true, false], [false, true], [false, false]] {
        for block in 0..20 {
            blocks.push((silence, (block == 0).then_some(pair)));
        }
    }
    blocks.push((silence, None));
    let actual = subject.process_sequence(&blocks);
    assert_eq!(actual.len(), blocks.len());
    let mut last_expected = Vec::new();
    for ((input, pair), (status, output)) in blocks.iter().zip(actual) {
        if let Some(pair) = pair {
            // Independent reference: the existing DSP setters admit the same valid
            // tuple by disabling first. Solo intentionally gates processing branches,
            // so the reference advances under the identical tuple and input history.
            for enabled in [false, true] {
                for (id, value) in ["solo_early", "solo_late"].into_iter().zip(*pair) {
                    if value == enabled {
                        control
                            .set_parameter(ParameterId::from(id), ParameterValue::Bool(value))
                            .unwrap();
                    }
                }
            }
        }
        let input = input.iter().flatten().copied().collect::<Vec<_>>();
        last_expected = render(control.as_mut(), &input);
        assert_ne!(status, CLAP_PROCESS_ERROR);
        assert_eq!(output, last_expected);
    }
    assert!(
        last_expected.iter().any(|sample| sample.abs() > 1e-8),
        "history oracle must contain a retained tail"
    );
    assert_eq!(subject.pair(), [0.0, 0.0]);
}

fn native_parameters() -> Arc<DynamicParams> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("AAE"));
    let infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();
    DynamicParams::from_infos_for_native_plugin("AAE", &infos)
}

#[test]
fn aae_solo_native_coupling_preserves_event_order_echo_and_refresh_retry() {
    use nih_plug::prelude::{Param, Params};
    let params = native_parameters();
    let early = &params.bool_params[params.param_map["solo_early"].index];
    let late = &params.bool_params[params.param_map["solo_late"].index];
    let early_flags = early.flags();
    let late_flags = late.flags();
    early.set_plain_value_for_initialization(true);
    late.set_plain_value_for_initialization(true);
    assert!(!early.value());
    assert!(!early.unmodulated_plain_value());
    assert!(late.value());
    assert!(late.unmodulated_plain_value());
    let first = params.begin_parameter_value_rescan().unwrap();
    assert!(params.begin_parameter_value_rescan().is_none());
    params.finish_parameter_value_rescan(first, false);
    assert_eq!(params.begin_parameter_value_rescan(), Some(first));
    // A newer peer correction during an outstanding refresh remains pending.
    early.set_plain_value_for_initialization(true);
    assert!(early.value());
    assert!(!late.value());
    params.finish_parameter_value_rescan(first, true);
    let newer = params.begin_parameter_value_rescan().unwrap();
    assert!(newer > first);
    params.finish_parameter_value_rescan(newer, true);
    assert!(params.begin_parameter_value_rescan().is_none());
    assert!(!early.set_plain_value_for_initialization(true));
    assert!(params.begin_parameter_value_rescan().is_none());
    assert_eq!(early.flags(), early_flags);
    assert_eq!(late.flags(), late_flags);
    let mut plugin = dsp(&params);
    assert_pair(&plugin, true, false);
    late.set_plain_value_for_initialization(true);
    params.sync_to_plugin(&mut plugin).unwrap();
    assert_pair(&plugin, false, true);
}

#[test]
fn aae_solo_native_invalid_preset_is_rejected_before_parameter_mutation() {
    use nih_plug::prelude::Params;
    use nih_plug::wrapper::state::{ParamValue, PluginState};
    let params = native_parameters();
    let state = PluginState {
        params: std::collections::BTreeMap::from([
            ("solo_early".to_owned(), ParamValue::Bool(true)),
            ("solo_late".to_owned(), ParamValue::Bool(true)),
        ]),
        version: String::new(),
        fields: std::collections::BTreeMap::new(),
    };
    assert!(!params.validate_state(&state, false, false, None));
    assert_eq!(
        params.value("solo_early"),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(params.value("solo_late"), Some(ParameterValue::Bool(false)));
    assert!(params.begin_parameter_value_rescan().is_none());
    let mut native = NativeAae::new();
    native.activate();
    assert_ne!(
        native.process(&[[0.0; 2]; FRAMES], Some([false, true])).0,
        CLAP_PROCESS_ERROR
    );
    let before = native.saved_state();
    let mut invalid = before.clone();
    invalid
        .params
        .insert("solo_early".to_owned(), ParamValue::Bool(true));
    invalid
        .params
        .insert("solo_late".to_owned(), ParamValue::Bool(true));
    assert!(!native.load_state(&invalid));
    assert_eq!(native.pair(), [0.0, 1.0]);
    assert_eq!(
        serde_json::to_value(native.saved_state()).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    invalid
        .params
        .insert("solo_early".to_owned(), ParamValue::F32(1.0));
    assert!(!native.load_state(&invalid));
    assert_eq!(
        serde_json::to_value(native.saved_state()).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}

#[test]
fn aae_solo_native_contradictory_automation_remains_continuous_and_last_event_wins() {
    let mut native = NativeAae::new();
    native.activate();
    let params = parameters("AAE");
    let control = super::configuration::create_plugin("AAE", SAMPLE_RATE as f64, &params).unwrap();
    let mut control = plugins_bridge::prepare_standalone_plugin(control, FRAMES).unwrap();
    control.initialize(SAMPLE_RATE as f64).unwrap();
    params.sync_to_plugin(control.as_mut()).unwrap();
    control.reset();
    let mut impulse = [[0.0; 2]; FRAMES];
    impulse[0] = [0.5, -0.25];
    let silence = [[0.0; 2]; FRAMES];
    let mut blocks = vec![(impulse, None)];
    for _ in 0..80 {
        blocks.push((silence, None));
    }
    for pair in [[true, true], [true, false], [true, true], [false, false]] {
        for block in 0..20 {
            blocks.push((silence, (block == 0).then_some(pair)));
        }
    }
    blocks.push((silence, None));
    let results = native.process_sequence(&blocks);
    assert_eq!(results.len(), blocks.len());
    let mut last_expected = Vec::new();
    for ((input, requested), (status, audio)) in blocks.iter().zip(results) {
        if let Some(requested) = requested {
            // The ABI fixture writes Early followed by Late. A contradictory
            // enabled pair therefore admits Late and disables Early. The direct
            // DSP reference receives only that final valid tuple, without using
            // the native coupled-control callbacks being tested.
            let admitted = if requested == &[true, true] {
                [false, true]
            } else {
                *requested
            };
            for enabled in [false, true] {
                for (id, value) in ["solo_early", "solo_late"].into_iter().zip(admitted) {
                    if value == enabled {
                        control
                            .set_parameter(ParameterId::from(id), ParameterValue::Bool(value))
                            .unwrap();
                    }
                }
            }
        }
        let input = input.iter().flatten().copied().collect::<Vec<_>>();
        last_expected = render(control.as_mut(), &input);
        assert_ne!(status, CLAP_PROCESS_ERROR);
        assert!(audio.iter().all(|sample| sample.is_finite()));
        assert_eq!(audio, last_expected);
    }
    assert_eq!(native.pair(), [0.0, 0.0]);
    assert!(last_expected.iter().any(|sample| sample.abs() > 1e-8));
}

#[test]
fn aae_solo_native_modulation_preserves_base_effective_state_and_peer_clear() {
    // The third element marks modulation rather than a plain-value write.
    let cases: &[(&[(&str, f64, bool)], [bool; 2], [f64; 2])] = &[
        (
            &[
                ("Solo Late", 1.0, false),
                ("Solo Early", -1.0, true),
                ("Solo Early", 1.0, false),
            ],
            [false, false],
            [1.0, 0.0],
        ),
        (
            &[
                ("Solo Early", -1.0, true),
                ("Solo Early", 1.0, false),
                ("Solo Late", 1.0, false),
            ],
            [false, true],
            [0.0, 1.0],
        ),
        (
            &[("Solo Late", 1.0, false), ("Solo Early", 1.0, true)],
            [true, false],
            [0.0, 0.0],
        ),
        (
            &[
                ("Solo Early", 1.0, true),
                ("Solo Late", 1.0, true),
                ("Solo Late", 1.0, false),
                ("Solo Late", 1.0, false),
                ("Solo Early", 1.0, false),
            ],
            [true, false],
            [1.0, 0.0],
        ),
        (
            &[("Solo Late", 1.0, false), ("Solo Early", 1.0, false)],
            [true, false],
            [1.0, 0.0],
        ),
    ];
    for (writes, expected_effective, expected_base) in cases {
        let mut native = NativeAae::new();
        native.activate();
        let params = parameters("AAE");
        let control =
            super::configuration::create_plugin("AAE", SAMPLE_RATE as f64, &params).unwrap();
        let mut control = plugins_bridge::prepare_standalone_plugin(control, FRAMES).unwrap();
        control.initialize(SAMPLE_RATE as f64).unwrap();
        params.sync_to_plugin(control.as_mut()).unwrap();
        control.reset();
        let mut impulse = [[0.0; 2]; FRAMES];
        impulse[0] = [0.5, 0.25];
        let silence = [[0.0; 2]; FRAMES];
        let mut blocks = vec![(impulse, None)];
        for _ in 0..100 {
            blocks.push((silence, None));
        }
        let mut events = (0..blocks.len()).map(|_| Vec::new()).collect::<Vec<_>>();
        events[81] = writes
            .iter()
            .map(|(name, value, modulation)| {
                let header = clap_event_header {
                    size: if *modulation {
                        std::mem::size_of::<clap_event_param_mod>()
                    } else {
                        std::mem::size_of::<clap_event_param_value>()
                    } as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: if *modulation {
                        CLAP_EVENT_PARAM_MOD
                    } else {
                        CLAP_EVENT_PARAM_VALUE
                    },
                    flags: 0,
                };
                if *modulation {
                    NativeParamEvent::Modulation(clap_event_param_mod {
                        header,
                        param_id: native.id(name),
                        cookie: ptr::null_mut(),
                        note_id: -1,
                        port_index: -1,
                        channel: -1,
                        key: -1,
                        amount: *value,
                    })
                } else {
                    NativeParamEvent::Value(clap_event_param_value {
                        header,
                        param_id: native.id(name),
                        cookie: ptr::null_mut(),
                        note_id: -1,
                        port_index: -1,
                        channel: -1,
                        key: -1,
                        value: *value,
                    })
                }
            })
            .collect();
        let actual = native.process_prepared_events(&blocks, events);
        let mut nonzero_tail = false;
        for (index, ((input, _), (status, audio))) in blocks.iter().zip(actual).enumerate() {
            if index == 81 {
                for enabled in [false, true] {
                    for (id, value) in ["solo_early", "solo_late"]
                        .into_iter()
                        .zip(*expected_effective)
                    {
                        if value == enabled {
                            control
                                .set_parameter(ParameterId::from(id), ParameterValue::Bool(value))
                                .unwrap();
                        }
                    }
                }
            }
            let input = input.iter().flatten().copied().collect::<Vec<_>>();
            let expected = render(control.as_mut(), &input);
            assert_ne!(status, CLAP_PROCESS_ERROR);
            assert_eq!(audio, expected);
            if index >= 81 {
                nonzero_tail |= expected.iter().any(|sample| sample.abs() > 1e-8);
            }
        }
        assert!(nonzero_tail);
        // NIH's existing CLAP getter reports the effective normalized value;
        // state serialization independently persists the unmodulated base value.
        assert_eq!(
            native.pair(),
            expected_effective.map(|value| f64::from(u8::from(value)))
        );
        assert!(!(expected_effective[0] && expected_effective[1]));
        assert!(!(expected_base[0] > 0.5 && expected_base[1] > 0.5));
        let saved = native.saved_state();
        for (id, value) in ["solo_early", "solo_late"].into_iter().zip(expected_base) {
            assert!(
                matches!(saved.params.get(id), Some(nih_plug::wrapper::state::ParamValue::Bool(saved)) if *saved == (*value > 0.5))
            );
        }
    }
}

#[test]
fn aae_solo_coupled_api_retains_ordinary_boolean_callback_and_echo_semantics() {
    use nih_plug::prelude::{BoolParam, Param};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let changed = Arc::new(AtomicUsize::new(0));
    let observed = changed.clone();
    let ordinary = BoolParam::new("Ordinary", false).with_callback(Arc::new(move |_| {
        observed.fetch_add(1, Ordering::Relaxed);
    }));
    let flags = ordinary.flags();
    assert!(!ordinary.set_plain_value_for_initialization(false));
    assert_eq!(changed.load(Ordering::Relaxed), 0);
    assert!(ordinary.set_plain_value_for_initialization(true));
    assert!(!ordinary.set_plain_value_for_initialization(true));
    assert_eq!(changed.load(Ordering::Relaxed), 1);
    assert!(ordinary.value());
    assert!(ordinary.unmodulated_plain_value());
    assert_eq!(ordinary.modulated_normalized_value(), 1.0);
    assert_eq!(ordinary.unmodulated_normalized_value(), 1.0);
    assert!(ordinary.set_plain_value_for_initialization(false));
    assert!(!ordinary.set_plain_value_for_initialization(false));
    assert_eq!(changed.load(Ordering::Relaxed), 2);
    assert_eq!(ordinary.flags(), flags);
    let updated = Arc::new(AtomicUsize::new(0));
    let observed = updated.clone();
    let coupled = BoolParam::new("Coupled", false).with_coupled_update_callback(Arc::new(
        move |base, effective| {
            assert_eq!(base, effective);
            observed.fetch_add(1, Ordering::Relaxed);
        },
    ));
    assert!(!coupled.set_plain_value_for_initialization(false));
    assert!(coupled.set_plain_value_for_initialization(true));
    assert!(!coupled.set_plain_value_for_initialization(true));
    assert_eq!(updated.load(Ordering::Relaxed), 3);
    assert_eq!(coupled.flags(), flags);
}

struct ClapRefreshState {
    calls: std::sync::atomic::AtomicUsize,
    callbacks: std::sync::atomic::AtomicUsize,
    plugin: std::sync::atomic::AtomicUsize,
    main_thread: std::thread::ThreadId,
}

unsafe extern "C" fn refresh_request_callback(host: *const clap_host) {
    // SAFETY: NativeAae retains this Arc allocation until after its wrapper drops.
    let state = unsafe { &*(*host).host_data.cast::<ClapRefreshState>() };
    state
        .callbacks
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

unsafe extern "C" fn refresh_rescan(host: *const clap_host, flags: u32) {
    use clap_sys::ext::params::CLAP_PARAM_RESCAN_VALUES;
    // SAFETY: the boxed host and its Arc state remain live throughout the callback.
    let state = unsafe { &*(*host).host_data.cast::<ClapRefreshState>() };
    assert_eq!(std::thread::current().id(), state.main_thread);
    assert_eq!(flags, CLAP_PARAM_RESCAN_VALUES);
    let plugin = state.plugin.load(std::sync::atomic::Ordering::Acquire) as *const clap_plugin;
    // Reenter the real parameter extension during notification. No extension
    // borrow or editor lock may be retained across this host callback.
    let params = unsafe { ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr()) }
        .cast::<clap_plugin_params>();
    assert!(!params.is_null());
    // Reenter GUI task dispatch while this notification is in flight. The
    // generation claim prevents recursive rescans without retaining any lock.
    unsafe { ((*plugin).on_main_thread.unwrap())(plugin) };
    let count = unsafe { ((*params).count.unwrap())(plugin) };
    for index in 0..count {
        let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
        assert!(unsafe { ((*params).get_info.unwrap())(plugin, index, info.as_mut_ptr()) });
        let info = unsafe { info.assume_init() };
        let mut value = f64::NAN;
        assert!(unsafe { ((*params).get_value.unwrap())(plugin, info.id, &mut value) });
        assert!(value.is_finite());
    }
    state
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

unsafe extern "C" fn refresh_extension(
    _host: *const clap_host,
    id: *const c_char,
) -> *const c_void {
    static PARAMS: clap_sys::ext::params::clap_host_params =
        clap_sys::ext::params::clap_host_params {
            rescan: Some(refresh_rescan),
            clear: None,
            request_flush: Some(no_request),
        };
    // SAFETY: the wrapper supplies a valid, null-terminated extension identifier.
    if unsafe { CStr::from_ptr(id) } == CLAP_EXT_PARAMS {
        (&PARAMS as *const clap_sys::ext::params::clap_host_params).cast()
    } else {
        ptr::null()
    }
}

#[test]
fn aae_solo_native_clap_host_refresh_is_main_thread_coalesced_and_reentrant() {
    use std::sync::atomic::Ordering;
    let mut native = NativeAae::new_with_refresh();
    native.activate();
    for expected in [1, 2] {
        assert_ne!(
            native.process(&[[0.0; 2]; FRAMES], Some([true, true])).0,
            CLAP_PROCESS_ERROR
        );
        let state = native._refresh_state.as_ref().unwrap();
        assert!(state.callbacks.load(Ordering::Relaxed) > 0);
        // SAFETY: this is the owning main thread after the exclusive audio session joined.
        unsafe { ((*native.plugin()).on_main_thread.unwrap())(native.plugin()) };
        assert_eq!(state.calls.load(Ordering::Relaxed), expected);
        assert_eq!(native.pair(), [0.0, 1.0]);
        unsafe { ((*native.plugin()).on_main_thread.unwrap())(native.plugin()) };
        assert_eq!(state.calls.load(Ordering::Relaxed), expected);
    }
}

// Linux identifies the creating host thread as its real GUI dispatch thread.
// macOS requires Cocoa main-loop ownership, deferred under the AU priority.
#[cfg(target_os = "linux")]
mod aae_vst3_refresh_fixture {
    use super::*;
    use nih_plug::wrapper::vst3::vst3_sys;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vst3_sys as vst3_com;
    use vst3_sys::base::{IPluginBase, IUnknown, kResultFalse, kResultOk, tresult};
    use vst3_sys::utils::{SharedVstPtr, VstPtr};
    use vst3_sys::vst::{IComponent, IComponentHandler, IEditController, RestartFlags};
    use vst3_sys::{ComInterface, VST3};

    struct RefreshState {
        calls: AtomicUsize,
        reject_first: bool,
        main_thread: std::thread::ThreadId,
    }

    #[VST3(implements(IComponentHandler))]
    struct RefreshHandler {
        state: Arc<RefreshState>,
    }
    impl IComponentHandler for RefreshHandler {
        unsafe fn begin_edit(&self, _id: u32) -> tresult {
            kResultOk
        }
        unsafe fn perform_edit(&self, _id: u32, _value: f64) -> tresult {
            kResultOk
        }
        unsafe fn end_edit(&self, _id: u32) -> tresult {
            kResultOk
        }
        unsafe fn restart_component(&self, flags: i32) -> tresult {
            assert_eq!(std::thread::current().id(), self.state.main_thread);
            assert_eq!(flags, RestartFlags::kParamValuesChanged as i32);
            let call = self.state.calls.fetch_add(1, Ordering::Relaxed);
            if self.state.reject_first && call == 0 {
                kResultFalse
            } else {
                kResultOk
            }
        }
    }

    fn parameter_id(id: &str) -> u32 {
        id.bytes().fold(0_u32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u32::from(byte))
        }) & !(1 << 31)
    }

    #[test]
    fn aae_solo_native_vst3_host_refresh_retains_missing_and_rejected_notifications() {
        for missing_first in [false, true] {
            let wrapper = nih_plug::wrapper::vst3::Wrapper::<crate::plugin::SotfAAE>::new();
            let mut object = ptr::null_mut::<c_void>();
            assert_eq!(
                unsafe { wrapper.query_interface(&<dyn IComponent>::IID, &mut object) },
                kResultOk
            );
            // Mirror the upstream factory's owned-reference handoff: query_interface
            // retained the requested interface; release the Box's COM reference and
            // prevent Rust from freeing an object now owned by that interface.
            unsafe {
                wrapper.release();
            }
            Box::leak(wrapper);
            // SAFETY: successful query returns one owned, correctly typed COM reference.
            let component = unsafe { VstPtr::<dyn IComponent>::owned(object.cast()).unwrap() };
            let controller = component.cast::<dyn IEditController>().unwrap();
            assert_eq!(unsafe { component.initialize(ptr::null_mut()) }, kResultOk);
            let state = Arc::new(RefreshState {
                calls: AtomicUsize::new(0),
                reject_first: !missing_first,
                main_thread: std::thread::current().id(),
            });
            let handler = RefreshHandler::allocate(state.clone());
            let mut raw = ptr::null_mut::<c_void>();
            assert_eq!(
                unsafe { handler.query_interface(&<dyn IComponentHandler>::IID, &mut raw) },
                kResultOk
            );
            // SAFETY: retain the query's owned reference while passing the
            // repr(transparent) borrowed interface argument synchronously.
            let owned_handler =
                unsafe { VstPtr::<dyn IComponentHandler>::owned(raw.cast()).unwrap() };
            // The query reference now owns the handler. As with the component,
            // transfer the Box's ownership to COM before any assertion can unwind.
            unsafe {
                handler.release();
            }
            Box::leak(handler);
            let shared = || -> SharedVstPtr<dyn IComponentHandler> {
                // SAFETY: SharedVstPtr is repr(transparent); the owned reference
                // remains alive throughout each synchronous borrowed call.
                unsafe { std::mem::transmute(owned_handler.as_ptr()) }
            };
            if !missing_first {
                assert_eq!(
                    unsafe { controller.set_component_handler(shared()) },
                    kResultOk
                );
            }
            for (id, value) in [("solo_early", 1.0), ("solo_late", 1.0)] {
                assert_eq!(
                    unsafe { controller.set_param_normalized(parameter_id(id), value) },
                    kResultOk
                );
            }
            assert_eq!(
                unsafe { controller.get_param_normalized(parameter_id("solo_early")) },
                0.0
            );
            assert_eq!(
                unsafe { controller.get_param_normalized(parameter_id("solo_late")) },
                1.0
            );
            if missing_first {
                assert_eq!(state.calls.load(Ordering::Relaxed), 0);
                assert_eq!(
                    unsafe { controller.set_component_handler(shared()) },
                    kResultOk
                );
            } else {
                assert_eq!(state.calls.load(Ordering::Relaxed), 1);
            }
            // A subsequent existing GUI dispatch retries the retained generation.
            assert_eq!(
                unsafe { controller.set_param_normalized(parameter_id("solo_late"), 0.0) },
                kResultOk
            );
            assert_eq!(
                state.calls.load(Ordering::Relaxed),
                if missing_first { 1 } else { 2 }
            );
            assert_eq!(
                unsafe { controller.set_param_normalized(parameter_id("solo_early"), 1.0) },
                kResultOk
            );
            assert_eq!(
                unsafe { controller.set_param_normalized(parameter_id("solo_late"), 1.0) },
                kResultOk
            );
            assert_eq!(
                state.calls.load(Ordering::Relaxed),
                if missing_first { 2 } else { 3 }
            );
            assert_eq!(unsafe { component.terminate() }, kResultOk);
            drop(controller);
            drop(component);
            drop(owned_handler);
            // The final COM release must free the transferred handler storage;
            // only this test's observation Arc remains. This detects a leaked
            // allocation as well as keeping ownership valid through teardown.
            assert_eq!(Arc::strong_count(&state), 1);
        }
    }
}
