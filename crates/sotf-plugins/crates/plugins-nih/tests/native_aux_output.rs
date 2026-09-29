//! Native bus-prefix regressions use valid hidden ports as out-of-count sentinels.
// Rust guideline compliant 2026-02-21
use nih_plug::prelude::*;
use std::sync::Arc;

#[derive(Default, Params)]
struct EmptyParams {}

#[derive(Default)]
struct OutputProbe {
    params: Arc<EmptyParams>,
}
impl Plugin for OutputProbe {
    const NAME: &'static str = "Auxiliary output probe";
    const VENDOR: &'static str = "SOTF tests";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: Some(nih_plug::audio_setup::new_nonzero_u32(1)),
        main_output_channels: Some(nih_plug::audio_setup::new_nonzero_u32(1)),
        aux_input_ports: &[],
        aux_output_ports: &[
            nih_plug::audio_setup::new_nonzero_u32(1),
            nih_plug::audio_setup::new_nonzero_u32(2),
        ],
        names: PortNames::const_default(),
    }];
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn Params> {
        Arc::clone(&self.params) as Arc<dyn Params>
    }
    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        buffer.as_slice()[0].fill(0.25);
        aux.outputs[0].as_slice()[0].fill(0.5);
        aux.outputs[1].as_slice()[0].fill(0.75);
        aux.outputs[1].as_slice()[1].fill(-0.75);
        ProcessStatus::Normal
    }
}
impl ClapPlugin for OutputProbe {
    const CLAP_ID: &'static str = "org.sotf.test.aux-output";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect];
}
impl Vst3Plugin for OutputProbe {
    const VST3_CLASS_ID: [u8; 16] = *b"SotfAuxOutProbe1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

fn verify_outputs(
    main: &[f32],
    mono: &[f32],
    stereo: &[[f32; 257]; 2],
    ports: usize,
    frames: usize,
) {
    if ports == 3 {
        assert!(main[..frames].iter().all(|sample| *sample == 0.25));
        assert!(mono[..frames].iter().all(|sample| *sample == 0.5));
        assert!(stereo[0][..frames].iter().all(|sample| *sample == 0.75));
        assert!(stereo[1][..frames].iter().all(|sample| *sample == -0.75));
    } else {
        // Missing required outputs skip DSP; the provided main input was copied
        // to main output by the buffer manager before that validity check.
        assert!(main[..frames].iter().all(|sample| *sample == 0.125));
        assert!(stereo.iter().flatten().all(|sample| *sample == 73.0));
        if ports == 1 {
            assert!(mono.iter().all(|sample| *sample == 71.0));
        } else {
            assert!(mono[..frames].iter().all(|sample| *sample == 0.0));
        }
    }
}

#[test]
fn clap_missing_auxiliary_output_never_accesses_hidden_valid_bus() {
    use clap_sys::audio_buffer::clap_audio_buffer;
    use clap_sys::host::clap_host;
    use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
    use std::ffi::{c_char, c_void};
    unsafe extern "C" fn extension(_: *const clap_host, _: *const c_char) -> *const c_void {
        std::ptr::null()
    }
    unsafe extern "C" fn request(_: *const clap_host) {}
    let host = Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(extension),
        request_restart: Some(request),
        request_process: Some(request),
        request_callback: Some(request),
    });
    // SAFETY: The host table outlives the native wrapper and all callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<OutputProbe>::new(&*host) };
    let plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: Calls follow the native lifecycle on this live instance.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        assert!(((*plugin).activate.unwrap())(plugin, 48_000.0, 1, 257));
        assert!(((*plugin).start_processing.unwrap())(plugin));
    }
    let mut input = [0.125_f32; 257];
    let mut main = [99.0_f32; 257];
    let mut mono = [71.0_f32; 257];
    let mut stereo = [[73.0_f32; 257]; 2];
    let mut input_ptrs = [input.as_mut_ptr()];
    let mut main_ptrs = [main.as_mut_ptr()];
    let mut mono_ptrs = [mono.as_mut_ptr()];
    let mut stereo_ptrs = [stereo[0].as_mut_ptr(), stereo[1].as_mut_ptr()];
    let bus = |pointers, channels| clap_audio_buffer {
        data32: pointers,
        data64: std::ptr::null_mut(),
        channel_count: channels,
        latency: 0,
        constant_mask: 0,
    };
    let input_bus = bus(input_ptrs.as_mut_ptr(), 1);
    let mut output_buses = [
        bus(main_ptrs.as_mut_ptr(), 1),
        bus(mono_ptrs.as_mut_ptr(), 1),
        bus(stereo_ptrs.as_mut_ptr(), 2),
    ];
    for (ports, frames) in [(3, 17), (2, 257), (3, 17), (1, 257), (3, 257)] {
        main.fill(99.0);
        mono.fill(71.0);
        stereo.iter_mut().for_each(|channel| channel.fill(73.0));
        let process = clap_process {
            steady_time: -1,
            frames_count: frames as u32,
            transport: std::ptr::null(),
            audio_inputs: &input_bus,
            audio_outputs: output_buses.as_mut_ptr(),
            audio_inputs_count: 1,
            audio_outputs_count: ports as u32,
            in_events: std::ptr::null(),
            out_events: std::ptr::null(),
        };
        // SAFETY: All declared pointers are live and frame-capable. Hidden
        // ports are physically valid sentinels outside the declared prefix.
        let status = assert_no_alloc::assert_no_alloc(|| unsafe {
            ((*plugin).process.unwrap())(plugin, &process)
        });
        assert_ne!(status, CLAP_PROCESS_ERROR);
        verify_outputs(&main, &mono, &stereo, ports, frames);
    }
    // SAFETY: Paired stop/deactivate on the activated instance.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
    }
}

