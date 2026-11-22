use super::super::PluginConfig;
use super::super::PluginGraphEdgeKind;
use super::misc::configure_host_oversampling;
use super::misc::create_plugin;
use crate::{EngineOversamplingPolicy, PluginBuildDiagnostic};
use sotf_plugins::speaker_config::{
    ChannelLayout, get_speaker_config, speaker_config_id_from_legacy_index,
};
use sotf_plugins::{
    EXTERNAL_PLUGIN_INSTANCE_ID_PARAMETER, PluginHost, ProcessingMode,
    create_plugin_with_external_key,
};

fn plugin_instance_id(parameters: &serde_json::Value) -> Option<usize> {
    parameters
        .get(EXTERNAL_PLUGIN_INSTANCE_ID_PARAMETER)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn upmixer_input_layout(
    plugin_type: &str,
    parameters: &serde_json::Value,
) -> Option<ChannelLayout> {
    if !plugin_type.eq_ignore_ascii_case("upmixer") {
        return None;
    }
    serde_json::from_value(parameters.get("input_layout")?.clone()).ok()
}

/// Resolve the declared `speaker_config`, accepting the string form and
/// the legacy numeric index form exactly as the plugin decoder does. A
/// present but undecodable value yields `None` (layout unknown) instead of
/// guessing a default the plugin did not select.
fn speaker_config_id(parameters: &serde_json::Value) -> Option<&str> {
    let value = parameters.get("speaker_config")?;
    if let Some(id) = value.as_str() {
        return Some(id);
    }
    if let Some(index) = value.as_u64() {
        return speaker_config_id_from_legacy_index(index);
    }
    let number = value.as_f64()?;
    (number.is_finite() && number.fract() == 0.0 && number >= 0.0)
        .then(|| speaker_config_id_from_legacy_index(number as u64))
        .flatten()
}

fn upmixer_output_layout(
    plugin_type: &str,
    parameters: &serde_json::Value,
    output_channels: usize,
) -> Option<ChannelLayout> {
    if !plugin_type.eq_ignore_ascii_case("upmixer") {
        return None;
    }
    let speaker_config =
        if parameters.get("binaural_preview").and_then(|v| v.as_bool()) == Some(true) {
            "2.0"
        } else if parameters.get("speaker_config").is_none() {
            // Mirrors the plugin field default.
            "5.1"
        } else {
            speaker_config_id(parameters)?
        };
    let layout = ChannelLayout::from_speaker_config(get_speaker_config(speaker_config)?).ok()?;
    (layout.channels.len() == output_channels).then_some(layout)
}

fn layout_mismatch(actual: &ChannelLayout, expected: &ChannelLayout) -> Option<String> {
    if actual.channels.len() != expected.channels.len() {
        return None;
    }
    (0..actual.channels.len()).find_map(|index| {
        let actual_role = actual.role_at(index);
        let expected_role = expected.role_at(index);
        (actual_role != expected_role).then(|| {
            format!(
                "channel {index} carries {actual_role:?}, but the upmixer expects {expected_role:?}"
            )
        })
    })
}

fn requires_complete_channel_route_candidate(plugin_type: &str) -> bool {
    plugin_type.eq_ignore_ascii_case("band_split")
        || plugin_type.eq_ignore_ascii_case("bandsplit")
        || plugin_type.eq_ignore_ascii_case("crossover")
        || plugin_type.eq_ignore_ascii_case("upmixer")
}

fn complete_channel_route_label(plugin_type: &str) -> &'static str {
    if plugin_type.eq_ignore_ascii_case("crossover") {
        "Crossover"
    } else if plugin_type.eq_ignore_ascii_case("upmixer") {
        "Upmixer"
    } else {
        "BandSplit"
    }
}

