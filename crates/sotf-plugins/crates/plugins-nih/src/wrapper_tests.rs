//! Process the generated wrappers through NIH buffers, including auxiliary buses.
use super::*;
use crate::params::ambisonics_custom::ambisonics_custom_state_field;
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

fn initialize<P: Plugin + ClapPlugin>(plugin: &mut P) {
    initialize_on_layout(plugin, &P::clap_audio_io_layouts()[0], PluginApi::Clap);
}

struct Vst3TestContext;

impl<P: Plugin> InitContext<P> for Vst3TestContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Vst3
    }
    fn execute(&self, _: P::BackgroundTask) {}
    fn set_latency_samples(&self, _: u32) {}
    fn set_current_voice_capacity(&self, _: u32) {}
}

struct StandaloneTestContext;

impl<P: Plugin> InitContext<P> for StandaloneTestContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }
    fn execute(&self, _: P::BackgroundTask) {}
    fn set_latency_samples(&self, _: u32) {}
    fn set_current_voice_capacity(&self, _: u32) {}
}

fn initialize_on_layout<P: Plugin>(plugin: &mut P, layout: &AudioIOLayout, api: PluginApi) {
    let config = BufferConfig {
        sample_rate: 48000.0,
        min_buffer_size: Some(1),
        max_buffer_size: 257,
        process_mode: ProcessMode::Realtime,
    };
    let initialized = if api == PluginApi::Vst3 {
        plugin.initialize(layout, &config, &mut Vst3TestContext)
    } else if api == PluginApi::Standalone {
        plugin.initialize(layout, &config, &mut StandaloneTestContext)
    } else {
        plugin.initialize(layout, &config, &mut TestContext)
    };
    assert!(
        initialized,
        "failed to initialize layout {layout:?} for {api:?}"
    );
}

