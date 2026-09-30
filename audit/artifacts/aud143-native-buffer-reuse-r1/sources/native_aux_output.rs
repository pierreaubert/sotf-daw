//! Native bus-prefix regressions use valid hidden ports as out-of-count sentinels.
// Rust guideline compliant 2026-02-21
use nih_plug::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        // An omitted auxiliary output suffix skips DSP; the main input was
        // copied to main output by the buffer manager before that validity check.
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
    let mut extra_mono = [83.0_f32; 257];
    let mut stereo = [[73.0_f32; 257]; 2];
    let mut input_ptrs = [input.as_mut_ptr()];
    let mut main_ptrs = [main.as_mut_ptr()];
    let mut mono_ptrs = [mono.as_mut_ptr(), extra_mono.as_mut_ptr()];
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

    // A delivered bus with the wrong width remains an error. Keep two live pointers behind the
    // bad mono declaration so the rejection and safe-silence path are exercised without an
    // invalid test fixture pointer. Surplus channel storage must remain untouched.
    main.fill(99.0);
    mono.fill(71.0);
    extra_mono.fill(83.0);
    stereo.iter_mut().for_each(|channel| channel.fill(73.0));
    output_buses[1].channel_count = 2;
    let malformed_process = clap_process {
        steady_time: -1,
        frames_count: 17,
        transport: std::ptr::null(),
        audio_inputs: &input_bus,
        audio_outputs: output_buses.as_mut_ptr(),
        audio_inputs_count: 1,
        audio_outputs_count: 3,
        in_events: std::ptr::null(),
        out_events: std::ptr::null(),
    };
    // SAFETY: Both pointers supplied for the malformed bus are live and the host arrays remain
    // valid for the callback. The wrapper must reject its wrong declared channel count.
    let status = assert_no_alloc::assert_no_alloc(|| unsafe {
        ((*plugin).process.unwrap())(plugin, &malformed_process)
    });
    assert_eq!(status, CLAP_PROCESS_ERROR);
    assert!(main[..17].iter().all(|sample| *sample == 0.0));
    assert!(mono[..17].iter().all(|sample| *sample == 0.0));
    assert!(
        stereo
            .iter()
            .all(|channel| channel[..17].iter().all(|sample| *sample == 0.0))
    );
    assert!(extra_mono.iter().all(|sample| *sample == 83.0));
    assert!(main[17..].iter().all(|sample| *sample == 99.0));
    // SAFETY: Paired stop/deactivate on the activated instance.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
    }
}

#[test]
fn vst3_missing_auxiliary_output_never_accesses_hidden_valid_bus() {
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::base::{IPluginBase, IUnknown, kResultFalse, kResultOk};
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
        let mut extra_mono = [83.0_f32; 257];
        let mut stereo = [[73.0_f32; 257]; 2];
        let mut input_ptrs = [input.as_mut_ptr().cast()];
        let mut main_ptrs = [main.as_mut_ptr().cast()];
        let mut mono_ptrs = [mono.as_mut_ptr().cast(), extra_mono.as_mut_ptr().cast()];
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

        // A delivered bus with the wrong width remains an error. Keep two live pointers behind
        // the bad mono declaration so the rejection and safe-silence path are exercised without
        // an invalid test fixture pointer. Surplus channel storage must remain untouched.
        main.fill(99.0);
        mono.fill(71.0);
        extra_mono.fill(83.0);
        stereo.iter_mut().for_each(|channel| channel.fill(73.0));
        output_buses[1].num_channels = 2;
        let mut malformed_data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: 17,
            num_inputs: 1,
            num_outputs: 3,
            inputs: &mut input_bus,
            outputs: output_buses.as_mut_ptr(),
            input_param_changes: std::mem::zeroed(),
            output_param_changes: std::mem::zeroed(),
            input_events: std::mem::zeroed(),
            output_events: std::mem::zeroed(),
            context: std::ptr::null_mut(),
        };
        // SAFETY: Both pointers supplied for the malformed bus are live and the host arrays remain
        // valid for the callback. The wrapper must reject its wrong declared channel count.
        assert_eq!(
            assert_no_alloc::assert_no_alloc(|| processor.process(&mut malformed_data)),
            kResultFalse
        );
        assert!(main[..17].iter().all(|sample| *sample == 0.0));
        assert!(mono[..17].iter().all(|sample| *sample == 0.0));
        assert!(
            stereo
                .iter()
                .all(|channel| channel[..17].iter().all(|sample| *sample == 0.0))
        );
        assert!(extra_mono.iter().all(|sample| *sample == 83.0));
        assert!(main[17..].iter().all(|sample| *sample == 99.0));
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}

