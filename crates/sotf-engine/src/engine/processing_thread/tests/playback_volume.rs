//! Host attenuation survives live changes and prepared chain replacement.

use super::super::build::{build_plugin_graph_host, build_plugin_host};
use super::{ProcessingState, handle_processing_command, request};
use crate::engine::{
    PluginConfig, PluginGraphConfig, PluginGraphEdgeConfig, PluginGraphNodeConfig,
    PreparedHostUpdate, ProcessingCommand, ProcessingResponse,
};
use sotf_plugins::PluginHost;
use sotf_plugins::parameters::{ParameterId, ParameterValue};

fn host() -> PluginHost {
    let config = PluginConfig::new(
        "loudness_compensation",
        serde_json::json!({
            "mode": 2, "auto_calibrated": true, "playback_volume_db": -33.0
        }),
    );
    let (mut host, warnings) = build_plugin_host(&[config], 48_000, 2).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    host.build().unwrap();
    host
}

fn send(command: ProcessingCommand, state: &mut ProcessingState) -> ProcessingResponse {
    let (tx, rx) = std::sync::mpsc::channel();
    let (events, _receiver) = crossbeam::channel::bounded(4);
    assert!(!handle_processing_command(request(command), state, &tx, &events).is_shutdown());
    rx.recv().unwrap().response
}

fn attenuation(host: &PluginHost) -> f32 {
    match host
        .get_plugin(0)
        .unwrap()
        .get_parameter(&ParameterId::from("playback_volume_db"))
    {
        Some(ParameterValue::Float(value)) => value,
        other => panic!("missing host attenuation: {other:?}"),
    }
}

#[test]
fn playback_volume_commands_reach_replacement_and_crossfade_hosts() {
    let mut state = ProcessingState::new(
        2,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = host();
    assert!(matches!(
        send(ProcessingCommand::SetPlaybackVolume(0.25), &mut state),
        ProcessingResponse::Ok
    ));
    let expected = -12.0412;
    assert!((attenuation(&state.host) - expected).abs() < 0.0001);
    let update =
        PreparedHostUpdate::prepare(host(), 48_000, 2, state.host.total_latency_samples()).unwrap();
    assert!(matches!(
        send(ProcessingCommand::CommitHostUpdate(update), &mut state),
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    assert!((attenuation(&state.host) - expected).abs() < 0.0001);
    assert!((attenuation(state.prev_host.as_ref().unwrap()) - expected).abs() < 0.0001);
    for (volume, expected) in [(0.0, -80.0), (1.0, 0.0), (0.1, -20.0)] {
        assert!(matches!(
            send(ProcessingCommand::SetPlaybackVolume(volume), &mut state),
            ProcessingResponse::Ok
        ));
        assert!((attenuation(&state.host) - expected).abs() < 0.0001);
        assert!((attenuation(state.prev_host.as_ref().unwrap()) - expected).abs() < 0.0001);
    }
    for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(matches!(
            send(ProcessingCommand::SetPlaybackVolume(invalid), &mut state),
            ProcessingResponse::Error(_)
        ));
        assert!((attenuation(&state.host) + 20.0).abs() < 0.0001);
        assert!((attenuation(state.prev_host.as_ref().unwrap()) + 20.0).abs() < 0.0001);
    }
}

fn graph_host() -> PluginHost {
    let graph = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(7, "loudness_compensation",
                serde_json::json!({"mode": 2, "auto_calibrated": true, "playback_volume_db": -33.0}), 2).unwrap(),
            PluginGraphNodeConfig::try_new(42, "loudness_compensation",
                serde_json::json!({"mode": 2, "auto_calibrated": true, "playback_volume_db": -4.0}), 2).unwrap(),
            PluginGraphNodeConfig::try_new(99, "gain",
                serde_json::json!({"gain_db": -3.0}), 2).unwrap(),
        ],
        vec![PluginGraphEdgeConfig::new(7, 42), PluginGraphEdgeConfig::new(42, 99)],
    ).unwrap();
    let (host, warnings) = build_plugin_graph_host(&graph, 48_000, 2).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    host
}

fn assert_graph_attenuation(host: &PluginHost, expected: f32) {
    assert_eq!(host.plugin_count(), 3);
    for index in [0, 1] {
        let plugin = host.get_plugin(index).unwrap();
        let parameter = plugin.get_parameter(&ParameterId::from("playback_volume_db"));
        assert_eq!(parameter, Some(ParameterValue::Float(expected)));
        let snapshot = plugin.get_data().unwrap();
        let data = snapshot
            .downcast_ref::<sotf_plugins::plugin_loudness_compensation::LoudnessCompensationData>()
            .unwrap();
        assert_eq!(data.playback_volume_db, expected);
    }
    assert_eq!(
        host.get_plugin(2)
            .unwrap()
            .get_parameter(&ParameterId::from("gain_db")),
        Some(ParameterValue::Float(-3.0))
    );
}

#[test]
fn playback_volume_reaches_all_sparse_graph_nodes_and_rebuilt_telemetry() {
    let mut state = ProcessingState::new(
        2,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = graph_host();
    assert!(matches!(
        send(ProcessingCommand::SetPlaybackVolume(0.1), &mut state),
        ProcessingResponse::Ok
    ));
    assert_graph_attenuation(&state.host, -20.0);
    let update =
        PreparedHostUpdate::prepare(graph_host(), 48_000, 2, state.host.total_latency_samples())
            .unwrap();
    assert!(matches!(
        send(ProcessingCommand::CommitHostUpdate(update), &mut state),
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    assert_graph_attenuation(&state.host, -20.0);
    assert_graph_attenuation(state.prev_host.as_ref().unwrap(), -20.0);
    assert!(matches!(
        send(ProcessingCommand::SetPlaybackVolume(0.0), &mut state),
        ProcessingResponse::Ok
    ));
    assert_graph_attenuation(&state.host, -80.0);
    assert_graph_attenuation(state.prev_host.as_ref().unwrap(), -80.0);
}
