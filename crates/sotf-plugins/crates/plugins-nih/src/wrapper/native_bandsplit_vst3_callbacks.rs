//! Exercise BandSplit discovery, compatibility, and audio routing at the VST3 ABI.
//!
//! The `vst3_com::vtable` expansion behind `#[VST3]` emits a trailing
//! semicolon (rust-lang/rust#79813); allowed here because the fix belongs
//! upstream in vst3-sys, not at this use site.
#![allow(
    semicolon_in_expressions_from_non_local_macros,
    reason = "vst3_com::vtable expansion emits a trailing semicolon; fixed upstream in vst3-sys, not locally"
)]
use super::*;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
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
    AudioBusBuffers, BusDirections, BusFlags, BusInfo, IAudioProcessor, IComponent, MediaTypes,
    ProcessData, ProcessSetup, SpeakerArrangement, SymbolicSampleSizes,
};
use vst3_sys::{ComInterface, VST3};

const FRAMES: usize = 37;
const STEREO: SpeakerArrangement = 0b11;
const LEGACY_PACKED: SpeakerArrangement = 0b1111;
const STEP_CANARY: f32 = -51_237.25;

type Interfaces = (
    Box<Wrapper<SplitWrapper>>,
    VstPtr<dyn IAudioProcessor>,
    VstPtr<dyn IComponent>,
);

fn new_interfaces() -> Interfaces {
    let wrapper = Wrapper::<SplitWrapper>::new();
    // SAFETY: each queried interface owns a COM reference and the wrapper remains alive in the
    // returned tuple until both references have been dropped.
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

fn set_processing_setup(processor: &VstPtr<dyn IAudioProcessor>) {
    let setup = ProcessSetup {
        process_mode: 0,
        symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
        max_samples_per_block: FRAMES as i32,
        sample_rate: 48_000.0,
    };
    // SAFETY: `setup` stays live for the synchronous ABI call.
    assert_eq!(unsafe { processor.setup_processing(&setup) }, kResultOk);
}

fn reference_split(
    params: &std::sync::Arc<crate::params::DynamicParams>,
    num_bands: usize,
) -> Box<dyn sotf_host::plugin::Plugin> {
    params
        .select_band_split_layout(num_bands)
        .expect("the reference uses the negotiated BandSplit count");
    let plugin = crate::params::configuration::create_plugin("BandSplit", 48_000, params).unwrap();
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, FRAMES).unwrap();
    plugin.initialize(48_000).unwrap();
    params.sync_to_plugin(plugin.as_mut()).unwrap();
    plugin
}

fn reference_block(
    plugin: &mut dyn sotf_host::plugin::Plugin,
    input: &[f32],
    output_channels: usize,
) -> Vec<f32> {
    let mut output = vec![0.0; FRAMES * output_channels];
    assert_eq!(
        plugin
            .process(
                input,
                &mut output,
                &sotf_host::plugin::ProcessContext::new(48_000, FRAMES),
            )
            .unwrap(),
        FRAMES
    );
    output
}

fn test_input() -> (Vec<f32>, Vec<Vec<f32>>, Vec<*mut c_void>) {
    let interleaved: Vec<f32> = (0..FRAMES)
        .flat_map(|frame| {
            [
                ((frame * 29 % 97) as f32 - 48.0) * 0.004,
                ((frame * 43 % 89) as f32 - 44.0) * 0.003,
            ]
        })
        .collect();
    let mut channels = (0..2)
        .map(|channel| {
            (0..FRAMES)
                .map(|frame| interleaved[frame * 2 + channel])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let pointers = channels
        .iter_mut()
        .map(|channel| channel.as_mut_ptr().cast())
        .collect();
    (interleaved, channels, pointers)
}

fn process_data(
    inputs: *mut AudioBusBuffers,
    num_inputs: i32,
    outputs: *mut AudioBusBuffers,
    num_outputs: i32,
) -> ProcessData {
    // SAFETY: VST3 defines its optional event, context, and parameter interfaces as nullable.
    // The required scalar fields and audio bus pointers are assigned below before the callback.
    let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
    data.process_mode = 0;
    data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
    data.num_samples = FRAMES as i32;
    data.num_inputs = num_inputs;
    data.num_outputs = num_outputs;
    data.inputs = inputs;
    data.outputs = outputs;
    data
}

fn process_wide_bus_block(
    processor: &VstPtr<dyn IAudioProcessor>,
    num_bands: usize,
    stage: &str,
    expected_status: tresult,
) -> Vec<f32> {
    let (_, _input_channels, mut input_pointers) = test_input();
    let mut input_bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: input_pointers.as_mut_ptr(),
    };
    let mut output_channels = guarded_output_channels(8);
    let output_pointers = output_channel_pointers(&mut output_channels);
    let mut output_bus_channels = [
        [output_pointers[0], output_pointers[1]],
        [output_pointers[2], output_pointers[3]],
        [output_pointers[4], output_pointers[5]],
        [output_pointers[6], output_pointers[7]],
    ];
    let mut output_buses: [AudioBusBuffers; 4] = std::array::from_fn(|index| AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: output_bus_channels[index].as_mut_ptr(),
    });
    let mut process = process_data(&mut input_bus, 1, output_buses.as_mut_ptr(), 4);
    // SAFETY: the input and all four declared output buses point to distinct, live test buffers;
    // inactive buses retain canary storage and are not consumed by the wrapper.
    let status = unsafe { processor.process(&mut process) };
    assert_eq!(status, expected_status, "{stage}");

    assert_output_guards(&output_channels);
    if expected_status == kResultOk {
        assert_output_region_is(&output_channels[num_bands * 2..], 0.0);
        assert!(output_channels[..num_bands * 2].iter().all(|channel| {
            channel[1..FRAMES + 1]
                .iter()
                .all(|sample| sample.is_finite())
        }));
        assert!(
            output_channels[..num_bands * 2]
                .iter()
                .any(|channel| channel[1..FRAMES + 1].iter().any(|sample| *sample != 0.0))
        );
    } else {
        assert_eq!(expected_status, kResultFalse, "unsupported expected status");
        assert_output_region_is(&output_channels, 0.0);
    }
    let mut interleaved = Vec::with_capacity(FRAMES * num_bands * 2);
    for frame in 0..FRAMES {
        for channel in output_channels.iter().take(num_bands * 2) {
            interleaved.push(channel[frame + 1]);
        }
    }
    interleaved
}

