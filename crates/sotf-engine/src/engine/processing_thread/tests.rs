use super::super::PluginDataCache;
use super::super::{
    PluginConfig, PluginGraphConfig, PluginGraphEdgeConfig, PluginGraphNodeConfig,
    PreparedHostUpdate, ProcessingCommand,
};
use super::build::{build_plugin_graph_host, build_plugin_host};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::isolated::isolated_external_plugin_event;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::isolated::isolated_external_plugin_sandbox_backend;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::isolated::isolated_external_plugin_status;
use super::misc::create_plugin;
use super::misc::send_or_interrupt;
use super::processing_state::{
    ProcessingState, handle_processing_command, update_plugin_data_cache,
};
use crate::plugins::{PluginSettings, PluginType};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::{
    IsolatedExternalPluginSandboxBackend, IsolatedExternalPluginSandboxStatus,
    IsolatedExternalPluginWorkerEvent,
};
use arc_swap::ArcSwap;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use sotf_plugins::ExternalPluginProcessEvent;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use sotf_plugins::IsolatedExternalPluginWorkerReport;
use sotf_plugins::PluginHost;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use sotf_plugins::{PluginSandboxBackendCode, PluginSandboxStatusCode};

fn request(command: ProcessingCommand) -> super::ProcessingRequest {
    super::ProcessingRequest {
        id: 1,
        command,
        ticket: super::ProcessingCommandTicket::new(),
    }
}

#[test]
fn processing_scratch_prepares_order_seven_input_extent() {
    let mut state = ProcessingState::new(
        2,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    let max_frames = crate::EngineConfig::MAX_FRAME_SIZE;
    let max_channels = crate::EngineConfig::MAX_INPUT_CHANNELS;
    let expected_samples = max_frames * max_channels * 2;
    assert!(state.process_buffer.capacity() >= expected_samples);
    assert!(state.prev_process_buffer.capacity() >= expected_samples);
    assert_eq!(state.recycle_fallback_pool.len(), 4);
    assert!(
        state
            .recycle_fallback_pool
            .iter()
            .all(|buffer| buffer.capacity() >= expected_samples)
    );

    // Emulate a host replacement that widens the input before the processing
    // thread sees its first callback. All fixed scratch storage was reserved
    // from the engine-wide bound at construction time.
    let process_ptr = state.process_buffer.as_ptr();
    let previous_ptr = state.prev_process_buffer.as_ptr();
    ProcessingState::prepare_scratch_buffer(&mut state.process_buffer, expected_samples);
    ProcessingState::prepare_scratch_buffer(&mut state.prev_process_buffer, expected_samples);
    assert_eq!(state.process_buffer.as_ptr(), process_ptr);
    assert_eq!(state.prev_process_buffer.as_ptr(), previous_ptr);

    let fallback_ptrs = state
        .recycle_fallback_pool
        .iter()
        .map(Vec::as_ptr)
        .collect::<Vec<_>>();
    assert!(fallback_ptrs.iter().all(|pointer| !pointer.is_null()));

    // Run the widest frame through the current and previous host paths. The
    // previous-host buffer is sized from its host topology and input frame
    // count, so this exercises the largest crossfade scratch request too.
    *state.host = PluginHost::new(max_channels, 48_000);
    state.prev_host = Some(Box::new(PluginHost::new(max_channels, 48_000)));
    state.channels = max_channels;
    state.crossfade_progress = 0.0;
    let input = vec![0.0; max_frames * max_channels];
    let mut output = vec![0.0; max_frames * max_channels];
    assert_eq!(
        state
            .process_frame(&input, &mut output, max_frames)
            .unwrap(),
        max_frames
    );
    assert!(
        state.prev_host.is_none(),
        "full block should settle the crossfade"
    );
    assert_eq!(state.process_buffer.as_ptr(), process_ptr);
    assert_eq!(state.prev_process_buffer.as_ptr(), previous_ptr);
    assert!(state.process_buffer.capacity() >= expected_samples);
    assert!(state.prev_process_buffer.capacity() >= expected_samples);
    assert_eq!(
        state
            .recycle_fallback_pool
            .iter()
            .map(Vec::as_ptr)
            .collect::<Vec<_>>(),
        fallback_ptrs,
        "processing the maximum frame must not resize recycle fallback storage"
    );

    let recycle = state.recycle_fallback_pool.pop().unwrap();
    let recycle_ptr = recycle.as_ptr();
    let mut recycle = recycle;
    ProcessingState::prepare_scratch_buffer(&mut recycle, expected_samples);
    state.recycle_output_buffer_locally(recycle);
    assert_eq!(state.recycle_fallback_pool.len(), 4);
    assert_eq!(
        state.recycle_fallback_pool.last().unwrap().as_ptr(),
        recycle_ptr
    );
}
use std::sync::Arc;

mod crossfade_clock;
mod eos;
mod final_meter_cache;
mod frame_format;
mod misc;
mod test;

#[test]
fn test_downmix_adapts_to_current_chain_channel_count() {
    let sample_rate = 48000;
    let settings = PluginSettings::default_for(&PluginType::Downmix).unwrap();
    let config = settings.to_plugin_config(sample_rate as f64);

    let plugin = create_plugin(&config.plugin_type, &config.parameters, 10, sample_rate)
        .expect("downmix should adapt default parameters to the chain width");
    assert_eq!(plugin.input_channels(), 10);
    assert_eq!(plugin.output_channels(), 2);

    let (host, warnings) = build_plugin_host(std::slice::from_ref(&config), sample_rate, 10)
        .expect("host should load adaptive downmix");
    assert!(
        warnings.is_empty(),
        "adaptive downmix should not be skipped: {:?}",
        warnings
    );
    assert_eq!(host.output_channels(), 2);
}

#[test]
fn invalid_spectrum_analyzer_config_is_reported() {
    let config = PluginConfig::new("spectrum_analyzer", serde_json::json!("not an object"));

    let (_host, warnings) = build_plugin_host(&[config], 48_000, 2).unwrap();

    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .message
            .contains("Failed to parse spectrum analyzer params"),
        "unexpected warning: {}",
        warnings[0]
    );
}

