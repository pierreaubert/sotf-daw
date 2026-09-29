//! Native tail queries use configuration bounds across processing lifecycle changes.

// Rust guideline compliant 2026-02-21
use super::*;
use sotf_host::TailLength;
use std::sync::atomic::AtomicU64;

crate::sotf_nih_plugin!(NativeDelay, plugin_type: "Delay", name: "Delay Tail Probe", clap_id: "org.sotf.delay-tail-probe", vst3_class_id: *b"SotfDelayTail001", channels: 2);
crate::sotf_nih_plugin!(NativeConvolution, plugin_type: "Convolution", name: "Convolution Tail Probe", clap_id: "org.sotf.convolution-tail-probe", vst3_class_id: *b"SotfConvTail0001", channels: 2);

#[test]
fn actual_convolution_native_initial_tail_includes_buffered_dry_output() {
    let mut host = NativeHost::<NativeConvolution>::with_tail_extension(1, true);
    assert_eq!(
        host.tail_samples(),
        1024,
        "initialized before first callback"
    );
    let plugin = host.wrapper.clap_plugin.as_ptr();
    // SAFETY: the live native plugin owns its immutable latency extension table.
    let latency = unsafe {
        use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
        let ext = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_LATENCY.as_ptr())
            .cast::<clap_plugin_latency>();
        ((*ext).get.unwrap())(plugin)
    };
    assert_eq!(latency, 1024, "tail includes physical delay once");
    let mut position = 0;
    for frames in [1, 17, 255, 511, 240, 1, 31] {
        host.input[0].fill(0.0);
        host.input[1].fill(0.0);
        if position == 0 {
            host.input[0][0] = 0.75;
            host.input[1][0] = -0.5;
        }
        assert_eq!(host.process_events(frames, None, &[]), CLAP_PROCESS_TAIL);
        for frame in 0..frames {
            for (channel, amplitude) in [(0, 0.75), (1, -0.5)] {
                let expected = if position + frame == 1024 {
                    amplitude
                } else {
                    0.0
                };
                assert_eq!(host.output[channel][frame], expected);
            }
        }
        position += frames;
    }
    host.reset();
    assert_eq!(host.tail_samples(), 1024);
    host.restart_processing();
    assert_eq!(host.tail_samples(), 1024);
}

#[test]
fn actual_gain_has_no_tail_before_or_after_native_processing() {
    let mut host = NativeHost::<GeneratedProbe>::with_tail_extension(1, true);
    assert_eq!(host.tail_samples(), 0);
    assert_eq!(
        host.process_events(17, None, &[]),
        CLAP_PROCESS_CONTINUE_IF_NOT_QUIET
    );
    host.reset();
    assert_eq!(host.tail_samples(), 0);
}

#[test]
fn actual_delay_native_tail_keeps_quiet_gap_and_final_echo() {
    use nih_plug::wrapper::state::ParamValue;
    let mut host = NativeHost::<NativeDelay>::with_tail_extension(1, true);
    assert_eq!(
        host.tail_samples(),
        u32::MAX,
        "default feedback is recursive"
    );
    let mut state = host.wrapper.get_state_object();
    state.params.insert("delay_ms".into(), ParamValue::F32(3.0));
    state.params.insert("feedback".into(), ParamValue::F32(0.0));
    state.params.insert("mix".into(), ParamValue::F32(1.0));
    assert!(host.wrapper.set_state_inner(&mut state));
    let bound = host.tail_samples();
    assert!((144..i32::MAX as u32).contains(&bound));
    let mut position = 0;
    let mut final_echo = false;
    for requested in [1, 23, 1, 17, 255, 511, 1024].into_iter().cycle() {
        let frames = requested.min(1 + bound as usize - position);
        if frames == 0 {
            break;
        }
        host.input[0].fill(0.0);
        host.input[1].fill(0.0);
        if position == 0 {
            host.input[0][0] = 0.75;
            host.input[1][0] = -0.5;
        }
        assert_eq!(host.process_events(frames, None, &[]), CLAP_PROCESS_TAIL);
        for frame in 0..frames {
            for (channel, amplitude) in [(0, 0.75), (1, -0.5)] {
                let expected = if position + frame == 144 {
                    amplitude
                } else {
                    0.0
                };
                assert!(
                    (host.output[channel][frame] - expected).abs() < 1e-5,
                    "delay frame={}, channel={channel}",
                    position + frame
                );
            }
            final_echo |= position + frame == 144 && host.output[0][frame].abs() > 0.7;
        }
        position += frames;
    }
    assert!(
        final_echo,
        "tail scheduling must preserve the delayed impulse after quiet blocks"
    );
    let feedback = host.parameter_id("Feedback");
    let event = super::automation::change(feedback, 0, (0.3 + 0.95) / 1.9);
    assert_eq!(
        host.process_events(17, None, &[event]),
        CLAP_PROCESS_CONTINUE
    );
    assert_eq!(host.tail_samples(), u32::MAX);
    assert_eq!(
        host.tail_observer.observed.load(Ordering::Relaxed),
        u32::MAX
    );
}