fn process_legacy_packed_block(processor: &VstPtr<dyn IAudioProcessor>) -> Vec<f32> {
    let (_, _input_channels, mut input_pointers) = test_input();
    let mut input_bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: input_pointers.as_mut_ptr(),
    };
    let mut output_channels = guarded_output_channels(4);
    let mut output_pointers = output_channel_pointers(&mut output_channels);
    let mut output_bus = AudioBusBuffers {
        num_channels: 4,
        silence_flags: 0,
        buffers: output_pointers.as_mut_ptr(),
    };
    let mut process = process_data(&mut input_bus, 1, &mut output_bus, 1);
    // SAFETY: the packed output has four distinct guarded channels and the input is stereo.
    unsafe { assert_eq!(processor.process(&mut process), kResultOk) };

    assert_output_guards(&output_channels);
    assert!(output_channels.iter().all(|channel| {
        channel[1..FRAMES + 1]
            .iter()
            .all(|sample| sample.is_finite())
    }));
    assert!(
        output_channels
            .iter()
            .any(|channel| channel[1..FRAMES + 1].iter().any(|sample| *sample != 0.0))
    );
    let mut interleaved = Vec::with_capacity(FRAMES * 4);
    for frame in 0..FRAMES {
        for channel in output_channels.iter().take(4) {
            interleaved.push(channel[frame + 1]);
        }
    }
    interleaved
}

fn assert_bus_matches(
    channels: &[Vec<f32>],
    actual_channel_offset: usize,
    expected_interleaved: &[f32],
    expected_channel_offset: usize,
    expected_stride: usize,
    bus_width: usize,
) {
    for channel in 0..bus_width {
        for frame in 0..FRAMES {
            let actual = channels[actual_channel_offset + channel][frame + 1];
            let expected =
                expected_interleaved[frame * expected_stride + expected_channel_offset + channel];
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "frame={frame}, channel={channel}, offset={expected_channel_offset}: {actual} vs {expected}"
            );
        }
    }
}

fn guarded_output_channels(channel_count: usize) -> Vec<Vec<f32>> {
    vec![vec![STEP_CANARY; FRAMES + 2]; channel_count]
}

fn output_channel_pointers(channels: &mut [Vec<f32>]) -> Vec<*mut c_void> {
    channels
        .iter_mut()
        .map(|channel| channel[1..].as_mut_ptr().cast())
        .collect()
}

fn assert_output_guards(channels: &[Vec<f32>]) {
    for channel in channels {
        assert_eq!(channel[0], STEP_CANARY, "leading output guard changed");
        assert_eq!(
            channel[FRAMES + 1],
            STEP_CANARY,
            "trailing output guard changed"
        );
    }
}

fn assert_output_region_is(channels: &[Vec<f32>], expected: f32) {
    assert!(channels.iter().all(|channel| {
        channel[1..FRAMES + 1]
            .iter()
            .all(|sample| *sample == expected)
    }));
}

