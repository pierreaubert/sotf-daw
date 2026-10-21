//! Validate the built Upmixer VST3 library with an independent macOS executable host.
//!
//! Run with the native library path as the only argument. All COM calls run on
//! the OS main thread. Audio buffers and JSON state cross the dynamic-library
//! boundary through C interfaces; Rust-owned state remains inside its producer.
//! This exercises library loading, not DAW installation or VST3 bundle packaging.

#![cfg_attr(
    target_os = "macos",
    expect(
        semicolon_in_expressions_from_non_local_macros,
        reason = "The pinned VST3 COM vtable generator emits expression-position semicolons"
    )
)]

// Rust guideline compliant 2026-02-21
#[cfg(target_os = "macos")]
mod host {
    use nih_plug::wrapper::state::{ParamValue, PluginState};
    use nih_plug::wrapper::vst3::vst3_sys;
    use std::cell::{Cell, RefCell};
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, ThreadId};
    use vst3_sys as vst3_com;
    use vst3_sys::base::{
        IBStream, IPluginBase, IPluginFactory, PClassInfo, kIBSeekCur, kIBSeekEnd, kIBSeekSet,
        kInvalidArgument, kResultFalse, kResultOk, tresult,
    };
    use vst3_sys::utils::{SharedVstPtr, StaticVstPtr, VstPtr};
    use vst3_sys::vst::{
        AudioBusBuffers, BusDirections, BusInfo, IAudioProcessor, IComponent, IComponentHandler,
        IEditController, MediaTypes, ProcessData, ProcessModes, ProcessSetup, RestartFlags,
        SymbolicSampleSizes,
    };
    use vst3_sys::vst::{IParamValueQueue, IParameterChanges};
    use vst3_sys::{ComInterface, VST3};

    const MAX_FRAMES: usize = 257;
    // POSIX RTLD_NOW: resolve every required symbol before running the tests.
    const RTLD_NOW: c_int = 2;
    unsafe extern "C" {
        fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn pthread_main_np() -> c_int;
    }

    struct Module {
        handle: *mut c_void,
        exit: unsafe extern "C" fn() -> bool,
    }
    impl Module {
        fn open(path: &Path) -> Self {
            let name = CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: the NUL-terminated path remains live through dlopen.
            let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
            assert!(!handle.is_null(), "cannot load {}", path.display());
            // SAFETY: this harness loads the selected NIH library whose exported
            // bundleEntry/bundleExit signatures were checked in the pinned source.
            // NIH does not dereference its unused bundleEntry argument.
            unsafe {
                let entry = dlsym(handle, c"bundleEntry".as_ptr());
                let exit = dlsym(handle, c"bundleExit".as_ptr());
                assert!(!entry.is_null() && !exit.is_null());
                let entry: unsafe extern "C" fn(*mut c_void) -> bool = std::mem::transmute(entry);
                let exit: unsafe extern "C" fn() -> bool = std::mem::transmute(exit);
                assert!(entry(ptr::null_mut()));
                Self { handle, exit }
            }
        }
        fn factory(&self) -> VstPtr<dyn IPluginFactory> {
            // SAFETY: the loaded module exports this exact VST3 factory signature.
            // The returned COM reference is owned and is released before module unload.
            unsafe {
                let symbol = dlsym(self.handle, c"GetPluginFactory".as_ptr());
                assert!(!symbol.is_null());
                let get: extern "system" fn() -> *mut c_void = std::mem::transmute(symbol);
                VstPtr::owned(get().cast()).unwrap()
            }
        }
    }
    impl Drop for Module {
        fn drop(&mut self) {
            // SAFETY: Native's borrow prevents unloading while any plugin COM
            // reference is live; all synchronous calls have completed.
            unsafe {
                assert!((self.exit)());
                assert_eq!(dlclose(self.handle), 0);
            }
        }
    }

    struct Notifications {
        flags: Mutex<Vec<i32>>,
        main: ThreadId,
        wrong_thread: AtomicBool,
    }
    #[VST3(implements(IComponentHandler))]
    struct Handler {
        state: Arc<Notifications>,
    }
    impl IComponentHandler for Handler {
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
            self.state
                .wrong_thread
                .fetch_or(thread::current().id() != self.state.main, Ordering::AcqRel);
            self.state.flags.lock().unwrap().push(flags);
            kResultOk
        }
    }

    #[VST3(implements(IParamValueQueue))]
    struct ScalarQueue {
        id: u32,
        normalized: f64,
    }
    impl IParamValueQueue for ScalarQueue {
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
            // SAFETY: valid queue result pointers supplied by the synchronous native caller.
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
    struct ScalarChanges {
        queue: VstPtr<dyn IParamValueQueue>,
    }
    impl IParameterChanges for ScalarChanges {
        unsafe fn get_parameter_count(&self) -> i32 {
            1
        }
        unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
            // SAFETY: StaticVstPtr is a transparent raw COM pointer. The owning
            // queue remains in ScalarChanges throughout the synchronous process call.
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
            // SAFETY: a null raw COM pointer is a valid absent StaticVstPtr result.
            unsafe { std::mem::zeroed() }
        }
    }

    #[VST3(implements(IBStream))]
    struct Stream {
        bytes: Rc<RefCell<Vec<u8>>>,
        cursor: Cell<usize>,
        writable: bool,
        max_read: usize,
    }
    impl IBStream for Stream {
        unsafe fn read(&self, output: *mut c_void, size: i32, read: *mut i32) -> tresult {
            let Ok(size) = usize::try_from(size) else {
                return kInvalidArgument;
            };
            if size > 0 && output.is_null() {
                return kInvalidArgument;
            }
            let bytes = self.bytes.borrow();
            let cursor = self.cursor.get();
            let count = size
                .min(bytes.len().saturating_sub(cursor))
                .min(self.max_read);
            if count > 0 {
                // SAFETY: source bounds are checked, and IBStream's caller supplies
                // size writable destination bytes. Host/plugin buffers are disjoint.
                unsafe {
                    ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), output.cast(), count);
                }
            }
            self.cursor.set(cursor + count);
            if !read.is_null() {
                // SAFETY: a non-null IBStream result pointer references writable i32 storage.
                unsafe {
                    *read = count as i32;
                }
            }
            kResultOk
        }
        unsafe fn write(&self, input: *const c_void, size: i32, written: *mut i32) -> tresult {
            let Ok(size) = usize::try_from(size) else {
                return kInvalidArgument;
            };
            if !self.writable || (size > 0 && input.is_null()) {
                return kResultFalse;
            }
            let Some(end) = self.cursor.get().checked_add(size) else {
                return kInvalidArgument;
            };
            let mut bytes = self.bytes.borrow_mut();
            let length = end.max(bytes.len());
            bytes.resize(length, 0);
            if size > 0 {
                // SAFETY: the caller supplies size readable bytes; the resized Vec
                // owns disjoint writable storage through end for this synchronous call.
                unsafe {
                    ptr::copy_nonoverlapping(
                        input.cast(),
                        bytes.as_mut_ptr().add(self.cursor.get()),
                        size,
                    );
                }
            }
            self.cursor.set(end);
            if !written.is_null() {
                // SAFETY: a non-null result pointer references writable i32 storage.
                unsafe {
                    *written = size as i32;
                }
            }
            kResultOk
        }
        unsafe fn seek(&self, offset: i64, mode: i32, result: *mut i64) -> tresult {
            let base = match mode {
                value if value == kIBSeekSet => 0_i128,
                value if value == kIBSeekCur => self.cursor.get() as i128,
                value if value == kIBSeekEnd => self.bytes.borrow().len() as i128,
                _ => return kInvalidArgument,
            };
            let target = base + i128::from(offset);
            if !(0..=i128::from(i64::MAX)).contains(&target) {
                return kInvalidArgument;
            }
            let Ok(target) = usize::try_from(target) else {
                return kInvalidArgument;
            };
            self.cursor.set(target);
            if !result.is_null() {
                // SAFETY: the caller supplies writable i64 result storage, and range was checked.
                unsafe {
                    *result = target as i64;
                }
            }
            kResultOk
        }
        unsafe fn tell(&self, result: *mut i64) -> tresult {
            if result.is_null() {
                return kInvalidArgument;
            }
            // SAFETY: result is writable and all cursor assignments fit i64.
            unsafe {
                *result = self.cursor.get() as i64;
            }
            kResultOk
        }
    }
    fn with_stream(
        initial: &[u8],
        writable: bool,
        start: usize,
        max_read: usize,
        call: impl FnOnce(SharedVstPtr<dyn IBStream>) -> tresult,
    ) -> (tresult, Vec<u8>) {
        assert!(start <= initial.len() && start <= i64::MAX as usize && max_read > 0);
        let bytes = Rc::new(RefCell::new(initial.to_vec()));
        let object = Stream::allocate(Rc::clone(&bytes), Cell::new(start), writable, max_read);
        // SAFETY: Stream's first COM interface is IBStream; ownership transfers once.
        // SharedVstPtr is a repr(transparent) raw COM pointer without a constructor
        // in the pinned bindings. The owning pointer stays live for the entire call.
        let owner = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(object).cast()).unwrap() };
        // SAFETY: same pointer representation; no reference count or lifetime is changed.
        let borrowed: SharedVstPtr<dyn IBStream> = unsafe { std::mem::transmute(owner.as_ptr()) };
        let result = call(borrowed);
        let contents = bytes.borrow().clone();
        (result, contents)
    }

    struct Native<'module> {
        component: VstPtr<dyn IComponent>,
        processor: VstPtr<dyn IAudioProcessor>,
        controller: VstPtr<dyn IEditController>,
        _handler: VstPtr<dyn IComponentHandler>,
        notices: Arc<Notifications>,
        active: bool,
        processing: bool,
        _module: &'module Module,
    }
    impl<'module> Native<'module> {
        fn new(module: &'module Module) -> Self {
            let factory = module.factory();
            // SAFETY: all calls use retained COM interfaces and initialized out pointers.
            let (component, processor, controller) = unsafe {
                assert_eq!(factory.count_classes(), 1);
                let mut info = std::mem::zeroed::<PClassInfo>();
                assert_eq!(factory.get_class_info(0, &mut info), kResultOk);
                assert_eq!(info.cid.data, *b"SotfUpmixer00001");
                let mut raw = ptr::null_mut::<c_void>();
                assert_eq!(
                    factory.create_instance(&info.cid, &<dyn IComponent>::IID, &mut raw),
                    kResultOk
                );
                let component = VstPtr::<dyn IComponent>::owned(raw.cast()).unwrap();
                let processor = component.cast::<dyn IAudioProcessor>().unwrap();
                let controller = component.cast::<dyn IEditController>().unwrap();
                assert_eq!(component.initialize(ptr::null_mut()), kResultOk);
                (component, processor, controller)
            };
            let notices = Arc::new(Notifications {
                flags: Mutex::new(Vec::new()),
                main: thread::current().id(),
                wrong_thread: AtomicBool::new(false),
            });
            let handler = Handler::allocate(Arc::clone(&notices));
            // SAFETY: Handler's first interface is IComponentHandler, retained until
            // after component termination. SharedVstPtr's transparent representation
            // is the raw COM pointer, borrowed only for this synchronous setter.
            let handler = unsafe {
                VstPtr::<dyn IComponentHandler>::owned(Box::into_raw(handler).cast()).unwrap()
            };
            // SAFETY: representation only; the owner remains live in Native and NIH clones its reference.
            let shared: SharedVstPtr<dyn IComponentHandler> =
                unsafe { std::mem::transmute(handler.as_ptr()) };
            // SAFETY: the initialized controller receives a retained valid handler.
            assert_eq!(
                unsafe { controller.set_component_handler(shared) },
                kResultOk
            );
            let native = Self {
                component,
                processor,
                controller,
                _handler: handler,
                notices,
                active: false,
                processing: false,
                _module: module,
            };
            native.ports();
            native
        }
        fn ports(&self) {
            // SAFETY: COM out values contain POD fields; interfaces remain retained.
            unsafe {
                for (direction, channels) in
                    [(BusDirections::kInput, 2), (BusDirections::kOutput, 6)]
                {
                    assert_eq!(
                        self.component
                            .get_bus_count(MediaTypes::kAudio as i32, direction as i32),
                        1
                    );
                    let mut info = std::mem::zeroed::<BusInfo>();
                    assert_eq!(
                        self.component.get_bus_info(
                            MediaTypes::kAudio as i32,
                            direction as i32,
                            0,
                            &mut info
                        ),
                        kResultOk
                    );
                    assert_eq!(info.channel_count, channels);
                    let mut arrangement = 0;
                    assert_eq!(
                        self.processor
                            .get_bus_arrangement(direction as i32, 0, &mut arrangement),
                        kResultOk
                    );
                    let expected = if channels == 2 {
                        vst3_sys::vst::kStereo
                    } else {
                        vst3_sys::vst::k51
                    };
                    assert_eq!(arrangement, expected);
                    assert_eq!(
                        self.component.activate_bus(
                            MediaTypes::kAudio as i32,
                            direction as i32,
                            0,
                            1
                        ),
                        kResultOk
                    );
                }
            }
        }
        fn latency(&self) -> u32 {
            // SAFETY: the processor remains retained and initialized.
            unsafe { self.processor.get_latency_samples() }
        }
        fn notifications(&self) -> Vec<i32> {
            assert!(!self.notices.wrong_thread.load(Ordering::Acquire));
            self.notices.flags.lock().unwrap().clone()
        }
        fn saved_bytes(&self) -> Vec<u8> {
            let (result, bytes) = with_stream(&[], true, 0, usize::MAX, |stream| {
                // SAFETY: stream context and controller stay alive during the synchronous save.
                unsafe { self.component.get_state(stream) }
            });
            assert_eq!(result, kResultOk);
            bytes
        }
        fn saved(&self) -> PluginState {
            serde_json::from_slice(&self.saved_bytes()).unwrap()
        }
        fn load_bytes(&self, bytes: &[u8], start: usize, max_read: usize) -> tresult {
            with_stream(bytes, false, start, max_read, |stream| {
                // SAFETY: retained stream/interface cover this synchronous control-thread load.
                unsafe { self.component.set_state(stream) }
            })
            .0
        }
        fn load(&self, state: &PluginState, prefixed: bool) -> tresult {
            let json = serde_json::to_vec(state).unwrap();
            let mut bytes = if prefixed {
                b"host-preset-header".to_vec()
            } else {
                Vec::new()
            };
            let start = bytes.len();
            bytes.extend_from_slice(&json);
            self.load_bytes(&bytes, start, usize::MAX)
        }
        fn activate(&mut self, rate: u32) {
            assert!(!self.active);
            let setup = ProcessSetup {
                process_mode: ProcessModes::kRealtime as i32,
                symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
                max_samples_per_block: MAX_FRAMES as i32,
                sample_rate: f64::from(rate),
            };
            // SAFETY: setup is retained through the call; activation matches valid stopped state.
            unsafe {
                assert_eq!(self.processor.setup_processing(&setup), kResultOk);
                assert_eq!(self.component.set_active(1), kResultOk);
            }
            self.active = true;
        }
        fn start(&mut self) {
            assert!(self.active && !self.processing);
            // SAFETY: activation succeeded, and this starts one paired processing session.
            assert_eq!(unsafe { self.processor.set_processing(1) }, kResultOk);
            self.processing = true;
        }
        fn pause(&mut self) {
            assert!(self.processing);
            // SAFETY: paired with the successful processing start, with no overlapping call.
            assert_eq!(unsafe { self.processor.set_processing(0) }, kResultOk);
            self.processing = false;
        }
        fn deactivate(&mut self) {
            if self.processing {
                self.pause();
            }
            if self.active {
                // SAFETY: paired with a successful activation; processing has stopped.
                assert_eq!(unsafe { self.component.set_active(0) }, kResultOk);
                self.active = false;
            }
        }
        fn process(&mut self, input: &[[f32; 2]]) -> Vec<f32> {
            self.process_with_scalar(input, None)
        }
        fn process_with_scalar(
            &mut self,
            input: &[[f32; 2]],
            scalar: Option<(&str, f64)>,
        ) -> Vec<f32> {
            let changes = scalar.map(|(id, plain)| {
                let id = parameter_id(id);
                // SAFETY: controller is retained and the stable parameter ID belongs
                // to the selected Upmixer schema. This only converts the host value.
                let normalized = unsafe { self.controller.plain_param_to_normalized(id, plain) };
                let queue = ScalarQueue::allocate(id, normalized);
                // SAFETY: ownership of each single-interface COM object transfers once.
                let queue = unsafe {
                    VstPtr::<dyn IParamValueQueue>::owned(Box::into_raw(queue).cast()).unwrap()
                };
                let changes = ScalarChanges::allocate(queue);
                // SAFETY: the first/only interface is IParameterChanges.
                unsafe {
                    VstPtr::<dyn IParameterChanges>::owned(Box::into_raw(changes).cast()).unwrap()
                }
            });
            assert!(self.processing && !input.is_empty() && input.len() <= MAX_FRAMES);
            let mut ins: [Vec<f32>; 2] =
                std::array::from_fn(|channel| input.iter().map(|s| s[channel]).collect());
            let mut outs: [Vec<f32>; 6] = std::array::from_fn(|_| vec![f32::NAN; input.len()]);
            let mut in_pointers: [*mut c_void; 2] =
                std::array::from_fn(|c| ins[c].as_mut_ptr().cast());
            let mut out_pointers: [*mut c_void; 6] =
                std::array::from_fn(|c| outs[c].as_mut_ptr().cast());
            let mut input_bus = AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                buffers: in_pointers.as_mut_ptr(),
            };
            let mut output_bus = AudioBusBuffers {
                num_channels: 6,
                silence_flags: 0,
                buffers: out_pointers.as_mut_ptr(),
            };
            // SAFETY: ProcessData consists of POD flags/counts and nullable event/bus pointers.
            let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
            data.process_mode = ProcessModes::kRealtime as i32;
            data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
            data.num_samples = input.len() as i32;
            data.num_inputs = 1;
            data.num_outputs = 1;
            data.inputs = &mut input_bus;
            data.outputs = &mut output_bus;
            if let Some(changes) = &changes {
                // SAFETY: StaticVstPtr is the transparent raw COM pointer; changes
                // and its queue remain owned until after the native callback returns.
                data.input_param_changes = unsafe {
                    std::mem::transmute::<
                        *mut *mut <dyn IParameterChanges as ComInterface>::VTable,
                        StaticVstPtr<dyn IParameterChanges>,
                    >(changes.as_ptr())
                };
            }
            // SAFETY: all disjoint planar buffers and pointer arrays contain num_samples
            // elements and remain live throughout this exclusive synchronous audio callback.
            assert_eq!(unsafe { self.processor.process(&mut data) }, kResultOk);
            let mut output = Vec::with_capacity(input.len() * 6);
            for frame in 0..input.len() {
                for channel in &outs {
                    output.push(channel[frame]);
                }
            }
            assert!(output.iter().all(|sample| sample.is_finite()));
            output
        }
    }
    impl Drop for Native<'_> {
        fn drop(&mut self) {
            self.deactivate();
            // SAFETY: matched initialized component; all owned interfaces are released
            // by field drops before the borrowed Module can be unloaded.
            assert_eq!(unsafe { self.component.terminate() }, kResultOk);
        }
    }
    fn parameter_id(id: &str) -> u32 {
        id.bytes().fold(0_u32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u32::from(byte))
        }) & !(1 << 31)
    }
    fn configured(native: &Native<'_>, bypass: bool, low: bool) -> PluginState {
        let mut state = native.saved();
        state
            .params
            .insert("bypass_all_processing".into(), ParamValue::Bool(bypass));
        state
            .params
            .insert("low_latency".into(), ParamValue::Bool(low));
        state
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
    fn reference(rate: u32, bypass: bool, low: bool) -> Box<dyn sotf_host::plugin::Plugin> {
        let config = serde_json::json!({"speaker_config":"5.1", "bypass_all_processing":bypass,
            "low_latency":low, "fft_size":if low {1024} else {2048}})
        .to_string();
        let mut direct =
            plugins_bridge::create_plugin("Upmixer", 2, f64::from(rate), &config).unwrap();
        direct.initialize(f64::from(rate)).unwrap();
        direct
    }
    fn direct_process(
        direct: &mut dyn sotf_host::plugin::Plugin,
        input: &[[f32; 2]],
        rate: u32,
    ) -> Vec<f32> {
        let flat: Vec<_> = input.iter().flatten().copied().collect();
        let mut output = vec![f32::NAN; input.len() * 6];
        assert_eq!(
            direct
                .process(
                    &flat,
                    &mut output,
                    &sotf_host::plugin::ProcessContext::new(rate, input.len())
                )
                .unwrap(),
            input.len()
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        output
    }
    fn error(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).fold(0.0_f32, |maximum, (a, b)| {
            assert!(a.is_finite() && b.is_finite());
            maximum.max((a - b).abs())
        })
    }

    fn waveform_matrix(module: &Module) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for bypass in [false, true] {
                for low in [false, true] {
                    let mut native = Native::new(module);
                    let snapshot = configured(&native, bypass, low);
                    assert_eq!(native.load(&snapshot, true), kResultOk);
                    native.activate(rate);
                    let latency = if bypass {
                        0
                    } else if low {
                        1024
                    } else {
                        2048
                    };
                    assert_eq!(native.latency(), latency);
                    if !bypass {
                        assert!(
                            native
                                .notifications()
                                .iter()
                                .any(|f| f & RestartFlags::kLatencyChanged as i32 != 0)
                        );
                    }
                    let mut direct = reference(rate, bypass, low);
                    assert_eq!(direct.latency_samples(), latency as usize);
                    native.start();
                    let mut position = 0;
                    let mut max_error = 0.0_f32;
                    let mut energy = 0.0_f64;
                    for requested in [1, 17, 257, 63, 2, 191, 256, 7].into_iter().cycle() {
                        let frames = requested.min(12_291 - position);
                        let input = program(position, frames);
                        let expected = direct_process(direct.as_mut(), &input, rate);
                        let actual = native.process(&input);
                        max_error = max_error.max(error(&actual, &expected));
                        for (index, &sample) in actual.iter().enumerate() {
                            energy += f64::from(sample).powi(2);
                            if position + index / 6 < latency as usize {
                                assert_eq!(sample, 0.0);
                            }
                            if bypass {
                                assert_eq!(
                                    sample,
                                    if index % 6 < 2 {
                                        input[index / 6][index % 6]
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
                    assert!(energy > 0.001);
                    assert!(max_error <= 2e-6);
                    native.notifications();
                    results.push(serde_json::json!({"rate":rate,"bypass":bypass,"low_latency":low,"latency":latency,"frames":position,"maximum_error":max_error}));
                }
            }
        }
        results
    }
    fn refused_loads(module: &Module) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        for processing in [false, true] {
            let mut native = Native::new(module);
            let mut control = Native::new(module);
            for instance in [&mut native, &mut control] {
                instance.activate(48_000);
                instance.start();
                for i in (0..4096).step_by(256) {
                    instance.process(&program(i, 256));
                }
                if !processing {
                    instance.pause();
                }
            }
            let before = native.saved();
            let notices = native.notifications();
            let mut structural = configured(&native, true, true);
            structural
                .params
                .insert("gain_front_direct".into(), ParamValue::F32(0.7));
            let mut scalar = native.saved();
            scalar
                .params
                .insert("gain_front_direct".into(), ParamValue::F32(0.7));
            let mut sparse = native.saved();
            sparse.params.clear();
            sparse
                .params
                .insert("gain_front_direct".into(), ParamValue::F32(0.7));
            for snapshot in [&before, &structural, &scalar, &sparse] {
                assert_eq!(native.load(snapshot, false), kResultFalse);
                assert_eq!(state_value(&native.saved()), state_value(&before));
                assert_eq!(native.latency(), 2048);
                assert_eq!(native.notifications(), notices);
            }
            if !processing {
                native.start();
                control.start();
            }
            let mut max_error = 0.0_f32;
            for i in (4096..12_288).step_by(256) {
                let input = program(i, 256);
                max_error = max_error.max(error(&native.process(&input), &control.process(&input)));
            }
            assert_eq!(max_error, 0.0);
            native.deactivate();
            assert_eq!(native.load(&structural, true), kResultOk);
            assert_eq!(state_value(&native.saved()), state_value(&structural));
            assert_eq!(native.latency(), 0);
            let after = native.notifications();
            assert_eq!(after.len(), notices.len() + 1);
            assert_ne!(
                after.last().unwrap() & RestartFlags::kLatencyChanged as i32,
                0
            );
            native.activate(96_000);
            assert_eq!(native.latency(), 0);
            native.start();
            let input = program(0, 17);
            let actual = native.process(&input);
            for (frame, source) in actual.as_chunks::<6>().0.iter().zip(input) {
                assert_eq!(frame, &[source[0], source[1], 0.0, 0.0, 0.0, 0.0]);
            }
            results.push(serde_json::json!({"processing":processing,"snapshot_shapes":4,"history_error":max_error,"exact_inactive_retry_latency":0}));
        }
        results
    }
    fn invalid_streams(module: &Module) {
        let native = Native::new(module);
        let bytes = native.saved_bytes();
        let before = state_value(&native.saved());
        for input in [&bytes[..0], &bytes[..bytes.len() - 1], b"{"] {
            assert_eq!(native.load_bytes(input, 0, usize::MAX), kResultFalse);
            assert_eq!(state_value(&native.saved()), before);
        }
        assert_eq!(native.load_bytes(&bytes, 0, 1), kResultFalse);
        assert_eq!(state_value(&native.saved()), before);
    }
    fn scalar_automation(module: &Module) -> serde_json::Value {
        use sotf_host::parameters::{ParameterId, ParameterValue};
        let mut native = Native::new(module);
        let mut control = Native::new(module);
        let mut direct = reference(48_000, false, false);
        native.activate(48_000);
        native.start();
        control.activate(48_000);
        control.start();
        let mut max_error = 0.0_f32;
        let mut changed = 0.0_f32;
        for i in (0..12_288).step_by(256) {
            if i == 4096 {
                direct
                    .set_parameter(
                        ParameterId::from("gain_front_direct"),
                        ParameterValue::Float(1.3),
                    )
                    .unwrap();
            }
            // Keep the program nonzero after the control event for this test.
            let input = program(i % 4096, 256);
            let actual = native
                .process_with_scalar(&input, (i == 4096).then_some(("gain_front_direct", 1.3)));
            let expected = direct_process(direct.as_mut(), &input, 48_000);
            let unchanged = control.process(&input);
            max_error = max_error.max(error(&actual, &expected));
            if i >= 4096 {
                changed = changed.max(error(&actual, &unchanged));
            }
        }
        assert!(max_error <= 2e-6);
        assert!(changed > 1e-5);
        assert_eq!(native.latency(), 2048);
        native.notifications();
        serde_json::json!({"maximum_reference_error":max_error,"maximum_change_from_unautomated":changed,"latency":2048})
    }
    /// Run native library lifecycle and independent waveform checks on the OS main thread.
    pub fn run() {
        // SAFETY: pthread_main_np has no pointer arguments or side effects.
        assert_ne!(
            unsafe { pthread_main_np() },
            0,
            "run this executable on the OS main thread"
        );
        let path = std::env::args_os()
            .nth(1)
            .expect("usage: upmixer_vst3_host <libplugins_nih.dylib>");
        let report = {
            let module = Module::open(Path::new(&path));
            let waveforms = waveform_matrix(&module);
            let refused = refused_loads(&module);
            invalid_streams(&module);
            let scalar = scalar_automation(&module);
            serde_json::json!({"waveforms":waveforms,"refused_loads":refused,"malformed_stream_cases":4,"scalar_automation":scalar,"os_main_thread":true})
        }; // Every plugin/factory COM reference is gone before bundleExit/dlclose.
        println!("SOTF_VST3_RESULT {report}");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    host::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This native VST3 QA host currently supports macOS.");
    std::process::exit(2);
}
