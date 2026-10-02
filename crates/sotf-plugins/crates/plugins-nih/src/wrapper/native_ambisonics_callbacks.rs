//! Exercise the generated CLAP C callback boundary for Ambisonics layouts.
use super::*;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_input_events,
};
use clap_sys::ext::ambisonic::{
    CLAP_AMBISONIC_NORMALIZATION_SN3D, CLAP_AMBISONIC_ORDERING_ACN, CLAP_EXT_AMBISONIC,
    clap_ambisonic_config, clap_plugin_ambisonic,
};
use clap_sys::ext::audio_ports::{
    CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_plugin_audio_ports_config,
};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use clap_sys::ext::surround::{CLAP_EXT_SURROUND, clap_plugin_surround};
use clap_sys::host::clap_host;
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use std::ffi::{CStr, c_char, c_void};

// Canonical DSP slot order per TARGET_LAYOUTS (5.1.4 at 3, 7.1.2 at 4);
// role bytes equal the host advertised `clap_channel_map` tables.
const TEST_CLAP_MAPS: [&[u8]; 6] = [
    &[0, 1, 2, 3, 9, 10],
    &[0, 1, 2, 3, 9, 10, 4, 5],
    &[0, 1, 2, 3, 9, 10, 12, 14],
    &[0, 1, 2, 3, 9, 10, 12, 14, 15, 17],
    &[0, 1, 2, 3, 9, 10, 4, 5, 12, 14],
    &[0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17],
];

const FRAME_CANARY: f32 = 123_456.75;
const CHANNEL_CANARY: f32 = -98_765.5;
const FRAME_CANARY_F64: f64 = 12_345_678.75;

fn guarded_f32_channel(samples: &[f32]) -> Vec<f32> {
    let mut guarded = vec![FRAME_CANARY; samples.len() + 2];
    guarded[1..samples.len() + 1].copy_from_slice(samples);
    guarded
}

fn assert_frame_canaries(channels: &[Vec<f32>], frames: usize) {
    for channel in channels {
        assert_eq!(channel[0], FRAME_CANARY);
        assert_eq!(channel[frames + 1], FRAME_CANARY);
    }
}

unsafe extern "C" fn no_extension(_host: *const clap_host, _id: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn no_host_request(_host: *const clap_host) {}

fn test_host() -> Box<clap_host> {
    Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF native callback test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_host_request),
        request_process: Some(no_host_request),
        request_callback: Some(no_host_request),
    })
}

fn extension<T>(plugin: *const clap_plugin, id: &CStr) -> *const T {
    // SAFETY: the wrapper is live and its get_extension callback returns a stable extension table.
    unsafe { ((*plugin).get_extension.unwrap())(plugin, id.as_ptr()).cast::<T>() }
}

unsafe extern "C" fn param_event_count(list: *const clap_input_events) -> u32 {
    // SAFETY: the list context points to the borrowed event slice in `set_bool_param`.
    let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
    events.len() as u32
}

unsafe extern "C" fn param_event_at(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    // SAFETY: the list context points to the borrowed event slice in `set_bool_param`.
    let events = unsafe { (*list).ctx.cast::<&[clap_event_param_value]>().read() };
    events
        .get(index as usize)
        .map_or(std::ptr::null(), |event| &event.header)
}

fn set_bool_param(plugin: *const clap_plugin, name: &str, value: f64) -> f64 {
    let params = extension::<clap_plugin_params>(plugin, CLAP_EXT_PARAMS);
    assert!(!params.is_null());

    // SAFETY: params info and flush callbacks are queried from the live plugin; the event slice is
    // kept alive until the synchronous flush callback returns.
    unsafe {
        let mut parameter_id = None;
        for index in 0..((*params).count.unwrap())(plugin) {
            let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
            assert!((*params).get_info.unwrap()(
                plugin,
                index,
                info.as_mut_ptr()
            ));
            let info = info.assume_init();
            if CStr::from_ptr(info.name.as_ptr()).to_bytes() == name.as_bytes() {
                parameter_id = Some(info.id);
                break;
            }
        }
        let parameter_id = parameter_id.unwrap_or_else(|| panic!("missing CLAP parameter {name}"));
        let event = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: parameter_id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        let events = [event];
        let mut event_slice: &[clap_event_param_value] = &events;
        let event_list = clap_input_events {
            ctx: (&mut event_slice as *mut &[clap_event_param_value]).cast(),
            size: Some(param_event_count),
            get: Some(param_event_at),
        };
        ((*params).flush.unwrap())(plugin, &event_list, std::ptr::null());

        let mut applied = f64::NAN;
        assert!((*params).get_value.unwrap()(
            plugin,
            parameter_id,
            &mut applied
        ));
        applied
    }
}