#[test]
fn graph_build_rejects_failed_node_instead_of_returning_partial_dag() {
    let gain = PluginSettings::default_for(&PluginType::Gain)
        .unwrap()
        .to_plugin_config(48_000.0);
    let trailing_gain = PluginSettings::default_for(&PluginType::Gain)
        .unwrap()
        .to_plugin_config(48_000.0);
    let graph = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(7, gain.plugin_type, gain.parameters, 2).unwrap(),
            PluginGraphNodeConfig::try_new(
                42,
                "definitely-not-a-real-plugin",
                serde_json::json!({}),
                2,
            )
            .unwrap(),
            PluginGraphNodeConfig::try_new(
                99,
                trailing_gain.plugin_type,
                trailing_gain.parameters,
                2,
            )
            .unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(7, 42),
            PluginGraphEdgeConfig::new(42, 99),
        ],
    )
    .unwrap();

    let error = match build_plugin_graph_host(&graph, 48_000, 2) {
        Ok(_) => panic!("invalid graph node unexpectedly produced a host"),
        Err(error) => error,
    };

    assert!(matches!(
        error.target,
        crate::PluginBuildTarget::GraphNode { node_id: 42 }
    ));
    assert!(error.message.contains("node 42"), "{error}");
    assert!(
        error.message.contains("definitely-not-a-real-plugin"),
        "{error}"
    );
    assert!(error.message.contains("failed to load"), "{error}");
}

/// Test that a non-square matrix (1 input → N outputs) is NOT auto-resized.
/// This is the exact routing used by recording: mono sweep → specific output channel.
/// Regression test for the bug where the matrix was incorrectly resized to 1×1,
/// causing all sweeps to play on channel 0 regardless of output_channel.
#[test]
fn test_matrix_mono_to_multichannel_not_resized() {
    let sample_rate = 48000;

    // Simulate recording routing: mono signal → channel 1 (Right) of a stereo output
    for target_ch in 0..4 {
        let hw_channels = 4;
        let mut matrix = vec![0.0f32; hw_channels];
        matrix[target_ch] = 1.0;

        let matrix_params = serde_json::json!({
            "input_channels": 1,
            "output_channels": hw_channels,
            "matrix": matrix,
        });

        let config = PluginConfig::new("matrix", matrix_params);

        // Chain starts with 1 channel (mono WAV file)
        let (host, _warnings) = build_plugin_host(std::slice::from_ref(&config), sample_rate, 1)
            .unwrap_or_else(|e| {
                panic!(
                    "build_plugin_host failed for 1→{} matrix targeting ch{}: {}",
                    hw_channels, target_ch, e
                )
            });

        // Verify the chain expanded to the correct output channel count
        assert_eq!(
            host.output_channels(),
            hw_channels,
            "Matrix 1→{} should produce {} output channels, got {}",
            hw_channels,
            hw_channels,
            host.output_channels()
        );
    }
}

/// Test that a square matrix IS auto-resized when chain channels differ.
/// E.g., a 2×2 matrix applied to a 4-channel chain should resize to 4×4.
#[test]
fn test_matrix_square_auto_resize() {
    let sample_rate = 48000;

    // 2×2 identity matrix applied to a 4-channel chain
    let matrix_params = serde_json::json!({
        "input_channels": 2,
        "output_channels": 2,
        "matrix": [1.0, 0.0, 0.0, 1.0],
    });

    let config = PluginConfig::new("matrix", matrix_params);
    let (host, _warnings) = build_plugin_host(std::slice::from_ref(&config), sample_rate, 4)
        .expect("build_plugin_host failed for 2×2 matrix on 4ch chain");

    // Should have been resized to 4×4
    assert_eq!(
        host.output_channels(),
        4,
        "Square 2×2 matrix on 4ch chain should auto-resize to 4×4"
    );
}

/// Test that a mono→stereo matrix correctly routes signal to the target channel.
#[test]
fn test_matrix_mono_routing_signal_integrity() {
    let sample_rate = 48000;
    let num_frames = 256;

    // Route mono to channel 1 (Right) of stereo output
    let matrix_params = serde_json::json!({
        "input_channels": 1,
        "output_channels": 2,
        "matrix": [0.0, 1.0],  // silence on L, signal on R
    });

    let config = PluginConfig::new("matrix", matrix_params);
    let (mut host, _warnings) = build_plugin_host(std::slice::from_ref(&config), sample_rate, 1)
        .expect("build_plugin_host failed");

    assert_eq!(host.output_channels(), 2);

    // Mono 440Hz sine input
    let input: Vec<f32> = (0..num_frames)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sample_rate as f32).sin() * 0.5)
        .collect();

    let mut output = vec![0.0f32; num_frames * 2];
    host.process(&input, &mut output).unwrap();

    // Channel 0 (Left) should be silent
    let left_max: f32 = output
        .iter()
        .step_by(2)
        .map(|s| s.abs())
        .fold(0.0, f32::max);
    // Channel 1 (Right) should have signal
    let right_max: f32 = output
        .iter()
        .skip(1)
        .step_by(2)
        .map(|s| s.abs())
        .fold(0.0, f32::max);

    assert!(
        left_max < 1e-6,
        "Left channel should be silent but has max={}",
        left_max
    );
    assert!(
        right_max > 0.1,
        "Right channel should have signal but max={}",
        right_max
    );
}