fn bus_name(info: &BusInfo) -> String {
    let end = info
        .name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(info.name.len());
    let utf16: Vec<u16> = info.name[..end].iter().map(|unit| *unit as u16).collect();
    String::from_utf16(&utf16).expect("VST3 bus name is valid UTF-16")
}

fn band_split_state(num_bands: usize) -> Vec<u8> {
    let state = PluginState {
        version: String::from("test"),
        params: BTreeMap::from([
            ("frequency".to_string(), ParamValue::F32(730.0)),
            ("crossover_type".to_string(), ParamValue::I32(1)),
            ("recombination_mode".to_string(), ParamValue::I32(1)),
            (
                "num_bands".to_string(),
                ParamValue::I32((num_bands - 2) as i32),
            ),
            ("frequency_2".to_string(), ParamValue::F32(2_100.0)),
            ("frequency_3".to_string(), ParamValue::F32(5_400.0)),
        ]),
        fields: BTreeMap::new(),
    };
    serde_json::to_vec(&state).unwrap()
}

fn legacy_band_split_state_without_band_count() -> Vec<u8> {
    // This is the complete pre-AUD143 NIH state schema for BandSplit: the old
    // wrapper exposed only these two static controls.
    let state = PluginState {
        version: String::from("legacy-test"),
        params: BTreeMap::from([
            ("frequency".to_string(), ParamValue::F32(730.0)),
            ("crossover_type".to_string(), ParamValue::I32(1)),
        ]),
        fields: BTreeMap::new(),
    };
    serde_json::to_vec(&state).unwrap()
}

#[VST3(implements(IBStream))]
struct TestMemoryStream {
    bytes: Rc<RefCell<Vec<u8>>>,
    cursor: Rc<Cell<usize>>,
    writable: bool,
}

impl TestMemoryStream {
    fn new(initial: &[u8], writable: bool) -> (Box<Self>, Rc<RefCell<Vec<u8>>>) {
        let bytes = Rc::new(RefCell::new(initial.to_vec()));
        let cursor = Rc::new(Cell::new(0));
        (Self::allocate(Rc::clone(&bytes), cursor, writable), bytes)
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
        if requested != 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let cursor = self.cursor.get();
        let count = requested.min(bytes.len().saturating_sub(cursor));
        if count > 0 {
            // SAFETY: the VST3 caller supplied writable storage for `requested` bytes and `count`
            // is bounded by both the source and destination regions.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), buffer.cast(), count)
            };
        }
        self.cursor.set(cursor + count);
        if !num_bytes_read.is_null() {
            // SAFETY: the optional out pointer is valid when non-null per the VST3 ABI.
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
        if !self.writable || (count != 0 && buffer.is_null()) {
            return kResultFalse;
        }
        let cursor = self.cursor.get();
        let Some(end) = cursor.checked_add(count) else {
            return kInvalidArgument;
        };
        let mut bytes = self.bytes.borrow_mut();
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        if count > 0 {
            // SAFETY: the VST3 caller supplied readable storage for `count` bytes and the vector
            // has been resized to hold the complete write.
            unsafe {
                std::ptr::copy_nonoverlapping(buffer.cast(), bytes.as_mut_ptr().add(cursor), count)
            };
        }
        self.cursor.set(end);
        if !num_bytes_written.is_null() {
            // SAFETY: the optional out pointer is valid when non-null per the VST3 ABI.
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
        let target = base + i128::from(pos);
        let Ok(target) = usize::try_from(target) else {
            return kInvalidArgument;
        };
        self.cursor.set(target);
        if !result.is_null() {
            // SAFETY: the optional out pointer is valid when non-null per the VST3 ABI.
            unsafe { *result = target as i64 };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: VST3 requires a writable output pointer for `tell()`.
        unsafe { *pos = self.cursor.get() as i64 };
        kResultOk
    }
}

unsafe fn shared_stream(stream: &VstPtr<dyn IBStream>) -> SharedVstPtr<dyn IBStream> {
    // SAFETY: SharedVstPtr is a transparent borrowed ABI pointer; its use is synchronous and the
    // owning VstPtr remains alive for the entire call.
    unsafe { std::mem::transmute(stream.as_ptr()) }
}

fn set_component_state(component: &VstPtr<dyn IComponent>, bytes: &[u8]) {
    assert_eq!(set_component_state_result(component, bytes), kResultOk);
}

fn set_component_state_result(component: &VstPtr<dyn IComponent>, bytes: &[u8]) -> tresult {
    let (stream, _) = TestMemoryStream::new(bytes, false);
    // SAFETY: `Box::into_raw` transfers the COM object to an owning VstPtr exactly once.
    let stream = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap() };
    // SAFETY: the stream pointer is valid and retained until the synchronous set_state call ends.
    unsafe { component.set_state(shared_stream(&stream)) }
}

fn read_component_state(component: &VstPtr<dyn IComponent>) -> PluginState {
    let (stream, bytes) = TestMemoryStream::new(&[], true);
    // SAFETY: `Box::into_raw` transfers the COM object to an owning VstPtr exactly once.
    let stream = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap() };
    // SAFETY: the stream pointer remains owned until this synchronous callback returns.
    assert_eq!(
        unsafe { component.get_state(shared_stream(&stream)) },
        kResultOk
    );
    serde_json::from_slice(&bytes.borrow()).expect("VST3 state is valid JSON")
}

