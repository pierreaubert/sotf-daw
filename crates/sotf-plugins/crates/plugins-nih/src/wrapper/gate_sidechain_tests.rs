//! Gate's optional native sidechain bus and actual callback routing.

use super::*;

fn key_layout() -> AudioIOLayout {
    let layouts = GateModeWrapper::AUDIO_IO_LAYOUTS;
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

fn activate(mode: i32, linked: bool, external: bool, max_frames: u32) -> GateModeWrapper {
    let mut wrapper = GateModeWrapper {
        params: restored_params(
            "Gate",
            &[
                ("mode", f64::from(mode)),
                ("sidechain_external", f64::from(external)),
                ("link_channels", f64::from(linked)),
                ("max_boost_db", 6.0),
                ("range_db", 6.0),
                ("threshold", -30.0),
                ("ratio", 3.0),
                ("attack", 0.1),
                ("hold", 0.0),
                ("hysteresis_db", 0.0),
                ("release", 10.0),
            ],
        ),
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
    wrapper: &mut GateModeWrapper,
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
fn native_gate_key_bus_drives_independent_signed_gain() {
    for mode in 0..=2 {
        for linked in [false, true] {
            let mut wrapper = activate(mode, linked, true, 8192);
            assert_eq!(wrapper.inner.as_ref().unwrap().input_channels(), 4);
            for frames in [1, 17, 257, 63, 8192, 8192, 8192] {
                let mut program = [vec![0.002; frames], vec![-0.001; frames]];
                let mut keys = [vec![0.1; frames], vec![0.001; frames]];
                let status = callback(&mut wrapper, &mut program, Some(&mut keys));
                assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
                if frames == 8192 {
                    for (channel, samples) in program.iter().enumerate() {
                        let triggered = linked || channel == 0;
                        let gain_db = match (mode, triggered) {
                            (0, false) | (2, true) => -6.0,
                            (1, true) => 6.0,
                            _ => 0.0,
                        };
                        let dry = if channel == 0 { 0.002 } else { -0.001 };
                        let expected = dry * 10.0_f64.powf(gain_db / 20.0);
                        let actual = f64::from(samples[frames - 1]);
                        // Independent plateau law; preserve Gate's existing
                        // 0.02 dB scalar fast-math accuracy contract.
                        assert!(
                            (20.0 * (actual / expected).log10()).abs() < 0.02,
                            "mode={mode} linked={linked} channel={channel}: {actual} vs {expected}"
                        );
                    }
                }
                assert!(keys[0].iter().all(|sample| *sample == 0.1));
                assert!(keys[1].iter().all(|sample| *sample == 0.001));
            }
        }
    }
}

#[test]
fn native_gate_invalid_key_buffers_preserve_detector_history() {
    for case in 0..4 {
        let mut wrapper = activate(2, false, true, 257);
        let mut reference = activate(2, false, true, 257);
        let frames = if case == 3 { 258 } else { 17 };
        let mut rejected = [vec![0.002; frames], vec![-0.001; frames]];
        let mut keys = match case {
            0 => vec![],
            1 => vec![vec![0.9; frames]],
            2 => vec![vec![0.9; frames - 1]; 2],
            _ => vec![vec![0.9; frames]; 2],
        };
        let status = callback(
            &mut wrapper,
            &mut rejected,
            if case == 0 { None } else { Some(&mut keys) },
        );
        assert!(matches!(status, ProcessStatus::Error(_)), "case={case}");
        assert!(rejected.iter().flatten().all(|sample| *sample == 0.0));

        for frames in [1, 17, 257, 63] {
            let mut actual = [vec![0.002; frames], vec![-0.001; frames]];
            let mut expected = actual.clone();
            let mut keys = [vec![0.1; frames], vec![0.001; frames]];
            let status = callback(&mut wrapper, &mut actual, Some(&mut keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            let status = callback(&mut reference, &mut expected, Some(&mut keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            assert_eq!(actual, expected, "case={case} frames={frames}");
        }
    }
}

#[test]
fn native_gate_internal_detection_ignores_connected_key_bus() {
    for mode in 0..=2 {
        let mut optional_bus = activate(mode, true, false, 257);
        let mut original_layout = GateModeWrapper {
            params: optional_bus.params.clone(),
            ..Default::default()
        };
        assert!(original_layout.initialize(
            &GateModeWrapper::AUDIO_IO_LAYOUTS[0],
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
            let mut actual = [vec![0.002; frames], vec![-0.001; frames]];
            let mut expected = actual.clone();
            let mut keys = [vec![0.9; frames], vec![-0.8; frames]];
            let status = callback(&mut optional_bus, &mut actual, Some(&mut keys));
            assert!(!matches!(status, ProcessStatus::Error(_)));
            let status = callback(&mut original_layout, &mut expected, None);
            assert!(!matches!(status, ProcessStatus::Error(_)));
            assert_eq!(actual, expected, "mode={mode} frames={frames}");
        }
    }
}

#[test]
fn native_gate_external_reset_replays_the_same_waveform() {
    for mode in 0..=2 {
        let mut wrapper = activate(mode, false, true, 257);
        let mut first = [vec![0.002; 257], vec![-0.001; 257]];
        let mut keys = [vec![0.1; 257], vec![0.001; 257]];
        let status = callback(&mut wrapper, &mut first, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        assert_no_alloc::assert_no_alloc(|| wrapper.reset());
        let mut replay = [vec![0.002; 257], vec![-0.001; 257]];
        let status = callback(&mut wrapper, &mut replay, Some(&mut keys));
        assert!(!matches!(status, ProcessStatus::Error(_)));
        assert_eq!(first, replay, "mode={mode}");
    }
}
