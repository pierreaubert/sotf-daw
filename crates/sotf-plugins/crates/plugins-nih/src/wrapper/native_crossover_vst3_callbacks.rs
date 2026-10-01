//! Check Crossover VST3 bus arrangements and speaker order with a raw ABI client.
use super::SotfCrossover;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
use plugins_bridge::param_bridge::ParamBridge;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::rc::Rc;
use vst3_sys as vst3_com;
use vst3_sys::base::{
    IBStream, IPluginBase, IUnknown, kIBSeekCur, kIBSeekEnd, kIBSeekSet, kInvalidArgument,
    kResultFalse, kResultOk, tresult,
};
use vst3_sys::utils::{SharedVstPtr, VstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, BusDirections, BusInfo, IAudioProcessor, IComponent, MediaTypes, ProcessData,
    ProcessSetup, SymbolicSampleSizes,
};
use vst3_sys::{ComInterface, VST3};

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 257;
const GUARD: f32 = -63_219.75;

#[test]
fn clap_crossover_advertises_surround_support() {
    assert!(
        std::hint::black_box(
            <SotfCrossover as nih_plug::prelude::ClapPlugin>::CLAP_SUPPORTS_SURROUND,
        ),
        "Crossover exposes negotiated multichannel layouts to CLAP hosts"
    );
}

type Interfaces = (
    Box<Wrapper<SotfCrossover>>,
    VstPtr<dyn IAudioProcessor>,
    VstPtr<dyn IComponent>,
);

fn interfaces() -> Interfaces {
    let wrapper = Wrapper::<SotfCrossover>::new();
    // SAFETY: each queried interface owns one COM reference. The wrapper remains in
    // the returned tuple until both interface references have been dropped.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
        (wrapper, processor, component)
    }
}

fn bus_name(info: &BusInfo) -> String {
    let end = info
        .name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(info.name.len());
    let utf16 = info.name[..end]
        .iter()
        .map(|unit| *unit as u16)
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&utf16)
}

#[VST3(implements(IBStream))]
struct TestMemoryStream {
    bytes: Rc<RefCell<Vec<u8>>>,
    cursor: Cell<usize>,
}

impl TestMemoryStream {
    fn from_bytes(bytes: &[u8]) -> Box<Self> {
        Self::allocate(Rc::new(RefCell::new(bytes.to_vec())), Cell::new(0))
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
            // SAFETY: the caller provides writable storage and count is bounded by both regions.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), buffer.cast(), count)
            };
        }
        self.cursor.set(cursor + count);
        if !num_bytes_read.is_null() {
            // SAFETY: VST3 specifies this optional pointer as writable when non-null.
            unsafe { *num_bytes_read = count as i32 };
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        _buffer: *const c_void,
        _num_bytes: i32,
        _num_bytes_written: *mut i32,
    ) -> tresult {
        kResultFalse
    }

    unsafe fn seek(&self, position: i64, mode: i32, result: *mut i64) -> tresult {
        let base = match mode {
            value if value == kIBSeekSet => 0_i128,
            value if value == kIBSeekCur => self.cursor.get() as i128,
            value if value == kIBSeekEnd => self.bytes.borrow().len() as i128,
            _ => return kInvalidArgument,
        };
        let Ok(target) = usize::try_from(base + i128::from(position)) else {
            return kInvalidArgument;
        };
        self.cursor.set(target);
        if !result.is_null() {
            // SAFETY: VST3 specifies this optional pointer as writable when non-null.
            unsafe { *result = target as i64 };
        }
        kResultOk
    }

    unsafe fn tell(&self, position: *mut i64) -> tresult {
        if position.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the VST3 ABI requires a valid writable output pointer here.
        unsafe { *position = self.cursor.get() as i64 };
        kResultOk
    }
}

unsafe fn shared_stream(stream: &VstPtr<dyn IBStream>) -> SharedVstPtr<dyn IBStream> {
    // SAFETY: the owning pointer is held through the synchronous set_state call below.
    unsafe { std::mem::transmute(stream.as_ptr()) }
}