#[test]
fn vst3_bandsplit_discovery_and_legacy_packed_layout_keep_four_bus_slots() {
    use vst3_sys::base::kResultFalse;

    let (_wrapper, processor, component) = new_interfaces();
    // SAFETY: these callbacks are invoked with valid output pointers and live interface objects.
    unsafe {
        assert_eq!(
            component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kInput as i32),
            1
        );
        assert_eq!(
            component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kOutput as i32),
            4
        );

        let mut info = std::mem::zeroed::<BusInfo>();
        for index in 0..4 {
            assert_eq!(
                component.get_bus_info(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    index,
                    &mut info
                ),
                kResultOk
            );
            assert_eq!(info.channel_count, 2);
            assert_eq!(bus_name(&info), format!("Band {}", index + 1));
            let active_by_default = info.flags & BusFlags::kDefaultActive as u32 != 0;
            assert_eq!(active_by_default, index < 2, "bus {index}");
            let mut arrangement = 0;
            assert_eq!(
                processor.get_bus_arrangement(
                    BusDirections::kOutput as i32,
                    index,
                    &mut arrangement
                ),
                kResultOk
            );
            assert_eq!(arrangement, STEREO);
        }

        // The prior release used one four-channel main bus. The callback accepts that exact saved
        // request while keeping the advertised bus count at four for subsequent host queries.
        let mut input = STEREO;
        let mut output = LEGACY_PACKED;
        assert_eq!(
            processor.set_bus_arrangements(&mut input, 1, &mut output, 1),
            kResultOk
        );
        assert_eq!(
            component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kOutput as i32),
            4
        );
        assert_eq!(
            component.get_bus_info(
                MediaTypes::kAudio as i32,
                BusDirections::kOutput as i32,
                0,
                &mut info
            ),
            kResultOk
        );
        assert_eq!(info.channel_count, 4);
        assert_eq!(bus_name(&info), "Bands 1+2 (legacy packed)");
        let mut arrangement = 0;
        assert_eq!(
            processor.get_bus_arrangement(BusDirections::kOutput as i32, 0, &mut arrangement),
            kResultOk
        );
        assert_eq!(arrangement, LEGACY_PACKED);
        for index in 1..4 {
            assert_eq!(
                component.get_bus_info(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    index,
                    &mut info
                ),
                kResultOk
            );
            assert_eq!(info.channel_count, 2);
            assert_eq!(
                bus_name(&info),
                match index {
                    1 => "Band 3",
                    2 => "Band 4",
                    3 => "Legacy compatibility slot",
                    _ => unreachable!(),
                }
            );
            assert_eq!(
                processor.get_bus_arrangement(
                    BusDirections::kOutput as i32,
                    index,
                    &mut arrangement
                ),
                kResultOk
            );
            assert_eq!(arrangement, STEREO);
            assert_eq!(info.flags & BusFlags::kDefaultActive as u32, 0);
        }
        // A near match is rejected. Compatibility is limited to the exact old packed arrangement.
        let mut near_miss = LEGACY_PACKED ^ (1 << 7);
        assert_eq!(
            processor.set_bus_arrangements(&mut input, 1, &mut near_miss, 1),
            kResultFalse
        );
    }
}

