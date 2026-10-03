use super::super::{
    AudioEngineState, EngineConfig, PlaybackState, ThreadEvent, plan_engine_features,
};
use super::config_error::ensure_output_channel_capacity;
use super::error::ConfigError;
use super::estimate::{estimate_graph_update_timeout, estimate_update_timeout};
use super::handle::handle_thread_event;
use super::misc::initial_engine_state_from_config;
#[cfg(feature = "streaming")]
use super::misc::start_network_stream_server;
use super::thread_event_visitor::apply_decoder_async_errors;
use super::thread_event_visitor::chain_decoder_causes;
use super::thread_event_visitor::expire_seeking_display;
use super::thread_event_visitor::record_decoder_error;
use super::thread_event_visitor::select_current_decoder_errors;
use super::validate::validate_gapless_source_compatible;
use super::validate::validate_plugin_configs;
use arc_swap::ArcSwap;
use std::sync::Arc;

mod misc;

#[test]
fn test_config_error_display() {
    // Test ParseError display
    let err = ConfigError::ParseError {
        path: std::path::PathBuf::from("/test/config.yaml"),
        reason: "invalid syntax".to_string(),
    };
    assert!(err.to_string().contains("Failed to parse config"));
    assert!(err.to_string().contains("invalid syntax"));

    // Test ValidationError display
    let err = ConfigError::ValidationError {
        plugin_index: 2,
        reason: "unknown plugin type".to_string(),
    };
    assert!(err.to_string().contains("Plugin 2"));
    assert!(err.to_string().contains("unknown plugin type"));

    // Test TimeoutError display
    let err = ConfigError::TimeoutError { waited_ms: 5000 };
    assert!(err.to_string().contains("5000ms"));

    // Test ProcessingError display
    let err = ConfigError::ProcessingError {
        reason: "plugin init failed".to_string(),
    };
    assert!(err.to_string().contains("plugin init failed"));

    // Test ChannelDisconnected display
    let err = ConfigError::ChannelDisconnected;
    assert!(err.to_string().contains("disconnected"));
}

#[test]
fn test_config_error_is_error_trait() {
    let err: Box<dyn std::error::Error> = Box::new(ConfigError::TimeoutError { waited_ms: 100 });
    assert!(err.to_string().contains("100ms"));
}

#[test]
fn plugin_host_builds_are_handed_to_named_worker_threads() {
    let apply = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/manager_thread/apply.rs"),
    )
    .unwrap();

    assert!(apply.contains("build_plugin_update_host_on_worker"));
    assert!(apply.contains("build_plugin_graph_host_on_worker"));
    assert!(apply.contains(".name(\"sotf-plugin-host-builder\".to_string())"));
    assert!(apply.contains(".name(\"sotf-plugin-graph-builder\".to_string())"));
}

#[test]
fn audio_engine_cached_reads_do_not_use_command_lock() {
    let audio_engine = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/audio_engine.rs"),
    )
    .unwrap();

    for method in [
        "pub fn get_state(&self)",
        "pub fn get_playback_state(&self)",
        "pub fn get_cached_plugin_data(",
    ] {
        let start = audio_engine.find(method).expect(method);
        let body = &audio_engine[start..audio_engine[start..].find("\n    }").unwrap() + start];
        assert!(
            !body.contains("command_lock"),
            "{method} should stay a cached/lock-free read path"
        );
        assert!(
            !body.contains("ManagerCommand::"),
            "{method} should not contend on the manager command response path"
        );
    }
}

#[test]
fn playback_feeder_does_not_claim_hardware_callback_realtime_policy() {
    let runtime = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/engine/playback_thread/runtime.rs"),
    )
    .unwrap();

    assert!(runtime.contains("CPAL owns the hardware callback"));
    assert!(runtime.contains("RtPriority::Processing"));
    assert!(!runtime.contains("RtPriority::Playback"));
}

#[test]
fn decoder_uses_soft_audio_work_priority() {
    let decoder = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/decoder_thread/types.rs"),
    )
    .unwrap();
    assert!(decoder.contains("RtPriority::Processing"));
    assert!(!decoder.contains("RtPriority::Playback"));
}

#[test]
fn validate_gapless_source_does_not_open_urls_on_manager_thread() {
    let source = crate::decoder::AudioSource::Url {
        url: "http://127.0.0.1:9/unreachable.wav".to_string(),
        format_hint: Some("wav".to_string()),
        seekable: true,
    };

    assert!(validate_gapless_source_compatible(&source, 2).is_ok());
}

#[test]
fn test_validate_plugin_configs_valid() {
    let configs = vec![
        super::super::PluginConfig {
            plugin_type: "gain".to_string(),
            parameters: serde_json::json!({"gain_db": -3.0}),
        },
        super::super::PluginConfig {
            plugin_type: "EQ".to_string(),
            parameters: serde_json::json!({"filters": []}),
        },
    ];
    assert!(validate_plugin_configs(&configs).is_ok());
}

#[test]
fn test_validate_plugin_configs_accepts_external_plugin_type() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("sotf-external-test-plugin.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();

    let configs = vec![super::super::PluginConfig {
        plugin_type: "external".to_string(),
        parameters: serde_json::json!({
            "path": plugin_path.to_string_lossy(),
            "audio_inputs": 2,
            "audio_outputs": 2,
            "format": "clap",
        }),
    }];

    assert!(validate_plugin_configs(&configs).is_ok());
}