#[test]
fn same_rate_crossfade_uses_constant_power_gains() {
    let (old_start, new_start) = ProcessingState::equal_power_crossfade_gains(0.0);
    let (old_mid, new_mid) = ProcessingState::equal_power_crossfade_gains(0.5);
    let (old_end, new_end) = ProcessingState::equal_power_crossfade_gains(1.0);

    assert!((old_start - 1.0).abs() < 1.0e-6);
    assert!(new_start.abs() < 1.0e-6);
    assert!((old_mid * old_mid + new_mid * new_mid - 1.0).abs() < 1.0e-6);
    assert!(old_end.abs() < 1.0e-6);
    assert!((new_end - 1.0).abs() < 1.0e-6);
}

#[test]
fn output_rate_changing_host_update_fades_through_silence_without_a_jump() {
    let sample_rate = 48_000;
    for reverse_rate_change in [false, true] {
        let gain = PluginConfig::new("gain", serde_json::json!({ "gain_db": 0.0 }));
        let resampler = PluginConfig::new(
            "resampler",
            serde_json::json!({
                "input_sample_rate": sample_rate,
                "output_sample_rate": 96_000,
                "chunk_size": 64
            }),
        );
        let (mut gain_host, gain_warnings) = build_plugin_host(&[gain], sample_rate, 1).unwrap();
        let (mut resampler_host, resampler_warnings) =
            build_plugin_host(&[resampler], sample_rate, 1).unwrap();
        assert!(gain_warnings.is_empty());
        assert!(resampler_warnings.is_empty());
        gain_host.build().unwrap();
        resampler_host.build().unwrap();
        let (mut old_host, new_host) = if reverse_rate_change {
            (resampler_host, gain_host)
        } else {
            (gain_host, resampler_host)
        };
        assert_ne!(
            old_host.total_latency_samples(),
            new_host.total_latency_samples()
        );
        for _ in 0..10 {
            let input = vec![1.0f32; 64];
            let old_output_frames = old_host.output_frames_for_input(64);
            let mut old_output = vec![0.0f32; old_output_frames];
            old_host.process(&input, &mut old_output).unwrap();
        }

        let mut state = ProcessingState::new(
            1,
            sample_rate,
            #[cfg(feature = "streaming")]
            None,
        );
        *state.host = old_host;
        let (response_tx, _response_rx) = std::sync::mpsc::channel();
        let (event_tx, _event_rx) = crossbeam::channel::bounded(32);

        let previous_latency = state.host.total_latency_samples();
        let prepared = PreparedHostUpdate::prepare(
            new_host,
            sample_rate,
            state.host.output_channels(),
            previous_latency,
        )
        .unwrap();
        assert!(
            !handle_processing_command(
                request(ProcessingCommand::CommitHostUpdate(prepared)),
                &mut state,
                &response_tx,
                &event_tx,
            )
            .is_shutdown()
        );
        assert!(
            state.prev_host.is_some(),
            "latency-changing update should retain the old host for a safe transition"
        );
        let mut rendered = Vec::new();
        for _ in 0..50 {
            let input = vec![1.0f32; 64];
            let output_frames = state.output_frames_for_input(64);
            let mut output = vec![0.0f32; output_frames];
            let actual = state.process_frame(&input, &mut output, 64).unwrap();
            rendered.extend_from_slice(&output[..actual]);
        }
        let max_jump = rendered
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_jump < 0.05,
            "rate-change direction reverse={reverse_rate_change} introduced an adjacent-sample jump of {max_jump}"
        );
    }
}

#[test]
fn same_rate_latency_change_prepares_an_aligned_crossfade() {
    let current = PluginConfig::new("gain", serde_json::json!({ "gain_db": 0.0 }));
    let candidate = PluginConfig::new(
        "linear_phase_eq",
        serde_json::json!({ "num_filters": 0, "fir_length_index": 2 }),
    );
    let (mut current_host, _) = build_plugin_host(&[current], 48_000, 1).unwrap();
    let (mut candidate_host, warnings) = build_plugin_host(&[candidate], 48_000, 1).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    current_host.build().unwrap();
    candidate_host.build().unwrap();
    assert_eq!(current_host.output_sample_rate(48_000), 48_000);
    assert_eq!(candidate_host.output_sample_rate(48_000), 48_000);
    assert_ne!(
        current_host.total_latency_samples(),
        candidate_host.total_latency_samples()
    );

    let mut state = ProcessingState::new(
        1,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = current_host;
    let prepared = PreparedHostUpdate::prepare(
        candidate_host,
        48_000,
        state.host.output_channels(),
        state.host.total_latency_samples(),
    )
    .unwrap();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);

    handle_processing_command(
        request(ProcessingCommand::CommitHostUpdate(prepared)),
        &mut state,
        &response_tx,
        &event_tx,
    );

    assert!(state.prev_host.is_some());
    assert!(!state.crossfade_through_silence);
    assert!(state.transition_delay_samples() > 0);
    assert!(matches!(
        response_rx.recv().unwrap().response,
        super::super::ProcessingResponse::PluginChainUpdated {
            previous_latency_samples: 0,
            latency_samples,
            latency_changed: true,
            ..
        } if latency_samples > 0
    ));
}

