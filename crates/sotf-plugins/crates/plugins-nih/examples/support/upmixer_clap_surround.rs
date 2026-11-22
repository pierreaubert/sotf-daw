mod clap_surround {
    use super::*;
    use clap_sys::audio_buffer::clap_audio_buffer;
    use clap_sys::entry::clap_plugin_entry;
    use clap_sys::events::{
        CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header,
        clap_event_param_value, clap_input_events,
    };
    use clap_sys::ext::audio_ports_config::{
        CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_plugin_audio_ports_config,
    };
    use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
    use clap_sys::ext::params::{
        CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY,
        clap_param_info, clap_plugin_params,
    };
    use clap_sys::ext::surround::{CLAP_EXT_SURROUND, clap_plugin_surround};
    use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
    use clap_sys::host::clap_host;
    use clap_sys::plugin::clap_plugin;
    use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
    use std::ffi::CStr;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy)]
    struct Layout {
        name: &'static str,
        roles: &'static [u8],
    }

    // CLAP channel identifiers are specified by the public surround extension.
    // https://github.com/free-audio/clap/blob/main/include/clap/ext/surround.h
    const INPUTS: [Layout; 3] = [
        Layout {
            name: "5.0",
            roles: &[0, 1, 2, 9, 10],
        },
        Layout {
            name: "5.1",
            roles: &[0, 1, 2, 3, 9, 10],
        },
        Layout {
            name: "7.1",
            roles: &[0, 1, 2, 3, 9, 10, 4, 5],
        },
    ];
    const OUTPUTS: [Layout; 7] = [
        INPUTS[0],
        INPUTS[1],
        Layout {
            name: "5.1.2",
            roles: &[0, 1, 2, 3, 9, 10, 12, 14],
        },
        Layout {
            name: "5.1.4",
            roles: &[0, 1, 2, 3, 9, 10, 12, 14, 15, 17],
        },
        INPUTS[2],
        Layout {
            name: "7.1.2",
            roles: &[0, 1, 2, 3, 9, 10, 4, 5, 12, 14],
        },
        Layout {
            name: "7.1.4",
            roles: &[0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17],
        },
    ];

    struct Module {
        handle: *mut c_void,
        entry: *const clap_plugin_entry,
    }

    impl Module {
        fn open(path: &Path) -> Self {
            let name = CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: the path is NUL-terminated, and the module is retained until deinit.
            unsafe {
                let handle = dlopen(name.as_ptr(), RTLD_NOW);
                assert!(!handle.is_null(), "cannot load {}", path.display());
                let entry = dlsym(handle, c"clap_entry".as_ptr()).cast::<clap_plugin_entry>();
                assert!(!entry.is_null(), "missing exported CLAP entry");
                assert!(((*entry).init.unwrap())(name.as_ptr()));
                Self { handle, entry }
            }
        }
    }

    impl Drop for Module {
        fn drop(&mut self) {
            // SAFETY: every plugin borrows this module and is destroyed before deinit/dlclose.
            unsafe {
                ((*self.entry).deinit.unwrap())();
                assert_eq!(dlclose(self.handle), 0);
            }
        }
    }

    unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
        ptr::null()
    }
    unsafe extern "C" fn no_request(_: *const clap_host) {}
    #[derive(Default)]
    struct HostState {
        callbacks: AtomicUsize,
        restarts: AtomicUsize,
    }
    unsafe extern "C" fn request_callback(host: *const clap_host) {
        // SAFETY: Native retains the boxed state until the plugin has been destroyed.
        let state = unsafe { &*((*host).host_data.cast::<HostState>()) };
        state.callbacks.fetch_add(1, Ordering::AcqRel);
    }
    unsafe extern "C" fn request_restart(host: *const clap_host) {
        // SAFETY: Native retains the boxed state until the plugin has been destroyed.
        let state = unsafe { &*((*host).host_data.cast::<HostState>()) };
        state.restarts.fetch_add(1, Ordering::AcqRel);
    }
    unsafe extern "C" fn one_event(_: *const clap_input_events) -> u32 {
        1
    }
    unsafe extern "C" fn event_at(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        if index != 0 {
            return ptr::null();
        }
        // SAFETY: the caller retains the event referenced by the list context.
        unsafe { &(*((*list).ctx.cast::<clap_event_param_value>())).header }
    }

    struct Native<'a> {
        plugin: *const clap_plugin,
        _host: Box<clap_host>,
        state: Box<HostState>,
        active: bool,
        processing: bool,
        _module: &'a Module,
    }

    impl<'a> Native<'a> {
        fn new(module: &'a Module) -> Self {
            let mut state = Box::<HostState>::default();
            let host = Box::new(clap_host {
                clap_version: clap_sys::version::CLAP_VERSION,
                host_data: (&mut *state as *mut HostState).cast(),
                name: c"SOTF external CLAP test".as_ptr(),
                vendor: c"SOTF".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(no_extension),
                request_restart: Some(request_restart),
                request_process: Some(no_request),
                request_callback: Some(request_callback),
            });
            // SAFETY: the entry, factory, host, and identifiers remain live through creation.
            let plugin = unsafe {
                let factory =
                    ((*module.entry).get_factory.unwrap())(CLAP_PLUGIN_FACTORY_ID.as_ptr())
                        .cast::<clap_plugin_factory>();
                assert!(!factory.is_null());
                assert_eq!(((*factory).get_plugin_count.unwrap())(factory), 1);
                let descriptor = ((*factory).get_plugin_descriptor.unwrap())(factory, 0);
                assert!(!descriptor.is_null());
                assert_eq!(
                    CStr::from_ptr((*descriptor).id),
                    c"org.spinorama.sotf.upmixer"
                );
                let plugin = ((*factory).create_plugin.unwrap())(
                    factory,
                    &*host,
                    c"org.spinorama.sotf.upmixer".as_ptr(),
                );
                assert!(!plugin.is_null());
                assert!(((*plugin).init.unwrap())(plugin));
                plugin
            };
            Self {
                plugin,
                _host: host,
                state,
                active: false,
                processing: false,
                _module: module,
            }
        }

        fn select(&self, input: Layout, output: Layout) {
            let expected_name = format!("{} to {}", input.name, output.name);
            // SAFETY: these extension tables are owned by the live initialized plugin.
            unsafe {
                let configs = ((*self.plugin).get_extension.unwrap())(
                    self.plugin,
                    CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr(),
                )
                .cast::<clap_plugin_audio_ports_config>();
                let surround = ((*self.plugin).get_extension.unwrap())(
                    self.plugin,
                    CLAP_EXT_SURROUND.as_ptr(),
                )
                .cast::<clap_plugin_surround>();
                assert!(!configs.is_null() && !surround.is_null());
                let count = ((*configs).count.unwrap())(self.plugin);
                assert_eq!(count, 17);
                let mut selected = None;
                for index in 0..count {
                    let mut info = std::mem::zeroed::<clap_audio_ports_config>();
                    assert!(((*configs).get.unwrap())(self.plugin, index, &mut info));
                    if CStr::from_ptr(info.name.as_ptr()).to_bytes() == expected_name.as_bytes() {
                        assert_eq!(info.main_input_channel_count as usize, input.roles.len());
                        assert_eq!(info.main_output_channel_count as usize, output.roles.len());
                        selected = Some(info.id);
                    }
                }
                assert!(((*configs).select.unwrap())(
                    self.plugin,
                    selected.expect("missing CLAP layout")
                ));
                assert!(!((*configs).select.unwrap())(self.plugin, u32::MAX));
                for (is_input, layout) in [(true, input), (false, output)] {
                    let mut actual = [u8::MAX; 16];
                    let length = ((*surround).get_channel_map.unwrap())(
                        self.plugin,
                        is_input,
                        0,
                        actual.as_mut_ptr(),
                        actual.len() as u32,
                    ) as usize;
                    assert_eq!(&actual[..length], layout.roles, "{expected_name}");
                    let mask = layout
                        .roles
                        .iter()
                        .fold(0_u64, |mask, role| mask | (1_u64 << role));
                    assert!(((*surround).is_channel_mask_supported.unwrap())(
                        self.plugin,
                        mask
                    ));
                }
            }
        }

        fn select_stereo(&self) {
            // SAFETY: the configuration extension belongs to the live, inactive plugin.
            unsafe {
                let configs = ((*self.plugin).get_extension.unwrap())(
                    self.plugin,
                    CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr(),
                )
                .cast::<clap_plugin_audio_ports_config>();
                assert!(!configs.is_null());
                let mut selected = None;
                for index in 0..((*configs).count.unwrap())(self.plugin) {
                    let mut info = std::mem::zeroed::<clap_audio_ports_config>();
                    assert!(((*configs).get.unwrap())(self.plugin, index, &mut info));
                    if CStr::from_ptr(info.name.as_ptr()) == c"Stereo to 5.1" {
                        assert_eq!(
                            (
                                info.main_input_channel_count,
                                info.main_output_channel_count
                            ),
                            (2, 6)
                        );
                        selected = Some(info.id);
                    }
                }
                assert!(((*configs).select.unwrap())(self.plugin, selected.unwrap()));
            }
        }

        fn param_info(&self, name: &CStr) -> clap_param_info {
            // SAFETY: the params table and each output record belong to this live plugin.
            unsafe {
                let params =
                    ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_PARAMS.as_ptr())
                        .cast::<clap_plugin_params>();
                assert!(!params.is_null());
                for index in 0..((*params).count.unwrap())(self.plugin) {
                    let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
                    assert!(((*params).get_info.unwrap())(
                        self.plugin,
                        index,
                        info.as_mut_ptr()
                    ));
                    let info = info.assume_init();
                    if CStr::from_ptr(info.name.as_ptr()) == name {
                        return info;
                    }
                }
            }
            panic!("missing CLAP control {}", name.to_string_lossy())
        }

        fn param_id(&self, name: &CStr) -> u32 {
            self.param_info(name).id
        }

        fn assert_bass_controls(&self) {
            for name in [
                c"Bass Mode",
                c"Bass Crossover",
                c"Bass Gain",
                c"LFE Playback Gain",
                c"Combine LFE",
                c"Bass Front Left",
                c"Bass Front Right",
                c"Bass Center",
                c"Bass Side Left",
                c"Bass Side Right",
                c"Bass Back Left",
                c"Bass Back Right",
            ] {
                let info = self.param_info(name);
                assert_eq!(
                    info.flags
                        & (CLAP_PARAM_IS_HIDDEN
                            | CLAP_PARAM_IS_READONLY
                            | CLAP_PARAM_IS_AUTOMATABLE),
                    0,
                    "{} must be visible, editable, and non-automatable",
                    name.to_string_lossy()
                );
            }
            let id = self.param_id(c"Bass Mode");
            // SAFETY: the extension and both conversion buffers remain live during these calls.
            unsafe {
                let params =
                    ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_PARAMS.as_ptr())
                        .cast::<clap_plugin_params>();
                for (value, label) in [(0.0, c"Off"), (1.0, c"Derived"), (2.0, c"Managed")] {
                    let mut display = [0 as c_char; 128];
                    assert!(((*params).value_to_text.unwrap())(
                        self.plugin,
                        id,
                        value,
                        display.as_mut_ptr(),
                        display.len() as u32
                    ));
                    assert_eq!(CStr::from_ptr(display.as_ptr()), label);
                    let mut converted = f64::NAN;
                    assert!(((*params).text_to_value.unwrap())(
                        self.plugin,
                        id,
                        label.as_ptr(),
                        &mut converted
                    ));
                    assert_eq!(converted, value);
                }
            }
        }

        fn flush_text(&self, name: &CStr, text: &CStr) {
            let id = self.param_id(name);
            let mut value = f64::NAN;
            // SAFETY: the text, extension, and result remain live during conversion.
            unsafe {
                let params =
                    ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_PARAMS.as_ptr())
                        .cast::<clap_plugin_params>();
                assert!(((*params).text_to_value.unwrap())(
                    self.plugin,
                    id,
                    text.as_ptr(),
                    &mut value
                ));
            }
            self.flush(name, value);
        }

        fn flush(&self, name: &CStr, value: f64) {
            let mut event = scalar_event(self.param_id(name), value);
            let events = event_list(&mut event);
            // SAFETY: the event and its list remain live during this inactive flush.
            unsafe {
                let params =
                    ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_PARAMS.as_ptr())
                        .cast::<clap_plugin_params>();
                ((*params).flush.unwrap())(self.plugin, &events, ptr::null());
                let mut actual = f64::NAN;
                assert!(((*params).get_value.unwrap())(
                    self.plugin,
                    event.param_id,
                    &mut actual
                ));
                assert_eq!(actual, value);
            }
        }

        fn latency(&self) -> usize {
            // SAFETY: the latency extension belongs to the live plugin.
            unsafe {
                let latency =
                    ((*self.plugin).get_extension.unwrap())(self.plugin, CLAP_EXT_LATENCY.as_ptr())
                        .cast::<clap_plugin_latency>();
                assert!(!latency.is_null());
                ((*latency).get.unwrap())(self.plugin) as usize
            }
        }

        fn wait_for_restart(&self) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if self.state.callbacks.swap(0, Ordering::AcqRel) != 0 {
                    // SAFETY: this host pumps requested callbacks on its main thread.
                    unsafe { ((*self.plugin).on_main_thread.unwrap())(self.plugin) };
                }
                if self.state.restarts.load(Ordering::Acquire) != 0 {
                    return;
                }
                std::thread::yield_now();
            }
            panic!("active CLAP bass edit did not request a restart");
        }
    }

    impl Drop for Native<'_> {
        fn drop(&mut self) {
            // SAFETY: callbacks are paired in lifecycle order while the module and host live.
            unsafe {
                if self.processing {
                    ((*self.plugin).stop_processing.unwrap())(self.plugin);
                }
                if self.active {
                    ((*self.plugin).deactivate.unwrap())(self.plugin);
                }
                ((*self.plugin).destroy.unwrap())(self.plugin);
            }
        }
    }

    fn scalar_event(id: u32, value: f64) -> clap_event_param_value {
        clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: id,
            cookie: ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        }
    }

    fn event_list(event: &mut clap_event_param_value) -> clap_input_events {
        clap_input_events {
            ctx: (event as *mut clap_event_param_value).cast(),
            size: Some(one_event),
            get: Some(event_at),
        }
    }

    fn exercise(module: &Module, input: Layout, output: Layout, rate: u32) -> serde_json::Value {
        let mut native = Native::new(module);
        native.select(input, output);
        for name in [c"Binaural Preview", c"Auto Gain", c"Subharmonic Synth"] {
            native.flush(name, 1.0);
        }
        let height_id = native.param_id(c"Bypass Heights");
        let surround_id = native.param_id(c"Bypass Surround");
        // SAFETY: plugin configuration is finalized and lifecycle calls are paired by Drop.
        unsafe {
            assert!(((*native.plugin).activate.unwrap())(
                native.plugin,
                f64::from(rate),
                1,
                MAX_FRAMES as u32
            ));
            native.active = true;
            assert!(((*native.plugin).start_processing.unwrap())(native.plugin));
            native.processing = true;
        }
        let latency = native.latency();
        let mut inputs = vec![vec![0.0_f32; MAX_FRAMES]; input.roles.len()];
        let mut outputs = vec![vec![0.0_f32; MAX_FRAMES]; output.roles.len()];
        let mut input_ptrs: Vec<_> = inputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut output_ptrs: Vec<_> = outputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let input_bus = clap_audio_buffer {
            data32: input_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: input.roles.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_bus = clap_audio_buffer {
            data32: output_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: output.roles.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let sources: Vec<_> = output
            .roles
            .iter()
            .map(|role| input.roles.iter().position(|candidate| candidate == role))
            .collect();
        let has_heights = output.roles.iter().any(|role| *role >= 12);
        let phases = [
            (None, false),
            (Some((height_id, 1.0)), true),
            (Some((height_id, 0.0)), false),
            (Some((surround_id, 1.0)), true),
            (Some((surround_id, 0.0)), false),
        ];
        let mut frame = 0;
        for (phase, (event, bypassed)) in phases.into_iter().enumerate() {
            let end = frame + 8192 + if phase == 0 { latency } else { 0 };
            let mut event = event.map(|(id, value)| scalar_event(id, value));
            let events = event.as_mut().map(event_list);
            let mut first = true;
            let mut block = 0;
            let mut energy = 0.0_f64;
            let mut peak = 0.0_f32;
            while frame < end {
                let count = [1, 17, 63, 128, 257, 7, 251][block % 7].min(end - frame);
                block += 1;
                for (channel, values) in inputs.iter_mut().enumerate() {
                    for (offset, value) in values[..count].iter_mut().enumerate() {
                        *value = super::surround::sample(channel, frame + offset);
                    }
                }
                for values in &mut outputs {
                    values[..count].fill(f32::NAN);
                }
                output_bus.constant_mask = 0;
                let process = clap_process {
                    steady_time: -1,
                    frames_count: count as u32,
                    transport: ptr::null(),
                    audio_inputs: &input_bus,
                    audio_outputs: &mut output_bus,
                    audio_inputs_count: 1,
                    audio_outputs_count: 1,
                    in_events: if first {
                        events.as_ref().map_or(ptr::null(), |value| value)
                    } else {
                        ptr::null()
                    },
                    out_events: ptr::null(),
                };
                // SAFETY: the disjoint buses, buffers, and optional event are live throughout process.
                let status =
                    unsafe { ((*native.plugin).process.unwrap())(native.plugin, &process) };
                assert_ne!(
                    status, CLAP_PROCESS_ERROR,
                    "{} to {} phase {phase}",
                    input.name, output.name
                );
                first = false;
                for (channel, values) in outputs.iter().enumerate() {
                    let role = output.roles[channel];
                    for (offset, &actual) in values[..count].iter().enumerate() {
                        assert!(actual.is_finite());
                        let time = frame + offset;
                        if role >= 12 {
                            if time >= end - 1024 {
                                energy += f64::from(actual).powi(2);
                                peak = peak.max(actual.abs());
                            }
                        } else {
                            let expected = sources[channel]
                                .filter(|_| time >= latency)
                                .map_or(0.0, |source| {
                                    super::surround::sample(source, time - latency)
                                });
                            assert_eq!(
                                actual.to_bits(),
                                expected.to_bits(),
                                "{} to {} role {role} frame {time} phase {phase}",
                                input.name,
                                output.name
                            );
                        }
                    }
                }
                frame += count;
            }
            if has_heights {
                if bypassed {
                    assert!(
                        peak < 1e-7,
                        "{} to {} phase {phase}: {peak}",
                        input.name,
                        output.name
                    );
                } else {
                    assert!(
                        energy > 1e-12,
                        "{} to {} phase {phase}",
                        input.name,
                        output.name
                    );
                }
            }
            assert_eq!(native.latency(), latency);
        }
        serde_json::json!({"input":input.name,"output":output.name,"rate":rate,"latency":latency,"frames":frame,"phases":phases.len()})
    }

    fn exercise_bass(
        module: &Module,
        input: Layout,
        output: Layout,
        rate: u32,
        mode: u32,
    ) -> serde_json::Value {
        let mut native = Native::new(module);
        native.select(input, output);
        if input.roles.contains(&3) {
            native.flush(c"Combine LFE", 1.0);
        }
        native.flush(c"Bass Mode", f64::from(mode));
        native.flush_text(c"Bass Gain", c"-6.0");
        native.flush_text(c"LFE Playback Gain", c"10.0");
        // SAFETY: plugin configuration is finalized and lifecycle calls are paired by Drop.
        unsafe {
            assert!(((*native.plugin).activate.unwrap())(
                native.plugin,
                f64::from(rate),
                1,
                MAX_FRAMES as u32,
            ));
            native.active = true;
            assert!(((*native.plugin).start_processing.unwrap())(native.plugin));
            native.processing = true;
        }
        let latency = native.latency();
        let amplitudes: Vec<_> = (0..input.roles.len())
            .map(|channel| (channel + 1) as f32 / 100.0)
            .collect();
        let mut inputs: Vec<_> = amplitudes
            .iter()
            .map(|amplitude| vec![*amplitude; MAX_FRAMES])
            .collect();
        let mut outputs = vec![vec![f32::NAN; MAX_FRAMES]; output.roles.len()];
        let mut input_ptrs: Vec<_> = inputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut output_ptrs: Vec<_> = outputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let input_bus = clap_audio_buffer {
            data32: input_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: input.roles.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_bus = clap_audio_buffer {
            data32: output_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: output.roles.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        const BLOCKS: usize = 200;
        for _ in 0..BLOCKS {
            let process = clap_process {
                steady_time: -1,
                frames_count: MAX_FRAMES as u32,
                transport: ptr::null(),
                audio_inputs: &input_bus,
                audio_outputs: &mut output_bus,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: ptr::null(),
                out_events: ptr::null(),
            };
            // SAFETY: disjoint planar buffers and bus pointers remain live through the call.
            let status = unsafe { ((*native.plugin).process.unwrap())(native.plugin, &process) };
            assert_ne!(status, CLAP_PROCESS_ERROR);
        }
        let sum: f32 = input
            .roles
            .iter()
            .zip(&amplitudes)
            .filter(|(role, _)| **role != 3)
            .map(|(_, amplitude)| *amplitude)
            .sum();
        let existing_lfe = input
            .roles
            .iter()
            .position(|role| *role == 3)
            .map_or(0.0, |index| amplitudes[index]);
        let expected_lfe = existing_lfe + sum * 10.0_f32.powf(-16.0 / 20.0);
        let lfe_index = output.roles.iter().position(|role| *role == 3).unwrap();
        let lfe = outputs[lfe_index][MAX_FRAMES - 1];
        assert!(
            (lfe - expected_lfe).abs() < 1e-5,
            "{} to {} mode {mode}: {lfe} vs {expected_lfe}",
            input.name,
            output.name
        );
        for (channel, role) in output.roles.iter().enumerate() {
            let actual = outputs[channel][MAX_FRAMES - 1];
            assert!(actual.is_finite());
            if let Some(source) = input.roles.iter().position(|candidate| candidate == role)
                && *role != 3
            {
                let expected = if mode == 1 { amplitudes[source] } else { 0.0 };
                assert!(
                    (actual - expected).abs() < 1e-5,
                    "{} to {} role {role} mode {mode}: {actual} vs {expected}",
                    input.name,
                    output.name
                );
            }
        }
        assert_eq!(native.latency(), latency);
        serde_json::json!({"input":input.name,"output":output.name,"rate":rate,"mode":if mode == 1 {"derived"} else {"managed"},"latency":latency,"frames":BLOCKS * MAX_FRAMES,"lfe":lfe,"expected_lfe":expected_lfe})
    }

    fn restart_bass(module: &Module) -> serde_json::Value {
        let mut native = Native::new(module);
        native.select(INPUTS[0], OUTPUTS[3]);
        // SAFETY: this is the first activation and processing session of the initialized plugin.
        unsafe {
            assert!(((*native.plugin).activate.unwrap())(
                native.plugin,
                48_000.0,
                1,
                MAX_FRAMES as u32
            ));
            native.active = true;
            assert!(((*native.plugin).start_processing.unwrap())(native.plugin));
            native.processing = true;
        }
        let latency = native.latency();
        let mut inputs: Vec<_> = (1..=5)
            .map(|channel| vec![channel as f32 / 100.0; MAX_FRAMES])
            .collect();
        let mut outputs = vec![vec![f32::NAN; MAX_FRAMES]; OUTPUTS[3].roles.len()];
        let mut input_ptrs: Vec<_> = inputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut output_ptrs: Vec<_> = outputs.iter_mut().map(Vec::as_mut_ptr).collect();
        let input_bus = clap_audio_buffer {
            data32: input_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: inputs.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_bus = clap_audio_buffer {
            data32: output_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: outputs.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut event = scalar_event(native.param_id(c"Bass Mode"), 1.0);
        let events = event_list(&mut event);
        for phase in 0..3 {
            if phase == 2 {
                native.wait_for_restart();
                // SAFETY: the host stops processing and deactivates before reactivation.
                unsafe {
                    ((*native.plugin).stop_processing.unwrap())(native.plugin);
                    native.processing = false;
                    ((*native.plugin).deactivate.unwrap())(native.plugin);
                    native.active = false;
                    assert!(((*native.plugin).activate.unwrap())(
                        native.plugin,
                        48_000.0,
                        1,
                        MAX_FRAMES as u32
                    ));
                    native.active = true;
                    assert!(((*native.plugin).start_processing.unwrap())(native.plugin));
                    native.processing = true;
                }
                assert_eq!(native.latency(), latency);
            }
            let blocks = if phase == 1 { 1 } else { 200 };
            for block in 0..blocks {
                let process = clap_process {
                    steady_time: -1,
                    frames_count: MAX_FRAMES as u32,
                    transport: ptr::null(),
                    audio_inputs: &input_bus,
                    audio_outputs: &mut output_bus,
                    audio_inputs_count: 1,
                    audio_outputs_count: 1,
                    in_events: if phase == 1 && block == 0 {
                        &events
                    } else {
                        ptr::null()
                    },
                    out_events: ptr::null(),
                };
                // SAFETY: the plugin owns no buffer; the disjoint buses remain valid for this call.
                assert_ne!(
                    unsafe { ((*native.plugin).process.unwrap())(native.plugin, &process) },
                    CLAP_PROCESS_ERROR
                );
            }
            let lfe = outputs[3][MAX_FRAMES - 1];
            if phase < 2 {
                assert_eq!(lfe, 0.0, "prepared bass path changed before restart");
            } else {
                assert!(
                    (lfe - 0.15).abs() < 1e-5,
                    "bass path did not change after restart: {lfe}"
                );
            }
        }
        serde_json::json!({"input":"5.0","output":"5.1.4","rate":48000,"latency":latency,"restart_requests":native.state.restarts.load(Ordering::Acquire)})
    }

    fn reject_unapproved_bass(module: &Module) -> usize {
        let cases = [
            (INPUTS[0], OUTPUTS[0], false),
            (INPUTS[1], OUTPUTS[3], true),
            (INPUTS[2], OUTPUTS[6], true),
        ];
        for (input, output, has_existing_lfe) in cases {
            let mut native = Native::new(module);
            native.select(input, output);
            native.flush(c"Bass Mode", 2.0);
            // SAFETY: the plugin is initialized, inactive, and configured by this host.
            assert!(!unsafe {
                ((*native.plugin).activate.unwrap())(native.plugin, 48_000.0, 1, MAX_FRAMES as u32)
            });
            if has_existing_lfe {
                native.flush(c"Combine LFE", 1.0);
            } else {
                native.flush(c"Bass Mode", 0.0);
            }
            // SAFETY: the corrected configuration is still inactive and owned by this host.
            unsafe {
                assert!(((*native.plugin).activate.unwrap())(
                    native.plugin,
                    48_000.0,
                    1,
                    MAX_FRAMES as u32,
                ));
                native.active = true;
                assert!(((*native.plugin).start_processing.unwrap())(native.plugin));
                native.processing = true;
            }
        }
        cases.len()
    }

    fn assert_stable_parameter_order(module: &Module) -> usize {
        let ids = |native: &Native<'_>| {
            // SAFETY: the extension and each output record belong to this live plugin.
            unsafe {
                let params = ((*native.plugin).get_extension.unwrap())(
                    native.plugin,
                    CLAP_EXT_PARAMS.as_ptr(),
                )
                .cast::<clap_plugin_params>();
                assert!(!params.is_null());
                (0..((*params).count.unwrap())(native.plugin))
                    .map(|index| {
                        let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
                        assert!(((*params).get_info.unwrap())(
                            native.plugin,
                            index,
                            info.as_mut_ptr()
                        ));
                        info.assume_init().id
                    })
                    .collect::<Vec<_>>()
            }
        };
        let expected = ids(&Native::new(module));
        assert!(!expected.is_empty());
        for _ in 0..4 {
            assert_eq!(ids(&Native::new(module)), expected);
        }
        expected.len()
    }

    fn stereo_bass_roundtrip(module: &Module) -> usize {
        for mode in [1, 2] {
            let mut native = Native::new(module);
            native.flush(c"Bass Mode", f64::from(mode));
            for input_channels in [2, 5, 2] {
                if input_channels == 2 {
                    native.select_stereo();
                } else {
                    native.select(INPUTS[0], OUTPUTS[1]);
                }
                // SAFETY: the host activates and starts the inactive plugin exactly once per phase.
                unsafe {
                    assert!(((*native.plugin).activate.unwrap())(
                        native.plugin,
                        48_000.0,
                        1,
                        MAX_FRAMES as u32
                    ));
                    native.active = true;
                    assert!(((*native.plugin).start_processing.unwrap())(native.plugin));
                    native.processing = true;
                    let params = ((*native.plugin).get_extension.unwrap())(
                        native.plugin,
                        CLAP_EXT_PARAMS.as_ptr(),
                    )
                    .cast::<clap_plugin_params>();
                    let mut actual = f64::NAN;
                    assert!(((*params).get_value.unwrap())(
                        native.plugin,
                        native.param_id(c"Bass Mode"),
                        &mut actual
                    ));
                    assert_eq!(actual, f64::from(mode));
                }
                let mut inputs: Vec<_> = (1..=input_channels)
                    .map(|channel| vec![channel as f32 / 100.0; MAX_FRAMES])
                    .collect();
                let mut outputs = vec![vec![0.0_f32; MAX_FRAMES]; 6];
                let mut input_ptrs: Vec<_> = inputs.iter_mut().map(Vec::as_mut_ptr).collect();
                let mut output_ptrs: Vec<_> = outputs.iter_mut().map(Vec::as_mut_ptr).collect();
                let input_bus = clap_audio_buffer {
                    data32: input_ptrs.as_mut_ptr(),
                    data64: ptr::null_mut(),
                    channel_count: input_channels,
                    latency: 0,
                    constant_mask: 0,
                };
                let mut output_bus = clap_audio_buffer {
                    data32: output_ptrs.as_mut_ptr(),
                    data64: ptr::null_mut(),
                    channel_count: 6,
                    latency: 0,
                    constant_mask: 0,
                };
                for _ in 0..200 {
                    let process = clap_process {
                        steady_time: -1,
                        frames_count: MAX_FRAMES as u32,
                        transport: ptr::null(),
                        audio_inputs: &input_bus,
                        audio_outputs: &mut output_bus,
                        audio_inputs_count: 1,
                        audio_outputs_count: 1,
                        in_events: ptr::null(),
                        out_events: ptr::null(),
                    };
                    // SAFETY: disjoint planar buffers and buses remain valid through the call.
                    assert_ne!(
                        unsafe { ((*native.plugin).process.unwrap())(native.plugin, &process) },
                        CLAP_PROCESS_ERROR
                    );
                }
                assert!(outputs.iter().flatten().all(|sample| sample.is_finite()));
                if input_channels == 5 {
                    assert!((outputs[3][MAX_FRAMES - 1] - 0.15).abs() < 1e-5);
                } else {
                    assert!(outputs.iter().flatten().any(|sample| *sample != 0.0));
                }
                // SAFETY: this pairs the processing and activation calls above.
                unsafe {
                    ((*native.plugin).stop_processing.unwrap())(native.plugin);
                    native.processing = false;
                    ((*native.plugin).deactivate.unwrap())(native.plugin);
                    native.active = false;
                }
            }
        }
        2
    }

    pub(super) fn run(path: &Path) -> serde_json::Value {
        let module = Module::open(path);
        let stable_parameter_count = assert_stable_parameter_order(&module);
        Native::new(&module).assert_bass_controls();
        let mut results = Vec::new();
        for input in INPUTS {
            for output in OUTPUTS {
                if !input.roles.iter().all(|role| output.roles.contains(role)) {
                    continue;
                }
                for rate in [48_000, 96_000] {
                    results.push(exercise(&module, input, output, rate));
                }
            }
        }
        assert_eq!(
            results.len(),
            32,
            "all supported CLAP surround layout/rate pairs must run"
        );
        let mut bass = Vec::new();
        for input in INPUTS {
            let output = if input.name == "7.1" {
                OUTPUTS[6]
            } else {
                OUTPUTS[3]
            };
            for rate in [48_000, 96_000] {
                for mode in [1, 2] {
                    bass.push(exercise_bass(&module, input, output, rate, mode));
                }
            }
        }
        assert_eq!(bass.len(), 12);
        let rejected_bass = reject_unapproved_bass(&module);
        let restart = restart_bass(&module);
        let stereo_bass_roundtrips = stereo_bass_roundtrip(&module);
        serde_json::json!({"cases":results,"bass_cases":bass,"rejected_bass_then_recovered":rejected_bass,"active_bass_restart":restart,"stereo_bass_roundtrips":stereo_bass_roundtrips,"stable_parameter_count":stable_parameter_count,"dynamic_library":true})
    }
}