fn check_routing<P: Plugin + ClapPlugin + TestProcess>(
    plugin: &mut P,
    name: &str,
    params: &crate::params::DynamicParams,
) {
    let (inputs, outputs) = plugin_io_channels(name);
    let layout = P::clap_audio_io_layouts()[0];
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
routing_test!(de_esser_routing, DeEsserWrapper, "DeEsser");

#[path = "wrapper/de_esser_sidechain_tests.rs"]
mod de_esser_sidechain_tests;

#[test]
fn ambisonics_default_order_one_waveform_matches_pre_edit_capture() {
    let bytes = ambisonics_default_waveform_bytes();
    let digest = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    assert_eq!(bytes.len(), 6_168);
    assert_eq!(digest, 0x74b4_0258_76fc_301b);
}

fn ambisonics_default_waveform_bytes() -> Vec<u8> {
    const FRAMES: usize = 257;
    const INPUTS: usize = 4;
    const OUTPUTS: usize = 6;

    let mut wrapper = AmbisonicsWrapper::default();
    initialize(&mut wrapper);
    let input: Vec<f32> = (0..FRAMES)
        .flat_map(|frame| {
            (0..INPUTS).map(move |channel| {
                let code = (frame * 37 + channel * 101 + frame * channel * 13) % 991;
                (code as f32 - 495.0) / 8192.0
            })
        })
        .collect();
    let mut channels = vec![vec![0.0_f32; FRAMES]; OUTPUTS];
    for (frame, samples) in input.as_chunks::<INPUTS>().0.iter().enumerate() {
        for channel in 0..INPUTS {
            channels[channel][frame] = samples[channel];
        }
    }
    let mut buffer = Buffer::default();
    // SAFETY: every channel owns FRAMES samples for the full callback.
    unsafe {
        buffer.set_slices(FRAMES, |slices| {
            slices.extend(channels.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let mut auxiliary = AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    let status = wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext);
    assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");

    let mut bytes = Vec::with_capacity(FRAMES * OUTPUTS * std::mem::size_of::<f32>());
    for frame in 0..FRAMES {
        for channel in buffer.as_slice_immutable() {
            bytes.extend_from_slice(&channel[frame].to_le_bytes());
        }
    }
    bytes
}

#[test]
#[ignore = "capture the pre-AUD135 default native wrapper waveform before adapter changes"]
fn capture_aud135_pre_edit_ambisonics_native_default_waveform() {
    let bytes = ambisonics_default_waveform_bytes();
    let capture_dir = std::env::var_os("SOTF_AUD135_CAPTURE_DIR")
        .expect("set SOTF_AUD135_CAPTURE_DIR to preserve the pre-edit audio artifact");
    let capture_dir = std::path::PathBuf::from(capture_dir);
    std::fs::create_dir_all(&capture_dir).unwrap();
    let path = capture_dir.join("ambisonics-nih-default-order1-5.1.f32le");
    std::fs::write(&path, &bytes).unwrap();
    eprintln!(
        "AUD135 native default waveform: {} bytes at {}",
        bytes.len(),
        path.display()
    );
}

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

#[test]
fn ambisonics_native_metadata_covers_truthful_layouts_and_role_maps() {
    // Canonical DSP slot order per TARGET_LAYOUTS (pinned by the DSP's own
    // `custom_choice_is_appended_without_moving_named_indices` test): 5.1.4
    // at 3, 7.1.2 at 4. Masks derive from the pinned VST3 SDK speaker bits
    // (kSpeakerSl=9, kSpeakerSr=10); permutations from the SOTF CONFIG
    // channel order (sides before backs); CLAP maps cross-checked against
    // the crossover's by-name constant use. Compatibility record: pre-fix
    // NIH slots 3/4 were swapped (7.1.2 wire at 3, 5.1.4 wire at 4) while
    // DSP/host indices stayed canonical, so old states with target 3/4
    // were self-contradictory; they now resolve to canonical DSP meaning.
    const CANONICAL_TARGETS: [&str; 8] = [
        "5.1", "7.1", "5.1.2", "5.1.4", "7.1.2", "7.1.4", "9.1.4", "9.1.6",
    ];
    const OUTPUT_MASKS: [u64; 8] = [
        0x0000_0000_0000_003f,
        0x0000_0000_0000_063f,
        0x0000_0000_0000_503f,
        0x0000_0000_0002_d03f,
        0x0000_0000_0000_563f,
        0x0000_0000_0002_d63f,
        0x1800_0000_0002_d63f,
        0x1800_0000_0302_d63f,
    ];
    const VST3_PERMUTATIONS: [&[usize]; 8] = [
        &[0, 1, 2, 3, 4, 5],
        &[0, 1, 2, 3, 6, 7, 4, 5],
        &[0, 1, 2, 3, 4, 5, 6, 7],
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9],
        &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11],
        &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 8, 9],
        &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 14, 15, 8, 9],
    ];
    const CLAP_MAPS: [&[u8]; 6] = [
        &[0, 1, 2, 3, 9, 10],
        &[0, 1, 2, 3, 9, 10, 4, 5],
        &[0, 1, 2, 3, 9, 10, 12, 14],
        &[0, 1, 2, 3, 9, 10, 12, 14, 15, 17],
        &[0, 1, 2, 3, 9, 10, 4, 5, 12, 14],
        &[0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17],
    ];
    let output_widths = [6_u32, 8, 8, 10, 10, 12, 14, 16];
    let vst3_layouts = <AmbisonicsWrapper as Plugin>::AUDIO_IO_LAYOUTS;
    assert_eq!(vst3_layouts.len(), 56);
    assert_eq!(
        <AmbisonicsWrapper as Vst3Plugin>::default_audio_io_layout(),
        vst3_layouts[0],
        "the existing default bus configuration must stay order-1 5.1"
    );

    for order_index in 0_usize..7 {
        let input_channels = (order_index + 2).pow(2) as u32;
        let expected_input_mask = if order_index < 4 {
            (0..input_channels).fold(0_u64, |mask, acn| {
                let bit = if acn < 4 { 20 + acn } else { 34 + acn };
                mask | (1_u64 << bit)
            })
        } else if input_channels == 64 {
            u64::MAX
        } else {
            (1_u64 << input_channels) - 1
        };
        for target in 0..8 {
            let index = order_index * 8 + target;
            let layout = &vst3_layouts[index];
            assert_eq!(layout.main_input_channels.unwrap().get(), input_channels);
            assert_eq!(
                layout.main_output_channels.unwrap().get(),
                output_widths[target]
            );
            let expected_name = format!("Order {} - {}", order_index + 1, CANONICAL_TARGETS[target]);
            assert_eq!(
                layout.names.layout,
                Some(expected_name.as_str()),
                "VST3 slot {target} must carry its canonical DSP name"
            );
            assert!(layout.aux_input_ports.is_empty());
            assert_eq!(
                <AmbisonicsWrapper as Vst3Plugin>::vst3_bus_arrangement(index, true, 0),
                Some(expected_input_mask),
                "VST3 ACN/SN3D mask for order {}",
                order_index + 1
            );
            assert_eq!(
                <AmbisonicsWrapper as Vst3Plugin>::vst3_bus_arrangement(index, false, 0),
                Some(OUTPUT_MASKS[target])
            );
            for (bus_channel, &sotf_channel) in VST3_PERMUTATIONS[target].iter().enumerate() {
                assert_eq!(
                    ambisonics_vst3_output_to_sotf(target, bus_channel),
                    Some(sotf_channel),
                    "VST3 permutation for target {target}"
                );
            }
            assert_eq!(
                ambisonics_vst3_output_to_sotf(target, VST3_PERMUTATIONS[target].len()),
                None
            );
        }
    }

    assert!(std::hint::black_box(
        <AmbisonicsWrapper as ClapPlugin>::CLAP_SUPPORTS_AMBISONIC
    ));
    assert!(std::hint::black_box(
        <AmbisonicsWrapper as ClapPlugin>::CLAP_SUPPORTS_SURROUND
    ));
    assert!(std::hint::black_box(crate::sotf_nih_supports_surround!(
        "Crossover"
    )));
    assert!(!std::hint::black_box(crate::sotf_nih_supports_surround!(
        "BandSplit"
    )));
    let clap_layouts = <AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts();
    assert_eq!(clap_layouts.len(), 42);
    for order_index in 0_usize..7 {
        let input_channels = (order_index + 2).pow(2) as u32;
        for target in 0..6 {
            let index = order_index * 6 + target;
            let layout = &clap_layouts[index];
            assert_eq!(layout.main_input_channels.unwrap().get(), input_channels);
            assert_eq!(
                layout.main_output_channels.unwrap().get(),
                output_widths[target]
            );
            let expected_name = format!("Order {} - {}", order_index + 1, CANONICAL_TARGETS[target]);
            assert_eq!(
                layout.names.layout,
                Some(expected_name.as_str()),
                "CLAP slot {target} must carry its canonical DSP name"
            );
            assert!(
                <AmbisonicsWrapper as ClapPlugin>::clap_audio_port_type(index, true, 0)
                    .is_some_and(|port_type| port_type.to_bytes() == b"ambisonic")
            );
            assert!(
                <AmbisonicsWrapper as ClapPlugin>::clap_audio_port_type(index, false, 0)
                    .is_some_and(|port_type| port_type.to_bytes() == b"surround")
            );
            assert!(
                <AmbisonicsWrapper as ClapPlugin>::clap_audio_port_type(index, true, 1).is_none()
            );
            let config = <AmbisonicsWrapper as ClapPlugin>::clap_ambisonic_config(true, 0)
                .expect("main Ambisonics input has standardized metadata");
            assert_eq!(
                config.ordering,
                clap_sys::ext::ambisonic::CLAP_AMBISONIC_ORDERING_ACN
            );
            assert_eq!(
                config.normalization,
                clap_sys::ext::ambisonic::CLAP_AMBISONIC_NORMALIZATION_SN3D
            );
            assert!(<AmbisonicsWrapper as ClapPlugin>::clap_ambisonic_config(false, 0).is_none());
            let channel_map =
                <AmbisonicsWrapper as ClapPlugin>::clap_surround_channel_map(index, false, 0)
                    .expect("supported surround target has a truthful role map");
            assert_eq!(channel_map, CLAP_MAPS[target]);
            assert!(
                <AmbisonicsWrapper as ClapPlugin>::clap_surround_channel_map(index, true, 0)
                    .is_none()
            );
            let expected_mask = channel_map
                .iter()
                .fold(0_u64, |mask, role| mask | (1_u64 << role));
            assert!(
                <AmbisonicsWrapper as ClapPlugin>::clap_surround_channel_mask_supported(
                    expected_mask
                )
            );
        }
        assert!(
            ambisonics_layout_index(&clap_layouts[order_index * 6 + 5], PluginApi::Clap).is_some()
        );
        assert!(
            ambisonics_layout_index(&vst3_layouts[order_index * 8 + 7], PluginApi::Clap).is_none()
        );
    }
}

