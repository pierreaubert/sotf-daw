//! Process the generated wrappers through NIH buffers, including auxiliary buses.
use super::*;
use nih_plug::prelude::*;

// Direct routing tests exercise the common process body. Native transport
// extraction is tested through actual NIH CLAP callbacks because NIH does not
// expose a safe constructor for its opaque Transport type.
trait TestProcess: Plugin {
    fn process_without_transport(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus;
}

struct TestContext;
impl<P: Plugin> InitContext<P> for TestContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute(&self, _: P::BackgroundTask) {}
    fn set_latency_samples(&self, _: u32) {}
    fn set_current_voice_capacity(&self, _: u32) {}
}
impl<P: Plugin> ProcessContext<P> for TestContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute_background(&self, _: P::BackgroundTask) {}
    fn execute_gui(&self, _: P::BackgroundTask) {}
    fn transport(&self) -> &Transport {
        panic!("transport not used by wrapper")
    }
    fn next_event(&mut self) -> Option<PluginNoteEvent<P>> {
        None
    }
    fn send_event(&mut self, _: PluginNoteEvent<P>) {}
    fn set_latency_samples(&self, _: u32) {}
    fn set_current_voice_capacity(&self, _: u32) {}
}

fn initialize<P: Plugin>(plugin: &mut P) {
    assert!(plugin.initialize(
        &P::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: 48000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext
    ));
}

fn check_routing<P: Plugin + TestProcess>(
    plugin: &mut P,
    name: &str,
    params: &crate::params::DynamicParams,
) {
    let (inputs, outputs) = plugin_io_channels(name);
    let layout = P::AUDIO_IO_LAYOUTS[0];
    let main_inputs = layout.main_input_channels.unwrap().get() as usize;
    assert_eq!(
        main_inputs
            + layout
                .aux_input_ports
                .iter()
                .map(|width| width.get() as usize)
                .sum::<usize>(),
        inputs
    );
    assert_eq!(layout.main_output_channels.unwrap().get() as usize, outputs);
    let mut direct = crate::params::configuration::create_plugin(name, 48_000, params).unwrap();
    direct = plugins_bridge::prepare_standalone_plugin(direct, 257).unwrap();
    direct.initialize(48000).unwrap();
    params.sync_to_plugin(direct.as_mut()).unwrap();
    let mut position = 0;
    for frames in [1, 17, 257, 63, 2, 191, 256, 7]
        .into_iter()
        .cycle()
        .take(32)
    {
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                (0..inputs).map(move |channel| {
                    let phase = (position + frame) as f32 * (0.013 + channel as f32 * 0.007);
                    phase.sin() * (channel + 1) as f32 * 0.03
                })
            })
            .collect();
        let mut expected = vec![0.0; frames * outputs];
        let produced = direct
            .process(
                &input,
                &mut expected,
                &sotf_host::plugin::ProcessContext::new(48000, frames),
            )
            .unwrap();
        assert_eq!(produced, frames, "{name}");
        let mut main = vec![vec![0.0; frames]; outputs];
        for channel in 0..main_inputs {
            for frame in 0..frames {
                main[channel][frame] = input[frame * inputs + channel];
            }
        }
        let mut aux_data = vec![vec![0.0; frames]; inputs - main_inputs];
        for (channel, data) in aux_data.iter_mut().enumerate() {
            for frame in 0..frames {
                data[frame] = input[frame * inputs + main_inputs + channel];
            }
        }
        let mut buffer = Buffer::default();
        // SAFETY: all slices have `frames` samples and live through the callback.
        unsafe {
            buffer.set_slices(frames, |slices| {
                slices.extend(main.iter_mut().map(Vec::as_mut_slice))
            });
        }
        let mut aux_buffer = Buffer::default();
        // SAFETY: auxiliary storage is distinct and lives through the callback.
        unsafe {
            aux_buffer.set_slices(frames, |slices| {
                slices.extend(aux_data.iter_mut().map(Vec::as_mut_slice))
            });
        }
        let mut aux_input = if inputs > main_inputs {
            vec![aux_buffer]
        } else {
            vec![]
        };
        let mut auxiliary = AuxiliaryBuffers {
            inputs: &mut aux_input,
            outputs: &mut [],
        };
        let status =
            plugin.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext);
        assert!(
            !matches!(status, ProcessStatus::Error(_)),
            "{name}: {status:?}"
        );
        for (channel, samples) in buffer.as_slice_immutable().iter().enumerate() {
            for (frame, actual) in samples.iter().enumerate() {
                let reference = expected[frame * outputs + channel];
                assert!(
                    actual.is_finite() && (*actual - reference).abs() < 1e-5,
                    "{name}: frame {} channel {channel}: {actual} vs {reference}",
                    position + frame
                );
            }
        }
        position += frames;
    }
}

