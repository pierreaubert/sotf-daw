//! Hetero drain-plan branch pin: envelope vs live derivation.
//!
//! Hetero graphs (resampler converters + transparent gain join) publish
//! drain and process envelopes on every node, so `rebuild_graph_drain_plan`
//! must take the envelope branch; the pin below asserts the branch flag on
//! the hetero SHAPE (two sources into one join) with envelope-publishing
//! fixtures, plus a live-fallback control with one publish-less node.
//! Behavior is safe either way (one-direction growth, loud overflow); the
//! pin prevents silent plan-shape drift.

use super::super::daw_host::DawHost;
use super::super::graph_edge::GraphEdge;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginInfo, ProcessContext};

const CHANNELS: usize = 2;
const RATE: u32 = 48_000;
const DRAIN_ENV: usize = 128;

/// Memoryless passthrough fixture with optional envelope publication.
struct PlanFixture {
    channels: usize,
    publish_envelopes: bool,
}

impl Plugin for PlanFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("PlanFixture", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("PlanFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn drain_output_frames_max(&self) -> usize {
        0
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        self.publish_envelopes.then_some(DRAIN_ENV)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        self.publish_envelopes.then_some(input_frames)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }
}

fn hetero_host(join_publishes: bool) -> DawHost {
    let mut host = DawHost::new(CHANNELS, RATE);
    let source_a = host
        .add_node(
            "source a".to_string(),
            Box::new(PlanFixture {
                channels: CHANNELS,
                publish_envelopes: true,
            }),
        )
        .unwrap();
    let source_b = host
        .add_node(
            "source b".to_string(),
            Box::new(PlanFixture {
                channels: CHANNELS,
                publish_envelopes: true,
            }),
        )
        .unwrap();
    let join = host
        .add_node(
            "join".to_string(),
            Box::new(PlanFixture {
                channels: CHANNELS,
                publish_envelopes: join_publishes,
            }),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source_a, join)).unwrap();
    host.add_edge(GraphEdge::new(source_b, join)).unwrap();
    host.build().unwrap();
    host
}

#[test]
fn hetero_graph_with_envelopes_takes_envelope_branch() {
    let host = hetero_host(true);
    let envelope = host.graph_drain_envelope_bound_for_test();
    assert!(
        envelope.is_some(),
        "all-publishing hetero graph must take the envelope branch"
    );
    assert_eq!(
        envelope.unwrap(),
        host.drain_output_frames_max(),
        "the public bound query must reflect the envelope branch"
    );
}

#[test]
fn hetero_graph_without_one_envelope_falls_back_to_live() {
    let host = hetero_host(false);
    assert_eq!(
        host.graph_drain_envelope_bound_for_test(),
        None,
        "one publish-less node must keep the live derivation"
    );
}