#[test]
fn test_validate_plugin_configs_rejects_insecure_external_plugin_config() {
    let configs = vec![super::super::PluginConfig {
        plugin_type: "external".to_string(),
        parameters: serde_json::json!({
            "path": "/tmp/fake.clap",
            "isolated": false,
        }),
    }];

    let result = validate_plugin_configs(&configs);
    assert!(result.is_err());
    if let Err(ConfigError::ValidationError {
        plugin_index,
        reason,
    }) = result
    {
        assert_eq!(plugin_index, 0);
        assert!(reason.contains("cannot disable process isolation"));
    }
}

#[test]
fn test_validate_plugin_configs_unknown_type() {
    let configs = vec![super::super::PluginConfig {
        plugin_type: "unknown_plugin".to_string(),
        parameters: serde_json::json!({}),
    }];
    let result = validate_plugin_configs(&configs);
    assert!(result.is_err());
    if let Err(ConfigError::ValidationError {
        plugin_index,
        reason,
    }) = result
    {
        assert_eq!(plugin_index, 0);
        assert!(reason.contains("Unknown plugin type"));
    } else {
        panic!("Expected ValidationError");
    }
}

#[test]
fn test_validate_plugin_configs_missing_gain_db() {
    let configs = vec![super::super::PluginConfig {
        plugin_type: "gain".to_string(),
        parameters: serde_json::json!({"other": 1.0}),
    }];
    let result = validate_plugin_configs(&configs);
    assert!(result.is_err());
    if let Err(ConfigError::ValidationError { reason, .. }) = result {
        assert!(reason.contains("gain_db"));
    } else {
        panic!("Expected ValidationError");
    }
}

#[test]
fn test_validate_plugin_configs_accepts_all_types() {
    use crate::plugins::{PluginSettings, PluginType};

    let sample_rate = 48000.0;

    for plugin_type in PluginType::all() {
        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate);

        let result = validate_plugin_configs(std::slice::from_ref(&config));
        assert!(
            result.is_ok(),
            "validate_plugin_configs rejected '{}': {:?}",
            config.plugin_type,
            result.unwrap_err()
        );
    }
}

#[test]
fn test_ensure_output_channel_capacity_warns_but_allows_mismatch() {
    // When the chain needs more channels than configured, the function
    // logs a warning but succeeds — the playback thread handles downmix.
    let result = ensure_output_channel_capacity(6, 2, Some("Built-in Output"));
    assert!(result.is_ok());
}

#[test]
fn graph_update_timeout_uses_node_complexity() {
    let graph = crate::engine::types::PluginGraphConfig {
        nodes: (0..3)
            .map(|id| crate::engine::types::PluginGraphNodeConfig {
                id,
                plugin_type: "convolution".to_string(),
                parameters: serde_json::json!({}),
                input_channels: 2,
                bypassed: false,
            })
            .collect(),
        edges: vec![],
    };

    assert!(estimate_graph_update_timeout(&graph) > std::time::Duration::from_millis(5000));
}

#[test]
fn lowercase_eq_timeout_counts_filters() {
    let plugins = [crate::engine::PluginConfig {
        plugin_type: "eq".to_string(),
        parameters: serde_json::json!({"filters": [{}, {}, {}]}),
    }];

    assert_eq!(
        estimate_update_timeout(&plugins),
        std::time::Duration::from_millis(230)
    );
}

#[test]
fn test_ensure_output_channel_capacity_accepts_supported_chain() {
    let result = ensure_output_channel_capacity(2, 2, Some("Built-in Output"));

    assert!(result.is_ok());
}

#[test]
fn test_handle_thread_event_uses_actual_underrun_count() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));

    handle_thread_event(ThreadEvent::PlaybackUnderrun(101), &state);

    assert_eq!(state.load().underruns, 101);

    handle_thread_event(ThreadEvent::PlaybackUnderrun(205), &state);

    assert_eq!(state.load().underruns, 205);
}

#[test]
fn test_handle_thread_event_updates_playback_channels() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));

    handle_thread_event(ThreadEvent::PlaybackChannelsChanged(6), &state);
    assert_eq!(state.load().playback_channels, 6);
    assert_eq!(state.load().num_channels, 2);

    handle_thread_event(ThreadEvent::PlaybackChannelsChanged(2), &state);
    assert_eq!(state.load().playback_channels, 2);
}

#[test]
fn test_handle_thread_event_updates_playback_output_device() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));

    handle_thread_event(
        ThreadEvent::PlaybackOutputDeviceChanged("ADAM Audio D3V".to_string()),
        &state,
    );

    assert_eq!(
        state.load().playback_output_device.as_deref(),
        Some("ADAM Audio D3V")
    );
}

#[test]
fn test_handle_thread_event_updates_output_access_status() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));

    handle_thread_event(
        ThreadEvent::PlaybackOutputAccessChanged(crate::OutputAccessStatus::FallbackShared),
        &state,
    );

    assert_eq!(
        state.load().output_access_status,
        crate::OutputAccessStatus::FallbackShared
    );
}

#[test]
fn test_handle_thread_event_updates_playback_stats() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));

    handle_thread_event(
        ThreadEvent::PlaybackStats {
            callback_count: 12,
            buffer_fill_percent: 75,
            stream_error_count: 1,
            frames_received: 40,
            frames_written: 39,
            frames_dropped: 1,
            effective_sample_rate: 48_000,
            epoch: 0,
        },
        &state,
    );

    let s = state.load();
    assert_eq!(s.playback_callback_count, 12);
    assert_eq!(s.playback_buffer_fill_percent, 75);
    assert_eq!(s.playback_stream_error_count, 1);
    assert_eq!(s.playback_frames_received, 40);
    assert_eq!(s.playback_frames_written, 39);
    assert_eq!(s.playback_frames_dropped, 1);
    assert_eq!(s.playback_effective_sample_rate, 48_000);
}