#[test]
fn clap_callbacks_negotiate_and_process_full_order_seven_vector() {
    const FRAMES: usize = 31;
    const LAYOUTS: usize = 42;
    const ORDER_SEVEN_START: usize = 36;
    const ORDER_SEVEN_TARGET_714: usize = 41;

    let host = test_host();
    // SAFETY: the boxed host stays alive for the wrapper and its callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<AmbisonicsWrapper>::new(&*host) };
    let plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: invoke the actual generated CLAP lifecycle callbacks in host order.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
    }

    let configs = extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG);
    let ports = extension::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS);
    let ambisonic = extension::<clap_plugin_ambisonic>(plugin, CLAP_EXT_AMBISONIC);
    let surround = extension::<clap_plugin_surround>(plugin, CLAP_EXT_SURROUND);
    assert!(!configs.is_null() && !ports.is_null() && !ambisonic.is_null() && !surround.is_null());

    // Query every standardized tuple through the host ABI extension callbacks.
    // CLAP has six truthful output maps; wide 9.1 targets are deliberately absent.
    unsafe {
        assert_eq!((*configs).count.unwrap()(plugin) as usize, LAYOUTS);
        for index in 0..LAYOUTS {
            let mut config: clap_audio_ports_config = std::mem::zeroed();
            assert!((*configs).get.unwrap()(plugin, index as u32, &mut config));
            let order = index / 6 + 1;
            let target = index % 6;
            assert_eq!(config.id, index as u32);
            assert!(config.has_main_input && config.has_main_output);
            assert_eq!(config.input_port_count, 1);
            assert_eq!(config.output_port_count, 1);
            assert_eq!(config.main_input_channel_count as usize, (order + 1).pow(2));
            assert_eq!(
                config.main_output_channel_count,
                [6, 8, 8, 10, 10, 12][target]
            );
            assert_eq!(
                CStr::from_ptr(config.main_input_port_type).to_bytes(),
                b"ambisonic"
            );
            assert_eq!(
                CStr::from_ptr(config.main_output_port_type).to_bytes(),
                b"surround"
            );
            assert!((*configs).select.unwrap()(plugin, config.id));

            let mut input_info: clap_audio_port_info = std::mem::zeroed();
            let mut output_info: clap_audio_port_info = std::mem::zeroed();
            assert!((*ports).get.unwrap()(plugin, 0, true, &mut input_info));
            assert!((*ports).get.unwrap()(plugin, 0, false, &mut output_info));
            assert_eq!(input_info.channel_count as usize, (order + 1).pow(2));
            assert_eq!(output_info.channel_count, config.main_output_channel_count);
            assert_eq!(
                CStr::from_ptr(input_info.port_type).to_bytes(),
                b"ambisonic"
            );
            assert_eq!(
                CStr::from_ptr(output_info.port_type).to_bytes(),
                b"surround"
            );

            let mut ambisonic_config: clap_ambisonic_config = std::mem::zeroed();
            assert!((*ambisonic).get_config.unwrap()(
                plugin,
                true,
                0,
                &mut ambisonic_config
            ));
            assert_eq!(ambisonic_config.ordering, CLAP_AMBISONIC_ORDERING_ACN);
            assert_eq!(
                ambisonic_config.normalization,
                CLAP_AMBISONIC_NORMALIZATION_SN3D
            );
            assert!(!(*ambisonic).get_config.unwrap()(
                plugin,
                false,
                0,
                &mut ambisonic_config
            ));

            let mut channel_map = [0_u8; 16];
            let mapped = (*surround).get_channel_map.unwrap()(
                plugin,
                false,
                0,
                channel_map.as_mut_ptr(),
                channel_map.len() as u32,
            );
            assert_eq!(mapped as usize, config.main_output_channel_count as usize);
            assert_eq!(&channel_map[..mapped as usize], TEST_CLAP_MAPS[target]);
            assert_eq!(
                (*surround).get_channel_map.unwrap()(
                    plugin,
                    true,
                    0,
                    channel_map.as_mut_ptr(),
                    channel_map.len() as u32,
                ),
                0,
                "Ambisonics input must not be mislabeled as a surround map"
            );
        }
    }

    assert_eq!(ORDER_SEVEN_START, 6 * 6);
    assert_eq!(ORDER_SEVEN_TARGET_714, LAYOUTS - 1);

    let config_index = ORDER_SEVEN_TARGET_714;
    let layout = &<AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts()[config_index];
    let input_channels = 64;
    let output_channels = 12;
    let mut reference = AmbisonicsWrapper::default();
    initialize_on_layout(&mut reference, layout, PluginApi::Clap);
    let input: Vec<Vec<f32>> = (0..input_channels)
        .map(|channel| {
            (0..FRAMES)
                .map(|frame| {
                    let code = (frame * 43 + channel * 71 + frame * channel * 11) % 997;
                    (code as f32 - 498.0) * 0.000_02
                })
                .collect()
        })
        .collect();
    let mut reference_channels = input.to_vec();
    reference_channels.resize_with(input_channels.max(output_channels), || vec![0.0; FRAMES]);
    let mut reference_buffer = Buffer::default();
    // SAFETY: channel vectors are disjoint, equal length, and live through processing.
    unsafe {
        reference_buffer.set_slices(FRAMES, |slices| {
            slices.extend(reference_channels.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let mut auxiliary = AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    let direct_status = reference.process_without_transport(
        &mut reference_buffer,
        &mut auxiliary,
        &mut TestContext,
    );
    assert!(
        !matches!(direct_status, ProcessStatus::Error(_)),
        "{direct_status:?}"
    );

    // The final standard CLAP tuple is order-seven 7.1.4: 64 inputs and 12 outputs.
    // Activate it and process through the actual clap_plugin::process function pointer.
    unsafe {
        assert!((*configs).select.unwrap()(plugin, config_index as u32));
        assert!(((*plugin).activate.unwrap())(
            plugin,
            48_000.0,
            1,
            FRAMES as u32
        ));
        assert!(((*plugin).start_processing.unwrap())(plugin));
    }
    let mut input_storage: Vec<Vec<f32>> = input
        .iter()
        .map(|channel| guarded_f32_channel(channel))
        .collect();
    let mut input_ptrs: Vec<*mut f32> = input_storage
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let mut output: Vec<Vec<f32>> = (0..output_channels)
        .map(|_| vec![FRAME_CANARY; FRAMES + 2])
        .collect();
    output
        .iter_mut()
        .for_each(|channel| channel[1..FRAMES + 1].fill(-0.875));
    // Keep extra host storage behind the advertised output width. A callback that writes using a
    // stale or unnegotiated wider layout must leave these channel canaries untouched.
    let mut extra_output_channels = vec![vec![CHANNEL_CANARY; FRAMES + 2]; 4];
    let mut output_ptrs: Vec<*mut f32> = output
        .iter_mut()
        .chain(extra_output_channels.iter_mut())
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let mut input_bus = clap_audio_buffer {
        data32: input_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: input_channels as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut output_bus = clap_audio_buffer {
        data32: output_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: output_channels as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut process = clap_process {
        steady_time: -1,
        frames_count: FRAMES as u32,
        transport: std::ptr::null(),
        audio_inputs: &input_bus,
        audio_outputs: &mut output_bus,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events: std::ptr::null(),
        out_events: std::ptr::null(),
    };
    // SAFETY: all channel data and pointer arrays remain live through the synchronous callback.
    let status = unsafe { ((*plugin).process.unwrap())(plugin, &process) };
    assert_ne!(status, CLAP_PROCESS_ERROR);
    assert_frame_canaries(&input_storage, FRAMES);
    assert_frame_canaries(&output, FRAMES);
    assert!(
        input_storage
            .iter()
            .zip(&input)
            .all(|(guarded, source)| guarded[1..FRAMES + 1] == source[..])
    );
    assert!(
        extra_output_channels
            .iter()
            .flatten()
            .all(|sample| *sample == CHANNEL_CANARY)
    );
    for (channel, samples) in output.iter().take(output_channels).enumerate() {
        assert_eq!(
            &samples[1..FRAMES + 1],
            reference_buffer.as_slice_immutable()[channel]
        );
    }

    // A host that supplies fewer than the negotiated 64 input channels must be rejected
    // before the DSP sees an incomplete ACN vector, and output is cleared.
    let mut short_ptrs = input_ptrs[..input_channels - 1].to_vec();
    input_bus.data32 = short_ptrs.as_mut_ptr();
    input_bus.channel_count = (input_channels - 1) as u32;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.75);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    assert_frame_canaries(&output, FRAMES);
    assert!(
        extra_output_channels
            .iter()
            .flatten()
            .all(|sample| *sample == CHANNEL_CANARY)
    );

    // Surplus input channels are invalid too: the negotiated ACN vector must have exactly 64
    // channels. The pointer array includes the duplicate slot advertised to the callback.
    let mut surplus_input_ptrs = input_ptrs.clone();
    surplus_input_ptrs.push(input_ptrs[0]);
    input_bus.data32 = surplus_input_ptrs.as_mut_ptr();
    input_bus.channel_count = 65;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.625);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    assert_frame_canaries(&output, FRAMES);
    input_bus.data32 = input_ptrs.as_mut_ptr();
    input_bus.channel_count = input_channels as u32;

    // An absent negotiated primary input is rejected before BufferManager or the DSP runs.
    process.audio_inputs = std::ptr::null();
    process.audio_inputs_count = 0;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.5);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    process.audio_inputs = &input_bus;
    process.audio_inputs_count = 1;

    // An extra host bus is a geometry error even when the primary bus itself is complete.
    let extra_input_bus = clap_audio_buffer {
        data32: input_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: input_channels as u32,
        latency: 0,
        constant_mask: 0,
    };
    let two_input_buses = [input_bus, extra_input_bus];
    process.audio_inputs = two_input_buses.as_ptr();
    process.audio_inputs_count = 2;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.4375);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    process.audio_inputs = &input_bus;
    process.audio_inputs_count = 1;

    let extra_output_bus = clap_audio_buffer {
        data32: output_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: output_channels as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut two_output_buses = [output_bus, extra_output_bus];
    process.audio_outputs = two_output_buses.as_mut_ptr();
    process.audio_outputs_count = 2;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.40625);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    process.audio_outputs = &mut output_bus;
    process.audio_outputs_count = 1;

    // Short and surplus output vectors are rejected. Only the declared intersection is silenced;
    // a surplus host channel and frame guard remain unchanged.
    output_bus.channel_count = 11;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.375);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output[..11]
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    assert!(
        output[11][1..FRAMES + 1]
            .iter()
            .all(|sample| *sample == -0.375)
    );
    assert_frame_canaries(&output, FRAMES);

    output_bus.channel_count = 13;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.25);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
    );
    assert!(
        extra_output_channels
            .iter()
            .flatten()
            .all(|sample| *sample == CHANNEL_CANARY)
    );
    output_bus.channel_count = output_channels as u32;

    // A missing output bus has nowhere to receive the safe-silence response, but is still rejected
    // without touching the host's retained output storage.
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.125);
    }
    process.audio_outputs = std::ptr::null_mut();
    process.audio_outputs_count = 0;
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(output.iter().all(|channel| {
        channel[1..FRAMES + 1]
            .iter()
            .all(|sample| *sample == -0.125)
    }));
    process.audio_outputs = &mut output_bus;
    process.audio_outputs_count = 1;

    // Oversized frames are rejected without reading or writing past the negotiated capacity.
    process.frames_count = (FRAMES + 1) as u32;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.0625);
    }
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(output.iter().all(|channel| {
        channel[1..FRAMES + 1]
            .iter()
            .all(|sample| *sample == -0.0625)
            && channel[FRAMES + 1] == FRAME_CANARY
    }));
    process.frames_count = FRAMES as u32;

    // CLAP has no advertised f64 support here. data64-only input and output buffers must be
    // rejected without reinterpreting their storage as f32 or touching host output memory.
    let mut input64: Vec<Vec<f64>> = vec![vec![FRAME_CANARY_F64; FRAMES + 2]; input_channels];
    let mut output64: Vec<Vec<f64>> = vec![vec![FRAME_CANARY_F64; FRAMES + 2]; output_channels];
    input64
        .iter_mut()
        .for_each(|channel| channel[1..FRAMES + 1].fill(0.125));
    output64
        .iter_mut()
        .for_each(|channel| channel[1..FRAMES + 1].fill(-9.25));
    let mut input64_ptrs: Vec<*mut f64> = input64
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let mut output64_ptrs: Vec<*mut f64> = output64
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let f64_input_bus = clap_audio_buffer {
        data32: std::ptr::null_mut(),
        data64: input64_ptrs.as_mut_ptr(),
        channel_count: input_channels as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut f64_output_bus = clap_audio_buffer {
        data32: std::ptr::null_mut(),
        data64: output64_ptrs.as_mut_ptr(),
        channel_count: output_channels as u32,
        latency: 0,
        constant_mask: 0,
    };
    input_bus.data32 = input_ptrs.as_mut_ptr();
    input_bus.channel_count = input_channels as u32;
    for channel in &mut output {
        channel[1..FRAMES + 1].fill(-0.625);
    }
    process.audio_inputs = &f64_input_bus;
    process.audio_outputs = &mut output_bus;
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(output.iter().all(|channel| {
        channel[1..FRAMES + 1]
            .iter()
            .all(|sample| *sample == -0.625)
    }));
    assert_frame_canaries(&output, FRAMES);
    process.audio_inputs = &input_bus;
    process.audio_outputs = &mut f64_output_bus;
    assert_eq!(
        unsafe { ((*plugin).process.unwrap())(plugin, &process) },
        CLAP_PROCESS_ERROR
    );
    assert!(
        output64
            .iter()
            .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == -9.25))
    );
    assert!(
        input64
            .iter()
            .chain(&output64)
            .all(|channel| channel[0] == FRAME_CANARY_F64)
    );
    assert!(
        input64
            .iter()
            .chain(&output64)
            .all(|channel| channel[FRAMES + 1] == FRAME_CANARY_F64)
    );

    // SAFETY: stop/deactivate follow the successful start/activate lifecycle.
    unsafe {
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
    }
}

