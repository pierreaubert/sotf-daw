//! Plugin factory and path builder for A/B Compare plugin.

use super::config::{GraphEdgeConfig, GraphNodeConfig, PathConfig};
use sotf_host::PluginFactoryFn;
use sotf_host::host::{DawHost, GraphEdge};
use sotf_host::plugin::Plugin;
use sotf_host::{ParametricInPlacePluginAdapter, ParametricPluginAdapter};
use sotf_plugin_delay::{DelayPlugin, DelayPluginParams};
use sotf_plugin_eq::{EqPlugin, EqPluginParams};
use sotf_plugin_gain::{GainPlugin, GainPluginParams};
use sotf_plugin_gate::{GatePlugin, GatePluginParams};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};
use sotf_plugin_resampler::ResamplerPlugin;
use std::collections::HashMap;

/// Input-frame chunk for ABCompare-owned clock converters (path output clock
/// back to the outer comparison clock).
///
/// 256 frames bounds converter burst granularity (per-chunk output is about
/// chunk x ratio plus rubato jitter) well inside the process staging queues
/// while keeping converter priming latency (chunk - 1 input frames plus the
/// rubato delay, converted to outer-clock frames by existing host latency
/// accounting) small relative to the maximum realtime block. Larger chunks
/// only grow latency and bursts; correctness never depends on this value.
const CONVERTER_CHUNK_FRAMES: usize = 256;

/// Create a plugin, delegating to the external factory if provided,
/// falling back to the built-in limited factory.
fn create_plugin(
    plugin_type: &str,
    parameters: &serde_json::Value,
    num_channels: usize,
    sample_rate: u32,
    external_factory: Option<PluginFactoryFn>,
) -> Result<Box<dyn Plugin>, String> {
    if let Some(factory) = external_factory {
        return factory(plugin_type, parameters, num_channels, sample_rate);
    }
    // Fallback: built-in limited factory (6 types)
    create_plugin_builtin(plugin_type, parameters, num_channels, sample_rate)
}