#[test]
fn test_handle_thread_event_updates_stream_metadata() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));
    let metadata = crate::StreamMetadata {
        stream_title: Some("Artist - Song".to_string()),
        stream_url: Some("https://station.example/live".to_string()),
        content_type: Some("audio/mpeg".to_string()),
        bitrate_kbps: Some(192),
    };

    handle_thread_event(
        ThreadEvent::StreamMetadataChanged(Some(metadata.clone())),
        &state,
    );
    assert_eq!(state.load().stream_metadata, Some(metadata));

    handle_thread_event(ThreadEvent::StreamMetadataChanged(None), &state);
    assert!(state.load().stream_metadata.is_none());
}

#[test]
fn test_latency_update_adjusts_position_to_prevent_jump() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        position: 10.0,
        sample_rate: 48000,
        plugin_latency_samples: 4800, // 100ms
        ..AudioEngineState::default()
    }));

    // Latency increases from 4800 to 9600 samples (100ms → 200ms).
    // Position should shift back by 100ms (the delta) so the displayed
    // position doesn't jump when the next PositionUpdate arrives.
    handle_thread_event(ThreadEvent::PluginLatencyUpdate(9600), &state);

    let s = state.load();
    assert_eq!(s.plugin_latency_samples, 9600);
    assert!((s.position - 9.9).abs() < 1e-9, "position={}", s.position);
}

#[test]
fn test_latency_update_clamps_position_to_zero() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        position: 0.05,
        sample_rate: 48000,
        plugin_latency_samples: 0,
        ..AudioEngineState::default()
    }));

    // Latency increase of 4800 samples (100ms) on a position of 50ms
    // should clamp to 0.0, not go negative.
    handle_thread_event(ThreadEvent::PluginLatencyUpdate(4800), &state);

    assert_eq!(state.load().position, 0.0);
}

#[test]
fn test_latency_decrease_shifts_position_forward() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        position: 5.0,
        sample_rate: 48000,
        plugin_latency_samples: 9600, // 200ms
        ..AudioEngineState::default()
    }));

    // Latency decreases from 9600 to 4800 (200ms → 100ms).
    // Position should shift forward by 100ms.
    handle_thread_event(ThreadEvent::PluginLatencyUpdate(4800), &state);

    let s = state.load();
    assert!((s.position - 5.1).abs() < 1e-9, "position={}", s.position);
}

#[test]
fn test_latency_update_noop_when_unchanged() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        position: 10.0,
        sample_rate: 48000,
        plugin_latency_samples: 4800,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PluginLatencyUpdate(4800), &state);

    assert!((state.load().position - 10.0).abs() < 1e-9);
}

#[test]
fn test_latency_update_skips_position_shift_when_compensation_disabled() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        position: 10.0,
        sample_rate: 48000,
        plugin_latency_samples: 0,
        latency_compensation_enabled: false,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PluginLatencyUpdate(4800), &state);

    let s = state.load();
    assert_eq!(s.plugin_latency_samples, 4800);
    assert!((s.position - 10.0).abs() < 1e-9);
}

#[test]
fn test_position_update_compensates_latency() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        sample_rate: 48000,
        plugin_latency_samples: 4800, // 100ms
        ..AudioEngineState::default()
    }));

    // Decoder reports position 5.0s, but pipeline latency is 100ms,
    // so displayed position should be 4.9s.
    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    let s = state.load();
    assert!((s.position - 4.9).abs() < 1e-9, "position={}", s.position);
}

#[test]
fn test_position_update_ignored_when_stopped() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Stopped,
        position: 0.0,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    assert!((state.load().position - 0.0).abs() < 1e-9);
}

#[test]
fn test_processing_warning_sets_error_without_stopping() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        output_peak_linear: 1.25,
        output_clipping_detected: true,
        ..AudioEngineState::default()
    }));

    handle_thread_event(
        ThreadEvent::ProcessingWarning("channel rebuild fallback".to_string()),
        &state,
    );

    let s = state.load();
    // Warning should record the error message...
    assert_eq!(s.last_error.as_deref(), Some("channel rebuild fallback"));
    // ...but NOT change playback state to Stopped
    assert_eq!(s.playback_state, PlaybackState::Playing);
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn test_handle_thread_event_updates_isolated_external_plugin_worker_statuses() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));
    let initial_status = crate::IsolatedExternalPluginWorkerStatus {
        plugin_index: 1,
        node_id: 77,
        plugin_instance_id: None,
        event: Some(crate::IsolatedExternalPluginWorkerEvent::NotRunning),
        error: Some("transient".to_string()),
        worker_start_count: 1,
        worker_exit_count: 2,
        worker_launch_failure_count: 3,
        worker_quarantined: false,
        worker_quarantine_reason: None,
        block_timeout_count: 4,
        block_worker_failure_count: 5,
        block_wrong_sequence_count: 6,
        sandbox_status: crate::IsolatedExternalPluginSandboxStatus::Unknown,
        sandbox_backend: crate::IsolatedExternalPluginSandboxBackend::Unknown,
        sandbox_reason: None,
    };

    handle_thread_event(
        ThreadEvent::IsolatedExternalPluginWorkerStatuses(vec![initial_status.clone()]),
        &state,
    );

    let current = state.load();
    assert_eq!(
        current.isolated_external_plugin_worker_statuses,
        vec![initial_status]
    );

    let replacement_status = crate::IsolatedExternalPluginWorkerStatus {
        plugin_index: 2,
        node_id: 99,
        plugin_instance_id: None,
        event: Some(crate::IsolatedExternalPluginWorkerEvent::Started { pid: 42 }),
        error: None,
        worker_start_count: 10,
        worker_exit_count: 20,
        worker_launch_failure_count: 30,
        worker_quarantined: false,
        worker_quarantine_reason: None,
        block_timeout_count: 40,
        block_worker_failure_count: 50,
        block_wrong_sequence_count: 60,
        sandbox_status: crate::IsolatedExternalPluginSandboxStatus::Enforced,
        sandbox_backend: crate::IsolatedExternalPluginSandboxBackend::LinuxLandlock,
        sandbox_reason: None,
    };

    handle_thread_event(
        ThreadEvent::IsolatedExternalPluginWorkerStatuses(vec![replacement_status.clone()]),
        &state,
    );

    let current = state.load();
    assert_eq!(
        current.isolated_external_plugin_worker_statuses,
        vec![replacement_status]
    );
}

