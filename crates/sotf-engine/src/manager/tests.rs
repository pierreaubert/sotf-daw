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