#[test]
fn ambisonics_vst3_and_clap_process_selected_full_input_vectors() {
    const FRAMES: usize = 31;
    // Canonical DSP slot names per TARGET_LAYOUTS; the reference DSP is
    // built from the same recorded target, so the wire-bytes pins live in
    // the metadata test and this loop anchors slot identity via names.
    const CANONICAL_TARGETS: [&str; 8] = [
        "5.1", "7.1", "5.1.2", "5.1.4", "7.1.2", "7.1.4", "9.1.4", "9.1.6",
    ];
    for (api, layouts, layouts_per_order, targets) in [
        (
            PluginApi::Vst3,
            <AmbisonicsWrapper as Plugin>::AUDIO_IO_LAYOUTS,
            8,
            8,
        ),
        (
            PluginApi::Clap,
            <AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts(),
            6,
            6,
        ),
    ] {
        for (layout_index, layout) in layouts.iter().enumerate() {
            let order = layout_index / layouts_per_order + 1;
            let target = layout_index % layouts_per_order;
            let input_channels = (order + 1).pow(2);
            let output_channels = output_width(layout).unwrap();
            let expected_name = format!("Order {order} - {}", CANONICAL_TARGETS[target]);
            assert_eq!(
                layout.names.layout,
                Some(expected_name.as_str()),
                "api={api:?}, layout={layout_index}: slot identity must match DSP canonical order"
            );
            let mut wrapper = AmbisonicsWrapper::default();
            initialize_on_layout(&mut wrapper, layout, api);
            assert_eq!(
                wrapper.inner.as_ref().unwrap().input_channels(),
                input_channels
            );
            assert_eq!(
                wrapper.inner.as_ref().unwrap().output_channels(),
                output_channels
            );

            let mut reference = crate::params::configuration::create_plugin(
                "AmbisonicsDecoder",
                48_000,
                &wrapper.params,
            )
            .unwrap();
            reference = plugins_bridge::prepare_standalone_plugin(reference, FRAMES).unwrap();
            reference.initialize(48_000).unwrap();
            wrapper.params.sync_to_plugin(reference.as_mut()).unwrap();

            let input: Vec<f32> = (0..FRAMES)
                .flat_map(|frame| {
                    (0..input_channels).map(move |channel| {
                        let code = (frame * 43 + channel * 71 + frame * channel * 11) % 997;
                        (code as f32 - 498.0) * 0.000_02
                    })
                })
                .collect();
            let mut expected = vec![0.0; FRAMES * output_channels];
            assert_eq!(
                reference
                    .process(
                        &input,
                        &mut expected,
                        &sotf_host::plugin::ProcessContext::new(48_000, FRAMES),
                    )
                    .unwrap(),
                FRAMES
            );

            let sentinel = -0.8125;
            let mut host_channels =
                vec![vec![sentinel; FRAMES]; input_channels.max(output_channels)];
            for frame in 0..FRAMES {
                for channel in 0..input_channels {
                    host_channels[channel][frame] = input[frame * input_channels + channel];
                }
            }
            let original_channels = host_channels.clone();
            let mut buffer = Buffer::default();
            // SAFETY: the host channel slices are disjoint and remain live through process().
            unsafe {
                buffer.set_slices(FRAMES, |slices| {
                    slices.extend(host_channels.iter_mut().map(Vec::as_mut_slice))
                });
            }
            let mut auxiliary = AuxiliaryBuffers {
                inputs: &mut [],
                outputs: &mut [],
            };
            let status =
                wrapper.process_with_api(&mut buffer, &mut auxiliary, api, Default::default());
            assert!(
                !matches!(status, ProcessStatus::Error(_)),
                "api={api:?}, layout={layout_index}: {status:?}"
            );
            for frame in 0..FRAMES {
                for bus_channel in 0..output_channels {
                    let sotf_channel = if api == PluginApi::Vst3 {
                        ambisonics_vst3_output_to_sotf(target, bus_channel).unwrap()
                    } else {
                        bus_channel
                    };
                    let actual = buffer.as_slice_immutable()[bus_channel][frame];
                    let expected = expected[frame * output_channels + sotf_channel];
                    assert!(
                        (actual - expected).abs() <= 1.0e-6,
                        "api={api:?}, order={order}, target={target}, frame={frame}, bus={bus_channel}: {actual} vs {expected}"
                    );
                }
            }
            for (channel, original) in original_channels.iter().enumerate().skip(output_channels) {
                assert_eq!(
                    buffer.as_slice_immutable()[channel],
                    *original,
                    "input-only channel {channel} was overwritten for api={api:?}, layout={layout_index}"
                );
            }
        }
        assert_eq!(layouts.len(), 7 * targets);
    }
}