fn crossover_state(topology: i32, mode: i32, num_bands: usize) -> Vec<u8> {
    let mut params = BTreeMap::from([
        ("family".to_string(), ParamValue::I32(2)),
        ("frequency".to_string(), ParamValue::F32(840.0)),
        ("frequency_2".to_string(), ParamValue::F32(2_100.0)),
        ("frequency_3".to_string(), ParamValue::F32(5_300.0)),
        ("mode".to_string(), ParamValue::I32(mode)),
        ("topology".to_string(), ParamValue::I32(topology)),
        (
            "band_count".to_string(),
            ParamValue::I32((num_bands - 2) as i32),
        ),
    ]);
    for channel in 0..16 {
        // Every SOTF speaker gets a different cutoff and alternating filter. This makes a
        // reciprocal host/internal channel permutation observable in the full output vector.
        params.insert(
            format!("channel_frequency_{channel}"),
            ParamValue::F32(260.0 + channel as f32 * 431.0),
        );
        params.insert(
            format!("channel_mode_{channel}"),
            ParamValue::I32(channel % 2),
        );
    }
    serde_json::to_vec(&PluginState {
        version: "aud142-callback-test".into(),
        params,
        fields: BTreeMap::new(),
    })
    .unwrap()
}

fn state_params(state_bytes: &[u8]) -> std::sync::Arc<crate::params::DynamicParams> {
    let state: PluginState = serde_json::from_slice(state_bytes).unwrap();
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("Crossover"));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    for info in &mut infos {
        if let Some(value) = state.params.get(&info.id) {
            info.default_value = match value {
                ParamValue::F32(value) => f64::from(*value),
                ParamValue::I32(value) => f64::from(*value),
                ParamValue::Bool(value) => f64::from(u8::from(*value)),
                _ => continue,
            };
        }
    }
    crate::params::DynamicParams::from_infos_for_plugin("Crossover", &infos)
}

fn expected_host_to_sotf(layout_index: usize) -> &'static [usize] {
    match layout_index {
        // These VST3 speaker bus orders are stated independently of the production mapping.
        0 => &[0, 1],
        1 => &[0],
        2 => &[0, 1, 2, 3],
        3 => &[0, 1, 2, 3, 4, 5],
        4 => &[0, 1, 2, 3, 6, 7, 4, 5],
        5 => &[0, 1, 2, 3, 4, 5, 6, 7],
        6 => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        7 => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9],
        8 => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11],
        9 => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 8, 9],
        10 => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 14, 15, 8, 9],
        _ => panic!("unsupported Crossover layout index {layout_index}"),
    }
}

fn expected_vst3_arrangement(layout_index: usize) -> u64 {
    // SDK speaker arrangements are literal fixture expectations. Mono uses
    // kSpeakerM (bit 19), while quad uses k40Music; these are not width masks.
    // The 8-channel 5.1.2/7.1 and 10-channel 5.1.4/7.1.2 layouts stay distinct.
    match layout_index {
        0 => 0x0000_0000_0000_0003,
        1 => 0x0000_0000_0008_0000,
        2 => 0x0000_0000_0000_0033,
        3 => 0x0000_0000_0000_003f,
        4 => 0x0000_0000_0000_063f,
        5 => 0x0000_0000_0000_503f,
        6 => 0x0000_0000_0002_d03f,
        7 => 0x0000_0000_0000_563f,
        8 => 0x0000_0000_0002_d63f,
        9 => 0x1800_0000_0002_d63f,
        10 => 0x1800_0000_0302_d63f,
        _ => panic!("unsupported Crossover layout index {layout_index}"),
    }
}

fn distinct_input(width: usize) -> Vec<Vec<f32>> {
    (0..width)
        .map(|channel| {
            (0..FRAMES + 2)
                .map(|index| {
                    if index == 0 || index == FRAMES + 1 {
                        GUARD
                    } else {
                        let frame = (index - 1) as f32;
                        let phase = frame * (0.019 + channel as f32 * 0.0027);
                        (phase.sin() * 0.21 + (phase * 0.37).cos() * 0.08)
                            * (0.6 + channel as f32 * 0.031)
                    }
                })
                .collect()
        })
        .collect()
}

fn direct_reference(
    state_bytes: &[u8],
    host_input: &[Vec<f32>],
    host_to_sotf: &[usize],
) -> Vec<f32> {
    let width = host_to_sotf.len();
    let params = state_params(state_bytes);
    let plugin = crate::params::configuration::create_plugin_with_input_channels(
        "Crossover",
        SAMPLE_RATE,
        &params,
        width,
    )
    .unwrap();
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, FRAMES).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    params.sync_to_plugin(plugin.as_mut()).unwrap();

    let input_width = width;
    let mut input_sotf = vec![0.0; FRAMES * input_width];
    for (host_channel, &sotf_channel) in host_to_sotf.iter().enumerate() {
        for frame in 0..FRAMES {
            input_sotf[frame * width + sotf_channel] = host_input[host_channel][frame + 1];
        }
    }
    let output_width = plugin.output_channels();
    let mut output_sotf = vec![0.0; FRAMES * output_width];
    assert_eq!(
        plugin
            .process(
                &input_sotf,
                &mut output_sotf,
                &sotf_host::plugin::ProcessContext::new(SAMPLE_RATE, FRAMES),
            )
            .unwrap(),
        FRAMES
    );
    output_sotf
}