#[test]
fn test_processing_error_stops_playback() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        output_peak_linear: 1.25,
        output_clipping_detected: true,
        ..AudioEngineState::default()
    }));

    handle_thread_event(
        ThreadEvent::ProcessingError("fatal error".to_string()),
        &state,
    );

    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.last_error.as_deref(), Some("fatal error"));
    assert_eq!(s.output_peak_linear, 0.0);
    assert!(!s.output_clipping_detected);
}

#[test]
fn test_position_update_skips_latency_when_bypassed() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        sample_rate: 48000,
        plugin_latency_samples: 4800, // 100ms
        processing_bypassed: true,
        ..AudioEngineState::default()
    }));

    // With bypass=true, latency compensation should be skipped.
    // Decoder reports 5.0s → displayed position should be 5.0s (not 4.9s).
    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    let s = state.load();
    assert!(
        (s.position - 5.0).abs() < 1e-9,
        "Bypass should skip latency compensation, got position={}",
        s.position
    );
}

#[test]
fn test_position_update_applies_latency_when_not_bypassed() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        sample_rate: 48000,
        plugin_latency_samples: 4800, // 100ms
        processing_bypassed: false,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    let s = state.load();
    assert!(
        (s.position - 4.9).abs() < 1e-9,
        "Should apply latency compensation, got position={}",
        s.position
    );
}

#[test]
fn test_position_update_skips_latency_when_compensation_disabled() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        sample_rate: 48000,
        plugin_latency_samples: 4800,
        latency_compensation_enabled: false,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    let s = state.load();
    assert!((s.position - 5.0).abs() < 1e-9);
}

#[test]
fn test_initial_engine_state_surfaces_feature_policies() {
    let config = EngineConfig {
        output_sample_rate: 96_000,
        output_channels: 6,
        volume: 0.5,
        muted: true,
        latency_compensation: crate::LatencyCompensationMode::Disabled,
        output_access: crate::OutputAccessMode::ExclusivePreferred,
        dsd_output: crate::DsdOutputMode::DopPreferred,
        oversampling_policy: crate::EngineOversamplingPolicy::Force2x,
        network_endpoint: crate::NetworkEndpointConfig {
            mode: crate::NetworkEndpointMode::HttpEndpoint,
            ..Default::default()
        },
        ..EngineConfig::default()
    };

    let state = initial_engine_state_from_config(&config);

    assert_eq!(state.sample_rate, 96_000);
    assert_eq!(state.num_channels, 6);
    assert_eq!(state.volume, 0.5);
    assert!(state.muted);
    assert!(!state.latency_compensation_enabled);
    #[cfg(target_os = "macos")]
    let expected_output_access_status = crate::OutputAccessStatus::ExclusivePending;
    #[cfg(not(target_os = "macos"))]
    let expected_output_access_status = crate::OutputAccessStatus::FallbackShared;
    assert_eq!(state.output_access_status, expected_output_access_status);
    assert_eq!(
        state.dsd_output_status,
        crate::DsdOutputStatus::DopFallbackPcm
    );
    assert_eq!(
        state.oversampling_policy,
        crate::EngineOversamplingPolicy::Force2x
    );
    assert_eq!(
        state.network_endpoint_status,
        crate::NetworkEndpointStatus::EndpointUnavailable
    );
}

#[test]
fn test_dsd_output_statuses_distinguish_pcm_fallbacks_from_required_bitstream() {
    let status_for_mode = |mode| {
        plan_engine_features(&EngineConfig {
            dsd_output: mode,
            ..Default::default()
        })
        .dsd_output
        .status
    };

    assert_eq!(
        status_for_mode(crate::DsdOutputMode::PcmDecode),
        crate::DsdOutputStatus::PcmDecodeAvailable
    );
    assert_eq!(
        status_for_mode(crate::DsdOutputMode::DopPreferred),
        crate::DsdOutputStatus::DopFallbackPcm
    );
    assert_eq!(
        status_for_mode(crate::DsdOutputMode::DopRequired),
        crate::DsdOutputStatus::DopUnavailable
    );
    assert_eq!(
        status_for_mode(crate::DsdOutputMode::NativePreferred),
        crate::DsdOutputStatus::NativeFallbackPcm
    );
    assert_eq!(
        status_for_mode(crate::DsdOutputMode::NativeRequired),
        crate::DsdOutputStatus::NativeUnavailable
    );
}

#[cfg(feature = "streaming")]
#[test]
fn test_start_network_stream_server_updates_state_and_publishes_audio() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));
    let config = EngineConfig {
        output_sample_rate: 48_000,
        output_channels: 2,
        network_endpoint: crate::NetworkEndpointConfig {
            mode: crate::NetworkEndpointMode::HttpEndpoint,
            bind_addr: "127.0.0.1".to_string(),
            port: 0,
        },
        ..EngineConfig::default()
    };

    let Some((mut server, handle)) = start_network_stream_server(&config, &state) else {
        let current = state.load();
        if current.last_error.as_deref().is_some_and(|err| {
            err.contains("Operation not permitted") || err.contains("Permission denied")
        }) {
            return;
        }
        panic!(
            "expected network stream server to start: {:?}",
            current.last_error
        );
    };

    let current = state.load();
    assert_eq!(
        current.network_endpoint_status,
        crate::NetworkEndpointStatus::EndpointRunning
    );
    assert_ne!(current.network_endpoint.port, 0);

    assert!(handle.publish(&[0.0, 0.0, 0.25, -0.25], 2, 2, 48_000));
    assert_eq!(handle.stats().published_chunks, 1);

    server.shutdown();
}

