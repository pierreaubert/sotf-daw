//! Exercise restart-required Dynamic EQ controls through the native VST3 callbacks.

use super::vst3;
use nih_plug::wrapper::vst3::vst3_sys;
use sotf_host::ParametricInPlacePlugin;
use sotf_plugins::plugin_dynamic_eq::DynEqShape;
use sotf_plugins::{DynamicEqPlugin, DynamicEqPluginParams, ProcessContext};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use vst3_sys as vst3_com;
use vst3_sys::base::{
    IBStream, IPluginBase, IPluginFactory, IPluginFactory3, IUnknown, kIBSeekCur, kIBSeekEnd,
    kIBSeekSet, kInvalidArgument, kResultFalse, kResultOk, tresult,
};
use vst3_sys::gui::linux::{FileDescriptor, IEventHandler, IRunLoop, ITimerHandler, TimerInterval};
use vst3_sys::utils::{SharedVstPtr, VstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, IAudioProcessor, IComponent, IComponentHandler, IEditController,
    ParameterFlags, ParameterInfo, ProcessData, ProcessModes, ProcessSetup, RestartFlags,
    SymbolicSampleSizes,
};
use vst3_sys::{ComInterface, VST3};

const FRAMES: usize = 128;
const SAMPLE_RATE: f64 = 48_000.0;

#[derive(Default)]
struct RunLoopState {
    registered: Mutex<Vec<(usize, FileDescriptor)>>,
    register_calls: AtomicUsize,
    unregister_calls: AtomicUsize,
}

impl RunLoopState {
    fn pump(&self) {
        let handlers = self.registered.lock().unwrap().clone();
        for (address, fd) in handlers {
            // SAFETY: an entry is recorded only while the plugin owns the handler COM object.
            // Tests call `pump()` on the same thread that supplied this run loop to the factory.
            let handler = unsafe {
                VstPtr::<dyn IEventHandler>::shared(address as *mut _)
                    .expect("registered handler stays live until unregister")
            };
            unsafe { handler.on_fd_is_set(fd) };
        }
    }
}

#[VST3(implements(IRunLoop))]
struct TestRunLoop {
    state: Arc<RunLoopState>,
    register_result: tresult,
}

impl TestRunLoop {
    fn accepting() -> (Box<Self>, Arc<RunLoopState>) {
        Self::with_result(kResultOk)
    }

    fn refusing() -> (Box<Self>, Arc<RunLoopState>) {
        Self::with_result(kResultFalse)
    }

    fn with_result(register_result: tresult) -> (Box<Self>, Arc<RunLoopState>) {
        let state = Arc::new(RunLoopState::default());
        (Self::allocate(Arc::clone(&state), register_result), state)
    }
}

impl IRunLoop for TestRunLoop {
    unsafe fn register_event_handler(
        &self,
        mut handler: SharedVstPtr<dyn IEventHandler>,
        fd: FileDescriptor,
    ) -> tresult {
        self.state.register_calls.fetch_add(1, Ordering::AcqRel);
        if self.register_result == kResultOk {
            self.state
                .registered
                .lock()
                .unwrap()
                .push((handler.as_ptr() as usize, fd));
        }
        self.register_result
    }

