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

fn activate_keyed(external: bool, release_ms: f64, max_frames: u32) -> DeEsserWrapper {
    let mut wrapper = DeEsserWrapper {
        params: de_esser_params_with(&[
            ("mode", 0.0),
            ("threshold", -20.0),
            ("ratio", 8.0),
            ("attack", 0.5),
            ("release", release_ms),
            ("mix", 1.0),
            ("range_db", 60.0),
            ("sidechain_external", f64::from(external)),
        ]),
        ..Default::default()
    };
    let layouts = DeEsserWrapper::AUDIO_IO_LAYOUTS;
    let layout = if external { &layouts[1] } else { &layouts[0] };
    assert!(wrapper.initialize(
        layout,
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

/// Phase-continuous 8 kHz sibilant tone at `peak`, absolute frames.
fn sibilant_tone(start_frame: usize, frames: usize, peak: f32) -> [Vec<f32>; 2] {
    let mut channels = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for frame in 0..frames {
        let t = (start_frame + frame) as f32 / 48_000.0;
        let sample = peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        channels[0].push(sample);
        channels[1].push(sample);
    }
    channels
}

fn tone_rms(channel: &[f32], from_sample: usize) -> f32 {
    let sum: f32 = channel[from_sample..].iter().map(|sample| sample * sample).sum();
    (sum / (channel.len() - from_sample) as f32).sqrt()
}

#[test]
fn native_deesser_hot_key_reduces_sibilant_program() {
    // Hot independent key on the real Stereo + Sidechain layout across
    // irregular partitions: the quiet sibilant program must reduce
    // without muting, every callback must succeed, and the key slices
    // must survive every callback bit-identical.
    let mut wrapper = activate_keyed(true, 20.0, 2048);
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 4);
    let mut rendered = [Vec::new(), Vec::new()];
    let mut position = 0;
    for frames in [1, 17, 257, 63, 1024, 191, 2048, 7]
        .into_iter()
        .cycle()
        .take(16)
    {
        let mut program = sibilant_tone(position, frames, 0.05);
        let mut keys = sibilant_tone(position, frames, 0.5);
        let key_snapshot = keys.clone();
        let status = callback(&mut wrapper, &mut program, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
        assert!(program.iter().flatten().all(|sample| sample.is_finite()));
        assert_eq!(keys, key_snapshot, "key bus must stay immutable");
        rendered[0].extend_from_slice(&program[0]);
        rendered[1].extend_from_slice(&program[1]);
        position += frames;
    }
    let reference = sibilant_tone(0, position, 0.05);
    let program_rms = tone_rms(&reference[0], position / 2);
    let hot_rms = tone_rms(&rendered[0], position / 2);
    let hot_db = 20.0 * (hot_rms / program_rms).log10();
    assert!(
        hot_db < -6.0,
        "hot key must reduce the program past -6 dB, got {hot_db:.2} dB"
    );
    assert!(
        hot_db > -20.0,
        "hot key must not mute the program, got {hot_db:.2} dB"
    );
}

#[test]
fn native_deesser_silent_and_swapped_keys_pass_program() {
    // Silent key passes the quiet program; swapped buses (hot program,
    // quiet key) pass the hot program. Either leg fails if the key
    // leaks into the program path or the detector self-triggers.
    for (program_peak, key_peak, label) in [(0.05, 0.0, "silent"), (0.5, 0.05, "swapped")] {
        let mut wrapper = activate_keyed(true, 20.0, 2048);
        let mut rendered = [Vec::new(), Vec::new()];
        let mut position = 0;
        for frames in [257, 63, 1024, 17] {
            let mut program = sibilant_tone(position, frames, program_peak);
            let mut keys = sibilant_tone(position, frames, key_peak);
            let status = callback(&mut wrapper, &mut program, Some(&mut keys));
            assert!(!matches!(status, ProcessStatus::Error(_)), "{label}: {status:?}");
            rendered[0].extend_from_slice(&program[0]);
            rendered[1].extend_from_slice(&program[1]);
            position += frames;
        }
        let reference = sibilant_tone(0, position, program_peak);
        let program_rms = tone_rms(&reference[0], position / 2);
        let out_rms = tone_rms(&rendered[0], position / 2);
        let ratio_db = 20.0 * (out_rms / program_rms).log10();
        assert!(
            ratio_db.abs() < 1.0,
            "{label} key must pass within 1 dB, got {ratio_db:.2} dB"
        );
    }
}

#[test]
fn native_deesser_internal_detection_with_hot_key_connected_matches_no_bus() {
    // Internal detection ignores even a hot connected key bus: the
    // sibilant program passes and renders bit-identical with and
    // without the auxiliary bus present.
    let mut with_bus = activate_keyed(false, 20.0, 2048);
    let mut no_bus = activate_keyed(false, 20.0, 2048);
    assert_eq!(with_bus.inner.as_ref().unwrap().input_channels(), 2);
    let mut rendered = [Vec::new(), Vec::new()];
    let mut position = 0;
    for frames in [1, 257, 63, 1024, 17] {
        let mut actual = sibilant_tone(position, frames, 0.05);
        let mut expected = actual.clone();
        let mut keys = sibilant_tone(position, frames, 0.5);
        let key_snapshot = keys.clone();
        let status = callback(&mut with_bus, &mut actual, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        let status = callback(&mut no_bus, &mut expected, None);
        assert!(!matches!(status, ProcessStatus::Error(_)));
        assert_eq!(actual, expected, "frames={frames}");
        assert_eq!(keys, key_snapshot, "connected key must stay immutable");
        rendered[0].extend_from_slice(&actual[0]);
        rendered[1].extend_from_slice(&actual[1]);
        position += frames;
    }
    let reference = sibilant_tone(0, position, 0.05);
    let ratio_db =
        20.0 * (tone_rms(&rendered[0], position / 2) / tone_rms(&reference[0], position / 2)).log10();
    assert!(
        ratio_db.abs() < 1.0,
        "internal detection must pass within 1 dB, got {ratio_db:.2} dB"
    );
}

#[test]
fn native_deesser_invalid_key_refusal_retains_engaged_history() {
    // Warm live + twin + cold control with engaged detectors. Invalid
    // key geometries fail the callback with zeroed program output and
    // untouched key slices, but the detector keeps its history: the
    // next valid continuation matches the warm twin bit-exact and
    // diverges from the cold control.
    for case in ["missing bus", "one channel", "short frames"] {
        let mut live = activate_keyed(true, 200.0, 2048);
        let mut twin = activate_keyed(true, 200.0, 2048);
        let mut cold = activate_keyed(true, 200.0, 2048);
        let mut position = 0;
        for frames in [257, 1024, 63] {
            let program = sibilant_tone(position, frames, 0.05);
            let keys = sibilant_tone(position, frames, 0.5);
            let mut live_out = program.clone();
            let mut twin_out = program.clone();
            let mut live_keys = keys.clone();
            let mut twin_keys = keys.clone();
            let status = callback(&mut live, &mut live_out, Some(&mut live_keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            let status = callback(&mut twin, &mut twin_out, Some(&mut twin_keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            assert_eq!(live_out, twin_out, "{case}: populated history must agree");
            position += frames;
        }

        let frames = 257;
        let mut rejected = sibilant_tone(position, frames, 0.05);
        let mut keys = match case {
            "missing bus" => vec![vec![0.2; frames], vec![0.15; frames]],
            "one channel" => vec![vec![0.2; frames]],
            _ => vec![vec![0.2; frames - 1], vec![0.15; frames - 1]],
        };
        let key_snapshot = keys.clone();
        let keyed = case != "missing bus";
        let status = callback(
            &mut live,
            &mut rejected,
            if keyed { Some(&mut keys) } else { None },
        );
        assert!(
            matches!(status, ProcessStatus::Error(_)),
            "{case}: invalid key geometry must fail"
        );
        assert!(rejected.iter().flatten().all(|sample| *sample == 0.0));
        if keyed {
            assert_eq!(keys, key_snapshot, "{case}: rejected key must stay immutable");
        }

        let program = sibilant_tone(position, frames, 0.05);
        let keys = sibilant_tone(position, frames, 0.5);
        let mut continued_live = program.clone();
        let mut continued_twin = program.clone();
        let mut continued_cold = program.clone();
        let mut live_keys = keys.clone();
        let mut twin_keys = keys.clone();
        let mut cold_keys = keys.clone();
        let status = callback(&mut live, &mut continued_live, Some(&mut live_keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        let status = callback(&mut twin, &mut continued_twin, Some(&mut twin_keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        let status = callback(&mut cold, &mut continued_cold, Some(&mut cold_keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        assert_eq!(
            continued_live, continued_twin,
            "{case}: refusal must not disturb engaged history"
        );
        assert!(
            continued_live.iter().flatten().any(|sample| sample.abs() > 1.0e-3),
            "{case}: rejected callback must leave the route processing"
        );
        let max_difference = continued_live
            .iter()
            .flatten()
            .zip(continued_cold.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_difference > 1.0e-4,
            "{case}: live continuation matches a cold wrapper; no retained history"
        );
    }
}

#[test]
fn native_deesser_structural_key_flip_fails_then_restart_recovers() {
    // Flipping the key route on the running internal instance fails
    // the audio path with the structural diagnostic; re-initializing
    // on the Stereo + Sidechain layout (the required restart) adopts
    // the key bus and reduces under a hot key.
    let mut wrapper = activate_keyed(false, 20.0, 2048);
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 2);
    // A DAW flips the control in place and restarts; the vendor setter
    // is private to nih-plug, so substitute an equivalent params
    // object. The wrapper observes parameters only through the
    // structural fingerprint and per-block sync, making this identical
    // to an in-place host edit on the guarded path.
    wrapper.params = de_esser_params_with(&[
        ("mode", 0.0),
        ("threshold", -20.0),
        ("ratio", 8.0),
        ("attack", 0.5),
        ("release", 20.0),
        ("mix", 1.0),
        ("range_db", 60.0),
        ("sidechain_external", 1.0),
    ]);
    let mut program = sibilant_tone(0, 257, 0.05);
    let status = callback(&mut wrapper, &mut program, None);
    assert!(
        matches!(status, ProcessStatus::Error(message) if message.contains("Structural")),
        "key flip must fail with the structural diagnostic, got {status:?}"
    );

    assert!(wrapper.initialize(
        &key_layout(),
        &BufferConfig {
            sample_rate: 48_000.0,
            min_buffer_size: Some(1),
            max_buffer_size: 2048,
            process_mode: ProcessMode::Realtime,
        },
        &mut TestContext,
    ));
    assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 4);
    let mut rendered = [Vec::new(), Vec::new()];
    let mut position = 0;
    for frames in [257, 1024, 2048, 63] {
        let mut program = sibilant_tone(position, frames, 0.05);
        let mut keys = sibilant_tone(position, frames, 0.5);
        let status = callback(&mut wrapper, &mut program, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
        rendered[0].extend_from_slice(&program[0]);
        rendered[1].extend_from_slice(&program[1]);
        position += frames;
    }
    let reference = sibilant_tone(0, position, 0.05);
    let hot_db =
        20.0 * (tone_rms(&rendered[0], position / 2) / tone_rms(&reference[0], position / 2)).log10();
    assert!(
        hot_db < -6.0,
        "restarted key route must reduce past -6 dB, got {hot_db:.2} dB"
    );
    assert!(
        hot_db > -20.0,
        "restarted key route must not mute, got {hot_db:.2} dB"
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
