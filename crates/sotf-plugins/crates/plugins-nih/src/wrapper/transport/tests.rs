//! Exercise the native NIH boundary with a minimal CLAP host and a recording DSP.

// Rust guideline compliant 2026-02-21
use super::*;
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_host_tail, clap_plugin_tail};
use clap_sys::plugin::clap_plugin;
use clap_sys::{
    audio_buffer::clap_audio_buffer, events::*, fixedpoint::*, host::clap_host, process::*,
};
use nih_plug::prelude as nih;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, PluginInfo, PluginResult};
use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<ProcessContext<'static>>>>;
thread_local! {
    static SETUP: RefCell<Option<(Log, u32)>> = const { RefCell::new(None) };
    static INITIALIZE_CALLS: Cell<usize> = const { Cell::new(0) };
}

crate::sotf_nih_plugin!(GeneratedProbe, plugin_type: "Gain", name: "Transport Probe", clap_id: "org.sotf.transport-probe", vst3_class_id: *b"SotfTransport001", channels: 2);

struct Recorder(Log);
impl Plugin for Recorder {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Recorder", "1", "Test")
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
    fn initialize(&mut self, _: f64) -> PluginResult<()> {
        Ok(())
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        // Prepared capacity and uncontended mutex belong to the test observer.
        // Production translation itself does not lock or allocate.
        self.0.lock().unwrap().push(
            ProcessContext::new(context.sample_rate, context.num_frames)
                .with_transport(context.transport),
        );
        output.copy_from_slice(input);
        Ok(context.num_frames)
    }
}

struct NativeProbe {
    inner: GeneratedProbe,
    log: Log,
    factor: u32,
}
impl Default for NativeProbe {
    fn default() -> Self {
        let (log, factor) = SETUP.with(|setup| setup.borrow().as_ref().unwrap().clone());
        Self {
            inner: GeneratedProbe::default(),
            log,
            factor,
        }
    }
}
impl nih::Plugin for NativeProbe {
    const NAME: &'static str = "Native Transport Probe";
    const VENDOR: &'static str = "SOTF";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1";
    const AUDIO_IO_LAYOUTS: &'static [nih::AudioIOLayout] = GeneratedProbe::AUDIO_IO_LAYOUTS;
    const SAMPLE_ACCURATE_AUTOMATION: bool = GeneratedProbe::SAMPLE_ACCURATE_AUTOMATION;
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn nih::Params> {
        self.inner.params()
    }
    fn initialize(
        &mut self,
        audio_io_layout: &nih::AudioIOLayout,
        config: &nih::BufferConfig,
        _: &mut impl nih::InitContext<Self>,
    ) -> bool {
        INITIALIZE_CALLS.with(|calls| calls.set(calls.get() + 1));
        self.inner.sample_rate = config.sample_rate;
        self.inner.max_frames = config.max_buffer_size as usize;
        self.inner.main_input_channels = audio_io_layout
            .main_input_channels
            .map_or(0, |channels| channels.get() as usize);
        self.inner.main_output_channels = audio_io_layout
            .main_output_channels
            .map_or(0, |channels| channels.get() as usize);
        self.inner.aux_output_channels = [0; 3];
        if audio_io_layout.aux_output_ports.len() > self.inner.aux_output_channels.len() {
            return false;
        }
        for (index, channels) in audio_io_layout.aux_output_ports.iter().enumerate() {
            self.inner.aux_output_channels[index] = channels.get() as usize;
        }
        self.inner.aux_output_count = audio_io_layout.aux_output_ports.len();
        let input_channels = self.inner.main_input_channels
            + audio_io_layout
                .aux_input_ports
                .iter()
                .map(|channels| channels.get() as usize)
                .sum::<usize>();
        let output_channels =
            self.inner.main_output_channels + self.inner.aux_output_channels.iter().sum::<usize>();
        self.inner.interleaved_in = vec![0.0; self.inner.max_frames * input_channels];
        self.inner.interleaved_out = vec![0.0; self.inner.max_frames * output_channels];
        self.inner.structural_fingerprint = self.inner.params.structural_fingerprint();
        self.inner.transport = TransportTracker::default();
        let recorder: Box<dyn Plugin> = Box::new(Recorder(self.log.clone()));
        let mut dsp: Box<dyn Plugin> = if self.factor > 1 {
            Box::new(
                sotf_host::AutoOversampledPlugin::new_with_max_frames(
                    recorder,
                    self.factor,
                    self.inner.max_frames,
                )
                .unwrap(),
            )
        } else {
            recorder
        };
        dsp.initialize(self.inner.sample_rate).unwrap();
        self.inner.inner = Some(dsp);
        true
    }
    fn process(
        &mut self,
        buffer: &mut nih::Buffer,
        aux: &mut nih::AuxiliaryBuffers,
        context: &mut impl nih::ProcessContext<Self>,
    ) -> nih::ProcessStatus {
        // Use the generated production Plugin::process, including native extraction.
        self.inner
            .process(buffer, aux, &mut ForwardContext(context.transport()))
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
}
impl nih::ClapPlugin for NativeProbe {
    const CLAP_ID: &'static str = "org.sotf.native-transport-probe";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [nih::ClapFeature] = &[];
}
struct ForwardContext<'a>(&'a nih::Transport);
impl nih::ProcessContext<GeneratedProbe> for ForwardContext<'_> {
    fn plugin_api(&self) -> nih::PluginApi {
        nih::PluginApi::Clap
    }
    fn transport(&self) -> &nih::Transport {
        self.0
    }
    fn execute_background(&self, _: ()) {}
    fn execute_gui(&self, _: ()) {}
    fn next_event(&mut self) -> Option<nih::PluginNoteEvent<GeneratedProbe>> {
        None
    }
    fn send_event(&mut self, _: nih::PluginNoteEvent<GeneratedProbe>) {}
    fn set_latency_samples(&self, _: u32) {}
    fn set_current_voice_capacity(&self, _: u32) {}
}