thread_local! {
    static TAIL_CONTROL: RefCell<Option<Arc<AtomicU64>>> = const { RefCell::new(None) };
    static RESTORE_BOUND: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    static GUI_CAPTURE: RefCell<Option<GuiCapture>> = const { RefCell::new(None) };
}

type GuiCapture = Arc<Mutex<Option<Arc<dyn nih::GuiContext>>>>;

struct ContextCaptureEditor(GuiCapture);

impl nih::Editor for ContextCaptureEditor {
    fn spawn(
        &self,
        _: nih::ParentWindowHandle,
        context: Arc<dyn nih::GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        *self.0.lock().unwrap() = Some(context);
        Box::new(())
    }
    fn size(&self) -> (u32, u32) {
        (1, 1)
    }
    fn set_scale_factor(&self, _: f32) -> bool {
        true
    }
    fn param_value_changed(&self, _: &str, _: f32) {}
    fn param_modulation_changed(&self, _: &str, _: f32) {}
    fn param_values_changed(&self) {}
}

struct TailDsp(Arc<AtomicU64>);
impl Plugin for TailDsp {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Tail probe", "1", "Test")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        2
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
        Ok(())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn tail_length(&self) -> TailLength {
        match self.0.load(Ordering::Relaxed) {
            u64::MAX => TailLength::Unknown,
            value if value == u64::MAX - 1 => TailLength::Infinite,
            value => TailLength::Finite(value),
        }
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        output.copy_from_slice(input);
        Ok(context.num_frames)
    }
}

