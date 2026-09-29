//! Discover, select, and process the actual native CLAP key bus.

use super::*;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::ext::audio_ports::{CLAP_EXT_AUDIO_PORTS, clap_plugin_audio_ports};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_plugin_audio_ports_config,
};
use clap_sys::host::clap_host;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use std::ffi::{c_char, c_void};
use std::sync::Arc;

struct NativeKeyGate(GateModeWrapper);

impl Default for NativeKeyGate {
    fn default() -> Self {
        Self(GateModeWrapper {
            params: restored_params(
                "Gate",
                &[
                    ("mode", 2.0),
                    ("sidechain_external", 1.0),
                    ("link_channels", 0.0),
                    ("threshold", -30.0),
                    ("ratio", 3.0),
                    ("range_db", 6.0),
                    ("attack", 0.1),
                    ("hold", 0.0),
                ],
            ),
            ..Default::default()
        })
    }
}

struct InitForward<'a, C>(&'a mut C);
impl<C: InitContext<NativeKeyGate>> InitContext<GateModeWrapper> for InitForward<'_, C> {
    fn plugin_api(&self) -> PluginApi {
        self.0.plugin_api()
    }
    fn execute(&self, _: ()) {}
    fn set_latency_samples(&self, samples: u32) {
        self.0.set_latency_samples(samples);
    }
    fn set_current_voice_capacity(&self, capacity: u32) {
        self.0.set_current_voice_capacity(capacity);
    }
}

impl Plugin for NativeKeyGate {
    const NAME: &'static str = "Gate Native Key Probe";
    const VENDOR: &'static str = "SOTF";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = GateModeWrapper::AUDIO_IO_LAYOUTS;
    const SAMPLE_ACCURATE_AUTOMATION: bool = GateModeWrapper::SAMPLE_ACCURATE_AUTOMATION;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.0.params()
    }
    fn initialize(
        &mut self,
        layout: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.0.initialize(layout, config, &mut InitForward(context))
    }
    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        self.0
            .process_with_transport(buffer, aux, context.transport().into())
    }
    fn reset(&mut self) {
        self.0.reset();
    }
}
impl ClapPlugin for NativeKeyGate {
    const CLAP_ID: &'static str = "org.sotf.gate-native-key-probe";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect];
}
impl Vst3Plugin for NativeKeyGate {
    const VST3_CLASS_ID: [u8; 16] = *b"SotfGateKey00001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
    fn default_audio_io_layout() -> AudioIOLayout {
        GateModeWrapper::default_audio_io_layout()
    }
}

