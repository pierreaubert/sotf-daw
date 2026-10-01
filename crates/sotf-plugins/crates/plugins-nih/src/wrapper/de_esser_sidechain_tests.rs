//! De-esser's optional native sidechain bus and actual callback routing.

use super::*;

fn key_layout() -> AudioIOLayout {
    let layouts = DeEsserWrapper::AUDIO_IO_LAYOUTS;
    assert_eq!(layouts.len(), 2, "stereo and stereo with a key bus");
    assert!(layouts[0].aux_input_ports.is_empty());
    let layout = layouts[1];
    assert_eq!(layout.main_input_channels.unwrap().get(), 2);
    assert_eq!(layout.main_output_channels.unwrap().get(), 2);
    assert_eq!(layout.aux_input_ports.len(), 1);
    assert_eq!(layout.aux_input_ports[0].get(), 2);
    assert!(layout.aux_output_ports.is_empty());
    layout
}

fn de_esser_params_with(
    overrides: &[(&str, f64)],
) -> std::sync::Arc<crate::params::DynamicParams> {
    // Specs provide stable integer Choice metadata for String-typed runtime
    // controls (mode, split_topology). Merge runtime for any missing IDs.
    let bridge = plugins_bridge::param_bridge::ParamBridge::new(
        crate::wrapper::get_param_specs("DeEsser"),
    );
    let mut infos = (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect::<Vec<_>>();
    let plugin = plugins_bridge::create_plugin(
        "DeEsser",
        crate::wrapper::plugin_constructor_channels("DeEsser"),
        48_000,
        &crate::wrapper::default_plugin_config("DeEsser"),
    )
    .expect("create DeEsser to inspect its complete parameter schema");
    for parameter in plugin.parameters() {
        if let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter)
            && !infos.iter().any(|existing| existing.id == info.id)
        {
            infos.push(info);
        }
    }
    for info in &mut infos {
        if let Some((_, value)) = overrides.iter().find(|(id, _)| *id == info.id) {
            info.default_value = *value;
        }
    }
    crate::params::DynamicParams::from_infos_for_plugin("DeEsser", &infos)
}

fn activate(external: bool, max_frames: u32) -> DeEsserWrapper {
    let mut wrapper = DeEsserWrapper {
        params: de_esser_params_with(&[
            ("frequency", 6_500.0),
            ("q", 1.5),
            ("threshold", -20.0),
            ("ratio", 3.0),
            ("attack", 1.0),
            ("release", 60.0),
            ("range_db", 12.0),
            ("mix", 1.0),
            ("lookahead_ms", 2.0),
            ("split_topology", 0.0),
            ("sidechain_external", f64::from(external)),
        ]),
        ..Default::default()
    };
    assert!(wrapper.initialize(
        &key_layout(),
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: max_frames,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext,
    ));
    wrapper
}

fn callback(
    wrapper: &mut DeEsserWrapper,
    program: &mut [Vec<f32>; 2],
    keys: Option<&mut [Vec<f32>]>,
) -> ProcessStatus {
    let frames = program[0].len();
    let mut main = Buffer::default();
    // SAFETY: disjoint program slices have equal lengths and outlive the callback.
    unsafe {
        main.set_slices(frames, |slices| {
            slices.extend(program.iter_mut().map(Vec::as_mut_slice));
        });
    }
    let mut key = Buffer::default();
    let mut buses = Vec::new();
    if let Some(keys) = keys {
        let key_frames = keys.first().map_or(0, Vec::len);
        assert!(keys.iter().all(|channel| channel.len() == key_frames));
        // SAFETY: key slices have equal lengths, are disjoint from main, and live
        // through processing. Invalid bus dimensions are valid NIH Buffer values.
        unsafe {
            key.set_slices(key_frames, |slices| {
                slices.extend(keys.iter_mut().map(Vec::as_mut_slice));
            });
        }
        buses.push(key);
    }
    assert_no_alloc::assert_no_alloc(|| {
        wrapper.process_without_transport(
            &mut main,
            &mut AuxiliaryBuffers {
                inputs: &mut buses,
                outputs: &mut [],
            },
            &mut TestContext,
        )
    })
}

#[test]
fn native_deesser_key_bus_processes_without_error_and_preserves_key() {
    let mut wrapper = activate(true, 8192);
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 4);
    for frames in [1, 17, 257, 63, 8192] {
        let mut program = [vec![0.05; frames], vec![-0.04; frames]];
        let mut keys = [vec![0.2; frames], vec![0.15; frames]];
        let status = callback(&mut wrapper, &mut program, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
        assert!(
            program.iter().flatten().all(|sample| sample.is_finite()),
            "external key output must stay finite"
        );
        assert!(keys[0].iter().all(|sample| *sample == 0.2));
        assert!(keys[1].iter().all(|sample| *sample == 0.15));
    }
}