struct TailProbe {
    inner: GeneratedProbe,
    control: Arc<AtomicU64>,
    restore_bound: Option<u64>,
}
impl Default for TailProbe {
    fn default() -> Self {
        Self {
            inner: GeneratedProbe::default(),
            control: TAIL_CONTROL.with(|value| value.borrow().as_ref().unwrap().clone()),
            restore_bound: RESTORE_BOUND.get(),
        }
    }
}
impl nih::Plugin for TailProbe {
    const NAME: &'static str = "Native Tail Probe";
    const VENDOR: &'static str = "SOTF";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1";
    const AUDIO_IO_LAYOUTS: &'static [nih::AudioIOLayout] = GeneratedProbe::AUDIO_IO_LAYOUTS;
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn nih::Params> {
        self.inner.params()
    }
    fn editor(&mut self, _: nih::AsyncExecutor<Self>) -> Option<Box<dyn nih::Editor>> {
        GUI_CAPTURE.with(|capture| {
            capture.borrow().as_ref().map(|capture| {
                Box::new(ContextCaptureEditor(Arc::clone(capture))) as Box<dyn nih::Editor>
            })
        })
    }
    fn tail_length(&self) -> Option<u32> {
        nih::Plugin::tail_length(&self.inner)
    }
    fn initialize(
        &mut self,
        _: &nih::AudioIOLayout,
        config: &nih::BufferConfig,
        context: &mut impl nih::InitContext<Self>,
    ) -> bool {
        if self.inner.inner.is_some()
            && let Some(value) = self.restore_bound
        {
            self.control.store(value, Ordering::Relaxed);
        }
        self.inner.sample_rate = config.sample_rate as u32;
        self.inner.max_frames = config.max_buffer_size as usize;
        self.inner
            .interleaved_in
            .resize(self.inner.max_frames * 2, 0.0);
        self.inner
            .interleaved_out
            .resize(self.inner.max_frames * 2, 0.0);
        self.inner.structural_fingerprint = self.inner.params.structural_fingerprint();
        if self.inner.inner.is_none() {
            self.inner.inner = Some(Box::new(TailDsp(self.control.clone())));
        }
        // Independent metadata values prove native tail does not add PDC again.
        context.set_latency_samples(7);
        true
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn process(
        &mut self,
        buffer: &mut nih::Buffer,
        aux: &mut nih::AuxiliaryBuffers,
        context: &mut impl nih::ProcessContext<Self>,
    ) -> nih::ProcessStatus {
        self.inner
            .process(buffer, aux, &mut ForwardContext(context.transport()))
    }
}

#[derive(Default)]
struct LegacyTailProbe;
impl nih::Plugin for LegacyTailProbe {
    const NAME: &'static str = "Legacy Tail Probe";
    const VENDOR: &'static str = "SOTF";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1";
    const AUDIO_IO_LAYOUTS: &'static [nih::AudioIOLayout] = GeneratedProbe::AUDIO_IO_LAYOUTS;
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn nih::Params> {
        Arc::new(crate::params::DynamicParams::from_infos(&[]))
    }
    fn process(
        &mut self,
        _: &mut nih::Buffer,
        _: &mut nih::AuxiliaryBuffers,
        _: &mut impl nih::ProcessContext<Self>,
    ) -> nih::ProcessStatus {
        nih::ProcessStatus::Tail(37)
    }
}
impl nih::ClapPlugin for LegacyTailProbe {
    const CLAP_ID: &'static str = "org.sotf.legacy-tail-probe";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [nih::ClapFeature] = &[];
}

#[test]
fn upstream_plugins_without_explicit_bounds_preserve_legacy_tail_behavior() {
    let mut host = NativeHost::<LegacyTailProbe>::with_tail_extension(1, true);
    assert_eq!(host.tail_samples(), 0);
    assert_eq!(host.process_events(7, None, &[]), CLAP_PROCESS_CONTINUE);
    assert_eq!(host.tail_samples(), 37);
    assert_eq!(host.tail_observer.calls.load(Ordering::Relaxed), 0);
    host.restart_processing();
    assert_eq!(host.tail_samples(), 0);
}

#[test]
fn queued_gui_state_tail_is_published_before_the_same_native_callback_returns() {
    let control = Arc::new(AtomicU64::new(0));
    TAIL_CONTROL.with(|slot| *slot.borrow_mut() = Some(control));
    RESTORE_BOUND.set(Some(91));
    let mut host = NativeHost::<TailProbe>::with_tail_extension(1, true);
    TAIL_CONTROL.with(|slot| *slot.borrow_mut() = None);
    RESTORE_BOUND.set(None);
    assert_eq!(
        host.process_events(1, None, &[]),
        CLAP_PROCESS_CONTINUE_IF_NOT_QUIET
    );
    let wrapper = Arc::clone(&host.wrapper);
    let state = wrapper.get_state_object();
    let gui = std::thread::spawn(move || wrapper.set_state_object_from_gui(state));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut restored = false;
    while std::time::Instant::now() < deadline {
        let status = host.process_events(1, None, &[]);
        if host.tail_samples() == 91 {
            assert_eq!(
                status, CLAP_PROCESS_TAIL,
                "new state must keep this silent plugin scheduled"
            );
            assert_eq!(host.tail_observer.observed.load(Ordering::Relaxed), 91);
            restored = true;
            break;
        }
        std::thread::yield_now();
    }
    if !restored {
        // Avoid stranding the GUI's zero-capacity state handoff on a failed test.
        // SAFETY: no callback is active and the plugin remains owned by host.
        let plugin = host.wrapper.clap_plugin.as_ptr();
        unsafe {
            ((*plugin).stop_processing.unwrap())(plugin);
        }
    }
    gui.join().unwrap();
    assert!(
        restored,
        "GUI state was not received within the test deadline"
    );
}
impl nih::ClapPlugin for TailProbe {
    const CLAP_ID: &'static str = "org.sotf.native-tail-probe";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [nih::ClapFeature] = &[];
}
impl nih::Vst3Plugin for TailProbe {
    const VST3_CLASS_ID: [u8; 16] = *b"SotfTailProbe001";
    const VST3_SUBCATEGORIES: &'static [nih::Vst3SubCategory] = &[nih::Vst3SubCategory::Fx];
}

impl<P: nih::ClapPlugin> NativeHost<P> {
    fn tail_samples(&self) -> u32 {
        let plugin = self.wrapper.clap_plugin.as_ptr();
        // SAFETY: the native plugin owns a live immutable tail extension table.
        unsafe {
            let ext = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_TAIL.as_ptr())
                .cast::<clap_plugin_tail>();
            assert!(!ext.is_null());
            ((*ext).get.unwrap())(plugin)
        }
    }
    fn restart_processing(&mut self) {
        let plugin = self.wrapper.clap_plugin.as_ptr();
        // SAFETY: the test has exclusive access to the activated plugin.
        unsafe {
            ((*plugin).stop_processing.unwrap())(plugin);
            assert!(((*plugin).start_processing.unwrap())(plugin));
        }
    }
}

