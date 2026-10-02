//! Plugin configuration types for serialization/deserialization.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};

/// Plugin configuration for serialization/deserialization
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin type identifier
    pub plugin_type: String,
    /// Plugin parameters
    pub parameters: serde_json::Value,
}

impl PluginConfig {
    /// Create a new plugin config
    pub fn new(plugin_type: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self {
            plugin_type: plugin_type.into(),
            parameters,
        }
    }

    /// Create a plugin config and validate its invariants.
    pub fn try_new(
        plugin_type: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Result<Self, Cow<'static, str>> {
        let config = Self::new(plugin_type, parameters);
        config.validate()?;
        Ok(config)
    }

    /// Validate plugin config invariants that serde cannot express.
    pub fn validate(&self) -> Result<(), Cow<'static, str>> {
        if self.plugin_type.trim().is_empty() {
            return Err(Cow::Borrowed("plugin_type must not be empty"));
        }

        Ok(())
    }
}

/// Graph-based plugin configuration for DAG processing.
///
/// Unlike `Vec<PluginConfig>` (linear chain), this supports parallel paths
/// needed for multi-driver crossover setups.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginGraphConfig {
    pub nodes: Vec<PluginGraphNodeConfig>,
    pub edges: Vec<PluginGraphEdgeConfig>,
}

impl PluginGraphConfig {
    /// Create a plugin graph config and validate that it is a DAG.
    pub fn try_new(
        nodes: Vec<PluginGraphNodeConfig>,
        edges: Vec<PluginGraphEdgeConfig>,
    ) -> Result<Self, Cow<'static, str>> {
        let config = Self { nodes, edges };
        config.validate()?;
        Ok(config)
    }

    /// Validate graph invariants: unique nodes, valid endpoints, and acyclicity.
    pub fn validate(&self) -> Result<(), Cow<'static, str>> {
        let mut node_ids = HashSet::with_capacity(self.nodes.len());
        for node in &self.nodes {
            node.validate()?;
            if !node_ids.insert(node.id) {
                return Err(Cow::Owned(format!(
                    "duplicate plugin graph node id {}",
                    node.id
                )));
            }
        }

        let incoming_counts: HashMap<usize, usize> =
            self.nodes.iter().map(|node| (node.id, 0)).collect();

        for edge in &self.edges {
            if !node_ids.contains(&edge.from_node) {
                return Err(Cow::Owned(format!(
                    "plugin graph edge references missing from_node {}",
                    edge.from_node
                )));
            }
            if !node_ids.contains(&edge.to_node) {
                return Err(Cow::Owned(format!(
                    "plugin graph edge references missing to_node {}",
                    edge.to_node
                )));
            }
        }

        let mut outgoing_edges: HashMap<usize, Vec<usize>> =
            HashMap::with_capacity(self.edges.len());
        for edge in &self.edges {
            outgoing_edges
                .entry(edge.from_node)
                .or_default()
                .push(edge.to_node);
        }

        validate_graph_topology(
            &self.edges,
            &outgoing_edges,
            incoming_counts,
            self.nodes.len(),
        )
    }
}

fn validate_graph_topology(
    edges: &[PluginGraphEdgeConfig],
    outgoing_edges: &HashMap<usize, Vec<usize>>,
    mut incoming_counts: HashMap<usize, usize>,
    num_nodes: usize,
) -> Result<(), Cow<'static, str>> {
    for edge in edges {
        if let Some(count) = incoming_counts.get_mut(&edge.to_node) {
            *count += 1;
        } else {
            return Err(Cow::Owned(format!(
                "internal error: incoming count missing for node {}",
                edge.to_node
            )));
        }
    }

    let mut ready: VecDeque<usize> = incoming_counts
        .iter()
        .filter_map(|(&node_id, &count)| (count == 0).then_some(node_id))
        .collect();
    let mut visited = 0;

    while let Some(node_id) = ready.pop_front() {
        visited += 1;
        if let Some(targets) = outgoing_edges.get(&node_id) {
            for &target in targets {
                if let Some(count) = incoming_counts.get_mut(&target) {
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(target);
                    }
                } else {
                    return Err(Cow::Owned(format!(
                        "internal error: incoming count missing for node {}",
                        target
                    )));
                }
            }
        }
    }

    if visited != num_nodes {
        return Err(Cow::Borrowed("plugin graph must be acyclic"));
    }

    Ok(())
}