#[test]
fn bypass_retires_an_in_flight_crossfade() {
    let sample_rate = 48_000;
    let current = PluginConfig::new("gain", serde_json::json!({ "gain_db": 0.0 }));
    let candidate = PluginConfig::new("gain", serde_json::json!({ "gain_db": -6.0 }));
    let (mut current_host, _) = build_plugin_host(&[current], sample_rate, 2).unwrap();
    let (mut candidate_host, _) = build_plugin_host(&[candidate], sample_rate, 2).unwrap();
    current_host.build().unwrap();
    candidate_host.build().unwrap();

    let mut state = ProcessingState::new(
        2,
        sample_rate,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = current_host;
    let prepared = PreparedHostUpdate::prepare(candidate_host, sample_rate, 2, 0).unwrap();
    let (response_tx, _response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);

    handle_processing_command(
        request(ProcessingCommand::CommitHostUpdate(prepared)),
        &mut state,
        &response_tx,
        &event_tx,
    );
    assert!(state.prev_host.is_some());

    handle_processing_command(
        request(ProcessingCommand::Bypass(true)),
        &mut state,
        &response_tx,
        &event_tx,
    );

    assert!(state.bypassed);
    assert!(state.prev_host.is_none());
    assert_eq!(state.crossfade_progress, 1.0);
    assert_eq!(state.transition_delay_samples(), 0);
}

#[test]
fn bypass_preserves_frames_across_channel_changing_host() {
    let config = PluginConfig::new("upmixer", serde_json::json!({"speaker_config": "5.0"}));
    let (mut host, _) = build_plugin_host(&[config], 48_000, 2).unwrap();
    host.build().unwrap();
    let mut state = ProcessingState::new(
        2,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = host;
    state.channels = 5;
    state.bypassed = true;

    let mut output = [99.0; 10];
    assert_eq!(
        state
            .process_frame(&[1.0, 2.0, 3.0, 4.0], &mut output, 2)
            .unwrap(),
        2
    );
    assert_eq!(output, [1.0, 2.0, 0.0, 0.0, 0.0, 3.0, 4.0, 0.0, 0.0, 0.0]);
}

#[test]
fn live_structural_parameter_is_rejected_before_mutation() {
    let config = PluginConfig::new("upmixer", serde_json::json!({"speaker_config": "5.0"}));
    let (mut host, _) = build_plugin_host(&[config], 48_000, 2).unwrap();
    host.build().unwrap();
    let mut state = ProcessingState::new(
        2,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = host;
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);

    handle_processing_command(
        request(ProcessingCommand::SetParameter {
            plugin_index: 0,
            param_id: "speaker_config".to_string(),
            value: "3".to_string(),
        }),
        &mut state,
        &response_tx,
        &event_tx,
    );

    assert!(matches!(
        response_rx.recv().unwrap().response,
        super::super::ProcessingResponse::Error(message)
            if message.contains("requires rebuilding")
    ));
    assert_eq!(state.host.output_channels(), 5);
}

#[test]
fn prepared_host_update_rejects_a_stale_base_without_replacing_the_working_host() {
    let sample_rate = 48_000;
    let current = PluginConfig::new("gain", serde_json::json!({ "gain_db": 0.0 }));
    let candidate = PluginConfig::new("gain", serde_json::json!({ "gain_db": -6.0 }));
    let (mut current_host, _) = build_plugin_host(&[current], sample_rate, 2).unwrap();
    let (mut candidate_host, _) = build_plugin_host(&[candidate], sample_rate, 2).unwrap();
    current_host.build().unwrap();
    candidate_host.build().unwrap();

    let prepared = PreparedHostUpdate::prepare(candidate_host, sample_rate, 2, 123)
        .expect("candidate preparation itself should succeed");
    let mut state = ProcessingState::new(
        2,
        sample_rate,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = current_host;
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);

    handle_processing_command(
        request(ProcessingCommand::CommitHostUpdate(prepared)),
        &mut state,
        &response_tx,
        &event_tx,
    );

    assert_eq!(state.host.total_latency_samples(), 0);
    assert!(matches!(
        response_rx.recv().unwrap().response,
        super::super::ProcessingResponse::Error(message)
            if message.contains("stale prepared host update")
    ));
}

#[test]
fn cancelled_prepared_host_update_cannot_commit() {
    let sample_rate = 48_000;
    let current = PluginConfig::new("gain", serde_json::json!({ "gain_db": 0.0 }));
    let candidate = PluginConfig::new("gain", serde_json::json!({ "gain_db": -6.0 }));
    let (mut current_host, _) = build_plugin_host(&[current], sample_rate, 2).unwrap();
    let (mut candidate_host, _) = build_plugin_host(&[candidate], sample_rate, 2).unwrap();
    current_host.build().unwrap();
    candidate_host.build().unwrap();
    let prepared = PreparedHostUpdate::prepare(candidate_host, sample_rate, 2, 0).unwrap();
    assert!(prepared.ticket().cancel());

    let mut state = ProcessingState::new(
        2,
        sample_rate,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = current_host;
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    handle_processing_command(
        request(ProcessingCommand::CommitHostUpdate(prepared)),
        &mut state,
        &response_tx,
        &event_tx,
    );

    assert!(matches!(
        response_rx.recv().unwrap().response,
        super::super::ProcessingResponse::Error(message) if message.contains("cancelled")
    ));
}

#[test]
fn prepared_host_update_owns_analyzer_cache_storage_before_commit() {
    let config = PluginConfig::new("spectrum_analyzer", serde_json::json!(null));
    let (mut host, warnings) = build_plugin_host(&[config], 48_000, 2).unwrap();
    assert!(warnings.is_empty());
    host.build().unwrap();

    let prepared = PreparedHostUpdate::prepare(host, 48_000, 2, 0).unwrap();
    assert_eq!(prepared.prepared_analyzer_slots(), 1);
}

#[test]
fn processing_hot_path_uses_prepared_buffers_for_output_and_crossfade() {
    let source = include_str!("../processing_thread.rs");

    assert!(
        !source.contains(concat!("process_buffer.", "resize(output_samples, 0.0)")),
        "processing thread must not allocate/resize the process buffer in the frame hot path"
    );
    assert!(
        !source.contains(concat!("prev_process_buffer.", "resize(buf_len, 0.0)")),
        "crossfade processing must not allocate/resize the previous-host buffer in process_frame"
    );
}

#[test]
fn send_or_interrupt_delivers_message_when_buffer_has_space() {
    let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(4);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<super::ProcessingRequest>();

    let handle = std::thread::spawn(move || send_or_interrupt(&tx, &cmd_rx, 42));

    let result = handle.join().expect("thread panicked");
    assert!(result.is_ok());
    assert!(result.unwrap().is_none()); // No interruption
    assert_eq!(rx.recv().unwrap(), 42);
    drop(cmd_tx); // keep cmd_tx alive until assertion
}

#[test]
fn send_or_interrupt_returns_command_when_interrupted_during_backpressure() {
    // Buffer capacity 1, pre-fill it so the next send blocks
    let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(1);
    tx.send(99).unwrap(); // Fill the buffer

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<super::ProcessingRequest>();

    // Send a command that will be found during the backpressure retry
    cmd_tx.send(request(ProcessingCommand::Stop)).unwrap();

    let handle = std::thread::spawn(move || send_or_interrupt(&tx, &cmd_rx, 42));

    let result = handle.join().expect("thread panicked");
    let (cmd, unsent_msg) = result.unwrap().expect("should have been interrupted");
    assert!(matches!(cmd.command, ProcessingCommand::Stop));
    assert_eq!(unsent_msg.unwrap(), 42); // Message returned, not lost
    assert_eq!(rx.recv().unwrap(), 99); // Original message still in buffer
}

#[test]
fn send_or_interrupt_errors_when_channel_disconnected() {
    let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(4);
    let (_cmd_tx, cmd_rx) = std::sync::mpsc::channel::<super::ProcessingRequest>();
    drop(rx); // Disconnect the receiver

    let result = send_or_interrupt(&tx, &cmd_rx, 42);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("disconnected"));
}

#[test]
fn correlated_processing_wait_buffers_unmatched_response() {
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let processing = super::ProcessingThread {
        command_tx,
        response_inbox: std::sync::Mutex::new(super::ProcessingResponseInbox {
            rx: response_rx,
            buffered: std::collections::HashMap::new(),
            abandoned: std::collections::HashSet::new(),
        }),
        request_tickets: std::sync::Mutex::new(std::collections::HashMap::new()),
        next_request_id: std::sync::atomic::AtomicU64::new(3),
        thread_handle: None,
        host_generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };
    response_tx
        .send(super::ProcessingReply {
            id: 1,
            response: super::super::ProcessingResponse::Error("late".to_string()),
        })
        .unwrap();
    response_tx
        .send(super::ProcessingReply {
            id: 2,
            response: super::super::ProcessingResponse::Ok,
        })
        .unwrap();

    assert!(matches!(
        processing.try_recv_response_for(2),
        Some(super::super::ProcessingResponse::Ok)
    ));
    assert!(matches!(
        processing.try_recv_response_for(1),
        Some(super::super::ProcessingResponse::Error(message)) if message == "late"
    ));
}

#[test]
fn cancelled_processing_request_is_rejected_before_mutation() {
    let mut state = ProcessingState::new(
        2,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    let ticket = super::ProcessingCommandTicket::new();
    assert!(ticket.cancel());

    let shutdown = handle_processing_command(
        super::ProcessingRequest {
            id: 7,
            command: ProcessingCommand::Bypass(true),
            ticket,
        },
        &mut state,
        &response_tx,
        &event_tx,
    )
    .is_shutdown();

    assert!(!shutdown);
    assert!(!state.bypassed);
    assert!(matches!(
        response_rx.recv().unwrap().response,
        super::super::ProcessingResponse::Error(message) if message.contains("cancelled")
    ));
}

#[test]
fn audio_frame_new_enforces_data_length_invariant() {
    use crate::AudioFrame;

    // Valid: data.len() == num_frames * num_channels
    let frame = AudioFrame::new(vec![0.0; 2048], 1024, 2, 48000);
    assert_eq!(frame.num_samples(), 2048);
    assert_eq!(frame.num_frames, 1024);
    assert_eq!(frame.num_channels, 2);
}

#[test]
fn audio_frame_silent_produces_all_zeros() {
    use crate::AudioFrame;

    let frame = AudioFrame::silent(512, 6, 48000);
    assert_eq!(frame.data.len(), 512 * 6);
    assert!(frame.data.iter().all(|&s| s == 0.0));
}

#[test]
fn audio_frame_clear_resets_to_silence() {
    use crate::AudioFrame;

    let mut frame = AudioFrame::new(vec![1.0; 1024], 512, 2, 48000);
    assert!(frame.data.iter().all(|&s| s == 1.0));

    frame.clear();
    assert!(frame.data.iter().all(|&s| s == 0.0));
    // Metadata unchanged
    assert_eq!(frame.num_frames, 512);
    assert_eq!(frame.num_channels, 2);
}

#[test]
fn audio_frame_invariants_across_channel_counts() {
    use crate::AudioFrame;

    for channels in [1, 2, 4, 6, 8] {
        let frames = 256;
        let total = frames * channels;
        let data: Vec<f32> = (0..total).map(|i| i as f32 / total as f32).collect();
        let frame = AudioFrame::new(data, frames, channels, 48000);

        assert_eq!(frame.num_samples(), total);
        assert_eq!(frame.data.len(), total);
        // All samples in [-1, 1) range for this test data
        assert!(frame.data.iter().all(|&s| (0.0..1.0).contains(&s)));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn test_isolated_external_plugin_event_and_status_mappings() {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;

    let event = isolated_external_plugin_event(ExternalPluginProcessEvent::AlreadyRunning);
    assert!(matches!(
        event,
        IsolatedExternalPluginWorkerEvent::AlreadyRunning
    ));

    let event = isolated_external_plugin_event(ExternalPluginProcessEvent::NotRunning);
    assert!(matches!(
        event,
        IsolatedExternalPluginWorkerEvent::NotRunning
    ));

    let event = isolated_external_plugin_event(ExternalPluginProcessEvent::Started { pid: 555 });
    assert!(matches!(
        event,
        IsolatedExternalPluginWorkerEvent::Started { pid } if pid == 555
    ));

    #[cfg(unix)]
    {
        let status = ExitStatusExt::from_raw(11 << 8);
        let event = isolated_external_plugin_event(ExternalPluginProcessEvent::Exited { status });
        assert!(matches!(
            event,
            IsolatedExternalPluginWorkerEvent::Exited {
                exit_code: Some(11)
            }
        ));
    }
    #[cfg(windows)]
    {
        let status = ExitStatusExt::from_raw((11 << 8) as u32);
        let event = isolated_external_plugin_event(ExternalPluginProcessEvent::Exited { status });
        assert!(matches!(
            event,
            IsolatedExternalPluginWorkerEvent::Exited {
                exit_code: Some(11)
            }
        ));
    }

    let report = IsolatedExternalPluginWorkerReport {
        plugin_index: 3,
        node_id: 9,
        plugin_instance_id: Some(77),
        event: Some(ExternalPluginProcessEvent::Started { pid: 777 }),
        error: Some("blocked".into()),
        worker_start_count: 4,
        worker_exit_count: 2,
        worker_launch_failure_count: 1,
        worker_quarantined: true,
        worker_quarantine_reason: Some("quarantined".into()),
        block_timeout_count: 3,
        block_worker_failure_count: 4,
        block_wrong_sequence_count: 5,
        sandbox_status: PluginSandboxStatusCode::Enforced,
        sandbox_backend: PluginSandboxBackendCode::LinuxLandlock,
        sandbox_reason: None,
    };
    let status = isolated_external_plugin_status(report);
    assert_eq!(status.plugin_index, 3);
    assert_eq!(status.node_id, 9);
    assert_eq!(status.plugin_instance_id, Some(77));
    assert_eq!(status.error, Some("blocked".into()));
    assert_eq!(status.worker_start_count, 4);
    assert_eq!(status.worker_exit_count, 2);
    assert_eq!(status.worker_launch_failure_count, 1);
    assert!(status.worker_quarantined);
    assert_eq!(status.worker_quarantine_reason, Some("quarantined".into()));
    assert_eq!(status.block_timeout_count, 3);
    assert_eq!(status.block_worker_failure_count, 4);
    assert_eq!(status.block_wrong_sequence_count, 5);
    assert_eq!(
        status.sandbox_status,
        IsolatedExternalPluginSandboxStatus::Enforced
    );
    assert_eq!(
        status.sandbox_backend,
        IsolatedExternalPluginSandboxBackend::LinuxLandlock
    );
    assert_eq!(status.sandbox_reason, None);
    assert!(matches!(
        status.event,
        Some(IsolatedExternalPluginWorkerEvent::Started { pid }) if pid == 777
    ));
    assert_eq!(
        isolated_external_plugin_sandbox_backend(PluginSandboxBackendCode::MacosAppSandboxHelper),
        IsolatedExternalPluginSandboxBackend::MacosAppSandboxHelper
    );
}

/// Regression test for the analyzer-cache fallback allocation path.
///
/// When a UI reader holds the current cache Arc, the processing thread must
/// skip the update rather than count the contention as a fallback allocation.
#[test]
fn plugin_cache_update_skips_under_ui_contention_without_fallback() {
    let sample_rate = 48_000;
    let config = PluginConfig::new("spectrum_analyzer", serde_json::json!(null));
    let (mut host, warnings) = build_plugin_host(std::slice::from_ref(&config), sample_rate, 2)
        .expect("spectrum analyzer host should build");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    host.build().expect("host should build");
    assert!(
        !host.analyzer_indices().is_empty(),
        "spectrum analyzer must register as an analyzer"
    );

    let plugin_count = host.plugin_count();

    let mut state = ProcessingState::new(
        host.output_channels(),
        sample_rate,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = host;

    // Pre-size both the published cache and the spare so no bootstrap resize
    // is needed. This mirrors what PreparedHostUpdate does off-thread.
    let plugin_data_cache: PluginDataCache =
        Arc::new(ArcSwap::from_pointee(vec![None; plugin_count]));
    state.spare_cache_arc = Some(Arc::new(vec![None; plugin_count]));

    // Simulate a UI reader that keeps a clone of the current cache Arc.
    let _ui_holder = Arc::clone(&*plugin_data_cache.load());

    // Run enough updates that some will hit contention (the UI holds the Arc
    // that becomes the spare after the first successful swap).
    let mut updated_frames = 0;
    for _ in 0..20 {
        if update_plugin_data_cache(&mut state, &plugin_data_cache) {
            updated_frames += 1;
        }
    }

    assert!(
        updated_frames > 0,
        "at least one cache update should succeed before the UI clone causes contention"
    );
    assert!(
        updated_frames < 20,
        "some updates should be skipped due to simulated UI contention"
    );
    assert_eq!(
        state.cache_fallback_count, 0,
        "UI contention must not be reported as a cache fallback allocation"
    );
}

/// If the spare cache Arc is unexpectedly missing, the processing hot path
/// must skip this frame rather than allocate a replacement cache.
#[test]
fn plugin_cache_update_skips_missing_spare_without_allocating() {
    let sample_rate = 48_000;
    let config = PluginConfig::new("spectrum_analyzer", serde_json::json!(null));
    let (mut host, warnings) = build_plugin_host(std::slice::from_ref(&config), sample_rate, 2)
        .expect("spectrum analyzer host should build");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    host.build().expect("host should build");

    let plugin_count = host.plugin_count();

    let mut state = ProcessingState::new(
        host.output_channels(),
        sample_rate,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = host;
    state.spare_cache_arc = None;

    let plugin_data_cache: PluginDataCache =
        Arc::new(ArcSwap::from_pointee(vec![None; plugin_count]));

    assert!(!update_plugin_data_cache(&mut state, &plugin_data_cache));
    assert_eq!(state.cache_fallback_count, 1);
    assert!(state.spare_cache_arc.is_none());
}

#[test]
fn processing_thread_idle_wait_blocks_instead_of_micro_spinning() {
    let source = include_str!("processing_state.rs");
    assert!(
        source.contains("let mut decoder_stream_active = true"),
        "processing thread should start in low-latency mode before the first decoded frame"
    );
    assert!(
        source.contains("IDLE_EMPTY_SLEEP_PROCESSING_MS"),
        "processing thread should use a coarser wait after the decoder has gone idle"
    );
    assert!(
        source.contains("recv_timeout"),
        "processing thread should wake immediately when decoder frames arrive"
    );
    assert!(
        !source.contains("TryRecvError::Empty"),
        "processing thread must not sleep after an empty try_recv"
    );
}

#[test]
fn recycle_queue_prefill_is_generous() {
    let source = include_str!("../manager_thread/config_update_queue.rs");
    assert!(
        source.contains("queue_capacity * 4"),
        "recycle queues should be pre-filled with a generous multiple of queue capacity"
    );
    assert!(
        source.contains("frame_size * 64"),
        "recycle buffers should be sized for high channel counts and resampler headroom"
    );
}

type HissSnapshot = sotf_plugins::plugin_hiss_reducer::snapshot::ProfileSnapshot;

fn hiss_command_state(params: serde_json::Value) -> ProcessingState {
    let config = PluginConfig::new("hiss_reducer", params);
    let (mut host, warnings) = build_plugin_host(&[config], 48_000, 1)
        .expect("hiss host must build");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    host.build().expect("host must build");
    let mut state = ProcessingState::new(
        1,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = host;
    state
}

fn hiss_send_set_parameter(
    state: &mut ProcessingState,
    param_id: &str,
    value: &str,
) -> super::super::ProcessingResponse {
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    handle_processing_command(
        request(ProcessingCommand::SetParameter {
            plugin_index: 0,
            param_id: param_id.to_string(),
            value: value.to_string(),
        }),
        state,
        &response_tx,
        &event_tx,
    );
    response_rx.recv().expect("handler must reply").response
}

fn hiss_command_snapshot(host: &PluginHost) -> std::sync::Arc<HissSnapshot> {
    sotf_plugins::Host::get_plugin_data(host, 0)
        .expect("host must transport Hiss snapshot")
        .downcast::<HissSnapshot>()
        .expect("snapshot must downcast")
}

fn hiss_command_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

fn hiss_command_hiss(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let white = hiss_command_noise(frames, 1.0, seed);
    let mut previous = 0.0f32;
    white
        .iter()
        .map(|&sample| {
            let high_pass = amplitude * (sample - previous);
            previous = sample;
            high_pass
        })
        .collect()
}

fn hiss_command_render(
    host: &mut PluginHost,
    input: &[f32],
    block: usize,
) -> Vec<f32> {
    let mut rendered = Vec::with_capacity(input.len());
    for chunk in input.chunks(block) {
        let mut output = vec![0.0f32; chunk.len()];
        let frames = host.process(chunk, &mut output).expect("host must process");
        assert_eq!(frames, chunk.len());
        rendered.extend_from_slice(&output);
    }
    rendered
}

fn hiss_command_drain(host: &mut PluginHost) -> Vec<f32> {
    let mut output = Vec::new();
    let capacity = host.drain_output_frames_max().max(1);
    for _ in 0..4096 {
        let mut block = vec![0.0f32; capacity];
        let status = host.drain(&mut block).expect("host must drain");
        output.extend_from_slice(&block[..status.frames]);
        if status.complete {
            return output;
        }
    }
    panic!("host drain did not complete");
}

#[test]
fn engine_set_parameter_drives_hiss_capture_cancel_and_clear() {
    let mut state = hiss_command_state(serde_json::json!({}));
    let snapshot = hiss_command_snapshot(&state.host);
    let gen_empty = snapshot.try_status().expect("status").generation;
    assert!(snapshot.try_export().expect("export").is_none());

    // Start through production submission, string parsing, and response.
    let response = hiss_send_set_parameter(&mut state, "learn_noise", "true");
    assert!(
        matches!(
            response,
            super::super::ProcessingResponse::ParameterUpdated {
                output_channels: 1,
                ..
            }
        ),
        "start must acknowledge with updated metadata"
    );
    assert_eq!(snapshot.capture_state(), (true, 0.0));

    let colored = hiss_command_hiss(48_000, 0.035, 0xe940c);
    hiss_command_render(&mut state.host, &colored[..12288], 4096);
    let (active, progress) = snapshot.capture_state();
    assert!(active);
    assert_eq!(progress, 12288f32 / 48000f32);
    assert!(snapshot.try_export().expect("export").is_none());
    assert_eq!(snapshot.try_status().expect("status").generation, gen_empty);

    hiss_command_render(&mut state.host, &colored[12288..], 4096);
    assert_eq!(snapshot.capture_state(), (false, 0.0));
    let done = snapshot
        .try_export()
        .expect("export")
        .expect("capture must export");
    assert_eq!(done.profile.format_version, 2);
    assert!(done.generation > gen_empty);

    // Restart then cancel: the prior accepted history survives intact.
    let accepted = done.profile.clone();
    let response = hiss_send_set_parameter(&mut state, "learn_noise", "true");
    assert!(matches!(
        response,
        super::super::ProcessingResponse::ParameterUpdated { .. }
    ));
    hiss_command_render(&mut state.host, &colored[..8192], 4096);
    assert!(snapshot.capture_state().0);
    let during = snapshot
        .try_export()
        .expect("export")
        .expect("prior stays readable");
    assert_eq!(during.profile, accepted);
    let response = hiss_send_set_parameter(&mut state, "learn_noise", "false");
    assert!(matches!(
        response,
        super::super::ProcessingResponse::ParameterUpdated { .. }
    ));
    assert_eq!(snapshot.capture_state(), (false, 0.0));
    let kept = snapshot
        .try_export()
        .expect("export")
        .expect("cancel keeps prior");
    assert_eq!(kept.profile, accepted);

    // Deliberate clear drops the accepted profile through the same path.
    let response = hiss_send_set_parameter(&mut state, "clear_profile", "true");
    assert!(matches!(
        response,
        super::super::ProcessingResponse::ParameterUpdated { .. }
    ));
    assert!(snapshot.try_export().expect("export").is_none());

    // Structural rebuild policy still refuses through production handling.
    let response = hiss_send_set_parameter(&mut state, "spectral_mode", "true");
    assert!(matches!(
        response,
        super::super::ProcessingResponse::Error(message)
            if message.contains("requires rebuilding")
    ));
}

#[test]
fn engine_set_parameter_hiss_carrier_rebuild_matches_audio_and_eof() {
    let mut state = hiss_command_state(serde_json::json!({
        "spectral_mode": true,
        "strength": 0.85,
    }));
    let response = hiss_send_set_parameter(&mut state, "learn_noise", "true");
    assert!(matches!(
        response,
        super::super::ProcessingResponse::ParameterUpdated { .. }
    ));
    let colored = hiss_command_hiss(48_000, 0.035, 0xe940c);
    hiss_command_render(&mut state.host, &colored, 4096);
    let snapshot = hiss_command_snapshot(&state.host);
    let live = snapshot
        .try_export()
        .expect("export")
        .expect("live capture");
    assert_eq!(live.profile.format_version, 2);
    let response = hiss_send_set_parameter(&mut state, "use_captured_profile", "true");
    assert!(matches!(
        response,
        super::super::ProcessingResponse::ParameterUpdated { .. }
    ));
    state.host.reset();

    let hiss = hiss_command_noise(16384, 0.04, 0xe9f1);
    let tone = hiss_command_noise(16384, 0.06, 0x51ab);
    let mix: Vec<f32> = hiss
        .iter()
        .zip(tone.iter())
        .map(|(first, second)| first + second)
        .collect();
    let mut original = hiss_command_render(&mut state.host, &mix, 4096);
    original.extend(hiss_command_drain(&mut state.host));
    assert!(original.iter().all(|sample| sample.is_finite()));
    assert_ne!(
        original[1024..16384],
        mix[0..15360],
        "engaged capture must process audio"
    );

    // Typed carrier round-trip from the live export; never handcrafted.
    let mut settings =
        PluginSettings::default_for(&PluginType::HissReducer).expect("hiss settings");
    if let PluginSettings::HissReducer {
        captured_profile,
        use_captured_profile,
        spectral_mode,
        strength,
        ..
    } = &mut settings
    {
        *captured_profile = Some(live.profile.clone());
        *use_captured_profile = true;
        *spectral_mode = true;
        *strength = 0.85;
    } else {
        panic!("expected HissReducer settings");
    }
    let json = serde_json::to_value(&settings).expect("encode");
    assert!(json["HissReducer"].get("captured_profile").is_some());
    let restored: PluginSettings = serde_json::from_value(json).expect("decode");
    let config = restored.to_plugin_config(48_000.0);
    assert!(config.parameters.get("captured_profile").is_some());
    assert!(config.parameters.get("learn_noise").is_none());
    assert!(config.parameters.get("clear_profile").is_none());

    // Fresh rebuild from the converted carrier through the same builder.
    let (mut rebuilt_host, warnings) =
        build_plugin_host(std::slice::from_ref(&config), 48_000, 1).expect("rebuild");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    rebuilt_host.build().expect("rebuilt host must build");
    let rebuilt_snapshot = hiss_command_snapshot(&rebuilt_host);
    let rebuilt_export = rebuilt_snapshot
        .try_export()
        .expect("export")
        .expect("rebuilt carrier");
    assert_eq!(rebuilt_export.profile, live.profile);

    let mut rebuilt = hiss_command_render(&mut rebuilt_host, &mix, 4096);
    rebuilt.extend(hiss_command_drain(&mut rebuilt_host));
    assert!(rebuilt.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        original, rebuilt,
        "typed-carrier rebuild must match live audio and EOF bit-exactly"
    );
}