type Host = NativeHost<NativeProbe>;

struct TailObserver {
    enabled: bool,
    plugin: AtomicPtr<clap_plugin>,
    calls: AtomicUsize,
    observed: AtomicU32,
    in_process: AtomicBool,
}

struct NativeHost<P: nih::ClapPlugin> {
    wrapper: Arc<nih_plug::wrapper::clap::Wrapper<P>>,
    _host: Box<clap_host>,
    tail_observer: Box<TailObserver>,
    log: Log,
    input: [[f32; 1024]; 2],
    output: [[f32; 1024]; 2],
}
impl<P: nih::ClapPlugin> NativeHost<P> {
    fn new(factor: u32) -> Self {
        Self::with_tail_extension(factor, false)
    }
    fn with_tail_extension(factor: u32, enabled: bool) -> Self {
        Self::with_rate_and_tail_extension(factor, enabled, 48_000.0)
    }
    fn with_rate_and_tail_extension(factor: u32, enabled: bool, sample_rate: f64) -> Self {
        unsafe extern "C" fn changed(host: *const clap_host) {
            // SAFETY: the observer and plugin outlive the synchronous callback.
            let observer = unsafe { &*((*host).host_data.cast::<TailObserver>()) };
            assert!(observer.in_process.load(Ordering::Relaxed));
            let plugin = observer.plugin.load(Ordering::Relaxed);
            // Reenter the native tail query while changed() is on the stack.
            // This must observe publication without locking the DSP instance.
            let tail =
                unsafe { ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_TAIL.as_ptr()) }
                    .cast::<clap_plugin_tail>();
            let value = unsafe { ((*tail).get.unwrap())(plugin) };
            observer.observed.store(value, Ordering::Relaxed);
            observer.calls.fetch_add(1, Ordering::Relaxed);
        }
        static HOST_TAIL: clap_host_tail = clap_host_tail {
            changed: Some(changed),
        };
        unsafe extern "C" fn extension(host: *const clap_host, id: *const c_char) -> *const c_void {
            // SAFETY: CLAP supplies a terminated extension ID and our live host.
            let observer = unsafe { &*((*host).host_data.cast::<TailObserver>()) };
            if observer.enabled && unsafe { std::ffi::CStr::from_ptr(id) } == CLAP_EXT_TAIL {
                (&HOST_TAIL as *const clap_host_tail).cast()
            } else {
                std::ptr::null()
            }
        }
        unsafe extern "C" fn request(_: *const clap_host) {}
        let mut tail_observer = Box::new(TailObserver {
            enabled,
            plugin: AtomicPtr::new(std::ptr::null_mut()),
            calls: AtomicUsize::new(0),
            observed: AtomicU32::new(0),
            in_process: AtomicBool::new(false),
        });
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: (&mut *tail_observer as *mut TailObserver).cast(),
            name: c"Test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(extension),
            request_restart: Some(request),
            request_process: Some(request),
            request_callback: Some(request),
        });
        let log = Arc::new(Mutex::new(Vec::with_capacity(128)));
        // Prepare the observer's native mutex resource before measuring audio,
        // including platforms that lazily initialize it on the first lock.
        drop(log.lock().unwrap());
        SETUP.with(|setup| *setup.borrow_mut() = Some((log.clone(), factor)));
        // SAFETY: stable host allocation outlives the NIH wrapper; callbacks are valid.
        let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<P>::new(&*host) };
        SETUP.with(|setup| *setup.borrow_mut() = None);
        let plugin = wrapper.clap_plugin.as_ptr();
        tail_observer.plugin.store(plugin, Ordering::Relaxed);
        // SAFETY: valid NIH plugin, control-thread lifecycle in CLAP order.
        unsafe {
            assert!(((*plugin).init.unwrap())(plugin));
            assert!(((*plugin).activate.unwrap())(plugin, sample_rate, 1, 1024));
            assert!(((*plugin).start_processing.unwrap())(plugin));
        }
        Self {
            wrapper,
            _host: host,
            tail_observer,
            log,
            input: [[0.0; 1024]; 2],
            output: [[0.0; 1024]; 2],
        }
    }
    fn process(&mut self, frames: usize, transport: Option<&clap_event_transport>) {
        self.process_events(frames, transport, &[]);
    }

    fn process_events(
        &mut self,
        frames: usize,
        transport: Option<&clap_event_transport>,
        events: &[clap_event_param_value],
    ) -> clap_process_status {
        assert!(frames <= 1024);
        // The list and borrowed event storage remain live for the synchronous
        // CLAP callback. Event offsets are checked by each test case.
        unsafe extern "C" fn event_count(list: *const clap_input_events) -> u32 {
            // SAFETY: this callback is only used with the slice reference below.
            let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
            events.len() as u32
        }
        unsafe extern "C" fn event_at(
            list: *const clap_input_events,
            index: u32,
        ) -> *const clap_event_header {
            // SAFETY: same borrowed slice; out-of-range requests return null.
            let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
            events
                .get(index as usize)
                .map_or(std::ptr::null(), |event| &event.header)
        }
        let mut event_slice = events;
        let event_list = clap_input_events {
            ctx: (&mut event_slice as *mut &[clap_event_param_value]).cast(),
            size: Some(event_count),
            get: Some(event_at),
        };
        let mut inputs = [self.input[0].as_mut_ptr(), self.input[1].as_mut_ptr()];
        let mut outputs = [self.output[0].as_mut_ptr(), self.output[1].as_mut_ptr()];
        let input = clap_audio_buffer {
            data32: inputs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut output = clap_audio_buffer {
            data32: outputs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let process = clap_process {
            steady_time: -1,
            frames_count: frames as u32,
            transport: transport.map_or(std::ptr::null(), |value| value),
            audio_inputs: &input,
            audio_outputs: &mut output,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &event_list,
            out_events: std::ptr::null(),
        };
        let plugin = self.wrapper.clap_plugin.as_ptr();
        // SAFETY: all channel buffers and transport are live for this synchronous
        // callback; their sizes/layout match the activated plugin.
        self.tail_observer.in_process.store(true, Ordering::Relaxed);
        let status = assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        });
        self.tail_observer
            .in_process
            .store(false, Ordering::Relaxed);
        assert_ne!(status, CLAP_PROCESS_ERROR);
        status
    }
    fn reset(&mut self) {
        let plugin = self.wrapper.clap_plugin.as_ptr();
        // SAFETY: valid activated plugin; this test has no concurrent process call.
        unsafe {
            ((*plugin).reset.unwrap())(plugin);
        }
    }
}
impl<P: nih::ClapPlugin> Drop for NativeHost<P> {
    fn drop(&mut self) {
        let plugin = self.wrapper.clap_plugin.as_ptr();
        // SAFETY: match successful start/activate. Arc owns destruction, so the
        // CLAP destroy callback (which consumes a leaked Arc) is not called.
        unsafe {
            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).deactivate.unwrap())(plugin);
        }
    }
}

