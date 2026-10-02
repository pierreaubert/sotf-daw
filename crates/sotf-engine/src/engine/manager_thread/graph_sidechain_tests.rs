//! Manager-route proof for graph sidechain refusal.
//!
//! A sidechain edge into a node with no key bus is refused
//! transactionally through [`apply_plugin_graph_update`]: no processing
//! host replacement, no playback reconfiguration, engine snapshot
//! preserved, and an edge-targeted diagnostic recorded. The accepted-path
//! twin render across the refusal event is proven at build level (see
//! `processing_thread::tests::sidechain_graph`), since the success path
//! blocks waiting on a live processing thread the command probes do not
//! run.

use super::super::{
    AudioEngineState, EngineOversamplingPolicy, PlaybackThread, PluginGraphConfig,
    PluginGraphEdgeConfig, PluginGraphNodeConfig, ProcessingThread,
};
use super::apply::apply_plugin_graph_update;
use arc_swap::ArcSwap;
use std::sync::Arc;

#[test]
fn refused_sidechain_graph_preserves_working_host_and_engine_snapshot() {
    let (mut processing, processing_commands) = ProcessingThread::command_probe();
    let (mut playback, playback_commands) = PlaybackThread::command_probe();
    let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
        num_channels: 4,
        playback_channels: 2,
        sample_rate: 48_000,
        plugin_latency_samples: 321,
        last_error: Some("existing device diagnostic".to_string()),
        ..AudioEngineState::default()
    }));
    let mut program_matrix = vec![0.0f32; 8];
    program_matrix[0] = 1.0;
    program_matrix[5] = 1.0;
    let mut key_matrix = vec![0.0f32; 8];
    key_matrix[2] = 1.0;
    key_matrix[7] = 1.0;
    let graph = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(
                1,
                "matrix",
                serde_json::json!({
                    "input_channels": 4,
                    "output_channels": 2,
                    "matrix": program_matrix,
                }),
                4,
            )
            .unwrap(),
            PluginGraphNodeConfig::try_new(
                2,
                "matrix",
                serde_json::json!({
                    "input_channels": 4,
                    "output_channels": 2,
                    "matrix": key_matrix,
                }),
                4,
            )
            .unwrap(),
            PluginGraphNodeConfig::try_new(3, "gain", serde_json::json!({ "gain_db": 0.0 }), 2)
                .unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 3),
            PluginGraphEdgeConfig::sidechain(2, 3),
        ],
    )
    .unwrap();

    let error = apply_plugin_graph_update(
        &mut processing,
        &mut playback,
        &state,
        graph,
        48_000,
        4,
        2,
        EngineOversamplingPolicy::PluginPreferred,
    )
    .expect_err("a sidechain edge into a bus-less node must abort the candidate");

    assert!(error.to_string().contains("sidechain"), "{error}");
    assert!(
        matches!(
            processing_commands.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ),
        "a failed candidate must not replace the processing host"
    );
    assert!(
        matches!(
            playback_commands.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ),
        "a failed candidate must not reconfigure playback"
    );
    let current = state.load();
    assert_eq!(current.num_channels, 4);
    assert_eq!(current.playback_channels, 2);
    assert_eq!(current.plugin_latency_samples, 321);
    assert_eq!(
        current.last_error.as_deref(),
        Some("existing device diagnostic")
    );
    assert_eq!(current.plugin_build_diagnostics.len(), 1);
    assert!(matches!(
        current.plugin_build_diagnostics[0].target,
        crate::PluginBuildTarget::GraphEdge {
            from_node: 2,
            to_node: 3
        }
    ));
}