#[test]
fn native_deesser_invalid_key_buffers_preserve_detector_history() {
    for case in 0..4 {
        let mut wrapper = activate(true, 257);
        let mut reference = activate(true, 257);
        let frames = if case == 3 { 258 } else { 17 };
        let mut rejected = [vec![0.05; frames], vec![-0.04; frames]];
        let mut keys = match case {
            0 => vec![],
            1 => vec![vec![0.2; frames]],
            2 => vec![vec![0.2; frames - 1]; 2],
            _ => vec![vec![0.2; frames]; 2],
        };
        let status = callback(
            &mut wrapper,
            &mut rejected,
            if case == 0 { None } else { Some(&mut keys) },
        );
        assert!(matches!(status, ProcessStatus::Error(_)), "case={case}");
        assert!(rejected.iter().flatten().all(|sample| *sample == 0.0));

        for frames in [1, 17, 257, 63] {
            let mut actual = [vec![0.05; frames], vec![-0.04; frames]];
            let mut expected = actual.clone();
            let mut keys = [vec![0.2; frames], vec![0.15; frames]];
            let status = callback(&mut wrapper, &mut actual, Some(&mut keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            let status = callback(&mut reference, &mut expected, Some(&mut keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            assert_eq!(actual, expected, "case={case} frames={frames}");
        }
    }
}

#[test]
fn native_deesser_internal_detection_ignores_connected_key_bus() {
    let mut optional_bus = activate(false, 257);
    let mut original_layout = DeEsserWrapper {
        params: optional_bus.params.clone(),
        ..Default::default()
    };
    assert!(original_layout.initialize(
        &DeEsserWrapper::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext,
    ));
    assert_eq!(optional_bus.inner.as_ref().unwrap().input_channels(), 2);
    for frames in [1, 17, 257, 63, 257] {
        let mut actual = [vec![0.05; frames], vec![-0.04; frames]];
        let mut expected = actual.clone();
        let mut keys = [vec![0.9; frames], vec![-0.8; frames]];
        let status = callback(&mut optional_bus, &mut actual, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        let status = callback(&mut original_layout, &mut expected, None);
        assert!(!matches!(status, ProcessStatus::Error(_)));
        assert_eq!(actual, expected, "frames={frames}");
    }
}

#[test]
fn native_deesser_external_reset_replays_the_same_waveform() {
    let mut wrapper = activate(true, 257);
    let mut first = [vec![0.05; 257], vec![-0.04; 257]];
    let mut keys = [vec![0.2; 257], vec![0.15; 257]];
    let status = callback(&mut wrapper, &mut first, Some(&mut keys));
    assert!(!matches!(status, ProcessStatus::Error(_)));
    assert_no_alloc::assert_no_alloc(|| wrapper.reset());
    let mut replay = [vec![0.05; 257], vec![-0.04; 257]];
    let status = callback(&mut wrapper, &mut replay, Some(&mut keys));
    assert!(!matches!(status, ProcessStatus::Error(_)));
    assert_eq!(first, replay);
}

#[test]
fn native_deesser_stereo_only_layout_explicitly_rejects_external_key_state() {
    let layout = DeEsserWrapper::AUDIO_IO_LAYOUTS[0];
    assert_eq!(layout.main_input_channels.unwrap().get(), 2);
    assert_eq!(layout.main_output_channels.unwrap().get(), 2);
    assert!(layout.aux_input_ports.is_empty());
    let mut wrapper = DeEsserWrapper {
        params: de_esser_params_with(&[("sidechain_external", 1.0)]),
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
        &mut TestContext,
    ));
    assert!(wrapper.inner.is_none());
}

#[test]
fn native_deesser_reports_lookahead_plus_split_latency() {
    let mut wrapper = DeEsserWrapper {
        params: de_esser_params_with(&[("lookahead_ms", 5.0), ("split_topology", 1.0)]),
        ..Default::default()
    };
    let mut context = LatencyContext(std::cell::Cell::new(0));
    assert!(wrapper.initialize(
        &DeEsserWrapper::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime,
        },
        &mut context,
    ));
    let inner = wrapper.inner.as_ref().unwrap();
    assert_eq!(inner.latency_samples(), context.0.get() as usize);
    assert!(
        inner.latency_samples() >= 240,
        "5 ms at 48 kHz must report at least 240 frames, got {}",
        inner.latency_samples()
    );
}

struct LatencyContext(std::cell::Cell<u32>);

impl<P: Plugin> InitContext<P> for LatencyContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute(&self, _: P::BackgroundTask) {}
    fn set_latency_samples(&self, samples: u32) {
        self.0.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}