#[test]
fn vst3_missing_auxiliary_output_never_accesses_hidden_valid_bus() {
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::base::{IPluginBase, IUnknown, kResultOk};
    use vst3_sys::vst::{AudioBusBuffers, IAudioProcessor, IComponent, ProcessData, ProcessSetup};
    use vst3_sys::{ComInterface, VstPtr};
    let wrapper = Wrapper::<OutputProbe>::new();
    // SAFETY: COM references are adopted into owning VstPtr values and outlive
    // all calls. All audio storage remains live and disjoint for each callback.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
        // Mono main/aux plus stereo final aux catches the reversed output
        // arrangement index independently of Gate's input arrangement fixture.
        let mut input_layout = [0b100];
        let mut output_layout = [0b100, 0b100, 0b11];
        assert_eq!(
            processor.set_bus_arrangements(
                input_layout.as_mut_ptr(),
                1,
                output_layout.as_mut_ptr(),
                3
            ),
            kResultOk
        );
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
        let mut input = [0.125_f32; 257];
        let mut main = [99.0_f32; 257];
        let mut mono = [71.0_f32; 257];
        let mut stereo = [[73.0_f32; 257]; 2];
        let mut input_ptrs = [input.as_mut_ptr().cast()];
        let mut main_ptrs = [main.as_mut_ptr().cast()];
        let mut mono_ptrs = [mono.as_mut_ptr().cast()];
        let mut stereo_ptrs = [stereo[0].as_mut_ptr().cast(), stereo[1].as_mut_ptr().cast()];
        let bus = |pointers, channels| AudioBusBuffers {
            num_channels: channels,
            silence_flags: 0,
            buffers: pointers,
        };
        let mut input_bus = bus(input_ptrs.as_mut_ptr(), 1);
        let mut output_buses = [
            bus(main_ptrs.as_mut_ptr(), 1),
            bus(mono_ptrs.as_mut_ptr(), 1),
            bus(stereo_ptrs.as_mut_ptr(), 2),
        ];
        for (ports, frames) in [(3, 17), (2, 257), (3, 17), (1, 257), (3, 257)] {
            main.fill(99.0);
            mono.fill(71.0);
            stereo.iter_mut().for_each(|channel| channel.fill(73.0));
            let mut data = ProcessData {
                process_mode: 0,
                symbolic_sample_size: 0,
                num_samples: frames as i32,
                num_inputs: 1,
                num_outputs: ports as i32,
                inputs: &mut input_bus,
                outputs: output_buses.as_mut_ptr(),
                input_param_changes: std::mem::zeroed(),
                output_param_changes: std::mem::zeroed(),
                input_events: std::mem::zeroed(),
                output_events: std::mem::zeroed(),
                context: std::ptr::null_mut(),
            };
            assert_eq!(
                assert_no_alloc::assert_no_alloc(|| processor.process(&mut data)),
                kResultOk
            );
            verify_outputs(&main, &mono, &stereo, ports, frames);
        }
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}