/// Build a plugin host from configs.
///
/// Plugins that fail to create or have channel mismatches are skipped rather
/// than aborting the entire chain, except BandSplit, Crossover, and Upmixer: skipping a
/// requested channel-routing plugin would silently commit a different route
/// and output layout. The second element of the returned tuple contains
/// warnings about other skipped plugins.
pub fn build_plugin_host(
    configs: &[PluginConfig],
    sample_rate: u32,
    channels: usize,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    build_plugin_host_with_policy(
        configs,
        sample_rate,
        channels,
        EngineOversamplingPolicy::PluginPreferred,
    )
}

/// Build a plugin chain for realtime playback or offline rendering.
///
/// Mode is fixed before any plugin initializes. Offline hosts belong on a
/// rendering thread; synchronous model work is allowed there.
///
/// # Errors
/// Returns host preparation errors or mandatory routing-plugin failures.
pub fn build_plugin_host_for_mode(
    configs: &[PluginConfig],
    sample_rate: u32,
    channels: usize,
    mode: ProcessingMode,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    build_plugin_host_with_policy_and_mode(
        configs,
        sample_rate,
        channels,
        EngineOversamplingPolicy::PluginPreferred,
        mode,
    )
}

pub fn build_plugin_host_with_policy(
    configs: &[PluginConfig],
    sample_rate: u32,
    channels: usize,
    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    build_plugin_host_with_policy_and_mode(
        configs,
        sample_rate,
        channels,
        oversampling_policy,
        ProcessingMode::Realtime,
    )
}

