use super::super::{AudioEngineState, PlaybackThread, PreparedHostUpdate, ProcessingThread};
use super::config_update_queue::ConfigUpdateQueue;
use super::error::ConfigError;
use super::estimate::estimate_graph_update_timeout;
use super::estimate::estimate_update_timeout;
use super::wait::wait_for_plugin_chain_update;
use crate::engine::processing_thread::{
    build_plugin_graph_host_with_policy, build_plugin_host_with_policy,
};
use crate::{EngineOversamplingPolicy, PluginBuildDiagnostic};
use arc_swap::ArcSwap;
use sotf_plugins::PluginHost;
use std::sync::Arc;

const HOST_BUILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

// The processing thread validates the host snapshot at the block boundary.
// A host update from another manager-side source can win between the state
// snapshot and that commit; rebuild against the new snapshot a small number
// of times instead of surfacing this recoverable race to the caller.
const MAX_STALE_HOST_UPDATE_RETRIES: usize = 2;

fn is_required_external_update_failure(diagnostic: &PluginBuildDiagnostic) -> bool {
    matches!(
        &diagnostic.target,
        crate::PluginBuildTarget::ChainPlugin { .. }
    ) && diagnostic
        .plugin_type
        .as_deref()
        .is_some_and(|plugin_type| {
            plugin_type.eq_ignore_ascii_case("external")
                || plugin_type.eq_ignore_ascii_case("external_plugin")
        })
}

fn is_stale_host_update(error: &ConfigError) -> bool {
    matches!(
        error,
        ConfigError::ProcessingError { reason }
            if reason == "stale prepared host update: active host changed before commit"
    )
}

fn build_plugin_update_host_on_worker(
    plugins: Vec<super::super::PluginConfig>,
    sample_rate: u32,
    input_channels: usize,
    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("sotf-plugin-host-builder".to_string())
        .spawn(move || {
            let result = build_plugin_host_with_policy(
                &plugins,
                sample_rate,
                input_channels,
                oversampling_policy,
            );
            result_tx.send(result).ok();
        })
        .map_err(|e| {
            PluginBuildDiagnostic::host(format!("failed to spawn plugin host builder: {e}"))
        })?;
    result_rx
        .recv_timeout(HOST_BUILD_TIMEOUT)
        .map_err(|error| {
            PluginBuildDiagnostic::host(match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    format!("plugin host build timed out after {:?}", HOST_BUILD_TIMEOUT)
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    "plugin host builder panicked".to_string()
                }
            })
        })?
}

fn build_plugin_graph_host_on_worker(
    graph_config: super::super::types::PluginGraphConfig,
    sample_rate: u32,
    input_channels: usize,
    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("sotf-plugin-graph-builder".to_string())
        .spawn(move || {
            let result = build_plugin_graph_host_with_policy(
                &graph_config,
                sample_rate,
                input_channels,
                oversampling_policy,
            );
            result_tx.send(result).ok();
        })
        .map_err(|e| {
            PluginBuildDiagnostic::host(format!("failed to spawn plugin graph builder: {e}"))
        })?;
    result_rx
        .recv_timeout(HOST_BUILD_TIMEOUT)
        .map_err(|error| {
            PluginBuildDiagnostic::host(match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => format!(
                    "plugin graph build timed out after {:?}",
                    HOST_BUILD_TIMEOUT
                ),
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    "plugin graph builder panicked".to_string()
                }
            })
        })?
}

pub(in crate::engine::manager_thread) fn store_plugin_build_diagnostics(
    state: &Arc<ArcSwap<AudioEngineState>>,
    diagnostics: Vec<PluginBuildDiagnostic>,
) {
    let mut new_state = (**state.load()).clone();
    new_state.plugin_build_diagnostics = diagnostics;
    state.store(Arc::new(new_state));
}