#[path = "tail_tests.rs"]
mod tail_tests;

#[path = "automation_tests.rs"]
mod automation;

fn native(seconds: f64, beats: f64) -> clap_event_transport {
    clap_event_transport {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_transport>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_TRANSPORT,
            flags: 0,
        },
        flags: CLAP_TRANSPORT_HAS_TEMPO
            | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
            | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
            | CLAP_TRANSPORT_HAS_TIME_SIGNATURE
            | CLAP_TRANSPORT_IS_PLAYING
            | CLAP_TRANSPORT_IS_RECORDING
            | CLAP_TRANSPORT_IS_LOOP_ACTIVE,
        song_pos_beats: (beats * CLAP_BEATTIME_FACTOR as f64).round() as i64,
        song_pos_seconds: (seconds * CLAP_SECTIME_FACTOR as f64).round() as i64,
        tempo: 90.0,
        tempo_inc: 0.0,
        loop_start_beats: 4 * CLAP_BEATTIME_FACTOR,
        loop_end_beats: 16 * CLAP_BEATTIME_FACTOR,
        loop_start_seconds: 2 * CLAP_SECTIME_FACTOR,
        loop_end_seconds: 10 * CLAP_SECTIME_FACTOR,
        bar_start: 0,
        bar_number: 0,
        tsig_num: 7,
        tsig_denom: 8,
    }
}