    unsafe fn unregister_event_handler(
        &self,
        mut handler: SharedVstPtr<dyn IEventHandler>,
    ) -> tresult {
        self.state.unregister_calls.fetch_add(1, Ordering::AcqRel);
        let address = handler.as_ptr() as usize;
        self.state
            .registered
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

#[derive(Clone)]
struct RestartCall {
    flags: i32,
    thread: ThreadId,
    result: tresult,
}

#[derive(Default)]
struct HandlerState {
    calls: Vec<RestartCall>,
    responses: VecDeque<tresult>,
    main_thread: Option<ThreadId>,
}

#[VST3(implements(IComponentHandler))]
struct TestComponentHandler {
    state: Arc<Mutex<HandlerState>>,
}

impl TestComponentHandler {
    fn new(responses: impl IntoIterator<Item = tresult>) -> (Box<Self>, Arc<Mutex<HandlerState>>) {
        let state = Arc::new(Mutex::new(HandlerState {
            calls: Vec::new(),
            responses: responses.into_iter().collect(),
            main_thread: Some(thread::current().id()),
        }));
        (Self::allocate(Arc::clone(&state)), state)
    }
}

impl IComponentHandler for TestComponentHandler {
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
        let mut state = self.state.lock().unwrap();
        let result = state.responses.pop_front().unwrap_or(kResultOk);
        state.calls.push(RestartCall {
            flags,
            thread: thread::current().id(),
            result,
        });
        result
    }
}

struct TestInstance {
    component: VstPtr<dyn IComponent>,
    processor: VstPtr<dyn IAudioProcessor>,
    controller: VstPtr<dyn IEditController>,
    active: bool,
    processing: bool,
}

impl TestInstance {
    fn new(factory: &vst3::Factory) -> Self {
        let mut class_info = unsafe { std::mem::zeroed::<vst3_sys::base::PClassInfo>() };
        assert_eq!(
            unsafe { factory.get_class_info(0, &mut class_info) },
            kResultOk
        );

        let mut object = std::ptr::null_mut::<c_void>();
        assert_eq!(
            unsafe {
                factory.create_instance(&class_info.cid, &<dyn IComponent>::IID, &mut object)
            },
            kResultOk
        );
        // SAFETY: `create_instance` returns one owned IComponent reference on success.
        let component = unsafe { VstPtr::<dyn IComponent>::owned(object.cast()).unwrap() };
        let processor = component.cast::<dyn IAudioProcessor>().unwrap();
        let controller = component.cast::<dyn IEditController>().unwrap();
        assert_eq!(
            unsafe { component.initialize(std::ptr::null_mut()) },
            kResultOk
        );
        Self {
            component,
            processor,
            controller,
            active: false,
            processing: false,
        }
    }

    fn set_component_handler(&self, handler: &TestComponentHandler) {
        let mut raw = std::ptr::null_mut::<c_void>();
        assert_eq!(
            unsafe { handler.query_interface(&<dyn IComponentHandler>::IID, &mut raw) },
            kResultOk
        );
        // SAFETY: the query result is borrowed for the synchronous callback; the boxed handler
        // remains alive for the entire test.
        let shared: SharedVstPtr<dyn IComponentHandler> = unsafe { std::mem::transmute(raw) };
        assert_eq!(
            unsafe { self.controller.set_component_handler(shared) },
            kResultOk
        );
    }

    fn setup_and_activate(&mut self, sample_rate: f64) -> bool {
        let setup = ProcessSetup {
            process_mode: ProcessModes::kRealtime as i32,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            max_samples_per_block: FRAMES as i32,
            sample_rate,
        };
        assert_eq!(
            unsafe { self.processor.setup_processing(&setup) },
            kResultOk
        );
        let result = unsafe { self.component.set_active(1) };
        self.active = result == kResultOk;
        if self.active {
            assert_eq!(unsafe { self.processor.set_processing(1) }, kResultOk);
            self.processing = true;
        }
        self.active
    }

    fn deactivate(&mut self) {
        if self.processing {
            assert_eq!(unsafe { self.processor.set_processing(0) }, kResultOk);
            self.processing = false;
        }
        if self.active {
            assert_eq!(unsafe { self.component.set_active(0) }, kResultOk);
            self.active = false;
        }
    }

    fn pause_processing(&mut self) {
        if self.processing {
            assert_eq!(unsafe { self.processor.set_processing(0) }, kResultOk);
            self.processing = false;
        }
    }

    fn resume_processing(&mut self) {
        if !self.processing {
            assert_eq!(unsafe { self.processor.set_processing(1) }, kResultOk);
            self.processing = true;
        }
    }