fn output_width(layout: &AudioIOLayout) -> Option<usize> {
    layout
        .main_output_channels
        .map(|channels| channels.get() as usize)
}

#[test]
fn bandsplit_clap_layouts_route_all_selected_band_major_channels() {
    const MAX_FRAMES: usize = 127;
    let layouts = <SplitWrapper as ClapPlugin>::clap_audio_io_layouts();
    assert_eq!(layouts.len(), 3);
    assert_eq!(layouts[0], crate::wrapper::BAND_SPLIT_CLAP_LAYOUTS[0]);
    assert_eq!(
        layouts
            .iter()
            .map(|layout| output_width(layout).unwrap())
            .collect::<Vec<_>>(),
        [4, 6, 8]
    );

    for (layout_index, num_bands) in [(0, 2), (1, 3), (2, 4)] {
        let mut wrapper = SplitWrapper {
            params: restored_params(
                "BandSplit",
                &[
                    ("crossover_type", 1.0),
                    ("recombination_mode", 0.0),
                    ("num_bands", 0.0),
                ],
            ),
            ..Default::default()
        };
        initialize_on_layout(&mut wrapper, &layouts[layout_index], PluginApi::Clap);
        let output_channels = num_bands * 2;
        let plugin = wrapper.inner.as_ref().unwrap();
        assert_eq!(
            (plugin.input_channels(), plugin.output_channels()),
            (2, output_channels)
        );
        assert_eq!(
            plugin.get_parameter(&"num_bands".into()),
            Some(sotf_host::ParameterValue::Int((num_bands - 2) as i32))
        );
        assert_eq!(
            plugin.get_parameter(&"type".into()),
            Some(sotf_host::ParameterValue::Int(1))
        );
        assert_eq!(
            plugin.get_parameter(&"recombination_mode".into()),
            Some(sotf_host::ParameterValue::Int(0))
        );

        let mut direct =
            crate::params::configuration::create_plugin("BandSplit", 48_000, &wrapper.params)
                .unwrap();
        direct = plugins_bridge::prepare_standalone_plugin(direct, MAX_FRAMES).unwrap();
        direct.initialize(48_000).unwrap();
        wrapper.params.sync_to_plugin(direct.as_mut()).unwrap();

        let mut frame_position = 0;
        for frames in [31, 53] {
            let input = (0..frames)
                .flat_map(|frame| {
                    let absolute = frame_position + frame;
                    [
                        ((absolute * 29 % 101) as f32 - 50.0) * 0.003,
                        ((absolute * 47 % 113) as f32 - 56.0) * 0.002,
                    ]
                })
                .collect::<Vec<_>>();
            let mut expected = vec![0.0; frames * output_channels];
            assert_eq!(
                direct
                    .process(
                        &input,
                        &mut expected,
                        &sotf_host::plugin::ProcessContext::new(48_000, frames),
                    )
                    .unwrap(),
                frames
            );

            let mut channels = vec![vec![0.0; frames]; output_channels];
            for frame in 0..frames {
                channels[0][frame] = input[frame * 2];
                channels[1][frame] = input[frame * 2 + 1];
            }
            let mut buffer = Buffer::default();
            // SAFETY: each output bus channel owns a disjoint slice for the callback.
            unsafe {
                buffer.set_slices(frames, |slices| {
                    slices.extend(channels.iter_mut().map(Vec::as_mut_slice))
                });
            }
            let mut auxiliary = AuxiliaryBuffers {
                inputs: &mut [],
                outputs: &mut [],
            };
            let status =
                wrapper.process_without_transport(&mut buffer, &mut auxiliary, &mut TestContext);
            assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
            for channel in 0..output_channels {
                for frame in 0..frames {
                    let actual = buffer.as_slice_immutable()[channel][frame];
                    let reference = expected[frame * output_channels + channel];
                    assert!(
                        (actual - reference).abs() < 1.0e-6,
                        "bands={num_bands}, frame={}, channel={channel}: {actual} vs {reference}",
                        frame_position + frame
                    );
                }
            }
            frame_position += frames;
        }
    }
}

