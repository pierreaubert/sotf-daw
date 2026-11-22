mod surround {
    use super::*;
    use std::io::{BufWriter, Write};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }

    #[derive(Clone, Copy, Debug)]
    struct Layout {
        name: &'static str,
        arrangement: u64,
        roles: &'static [&'static str],
    }

    // VST3 orders bus channels by speaker bit position. In 7.1, Ls/Rs
    // represent the rear pair and Sl/Sr the side pair. These fixtures use
    // SDK speaker identities rather than the plugin's channel mapping.
    // https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__speakerArrangements.html
    const WIDE: u64 = (1 << 59) | (1 << 60);
    const INPUTS: [Layout; 3] = [
        Layout {
            name: "5.0",
            arrangement: vst3_sys::vst::k50,
            roles: &["FL", "FR", "C", "SL", "SR"],
        },
        Layout {
            name: "5.1",
            arrangement: vst3_sys::vst::k51,
            roles: &["FL", "FR", "C", "LFE", "SL", "SR"],
        },
        Layout {
            name: "7.1",
            arrangement: vst3_sys::vst::k71Music,
            roles: &["FL", "FR", "C", "LFE", "BL", "BR", "SL", "SR"],
        },
    ];
    const OUTPUTS: [Layout; 10] = [
        INPUTS[0],
        INPUTS[1],
        Layout {
            name: "5.1.2",
            arrangement: vst3_sys::vst::k51 | vst3_sys::vst::kStereoTF,
            roles: &["FL", "FR", "C", "LFE", "SL", "SR", "TFL", "TFR"],
        },
        Layout {
            name: "5.1.4",
            arrangement: vst3_sys::vst::k51_4,
            roles: &[
                "FL", "FR", "C", "LFE", "SL", "SR", "TFL", "TFR", "TBL", "TBR",
            ],
        },
        INPUTS[2],
        Layout {
            name: "7.1.2",
            arrangement: vst3_sys::vst::k71Music | vst3_sys::vst::kStereoTF,
            roles: &["FL", "FR", "C", "LFE", "BL", "BR", "SL", "SR", "TFL", "TFR"],
        },
        Layout {
            name: "7.1.4",
            arrangement: vst3_sys::vst::k71_4,
            roles: &[
                "FL", "FR", "C", "LFE", "BL", "BR", "SL", "SR", "TFL", "TFR", "TBL", "TBR",
            ],
        },
        Layout {
            name: "9.1.4",
            arrangement: vst3_sys::vst::k71_4 | WIDE,
            roles: &[
                "FL", "FR", "C", "LFE", "BL", "BR", "SL", "SR", "TFL", "TFR", "TBL", "TBR", "WL",
                "WR",
            ],
        },
        Layout {
            name: "9.1.6",
            arrangement: vst3_sys::vst::k71_4 | vst3_sys::vst::kStereoTS | WIDE,
            roles: &[
                "FL", "FR", "C", "LFE", "BL", "BR", "SL", "SR", "TFL", "TFR", "TBL", "TBR", "TML",
                "TMR", "WL", "WR",
            ],
        },
        Layout {
            name: "5.1.6",
            arrangement: vst3_sys::vst::k51
                | vst3_sys::vst::kStereoTF
                | vst3_sys::vst::kStereoTR
                | vst3_sys::vst::kStereoTS,
            roles: &[
                "FL", "FR", "C", "LFE", "SL", "SR", "TFL", "TFR", "TBL", "TBR", "TML", "TMR",
            ],
        },
    ];

    pub(super) fn sample(channel: usize, frame: usize) -> f32 {
        let mut value = (frame as u32).wrapping_add(1).wrapping_mul(0x9e37_79b9)
            ^ (channel as u32).wrapping_add(1).wrapping_mul(0x85eb_ca6b);
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        ((value >> 8) as f32 / 16_777_216.0 - 0.5) * 0.2
    }

    fn configure(native: &Native<'_>, input: Layout, output: Layout) {
        let mut input_arrangement = input.arrangement;
        let mut output_arrangement = output.arrangement;
        // SAFETY: the retained component is initialized and both arrangement pointers
        // refer to live values for this synchronous, inactive host call.
        unsafe {
            assert_eq!(
                native.processor.set_bus_arrangements(
                    &mut input_arrangement,
                    1,
                    &mut output_arrangement,
                    1,
                ),
                kResultOk,
                "{} to {}",
                input.name,
                output.name
            );
            for (direction, layout) in [
                (BusDirections::kInput, input),
                (BusDirections::kOutput, output),
            ] {
                assert_eq!(
                    native
                        .component
                        .get_bus_count(MediaTypes::kAudio as i32, direction as i32),
                    1
                );
                let mut info = std::mem::zeroed::<BusInfo>();
                assert_eq!(
                    native.component.get_bus_info(
                        MediaTypes::kAudio as i32,
                        direction as i32,
                        0,
                        &mut info,
                    ),
                    kResultOk
                );
                assert_eq!(info.channel_count as usize, layout.roles.len());
                let mut actual = 0;
                assert_eq!(
                    native
                        .processor
                        .get_bus_arrangement(direction as i32, 0, &mut actual),
                    kResultOk
                );
                assert_eq!(actual, layout.arrangement);
                assert_eq!(
                    native.component.activate_bus(
                        MediaTypes::kAudio as i32,
                        direction as i32,
                        0,
                        1,
                    ),
                    kResultOk
                );
            }
        }
    }

    fn scalar_event(native: &Native<'_>, id: &str, value: f64) -> VstPtr<dyn IParameterChanges> {
        let id = parameter_id(id);
        // SAFETY: the retained controller owns the stable parameter ID.
        let normalized = unsafe { native.controller.plain_param_to_normalized(id, value) };
        let queue = ScalarQueue::allocate(id, normalized);
        // SAFETY: each boxed object transfers its sole COM interface exactly once.
        let queue =
            unsafe { VstPtr::<dyn IParamValueQueue>::owned(Box::into_raw(queue).cast()).unwrap() };
        let changes = ScalarChanges::allocate(queue);
        // SAFETY: the changes object owns the queue for its complete lifetime.
        unsafe { VstPtr::<dyn IParameterChanges>::owned(Box::into_raw(changes).cast()).unwrap() }
    }

    fn assert_stable_parameter_order(module: &Module) -> usize {
        use vst3_sys::vst::ParameterInfo;
        let ids = |native: &Native<'_>| {
            // SAFETY: the retained controller fills each live POD record synchronously.
            unsafe {
                (0..native.controller.get_parameter_count())
                    .map(|index| {
                        let mut info = std::mem::zeroed::<ParameterInfo>();
                        assert_eq!(
                            native.controller.get_parameter_info(index, &mut info),
                            kResultOk
                        );
                        info.id
                    })
                    .collect::<Vec<_>>()
            }
        };
        let expected = ids(&Native::new_unconnected(module));
        assert!(!expected.is_empty());
        for _ in 0..4 {
            assert_eq!(ids(&Native::new_unconnected(module)), expected);
        }
        expected.len()
    }

    fn assert_bass_controls(module: &Module) {
        use vst3_sys::vst::{ParameterFlags, ParameterInfo};
        let native = Native::new_unconnected(module);
        for id in [
            "bass_mode",
            "bass_cutoff_hz",
            "bass_gain_db",
            "lfe_playback_gain_db",
            "combine_input_lfe",
            "bass_source_front_left",
            "bass_source_front_right",
            "bass_source_center",
            "bass_source_side_left",
            "bass_source_side_right",
            "bass_source_back_left",
            "bass_source_back_right",
        ] {
            let mut found = false;
            // SAFETY: the retained controller fills the initialized POD record synchronously.
            unsafe {
                for index in 0..native.controller.get_parameter_count() {
                    let mut info = std::mem::zeroed::<ParameterInfo>();
                    assert_eq!(
                        native.controller.get_parameter_info(index, &mut info),
                        kResultOk
                    );
                    if info.id == parameter_id(id) {
                        const VST3_HIDDEN: i32 = 1 << 4;
                        assert_eq!(
                            info.flags
                                & (VST3_HIDDEN
                                    | ParameterFlags::kIsReadOnly as i32
                                    | ParameterFlags::kCanAutomate as i32),
                            0,
                            "{id} must be visible, editable, and non-automatable"
                        );
                        found = true;
                        break;
                    }
                }
            }
            assert!(found, "missing VST3 bass control {id}");
        }
        for (normalized, expected) in [(0.0, "Off"), (0.5, "Derived"), (1.0, "Managed")] {
            let mut text = [0_i16; 128];
            // SAFETY: the controller and writable UTF-16 buffer remain live during the call.
            assert_eq!(
                unsafe {
                    native.controller.get_param_string_by_value(
                        parameter_id("bass_mode"),
                        normalized,
                        text.as_mut_ptr(),
                    )
                },
                kResultOk
            );
            let length = text.iter().position(|value| *value == 0).unwrap();
            assert!(
                text[..length]
                    .iter()
                    .copied()
                    .eq(expected.encode_utf16().map(|value| value as i16))
            );
        }
    }

    fn exercise(
        module: &Module,
        input: Layout,
        output: Layout,
        rate: u32,
        capture: &mut Option<BufWriter<std::fs::File>>,
    ) -> serde_json::Value {
        let mut native = Native::new_unconnected(module);
        configure(&native, input, output);
        let mut saved = native.saved();
        for id in [
            "binaural_preview",
            "auto_gain_enabled",
            "enable_subharmonic_synth",
        ] {
            saved.params.insert(id.into(), ParamValue::Bool(true));
        }
        assert_eq!(native.load(&saved, true), kResultOk);
        let restored = native.saved();
        for id in [
            "binaural_preview",
            "auto_gain_enabled",
            "enable_subharmonic_synth",
        ] {
            assert!(matches!(
                restored.params.get(id),
                Some(ParamValue::Bool(true))
            ));
        }
        native.activate(rate);
        let latency = native.latency() as usize;
        native.start();

        let mut inputs = vec![vec![0.0_f32; MAX_FRAMES]; input.roles.len()];
        let mut outputs = vec![vec![0.0_f32; MAX_FRAMES]; output.roles.len()];
        let mut input_ptrs: Vec<*mut c_void> = inputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect();
        let mut output_ptrs: Vec<*mut c_void> = outputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect();
        let mut input_bus = AudioBusBuffers {
            num_channels: input.roles.len() as i32,
            silence_flags: 0,
            buffers: input_ptrs.as_mut_ptr(),
        };
        let mut output_bus = AudioBusBuffers {
            num_channels: output.roles.len() as i32,
            silence_flags: 0,
            buffers: output_ptrs.as_mut_ptr(),
        };
        // SAFETY: ProcessData consists of POD fields and nullable host pointers.
        let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
        data.process_mode = ProcessModes::kRealtime as i32;
        data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
        data.num_inputs = 1;
        data.num_outputs = 1;
        data.inputs = &mut input_bus;
        data.outputs = &mut output_bus;
        let sources: Vec<_> = output
            .roles
            .iter()
            .map(|role| input.roles.iter().position(|candidate| candidate == role))
            .collect();
        let has_heights = output.roles.iter().any(|role| role.starts_with('T'));
        let mut frame = 0;
        let mut sample_hash = 0xcbf2_9ce4_8422_2325_u64;
        let phases = [
            (None, false),
            (Some(("bypass_heights", 1.0)), true),
            (Some(("bypass_heights", 0.0)), false),
            (Some(("bypass_surround", 1.0)), true),
            (Some(("bypass_surround", 0.0)), false),
        ];
        for (phase, (event, bypassed)) in phases.into_iter().enumerate() {
            let length = 8192 + if phase == 0 { latency } else { 0 };
            let end = frame + length;
            let event = event.map(|(id, value)| scalar_event(&native, id, value));
            let mut first = true;
            let mut block_index = 0;
            let mut height_energy = 0.0_f64;
            let mut height_peak = 0.0_f32;
            while frame < end {
                let count = [1, 17, 63, 128, 257, 7, 251][block_index % 7].min(end - frame);
                block_index += 1;
                for (channel, values) in inputs.iter_mut().enumerate() {
                    for (offset, value) in values[..count].iter_mut().enumerate() {
                        *value = sample(channel, frame + offset);
                    }
                }
                for channel in &mut outputs {
                    channel[..count].fill(f32::NAN);
                }
                // SAFETY: both bus pointers refer to the live host-owned bus values above.
                unsafe {
                    (*data.inputs).silence_flags = 0;
                    (*data.outputs).silence_flags = 0;
                }
                data.num_samples = count as i32;
                if first && let Some(changes) = &event {
                    // SAFETY: the borrowed interface points to `event`, retained through process.
                    data.input_param_changes = unsafe {
                        std::mem::transmute::<
                            *mut *mut <dyn IParameterChanges as ComInterface>::VTable,
                            StaticVstPtr<dyn IParameterChanges>,
                        >(changes.as_ptr())
                    };
                }
                // SAFETY: disjoint planar buffers, bus pointers, and any event remain live.
                let status = unsafe { native.processor.process(&mut data) };
                assert_eq!(
                    status, kResultOk,
                    "{} to {} phase {phase}",
                    input.name, output.name
                );
                // SAFETY: the zero interface is a valid absence of parameter changes.
                data.input_param_changes = unsafe { std::mem::zeroed() };
                first = false;
                for (channel, values) in outputs.iter().enumerate() {
                    let role = output.roles[channel];
                    for (offset, &actual) in values[..count].iter().enumerate() {
                        if let Some(capture) = capture.as_mut() {
                            capture.write_all(&actual.to_bits().to_le_bytes()).unwrap();
                        }
                        sample_hash ^= u64::from(actual.to_bits());
                        sample_hash = sample_hash.wrapping_mul(0x100_0000_01b3);
                        assert!(
                            actual.is_finite(),
                            "{} to {} {role}",
                            input.name,
                            output.name
                        );
                        let time = frame + offset;
                        if role.starts_with('T') {
                            if time >= end - 1024 {
                                height_energy += f64::from(actual).powi(2);
                                height_peak = height_peak.max(actual.abs());
                            }
                        } else {
                            let expected = sources[channel]
                                .filter(|_| time >= latency)
                                .map_or(0.0, |source| sample(source, time - latency));
                            assert_eq!(
                                actual.to_bits(),
                                expected.to_bits(),
                                "{} to {} {role} frame {time} phase {phase}",
                                input.name,
                                output.name,
                            );
                        }
                    }
                }
                frame += count;
            }
            if has_heights {
                if bypassed {
                    assert!(
                        height_peak < 1e-7,
                        "{} to {} phase {phase}: {height_peak}",
                        input.name,
                        output.name
                    );
                } else {
                    assert!(
                        height_energy > 1e-12,
                        "{} to {} phase {phase}",
                        input.name,
                        output.name
                    );
                }
            }
            assert_eq!(native.latency() as usize, latency);
        }
        native.notifications();
        serde_json::json!({"input":input.name,"output":output.name,"rate":rate,"latency":latency,"frames":frame,"phases":phases.len(),"sample_hash":format!("{sample_hash:016x}")})
    }

    fn exercise_bass(
        module: &Module,
        input: Layout,
        output: Layout,
        rate: u32,
        mode: i32,
    ) -> serde_json::Value {
        let mut native = Native::new_unconnected(module);
        configure(&native, input, output);
        let mut state = native.saved();
        state
            .params
            .insert("bass_mode".into(), ParamValue::I32(mode));
        state
            .params
            .insert("bass_gain_db".into(), ParamValue::F32(-6.0));
        state
            .params
            .insert("lfe_playback_gain_db".into(), ParamValue::F32(10.0));
        state.params.insert(
            "combine_input_lfe".into(),
            ParamValue::Bool(input.roles.contains(&"LFE")),
        );
        assert_eq!(native.load(&state, true), kResultOk);
        assert!(
            matches!(native.saved().params.get("bass_mode"), Some(ParamValue::I32(value)) if *value == mode)
        );
        native.activate(rate);
        let latency = native.latency();
        native.start();

        let amplitudes: Vec<_> = (0..input.roles.len())
            .map(|channel| (channel + 1) as f32 / 100.0)
            .collect();
        let mut inputs: Vec<_> = amplitudes
            .iter()
            .map(|amplitude| vec![*amplitude; MAX_FRAMES])
            .collect();
        let mut outputs = vec![vec![f32::NAN; MAX_FRAMES]; output.roles.len()];
        let mut input_ptrs: Vec<*mut c_void> = inputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect();
        let mut output_ptrs: Vec<*mut c_void> = outputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect();
        let mut input_bus = AudioBusBuffers {
            num_channels: input.roles.len() as i32,
            silence_flags: 0,
            buffers: input_ptrs.as_mut_ptr(),
        };
        let mut output_bus = AudioBusBuffers {
            num_channels: output.roles.len() as i32,
            silence_flags: 0,
            buffers: output_ptrs.as_mut_ptr(),
        };
        // SAFETY: ProcessData consists of POD fields and nullable host pointers.
        let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
        data.process_mode = ProcessModes::kRealtime as i32;
        data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
        data.num_samples = MAX_FRAMES as i32;
        data.num_inputs = 1;
        data.num_outputs = 1;
        data.inputs = &mut input_bus;
        data.outputs = &mut output_bus;
        const BLOCKS: usize = 200;
        for _ in 0..BLOCKS {
            // SAFETY: disjoint planar buffers and bus pointers remain live through the call.
            assert_eq!(unsafe { native.processor.process(&mut data) }, kResultOk);
        }

        let sum: f32 = input
            .roles
            .iter()
            .zip(&amplitudes)
            .filter(|(role, _)| **role != "LFE")
            .map(|(_, amplitude)| *amplitude)
            .sum();
        let existing_lfe = input
            .roles
            .iter()
            .position(|role| *role == "LFE")
            .map_or(0.0, |index| amplitudes[index]);
        let expected_lfe = existing_lfe + sum * 10.0_f32.powf(-16.0 / 20.0);
        let lfe_index = output.roles.iter().position(|role| *role == "LFE").unwrap();
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
                && *role != "LFE"
            {
                let expected = if mode == 1 { amplitudes[source] } else { 0.0 };
                assert!(
                    (actual - expected).abs() < 1e-5,
                    "{} to {} {role} mode {mode}: {actual} vs {expected}",
                    input.name,
                    output.name
                );
            }
        }
        assert_eq!(native.latency(), latency);
        native.notifications();
        serde_json::json!({"input":input.name,"output":output.name,"rate":rate,"mode":if mode == 1 {"derived"} else {"managed"},"latency":latency,"frames":BLOCKS * MAX_FRAMES,"lfe":lfe,"expected_lfe":expected_lfe})
    }

    fn reject_unsupported(module: &Module) -> usize {
        let cases = [
            ("7.1 to 5.1", INPUTS[2].arrangement, OUTPUTS[1].arrangement),
            (
                "7.1 to 5.1.6",
                INPUTS[2].arrangement,
                OUTPUTS[9].arrangement,
            ),
            ("5.1 to 5.0", INPUTS[1].arrangement, OUTPUTS[0].arrangement),
            (
                "5.0 to stereo",
                INPUTS[0].arrangement,
                vst3_sys::vst::kStereo,
            ),
            (
                "7.1 to 7.1.2 top-side",
                INPUTS[2].arrangement,
                vst3_sys::vst::k71_2,
            ),
        ];
        for (name, mut input, mut output) in cases {
            let native = Native::new_unconnected(module);
            // SAFETY: the initialized component and arrangement pointers remain live.
            let result = unsafe {
                native
                    .processor
                    .set_bus_arrangements(&mut input, 1, &mut output, 1)
            };
            assert_ne!(result, kResultOk, "unsupported layout {name} was accepted");
        }
        cases.len()
    }

    fn reject_unapproved_bass(module: &Module) -> usize {
        let cases = [
            (INPUTS[0], OUTPUTS[0], false),
            (INPUTS[1], OUTPUTS[3], true),
            (INPUTS[2], OUTPUTS[6], true),
        ];
        for (input, output, has_existing_lfe) in cases {
            let mut native = Native::new_unconnected(module);
            configure(&native, input, output);
            let mut state = native.saved();
            state.params.insert("bass_mode".into(), ParamValue::I32(2));
            state
                .params
                .insert("combine_input_lfe".into(), ParamValue::Bool(false));
            assert_eq!(native.load(&state, true), kResultOk);
            let setup = ProcessSetup {
                process_mode: ProcessModes::kRealtime as i32,
                symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
                max_samples_per_block: MAX_FRAMES as i32,
                sample_rate: 48_000.0,
            };
            // SAFETY: the inactive component and setup remain valid through both calls.
            unsafe {
                assert_eq!(native.processor.setup_processing(&setup), kResultOk);
                assert_ne!(
                    native.component.set_active(1),
                    kResultOk,
                    "unsafe bass route accepted for {} to {}",
                    input.name,
                    output.name
                );
            }
            let mut corrected = native.saved();
            if has_existing_lfe {
                corrected
                    .params
                    .insert("combine_input_lfe".into(), ParamValue::Bool(true));
            } else {
                corrected
                    .params
                    .insert("bass_mode".into(), ParamValue::I32(0));
            }
            assert_eq!(native.load(&corrected, true), kResultOk);
            native.activate(48_000);
            native.start();
            native.deactivate();
        }
        cases.len()
    }

    fn restart_bass(module: &Module) -> serde_json::Value {
        let mut native = Native::new_unconnected(module);
        configure(&native, INPUTS[0], OUTPUTS[3]);
        native.activate(48_000);
        let latency = native.latency();
        native.start();
        let mut inputs: Vec<_> = (1..=5)
            .map(|channel| vec![channel as f32 / 100.0; MAX_FRAMES])
            .collect();
        let mut outputs = vec![vec![f32::NAN; MAX_FRAMES]; OUTPUTS[3].roles.len()];
        let mut input_ptrs: Vec<*mut c_void> = inputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect();
        let mut output_ptrs: Vec<*mut c_void> = outputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect();
        let mut input_bus = AudioBusBuffers {
            num_channels: inputs.len() as i32,
            silence_flags: 0,
            buffers: input_ptrs.as_mut_ptr(),
        };
        let mut output_bus = AudioBusBuffers {
            num_channels: outputs.len() as i32,
            silence_flags: 0,
            buffers: output_ptrs.as_mut_ptr(),
        };
        // SAFETY: ProcessData is POD and a zero interface represents no parameter changes.
        let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
        data.process_mode = ProcessModes::kRealtime as i32;
        data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
        data.num_samples = MAX_FRAMES as i32;
        data.num_inputs = 1;
        data.num_outputs = 1;
        data.inputs = &mut input_bus;
        data.outputs = &mut output_bus;
        let event = scalar_event(&native, "bass_mode", 1.0);
        for phase in 0..3 {
            if phase == 2 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !native
                    .notifications()
                    .iter()
                    .any(|flags| flags & RestartFlags::kReloadComponent as i32 != 0)
                {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "active VST3 bass edit did not request a reload"
                    );
                    // SAFETY: the documented default mode is live, and this runs on the host main thread.
                    unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
                }
                native.deactivate();
                native.activate(48_000);
                native.start();
                assert_eq!(native.latency(), latency);
            }
            let blocks = if phase == 1 { 1 } else { 200 };
            for block in 0..blocks {
                if phase == 1 && block == 0 {
                    // SAFETY: the COM interface remains retained by event during this process call.
                    data.input_param_changes = unsafe {
                        std::mem::transmute::<
                            *mut *mut <dyn IParameterChanges as ComInterface>::VTable,
                            StaticVstPtr<dyn IParameterChanges>,
                        >(event.as_ptr())
                    };
                }
                // SAFETY: disjoint planar buffers, buses, and any event remain valid for the call.
                assert_eq!(
                    unsafe { native.processor.process(&mut data) },
                    kResultOk,
                    "phase {phase} block {block}"
                );
                // SAFETY: a zero interface represents no parameter changes.
                data.input_param_changes = unsafe { std::mem::zeroed() };
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
        serde_json::json!({"input":"5.0","output":"5.1.4","rate":48000,"latency":latency,"reload":true})
    }

    fn stereo_bass_roundtrip(module: &Module) -> usize {
        let stereo = Layout {
            name: "Stereo",
            arrangement: vst3_sys::vst::kStereo,
            roles: &["FL", "FR"],
        };
        for mode in [1, 2] {
            let mut native = Native::new_unconnected(module);
            configure(&native, stereo, OUTPUTS[1]);
            let mut state = native.saved();
            state
                .params
                .insert("bass_mode".into(), ParamValue::I32(mode));
            assert_eq!(native.load(&state, true), kResultOk);
            for input in [stereo, INPUTS[0], stereo] {
                configure(&native, input, OUTPUTS[1]);
                native.activate(48_000);
                native.start();
                assert!(
                    matches!(native.saved().params.get("bass_mode"), Some(ParamValue::I32(value)) if *value == mode)
                );
                if input.name == "Stereo" {
                    let mut reference = Native::new(module);
                    reference.activate(48_000);
                    reference.start();
                    let samples = vec![[0.01, 0.02]; MAX_FRAMES];
                    for _ in 0..16 {
                        assert_eq!(native.process(&samples), reference.process(&samples));
                    }
                } else {
                    let mut inputs: Vec<_> = (1..=5)
                        .map(|channel| vec![channel as f32 / 100.0; MAX_FRAMES])
                        .collect();
                    let mut outputs = vec![vec![0.0_f32; MAX_FRAMES]; 6];
                    let mut input_ptrs: Vec<*mut c_void> = inputs
                        .iter_mut()
                        .map(|channel| channel.as_mut_ptr().cast())
                        .collect();
                    let mut output_ptrs: Vec<*mut c_void> = outputs
                        .iter_mut()
                        .map(|channel| channel.as_mut_ptr().cast())
                        .collect();
                    let mut input_bus = AudioBusBuffers {
                        num_channels: 5,
                        silence_flags: 0,
                        buffers: input_ptrs.as_mut_ptr(),
                    };
                    let mut output_bus = AudioBusBuffers {
                        num_channels: 6,
                        silence_flags: 0,
                        buffers: output_ptrs.as_mut_ptr(),
                    };
                    // SAFETY: ProcessData is POD; the disjoint planar buffers live through every call.
                    let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
                    data.process_mode = ProcessModes::kRealtime as i32;
                    data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
                    data.num_samples = MAX_FRAMES as i32;
                    data.num_inputs = 1;
                    data.num_outputs = 1;
                    data.inputs = &mut input_bus;
                    data.outputs = &mut output_bus;
                    for _ in 0..200 {
                        // SAFETY: the retained processor and all buffers remain live during the call.
                        assert_eq!(unsafe { native.processor.process(&mut data) }, kResultOk);
                    }
                    assert!((outputs[3][MAX_FRAMES - 1] - 0.15).abs() < 1e-5);
                }
                native.deactivate();
            }
        }
        2
    }

    pub(super) fn failed_candidate_recovers(module: &Module) {
        let mut native = Native::new_unconnected(module);
        configure(&native, INPUTS[1], OUTPUTS[3]);
        native.activate(96_000);
        let mut invalid = native.saved();
        invalid
            .params
            .insert("bass_mode".into(), ParamValue::I32(1));
        invalid
            .params
            .insert("combine_input_lfe".into(), ParamValue::Bool(false));
        assert_eq!(native.load(&invalid, false), kResultOk);
        native.deactivate();
        // SAFETY: the component remains inactive after this failed candidate activation.
        assert_eq!(unsafe { native.component.set_active(1) }, kResultFalse);
        let mut sparse = native.saved();
        sparse.params.retain(|id, _| id == "height_gain");
        sparse
            .params
            .insert("height_gain".into(), ParamValue::F32(0.3));
        assert_eq!(native.load(&sparse, false), kResultOk);
        native.activate(96_000);
        assert_eq!(native.latency(), 2176);
        assert!(matches!(
            native.saved().params.get("bass_mode"),
            Some(ParamValue::I32(0))
        ));
    }

    pub(super) fn benchmark(module: &Module) -> serde_json::Value {
        const WARMUP: usize = 512;
        const BLOCKS: usize = 4_000;
        let mut cases = Vec::new();
        for (input, output) in [(INPUTS[0], OUTPUTS[3]), (INPUTS[2], OUTPUTS[6])] {
            for (rate, frames) in [(48_000, 128), (96_000, 64)] {
                for bypass in [false, true] {
                    let mut native = Native::new_unconnected(module);
                    configure(&native, input, output);
                    let mut state = native.saved();
                    state
                        .params
                        .insert("bypass_surround".into(), ParamValue::Bool(bypass));
                    assert_eq!(native.load(&state, true), kResultOk);
                    native.activate(rate);
                    assert_eq!(native.latency(), if rate >= 96_000 { 2176 } else { 2048 });
                    native.start();
                    let mut inputs: Vec<Vec<f32>> = (0..input.roles.len())
                        .map(|channel| (0..frames).map(|frame| sample(channel, frame)).collect())
                        .collect();
                    let mut outputs = vec![vec![0.0_f32; frames]; output.roles.len()];
                    let mut input_ptrs: Vec<*mut c_void> = inputs
                        .iter_mut()
                        .map(|channel| channel.as_mut_ptr().cast())
                        .collect();
                    let mut output_ptrs: Vec<*mut c_void> = outputs
                        .iter_mut()
                        .map(|channel| channel.as_mut_ptr().cast())
                        .collect();
                    let mut input_bus = AudioBusBuffers {
                        num_channels: inputs.len() as i32,
                        silence_flags: 0,
                        buffers: input_ptrs.as_mut_ptr(),
                    };
                    let mut output_bus = AudioBusBuffers {
                        num_channels: outputs.len() as i32,
                        silence_flags: 0,
                        buffers: output_ptrs.as_mut_ptr(),
                    };
                    // SAFETY: ProcessData is POD; all planar buffers remain live and disjoint.
                    let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
                    data.process_mode = ProcessModes::kRealtime as i32;
                    data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
                    data.num_samples = frames as i32;
                    data.num_inputs = 1;
                    data.num_outputs = 1;
                    data.inputs = &mut input_bus;
                    data.outputs = &mut output_bus;
                    let mut errors = 0;
                    for _ in 0..WARMUP {
                        // SAFETY: the retained processor and prepared buffers remain valid.
                        errors += usize::from(
                            unsafe { native.processor.process(&mut data) } != kResultOk,
                        );
                    }
                    let start = std::time::Instant::now();
                    for _ in 0..BLOCKS {
                        // SAFETY: the retained processor and prepared buffers remain valid.
                        errors += usize::from(
                            unsafe { native.processor.process(&mut data) } != kResultOk,
                        );
                    }
                    let elapsed_ns = start.elapsed().as_nanos();
                    let energy: f64 = outputs
                        .iter()
                        .flatten()
                        .map(|sample| f64::from(*sample).powi(2))
                        .sum();
                    let height_energy: f64 = outputs[outputs.len() - 4..]
                        .iter()
                        .flatten()
                        .map(|sample| f64::from(*sample).powi(2))
                        .sum();
                    assert_eq!(errors, 0);
                    assert!(energy.is_finite() && energy > 0.0);
                    if bypass {
                        assert_eq!(height_energy, 0.0);
                    } else {
                        assert!(height_energy.is_finite() && height_energy > 0.0);
                    }
                    cases.push(serde_json::json!({
                        "input": input.name, "output": output.name, "rate": rate,
                        "frames": frames, "bypass": bypass, "warmup_blocks": WARMUP,
                        "blocks": BLOCKS, "elapsed_ns": elapsed_ns, "errors": errors,
                        "output_energy": energy, "height_energy": height_energy,
                    }));
                }
            }
        }
        serde_json::json!({"dynamic_library": true, "cases": cases})
    }

    fn five_one_six_preset_roundtrip(module: &Module) {
        let mut native = Native::new_unconnected(module);
        configure(&native, INPUTS[0], OUTPUTS[9]);
        native.activate(48_000);
        assert!(matches!(
            native.saved().params.get("speaker_config"),
            Some(ParamValue::I32(10))
        ));
        let preset = native.saved_bytes();
        native.deactivate();
        assert_eq!(native.load_bytes(&preset, 0, usize::MAX), kResultOk);
        native.activate(48_000);
        let mut output_arrangement = 0;
        // SAFETY: the retained processor writes one live arrangement value.
        assert_eq!(
            unsafe {
                native.processor.get_bus_arrangement(
                    BusDirections::kOutput as i32,
                    0,
                    &mut output_arrangement,
                )
            },
            kResultOk
        );
        assert_eq!(output_arrangement, OUTPUTS[9].arrangement);
        assert!(matches!(
            native.saved().params.get("speaker_config"),
            Some(ParamValue::I32(10))
        ));
    }

    pub(super) fn run(module: &Module) -> serde_json::Value {
        let stable_parameter_count = assert_stable_parameter_order(module);
        assert_bass_controls(module);
        five_one_six_preset_roundtrip(module);
        let mut capture = std::env::var_os("SOTF_UPMIXER_QA_OUTPUT").map(|path| {
            BufWriter::new(std::fs::File::create(path).expect("create sample capture"))
        });
        let mut results = Vec::new();
        for input in INPUTS {
            for output in OUTPUTS {
                if !input.roles.iter().all(|role| output.roles.contains(role)) {
                    continue;
                }
                for rate in [48_000, 96_000] {
                    results.push(exercise(module, input, output, rate, &mut capture));
                }
            }
        }
        assert_eq!(results.len(), 48, "all planned layout/rate pairs must run");
        if let Some(capture) = capture.as_mut() {
            capture.flush().expect("flush sample capture");
        }
        let mut bass = Vec::new();
        for input in INPUTS {
            let output = if input.name == "7.1" {
                OUTPUTS[6]
            } else {
                OUTPUTS[3]
            };
            for rate in [48_000, 96_000] {
                for mode in [1, 2] {
                    bass.push(exercise_bass(module, input, output, rate, mode));
                }
            }
        }
        for input in [INPUTS[0], INPUTS[1]] {
            for rate in [48_000, 96_000] {
                for mode in [1, 2] {
                    bass.push(exercise_bass(module, input, OUTPUTS[9], rate, mode));
                }
            }
        }
        assert_eq!(bass.len(), 20);
        let rejected = reject_unsupported(module);
        let rejected_bass = reject_unapproved_bass(module);
        let restart = restart_bass(module);
        let stereo_bass_roundtrips = stereo_bass_roundtrip(module);
        serde_json::json!({"cases":results,"bass_cases":bass,"rejected_layouts":rejected,"rejected_bass_then_recovered":rejected_bass,"active_bass_restart":restart,"stereo_bass_roundtrips":stereo_bass_roundtrips,"stable_parameter_count":stable_parameter_count,"dynamic_library":true})
    }
}