fn run_vst3_case(layout_index: usize, topology: i32, mode: i32, num_bands: usize) {
    let host_to_sotf = expected_host_to_sotf(layout_index);
    let width = host_to_sotf.len();
    let state_bytes = crossover_state(topology, mode, num_bands);
    let active_bands = if topology == 1 { 1 } else { num_bands };
    let (_wrapper, processor, component) = interfaces();
    let stream = TestMemoryStream::from_bytes(&state_bytes);
    // SAFETY: the stream COM reference is owned until the synchronous state restore completes.
    let stream = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap() };
    // SAFETY: the stream remains live through this synchronous call.
    assert_eq!(
        unsafe { component.set_state(shared_stream(&stream)) },
        kResultOk
    );

    let input_arrangement = expected_vst3_arrangement(layout_index);
    let mut input_arrangements = [input_arrangement];
    let mut output_arrangements = [input_arrangement; 4];
    // SAFETY: all arrangement arrays have the declared bus counts and remain live during the call.
    unsafe {
        assert_eq!(
            processor.set_bus_arrangements(
                input_arrangements.as_mut_ptr(),
                1,
                output_arrangements.as_mut_ptr(),
                4,
            ),
            kResultOk
        );
        for bus in 0..active_bands {
            assert_eq!(
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    bus as i32,
                    1,
                ),
                kResultOk
            );
        }
    }

    let setup = ProcessSetup {
        process_mode: 0,
        symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
        max_samples_per_block: FRAMES as i32,
        sample_rate: SAMPLE_RATE as f64,
    };
    // SAFETY: setup is valid and remains live for the synchronous ABI call.
    unsafe {
        assert_eq!(processor.setup_processing(&setup), kResultOk);
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);
    }

    let mut input_channels = distinct_input(width);
    let mut input_pointers: Vec<*mut c_void> = input_channels
        .iter_mut()
        .map(|channel| channel[1..].as_mut_ptr().cast())
        .collect();
    let mut input_bus = AudioBusBuffers {
        num_channels: width as i32,
        silence_flags: 0,
        buffers: input_pointers.as_mut_ptr(),
    };

    let mut output_channels: Vec<Vec<Vec<f32>>> = (0..4)
        .map(|_| (0..width).map(|_| vec![GUARD; FRAMES + 2]).collect())
        .collect();
    let mut output_pointers: Vec<Vec<*mut c_void>> = output_channels
        .iter_mut()
        .enumerate()
        .map(|(bus, channels)| {
            if bus < active_bands {
                channels
                    .iter_mut()
                    .map(|channel| channel[1..].as_mut_ptr().cast())
                    .collect()
            } else {
                // The ABI still receives a width-sized channel-pointer array for an inactive bus;
                // each sample pointer is nullable, matching the production AUD143 contract.
                vec![std::ptr::null_mut(); width]
            }
        })
        .collect();
    let mut output_buses: Vec<AudioBusBuffers> = output_pointers
        .iter_mut()
        .map(|pointers| AudioBusBuffers {
            num_channels: width as i32,
            silence_flags: 0,
            buffers: pointers.as_mut_ptr(),
        })
        .collect();
    // SAFETY: the input and each active output channel point at disjoint live storage. Inactive
    // buses have four width-sized arrays whose sample pointers are all null by VST3 contract.
    let mut process = unsafe { std::mem::zeroed::<ProcessData>() };
    process.process_mode = 0;
    process.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
    process.num_samples = FRAMES as i32;
    process.num_inputs = 1;
    process.num_outputs = 4;
    process.inputs = &mut input_bus;
    process.outputs = output_buses.as_mut_ptr();
    // SAFETY: ProcessData points to valid bus arrays and all active sample buffers outlive process.
    assert_eq!(unsafe { processor.process(&mut process) }, kResultOk);

    let expected = direct_reference(&state_bytes, &input_channels, host_to_sotf);
    let expected_bands = if topology == 1 { 1 } else { num_bands };
    assert_eq!(expected.len(), FRAMES * width * expected_bands);
    assert_eq!(output_channels.len(), 4, "VST3 reports four output buses");
    for (bus, channels) in output_channels.iter().take(expected_bands).enumerate() {
        assert_eq!(channels.len(), width, "output bus {bus} channel width");
        for (channel, actual) in channels.iter().enumerate() {
            assert_eq!(
                actual[0], GUARD,
                "leading guard bus {bus} channel {channel}"
            );
            assert_eq!(
                actual[FRAMES + 1],
                GUARD,
                "trailing guard bus {bus} channel {channel}"
            );
            let sotf_channel = host_to_sotf[channel];
            for frame in 0..FRAMES {
                let expected_index = frame * width * expected_bands + bus * width + sotf_channel;
                let reference = expected[expected_index];
                assert!(reference.is_finite());
                let sample = actual[frame + 1];
                assert!(sample.is_finite());
                assert!(
                    (sample - reference).abs() <= 2.0e-5,
                    "layout={layout_index}, topology={topology}, mode={mode}, bands={num_bands}, bus={bus}, host-channel={channel}, frame={frame}: {sample} != {reference}"
                );
            }
        }
    }
    for (bus, channels) in output_channels
        .iter()
        .enumerate()
        .skip(expected_bands)
        .take(4 - expected_bands)
    {
        assert_eq!(
            channels.len(),
            width,
            "silent output bus {bus} channel width"
        );
        for (channel, samples) in channels.iter().enumerate() {
            assert_eq!(
                samples[0], GUARD,
                "leading guard bus {bus} channel {channel}"
            );
            assert_eq!(
                samples[FRAMES + 1],
                GUARD,
                "trailing guard bus {bus} channel {channel}"
            );
            assert!(samples[1..FRAMES + 1].iter().all(|sample| *sample == GUARD));
        }
    }
    // Non-identity maps and distinct controls make an accidental shared input/output swap visible.
    let identity = (0..width).collect::<Vec<_>>();
    if host_to_sotf != identity {
        let wrong_order = direct_reference(&state_bytes, &input_channels, &identity);
        let mut wrong_order_delta = 0.0_f32;
        for band in 0..expected_bands {
            for channel in 0..width {
                for frame in 0..FRAMES {
                    let index = frame * width * expected_bands + band * width + channel;
                    wrong_order_delta =
                        wrong_order_delta.max((expected[index] - wrong_order[index]).abs());
                }
            }
        }
        assert!(
            wrong_order_delta > 1.0e-4,
            "speaker-order sensitivity control"
        );
    }

    // SAFETY: processing is stopped before deactivation and COM teardown.
    unsafe {
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}