#[test]
fn bandsplit_state_migration_preserves_legacy_mode_and_rejects_layout_conflict() {
    use nih_plug::wrapper::state::{ParamValue, PluginState};
    use std::collections::BTreeMap;

    let mut old_state = PluginState {
        version: String::new(),
        params: BTreeMap::from([
            ("frequency".to_string(), ParamValue::F32(730.0)),
            ("crossover_type".to_string(), ParamValue::I32(1)),
        ]),
        fields: BTreeMap::new(),
    };
    <SplitWrapper as Plugin>::filter_state(&mut old_state);
    assert!(matches!(
        old_state.params.get("recombination_mode"),
        Some(ParamValue::I32(value)) if *value == 0
    ));
    assert!(matches!(
        old_state.params.get("num_bands"),
        Some(ParamValue::I32(0))
    ));
    assert!(
        old_state
            .fields
            .contains_key(crate::params::BAND_SPLIT_LAYOUT_RESTORE_MARKER)
    );

    let mut migrated_old = SplitWrapper {
        params: restored_params(
            "BandSplit",
            &[
                ("frequency", 730.0),
                ("crossover_type", 1.0),
                ("recombination_mode", 0.0),
                ("num_bands", 0.0),
            ],
        ),
        ..Default::default()
    };
    <crate::params::DynamicParams as Params>::deserialize_fields(
        &migrated_old.params,
        &old_state.fields,
    );
    let old_layout_rejected = <SplitWrapper as Plugin>::initialize(
        &mut migrated_old,
        &BAND_SPLIT_CLAP_LAYOUTS[1],
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 127,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext,
    );
    assert!(!old_layout_rejected);
    assert!(migrated_old.inner.is_none());
    assert!(migrated_old.params.band_split_layout_restore_pending());
    assert_eq!(
        migrated_old.params.value("num_bands"),
        Some(sotf_host::ParameterValue::Int(0))
    );
    initialize_on_layout(
        &mut migrated_old,
        &BAND_SPLIT_CLAP_LAYOUTS[0],
        PluginApi::Clap,
    );
    assert!(!migrated_old.params.band_split_layout_restore_pending());
    let plugin = migrated_old.inner.as_ref().unwrap();
    assert_eq!(plugin.output_channels(), 4);
    assert_eq!(
        plugin.get_parameter(&"recombination_mode".into()),
        Some(sotf_host::ParameterValue::Int(0))
    );
    assert_eq!(
        plugin.get_parameter(&"type".into()),
        Some(sotf_host::ParameterValue::Int(1))
    );

    let mut imported = PluginState {
        version: String::new(),
        params: BTreeMap::from([
            ("frequency".to_string(), ParamValue::F32(730.0)),
            ("crossover_type".to_string(), ParamValue::I32(1)),
            ("recombination_mode".to_string(), ParamValue::I32(1)),
            ("num_bands".to_string(), ParamValue::I32(2)),
        ]),
        fields: BTreeMap::new(),
    };
    <SplitWrapper as Plugin>::filter_state(&mut imported);
    assert_eq!(
        imported
            .fields
            .get(crate::params::BAND_SPLIT_LAYOUT_RESTORE_MARKER)
            .map(String::as_str),
        Some("1")
    );

    let imported_params = restored_params(
        "BandSplit",
        &[
            ("frequency", 730.0),
            ("crossover_type", 1.0),
            ("recombination_mode", 1.0),
            ("num_bands", 2.0),
        ],
    );
    <crate::params::DynamicParams as Params>::deserialize_fields(
        &imported_params,
        &imported.fields,
    );
    let mut restored = SplitWrapper {
        params: imported_params,
        ..Default::default()
    };
    let rejected = <SplitWrapper as Plugin>::initialize(
        &mut restored,
        &BAND_SPLIT_CLAP_LAYOUTS[0],
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 127,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext,
    );
    assert!(!rejected);
    assert!(restored.inner.is_none());
    assert!(restored.params.band_split_layout_restore_pending());
    assert_eq!(
        restored.params.value("num_bands"),
        Some(sotf_host::ParameterValue::Int(2))
    );

    initialize_on_layout(&mut restored, &BAND_SPLIT_CLAP_LAYOUTS[2], PluginApi::Clap);
    assert!(!restored.params.band_split_layout_restore_pending());
    let plugin = restored.inner.as_ref().unwrap();
    assert_eq!(plugin.output_channels(), 8);
    assert_eq!(
        plugin.get_parameter(&"recombination_mode".into()),
        Some(sotf_host::ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&"type".into()),
        Some(sotf_host::ParameterValue::Int(1))
    );
}