#[test]
fn actual_native_context_preserves_independent_origins_flags_and_seeks() {
    let mut host = Host::new(1);
    for (frames, seconds, ppq) in [
        (17, 3.0, 19.5),
        (63, 3.0 + 17.0 / 48_000.0, 19.5 + 17.0 / 32_000.0),
        (1, 2.0, 8.0),
    ] {
        host.process(frames, Some(&native(seconds, ppq)));
    }
    let mut stopped = native(2.0, 8.0);
    stopped.flags &=
        !(CLAP_TRANSPORT_IS_PLAYING | CLAP_TRANSPORT_IS_RECORDING | CLAP_TRANSPORT_IS_LOOP_ACTIVE);
    host.process(3, Some(&stopped));
    let log = host.log.lock().unwrap();
    for (index, (sample, ppq, frames)) in [
        (144_000, 19.5, 17),
        (144_017, 19.5 + 17.0 / 32_000.0, 63),
        (96_000, 8.0, 1),
        (96_000, 8.0, 3),
    ]
    .into_iter()
    .enumerate()
    {
        let context = log[index];
        assert_eq!(context.transport.sample_position, sample);
        assert!((context.transport.ppq_position - ppq).abs() < 1e-8);
        assert_eq!(context.num_frames, frames);
        assert_eq!(context.sample_rate, 48_000);
        assert_eq!(context.transport.bpm, 90.0);
        assert_eq!(
            context.transport.time_signature,
            TimeSignature {
                numerator: 7,
                denominator: 8
            }
        );
        assert_eq!(context.transport.playing, index < 3);
        assert_eq!(context.transport.recording, index < 3);
        assert_eq!(context.transport.looping, index < 3);
        assert_eq!(
            context.transport.loop_range,
            if index < 3 {
                LoopRange::new(96_000, 480_000)
            } else {
                None
            }
        );
    }
}