fn build_plugin_host_with_policy_and_mode(
    configs: &[PluginConfig],
    sample_rate: u32,
    channels: usize,
    oversampling_policy: EngineOversamplingPolicy,
    mode: ProcessingMode,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    let mut host = PluginHost::new_with_processing_mode(channels, sample_rate, mode);
    configure_host_oversampling(&mut host, oversampling_policy)
        .map_err(PluginBuildDiagnostic::host)?;
    let mut current_channels = channels;
    // The source currently supplies only a width. Retain layouts once a
    // producer declares them, and invalidate them at unmodelled transforms.
    let mut current_layout: Option<ChannelLayout> = None;
    let mut warnings: Vec<PluginBuildDiagnostic> = Vec::new();

    for (i, config) in configs.iter().enumerate() {
        log::info!(
            "[Processing Thread] Loading plugin {}: {}",
            i,
            config.plugin_type
        );

        match create_plugin(
            &config.plugin_type,
            &config.parameters,
            current_channels,
            sample_rate,
        ) {
            Ok(plugin) => {
                // Check channel compatibility
                if plugin.input_channels() != current_channels {
                    let reason = format!(
                        "expects {} input channels, but chain provides {}",
                        plugin.input_channels(),
                        current_channels
                    );
                    let msg = if requires_complete_channel_route_candidate(&config.plugin_type) {
                        format!(
                            "{} candidate rejected: {reason}",
                            complete_channel_route_label(&config.plugin_type)
                        )
                    } else {
                        format!("Plugin '{}' skipped: {reason}", config.plugin_type)
                    };
                    let diagnostic = PluginBuildDiagnostic::chain_plugin(
                        i,
                        plugin_instance_id(&config.parameters),
                        &config.plugin_type,
                        msg.clone(),
                    );
                    log::warn!("[Processing Thread] {}", msg);
                    if requires_complete_channel_route_candidate(&config.plugin_type) {
                        return Err(diagnostic);
                    }
                    warnings.push(diagnostic);
                    continue;
                }

                if let (Some(actual), Some(expected)) = (
                    current_layout.as_ref(),
                    upmixer_input_layout(&config.plugin_type, &config.parameters),
                ) && let Some(reason) = layout_mismatch(actual, &expected)
                {
                    return Err(PluginBuildDiagnostic::chain_plugin(
                        i,
                        plugin_instance_id(&config.parameters),
                        &config.plugin_type,
                        format!("Upmixer candidate rejected: {reason}"),
                    ));
                }

                let produced_layout = upmixer_output_layout(
                    &config.plugin_type,
                    &config.parameters,
                    plugin.output_channels(),
                );
                if produced_layout.is_some() {
                    current_layout = produced_layout;
                } else if !config.plugin_type.eq_ignore_ascii_case("gain")
                    || plugin.input_channels() != plugin.output_channels()
                {
                    current_layout = None;
                }

                // Update current channel count for next plugin
                current_channels = plugin.output_channels();

                log::info!(
                    "[Processing Thread] Plugin '{}' loaded: {}ch -> {}ch",
                    config.plugin_type,
                    plugin.input_channels(),
                    plugin.output_channels()
                );

                host.add_plugin(plugin).map_err(|error| {
                    PluginBuildDiagnostic::chain_plugin(
                        i,
                        plugin_instance_id(&config.parameters),
                        &config.plugin_type,
                        format!(
                            "Plugin '{}' could not be added: {error}",
                            config.plugin_type
                        ),
                    )
                })?;
            }
            Err(e) => {
                let msg = if requires_complete_channel_route_candidate(&config.plugin_type) {
                    format!(
                        "{} candidate rejected: {e}",
                        complete_channel_route_label(&config.plugin_type)
                    )
                } else {
                    format!("Plugin '{}' skipped: {}", config.plugin_type, e)
                };
                log::warn!("[Processing Thread] {}", msg);
                let diagnostic = PluginBuildDiagnostic::chain_plugin(
                    i,
                    plugin_instance_id(&config.parameters),
                    &config.plugin_type,
                    msg,
                );
                if requires_complete_channel_route_candidate(&config.plugin_type) {
                    return Err(diagnostic);
                }
                warnings.push(diagnostic);
            }
        }
    }

    host.build().map_err(|error| {
        PluginBuildDiagnostic::host(format!("Plugin host build failed: {error}"))
    })?;

    log::info!(
        "[Processing Thread] Plugin chain loaded: {} plugins ({}ch -> {}ch), {} skipped",
        configs.len() - warnings.len(),
        channels,
        host.output_channels(),
        warnings.len()
    );

    Ok((host, warnings))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::build_plugin_graph_host;
    use crate::engine::{PluginGraphConfig, PluginGraphNodeConfig};

    #[test]
    fn retained_bypass_stage_preserves_program_gain_and_channel_order() {
        use crate::engine::PluginGraphEdgeConfig;
        let mut bypass = PluginGraphNodeConfig::try_new(
            42,
            "gain",
            serde_json::json!({"gain_db": 20.0, "smoothing_ms": 0.0}),
            2,
        )
        .unwrap();
        bypass.bypassed = true;
        let mut graph = PluginGraphConfig::try_new(
            vec![
                PluginGraphNodeConfig::try_new(
                    7,
                    "gain",
                    serde_json::json!({"gain_db": -6.0, "smoothing_ms": 0.0}),
                    2,
                )
                .unwrap(),
                bypass,
                PluginGraphNodeConfig::try_new(
                    99,
                    "gain",
                    serde_json::json!({"gain_db": -3.0, "smoothing_ms": 0.0}),
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
        // Deliberately serialize sink first: control order must not depend
        // on dependency stage order or numeric config IDs.
        graph.nodes.rotate_right(1);
        let (mut host, warnings) = build_plugin_graph_host(&graph, 48_000, 2).unwrap();
        let gain_id = sotf_plugins::ParameterId::from("gain_db");
        for (index, expected) in [-3.0f32, -6.0, 20.0].into_iter().enumerate() {
            assert_eq!(
                host.get_plugin(index).unwrap().get_parameter(&gain_id),
                Some(sotf_plugins::ParameterValue::Float(expected))
            );
        }
        for invalid in [vec![0, 0, 2], vec![0, 1, 99], vec![0, 1]] {
            assert!(host.set_graph_plugin_order(&invalid).is_err());
            assert_eq!(
                host.get_plugin(0).unwrap().get_parameter(&gain_id),
                Some(sotf_plugins::ParameterValue::Float(-3.0))
            );
        }
        host.build().unwrap();
        assert_eq!(
            host.get_plugin(0).unwrap().get_parameter(&gain_id),
            Some(sotf_plugins::ParameterValue::Float(-3.0))
        );
        assert!(warnings.is_empty());
        let input: Vec<f32> = (0..256)
            .flat_map(|frame| {
                [
                    0.2 * (frame as f32 * 0.17).sin(),
                    -0.1 * (frame as f32 * 0.31).cos(),
                ]
            })
            .collect();
        let mut output = vec![0.0; input.len()];
        let expected_gain = 10.0f32.powf(-9.0 / 20.0);
        for _ in 0..3 {
            assert_eq!(host.process(&input, &mut output).unwrap(), 256);
            for (actual, source) in output.iter().zip(&input) {
                assert!(
                    (actual - source * expected_gain).abs() < 1e-6,
                    "retained bypass must pass both channels without applying its saved gain"
                );
            }
        }
    }

    #[test]
    fn serialized_graph_order_refreshes_analyzer_indices() {
        use crate::engine::PluginGraphEdgeConfig;
        let graph = PluginGraphConfig::try_new(
            vec![
                PluginGraphNodeConfig::try_new(42, "loudness_monitor", serde_json::json!({}), 2)
                    .unwrap(),
                PluginGraphNodeConfig::try_new(99, "gain", serde_json::json!({"gain_db": -3.0}), 2)
                    .unwrap(),
                PluginGraphNodeConfig::try_new(7, "gain", serde_json::json!({"gain_db": -6.0}), 2)
                    .unwrap(),
            ],
            vec![
                PluginGraphEdgeConfig::new(7, 42),
                PluginGraphEdgeConfig::new(42, 99),
            ],
        )
        .unwrap();
        let (mut host, warnings) = build_plugin_graph_host(&graph, 48_000, 2).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(host.analyzer_indices(), &[0]);
        assert!(host.get_plugin(0).unwrap().get_data().is_some());
        assert!(host.get_plugin(1).unwrap().get_data().is_none());
        host.build().unwrap();
        assert_eq!(host.analyzer_indices(), &[0]);
        assert!(host.get_plugin(0).unwrap().get_data().is_some());
    }

    #[test]
    fn serialized_graph_bypass_reaches_plugin_host() {
        let mut node = PluginGraphNodeConfig::try_new(7, "gain", serde_json::json!({}), 2).unwrap();
        node.bypassed = true;
        let graph = PluginGraphConfig::try_new(vec![node], vec![]).unwrap();

        let (host, warnings) = build_plugin_graph_host(&graph, 48_000, 2).unwrap();

        assert!(warnings.is_empty());
        assert!(host.is_node_bypassed(0).unwrap());
    }
}

/// Build a plugin host from a graph config (DAG topology).
///
/// Unlike `build_plugin_host` which chains plugins linearly, this uses
/// `DawHost::add_node()` + `add_edge()` to create arbitrary graph topologies
/// needed for multi-driver crossover setups.
///
/// Graph construction is atomic: every declared node and edge must load before
/// a host is returned. Skipping a failed node would silently change topology
/// and can leave an apparently built but disconnected processing DAG.
/// Sidechain edges map to host key-bus routing; key-bus geometry is
/// validated here so bus-less or over-wide key routing fails loudly
/// instead of degrading the render.
#[allow(dead_code)]
pub fn build_plugin_graph_host(
    config: &super::super::types::PluginGraphConfig,
    sample_rate: u32,
    channels: usize,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    build_plugin_graph_host_with_policy(
        config,
        sample_rate,
        channels,
        EngineOversamplingPolicy::PluginPreferred,
    )
}

pub fn build_plugin_graph_host_with_policy(
    config: &super::super::types::PluginGraphConfig,
    sample_rate: u32,
    channels: usize,
    oversampling_policy: EngineOversamplingPolicy,
) -> Result<(PluginHost, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
    use sotf_plugins::GraphEdge;
    use std::collections::HashMap;

    config.validate().map_err(|error| {
        PluginBuildDiagnostic::host(format!("Invalid plugin graph configuration: {error}"))
    })?;

    let mut host = PluginHost::new(channels, sample_rate);
    configure_host_oversampling(&mut host, oversampling_policy)
        .map_err(PluginBuildDiagnostic::host)?;
    let mut id_map: HashMap<usize, usize> = HashMap::new();
    // Constructed (input, output) widths and types per config node id for
    // sidechain key-bus validation below.
    let mut widths: HashMap<usize, (usize, usize)> = HashMap::new();
    let mut node_types: HashMap<usize, &str> = HashMap::new();
    let mut known_output_layouts: HashMap<usize, ChannelLayout> = HashMap::new();

    for node_config in &config.nodes {
        // Graph context: external-key buses are routable through sidechain
        // edges, so keyed nodes construct here. Linear chains keep the
        // default entry and its loud single-width refusal.
        let plugin = create_plugin_with_external_key(
            &node_config.plugin_type,
            &node_config.parameters,
            node_config.input_channels,
            sample_rate,
        )
        .map_err(|error| {
            PluginBuildDiagnostic::graph_node(
                node_config.id,
                plugin_instance_id(&node_config.parameters),
                &node_config.plugin_type,
                format!(
                    "Plugin graph node {} ('{}') failed to load: {error}",
                    node_config.id, node_config.plugin_type
                ),
            )
        })?;
        widths.insert(
            node_config.id,
            (plugin.input_channels(), plugin.output_channels()),
        );
        if !node_config.bypassed
            && let Some(layout) = upmixer_output_layout(
                &node_config.plugin_type,
                &node_config.parameters,
                plugin.output_channels(),
            )
        {
            known_output_layouts.insert(node_config.id, layout);
        }
        node_types.insert(node_config.id, node_config.plugin_type.as_str());
        let host_id = host
            .add_node(format!("node_{}", node_config.id), plugin)
            .map_err(|error| {
                PluginBuildDiagnostic::graph_node(
                    node_config.id,
                    plugin_instance_id(&node_config.parameters),
                    &node_config.plugin_type,
                    format!(
                        "Plugin graph node {} ('{}') could not be added: {error}",
                        node_config.id, node_config.plugin_type
                    ),
                )
            })?;
        if node_config.bypassed {
            host.bypass_node(host_id).map_err(|error| {
                PluginBuildDiagnostic::graph_node(
                    node_config.id,
                    plugin_instance_id(&node_config.parameters),
                    &node_config.plugin_type,
                    format!(
                        "Plugin graph node {} ('{}') could not be bypassed: {error}",
                        node_config.id, node_config.plugin_type
                    ),
                )
            })?;
        }
        id_map.insert(node_config.id, host_id);
    }

    // Gain is known to keep channel roles. For other nodes, leave roles
    // unknown unless they explicitly declare an upmixer output layout.
    for _ in 0..config.nodes.len() {
        let mut changed = false;
        for node in &config.nodes {
            if !node.plugin_type.eq_ignore_ascii_case("gain")
                || widths[&node.id].0 != widths[&node.id].1
                || known_output_layouts.contains_key(&node.id)
            {
                continue;
            }
            let mut inputs = config
                .edges
                .iter()
                .filter(|edge| edge.kind == PluginGraphEdgeKind::Audio && edge.to_node == node.id);
            let Some(edge) = inputs.next() else { continue };
            if inputs.next().is_some() {
                continue;
            }
            if let Some(layout) = known_output_layouts.get(&edge.from_node).cloned()
                && layout.channels.len() == widths[&node.id].0
            {
                known_output_layouts.insert(node.id, layout);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for edge in &config.edges {
        if edge.kind != PluginGraphEdgeKind::Audio {
            continue;
        }
        let Some(actual) = known_output_layouts.get(&edge.from_node) else {
            continue;
        };
        let Some(target) = config.nodes.iter().find(|node| node.id == edge.to_node) else {
            continue;
        };
        if let Some(expected) = upmixer_input_layout(&target.plugin_type, &target.parameters)
            && let Some(reason) = layout_mismatch(actual, &expected)
        {
            return Err(PluginBuildDiagnostic::graph_edge(
                edge.from_node,
                edge.to_node,
                format!("Upmixer candidate rejected: {reason}"),
            ));
        }
    }

    // Key channels already routed per target node id. The host packs each
    // sidechain edge's source channels consecutively after the target's
    // program bus in edge order and silently drops overflow, so cumulative
    // key width is tracked in the same order and refused loudly instead.
    let mut key_usage: HashMap<usize, usize> = HashMap::new();
    for edge in &config.edges {
        let from = *id_map.get(&edge.from_node).ok_or_else(|| {
            PluginBuildDiagnostic::graph_edge(
                edge.from_node,
                edge.to_node,
                format!(
                    "Plugin graph edge {} -> {} references unloaded from_node {}",
                    edge.from_node, edge.to_node, edge.from_node
                ),
            )
        })?;
        let to = *id_map.get(&edge.to_node).ok_or_else(|| {
            PluginBuildDiagnostic::graph_edge(
                edge.from_node,
                edge.to_node,
                format!(
                    "Plugin graph edge {} -> {} references unloaded to_node {}",
                    edge.from_node, edge.to_node, edge.to_node
                ),
            )
        })?;
        let host_edge = match edge.kind {
            PluginGraphEdgeKind::Audio => GraphEdge::new(from, to),
            PluginGraphEdgeKind::Sidechain => {
                // Config validation guarantees both endpoints were built, so
                // their constructed widths are present.
                let (target_in, target_out) = widths[&edge.to_node];
                if target_in <= target_out {
                    let target_type = node_types[&edge.to_node];
                    return Err(PluginBuildDiagnostic::graph_edge(
                        edge.from_node,
                        edge.to_node,
                        format!(
                            "Plugin graph sidechain edge {} -> {} is invalid: node {} ('{}') exposes no key bus ({} in vs {} out)",
                            edge.from_node,
                            edge.to_node,
                            edge.to_node,
                            target_type,
                            target_in,
                            target_out
                        ),
                    ));
                }
                let key_bus = target_in - target_out;
                let source_out = widths[&edge.from_node].1;
                let used = key_usage.entry(edge.to_node).or_default();
                *used += source_out;
                if *used > key_bus {
                    let target_type = node_types[&edge.to_node];
                    return Err(PluginBuildDiagnostic::graph_edge(
                        edge.from_node,
                        edge.to_node,
                        format!(
                            "Plugin graph sidechain edge {} -> {} is invalid: routed key width {} exceeds the {}-channel key bus of node {} ('{}')",
                            edge.from_node, edge.to_node, *used, key_bus, edge.to_node, target_type
                        ),
                    ));
                }
                GraphEdge::sidechain(from, to)
            }
        };
        host.add_edge(host_edge).map_err(|error| {
            PluginBuildDiagnostic::graph_edge(
                edge.from_node,
                edge.to_node,
                format!(
                    "Plugin graph edge {} -> {} is invalid: {error}",
                    edge.from_node, edge.to_node
                ),
            )
        })?;
    }

    host.build().map_err(|error| {
        PluginBuildDiagnostic::host(format!("Plugin graph host build failed: {error}"))
    })?;

    // Serialized node positions are the engine's stable control/telemetry
    // indices, independent of branch scheduling inside a processing stage.
    let control_order: Vec<_> = config.nodes.iter().map(|node| id_map[&node.id]).collect();
    host.set_graph_plugin_order(&control_order)
        .map_err(PluginBuildDiagnostic::host)?;

    log::info!(
        "[Processing Thread] Plugin graph loaded: {} nodes, {} edges ({}ch -> {}ch), {} warnings",
        id_map.len(),
        config.edges.len(),
        channels,
        host.output_channels(),
        0
    );

    Ok((host, Vec::new()))
}

#[cfg(test)]
#[path = "build/aud142_crossover_tests.rs"]
mod aud142_crossover_tests;