#[path = "wrapper/native_ambisonics_callbacks.rs"]
mod native_ambisonics_callbacks;

#[path = "wrapper/native_bandsplit_vst3_callbacks.rs"]
mod native_bandsplit_vst3_callbacks;

#[cfg(feature = "convolution")]
#[path = "wrapper/native_convolution_state_callbacks.rs"]
mod native_convolution_state_callbacks;

fn ambisonics_custom_test_json(name: &str, speakers: &[(&str, f32, f32, bool)]) -> String {
    let entries = speakers
        .iter()
        .map(|(label, azimuth, elevation, is_lfe)| {
            serde_json::json!({
                "label": label,
                "azimuth_deg": azimuth,
                "elevation_deg": elevation,
                "is_lfe": is_lfe,
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"name\":\"{name}\",\"speakers\":[{entries}]}}")
}

fn ambisonics_custom_7_1_4_json() -> String {
    ambisonics_custom_test_json(
        "custom-7.1.4",
        &[
            ("FL", 30.0, 0.0, false),
            ("FR", -30.0, 0.0, false),
            ("FC", 0.0, 0.0, false),
            ("LFE", 0.0, 0.0, true),
            ("SL", 90.0, 0.0, false),
            ("SR", -90.0, 0.0, false),
            ("BL", 150.0, 0.0, false),
            ("BR", -150.0, 0.0, false),
            ("TFL", 30.0, 45.0, false),
            ("TFR", -30.0, 45.0, false),
            ("TBL", 150.0, 45.0, false),
            ("TBR", -150.0, 45.0, false),
        ],
    )
}

fn ambisonics_custom_9_1_6_json() -> String {
    ambisonics_custom_test_json(
        "custom-9.1.6",
        &[
            ("FL", 30.0, 0.0, false),
            ("FR", -30.0, 0.0, false),
            ("FC", 0.0, 0.0, false),
            ("LFE", 0.0, 0.0, true),
            ("SL", 90.0, 0.0, false),
            ("SR", -90.0, 0.0, false),
            ("BL", 150.0, 0.0, false),
            ("BR", -150.0, 0.0, false),
            ("WL", 60.0, 0.0, false),
            ("WR", -60.0, 0.0, false),
            ("TFL", 30.0, 45.0, false),
            ("TFR", -30.0, 45.0, false),
            ("TBL", 150.0, 45.0, false),
            ("TBR", -150.0, 45.0, false),
            ("TMiL", 90.0, 45.0, false),
            ("TMiR", -90.0, 45.0, false),
        ],
    )
}

fn ambisonics_custom_7_1_2_json() -> String {
    ambisonics_custom_test_json(
        "custom-7.1.2",
        &[
            ("FL", 30.0, 0.0, false),
            ("FR", -30.0, 0.0, false),
            ("FC", 0.0, 0.0, false),
            ("LFE", 0.0, 0.0, true),
            ("SL", 90.0, 0.0, false),
            ("SR", -90.0, 0.0, false),
            ("BL", 150.0, 0.0, false),
            ("BR", -150.0, 0.0, false),
            ("TFL", 30.0, 45.0, false),
            ("TFR", -30.0, 45.0, false),
        ],
    )
}

fn stage_custom_on_wrapper(wrapper: &mut AmbisonicsWrapper, order: usize, field: &str) {
    wrapper.params.set_ambisonics_layout(order, 8).unwrap();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(ambisonics_custom_state_field().to_string(), field.to_string());
    wrapper.params.deserialize_fields(&fields);
}

fn check_custom_wrapper_process(
    wrapper: &mut AmbisonicsWrapper,
    api: PluginApi,
    inputs: usize,
    outputs: usize,
    bus_to_sotf: &[usize],
) {
    const FRAMES: usize = 31;
    let input: Vec<f32> = (0..FRAMES)
        .flat_map(|frame| {
            (0..inputs).map(move |channel| {
                let code = (frame * 43 + channel * 71 + frame * channel * 11) % 997;
                (code as f32 - 498.0) * 0.000_02
            })
        })
        .collect();
    let mut reference = crate::params::configuration::create_plugin(
        "AmbisonicsDecoder",
        48_000,
        &wrapper.params,
    )
    .unwrap();
    reference = plugins_bridge::prepare_standalone_plugin(reference, FRAMES).unwrap();
    reference.initialize(48_000).unwrap();
    let mut expected = vec![0.0; FRAMES * outputs];
    assert_eq!(
        reference
            .process(
                &input,
                &mut expected,
                &sotf_host::plugin::ProcessContext::new(48_000, FRAMES),
            )
            .unwrap(),
        FRAMES
    );
    let mut host_channels = vec![vec![0.0; FRAMES]; inputs.max(outputs)];
    for frame in 0..FRAMES {
        for channel in 0..inputs {
            host_channels[channel][frame] = input[frame * inputs + channel];
        }
    }
    let mut buffer = Buffer::default();
    // SAFETY: the host channel slices are disjoint and remain live through process().
    unsafe {
        buffer.set_slices(FRAMES, |slices| {
            slices.extend(host_channels.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let mut auxiliary = AuxiliaryBuffers {
        inputs: &mut [],
        outputs: &mut [],
    };
    let status = wrapper.process_with_api(&mut buffer, &mut auxiliary, api, Default::default());
    assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
    let mut nonzero = false;
    for frame in 0..FRAMES {
        for bus_channel in 0..outputs {
            let actual = buffer.as_slice_immutable()[bus_channel][frame];
            assert!(actual.is_finite(), "frame {frame} bus {bus_channel}: {actual}");
            nonzero |= actual.abs() > 1.0e-6;
            let reference = expected[frame * outputs + bus_to_sotf[bus_channel]];
            assert!(
                (actual - reference).abs() <= 1.0e-6,
                "frame={frame}, bus={bus_channel}: {actual} vs {reference}"
            );
        }
    }
    assert!(nonzero, "custom wrapper render is silent");
}

#[test]
fn ambisonics_custom_clap_init_processes_matched_layout() {
    let mut wrapper = AmbisonicsWrapper::default();
    stage_custom_on_wrapper(&mut wrapper, 7, &ambisonics_custom_7_1_4_json());
    let layouts = <AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts();
    initialize_on_layout(&mut wrapper, &layouts[41], PluginApi::Clap);
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 64);
    assert_eq!(wrapper.inner.as_ref().unwrap().output_channels(), 12);
    assert_eq!(wrapper.ambisonics_target_layout, 8);
    assert_eq!(wrapper.ambisonics_custom_vst3_channels, 12);
    let identity: Vec<usize> = (0..12).collect();
    check_custom_wrapper_process(&mut wrapper, PluginApi::Clap, 64, 12, &identity);
}

#[test]
fn ambisonics_custom_vst3_init_installs_host_agreed_permutation() {
    let json = ambisonics_custom_9_1_6_json();
    let mut wrapper = AmbisonicsWrapper::default();
    stage_custom_on_wrapper(&mut wrapper, 7, &json);
    let layouts = <AmbisonicsWrapper as Plugin>::AUDIO_IO_LAYOUTS;
    initialize_on_layout(&mut wrapper, &layouts[55], PluginApi::Vst3);
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 64);
    assert_eq!(wrapper.inner.as_ref().unwrap().output_channels(), 16);
    let geometry: sotf_host::external_plugin::NativeAmbisonicsCustomGeometry =
        serde_json::from_str(&json).unwrap();
    let (mask, expected) = geometry.vst3_arrangement(7).unwrap();
    assert_eq!(mask, 0x1800_0000_0302_d63f);
    assert_eq!(
        &wrapper.ambisonics_custom_vst3_to_sotf[..16],
        expected.as_slice()
    );
    check_custom_wrapper_process(&mut wrapper, PluginApi::Vst3, 64, 16, &expected);
}

#[test]
fn ambisonics_custom_init_rejects_mismatched_layouts() {
    let config = BufferConfig {
        sample_rate: 48000.0,
        min_buffer_size: Some(1),
        max_buffer_size: 257,
        process_mode: ProcessMode::Realtime,
    };
    let clap_layouts = <AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts();
    // Wrong width: 12-channel geometry against the 6-channel 5.1 layout.
    let mut wrapper = AmbisonicsWrapper::default();
    stage_custom_on_wrapper(&mut wrapper, 7, &ambisonics_custom_7_1_4_json());
    assert!(!wrapper.initialize(&clap_layouts[36], &config, &mut TestContext));
    // Same width, other roles: 7.1.2-shaped geometry against 5.1.4
    // (slot 3 in canonical DSP order; pre-F1-fix this slot was mislabeled 4).
    let mut wrapper = AmbisonicsWrapper::default();
    stage_custom_on_wrapper(&mut wrapper, 7, &ambisonics_custom_7_1_2_json());
    assert!(!wrapper.initialize(&clap_layouts[39], &config, &mut TestContext));
    // VST3 width mismatch: 16-channel geometry against the 5.1 layout.
    let vst3_layouts = <AmbisonicsWrapper as Plugin>::AUDIO_IO_LAYOUTS;
    let mut wrapper = AmbisonicsWrapper::default();
    stage_custom_on_wrapper(&mut wrapper, 7, &ambisonics_custom_9_1_6_json());
    assert!(!wrapper.initialize(&vst3_layouts[48], &config, &mut Vst3TestContext));
}

#[test]
fn ambisonics_custom_init_rejects_missing_geometry() {
    let config = BufferConfig {
        sample_rate: 48000.0,
        min_buffer_size: Some(1),
        max_buffer_size: 257,
        process_mode: ProcessMode::Realtime,
    };
    let clap_layouts = <AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts();
    let mut wrapper = AmbisonicsWrapper::default();
    wrapper.params.set_ambisonics_layout(7, 8).unwrap();
    assert!(!wrapper.initialize(&clap_layouts[41], &config, &mut TestContext));
}

#[test]
fn ambisonics_custom_init_refuses_fieldless_restore_until_valid_stage() {
    let config = BufferConfig {
        sample_rate: 48000.0,
        min_buffer_size: Some(1),
        max_buffer_size: 257,
        process_mode: ProcessMode::Realtime,
    };
    let clap_layouts = <AmbisonicsWrapper as ClapPlugin>::clap_audio_io_layouts();
    // Commit accepted good geometry first through a real initialize.
    let mut wrapper = AmbisonicsWrapper::default();
    stage_custom_on_wrapper(&mut wrapper, 7, &ambisonics_custom_7_1_4_json());
    initialize_on_layout(&mut wrapper, &clap_layouts[41], PluginApi::Clap);
    assert_eq!(wrapper.ambisonics_target_layout, 8);
    // Real fieldless target-8 restore: structural ints as nih-plug
    // applies them, then opaque fields without the geometry key.
    wrapper.params.set_ambisonics_layout(7, 8).unwrap();
    wrapper
        .params
        .deserialize_fields(&std::collections::BTreeMap::new());
    // Actual initialize on the matching layout refuses, repeatedly:
    // retry/drop must not silently heal into committed geometry.
    assert!(
        !wrapper.initialize(&clap_layouts[41], &config, &mut TestContext),
        "fieldless restore must refuse init while missing=true"
    );
    assert!(
        !wrapper.initialize(&clap_layouts[41], &config, &mut TestContext),
        "repeated retry/drop must not silently heal into committed geometry"
    );
    // Valid-stage retry succeeds on the same wrapper with real audio IO.
    stage_custom_on_wrapper(&mut wrapper, 7, &ambisonics_custom_7_1_4_json());
    initialize_on_layout(&mut wrapper, &clap_layouts[41], PluginApi::Clap);
    assert_eq!(
        wrapper.params.value("target_layout"),
        Some(sotf_host::parameters::ParameterValue::Int(8))
    );
    assert_eq!(wrapper.ambisonics_target_layout, 8);
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 64);
    assert_eq!(wrapper.inner.as_ref().unwrap().output_channels(), 12);
}

#[test]
fn ambisonics_standalone_init_resolves_8_target_identity_for_high_slots() {
    // Standalone resolves layouts against the 8-target VST3 table, so the
    // order/target derivation must divide by 8: a divisor of 6
    // misidentifies every layout index >= 6 and fails loudly past 47.
    // This exercises actual `initialize`, not the direct negotiate helper.
    const WIDTHS: [u32; 8] = [6, 8, 8, 10, 10, 12, 14, 16];
    let layouts = <AmbisonicsWrapper as Plugin>::AUDIO_IO_LAYOUTS;
    assert_eq!(layouts.len(), 56);
    for layout_index in [0, 5, 6, 7, 14, 15, 48, 55] {
        let order = layout_index / 8 + 1;
        let target = layout_index % 8;
        let mut wrapper = AmbisonicsWrapper::default();
        initialize_on_layout(&mut wrapper, &layouts[layout_index], PluginApi::Standalone);
        assert_eq!(
            wrapper.params.value("order"),
            Some(sotf_host::parameters::ParameterValue::Int(order as i32)),
            "standalone layout {layout_index} must record order {order}"
        );
        assert_eq!(
            wrapper.params.value("target_layout"),
            Some(sotf_host::parameters::ParameterValue::Int(target as i32)),
            "standalone layout {layout_index} must record target {target}"
        );
        assert_eq!(wrapper.ambisonics_target_layout, target);
        assert_eq!(
            wrapper.inner.as_ref().unwrap().input_channels(),
            (order + 1) * (order + 1)
        );
        assert_eq!(
            wrapper.inner.as_ref().unwrap().output_channels(),
            WIDTHS[target] as usize
        );
    }
}