#[test]
fn clap_tail_query_notifications_and_process_status_follow_configuration() {
    // A new thread exercises the cold native callback, including host callbacks.
    std::thread::spawn(|| {
        for enabled in [false, true] {
            let control = Arc::new(AtomicU64::new(37));
            TAIL_CONTROL.with(|slot| *slot.borrow_mut() = Some(control.clone()));
            let mut host = NativeHost::<TailProbe>::with_tail_extension(1, enabled);
            TAIL_CONTROL.with(|slot| *slot.borrow_mut() = None);
            assert_eq!(host.tail_samples(), 37, "before first callback");
            assert_eq!(host.process_events(17, None, &[]), CLAP_PROCESS_TAIL);
            assert_eq!(
                host.tail_observer.calls.load(Ordering::Relaxed),
                usize::from(enabled)
            );
            assert_eq!(host.process_events(1, None, &[]), CLAP_PROCESS_TAIL);
            assert_eq!(
                host.tail_observer.calls.load(Ordering::Relaxed),
                usize::from(enabled)
            );
            let cases = [
                (81, 81, CLAP_PROCESS_TAIL),
                (u64::MAX - 1, u32::MAX, CLAP_PROCESS_CONTINUE),
                (0, 0, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET),
                (u64::MAX, u32::MAX, CLAP_PROCESS_CONTINUE),
                (i32::MAX as u64, u32::MAX, CLAP_PROCESS_CONTINUE),
                (123, 123, CLAP_PROCESS_TAIL),
            ];
            let mut expected_calls = usize::from(enabled);
            let mut previous = 37;
            for (bound, native, status) in cases {
                control.store(bound, Ordering::Relaxed);
                assert_eq!(host.process_events(63, None, &[]), status);
                assert_eq!(host.tail_samples(), native);
                if enabled && previous != native {
                    expected_calls += 1;
                }
                assert_eq!(
                    host.tail_observer.calls.load(Ordering::Relaxed),
                    expected_calls
                );
                if enabled {
                    assert_eq!(host.tail_observer.observed.load(Ordering::Relaxed), native);
                }
                previous = native;
            }
            host.reset();
            assert_eq!(host.tail_samples(), 123, "reset preserves current bound");
            host.restart_processing();
            assert_eq!(host.tail_samples(), 123, "restart preserves current bound");
            assert_eq!(host.process_events(1, None, &[]), CLAP_PROCESS_TAIL);
            assert_eq!(
                host.tail_observer.calls.load(Ordering::Relaxed),
                expected_calls
            );
            control.store(227, Ordering::Relaxed);
            host.reset();
            assert_eq!(host.tail_samples(), 227);
            assert_eq!(host.process_events(1, None, &[]), CLAP_PROCESS_TAIL);
            assert_eq!(
                host.tail_observer.calls.load(Ordering::Relaxed),
                expected_calls + usize::from(enabled)
            );
        }
    })
    .join()
    .unwrap();
}

#[test]
fn native_tail_conversion_never_truncates_large_or_unknown_bounds() {
    use crate::wrapper::native_tail_samples;
    assert_eq!(native_tail_samples(TailLength::Finite(0)), 0);
    assert_eq!(
        native_tail_samples(TailLength::Finite(i32::MAX as u64 - 1)),
        i32::MAX as u32 - 1
    );
    for tail in [
        TailLength::Finite(i32::MAX as u64),
        TailLength::Finite(u64::MAX),
        TailLength::Infinite,
        TailLength::Unknown,
    ] {
        assert_eq!(native_tail_samples(tail), u32::MAX);
    }
}