    fn set_normalized(&self, id: &str, normalized: f64) {
        assert_eq!(
            unsafe {
                self.controller
                    .set_param_normalized(parameter_id(id), normalized)
            },
            kResultOk,
            "set {id}"
        );
    }

    fn normalized(&self, id: &str) -> f64 {
        unsafe { self.controller.get_param_normalized(parameter_id(id)) }
    }

    fn plain(&self, id: &str, normalized: f64) -> f32 {
        unsafe {
            self.controller
                .normalized_param_to_plain(parameter_id(id), normalized) as f32
        }
    }

    fn process_block(&self, first_frame: usize) -> [Vec<f32>; 2] {
        let mut left_input = Vec::with_capacity(FRAMES);
        let mut right_input = Vec::with_capacity(FRAMES);
        for frame in 0..FRAMES {
            let time = (first_frame + frame) as f32 / SAMPLE_RATE as f32;
            let sample = (std::f32::consts::TAU * 120.0 * time).sin() * 0.1;
            left_input.push(sample);
            right_input.push(sample * 0.7);
        }
        let mut left_output = vec![f32::NAN; FRAMES];
        let mut right_output = vec![f32::NAN; FRAMES];
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
        let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
        data.process_mode = ProcessModes::kRealtime as i32;
        data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
        data.num_samples = FRAMES as i32;
        data.num_inputs = 1;
        data.num_outputs = 1;
        data.inputs = &mut input_bus;
        data.outputs = &mut output_bus;
        assert_eq!(unsafe { self.processor.process(&mut data) }, kResultOk);
        assert!(
            left_output
                .iter()
                .chain(&right_output)
                .all(|sample| sample.is_finite())
        );
        [left_output, right_output]
    }

    fn process_blocks(&self, first_frame: usize, count: usize) -> [Vec<f32>; 2] {
        let mut output = [
            Vec::with_capacity(count * FRAMES),
            Vec::with_capacity(count * FRAMES),
        ];
        for block in 0..count {
            let current = self.process_block(first_frame + block * FRAMES);
            output[0].extend_from_slice(&current[0]);
            output[1].extend_from_slice(&current[1]);
        }
        output
    }
}

impl Drop for TestInstance {
    fn drop(&mut self) {
        self.deactivate();
        unsafe { self.component.terminate() };
    }
}

#[VST3(implements(IBStream))]
struct TestMemoryStream {
    bytes: Rc<RefCell<Vec<u8>>>,
    cursor: Cell<usize>,
    writable: bool,
}

impl TestMemoryStream {
    fn new(initial: &[u8], writable: bool) -> (Box<Self>, Rc<RefCell<Vec<u8>>>) {
        let bytes = Rc::new(RefCell::new(initial.to_vec()));
        (
            Self::allocate(Rc::clone(&bytes), Cell::new(0), writable),
            bytes,
        )
    }
}

impl IBStream for TestMemoryStream {
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
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), buffer.cast(), count)
            };
        }
        self.cursor.set(cursor + count);
        if !num_bytes_read.is_null() {
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
        let Some(end) = self.cursor.get().checked_add(count) else {
            return kInvalidArgument;
        };
        let mut bytes = self.bytes.borrow_mut();
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        let cursor = self.cursor.get();
        if count > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(buffer.cast(), bytes.as_mut_ptr().add(cursor), count)
            };
        }
        self.cursor.set(end);
        if !num_bytes_written.is_null() {
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
            unsafe { *result = target as i64 };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        unsafe { *pos = self.cursor.get() as i64 };
        kResultOk
    }
}

fn with_stream(
    bytes: &[u8],
    writable: bool,
    callback: impl FnOnce(SharedVstPtr<dyn IBStream>) -> tresult,
) -> (tresult, Vec<u8>) {
    let (stream, contents) = TestMemoryStream::new(bytes, writable);
    // SAFETY: ownership transfers exactly once; the borrowed callback pointer is used
    // synchronously while the owning VstPtr remains in scope.
    let stream = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap() };
    let shared: SharedVstPtr<dyn IBStream> = unsafe { std::mem::transmute(stream.as_ptr()) };
    let result = callback(shared);
    (result, contents.borrow().clone())
}