#[test]
fn test_handle_thread_event_decoder_end_of_stream() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        output_peak_linear: 1.25,
        output_clipping_detected: true,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::DecoderEndOfStream, &state);

    // State should remain unchanged
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Playing);
}

#[test]
fn test_handle_thread_event_decoder_gapless_transition() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));
    let source = crate::decoder::AudioSource::File(std::path::PathBuf::from("/tmp/test.wav"));

    handle_thread_event(ThreadEvent::DecoderGaplessTransition(source), &state);

    let s = state.load();
    assert_eq!(
        s.current_file,
        Some(std::path::PathBuf::from("/tmp/test.wav"))
    );
    assert!(s.current_source.is_some());
    assert_eq!(s.position, 0.0);
}

#[test]
fn test_handle_thread_event_playback_drained() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        last_error: Some("previous error".to_string()),
        output_peak_linear: 1.25,
        output_clipping_detected: true,
        playback_peak_max_linear: 1.25,
        ..AudioEngineState::default()
    }));

    // Lossless run shape: the terminal record equals the latched max.
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 0,
            epoch_peak_max: 1.25,
            flush_gen: 0,
        },
        &state,
    );

    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert!(s.last_error.is_none());
    assert_eq!(s.output_peak_linear, 0.0);
    assert!(!s.output_clipping_detected);
    assert_eq!(s.playback_peak_max_linear, 1.25);
    drop(s);

    handle_thread_event(
        ThreadEvent::PlaybackOutputMeter {
            peak_linear: 1.5,
            clipping_detected: true,
            epoch: 0,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.output_peak_linear, 0.0);
    assert!(!s.output_clipping_detected);
    assert_eq!(s.playback_peak_max_linear, 1.25);
}

#[test]
fn test_handle_thread_event_stale_meter_rejected_by_epoch() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        output_peak_linear: 0.40,
        playback_peak_max_linear: 0.40,
        ..AudioEngineState::default()
    }));

    // Stale snapshot from the previous epoch: must not move live or latch.
    handle_thread_event(
        ThreadEvent::PlaybackOutputMeter {
            peak_linear: 0.90,
            clipping_detected: true,
            epoch: 4,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.output_peak_linear, 0.40);
    assert!(!s.output_clipping_detected);
    assert_eq!(s.playback_peak_max_linear, 0.40);
    drop(s);

    // Mis-tagged future epoch: the gate is equality, direction-agnostic.
    handle_thread_event(
        ThreadEvent::PlaybackOutputMeter {
            peak_linear: 0.90,
            clipping_detected: true,
            epoch: 6,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.output_peak_linear, 0.40);
    assert_eq!(s.playback_peak_max_linear, 0.40);
    drop(s);

    // Current epoch: accepted into live and latch.
    handle_thread_event(
        ThreadEvent::PlaybackOutputMeter {
            peak_linear: 0.60,
            clipping_detected: false,
            epoch: 5,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.output_peak_linear, 0.60);
    assert_eq!(s.playback_peak_max_linear, 0.60);
}

#[test]
fn test_handle_thread_event_stale_drained_rejected_by_epoch() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        output_peak_linear: 0.30,
        playback_peak_max_linear: 0.40,
        ..AudioEngineState::default()
    }));

    // Late receipt for the previous epoch: transport and meters untouched.
    // The stale peak (above the latch) must not leak into the latch.
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 4,
            epoch_peak_max: 0.99,
            flush_gen: 0,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Playing);
    assert_eq!(s.output_peak_linear, 0.30);
    assert_eq!(s.playback_peak_max_linear, 0.40);
}

#[test]
fn test_handle_thread_event_terminal_snapshot_before_drain_preserved() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        ..AudioEngineState::default()
    }));

    // Ordinary terminal order: snapshot applied, then the drained receipt.
    handle_thread_event(
        ThreadEvent::PlaybackOutputMeter {
            peak_linear: 0.62,
            clipping_detected: false,
            epoch: 5,
        },
        &state,
    );
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.62,
            flush_gen: 0,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.output_peak_linear, 0.0);
    assert_eq!(s.playback_peak_max_linear, 0.62);
}

#[test]
fn test_handle_thread_event_drained_folds_terminal_peak_above_latch() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        ..AudioEngineState::default()
    }));

    // Only the quiet window's report arrived; the loud window's report
    // dropped in transport but survives in the terminal record.
    handle_thread_event(
        ThreadEvent::PlaybackOutputMeter {
            peak_linear: 0.30,
            clipping_detected: false,
            epoch: 5,
        },
        &state,
    );
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.90,
            flush_gen: 0,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.output_peak_linear, 0.0);
    assert_eq!(s.playback_peak_max_linear, 0.90);
}

#[test]
fn test_handle_thread_event_silent_epoch_latch_stays_exact_zero() {
    // Post-Play shape: the epoch bump clears the latch (play.rs), so a
    // silent epoch must read exactly 0.0 with no leak from prior epochs.
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 6,
        playback_peak_max_linear: 0.0,
        ..AudioEngineState::default()
    }));

    for _ in 0..2 {
        handle_thread_event(
            ThreadEvent::PlaybackOutputMeter {
                peak_linear: 0.0,
                clipping_detected: false,
                epoch: 6,
            },
            &state,
        );
    }
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 6,
            epoch_peak_max: 0.0,
            flush_gen: 0,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.output_peak_linear, 0.0);
    assert_eq!(s.playback_peak_max_linear, 0.0);
}