#[test]
fn vst3_crossover_surround_routes_preserve_speaker_and_band_order() {
    for layout_index in 0..=10 {
        run_vst3_case(layout_index, 1, 0, 2);
        for num_bands in 2..=4 {
            run_vst3_case(layout_index, 0, 2, num_bands);
        }
    }
}

#[test]
fn crossover_vst3_advertises_four_fixed_width_output_buses() {
    let (_wrapper, processor, component) = interfaces();

    // VST3 keeps four width-sized output buses available in every mode. The active
    // prefix is negotiated separately; unused buses remain present and inactive.
    // SAFETY: all callbacks receive valid output pointers and live COM interfaces.
    unsafe {
        assert_eq!(
            component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kInput as i32),
            1
        );
        assert_eq!(
            component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kOutput as i32),
            4
        );

        let mut input = std::mem::zeroed::<BusInfo>();
        assert_eq!(
            component.get_bus_info(
                MediaTypes::kAudio as i32,
                BusDirections::kInput as i32,
                0,
                &mut input,
            ),
            kResultOk
        );
        assert_eq!(input.channel_count, 2, "default input is stereo");
        assert_eq!(
            bus_name(&input),
            "Stereo Input",
            "the negotiated input bus is being queried"
        );

        let mut arrangement = 0;
        assert_eq!(
            processor.get_bus_arrangement(BusDirections::kInput as i32, 0, &mut arrangement),
            kResultOk
        );
        assert_eq!(arrangement, 0b11);

        for bus_index in 0..4 {
            let mut output = std::mem::zeroed::<BusInfo>();
            assert_eq!(
                component.get_bus_info(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    bus_index,
                    &mut output,
                ),
                kResultOk
            );
            assert_eq!(
                output.channel_count, 2,
                "output bus {bus_index} retains negotiated speaker width"
            );
            assert_eq!(
                processor.get_bus_arrangement(
                    BusDirections::kOutput as i32,
                    bus_index,
                    &mut arrangement,
                ),
                kResultOk
            );
            assert_eq!(arrangement, 0b11, "output bus {bus_index} layout");
        }
        assert_eq!(component.terminate(), kResultOk);
    }
}