fn new_factory_with_context(run_loop: Option<&TestRunLoop>) -> Box<vst3::Factory> {
    let factory = vst3::Factory::new();
    if let Some(run_loop) = run_loop {
        let mut unknown = std::ptr::null_mut::<c_void>();
        assert_eq!(
            unsafe { run_loop.query_interface(&<dyn IUnknown>::IID, &mut unknown) },
            kResultOk
        );
        // SAFETY: the query owns one IUnknown reference, which the Factory clones during the call.
        let unknown = unsafe { VstPtr::<dyn IUnknown>::owned(unknown.cast()).unwrap() };
        assert_eq!(
            unsafe { factory.set_host_context(unknown.as_ptr().cast()) },
            kResultOk
        );
    }
    factory
}

fn set_handler(instance: &TestInstance, handler: &TestComponentHandler) {
    instance.set_component_handler(handler);
}

fn parameter_id(id: &str) -> u32 {
    let mut hash = 0_u32;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    hash & !(1 << 31)
}

fn parameter_info(instance: &TestInstance, id: &str) -> ParameterInfo {
    let wanted = parameter_id(id);
    let count = unsafe { instance.controller.get_parameter_count() };
    for index in 0..count {
        let mut info = unsafe { std::mem::zeroed::<ParameterInfo>() };
        assert_eq!(
            unsafe { instance.controller.get_parameter_info(index, &mut info) },
            kResultOk
        );
        if info.id == wanted {
            return info;
        }
    }
    panic!("missing VST3 parameter {id}");
}