#[test]
fn clap_gate_selects_sidechain_ports_and_processes_separate_key_channels() {
    unsafe extern "C" fn extension(_: *const clap_host, _: *const c_char) -> *const c_void {
        std::ptr::null()
    }
    unsafe extern "C" fn request(_: *const clap_host) {}
    let host = Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF Test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(extension),
        request_restart: Some(request),
        request_process: Some(request),
        request_callback: Some(request),
    });
    // SAFETY: stable host storage and its callback functions outlive the wrapper.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<NativeKeyGate>::new(&*host) };
    let plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: native extension queries and lifecycle calls use the live wrapper.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let configs =
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr())
                .cast::<clap_plugin_audio_ports_config>();
        assert!(!configs.is_null());
        assert_eq!(((*configs).count.unwrap())(plugin), 2);
        assert!(((*configs).select.unwrap())(plugin, 0));
        for (index, inputs) in [(0, 1), (1, 2)] {
            let mut config = std::mem::MaybeUninit::uninit();
            assert!(((*configs).get.unwrap())(
                plugin,
                index,
                config.as_mut_ptr()
            ));
            let config = config.assume_init();
            assert_eq!(config.id, index);
            assert_eq!(config.input_port_count, inputs);
            assert_eq!(config.output_port_count, 1);
        }
        assert!(((*configs).select.unwrap())(plugin, 1));
        let ports = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr())
            .cast::<clap_plugin_audio_ports>();
        assert!(!ports.is_null());
        assert_eq!(((*ports).count.unwrap())(plugin, true), 2);
        assert_eq!(((*ports).count.unwrap())(plugin, false), 1);
        assert!(((*plugin).activate.unwrap())(plugin, 48_000.0, 1, 257));
        assert!(((*plugin).start_processing.unwrap())(plugin));
    }
    let mut program = [[0.002_f32; 257], [-0.001; 257]];
    let mut keys = [[0.1_f32; 257], [0.001; 257]];
    let mut result = [[0.0_f32; 257]; 2];
    let mut program_ptrs = [program[0].as_mut_ptr(), program[1].as_mut_ptr()];
    let mut key_ptrs = [keys[0].as_mut_ptr(), keys[1].as_mut_ptr()];
    let mut result_ptrs = [result[0].as_mut_ptr(), result[1].as_mut_ptr()];
    let audio_bus = |pointers| clap_audio_buffer {
        data32: pointers,
        data64: std::ptr::null_mut(),
        channel_count: 2,
        latency: 0,
        constant_mask: 0,
    };
    let mut inputs = [
        audio_bus(program_ptrs.as_mut_ptr()),
        audio_bus(key_ptrs.as_mut_ptr()),
    ];
    let mut output = audio_bus(result_ptrs.as_mut_ptr());
    let mut process = clap_process {
        steady_time: -1,
        frames_count: 257,
        transport: std::ptr::null(),
        audio_inputs: inputs.as_ptr(),
        audio_outputs: &mut output,
        audio_inputs_count: 2,
        audio_outputs_count: 1,
        in_events: std::ptr::null(),
        out_events: std::ptr::null(),
    };
    for _ in 0..32 {
        // SAFETY: all disjoint channel arrays and port tables live through the
        // synchronous call and match the selected, activated 2+2 -> 2 layout.
        let status = assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        });
        assert_ne!(status, CLAP_PROCESS_ERROR);
    }
    let expected = [0.002 * 10.0_f64.powf(-6.0 / 20.0), -0.001];
    for channel in 0..2 {
        assert!(
            (20.0 * (f64::from(result[channel][256]) / expected[channel]).log10()).abs() < 0.02
        );
    }
    assert!(program[0].iter().all(|sample| *sample == 0.002));
    assert!(keys[0].iter().all(|sample| *sample == 0.1));

    // Shrink the prepared auxiliary slice before requesting a larger silent
    // replacement. This catches stale slice lengths in the native buffer manager.
    process.frames_count = 17;
    // SAFETY: the live channel arrays have capacity for the shorter callback.
    assert_ne!(
        assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        }),
        CLAP_PROCESS_ERROR
    );

    // A missing key bus becomes prepared silence. Keep a physically valid,
    // loud sentinel after the declared one-port prefix: reading it would be
    // semantically wrong without making this regression dereference bad memory.
    process.audio_inputs_count = 1;
    process.frames_count = 257;
    // SAFETY: exclusive control-thread lifecycle, with no callback in flight.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).reset.unwrap())(plugin);
        assert!(((*plugin).start_processing.unwrap())(plugin));
    }
    for _ in 0..32 {
        // SAFETY: the declared main bus and output are live. The remaining
        // allocated sentinel is outside the declared bus prefix and is ignored.
        let status = assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        });
        assert_ne!(status, CLAP_PROCESS_ERROR);
    }
    assert!((result[0][256] - 0.002).abs() < 1e-8);
    assert!((result[1][256] + 0.001).abs() < 1e-8);

    // Also grow after a short block when only one key channel is supplied.
    process.audio_inputs_count = 2;
    process.frames_count = 17;
    // SAFETY: full supplied port tables and live 257-sample channel arrays.
    assert_ne!(
        assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        }),
        CLAP_PROCESS_ERROR
    );
    inputs[1].channel_count = 1;
    process.audio_inputs = inputs.as_ptr();
    process.frames_count = 257;
    for _ in 0..32 {
        // SAFETY: the supplied channel pointer prefix contains one live channel;
        // the native buffer manager supplies prepared silence for the second.
        assert_ne!(
            assert_no_alloc::assert_no_alloc(|| unsafe {
                ((*plugin).process.unwrap())(plugin, &process)
            }),
            CLAP_PROCESS_ERROR
        );
    }
    for channel in 0..2 {
        assert!(
            (20.0 * (f64::from(result[channel][256]) / expected[channel]).log10()).abs() < 0.02
        );
    }
    // SAFETY: stop and deactivate follow successful activation/processing.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
    }
}

