// Rust guideline compliant 2026-02-21

use super::audio_engine_manager::AudioEngineManager;
use super::select::select_output_sample_rate_with_verifier;
use super::streaming_state::StreamingState;
use super::types::StreamingEvent;
use super::types::cache_verified_rate;
use super::types::clear_verified_rate_cache;
use super::types::get_cached_verified_rate;
use super::types::verified_rate_cache_key;
use serial_test::serial;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[test]
fn test_manager_creation() {
    let manager = AudioEngineManager::new();
    assert_eq!(manager.get_state(), StreamingState::Idle);
    assert!(manager.get_audio_info().is_none());
}

#[test]
fn driver_capture_runs_through_dsp_and_device_free_output() {
    use driver_common::{CaptureInput, CaptureSource, DriverConfig};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    struct MarkedCapture(Mutex<usize>);
    impl CaptureInput for MarkedCapture {
        fn configuration(&self) -> DriverConfig {
            DriverConfig::new(48_000, 128, 2)
        }
        fn available_frames(&self) -> usize {
            *self.0.lock().unwrap()
        }
        fn read_frames(&self, output: &mut [f32], expected: DriverConfig) -> usize {
            if expected != self.configuration() {
                return 0;
            }
            let mut remaining = self.0.lock().unwrap();
            let frames = (*remaining).min(output.len() / 2);
            for frame in output[..frames * 2].as_chunks_mut::<2>().0 {
                frame.copy_from_slice(&[0.5, -0.25]);
            }
            *remaining -= frames;
            frames
        }
    }

    let source = CaptureSource::new(MarkedCapture(Mutex::new(48_000)));
    let mut manager = AudioEngineManager::new();
    manager
        .start_driver_playback_with_source(
            None,
            vec![crate::PluginConfig::new(
                "gain",
                serde_json::json!({"gain_db": -6.020599913279624}),
            )],
            2,
            48_000,
            128,
            2,
            crate::SinkType::LabNull,
            Some(source),
        )
        .unwrap();
    assert_eq!(manager.get_playback_state(), crate::PlaybackState::Playing);
    // Published progress/meters have a five-second cadence; allow a full report
    // interval plus scheduling margin rather than racing the exact boundary.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = manager.get_engine_state();
        if state.playback_peak_max_linear > 0.01 {
            let peak = state.playback_peak_max_linear;
            manager.stop().unwrap();
            assert!(
                (peak - 0.25).abs() < 0.001,
                "captured PCM must pass through gain: {peak}"
            );
            break;
        }
        if Instant::now() >= deadline {
            manager.stop().unwrap();
            panic!("marked capture never reached device-free output: {state:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "test-util")]
#[test]
fn invalid_initial_driver_graph_never_admits_capture_or_output() {
    use driver_common::{CaptureInput, CaptureSource, DriverConfig};

    #[derive(Debug)]
    struct ObservedCapture(Arc<AtomicUsize>);
    impl CaptureInput for ObservedCapture {
        fn configuration(&self) -> DriverConfig {
            DriverConfig::new(48_000, 512, 2)
        }
        fn available_frames(&self) -> usize {
            4096
        }
        fn read_frames(&self, _output: &mut [f32], _expected: DriverConfig) -> usize {
            self.0.fetch_add(1, Ordering::Relaxed);
            0
        }
    }

    let output = crate::lab_pcm::TestOutput::new("Graph failure output");
    let probe = output.probe();
    probe.arm();
    let reads = Arc::new(AtomicUsize::new(0));
    let graph: crate::engine::PluginGraphConfig = serde_json::from_value(serde_json::json!({
        "nodes":[{"id":7,"plugin_type":"missing-graph-startup-fixture","parameters":{},"input_channels":2}],
        "edges":[]
    })).unwrap();
    let mut manager = AudioEngineManager::new();
    let result = manager.start_driver_playback_with_graph_and_source(
        Some(output.name().to_owned()),
        Vec::new(),
        2,
        48_000,
        512,
        2,
        crate::SinkType::TestOutput(output),
        Some(CaptureSource::new(ObservedCapture(Arc::clone(&reads)))),
        Some(graph),
    );
    assert!(result.is_err(), "an unbuildable graph must fail startup");
    manager.stop().unwrap();
    assert_eq!(
        reads.load(Ordering::Relaxed),
        0,
        "capture started before graph validation/commit"
    );
    assert_eq!(
        probe.disarm().unwrap().blocks,
        0,
        "output opened before graph construction succeeded"
    );
}

#[cfg(feature = "test-util")]
#[test]
fn test_file_output_runs_decode_dsp_and_transport_without_hardware() {
    use crate::PlaybackState;
    use std::time::{Duration, Instant};

    const SAMPLE_RATE: u32 = 48_000;
    const FILE_SECONDS: usize = 4;
    const FRAME_COUNT: usize = SAMPLE_RATE as usize * FILE_SECONDS;

    let directory = tempfile::tempdir().unwrap();
    let file_path = directory.path().join("transport-test.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&file_path, spec).unwrap();
    for frame in 0..FRAME_COUNT {
        let phase = std::f32::consts::TAU * 440.0 * frame as f32 / SAMPLE_RATE as f32;
        let sample = (phase.sin() * 8_000.0) as i16;
        writer.write_sample(sample).unwrap();
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();

    let mut manager = AudioEngineManager::new();
    manager.set_test_output_enabled(true).unwrap();
    let info = manager.load_file(&file_path).unwrap();
    assert_eq!(info.spec.sample_rate, SAMPLE_RATE);
    // A deliberately nonexistent device must be ignored by the selected
    // device-free output; this would fail if CPAL enumeration were reached.
    manager
        .start_playback(
            Some("no physical device should be opened".to_string()),
            vec![crate::PluginConfig::new(
                "gain",
                serde_json::json!({"gain_db": -6.0}),
            )],
            2,
        )
        .unwrap();

    let wait_for_playback = |manager: &AudioEngineManager| {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let state = manager.get_engine_state();
            if state.playback_output_device.as_deref() == Some("Test File Output")
                && state.position > 0.1
                && state.output_peak_linear > 0.05
            {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "file playback did not render: {state:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let initial = wait_for_playback(&manager);
    assert_eq!(initial.sample_rate, SAMPLE_RATE);
    assert_eq!(initial.playback_state, PlaybackState::Playing);
    assert!(initial.output_peak_linear > 0.05, "DSP meter: {initial:?}");

    manager.pause().unwrap();
    assert_eq!(manager.get_playback_state(), PlaybackState::Paused);
    let paused_position = manager.get_engine_state().position;
    std::thread::sleep(Duration::from_millis(60));
    assert!(
        (manager.get_engine_state().position - paused_position).abs() < 0.03,
        "position advanced while paused"
    );

    manager.resume().unwrap();
    assert_eq!(manager.get_playback_state(), PlaybackState::Playing);
    manager.seek(1.25).unwrap();
    let sought = manager.get_engine_state();
    assert_eq!(sought.playback_state, PlaybackState::Playing);
    assert!(
        (1.0..=1.6).contains(&sought.position),
        "seek position: {sought:?}"
    );

    manager.stop().unwrap();
    assert!(manager.engine.load().is_none());
    // The selector belongs to this manager and applies after a restart.
    manager
        .start_playback_at(None, Vec::new(), 2, Some(0.5))
        .unwrap();
    let restarted = wait_for_playback(&manager);
    assert_eq!(
        restarted.playback_output_device.as_deref(),
        Some("Test File Output")
    );
    assert_eq!(restarted.sample_rate, SAMPLE_RATE);
    let processed_to_dry_peak_ratio = initial.output_peak_linear / restarted.output_peak_linear;
    assert!(
        (0.45..=0.55).contains(&processed_to_dry_peak_ratio),
        "-6 dB gain should halve the measured output peak; processed={:?}, dry={:?}, ratio={processed_to_dry_peak_ratio}",
        initial.output_peak_linear,
        restarted.output_peak_linear
    );
    manager.stop().unwrap();
}

#[test]
fn bypass_is_retained_without_an_engine() {
    let mut manager = AudioEngineManager::new();
    for bypass in [true, false, true] {
        manager.set_bypass(bypass).unwrap();
        manager.stop().unwrap();
        assert_eq!(manager.current_bypassed.load(Ordering::Relaxed), bypass);
        assert!(manager.engine.load().is_none());
    }
}

#[test]
fn rack_bypass_keeps_live_upmixer_engine_and_survives_restart() {
    let mut manager = AudioEngineManager::new();
    manager.set_bypass(true).unwrap();
    for _ in 0..2 {
        manager
            .start_driver_playback_with_driver_config_and_sink(
                None,
                vec![crate::PluginConfig::new(
                    "upmixer",
                    serde_json::json!({"speaker_config": "7.1.2"}),
                )],
                10,
                48_000,
                256,
                2,
                crate::SinkType::LabNull,
            )
            .unwrap();
        let engine = manager.engine.load_full().unwrap();
        assert!(engine.get_state().processing_bypassed);
        for bypass in [false, true, false, true] {
            manager.set_bypass(bypass).unwrap();
            let current = manager.engine.load_full().unwrap();
            assert!(
                Arc::ptr_eq(&engine, &current),
                "bypass must retain the engine"
            );
            let state = current.get_state();
            assert_eq!(state.processing_bypassed, bypass);
            assert_eq!(state.num_channels, 10);
            assert_eq!(state.sample_rate, 48_000);
            assert!(state.last_error.is_none(), "{state:?}");
        }
        manager.stop().unwrap();
    }
}

#[test]
fn signal_watching_is_forwarded_without_enabling_file_watching() {
    let manager = AudioEngineManager::with_signal_watching(true);
    assert_eq!(manager.engine_watch_flags(), (false, true));
}

#[test]
fn test_state_transitions() {
    let manager = AudioEngineManager::new();

    assert_eq!(manager.get_state(), StreamingState::Idle);

    // Loading state would be set by load_file
    manager.set_state(StreamingState::Loading);
    assert_eq!(manager.get_state(), StreamingState::Loading);

    manager.set_state(StreamingState::Ready);
    assert_eq!(manager.get_state(), StreamingState::Ready);
}

#[test]
fn try_recv_event_emits_end_of_stream_when_stopped_from_playing() {
    let manager = AudioEngineManager::new();

    // Simulate that we were previously playing; engine state defaults to Stopped
    manager.set_state(StreamingState::Playing);

    let event = manager.try_recv_event();
    assert!(matches!(event, Some(StreamingEvent::EndOfStream)));
    assert_eq!(manager.get_state(), StreamingState::Idle);
}

#[test]
fn invalid_atomic_state_maps_to_error_instead_of_panicking() {
    let manager = AudioEngineManager::new();
    manager.state.store(255, Ordering::Relaxed);

    assert_eq!(manager.get_state(), StreamingState::Error);
}

#[test]
fn failed_load_does_not_leave_manager_loading() {
    let mut manager = AudioEngineManager::new();

    let result = manager.load_file("/definitely/not/a/real/file.wav");

    assert!(result.is_err());
    assert_eq!(manager.get_state(), StreamingState::Error);
}

#[test]
fn failed_seek_restores_previous_streaming_state() {
    let manager = AudioEngineManager::new();
    manager.set_state(StreamingState::Ready);

    let result = manager.seek(1.0);

    assert!(result.is_err());
    assert_eq!(manager.get_state(), StreamingState::Ready);
}

#[test]
fn identical_latched_error_is_reported_only_once() {
    let manager = AudioEngineManager::new();

    assert_eq!(
        manager.take_unreported_error("decoder failed".to_string()),
        Some("decoder failed".to_string())
    );
    assert_eq!(
        manager.take_unreported_error("decoder failed".to_string()),
        None
    );
    assert_eq!(
        manager.take_unreported_error("device unplugged".to_string()),
        Some("device unplugged".to_string())
    );
}

#[test]
fn clearing_latched_error_allows_same_error_to_be_reported_again() {
    let manager = AudioEngineManager::new();
    assert!(
        manager
            .take_unreported_error("decoder failed".to_string())
            .is_some()
    );

    // A healthy engine snapshot clears the event-level deduplication latch.
    assert!(manager.try_recv_event().is_none());
    assert!(
        manager
            .take_unreported_error("decoder failed".to_string())
            .is_some()
    );
}

#[test]
fn verified_rate_cache_key_includes_output_channels() {
    assert_ne!(
        verified_rate_cache_key(Some("Built-in Output"), 2, 48_000),
        verified_rate_cache_key(Some("Built-in Output"), 6, 48_000)
    );
}

// Serialized with the fallback-reuse regression below under the shared
// `verified_rate_cache` key: this is the only test that clears the
// process-global cache, and the reuse test asserts an exact probe count
// across two calls. Every other cache test uses unique device keys and runs
// fully parallel. Any future test that clears the cache must join this
// named serial pair.
#[test]
#[serial(verified_rate_cache)]
fn verified_rate_cache_stores_multiple_device_channel_entries() {
    clear_verified_rate_cache();

    let built_in = verified_rate_cache_key(Some("Built-in Output"), 2, 48_000);
    let surround = verified_rate_cache_key(Some("Built-in Output"), 6, 96_000);
    let default = verified_rate_cache_key(None, 2, 44_100);

    cache_verified_rate(built_in.clone(), 48_000);
    cache_verified_rate(surround.clone(), 96_000);
    cache_verified_rate(default.clone(), 44_100);

    assert_eq!(get_cached_verified_rate(&built_in), Some(48_000));
    assert_eq!(get_cached_verified_rate(&surround), Some(96_000));
    assert_eq!(get_cached_verified_rate(&default), Some(44_100));

    clear_verified_rate_cache();
    assert_eq!(get_cached_verified_rate(&built_in), None);
}

/// R13 regression: same device and channels, but 44100 and 48000 requests
/// must not poison each other. The scripted verifier echoes the requested
/// rate (as the PipeWire path does), so any cross-rate cache hit returns
/// the wrong rate and fails loudly.
#[test]
fn mixed_requested_rates_do_not_poison_each_other() {
    let device = Some("output-rate-cache-mixed-rates-device");
    let probes = Arc::new(AtomicUsize::new(0));
    let verify = {
        let probes = Arc::clone(&probes);
        move |_device: Option<&str>, requested: u32, _channels: usize| {
            probes.fetch_add(1, Ordering::SeqCst);
            Some(requested)
        }
    };

    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device, 2, &verify),
        44_100
    );
    assert_eq!(
        select_output_sample_rate_with_verifier(48_000, device, 2, &verify),
        48_000
    );
    // Fresh key per request, so each call probes exactly once; a global
    // clear cannot add probes beyond one per call.
    assert_eq!(probes.load(Ordering::SeqCst), 2);
}

/// A verified fallback must be reused for the same request without
/// re-probing. In particular, a cached rate that differs from the request
/// is a legitimate fallback, not a reason to probe again.
#[test]
#[serial(verified_rate_cache)]
fn repeated_request_reuses_verified_fallback_without_reprobe() {
    let device = Some("output-rate-cache-fallback-reuse-device");
    let probes = Arc::new(AtomicUsize::new(0));
    let verify = {
        let probes = Arc::clone(&probes);
        move |_device: Option<&str>, _requested: u32, _channels: usize| {
            probes.fetch_add(1, Ordering::SeqCst);
            Some(48_000)
        }
    };

    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device, 2, &verify),
        48_000
    );
    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device, 2, &verify),
        48_000
    );
    assert_eq!(probes.load(Ordering::SeqCst), 1);
}

/// Device and channel separation survive the rate dimension: distinct
/// (device, channels) pairs verify independently even for one request.
#[test]
fn device_and_channel_separation_survive_rate_dimension() {
    let device_a = Some("output-rate-cache-separation-device-a");
    let device_b = Some("output-rate-cache-separation-device-b");
    let probes = Arc::new(AtomicUsize::new(0));
    let verify = {
        let probes = Arc::clone(&probes);
        move |device: Option<&str>, _requested: u32, channels: usize| {
            probes.fetch_add(1, Ordering::SeqCst);
            match (device, channels) {
                (Some("output-rate-cache-separation-device-a"), 2) => Some(48_000),
                (Some("output-rate-cache-separation-device-a"), 6) => Some(96_000),
                (Some("output-rate-cache-separation-device-b"), 2) => Some(44_100),
                _ => None,
            }
        }
    };

    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device_a, 2, &verify),
        48_000
    );
    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device_a, 6, &verify),
        96_000
    );
    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device_b, 2, &verify),
        44_100
    );
    // Fresh key per pair, so each call probes exactly once.
    assert_eq!(probes.load(Ordering::SeqCst), 3);
}

/// Failed verification still falls back to the requested rate and still
/// caches nothing: every call re-verifies.
#[test]
fn verification_failure_returns_candidate_without_caching() {
    let device = Some("output-rate-cache-failure-device");
    let probes = Arc::new(AtomicUsize::new(0));
    let verify = {
        let probes = Arc::clone(&probes);
        move |_device: Option<&str>, _requested: u32, _channels: usize| {
            probes.fetch_add(1, Ordering::SeqCst);
            None
        }
    };

    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device, 2, &verify),
        44_100
    );
    assert_eq!(
        select_output_sample_rate_with_verifier(44_100, device, 2, &verify),
        44_100
    );
    assert_eq!(probes.load(Ordering::SeqCst), 2);
    assert_eq!(
        get_cached_verified_rate(&verified_rate_cache_key(device, 2, 44_100)),
        None
    );
}

#[test]
fn public_state_accessors_recover_after_mutex_poisoning() {
    let manager = AudioEngineManager::new();
    let poisoned_audio_info = Arc::clone(&manager.current_audio_info);

    let _ = std::panic::catch_unwind(move || {
        let _guard = poisoned_audio_info.lock().unwrap();
        panic!("poison current_audio_info");
    });

    assert!(manager.get_audio_info().is_none());
}