#[test]
fn test_handle_thread_event_meters_alone_never_complete_epoch() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        ..AudioEngineState::default()
    }));

    // Completion requires the drained receipt: a dropped terminal leaves
    // the epoch Playing (the harness deadline fails loud, never a pass).
    for _ in 0..3 {
        handle_thread_event(
            ThreadEvent::PlaybackOutputMeter {
                peak_linear: 0.50,
                clipping_detected: false,
                epoch: 5,
            },
            &state,
        );
    }
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Playing);
    assert_eq!(s.playback_peak_max_linear, 0.50);
}

#[test]
fn test_handle_thread_event_stale_stats_rejected_by_epoch() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        playback_callback_count: 50,
        playback_frames_received: 100,
        playback_frames_written: 100,
        ..AudioEngineState::default()
    }));

    // Stale snapshot from the previous epoch: counters untouched.
    handle_thread_event(
        ThreadEvent::PlaybackStats {
            callback_count: 9999,
            buffer_fill_percent: 99,
            stream_error_count: 9,
            frames_received: 9999,
            frames_written: 9999,
            frames_dropped: 9,
            effective_sample_rate: 99_000,
            epoch: 4,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_callback_count, 50);
    assert_eq!(s.playback_frames_received, 100);
    assert_eq!(s.playback_frames_written, 100);
    drop(s);

    // Mis-tagged future epoch: the gate is equality, direction-agnostic.
    handle_thread_event(
        ThreadEvent::PlaybackStats {
            callback_count: 8888,
            buffer_fill_percent: 88,
            stream_error_count: 8,
            frames_received: 8888,
            frames_written: 8888,
            frames_dropped: 8,
            effective_sample_rate: 88_000,
            epoch: 6,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_callback_count, 50);
    assert_eq!(s.playback_frames_received, 100);
    assert_eq!(s.playback_frames_written, 100);
    drop(s);

    // Current epoch: applied.
    handle_thread_event(
        ThreadEvent::PlaybackStats {
            callback_count: 60,
            buffer_fill_percent: 10,
            stream_error_count: 0,
            frames_received: 150,
            frames_written: 150,
            frames_dropped: 0,
            effective_sample_rate: 48_000,
            epoch: 5,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_callback_count, 60);
    assert_eq!(s.playback_buffer_fill_percent, 10);
    assert_eq!(s.playback_stream_error_count, 0);
    assert_eq!(s.playback_frames_received, 150);
    assert_eq!(s.playback_frames_written, 150);
    assert_eq!(s.playback_frames_dropped, 0);
    assert_eq!(s.playback_effective_sample_rate, 48_000);
}

#[test]
fn test_handle_thread_event_decoder_error() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        output_peak_linear: 1.25,
        output_clipping_detected: true,
        playback_peak_max_linear: 1.25,
        ..AudioEngineState::default()
    }));

    handle_thread_event(
        ThreadEvent::DecoderError("decode failed".to_string()),
        &state,
    );

    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.last_error.as_deref(), Some("decode failed"));
    assert_eq!(s.output_peak_linear, 0.0);
    assert!(!s.output_clipping_detected);
    assert_eq!(s.playback_peak_max_linear, 1.25);
}

#[test]
fn test_handle_thread_event_thread_panic() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        output_peak_linear: 1.25,
        output_clipping_detected: true,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::ThreadPanic("playback".to_string()), &state);

    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.last_error.as_deref(), Some("Thread panicked: playback"));
    assert_eq!(s.output_peak_linear, 0.0);
    assert!(!s.output_clipping_detected);
}

#[test]
fn test_handle_thread_event_seek_complete() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        seeking: true,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::SeekComplete, &state);

    assert!(!state.load().seeking);
}

#[test]
fn test_position_update_ignored_when_seeking() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        seeking: true,
        position: 1.0,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    let s = state.load();
    assert!((s.position - 1.0).abs() < 1e-9);
}

#[test]
fn test_position_update_skips_latency_when_sample_rate_zero() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        sample_rate: 0,
        plugin_latency_samples: 4800,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PositionUpdate(5.0), &state);

    let s = state.load();
    assert!((s.position - 5.0).abs() < 1e-9);
}

#[test]
fn test_latency_update_skips_position_shift_when_sample_rate_zero() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        sample_rate: 0,
        plugin_latency_samples: 0,
        position: 10.0,
        latency_compensation_enabled: true,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::PluginLatencyUpdate(4800), &state);

    let s = state.load();
    assert_eq!(s.plugin_latency_samples, 4800);
    assert!((s.position - 10.0).abs() < 1e-9);
}

#[test]
fn test_update_engine_state_mutates_snapshot_in_place() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        sample_rate: 96_000,
        ..AudioEngineState::default()
    }));

    super::state_helpers::update_engine_state(&state, |new_state| {
        new_state.last_error = Some("boom".to_string());
        new_state.playback_state = PlaybackState::Stopped;
    });

    let s = state.load();
    assert_eq!(s.last_error.as_deref(), Some("boom"));
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    // Unrelated fields must be preserved.
    assert_eq!(s.sample_rate, 96_000);
}