macro_rules! routing_test {
    ($test:ident, $wrapper:ident, $name:tt) => {
        sotf_nih_plugin!($wrapper, plugin_type: $name, name: $name, clap_id: "org.sotf.test", vst3_class_id: *b"SotfTest00000001", channels: 2);
        impl TestProcess for $wrapper {
            fn process_without_transport(
                &mut self,
                buffer: &mut Buffer,
                aux: &mut AuxiliaryBuffers,
                _context: &mut impl ProcessContext<Self>,
            ) -> ProcessStatus {
                self.process_with_transport(buffer, aux, Default::default())
            }
        }
        #[test]
        fn $test() {
            let mut plugin = $wrapper::default();
            initialize(&mut plugin);
            let params = plugin.params.clone();
            check_routing(&mut plugin, $name, &params);
        }
    };
}
routing_test!(stereo_routing, StereoWrapper, "Gain");
routing_test!(mono_to_stereo_routing, MonoWrapper, "MonoToStereo");
routing_test!(ambisonics_routing, AmbisonicsWrapper, "AmbisonicsDecoder");
routing_test!(upmixer_routing, UpmixerWrapper, "Upmixer");
routing_test!(aae_routing, AaeWrapper, "AAE");
routing_test!(band_split_routing, SplitWrapper, "BandSplit");
routing_test!(band_merge_routing, MergeWrapper, "BandMerge");
routing_test!(aec_routing, AecWrapper, "AEC");
routing_test!(beamformer_routing, BeamformerWrapper, "Beamformer");
routing_test!(eq_routing, EqWrapper, "EQ");
routing_test!(limiter_routing, LimiterWrapper, "Limiter");
routing_test!(crossfeed_routing, CrossfeedWrapper, "Crossfeed");
routing_test!(saturation_routing, SaturationWrapper, "Saturation");

fn restored_params(
    name: &str,
    overrides: &[(&str, f64)],
) -> std::sync::Arc<crate::params::DynamicParams> {
    let bridge = plugins_bridge::param_bridge::ParamBridge::new(get_param_specs(name));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    for info in &mut infos {
        info.id = legacy_external_param_id(name, &info.id).into_owned();
        if let Some((_, value)) = overrides.iter().find(|(id, _)| *id == info.id) {
            info.default_value = *value;
        }
    }
    crate::params::DynamicParams::from_infos(&infos)
}

#[test]
fn saturation_restores_all_modes_and_oversampling_with_actual_host_latency() {
    struct LatencyContext(std::cell::Cell<u32>);
    impl InitContext<SaturationWrapper> for LatencyContext {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        fn execute(&self, _: ()) {}
        fn set_latency_samples(&self, value: u32) {
            self.0.set(value);
        }
        fn set_current_voice_capacity(&self, _: u32) {}
    }
    let modes = ["Soft Clip", "Tube", "Tape", "Exciter", "Asymmetric"];
    let factors = ["Off", "2x", "4x"];
    for (mode, label) in modes.iter().enumerate() {
        for (index, factor) in factors.iter().enumerate() {
            let mut wrapper = SaturationWrapper {
                params: restored_params(
                    "Saturation",
                    &[("mode", mode as f64), ("oversampling", index as f64)],
                ),
                ..Default::default()
            };
            let mut context = LatencyContext(std::cell::Cell::new(u32::MAX));
            assert!(
                wrapper.initialize(
                    &SaturationWrapper::AUDIO_IO_LAYOUTS[0],
                    &BufferConfig {
                        sample_rate: 48_000.0,
                        min_buffer_size: Some(1),
                        max_buffer_size: 257,
                        process_mode: ProcessMode::Realtime
                    },
                    &mut context,
                ),
                "mode={label}, oversampling={factor}"
            );
            let inner = wrapper.inner.as_ref().unwrap();
            assert_eq!(
                inner.get_parameter(&"mode".into()),
                Some(sotf_host::ParameterValue::String((*label).into()))
            );
            assert_eq!(
                inner.get_parameter(&"oversampling".into()),
                Some(sotf_host::ParameterValue::String((*factor).into()))
            );
            assert_eq!(
                inner.preferred_oversampling(),
                None,
                "preference must be consumed"
            );
            assert_eq!(inner.latency_samples(), context.0.get() as usize);
            assert_eq!(context.0.get() == 0, index == 0);
            let params = wrapper.params.clone();
            check_routing(&mut wrapper, "Saturation", &params);
        }
    }
}