#[test]
fn vst3_bandsplit_legacy_packed_process_preserves_old_audio_and_silences_reserved_slot() {
    let (_wrapper, processor, component) = new_interfaces();
    set_component_state(&component, &band_split_state(2));
    let params = restored_params(
        "BandSplit",
        &[
            ("frequency", 730.0),
            ("crossover_type", 1.0),
            ("recombination_mode", 1.0),
            ("num_bands", 0.0),
            ("frequency_2", 2_100.0),
            ("frequency_3", 5_400.0),
        ],
    );
    let mut reference = reference_split(&params, 2);
    reference.reset();

    // SAFETY: callbacks use valid arrangements and a live component lifecycle.
    unsafe {
        let mut input_arrangement = STEREO;
        let mut output_arrangement = LEGACY_PACKED;
        assert_eq!(
            processor.set_bus_arrangements(&mut input_arrangement, 1, &mut output_arrangement, 1),
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
    }
    set_processing_setup(&processor);
    // SAFETY: activation follows initialization and setup_processing.
    unsafe {
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);
    }

    let (interleaved_input, input_channels, mut input_pointers) = test_input();
    let mut output_channels = guarded_output_channels(4);
    let mut output_pointers = output_channel_pointers(&mut output_channels);
    let mut input_bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: input_pointers.as_mut_ptr(),
    };
    let mut output_bus = AudioBusBuffers {
        num_channels: 4,
        silence_flags: 0,
        buffers: output_pointers.as_mut_ptr(),
    };
    let mut process = process_data(&mut input_bus, 1, &mut output_bus, 1);
    let expected = reference_block(reference.as_mut(), &interleaved_input, 4);
    // SAFETY: input/output channel allocations outlive this callback and are disjoint.
    unsafe { assert_eq!(processor.process(&mut process), kResultOk) };
    assert_bus_matches(&output_channels, 0, &expected, 0, 4, 4);
    assert!(
        output_channels
            .iter()
            .flatten()
            .all(|sample| sample.is_finite())
    );
    assert_output_guards(&output_channels);

    // Activating the reserved fourth slot is explicitly supported as a silent output. It does not
    // imply a fifth DSP band; inactive buses still provide width-sized arrays with null samples.
    // SAFETY: set_processing/set_active stop the current DSP before activation changes.
    unsafe {
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(
            component.activate_bus(
                MediaTypes::kAudio as i32,
                BusDirections::kOutput as i32,
                3,
                1
            ),
            kResultOk
        );
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);
    }

    output_channels
        .iter_mut()
        .for_each(|channel| channel.fill(STEP_CANARY));
    reference.reset();
    let expected = reference_block(reference.as_mut(), &interleaved_input, 4);
    let mut reserved_channels = guarded_output_channels(2);
    let mut reserved_pointers = output_channel_pointers(&mut reserved_channels);
    let mut inactive_bus_one_pointers = vec![std::ptr::null_mut::<c_void>(); 2];
    let mut inactive_bus_two_pointers = vec![std::ptr::null_mut::<c_void>(); 2];
    let mut output_buses = [
        AudioBusBuffers {
            num_channels: 4,
            silence_flags: 0,
            buffers: output_pointers.as_mut_ptr(),
        },
        AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: inactive_bus_one_pointers.as_mut_ptr(),
        },
        AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: inactive_bus_two_pointers.as_mut_ptr(),
        },
        AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: reserved_pointers.as_mut_ptr(),
        },
    ];
    process.num_outputs = 4;
    process.outputs = output_buses.as_mut_ptr();
    // SAFETY: every bus has a width-sized pointer array; inactive buses contain null sample
    // pointers, and active bus 0 plus reserved bus 3 have valid disjoint buffers.
    unsafe { assert_eq!(processor.process(&mut process), kResultOk) };
    assert_bus_matches(&output_channels, 0, &expected, 0, 4, 4);
    assert_output_region_is(&reserved_channels, 0.0);
    assert_output_guards(&output_channels);
    assert_output_guards(&reserved_channels);
    assert!(
        input_channels
            .iter()
            .flatten()
            .all(|sample| sample.is_finite())
    );

    // SAFETY: finish the successful VST3 lifecycle in host order.
    unsafe {
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}