#[test]
fn vst3_gate_discovers_keys_validates_widths_and_processes_audio() {
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::base::{IPluginBase, IUnknown, kResultOk};
    use vst3_sys::vst::{
        AudioBusBuffers, BusDirections, IAudioProcessor, IComponent, MediaTypes, ProcessData,
        ProcessSetup, SpeakerArrangement,
    };
    use vst3_sys::{ComInterface, VstPtr};

    let wrapper = Wrapper::<NativeKeyGate>::new();
    // SAFETY: query_interface yields an owned COM reference adopted by VstPtr;
    // the original wrapper lives until all derived interface references drop.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
        let audio = MediaTypes::kAudio as i32;
        let input = BusDirections::kInput as i32;
        assert_eq!(component.get_bus_count(audio, input), 2);
        // VST3 L/R stereo and center mono speaker bit masks. NIH matches their
        // channel counts; the key port must validate its own mask, not main's.
        let stereo: SpeakerArrangement = 0b11;
        let mono: SpeakerArrangement = 0b100;
        let mut inputs = [stereo, mono];
        let mut outputs = [stereo];
        assert_ne!(
            processor.set_bus_arrangements(inputs.as_mut_ptr(), 2, outputs.as_mut_ptr(), 1),
            kResultOk
        );
        assert_eq!(component.get_bus_count(audio, input), 2);
        // Saved stereo-only sessions remain negotiable even though a restored
        // external-detector state requires the key bus at activation.
        assert_eq!(
            processor.set_bus_arrangements(inputs.as_mut_ptr(), 1, outputs.as_mut_ptr(), 1),
            kResultOk
        );
        assert_eq!(component.get_bus_count(audio, input), 1);
        inputs[1] = stereo;
        assert_eq!(
            processor.set_bus_arrangements(inputs.as_mut_ptr(), 2, outputs.as_mut_ptr(), 1),
            kResultOk
        );
        assert_eq!(component.get_bus_count(audio, input), 2);
        assert_eq!(
            processor.setup_processing(&ProcessSetup {
                process_mode: 0,
                symbolic_sample_size: 0,
                max_samples_per_block: 257,
                sample_rate: 48_000.0,
            }),
            kResultOk
        );
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);

        let mut program = [[0.002_f32; 257], [-0.001; 257]];
        let mut keys = [[0.1_f32; 257], [0.001; 257]];
        let mut result = [[0.0_f32; 257]; 2];
        let mut program_ptrs = [
            program[0].as_mut_ptr().cast(),
            program[1].as_mut_ptr().cast(),
        ];
        let mut key_ptrs = [keys[0].as_mut_ptr().cast(), keys[1].as_mut_ptr().cast()];
        let mut result_ptrs = [result[0].as_mut_ptr().cast(), result[1].as_mut_ptr().cast()];
        let bus = |pointers| AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: pointers,
        };
        let mut inputs = [bus(program_ptrs.as_mut_ptr()), bus(key_ptrs.as_mut_ptr())];
        let mut output = bus(result_ptrs.as_mut_ptr());
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: 257,
            num_inputs: 2,
            num_outputs: 1,
            inputs: inputs.as_mut_ptr(),
            outputs: &mut output,
            // SAFETY: StaticVstPtr is nullable; these optional event lists are absent.
            input_param_changes: std::mem::zeroed(),
            output_param_changes: std::mem::zeroed(),
            input_events: std::mem::zeroed(),
            output_events: std::mem::zeroed(),
            context: std::ptr::null_mut(),
        };
        for _ in 0..32 {
            assert_eq!(
                assert_no_alloc::assert_no_alloc(|| processor.process(&mut data)),
                kResultOk
            );
        }
        let expected = [0.002 * 10.0_f64.powf(-6.0 / 20.0), -0.001];
        for channel in 0..2 {
            assert!(
                (20.0 * (f64::from(result[channel][256]) / expected[channel]).log10()).abs() < 0.02
            );
        }
        data.num_samples = 17;
        assert_eq!(
            assert_no_alloc::assert_no_alloc(|| processor.process(&mut data)),
            kResultOk
        );
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);
        // The physical second port remains a valid loud sentinel while the
        // declared input prefix contains only main. Output count is unrelated.
        data.num_inputs = 1;
        data.num_samples = 257;
        for _ in 0..32 {
            assert_eq!(
                assert_no_alloc::assert_no_alloc(|| processor.process(&mut data)),
                kResultOk
            );
        }
        assert!((result[0][256] - 0.002).abs() < 1e-8);
        assert!((result[1][256] + 0.001).abs() < 1e-8);
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}
