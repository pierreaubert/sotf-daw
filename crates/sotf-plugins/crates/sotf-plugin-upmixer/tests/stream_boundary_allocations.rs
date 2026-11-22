//! Cold callback and reset heap checks for prefixed analysis windows.

use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: Layout/pointer handling is delegated unchanged to CountingAlloc;
// instrumentation uses only constant-initialized nonallocating TLS cells.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the caller's valid allocation layout unchanged.
        unsafe { sotf_host::CountingAlloc.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the original pointer and layout unchanged.
        unsafe { sotf_host::CountingAlloc.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

#[test]
fn automated_analysis_clocks_and_partial_hop_reset_have_no_heap_activity() {
    for fft in [2, 512, 4096] {
        for hr in [false, true] {
            let params = serde_json::from_value(serde_json::json!({"fft_size":fft,
                "speaker_config":"9.1.6","enable_hr_direct":hr,"auto_gain_enabled":false}))
            .unwrap();
            let mut plugin = UpmixerPlugin::from_params(params);
            plugin.initialize(48_000.0).unwrap();
            plugin
                .set_parameter(
                    "gain_front_direct".into(),
                    sotf_host::ParameterValue::Float(0.2),
                )
                .unwrap();
            for (key, target) in [
                ("lfe_cutoff_hz", 180.0),
                ("bandpass_hz", 200.0),
                ("height_hf_cap_hz", 8000.0),
            ] {
                plugin
                    .set_parameter(key.into(), sotf_host::ParameterValue::Float(target))
                    .unwrap();
            }
            let input = vec![0.02; 257 * 2];
            let mut output = vec![0.0; 257 * plugin.output_channels()];
            let counts = std::thread::spawn(move || {
                ALLOCATIONS.set(0);
                DEALLOCATIONS.set(0);
                TRACKING.set(true);
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 257))
                    .unwrap();
                plugin.reset();
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 257))
                    .unwrap();
                TRACKING.set(false);
                (ALLOCATIONS.get(), DEALLOCATIONS.get())
            })
            .join()
            .unwrap();
            assert_eq!(counts, (0, 0), "fft={fft} hr={hr}");
        }
    }
}