/// A node in the plugin graph
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginGraphNodeConfig {
    /// Unique node ID (used to reference in edges)
    pub id: usize,
    pub plugin_type: String,
    pub parameters: serde_json::Value,
    /// Number of input channels this node expects
    pub input_channels: usize,
    /// Whether this node is bypassed while preserving graph topology.
    #[serde(default)]
    pub bypassed: bool,
}

impl PluginGraphNodeConfig {
    /// Create a plugin graph node config and validate its local invariants.
    pub fn try_new(
        id: usize,
        plugin_type: impl Into<String>,
        parameters: serde_json::Value,
        input_channels: usize,
    ) -> Result<Self, Cow<'static, str>> {
        let config = Self {
            id,
            plugin_type: plugin_type.into(),
            parameters,
            input_channels,
            bypassed: false,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate node-local invariants.
    pub fn validate(&self) -> Result<(), Cow<'static, str>> {
        if self.plugin_type.trim().is_empty() {
            return Err(Cow::Owned(format!(
                "plugin graph node {} plugin_type is empty",
                self.id
            )));
        }
        if self.input_channels == 0 {
            return Err(Cow::Owned(format!(
                "plugin graph node {} input_channels must be greater than 0",
                self.id
            )));
        }

        Ok(())
    }
}

/// How a graph edge feeds its target node.
///
/// `Audio` (the default) mixes into the target's program bus; `Sidechain`
/// appends after the program bus into the target's key bus, which the
/// target's detector reads instead of (or in addition to) the program.
/// Serialized as `"sidechain"` only when non-default, so audio edges keep
/// byte-identical persisted form; a missing `kind` field deserializes to
/// `Audio` so graphs persisted before sidechain routing keep identical
/// behavior in both directions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginGraphEdgeKind {
    /// Program audio mixed into the target's primary input channels.
    #[default]
    Audio,
    /// Key signal appended after the program bus for external detection.
    Sidechain,
}

impl PluginGraphEdgeKind {
    /// Whether this kind keeps the legacy audio-only serialized form.
    pub fn is_audio(&self) -> bool {
        matches!(self, PluginGraphEdgeKind::Audio)
    }
}

/// An edge connecting two nodes in the plugin graph
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginGraphEdgeConfig {
    pub from_node: usize,
    pub to_node: usize,
    /// Edge kind; defaults to [`PluginGraphEdgeKind::Audio`] when absent
    /// and is omitted on serialize when audio, so persisted audio-only
    /// graphs keep byte-identical form in both directions.
    #[serde(default, skip_serializing_if = "PluginGraphEdgeKind::is_audio")]
    pub kind: PluginGraphEdgeKind,
}

impl PluginGraphEdgeConfig {
    /// Create an audio plugin graph edge. Endpoint existence is validated
    /// by `PluginGraphConfig`.
    pub fn new(from_node: usize, to_node: usize) -> Self {
        Self {
            from_node,
            to_node,
            kind: PluginGraphEdgeKind::Audio,
        }
    }