#[test]
fn vst3_tail_queries_use_native_interfaces_before_process_and_after_restart() {
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::base::{IPluginBase, IUnknown, kResultOk};
    use vst3_sys::gui::IPlugView;
    use vst3_sys::vst::{
        AudioBusBuffers, IAudioProcessor, IComponent, IEditController, ProcessData, ProcessSetup,
    };
    use vst3_sys::{ComInterface, VstPtr};

    let control = Arc::new(AtomicU64::new(37));
    let capture = Arc::new(Mutex::new(None));
    GUI_CAPTURE.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&capture)));
    TAIL_CONTROL.with(|slot| *slot.borrow_mut() = Some(control.clone()));
    let wrapper = Wrapper::<TailProbe>::new();
    TAIL_CONTROL.with(|slot| *slot.borrow_mut() = None);
    GUI_CAPTURE.with(|slot| *slot.borrow_mut() = None);
    // SAFETY: query_interface returns an owned reference, adopted by VstPtr.
    // Keep the wrapper's original Box reference until all interface references drop.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        let controller = processor.cast::<dyn IEditController>().unwrap();
        let view =
            VstPtr::<dyn IPlugView>::owned(controller.create_view(c"editor".as_ptr()).cast())
                .unwrap();
        // The test editor only captures GuiContext and never dereferences or
        // creates a platform window from this inert host handle.
        #[cfg(target_os = "macos")]
        let platform = c"NSView";
        #[cfg(target_os = "windows")]
        let platform = c"HWND";
        #[cfg(all(unix, not(target_os = "macos")))]
        let platform = c"X11EmbedWindowID";
        assert_eq!(
            view.attached(
                std::ptr::dangling_mut::<std::ffi::c_void>(),
                platform.as_ptr()
            ),
            kResultOk
        );
        let gui_context = capture.lock().unwrap().take().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
        let setup = ProcessSetup {
            process_mode: 0,
            symbolic_sample_size: 0,
            max_samples_per_block: 127,
            sample_rate: 48_000.0,
        };
        assert_eq!(processor.setup_processing(&setup), kResultOk);
        assert_eq!(
            processor.get_tail_samples(),
            u32::MAX,
            "DSP not activated yet"
        );
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(
            processor.get_tail_samples(),
            37,
            "activated before processing"
        );
        assert_eq!(processor.get_latency_samples(), 7);
        assert_eq!(processor.set_processing(1), kResultOk);
        assert_eq!(processor.get_tail_samples(), 37);

        let mut input = [[0.0f32; 127]; 2];
        let mut output = [[0.0f32; 127]; 2];
        let mut in_ptrs = [input[0].as_mut_ptr().cast(), input[1].as_mut_ptr().cast()];
        let mut out_ptrs = [output[0].as_mut_ptr().cast(), output[1].as_mut_ptr().cast()];
        let mut inputs = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 3,
            buffers: in_ptrs.as_mut_ptr(),
        };
        let mut outputs = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: out_ptrs.as_mut_ptr(),
        };
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: 127,
            num_inputs: 1,
            num_outputs: 1,
            inputs: &mut inputs,
            outputs: &mut outputs,
            // SAFETY: StaticVstPtr is a nullable raw interface pointer; these optional lists are absent.
            input_param_changes: std::mem::zeroed(),
            output_param_changes: std::mem::zeroed(),
            input_events: std::mem::zeroed(),
            output_events: std::mem::zeroed(),
            context: std::ptr::null_mut(),
        };
        for (value, expected) in [(81, 81), (u64::MAX, u32::MAX), (0, 0), (123, 123)] {
            control.store(value, Ordering::Relaxed);
            assert_eq!(
                assert_no_alloc::assert_no_alloc(|| processor.process(&mut data)),
                kResultOk
            );
            assert_eq!(processor.get_tail_samples(), expected);
        }
        // Exercise the actual VST3 GUI restoration path with concurrent callers.
        // Each call must receive its retired state before its control lock ends.
        let completed = Arc::new(AtomicUsize::new(0));
        let controls: Vec<_> = (0..4)
            .map(|_| {
                let gui = Arc::clone(&gui_context);
                let completed = Arc::clone(&completed);
                let state = gui.get_state();
                std::thread::spawn(move || {
                    gui.set_state(state);
                    completed.fetch_add(1, Ordering::Release);
                })
            })
            .collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while completed.load(Ordering::Acquire) != controls.len()
            && std::time::Instant::now() < deadline
        {
            assert_eq!(
                assert_no_alloc::assert_no_alloc(|| processor.process(&mut data)),
                kResultOk
            );
            std::thread::yield_now();
        }
        let completed_during_processing = completed.load(Ordering::Acquire);
        // If a request was never accepted, stopping processing lets its existing
        // request timeout fall back to control-thread restoration and retire it.
        assert_eq!(processor.set_processing(0), kResultOk);
        for control in controls {
            control.join().unwrap();
        }
        assert_eq!(
            completed_during_processing, 4,
            "native callbacks must retire every accepted GUI state"
        );
        assert_eq!(completed.load(Ordering::Acquire), 4);
        assert_eq!(processor.get_tail_samples(), 123);
        assert_eq!(processor.set_processing(1), kResultOk);
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);
        assert_eq!(processor.get_tail_samples(), 123);
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(
            processor.get_tail_samples(),
            u32::MAX,
            "deactivated bound is unknown"
        );
        control.store(227, Ordering::Relaxed);
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.get_tail_samples(), 227);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(view.removed(), kResultOk);
        drop(gui_context);
        drop(view);
        drop(controller);
        assert_eq!(component.terminate(), kResultOk);
        drop(component);
        drop(processor);
    }
}