/// Built-in factory supporting a minimal set of plugin types.
/// Used when no external factory is provided (e.g., in unit tests).
fn create_plugin_builtin(
    plugin_type: &str,
    parameters: &serde_json::Value,
    num_channels: usize,
    sample_rate: u32,
) -> Result<Box<dyn Plugin>, String> {
    match plugin_type.to_lowercase().as_str() {
        "eq" => {
            let params: EqPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Invalid EQ params: {}", e))?;
            EqPlugin::from_params(num_channels, sample_rate, params).map(|p| p.into_boxed_plugin())
        }
        "gain" => {
            let params: GainPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Invalid Gain params: {}", e))?;
            let plugin = GainPlugin::from_params(num_channels, params)?;
            Ok(Box::new(ParametricPluginAdapter::new(plugin)))
        }
        "compressor" => {
            let params: MultibandCompressorPluginParams =
                serde_json::from_value(parameters.clone())
                    .map_err(|e| format!("Invalid Compressor params: {}", e))?;
            let plugin =
                MultibandCompressorPlugin::try_from_params(num_channels, params, sample_rate)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }
        "limiter" => {
            let params: LimiterPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Invalid Limiter params: {}", e))?;
            let plugin = LimiterPlugin::from_params(num_channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }
        "gate" => {
            let params: GatePluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Invalid Gate params: {}", e))?;
            let plugin = GatePlugin::from_params(num_channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }
        "delay" => {
            let params: DelayPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Invalid Delay params: {}", e))?;
            let plugin = DelayPlugin::from_params(num_channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }
        _ => Err(format!("Unknown plugin type: {}", plugin_type)),
    }
}

/// Build a DawHost from a PathConfig, optionally using an external plugin factory.
pub fn build_path_from_config(
    config: &PathConfig,
    num_channels: usize,
    sample_rate: u32,
) -> Result<DawHost, String> {
    build_path_from_config_with_factory(config, num_channels, sample_rate, None)
}

/// Build a DawHost from a PathConfig with an explicit factory function.
///
/// The returned host always produces the outer `sample_rate` clock: chain
/// (plugin/rack) paths whose output clock differs gain an ABCompare-owned
/// production-resampler converter stage appended to the chain, so downstream
/// staging, latency, and drain logic observe honest same-clock variable
/// production. Single-sink graph paths convert the same way, and
/// multi-sink graphs convert per sink plus one transparent output join:
/// mid-graph stages construct and initialize at their topologically
/// resolved input clocks, each mismatched sink gains the owned converter
/// appended at its own sink clock, and the join sums every (converted)
/// sink through the host's fan-in merge with retention. Graphs the
/// factory cannot clock honestly (fan-in disagreement, cycles) fail
/// loudly here instead of corrupting silently downstream.
pub fn build_path_from_config_with_factory(
    config: &PathConfig,
    num_channels: usize,
    sample_rate: u32,
    factory: Option<PluginFactoryFn>,
) -> Result<DawHost, String> {
    let mut host = DawHost::new(num_channels, sample_rate);

    let (graph_built, chain_final_rate) = match config {
        PathConfig::None => {
            // Empty host = pass-through, already at the outer clock.
            return Ok(host);
        }
        PathConfig::Plugin {
            plugin_type,
            parameters,
        } => {
            let plugin =
                create_plugin(plugin_type, parameters, num_channels, sample_rate, factory)?;
            let folded = plugin.output_sample_rate(sample_rate);
            host.add_plugin(plugin)?;
            (None, Some(folded))
        }
        PathConfig::Rack { plugins } => {
            // Running fold: each stage constructs at the upstream output
            // clock, so coefficient-bearing stages design for their true
            // input rate. Single construction in rack order; no factory
            // purity assumption, no double-construct.
            let mut running_rate = sample_rate;
            for p in plugins {
                let plugin = create_plugin(
                    &p.plugin_type,
                    &p.parameters,
                    num_channels,
                    running_rate,
                    factory,
                )?;
                running_rate = plugin.output_sample_rate(running_rate);
                host.add_plugin(plugin)?;
            }
            (None, Some(running_rate))
        }
        PathConfig::Graph { nodes, edges } => (
            Some(build_graph(
                &mut host,
                nodes,
                edges,
                num_channels,
                sample_rate,
                factory,
            )?),
            None,
        ),
    };

    normalize_path_output_clock(
        &mut host,
        config,
        graph_built.as_ref().map(|built| &built.node_ids),
        graph_built.as_ref().map(|built| &built.output_clocks),
        chain_final_rate,
        num_channels,
        sample_rate,
    )?;

    Ok(host)
}

/// Append an ABCompare-owned converter when a path ends at a different clock.
///
/// Chain append uses `add_plugin`, which folds the existing chain rates to
/// initialize the converter at exactly the path output rate; the following
/// rebuild wires rates, latency, and drain plans through existing host
/// accounting. Single-sink graph append uses `add_node_at_rate` at the sink
/// clock with an explicit sink edge for the same accounting, and multi-sink
/// graphs convert per sink plus one transparent output join. Graphs the
/// factory cannot clock honestly fail here with the exact gap instead of
/// reaching the audio thread.
fn normalize_path_output_clock(
    host: &mut DawHost,
    config: &PathConfig,
    graph_node_ids: Option<&HashMap<String, usize>>,
    graph_output_clocks: Option<&HashMap<String, u32>>,
    chain_final_rate: Option<u32>,
    num_channels: usize,
    outer_rate: u32,
) -> Result<(), String> {
    // Multi-sink graphs arrive unbuilt: normalize converts, joins, and
    // builds them once below. All other paths build here for negotiation.
    if let PathConfig::Graph { nodes, edges } = config {
        let multi_sink = nodes
            .iter()
            .filter(|node| !edges.iter().any(|edge| edge.from == node.id))
            .take(2)
            .count()
            > 1;
        if multi_sink {
            return normalize_multi_sink_graph_output_clock(
                host,
                nodes,
                edges,
                graph_node_ids,
                graph_output_clocks,
                num_channels,
                outer_rate,
            );
        }
    }
    host.build()?;
    let path_rate = host.output_sample_rate(outer_rate);
    // The factory fold must agree with host negotiation exactly: any drift
    // means a stale running rate, which fails here instead of clocking a
    // stage wrong downstream.
    if let Some(expected) = chain_final_rate
        && path_rate != expected
    {
        return Err(format!(
            "A/B Compare chain folded to {expected} Hz but the host negotiated {path_rate} Hz; refusing a stale running rate"
        ));
    }
    if let PathConfig::Graph { nodes, edges } = config {
        if path_rate == outer_rate {
            return Ok(());
        }
        return normalize_graph_output_clock(
            host,
            nodes,
            edges,
            graph_node_ids,
            num_channels,
            path_rate,
            outer_rate,
        );
    }
    if path_rate == outer_rate {
        return Ok(());
    }
    let converter =
        ResamplerPlugin::new(num_channels, path_rate, outer_rate, CONVERTER_CHUNK_FRAMES)
            .map_err(|error| format!("A/B Compare clock converter failed: {error}"))?;
    host.add_plugin(Box::new(converter))?;
    host.build()?;
    let converted_rate = host.output_sample_rate(outer_rate);
    if converted_rate != outer_rate {
        return Err(format!(
            "A/B Compare clock converter produced {converted_rate} Hz instead of {outer_rate} Hz"
        ));
    }
    Ok(())
}

/// Append per-sink converters plus one transparent output join.
///
/// Each off-clock sink gains its owned converter at its own resolved
/// clock; then every (converted) sink feeds a single 0 dB gain join, so
/// the host's fan-in merge — per-edge retention queues plus
/// minimum-depth summation — composes heterogeneous per-call production
/// losslessly where minimum-count multi-output collection would drop a
/// longer sink's surplus. The join is bit-transparent (settled 0 dB from
/// construction, zero latency), so the joined output equals the native
/// multi-output sum with retention. The single post-join build observes
/// one outer-clock output.
fn normalize_multi_sink_graph_output_clock(
    host: &mut DawHost,
    nodes: &[GraphNodeConfig],
    edges: &[GraphEdgeConfig],
    node_ids: Option<&HashMap<String, usize>>,
    output_clocks: Option<&HashMap<String, u32>>,
    num_channels: usize,
    outer_rate: u32,
) -> Result<(), String> {
    let sinks: Vec<&str> = nodes
        .iter()
        .filter(|node| !edges.iter().any(|edge| edge.from == node.id))
        .map(|node| node.id.as_str())
        .collect();
    let mut taken: Vec<String> = nodes.iter().map(|node| node.id.clone()).collect();
    let mut join_sources: Vec<usize> = Vec::with_capacity(sinks.len());
    for sink in &sinks {
        let clock = output_clocks
            .and_then(|clocks| clocks.get(*sink).copied())
            .ok_or_else(|| format!("A/B Compare graph lost the resolved clock of sink {sink}"))?;
        let sink_id = node_ids
            .and_then(|ids| ids.get(*sink).copied())
            .ok_or_else(|| format!("A/B Compare graph lost its sink node {sink}"))?;
        if clock == outer_rate {
            join_sources.push(sink_id);
            continue;
        }
        let converter =
            ResamplerPlugin::new(num_channels, clock, outer_rate, CONVERTER_CHUNK_FRAMES)
                .map_err(|error| format!("A/B Compare clock converter failed: {error}"))?;
        let mut name = String::from("abcompare-clock-converter");
        while taken.iter().any(|taken| taken == &name) {
            name.push('~');
        }
        taken.push(name.clone());
        let converter_id = host.add_node_at_rate(name, Box::new(converter), clock)?;
        host.add_edge(GraphEdge::new(sink_id, converter_id))?;
        join_sources.push(converter_id);
    }
    let join = ParametricPluginAdapter::new(GainPlugin::new(num_channels, 0.0));
    let mut join_name = String::from("abcompare-output-join");
    while taken.iter().any(|taken| taken == &join_name) {
        join_name.push('~');
    }
    let join_id = host.add_node_at_rate(join_name, Box::new(join), outer_rate)?;
    for source in join_sources {
        host.add_edge(GraphEdge::new(source, join_id))?;
    }
    host.build()?;
    if !host.all_output_sample_rates_equal(outer_rate, outer_rate) {
        return Err(format!(
            "A/B Compare multi-sink converters and output join did not all reach {outer_rate} Hz"
        ));
    }
    Ok(())
}

/// Append the owned converter to a single-sink graph path.
///
/// The converter initializes at the sink clock and wires behind the sink, so
/// the rebuild observes honest same-clock variable production at the outer
/// rate. Multi-sink graphs route to converters-plus-join instead.
fn normalize_graph_output_clock(
    host: &mut DawHost,
    nodes: &[GraphNodeConfig],
    edges: &[GraphEdgeConfig],
    node_ids: Option<&HashMap<String, usize>>,
    num_channels: usize,
    path_rate: u32,
    outer_rate: u32,
) -> Result<(), String> {
    // Single-sink only by dispatch (multi-sink routes to
    // converters-plus-join); a missing sink here fails closed below.
    let sink_name = nodes
        .iter()
        .find(|node| !edges.iter().any(|edge| edge.from == node.id))
        .map(|node| node.id.as_str());
    let sink = node_ids
        .and_then(|ids| sink_name.and_then(|name| ids.get(name).copied()))
        .ok_or_else(|| "A/B Compare graph lost its single sink node".to_string())?;
    let converter =
        ResamplerPlugin::new(num_channels, path_rate, outer_rate, CONVERTER_CHUNK_FRAMES)
            .map_err(|error| format!("A/B Compare clock converter failed: {error}"))?;
    let mut name = String::from("abcompare-clock-converter");
    while nodes.iter().any(|node| node.id == name) {
        name.push('~');
    }
    let converter_id = host.add_node_at_rate(name, Box::new(converter), path_rate)?;
    host.add_edge(GraphEdge::new(sink, converter_id))?;
    host.build()?;
    let converted_rate = host.output_sample_rate(outer_rate);
    if converted_rate != outer_rate {
        return Err(format!(
            "A/B Compare clock converter produced {converted_rate} Hz instead of {outer_rate} Hz"
        ));
    }
    Ok(())
}

/// Upstream-before-downstream construction order for graph nodes (config-only).
///
/// Returns the node indices in topological order plus the per-node incoming
/// adjacency. Unknown node ids and rate cycles fail here with the exact gap,
/// before any factory call, so construction never starts on an unbuildable
/// graph. Sibling order follows the same stack discipline as before for
/// deterministic construction.
fn graph_construction_order(
    nodes: &[GraphNodeConfig],
    edges: &[GraphEdgeConfig],
) -> Result<(Vec<usize>, Vec<Vec<usize>>), String> {
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for edge in edges {
        let from = nodes
            .iter()
            .position(|node| node.id == edge.from)
            .ok_or_else(|| format!("Unknown node id in edge: {}", edge.from))?;
        let to = nodes
            .iter()
            .position(|node| node.id == edge.to)
            .ok_or_else(|| format!("Unknown node id in edge: {}", edge.to))?;
        incoming[to].push(from);
    }
    let mut downstream: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (to, ups) in incoming.iter().enumerate() {
        for &from in ups {
            downstream[from].push(to);
        }
    }
    let mut pending: Vec<usize> = incoming.iter().map(Vec::len).collect();
    let mut ready: Vec<usize> = (0..nodes.len())
        .filter(|&index| incoming[index].is_empty())
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(index) = ready.pop() {
        order.push(index);
        for &down in &downstream[index] {
            pending[down] -= 1;
            if pending[down] == 0 {
                ready.push(down);
            }
        }
    }
    if order.len() != nodes.len() {
        let mut visited = vec![false; nodes.len()];
        for &index in &order {
            visited[index] = true;
        }
        let stuck = visited.iter().position(|&done| !done).unwrap_or(0);
        return Err(format!(
            "A/B Compare graph has a rate cycle involving node {}",
            nodes[stuck].id
        ));
    }
    Ok((order, incoming))
}

/// Graph construction outputs: host node ids plus resolved output clocks.
///
/// Per-sink conversion consumes the resolved clocks without re-querying
/// the host, so each converter initializes at its own sink clock.
struct ConstructedGraph {
    node_ids: HashMap<String, usize>,
    output_clocks: HashMap<String, u32>,
}

fn build_graph(
    host: &mut DawHost,
    nodes: &[GraphNodeConfig],
    edges: &[GraphEdgeConfig],
    num_channels: usize,
    sample_rate: u32,
    factory: Option<PluginFactoryFn>,
) -> Result<ConstructedGraph, String> {
    // Single construction in topological order: each node constructs at its
    // resolved input clock (outer for sources, upstream outputs otherwise),
    // so coefficient-bearing stages design for their true rate. Upstream
    // instances already exist when downstream resolves, so this needs no
    // factory purity assumption and no double-construction. Fan-in agreement
    // resolves before the fan-in node is created. No host mutation happens
    // until every rate resolves, preserving failure atomicity.
    let (order, incoming) = graph_construction_order(nodes, edges)?;
    let mut constructed: Vec<Option<Box<dyn Plugin>>> = Vec::with_capacity(nodes.len());
    constructed.resize_with(nodes.len(), || None);
    let mut input_rates: HashMap<String, u32> = HashMap::new();
    let mut output_rates: HashMap<String, u32> = HashMap::new();
    for &index in &order {
        let node = &nodes[index];
        let mut upstream = Vec::with_capacity(incoming[index].len());
        for &up in &incoming[index] {
            let upstream_plugin = constructed[up]
                .as_ref()
                .expect("topological construction visits upstream first");
            let upstream_input = input_rates
                .get(&nodes[up].id)
                .copied()
                .expect("topological construction resolves upstream input clocks first");
            upstream.push(upstream_plugin.output_sample_rate(upstream_input));
        }
        let mut rate = sample_rate;
        if let Some((&first, rest)) = upstream.split_first() {
            if rest.iter().any(|&upstream_rate| upstream_rate != first) {
                return Err(format!(
                    "A/B Compare graph node {} has disagreeing upstream clocks {upstream:?}; fan-in needs one input clock",
                    node.id
                ));
            }
            rate = first;
        }
        let plugin = create_plugin(
            &node.plugin_type,
            &node.parameters,
            num_channels,
            rate,
            factory,
        )?;
        output_rates.insert(node.id.clone(), plugin.output_sample_rate(rate));
        constructed[index] = Some(plugin);
        input_rates.insert(node.id.clone(), rate);
    }

    // Insertion order is irrelevant (no edges exist yet and every rate is
    // explicit), so config order is preserved. The construction clock above
    // is the insertion clock by construction: the same resolved `rate` feeds
    // both, so mid-graph stages initialize at their true input clock with no
    // stale rate possible.
    let mut node_ids: HashMap<String, usize> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let rate = input_rates.get(&node.id).copied().ok_or_else(|| {
            format!(
                "A/B Compare graph has no resolved input clock for node {}",
                node.id
            )
        })?;
        let plugin = constructed[index].take().ok_or_else(|| {
            format!(
                "A/B Compare graph lost node {} during construction",
                node.id
            )
        })?;
        let id = host.add_node_at_rate(node.id.clone(), plugin, rate)?;
        node_ids.insert(node.id.clone(), id);
    }

    for edge in edges {
        let from_id = *node_ids
            .get(&edge.from)
            .ok_or_else(|| format!("Unknown node id in edge: {}", edge.from))?;
        let to_id = *node_ids
            .get(&edge.to)
            .ok_or_else(|| format!("Unknown node id in edge: {}", edge.to))?;

        let graph_edge = match (&edge.channel_map, edge.destination_offset) {
            (Some(map), destination_offset) => {
                GraphEdge::with_channel_route(from_id, to_id, map.clone(), destination_offset)
            }
            (None, 0) => GraphEdge::new(from_id, to_id),
            (None, destination_offset) => {
                let source_channels = (0..num_channels).collect();
                GraphEdge::with_channel_route(from_id, to_id, source_channels, destination_offset)
            }
        };
        host.add_edge(graph_edge)?;
    }

    // Multi-sink graphs skip this build: normalize appends per-sink
    // converters and the transparent output join first, then builds once.
    // (An off-clock hetero graph would fail here on incompatible output
    // rates before conversion could land.)
    let multi_sink = nodes
        .iter()
        .filter(|node| !edges.iter().any(|edge| edge.from == node.id))
        .take(2)
        .count()
        > 1;
    if !multi_sink {
        host.build()?;
    }
    Ok(ConstructedGraph {
        node_ids,
        output_clocks: output_rates,
    })
}