#[test]
fn clap_unequal_width_primary_ports_do_not_advertise_in_place_pairing() {
    let host = test_host();
    // SAFETY: the boxed host stays alive for the wrapper and its callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<AmbisonicsWrapper>::new(&*host) };
    let plugin = wrapper.clap_plugin.as_ptr();

    // SAFETY: initialization and extension queries use the live wrapper.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let configs =
            extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG);
        let ports = extension::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS);
        assert!(!configs.is_null() && !ports.is_null());

        for config_id in 0..42 {
            assert!((*configs).select.unwrap()(plugin, config_id));
            let mut input = std::mem::MaybeUninit::<clap_audio_port_info>::uninit();
            let mut output = std::mem::MaybeUninit::<clap_audio_port_info>::uninit();
            assert!((*ports).get.unwrap()(plugin, 0, true, input.as_mut_ptr()));
            assert!((*ports).get.unwrap()(plugin, 0, false, output.as_mut_ptr()));
            let input = input.assume_init();
            let output = output.assume_init();

            assert_ne!(input.channel_count, output.channel_count);
            assert_eq!(input.in_place_pair, CLAP_INVALID_ID);
            assert_eq!(output.in_place_pair, CLAP_INVALID_ID);
        }
    }
}

#[test]
fn clap_layout_selection_is_rejected_until_deactivated() {
    const MAX_FRAMES: u32 = 31;
    const ORDER_SEVEN_714: u32 = 41;
    const ORDER_ONE_51: u32 = 0;

    let host = test_host();
    // SAFETY: the boxed host stays alive for the wrapper and its callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<AmbisonicsWrapper>::new(&*host) };
    let plugin = wrapper.clap_plugin.as_ptr();

    // SAFETY: lifecycle and extension callbacks follow CLAP's host call order.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let configs =
            extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG);
        let ports = extension::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS);
        assert!(!configs.is_null() && !ports.is_null());
        assert!((*configs).select.unwrap()(plugin, ORDER_SEVEN_714));

        assert!(((*plugin).activate.unwrap())(
            plugin, 48_000.0, 1, MAX_FRAMES
        ));
        let mut input: clap_audio_port_info = std::mem::zeroed();
        let mut output: clap_audio_port_info = std::mem::zeroed();
        assert!((*ports).get.unwrap()(plugin, 0, true, &mut input));
        assert!((*ports).get.unwrap()(plugin, 0, false, &mut output));
        assert_eq!(input.channel_count, 64);
        assert_eq!(output.channel_count, 12);

        // Selection must be refused for the entire activated interval, including the
        // activated-but-not-processing and stopped-but-not-deactivated states.
        assert!(!(*configs).select.unwrap()(plugin, ORDER_ONE_51));
        assert!(((*plugin).start_processing.unwrap())(plugin));
        assert!(!(*configs).select.unwrap()(plugin, ORDER_ONE_51));
        ((*plugin).stop_processing.unwrap())(plugin);
        assert!(!(*configs).select.unwrap()(plugin, ORDER_ONE_51));

        // Deactivation permits the layout change. The new metadata must then be visible, and the
        // newly selected configuration must be able to activate again.
        ((*plugin).deactivate.unwrap())(plugin);
        assert!((*configs).select.unwrap()(plugin, ORDER_ONE_51));
        assert!((*ports).get.unwrap()(plugin, 0, true, &mut input));
        assert!((*ports).get.unwrap()(plugin, 0, false, &mut output));
        assert_eq!(input.channel_count, 4);
        assert_eq!(output.channel_count, 6);
        assert!(((*plugin).activate.unwrap())(
            plugin, 48_000.0, 1, MAX_FRAMES
        ));
        ((*plugin).deactivate.unwrap())(plugin);
    }
}