#[test]
fn saturation_callback_reads_and_updates_continuous_controls_without_allocating() {
    let mut wrapper = SaturationWrapper::default();
    initialize(&mut wrapper);
    let mut samples = [vec![0.1; 257], vec![-0.2; 257]];
    let mut buffer = Buffer::default();
    // SAFETY: disjoint channel slices have 257 samples and outlive the callback.
    unsafe {
        buffer.set_slices(257, |slices| {
            slices.extend(samples.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let mut auxiliary = AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    for overrides in [
        [
            ("drive", 5.0),
            ("tone", 2.0),
            ("mix", 0.75),
            ("output_gain", -3.0),
            ("dynamic_amount", 0.3),
            ("dynamic_attack_ms", 2.0),
            ("dynamic_release_ms", 100.0),
        ],
        [
            ("drive", 1.0),
            ("tone", 1.0),
            ("mix", 1.0),
            ("output_gain", 0.0),
            ("dynamic_amount", 0.0),
            ("dynamic_attack_ms", 5.0),
            ("dynamic_release_ms", 50.0),
        ],
    ] {
        wrapper.params = restored_params("Saturation", &overrides);
        let status = assert_no_alloc::assert_no_alloc(|| {
            wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext)
        });
        assert!(!matches!(status, ProcessStatus::Error(_)));
        for (id, value) in overrides {
            assert_eq!(
                wrapper.inner.as_ref().unwrap().get_parameter(&id.into()),
                Some(sotf_host::ParameterValue::Float(value as f32))
            );
        }
    }
}

#[test]
fn oversampled_native_callbacks_fit_small_and_large_negotiated_buffers() {
    for max_frames in [1, 127, 8192] {
        for factor in [1.0, 2.0] {
            let mut wrapper = SaturationWrapper {
                params: restored_params("Saturation", &[("oversampling", factor)]),
                ..Default::default()
            };
            assert!(wrapper.initialize(
                &SaturationWrapper::AUDIO_IO_LAYOUTS[0],
                &BufferConfig {
                    sample_rate: 48_000.0,
                    min_buffer_size: Some(1),
                    max_buffer_size: max_frames as u32,
                    process_mode: ProcessMode::Realtime,
                },
                &mut TestContext,
            ));
            let mut samples = [vec![0.1; max_frames], vec![-0.2; max_frames]];
            let mut buffer = Buffer::default();
            // SAFETY: disjoint channel slices have max_frames samples and outlive processing.
            unsafe {
                buffer.set_slices(max_frames, |slices| {
                    slices.extend(samples.iter_mut().map(Vec::as_mut_slice))
                });
            }
            let mut auxiliary = AuxiliaryBuffers {
                inputs: &mut [],
                outputs: &mut [],
            };
            for _ in 0..(8192_usize.div_ceil(max_frames) + 3) {
                let status = assert_no_alloc::assert_no_alloc(|| {
                    wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext)
                });
                assert!(
                    !matches!(status, ProcessStatus::Error(_)),
                    "frames={max_frames}, factor_index={factor}"
                );
            }
            assert_no_alloc::assert_no_alloc(|| wrapper.reset());
            let status = assert_no_alloc::assert_no_alloc(|| {
                wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext)
            });
            assert!(!matches!(status, ProcessStatus::Error(_)));
        }
    }
}

#[test]
fn limiter_restored_lookahead_and_isp_report_actual_latency() {
    struct LatencyContext(std::cell::Cell<u32>);
    impl InitContext<LimiterWrapper> for LatencyContext {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        fn execute(&self, _: ()) {}
        fn set_latency_samples(&self, value: u32) {
            self.0.set(value);
        }
        fn set_current_voice_capacity(&self, _: u32) {}
    }
    let mut wrapper = LimiterWrapper {
        params: restored_params("Limiter", &[("lookahead", 8.0), ("isp_mode", 1.0)]),
        ..Default::default()
    };
    let mut context = LatencyContext(std::cell::Cell::new(0));
    assert!(wrapper.initialize(
        &LimiterWrapper::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime,
        },
        &mut context
    ));
    let plugin = wrapper.inner.as_ref().unwrap();
    assert_eq!(
        plugin.get_parameter(&"lookahead".into()),
        Some(sotf_host::ParameterValue::Float(8.0))
    );
    assert_eq!(
        plugin.get_parameter(&"isp_mode".into()),
        Some(sotf_host::ParameterValue::Bool(true))
    );
    // 8 ms at 48 kHz plus the limiter's 18-frame output true-peak correction.
    assert_eq!(context.0.get(), 384 + 18);
    assert_eq!(plugin.latency_samples(), context.0.get() as usize);
}