#[test]
fn test_manager_thread_init_failure_records_error_without_full_state_clone() {
    // DSD bitstream required is unavailable on every platform, so
    // run_manager_thread will fail before spawning worker threads.
    let config = EngineConfig {
        dsd_output: crate::DsdOutputMode::DopRequired,
        ..EngineConfig::default()
    };

    let manager = super::ManagerThread::new(config).expect("manager thread should spawn");
    // Give the manager thread time to exit and write the error state.
    std::thread::sleep(std::time::Duration::from_millis(100));

    let state = manager.get_state();
    assert_eq!(state.playback_state, PlaybackState::Stopped);
    let error = state.last_error.expect("last_error should be set");
    assert!(
        error.contains("Engine manager exited with an error"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("DSD") || error.contains("bitstream") || error.contains("DoP"),
        "unexpected error: {error}"
    );
}

#[test]
fn correlated_manager_wait_buffers_unmatched_response() {
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let manager = super::ManagerThread {
        command_tx,
        response_inbox: std::sync::Mutex::new(super::ManagerResponseInbox {
            rx: response_rx,
            buffered: std::collections::HashMap::new(),
            abandoned: std::collections::HashSet::new(),
        }),
        next_request_id: std::sync::atomic::AtomicU64::new(3),
        state: Arc::new(ArcSwap::from_pointee(AudioEngineState::default())),
        plugin_data_cache: Arc::new(ArcSwap::from_pointee(Vec::new())),
        thread_handle: None,
    };
    response_tx
        .send(super::ManagerReply {
            id: 1,
            response: super::ManagerResponse::Error("late".to_string()),
        })
        .unwrap();
    response_tx
        .send(super::ManagerReply {
            id: 2,
            response: super::ManagerResponse::Ok,
        })
        .unwrap();

    assert!(matches!(
        manager.recv_response_for(2, std::time::Duration::from_millis(20)),
        Ok(super::ManagerResponse::Ok)
    ));
    assert!(matches!(
        manager.recv_response_for(1, std::time::Duration::from_millis(20)),
        Ok(super::ManagerResponse::Error(message)) if message == "late"
    ));
}

#[test]
fn correlated_manager_wait_discards_an_abandoned_late_response() {
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let manager = super::ManagerThread {
        command_tx,
        response_inbox: std::sync::Mutex::new(super::ManagerResponseInbox {
            rx: response_rx,
            buffered: std::collections::HashMap::new(),
            abandoned: std::collections::HashSet::new(),
        }),
        next_request_id: std::sync::atomic::AtomicU64::new(3),
        state: Arc::new(ArcSwap::from_pointee(AudioEngineState::default())),
        plugin_data_cache: Arc::new(ArcSwap::from_pointee(Vec::new())),
        thread_handle: None,
    };

    assert!(
        manager
            .recv_response_for(1, std::time::Duration::from_millis(1))
            .is_err()
    );
    response_tx
        .send(super::ManagerReply {
            id: 1,
            response: super::ManagerResponse::Error("late".to_string()),
        })
        .unwrap();
    response_tx
        .send(super::ManagerReply {
            id: 2,
            response: super::ManagerResponse::Ok,
        })
        .unwrap();

    assert!(matches!(
        manager.recv_response_for(2, std::time::Duration::from_millis(20)),
        Ok(super::ManagerResponse::Ok)
    ));
    assert!(manager.response_inbox.lock().unwrap().buffered.is_empty());
}

#[test]
fn drained_born_after_seek_complete_rejects_transport_but_folds_peak() {
    // Adversarial MPSC order: the fast SeekComplete lands first and
    // clears the indicator; the stale drain (born on the slow path
    // before the seek Flush) arrives after. A flag gate would accept
    // it here; the generation gate holds transport. Sequential
    // delivery in this order proves the gate, not the topology: any
    // MPSC interleaving is permissible input.
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        flushes_sent: 1,
        seeking: true,
        output_peak_linear: 0.30,
        playback_peak_max_linear: 0.30,
        ..AudioEngineState::default()
    }));

    handle_thread_event(ThreadEvent::SeekComplete, &state);
    assert!(!state.load().seeking);

    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.90,
            flush_gen: 0,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Playing);
    assert_eq!(s.output_peak_linear, 0.30);
    assert_eq!(s.playback_peak_max_linear, 0.90);
    drop(s);

    // The legitimate post-Flush drain completes the epoch.
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.90,
            flush_gen: 1,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.playback_peak_max_linear, 0.90);
}

#[test]
fn double_seek_gates_each_drain_by_generation() {
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        flushes_sent: 2,
        last_error: Some("previous error".to_string()),
        playback_peak_max_linear: 0.20,
        ..AudioEngineState::default()
    }));

    // Inter-Flush drain (born between the two seeks): held, folded,
    // and the pending error untouched.
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.60,
            flush_gen: 1,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Playing);
    assert_eq!(s.playback_peak_max_linear, 0.60);
    assert_eq!(s.last_error.as_deref(), Some("previous error"));
    drop(s);

    // Post-second-Flush drain: completes and clears the error.
    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.70,
            flush_gen: 2,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.playback_peak_max_linear, 0.70);
    assert!(s.last_error.is_none());
}

#[test]
fn future_generation_drain_degrades_to_epoch_gate() {
    // flush_gen above sent is impossible by construction; the gate
    // degrades to the epoch-only behavior rather than wedging.
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Playing,
        playback_epoch: 5,
        flushes_sent: 5,
        ..AudioEngineState::default()
    }));

    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.40,
            flush_gen: 7,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.playback_peak_max_linear, 0.40);
    // D3: the degradation is state-visible, not log-only.
    assert_eq!(s.gen_ahead_events, 1);
}