#[test]
fn first_callback_and_reset_have_no_heap_activity() {
    let mut plugin = UpmixerPlugin::from_params(UpmixerPluginParams::default());
    plugin.initialize(48_000.0).unwrap();
    let input = vec![0.1; 4096 * 2];
    let mut output = vec![0.0; 4096 * plugin.output_channels()];
    let (plugin, counts) = std::thread::spawn(move || {
        let context = ProcessContext::new(48_000, 4096);
        TRACKING.set(true);
        plugin.process(&input, &mut output, &context).unwrap();
        while !plugin.drain(&mut output, &context).unwrap().complete {}
        plugin.reset();
        plugin.process(&input, &mut output, &context).unwrap();
        while !plugin.drain(&mut output, &context).unwrap().complete {}
        TRACKING.set(false);
        (plugin, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
    })
    .join()
    .unwrap();
    drop(plugin);
    assert_eq!(counts, (0, 0), "cold/reset allocations/deallocations");
}

#[test]
fn small_fft_callbacks_and_reset_have_no_heap_activity() {
    for fft_size in [2, 4, 8, 16, 32] {
        let params = serde_json::from_value(serde_json::json!({
            "fft_size": fft_size,
            "speaker_config": "7.1.4",
            "enable_hr_direct": true,
            "auto_gain_enabled": true
        }))
        .unwrap();
        let mut plugin = UpmixerPlugin::from_params(params);
        plugin.initialize(48_000.0).unwrap();
        let frames = 512;
        let input = vec![0.1; frames * 2];
        let mut output = vec![0.0; frames * plugin.output_channels()];
        let (plugin, counts) = std::thread::spawn(move || {
            let context = ProcessContext::new(48_000, frames);
            TRACKING.set(true);
            assert_eq!(
                plugin.process(&input, &mut output, &context).unwrap(),
                frames
            );
            plugin.reset();
            assert_eq!(
                plugin.process(&input, &mut output, &context).unwrap(),
                frames
            );
            TRACKING.set(false);
            (plugin, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
        })
        .join()
        .unwrap();
        drop(plugin);
        assert_eq!(
            counts,
            (0, 0),
            "small FFT callback allocations/deallocations at N={fft_size}"
        );
    }
}

#[test]
fn first_drain_on_fresh_callback_thread_has_no_heap_activity() {
    let params = serde_json::from_value(serde_json::json!({
        "enable_subharmonic_synth": true,
        "subharmonic_release_ms": 10.0,
        "auto_gain_enabled": true
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(48_000.0).unwrap();
    plugin
        .process(&[0.2, -0.1], &mut [0.0; 6], &ProcessContext::new(48_000, 1))
        .unwrap();
    let mut output = vec![0.0; plugin.drain_output_frames_max() * plugin.output_channels()];
    let (plugin, counts) = std::thread::spawn(move || {
        TRACKING.set(true);
        loop {
            assert!(plugin.drain_call_bound().is_some());
            let result = plugin
                .drain(&mut output, &ProcessContext::new(48_000, 1))
                .unwrap();
            if result.complete {
                break;
            }
            assert!(result.frames > 0);
        }
        TRACKING.set(false);
        (plugin, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
    })
    .join()
    .unwrap();
    drop(plugin);
    assert_eq!(counts, (0, 0), "cold drain allocations/deallocations");
}

struct StopTracking;
impl Drop for StopTracking {
    fn drop(&mut self) {
        TRACKING.set(false);
    }
}

fn measure_heap<T>(operation: impl FnOnce() -> T) -> (T, (usize, usize)) {
    ALLOCATIONS.set(0);
    DEALLOCATIONS.set(0);
    TRACKING.set(true);
    let guard = StopTracking;
    let result = operation();
    drop(guard);
    (result, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
}

#[test]
fn scheduled_surround_reset_automation_and_drain_do_not_allocate() {
    use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
    use sotf_host::{ParameterId, ParameterValue};
    use sotf_plugin_upmixer::SurroundBassMode;

    const FRAMES: usize = 64;
    for rate in [96_000_u32, 192_000, 384_000] {
        for (input_id, output_id) in [("5.0", "5.1.4"), ("5.1", "7.1.4"), ("7.1", "9.1.6")] {
            for mode in [
                SurroundBassMode::Off,
                SurroundBassMode::Derived,
                SurroundBassMode::Managed,
            ] {
                let layout =
                    ChannelLayout::from_speaker_config(get_speaker_config(input_id).unwrap())
                        .unwrap();
                let input_channels = layout.channels.len();
                let mut params = UpmixerPluginParams::default();
                params.core.speaker_config = output_id.into();
                params.multichannel.input_layout = Some(layout);
                params.multichannel.bass_mode = mode;
                params.multichannel.combine_input_lfe = true;
                let mut reused = UpmixerPlugin::try_from_params(params.clone()).unwrap();
                let mut fresh = UpmixerPlugin::try_from_params(params).unwrap();
                reused.initialize(f64::from(rate)).unwrap();
                fresh.initialize(f64::from(rate)).unwrap();
                assert_eq!(reused.latency_samples(), 2176);
                let input = vec![0.02; FRAMES * input_channels];
                let mut reused_output = vec![0.0; FRAMES * reused.output_channels()];
                let mut fresh_output = vec![0.0; FRAMES * fresh.output_channels()];
                let mut tiny_output = vec![0.0; reused.output_channels()];
                let context = ProcessContext::new(rate, FRAMES);
                let tiny_context = ProcessContext::new(rate, 1);
                let gain: ParameterId = "height_gain".into();
                let cap: ParameterId = "safety_cap_db".into();
                let height_bypass: ParameterId = "bypass_heights".into();
                let surround_bypass: ParameterId = "bypass_surround".into();
                for name in [
                    "height_gain",
                    "safety_cap_db",
                    "bypass_heights",
                    "bypass_surround",
                ] {
                    assert!(
                        reused
                            .parameters()
                            .iter()
                            .any(|parameter| parameter.id.as_str() == name
                                && parameter.update_mode
                                    == sotf_host::param_specs::UpdateMode::Realtime)
                    );
                }
                let (_, counts) = measure_heap(|| {
                    for _ in 0..17 {
                        assert_eq!(
                            reused
                                .process(&input, &mut reused_output, &context)
                                .unwrap(),
                            FRAMES
                        );
                    }
                    assert_eq!(
                        reused
                            .process(&input[..input_channels], &mut tiny_output, &tiny_context)
                            .unwrap(),
                        1
                    );
                    reused.reset();
                    for _ in 0..64 {
                        assert_eq!(
                            reused
                                .process(&input, &mut reused_output, &context)
                                .unwrap(),
                            FRAMES
                        );
                        assert_eq!(
                            fresh.process(&input, &mut fresh_output, &context).unwrap(),
                            FRAMES
                        );
                        assert_eq!(reused_output, fresh_output);
                    }
                    for block in 0..80 {
                        reused
                            .set_parameter(
                                gain.clone(),
                                ParameterValue::Float(if block % 2 == 0 { 0.3 } else { 1.2 }),
                            )
                            .unwrap();
                        reused
                            .set_parameter(
                                cap.clone(),
                                ParameterValue::Float(if block % 2 == 0 { 0.0 } else { 3.0 }),
                            )
                            .unwrap();
                        reused
                            .set_parameter(
                                height_bypass.clone(),
                                ParameterValue::Bool(block % 2 == 0),
                            )
                            .unwrap();
                        reused
                            .set_parameter(
                                surround_bypass.clone(),
                                ParameterValue::Bool(block % 2 != 0),
                            )
                            .unwrap();
                        assert_eq!(
                            reused
                                .process(&input, &mut reused_output, &context)
                                .unwrap(),
                            FRAMES
                        );
                        if block % 17 == 0 {
                            reused.reset();
                        }
                    }
                    let mut calls = 0;
                    while !reused.drain(&mut reused_output, &context).unwrap().complete {
                        calls += 1;
                        // Managed and derived bass allow a two-second IIR tail.
                        assert!(calls < 30_000);
                    }
                });
                assert_eq!(
                    counts,
                    (0, 0),
                    "input={input_id} output={output_id} mode={mode:?} rate={rate}"
                );
            }
        }
    }
}

#[test]
fn every_realtime_surround_control_and_pending_fft_switch_has_no_heap_activity() {
    use sotf_host::ParameterValue;
    use sotf_host::param_specs::UpdateMode;
    use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};

    const FRAMES: usize = 64;
    for rate in [96_000_u32, 192_000, 384_000] {
        let mut params = UpmixerPluginParams::default();
        params.core.speaker_config = "5.1.4".into();
        params.multichannel.input_layout =
            Some(ChannelLayout::from_speaker_config(get_speaker_config("5.0").unwrap()).unwrap());
        let prepared = UpmixerPlugin::try_from_params(params.clone()).unwrap();
        for name in ["auto_gain_enabled", "enable_subharmonic_synth"] {
            assert!(
                prepared
                    .parameters()
                    .iter()
                    .any(|parameter| parameter.id.as_str() == name
                        && parameter.update_mode == UpdateMode::Realtime)
            );
            let mut rejected = UpmixerPlugin::try_from_params(params.clone()).unwrap();
            rejected.initialize(f64::from(rate)).unwrap();
            let id = name.into();
            let before = rejected.get_parameter(&id);
            assert!(
                rejected
                    .set_parameter(id.clone(), ParameterValue::Bool(true))
                    .is_err()
            );
            assert_eq!(rejected.get_parameter(&id), before);
        }
        let controls: Vec<_> = prepared
            .parameters()
            .into_iter()
            .filter(|parameter| parameter.update_mode == UpdateMode::Realtime)
            .filter(|parameter| {
                !matches!(
                    parameter.id.as_str(),
                    "auto_gain_enabled" | "enable_subharmonic_synth"
                )
            })
            .map(|parameter| {
                let changed = match (
                    &parameter.default_value,
                    &parameter.min_value,
                    &parameter.max_value,
                ) {
                    (ParameterValue::Bool(value), _, _) => ParameterValue::Bool(!value),
                    (
                        ParameterValue::Float(_),
                        Some(ParameterValue::Float(lo)),
                        Some(ParameterValue::Float(hi)),
                    ) => ParameterValue::Float(lo + 0.37 * (hi - lo)),
                    _ => panic!("unexpected realtime type: {}", parameter.id.as_str()),
                };
                (parameter.id, changed)
            })
            .collect();
        assert!(!controls.is_empty());
        for (id, changed) in controls {
            for switch in [false, true] {
                let mut plugin = UpmixerPlugin::try_from_params(params.clone()).unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                assert_eq!(plugin.latency_samples(), 2176);
                let input = vec![0.02; FRAMES * 5];
                let mut output = vec![0.0; FRAMES * plugin.output_channels()];
                let mut tiny_output = vec![0.0; plugin.output_channels()];
                let context = ProcessContext::new(rate, FRAMES);
                let tiny_context = ProcessContext::new(rate, 1);
                for _ in 0..17 {
                    assert_eq!(
                        plugin.process(&input, &mut output, &context).unwrap(),
                        FRAMES
                    );
                }
                let (_, counts) = measure_heap(|| {
                    plugin.set_parameter(id.clone(), changed.clone()).unwrap();
                    if switch {
                        assert_eq!(
                            plugin
                                .process(&input[..5], &mut tiny_output, &tiny_context)
                                .unwrap(),
                            1
                        );
                    } else {
                        assert_eq!(
                            plugin.process(&input, &mut output, &context).unwrap(),
                            FRAMES
                        );
                    }
                    assert_eq!(
                        plugin.process(&input, &mut output, &context).unwrap(),
                        FRAMES
                    );
                    plugin.reset();
                    assert_eq!(
                        plugin.process(&input, &mut output, &context).unwrap(),
                        FRAMES
                    );
                });
                assert_eq!(
                    counts,
                    (0, 0),
                    "rate={rate} parameter={} switch={switch}",
                    id.as_str()
                );
            }
        }
    }
}

#[test]
fn surround_reinitialization_keeps_bed_and_height_latency_aligned() {
    use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
    use sotf_plugin_upmixer::SurroundBassMode;

    for (input_id, output_id) in [("5.0", "5.1.4"), ("7.1", "9.1.6")] {
        for mode in [
            SurroundBassMode::Off,
            SurroundBassMode::Derived,
            SurroundBassMode::Managed,
        ] {
            let layout =
                ChannelLayout::from_speaker_config(get_speaker_config(input_id).unwrap()).unwrap();
            let channels = layout.channels.len();
            let mut params = UpmixerPluginParams::default();
            params.core.speaker_config = output_id.into();
            params.multichannel.input_layout = Some(layout);
            params.multichannel.bass_mode = mode;
            params.multichannel.combine_input_lfe = true;
            let mut reused = UpmixerPlugin::try_from_params(params.clone()).unwrap();
            for rate in [48_000_u32, 96_000, 192_000, 384_000, 48_000, 96_000] {
                reused.initialize(f64::from(rate)).unwrap();
                let mut fresh = UpmixerPlugin::try_from_params(params.clone()).unwrap();
                fresh.initialize(f64::from(rate)).unwrap();
                assert_eq!(
                    reused.latency_samples(),
                    if rate >= 96_000 { 2176 } else { 2048 }
                );
                assert_eq!(reused.latency_samples(), fresh.latency_samples());
                let input: Vec<f32> = (0..4096 * channels)
                    .map(|index| 0.08 * ((index as f32) * 0.027).sin())
                    .collect();
                let mut actual = vec![0.0; 4096 * reused.output_channels()];
                let mut expected = vec![0.0; actual.len()];
                let context = ProcessContext::new(rate, 4096);
                reused.process(&input, &mut actual, &context).unwrap();
                fresh.process(&input, &mut expected, &context).unwrap();
                assert!(
                    actual
                        .iter()
                        .zip(expected.iter())
                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                    "input={input_id} output={output_id} mode={mode:?} rate={rate}"
                );
            }
        }
    }
}

#[test]
fn surround_low_latency_requires_a_new_instance_and_reports_its_latency() {
    use sotf_host::ParameterValue;
    use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};

    for (rate, normal, low) in [(48_000.0, 2048, 1024), (96_000.0, 2176, 1152)] {
        let mut params = UpmixerPluginParams::default();
        params.core.speaker_config = "5.1.4".into();
        params.multichannel.input_layout =
            Some(ChannelLayout::from_speaker_config(get_speaker_config("5.0").unwrap()).unwrap());
        let mut plugin = UpmixerPlugin::try_from_params(params.clone()).unwrap();
        plugin.initialize(rate).unwrap();
        assert_eq!(plugin.latency_samples(), normal);

        assert!(
            plugin
                .set_parameter("low_latency".into(), ParameterValue::Bool(true))
                .is_err()
        );
        assert_eq!(plugin.latency_samples(), normal, "rate={rate}");

        params.core.low_latency = true;
        let mut replacement = UpmixerPlugin::try_from_params(params).unwrap();
        replacement.initialize(rate).unwrap();
        assert_eq!(replacement.latency_samples(), low, "rate={rate}");
        assert!(
            replacement
                .set_parameter("low_latency".into(), ParameterValue::Bool(false))
                .is_err()
        );
        assert_eq!(replacement.latency_samples(), low, "rate={rate}");
    }
}

#[test]
fn realtime_scalar_updates_and_first_audio_have_no_heap_activity() {
    use sotf_host::{ParameterValue, param_specs::UpdateMode};

    let mut reports = Vec::new();
    for rate in [48_000, 192_000] {
        for fft in [512, 4096] {
            for layout in ["2.0", "9.1.6"] {
                let params = serde_json::from_value(serde_json::json!({
                    "fft_size": fft, "speaker_config": layout,
                    "auto_gain_enabled": false
                }))
                .unwrap();
                let mut plugin = UpmixerPlugin::from_params(params);
                plugin.initialize(f64::from(rate)).unwrap();
                let cases: Vec<_> = plugin
                    .parameters()
                    .into_iter()
                    .filter(|p| p.update_mode == UpdateMode::Realtime)
                    .map(|p| {
                        let changed = match (&p.default_value, &p.min_value, &p.max_value) {
                            (ParameterValue::Bool(v), _, _) => ParameterValue::Bool(!v),
                            (
                                ParameterValue::Float(_),
                                Some(ParameterValue::Float(lo)),
                                Some(ParameterValue::Float(hi)),
                            ) => ParameterValue::Float(lo + 0.37 * (hi - lo)),
                            _ => panic!("unexpected realtime type: {}", p.id.as_str()),
                        };
                        (p, changed)
                    })
                    .collect();
                for id in ["height_gain", "bypass_heights"] {
                    assert!(
                        cases
                            .iter()
                            .any(|(parameter, _)| parameter.id.as_str() == id)
                    );
                }
                let case_count = cases.len();
                let input = vec![0.02; 17 * 2];
                let mut output = vec![0.0; 17 * plugin.output_channels()];
                let report = std::thread::spawn(move || {
                    let mut setter_max = (0, 0);
                    let mut audio_max = (0, 0);
                    for (metadata, changed) in &cases {
                        for _ in 0..2 {
                            // Changed target followed by an identical snapshot.
                            let (result, counts) = measure_heap(|| {
                                plugin.set_parameter(metadata.id.clone(), changed.clone())
                            });
                            result.unwrap();
                            setter_max = (setter_max.0.max(counts.0), setter_max.1.max(counts.1));
                            let cached = plugin
                                .parameters()
                                .into_iter()
                                .find(|p| p.id == metadata.id)
                                .unwrap();
                            assert_eq!(
                                Some(cached.default_value.clone()),
                                plugin.get_parameter(&metadata.id)
                            );
                            let mut before = serde_json::to_value(metadata).unwrap();
                            let mut after = serde_json::to_value(&cached).unwrap();
                            before.as_object_mut().unwrap().remove("default_value");
                            after.as_object_mut().unwrap().remove("default_value");
                            assert_eq!(before, after);
                            let (result, counts) = measure_heap(|| {
                                plugin.process(&input, &mut output, &ProcessContext::new(rate, 17))
                            });
                            assert_eq!(result.unwrap(), 17);
                            audio_max = (audio_max.0.max(counts.0), audio_max.1.max(counts.1));
                        }
                    }
                    (setter_max, audio_max)
                })
                .join()
                .unwrap();
                println!(
                    "SCALAR_HEAP rate={rate} fft={fft} layout={layout} controls={case_count} setter={:?} audio={:?}",
                    report.0, report.1,
                );
                reports.push(report);
            }
        }
    }
    assert!(
        reports.iter().all(|r| *r == ((0, 0), (0, 0))),
        "{reports:?}"
    );
}

#[test]
fn scalar_cache_preserves_canonical_types_clamps_and_rejected_state() {
    use sotf_host::ParameterValue;
    let mut plugin = UpmixerPlugin::from_params(UpmixerPluginParams::default());
    let id: sotf_host::ParameterId = "gain_front_direct".into();
    for (requested, expected) in [
        (ParameterValue::Int(1000), ParameterValue::Float(2.0)),
        (ParameterValue::Float(-10.0), ParameterValue::Float(0.0)),
        (ParameterValue::Float(0.37), ParameterValue::Float(0.37)),
    ] {
        plugin.set_parameter(id.clone(), requested).unwrap();
        assert_eq!(plugin.get_parameter(&id), Some(expected.clone()));
        assert_eq!(
            plugin
                .parameters()
                .into_iter()
                .find(|p| p.id == id)
                .unwrap()
                .default_value,
            expected
        );
    }
    let before = serde_json::to_value(plugin.parameters()).unwrap();
    for invalid in [
        ParameterValue::Float(f32::NAN),
        ParameterValue::Float(f32::INFINITY),
        ParameterValue::Bool(true),
    ] {
        assert!(plugin.set_parameter(id.clone(), invalid).is_err());
        assert_eq!(serde_json::to_value(plugin.parameters()).unwrap(), before);
        assert_eq!(plugin.get_parameter(&id), Some(ParameterValue::Float(0.37)));
    }
    assert!(
        plugin
            .set_parameter("unknown".into(), ParameterValue::Float(1.0))
            .is_err()
    );
    assert_eq!(serde_json::to_value(plugin.parameters()).unwrap(), before);
}

#[test]
fn resource_controls_are_structural_and_scalar_controls_remain_realtime() {
    use sotf_host::param_specs::UpdateMode;
    let plugin = UpmixerPlugin::from_params(UpmixerPluginParams::default());
    let structural = [
        "speaker_config",
        "lfe_cutoff_hz",
        "bandpass_hz",
        "decorrelation_mode",
        "velvet_noise_duration_ms",
        "velvet_noise_density",
        "height_hf_cap_hz",
        "low_latency",
        "frequency_resolution",
        "bypass_decorrelation",
        "bypass_all_processing",
        "enable_ml_detection",
        "binaural_preview",
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
        "input_format",
    ];
    let parameters = plugin.parameters();
    for id in structural {
        assert!(
            parameters
                .iter()
                .any(|parameter| parameter.id.as_str() == id),
            "missing {id}"
        );
    }
    for parameter in &parameters {
        assert_eq!(
            parameter.update_mode,
            if structural.contains(&parameter.id.as_str()) {
                UpdateMode::Structural
            } else {
                UpdateMode::Realtime
            },
            "{}",
            parameter.id.as_str()
        );
    }
}

#[test]
fn hot_safety_automation_with_autogain_and_partial_eof_has_no_heap_activity() {
    for rate in [48_000, 192_000] {
        for fft in [2, 512, 4096] {
            for binaural in [false, true] {
                let params = serde_json::from_value(serde_json::json!({
                    "fft_size":fft,"speaker_config":"9.1.6","enable_hr_direct":true,
                    "auto_gain_enabled":true,"binaural_preview":binaural
                }))
                .unwrap();
                let mut p = UpmixerPlugin::from_params(params);
                p.initialize(f64::from(rate)).unwrap();
                p.set_parameter(
                    "safety_cap_db".into(),
                    sotf_host::ParameterValue::Float(0.0),
                )
                .unwrap();
                let input = vec![4.0; 8193 * 2];
                let ch = p.output_channels();
                let mut output = vec![0.0; 8193 * ch];
                let mut tail = vec![0.0; p.drain_output_frames_max() * ch];
                let counts = std::thread::spawn(move || {
                    let (_, counts) = measure_heap(|| {
                        for _ in 0..2 {
                            assert_eq!(
                                p.process(&input, &mut output, &ProcessContext::new(rate, 8193))
                                    .unwrap(),
                                8193
                            );
                            assert!(output.iter().all(|s| s.is_finite() && s.abs() <= 1.413));
                            let bound = p.drain_call_bound().unwrap().get();
                            let mut complete = false;
                            for _ in 0..bound {
                                let result =
                                    p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
                                assert!(
                                    tail[..result.frames * ch]
                                        .iter()
                                        .all(|s| s.is_finite() && s.abs() <= 1.413)
                                );
                                if result.complete {
                                    complete = true;
                                    break;
                                }
                            }
                            assert!(complete);
                            p.reset();
                        }
                    });
                    counts
                })
                .join()
                .unwrap();
                println!("safety_heap rate={rate} fft={fft} binaural={binaural} counts={counts:?}");
                assert_eq!(counts, (0, 0));
            }
        }
    }
}

#[test]
fn prepared_host_scalar_queue_and_first_audio_have_no_heap_activity() {
    for rate in [48_000, 192_000] {
        for fft in [512, 4096] {
            let params =
                serde_json::from_value(serde_json::json!({"fft_size":fft,"speaker_config":"2.0"}))
                    .unwrap();
            let mut host = sotf_host::host::DawHost::new(2, rate);
            host.set_parallel_enabled(false).unwrap();
            let node = host
                .add_node(
                    "upmixer".into(),
                    Box::new(UpmixerPlugin::from_params(params)),
                )
                .unwrap();
            host.build().unwrap();
            let owner: sotf_host::ParameterId = "gain_front_direct".into();
            let input = vec![0.02; 514];
            let mut output = vec![0.0; 514];
            let counts = std::thread::spawn(move || {
                let (_, counts) = measure_heap(|| {
                    host.queue_node_parameter_at(
                        node,
                        owner.clone(),
                        sotf_host::ParameterValue::Float(0.3),
                        17,
                    )
                    .unwrap();
                    assert_eq!(host.process(&input, &mut output).unwrap(), 257);
                });
                assert_eq!(
                    host.get_plugin(0).unwrap().get_parameter(&owner),
                    Some(sotf_host::ParameterValue::Float(0.3))
                );
                counts
            })
            .join()
            .unwrap();
            println!("host_queue_heap rate={rate} fft={fft} counts={counts:?}");
            assert_eq!(counts, (0, 0));
        }
    }
}