#[test]
fn limiter_rejects_incompatible_saved_isp_controls_during_activation() {
    for (id, value) in [("mix", 0.5), ("soft", 1.0)] {
        let mut wrapper = LimiterWrapper {
            params: restored_params("Limiter", &[("isp_mode", 1.0), (id, value)]),
            ..Default::default()
        };
        assert!(
            !wrapper.initialize(
                &LimiterWrapper::AUDIO_IO_LAYOUTS[0],
                &BufferConfig {
                    sample_rate: 48_000.0,
                    min_buffer_size: Some(1),
                    max_buffer_size: 257,
                    process_mode: ProcessMode::Realtime,
                },
                &mut TestContext,
            ),
            "ISP with {id}={value} must fail activation"
        );
        assert!(wrapper.inner.is_none());
    }
}

#[test]
fn aae_and_crossfeed_restore_canonical_choices_from_saved_ids() {
    let mut aae = AaeWrapper {
        params: restored_params("AAE", &[("room_preset", 3.0)]),
        ..Default::default()
    };
    initialize(&mut aae);
    assert_eq!(
        aae.inner
            .as_ref()
            .unwrap()
            .get_parameter(&"room_preset".into()),
        Some(sotf_host::ParameterValue::String("cathedral".to_string()))
    );

    let mut crossfeed = CrossfeedWrapper {
        params: restored_params(
            "Crossfeed",
            &[("crossfeed_mode", 1.0), ("crossfeed_preset", 2.0)],
        ),
        ..Default::default()
    };
    initialize(&mut crossfeed);
    let plugin = crossfeed.inner.as_ref().unwrap();
    assert_eq!(
        plugin.get_parameter(&"mode".into()),
        Some(sotf_host::ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&"preset".into()),
        Some(sotf_host::ParameterValue::Int(2))
    );
}