#[test]
fn native_clap_fractional_rate_reaches_dsp_and_transport_without_integer_rounding() {
    for sample_rate in [1_234.567_8, 12_345.678, 48_000.0] {
        let mut host = Host::with_rate_and_tail_extension(1, false, sample_rate);
        host.process(17, Some(&native(0.0, 0.0)));
        let mut fallback = native(0.0, 0.0);
        fallback.flags &= !(CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_HAS_BEATS_TIMELINE);
        host.process(63, Some(&fallback));

        let log = host.log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].sample_rate, sample_rate);
        assert_eq!(log[1].sample_rate, sample_rate);
        assert_eq!(log[1].transport.sample_position, 17);
        let expected_ppq = 17.0 / sample_rate * 90.0 / 60.0;
        assert!((log[1].transport.ppq_position - expected_ppq).abs() < 1e-10);
    }
}

#[test]
fn native_clap_rejects_invalid_rates_before_plugin_initialization() {
    let host = Host::new(1);
    let plugin = host.wrapper.clap_plugin.as_ptr();
    let initialized_before_invalid_rates = INITIALIZE_CALLS.with(Cell::get);
    // SAFETY: the host owns this live plugin and the test uses CLAP lifecycle
    // calls serially on the control thread.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
        for sample_rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(!((*plugin).activate.unwrap())(plugin, sample_rate, 1, 1024));
        }
        assert_eq!(INITIALIZE_CALLS.with(Cell::get), initialized_before_invalid_rates);
        assert!(((*plugin).activate.unwrap())(plugin, 48_000.0, 1, 1024));
        assert_eq!(INITIALIZE_CALLS.with(Cell::get), initialized_before_invalid_rates + 1);
        assert!(((*plugin).start_processing.unwrap())(plugin));
    }
}

#[test]
fn actual_native_missing_positions_advance_hold_and_reset() {
    let mut host = Host::new(1);
    host.process(17, Some(&native(3.0, 19.5)));
    let mut missing = native(0.0, 0.0);
    missing.flags = CLAP_TRANSPORT_IS_PLAYING;
    host.process(63, Some(&missing));
    host.process(1, Some(&missing));
    missing.flags = 0;
    host.process(257, Some(&missing));
    host.process(257, None);
    {
        let log = host.log.lock().unwrap();
        for (index, expected) in [144_000, 144_017, 144_080, 144_081, 144_081]
            .into_iter()
            .enumerate()
        {
            assert_eq!(log[index].transport.sample_position, expected);
            assert!(
                (log[index].transport.ppq_position
                    - (19.5 + (expected - 144_000) as f64 / 32_000.0))
                    .abs()
                    < 1e-8
            );
        }
    }
    host.reset();
    host.process(1, None);
    assert_eq!(
        host.log
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .transport
            .sample_position,
        0
    );
}