#[test]
fn vst3_bandsplit_legacy_packed_routes_band_three_and_four_to_declared_aux_buses() {
    let (_wrapper, processor, component) = new_interfaces();
    set_component_state(&component, &band_split_state(4));

    let params = restored_params(
        "BandSplit",
        &[
            ("frequency", 730.0),
            ("crossover_type", 1.0),
            ("recombination_mode", 1.0),
            ("num_bands", 2.0),
            ("frequency_2", 2_100.0),
            ("frequency_3", 5_400.0),
        ],
    );
    let mut reference = reference_split(&params, 4);
    reference.reset();

    // Preserve the old packed main-bus request. Only bands 3 and 4 are activated on the
    // auxiliary buses; the final reserved compatibility slot remains inactive and absent.
    // SAFETY: the exact legacy arrangement is declared, and each activated index exists in the
    // four-bus compatibility layout.
    unsafe {
        let mut input_arrangement = STEREO;
        let mut output_arrangement = LEGACY_PACKED;
        assert_eq!(
            processor.set_bus_arrangements(&mut input_arrangement, 1, &mut output_arrangement, 1),
            kResultOk
        );
        for bus_index in 0..3 {
            assert_eq!(
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    bus_index,
                    1
                ),
                kResultOk
            );
        }
    }
    set_processing_setup(&processor);
    // SAFETY: all required setup has completed before activation and processing.
    unsafe {
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);
    }

    let (input, _input_channels, mut input_pointers) = test_input();
    let expected = reference_block(reference.as_mut(), &input, 8);
    let mut main_channels = guarded_output_channels(4);
    let mut band_three_channels = guarded_output_channels(2);
    let mut band_four_channels = guarded_output_channels(2);
    let mut main_pointers = output_channel_pointers(&mut main_channels);
    let mut band_three_pointers = output_channel_pointers(&mut band_three_channels);
    let mut band_four_pointers = output_channel_pointers(&mut band_four_channels);
    let mut input_bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: input_pointers.as_mut_ptr(),
    };
    let mut output_buses = [
        AudioBusBuffers {
            num_channels: 4,
            silence_flags: 0,
            buffers: main_pointers.as_mut_ptr(),
        },
        AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: band_three_pointers.as_mut_ptr(),
        },
        AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: band_four_pointers.as_mut_ptr(),
        },
    ];
    let mut process = process_data(
        &mut input_bus,
        1,
        output_buses.as_mut_ptr(),
        output_buses.len() as i32,
    );

    // SAFETY: main and auxiliary output buffers are separate guarded allocations and match the
    // legacy layout's declared 4+2+2 active channels.
    unsafe { assert_eq!(processor.process(&mut process), kResultOk) };
    assert_bus_matches(&main_channels, 0, &expected, 0, 8, 4);
    assert_bus_matches(&band_three_channels, 0, &expected, 4, 8, 2);
    assert_bus_matches(&band_four_channels, 0, &expected, 6, 8, 2);
    assert_output_guards(&main_channels);
    assert_output_guards(&band_three_channels);
    assert_output_guards(&band_four_channels);

    // SAFETY: stop and deactivate before releasing the component.
    unsafe {
        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
        assert_eq!(component.terminate(), kResultOk);
    }
}