#[test]
fn malformed_clap_callback_does_not_advance_ambisonics_state() {
    const FRAMES: usize = 31;
    const ORDER_SEVEN_714: u32 = 41;
    const INPUT_CHANNELS: usize = 64;
    const OUTPUT_CHANNELS: usize = 12;

    let subject_host = test_host();
    let control_host = test_host();
    // SAFETY: each boxed host stays alive for its wrapper and every callback below.
    let subject =
        unsafe { nih_plug::wrapper::clap::Wrapper::<AmbisonicsWrapper>::new(&*subject_host) };
    let control =
        unsafe { nih_plug::wrapper::clap::Wrapper::<AmbisonicsWrapper>::new(&*control_host) };
    let subject_plugin = subject.clap_plugin.as_ptr();
    let control_plugin = control.clap_plugin.as_ptr();

    // SAFETY: select the same supported layout, then set the structural dual-band value while
    // inactive through the native CLAP parameter-flush path before activating both instances.
    unsafe {
        for plugin in [subject_plugin, control_plugin] {
            assert!(((*plugin).init.unwrap())(plugin));
            let configs =
                extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG);
            assert!((*configs).select.unwrap()(plugin, ORDER_SEVEN_714));
            assert_eq!(set_bool_param(plugin, "Dual-Band", 1.0), 1.0);
            assert!(((*plugin).activate.unwrap())(
                plugin,
                48_000.0,
                1,
                FRAMES as u32,
            ));
            assert!(((*plugin).start_processing.unwrap())(plugin));
        }
    }

    let input: Vec<Vec<f32>> = (0..INPUT_CHANNELS)
        .map(|channel| {
            (0..FRAMES)
                .map(|frame| ((frame * 29 + channel * 47) as f32 - 700.0) * 0.000_01)
                .collect()
        })
        .collect();
    let mut subject_input: Vec<Vec<f32>> = input
        .iter()
        .map(|samples| guarded_f32_channel(samples))
        .collect();
    let mut control_input: Vec<Vec<f32>> = input
        .iter()
        .map(|samples| guarded_f32_channel(samples))
        .collect();
    let mut subject_input_ptrs: Vec<*mut f32> = subject_input
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let mut control_input_ptrs: Vec<*mut f32> = control_input
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let mut subject_output: Vec<Vec<f32>> = (0..OUTPUT_CHANNELS)
        .map(|_| vec![FRAME_CANARY; FRAMES + 2])
        .collect();
    let mut control_output: Vec<Vec<f32>> = (0..OUTPUT_CHANNELS)
        .map(|_| vec![FRAME_CANARY; FRAMES + 2])
        .collect();
    let mut subject_output_ptrs: Vec<*mut f32> = subject_output
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let mut control_output_ptrs: Vec<*mut f32> = control_output
        .iter_mut()
        .map(|channel| unsafe { channel.as_mut_ptr().add(1) })
        .collect();
    let subject_input_bus = clap_audio_buffer {
        data32: subject_input_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: INPUT_CHANNELS as u32,
        latency: 0,
        constant_mask: 0,
    };
    let control_input_bus = clap_audio_buffer {
        data32: control_input_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: INPUT_CHANNELS as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut subject_output_bus = clap_audio_buffer {
        data32: subject_output_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: OUTPUT_CHANNELS as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut control_output_bus = clap_audio_buffer {
        data32: control_output_ptrs.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: OUTPUT_CHANNELS as u32,
        latency: 0,
        constant_mask: 0,
    };
    let mut subject_process = clap_process {
        steady_time: -1,
        frames_count: FRAMES as u32,
        transport: std::ptr::null(),
        audio_inputs: &subject_input_bus,
        audio_outputs: &mut subject_output_bus,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events: std::ptr::null(),
        out_events: std::ptr::null(),
    };
    let control_process = clap_process {
        steady_time: -1,
        frames_count: FRAMES as u32,
        transport: std::ptr::null(),
        audio_inputs: &control_input_bus,
        audio_outputs: &mut control_output_bus,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events: std::ptr::null(),
        out_events: std::ptr::null(),
    };

    // SAFETY: all channel arrays and pointer tables remain alive across synchronous callbacks.
    unsafe {
        assert_ne!(
            ((*subject_plugin).process.unwrap())(subject_plugin, &subject_process),
            CLAP_PROCESS_ERROR
        );
        assert_ne!(
            ((*control_plugin).process.unwrap())(control_plugin, &control_process),
            CLAP_PROCESS_ERROR
        );

        let short_input_bus = clap_audio_buffer {
            channel_count: (INPUT_CHANNELS - 1) as u32,
            ..subject_input_bus
        };
        subject_process.audio_inputs = &short_input_bus;
        for channel in &mut subject_output {
            channel[1..FRAMES + 1].fill(-0.5);
        }
        assert_eq!(
            ((*subject_plugin).process.unwrap())(subject_plugin, &subject_process),
            CLAP_PROCESS_ERROR
        );
        assert!(
            subject_output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        assert_frame_canaries(&subject_output, FRAMES);
        subject_process.audio_inputs = &subject_input_bus;

        assert_ne!(
            ((*subject_plugin).process.unwrap())(subject_plugin, &subject_process),
            CLAP_PROCESS_ERROR
        );
        assert_ne!(
            ((*control_plugin).process.unwrap())(control_plugin, &control_process),
            CLAP_PROCESS_ERROR
        );
    }
    assert!(
        subject_output
            .iter()
            .zip(&control_output)
            .all(|(subject, control)| subject[1..FRAMES + 1] == control[1..FRAMES + 1])
    );

    let previous_output: Vec<Vec<f32>> = control_output
        .iter()
        .map(|channel| channel[1..FRAMES + 1].to_vec())
        .collect();
    for channel in &mut control_output {
        channel[1..FRAMES + 1].fill(-0.5);
    }
    // A valid extra block is a positive sensitivity control: dual-band crossover history must
    // affect the following output, proving this fixture can observe processing-state advances.
    unsafe {
        assert_ne!(
            ((*control_plugin).process.unwrap())(control_plugin, &control_process),
            CLAP_PROCESS_ERROR
        );
    }
    assert!(
        control_output
            .iter()
            .zip(&previous_output)
            .any(|(actual, previous)| actual[1..FRAMES + 1] != previous[..])
    );

    // SAFETY: stop/deactivate follow the successful start/activate lifecycle.
    unsafe {
        for plugin in [subject_plugin, control_plugin] {
            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).deactivate.unwrap())(plugin);
        }
    }
}