const INACTIVE_OUTPUT_CALLBACKS: usize = 3;
const INACTIVE_OUTPUT_PORTS: usize = 2;

struct InactiveOutputObservation {
    samples: AtomicUsize,
    channels: AtomicUsize,
    slice_lengths_match: AtomicBool,
    samples_are_zero: AtomicBool,
}

impl InactiveOutputObservation {
    const fn new() -> Self {
        Self {
            samples: AtomicUsize::new(usize::MAX),
            channels: AtomicUsize::new(usize::MAX),
            slice_lengths_match: AtomicBool::new(false),
            samples_are_zero: AtomicBool::new(false),
        }
    }
}

static INACTIVE_OUTPUT_CALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);
static INACTIVE_OUTPUT_OBSERVATIONS: [[InactiveOutputObservation; INACTIVE_OUTPUT_PORTS];
    INACTIVE_OUTPUT_CALLBACKS] = [
    [
        InactiveOutputObservation::new(),
        InactiveOutputObservation::new(),
    ],
    [
        InactiveOutputObservation::new(),
        InactiveOutputObservation::new(),
    ],
    [
        InactiveOutputObservation::new(),
        InactiveOutputObservation::new(),
    ],
];

#[derive(Default)]
struct InactiveOutputProbe {
    params: Arc<EmptyParams>,
}

impl Plugin for InactiveOutputProbe {
    const NAME: &'static str = "Inactive auxiliary output probe";
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
        let callback = INACTIVE_OUTPUT_CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
        if callback < INACTIVE_OUTPUT_CALLBACKS {
            for (port, output) in aux
                .outputs
                .iter_mut()
                .enumerate()
                .take(INACTIVE_OUTPUT_PORTS)
            {
                let observation = &INACTIVE_OUTPUT_OBSERVATIONS[callback][port];
                let samples = output.samples();
                let channels = output.as_slice();
                let expected_channels = if port == 0 { 1 } else { 2 };
                let slice_lengths_match = channels.iter().all(|channel| channel.len() == samples);
                let samples_are_zero = channels
                    .iter()
                    .flat_map(|channel| channel.iter())
                    .all(|sample| *sample == 0.0);

                observation.samples.store(samples, Ordering::Relaxed);
                observation
                    .channels
                    .store(channels.len(), Ordering::Relaxed);
                observation.slice_lengths_match.store(
                    slice_lengths_match && channels.len() == expected_channels,
                    Ordering::Relaxed,
                );
                observation
                    .samples_are_zero
                    .store(samples_are_zero, Ordering::Relaxed);
            }
        }