#[test]
fn vst3_bandsplit_sparse_four_bus_route_preserves_saved_band_count_across_reactivation() {
    for num_bands in [3, 4] {
        let (_wrapper, processor, component) = new_interfaces();
        let saved_state = band_split_state(num_bands);
        set_component_state(&component, &saved_state);

        let params = restored_params(
            "BandSplit",
            &[
                ("frequency", 730.0),
                ("crossover_type", 1.0),
                ("recombination_mode", 1.0),
                ("num_bands", (num_bands - 2) as f64),
                ("frequency_2", 2_100.0),
                ("frequency_3", 5_400.0),
            ],
        );
        let mut reference = reference_split(&params, num_bands);
        reference.reset();

        // Fresh hosts discover max-four stereo buses. Keep only the main bus active for a saved
        // three-band instance, and select the non-contiguous Band 4 bus for a saved four-band
        // instance. Inactive intermediate buses have no channel-pointer array.
        let sparse_bus = (num_bands == 4).then_some(3);
        // SAFETY: all bus arrangements match the declared max-four layout.
        unsafe {
            let mut input_arrangements = [STEREO];
            let mut output_arrangements = [STEREO; 4];
            assert_eq!(
                processor.set_bus_arrangements(
                    input_arrangements.as_mut_ptr(),
                    1,
                    output_arrangements.as_mut_ptr(),
                    4
                ),
                kResultOk
            );
            assert_eq!(
                component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kOutput as i32),
                4
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
            if let Some(bus_index) = sparse_bus {
                assert_eq!(
                    component.activate_bus(
                        MediaTypes::kAudio as i32,
                        BusDirections::kOutput as i32,
                        bus_index,
                        1
                    ),
                    kResultOk
                );
            }
        }
        set_processing_setup(&processor);

        for cycle in 0..3 {
            // SAFETY: first activation follows setup; later cycles perform ordinary deactivate/activate
            // without changing bus activation, which must not recompute the saved DSP band count.
            unsafe {
                assert_eq!(component.set_active(1), kResultOk);
                assert_eq!(processor.set_processing(1), kResultOk);
            }

            let (_, mut input_channels, mut input_pointers) = test_input();
            let (left, right) = input_channels.split_at_mut(1);
            for (frame, (left, right)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
                let variation = (cycle as f32 + 1.0) * 0.001;
                *left += variation * (frame as f32 + 1.0);
                *right -= variation * 0.5;
            }
            let input: Vec<f32> = (0..FRAMES)
                .flat_map(|frame| [input_channels[0][frame], input_channels[1][frame]])
                .collect();
            let expected = reference_block(reference.as_mut(), &input, num_bands * 2);
            let mut output_channels = guarded_output_channels(num_bands * 2);
            let output_pointers = output_channel_pointers(&mut output_channels);
            let mut main_output_pointers = [output_pointers[0], output_pointers[1]];
            let mut sparse_output_pointers = [output_pointers[2], output_pointers[3]];
            let mut input_bus = AudioBusBuffers {
                num_channels: 2,
                silence_flags: 0,
                buffers: input_pointers.as_mut_ptr(),
            };
            let mut output_buses = [
                AudioBusBuffers {
                    num_channels: 2,
                    silence_flags: 0,
                    buffers: main_output_pointers.as_mut_ptr(),
                },
                AudioBusBuffers {
                    num_channels: 2,
                    silence_flags: 0,
                    buffers: std::ptr::null_mut(),
                },
                AudioBusBuffers {
                    num_channels: 2,
                    silence_flags: 0,
                    buffers: std::ptr::null_mut(),
                },
                AudioBusBuffers {
                    num_channels: 2,
                    silence_flags: 0,
                    buffers: sparse_output_pointers.as_mut_ptr(),
                },
            ];
            let mut process = process_data(
                &mut input_bus,
                1,
                output_buses.as_mut_ptr(),
                if sparse_bus.is_some() { 4 } else { 1 },
            );
            // SAFETY: active output buffers are disjoint. For the four-band case, inactive buses
            // 1 and 2 have null pointer arrays while bus 3 has two valid guarded channels.
            unsafe { assert_eq!(processor.process(&mut process), kResultOk) };
            assert_bus_matches(&output_channels, 0, &expected, 0, num_bands * 2, 2);
            if sparse_bus.is_some() {
                assert_bus_matches(&output_channels, 2, &expected, 6, num_bands * 2, 2);
            }
            let untouched_start = if sparse_bus.is_some() { 4 } else { 2 };
            assert_output_region_is(&output_channels[untouched_start..], STEP_CANARY);
            assert_output_guards(&output_channels);
            assert!(
                output_channels
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite())
            );

            // Capture state after the actual callback and verify the saved structural index agrees
            // with the restored DSP layout; unrelated frequency/mode controls remain serialized.
            let (state_stream, state_bytes) = TestMemoryStream::new(&[], true);
            // SAFETY: `Box::into_raw` transfers the COM object to an owning VstPtr exactly once.
            let state_stream = unsafe {
                VstPtr::<dyn IBStream>::owned(Box::into_raw(state_stream).cast()).unwrap()
            };
            // SAFETY: state stream remains owned until get_state returns and the method writes only
            // through the provided valid stream interface.
            assert_eq!(
                unsafe { component.get_state(shared_stream(&state_stream)) },
                kResultOk
            );
            let bytes = state_bytes.borrow();
            let state: PluginState = serde_json::from_slice(&bytes).unwrap();
            assert!(
                matches!(state.params.get("num_bands"), Some(ParamValue::I32(index)) if *index == (num_bands - 2) as i32)
            );
            assert!(
                matches!(state.params.get("frequency"), Some(ParamValue::F32(value)) if (*value - 730.0).abs() < 1.0e-4)
            );
            assert!(matches!(
                state.params.get("recombination_mode"),
                Some(ParamValue::I32(1))
            ));

            // SAFETY: stop/deactivate precede reinitialization; do not send activateBus again.
            unsafe {
                assert_eq!(processor.set_processing(0), kResultOk);
                assert_eq!(component.set_active(0), kResultOk);
            }
            reference.reset();
        }

        // SAFETY: finish the final active cycle before releasing the component.
        unsafe {
            assert_eq!(processor.set_processing(0), kResultOk);
            assert_eq!(component.set_active(0), kResultOk);
            assert_eq!(component.terminate(), kResultOk);
        }
    }
}