#[test]
fn vst3_com_callbacks_negotiate_and_process_order_seven_wide_layout() {
    use nih_plug::wrapper::vst3::{Wrapper, vst3_sys};
    use vst3_sys::base::{IPluginBase, IUnknown, kResultFalse, kResultOk};
    use vst3_sys::vst::{
        AudioBusBuffers, BusDirections, IAudioProcessor, IComponent, ProcessData, ProcessSetup,
        SpeakerArrangement, SymbolicSampleSizes,
    };
    use vst3_sys::{ComInterface, VstPtr};

    const FRAMES: usize = 31;
    const ORDER_SEVEN_LAYOUT: usize = 55;
    const INPUT_MASK: SpeakerArrangement = u64::MAX;
    const OUTPUT_MASK: SpeakerArrangement = 0x1800_0000_0302_d63f;

    let wrapper = Wrapper::<AmbisonicsWrapper>::new();
    // SAFETY: query_interface returns an owned COM reference; keep the original wrapper alive
    // until all references obtained from it have been dropped.
    unsafe {
        let mut pointer = std::ptr::null_mut();
        assert_eq!(
            wrapper.query_interface(&<dyn IAudioProcessor>::IID, &mut pointer),
            kResultOk
        );
        let processor = VstPtr::<dyn IAudioProcessor>::owned(pointer.cast()).unwrap();
        let component = processor.cast::<dyn IComponent>().unwrap();
        assert_eq!(component.initialize(std::ptr::null_mut()), kResultOk);
        let mut input_arrangement = INPUT_MASK;
        let mut output_arrangement = OUTPUT_MASK;
        assert_eq!(
            processor.set_bus_arrangements(&mut input_arrangement, 1, &mut output_arrangement, 1,),
            kResultOk
        );
        let mut negotiated = 0;
        assert_eq!(
            processor.get_bus_arrangement(BusDirections::kInput as i32, 0, &mut negotiated,),
            kResultOk
        );
        assert_eq!(negotiated, INPUT_MASK);
        assert_eq!(
            processor.get_bus_arrangement(BusDirections::kOutput as i32, 0, &mut negotiated,),
            kResultOk
        );
        assert_eq!(negotiated, OUTPUT_MASK);
        assert_ne!(
            processor.can_process_sample_size(SymbolicSampleSizes::kSample64 as i32),
            kResultOk,
            "NIH VST3 advertises f32 only"
        );
        assert_eq!(
            processor.can_process_sample_size(SymbolicSampleSizes::kSample32 as i32),
            kResultOk
        );
        let rejected_f64_setup = ProcessSetup {
            process_mode: 0,
            symbolic_sample_size: SymbolicSampleSizes::kSample64 as i32,
            max_samples_per_block: FRAMES as i32,
            sample_rate: 48_000.0,
        };
        assert_eq!(
            processor.setup_processing(&rejected_f64_setup),
            kResultFalse
        );

        let setup = ProcessSetup {
            process_mode: 0,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            max_samples_per_block: FRAMES as i32,
            sample_rate: 48_000.0,
        };
        assert_eq!(processor.setup_processing(&setup), kResultOk);
        assert_eq!(component.set_active(1), kResultOk);
        assert_eq!(processor.set_processing(1), kResultOk);

        let input_channels = 64;
        let output_channels = 16;
        let layout = &<AmbisonicsWrapper as Plugin>::AUDIO_IO_LAYOUTS[ORDER_SEVEN_LAYOUT];
        let mut reference = AmbisonicsWrapper::default();
        initialize_on_layout(&mut reference, layout, PluginApi::Vst3);
        let input: Vec<Vec<f32>> = (0..input_channels)
            .map(|channel| {
                (0..FRAMES)
                    .map(|frame| {
                        let code = (frame * 43 + channel * 71 + frame * channel * 11) % 997;
                        (code as f32 - 498.0) * 0.000_02
                    })
                    .collect()
            })
            .collect();
        let mut reference_channels = input.clone();
        reference_channels.resize_with(input_channels.max(output_channels), || vec![0.0; FRAMES]);
        let mut reference_buffer = Buffer::default();
        // SAFETY: channel vectors are disjoint, equal length, and live through processing.
        reference_buffer.set_slices(FRAMES, |slices| {
            slices.extend(reference_channels.iter_mut().map(Vec::as_mut_slice))
        });
        let mut auxiliary = AuxiliaryBuffers {
            inputs: &mut [],
            outputs: &mut [],
        };
        let direct_status = reference.process_with_api(
            &mut reference_buffer,
            &mut auxiliary,
            PluginApi::Vst3,
            Default::default(),
        );
        assert!(
            !matches!(direct_status, ProcessStatus::Error(_)),
            "{direct_status:?}"
        );

        let mut input_storage: Vec<Vec<f32>> = input
            .iter()
            .map(|channel| guarded_f32_channel(channel))
            .collect();
        let mut input_ptrs: Vec<*mut c_void> = input_storage
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().add(1).cast())
            .collect();
        let mut output: Vec<Vec<f32>> = (0..output_channels)
            .map(|_| vec![FRAME_CANARY; FRAMES + 2])
            .collect();
        output
            .iter_mut()
            .for_each(|channel| channel[1..FRAMES + 1].fill(-0.8125));
        let mut overflow_output = vec![vec![CHANNEL_CANARY; FRAMES + 2]; 1];
        let mut output_ptrs: Vec<*mut c_void> = output
            .iter_mut()
            .chain(overflow_output.iter_mut())
            .map(|channel| channel.as_mut_ptr().add(1).cast())
            .collect();
        let mut input_bus = AudioBusBuffers {
            num_channels: input_channels as i32,
            silence_flags: 0,
            buffers: input_ptrs.as_mut_ptr(),
        };
        let mut output_bus = AudioBusBuffers {
            num_channels: output_channels as i32,
            silence_flags: 0,
            buffers: output_ptrs.as_mut_ptr(),
        };
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            num_samples: FRAMES as i32,
            num_inputs: 1,
            num_outputs: 1,
            inputs: &mut input_bus,
            outputs: &mut output_bus,
            input_param_changes: std::mem::zeroed(),
            output_param_changes: std::mem::zeroed(),
            input_events: std::mem::zeroed(),
            output_events: std::mem::zeroed(),
            context: std::ptr::null_mut(),
        };
        assert_eq!(processor.process(&mut data), kResultOk);
        assert_frame_canaries(&input_storage, FRAMES);
        assert_frame_canaries(&output, FRAMES);
        assert!(
            input_storage
                .iter()
                .zip(&input)
                .all(|(guarded, source)| guarded[1..FRAMES + 1] == source[..])
        );
        assert!(
            overflow_output
                .iter()
                .flatten()
                .all(|sample| *sample == CHANNEL_CANARY)
        );
        for (bus_channel, samples) in output.iter().take(output_channels).enumerate() {
            // `process_with_api(Vst3)` already writes bus order into the reference buffer.
            assert_eq!(
                &samples[1..FRAMES + 1],
                reference_buffer.as_slice_immutable()[bus_channel]
            );
        }

        // Reject both short and surplus primary input vectors before the plugin sees an incomplete
        // or non-negotiated ACN vector. Supplied f32 outputs are safely silenced.
        input_bus.num_channels = (input_channels - 1) as i32;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.625);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        assert_frame_canaries(&output, FRAMES);

        let mut surplus_input_ptrs = input_ptrs.clone();
        surplus_input_ptrs.push(input_ptrs[0]);
        input_bus.buffers = surplus_input_ptrs.as_mut_ptr();
        input_bus.num_channels = (input_channels + 1) as i32;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.5);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        assert!(
            overflow_output
                .iter()
                .flatten()
                .all(|sample| *sample == CHANNEL_CANARY)
        );
        input_bus.buffers = input_ptrs.as_mut_ptr();
        input_bus.num_channels = input_channels as i32;

        // A missing primary input is rejected. A missing primary output is also rejected and
        // leaves host memory untouched because there is no delivered output storage to silence.
        data.inputs = std::ptr::null_mut();
        data.num_inputs = 0;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.375);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        data.inputs = &mut input_bus;
        data.num_inputs = 1;

        // Extra input/output bus entries are rejected by count before a pointer from either extra
        // bus can be consumed. The declared output intersection is silenced on input mismatch.
        let mut two_input_buses = [
            AudioBusBuffers {
                num_channels: input_channels as i32,
                silence_flags: 0,
                buffers: input_ptrs.as_mut_ptr(),
            },
            AudioBusBuffers {
                num_channels: input_channels as i32,
                silence_flags: 0,
                buffers: input_ptrs.as_mut_ptr(),
            },
        ];
        data.inputs = two_input_buses.as_mut_ptr();
        data.num_inputs = 2;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.4375);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        data.inputs = &mut input_bus;
        data.num_inputs = 1;

        let mut two_output_buses = [
            AudioBusBuffers {
                num_channels: output_channels as i32,
                silence_flags: 0,
                buffers: output_ptrs.as_mut_ptr(),
            },
            AudioBusBuffers {
                num_channels: output_channels as i32,
                silence_flags: 0,
                buffers: output_ptrs.as_mut_ptr(),
            },
        ];
        data.outputs = two_output_buses.as_mut_ptr();
        data.num_outputs = 2;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.40625);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        data.outputs = &mut output_bus;
        data.num_outputs = 1;

        data.outputs = std::ptr::null_mut();
        data.num_outputs = 0;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.25);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == -0.25))
        );
        data.outputs = &mut output_bus;
        data.num_outputs = 1;

        // Short output widths are rejected and only the delivered negotiated intersection is
        // silenced. Surplus host channels remain protected by their canaries.
        output_bus.num_channels = (output_channels - 1) as i32;
        assert_eq!(std::hint::black_box(output_bus.num_channels), 15);
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.125);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output[..output_channels - 1]
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        assert!(
            output[output_channels - 1][1..FRAMES + 1]
                .iter()
                .all(|sample| *sample == -0.125)
        );
        assert_frame_canaries(&output, FRAMES);

        output_bus.num_channels = (output_channels + 1) as i32;
        assert_eq!(std::hint::black_box(output_bus.num_channels), 17);
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(-0.0625);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(
            output
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == 0.0))
        );
        assert!(
            overflow_output
                .iter()
                .flatten()
                .all(|sample| *sample == CHANNEL_CANARY)
        );
        output_bus.num_channels = output_channels as i32;
        assert_eq!(std::hint::black_box(output_bus.num_channels), 16);

        // The configured maximum is 31 frames. A 32-frame request must be rejected before any
        // write reaches the trailing canary, even though that storage follows the sample array.
        data.num_samples = (FRAMES + 1) as i32;
        for channel in &mut output {
            channel[1..FRAMES + 1].fill(0.03125);
        }
        assert_eq!(processor.process(&mut data), kResultFalse);
        assert!(output.iter().all(|channel| {
            channel[1..FRAMES + 1]
                .iter()
                .all(|sample| *sample == 0.03125)
                && channel[FRAMES + 1] == FRAME_CANARY
        }));
        // The real IAudioProcessor::process entrypoint rejects f64 before reading its pointer
        // array or writing output memory, even if a host ignores can_process_sample_size().
        let mut input64 = vec![vec![FRAME_CANARY_F64; FRAMES + 2]; input_channels];
        let mut output64 = vec![vec![FRAME_CANARY_F64; FRAMES + 2]; output_channels];
        input64
            .iter_mut()
            .for_each(|channel| channel[1..FRAMES + 1].fill(0.125));
        output64
            .iter_mut()
            .for_each(|channel| channel[1..FRAMES + 1].fill(-9.5));
        let mut input64_ptrs: Vec<*mut c_void> = input64
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().add(1).cast())
            .collect();
        let mut output64_ptrs: Vec<*mut c_void> = output64
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().add(1).cast())
            .collect();
        let mut f64_input_bus = AudioBusBuffers {
            num_channels: input_channels as i32,
            silence_flags: 0,
            buffers: input64_ptrs.as_mut_ptr(),
        };
        let mut f64_output_bus = AudioBusBuffers {
            num_channels: output_channels as i32,
            silence_flags: 0,
            buffers: output64_ptrs.as_mut_ptr(),
        };
        let mut f64_data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: SymbolicSampleSizes::kSample64 as i32,
            num_samples: FRAMES as i32,
            num_inputs: 1,
            num_outputs: 1,
            inputs: &mut f64_input_bus,
            outputs: &mut f64_output_bus,
            input_param_changes: std::mem::zeroed(),
            output_param_changes: std::mem::zeroed(),
            input_events: std::mem::zeroed(),
            output_events: std::mem::zeroed(),
            context: std::ptr::null_mut(),
        };
        assert_eq!(processor.process(&mut f64_data), kResultFalse);
        assert!(
            output64
                .iter()
                .all(|channel| channel[1..FRAMES + 1].iter().all(|sample| *sample == -9.5))
        );
        assert!(
            input64
                .iter()
                .chain(&output64)
                .all(|channel| channel[0] == FRAME_CANARY_F64)
        );
        assert!(
            input64
                .iter()
                .chain(&output64)
                .all(|channel| channel[FRAMES + 1] == FRAME_CANARY_F64)
        );

        assert_eq!(processor.set_processing(0), kResultOk);
        assert_eq!(component.set_active(0), kResultOk);
    }
}