    /// Create a sidechain plugin graph edge feeding the target's key bus.
    /// Endpoint existence is validated by `PluginGraphConfig`; key-bus
    /// geometry is validated when the graph host is built.
    pub fn sidechain(from_node: usize, to_node: usize) -> Self {
        Self {
            from_node,
            to_node,
            kind: PluginGraphEdgeKind::Sidechain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: usize) -> PluginGraphNodeConfig {
        PluginGraphNodeConfig {
            id,
            plugin_type: "gain".to_string(),
            parameters: json!({}),
            input_channels: 2,
            bypassed: false,
        }
    }

    #[test]
    fn plugin_config_rejects_empty_type() {
        let error = PluginConfig::try_new(" ", json!({})).unwrap_err();
        assert!(error.contains("plugin_type"));
        assert!(
            matches!(error, Cow::Borrowed(_)),
            "static validation errors should not allocate"
        );
    }

    #[test]
    fn plugin_graph_accepts_valid_dag() {
        let graph = PluginGraphConfig::try_new(
            vec![node(0), node(1), node(2)],
            vec![
                PluginGraphEdgeConfig::new(0, 1),
                PluginGraphEdgeConfig::new(0, 2),
            ],
        )
        .unwrap();

        assert_eq!(graph.nodes.len(), 3);
    }

    #[test]
    fn plugin_graph_rejects_duplicate_node_ids() {
        let error = PluginGraphConfig::try_new(vec![node(0), node(0)], vec![]).unwrap_err();
        assert!(error.contains("duplicate"));
    }

    #[test]
    fn plugin_graph_rejects_missing_edge_endpoint() {
        let error =
            PluginGraphConfig::try_new(vec![node(0)], vec![PluginGraphEdgeConfig::new(0, 1)])
                .unwrap_err();
        assert!(error.contains("to_node"));
    }

    #[test]
    fn plugin_graph_rejects_cycles() {
        let error = PluginGraphConfig::try_new(
            vec![node(0), node(1), node(2)],
            vec![
                PluginGraphEdgeConfig::new(0, 1),
                PluginGraphEdgeConfig::new(1, 2),
                PluginGraphEdgeConfig::new(2, 0),
            ],
        )
        .unwrap_err();
        assert!(error.contains("acyclic"));
        assert!(
            matches!(error, Cow::Borrowed(_)),
            "static validation errors should not allocate"
        );
    }

    #[test]
    fn plugin_graph_rejects_zero_channel_nodes() {
        let mut invalid = node(0);
        invalid.input_channels = 0;

        let error = PluginGraphConfig::try_new(vec![invalid], vec![]).unwrap_err();
        assert!(error.contains("input_channels"));
    }

    #[test]
    fn plugin_graph_node_bypass_is_version_tolerant() {
        let legacy: PluginGraphNodeConfig = serde_json::from_value(json!({
            "id": 1,
            "plugin_type": "gain",
            "parameters": {},
            "input_channels": 2
        }))
        .unwrap();
        assert!(!legacy.bypassed);

        let bypassed: PluginGraphNodeConfig = serde_json::from_value(json!({
            "id": 1,
            "plugin_type": "gain",
            "parameters": {},
            "input_channels": 2,
            "bypassed": true
        }))
        .unwrap();
        assert!(bypassed.bypassed);
    }

    #[test]
    fn plugin_config_accepts_valid() {
        let config = PluginConfig::try_new("eq", json!({"freq": 1000.0})).unwrap();
        assert_eq!(config.plugin_type, "eq");
    }

    #[test]
    fn plugin_config_validate_accepts_non_empty_type() {
        let config = PluginConfig::new("gain", json!({}));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn plugin_graph_accepts_empty_graph() {
        let graph = PluginGraphConfig::try_new(vec![], vec![]).unwrap();
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn plugin_graph_accepts_disconnected_nodes() {
        let graph = PluginGraphConfig::try_new(vec![node(0), node(1)], vec![]).unwrap();
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn plugin_graph_rejects_self_loop() {
        let error =
            PluginGraphConfig::try_new(vec![node(0)], vec![PluginGraphEdgeConfig::new(0, 0)])
                .unwrap_err();
        assert!(error.contains("acyclic"));
    }

    #[test]
    fn plugin_graph_rejects_missing_from_node() {
        let error =
            PluginGraphConfig::try_new(vec![node(1)], vec![PluginGraphEdgeConfig::new(0, 1)])
                .unwrap_err();
        assert!(error.contains("from_node"));
    }

    #[test]
    fn plugin_graph_accepts_multiple_edges() {
        let graph = PluginGraphConfig::try_new(
            vec![node(0), node(1)],
            vec![
                PluginGraphEdgeConfig::new(0, 1),
                PluginGraphEdgeConfig::new(0, 1),
            ],
        )
        .unwrap();
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn plugin_graph_rejects_large_cycle() {
        let error = PluginGraphConfig::try_new(
            vec![node(0), node(1), node(2), node(3)],
            vec![
                PluginGraphEdgeConfig::new(0, 1),
                PluginGraphEdgeConfig::new(1, 2),
                PluginGraphEdgeConfig::new(2, 3),
                PluginGraphEdgeConfig::new(3, 0),
            ],
        )
        .unwrap_err();
        assert!(error.contains("acyclic"));
    }

    #[test]
    fn plugin_graph_node_try_new_valid_and_invalid() {
        let valid = PluginGraphNodeConfig::try_new(7, "delay", json!({"ms": 100}), 6).unwrap();
        assert_eq!(valid.id, 7);
        assert_eq!(valid.input_channels, 6);

        let err = PluginGraphNodeConfig::try_new(8, "   ", json!({}), 2).unwrap_err();
        assert!(err.contains("plugin_type"));

        let err = PluginGraphNodeConfig::try_new(9, "mixer", json!({}), 0).unwrap_err();
        assert!(err.contains("input_channels"));
    }

    #[test]
    fn validate_graph_topology_rejects_missing_incoming_count_increment() {
        let edges = vec![PluginGraphEdgeConfig::new(0, 1)];
        let outgoing_edges = std::collections::HashMap::new();
        let mut incoming_counts = std::collections::HashMap::new();
        incoming_counts.insert(0, 0);
        let result = validate_graph_topology(&edges, &outgoing_edges, incoming_counts, 2);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("incoming count missing for node 1")
        );
    }

    #[test]
    fn validate_graph_topology_rejects_missing_incoming_count_decrement() {
        let edges = vec![];
        let mut outgoing_edges = std::collections::HashMap::new();
        outgoing_edges.insert(0, vec![1]);
        let mut incoming_counts = std::collections::HashMap::new();
        incoming_counts.insert(0, 0);
        let result = validate_graph_topology(&edges, &outgoing_edges, incoming_counts, 2);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("incoming count missing for node 1")
        );
    }

    #[test]
    fn edge_kind_defaults_to_audio_and_constructors_agree() {
        assert_eq!(PluginGraphEdgeKind::default(), PluginGraphEdgeKind::Audio);
        let audio = PluginGraphEdgeConfig::new(0, 1);
        assert_eq!(audio.kind, PluginGraphEdgeKind::Audio);
        let key = PluginGraphEdgeConfig::sidechain(0, 1);
        assert_eq!(key.from_node, 0);
        assert_eq!(key.to_node, 1);
        assert_eq!(key.kind, PluginGraphEdgeKind::Sidechain);
    }

    #[test]
    fn edge_kind_is_version_tolerant() {
        // Graphs persisted before sidechain routing carry no kind field and
        // must keep audio behavior.
        let legacy: PluginGraphEdgeConfig =
            serde_json::from_value(json!({"from_node": 7, "to_node": 42})).unwrap();
        assert_eq!(legacy.kind, PluginGraphEdgeKind::Audio);

        let explicit_audio: PluginGraphEdgeConfig = serde_json::from_value(json!({
            "from_node": 7,
            "to_node": 42,
            "kind": "audio"
        }))
        .unwrap();
        assert_eq!(explicit_audio, PluginGraphEdgeConfig::new(7, 42));

        let key: PluginGraphEdgeConfig = serde_json::from_value(json!({
            "from_node": 7,
            "to_node": 42,
            "kind": "sidechain"
        }))
        .unwrap();
        assert_eq!(key, PluginGraphEdgeConfig::sidechain(7, 42));

        // Round-trips preserve the kind in both directions, and audio
        // edges keep the byte-identical legacy form with no kind field.
        assert_eq!(
            serde_json::to_value(PluginGraphEdgeConfig::new(7, 42)).unwrap(),
            json!({"from_node": 7, "to_node": 42})
        );
        for edge in [
            PluginGraphEdgeConfig::new(1, 2),
            PluginGraphEdgeConfig::sidechain(1, 2),
        ] {
            let value = serde_json::to_value(&edge).unwrap();
            assert_eq!(
                serde_json::from_value::<PluginGraphEdgeConfig>(value).unwrap(),
                edge
            );
        }
    }

    #[test]
    fn plugin_graph_validates_mixed_edge_endpoints_and_cycles() {
        let error =
            PluginGraphConfig::try_new(vec![node(1)], vec![PluginGraphEdgeConfig::sidechain(0, 1)])
                .unwrap_err();
        assert!(error.contains("from_node"));

        // Sidechain participation does not exempt an edge from DAG order.
        let error = PluginGraphConfig::try_new(
            vec![node(0), node(1)],
            vec![
                PluginGraphEdgeConfig::new(0, 1),
                PluginGraphEdgeConfig::sidechain(1, 0),
            ],
        )
        .unwrap_err();
        assert!(error.contains("acyclic"));

        // A key tap alongside program audio is a valid DAG.
        let graph = PluginGraphConfig::try_new(
            vec![node(0), node(1), node(2)],
            vec![
                PluginGraphEdgeConfig::new(0, 2),
                PluginGraphEdgeConfig::sidechain(1, 2),
            ],
        )
        .unwrap();
        assert_eq!(graph.edges.len(), 2);
    }
}