        buffer.as_slice()[0].fill(0.25);
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for InactiveOutputProbe {
    const VST3_CLASS_ID: [u8; 16] = *b"SotfAuxReuseP001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];

    fn vst3_allows_inactive_audio_output_buses() -> bool {
        true
    }
}

fn reset_inactive_output_observations() {
    INACTIVE_OUTPUT_CALLBACK_COUNT.store(0, Ordering::Relaxed);
    for callback in &INACTIVE_OUTPUT_OBSERVATIONS {
        for port in callback {
            port.samples.store(usize::MAX, Ordering::Relaxed);
            port.channels.store(usize::MAX, Ordering::Relaxed);
            port.slice_lengths_match.store(false, Ordering::Relaxed);
            port.samples_are_zero.store(false, Ordering::Relaxed);
        }
    }
}

fn assert_inactive_output_observation(
    callback: usize,
    port: usize,
    samples: usize,
    channels: usize,
) {
    let observation = &INACTIVE_OUTPUT_OBSERVATIONS[callback][port];
    assert_eq!(observation.samples.load(Ordering::Relaxed), samples);
    assert_eq!(observation.channels.load(Ordering::Relaxed), channels);
    assert!(observation.slice_lengths_match.load(Ordering::Relaxed));
    assert!(observation.samples_are_zero.load(Ordering::Relaxed));
}

#[test]
fn vst3_inactive_auxiliary_output_reuse_tracks_absent_present_absent_buffers() {
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::base::{IPluginBase, IUnknown, kResultOk};
    use vst3_sys::vst::{
        AudioBusBuffers, BusDirections, IAudioProcessor, IComponent, MediaTypes, ProcessData,
        ProcessSetup,
    };
    use vst3_sys::{ComInterface, VstPtr};

    reset_inactive_output_observations();
    let wrapper = Wrapper::<InactiveOutputProbe>::new();
    // SAFETY: COM references are adopted into owning VstPtr values and outlive all calls. Audio
    // storage stays live and disjoint for the full wrapper lifecycle.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);

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
            component.activate_bus(
                MediaTypes::kAudio as i32,
                BusDirections::kOutput as i32,
                0,
                1
            ),
            kResultOk
        );
        for bus_index in 1..=2 {
            assert_eq!(
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    bus_index,
                    0
                ),
                kResultOk
            );
        }
        assert_eq!(
            processor.setup_processing(&ProcessSetup {
                process_mode: 0,
                symbolic_sample_size: 0,
                max_samples_per_block: 64,
                sample_rate: 48_000.0,
            }),
            kResultOk
        );
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);

        let mut input = [0.125_f32; 64];
        let mut main = [99.0_f32; 64];
        let mut mono = [71.0_f32; 64];
        let mut stereo = [[73.0_f32; 64]; 2];
        let mut input_ptrs = [input.as_mut_ptr().cast()];
        let mut main_ptrs = [main.as_mut_ptr().cast()];
        let mut mono_ptrs = [mono.as_mut_ptr().cast()];
        let mut stereo_ptrs = [stereo[0].as_mut_ptr().cast(), stereo[1].as_mut_ptr().cast()];
        let mut input_bus = AudioBusBuffers {
            num_channels: 1,
            silence_flags: 0,
            buffers: input_ptrs.as_mut_ptr(),
        };
        let mut output_buses = [
            AudioBusBuffers {
                num_channels: 1,
                silence_flags: 0,
                buffers: main_ptrs.as_mut_ptr(),
            },
            AudioBusBuffers {
                num_channels: 1,
                silence_flags: 0,
                buffers: std::ptr::null_mut(),
            },
            AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                buffers: std::ptr::null_mut(),
            },
        ];

        for (callback, frames) in [(0, 17), (1, 33), (2, 9)] {
            main.fill(99.0);
            mono.fill(71.0);
            stereo.iter_mut().for_each(|channel| channel.fill(73.0));

            if callback == 1 {
                output_buses[1].buffers = mono_ptrs.as_mut_ptr();
                output_buses[2].buffers = stereo_ptrs.as_mut_ptr();
            } else {
                output_buses[1].buffers = std::ptr::null_mut();
                output_buses[2].buffers = std::ptr::null_mut();
            }

            let mut process = ProcessData {
                process_mode: 0,
                symbolic_sample_size: 0,
                num_samples: frames,
                num_inputs: 1,
                num_outputs: 3,
                inputs: &mut input_bus,
                outputs: output_buses.as_mut_ptr(),
                input_param_changes: std::mem::zeroed(),
                output_param_changes: std::mem::zeroed(),
                input_events: std::mem::zeroed(),
                output_events: std::mem::zeroed(),
                context: std::ptr::null_mut(),
            };
            // Let the buffer invariant report directly if it regresses; a debug assertion while
            // preparing this callback must not be masked by an allocation guard's panic path.
            assert_eq!(processor.process(&mut process), kResultOk);
            assert!(main[..frames as usize].iter().all(|sample| *sample == 0.25));

            if callback == 1 {
                assert!(mono[..frames as usize].iter().all(|sample| *sample == 0.0));
                assert!(stereo.iter().all(|channel| {
                    channel[..frames as usize]
                        .iter()
                        .all(|sample| *sample == 0.0)
                }));
                assert!(mono[frames as usize..].iter().all(|sample| *sample == 71.0));
                assert!(stereo.iter().all(|channel| {
                    channel[frames as usize..]
                        .iter()
                        .all(|sample| *sample == 73.0)
                }));
            } else {
                assert!(mono.iter().all(|sample| *sample == 71.0));
                assert!(stereo.iter().flatten().all(|sample| *sample == 73.0));
            }
        }

        assert_eq!(
            INACTIVE_OUTPUT_CALLBACK_COUNT.load(Ordering::Relaxed),
            INACTIVE_OUTPUT_CALLBACKS
        );
        for callback in [0, 2] {
            assert_inactive_output_observation(callback, 0, 0, 1);
            assert_inactive_output_observation(callback, 1, 0, 2);
        }
        assert_inactive_output_observation(1, 0, 33, 1);
        assert_inactive_output_observation(1, 1, 33, 2);

        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}