#[test]
fn actual_native_tempo_signature_and_available_clock_updates_are_forwarded() {
    let mut host = Host::new(1);
    host.process(48, Some(&native(1.0, 23.0)));
    let mut update = native(0.0, 0.0);
    update.flags =
        CLAP_TRANSPORT_IS_PLAYING | CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
    update.tempo = 150.0;
    update.tsig_num = 3;
    update.tsig_denom = 4;
    host.process(96, Some(&update));
    host.process(1, Some(&update));
    // A seconds-only native clock is converted by NIH to samples and beats.
    update.flags |= CLAP_TRANSPORT_HAS_SECONDS_TIMELINE;
    update.song_pos_seconds = 2 * CLAP_SECTIME_FACTOR;
    host.process(7, Some(&update));
    // A beats-only clock likewise reaches the DSP through NIH's public API.
    update.flags &= !CLAP_TRANSPORT_HAS_SECONDS_TIMELINE;
    update.flags |= CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
    update.song_pos_beats = 10 * CLAP_BEATTIME_FACTOR;
    host.process(17, Some(&update));
    let log = host.log.lock().unwrap();
    assert_eq!(log[1].transport.sample_position, 48_048);
    assert!((log[1].transport.ppq_position - 23.0015).abs() < 1e-12);
    assert!((log[2].transport.ppq_position - 23.0065).abs() < 1e-12);
    assert_eq!(log[1].transport.bpm, 150.0);
    assert_eq!(
        log[1].transport.time_signature,
        TimeSignature {
            numerator: 3,
            denominator: 4
        }
    );
    assert_eq!(log[3].transport.sample_position, 96_000);
    assert_eq!(log[3].transport.ppq_position, 5.0);
    assert_eq!(log[4].transport.sample_position, 192_000);
    assert_eq!(log[4].transport.ppq_position, 10.0);
}

#[test]
fn actual_native_origins_survive_oversampling_and_partial_callbacks() {
    for factor in [2, 4] {
        let mut host = Host::new(factor);
        let mut position = 0;
        for frames in [17, 63, 257, 7, 1024] {
            host.process(
                frames,
                Some(&native(
                    3.0 + position as f64 / 48_000.0,
                    19.5 + position as f64 / 32_000.0,
                )),
            );
            position += frames;
        }
        let log = host.log.lock().unwrap();
        assert_eq!(log.len(), position / 256);
        for (chunk, context) in log.iter().enumerate() {
            assert_eq!(context.sample_rate, 48_000 * factor);
            assert_eq!(context.num_frames, 256 * factor as usize);
            assert_eq!(
                context.transport.sample_position,
                (144_000 + chunk as u64 * 256) * u64::from(factor)
            );
            assert!(
                (context.transport.ppq_position - (19.5 + chunk as f64 * 256.0 / 32_000.0)).abs()
                    < 1e-7
            );
            assert_eq!(
                context.transport.loop_range,
                LoopRange::new(96_000 * u64::from(factor), 480_000 * u64::from(factor))
            );
            assert!(
                context.transport.playing
                    && context.transport.recording
                    && context.transport.looping
            );
        }
    }
}

#[test]
fn malformed_values_preroll_and_overflow_keep_fallback_bounded() {
    let mut tracker = TransportTracker::default();
    let mut native = NativeTransport {
        playing: true,
        sample_position: Some(-17),
        ppq_position: Some(-2.0),
        bpm: Some(60.0),
        loop_range: Some((-1, 20)),
        ..Default::default()
    };
    let first = tracker.context(native, 48_000, 7);
    assert_eq!(first.transport.sample_position, 0);
    assert_eq!(first.transport.ppq_position, -2.0);
    assert_eq!(first.transport.loop_range, LoopRange::new(0, 20));
    native.sample_position = None;
    native.ppq_position = Some(f64::NAN);
    native.bpm = Some(f64::INFINITY);
    native.time_signature = Some((0, 999));
    native.loop_range = Some((20, 10));
    let second = tracker.context(native, 48_000, 17);
    assert_eq!(second.transport.sample_position, 0);
    assert_eq!(second.transport.bpm, 60.0);
    assert!((second.transport.ppq_position - (-2.0 + 7.0 / 48_000.0)).abs() < 1e-12);
    assert_eq!(second.transport.time_signature, TimeSignature::default());
    assert_eq!(second.transport.loop_range, None);
    assert_eq!(
        tracker.context(native, 48_000, 1).transport.sample_position,
        7
    );
    native.sample_position = Some(i64::MAX - 1);
    tracker.context(native, 48_000, usize::MAX);
    native.sample_position = None;
    let saturated = tracker.context(native, 48_000, usize::MAX);
    assert_eq!(saturated.transport.sample_position, i64::MAX as u64);
    assert!(saturated.transport.ppq_position.is_finite());
}