#[test]
fn async_decoder_error_selects_current_tag_and_drops_others() {
    // A5 identity rule, pure over a hand-fed drain: tag-equal yields
    // text in drain order; older (superseded session) and ahead
    // (impossible — adoption follows assignment) drop with a warn.
    use super::super::DecoderAsyncError;
    let errors = vec![
        DecoderAsyncError {
            attempt: 1,
            message: "old phase died".to_string(),
        },
        DecoderAsyncError {
            attempt: 2,
            message: "queue stuck".to_string(),
        },
        DecoderAsyncError {
            attempt: 9,
            message: "impossible future".to_string(),
        },
    ];
    assert_eq!(
        select_current_decoder_errors(errors, 2),
        vec!["queue stuck".to_string()]
    );
    // Empty drain selects nothing (the tick skips its store).
    assert!(select_current_decoder_errors(Vec::new(), 2).is_empty());
}

#[test]
fn async_decoder_error_applies_into_playing_and_stopped() {
    // Current-tag failure stops a Playing transport with the root
    // cause visible; into an already-Stopped transport the record is
    // informative (Stop-after-unapplied surfaces as cause).
    let mut playing = AudioEngineState {
        playback_state: PlaybackState::Playing,
        decoder_attempt: 2,
        ..AudioEngineState::default()
    };
    assert!(apply_decoder_async_errors(
        &mut playing,
        &["queue stuck".to_string()]
    ));
    assert_eq!(playing.playback_state, PlaybackState::Stopped);
    assert_eq!(playing.last_error.as_deref(), Some("queue stuck"));

    let mut stopped = AudioEngineState {
        playback_state: PlaybackState::Stopped,
        decoder_attempt: 2,
        ..AudioEngineState::default()
    };
    assert!(apply_decoder_async_errors(
        &mut stopped,
        &["late HAL failure".to_string()]
    ));
    assert_eq!(stopped.playback_state, PlaybackState::Stopped);
    assert_eq!(stopped.last_error.as_deref(), Some("late HAL failure"));

    // Nothing selected: no application, no store.
    let mut idle = AudioEngineState::default();
    assert!(!apply_decoder_async_errors(&mut idle, &[]));
    assert_eq!(idle.last_error, None);
}

#[test]
fn async_decoder_error_skips_apply_on_poisoned_transport() {
    // The tag stays current forever (no new Play while poisoned), so
    // without the guard every queued pre-death symptom would clobber
    // the death record. Poisoned: warn-logged, skipped, no store.
    let mut poisoned = AudioEngineState {
        playback_state: PlaybackState::Stopped,
        decoder_attempt: 2,
        last_error: Some("worker dead (decoder): panicked".to_string()),
        worker_death_poisoned: true,
        peak_record_complete: false,
        ..AudioEngineState::default()
    };
    assert!(!apply_decoder_async_errors(
        &mut poisoned,
        &["queue stuck".to_string()]
    ));
    assert_eq!(
        poisoned.last_error.as_deref(),
        Some("worker dead (decoder): panicked")
    );
    assert_eq!(poisoned.playback_state, PlaybackState::Stopped);
}

#[test]
fn sync_decoder_failure_chains_async_root_cause() {
    // Failed-seek-after-error composition: the sync symptom and the
    // drained async root cause stay visible together. Identity on
    // empty causes (sync-only failure keeps today's exact text).
    assert_eq!(
        chain_decoder_causes("No decoder".to_string(), &[]),
        "No decoder".to_string()
    );
    let chained = chain_decoder_causes("No decoder".to_string(), &["queue stuck".to_string()]);
    assert_eq!(
        chained,
        "No decoder (decoder stopped: queue stuck)".to_string()
    );
    let mut state = AudioEngineState {
        playback_state: PlaybackState::Playing,
        ..AudioEngineState::default()
    };
    record_decoder_error(&mut state, chained);
    assert_eq!(state.playback_state, PlaybackState::Stopped);
    assert_eq!(
        state.last_error.as_deref(),
        Some("No decoder (decoder stopped: queue stuck)")
    );
}

#[test]
fn queued_drained_after_stopped_stop_folds_peak_without_first_terminal_wins() {
    // Stop-after-EOF shape: the worker answered Stop with a partial
    // record, then the queued complete receipt arrives at Stopped.
    // First-terminal-wins would keep 0.30; max-folding recovers 0.90.
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        playback_state: PlaybackState::Stopped,
        playback_epoch: 5,
        flushes_sent: 3,
        output_peak_linear: 0.0,
        playback_peak_max_linear: 0.30,
        ..AudioEngineState::default()
    }));

    handle_thread_event(
        ThreadEvent::PlaybackDrained {
            epoch: 5,
            epoch_peak_max: 0.90,
            flush_gen: 3,
        },
        &state,
    );
    let s = state.load();
    assert_eq!(s.playback_state, PlaybackState::Stopped);
    assert_eq!(s.playback_peak_max_linear, 0.90);
    assert!(s.last_error.is_none());
}

#[test]
fn seeking_display_expires_only_past_timeout() {
    let fresh = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        seeking: true,
        seeking_since: Some(std::time::Instant::now()),
        ..AudioEngineState::default()
    }));
    expire_seeking_display(&fresh);
    assert!(fresh.load().seeking);

    let stale = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        seeking: true,
        seeking_since: Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(3))
                .unwrap(),
        ),
        ..AudioEngineState::default()
    }));
    expire_seeking_display(&stale);
    let s = stale.load();
    assert!(!s.seeking);
    assert!(s.seeking_since.is_none());
    drop(s);

    // A flag without a timestamp (deserialized legacy state) clears:
    // it cannot be a live seek interval.
    let dateless = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        seeking: true,
        seeking_since: None,
        ..AudioEngineState::default()
    }));
    expire_seeking_display(&dateless);
    assert!(!dateless.load().seeking);
}