/// Apply a plugin update with proper synchronization.
/// A failed candidate is never committed, so the active host remains unchanged.
/// Waits for confirmation from processing thread and updates playback thread if needed.
#[allow(
    clippy::too_many_arguments,
    reason = "manager command handler: one argument per engine subsystem involved"
)]
fn apply_plugin_update_once(
    processing: &mut ProcessingThread,

    playback: &mut PlaybackThread,

    state: &Arc<ArcSwap<AudioEngineState>>,

    config_queue: &mut ConfigUpdateQueue,

    plugins: Vec<super::super::PluginConfig>,

    sample_rate: u32,

    input_channels: usize,

    playback_channel_limit: usize,

    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(), ConfigError> {
    log::debug!(
        "[Manager Thread] apply_plugin_update: Starting update with {} plugins at {}Hz",
        plugins.len(),
        sample_rate
    );

    let start_build = std::time::Instant::now();

    log::debug!("[Manager Thread] Building plugin host on worker thread...");

    let (host, build_diagnostics) = match build_plugin_update_host_on_worker(
        plugins.clone(),
        sample_rate,
        input_channels,
        oversampling_policy,
    ) {
        Ok(result) => result,
        Err(diagnostic) => {
            log::error!("[Manager Thread] Worker build failed: {}", diagnostic);
            store_plugin_build_diagnostics(state, vec![diagnostic.clone()]);
            return Err(ConfigError::PluginBuild { diagnostic });
        }
    };

    for diagnostic in &build_diagnostics {
        log::warn!("[Manager Thread] {}", diagnostic);
    }
    let failed_external_candidate = build_diagnostics
        .iter()
        .find(|diagnostic| is_required_external_update_failure(diagnostic))
        .cloned();
    // Surface build diagnostics in their dedicated state field. A clean build
    // deliberately clears diagnostics from the previous attempt without
    // disturbing the independently-owned general engine error.
    store_plugin_build_diagnostics(state, build_diagnostics);

    // Engine startup retains its documented best-effort plugin policy. A
    // requested host replacement is a complete candidate, however, so a
    // skipped external plugin must fail before PreparedHostUpdate is sent.
    if let Some(diagnostic) = failed_external_candidate {
        return Err(ConfigError::PluginBuild { diagnostic });
    }

    log::debug!(
        "[Manager Thread] Worker build successful in {:?}, output channels: {}",
        start_build.elapsed(),
        host.output_channels()
    );
    // Send update command to processing thread

    let current = state.load();
    let prepared = PreparedHostUpdate::prepare(
        host,
        sample_rate,
        current.num_channels,
        current.plugin_latency_samples,
    )
    .map_err(|message| ConfigError::PluginBuild {
        diagnostic: PluginBuildDiagnostic::host(message),
    })?;
    drop(current);
    let (generation, request_id, ticket) = processing.send_host_update(prepared).map_err(|_| {
        log::error!("[Manager Thread] Failed to send CommitHostUpdate command: disconnected");
        ConfigError::ChannelDisconnected
    })?;

    log::debug!(
        "[Manager Thread] apply_plugin_update: Sent CommitHostUpdate to processing thread, waiting for ACK..."
    );

    // Calculate adaptive timeout based on plugin complexity

    let timeout = estimate_update_timeout(&plugins);

    log::debug!("[Manager Thread] Using adaptive timeout: {:?}", timeout);

    let start = std::time::Instant::now();
    let (output_channels, output_sample_rate, latency_samples) =
        match wait_for_plugin_chain_update(processing, request_id, generation, &ticket, timeout) {
            Ok(metadata) => metadata,
            Err(error) => {
                config_queue.metrics.record_failure();
                return Err(error);
            }
        };

    let current = state.load();
    let old_playback_channels = current.playback_channels;
    let old_sample_rate = current.sample_rate;
    let playback_channels = if playback_channel_limit > 0 {
        output_channels.min(playback_channel_limit)
    } else {
        output_channels
    };
    let mut new_state = (**current).clone();
    drop(current);
    let (actual_playback_channels, actual_playback_sample_rate) =
        if playback_channels != old_playback_channels || output_sample_rate != old_sample_rate {
            match playback.reconfigure(output_sample_rate, playback_channels) {
                Ok(actual) => (actual.channels, actual.sample_rate),
                Err(error) => {
                    let reason = format!(
                        "Plugin host committed, but playback output reconfiguration failed: {error}"
                    );
                    new_state.num_channels = output_channels;
                    new_state.sample_rate = output_sample_rate;
                    new_state.plugin_latency_samples = latency_samples;
                    new_state.last_error = Some(reason.clone());
                    new_state.playback_state = crate::PlaybackState::Stopped;
                    state.store(Arc::new(new_state));
                    config_queue.metrics.record_failure();
                    return Err(ConfigError::ProcessingError { reason });
                }
            }
        } else {
            (old_playback_channels, old_sample_rate)
        };
    new_state.num_channels = output_channels;
    new_state.playback_channels = actual_playback_channels;
    new_state.sample_rate = actual_playback_sample_rate;
    new_state.plugin_latency_samples = latency_samples;
    state.store(Arc::new(new_state));

    config_queue.metrics.record_success(start.elapsed());
    Ok(())
}

/// Retry a host replacement when its control-thread snapshot becomes stale
/// before the processing thread reaches its commit boundary.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine::manager_thread) fn apply_plugin_update(
    processing: &mut ProcessingThread,
    playback: &mut PlaybackThread,
    state: &Arc<ArcSwap<AudioEngineState>>,
    config_queue: &mut ConfigUpdateQueue,
    plugins: Vec<super::super::PluginConfig>,
    sample_rate: u32,
    input_channels: usize,
    playback_channel_limit: usize,
    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(), ConfigError> {
    for attempt in 0..=MAX_STALE_HOST_UPDATE_RETRIES {
        match apply_plugin_update_once(
            processing,
            playback,
            state,
            config_queue,
            plugins.clone(),
            sample_rate,
            input_channels,
            playback_channel_limit,
            oversampling_policy,
        ) {
            Err(error)
                if is_stale_host_update(&error) && attempt < MAX_STALE_HOST_UPDATE_RETRIES =>
            {
                log::debug!(
                    "[Manager Thread] Prepared host became stale; retrying against the active host snapshot"
                );
            }
            result => return result,
        }
    }

    unreachable!("stale host update retry loop must return from the match");
}

/// Build a DawHost from a graph config and convert to a linear `Vec<PluginConfig>` isn't
/// possible for graph topologies. Instead, build the host directly and send it to the
/// processing thread. This reuses the same host-swap mechanism as `apply_plugin_update`.
#[allow(
    clippy::too_many_arguments,
    reason = "graph update coordinates the manager-owned processing and playback subsystems"
)]
pub(in crate::engine::manager_thread) fn apply_plugin_graph_update(
    processing: &mut ProcessingThread,
    playback: &mut PlaybackThread,
    state: &Arc<ArcSwap<AudioEngineState>>,
    graph_config: super::super::types::PluginGraphConfig,
    sample_rate: u32,
    input_channels: usize,
    playback_channel_limit: usize,
    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(), ConfigError> {
    log::debug!(
        "[Manager Thread] apply_plugin_graph_update: {} nodes, {} edges at {}Hz",
        graph_config.nodes.len(),
        graph_config.edges.len(),
        sample_rate
    );

    let (host, build_diagnostics) = match build_plugin_graph_host_on_worker(
        graph_config.clone(),
        sample_rate,
        input_channels,
        oversampling_policy,
    ) {
        Ok(result) => result,
        Err(diagnostic) => {
            log::error!("[Manager Thread] Graph build failed: {}", diagnostic);
            store_plugin_build_diagnostics(state, vec![diagnostic.clone()]);
            return Err(ConfigError::PluginBuild { diagnostic });
        }
    };

    for diagnostic in &build_diagnostics {
        log::warn!("[Manager Thread] {}", diagnostic);
    }
    store_plugin_build_diagnostics(state, build_diagnostics);

    let current = state.load();
    let prepared = PreparedHostUpdate::prepare(
        host,
        sample_rate,
        current.num_channels,
        current.plugin_latency_samples,
    )
    .map_err(|message| ConfigError::PluginBuild {
        diagnostic: PluginBuildDiagnostic::host(message),
    })?;
    drop(current);
    let (generation, request_id, ticket) = processing
        .send_host_update(prepared)
        .map_err(|_| ConfigError::ChannelDisconnected)?;

    let current = state.load();
    let old_playback_channels = current.playback_channels;
    let old_sample_rate = current.sample_rate;
    drop(current);
    let timeout = estimate_graph_update_timeout(&graph_config);
    let (output_channels, output_sample_rate, latency_samples) =
        wait_for_plugin_chain_update(processing, request_id, generation, &ticket, timeout)?;
    let playback_channels = if playback_channel_limit > 0 {
        output_channels.min(playback_channel_limit)
    } else {
        output_channels
    };

    let mut new_state = (**state.load()).clone();
    let (actual_playback_channels, actual_playback_sample_rate) = if playback_channels
        != old_playback_channels
        || output_sample_rate != old_sample_rate
    {
        match playback.reconfigure(output_sample_rate, playback_channels) {
            Ok(actual) => (actual.channels, actual.sample_rate),
            Err(error) => {
                let reason = format!(
                    "Plugin graph committed, but playback output reconfiguration failed: {error}"
                );
                new_state.num_channels = output_channels;
                new_state.sample_rate = output_sample_rate;
                new_state.plugin_latency_samples = latency_samples;
                new_state.last_error = Some(reason.clone());
                new_state.playback_state = crate::PlaybackState::Stopped;
                state.store(Arc::new(new_state));
                return Err(ConfigError::ProcessingError { reason });
            }
        }
    } else {
        (old_playback_channels, old_sample_rate)
    };
    new_state.num_channels = output_channels;
    new_state.playback_channels = actual_playback_channels;
    new_state.sample_rate = actual_playback_sample_rate;
    new_state.plugin_latency_samples = latency_samples;
    state.store(Arc::new(new_state));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        AudioFrame, DecoderMessage, PlaybackCommand, PlaybackConfiguration, PlaybackThread,
        PluginConfig, PluginGraphConfig, PluginGraphEdgeConfig, PluginGraphNodeConfig,
        ProcessingMessage, ProcessingThread, ThreadEvent,
    };
    use crate::engine::{GcItem, GcSender};
    use crate::plugins::PluginSettings;
    use sotf_plugins::plugin::TailLength;
    use sotf_plugins::{
        ExternalPlugin, ExternalPluginSandboxMode, ExternalPluginState, Plugin, PluginDescriptor,
        PluginFormat, PluginScanStatus, ProcessContext,
    };
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    const NATIVE_ROUTE_SAMPLE_RATE: u32 = 48_000;
    const NATIVE_ROUTE_INPUT_CHANNELS: usize = 64;
    const NATIVE_ROUTE_BLOCK_FRAMES: usize = 128;
    const NATIVE_ROUTE_WAIT: Duration = Duration::from_secs(10);
    // The isolated worker reserves one maximum-size process block for IPC.
    const NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES: usize = 8_192;

    struct RunningEngineRoute {
        processing: ProcessingThread,
        decoder_tx: Sender<DecoderMessage>,
        frame_rx: Receiver<ProcessingMessage>,
        playback: PlaybackThread,
        playback_command_rx: Option<Receiver<PlaybackCommand>>,
        state: Arc<ArcSwap<AudioEngineState>>,
        config_queue: ConfigUpdateQueue,
        event_rx: crossbeam::channel::Receiver<ThreadEvent>,
        _gc_rx: crossbeam::channel::Receiver<GcItem>,
        _recycle_tx: Sender<Vec<f32>>,
        _decoder_recycle_rx: Receiver<Vec<f32>>,
    }

    impl RunningEngineRoute {
        fn new(input_channels: usize) -> Self {
            let (decoder_tx, decoder_rx) = mpsc::channel();
            let (frame_tx, frame_rx) = mpsc::sync_channel(8);
            let (event_tx, event_rx) = crossbeam::channel::bounded(64);
            let (gc_tx, gc_rx): (GcSender, crossbeam::channel::Receiver<GcItem>) =
                crossbeam::channel::bounded(64);
            let (recycle_tx, recycle_rx) = mpsc::channel();
            let (decoder_recycle_tx, decoder_recycle_rx) = mpsc::sync_channel(8);
            let processing = ProcessingThread::new(
                decoder_rx,
                frame_tx,
                event_tx,
                NATIVE_ROUTE_SAMPLE_RATE,
                input_channels,
                Arc::new(ArcSwap::from_pointee(Vec::new())),
                gc_tx,
                recycle_rx,
                decoder_recycle_tx,
                #[cfg(feature = "streaming")]
                None,
            )
            .expect("start real processing worker");
            let (playback, playback_command_rx) = PlaybackThread::command_probe();
            Self {
                processing,
                decoder_tx,
                frame_rx,
                playback,
                playback_command_rx: Some(playback_command_rx),
                state: Arc::new(ArcSwap::from_pointee(AudioEngineState {
                    num_channels: input_channels,
                    playback_channels: input_channels,
                    sample_rate: NATIVE_ROUTE_SAMPLE_RATE,
                    ..AudioEngineState::default()
                })),
                config_queue: ConfigUpdateQueue::new(),
                event_rx,
                _gc_rx: gc_rx,
                _recycle_tx: recycle_tx,
                _decoder_recycle_rx: decoder_recycle_rx,
            }
        }

        fn apply(
            &mut self,
            plugins: Vec<PluginConfig>,
            playback_channel_limit: usize,
        ) -> Result<(), ConfigError> {
            apply_plugin_update(
                &mut self.processing,
                &mut self.playback,
                &self.state,
                &mut self.config_queue,
                plugins,
                NATIVE_ROUTE_SAMPLE_RATE,
                NATIVE_ROUTE_INPUT_CHANNELS,
                playback_channel_limit,
                EngineOversamplingPolicy::PluginPreferred,
            )
        }

        fn process(&self, input: Vec<f32>, channels: usize, frames: usize) -> AudioFrame {
            self.decoder_tx
                .send(DecoderMessage::Frame(
                    AudioFrame::try_new(input, frames, channels, NATIVE_ROUTE_SAMPLE_RATE)
                        .expect("well-formed input frame"),
                ))
                .expect("send audio to processing thread");
            match self
                .frame_rx
                .recv_timeout(NATIVE_ROUTE_WAIT)
                .expect("processing frame")
            {
                ProcessingMessage::Frame(frame) => frame,
                other => panic!("expected processed frame, got {other:?}"),
            }
        }

        fn start(&mut self, plugins: Vec<PluginConfig>) {
            self.apply(plugins, NATIVE_ROUTE_INPUT_CHANNELS)
                .expect("install initial real plugin host");
            let state = self.state.load();
            assert_eq!(state.num_channels, NATIVE_ROUTE_INPUT_CHANNELS);
            assert_eq!(state.playback_channels, NATIVE_ROUTE_INPUT_CHANNELS);
            assert_eq!(state.sample_rate, NATIVE_ROUTE_SAMPLE_RATE);
            assert!(matches!(
                self.playback_command_rx
                    .as_ref()
                    .expect("playback probe remains available")
                    .try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
        }

        fn acknowledge_next_playback_reconfiguration(
            &mut self,
        ) -> std::thread::JoinHandle<PlaybackConfiguration> {
            let command_rx = self
                .playback_command_rx
                .take()
                .expect("playback probe has not been consumed");
            std::thread::spawn(move || {
                let PlaybackCommand::Reconfigure(request) = command_rx
                    .recv_timeout(NATIVE_ROUTE_WAIT)
                    .expect("engine requests playback reconfiguration for new width")
                else {
                    panic!("expected playback reconfiguration command");
                };
                assert!(request.ticket.try_commit());
                let actual = request.requested;
                request
                    .reply_tx
                    .send(Ok(actual))
                    .expect("return actual playback configuration");
                actual
            })
        }
    }

    struct WorkerBinaryLink {
        path: std::path::PathBuf,
        remove_on_drop: bool,
    }

    impl WorkerBinaryLink {
        #[cfg(target_os = "linux")]
        fn install() -> Self {
            use std::os::unix::fs::symlink;

            let test_binary = std::env::current_exe().expect("test executable path");
            let deps_dir = test_binary.parent().expect("test executable directory");
            let target_worker = deps_dir
                .parent()
                .expect("target debug directory")
                .join("sotf-external-plugin-worker");
            assert!(
                target_worker.is_file(),
                "build sotf-external-plugin-worker before this ignored native test"
            );
            let path = deps_dir.join("sotf-external-plugin-worker");
            match std::fs::symlink_metadata(&path) {
                Ok(_) => assert_eq!(
                    std::fs::canonicalize(&path).expect("existing worker link resolves"),
                    std::fs::canonicalize(&target_worker).expect("worker target resolves"),
                    "test worker sibling must refer to the freshly built binary"
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    symlink(&target_worker, &path).expect("install worker sibling link");
                    return Self {
                        path,
                        remove_on_drop: true,
                    };
                }
                Err(error) => panic!("inspect worker sibling: {error}"),
            }
            Self {
                path,
                remove_on_drop: false,
            }
        }
    }

    impl Drop for WorkerBinaryLink {
        fn drop(&mut self) {
            if self.remove_on_drop {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    fn old_stateful_chain() -> Vec<PluginConfig> {
        vec![
            PluginConfig::new(
                "gain",
                serde_json::json!({
                    "channels": NATIVE_ROUTE_INPUT_CHANNELS,
                    "gain_db": -6.0,
                    "smoothing_ms": 0.0
                }),
            ),
            PluginConfig::new(
                "delay",
                serde_json::json!({
                    "channels": NATIVE_ROUTE_INPUT_CHANNELS,
                    "delay_ms": 10.0,
                    "feedback": 0.25,
                    "mix": 1.0,
                    "lfo_rate_hz": 0.0,
                    "lfo_depth_ms": 0.0,
                    "pitch_preserving": false,
                    "allpass_feedback": false,
                    "channel_delays_ms": []
                }),
            ),
        ]
    }

    fn ambisonics_descriptor() -> PluginDescriptor {
        PluginDescriptor {
            id: "536F7466416D6269736E696330303031".into(),
            name: "SOTF: Ambisonics Decoder".into(),
            vendor: "SOTF".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            format: PluginFormat::Vst3,
            path: std::path::PathBuf::from(
                std::env::var_os("SOTF_TEST_AMBISONICS_VST3_PLUGIN")
                    .expect("set SOTF_TEST_AMBISONICS_VST3_PLUGIN to the VST3 bundle"),
            ),
            audio_inputs: 4,
            audio_outputs: 6,
            is_instrument: false,
            categories: vec!["audio-effect".into()],
            scan_status: PluginScanStatus::Loadable,
        }
    }

    fn with_order_seven_setup(state: ExternalPluginState) -> ExternalPluginState {
        let mut value = serde_json::to_value(state).expect("serialize external state");
        value["audio_setup"] = serde_json::json!({
            "type": "ambisonics",
            "order": 7,
            "target_layout": "nine_one_six_wide"
        });
        serde_json::from_value(value).expect("deserialize order-seven external state")
    }

    fn external_config(state: ExternalPluginState) -> PluginConfig {
        PluginSettings::External { state }.to_plugin_config(NATIVE_ROUTE_SAMPLE_RATE as f64)
    }

    fn native_ambisonics_state(
        opaque_state_is_order_seven: bool,
        typed_setup_is_order_seven: bool,
    ) -> ExternalPluginState {
        let descriptor = ambisonics_descriptor();
        let mut seed_state = ExternalPluginState::new(
            descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            Vec::new(),
        );
        if opaque_state_is_order_seven {
            seed_state = with_order_seven_setup(seed_state);
        }
        let seed = ExternalPlugin::from_placeholder_state(&seed_state, NATIVE_ROUTE_SAMPLE_RATE)
            .expect("construct native state seed for the requested setup");
        let mut state = ExternalPluginState::new(
            descriptor,
            ExternalPluginSandboxMode::Isolated,
            seed.save_opaque_state()
                .expect("save native state for isolated candidate"),
        );
        if typed_setup_is_order_seven {
            state = with_order_seven_setup(state);
        }
        state
    }

    fn zero_block(frames: usize, channels: usize) -> Vec<f32> {
        vec![0.0; frames * channels]
    }

    fn native_route_probe_block(start_frame: usize, frames: usize) -> Vec<f32> {
        let mut block = zero_block(frames, NATIVE_ROUTE_INPUT_CHANNELS);
        for frame in 0..frames {
            for channel in 0..NATIVE_ROUTE_INPUT_CHANNELS {
                let code = (start_frame + frame)
                    .wrapping_mul(17)
                    .wrapping_add(channel.wrapping_mul(31))
                    % 509;
                block[frame * NATIVE_ROUTE_INPUT_CHANNELS + channel] =
                    (code as f32 - 254.0) / 8192.0;
            }
        }
        block
    }

    fn populate_stateful_controls(
        live: &RunningEngineRoute,
        synchronized_twin: &RunningEngineRoute,
    ) {
        let mut impulse = zero_block(NATIVE_ROUTE_BLOCK_FRAMES, NATIVE_ROUTE_INPUT_CHANNELS);
        impulse[0] = 0.5;
        let live_prime = live.process(
            impulse.clone(),
            NATIVE_ROUTE_INPUT_CHANNELS,
            NATIVE_ROUTE_BLOCK_FRAMES,
        );
        let twin_prime = synchronized_twin.process(
            impulse,
            NATIVE_ROUTE_INPUT_CHANNELS,
            NATIVE_ROUTE_BLOCK_FRAMES,
        );
        assert_eq!(live_prime.data, twin_prime.data);
        for _ in 0..2 {
            let input = zero_block(NATIVE_ROUTE_BLOCK_FRAMES, NATIVE_ROUTE_INPUT_CHANNELS);
            let live_frame = live.process(
                input.clone(),
                NATIVE_ROUTE_INPUT_CHANNELS,
                NATIVE_ROUTE_BLOCK_FRAMES,
            );
            let twin_frame = synchronized_twin.process(
                input,
                NATIVE_ROUTE_INPUT_CHANNELS,
                NATIVE_ROUTE_BLOCK_FRAMES,
            );
            assert_eq!(live_frame.data, twin_frame.data);
        }
    }

    fn reject_candidate_and_check_live_history(
        live: &mut RunningEngineRoute,
        synchronized_twin: &RunningEngineRoute,
        cold_control: &RunningEngineRoute,
    ) {
        let state_before_failure = {
            let state = live.state.load();
            (
                state.num_channels,
                state.playback_channels,
                state.sample_rate,
                state.plugin_latency_samples,
                state.last_error.clone(),
                state.isolated_external_plugin_worker_statuses.clone(),
            )
        };
        assert_eq!(state_before_failure.0, NATIVE_ROUTE_INPUT_CHANNELS);

        let failing_state = native_ambisonics_state(false, true);
        failing_state
            .validate()
            .expect("the outer order-seven state envelope is valid before native loading");
        let failure = live
            .apply(
                vec![external_config(failing_state)],
                NATIVE_ROUTE_INPUT_CHANNELS,
            )
            .expect_err("the worker must reject order-one opaque state as order seven");
        assert!(
            failure
                .to_string()
                .contains("failed to restore component state"),
            "candidate must fail during native VST3 state loading after outer validation: {failure}"
        );
        let failed_state = live.state.load();
        assert_eq!(
            (
                failed_state.num_channels,
                failed_state.playback_channels,
                failed_state.sample_rate,
                failed_state.plugin_latency_samples,
                failed_state.last_error.clone(),
                failed_state
                    .isolated_external_plugin_worker_statuses
                    .clone(),
            ),
            state_before_failure,
            "failed candidate construction must leave engine and worker metadata intact"
        );
        assert!(
            failed_state
                .plugin_build_diagnostics
                .iter()
                .any(|diagnostic| diagnostic
                    .message
                    .contains("failed to restore component state")),
            "the engine records the late candidate diagnostic"
        );
        assert!(matches!(
            live.playback_command_rx
                .as_ref()
                .expect("failed candidate did not consume playback probe")
                .try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        let continuation = zero_block(NATIVE_ROUTE_BLOCK_FRAMES, NATIVE_ROUTE_INPUT_CHANNELS);
        let live_continuation = live.process(
            continuation.clone(),
            NATIVE_ROUTE_INPUT_CHANNELS,
            NATIVE_ROUTE_BLOCK_FRAMES,
        );
        let twin_continuation = synchronized_twin.process(
            continuation.clone(),
            NATIVE_ROUTE_INPUT_CHANNELS,
            NATIVE_ROUTE_BLOCK_FRAMES,
        );
        assert_eq!(
            live_continuation.data, twin_continuation.data,
            "the running engine must continue the exact pre-failure delay history"
        );
        assert!(
            live_continuation
                .data
                .iter()
                .any(|sample| sample.abs() > 1.0e-4),
            "the delayed impulse must still emerge after the failed update"
        );
        let cold_continuation = cold_control.process(
            continuation,
            NATIVE_ROUTE_INPUT_CHANNELS,
            NATIVE_ROUTE_BLOCK_FRAMES,
        );
        assert!(
            cold_continuation.data.iter().all(|sample| *sample == 0.0),
            "the cold control proves the continuation contains retained delay history"
        );
    }

    fn commit_candidate_and_verify_eos(live: &mut RunningEngineRoute) {
        let valid_state = native_ambisonics_state(true, true);
        valid_state
            .validate()
            .expect("valid order-seven native state has a valid typed envelope");
        let playback_responder = live.acknowledge_next_playback_reconfiguration();
        live.apply(
            vec![external_config(valid_state.clone())],
            NATIVE_ROUTE_INPUT_CHANNELS,
        )
        .expect("valid order-seven retry commits through the live engine");
        let actual_playback = playback_responder.join().expect("playback response thread");
        assert_eq!(actual_playback.channels, 16);
        assert_eq!(actual_playback.sample_rate, NATIVE_ROUTE_SAMPLE_RATE);
        let committed = live.state.load();
        assert_eq!(committed.num_channels, 16);
        assert_eq!(committed.playback_channels, 16);
        assert_eq!(committed.sample_rate, NATIVE_ROUTE_SAMPLE_RATE);
        assert_eq!(
            committed.plugin_latency_samples,
            NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES
        );
        assert!(committed.plugin_build_diagnostics.is_empty());

        let mut direct_state = valid_state;
        direct_state.sandbox_mode = ExternalPluginSandboxMode::InProcess;
        let mut direct =
            ExternalPlugin::from_placeholder_state(&direct_state, NATIVE_ROUTE_SAMPLE_RATE)
                .expect("construct independent in-process Ambisonics reference");
        let native_tail_frames = match direct.tail_length() {
            TailLength::Finite(frames) => frames,
            tail => panic!("loaded Ambisonics reference needs finite tail metadata, got {tail:?}"),
        };
        assert!(
            native_tail_frames <= u64::from(NATIVE_ROUTE_SAMPLE_RATE) * 2,
            "native tail must fit this bounded end-to-end fixture"
        );

        let block_sizes = [127, 4_096, 251, 8_192, 353, 1_024, 257];
        let mut reference_audio = Vec::new();
        let mut engine_audio = Vec::new();
        let mut input_frame = 0usize;
        for frames in block_sizes {
            let input = native_route_probe_block(input_frame, frames);
            let mut reference_block = zero_block(frames, 16);
            let reference_context = ProcessContext::new(NATIVE_ROUTE_SAMPLE_RATE, frames)
                .with_sample_position(input_frame as u64);
            let reference_processed = direct
                .process(&input, &mut reference_block, &reference_context)
                .expect("direct VST3 Ambisonics callback succeeds");
            assert_eq!(reference_processed, frames);
            reference_audio.extend_from_slice(&reference_block);

            let engine_frame = live.process(input, NATIVE_ROUTE_INPUT_CHANNELS, frames);
            assert_eq!(engine_frame.num_channels, 16);
            assert_eq!(engine_frame.num_frames, frames);
            assert_eq!(engine_frame.data.len(), frames * 16);
            assert_eq!(engine_frame.sample_rate, NATIVE_ROUTE_SAMPLE_RATE);
            assert!(engine_frame.data.iter().all(|sample| sample.is_finite()));
            engine_audio.extend_from_slice(&engine_frame.data);
            input_frame += frames;

            // The engine-side worker is asynchronous; model the elapsed audio
            // duration so each callback has its real-time interval to finish.
            let callback_micros =
                (frames as u64 * 1_000_000 / u64::from(NATIVE_ROUTE_SAMPLE_RATE)) + 5_000;
            // Let each earlier request finish before advancing the simulated
            // source clock far enough to expose its output. This fixture is
            // checking engine state replacement and EOS delivery, not the
            // worker's real-time deadline performance.
            std::thread::sleep(
                Duration::from_micros(callback_micros).max(Duration::from_millis(250)),
            );
        }

        // Use the maximum accepted isolated block for the final worker
        // request, then queue true EOS immediately after its engine output.
        // This keeps the worker's tail metadata potentially in-flight at the
        // EOS boundary instead of letting a test-only sleep hide that case.
        let silence_frames = NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES;
        let silence_input = zero_block(silence_frames, NATIVE_ROUTE_INPUT_CHANNELS);
        let mut reference_silence = zero_block(silence_frames, 16);
        let silence_context = ProcessContext::new(NATIVE_ROUTE_SAMPLE_RATE, silence_frames)
            .with_sample_position(input_frame as u64);
        assert_eq!(
            direct
                .process(&silence_input, &mut reference_silence, &silence_context)
                .expect("direct silence callback succeeds"),
            silence_frames
        );
        reference_audio.extend_from_slice(&reference_silence);
        let ordinary_silence =
            live.process(silence_input, NATIVE_ROUTE_INPUT_CHANNELS, silence_frames);
        assert_eq!(ordinary_silence.num_channels, 16);
        assert_eq!(ordinary_silence.num_frames, silence_frames);
        assert_eq!(ordinary_silence.data.len(), silence_frames * 16);
        assert!(
            ordinary_silence
                .data
                .iter()
                .all(|sample| sample.is_finite())
        );
        engine_audio.extend_from_slice(&ordinary_silence.data);
        input_frame += silence_frames;
        // The silence is a distinct ordinary callback. Give its request time
        // to complete before the final signal request, which is followed by
        // true EOS without an extra wait.
        std::thread::sleep(Duration::from_millis(250));

        // Exercise the pending-worker EOS edge with a final accepted maximum
        // block that contains signal. For a zero native tail, this is exactly
        // the program block the worker-pipeline drain must return.
        let mut final_program_input =
            native_route_probe_block(input_frame, NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES);
        let final_input_offset =
            (NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES - 1) * NATIVE_ROUTE_INPUT_CHANNELS;
        for (channel, sample) in final_program_input
            [final_input_offset..final_input_offset + NATIVE_ROUTE_INPUT_CHANNELS]
            .iter_mut()
            .enumerate()
        {
            *sample = (channel as f32 + 1.0) / 8192.0;
        }
        let final_frame = &final_program_input
            [final_input_offset..final_input_offset + NATIVE_ROUTE_INPUT_CHANNELS];
        assert!(
            final_frame
                .iter()
                .all(|sample| sample.is_finite() && *sample != 0.0)
        );
        assert!(
            final_frame
                .windows(2)
                .all(|samples| samples[0] != samples[1])
        );

        let mut reference_final_program = zero_block(silence_frames, 16);
        let final_context = ProcessContext::new(NATIVE_ROUTE_SAMPLE_RATE, silence_frames)
            .with_sample_position(input_frame as u64);
        assert_eq!(
            direct
                .process(
                    &final_program_input,
                    &mut reference_final_program,
                    &final_context,
                )
                .expect("direct final program callback succeeds"),
            silence_frames
        );
        reference_audio.extend_from_slice(&reference_final_program);
        let final_program = live.process(
            final_program_input,
            NATIVE_ROUTE_INPUT_CHANNELS,
            silence_frames,
        );
        assert_eq!(final_program.num_channels, 16);
        assert_eq!(final_program.num_frames, silence_frames);
        assert_eq!(final_program.data.len(), silence_frames * 16);
        assert!(final_program.data.iter().all(|sample| sample.is_finite()));
        engine_audio.extend_from_slice(&final_program.data);
        input_frame += silence_frames;

        live.decoder_tx
            .send(DecoderMessage::EndOfStream)
            .expect("send true end-of-stream marker");
        let mut drained_frames = 0usize;
        let mut drained_audio = Vec::new();
        loop {
            let message = live.frame_rx.recv_timeout(NATIVE_ROUTE_WAIT).unwrap_or_else(|error| {
                let events: Vec<_> = live.event_rx.try_iter().collect();
                panic!(
                    "true EOS must complete independently of ordinary silence; receive={error}; thread events={events:?}"
                );
            });
            match message {
                ProcessingMessage::Frame(frame) => {
                    drained_frames += frame.num_frames;
                    assert_eq!(frame.num_channels, 16);
                    assert_eq!(frame.data.len(), frame.num_frames * 16);
                    assert_eq!(frame.sample_rate, NATIVE_ROUTE_SAMPLE_RATE);
                    assert!(frame.data.iter().all(|sample| sample.is_finite()));
                    drained_audio.extend_from_slice(&frame.data);
                }
                ProcessingMessage::EndOfStream => break,
                ProcessingMessage::Flush => panic!("EOS path must not emit Flush"),
            }
        }

        let expected_drain_frames =
            NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES + native_tail_frames as usize;
        assert_eq!(drained_frames, expected_drain_frames);
        assert_eq!(
            drained_audio.len(),
            expected_drain_frames * 16,
            "true EOS must return every native-tail and worker-latency sample exactly once"
        );
        assert!(
            drained_audio.iter().any(|sample| sample.abs() > 1.0e-8),
            "the EOS drain must return pending final-block signal, not only silence"
        );

        let mut remaining_tail = native_tail_frames;
        while remaining_tail > 0 {
            let frames = usize::try_from(remaining_tail)
                .unwrap_or(usize::MAX)
                .min(NATIVE_ROUTE_BLOCK_FRAMES);
            let input = zero_block(frames, NATIVE_ROUTE_INPUT_CHANNELS);
            let mut output = zero_block(frames, 16);
            let context = ProcessContext::new(NATIVE_ROUTE_SAMPLE_RATE, frames)
                .with_sample_position(
                    input_frame as u64
                        + silence_frames as u64
                        + (native_tail_frames - remaining_tail),
                );
            assert_eq!(
                direct
                    .process(&input, &mut output, &context)
                    .expect("direct native-tail callback succeeds"),
                frames
            );
            reference_audio.extend_from_slice(&output);
            remaining_tail -= frames as u64;
        }

        let mut expected_audio = zero_block(NATIVE_ROUTE_ISOLATED_LATENCY_SAMPLES, 16);
        expected_audio.extend_from_slice(&reference_audio);
        let mut actual_audio = engine_audio;
        actual_audio.extend_from_slice(&drained_audio);
        assert_eq!(actual_audio.len(), expected_audio.len());
        assert!(actual_audio.iter().all(|sample| sample.is_finite()));
        assert!(expected_audio.iter().all(|sample| sample.is_finite()));
        assert!(
            expected_audio.iter().any(|sample| sample.abs() > 1.0e-7),
            "direct Ambisonics reference must contain nonzero audio"
        );
        let max_residual = actual_audio
            .iter()
            .zip(&expected_audio)
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_residual <= 2.0e-5,
            "isolated engine waveform differs from direct VST3 composition reference; max residual={max_residual}"
        );

        assert!(
            matches!(
                live.frame_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
            ),
            "true EOS must be emitted once without a duplicate marker"
        );
    }

    #[test]
    fn plugin_build_diagnostics_persist_warnings_and_clear_stale_values() {
        let stale = PluginBuildDiagnostic::host("stale build warning");
        let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
            last_error: Some("output device disconnected".to_string()),
            plugin_build_diagnostics: vec![stale],
            ..AudioEngineState::default()
        }));

        store_plugin_build_diagnostics(&state, Vec::new());
        assert!(state.load().plugin_build_diagnostics.is_empty());
        assert_eq!(
            state.load().last_error.as_deref(),
            Some("output device disconnected")
        );

        let diagnostic = PluginBuildDiagnostic::graph_node(
            42,
            Some(91),
            "external",
            "External plugin `/missing/example.vst3` could not be loaded",
        );
        store_plugin_build_diagnostics(&state, vec![diagnostic.clone()]);
        assert_eq!(state.load().plugin_build_diagnostics, vec![diagnostic]);
        assert_eq!(
            state.load().last_error.as_deref(),
            Some("output device disconnected")
        );
    }

    #[test]
    fn failed_graph_candidate_preserves_working_host_and_engine_snapshot() {
        let (mut processing, processing_commands) = ProcessingThread::command_probe();
        let (mut playback, playback_commands) = PlaybackThread::command_probe();
        let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
            num_channels: 6,
            plugin_latency_samples: 321,
            last_error: Some("existing device diagnostic".to_string()),
            ..AudioEngineState::default()
        }));
        let graph = PluginGraphConfig::try_new(
            vec![
                PluginGraphNodeConfig::try_new(7, "gain", serde_json::json!({ "gain_db": 0.0 }), 2)
                    .unwrap(),
                PluginGraphNodeConfig::try_new(
                    42,
                    "definitely-not-a-real-plugin",
                    serde_json::json!({}),
                    2,
                )
                .unwrap(),
                PluginGraphNodeConfig::try_new(
                    99,
                    "gain",
                    serde_json::json!({ "gain_db": -6.0 }),
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

        let error = apply_plugin_graph_update(
            &mut processing,
            &mut playback,
            &state,
            graph,
            48_000,
            2,
            2,
            EngineOversamplingPolicy::PluginPreferred,
        )
        .expect_err("a requested graph node failure must abort the candidate");

        assert!(error.to_string().contains("node 42"), "{error}");
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
        assert_eq!(current.num_channels, 6);
        assert_eq!(current.plugin_latency_samples, 321);
        assert_eq!(
            current.last_error.as_deref(),
            Some("existing device diagnostic")
        );
        assert_eq!(current.plugin_build_diagnostics.len(), 1);
        assert!(matches!(
            current.plugin_build_diagnostics[0].target,
            crate::PluginBuildTarget::GraphNode { node_id: 42 }
        ));
    }

    #[test]
    fn invalid_band_split_candidate_does_not_replace_working_host() {
        let (mut processing, processing_commands) = ProcessingThread::command_probe();
        let (mut playback, playback_commands) = PlaybackThread::command_probe();
        let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
            num_channels: 2,
            playback_channels: 2,
            sample_rate: 48_000,
            plugin_latency_samples: 321,
            last_error: Some("existing device diagnostic".to_string()),
            ..AudioEngineState::default()
        }));
        let mut config_queue = ConfigUpdateQueue::new();
        let (legacy_empty_host, legacy_empty_warnings) = build_plugin_host_with_policy(
            &[PluginConfig::new(
                "band_split",
                serde_json::json!({"frequencies": []}),
            )],
            48_000,
            2,
            EngineOversamplingPolicy::PluginPreferred,
        )
        .expect("a legacy empty cutoff vector must retain its historical default behavior");
        assert!(legacy_empty_warnings.is_empty());
        assert_eq!(legacy_empty_host.output_channels(), 4);

        let typed_invalid_split = crate::plugins::PluginSettings::BandSplit {
            channels: 2,
            frequency: 1_000.0,
            crossover_type: "LR24".to_string(),
            frequencies: Some(Vec::new()),
            recombination_mode: Default::default(),
            num_bands: 2,
            frequency_2: 4_000.0,
            frequency_3: 8_000.0,
        }
        .to_plugin_config(48_000.0);
        assert_eq!(typed_invalid_split.plugin_type, "band_split");
        assert_eq!(
            typed_invalid_split.parameters["explicit_frequencies"],
            serde_json::json!([])
        );
        let candidate = vec![
            PluginConfig {
                plugin_type: "gain".to_string(),
                parameters: serde_json::json!({"gain_db": -3.0}),
            },
            typed_invalid_split,
        ];

        let error = apply_plugin_update(
            &mut processing,
            &mut playback,
            &state,
            &mut config_queue,
            candidate,
            48_000,
            2,
            2,
            EngineOversamplingPolicy::PluginPreferred,
        )
        .expect_err("an invalid BandSplit candidate must abort the chain update");

        assert!(
            matches!(
                processing_commands.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "an invalid candidate must not replace the processing host"
        );
        assert!(
            error
                .to_string()
                .contains("At least one crossover frequency is required"),
            "candidate failure should report the invalid explicit cutoffs: {error}"
        );
        assert!(
            matches!(
                playback_commands.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "an invalid candidate must not reconfigure playback"
        );
        let current = state.load();
        assert_eq!(current.num_channels, 2);
        assert_eq!(current.playback_channels, 2);
        assert_eq!(current.sample_rate, 48_000);
        assert_eq!(current.plugin_latency_samples, 321);
        assert_eq!(
            current.last_error.as_deref(),
            Some("existing device diagnostic")
        );
        assert_eq!(current.plugin_build_diagnostics.len(), 1);
        assert!(matches!(
            current.plugin_build_diagnostics[0].target,
            crate::PluginBuildTarget::ChainPlugin { plugin_index: 1 }
        ));
    }

    #[test]
    fn failed_requested_external_candidate_does_not_send_host_update() {
        let (mut processing, processing_commands) = ProcessingThread::command_probe();
        let (mut playback, playback_commands) = PlaybackThread::command_probe();
        let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
            num_channels: 2,
            playback_channels: 2,
            sample_rate: 48_000,
            plugin_latency_samples: 321,
            last_error: Some("existing device diagnostic".to_string()),
            ..AudioEngineState::default()
        }));
        let mut config_queue = ConfigUpdateQueue::new();
        let candidate = vec![
            PluginConfig::new("gain", serde_json::json!({"gain_db": -3.0})),
            PluginConfig::new(
                "external",
                serde_json::json!({"isolated": true, "plugin_trust": "unknown"}),
            ),
        ];
        let (startup_host, startup_warnings) = build_plugin_host_with_policy(
            &candidate,
            48_000,
            2,
            EngineOversamplingPolicy::PluginPreferred,
        )
        .expect("initial build retains its best-effort external-plugin policy");
        assert_eq!(startup_host.plugin_count(), 1);
        assert_eq!(startup_warnings.len(), 1);
        assert_eq!(startup_warnings[0].plugin_type.as_deref(), Some("external"));

        let error = apply_plugin_update(
            &mut processing,
            &mut playback,
            &state,
            &mut config_queue,
            candidate,
            48_000,
            2,
            2,
            EngineOversamplingPolicy::PluginPreferred,
        )
        .expect_err("a requested external plugin failure must abort the candidate");

        assert!(
            matches!(
                processing_commands.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "a failed external candidate must not send a host replacement"
        );
        assert!(
            matches!(
                playback_commands.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "a failed external candidate must not reconfigure playback"
        );
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("external plugin"),
            "report the external candidate failure instead of a later timeout: {error}"
        );
        let current = state.load();
        assert_eq!(current.num_channels, 2);
        assert_eq!(current.playback_channels, 2);
        assert_eq!(current.sample_rate, 48_000);
        assert_eq!(current.plugin_latency_samples, 321);
        assert_eq!(
            current.last_error.as_deref(),
            Some("existing device diagnostic")
        );
        assert_eq!(current.plugin_build_diagnostics.len(), 1);
        assert_eq!(
            current.plugin_build_diagnostics[0].plugin_type.as_deref(),
            Some("external")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires the exported Ambisonics VST3 bundle and built isolated worker"]
    fn late_native_candidate_failure_preserves_live_engine_audio_and_retry_reconfigures() {
        let _worker_link = WorkerBinaryLink::install();
        assert!(
            std::path::Path::new(
                &std::env::var_os("SOTF_TEST_AMBISONICS_VST3_PLUGIN")
                    .expect("set SOTF_TEST_AMBISONICS_VST3_PLUGIN to the VST3 bundle"),
            )
            .is_dir(),
            "the test variable must name the actual VST3 bundle"
        );

        let old_plugins = old_stateful_chain();
        let mut live = RunningEngineRoute::new(NATIVE_ROUTE_INPUT_CHANNELS);
        let mut synchronized_twin = RunningEngineRoute::new(NATIVE_ROUTE_INPUT_CHANNELS);
        let mut cold_control = RunningEngineRoute::new(NATIVE_ROUTE_INPUT_CHANNELS);
        live.start(old_plugins.clone());
        synchronized_twin.start(old_plugins.clone());
        cold_control.start(old_plugins);
        populate_stateful_controls(&live, &synchronized_twin);
        reject_candidate_and_check_live_history(&mut live, &synchronized_twin, &cold_control);
        commit_candidate_and_verify_eos(&mut live);
    }

    #[test]
    fn failed_external_graph_candidate_does_not_send_host_update() {
        let (mut processing, processing_commands) = ProcessingThread::command_probe();
        let (mut playback, playback_commands) = PlaybackThread::command_probe();
        let state = Arc::new(ArcSwap::from_pointee(AudioEngineState {
            num_channels: 2,
            playback_channels: 2,
            sample_rate: 48_000,
            plugin_latency_samples: 321,
            ..AudioEngineState::default()
        }));
        let graph = PluginGraphConfig::try_new(
            vec![
                PluginGraphNodeConfig::try_new(7, "gain", serde_json::json!({"gain_db": -3.0}), 2)
                    .unwrap(),
                PluginGraphNodeConfig::try_new(
                    42,
                    "external",
                    serde_json::json!({"isolated": true, "plugin_trust": "unknown"}),
                    2,
                )
                .unwrap(),
            ],
            vec![PluginGraphEdgeConfig::new(7, 42)],
        )
        .unwrap();

        let error = apply_plugin_graph_update(
            &mut processing,
            &mut playback,
            &state,
            graph,
            48_000,
            2,
            2,
            EngineOversamplingPolicy::PluginPreferred,
        )
        .expect_err("a graph with a failed external node must abort the candidate");

        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("external plugin"),
            "graph update should report external construction failure: {error}"
        );
        assert!(
            matches!(
                processing_commands.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "a failed graph candidate must not send a host replacement"
        );
        assert!(
            matches!(
                playback_commands.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "a failed graph candidate must not reconfigure playback"
        );
        let current = state.load();
        assert_eq!(current.num_channels, 2);
        assert_eq!(current.playback_channels, 2);
        assert_eq!(current.sample_rate, 48_000);
        assert_eq!(current.plugin_latency_samples, 321);
        assert_eq!(current.plugin_build_diagnostics.len(), 1);
        assert!(matches!(
            current.plugin_build_diagnostics[0].target,
            crate::PluginBuildTarget::GraphNode { node_id: 42 }
        ));
    }
}