#[test]
fn structural_restore_while_active_requires_reactivation_without_realtime_allocation() {
    let mut wrapper = LimiterWrapper::default();
    initialize(&mut wrapper);
    let original_latency = wrapper.inner.as_ref().unwrap().latency_samples();
    wrapper.params = restored_params("Limiter", &[("lookahead", 8.0)]);
    let mut samples = [vec![0.25; 17], vec![0.5; 17]];
    let mut buffer = Buffer::default();
    // SAFETY: both disjoint channel slices have 17 samples and outlive processing.
    unsafe {
        buffer.set_slices(17, |slices| {
            slices.extend(samples.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let mut auxiliary = AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    let status = assert_no_alloc::assert_no_alloc(|| {
        wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext)
    });
    assert!(matches!(
        status,
        ProcessStatus::Error("Structural parameter state changed; reactivate plugin")
    ));
    assert!(
        buffer
            .as_slice_immutable()
            .iter()
            .flat_map(|channel| channel.iter())
            .all(|sample| *sample == 0.0)
    );
    assert_eq!(
        wrapper.inner.as_ref().unwrap().latency_samples(),
        original_latency
    );
    initialize(&mut wrapper);
    assert_eq!(wrapper.inner.as_ref().unwrap().latency_samples(), 384);
    let status = assert_no_alloc::assert_no_alloc(|| {
        wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext)
    });
    assert!(!matches!(status, ProcessStatus::Error(_)));
}

#[test]
fn eq_exposes_neutral_bands_and_automates_without_allocations() {
    use plugins_bridge::param_bridge::ParamBridge;
    use sotf_host::parameters::ParameterValue;
    let wrapper = EqWrapper::default();
    let map = wrapper.params.param_map();
    for band in 0..NIH_EQ_BANDS {
        for field in ["freq", "q", "gain", "filter_type", "order"] {
            assert!(
                map.iter()
                    .any(|(id, _, _)| id == &format!("band_{band}_{field}"))
            );
        }
    }
    let mut plugin =
        plugins_bridge::create_plugin("EQ", 2, 48000, &default_plugin_config("EQ")).unwrap();
    plugin.initialize(48000).unwrap();
    let bridge = ParamBridge::new(get_param_specs("EQ"));
    let mut infos: Vec<_> = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect();
    infos.extend(
        plugin
            .parameters()
            .iter()
            .filter(|param| param.id.as_str().starts_with("band_"))
            .filter_map(bridged_info_from_parameter),
    );
    let neutral = crate::params::DynamicParams::from_infos(&infos);
    infos
        .iter_mut()
        .find(|info| info.id == "band_0_gain")
        .unwrap()
        .default_value = 6.0;
    let boosted = crate::params::DynamicParams::from_infos(&infos);
    let input: Vec<_> = (0..512)
        .flat_map(|frame| {
            let value = (std::f32::consts::TAU * frame as f32 * 1000.0 / 48000.0).sin() * 0.1;
            [value, value * 0.5]
        })
        .collect();
    let mut output = vec![0.0; input.len()];
    let context = sotf_host::plugin::ProcessContext::new(48000, 512);
    plugin.process(&input, &mut output, &context).unwrap();
    assert!(
        input
            .iter()
            .zip(&output)
            .all(|(input, output)| (input - output).abs() < 1e-6)
    );
    assert_no_alloc::assert_no_alloc(|| {
        for params in [&boosted, &neutral, &boosted] {
            params.sync_to_plugin(plugin.as_mut()).unwrap();
            plugin.process(&input, &mut output, &context).unwrap();
        }
        plugin.reset();
    });
    assert_eq!(
        plugin.get_parameter(&"band_0_gain".into()),
        Some(ParameterValue::Float(6.0))
    );
    let energy = |samples: &[f32]| {
        samples[512..]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
    };
    assert!(energy(&output) > 3.8 * energy(&input));
}

routing_test!(gate_routing, GateModeWrapper, "Gate");

#[path = "wrapper/gate_sidechain_tests.rs"]
mod gate_sidechain_tests;

#[path = "wrapper/gate_sidechain_clap_tests.rs"]
mod gate_sidechain_clap_tests;

#[test]
fn gate_modes_restore_through_native_activation_with_independent_gain_caps() {
    use sotf_host::ParameterValue;
    for (mode, signed_cap) in [(0, 0.0_f64), (1, 6.0), (2, -6.0)] {
        let mut wrapper = GateModeWrapper {
            params: restored_params(
                "Gate",
                &[
                    ("mode", mode as f64),
                    ("max_boost_db", 6.0),
                    ("range_db", 6.0),
                    ("threshold", -30.0),
                    ("ratio", 3.0),
                    ("attack", 0.1),
                    ("hold", 0.0),
                    ("release", 10.0),
                ],
            ),
            ..Default::default()
        };
        initialize(&mut wrapper);
        assert_eq!(
            wrapper
                .inner
                .as_ref()
                .unwrap()
                .get_parameter(&"mode".into()),
            Some(ParameterValue::Int(mode))
        );
        let mut samples = [vec![0.1; 257], vec![-0.1; 257]];
        let mut buffer = Buffer::default();
        // SAFETY: Both disjoint channel slices have 257 samples and outlive processing.
        unsafe {
            buffer.set_slices(257, |slices| {
                slices.extend(samples.iter_mut().map(Vec::as_mut_slice))
            });
        }
        let mut auxiliary = AuxiliaryBuffers {
            inputs: &mut [],
            outputs: &mut [],
        };
        for _ in 0..200 {
            for (channel, samples) in buffer.as_slice().iter_mut().enumerate() {
                samples.fill(if channel == 0 { 0.1 } else { -0.1 });
            }
            let status = assert_no_alloc::assert_no_alloc(|| {
                wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext)
            });
            assert!(
                !matches!(status, ProcessStatus::Error(_)),
                "mode {mode}: {status:?}"
            );
        }
        // Input is -20 dB, 10 dB above threshold. At 3:1 the 20 dB
        // above-threshold effect is capped at +/-6 dB; downward remains open.
        // Allow the DSP scalar fast-power contract of 0.02 dB.
        let expected = 0.1 * 10.0_f64.powf(signed_cap / 20.0);
        for (channel, samples) in buffer.as_slice_immutable().iter().enumerate() {
            let sign = if channel == 0 { 1.0 } else { -1.0 };
            assert!(
                (20.0 * (f64::from(samples[256]) / (sign * expected)).log10()).abs() < 0.02,
                "mode {mode}"
            );
        }
        assert_eq!(
            wrapper
                .inner
                .as_ref()
                .unwrap()
                .get_parameter(&"max_boost_db".into()),
            Some(ParameterValue::Float(6.0))
        );
    }
}

#[test]
fn gate_stereo_only_layout_explicitly_rejects_external_key_state() {
    let layout = GateModeWrapper::AUDIO_IO_LAYOUTS[0];
    assert_eq!(layout.main_input_channels.unwrap().get(), 2);
    assert_eq!(layout.main_output_channels.unwrap().get(), 2);
    assert!(layout.aux_input_ports.is_empty());
    let mut wrapper = GateModeWrapper {
        params: restored_params("Gate", &[("mode", 2.0), ("sidechain_external", 1.0)]),
        ..Default::default()
    };
    assert!(!wrapper.initialize(
        &layout,
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext
    ));
    assert!(wrapper.inner.is_none());
}