fn direct_shelf_reference(
    first_frame: usize,
    block_count: usize,
    template: &TestInstance,
) -> [Vec<f32>; 2] {
    let mut params = DynamicEqPluginParams {
        threshold: template.plain("threshold", template.normalized("threshold")),
        ratio: template.plain("ratio", template.normalized("ratio")),
        attack_ms: template.plain("attack", template.normalized("attack")),
        release_ms: template.plain("release", template.normalized("release")),
        knee: template.plain("knee", template.normalized("knee")),
        mix: template.plain("mix", template.normalized("mix")),
        link_channels: template.plain("link_channels", template.normalized("link_channels")) >= 0.5,
        ..DynamicEqPluginParams::default()
    };
    let band = &mut params.bands[0];
    band.shape = DynEqShape::LowShelf;
    band.shelf_slope = template.plain(
        "band_0_shelf_slope",
        template.normalized("band_0_shelf_slope"),
    );
    band.frequency = template.plain("band_0_frequency", template.normalized("band_0_frequency"));
    band.q = template.plain("band_0_q", template.normalized("band_0_q"));
    band.gain = template.plain("band_0_gain", template.normalized("band_0_gain"));
    band.band_threshold = template.plain(
        "band_0_band_threshold",
        template.normalized("band_0_band_threshold"),
    );
    band.band_ratio = template.plain(
        "band_0_band_ratio",
        template.normalized("band_0_band_ratio"),
    );

    let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(2, params, SAMPLE_RATE as u32)
        .expect("the direct shelf fixture is valid");
    plugin.initialize(SAMPLE_RATE as u32).unwrap();
    let mut output = [
        Vec::with_capacity(block_count * FRAMES),
        Vec::with_capacity(block_count * FRAMES),
    ];
    for block in 0..block_count {
        let mut interleaved = Vec::with_capacity(FRAMES * 2);
        for frame in 0..FRAMES {
            let time = (first_frame + block * FRAMES + frame) as f32 / SAMPLE_RATE as f32;
            let sample = (std::f32::consts::TAU * 120.0 * time).sin() * 0.1;
            interleaved.extend_from_slice(&[sample, sample * 0.7]);
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

fn assert_same_audio(actual: &[Vec<f32>; 2], expected: &[Vec<f32>; 2]) {
    for channel in 0..2 {
        assert_eq!(actual[channel].len(), expected[channel].len());
        for (frame, (actual, expected)) in
            actual[channel].iter().zip(&expected[channel]).enumerate()
        {
            assert!(
                actual.is_finite(),
                "nonfinite output at channel={channel} frame={frame}"
            );
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "mismatch at channel={channel} frame={frame}: {actual} vs {expected}"
            );
        }
    }
}

fn assert_audio_differs(actual: &[Vec<f32>; 2], expected: &[Vec<f32>; 2]) {
    let square_error: f64 = actual
        .iter()
        .zip(expected)
        .flat_map(|(actual, expected)| actual.iter().zip(expected))
        .map(|(actual, expected)| f64::from(*actual - *expected).powi(2))
        .sum();
    let sample_count = actual[0].len() + actual[1].len();
    assert!(sample_count > 0);
    assert!((square_error / sample_count as f64).sqrt() > 1.0e-3);
}

fn wait_for_calls(run_loop: &RunLoopState, state: &Mutex<HandlerState>, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while state.lock().unwrap().calls.len() < count && Instant::now() < deadline {
        run_loop.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        state.lock().unwrap().calls.len() >= count,
        "host restart request {count} timed out"
    );
}

fn assert_restart_call(state: &Mutex<HandlerState>, index: usize) {
    let state = state.lock().unwrap();
    let call = &state.calls[index];
    assert_eq!(call.flags, RestartFlags::kReloadComponent as i32);
    assert_eq!(call.thread, state.main_thread.unwrap());
}

fn capture_state(instance: &TestInstance) -> Vec<u8> {
    let (result, bytes) = with_stream(&[], true, |stream| unsafe {
        instance.component.get_state(stream)
    });
    assert_eq!(result, kResultOk);
    assert!(!bytes.is_empty());
    bytes
}

fn restore_state(instance: &TestInstance, bytes: &[u8]) {
    let (result, _) = with_stream(bytes, false, |stream| unsafe {
        instance.component.set_state(stream)
    });
    assert_eq!(result, kResultOk);
}

#[test]
fn vst3_structural_shelf_restart_is_deferred_retryable_and_survives_reload() {
    let (run_loop, run_loop_state) = TestRunLoop::accepting();
    let factory = new_factory_with_context(Some(&run_loop));
    let (handler, handler_state) = TestComponentHandler::new([kResultFalse, kResultOk, kResultOk]);
    let (peak_handler, _) = TestComponentHandler::new([]);
    // Host callback objects are declared before instances so their owning boxes outlive every
    // wrapper-held COM reference.
    let mut subject = TestInstance::new(&factory);
    let mut peak = TestInstance::new(&factory);
    set_handler(&subject, &handler);
    set_handler(&peak, &peak_handler);

    for id in ["band_0_shape", "band_0_shelf_slope"] {
        assert_eq!(
            parameter_info(&subject, id).flags & ParameterFlags::kCanAutomate as i32,
            0,
            "{id} must remain a non-automatable structural control"
        );
    }

    for instance in [&subject, &peak] {
        instance.set_normalized("band_0_frequency", 0.5);
        instance.set_normalized("band_0_gain", 0.75);
        instance.set_normalized("band_0_band_threshold", 0.25);
    }
    assert!(subject.setup_and_activate(SAMPLE_RATE));
    assert!(peak.setup_and_activate(SAMPLE_RATE));
    assert_same_audio(&subject.process_blocks(0, 8), &peak.process_blocks(0, 8));

    // VST3 setProcessing(true) resets DSP state. Verify the reset itself leaves two identically
    // prepared Peak instances aligned before introducing the pending structural edit.
    subject.pause_processing();
    peak.pause_processing();
    subject.resume_processing();
    peak.resume_processing();
    assert_same_audio(
        &subject.process_blocks(8 * FRAMES, 8),
        &peak.process_blocks(8 * FRAMES, 8),
    );

    // While a VST3 processor is running, the host sends controller values as process parameter
    // changes instead of calling setParamNormalized directly. Pause processing to model a
    // non-automatable structural control edit, then resume the same prepared graph without a
    // component reload.
    subject.pause_processing();
    peak.pause_processing();
    // Shape is a three-choice index: normalized 0.5 selects choice 1 (Low shelf).
    subject.set_normalized("band_0_shape", 0.5);
    subject.set_normalized("band_0_shelf_slope", 0.75);
    assert_eq!(
        subject.plain("band_0_shape", subject.normalized("band_0_shape")),
        1.0,
        "the direct-core reference below exercises the selected Low shelf"
    );
    assert!(
        handler_state.lock().unwrap().calls.is_empty(),
        "restart ran inline"
    );
    subject.resume_processing();
    peak.resume_processing();
    assert_same_audio(
        &subject.process_blocks(16 * FRAMES, 8),
        &peak.process_blocks(16 * FRAMES, 8),
    );

    // The first callback is refused. Same-value retry must still be deliverable, and an accepted
    // request that the host ignores must also be retryable by a same-value echo.
    wait_for_calls(&run_loop_state, &handler_state, 1);
    assert_restart_call(&handler_state, 0);
    assert_eq!(handler_state.lock().unwrap().calls[0].result, kResultFalse);
    subject.pause_processing();
    peak.pause_processing();
    subject.set_normalized("band_0_shelf_slope", 0.75);
    subject.resume_processing();
    peak.resume_processing();
    wait_for_calls(&run_loop_state, &handler_state, 2);
    assert_restart_call(&handler_state, 1);
    assert_eq!(handler_state.lock().unwrap().calls[1].result, kResultOk);
    subject.pause_processing();
    peak.pause_processing();
    subject.set_normalized("band_0_shape", 0.5);
    subject.resume_processing();
    peak.resume_processing();
    wait_for_calls(&run_loop_state, &handler_state, 3);
    assert_restart_call(&handler_state, 2);
    assert_eq!(handler_state.lock().unwrap().calls[2].result, kResultOk);

    // Even after a successful notification, the host has not yet performed the requested reload.
    // The old prepared Peak graph must continue to render its exact stream until reactivation.
    assert_same_audio(
        &subject.process_blocks(24 * FRAMES, 4),
        &peak.process_blocks(24 * FRAMES, 4),
    );

    // A cutoff valid at 48 kHz is refused at 8 kHz. The requested shape and slope remain stored,
    // while the already prepared plugin remains inactive until a valid retry is set up.
    let requested_shape = subject.normalized("band_0_shape");
    let requested_slope = subject.normalized("band_0_shelf_slope");
    subject.deactivate();
    assert!(!subject.setup_and_activate(8_000.0));
    assert!((subject.normalized("band_0_shape") - requested_shape).abs() < 1.0e-12);
    assert!((subject.normalized("band_0_shelf_slope") - requested_slope).abs() < 1.0e-12);
    assert!(subject.setup_and_activate(SAMPLE_RATE));
    assert!((subject.normalized("band_0_shape") - requested_shape).abs() < 1.0e-12);
    assert!((subject.normalized("band_0_shelf_slope") - requested_slope).abs() < 1.0e-12);

    let shelf_audio = subject.process_blocks(28 * FRAMES, 32);
    let peak_audio = peak.process_blocks(28 * FRAMES, 32);
    let direct_audio = direct_shelf_reference(28 * FRAMES, 32, &subject);
    assert_same_audio(&shelf_audio, &direct_audio);
    assert_audio_differs(&shelf_audio, &peak_audio);

    let saved_state = capture_state(&subject);
    subject.deactivate();
    drop(subject);

    // Simulate a complete VST3 unload/recreate: the factory creates a new wrapper, state is
    // restored through IComponent::set_state, and the same shelf program renders the same audio.
    let (restored_handler, _) = TestComponentHandler::new([]);
    let mut restored = TestInstance::new(&factory);
    restore_state(&restored, &saved_state);
    set_handler(&restored, &restored_handler);
    assert!(restored.setup_and_activate(SAMPLE_RATE));
    assert!((restored.normalized("band_0_shape") - requested_shape).abs() < 1.0e-12);
    assert!((restored.normalized("band_0_shelf_slope") - requested_slope).abs() < 1.0e-12);
    assert_same_audio(
        &restored.process_blocks(60 * FRAMES, 32),
        &direct_shelf_reference(60 * FRAMES, 32, &restored),
    );
}

fn assert_restart_is_not_delivered_without_registered_host_loop(
    factory: &vst3::Factory,
    run_loop_state: Option<&RunLoopState>,
) -> Vec<u8> {
    let (handler, state) = TestComponentHandler::new([]);
    let mut instance = TestInstance::new(factory);
    set_handler(&instance, &handler);
    assert!(instance.setup_and_activate(SAMPLE_RATE));
    instance.pause_processing();
    instance.set_normalized("band_0_shape", 0.5);
    instance.set_normalized("band_0_shelf_slope", 0.75);
    let requested_shape = instance.normalized("band_0_shape");
    let requested_slope = instance.normalized("band_0_shelf_slope");
    assert_eq!(instance.plain("band_0_shape", requested_shape), 1.0);
    instance.resume_processing();

    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        if let Some(run_loop_state) = run_loop_state {
            run_loop_state.pump();
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(state.lock().unwrap().calls.is_empty());
    let saved = capture_state(&instance);
    assert!((instance.normalized("band_0_shape") - requested_shape).abs() < 1.0e-12);
    assert!((instance.normalized("band_0_shelf_slope") - requested_slope).abs() < 1.0e-12);
    saved
}

#[test]
fn vst3_restart_waits_safely_when_host_has_no_usable_run_loop() {
    let factory = new_factory_with_context(None);
    let saved_pending_state =
        assert_restart_is_not_delivered_without_registered_host_loop(&factory, None);

    let (run_loop, state) = TestRunLoop::refusing();
    let factory = new_factory_with_context(Some(&run_loop));
    let _refused_registration_state =
        assert_restart_is_not_delivered_without_registered_host_loop(&factory, Some(&state));
    assert_eq!(state.register_calls.load(Ordering::Acquire), 1);
    assert_eq!(state.unregister_calls.load(Ordering::Acquire), 0);
    assert!(state.registered.lock().unwrap().is_empty());

    // A later instance created by a host with a usable factory IRunLoop can restore the
    // unserviced control state before activation. Initialization applies the saved shelf values
    // directly, while same-value callback retries after a refused request are covered above.
    let (run_loop, run_loop_state) = TestRunLoop::accepting();
    let factory = new_factory_with_context(Some(&run_loop));
    let (handler, handler_state) = TestComponentHandler::new([]);
    let mut retry = TestInstance::new(&factory);
    set_handler(&retry, &handler);
    restore_state(&retry, &saved_pending_state);
    let retry_shape = retry.normalized("band_0_shape");
    let retry_slope = retry.normalized("band_0_shelf_slope");
    assert_eq!(retry.plain("band_0_shape", retry_shape), 1.0);
    assert!(retry.setup_and_activate(SAMPLE_RATE));
    assert!((retry.normalized("band_0_shape") - retry_shape).abs() < 1.0e-12);
    assert!((retry.normalized("band_0_shelf_slope") - retry_slope).abs() < 1.0e-12);
    assert_same_audio(
        &retry.process_blocks(0, 32),
        &direct_shelf_reference(0, 32, &retry),
    );
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        run_loop_state.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(handler_state.lock().unwrap().calls.is_empty());
}