#[test]
fn vst3_old_full_state_without_band_count_migrates_populated_wide_instance() {
    for num_bands in [3, 4] {
        let (_wrapper, processor, component) = new_interfaces();
        set_component_state(&component, &band_split_state(num_bands));

        // Populate a real wider DSP instance through VST3's negotiated buses before importing
        // the old preset. The old state predates both structural fields.
        // SAFETY: all arrangements match the declared max-four stereo layout.
        unsafe {
            let mut input_arrangements = [STEREO];
            let mut output_arrangements = [STEREO; 4];
            assert_eq!(
                processor.set_bus_arrangements(
                    input_arrangements.as_mut_ptr(),
                    1,
                    output_arrangements.as_mut_ptr(),
                    4
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
            for bus_index in 1..num_bands {
                assert_eq!(
                    component.activate_bus(
                        MediaTypes::kAudio as i32,
                        BusDirections::kOutput as i32,
                        bus_index as i32,
                        1
                    ),
                    kResultOk
                );
            }
        }
        set_processing_setup(&processor);
        // SAFETY: setup and bus activation precede component activation.
        unsafe {
            assert_eq!(component.set_active(1), kResultOk);
            assert_eq!(processor.set_processing(1), kResultOk);
        }

        let wide_params = restored_params(
            "BandSplit",
            &[
                ("frequency", 730.0),
                ("crossover_type", 1.0),
                ("recombination_mode", 1.0),
                ("num_bands", (num_bands - 2) as f64),
                ("frequency_2", 2_100.0),
                ("frequency_3", 5_400.0),
            ],
        );
        let mut wide_reference = reference_split(&wide_params, num_bands);
        wide_reference.reset();
        let wide_input = test_input().0;
        let expected_wide = reference_block(wide_reference.as_mut(), &wide_input, num_bands * 2);
        let actual_wide = process_wide_bus_block(
            &processor,
            num_bands,
            "initial populated wide route",
            kResultOk,
        );
        assert_eq!(actual_wide.len(), expected_wide.len());
        for (sample_index, (actual, expected)) in
            actual_wide.iter().zip(expected_wide.iter()).enumerate()
        {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "wide setup frame={}, channel={}: {actual} vs {expected}",
                sample_index / (num_bands * 2),
                sample_index % (num_bands * 2)
            );
        }

        let legacy_state = legacy_band_split_state_without_band_count();
        assert_eq!(
            set_component_state_result(&component, &legacy_state),
            kResultFalse
        );
        let migrated = read_component_state(&component);
        assert!(matches!(
            migrated.params.get("num_bands"),
            Some(ParamValue::I32(0))
        ));
        assert!(matches!(
            migrated.params.get("recombination_mode"),
            Some(ParamValue::I32(0))
        ));
        assert!(matches!(
            migrated.params.get("frequency"),
            Some(ParamValue::F32(value)) if (*value - 730.0).abs() <= f32::EPSILON
        ));
        assert!(matches!(
            migrated.params.get("crossover_type"),
            Some(ParamValue::I32(1))
        ));

        let refused_output = process_wide_bus_block(
            &processor,
            num_bands,
            "same instance after incompatible old-state refusal",
            kResultFalse,
        );
        assert!(refused_output.iter().all(|sample| *sample == 0.0));

        // SAFETY: stop processing before changing the host bus arrangement.
        unsafe {
            assert_eq!(processor.set_processing(0), kResultOk);
            assert_eq!(component.set_active(0), kResultOk);
        }

        // The incompatible restore was refused. The host now negotiates a compatible packed
        // two-band layout before retrying the same full old preset.
        // SAFETY: the component is inactive and the requested layout is advertised.
        unsafe {
            let mut input_arrangement = STEREO;
            let mut output_arrangement = LEGACY_PACKED;
            assert_eq!(
                processor.set_bus_arrangements(
                    &mut input_arrangement,
                    1,
                    &mut output_arrangement,
                    1
                ),
                kResultOk
            );
            for bus_index in 1..4 {
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
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    0,
                    1
                ),
                kResultOk
            );
        }
        assert_eq!(
            set_component_state_result(&component, &legacy_state),
            kResultOk
        );
        set_processing_setup(&processor);
        // SAFETY: the compatible packed geometry is negotiated before activation.
        unsafe {
            assert_eq!(component.set_active(1), kResultOk);
            assert_eq!(processor.set_processing(1), kResultOk);
        }
        let restored = read_component_state(&component);
        assert!(matches!(
            restored.params.get("num_bands"),
            Some(ParamValue::I32(0))
        ));
        assert!(matches!(
            restored.params.get("recombination_mode"),
            Some(ParamValue::I32(0))
        ));
        assert!(matches!(
            restored.params.get("frequency"),
            Some(ParamValue::F32(value)) if (*value - 730.0).abs() <= f32::EPSILON
        ));
        assert!(matches!(
            restored.params.get("crossover_type"),
            Some(ParamValue::I32(1))
        ));

        let legacy_params = restored_params(
            "BandSplit",
            &[
                ("frequency", 730.0),
                ("crossover_type", 1.0),
                ("recombination_mode", 0.0),
                ("num_bands", 0.0),
            ],
        );
        let mut legacy_reference = reference_split(&legacy_params, 2);
        legacy_reference.reset();
        let expected_legacy = reference_block(legacy_reference.as_mut(), &test_input().0, 4);
        let actual_legacy = process_legacy_packed_block(&processor);
        assert_eq!(actual_legacy.len(), expected_legacy.len());
        for (sample_index, (actual, expected)) in
            actual_legacy.iter().zip(expected_legacy.iter()).enumerate()
        {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "migrated two-band frame={}, channel={}: {actual} vs {expected}",
                sample_index / 4,
                sample_index % 4
            );
        }

        // SAFETY: stop processing and deactivate before terminating the component.
        unsafe {
            assert_eq!(processor.set_processing(0), kResultOk);
            assert_eq!(component.set_active(0), kResultOk);
            assert_eq!(component.terminate(), kResultOk);
        }
    }
}
