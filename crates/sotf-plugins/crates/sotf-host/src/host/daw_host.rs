use super::Host;
use super::audio_sample::AudioSample;
use super::audio_sample::ensure_len;
use super::audio_sample::write_plugin_failure_passthrough;
use super::buffer_guard::BufferGuard;
use super::compensation_delays::CompensationDelays;
use super::compiled_plan::{
    CompiledGraphPlan, CompiledLinearPlan, CompiledOp, CompiledOpKind, CompiledRenderPlan,
    build_segments,
};
use super::delay_buffer::DelayBuffer;
use super::exact_clock::{ExactClock, ExactRate};
use super::graph_edge::GraphEdge;
use super::graph_mutation_sender::GraphMutationSender;
use super::graph_node::GraphNode;
use super::graph_topology::GraphTopology;
use super::misc::DEFAULT_PARALLEL_NODE_COST;
use super::misc::GRAPH_MUTATION_QUEUE_CAPACITY;
use super::misc::HEAVY_PARALLEL_NODE_COST;
use super::misc::MIN_PARALLEL_STAGE_WORK_UNITS;
use super::misc::MODERATE_PARALLEL_NODE_COST;
use super::misc::PARAMETER_EVENT_QUEUE_CAPACITY;
use super::misc::panic_payload_description;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::misc::sandbox_reason_text;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::misc::sandbox_unsupported_detail;
use super::node_buffer::NodeBuffer;
use super::parameter_event::ParameterEvent;
use super::parameter_event_sender::ParameterEventSender;
use super::processing_stage::ProcessingStage;
use super::types::AutomationSlot;
use super::types::EdgeType;
use super::types::GraphMutation;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::types::IsolatedExternalPluginWorkerReport;
use super::types::NodeId;
use super::types::ProcessBuffers;
use crate::automation::{ParameterAutomation, automation_utils};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::external_plugin_isolated::IsolatedExternalPlugin;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::external_plugin_process::ExternalPluginProcessEvent;
use crate::parameters::{ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginCompiledOp, PluginCostClass, PluginDrainResult, ProcessContext,
    SinkAppendFailure, SinkQueueState, SinkServiceFailure, SinkTailPreflightError,
    SinkTransportFormat, SinkTransportRecoveryError as PluginSinkRecoveryError,
    SinkTransportRecoveryStatus, SinkTransportRepreparePlan, TailLength,
};
use arc_swap::ArcSwap;
use rayon::prelude::*;
use rtrb::{Consumer, Producer, RingBuffer};
use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledLinearSource {
    ExternalInput,
    ScratchInput,
    ScratchOutput,
}

fn plugin_compiled_op(kind: CompiledOpKind) -> Option<PluginCompiledOp> {
    match kind {
        CompiledOpKind::ApplyGain => Some(PluginCompiledOp::ApplyGain),
        CompiledOpKind::EqBiquadBank => Some(PluginCompiledOp::EqBiquadBank),
        CompiledOpKind::ChannelMuteSolo => Some(PluginCompiledOp::ChannelMuteSolo),
        CompiledOpKind::Limiter => Some(PluginCompiledOp::Limiter),
        CompiledOpKind::MultibandCompressor => Some(PluginCompiledOp::MultibandCompressor),
        CompiledOpKind::AnalyzerTap => Some(PluginCompiledOp::AnalyzerTap),
        _ => None,
    }
}

pub(super) struct DawQueueEndpoints {
    pub(super) parameter_event_tx: Option<Producer<ParameterEvent>>,
    pub(super) parameter_event_rx: Consumer<ParameterEvent>,
    pub(super) parameter_event_scratch: Vec<ParameterEvent>,
    pub(super) graph_mutation_tx: Option<Producer<GraphMutation>>,
    pub(super) graph_mutation_rx: Consumer<GraphMutation>,
}

pub(super) struct DawQueueState {
    pub(super) graph_next_node_id: Arc<AtomicUsize>,
    pub(super) dropped_parameter_events: u64,
    pub(super) dropped_graph_mutations: u64,
}

pub(super) struct DawAutomationState {
    /// Parameter automation state. Key = (NodeId, ParameterId).
    /// Evaluated before each processing stage.
    pub(super) automation: Vec<AutomationSlot>,
    /// Control-thread lookup for automation slots. The audio path iterates
    /// `automation` by index and never hashes `(NodeId, ParameterId)`.
    pub(super) automation_index: HashMap<(NodeId, ParameterId), usize>,
    /// Current playback position in samples, advanced each process() call.
    pub(super) playback_position: usize,
}

pub(super) struct DawConfig {
    pub(super) sample_rate: f64,
    pub(super) parallel_enabled: bool,
    pub(super) compiled_linear_enabled: bool,
    pub(super) initial_input_channels: usize,
    /// Cached worst-case plugin work quantum in host-input-rate frames.
    pub(super) realtime_quantum_frames: usize,
    /// Whether plugins may request their own preferred oversampling wrapper.
    pub(super) plugin_preferred_oversampling_enabled: bool,
    /// Force an oversampling wrapper around all same-I/O plugins when set.
    pub(super) forced_oversampling_factor: Option<u32>,
    /// Current immutable topology snapshot, published with ArcSwap after build.
    pub(super) topology: Arc<ArcSwap<GraphTopology>>,
    pub(super) f64_input_scratch: Vec<f32>,
    pub(super) f64_output_scratch: Vec<f32>,
    pub(super) f64_chain_scratch: Vec<f64>,
    pub(super) f64_chain_scratch_alt: Vec<f64>,
}

/// Unknown plugins retain the historical finite allowance, independently per stage.
const UNKNOWN_DRAIN_CALL_LIMIT: u64 = 4096;

#[derive(Default)]
struct DrainState {
    completed_prefix: usize,
    active_node: Option<NodeId>,
    prepared: bool,
    remaining_calls: Option<u64>,
    /// Live tail observed when the current quota was granted (`None`
    /// exactly when no quota is granted). The progress-gated single
    /// refresh compares the exhaustion-time tail against this value.
    quota_grant_tail: Option<TailLength>,
}

/// Lifecycle of one node inside the branched graph EOS scheduler.
///
/// `Running` nodes still expect input waves and consume aligned frames
/// from their incoming edge queues. `Draining` nodes saw every incoming
/// edge reach EOF with empty queues, so no further `process()` call can
/// arrive; they emit their own tail exactly like a linear stage. The
/// Running-to-Draining transition is the graph form of the linear
/// `completed_prefix` rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum GraphNodeDrainPhase {
    #[default]
    Running,
    Draining {
        prepared: bool,
        remaining_calls: Option<u64>,
        /// Live tail at quota grant (`None` exactly when ungranted);
        /// the single refresh compares exhaustion-time tails to it.
        grant_tail: Option<TailLength>,
    },
    Complete,
}

/// Progress-gated single quota refresh for deferred arming.
///
/// On quota exhaustion, a plugin whose live tail transitioned from
/// non-`Finite` at grant time to `Finite` now (the A/B mask derives its
/// flush once driving content exhausts) earns ONE re-queried budget for
/// the proven remainder. Anything else — a still-unprovable tail, no
/// re-queried bound, or a grant-time `Finite` promise broken (true
/// defects trip at exactly the granted budget) — answers `None` and the
/// caller trips loudly. Single-refresh: the caller records the returned
/// `Finite` tail as the new grant tail, so a second exhaustion always
/// trips. Scalar queries only, no allocation, and only on the
/// exhaustion path — never per call.
fn drain_quota_refresh(plugin: &dyn Plugin, grant_tail: TailLength) -> Option<(u64, TailLength)> {
    if matches!(grant_tail, TailLength::Finite(_)) {
        return None;
    }
    let current = plugin.tail_length();
    if !matches!(current, TailLength::Finite(_)) {
        return None;
    }
    let budget = plugin.drain_call_bound()?.get();
    Some((budget, current))
}

/// Static per-edge plan for graph EOS, computed at `build()`.
#[derive(Debug, Clone, Copy, Default)]
struct GraphEdgeDrainPlan {
    /// Routed channel width (post channel-map selection).
    routed_channels: usize,
    /// Bounded FIFO capacity in frames.
    queue_cap_frames: usize,
    /// Compensation-delay flush length in frames.
    comp_frames: usize,
}

/// Dynamic per-edge EOS state. Queues hold post-compensation frames, so
/// equal queue indices are equal sample times in the destination clock.
#[derive(Debug, Clone, Default)]
struct GraphEdgeDrainState {
    queue: VecDeque<f32>,
    /// Compensation frames still to push after the source completes.
    flush_remaining: usize,
    eof: bool,
}

/// Stream-independent envelope snapshot for one graph EOS node.
///
/// Recorded when every node publishes envelopes. The guard check compares
/// live bypass flags and envelope answers against this; any mismatch
/// falls back to a live re-derivation, so envelope-sized reservations
/// never cover waves they were not derived for.
#[derive(Debug, Clone, Copy, Default)]
struct GraphNodeEnvelope {
    /// Envelope quantum this entry was derived at.
    quantum: usize,
    /// `output_frames_envelope(quantum)` when this entry was derived.
    offi: usize,
    /// `drain_frames_envelope()` when this entry was derived.
    drain: usize,
}

/// Static per-node plan for graph EOS, computed at `build()`.
///
/// The plan holds wave-scale bounds only (quantum, single emission).
/// No lifetime total exists anywhere: queues are wave-bounded and
/// paced by backpressure, so declared tails of any finite length
/// drain without lifetime-sized reservations.
#[derive(Debug, Clone, Copy, Default)]
struct GraphNodeDrainPlan {
    /// Maximum frames consumed from each input edge per round.
    process_quantum: usize,
    /// Maximum frames produced by one round action.
    single_emission: usize,
    /// Bypass flag when this entry was derived. Bypass toggles change
    /// scheduling even on envelope-sized plans, so the guard check
    /// compares it live.
    bypassed: bool,
    /// Envelope snapshot when every node published envelopes (`None`
    /// otherwise, including for bypassed nodes, which derive no query).
    envelope: Option<GraphNodeEnvelope>,
    /// Live `drain_output_frames_max` when this entry was derived (0 for
    /// bypassed nodes). Stateful plugins change this across the stream (a
    /// fresh resampler reports 0, mid-stream its block maximum); the drain
    /// entry and bound query compare it allocation-free and re-derive the
    /// plan only when it changed.
    drain_max_snapshot: usize,
    /// Live `output_frames_for_input(process_quantum)` when this entry was
    /// derived (0 for bypassed nodes, which derive no query). Residual
    /// carry drifts this mid-drain even while the drain bound holds still;
    /// the drift check fires only on growth past this peak, since the
    /// grow-only holdover already covers anything at or below it.
    offi_snapshot: usize,
}

/// Static graph EOS plan. All values derive from declared plugin
/// metadata plus topology; see `design.md` in the graph-final-stream-r1
/// lane for the scheduling proof.
#[derive(Debug, Clone, Default)]
struct GraphDrainPlan {
    topo_order: Vec<NodeId>,
    node_plan: Vec<GraphNodeDrainPlan>,
    edge_plan: Vec<GraphEdgeDrainPlan>,
    /// Output-delay flush length per output node, in output order.
    output_comp_frames: Vec<usize>,
    /// Output-queue capacity per output node, in output order.
    output_queue_cap_frames: Vec<usize>,
    /// Conservative per-call output bound in frames.
    bound_frames: usize,
    /// Stream-independent per-call output bound when every node published
    /// envelopes (`None` otherwise). The bound query answers this without
    /// re-deriving while the guard snapshots hold; the quanta, emissions,
    /// and caps above are the envelope derivation then, so no mid-drain
    /// growth is possible and the refresh path stays quiet.
    envelope_bound: Option<usize>,
    /// Topology signature this plan was built for.
    signature: GraphDrainSignature,
}

/// Topology covered by a graph EOS plan and its dynamic state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GraphDrainSignature {
    nodes: Vec<NodeId>,
    edges: Vec<(NodeId, NodeId, usize, EdgeType)>,
    channel_routes: Vec<(Option<Vec<usize>>, usize)>,
}

/// Structural inputs shared by the live and envelope EOS derivations.
///
/// Topology, routing, and compensation delays are identical for both;
/// only the per-node emission queries differ (live declarations versus
/// stream-independent envelopes). Built on the control thread only.
struct GraphDrainTopology {
    signature: GraphDrainSignature,
    topo_order: Vec<NodeId>,
    routed: Vec<usize>,
    comp_frames: Vec<usize>,
    output_comp_frames: Vec<usize>,
}

/// Dynamic graph EOS state. Reset exactly where the linear `DrainState`
/// resets; preserved across a no-change rebuild keyed by edge id.
#[derive(Debug, Clone, Default)]
struct GraphDrainState {
    phases: Vec<GraphNodeDrainPhase>,
    edges: Vec<GraphEdgeDrainState>,
    /// One retained output wave per node; committed per edge.
    holdover_data: Vec<Vec<f32>>,
    holdover_frames: Vec<usize>,
    /// Per-edge commit flags for the source's current holdover wave.
    edge_committed: Vec<bool>,
    output_flush_remaining: Vec<usize>,
    /// Post-output-delay frames per output node, consumed aligned.
    output_queues: Vec<VecDeque<f32>>,
    /// Per-call output bound frozen at the first drain entry of the
    /// session (`None` outside one). Live declarations can legitimately
    /// rise mid-drain (chunk-quantized converters straddling a block
    /// boundary as residuals walk); internal reservations keep growing
    /// to cover them, but once-sized callers must never see the enforced
    /// bound move under a buffer they sized from the begin query, so
    /// preflight, pacing, and the bound query all use this value for the
    /// whole session. Cleared with all dynamic state on new accepted
    /// input, mutation, or reset.
    session_output_bound: Option<usize>,
    /// Test-only count of full output-FIFO commit retries (waves
    /// retained by backpressure). Proves the retry path executes;
    /// compiled out of production builds.
    #[cfg(test)]
    output_commit_retries: u64,
    /// Test-only count of drain-entry plan refreshes (declaration drift
    /// detected after the session started). Proves the refresh path
    /// executes; compiled out of production builds.
    #[cfg(test)]
    drain_refresh_count: u64,
}

impl GraphDrainState {
    fn reset_dynamic(&mut self) {
        self.phases.fill(GraphNodeDrainPhase::Running);
        for edge in &mut self.edges {
            edge.queue.clear();
            edge.flush_remaining = 0;
            edge.eof = false;
        }
        self.holdover_frames.fill(0);
        self.edge_committed.fill(false);
        self.output_flush_remaining.fill(0);
        for queue in &mut self.output_queues {
            queue.clear();
        }
        self.session_output_bound = None;
        #[cfg(test)]
        {
            self.output_commit_retries = 0;
            self.drain_refresh_count = 0;
        }
    }

    fn rearm_node(&mut self, node_id: NodeId) {
        if let Some(GraphNodeDrainPhase::Draining {
            remaining_calls,
            grant_tail,
            ..
        }) = self.phases.get_mut(node_id)
        {
            *remaining_calls = None;
            *grant_tail = None;
        }
    }
}

/// Per-round scheduler accumulator, kept on the stack.
#[derive(Debug, Clone, Copy, Default)]
struct GraphDrainRound {
    acted: bool,
    frames: usize,
}

/// Snapshot of compensation-delay history carried across a no-change
/// rebuild so drain waves keep their process-time alignment.
/// Each entry is `(ring buffer, cursor, channels)`; `None` means the
/// slot held no delay. Allocated on the build path only.
#[derive(Debug, Clone, Default)]
struct CompDelaySnapshot<T> {
    edges: Vec<Option<(Vec<T>, usize, usize)>>,
    outputs: Vec<Option<(Vec<T>, usize, usize)>>,
}

/// Disjoint host borrows for one graph EOS round.
///
/// The round runs while `BufferGuard` holds `process_buffers`, so it
/// cannot take `&mut self`; every field below borrows a disjoint piece
/// of the host instead. References only: constructing and passing this
/// context allocates nothing on the realtime path.
struct GraphDrainRoundCtx<'a> {
    state: &'a mut GraphDrainState,
    plan: &'a GraphDrainPlan,
    plugins: &'a mut [Option<Box<dyn Plugin>>],
    nodes: &'a HashMap<NodeId, GraphNode>,
    edges: &'a [GraphEdge],
    predecessors: &'a [Vec<GraphEdge>],
    successors: &'a [Vec<usize>],
    predecessor_edge_indices: &'a [Vec<usize>],
    output_nodes: &'a [NodeId],
    is_output_node: &'a [bool],
    node_input_sample_rates: &'a [f64],
    node_input_positions: &'a mut [u64],
    output_channels: usize,
    /// Session-frozen per-call output bound. Internal waves may exceed it
    /// mid-session (reservations grow); emission never does.
    session_output_bound: usize,
}

struct PreparedTerminalSinkBuffers {
    process_buffers: ProcessBuffers<f32>,
    process_buffers_f64: ProcessBuffers<f64>,
    f64_input_scratch: Vec<f32>,
    f64_output_scratch: Vec<f32>,
    f64_chain_scratch: Vec<f64>,
    f64_chain_scratch_alt: Vec<f64>,
    terminal_sink_staging: Vec<f32>,
}

fn try_zeroed_vec<T: Copy + Default>(len: usize, label: &str) -> Result<Vec<T>, String> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(len)
        .map_err(|error| format!("{label} reservation failed: {error}"))?;
    values.resize(len, T::default());
    Ok(values)
}

fn try_copy_vec_with_min_len<T: Copy + Default>(
    previous: &[T],
    minimum_len: usize,
    label: &str,
) -> Result<Vec<T>, String> {
    let mut values = try_zeroed_vec(previous.len().max(minimum_len), label)?;
    values[..previous.len()].copy_from_slice(previous);
    Ok(values)
}

fn try_vec_with_capacity<T>(capacity: usize, label: &str) -> Result<Vec<T>, String> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|error| format!("{label} reservation failed: {error}"))?;
    Ok(values)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalSinkLifecycle {
    Running,
    Draining,
    Complete,
    ResetRequired,
}

/// Result of processing one input block through an explicitly selected sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkProcessResult {
    /// Input frames consumed by the host graph and terminal sink.
    pub input_frames_consumed: usize,
    /// Frames still retained by the terminal sink after one bounded service step.
    pub pending_sink_frames: usize,
}

/// Why sink input could not be admitted before any source callback ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkPreflightError {
    /// Host-owned graph commands must be settled on the control thread first.
    PendingGraphMutation,
    /// The graph must be prepared on the control thread before sink processing.
    GraphNotBuilt,
    /// The complete block exceeds total prepared sink capacity.
    InputExceedsCapacity {
        input_frames: usize,
        capacity_frames: usize,
    },
    /// Input did not match the prepared frame/sample contract.
    InvalidInput(String),
    /// The sink refused the read-only admission check.
    Sink(SinkTailPreflightError),
}

/// Input consumption disposition after a sink-route failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkInputDisposition {
    /// No source producer processed this input block.
    NotAdmitted,
    /// The whole input block was appended exactly once.
    Admitted { frames: usize },
    /// A producer or append may have made partial progress; never replay it.
    Indeterminate,
}

/// Failure that poisons the current sink programme until explicit reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkFailure {
    ControlInvalidatedRoute(String),
    UpstreamProcess(String),
    InvalidProducedFrameCount { reported: usize, expected: usize },
    AppendContractViolation,
    ServiceContractViolation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkProcessError {
    RetryablePreflight(SinkPreflightError),
    ResetRequired {
        cause: SinkFailure,
        input: SinkInputDisposition,
    },
    Lifecycle(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkGraphSettlementResult {
    /// A single bounded service call ran; graph commands remain queued.
    ServiceOnly { pending_sink_frames: usize },
    /// All currently queued commands were applied, built and route-validated.
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkGraphSettlementError {
    NotRunning,
    ResetRequired(SinkFailure),
    Graph(String),
}

/// Result of one bounded terminal-sink EOF step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkDrainResult {
    /// Source-tail frames appended to the sink's prepared pending queue.
    pub source_tail_frames_handed_to_sink: usize,
    /// True only after the source is complete and the sink's pending queue is empty.
    pub complete: bool,
}

/// A typed refusal or failure from terminal-sink end-of-stream processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkDrainError {
    /// Terminal-sink mode was not enabled.
    ModeDisabled,
    /// EOF cannot build a graph because building is a separate control operation.
    GraphNotBuilt,
    /// Host-owned graph mutations must be settled before EOF begins.
    PendingGraphMutation,
    /// The prepared graph is not a supported serial terminal-sink route.
    InvalidRoute(String),
    /// A producer does not have the finite tail metadata required by this route.
    UnsupportedTailMetadata { node_id: NodeId, tail: TailLength },
    /// The sink cannot reserve the producer's declared maximum tail chunk.
    SinkPreflight(SinkTailPreflightError),
    /// Prepared host scratch cannot hold the producer's declared maximum chunk.
    ScratchCapacity {
        /// Required interleaved scratch samples.
        required_samples: usize,
        /// Samples currently available in prepared scratch.
        available_samples: usize,
    },
    /// An operation failed after EOF began and the host requires reset.
    ResetRequired(String),
    /// Another route operation failed before source state advanced.
    Operation(String),
}

impl std::fmt::Display for SinkDrainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModeDisabled => formatter.write_str("terminal sink mode is not enabled"),
            Self::GraphNotBuilt => formatter.write_str("terminal sink graph must be built first"),
            Self::PendingGraphMutation => {
                formatter.write_str("terminal sink EOF requires graph mutations to be settled")
            }
            Self::InvalidRoute(error) | Self::Operation(error) => formatter.write_str(error),
            Self::UnsupportedTailMetadata { node_id, tail } => write!(
                formatter,
                "terminal sink producer {node_id} has unsupported tail metadata {tail:?}"
            ),
            Self::SinkPreflight(error) => {
                write!(formatter, "terminal sink tail preflight failed: {error:?}")
            }
            Self::ScratchCapacity {
                required_samples,
                available_samples,
            } => write!(
                formatter,
                "terminal tail scratch capacity is too small: need {required_samples} samples, have {available_samples}"
            ),
            Self::ResetRequired(error) => {
                write!(formatter, "terminal sink reset required: {error}")
            }
        }
    }
}

impl std::error::Error for SinkDrainError {}

impl From<String> for SinkDrainError {
    fn from(error: String) -> Self {
        Self::Operation(error)
    }
}

impl From<&str> for SinkDrainError {
    fn from(error: &str) -> Self {
        Self::Operation(error.to_string())
    }
}

/// A host-side refusal or failure from control-thread terminal-sink recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSinkRecoveryError {
    /// Terminal-sink mode was not enabled.
    ModeDisabled,
    /// The route has not been built and prepared.
    GraphNotBuilt,
    /// Recovery is allowed only while the programme is running or draining.
    InvalidLifecycle,
    /// Queued graph changes must be explicitly settled first.
    PendingGraphMutation,
    /// The current graph no longer satisfies the terminal route contract.
    InvalidRoute(String),
    /// The plugin refused or failed transport recovery.
    Sink(PluginSinkRecoveryError),
    /// The host could not prepare replacement storage before changing the sink.
    HostPreparation(String),
    /// Recovery changed a host-prepared queue, latency, or lifecycle invariant.
    ContractViolation,
}

impl std::fmt::Display for TerminalSinkRecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModeDisabled => formatter.write_str("terminal sink mode is not enabled"),
            Self::GraphNotBuilt => formatter.write_str("terminal sink graph must be built first"),
            Self::InvalidLifecycle => formatter
                .write_str("terminal sink recovery requires a running or draining programme"),
            Self::PendingGraphMutation => {
                formatter.write_str("settle queued graph mutations before sink recovery")
            }
            Self::InvalidRoute(error) => formatter.write_str(error),
            Self::Sink(error) => error.fmt(formatter),
            Self::HostPreparation(error) => {
                write!(formatter, "terminal sink host reprepare failed: {error}")
            }
            Self::ContractViolation => {
                formatter.write_str("terminal sink recovery changed prepared host state")
            }
        }
    }
}

impl std::error::Error for TerminalSinkRecoveryError {}

pub struct DawHost {
    pub(super) nodes: HashMap<NodeId, GraphNode>,
    /// Plugin storage indexed by NodeId — disjoint from `nodes` for borrow checker.
    /// `process()` can borrow `&self.nodes` (topology) and `&mut self.plugins[nid]` (plugin)
    /// without conflict.
    pub(super) plugins: Vec<Option<Box<dyn Plugin>>>,
    pub(super) edges: Vec<GraphEdge>,
    pub(super) stages: Vec<ProcessingStage>,
    pub(super) input_nodes: Vec<NodeId>,
    pub(super) output_nodes: Vec<NodeId>,
    pub(super) next_node_id: NodeId,
    pub(super) chain_nodes: Vec<NodeId>,
    /// True once the chain API (`add_plugin`) has placed a node. Pure-graph
    /// hosts (never chain-built) refresh the derived `chain_nodes` order from
    /// stages whenever node membership changes, so appended nodes join the
    /// chain-indexed contracts after rebuild; chain-built and mixed hosts keep
    /// chain-API order untouched.
    pub(super) chain_built: bool,
    pub(super) built: bool,
    pub(super) process_buffers: Option<ProcessBuffers<f32>>,
    pub(super) process_buffers_f64: Option<ProcessBuffers<f64>>,
    pub(super) predecessors: Vec<Vec<GraphEdge>>,
    pub(super) is_input_node: Vec<bool>,
    pub(super) is_output_node: Vec<bool>,
    pub(super) has_variable_frame_plugin: bool,
    /// True if all plugins return input_frames unchanged from output_frames_for_input()
    pub(super) cached_frames_identity: bool,
    /// True if all plugins return input_rate unchanged from output_sample_rate()
    pub(super) cached_rate_identity: bool,
    /// Cached per-node output frame ratios for non-identity chains (rare)
    /// Only populated when cached_frames_identity is false
    pub(super) cached_output_frame_ratios: Vec<(NodeId, f64)>,
    /// Indices of plugins in chain_nodes that have analyzer data (get_data() returns Some)
    pub(super) analyzer_indices: Vec<usize>,
    /// Cached total latency in samples, computed during build() and invalidated on graph changes
    pub(super) cached_latency: Option<usize>,
    pub(super) compiled_plan: CompiledRenderPlan,
    /// Per-node cost estimate used to decide whether a parallel stage has enough
    /// work to amortize scheduler overhead. Built off the audio path.
    pub(super) cached_parallel_node_costs: Vec<u32>,
    /// Flat cache of bypass state for O(1) audio-thread lookup. **Mirror** of
    /// the authoritative `GraphNode::bypassed`; rebuilt in `build()` and kept
    /// in sync exclusively through `Self::set_bypass_state` so the two flags
    /// can never disagree.
    pub(super) bypassed: Vec<bool>,
    /// Per-node cumulative latency from graph inputs, computed during build().
    /// Used to calculate compensation delays at merge points.
    pub(super) node_latency_from_input: Vec<usize>,
    /// Negotiated input sample rate for each node, indexed by NodeId.
    pub(super) node_input_sample_rates: Vec<f64>,
    /// Output clock for latency and edge routing, indexed by node.
    pub(super) node_output_sample_rates: Vec<f64>,
    /// Exact binary-rational timeline clock, prepared before audio processing.
    exact_clock: Option<ExactClock>,
    prepared_clock_horizon_frames: usize,
    /// Next accepted input-frame position in each node's negotiated clock.
    /// Upstream buffering and fractional conversion make this independent of
    /// the current host callback position.
    pub(super) node_input_positions: Vec<u64>,
    drain_state: DrainState,
    graph_drain_state: GraphDrainState,
    graph_drain_plan: GraphDrainPlan,
    /// Reusable per-node scratch for the remaining-tail fold
    /// ([`DawHost::tail_length`]/[`DawHost::tail_support`]), indexed by node
    /// slot. `RefCell` keeps the `&self` query signature; the fold borrows
    /// it once per call and never re-enters (nested hosts own their own
    /// scratch), so the borrow cannot fail. Sized at plan derivation
    /// (off-RT build work); the fold grows it only when the topology grew
    /// without a fresh derivation, so steady-state queries allocate nothing.
    tail_fold_scratch: RefCell<Vec<Option<u64>>>,
    /// Sticky merge-retention overflow poison: set when a process-phase merge
    /// queue would exceed its declared cap. `process`, `process_f64`,
    /// `drain`, and `build` refuse while set; only `reset` recovers, because
    /// positions, delays, and queues all hold a partial block.
    merge_overflow_poisoned: bool,
    /// Outgoing edge indices per node slot, rebuilt in `build()`.
    pub(super) successors: Vec<Vec<usize>>,
    /// Incoming edge indices per node slot, parallel to `predecessors`.
    pub(super) predecessor_edge_indices: Vec<Vec<usize>>,
    terminal_sink_lifecycle: Option<TerminalSinkLifecycle>,
    terminal_sink_source_complete: bool,
    terminal_sink_producer_started: bool,
    terminal_sink_staging: Vec<f32>,
    /// Pre-allocated scratch buffer for automation updates (avoids per-process() heap allocation).
    pub(super) automation_scratch: Vec<(usize, f32)>,
    pub(super) queues: DawQueueEndpoints,
    pub(super) queue_state: DawQueueState,
    pub(super) automation_state: DawAutomationState,
    pub(super) config: DawConfig,
}

/// Outcome of an isolated plugin process call.
///
/// `frames` is the usable count: the plugin's return, or the
/// passthrough-substituted input count after a failure. `over_reported`
/// carries the plugin's raw return when it over-reported past staging —
/// the measured evidence for the truthful contract refusal at the call
/// site. Failures and panics substitute (rate-limited log) and report
/// `None`, preserving host resilience.
#[derive(Debug, Clone, Copy)]
pub(super) struct IsolatedProcessOutcome {
    pub(super) frames: usize,
    pub(super) over_reported: Option<usize>,
}

impl DawHost {
    /// Maximum block size the host pre-sizes its internal scratch and node
    /// buffers for. Covers all current SOTF engine block sizes; larger blocks
    /// still work but may trigger a one-time allocation on the audio thread.
    const MAX_BLOCK_FRAMES: usize = 8192;

    fn ensure_sink_commands_allowed(&self) -> Result<(), String> {
        match self.terminal_sink_lifecycle {
            Some(TerminalSinkLifecycle::Draining)
            | Some(TerminalSinkLifecycle::Complete)
            | Some(TerminalSinkLifecycle::ResetRequired) => {
                Err("terminal sink host is frozen; only DawHost::reset can reopen it".to_string())
            }
            Some(TerminalSinkLifecycle::Running) | None => Ok(()),
        }
    }

    fn ensure_sink_running(&self) -> Result<(), String> {
        match self.terminal_sink_lifecycle {
            Some(TerminalSinkLifecycle::Running) => Ok(()),
            Some(TerminalSinkLifecycle::Draining) => {
                Err("terminal sink host is draining; reset before accepting input".to_string())
            }
            Some(TerminalSinkLifecycle::Complete) => {
                Err("terminal sink stream is complete; reset before new input".to_string())
            }
            Some(TerminalSinkLifecycle::ResetRequired) => {
                Err("terminal sink host requires reset after an uncertain failure".to_string())
            }
            None => Err("terminal sink mode is not enabled".to_string()),
        }
    }

    fn ensure_sink_graph_mutation_allowed(&self) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        if self.terminal_sink_lifecycle != Some(TerminalSinkLifecycle::Running) {
            return Ok(());
        }

        let Some(sink_id) = self.chain_nodes.last().copied() else {
            return Ok(());
        };
        let Some(sink) = self
            .plugins
            .get(sink_id)
            .and_then(Option::as_ref)
            .and_then(|plugin| plugin.terminal_sink())
        else {
            return Ok(());
        };
        let state = sink.queue_state();
        if state.pending_frames.checked_add(state.free_prepared_frames)
            != Some(state.capacity_frames)
        {
            return Err("terminal sink has invalid prepared queue state".into());
        }
        if state.pending_frames > 0 {
            return Err(
                "terminal sink has retained audio; service it before direct graph mutation".into(),
            );
        }
        Ok(())
    }

    fn has_advertised_terminal_sink(&self) -> bool {
        self.plugins
            .iter()
            .flatten()
            .any(|plugin| plugin.terminal_sink().is_some())
    }

    /// Opt this host into the explicit terminal-sink process and EOF APIs.
    ///
    /// Sink mode requires both asynchronous command producers to remain owned by
    /// the host. Call this before exporting either command sender.
    pub fn enable_terminal_sink_mode(&mut self) -> Result<(), String> {
        if let Some(lifecycle) = self.terminal_sink_lifecycle {
            return if lifecycle == TerminalSinkLifecycle::Running {
                Ok(())
            } else {
                Err("terminal sink mode cannot be re-enabled after stream activity".into())
            };
        }
        if self.queues.parameter_event_tx.is_none() || self.queues.graph_mutation_tx.is_none() {
            return Err(
                "terminal sink mode requires both command producers to remain host-owned".into(),
            );
        }
        self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::Running);
        self.terminal_sink_source_complete = false;
        self.terminal_sink_producer_started = false;
        Ok(())
    }

    pub fn new<S: Into<f64>>(channels: usize, sample_rate: S) -> Self {
        let sample_rate = sample_rate.into();
        let (parameter_event_tx, parameter_event_rx) =
            RingBuffer::new(PARAMETER_EVENT_QUEUE_CAPACITY);
        let (graph_mutation_tx, graph_mutation_rx) = RingBuffer::new(GRAPH_MUTATION_QUEUE_CAPACITY);
        let graph_next_node_id = Arc::new(AtomicUsize::new(0));
        Self {
            nodes: HashMap::new(),
            plugins: Vec::new(),
            edges: Vec::new(),
            stages: Vec::new(),
            input_nodes: Vec::new(),
            output_nodes: Vec::new(),
            next_node_id: 0,
            chain_nodes: Vec::new(),
            chain_built: false,
            built: false,
            process_buffers: None,
            process_buffers_f64: None,
            predecessors: Vec::new(),
            is_input_node: Vec::new(),
            is_output_node: Vec::new(),
            has_variable_frame_plugin: false,
            cached_frames_identity: true,
            cached_rate_identity: true,
            cached_output_frame_ratios: Vec::new(),
            analyzer_indices: Vec::new(),
            cached_latency: None,
            compiled_plan: CompiledRenderPlan::default(),
            cached_parallel_node_costs: Vec::new(),
            bypassed: Vec::new(),
            node_latency_from_input: Vec::new(),
            node_input_sample_rates: Vec::new(),
            node_output_sample_rates: Vec::new(),
            exact_clock: None,
            prepared_clock_horizon_frames: 0,
            node_input_positions: Vec::new(),
            drain_state: DrainState::default(),
            graph_drain_state: GraphDrainState::default(),
            graph_drain_plan: GraphDrainPlan::default(),
            tail_fold_scratch: RefCell::new(Vec::new()),
            merge_overflow_poisoned: false,
            successors: Vec::new(),
            predecessor_edge_indices: Vec::new(),
            terminal_sink_lifecycle: None,
            terminal_sink_source_complete: false,
            terminal_sink_producer_started: false,
            terminal_sink_staging: Vec::new(),
            automation_scratch: Vec::new(),
            queues: DawQueueEndpoints {
                parameter_event_tx: Some(parameter_event_tx),
                parameter_event_rx,
                parameter_event_scratch: Vec::with_capacity(PARAMETER_EVENT_QUEUE_CAPACITY),
                graph_mutation_tx: Some(graph_mutation_tx),
                graph_mutation_rx,
            },
            queue_state: DawQueueState {
                graph_next_node_id,
                dropped_parameter_events: 0,
                dropped_graph_mutations: 0,
            },
            automation_state: DawAutomationState {
                automation: Vec::new(),
                automation_index: HashMap::new(),
                playback_position: 0,
            },
            config: DawConfig {
                sample_rate,
                parallel_enabled: true,
                compiled_linear_enabled: true,
                initial_input_channels: channels,
                realtime_quantum_frames: 1,
                plugin_preferred_oversampling_enabled: true,
                forced_oversampling_factor: None,
                topology: Arc::new(ArcSwap::from_pointee(GraphTopology::empty())),
                f64_input_scratch: Vec::new(),
                f64_output_scratch: Vec::new(),
                f64_chain_scratch: Vec::new(),
                f64_chain_scratch_alt: Vec::new(),
            },
        }
    }
    pub fn new_default(sr: u32) -> Self {
        Self::new(2, sr)
    }
    pub fn set_parallel_enabled(&mut self, e: bool) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.config.parallel_enabled = e;
        Ok(())
    }

    pub fn set_compiled_linear_enabled(&mut self, enabled: bool) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        if self.config.compiled_linear_enabled != enabled {
            self.config.compiled_linear_enabled = enabled;
            self.built = false;
        }
        Ok(())
    }

    /// Enable or disable plugins' `preferred_oversampling()` requests.
    pub fn set_plugin_preferred_oversampling_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.config.plugin_preferred_oversampling_enabled = enabled;
        Ok(())
    }

    /// Force a host oversampling wrapper around same-I/O plugins.
    pub fn set_forced_oversampling_factor(&mut self, factor: Option<u32>) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        if let Some(factor) = factor
            && factor != 2
            && factor != 4
        {
            return Err(format!(
                "Invalid forced oversampling factor {factor}: expected 2 or 4"
            ));
        }
        self.config.forced_oversampling_factor = factor;
        Ok(())
    }

    /// Set an automation curve for a parameter on a specific node.
    /// The curve is evaluated during `process()` before each stage.
    pub fn set_automation(
        &mut self,
        node_id: NodeId,
        param_id: ParameterId,
        curve: crate::automation::AutomationCurve,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        let auto = ParameterAutomation {
            param_id: param_id.clone(),
            mode: crate::automation::AutomationMode::Host,
            curve: Some(curve),
            position: 0,
            base_value: 0.0,
            last_value: 0.0,
        };
        let key = (node_id, param_id.clone());
        if let Some(&idx) = self.automation_state.automation_index.get(&key) {
            self.automation_state.automation[idx].automation = auto;
            return Ok(());
        }
        let idx = self.automation_state.automation.len();
        self.automation_state.automation.push(AutomationSlot {
            node_id,
            param_id,
            automation: auto,
        });
        self.automation_state.automation_index.insert(key, idx);
        Ok(())
    }

    /// Remove automation for a specific parameter on a node.
    pub fn clear_automation(
        &mut self,
        node_id: NodeId,
        param_id: &ParameterId,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        let key = (node_id, param_id.clone());
        let Some(idx) = self.automation_state.automation_index.remove(&key) else {
            return Ok(());
        };
        self.automation_state.automation.swap_remove(idx);
        if idx < self.automation_state.automation.len() {
            let moved = &self.automation_state.automation[idx];
            self.automation_state
                .automation_index
                .insert((moved.node_id, moved.param_id.clone()), idx);
        }
        Ok(())
    }

    /// Remove all automation.
    pub fn clear_all_automation(&mut self) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.automation_state.automation.clear();
        self.automation_state.automation_index.clear();
        Ok(())
    }

    /// Reset playback position to 0.
    pub fn reset_playback_position(&mut self) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.reanchor_node_positions(0)?;
        self.automation_state.playback_position = 0;
        for slot in &mut self.automation_state.automation {
            slot.automation.position = 0;
        }
        Ok(())
    }

    /// Synchronize the host timeline to an externally-owned transport clock.
    ///
    /// Embedded/DAW hosts call this at discontinuities; continuous processing
    /// advances the position internally without any control-thread traffic.
    pub fn set_playback_position(&mut self, sample_position: u64) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        let position = usize::try_from(sample_position)
            .map_err(|_| "Playback position exceeds addressable frames".to_owned())?;
        if self.automation_state.playback_position != position {
            self.reanchor_node_positions(sample_position)?;
            self.automation_state.playback_position = position;
        }
        Ok(())
    }

    fn reanchor_node_positions(&mut self, sample_position: u64) -> Result<(), String> {
        let source = ExactRate::new(self.config.sample_rate)?;
        let rates = self
            .nodes
            .keys()
            .map(|&id| ExactRate::new(self.node_input_sample_rates[id]))
            .collect::<Result<Vec<_>, _>>()?;
        let clock = if let Some(clock) = self.exact_clock {
            clock
        } else {
            ExactClock::new(std::iter::once(source).chain(rates.iter().copied()))?
        };
        let mut positions = self.node_input_positions.clone();
        for &id in self.nodes.keys() {
            positions[id] = clock.convert_position(
                sample_position,
                source,
                ExactRate::new(self.node_input_sample_rates[id])?,
            )?;
        }
        self.node_input_positions = positions;
        Ok(())
    }

    /// Take a snapshot of the current graph topology.
    /// Can be used with `ArcSwap` for lock-free graph updates from a control thread.
    pub fn topology_snapshot(&self) -> GraphTopology {
        GraphTopology {
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            stages: self.stages.clone(),
            input_nodes: self.input_nodes.clone(),
            output_nodes: self.output_nodes.clone(),
            predecessors: self.predecessors.clone(),
            is_input_node: self.is_input_node.clone(),
            is_output_node: self.is_output_node.clone(),
        }
    }

    /// Returns a lock-free handle to the current immutable topology snapshot.
    pub fn topology_handle(&self) -> Arc<ArcSwap<GraphTopology>> {
        Arc::clone(&self.config.topology)
    }

    /// Atomically load the current topology snapshot.
    pub fn current_topology(&self) -> Arc<GraphTopology> {
        self.config.topology.load_full()
    }

    pub(super) fn publish_topology_snapshot(&self) {
        self.config
            .topology
            .store(Arc::new(self.topology_snapshot()));
    }

    pub(super) fn reserve_node_id(&mut self) -> NodeId {
        let externally_reserved = self.queue_state.graph_next_node_id.load(Ordering::Acquire);
        if externally_reserved > self.next_node_id {
            self.next_node_id = externally_reserved;
        }
        let id = self.next_node_id;
        self.next_node_id += 1;
        self.queue_state
            .graph_next_node_id
            .store(self.next_node_id, Ordering::Release);
        id
    }

    pub fn add_node(&mut self, name: String, plugin: Box<dyn Plugin>) -> Result<NodeId, String> {
        self.ensure_sink_graph_mutation_allowed()?;
        let id = self.reserve_node_id();
        self.add_node_with_id_at_rate(id, name, plugin, self.config.sample_rate)?;
        Ok(id)
    }

    /// Add a graph node initialized at an explicit input rate.
    ///
    /// Unlike [`Self::add_node`], which initializes at the host rate, the
    /// plugin is initialized at `input_sample_rate`, recorded as the node's
    /// input rate. Needed for mid-graph rate-changing nodes whose
    /// initialization depends on the input rate (plain `add_node` fails for
    /// them before `build` can negotiate). `build()` still derives each
    /// node's negotiated rate from its predecessors and re-initializes on
    /// mismatch, failing loudly for inconsistent graphs. Follows the
    /// `add_node` mutation guard/reservation path; no behavior change to
    /// existing callers.
    ///
    /// # Errors
    ///
    /// Returns an error when sink mutation guards forbid the insertion, the
    /// reserved id collides, or the plugin rejects initialization at the
    /// given rate.
    pub fn add_node_at_rate<S: Into<f64>>(
        &mut self,
        name: String,
        plugin: Box<dyn Plugin>,
        input_sample_rate: S,
    ) -> Result<NodeId, String> {
        self.ensure_sink_graph_mutation_allowed()?;
        let id = self.reserve_node_id();
        self.add_node_with_id_at_rate(id, name, plugin, input_sample_rate.into())?;
        Ok(id)
    }

    pub(super) fn add_node_with_id(
        &mut self,
        id: NodeId,
        name: String,
        plugin: Box<dyn Plugin>,
    ) -> Result<(), String> {
        self.add_node_with_id_at_rate(id, name, plugin, self.config.sample_rate)
    }

    fn add_node_with_id_at_rate(
        &mut self,
        id: NodeId,
        name: String,
        mut plugin: Box<dyn Plugin>,
        input_sample_rate: f64,
    ) -> Result<(), String> {
        self.ensure_sink_graph_mutation_allowed()?;
        if self.nodes.contains_key(&id) {
            return Err(format!("Node {id} already exists"));
        }
        // An invalid or unrepresentable clock must not reach plugin setup.
        ExactRate::new(input_sample_rate)?;
        if id >= self.next_node_id {
            self.next_node_id = id + 1;
            self.queue_state
                .graph_next_node_id
                .store(self.next_node_id, Ordering::Release);
        }
        plugin = self.auto_oversample_plugin(plugin)?;
        plugin.initialize(input_sample_rate)?;
        let input_channels = plugin.input_channels();
        let output_channels = plugin.output_channels();
        self.nodes.insert(
            id,
            GraphNode::new(id, name, input_channels, output_channels),
        );
        // Grow plugins vec to accommodate the new id
        if id >= self.plugins.len() {
            self.plugins.resize_with(id + 1, || None);
        }
        self.node_input_sample_rates
            .resize(self.plugins.len(), self.config.sample_rate);
        self.node_input_sample_rates[id] = input_sample_rate;
        self.plugins[id] = Some(plugin);
        self.built = false;
        self.cached_latency = None;
        self.drain_state = DrainState::default();
        self.graph_drain_state.reset_dynamic();
        Ok(())
    }

    pub fn add_edge(&mut self, mut edge: GraphEdge) -> Result<(), String> {
        self.ensure_sink_graph_mutation_allowed()?;
        if !self.nodes.contains_key(&edge.from_node) || !self.nodes.contains_key(&edge.to_node) {
            return Err("Node not found".into());
        }
        if edge.from_node == edge.to_node {
            return Err("Self-loop".into());
        }
        edge.id = self.edges.len();
        self.edges.push(edge);
        self.built = false;
        self.cached_latency = None;
        self.drain_state = DrainState::default();
        self.graph_drain_state.reset_dynamic();
        Ok(())
    }

    pub(super) fn auto_oversample_plugin(
        &self,
        plugin: Box<dyn Plugin>,
    ) -> Result<Box<dyn Plugin>, String> {
        let factor = self.config.forced_oversampling_factor.or_else(|| {
            if self.config.plugin_preferred_oversampling_enabled {
                plugin.preferred_oversampling()
            } else {
                None
            }
        });

        let Some(factor) = factor else {
            return Ok(plugin);
        };
        if factor != 2 && factor != 4 {
            return Err(format!(
                "Invalid preferred oversampling factor {factor}: expected 2 or 4"
            ));
        }
        if plugin.input_channels() != plugin.output_channels() {
            crate::rate_limited_log!(
                warn,
                5,
                "host: plugin '{}' requested {}x oversampling but has mismatched I/O channels ({} -> {}); leaving unwrapped",
                plugin.info().name,
                factor,
                plugin.input_channels(),
                plugin.output_channels()
            );
            return Ok(plugin);
        }
        Ok(Box::new(
            crate::oversampling::AutoOversampledPlugin::new_with_max_frames(
                plugin,
                factor,
                Self::MAX_BLOCK_FRAMES,
            )?,
        ))
    }

    /// Add a sidechain edge: the output of `from` is routed as sidechain input
    /// to `to`. During processing, sidechain data is appended after the node's
    /// primary audio input channels.
    pub fn add_sidechain_edge(&mut self, from: NodeId, to: NodeId) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.add_edge(GraphEdge::sidechain(from, to))
    }

    pub fn build(&mut self) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        // A failed rebuild must never leave the previous plan marked usable.
        self.built = false;
        // Reject an unsupported host clock before any plugin is prepared at it.
        ExactRate::new(self.config.sample_rate)?;
        if self.merge_overflow_poisoned {
            return Err("host merge retention overflowed; reset the host before rebuilding".into());
        }
        if self.has_cycle() {
            return Err("Cycle".into());
        }
        self.compute_io_nodes();
        self.compute_stages()?;
        // Graph topologies bypass the chain APIs, leaving `chain_nodes`
        // empty. Chain-indexed contracts (analyzer discovery, plugin data
        // lookup, plugin count, per-node rates) would then silently report
        // nothing — daemon metering falls back to zeros in graph mode.
        // Derive signal order from the computed stages so fresh linear
        // graphs behave exactly like their chain equivalent. Hosts with
        // chain-built nodes (including mixed chain/graph usage, where side
        // nodes must stay out of `chain_nodes`) are deliberately untouched.
        // Pure-graph hosts (never chain-built) additionally refresh the
        // derived order whenever node membership changed since the last
        // derivation, so nodes appended after a build (e.g. an explicitly
        // rated converter) join the chain-indexed contracts instead of
        // leaving them stale. Non-membership rebuilds keep the existing
        // order, so branch positions stay stable across bypass/edge-only
        // rebuilds despite nondeterministic stage order within a layer.
        let membership_changed = self.chain_nodes.len() != self.nodes.len()
            || self
                .chain_nodes
                .iter()
                .any(|id| !self.nodes.contains_key(id));
        if self.chain_nodes.is_empty() || (!self.chain_built && membership_changed) {
            self.chain_nodes = self
                .stages
                .iter()
                .flat_map(|stage| stage.nodes.iter().copied())
                .collect();
        }
        let max_id = self.nodes.keys().copied().max().unwrap_or(0);
        let num_slots = if self.nodes.is_empty() { 0 } else { max_id + 1 };
        self.predecessors = vec![Vec::new(); num_slots];
        self.successors = vec![Vec::new(); num_slots];
        self.predecessor_edge_indices = vec![Vec::new(); num_slots];
        self.is_input_node = vec![false; num_slots];
        self.is_output_node = vec![false; num_slots];
        for (index, edge) in self.edges.iter().enumerate() {
            self.predecessors[edge.to_node].push(edge.clone());
            self.successors[edge.from_node].push(index);
            self.predecessor_edge_indices[edge.to_node].push(index);
        }
        for &id in &self.input_nodes {
            self.is_input_node[id] = true;
        }
        for &id in &self.output_nodes {
            self.is_output_node[id] = true;
        }
        let previous_input_rates = self.node_input_sample_rates.clone();
        self.node_input_sample_rates
            .resize(num_slots, self.config.sample_rate);
        self.node_output_sample_rates = vec![self.config.sample_rate; num_slots];
        self.node_input_positions.resize(num_slots, 0);
        for stage in &self.stages {
            for &id in &stage.nodes {
                let input_rate = self.predecessors[id]
                    .first()
                    .map_or(self.config.sample_rate, |edge| {
                        self.node_output_sample_rates[edge.from_node]
                    });
                if self.predecessors[id]
                    .iter()
                    .any(|edge| self.node_output_sample_rates[edge.from_node] != input_rate)
                {
                    return Err(format!(
                        "Node {id} joins incompatible sample rates; add resamplers before the join"
                    ));
                }
                let plugin = self.plugins[id].as_mut().unwrap();
                if self.node_input_sample_rates[id] != input_rate {
                    plugin.initialize(input_rate)?;
                }
                self.node_input_sample_rates[id] = input_rate;
                let output_rate = if self.nodes[&id].bypassed {
                    input_rate
                } else {
                    Self::plugin_output_sample_rate_isolated(
                        plugin.as_ref(),
                        id,
                        &self.nodes[&id].name,
                        input_rate,
                    )?
                };
                if output_rate == 0.0 {
                    return Err(format!("Node {id} returned a zero output sample rate"));
                }
                self.node_output_sample_rates[id] = output_rate;
            }
        }
        if let Some(&first) = self.output_nodes.first()
            && self.output_nodes.iter().any(|&id| {
                self.node_output_sample_rates[id] != self.node_output_sample_rates[first]
            })
        {
            return Err("Graph outputs have incompatible sample rates; add resamplers before the output mix".into());
        }
        for (id, plugin) in self
            .nodes
            .keys()
            .filter_map(|&id| self.plugins[id].as_ref().map(|plugin| (id, plugin)))
        {
            if plugin.terminal_sink().is_some() {
                Self::checked_native_sink_rate(self.node_input_sample_rates[id])?;
            }
        }
        let clock = ExactClock::new(
            std::iter::once(self.config.sample_rate)
                .chain(
                    self.nodes
                        .keys()
                        .map(|&id| self.node_input_sample_rates[id]),
                )
                .chain(
                    self.nodes
                        .keys()
                        .map(|&id| self.node_output_sample_rates[id]),
                )
                .map(ExactRate::new)
                .collect::<Result<Vec<_>, _>>()?,
        )?;
        let host_rate = ExactRate::new(self.config.sample_rate)?;
        let position = u64::try_from(self.automation_state.playback_position)
            .map_err(|_| "Graph playback position exceeds u64 frames".to_owned())?;
        let mut prepared_positions = self.node_input_positions.clone();
        for &id in self.nodes.keys() {
            let rate = self.node_input_sample_rates[id];
            let rate = ExactRate::new(rate)?;
            if previous_input_rates.get(id).copied() != Some(self.node_input_sample_rates[id]) {
                prepared_positions[id] = clock.convert_position(position, host_rate, rate)?;
            }
        }
        let prepared_sink_frames = self
            .chain_nodes
            .last()
            .and_then(|&sink_id| self.plugins[sink_id].as_ref())
            .and_then(|plugin| plugin.terminal_sink())
            .map_or(Self::MAX_BLOCK_FRAMES, |sink| {
                sink.queue_state()
                    .capacity_frames
                    .max(Self::MAX_BLOCK_FRAMES)
            });
        // Envelope-or-live shared scratch (F1): the live declaration
        // walks with stream state (chunk residuals), but preparation
        // runs once at build; size by the envelope where published so
        // straddle peaks never grow scratch mid-stream. Envelopes
        // dominate live values, so this only grows.
        let max_graph_frames = self
            .nodes
            .keys()
            .map(|&id| {
                self.path_output_envelope(id, prepared_sink_frames)
                    .unwrap_or_else(|| self.path_output_frames(id, prepared_sink_frames))
            })
            .max()
            .unwrap_or(Self::MAX_BLOCK_FRAMES)
            .max(Self::MAX_BLOCK_FRAMES);
        // Check the declared per-call frame horizon at each negotiated clock
        // before any block can use the plan. Later transport seeks are checked
        // against their actual position in `set_playback_position`.
        for &id in self.nodes.keys() {
            let rate = self.node_input_sample_rates[id];
            clock.preflight_horizon(
                prepared_positions[id],
                max_graph_frames,
                ExactRate::new(rate)?,
            )?;
        }
        let graph_scratch_samples = max_graph_frames
            .checked_mul(32)
            .ok_or("Graph frame expansion exceeds addressable storage")?;
        // Cold-process preparation: size every node buffer for the
        // largest per-call need at MAX_BLOCK_FRAMES (see
        // `max_block_process_need`). Larger blocks keep today's one-time
        // first-touch growth, exactly as MAX_BLOCK_FRAMES documents.
        let max_block_need = self.max_block_process_need();
        let mut node_buffers = (0..num_slots).map(|_| None).collect::<Vec<_>>();
        let mut node_buffers_f64 = (0..num_slots).map(|_| None).collect::<Vec<_>>();
        for (&id, node) in &self.nodes {
            // Envelope preparation covers every stream state (residual
            // straddle included); unknown envelopes keep live sizing.
            // Envelopes dominate live values, so this only grows.
            let path_prepared = self
                .path_output_envelope(id, prepared_sink_frames)
                .unwrap_or_else(|| self.path_output_frames(id, prepared_sink_frames));
            // Saturate only against addressable overflow (absurd
            // topologies): under-preparation degrades to today's
            // first-touch growth, never a new failure mode.
            let addressable = (isize::MAX as usize) / node.output_channels().max(1);
            let prepared = max_block_need.max(path_prepared).min(addressable);
            node_buffers[id] = Some(NodeBuffer::<f32>::new(prepared, node.output_channels()));
            node_buffers_f64[id] = Some(NodeBuffer::<f64>::new(prepared, node.output_channels()));
        }
        // Cache per-node bypass flags before computing compensation delays
        // (compensation needs to know which nodes are bypassed for latency calculation)
        self.bypassed = vec![false; num_slots];
        self.cached_parallel_node_costs = vec![DEFAULT_PARALLEL_NODE_COST; num_slots];
        for (&id, node) in &self.nodes {
            self.bypassed[id] = node.bypassed;
            let plugin = self.plugins[id].as_ref().unwrap();
            self.cached_parallel_node_costs[id] =
                Self::estimate_parallel_node_cost(plugin.as_ref(), id, &node.name);
        }
        // Convert every node-local work quantum into the host input clock.
        // Using u128 keeps the ceiling conversion exact without overflow even
        // for adversarial plugin values; the engine separately bounds the
        // resulting contract before activation.
        // Sum every active node, rather than only the graph critical path.
        // Stage execution may fall back from parallel to serial for topology,
        // channel-map, or cost reasons, so branch work can coincide in one
        // physical callback just like work in a linear chain.
        let host_period = clock.ticks_per_sample(host_rate)?;
        self.config.realtime_quantum_frames = self
            .plugins
            .iter()
            .enumerate()
            .try_fold(0_usize, |total, (id, plugin)| {
                let Some(plugin) = plugin.as_ref() else {
                    return Ok::<_, String>(total);
                };
                if self.bypassed.get(id).copied().unwrap_or(false) {
                    return Ok(total);
                }
                let node_rate = self
                    .node_input_sample_rates
                    .get(id)
                    .copied()
                    .unwrap_or(self.config.sample_rate);
                let node_period = clock.ticks_per_sample(ExactRate::new(node_rate)?)?;
                let node_ticks = (plugin.realtime_quantum_frames().max(1) as u128)
                    .checked_mul(node_period)
                    .ok_or_else(|| "Graph work quantum exceeds the exact clock range".to_owned())?;
                let host_frames = usize::try_from(node_ticks.div_ceil(host_period))
                    .map_err(|_| "Graph work quantum exceeds addressable frames".to_owned())?;
                total
                    .checked_add(host_frames)
                    .ok_or_else(|| "Graph work quantum exceeds addressable frames".to_owned())
            })?
            .max(1);
        // A no-change rebuild preserves drain waves queued downstream of
        // the compensation delays, so it must also preserve the delay
        // history those waves were pushed through. Snapshot first; any
        // topology mutation changes the signature and takes fresh
        // delays instead.
        let preserve_delays = self.graph_drain_signature() == self.graph_drain_plan.signature;
        let old_delays = self
            .process_buffers
            .as_ref()
            .map(|buffers| Self::snapshot_compensation(&buffers.compensation_delays));
        let old_delays_f64 = self
            .process_buffers_f64
            .as_ref()
            .map(|buffers| Self::snapshot_compensation(&buffers.compensation_delays));
        // Compute per-node cumulative latency from inputs and compensation delays
        let mut compensation_delays = self.compute_compensation_delays::<f32>(num_slots)?;
        let mut compensation_delays_f64 = self.compute_compensation_delays::<f64>(num_slots)?;
        if preserve_delays {
            if let Some(old) = old_delays {
                Self::restore_compensation(&mut compensation_delays, old);
            }
            if let Some(old) = old_delays_f64 {
                Self::restore_compensation(&mut compensation_delays_f64, old);
            }
        }

        // Process-phase merge retention: derive per-edge caps from the fresh
        // compensation lengths plus declared single-wave emissions, then size
        // the join scratch for the largest retained join. Queues move over
        // from the old buffers on a no-change rebuild only.
        let merge_comp_frames: Vec<usize> = compensation_delays
            .delays
            .iter()
            .map(|delay| delay.as_ref().map_or(0, |entry| entry.delay))
            .collect();
        let merge_queue_caps =
            self.merge_retention_caps(&merge_comp_frames, prepared_sink_frames)?;
        let merge_routed = Self::edge_routed_channels(&self.nodes, &self.edges, &self.predecessors);
        let mut merge_scratch_samples = graph_scratch_samples;
        for node_id in self.nodes.keys() {
            let incoming = &self.predecessors[*node_id];
            if incoming.len() < 2 {
                continue;
            }
            let max_cap = incoming
                .iter()
                .map(|edge| merge_queue_caps.get(edge.id).copied().unwrap_or(0))
                .max()
                .unwrap_or(0);
            let need = max_cap
                .checked_mul(self.nodes[node_id].input_channels())
                .ok_or("graph merge retention join exceeds addressable samples")?;
            merge_scratch_samples = merge_scratch_samples.max(need);
        }
        // Chain EOS preparation: when every stage publishes envelopes,
        // cover the largest intermediate wave so chain drain scratch
        // never grows mid-drain. Same chain predicate the drain path
        // uses, so preparation applies exactly when chain drain runs.
        if self.is_chain_topology_for_drain()
            && let Some(prep_frames) = self.chain_envelope_prep_frames()
        {
            let max_channels = self
                .chain_nodes
                .iter()
                .filter_map(|id| self.nodes.get(id))
                .map(|node| node.input_channels().max(node.output_channels()))
                .max()
                .unwrap_or(1);
            if let Some(prep_samples) = prep_frames.checked_mul(max_channels) {
                merge_scratch_samples = merge_scratch_samples.max(prep_samples);
            }
        }
        let old_merge_queues = self
            .process_buffers
            .as_mut()
            .map(|buffers| std::mem::take(&mut buffers.merge_queues));
        let old_merge_queues_f64 = self
            .process_buffers_f64
            .as_mut()
            .map(|buffers| std::mem::take(&mut buffers.merge_queues));
        let merge_queues = Self::restore_merge_queues(
            old_merge_queues.as_ref(),
            &merge_queue_caps,
            &merge_routed,
            preserve_delays,
        )?;
        let merge_queues_f64 = Self::restore_merge_queues(
            old_merge_queues_f64.as_ref(),
            &merge_queue_caps,
            &merge_routed,
            preserve_delays,
        )?;

        self.process_buffers = Some(ProcessBuffers {
            node_buffers,
            scratch_input: vec![0.0f32; merge_scratch_samples],
            scratch_output: vec![0.0f32; merge_scratch_samples],
            merge_buffer: vec![0.0f32; merge_scratch_samples],
            channel_map_buffer: vec![0.0f32; merge_scratch_samples],
            compensation_delays,
            delay_scratch: vec![0.0f32; graph_scratch_samples + 32],
            merge_queues,
            merge_queue_caps: merge_queue_caps.clone(),
            parallel_scratch: (0..num_slots)
                .map(|id| {
                    if let Some(node) = self.nodes.get(&id) {
                        // Output slot covers the MAX_BLOCK bound (upsampler
                        // nodes exceed MAX_BLOCK frames); saturating multiply
                        // degrades absurd topologies to first-touch growth.
                        let out_frames = self.parallel_output_prep_frames(id, node);
                        (
                            vec![0.0f32; Self::MAX_BLOCK_FRAMES * node.input_channels()],
                            vec![0.0f32; out_frames.saturating_mul(node.output_channels())],
                            vec![0.0f32; Self::MAX_BLOCK_FRAMES * node.input_channels()],
                        )
                    } else {
                        (Vec::new(), Vec::new(), Vec::new())
                    }
                })
                .collect(),
            parallel_results: Vec::with_capacity(
                self.stages
                    .iter()
                    .map(|stage| stage.nodes.len())
                    .max()
                    .unwrap_or(0),
            ),
        });
        self.process_buffers_f64 = Some(ProcessBuffers {
            node_buffers: node_buffers_f64,
            scratch_input: vec![0.0f64; merge_scratch_samples],
            scratch_output: vec![0.0f64; merge_scratch_samples],
            merge_buffer: vec![0.0f64; merge_scratch_samples],
            channel_map_buffer: vec![0.0f64; merge_scratch_samples],
            compensation_delays: compensation_delays_f64,
            delay_scratch: vec![0.0f64; graph_scratch_samples + 32],
            merge_queues: merge_queues_f64,
            merge_queue_caps,
            parallel_scratch: (0..num_slots)
                .map(|id| {
                    if let Some(node) = self.nodes.get(&id) {
                        // Output slot covers the MAX_BLOCK bound (upsampler
                        // nodes exceed MAX_BLOCK frames); saturating multiply
                        // degrades absurd topologies to first-touch growth.
                        let out_frames = self.parallel_output_prep_frames(id, node);
                        (
                            vec![0.0f64; Self::MAX_BLOCK_FRAMES * node.input_channels()],
                            vec![0.0f64; out_frames.saturating_mul(node.output_channels())],
                            vec![0.0f64; Self::MAX_BLOCK_FRAMES * node.input_channels()],
                        )
                    } else {
                        (Vec::new(), Vec::new(), Vec::new())
                    }
                })
                .collect(),
            parallel_results: Vec::with_capacity(
                self.stages
                    .iter()
                    .map(|stage| stage.nodes.len())
                    .max()
                    .unwrap_or(0),
            ),
        });
        // Native f64 chains and f32 fallback use these buffers on their first
        // callback too; reserve them alongside the graph processing buffers.
        // Chain scratch follows merge scratch (a superset of graph scratch
        // that includes the chain envelope preparation), so intermediate
        // chain waves never grow it mid-stream.
        ensure_len(&mut self.config.f64_chain_scratch, merge_scratch_samples);
        ensure_len(
            &mut self.config.f64_chain_scratch_alt,
            merge_scratch_samples,
        );
        ensure_len(&mut self.config.f64_input_scratch, graph_scratch_samples);
        ensure_len(&mut self.config.f64_output_scratch, graph_scratch_samples);
        // Cache per-frame properties to avoid mutex locks during process()
        self.cached_frames_identity = true;
        self.cached_rate_identity = true;
        self.cached_output_frame_ratios.clear();
        self.analyzer_indices.clear();

        for (&id, node) in &self.nodes {
            let p = self.plugins[id].as_ref().unwrap();
            if !node.bypassed
                && Self::plugin_output_frames_for_input_isolated(p.as_ref(), id, &node.name, 100)
                    != 100
            {
                self.cached_frames_identity = false;
            }
            if self.node_output_sample_rates[id] != self.node_input_sample_rates[id] {
                self.cached_rate_identity = false;
            }
        }
        for (chain_idx, &id) in self.chain_nodes.iter().enumerate() {
            if self.plugins[id].as_ref().unwrap().get_data().is_some() {
                self.analyzer_indices.push(chain_idx);
            }
        }

        self.has_variable_frame_plugin = self.nodes.keys().any(|&id| {
            let p = self.plugins[id].as_ref().unwrap();
            let node = &self.nodes[&id];
            Self::plugin_output_frames_for_input_isolated(p.as_ref(), id, &node.name, 100) != 100
                || p.latency_samples() > 0
        });
        // Cache total latency so total_latency_samples() is O(1)
        self.cached_latency = Some(self.compute_latency());
        self.compiled_plan = self.compile_render_plan();
        if let Some(&sink_id) = self.chain_nodes.last()
            && let Some(sink) = self.plugins[sink_id]
                .as_ref()
                .and_then(|plugin| plugin.terminal_sink())
        {
            let staging_samples = sink
                .queue_state()
                .capacity_frames
                .checked_mul(self.nodes[&sink_id].input_channels())
                .ok_or("terminal sink staging size overflow")?;
            ensure_len(&mut self.terminal_sink_staging, staging_samples);
        }
        self.rebuild_graph_drain_plan()?;
        // Publish the negotiated clock and positions only after every
        // fallible latency and drain-plan preparation has succeeded.
        self.node_input_positions = prepared_positions;
        self.exact_clock = Some(clock);
        self.prepared_clock_horizon_frames = max_graph_frames;
        self.built = true;
        self.publish_topology_snapshot();
        Ok(())
    }

    fn prepare_terminal_sink_process_buffers<T: AudioSample>(
        &self,
        prepared_sink_frames: usize,
        old_buffers: Option<&ProcessBuffers<T>>,
    ) -> Result<ProcessBuffers<T>, String> {
        let num_slots = match self.nodes.keys().copied().max() {
            Some(max_id) => max_id
                .checked_add(1)
                .ok_or_else(|| "node-buffer slot count overflow".to_string())?,
            None => 0,
        };
        let mut node_buffers = Vec::new();
        node_buffers
            .try_reserve_exact(num_slots)
            .map_err(|error| format!("node-buffer slot reservation failed: {error}"))?;
        node_buffers.resize_with(num_slots, || None);

        // Same cold-process preparation as the regular builder.
        let max_block_need = self.max_block_process_need();
        for (&id, node) in &self.nodes {
            let channels = node.output_channels();
            let old = old_buffers
                .and_then(|buffers| buffers.node_buffers.get(id))
                .and_then(Option::as_ref);
            if channels == 0 {
                if old.is_some_and(|buffer| {
                    buffer.num_channels != 0 || buffer.actual_len != 0 || !buffer.data.is_empty()
                }) {
                    return Err(format!(
                        "node {id} zero-width output retained unexpected buffer data"
                    ));
                }
                // A terminal sink has no output buffer. The regular graph
                // builder represents that slot as an empty NodeBuffer too.
                node_buffers[id] = Some(NodeBuffer {
                    data: Vec::new(),
                    actual_len: 0,
                    num_channels: 0,
                });
                continue;
            }
            let path_prepared = self
                .path_output_envelope(id, prepared_sink_frames)
                .unwrap_or_else(|| self.path_output_frames(id, prepared_sink_frames));
            let frames = max_block_need.max(path_prepared);
            let required_samples = frames
                .checked_mul(channels)
                .ok_or_else(|| format!("node {id} prepared sample extent overflow"))?;
            let retained_len = old.map_or(0, |buffer| buffer.actual_len);
            if let Some(old) = old
                && (old.num_channels != channels
                    || retained_len > old.data.len()
                    || !retained_len.is_multiple_of(channels))
            {
                return Err(format!(
                    "node {id} retained buffer does not match the current graph geometry"
                ));
            }
            let previous_data = old.map_or(&[][..], |buffer| buffer.data.as_slice());
            let data = try_copy_vec_with_min_len(previous_data, required_samples, "node buffer")?;
            node_buffers[id] = Some(NodeBuffer {
                data,
                actual_len: retained_len,
                num_channels: channels,
            });
        }

        let (graph_scratch_samples, delay_scratch_samples, max_channels) =
            self.terminal_sink_graph_scratch_extents(prepared_sink_frames)?;

        let mut parallel_scratch = Vec::new();
        parallel_scratch
            .try_reserve_exact(num_slots)
            .map_err(|error| format!("parallel scratch slot reservation failed: {error}"))?;
        for id in 0..num_slots {
            if let Some(node) = self.nodes.get(&id) {
                let input_samples = Self::MAX_BLOCK_FRAMES
                    .checked_mul(node.input_channels())
                    .ok_or_else(|| format!("node {id} parallel input extent overflow"))?;
                let output_samples = self
                    .parallel_output_prep_frames(id, node)
                    .checked_mul(node.output_channels())
                    .ok_or_else(|| format!("node {id} parallel output extent overflow"))?;
                let old = old_buffers.and_then(|buffers| buffers.parallel_scratch.get(id));
                parallel_scratch.push((
                    try_copy_vec_with_min_len(
                        old.map_or(&[][..], |scratch| scratch.0.as_slice()),
                        input_samples,
                        "parallel input scratch",
                    )?,
                    try_copy_vec_with_min_len(
                        old.map_or(&[][..], |scratch| scratch.1.as_slice()),
                        output_samples,
                        "parallel output scratch",
                    )?,
                    try_copy_vec_with_min_len(
                        old.map_or(&[][..], |scratch| scratch.2.as_slice()),
                        input_samples,
                        "parallel merge scratch",
                    )?,
                ));
            } else {
                let old = old_buffers.and_then(|buffers| buffers.parallel_scratch.get(id));
                parallel_scratch.push((
                    try_copy_vec_with_min_len(
                        old.map_or(&[][..], |scratch| scratch.0.as_slice()),
                        0,
                        "parallel input scratch",
                    )?,
                    try_copy_vec_with_min_len(
                        old.map_or(&[][..], |scratch| scratch.1.as_slice()),
                        0,
                        "parallel output scratch",
                    )?,
                    try_copy_vec_with_min_len(
                        old.map_or(&[][..], |scratch| scratch.2.as_slice()),
                        0,
                        "parallel merge scratch",
                    )?,
                ));
            }
        }

        let result_capacity = self
            .stages
            .iter()
            .map(|stage| stage.nodes.len())
            .max()
            .unwrap_or(0);
        let parallel_results = try_vec_with_capacity(result_capacity, "parallel results")?;

        // Process-phase retention for the sink reprepare path. Terminal sink
        // graphs are serial chains (validated), so no merge caps arise; old
        // queues move over when the edge set matches, anything else fails
        // loudly, and fresh merges without compensation state refuse rather
        // than undersize.
        let sink_merge_caps = self.merge_retention_caps(&[], prepared_sink_frames)?;
        if old_buffers.is_none() && sink_merge_caps.iter().any(|&cap| cap > 0) {
            return Err(
                "terminal sink reprepare cannot size merge retention without compensation state"
                    .into(),
            );
        }
        let sink_routed = Self::edge_routed_channels(&self.nodes, &self.edges, &self.predecessors);
        let sink_preserve =
            old_buffers.is_some_and(|old| old.merge_queues.len() == self.edges.len());
        let sink_merge_queues = Self::restore_merge_queues(
            old_buffers.map(|buffers| &buffers.merge_queues),
            &sink_merge_caps,
            &sink_routed,
            sink_preserve,
        )?;

        Ok(ProcessBuffers {
            node_buffers,
            scratch_input: try_copy_vec_with_min_len(
                old_buffers.map_or(&[][..], |buffers| buffers.scratch_input.as_slice()),
                graph_scratch_samples,
                "graph input scratch",
            )?,
            scratch_output: try_copy_vec_with_min_len(
                old_buffers.map_or(&[][..], |buffers| buffers.scratch_output.as_slice()),
                graph_scratch_samples,
                "graph output scratch",
            )?,
            merge_buffer: try_copy_vec_with_min_len(
                old_buffers.map_or(&[][..], |buffers| buffers.merge_buffer.as_slice()),
                graph_scratch_samples,
                "graph merge scratch",
            )?,
            channel_map_buffer: try_copy_vec_with_min_len(
                old_buffers.map_or(&[][..], |buffers| buffers.channel_map_buffer.as_slice()),
                graph_scratch_samples,
                "channel-map scratch",
            )?,
            // The old delay objects are moved into this bundle only after the
            // sink accepts its plan, preserving their ring positions/history.
            compensation_delays: CompensationDelays::empty(),
            delay_scratch: try_copy_vec_with_min_len(
                old_buffers.map_or(&[][..], |buffers| buffers.delay_scratch.as_slice()),
                delay_scratch_samples.max(max_channels),
                "graph delay scratch",
            )?,
            merge_queues: sink_merge_queues,
            merge_queue_caps: sink_merge_caps,
            parallel_scratch,
            parallel_results,
        })
    }

    fn terminal_sink_graph_scratch_extents(
        &self,
        prepared_sink_frames: usize,
    ) -> Result<(usize, usize, usize), String> {
        let mut graph_samples = 0usize;
        let mut maximum_channels = 1usize;
        for (&id, node) in &self.nodes {
            let channels = node.input_channels().max(node.output_channels());
            if channels == 0 {
                return Err(format!("node {id} has zero channel geometry"));
            }
            maximum_channels = maximum_channels.max(channels);
            // Envelope-or-live shared scratch, matching the build path:
            // reprepare runs at one stream state but later blocks walk
            // residuals, so size by the envelope where published.
            // Envelopes dominate live values, so this only grows.
            let frames = self
                .path_output_envelope(id, prepared_sink_frames)
                .unwrap_or_else(|| self.path_output_frames(id, prepared_sink_frames))
                .max(Self::MAX_BLOCK_FRAMES);
            let samples = frames
                .checked_mul(channels)
                .ok_or_else(|| format!("node {id} graph scratch extent overflow"))?;
            graph_samples = graph_samples.max(samples);
        }
        let delay_samples = graph_samples
            .checked_add(maximum_channels)
            .ok_or_else(|| "graph delay scratch extent overflow".to_string())?;
        Ok((graph_samples, delay_samples, maximum_channels))
    }

    fn prepare_terminal_sink_host_buffers(
        &self,
        sink_id: NodeId,
        plan: SinkTransportRepreparePlan,
    ) -> Result<PreparedTerminalSinkBuffers, String> {
        let prepared_sink_frames = plan.queue_capacity_frames.max(Self::MAX_BLOCK_FRAMES);
        let channels = self.nodes[&sink_id].input_channels();
        let staging_samples = plan
            .queue_capacity_frames
            .checked_mul(channels)
            .ok_or_else(|| "terminal sink staging sample extent overflow".to_string())?;
        let (graph_scratch_samples, _, _) =
            self.terminal_sink_graph_scratch_extents(prepared_sink_frames)?;

        Ok(PreparedTerminalSinkBuffers {
            process_buffers: self.prepare_terminal_sink_process_buffers(
                prepared_sink_frames,
                self.process_buffers.as_ref(),
            )?,
            process_buffers_f64: self.prepare_terminal_sink_process_buffers(
                prepared_sink_frames,
                self.process_buffers_f64.as_ref(),
            )?,
            f64_input_scratch: try_copy_vec_with_min_len(
                &self.config.f64_input_scratch,
                graph_scratch_samples,
                "f64 input scratch",
            )?,
            f64_output_scratch: try_copy_vec_with_min_len(
                &self.config.f64_output_scratch,
                graph_scratch_samples,
                "f64 output scratch",
            )?,
            f64_chain_scratch: try_copy_vec_with_min_len(
                &self.config.f64_chain_scratch,
                graph_scratch_samples,
                "f64 chain scratch",
            )?,
            f64_chain_scratch_alt: try_copy_vec_with_min_len(
                &self.config.f64_chain_scratch_alt,
                graph_scratch_samples,
                "alternate f64 chain scratch",
            )?,
            terminal_sink_staging: try_copy_vec_with_min_len(
                &self.terminal_sink_staging,
                staging_samples,
                "terminal sink staging",
            )?,
        })
    }

    fn validate_terminal_sink_reprepare_plan(
        &self,
        sink_id: NodeId,
        queue: SinkQueueState,
        plan: SinkTransportRepreparePlan,
    ) -> Result<(), String> {
        let node = &self.nodes[&sink_id];
        let expected_rate = Self::checked_native_sink_rate(self.node_input_sample_rates[sink_id])?;
        let expected_channels = node.input_channels();
        if !plan.configuration_changed
            || plan.prepared_format.sample_rate != expected_rate
            || plan.prepared_format.channels != expected_channels
            || plan.target_format.sample_rate != expected_rate
            || plan.target_format.channels != expected_channels
            || plan.target_format.buffer_frames == 0
            || plan.prepared_format.buffer_frames == 0
            || plan.target_format.buffer_frames == plan.prepared_format.buffer_frames
            || plan.pending_frames != queue.pending_frames
            || plan.queue_capacity_frames
                != plan.target_format.buffer_frames.max(queue.pending_frames)
        {
            return Err(
                "terminal sink reprepare plan does not match the built route and queue".into(),
            );
        }
        plan.queue_capacity_frames
            .checked_mul(expected_channels)
            .ok_or_else(|| "terminal sink reprepare queue sample extent overflow".to_string())?;
        Ok(())
    }

    pub(super) fn compile_render_plan(&self) -> CompiledRenderPlan {
        if self.nodes.is_empty() {
            CompiledRenderPlan::EmptyPassthrough
        } else if self.config.compiled_linear_enabled && self.can_process_f32_linear_chain() {
            CompiledRenderPlan::LinearF32(self.compile_linear_f32_plan())
        } else {
            CompiledRenderPlan::Graph(self.compile_graph_plan())
        }
    }

    pub(super) fn compile_linear_f32_plan(&self) -> CompiledLinearPlan {
        let ops = self
            .chain_nodes
            .iter()
            .map(|&id| self.compile_op(id))
            .collect::<Vec<_>>();
        CompiledLinearPlan {
            input_channels: self.input_channels(),
            output_channels: self.output_channels(),
            segments: build_segments(&ops),
            ops,
        }
    }

    pub(super) fn compile_graph_plan(&self) -> CompiledGraphPlan {
        let ops = self
            .stages
            .iter()
            .flat_map(|stage| stage.nodes.iter().copied())
            .map(|id| self.compile_op(id))
            .collect::<Vec<_>>();
        CompiledGraphPlan {
            segments: build_segments(&ops),
        }
    }

    pub(super) fn compile_op(&self, id: NodeId) -> CompiledOp {
        let plugin = self.plugins[id].as_ref().unwrap();
        let node = &self.nodes[&id];
        let metadata = plugin.compile_metadata();
        CompiledOp::from_plugin(
            node,
            metadata,
            plugin.supports_f64(),
            Self::plugin_output_frames_for_input_isolated(plugin.as_ref(), id, &node.name, 100)
                == 100,
            Self::plugin_output_sample_rate_isolated(plugin.as_ref(), id, &node.name, 48_000.0)
                .is_ok_and(|rate| rate == 48_000.0),
        )
    }

    pub(super) fn can_process_f32_linear_chain(&self) -> bool {
        if !self.cached_frames_identity
            || !self.cached_rate_identity
            || self.has_variable_frame_plugin
        {
            return false;
        }
        self.is_topologically_linear_chain()
    }

    fn is_topologically_linear_chain(&self) -> bool {
        if self.chain_nodes.is_empty() || self.chain_nodes.len() != self.nodes.len() {
            return false;
        }
        if self.input_nodes.len() != 1 || self.output_nodes.len() != 1 {
            return false;
        }
        if self.input_nodes[0] != self.chain_nodes[0]
            || self.output_nodes[0] != *self.chain_nodes.last().unwrap()
        {
            return false;
        }
        if self.edges.len() != self.chain_nodes.len().saturating_sub(1) {
            return false;
        }
        for &nid in &self.chain_nodes {
            let Some(node) = self.nodes.get(&nid) else {
                return false;
            };
            if node.input_channels() != node.output_channels() || self.plugins[nid].is_none() {
                return false;
            }
        }
        for pair in self.chain_nodes.windows(2) {
            let from = pair[0];
            let to = pair[1];
            let Some(edge) = self
                .edges
                .iter()
                .find(|edge| edge.from_node == from && edge.to_node == to)
            else {
                return false;
            };
            if edge.edge_type != EdgeType::Audio
                || edge.channel_map.is_some()
                || edge.destination_offset != 0
            {
                return false;
            }
        }
        true
    }

    /// Validate the narrower serial topology admitted for channel-changing EOF.
    /// The ordinary processing path deliberately continues to use
    /// `is_topologically_linear_chain` so its existing fast-path boundary does
    /// not change.
    fn validate_channel_changing_drain_chain(&mut self) -> Result<(), String> {
        let invalid_topology =
            || "end-of-stream drain currently requires a linear plugin graph".to_owned();
        if !self.built
            || self.chain_nodes.is_empty()
            || self.chain_nodes.len() != self.nodes.len()
            || self.input_nodes.len() != 1
            || self.output_nodes.len() != 1
            || self.input_nodes[0] != self.chain_nodes[0]
            || self.output_nodes[0] != *self.chain_nodes.last().unwrap()
            || self.edges.len() != self.chain_nodes.len().saturating_sub(1)
        {
            return Err(invalid_topology());
        }

        for pair in self.chain_nodes.windows(2) {
            let Some(edge) = self
                .edges
                .iter()
                .find(|edge| edge.from_node == pair[0] && edge.to_node == pair[1])
            else {
                return Err(invalid_topology());
            };
            if edge.edge_type != EdgeType::Audio
                || edge.channel_map.is_some()
                || edge.destination_offset != 0
            {
                return Err(invalid_topology());
            }
        }

        let mut expected_input_channels = self.config.initial_input_channels;
        let mut has_active_width_change = false;
        for (chain_index, &node_id) in self.chain_nodes.iter().enumerate() {
            let node = self.nodes.get(&node_id).ok_or_else(invalid_topology)?;
            let plugin = self
                .plugins
                .get_mut(node_id)
                .and_then(Option::as_deref_mut)
                .ok_or_else(invalid_topology)?;
            if node.input_channels() != expected_input_channels {
                return Err(format!(
                    "channel-changing drain requires contiguous node widths: expected {expected_input_channels} input channels at '{}', got {}",
                    node.name,
                    node.input_channels()
                ));
            }

            if node.bypassed {
                if node.input_channels() != node.output_channels() {
                    return Err(format!(
                        "channel-changing drain cannot bypass width-changing node '{}'",
                        node.name
                    ));
                }
            } else {
                let input_rate = self
                    .node_input_sample_rates
                    .get(node_id)
                    .copied()
                    .ok_or_else(invalid_topology)?;
                let output_rate = self
                    .node_output_sample_rates
                    .get(node_id)
                    .copied()
                    .ok_or_else(invalid_topology)?;
                if input_rate != self.config.sample_rate || output_rate != self.config.sample_rate {
                    return Err(format!(
                        "channel-changing drain requires the host sample rate at '{}' (host {}, node input {input_rate}, output {output_rate})",
                        node.name, self.config.sample_rate
                    ));
                }
                if !plugin.guarantees_identity_frame_geometry() {
                    return Err(format!(
                        "channel-changing drain requires explicit identity frame geometry at '{}'",
                        node.name
                    ));
                }
                let metadata_was_prepared = chain_index < self.drain_state.completed_prefix
                    || (self.drain_state.active_node == Some(node_id) && self.drain_state.prepared);
                if !metadata_was_prepared {
                    plugin.prepare_drain_metadata().map_err(|error| {
                        format!(
                            "channel-changing drain could not prepare tail metadata at '{}': {error}",
                            node.name
                        )
                    })?;
                    match plugin.tail_length() {
                        TailLength::Finite(_) => {}
                        TailLength::Unknown => {
                            return Err(format!(
                                "channel-changing drain requires finite tail metadata at '{}'",
                                node.name
                            ));
                        }
                        TailLength::Infinite => {
                            return Err(format!(
                                "channel-changing drain cannot complete an infinite tail at '{}'",
                                node.name
                            ));
                        }
                    }
                }
                has_active_width_change |= node.input_channels() != node.output_channels();
            }

            expected_input_channels = node.output_channels();
        }

        if !has_active_width_change {
            return Err(invalid_topology());
        }
        Ok(())
    }

    /// Check every scratch extent required by a channel-changing drain before
    /// any plugin enters its native EOS state. The route has already proved
    /// identity frame geometry and equal sample rates, so each downstream
    /// process stage receives the source tail's frame count unchanged.
    fn validate_channel_changing_drain_scratch(&self) -> Result<(), String> {
        let buffers = self
            .process_buffers
            .as_ref()
            .ok_or("channel-changing drain requires prepared graph scratch")?;
        let mut required_input_samples = 0usize;
        let mut required_output_samples = 0usize;

        for (source_index, &source_id) in self
            .chain_nodes
            .iter()
            .enumerate()
            .skip(self.drain_state.completed_prefix)
        {
            if self.nodes[&source_id].bypassed {
                continue;
            }
            let source = self.plugins[source_id]
                .as_ref()
                .ok_or("channel-changing drain source plugin is unavailable")?;
            let frames = source.drain_output_frames_max();
            let source_samples = frames
                .checked_mul(self.nodes[&source_id].output_channels())
                .ok_or("channel-changing drain source scratch extent overflow")?;
            required_output_samples = required_output_samples.max(source_samples);
            let mut channels = self.nodes[&source_id].output_channels();

            for &downstream_id in &self.chain_nodes[source_index + 1..] {
                let downstream_node = &self.nodes[&downstream_id];
                if downstream_node.bypassed {
                    channels = downstream_node.output_channels();
                    continue;
                }
                let input_samples = frames
                    .checked_mul(channels)
                    .ok_or("channel-changing drain input scratch extent overflow")?;
                required_input_samples = required_input_samples.max(input_samples);

                let downstream = self.plugins[downstream_id]
                    .as_ref()
                    .ok_or("channel-changing drain downstream plugin is unavailable")?;
                let output_frames = downstream.output_frames_for_input(frames);
                if output_frames < frames {
                    return Err(format!(
                        "channel-changing drain plugin '{}' reports output capacity {output_frames} below its identity frame count {frames}",
                        downstream_node.name
                    ));
                }
                let output_samples = output_frames
                    .checked_mul(downstream_node.output_channels())
                    .ok_or("channel-changing drain output scratch extent overflow")?;
                required_output_samples = required_output_samples.max(output_samples);

                // The explicit identity capability is the behavior proof;
                // output_frames_for_input is only a storage bound. The next
                // process stage receives the unchanged `frames` count.
                channels = downstream_node.output_channels();
            }
        }

        if required_input_samples > buffers.scratch_input.len() {
            return Err(format!(
                "channel-changing drain needs {required_input_samples} input scratch samples, prepared capacity is {}",
                buffers.scratch_input.len()
            ));
        }
        if required_output_samples > buffers.scratch_output.len() {
            return Err(format!(
                "channel-changing drain needs {required_output_samples} output scratch samples, prepared capacity is {}",
                buffers.scratch_output.len()
            ));
        }
        Ok(())
    }

    pub(super) fn estimate_parallel_node_cost(
        plugin: &dyn Plugin,
        node_id: NodeId,
        node_name: &str,
    ) -> u32 {
        let metadata = plugin.compile_metadata();
        let has_latency_or_variable_frames = metadata.latency_samples > 0
            || Self::plugin_output_frames_for_input_isolated(plugin, node_id, node_name, 100)
                != 100
            || !Self::plugin_output_sample_rate_isolated(plugin, node_id, node_name, 48_000.0)
                .is_ok_and(|rate| rate == 48_000.0);
        if has_latency_or_variable_frames {
            return HEAVY_PARALLEL_NODE_COST;
        }

        match metadata.cost_class {
            PluginCostClass::Scalar => {}
            PluginCostClass::Analyzer => return DEFAULT_PARALLEL_NODE_COST,
            PluginCostClass::Iir | PluginCostClass::Dynamics => {
                return MODERATE_PARALLEL_NODE_COST;
            }
            PluginCostClass::Fft | PluginCostClass::Convolution | PluginCostClass::External => {
                return HEAVY_PARALLEL_NODE_COST;
            }
        }

        let name = plugin.info().name.to_ascii_lowercase();
        if name.contains("spectral")
            || name.contains("linear phase")
            || name.contains("convolution")
            || name.contains("denoiser")
            || name.contains("declick")
            || name.contains("hiss")
            || name.contains("fir")
            || name.contains("fft")
        {
            HEAVY_PARALLEL_NODE_COST
        } else if name.contains("eq")
            || name.contains("compressor")
            || name.contains("limiter")
            || name.contains("expander")
            || name.contains("saturation")
            || name.contains("transient")
            || name.contains("delay")
            || name.contains("external")
        {
            MODERATE_PARALLEL_NODE_COST
        } else {
            DEFAULT_PARALLEL_NODE_COST
        }
    }

    pub fn add_plugin(&mut self, plugin: Box<dyn Plugin>) -> Result<(), String> {
        self.ensure_sink_graph_mutation_allowed()?;
        let id = self.reserve_node_id();
        self.add_plugin_with_id(id, plugin).map(|_| ())
    }

    pub(super) fn add_plugin_with_id(
        &mut self,
        id: NodeId,
        plugin: Box<dyn Plugin>,
    ) -> Result<NodeId, String> {
        self.ensure_sink_graph_mutation_allowed()?;
        let expected = if self.chain_nodes.is_empty() {
            self.config.initial_input_channels
        } else {
            self.nodes[self.chain_nodes.last().unwrap()].output_channels()
        };
        if plugin.input_channels() != expected {
            return Err("mismatch".into());
        }
        let name = format!("plugin_{id}");
        let input_rate =
            self.chain_nodes
                .iter()
                .try_fold(self.config.sample_rate, |rate, &node_id| {
                    let plugin = self.plugins[node_id].as_ref().unwrap();
                    Self::plugin_output_sample_rate_isolated(
                        plugin.as_ref(),
                        node_id,
                        &self.nodes[&node_id].name,
                        rate,
                    )
                })?;
        self.add_node_with_id_at_rate(id, name, plugin, input_rate)?;
        if let Some(&prev) = self.chain_nodes.last() {
            self.add_edge(GraphEdge::new(prev, id))?;
        }
        self.chain_nodes.push(id);
        self.chain_built = true;
        self.built = false;
        Ok(id)
    }

    pub fn remove_plugin(&mut self, index: usize) -> Result<Box<dyn Plugin>, String> {
        self.ensure_sink_graph_mutation_allowed()?;
        if index >= self.chain_nodes.len() {
            return Err("oob".into());
        }
        let id = self.chain_nodes.remove(index);
        self.edges.retain(|e| e.from_node != id && e.to_node != id);
        self.renumber_edges();
        if index > 0 && index < self.chain_nodes.len() {
            self.add_edge(GraphEdge::new(
                self.chain_nodes[index - 1],
                self.chain_nodes[index],
            ))?;
        }
        self.nodes.remove(&id).unwrap();
        self.built = false;
        self.cached_latency = None;
        self.drain_state = DrainState::default();
        self.graph_drain_state.reset_dynamic();
        Ok(self.plugins[id].take().unwrap())
    }

    pub(super) fn renumber_edges(&mut self) {
        for (idx, edge) in self.edges.iter_mut().enumerate() {
            edge.id = idx;
        }
    }

    pub fn plugin_count(&self) -> usize {
        self.chain_nodes.len()
    }
    pub fn get_plugin(&self, index: usize) -> Option<&dyn Plugin> {
        let &nid = self.chain_nodes.get(index)?;
        self.plugins.get(nid)?.as_deref()
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub fn poll_isolated_external_plugin_workers(
        &mut self,
    ) -> Vec<IsolatedExternalPluginWorkerReport> {
        let mut reports = Vec::new();
        for (plugin_index, node_id) in self.worker_report_nodes() {
            let Some(Some(plugin)) = self.plugins.get_mut(node_id) else {
                continue;
            };
            let Some(isolated) = plugin
                .as_any_mut()
                .and_then(|plugin| plugin.downcast_mut::<IsolatedExternalPlugin>())
            else {
                continue;
            };

            let (event, error) = match isolated.poll_worker() {
                Ok(event) => (event, None),
                Err(err) => (None, Some(err)),
            };
            reports.push(Self::isolated_external_plugin_report(
                plugin_index,
                node_id,
                isolated,
                event,
                error,
            ));
        }
        reports
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub fn ensure_isolated_external_plugin_workers_running(
        &mut self,
    ) -> Vec<IsolatedExternalPluginWorkerReport> {
        let mut reports = Vec::new();
        for (plugin_index, node_id) in self.worker_report_nodes() {
            let Some(Some(plugin)) = self.plugins.get_mut(node_id) else {
                continue;
            };
            let Some(isolated) = plugin
                .as_any_mut()
                .and_then(|plugin| plugin.downcast_mut::<IsolatedExternalPlugin>())
            else {
                continue;
            };

            let (event, error) = match isolated.ensure_worker_running_event() {
                Ok(event) => (Some(event), None),
                Err(err) => (None, Some(err)),
            };
            reports.push(Self::isolated_external_plugin_report(
                plugin_index,
                node_id,
                isolated,
                event,
                error,
            ));
        }
        reports
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub(super) fn isolated_external_plugin_report(
        plugin_index: usize,
        node_id: NodeId,
        plugin: &IsolatedExternalPlugin,
        event: Option<ExternalPluginProcessEvent>,
        error: Option<String>,
    ) -> IsolatedExternalPluginWorkerReport {
        let sandbox = plugin.worker_sandbox_status();
        // Prefer the worker's own diagnostic (captured stderr) over the
        // canned backend message when it reports itself unsupported.
        let worker_detail =
            sandbox_unsupported_detail(sandbox.status, plugin.last_worker_stderr().as_deref());
        let sandbox_reason = sandbox_reason_text(
            sandbox.status,
            sandbox.backend,
            error.as_deref().or(worker_detail.as_deref()),
        );
        IsolatedExternalPluginWorkerReport {
            plugin_index,
            node_id,
            plugin_instance_id: plugin.plugin_instance_id(),
            event,
            error,
            worker_start_count: plugin.worker_start_count(),
            worker_exit_count: plugin.worker_exit_count(),
            worker_launch_failure_count: plugin.worker_launch_failure_count(),
            worker_quarantined: plugin.is_worker_quarantined(),
            worker_quarantine_reason: plugin.worker_quarantine_reason().map(str::to_string),
            block_timeout_count: plugin.block_timeout_count(),
            block_worker_failure_count: plugin.block_worker_failure_count(),
            block_wrong_sequence_count: plugin.block_wrong_sequence_count(),
            sandbox_status: sandbox.status,
            sandbox_backend: sandbox.backend,
            sandbox_reason,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn worker_report_nodes(&self) -> Vec<(usize, NodeId)> {
        let mut node_ids = self.nodes.keys().copied().collect::<Vec<_>>();
        node_ids.sort_unstable();
        node_ids
            .into_iter()
            .map(|node_id| {
                let plugin_index = self
                    .chain_nodes
                    .iter()
                    .position(|&chain_node_id| chain_node_id == node_id)
                    .unwrap_or(node_id);
                (plugin_index, node_id)
            })
            .collect()
    }

    pub fn input_channels(&self) -> usize {
        if self.chain_nodes.is_empty() {
            self.config.initial_input_channels
        } else {
            self.nodes[&self.chain_nodes[0]].input_channels()
        }
    }
    pub fn output_channels(&self) -> usize {
        if self.chain_nodes.is_empty() {
            self.config.initial_input_channels
        } else {
            self.nodes[self.chain_nodes.last().unwrap()].output_channels()
        }
    }

    /// Test-only count of full output-FIFO commit retries retained
    /// by backpressure during the current graph drain stream.
    #[cfg(test)]
    pub(crate) fn graph_drain_output_commit_retries(&self) -> u64 {
        self.graph_drain_state.output_commit_retries
    }
    pub fn output_frames_for_input(&self, f: usize) -> usize {
        if self.cached_frames_identity {
            return f;
        }
        self.output_nodes
            .iter()
            .map(|&id| self.path_output_frames(id, f))
            .max()
            .unwrap_or(f)
    }

    /// Returns whether every active node guarantees identity frame geometry.
    ///
    /// Unlike the cached fast-path hint, this query does not infer identity
    /// from sampled block sizes. Every non-bypassed node must opt in, and its
    /// negotiated input and output sample rates must match.
    pub fn has_identity_frame_geometry(&self) -> bool {
        self.built
            && self.nodes.iter().all(|(&node_id, node)| {
                if node.bypassed {
                    return true;
                }

                let same_rate = self
                    .node_input_sample_rates
                    .get(node_id)
                    .zip(self.node_output_sample_rates.get(node_id))
                    .is_some_and(|(input_rate, output_rate)| input_rate == output_rate);
                let declared_identity = self
                    .plugins
                    .get(node_id)
                    .and_then(|plugin| plugin.as_deref())
                    .is_some_and(Plugin::guarantees_identity_frame_geometry);

                same_rate && declared_identity
            })
    }

    fn validate_terminal_sink_graph(&self) -> Result<(NodeId, Option<NodeId>), String> {
        if !self.built {
            return Err("terminal sink graph must be built before processing".into());
        }
        if self.chain_nodes.is_empty()
            || self.chain_nodes.len() != self.nodes.len()
            || self.input_nodes.len() != 1
            || self.output_nodes.len() != 1
            || self.input_nodes[0] != self.chain_nodes[0]
            || self.output_nodes[0] != *self.chain_nodes.last().unwrap()
            || self.edges.len() != self.chain_nodes.len().saturating_sub(1)
        {
            return Err("terminal sink route requires one connected serial chain".into());
        }
        let first_node = &self.nodes[&self.chain_nodes[0]];
        if first_node.input_channels() != self.config.initial_input_channels {
            return Err(format!(
                "terminal sink route starts with {} channels but the host input has {}",
                first_node.input_channels(),
                self.config.initial_input_channels
            ));
        }
        if !self.has_identity_frame_geometry() {
            return Err(
                "terminal sink route requires declared identity frame geometry and equal rates"
                    .into(),
            );
        }

        for (index, &node_id) in self.chain_nodes.iter().enumerate() {
            let node = &self.nodes[&node_id];
            if node.bypassed {
                return Err("terminal sink route does not support bypassed nodes".into());
            }
            let plugin = self.plugins[node_id]
                .as_ref()
                .ok_or("terminal sink route contains a missing plugin")?;
            if plugin.input_channels() != node.input_channels()
                || plugin.output_channels() != node.output_channels()
            {
                return Err(format!(
                    "terminal sink route node {node_id} changed its declared channel geometry"
                ));
            }
            if index + 1 < self.chain_nodes.len() && node.input_channels() != node.output_channels()
            {
                return Err("terminal sink route requires channel-preserving source nodes".into());
            }
            if index > 0 {
                let previous = self.chain_nodes[index - 1];
                if self.nodes[&previous].output_channels() != node.input_channels() {
                    return Err(format!(
                        "terminal sink route channel mismatch between nodes {previous} and {node_id}"
                    ));
                }
                let edge = self
                    .edges
                    .iter()
                    .find(|edge| edge.from_node == previous && edge.to_node == node_id)
                    .ok_or("terminal sink route is missing a serial audio edge")?;
                if edge.edge_type != EdgeType::Audio
                    || edge.channel_map.is_some()
                    || edge.destination_offset != 0
                {
                    return Err("terminal sink route requires direct serial audio edges".into());
                }
            }
        }

        let sink_id = *self.chain_nodes.last().unwrap();
        Self::checked_native_sink_rate(self.node_input_sample_rates[sink_id])?;
        let sink_node = &self.nodes[&sink_id];
        let sink_plugin = self.plugins[sink_id].as_ref().unwrap();
        if sink_node.input_channels() == 0
            || sink_node.output_channels() != 0
            || sink_plugin.terminal_sink().is_none()
        {
            return Err(
                "terminal chain node does not advertise a zero-output sink contract".into(),
            );
        }
        let source_id = self
            .chain_nodes
            .len()
            .checked_sub(2)
            .map(|index| self.chain_nodes[index]);
        if let Some(source_id) = source_id {
            let source = &self.nodes[&source_id];
            if source.output_channels() != sink_node.input_channels() {
                return Err("terminal source and sink channel geometry do not match".into());
            }
        }
        Ok((sink_id, source_id))
    }

    fn checked_native_sink_rate(rate: f64) -> Result<u32, String> {
        if !rate.is_finite() || rate <= 0.0 || rate > f64::from(u32::MAX) || rate.fract() != 0.0 {
            return Err(format!(
                "Physical sink requires a positive integral u32 sample rate, got {rate}"
            ));
        }
        Ok(rate as u32)
    }

    fn advance_sample_position(position: u64, frames: usize) -> Result<u64, String> {
        let frames =
            u64::try_from(frames).map_err(|_| "Graph frame count exceeds u64".to_owned())?;
        position
            .checked_add(frames)
            .ok_or_else(|| "Graph timeline position exceeds u64".to_owned())
    }

    fn preflight_process_horizon(&self, input_frames: usize) -> Result<(), String> {
        let clock = self
            .exact_clock
            .ok_or("Graph exact clock is not prepared")?;
        let host_position = u64::try_from(self.automation_state.playback_position)
            .map_err(|_| "Graph playback position exceeds u64 frames".to_owned())?;
        clock.preflight_horizon(
            host_position,
            input_frames,
            ExactRate::new(self.config.sample_rate)?,
        )?;
        // Prepared capacity bounds every node's accepted INPUT wave (including
        // a downsampler, whose output can be much shorter) for ordinary
        // callbacks. A larger callback also accounts for pre-existing queued
        // waves; only that exceptional path walks the graph envelopes.
        let frames = if input_frames <= Self::MAX_BLOCK_FRAMES {
            self.prepared_clock_horizon_frames
        } else {
            let current_wave = self
                .nodes
                .keys()
                .map(|&id| {
                    self.path_output_envelope(id, input_frames)
                        .unwrap_or_else(|| self.path_output_frames(id, input_frames))
                })
                .max()
                .unwrap_or(input_frames)
                .max(input_frames);
            self.prepared_clock_horizon_frames
                .checked_add(current_wave)
                .ok_or("Graph input wave horizon exceeds addressable frames")?
        };
        for &id in self.nodes.keys() {
            let rate = self.node_input_sample_rates[id];
            clock.preflight_horizon(
                self.node_input_positions[id],
                frames,
                ExactRate::new(rate)?,
            )?;
        }
        Ok(())
    }

    fn validate_terminal_sink_tail_source(
        &self,
        source_id: Option<NodeId>,
    ) -> Result<(), SinkDrainError> {
        let Some(sink_id) = self.chain_nodes.last().copied() else {
            return Err(SinkDrainError::InvalidRoute(
                "terminal sink route has no sink node".into(),
            ));
        };
        for &node_id in self
            .chain_nodes
            .iter()
            .filter(|&&node_id| node_id != sink_id)
        {
            let plugin = self.plugins[node_id]
                .as_ref()
                .ok_or_else(|| SinkDrainError::InvalidRoute(format!("missing plugin {node_id}")))?;
            let tail = plugin.tail_length();
            if Some(node_id) == source_id {
                if !matches!(tail, TailLength::Finite(_)) {
                    return Err(SinkDrainError::UnsupportedTailMetadata { node_id, tail });
                }
            } else if tail != TailLength::Finite(0) {
                return Err(SinkDrainError::UnsupportedTailMetadata { node_id, tail });
            } else if plugin.drain_output_frames_max() != 0 {
                return Err(SinkDrainError::InvalidRoute(format!(
                    "upstream terminal-sink node {node_id} reports drain output despite a zero tail"
                )));
            }
        }
        Ok(())
    }

    fn path_output_frames(&self, id: NodeId, frames: usize) -> usize {
        let input_frames = self
            .predecessors
            .get(id)
            .into_iter()
            .flatten()
            .map(|edge| self.path_output_frames(edge.from_node, frames))
            .max()
            .unwrap_or(frames);
        if self.nodes[&id].bypassed {
            return input_frames;
        }
        Self::plugin_output_frames_for_input_isolated(
            self.plugins[id].as_ref().unwrap().as_ref(),
            id,
            &self.nodes[&id].name,
            input_frames,
        )
    }

    /// Stream-independent upper bound on whole-host production for `f`
    /// input frames, mirroring [`Self::output_frames_for_input`]. Any
    /// unknown plugin envelope, missing node, or structural anomaly
    /// answers `None` so callers keep live sizing. Allocation-free apart
    /// from the plugin queries themselves (which are realtime-safe by
    /// contract); the recursion depth follows the topology. Like the live
    /// query, this trusts the build-time sampled identity fast path: a
    /// variable geometry coinciding with identity at the probe size would
    /// mislead both identically (mid-call growth, never silent
    /// corruption); no in-tree geometry does.
    pub fn output_frames_envelope(&self, f: usize) -> Option<usize> {
        if self.cached_frames_identity {
            return Some(f);
        }
        let mut maximum: Option<usize> = None;
        for &id in &self.output_nodes {
            let path = self.path_output_envelope(id, f)?;
            maximum = Some(maximum.map_or(path, |current| current.max(path)));
        }
        Some(maximum.unwrap_or(f))
    }

    /// Envelope propagation along one path, mirroring
    /// [`Self::path_output_frames`]. Bypassed nodes pass the count
    /// through; any unknown envelope answers `None`.
    fn path_output_envelope(&self, id: NodeId, frames: usize) -> Option<usize> {
        let mut input_frames: Option<usize> = None;
        if let Some(predecessors) = self.predecessors.get(id) {
            for edge in predecessors {
                let path = self.path_output_envelope(edge.from_node, frames)?;
                input_frames = Some(input_frames.map_or(path, |current| current.max(path)));
            }
        }
        let input_frames = input_frames.unwrap_or(frames);
        let node = self.nodes.get(&id)?;
        if node.bypassed {
            return Some(input_frames);
        }
        let plugin = self.plugins.get(id)?.as_ref()?;
        Self::plugin_output_frames_envelope_isolated(plugin.as_ref(), id, &node.name, input_frames)
    }

    /// Largest per-call node-buffer need at MAX_BLOCK_FRAMES: the block
    /// size itself maxed with the host output propagation (envelope where
    /// known, live otherwise). Sizes cold-process preparation, so the
    /// first callback (and every block within the documented ceiling)
    /// allocates nothing. The per-call need `nf.max(host(nf))` is
    /// non-decreasing in `nf` for envelopes (contract) and assumed so
    /// for live declarations (the same assumption live sizing already
    /// makes). Propagated directly (no cached fast path: build refreshes
    /// the cache after preparing buffers). Control-thread only.
    fn max_block_process_need(&self) -> usize {
        let mut need = Self::MAX_BLOCK_FRAMES;
        for &id in &self.output_nodes {
            let path = self
                .path_output_envelope(id, Self::MAX_BLOCK_FRAMES)
                .unwrap_or_else(|| self.path_output_frames(id, Self::MAX_BLOCK_FRAMES));
            need = need.max(path);
        }
        need
    }

    /// Parallel-scratch output need for one node at MAX_BLOCK_FRAMES: the
    /// isolated envelope-or-live bound, never below MAX_BLOCK (today's
    /// size). The parallel fast path sizes per-call output from the live
    /// declaration at the block size, so the MAX_BLOCK value covers every
    /// smaller block (envelope monotonicity; same live assumption as
    /// everywhere else). Control-thread only.
    fn parallel_output_prep_frames(&self, id: NodeId, node: &GraphNode) -> usize {
        if node.bypassed {
            return Self::MAX_BLOCK_FRAMES;
        }
        let Some(plugin) = self.plugins.get(id).and_then(Option::as_ref) else {
            return Self::MAX_BLOCK_FRAMES;
        };
        Self::MAX_BLOCK_FRAMES.max(
            Self::plugin_output_frames_envelope_isolated(
                plugin.as_ref(),
                id,
                &node.name,
                Self::MAX_BLOCK_FRAMES,
            )
            .unwrap_or_else(|| {
                Self::plugin_output_frames_for_input_isolated(
                    plugin.as_ref(),
                    id,
                    &node.name,
                    Self::MAX_BLOCK_FRAMES,
                )
            }),
        )
    }

    pub fn output_sample_rate<S: Into<f64>>(&self, rate: S) -> Result<f64, String> {
        let rate = rate.into();
        if !rate.is_finite() || rate <= 0.0 {
            return Err("Input sample rate must be finite and positive".to_string());
        }
        // Build-time identity describes the prepared graph, but a plugin may
        // report invalid rate metadata later. Query the active path so the
        // native boundary can reject it before the audio frame is published.
        self.output_nodes
            .first()
            .map_or(Ok(rate), |&id| self.path_output_rate(id, rate))
    }

    /// Checked boundary for native device and file clocks, which are u32.
    pub fn output_sample_rate_native(&self, input_rate: u32) -> Result<u32, String> {
        Self::checked_native_sink_rate(self.output_sample_rate(input_rate)?)
    }

    /// Whether every host output negotiates to `expected` from `input_rate`.
    ///
    /// Same isolated rate walk as [`Self::output_sample_rate`] over all
    /// output nodes instead of the first only, so multi-output gates cannot
    /// miss a misclocked sibling. Allocates nothing; audio-thread safe. An
    /// empty host passes through, so it checks the input rate itself.
    pub fn all_output_sample_rates_equal<I: Into<f64>, E: Into<f64>>(
        &self,
        input_rate: I,
        expected: E,
    ) -> bool {
        let input_rate = input_rate.into();
        let expected = expected.into();
        if !input_rate.is_finite() || input_rate <= 0.0 || !expected.is_finite() || expected <= 0.0
        {
            return false;
        }
        if self.output_nodes.is_empty() {
            return input_rate == expected;
        }
        self.output_nodes.iter().all(|&id| {
            self.path_output_rate(id, input_rate)
                .is_ok_and(|rate| rate == expected)
        })
    }

    fn path_output_rate(&self, id: NodeId, rate: f64) -> Result<f64, String> {
        let input_rate = self
            .predecessors
            .get(id)
            .and_then(|edges| edges.first())
            .map_or(Ok(rate), |edge| self.path_output_rate(edge.from_node, rate))?;
        if self.nodes[&id].bypassed {
            return Ok(input_rate);
        }
        Self::plugin_output_sample_rate_isolated(
            self.plugins[id].as_ref().unwrap().as_ref(),
            id,
            &self.nodes[&id].name,
            input_rate,
        )
    }

    pub fn last_output_frames(&self) -> Option<usize> {
        for &id in self.chain_nodes.iter().rev() {
            if let Some(f) = self.plugins[id].as_ref().unwrap().last_output_frames() {
                return Some(f);
            }
        }
        None
    }
    pub fn set_plugin_parameter(
        &mut self,
        index: usize,
        id: &str,
        val: super::super::parameters::ParameterValue,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        let &nid = self.chain_nodes.get(index).ok_or("oob")?;
        self.queue_node_parameter(nid, super::super::parameters::ParameterId::from(id), val)
    }

    /// Validate a host-visible parameter without mutating or queueing it.
    /// This is a control-thread operation: querying plugin metadata may allocate.
    pub fn validate_plugin_parameter(
        &self,
        index: usize,
        id: &str,
        value: &super::super::parameters::ParameterValue,
    ) -> Result<(), String> {
        let &node_id = self
            .chain_nodes
            .get(index)
            .ok_or("plugin index out of bounds")?;
        let plugin = self
            .plugins
            .get(node_id)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("Plugin for node {node_id} not found"))?;
        let parameters =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.parameters()))
                .map_err(|_| {
                    format!("Plugin at index {index} panicked while listing parameters")
                })?;
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.id.as_str() == id)
            .ok_or_else(|| format!("Unknown parameter '{id}' for plugin index {index}"))?;
        parameter
            .validate(value)
            .map_err(|reason| format!("Invalid value for parameter '{id}': {reason}"))
    }

    /// Validate a parameter for sample-accurate live automation. Structural
    /// controls must rebuild the graph and are never safe to apply mid-block.
    pub fn validate_automatable_plugin_parameter(
        &self,
        index: usize,
        id: &str,
        value: &super::super::parameters::ParameterValue,
    ) -> Result<(), String> {
        self.validate_plugin_parameter(index, id, value)?;
        let &node_id = self
            .chain_nodes
            .get(index)
            .ok_or("plugin index out of bounds")?;
        let plugin = self
            .plugins
            .get(node_id)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("Plugin for node {node_id} not found"))?;
        let parameters =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.parameters()))
                .map_err(|_| {
                    format!("Plugin at index {index} panicked while listing parameters")
                })?;
        if parameters.iter().any(|parameter| {
            parameter.id.as_str() == id
                && parameter.update_mode == crate::param_specs::UpdateMode::Structural
        }) {
            return Err(format!(
                "Parameter {id} requires rebuilding the plugin chain"
            ));
        }
        Ok(())
    }

    /// Queue a parameter change for audio-thread application at `sample_offset`
    /// frames into the next `process()` call.
    pub fn set_plugin_parameter_at(
        &mut self,
        index: usize,
        id: &str,
        val: super::super::parameters::ParameterValue,
        sample_offset: usize,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.validate_automatable_plugin_parameter(index, id, &val)?;
        let &nid = self.chain_nodes.get(index).ok_or("oob")?;
        self.queue_node_parameter_at(
            nid,
            super::super::parameters::ParameterId::from(id),
            val,
            sample_offset,
        )
    }

    /// Queue a parameter change for audio-thread application at the start of
    /// the next `process()` call.
    pub fn queue_node_parameter(
        &mut self,
        node_id: NodeId,
        param_id: ParameterId,
        value: ParameterValue,
    ) -> Result<(), String> {
        self.queue_node_parameter_at(node_id, param_id, value, 0)
    }

    /// Queue a parameter change for audio-thread application at `sample_offset`
    /// frames into the next `process()` call.
    pub fn queue_node_parameter_at(
        &mut self,
        node_id: NodeId,
        param_id: ParameterId,
        value: ParameterValue,
        sample_offset: usize,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        if !self.nodes.contains_key(&node_id) {
            return Err("Node not found".into());
        }

        let event = ParameterEvent::new(node_id, param_id, value, sample_offset);
        let producer = self.queues.parameter_event_tx.as_mut().ok_or_else(|| {
            "parameter event sender has been taken; use the returned ParameterEventSender"
                .to_string()
        })?;
        producer.push(event).map_err(|err| {
            self.queue_state.dropped_parameter_events =
                self.queue_state.dropped_parameter_events.saturating_add(1);
            crate::rate_limited_log!(
                warn,
                5,
                "host: parameter event queue full; dropped {} events",
                self.queue_state.dropped_parameter_events
            );
            format!("parameter event queue full: {err:?}")
        })
    }

    /// Move the parameter-event producer out of the host.
    ///
    /// After this is called, use the returned `ParameterEventSender` from the
    /// control/UI side. `DawHost` keeps the consumer and continues draining
    /// events in `process()`.
    pub fn take_parameter_event_sender(&mut self) -> Option<ParameterEventSender> {
        if self.terminal_sink_lifecycle.is_some() {
            return None;
        }
        let chain_nodes = self.chain_nodes.clone();
        let parameters = chain_nodes
            .iter()
            .filter_map(|&node_id| {
                let plugin = self.plugins.get(node_id)?.as_ref()?;
                let parameters =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.parameters()))
                        .ok()?;
                Some((node_id, parameters))
            })
            .collect();
        self.queues
            .parameter_event_tx
            .take()
            .map(|producer| ParameterEventSender {
                producer,
                dropped_events: 0,
                chain_nodes,
                parameters,
            })
    }

    /// Move the graph-mutation producer out of the host.
    ///
    /// After this is called, use the returned `GraphMutationSender` from the
    /// control/UI side. `DawHost` keeps the consumer, applies queued graph
    /// changes before processing, and publishes the rebuilt topology snapshot.
    pub fn take_graph_mutation_sender(&mut self) -> Option<GraphMutationSender> {
        if self.terminal_sink_lifecycle.is_some() {
            return None;
        }
        self.queues
            .graph_mutation_tx
            .take()
            .map(|producer| GraphMutationSender {
                producer,
                next_node_id: Arc::clone(&self.queue_state.graph_next_node_id),
                dropped_mutations: 0,
            })
    }

    /// Apply a parameter immediately on the calling thread.
    ///
    /// Control thread only, between blocks: the engine runs this on the
    /// processing thread for `SetParameter` commands; offline setup, tests,
    /// and migration code call it directly. Never call from the audio
    /// callback; use `set_plugin_parameter()` / `queue_node_parameter()`
    /// for realtime control. Structural parameters still require a
    /// rebuild, except ids the plugin explicitly opts in via
    /// `supports_immediate_momentary_control`. Metadata probes fail
    /// closed: a panicking plugin reports an error without mutation.
    pub fn set_plugin_parameter_immediate(
        &mut self,
        index: usize,
        id: &str,
        val: super::super::parameters::ParameterValue,
    ) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        let &nid = self.chain_nodes.get(index).ok_or("oob")?;
        let (is_structural, momentary_allowed) = {
            let plugin = self
                .plugins
                .get(nid)
                .and_then(Option::as_ref)
                .ok_or("plugin not found")?;
            let parameters =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.parameters()))
                    .map_err(|_| {
                        format!("Plugin at index {index} panicked while listing parameters")
                    })?;
            let is_structural = parameters
                .iter()
                .find(|parameter| parameter.id.as_str() == id)
                .is_some_and(|parameter| {
                    parameter.update_mode == crate::param_specs::UpdateMode::Structural
                });
            let allowed = if is_structural {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    plugin.supports_immediate_momentary_control(
                        &super::super::parameters::ParameterId::from(id),
                    )
                }))
                .map_err(|_| {
                    format!("Plugin at index {index} panicked while reporting momentary controls")
                })?
            } else {
                false
            };
            (is_structural, allowed)
        };
        if is_structural && !momentary_allowed {
            return Err(format!(
                "Parameter {id} requires rebuilding the plugin chain"
            ));
        }
        self.apply_parameter_event(ParameterEvent {
            node_id: nid,
            param_id: super::super::parameters::ParameterId::from(id),
            value: val,
            sample_offset: 0,
        })
    }

    pub(super) fn drain_parameter_events_into(&mut self, events: &mut Vec<ParameterEvent>) {
        events.clear();
        while let Ok(event) = self.queues.parameter_event_rx.pop() {
            events.push(event);
        }
    }

    pub(super) fn plugin_supports_f64_isolated(
        plugin: &dyn Plugin,
        node_id: NodeId,
        node_name: &str,
    ) -> bool {
        match catch_unwind(AssertUnwindSafe(|| plugin.supports_f64())) {
            Ok(supports_f64) => supports_f64,
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in supports_f64: {}; using f32 bridge",
                    node_name,
                    node_id,
                    reason
                );
                false
            }
        }
    }

    pub(super) fn plugin_output_frames_for_input_isolated(
        plugin: &dyn Plugin,
        node_id: NodeId,
        node_name: &str,
        input_frames: usize,
    ) -> usize {
        match catch_unwind(AssertUnwindSafe(|| {
            plugin.output_frames_for_input(input_frames)
        })) {
            Ok(output_frames) => output_frames,
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in output_frames_for_input: {}; assuming identity frame count",
                    node_name,
                    node_id,
                    reason
                );
                input_frames
            }
        }
    }

    /// Isolated `output_frames_envelope` query: a panicking plugin
    /// answers unknown (`None`) instead of unwinding into the host, so the
    /// graph keeps live sizing. Allocation-free on the query itself; the
    /// rate-limited panic log may allocate, exactly like the live query.
    pub(super) fn plugin_output_frames_envelope_isolated(
        plugin: &dyn Plugin,
        node_id: NodeId,
        node_name: &str,
        input_frames: usize,
    ) -> Option<usize> {
        match catch_unwind(AssertUnwindSafe(|| {
            plugin.output_frames_envelope(input_frames)
        })) {
            Ok(envelope) => envelope,
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in output_frames_envelope: {}; assuming unknown envelope",
                    node_name,
                    node_id,
                    reason
                );
                None
            }
        }
    }

    pub(super) fn plugin_output_sample_rate_isolated(
        plugin: &dyn Plugin,
        node_id: NodeId,
        node_name: &str,
        input_rate: f64,
    ) -> Result<f64, String> {
        match catch_unwind(AssertUnwindSafe(|| plugin.output_sample_rate(input_rate))) {
            Ok(output_rate) => {
                if !output_rate.is_finite() || output_rate <= 0.0 {
                    return Err(format!(
                        "Node {node_id} ({node_name}) returned invalid output sample rate {output_rate}"
                    ));
                }
                Ok(output_rate)
            }
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in output_sample_rate: {}; assuming identity sample rate",
                    node_name,
                    node_id,
                    reason
                );
                Ok(input_rate)
            }
        }
    }

    pub(super) fn process_plugin_f32_isolated(
        plugin: &mut dyn Plugin,
        node: &GraphNode,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext<'_>,
    ) -> IsolatedProcessOutcome {
        let fallback = |output: &mut [f32]| {
            write_plugin_failure_passthrough(
                input,
                output,
                context.num_frames,
                node.input_channels(),
                node.output_channels(),
            )
        };

        match catch_unwind(AssertUnwindSafe(|| plugin.process(input, output, context))) {
            Ok(Ok(frames)) => {
                if frames.saturating_mul(node.output_channels()) <= output.len() {
                    IsolatedProcessOutcome {
                        frames,
                        over_reported: None,
                    }
                } else {
                    crate::rate_limited_log!(
                        error,
                        5,
                        "host: plugin '{}' (node {}) returned {} frames but output buffer holds {} samples; using passthrough for this block",
                        node.name,
                        node.id,
                        frames,
                        output.len()
                    );
                    IsolatedProcessOutcome {
                        frames: fallback(output),
                        over_reported: Some(frames),
                    }
                }
            }
            Ok(Err(err)) => {
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) process failed: {err}; using passthrough for this block",
                    node.name,
                    node.id
                );
                IsolatedProcessOutcome {
                    frames: fallback(output),
                    over_reported: None,
                }
            }
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in process: {}; using passthrough for this block",
                    node.name,
                    node.id,
                    reason
                );
                IsolatedProcessOutcome {
                    frames: fallback(output),
                    over_reported: None,
                }
            }
        }
    }

    pub(super) fn process_compiled_plugin_f32_isolated(
        plugin: &mut dyn Plugin,
        node: &GraphNode,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext<'_>,
    ) -> Option<usize> {
        let result = match catch_unwind(AssertUnwindSafe(|| {
            plugin.process_compiled_f32(op, input, output, context)
        })) {
            Ok(result) => result?,
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in compiled {:?}: {}; using regular process for this block",
                    node.name,
                    node.id,
                    op,
                    reason
                );
                return None;
            }
        };

        match result {
            Ok(frames) => {
                if frames.saturating_mul(node.output_channels()) <= output.len() {
                    Some(frames)
                } else {
                    crate::rate_limited_log!(
                        error,
                        5,
                        "host: plugin '{}' (node {}) compiled {:?} returned {} frames but output buffer holds {} samples; using regular process for this block",
                        node.name,
                        node.id,
                        op,
                        frames,
                        output.len()
                    );
                    None
                }
            }
            Err(err) => {
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) compiled {:?} failed: {err}; using regular process for this block",
                    node.name,
                    node.id,
                    op
                );
                None
            }
        }
    }

    pub(super) fn process_analyzer_tap_f32_isolated(
        plugin: &mut dyn Plugin,
        node: &GraphNode,
        input: &[f32],
        context: &ProcessContext<'_>,
    ) -> bool {
        let result = match catch_unwind(AssertUnwindSafe(|| {
            plugin.process_analyzer_tap_f32(input, context)
        })) {
            Ok(None) => return false,
            Ok(Some(result)) => result,
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in zero-copy analyzer tap: {}; preserving transparent audio without analyzer reentry",
                    node.name,
                    node.id,
                    reason
                );
                return true;
            }
        };
        match result {
            Ok(frames) if frames == context.num_frames => true,
            Ok(frames) => {
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) zero-copy analyzer tap returned {} frames, expected {}; preserving transparent audio without analyzer reentry",
                    node.name,
                    node.id,
                    frames,
                    context.num_frames
                );
                true
            }
            Err(err) => {
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) zero-copy analyzer tap failed: {err}; preserving transparent audio without analyzer reentry",
                    node.name,
                    node.id
                );
                true
            }
        }
    }

    pub(super) fn process_plugin_f64_isolated(
        plugin: &mut dyn Plugin,
        node: &GraphNode,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext<'_>,
    ) -> IsolatedProcessOutcome {
        let fallback = |output: &mut [f64]| {
            write_plugin_failure_passthrough(
                input,
                output,
                context.num_frames,
                node.input_channels(),
                node.output_channels(),
            )
        };

        match catch_unwind(AssertUnwindSafe(|| {
            plugin.process_f64(input, output, context)
        })) {
            Ok(Ok(frames)) => {
                if frames.saturating_mul(node.output_channels()) <= output.len() {
                    IsolatedProcessOutcome {
                        frames,
                        over_reported: None,
                    }
                } else {
                    crate::rate_limited_log!(
                        error,
                        5,
                        "host: plugin '{}' (node {}) returned {} f64 frames but output buffer holds {} samples; using passthrough for this block",
                        node.name,
                        node.id,
                        frames,
                        output.len()
                    );
                    IsolatedProcessOutcome {
                        frames: fallback(output),
                        over_reported: Some(frames),
                    }
                }
            }
            Ok(Err(err)) => {
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) f64 process failed: {err}; using passthrough for this block",
                    node.name,
                    node.id
                );
                IsolatedProcessOutcome {
                    frames: fallback(output),
                    over_reported: None,
                }
            }
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked in f64 process: {}; using passthrough for this block",
                    node.name,
                    node.id,
                    reason
                );
                IsolatedProcessOutcome {
                    frames: fallback(output),
                    over_reported: None,
                }
            }
        }
    }

    pub(super) fn apply_parameter_event(&mut self, event: ParameterEvent) -> Result<(), String> {
        let ParameterEvent {
            node_id,
            param_id,
            value,
            sample_offset: _,
        } = event;
        let node = self
            .nodes
            .get(&node_id)
            .ok_or_else(|| format!("Node {node_id} not found"))?;
        let plugin = self
            .plugins
            .get_mut(node_id)
            .and_then(Option::as_mut)
            .ok_or_else(|| format!("Plugin for node {node_id} not found"))?;
        let result = match catch_unwind(AssertUnwindSafe(|| plugin.set_parameter(param_id, value)))
        {
            Ok(result) => result.map_err(|err| {
                crate::rate_limited_log!(
                    warn,
                    5,
                    "host: queued parameter event failed for node {} '{}': {err}",
                    node_id,
                    node.name
                );
                err
            }),
            Err(payload) => {
                let reason = panic_payload_description(payload.as_ref());
                crate::rate_limited_log!(
                    error,
                    5,
                    "host: plugin '{}' (node {}) panicked while applying parameter: {}",
                    node.name,
                    node_id,
                    reason
                );
                Err(format!(
                    "plugin '{}' (node {}) panicked while applying parameter: {}",
                    node.name, node_id, reason
                ))
            }
        };
        if result.is_ok() && self.drain_state.active_node == Some(node_id) {
            // Accepted actions can restart work even when getter values are
            // unchanged. Unrelated controls and rejected writes do not rearm.
            self.drain_state.remaining_calls = None;
            self.drain_state.quota_grant_tail = None;
        }
        if result.is_ok() {
            self.graph_drain_state.rearm_node(node_id);
        }
        result
    }

    /// Number of parameter events dropped because the RT queue was full.
    pub fn dropped_parameter_events(&self) -> u64 {
        self.queue_state.dropped_parameter_events
    }

    /// Queue a graph mutation through the host-owned producer.
    ///
    /// This is useful before handing the producer to another thread. After
    /// `take_graph_mutation_sender()` is called, use that sender instead.
    pub(super) fn queue_graph_mutation(&mut self, mutation: GraphMutation) -> Result<(), String> {
        let producer = self.queues.graph_mutation_tx.as_mut().ok_or_else(|| {
            "graph mutation sender has been taken; use the returned GraphMutationSender".to_string()
        })?;
        producer.push(mutation).map_err(|err| {
            self.queue_state.dropped_graph_mutations =
                self.queue_state.dropped_graph_mutations.saturating_add(1);
            crate::rate_limited_log!(
                warn,
                5,
                "host: graph mutation queue full; dropped {} mutations",
                self.queue_state.dropped_graph_mutations
            );
            format!("graph mutation queue full: {err:?}")
        })
    }

    /// Queue a linear-chain plugin append for audio-thread application.
    pub fn queue_add_plugin(&mut self, plugin: Box<dyn Plugin>) -> Result<NodeId, String> {
        self.ensure_sink_commands_allowed()?;
        let id = self.reserve_node_id();
        self.queue_graph_mutation(GraphMutation::AddPlugin { id, plugin })
            .map(|()| id)
    }

    /// Reserve a node id and queue a named node insertion for audio-thread application.
    pub fn queue_add_node(
        &mut self,
        name: String,
        plugin: Box<dyn Plugin>,
    ) -> Result<NodeId, String> {
        self.ensure_sink_commands_allowed()?;
        let id = self.reserve_node_id();
        self.queue_graph_mutation(GraphMutation::AddNode { id, name, plugin })
            .map(|()| id)
    }

    /// Queue an edge insertion for audio-thread application.
    pub fn queue_add_edge(&mut self, edge: GraphEdge) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.queue_graph_mutation(GraphMutation::AddEdge(edge))
    }

    /// Queue a linear-chain plugin removal for audio-thread application.
    pub fn queue_remove_plugin(&mut self, index: usize) -> Result<(), String> {
        self.ensure_sink_commands_allowed()?;
        self.queue_graph_mutation(GraphMutation::RemovePlugin { index })
    }

    /// Number of graph mutations dropped because the RT queue was full.
    pub fn dropped_graph_mutations(&self) -> u64 {
        self.queue_state.dropped_graph_mutations
    }

    pub(super) fn drain_graph_mutations(&mut self) -> Result<(), String> {
        while let Ok(mutation) = self.queues.graph_mutation_rx.pop() {
            self.apply_graph_mutation(mutation)?;
        }
        Ok(())
    }

    pub(super) fn apply_graph_mutation(&mut self, mutation: GraphMutation) -> Result<(), String> {
        match mutation {
            GraphMutation::AddNode { id, name, plugin } => self.add_node_with_id(id, name, plugin),
            GraphMutation::AddPlugin { id, plugin } => {
                self.add_plugin_with_id(id, plugin).map(|_| ())
            }
            GraphMutation::AddEdge(edge) => self.add_edge(edge),
            GraphMutation::RemovePlugin { index } => self.remove_plugin(index).map(|_| ()),
        }
    }

    /// Bypass a node so its plugin is skipped during processing.
    /// When bypassed, input is passed directly to output.
    /// Only works for nodes with matching input/output channel counts.
    pub fn bypass_node(&mut self, id: NodeId) -> Result<(), String> {
        self.ensure_sink_graph_mutation_allowed()?;
        {
            let node = self.nodes.get(&id).ok_or("Node not found")?;
            if node.input_channels != node.output_channels {
                return Err(format!(
                    "Cannot bypass node '{}': input channels ({}) != output channels ({})",
                    node.name, node.input_channels, node.output_channels
                ));
            }
        }
        self.set_bypass_state(id, true);
        Ok(())
    }

    /// Unbypass a node so its plugin resumes processing.
    pub fn unbypass_node(&mut self, id: NodeId) -> Result<(), String> {
        self.ensure_sink_graph_mutation_allowed()?;
        if !self.nodes.contains_key(&id) {
            return Err("Node not found".into());
        }
        self.set_bypass_state(id, false);
        Ok(())
    }

    /// Single write-through helper that keeps `GraphNode::bypassed` (the
    /// authoritative model) and the flat `self.bypassed[id]` cache in sync.
    /// All bypass mutations must funnel through here so the two views can
    /// never diverge.
    pub(super) fn set_bypass_state(&mut self, id: NodeId, bypassed: bool) {
        if let Some(node) = self.nodes.get_mut(&id) {
            if node.bypassed != bypassed {
                self.drain_state = DrainState::default();
                self.graph_drain_state.reset_dynamic();
            }
            node.bypassed = bypassed;
        }
        if id < self.bypassed.len() {
            self.bypassed[id] = bypassed;
        }
        self.cached_latency = None;
        self.built = false;
    }

    /// Returns true if the given node is bypassed.
    pub fn is_node_bypassed(&self, id: NodeId) -> Result<bool, String> {
        let node = self.nodes.get(&id).ok_or("Node not found")?;
        Ok(node.bypassed)
    }

    /// Bypass a plugin by chain index (0-based index into the plugin chain).
    pub fn bypass_plugin(&mut self, index: usize) -> Result<(), String> {
        let &nid = self.chain_nodes.get(index).ok_or("oob")?;
        self.bypass_node(nid)
    }

    /// Unbypass a plugin by chain index.
    pub fn unbypass_plugin(&mut self, index: usize) -> Result<(), String> {
        let &nid = self.chain_nodes.get(index).ok_or("oob")?;
        self.unbypass_node(nid)
    }

    /// Returns true if the plugin at the given chain index is bypassed.
    pub fn is_plugin_bypassed(&self, index: usize) -> Result<bool, String> {
        let &nid = self.chain_nodes.get(index).ok_or("oob")?;
        self.is_node_bypassed(nid)
    }

    /// Returns indices of plugins that have analyzer data (get_data() returns Some).
    /// Computed during build() to avoid per-frame discovery.
    pub fn analyzer_indices(&self) -> &[usize] {
        &self.analyzer_indices
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<usize, String> {
        if self.merge_overflow_poisoned {
            return Err(
                "host merge retention overflowed; reset the host before further processing".into(),
            );
        }
        if self.terminal_sink_lifecycle.is_some() || self.has_advertised_terminal_sink() {
            return Err("terminal sink graphs require explicit process_to_sink processing".into());
        }
        self.drain_graph_mutations()?;
        if !self.built {
            self.build()?;
        }
        self.preflight_process_horizon(input.len() / self.input_channels())?;
        if let Some((index, sample)) = input
            .iter()
            .copied()
            .enumerate()
            .find(|(_, sample)| !sample.is_finite())
        {
            output.fill(0.0);
            return Err(format!(
                "host input contains non-finite sample at index {index}: {sample}"
            ));
        }
        let mut events = std::mem::take(&mut self.queues.parameter_event_scratch);
        self.drain_parameter_events_into(&mut events);
        let result = self.process_with_parameter_events(input, output, &mut events, true);
        self.queues.parameter_event_scratch = events;
        if result.is_ok() && !input.is_empty() {
            self.drain_state = DrainState::default();
            self.graph_drain_state.reset_dynamic();
        }
        result
    }

    /// Process an input block and report actual produced frames, never padded.
    ///
    /// Same render core as [`process`](Self::process) (guards, parameter
    /// events, automation, drain-state reset), except short blocks are NOT
    /// padded up to the input length: the return value is the actual
    /// collected frame count `cf`, and output beyond `cf` frames is left
    /// untouched, so the caller must use the returned count. Identity and
    /// rate-converting chains report identically on both entries (padding
    /// only ever triggers for same-clock variable chains); only the masked
    /// short-production case differs, and there this entry is the truthful
    /// one. Additive API: `process` keeps its padded contract for existing
    /// callers. This is the structural closer for the AB observability
    /// residual (R2 §5): consumers that pair frames by stream position no
    /// longer infer production from declarations plus probes.
    pub fn process_unpadded(&mut self, input: &[f32], output: &mut [f32]) -> Result<usize, String> {
        if self.merge_overflow_poisoned {
            return Err(
                "host merge retention overflowed; reset the host before further processing".into(),
            );
        }
        if self.terminal_sink_lifecycle.is_some() || self.has_advertised_terminal_sink() {
            return Err("terminal sink graphs require explicit process_to_sink processing".into());
        }
        self.drain_graph_mutations()?;
        if !self.built {
            self.build()?;
        }
        self.preflight_process_horizon(input.len() / self.input_channels())?;
        if let Some((index, sample)) = input
            .iter()
            .copied()
            .enumerate()
            .find(|(_, sample)| !sample.is_finite())
        {
            output.fill(0.0);
            return Err(format!(
                "host input contains non-finite sample at index {index}: {sample}"
            ));
        }
        let mut events = std::mem::take(&mut self.queues.parameter_event_scratch);
        self.drain_parameter_events_into(&mut events);
        let result = self.process_with_parameter_events(input, output, &mut events, false);
        self.queues.parameter_event_scratch = events;
        if result.is_ok() && !input.is_empty() {
            self.drain_state = DrainState::default();
            self.graph_drain_state.reset_dynamic();
        }
        result
    }

    /// Process an input block through a prepared graph ending in an opted-in sink.
    ///
    /// The block is admitted atomically into prepared sink storage before any
    /// source plugin or queued parameter event advances. A zero-consumed result
    /// is a service-only response: retry the same input slice on a later call.
    pub fn process_to_sink(
        &mut self,
        input: &[f32],
    ) -> Result<SinkProcessResult, SinkProcessError> {
        self.ensure_sink_running()
            .map_err(SinkProcessError::Lifecycle)?;
        if !self.queues.graph_mutation_rx.is_empty() {
            return Err(SinkProcessError::RetryablePreflight(
                SinkPreflightError::PendingGraphMutation,
            ));
        }
        if !self.built {
            return Err(SinkProcessError::RetryablePreflight(
                SinkPreflightError::GraphNotBuilt,
            ));
        }
        let (sink_id, _) = self.validate_terminal_sink_graph().map_err(|_error| {
            SinkProcessError::RetryablePreflight(SinkPreflightError::Sink(
                SinkTailPreflightError::InvalidGeometry,
            ))
        })?;
        let input_channels = self.input_channels();
        if input_channels == 0 || !input.len().is_multiple_of(input_channels) {
            return Err(SinkProcessError::RetryablePreflight(
                SinkPreflightError::InvalidInput(format!(
                    "terminal sink input must contain whole frames of {input_channels} channels"
                )),
            ));
        }
        if let Some((index, sample)) = input
            .iter()
            .copied()
            .enumerate()
            .find(|(_, sample)| !sample.is_finite())
        {
            return Err(SinkProcessError::RetryablePreflight(
                SinkPreflightError::InvalidInput(format!(
                    "host input contains non-finite sample at index {index}: {sample}"
                )),
            ));
        }
        let input_frames = input.len() / input_channels;
        self.preflight_process_horizon(input_frames)
            .map_err(|error| {
                SinkProcessError::RetryablePreflight(SinkPreflightError::InvalidInput(error))
            })?;
        self.validate_terminal_sink_input_geometry(input_frames)
            .map_err(|_| {
                SinkProcessError::RetryablePreflight(SinkPreflightError::Sink(
                    SinkTailPreflightError::InvalidGeometry,
                ))
            })?;

        let sink_state = self.terminal_sink_queue_state(sink_id).map_err(|_| {
            SinkProcessError::RetryablePreflight(SinkPreflightError::Sink(
                SinkTailPreflightError::InvalidGeometry,
            ))
        })?;
        if input_frames > sink_state.capacity_frames {
            return Err(SinkProcessError::RetryablePreflight(
                SinkPreflightError::InputExceedsCapacity {
                    input_frames,
                    capacity_frames: sink_state.capacity_frames,
                },
            ));
        }

        // Empty input is an explicit bounded transport service request. It
        // never dequeues controls or advances any source/host clock.
        if input.is_empty() {
            let pending_sink_frames = if sink_state.pending_frames == 0 {
                0
            } else {
                match self.service_terminal_sink_pending(sink_id) {
                    Ok(state) => state.pending_frames,
                    Err(cause) => {
                        self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                        return Err(SinkProcessError::ResetRequired {
                            cause,
                            input: SinkInputDisposition::NotAdmitted,
                        });
                    }
                }
            };
            return Ok(SinkProcessResult {
                input_frames_consumed: 0,
                pending_sink_frames,
            });
        }

        if input_frames > sink_state.free_prepared_frames {
            let pending_sink_frames = match self.service_terminal_sink_pending(sink_id) {
                Ok(state) => state.pending_frames,
                Err(cause) => {
                    self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                    return Err(SinkProcessError::ResetRequired {
                        cause,
                        input: SinkInputDisposition::NotAdmitted,
                    });
                }
            };
            return Ok(SinkProcessResult {
                input_frames_consumed: 0,
                pending_sink_frames,
            });
        }

        let sink_position = self.node_input_positions[sink_id];
        let sink_rate = self.node_input_sample_rates[sink_id];
        let context = ProcessContext::new(f64::from(sink_rate), input_frames)
            .with_sample_position(sink_position);
        self.preflight_terminal_sink_append(sink_id, input_frames, &context)
            .map_err(SinkProcessError::RetryablePreflight)?;
        if self.terminal_sink_staging.len() < input.len() {
            return Err(SinkProcessError::RetryablePreflight(
                SinkPreflightError::Sink(SinkTailPreflightError::InvalidGeometry),
            ));
        }

        self.terminal_sink_producer_started = false;
        let mut events = std::mem::take(&mut self.queues.parameter_event_scratch);
        self.drain_parameter_events_into(&mut events);
        let mut staging = std::mem::take(&mut self.terminal_sink_staging);
        let block_start_sample = self.automation_state.playback_position as u64;
        let process_result = self.process_with_parameter_events_to_sink(
            input,
            &mut staging[..input.len()],
            &mut events,
            block_start_sample,
        );
        self.terminal_sink_staging = staging;
        self.queues.parameter_event_scratch = events;
        let processed_frames = match process_result {
            Ok(frames) => frames,
            Err(error) => {
                let (cause, disposition) = if self.terminal_sink_producer_started {
                    (
                        SinkFailure::UpstreamProcess(error),
                        SinkInputDisposition::Indeterminate,
                    )
                } else {
                    (
                        SinkFailure::ControlInvalidatedRoute(error),
                        SinkInputDisposition::NotAdmitted,
                    )
                };
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkProcessError::ResetRequired {
                    cause,
                    input: disposition,
                });
            }
        };
        if processed_frames != input_frames {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
            return Err(SinkProcessError::ResetRequired {
                cause: SinkFailure::InvalidProducedFrameCount {
                    reported: processed_frames,
                    expected: input_frames,
                },
                input: SinkInputDisposition::Indeterminate,
            });
        }
        // The successful preflight reserved this complete block before any
        // producer advanced. Do not repeat transport or geometry preflight
        // after processing: a device notification may arrive meanwhile, but it
        // cannot revoke the already admitted local block.
        let Some(terminal_sink) = self.plugins[sink_id]
            .as_mut()
            .and_then(|plugin| plugin.terminal_sink_mut())
        else {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
            return Err(SinkProcessError::ResetRequired {
                cause: SinkFailure::AppendContractViolation,
                input: SinkInputDisposition::Indeterminate,
            });
        };
        let append_result =
            terminal_sink.append_preflighted(&self.terminal_sink_staging[..input.len()], &context);
        if append_result.is_err() {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
            return Err(SinkProcessError::ResetRequired {
                cause: SinkFailure::AppendContractViolation,
                input: SinkInputDisposition::Indeterminate,
            });
        }
        self.drain_state = DrainState::default();
        self.graph_drain_state.reset_dynamic();

        let pending_sink_frames = match self.service_terminal_sink_pending(sink_id) {
            Ok(state) => state.pending_frames,
            Err(cause) => {
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkProcessError::ResetRequired {
                    cause,
                    input: SinkInputDisposition::Admitted {
                        frames: input_frames,
                    },
                });
            }
        };
        Ok(SinkProcessResult {
            input_frames_consumed: input_frames,
            pending_sink_frames,
        })
    }

    /// Settle host-owned queued graph edits while preserving any retained sink
    /// audio. This is a control-thread operation and may allocate while building.
    pub fn settle_terminal_sink_graph_mutations(
        &mut self,
    ) -> Result<SinkGraphSettlementResult, SinkGraphSettlementError> {
        if self.terminal_sink_lifecycle != Some(TerminalSinkLifecycle::Running)
            || self.queues.parameter_event_tx.is_none()
            || self.queues.graph_mutation_tx.is_none()
        {
            return Err(SinkGraphSettlementError::NotRunning);
        }
        let sink_id = self
            .chain_nodes
            .last()
            .copied()
            .filter(|id| {
                self.plugins
                    .get(*id)
                    .and_then(Option::as_ref)
                    .is_some_and(|plugin| plugin.terminal_sink().is_some())
            })
            .ok_or_else(|| {
                SinkGraphSettlementError::Graph(
                    "current graph has no prepared terminal sink".into(),
                )
            })?;
        let queue_state = self
            .terminal_sink_queue_state(sink_id)
            .map_err(SinkGraphSettlementError::Graph)?;
        if queue_state.pending_frames > 0 {
            let pending_sink_frames = self
                .service_terminal_sink_pending(sink_id)
                .map_err(|cause| {
                    self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                    SinkGraphSettlementError::ResetRequired(cause)
                })?
                .pending_frames;
            return Ok(SinkGraphSettlementResult::ServiceOnly {
                pending_sink_frames,
            });
        }

        if !self.queues.graph_mutation_rx.is_empty() {
            self.built = false;
            self.drain_graph_mutations()
                .map_err(SinkGraphSettlementError::Graph)?;
        }
        if !self.built {
            self.build().map_err(SinkGraphSettlementError::Graph)?;
        }
        self.validate_terminal_sink_graph()
            .map_err(SinkGraphSettlementError::Graph)?;
        Ok(SinkGraphSettlementResult::Settled)
    }

    /// Recover an owned sink transport without rebuilding or resetting the graph.
    ///
    /// Call this from a control thread while the prepared programme is running
    /// or draining. Recovery preserves the host queue, cached latency, source
    /// drain cursor, and lifecycle. A changed transport sample rate, channel
    /// count, or ring capacity returns `NeedsReprepare`; this API does not
    /// provide a preserving reprepare operation.
    ///
    /// # Errors
    /// Returns an error when the graph is unbuilt, has pending graph mutations,
    /// is not in a recoverable lifecycle, or the sink cannot retain its
    /// prepared geometry.
    pub fn recover_terminal_sink_transport(
        &mut self,
    ) -> Result<SinkTransportRecoveryStatus, TerminalSinkRecoveryError> {
        match self.terminal_sink_lifecycle {
            None => return Err(TerminalSinkRecoveryError::ModeDisabled),
            Some(TerminalSinkLifecycle::Running | TerminalSinkLifecycle::Draining) => {}
            Some(TerminalSinkLifecycle::Complete | TerminalSinkLifecycle::ResetRequired) => {
                return Err(TerminalSinkRecoveryError::InvalidLifecycle);
            }
        }
        if !self.built {
            return Err(TerminalSinkRecoveryError::GraphNotBuilt);
        }
        if !self.queues.graph_mutation_rx.is_empty() {
            return Err(TerminalSinkRecoveryError::PendingGraphMutation);
        }
        let (sink_id, _) = self
            .validate_terminal_sink_graph()
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let queue_before = self
            .terminal_sink_queue_state(sink_id)
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let plugin = self.plugins[sink_id].as_ref().ok_or_else(|| {
            TerminalSinkRecoveryError::InvalidRoute("terminal sink plugin is missing".into())
        })?;
        let native_sink_rate =
            Self::checked_native_sink_rate(self.node_input_sample_rates[sink_id])
                .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let expected = plugin
            .terminal_sink()
            .and_then(|sink| sink.prepared_transport_format())
            .unwrap_or_else(|| SinkTransportFormat {
                sample_rate: native_sink_rate,
                channels: self.nodes[&sink_id].input_channels(),
                buffer_frames: queue_before.capacity_frames,
            });
        if expected.sample_rate != native_sink_rate
            || expected.channels != self.nodes[&sink_id].input_channels()
            || expected.buffer_frames == 0
        {
            return Err(TerminalSinkRecoveryError::InvalidRoute(
                "terminal sink prepared format does not match the built route".into(),
            ));
        }
        let latency_before = plugin.latency_samples();
        let cached_latency_before = self.cached_latency;
        let lifecycle_before = self.terminal_sink_lifecycle;
        let source_complete_before = self.terminal_sink_source_complete;
        let drain_cursor_before = (
            self.drain_state.completed_prefix,
            self.drain_state.active_node,
            self.drain_state.prepared,
            self.drain_state.remaining_calls,
        );
        let sink_position_before = self.node_input_positions[sink_id];

        let status = self.plugins[sink_id]
            .as_mut()
            .and_then(|plugin| plugin.terminal_sink_mut())
            .ok_or_else(|| {
                TerminalSinkRecoveryError::InvalidRoute("terminal sink contract disappeared".into())
            })?
            .recover_transport(expected)
            .map_err(TerminalSinkRecoveryError::Sink)?;

        let queue_after = self
            .terminal_sink_queue_state(sink_id)
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let latency_after = self.plugins[sink_id]
            .as_ref()
            .map_or(usize::MAX, |plugin| plugin.latency_samples());
        let drain_cursor_after = (
            self.drain_state.completed_prefix,
            self.drain_state.active_node,
            self.drain_state.prepared,
            self.drain_state.remaining_calls,
        );
        if queue_after != queue_before
            || latency_after != latency_before
            || self.cached_latency != cached_latency_before
            || self.terminal_sink_lifecycle != lifecycle_before
            || self.terminal_sink_source_complete != source_complete_before
            || drain_cursor_after != drain_cursor_before
            || self.node_input_positions[sink_id] != sink_position_before
            || self.validate_terminal_sink_graph().is_err()
        {
            return Err(TerminalSinkRecoveryError::ContractViolation);
        }
        Ok(status)
    }

    /// Reprepare host and sink storage after a physical ring-size change.
    ///
    /// This control-thread operation preserves the built graph, pending sink
    /// samples, source clocks, EOF cursor, compensation-delay history, and
    /// retained per-node output. Only the physical ring size may change; the
    /// sample rate and channel count must remain the same. Every fallible host
    /// allocation is staged before the sink is asked to commit its plan.
    pub fn reprepare_terminal_sink_transport(
        &mut self,
    ) -> Result<SinkTransportRecoveryStatus, TerminalSinkRecoveryError> {
        match self.terminal_sink_lifecycle {
            None => return Err(TerminalSinkRecoveryError::ModeDisabled),
            Some(TerminalSinkLifecycle::Running | TerminalSinkLifecycle::Draining) => {}
            Some(TerminalSinkLifecycle::Complete | TerminalSinkLifecycle::ResetRequired) => {
                return Err(TerminalSinkRecoveryError::InvalidLifecycle);
            }
        }
        if !self.built {
            return Err(TerminalSinkRecoveryError::GraphNotBuilt);
        }
        if !self.queues.graph_mutation_rx.is_empty() {
            return Err(TerminalSinkRecoveryError::PendingGraphMutation);
        }
        if self.process_buffers.is_none() || self.process_buffers_f64.is_none() {
            return Err(TerminalSinkRecoveryError::HostPreparation(
                "built sink route is missing prepared process buffers".into(),
            ));
        }
        let (sink_id, _) = self
            .validate_terminal_sink_graph()
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let queue_before = self
            .terminal_sink_queue_state(sink_id)
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let plan = self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or_else(|| {
                TerminalSinkRecoveryError::InvalidRoute("terminal sink contract disappeared".into())
            })?
            .reprepare_plan()
            .map_err(TerminalSinkRecoveryError::Sink)?;
        self.validate_terminal_sink_reprepare_plan(sink_id, queue_before, plan)
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;

        let lifecycle_before = self.terminal_sink_lifecycle;
        let source_complete_before = self.terminal_sink_source_complete;
        let cached_latency_before = self.cached_latency;
        let sink_position_before = self.node_input_positions[sink_id];
        let drain_cursor_before = (
            self.drain_state.completed_prefix,
            self.drain_state.active_node,
            self.drain_state.prepared,
            self.drain_state.remaining_calls,
        );
        let old_plugin_latency = self.plugins[sink_id]
            .as_ref()
            .map_or(usize::MAX, |plugin| plugin.latency_samples());

        let mut staged = self
            .prepare_terminal_sink_host_buffers(sink_id, plan)
            .map_err(TerminalSinkRecoveryError::HostPreparation)?;

        let current_plan = self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or_else(|| {
                TerminalSinkRecoveryError::InvalidRoute("terminal sink contract disappeared".into())
            })?
            .reprepare_plan()
            .map_err(TerminalSinkRecoveryError::Sink)?;
        if current_plan != plan
            || self
                .terminal_sink_queue_state(sink_id)
                .map_err(TerminalSinkRecoveryError::InvalidRoute)?
                != queue_before
        {
            return Err(TerminalSinkRecoveryError::Sink(
                PluginSinkRecoveryError::StalePlan,
            ));
        }

        let status = self.plugins[sink_id]
            .as_mut()
            .and_then(|plugin| plugin.terminal_sink_mut())
            .ok_or_else(|| {
                TerminalSinkRecoveryError::InvalidRoute("terminal sink contract disappeared".into())
            })?
            .reprepare_transport(plan)
            .map_err(TerminalSinkRecoveryError::Sink)?;

        let queue_after = self
            .terminal_sink_queue_state(sink_id)
            .map_err(TerminalSinkRecoveryError::InvalidRoute)?;
        let plugin = self.plugins[sink_id].as_ref().ok_or_else(|| {
            TerminalSinkRecoveryError::InvalidRoute("terminal sink plugin is missing".into())
        })?;
        let plugin_latency = plugin.latency_samples();
        let prepared_format = plugin
            .terminal_sink()
            .and_then(|sink| sink.prepared_transport_format());
        let drain_cursor_after = (
            self.drain_state.completed_prefix,
            self.drain_state.active_node,
            self.drain_state.prepared,
            self.drain_state.remaining_calls,
        );
        let host_state_unchanged = self.terminal_sink_lifecycle == lifecycle_before
            && self.terminal_sink_source_complete == source_complete_before
            && self.cached_latency == cached_latency_before
            && self.node_input_positions[sink_id] == sink_position_before
            && drain_cursor_after == drain_cursor_before;

        match status {
            SinkTransportRecoveryStatus::Waiting(_) => {
                if queue_after != queue_before
                    || plugin_latency != old_plugin_latency
                    || prepared_format.is_none_or(|format| format != plan.prepared_format)
                    || !host_state_unchanged
                {
                    self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                    return Err(TerminalSinkRecoveryError::ContractViolation);
                }
                return Ok(status);
            }
            SinkTransportRecoveryStatus::Ready => {
                if queue_after.pending_frames != queue_before.pending_frames
                    || queue_after.capacity_frames != plan.queue_capacity_frames
                    || plugin_latency != plan.latency_samples
                    || prepared_format != Some(plan.target_format)
                    || !host_state_unchanged
                {
                    self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                    return Err(TerminalSinkRecoveryError::ContractViolation);
                }
            }
        }

        let Some(old_f32) = self.process_buffers.take() else {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
            return Err(TerminalSinkRecoveryError::ContractViolation);
        };
        staged.process_buffers.compensation_delays = old_f32.compensation_delays;
        staged.process_buffers.parallel_results = old_f32.parallel_results;
        let Some(old_f64) = self.process_buffers_f64.take() else {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
            return Err(TerminalSinkRecoveryError::ContractViolation);
        };
        staged.process_buffers_f64.compensation_delays = old_f64.compensation_delays;
        staged.process_buffers_f64.parallel_results = old_f64.parallel_results;
        self.process_buffers = Some(staged.process_buffers);
        self.process_buffers_f64 = Some(staged.process_buffers_f64);
        self.config.f64_input_scratch = staged.f64_input_scratch;
        self.config.f64_output_scratch = staged.f64_output_scratch;
        self.config.f64_chain_scratch = staged.f64_chain_scratch;
        self.config.f64_chain_scratch_alt = staged.f64_chain_scratch_alt;
        self.terminal_sink_staging = staged.terminal_sink_staging;
        self.cached_latency = Some(self.compute_latency());
        Ok(SinkTransportRecoveryStatus::Ready)
    }

    fn terminal_sink_queue_state(&self, sink_id: NodeId) -> Result<SinkQueueState, String> {
        let state = self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or("terminal sink contract disappeared")?
            .queue_state();
        if state.pending_frames.checked_add(state.free_prepared_frames)
            != Some(state.capacity_frames)
        {
            return Err("terminal sink queue state has invalid frame geometry".into());
        }
        Ok(state)
    }

    fn preflight_terminal_sink_append(
        &self,
        sink_id: NodeId,
        frames: usize,
        context: &ProcessContext,
    ) -> Result<(), SinkPreflightError> {
        self.terminal_sink_queue_state(sink_id)
            .map_err(|_| SinkPreflightError::Sink(SinkTailPreflightError::InvalidGeometry))?;
        self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or(SinkPreflightError::Sink(
                SinkTailPreflightError::InvalidGeometry,
            ))?
            .preflight_append(frames, context)
            .map_err(SinkPreflightError::Sink)
    }

    fn service_terminal_sink_pending(
        &mut self,
        sink_id: NodeId,
    ) -> Result<SinkQueueState, SinkFailure> {
        let sample_rate = self.node_input_sample_rates[sink_id];
        let sample_position = self.node_input_positions[sink_id];
        let context =
            ProcessContext::new(f64::from(sample_rate), 0).with_sample_position(sample_position);
        self.plugins[sink_id]
            .as_mut()
            .and_then(|plugin| plugin.terminal_sink_mut())
            .ok_or(SinkFailure::ServiceContractViolation)?
            .service_pending(&context)
            .map_err(|SinkServiceFailure::ContractViolation| SinkFailure::ServiceContractViolation)
    }

    fn validate_terminal_sink_input_geometry(&self, frames: usize) -> Result<(), String> {
        let (sink_id, _) = self.validate_terminal_sink_graph()?;
        for &node_id in &self.chain_nodes {
            let node = &self.nodes[&node_id];
            let plugin = self.plugins[node_id]
                .as_ref()
                .ok_or_else(|| format!("terminal sink route is missing plugin {node_id}"))?;
            if plugin.input_channels() != node.input_channels()
                || plugin.output_channels() != node.output_channels()
            {
                return Err(format!(
                    "terminal sink route node {node_id} changed channel geometry"
                ));
            }
            if frames > 0 && node_id != sink_id && plugin.output_frames_for_input(frames) != frames
            {
                return Err(format!(
                    "terminal sink route node {node_id} changed frame geometry"
                ));
            }
            let input_rate = self.node_input_sample_rates[node_id];
            if Self::plugin_output_sample_rate_isolated(
                plugin.as_ref(),
                node_id,
                &node.name,
                input_rate,
            )? != self.node_output_sample_rates[node_id]
                || input_rate != self.node_output_sample_rates[node_id]
            {
                return Err(format!(
                    "terminal sink route node {node_id} changed sample-rate geometry"
                ));
            }
        }
        Ok(())
    }

    /// Required conservative frame capacity for one end-of-stream drain step.
    ///
    /// Multiply by [`Self::output_channels`] to size the interleaved destination.
    /// Queued graph mutations applied by [`Self::drain`] can change this bound.
    /// Chains use the propagated per-stage maximum; built branched graphs use
    /// the cached scheduler bound refreshed from live stream-state declarations
    /// (a fresh resampler reports 0, mid-stream its block maximum), so callers
    /// must query after the final process block, exactly like chains. Once a
    /// drain session starts, the enforced per-call bound freezes for the whole
    /// session: later live declaration growth only enlarges internal
    /// reservations while emission stays paced to the frozen value, so a
    /// buffer sized from the begin query stays valid to completion.
    /// Unbuilt branched graphs report a heuristic estimate until [`Self::build`]
    /// runs (`drain` refreshes before enforcing capacity, so the enforced
    /// bound is exact).
    pub fn drain_output_frames_max(&self) -> usize {
        if self.is_chain_topology_for_drain() {
            return self.chain_drain_output_frames_max();
        }
        if self.built {
            if let Some(bound) = self.graph_drain_state.session_output_bound {
                return bound;
            }
            let cached = self.graph_drain_plan.bound_frames;
            if !self.graph_node_declarations_changed() {
                return cached;
            }
            return cached.max(
                self.derive_graph_drain_plan()
                    .map_or(0, |plan| plan.bound_frames),
            );
        }
        self.unbuilt_graph_drain_estimate()
    }

    /// Test-only live (unfrozen) graph drain bound: what the bound query
    /// would answer from current declarations without the session freeze.
    /// `None` for chains and unbuilt graphs. Lets the session-freeze test
    /// prove it actually exercises mid-drain growth instead of passing
    /// vacuously on a constant derivation.
    #[cfg(test)]
    pub(super) fn graph_drain_live_bound_for_test(&self) -> Option<usize> {
        if self.is_chain_topology_for_drain() || !self.built {
            return None;
        }
        Some(
            self.graph_drain_plan.bound_frames.max(
                self.derive_graph_drain_plan()
                    .map_or(0, |plan| plan.bound_frames),
            ),
        )
    }

    /// Test-only rebuild-cached envelope bound: `Some` when the stored
    /// graph plan was derived from envelopes (chains answer their live
    /// propagation instead). Lets envelope tests prove they exercise the
    /// prepared path instead of passing vacuously on legacy sizing.
    #[cfg(test)]
    pub(super) fn graph_drain_envelope_bound_for_test(&self) -> Option<usize> {
        if self.is_chain_topology_for_drain() {
            return self.chain_drain_envelope_bound();
        }
        if !self.built {
            return None;
        }
        self.graph_drain_plan.envelope_bound
    }

    /// Test-only count of drain-entry plan refreshes since the last
    /// dynamic reset. Lets the session-freeze test prove drift refreshes
    /// actually ran instead of passing on a quiet derivation.
    #[cfg(test)]
    pub(super) fn graph_drain_refresh_count_for_test(&self) -> u64 {
        self.graph_drain_state.drain_refresh_count
    }

    fn chain_drain_output_frames_max(&self) -> usize {
        let mut maximum = 0usize;
        for (index, &node_id) in self.chain_nodes.iter().enumerate() {
            if self.nodes[&node_id].bypassed {
                continue;
            }
            let Some(plugin) = self.plugins[node_id].as_ref() else {
                continue;
            };
            let mut frames = plugin.drain_output_frames_max();
            for &downstream_id in &self.chain_nodes[index + 1..] {
                if self.nodes[&downstream_id].bypassed {
                    continue;
                }
                let downstream = self.plugins[downstream_id].as_ref().unwrap();
                frames = downstream.output_frames_for_input(frames);
            }
            maximum = maximum.max(frames);
        }
        maximum
    }

    /// Stream-independent chain EOS bound, mirroring
    /// [`Self::chain_drain_output_frames_max`] with envelope queries. Any
    /// unknown envelope or structural anomaly answers `None` so callers
    /// keep live sizing. Allocation-free.
    fn chain_drain_envelope_bound(&self) -> Option<usize> {
        let mut maximum = 0usize;
        for (index, &node_id) in self.chain_nodes.iter().enumerate() {
            let Some(node) = self.nodes.get(&node_id) else {
                continue;
            };
            if node.bypassed {
                continue;
            }
            let Some(plugin) = self.plugins.get(node_id).and_then(Option::as_ref) else {
                continue;
            };
            let mut frames = plugin.drain_frames_envelope()?;
            for &downstream_id in &self.chain_nodes[index + 1..] {
                let downstream_node = self.nodes.get(&downstream_id)?;
                if downstream_node.bypassed {
                    continue;
                }
                let downstream = self.plugins.get(downstream_id).and_then(Option::as_ref)?;
                frames = Self::plugin_output_frames_envelope_isolated(
                    downstream.as_ref(),
                    downstream_id,
                    &downstream_node.name,
                    frames,
                )?;
            }
            maximum = maximum.max(frames);
        }
        Some(maximum)
    }

    /// Largest intermediate chain wave over every envelope stage pair, for
    /// build-time scratch preparation. The final bound alone does not
    /// dominate intermediate extents (an upsampler's emission can exceed
    /// the downsampled sink bound), so preparation tracks every prefix of
    /// every propagation. `None` keeps today's sizing. Allocation-free.
    fn chain_envelope_prep_frames(&self) -> Option<usize> {
        let mut maximum = 0usize;
        for (index, &node_id) in self.chain_nodes.iter().enumerate() {
            let Some(node) = self.nodes.get(&node_id) else {
                continue;
            };
            if node.bypassed {
                continue;
            }
            let Some(plugin) = self.plugins.get(node_id).and_then(Option::as_ref) else {
                continue;
            };
            let mut wave = plugin.drain_frames_envelope()?;
            maximum = maximum.max(wave);
            for &downstream_id in &self.chain_nodes[index + 1..] {
                let downstream_node = self.nodes.get(&downstream_id)?;
                if downstream_node.bypassed {
                    continue;
                }
                let downstream = self.plugins.get(downstream_id).and_then(Option::as_ref)?;
                wave = Self::plugin_output_frames_envelope_isolated(
                    downstream.as_ref(),
                    downstream_id,
                    &downstream_node.name,
                    wave,
                )?;
                maximum = maximum.max(wave);
            }
        }
        Some(maximum)
    }

    /// Stream-independent per-call EOS bound for nesting hosts. Chains
    /// propagate envelopes live (allocation-free, bypass-aware); built
    /// graphs answer the rebuild-cached envelope bound; anything else
    /// (unbuilt graphs, legacy plans) answers `None` so the outer host
    /// keeps live sizing. Never allocates.
    pub fn drain_frames_envelope(&self) -> Option<usize> {
        if self.is_chain_topology_for_drain() {
            return self.chain_drain_envelope_bound();
        }
        if self.built {
            return self.graph_drain_plan.envelope_bound;
        }
        None
    }

    /// Conservative EOS bound for a graph that has not been built yet.
    ///
    /// Every path expansion is applied to the summed per-node allowances, so
    /// under the same monotonic `output_frames_for_input` assumption the
    /// chain bound already makes, this dominates the exact built bound.
    /// [`Self::drain`] always builds before enforcing capacity.
    fn unbuilt_graph_drain_estimate(&self) -> usize {
        let mut total = 0usize;
        for (&id, node) in &self.nodes {
            if node.bypassed {
                continue;
            }
            let Some(plugin) = self.plugins[id].as_ref() else {
                continue;
            };
            total = total
                .saturating_add(plugin.drain_output_frames_max())
                .saturating_add(plugin.latency_samples());
        }
        for (&id, node) in &self.nodes {
            if node.bypassed {
                continue;
            }
            let Some(plugin) = self.plugins[id].as_ref() else {
                continue;
            };
            total = total.max(Self::plugin_output_frames_for_input_isolated(
                plugin.as_ref(),
                id,
                &node.name,
                total,
            ));
        }
        total
    }

    /// Structural chain check for EOS routing, mirroring what the chain
    /// drain validators admit: serial links must be plain audio edges
    /// (no sidechain, map, or offset). Admitted chains keep the existing
    /// linear and channel-changing drain paths; anything else uses the
    /// branched graph scheduler, which routes key edges, maps, and
    /// offsets. This deliberately ignores build state so the capacity
    /// query matches before and after [`Self::build`].
    fn is_chain_topology_for_drain(&self) -> bool {
        if self.chain_nodes.is_empty()
            || self.chain_nodes.len() != self.nodes.len()
            || self.edges.len() != self.chain_nodes.len().saturating_sub(1)
        {
            return false;
        }
        for pair in self.chain_nodes.windows(2) {
            let linked = self.edges.iter().any(|edge| {
                edge.from_node == pair[0]
                    && edge.to_node == pair[1]
                    && edge.edge_type == EdgeType::Audio
                    && edge.channel_map.is_none()
                    && edge.destination_offset == 0
            });
            if !linked {
                return false;
            }
        }
        true
    }

    /// Drain plugin tails in causal order without allocating.
    ///
    /// Output from an upstream tail is processed through all downstream nodes
    /// before the downstream node's own tail is drained. This ordering is what
    /// prevents a resampler followed by a limiter/convolver from losing either
    /// plugin's final state. Linear chains keep their historical single-cursor
    /// walk; branched graphs run one bounded scheduler round per call, with
    /// per-edge EOF, timestamp-aligned joins, and latency-compensation
    /// flushing, until every node is complete.
    ///
    /// For a nonempty graph, `output` must contain whole output-channel frames
    /// and hold at least [`Self::drain_output_frames_max`] frames, even when the
    /// actual result is shorter. Capacity is checked after queued graph changes
    /// and parameter events are applied and before plugin EOS preparation,
    /// drain, or downstream audio processing. EOF has no new source frames, so
    /// queued event offsets apply at the current boundary. An empty or already
    /// completed graph completes without requiring output storage.
    ///
    /// Each active plugin gets its prepared `drain_call_bound`, or 4096 calls
    /// when unknown. Successful zero-output calls count too. Completed stages
    /// are never revisited until reset, new accepted input, or graph mutation.
    /// An accepted active-stage control refreshes only that stage's quota.
    /// Quota exhaustion trips loudly — except once per stage for deferred
    /// arming: a plugin whose tail turned `Finite` since the grant earns one
    /// re-queried budget (see `Plugin::drain_call_bound`).
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid graph, insufficient or misaligned output
    /// storage, invalid output geometry, exhausted native work quota, or a plugin
    /// processing failure. Capacity errors leave audio state unconsumed; queued
    /// graph and parameter changes may already have been applied. Plugin errors need not leave audio state unchanged.
    pub fn drain(&mut self, output: &mut [f32]) -> Result<PluginDrainResult, String> {
        if self.merge_overflow_poisoned {
            return Err(
                "host merge retention overflowed; reset the host before further processing or drain".into(),
            );
        }
        if self.terminal_sink_lifecycle.is_some() || self.has_advertised_terminal_sink() {
            return Err("terminal sink graphs require explicit drain_to_sink handling".into());
        }
        self.drain_graph_mutations()?;
        if !self.built {
            self.build()?;
        }
        let drain_horizon = self
            .chain_envelope_prep_frames()
            .unwrap_or_else(|| self.drain_output_frames_max());
        self.preflight_process_horizon(drain_horizon)?;
        // No source frames arrive during EOS: queued offsets apply at this
        // boundary, and each event is consumed once even if capacity is rejected.
        while let Ok(event) = self.queues.parameter_event_rx.pop() {
            let _ = self.apply_parameter_event(event);
        }
        if self.nodes.is_empty() {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if !self.is_topologically_linear_chain() && !self.is_chain_topology_for_drain() {
            return self.drain_graph(output);
        }
        let channel_changing_drain = !self.is_topologically_linear_chain();
        if self.drain_state.completed_prefix == self.chain_nodes.len() {
            return Ok(PluginDrainResult::COMPLETE);
        }

        let output_channels = self.output_channels();
        if output_channels == 0 || !output.len().is_multiple_of(output_channels) {
            return Err(format!(
                "Host drain output must contain whole frames of {output_channels} channels"
            ));
        }
        let required_samples = self
            .drain_output_frames_max()
            .checked_mul(output_channels)
            .ok_or("Host drain output capacity overflow")?;
        if output.len() < required_samples {
            return Err(format!(
                "Host drain output too small: need {required_samples} samples, got {}",
                output.len()
            ));
        }
        if channel_changing_drain {
            self.validate_channel_changing_drain_chain()?;
        }
        if channel_changing_drain {
            self.validate_channel_changing_drain_scratch()?;
        }

        let mut guard = BufferGuard::take(&mut self.process_buffers);
        let bufs = guard.get_mut();
        let mut input_rate = self.config.sample_rate;

        for chain_index in 0..self.chain_nodes.len() {
            let node_id = self.chain_nodes[chain_index];
            let node = &self.nodes[&node_id];
            if node.bypassed {
                if chain_index >= self.drain_state.completed_prefix {
                    self.drain_state.completed_prefix = chain_index + 1;
                }
                continue;
            }
            if chain_index < self.drain_state.completed_prefix {
                input_rate = Self::plugin_output_sample_rate_isolated(
                    self.plugins[node_id].as_ref().unwrap().as_ref(),
                    node_id,
                    &node.name,
                    input_rate,
                )?;
                continue;
            }
            let drain_capacity = self.plugins[node_id]
                .as_ref()
                .unwrap()
                .drain_output_frames_max();
            let drain_samples = drain_capacity
                .checked_mul(node.output_channels())
                .ok_or("Host native drain capacity overflow")?;
            if input_rate == 0.0 || node.input_channels() == 0 || node.output_channels() == 0 {
                return Err("Host native drain requires nonzero rate and channels".into());
            }
            ensure_len(&mut bufs.scratch_output, drain_samples);
            let drain_context = ProcessContext::new(f64::from(input_rate), 0)
                .with_sample_position(self.node_input_positions[node_id]);
            if self.drain_state.active_node != Some(node_id) {
                self.drain_state.active_node = Some(node_id);
                self.drain_state.prepared = false;
                self.drain_state.remaining_calls = None;
                self.drain_state.quota_grant_tail = None;
            }
            if !self.drain_state.prepared {
                self.plugins[node_id]
                    .as_mut()
                    .unwrap()
                    .begin_drain(&drain_context)?;
                self.drain_state.prepared = true;
            }
            if self.drain_state.remaining_calls.is_none() {
                let plugin = self.plugins[node_id].as_ref().unwrap();
                let bound = plugin
                    .drain_call_bound()
                    .map_or(UNKNOWN_DRAIN_CALL_LIMIT, std::num::NonZeroU64::get);
                self.drain_state.quota_grant_tail = Some(plugin.tail_length());
                self.drain_state.remaining_calls = Some(bound);
            }
            let mut remaining = self.drain_state.remaining_calls.unwrap_or(0);
            if remaining == 0 {
                let plugin = self.plugins[node_id].as_ref().unwrap();
                let grant_tail = self
                    .drain_state
                    .quota_grant_tail
                    .unwrap_or(TailLength::Unknown);
                if let Some((budget, current)) = drain_quota_refresh(plugin.as_ref(), grant_tail) {
                    self.drain_state.quota_grant_tail = Some(current);
                    self.drain_state.remaining_calls = Some(budget);
                    remaining = budget;
                } else {
                    return Err(format!(
                        "plugin '{}' drain did not converge within its call bound",
                        node.name
                    ));
                }
            }
            let result = self.plugins[node_id]
                .as_mut()
                .unwrap()
                .drain(&mut bufs.scratch_output[..drain_samples], &drain_context)?;
            self.drain_state.remaining_calls = Some(remaining - 1);
            if result.frames > drain_capacity {
                return Err("Plugin drain exceeded its declared output capacity".into());
            }

            if result.frames > 0 {
                let mut current_frames = result.frames;
                let mut current_channels = node.output_channels();
                let mut current_rate = Self::plugin_output_sample_rate_isolated(
                    self.plugins[node_id].as_ref().unwrap().as_ref(),
                    node_id,
                    &node.name,
                    input_rate,
                )?;

                for &downstream_id in &self.chain_nodes[chain_index + 1..] {
                    if self.nodes[&downstream_id].bypassed {
                        self.node_input_positions[downstream_id] = Self::advance_sample_position(
                            self.node_input_positions[downstream_id],
                            current_frames,
                        )?;
                        continue;
                    }
                    let samples = current_frames.saturating_mul(current_channels);
                    ensure_len(&mut bufs.scratch_input, samples);
                    bufs.scratch_input[..samples].copy_from_slice(&bufs.scratch_output[..samples]);

                    let downstream_node = &self.nodes[&downstream_id];
                    let downstream = self.plugins[downstream_id].as_mut().unwrap();
                    let capacity = downstream.output_frames_for_input(current_frames);
                    let output_samples = capacity.saturating_mul(downstream_node.output_channels());
                    ensure_len(&mut bufs.scratch_output, output_samples);
                    let context = ProcessContext::new(f64::from(current_rate), current_frames)
                        .with_sample_position(self.node_input_positions[downstream_id]);
                    current_frames = downstream.process(
                        &bufs.scratch_input[..samples],
                        &mut bufs.scratch_output[..output_samples],
                        &context,
                    )?;
                    self.node_input_positions[downstream_id] = Self::advance_sample_position(
                        self.node_input_positions[downstream_id],
                        context.num_frames,
                    )?;
                    current_channels = downstream_node.output_channels();
                    current_rate = Self::plugin_output_sample_rate_isolated(
                        downstream.as_ref(),
                        downstream_id,
                        &downstream_node.name,
                        current_rate,
                    )?;
                }

                let samples = current_frames.saturating_mul(current_channels);
                if output.len() < samples {
                    return Err(format!(
                        "Host drain output too small: need {samples} samples, got {}",
                        output.len()
                    ));
                }
                output[..samples].copy_from_slice(&bufs.scratch_output[..samples]);
                if result.complete {
                    self.drain_state.completed_prefix = chain_index + 1;
                    self.drain_state.active_node = None;
                }
                let is_last = self.chain_nodes[chain_index + 1..]
                    .iter()
                    .all(|id| self.nodes[id].bypassed);
                if is_last && result.complete {
                    self.drain_state.completed_prefix = self.chain_nodes.len();
                }
                return Ok(PluginDrainResult {
                    frames: current_frames,
                    complete: is_last && result.complete,
                });
            }
            if !result.complete {
                return Ok(result);
            }
            self.drain_state.completed_prefix = chain_index + 1;
            self.drain_state.active_node = None;
            input_rate = Self::plugin_output_sample_rate_isolated(
                self.plugins[node_id].as_ref().unwrap().as_ref(),
                node_id,
                &node.name,
                input_rate,
            )?;
        }

        Ok(PluginDrainResult::COMPLETE)
    }

    /// Copy delay-ring history out of built compensation delays.
    ///
    /// Build path only: the clone allocates.
    fn snapshot_compensation<T: AudioSample>(
        delays: &CompensationDelays<T>,
    ) -> CompDelaySnapshot<T> {
        fn snap<T: AudioSample>(slot: &Option<DelayBuffer<T>>) -> Option<(Vec<T>, usize, usize)> {
            slot.as_ref()
                .map(|delay| (delay.buffer.clone(), delay.pos, delay.channels))
        }
        CompDelaySnapshot {
            edges: delays.delays.iter().map(snap).collect(),
            outputs: delays.output_delays.iter().map(snap).collect(),
        }
    }

    /// Restore snapshotted history into matching fresh delays.
    ///
    /// A slot is restored only when delay length and channel count
    /// still match exactly (same ring size); anything else keeps its
    /// fresh zeroed history. Copies within equal lengths: no growth.
    fn restore_compensation<T: AudioSample>(
        delays: &mut CompensationDelays<T>,
        snapshot: CompDelaySnapshot<T>,
    ) {
        fn restore<T: AudioSample>(
            slots: &mut [Option<DelayBuffer<T>>],
            olds: Vec<Option<(Vec<T>, usize, usize)>>,
        ) {
            for (slot, old) in slots.iter_mut().zip(olds) {
                let (Some(delay), Some((buffer, pos, channels))) = (slot.as_mut(), old) else {
                    continue;
                };
                if delay.channels == channels
                    && delay.channels > 0
                    && delay.buffer.len() == buffer.len()
                    && pos < delay.delay
                {
                    delay.buffer.copy_from_slice(&buffer);
                    delay.pos = pos;
                }
            }
        }
        restore(&mut delays.delays, snapshot.edges);
        restore(&mut delays.output_delays, snapshot.outputs);
    }

    fn graph_drain_signature(&self) -> GraphDrainSignature {
        let mut nodes: Vec<NodeId> = self.nodes.keys().copied().collect();
        nodes.sort_unstable();
        GraphDrainSignature {
            nodes,
            edges: self
                .edges
                .iter()
                .map(|edge| (edge.from_node, edge.to_node, edge.id, edge.edge_type))
                .collect(),
            channel_routes: self
                .edges
                .iter()
                .map(|edge| (edge.channel_map.clone(), edge.destination_offset))
                .collect(),
        }
    }

    /// Derive the graph EOS plan values from live plugin declarations.
    ///
    /// Read-only: topology order, routed widths, compensation lengths, and
    /// the per-node quantum/emission recurrence are recomputed from current
    /// `drain_output_frames_max` / `output_frames_for_input` answers without
    /// touching dynamic drain state. [`Self::rebuild_graph_drain_plan`]
    /// merges the result with the stored plan; the bound query reads only
    /// `bound_frames` from it. Quota queries stay out: the plugin trait
    /// orders `drain_call_bound` after a successful `begin_drain`, itself
    /// after output-capacity validation.
    /// Structural inputs for one EOS derivation, shared by the live and
    /// envelope derivations. Control-thread only: this allocates.
    fn graph_drain_topology(&self) -> Result<GraphDrainTopology, String> {
        for (index, edge) in self.edges.iter().enumerate() {
            if edge.id != index {
                return Err(format!(
                    "graph drain plan requires edge ids to match edge order (edge {index} has id {})",
                    edge.id
                ));
            }
        }
        let signature = self.graph_drain_signature();
        let topo_order: Vec<NodeId> = self
            .stages
            .iter()
            .flat_map(|stage| stage.nodes.iter().copied())
            .collect();

        // Routed widths in compensation order: stages, nodes, predecessors.
        // Routed widths must match `compute_compensation_delays` exactly,
        // including the sidechain running-offset sequence, so queues hold
        // exactly the frames their delay line was built for. The shared
        // helper also sizes process-phase retention, keeping both identical.
        let routed = Self::edge_routed_channels(&self.nodes, &self.edges, &self.predecessors);

        let buffers = self
            .process_buffers
            .as_ref()
            .ok_or("graph drain plan requires prepared process buffers")?;
        let mut comp_frames = vec![0usize; self.edges.len()];
        for (index, delay) in buffers.compensation_delays.delays.iter().enumerate() {
            let Some(delay) = delay else { continue };
            if index >= routed.len() || delay.channels != routed[index] {
                return Err(format!(
                    "graph drain plan found a compensation delay that does not match edge {index}"
                ));
            }
            comp_frames[index] = delay.delay;
        }
        let output_comp_frames: Vec<usize> = buffers
            .compensation_delays
            .output_delays
            .iter()
            .map(|delay| delay.as_ref().map_or(0, |delay| delay.delay))
            .collect();
        if output_comp_frames.len() != self.output_nodes.len() {
            return Err(
                "graph drain plan found output delays that do not match output nodes".into(),
            );
        }
        Ok(GraphDrainTopology {
            signature,
            topo_order,
            routed,
            comp_frames,
            output_comp_frames,
        })
    }

    fn derive_graph_drain_plan(&self) -> Result<GraphDrainPlan, String> {
        let topo = self.graph_drain_topology()?;
        let num_slots = self.predecessors.len();
        // Size the tail-fold scratch off the audio path: derivation runs at
        // build (and on rare declaration-refresh derivations, which already
        // allocate), so steady-state tail queries never grow the scratch.
        // `resize` only allocates when the topology grew.
        self.tail_fold_scratch.borrow_mut().resize(num_slots, None);

        // Per-node quanta and single-emission bounds in topological order.
        let mut node_plan = vec![GraphNodeDrainPlan::default(); num_slots];
        for &node_id in &topo.topo_order {
            let node = &self.nodes[&node_id];
            let plugin = self.plugins[node_id]
                .as_ref()
                .ok_or_else(|| format!("graph drain plan is missing plugin {node_id}"))?;
            let drain_max = if node.bypassed {
                0
            } else {
                plugin.drain_output_frames_max()
            };
            let mut quantum = drain_max.max(1);
            for (pred_pos, edge) in self.predecessors[node_id].iter().enumerate() {
                let edge_index = self.predecessor_edge_indices[node_id][pred_pos];
                if topo.routed[edge_index] == 0 {
                    continue;
                }
                quantum = quantum.max(
                    node_plan[edge.from_node]
                        .single_emission
                        .saturating_add(topo.comp_frames[edge_index]),
                );
            }
            // Single query bound once: emission and the drift snapshot
            // must observe the same plugin state.
            let offi = if node.bypassed {
                0
            } else {
                Self::plugin_output_frames_for_input_isolated(
                    plugin.as_ref(),
                    node_id,
                    &node.name,
                    quantum,
                )
            };
            let emission = if node.bypassed {
                quantum
            } else {
                offi.max(drain_max)
            };
            // No lifetime total is computed here. The plugin trait
            // orders quota queries after a successful begin_drain,
            // itself after output-capacity validation, so build-time
            // planning must not query drain_call_bound (and must not
            // begin a drain either). All queues are wave-bounded from
            // pre-begin-legal metadata (frame geometry, drain chunk
            // size, latency); backpressure paces producers, so tails
            // of any finite declared length drain without
            // lifetime-sized reservations.
            node_plan[node_id] = GraphNodeDrainPlan {
                process_quantum: quantum,
                single_emission: emission,
                bypassed: node.bypassed,
                envelope: None,
                drain_max_snapshot: drain_max,
                offi_snapshot: offi,
            };
        }

        self.finish_graph_drain_plan(topo, node_plan)
    }

    /// Assemble one EOS plan from per-node emissions, shared by the live
    /// and envelope derivations. The bound, edge, and output formulas are
    /// identical; only the node emissions differ. The envelope derivation
    /// records its bound after assembly. Control-thread only: allocates.
    fn finish_graph_drain_plan(
        &self,
        topo: GraphDrainTopology,
        node_plan: Vec<GraphNodeDrainPlan>,
    ) -> Result<GraphDrainPlan, String> {
        let GraphDrainTopology {
            signature,
            topo_order,
            routed,
            comp_frames,
            output_comp_frames,
        } = topo;
        let mut bound_frames = 0usize;
        for (pos, &node_id) in self.output_nodes.iter().enumerate() {
            bound_frames = bound_frames.max(
                node_plan[node_id]
                    .single_emission
                    .saturating_add(output_comp_frames[pos]),
            );
        }
        // Every output owns a pending FIFO sized for one wave plus
        // its output-delay flush. A fast concurrent output that fills
        // its FIFO applies backpressure (it retains its wave and
        // produces nothing new) until the round-end emission drains
        // room; pacing never depends on total tail duration.
        let output_queue_cap_frames: Vec<usize> = self
            .output_nodes
            .iter()
            .enumerate()
            .map(|(pos, &node_id)| {
                node_plan[node_id]
                    .single_emission
                    .saturating_add(output_comp_frames[pos])
            })
            .collect();

        let process_caps: &[usize] = self
            .process_buffers
            .as_ref()
            .map(|buffers| buffers.merge_queue_caps.as_slice())
            .unwrap_or(&[]);
        let mut edge_plan = vec![GraphEdgeDrainPlan::default(); self.edges.len()];
        for (index, edge) in self.edges.iter().enumerate() {
            // The EOS queue also receives process-phase retention ahead of
            // native tails, so its cap covers the drain wave plus retention.
            let retention = process_caps.get(index).copied().unwrap_or(0);
            let queue_cap_frames = node_plan[edge.from_node]
                .single_emission
                .saturating_add(comp_frames[index])
                .checked_add(retention)
                .ok_or_else(|| format!("graph drain queue cap overflows on edge {index}"))?;
            edge_plan[index] = GraphEdgeDrainPlan {
                routed_channels: routed[index],
                queue_cap_frames,
                comp_frames: comp_frames[index],
            };
        }

        Ok(GraphDrainPlan {
            topo_order,
            node_plan,
            edge_plan,
            output_comp_frames,
            output_queue_cap_frames,
            bound_frames,
            envelope_bound: None,
            signature,
        })
    }

    /// Stream-independent EOS plan when every node publishes envelopes.
    ///
    /// Mirrors [`Self::derive_graph_drain_plan`] with envelope queries:
    /// quanta propagate envelope emissions plus compensation, and each
    /// emission is the envelope pair's maximum. Dominance over every live
    /// derivation follows by induction over topological order: drain
    /// envelopes cover every stream state by contract, and process
    /// envelopes are non-decreasing, so an envelope queried at the
    /// (larger) envelope quantum dominates live production at the (smaller
    /// or equal) live quantum without assuming live monotonicity.
    /// Per-call destinations additionally size from live declarations
    /// against these envelope holdovers, which is sound because the
    /// envelope contract requires the envelope to dominate the live
    /// declaration pointwise in every state (a loose-live publisher fails
    /// loudly at the holdover check instead of corrupting silently).
    /// Bypassed nodes record no query, exactly like the live derivation.
    /// Any unknown envelope, missing node, or structural anomaly answers
    /// `None` (the live derivation runs first and keeps its loud errors),
    /// so the host keeps freeze-plus-refresh sizing for that graph.
    /// Control-thread only: this allocates.
    fn derive_graph_envelope_plan(&self) -> Option<GraphDrainPlan> {
        let topo = self.graph_drain_topology().ok()?;
        let num_slots = self.predecessors.len();
        let mut node_plan = vec![GraphNodeDrainPlan::default(); num_slots];
        for &node_id in &topo.topo_order {
            let node = self.nodes.get(&node_id)?;
            let plugin = self.plugins.get(node_id)?.as_ref()?;
            let drain_env = if node.bypassed {
                0
            } else {
                plugin.drain_frames_envelope()?
            };
            let mut quantum = drain_env.max(1);
            let preds = self.predecessors.get(node_id)?;
            let pred_edges = self.predecessor_edge_indices.get(node_id)?;
            for (pred_pos, edge) in preds.iter().enumerate() {
                let edge_index = *pred_edges.get(pred_pos)?;
                if topo.routed.get(edge_index).copied().unwrap_or(0) == 0 {
                    continue;
                }
                let pred_emission = node_plan.get(edge.from_node)?.single_emission;
                quantum = quantum
                    .max(pred_emission.saturating_add(topo.comp_frames.get(edge_index).copied()?));
            }
            let (emission, envelope) = if node.bypassed {
                (quantum, None)
            } else {
                let offi = Self::plugin_output_frames_envelope_isolated(
                    plugin.as_ref(),
                    node_id,
                    &node.name,
                    quantum,
                )?;
                let snapshot = GraphNodeEnvelope {
                    quantum,
                    offi,
                    drain: drain_env,
                };
                (offi.max(drain_env), Some(snapshot))
            };
            // Live snapshots stay honest ("when this entry was derived")
            // even though the guards ignore them on envelope plans.
            let drain_max = if node.bypassed {
                0
            } else {
                plugin.drain_output_frames_max()
            };
            let offi_snapshot = if node.bypassed {
                0
            } else {
                Self::plugin_output_frames_for_input_isolated(
                    plugin.as_ref(),
                    node_id,
                    &node.name,
                    quantum,
                )
            };
            *node_plan.get_mut(node_id)? = GraphNodeDrainPlan {
                process_quantum: quantum,
                single_emission: emission,
                bypassed: node.bypassed,
                envelope,
                drain_max_snapshot: drain_max,
                offi_snapshot,
            };
        }
        let mut plan = self.finish_graph_drain_plan(topo, node_plan).ok()?;
        plan.envelope_bound = Some(plan.bound_frames);
        Some(plan)
    }

    /// True when live node declarations outgrow the plan snapshot.
    ///
    /// Compares the drain bound (any change) and `output_frames_for_input`
    /// at the recorded quantum (growth only): residual carry drifts the
    /// latter mid-drain even while the drain bound holds still, and a new
    /// peak means a materialized wave could exceed its holdover, so the
    /// plan must re-derive and grow. Shrinkage never fires: reservations
    /// only grow, so the recorded peak still covers current waves, and
    /// oscillating carry (any chunked fixture) must not churn refreshes
    /// on every round — the no-alloc drain paths depend on that quiet.
    /// Allocation-free (declaration queries plus integer compares only),
    /// so the drain entry and the bound query gate the allocating
    /// re-derivation on this: graphs of state-independent plugins never
    /// pay for the resampler case. Mirrors the derivation's bypass rule
    /// exactly (a bypassed node records zeros and derives no query).
    /// Structural anomalies (a missing plugin or node, a short plan)
    /// report false so the existing validators keep their exact loud
    /// errors instead of a re-derivation error.
    ///
    /// On envelope plans (every node published envelopes) this degrades
    /// to guard checks only: live declarations legitimately drift under
    /// envelope-sized reservations, so only a bypass toggle or a changed
    /// envelope answer (a plugin contract violation) re-derives. Both
    /// guards are allocation-free.
    fn graph_node_declarations_changed(&self) -> bool {
        if self.graph_drain_plan.envelope_bound.is_some() {
            return self.graph_envelope_guards_tripped();
        }
        for &node_id in &self.graph_drain_plan.topo_order {
            let Some(node) = self.nodes.get(&node_id) else {
                return false;
            };
            let Some(recorded) = self.graph_drain_plan.node_plan.get(node_id) else {
                return false;
            };
            let Some(plugin) = self.plugins.get(node_id).and_then(Option::as_ref) else {
                return false;
            };
            let live_drain = if node.bypassed {
                0
            } else {
                plugin.drain_output_frames_max()
            };
            if live_drain != recorded.drain_max_snapshot {
                return true;
            }
            if !node.bypassed {
                let live_offi = Self::plugin_output_frames_for_input_isolated(
                    plugin.as_ref(),
                    node_id,
                    &node.name,
                    recorded.process_quantum,
                );
                if live_offi > recorded.offi_snapshot {
                    return true;
                }
            }
        }
        false
    }

    /// Guard checks for envelope plans: bypass toggles and envelope
    /// mutation only. Live declaration drift is expected and covered, so
    /// it never fires here. Missing nodes, plugins, or snapshots report
    /// false so downstream validators keep their exact loud errors.
    /// Allocation-free: flag compares plus envelope queries only.
    fn graph_envelope_guards_tripped(&self) -> bool {
        for &node_id in &self.graph_drain_plan.topo_order {
            let Some(node) = self.nodes.get(&node_id) else {
                return false;
            };
            let Some(recorded) = self.graph_drain_plan.node_plan.get(node_id) else {
                return false;
            };
            if node.bypassed != recorded.bypassed {
                return true;
            }
            if node.bypassed {
                continue;
            }
            let Some(snapshot) = recorded.envelope else {
                // A non-bypassed node without a snapshot never belonged
                // to an envelope derivation: re-derive to be safe.
                return true;
            };
            let Some(plugin) = self.plugins.get(node_id).and_then(Option::as_ref) else {
                return false;
            };
            if plugin.drain_frames_envelope() != Some(snapshot.drain) {
                return true;
            }
            let live_offi = Self::plugin_output_frames_envelope_isolated(
                plugin.as_ref(),
                node_id,
                &node.name,
                snapshot.quantum,
            );
            if live_offi != Some(snapshot.offi) {
                return true;
            }
        }
        false
    }

    /// Build the static graph EOS plan and its preallocated state.
    ///
    /// Control-thread only: this may allocate. Dynamic progress (phases,
    /// quotas, queued waves, holdovers) is preserved when the topology
    /// signature matches the previous build, because the linear path
    /// preserves its cursor across no-change rebuilds too. Reservations
    /// never shrink below retained samples.
    ///
    /// Drain-time refresh re-derives from live stream-state declarations and
    /// merges monotonically: node scheduling follows the fresh derivation
    /// exactly, while edge, output, and per-call capacities only grow, so
    /// retained waves, queued audio, and already-sized caller buffers stay
    /// covered when declarations rise mid-stream.
    fn rebuild_graph_drain_plan(&mut self) -> Result<(), String> {
        let derived = self.derive_graph_drain_plan()?;
        // Prefer the envelope derivation when every node publishes
        // envelopes: scheduling and reservations size from
        // stream-independent bounds, so no mid-drain growth is possible.
        // The live derivation above already reported any structural
        // anomaly loudly, so `None` here safely keeps legacy sizing.
        let derived = self.derive_graph_envelope_plan().unwrap_or(derived);
        let preserve = derived.signature == self.graph_drain_plan.signature;
        let topo_order = derived.topo_order;
        let node_plan = derived.node_plan;
        let mut edge_plan = derived.edge_plan;
        let output_comp_frames = derived.output_comp_frames;
        let mut output_queue_cap_frames = derived.output_queue_cap_frames;
        let mut bound_frames = derived.bound_frames;
        let signature = derived.signature;
        // The envelope flag follows the fresh derivation, never the merge:
        // capacities max with retained content, but scheduling, snapshots,
        // and the bound query all follow the new derivation exactly.
        let envelope_bound = derived.envelope_bound;
        if preserve {
            let old = &self.graph_drain_plan;
            for (edge, old_edge) in edge_plan.iter_mut().zip(old.edge_plan.iter()) {
                edge.queue_cap_frames = edge.queue_cap_frames.max(old_edge.queue_cap_frames);
            }
            for (cap, old_cap) in output_queue_cap_frames
                .iter_mut()
                .zip(old.output_queue_cap_frames.iter())
            {
                *cap = (*cap).max(*old_cap);
            }
            bound_frames = bound_frames.max(old.bound_frames);
        }
        let num_slots = self.predecessors.len();

        // Drain scratch extents: assembled input, retained waves,
        // output-delay flush pushes, and compensation frame rows.
        // Existing graph scratch already covers these; growth stays on
        // the build path. The flush extent is load-bearing: output
        // flush shares the wave scratch, and its push can exceed any
        // single emission when output compensation is large.
        let mut need_input = 0usize;
        let mut need_wave = 0usize;
        let mut need_row = 0usize;
        for &node_id in &topo_order {
            let node = &self.nodes[&node_id];
            need_input = need_input.max(
                node_plan[node_id]
                    .process_quantum
                    .saturating_mul(node.input_channels()),
            );
            need_wave = need_wave.max(
                node_plan[node_id]
                    .single_emission
                    .saturating_mul(node.output_channels()),
            );
        }
        for (pos, &node_id) in self.output_nodes.iter().enumerate() {
            let node = &self.nodes[&node_id];
            need_wave =
                need_wave.max(output_comp_frames[pos].saturating_mul(node.output_channels()));
        }
        for plan in &edge_plan {
            need_row = need_row.max(plan.routed_channels.saturating_mul(2));
        }
        let buffers = self
            .process_buffers
            .as_mut()
            .ok_or("graph drain plan requires prepared process buffers")?;
        for (label, buffer, need) in [
            (
                "graph drain input scratch",
                &mut buffers.scratch_input,
                need_input,
            ),
            (
                "graph drain emission scratch",
                &mut buffers.scratch_output,
                need_wave,
            ),
            (
                "graph drain delay rows",
                &mut buffers.delay_scratch,
                need_row,
            ),
        ] {
            if buffer.len() < need {
                buffer
                    .try_reserve_exact(need - buffer.len())
                    .map_err(|error| format!("{label} reservation failed: {error}"))?;
                buffer.resize(need, 0.0);
            }
        }

        if preserve {
            let state = &self.graph_drain_state;
            let plan = &self.graph_drain_plan;
            let sizes_match = state.phases.len() == num_slots
                && state.holdover_data.len() == num_slots
                && state.holdover_frames.len() == num_slots
                && state.edges.len() == self.edges.len()
                && state.edge_committed.len() == self.edges.len()
                && state.output_flush_remaining.len() == self.output_nodes.len()
                && state.output_queues.len() == self.output_nodes.len()
                && plan.node_plan.len() == num_slots
                && plan.edge_plan.len() == self.edges.len()
                && plan.output_comp_frames.len() == self.output_nodes.len()
                && plan.output_queue_cap_frames.len() == self.output_nodes.len();
            if !sizes_match {
                return Err("graph drain state does not match the rebuilt topology".into());
            }
            // Never shrink below retained samples when metadata changed
            // without a topology edit.
            for (index, plan) in edge_plan.iter().enumerate() {
                let retained = self.graph_drain_state.edges[index].queue.len();
                let need = plan
                    .queue_cap_frames
                    .saturating_mul(plan.routed_channels)
                    .max(retained);
                let queue = &mut self.graph_drain_state.edges[index].queue;
                if queue.capacity() < need {
                    queue
                        .try_reserve_exact(need - queue.len())
                        .map_err(|error| {
                            format!("graph drain queue reservation failed: {error}")
                        })?;
                }
            }
            for &node_id in &topo_order {
                let need = node_plan[node_id]
                    .single_emission
                    .saturating_mul(self.nodes[&node_id].output_channels())
                    .max(self.graph_drain_state.holdover_data[node_id].len());
                let holdover = &mut self.graph_drain_state.holdover_data[node_id];
                if holdover.len() < need {
                    holdover
                        .try_reserve_exact(need - holdover.len())
                        .map_err(|error| {
                            format!("graph drain holdover reservation failed: {error}")
                        })?;
                    holdover.resize(need, 0.0);
                }
            }
            for (pos, &node_id) in self.output_nodes.iter().enumerate() {
                let retained = self.graph_drain_state.output_queues[pos].len();
                let need = output_queue_cap_frames[pos]
                    .saturating_mul(self.nodes[&node_id].output_channels())
                    .max(retained);
                let queue = &mut self.graph_drain_state.output_queues[pos];
                if queue.capacity() < need {
                    queue
                        .try_reserve_exact(need - queue.len())
                        .map_err(|error| {
                            format!("graph drain output queue reservation failed: {error}")
                        })?;
                }
            }
        } else {
            let mut edges = Vec::new();
            edges
                .try_reserve_exact(self.edges.len())
                .map_err(|error| format!("graph drain edge state reservation failed: {error}"))?;
            for plan in &edge_plan {
                let mut queue = VecDeque::new();
                let cap = plan.queue_cap_frames.saturating_mul(plan.routed_channels);
                if cap > 0 {
                    queue.try_reserve_exact(cap).map_err(|error| {
                        format!("graph drain queue reservation failed: {error}")
                    })?;
                }
                edges.push(GraphEdgeDrainState {
                    queue,
                    flush_remaining: 0,
                    eof: false,
                });
            }
            let mut holdover_data: Vec<Vec<f32>> = Vec::new();
            holdover_data
                .try_reserve_exact(num_slots)
                .map_err(|error| format!("graph drain holdover reservation failed: {error}"))?;
            for (node_id, plan) in node_plan.iter().enumerate() {
                let out_ch = self
                    .nodes
                    .get(&node_id)
                    .map_or(0, |node| node.output_channels());
                let samples = plan.single_emission.saturating_mul(out_ch);
                let mut wave = Vec::new();
                if samples > 0 {
                    wave.try_reserve_exact(samples).map_err(|error| {
                        format!("graph drain holdover reservation failed: {error}")
                    })?;
                    wave.resize(samples, 0.0);
                }
                holdover_data.push(wave);
            }
            let mut output_queues = Vec::new();
            output_queues
                .try_reserve_exact(self.output_nodes.len())
                .map_err(|error| format!("graph drain output queue reservation failed: {error}"))?;
            for (pos, &node_id) in self.output_nodes.iter().enumerate() {
                let mut queue = VecDeque::new();
                let cap = output_queue_cap_frames[pos]
                    .saturating_mul(self.nodes[&node_id].output_channels());
                if cap > 0 {
                    queue.try_reserve_exact(cap).map_err(|error| {
                        format!("graph drain output queue reservation failed: {error}")
                    })?;
                }
                output_queues.push(queue);
            }
            self.graph_drain_state = GraphDrainState {
                phases: vec![GraphNodeDrainPhase::Running; num_slots],
                edges,
                holdover_data,
                holdover_frames: vec![0; num_slots],
                edge_committed: vec![false; self.edges.len()],
                output_flush_remaining: vec![0; self.output_nodes.len()],
                output_queues,
                session_output_bound: None,
                #[cfg(test)]
                output_commit_retries: 0,
                #[cfg(test)]
                drain_refresh_count: 0,
            };
        }

        self.graph_drain_plan = GraphDrainPlan {
            topo_order,
            node_plan,
            edge_plan,
            output_comp_frames,
            output_queue_cap_frames,
            bound_frames,
            envelope_bound,
            signature,
        };
        Ok(())
    }

    /// Run one bounded EOS round over a branched graph.
    ///
    /// Precondition: built, nonempty, non-chain topology with mutations and
    /// parameter events already applied. Terminal state, then output
    /// geometry, then capacity are checked before any plugin call,
    /// mirroring the chain path. Each round visits nodes once in
    /// topological order; every node performs at most one native drain
    /// or process call per round.
    fn drain_graph(&mut self, output: &mut [f32]) -> Result<PluginDrainResult, String> {
        if Self::graph_drain_complete(&self.graph_drain_state, &self.nodes) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let output_channels = self.graph_drain_common_output_geometry()?;
        if output_channels == 0 || !output.len().is_multiple_of(output_channels) {
            return Err(format!(
                "Host drain output must contain whole frames of {output_channels} channels"
            ));
        }
        // Stream-state declarations outdate the build-time plan (a fresh
        // resampler reports a zero drain bound, mid-stream its block
        // maximum), so refresh from live metadata before enforcing
        // capacity. Detection is allocation-free; the merge only grows
        // capacities and reservations, never below retained content.
        if self.graph_node_declarations_changed() {
            self.rebuild_graph_drain_plan()?;
            #[cfg(test)]
            {
                self.graph_drain_state.drain_refresh_count += 1;
            }
        }
        // Freeze the enforced per-call output bound for the drain session.
        // No plugin call lands between the caller's begin query and this
        // first entry, so the merged peak equals what the caller sized
        // from; later live growth only enlarges internal reservations
        // while emission stays paced to this value.
        let session_output_bound = if let Some(bound) = self.graph_drain_state.session_output_bound
        {
            bound
        } else {
            let bound = self.graph_drain_plan.bound_frames;
            self.graph_drain_state.session_output_bound = Some(bound);
            bound
        };
        let required_samples = session_output_bound
            .checked_mul(output_channels)
            .ok_or("Host drain output capacity overflow")?;
        if output.len() < required_samples {
            return Err(format!(
                "Host drain output too small: need {required_samples} samples, got {}",
                output.len()
            ));
        }
        self.validate_graph_drain_geometry()?;
        self.transfer_process_retention_to_drain()?;
        let mut guard = BufferGuard::take(&mut self.process_buffers);
        let bufs = guard.get_mut();
        // The round cannot take `&mut self` while the guard holds
        // `process_buffers`; it runs on disjoint field borrows instead.
        // The guard still owns buffer restoration on every return path.
        let mut ctx = GraphDrainRoundCtx {
            state: &mut self.graph_drain_state,
            plan: &self.graph_drain_plan,
            plugins: &mut self.plugins,
            nodes: &self.nodes,
            edges: &self.edges,
            predecessors: &self.predecessors,
            successors: &self.successors,
            predecessor_edge_indices: &self.predecessor_edge_indices,
            output_nodes: &self.output_nodes,
            is_output_node: &self.is_output_node,
            node_input_sample_rates: &self.node_input_sample_rates,
            node_input_positions: &mut self.node_input_positions,
            output_channels,
            session_output_bound,
        };
        Self::drain_graph_round(&mut ctx, output, bufs)
    }

    /// True when every node completed and no wave, queue, or flush remains.
    fn graph_drain_complete(state: &GraphDrainState, nodes: &HashMap<NodeId, GraphNode>) -> bool {
        let nodes_done = nodes
            .keys()
            .all(|&id| state.phases.get(id) == Some(&GraphNodeDrainPhase::Complete));
        let waves_done = state.holdover_frames.iter().all(|&frames| frames == 0);
        let edges_done = state.edges.iter().all(|edge| edge.queue.is_empty());
        let flush_done = state.edges.iter().all(|edge| edge.flush_remaining == 0);
        let eof_done = state.edges.iter().all(|edge| edge.eof);
        let output_done = state
            .output_flush_remaining
            .iter()
            .all(|&frames| frames == 0)
            && state.output_queues.iter().all(|queue| queue.is_empty());
        nodes_done && waves_done && edges_done && flush_done && eof_done && output_done
    }

    /// Common host-output width for graph EOS, checked before capacity.
    ///
    /// Width-mismatched multiple outputs and mixed output rates are
    /// refused loudly here so the capacity preflight multiplies frames
    /// by a defined width. Single-output width is that output node's
    /// width, identical to `output_channels()` today.
    fn graph_drain_common_output_geometry(&self) -> Result<usize, String> {
        let Some(&first) = self.output_nodes.first() else {
            return Err("graph drain has no output node".into());
        };
        let width = self.nodes[&first].output_channels();
        for &node_id in &self.output_nodes {
            let node_width = self.nodes[&node_id].output_channels();
            if node_width != width {
                return Err(format!(
                    "graph drain requires every output width to match \
                     the host output width {width}, \
                     node {node_id} has {node_width}"
                ));
            }
        }
        let rate = self.node_output_sample_rates[first];
        for &node_id in &self.output_nodes {
            if self.node_output_sample_rates[node_id] != rate {
                return Err(format!(
                    "graph drain requires equal output sample rates, node {node_id} has {}",
                    self.node_output_sample_rates[node_id]
                ));
            }
        }
        Ok(width)
    }

    /// Retry-safe geometry gate: every check precedes any plugin call.
    fn validate_graph_drain_geometry(&self) -> Result<(), String> {
        for (&node_id, node) in &self.nodes {
            if node.input_channels() == 0 || node.output_channels() == 0 {
                return Err(format!(
                    "graph drain requires nonzero channels at '{}'",
                    node.name
                ));
            }
            if node.bypassed && node.input_channels() != node.output_channels() {
                return Err(format!(
                    "graph drain cannot bypass width-changing node '{}'",
                    node.name
                ));
            }
            let input_rate = self
                .node_input_sample_rates
                .get(node_id)
                .copied()
                .unwrap_or(0.0);
            if input_rate == 0.0 {
                return Err(format!(
                    "graph drain requires a nonzero sample rate at '{}'",
                    node.name
                ));
            }
            if self.plugins.get(node_id).and_then(Option::as_ref).is_none() {
                return Err(format!("graph drain is missing plugin {node_id}"));
            }
        }
        let state = &self.graph_drain_state;
        let plan = &self.graph_drain_plan;
        if state.phases.len() != self.predecessors.len()
            || state.edges.len() != self.edges.len()
            || state.output_flush_remaining.len() != self.output_nodes.len()
            || state.output_queues.len() != self.output_nodes.len()
            || plan.node_plan.len() != self.predecessors.len()
            || plan.edge_plan.len() != self.edges.len()
            || plan.output_comp_frames.len() != self.output_nodes.len()
            || plan.output_queue_cap_frames.len() != self.output_nodes.len()
            || plan.topo_order.len() != self.nodes.len()
        {
            return Err("graph drain plan is missing; rebuild the host".into());
        }
        Ok(())
    }

    #[allow(
        clippy::needless_range_loop,
        reason = "index loop required: the per-node call reborrows the whole round context"
    )]
    fn drain_graph_round(
        ctx: &mut GraphDrainRoundCtx<'_>,
        output: &mut [f32],
        bufs: &mut ProcessBuffers<f32>,
    ) -> Result<PluginDrainResult, String> {
        let mut round = GraphDrainRound::default();
        let order_len = ctx.plan.topo_order.len();
        for pos in 0..order_len {
            let node_id = ctx.plan.topo_order[pos];
            Self::graph_drain_node(ctx, node_id, &mut round, bufs)?;
        }
        Self::graph_drain_discard_key_surplus(ctx, &mut round);
        Self::graph_drain_output_flush(ctx, &mut round, bufs)?;
        Self::graph_drain_consume_outputs(ctx, output, &mut round);
        if !round.acted && !Self::graph_drain_complete(&*ctx.state, ctx.nodes) {
            return Err("graph drain scheduler stalled before completion".into());
        }
        Ok(PluginDrainResult {
            frames: round.frames,
            complete: Self::graph_drain_complete(&*ctx.state, ctx.nodes),
        })
    }

    /// Drop key frames queued for nodes past their audio input.
    ///
    /// Once a node leaves `Running`, its audio input is finished and key
    /// frames arriving later have no program to control, so they are
    /// popped without producing output. Discarding keeps room for the
    /// key source to finish its own drain instead of blocking forever
    /// on a full queue, and it touches only this edge: other consumers
    /// of the same key source are unaffected.
    fn graph_drain_discard_key_surplus(
        ctx: &mut GraphDrainRoundCtx<'_>,
        round: &mut GraphDrainRound,
    ) {
        for (edge_index, edge) in ctx.edges.iter().enumerate() {
            if edge.edge_type != EdgeType::Sidechain {
                continue;
            }
            if ctx.state.phases[edge.to_node] == GraphNodeDrainPhase::Running {
                continue;
            }
            let queue = &mut ctx.state.edges[edge_index].queue;
            if !queue.is_empty() {
                queue.clear();
                round.acted = true;
            }
        }
    }

    fn graph_drain_node(
        ctx: &mut GraphDrainRoundCtx<'_>,
        node_id: NodeId,
        round: &mut GraphDrainRound,
        bufs: &mut ProcessBuffers<f32>,
    ) -> Result<(), String> {
        if ctx.state.phases[node_id] == GraphNodeDrainPhase::Complete {
            // The final wave precedes compensation flushing in time.
            Self::graph_drain_commit_holdover(ctx, node_id, round, bufs)?;
            if ctx.state.holdover_frames[node_id] == 0 {
                Self::graph_drain_flush_edges(ctx, node_id, bufs, &mut round.acted)?;
            }
            return Ok(());
        }
        Self::graph_drain_commit_holdover(ctx, node_id, round, bufs)?;
        if ctx.state.holdover_frames[node_id] > 0 {
            return Ok(());
        }
        match ctx.state.phases[node_id] {
            GraphNodeDrainPhase::Complete => Ok(()),
            GraphNodeDrainPhase::Draining { .. } => {
                Self::graph_drain_native(ctx, node_id, round, bufs)
            }
            GraphNodeDrainPhase::Running => Self::graph_drain_consume(ctx, node_id, round, bufs),
        }
    }

    /// Feed compensation-flush zeros for a completed source node.
    #[allow(
        clippy::needless_range_loop,
        reason = "index loop required: the per-edge call reborrows the whole round context"
    )]
    fn graph_drain_flush_edges(
        ctx: &mut GraphDrainRoundCtx<'_>,
        source: NodeId,
        bufs: &mut ProcessBuffers<f32>,
        acted: &mut bool,
    ) -> Result<(), String> {
        let out_len = ctx.successors[source].len();
        for pos in 0..out_len {
            let edge_index = ctx.successors[source][pos];
            Self::graph_drain_flush_edge(ctx, edge_index, bufs, acted)?;
        }
        Ok(())
    }

    fn graph_drain_flush_edge(
        ctx: &mut GraphDrainRoundCtx<'_>,
        edge_index: usize,
        bufs: &mut ProcessBuffers<f32>,
        acted: &mut bool,
    ) -> Result<(), String> {
        if ctx.state.edges[edge_index].eof {
            return Ok(());
        }
        let plan = ctx.plan.edge_plan[edge_index];
        if plan.routed_channels == 0 {
            ctx.state.edges[edge_index].eof = true;
            *acted = true;
            return Ok(());
        }
        let width = plan.routed_channels;
        let cap_samples = plan.queue_cap_frames.saturating_mul(width);
        let queued = ctx.state.edges[edge_index].queue.len();
        let room_frames = cap_samples.saturating_sub(queued) / width;
        let push = ctx.state.edges[edge_index].flush_remaining.min(room_frames);
        if push > 0 {
            ensure_len(&mut bufs.delay_scratch, width * 2);
            let (frame_out, frame_zero) = bufs.delay_scratch.split_at_mut(width);
            frame_zero[..width].fill(0.0);
            let mut delay = bufs.compensation_delays.delays[edge_index].as_mut();
            let queue = &mut ctx.state.edges[edge_index].queue;
            for _ in 0..push {
                // Reborrow per iteration: the `&mut` must stay live for
                // the whole flush, not move into the first frame.
                if let Some(delay) = delay.as_mut() {
                    delay.process_frame(&frame_zero[..width], &mut frame_out[..width]);
                    queue.extend(frame_out[..width].iter().copied());
                } else {
                    queue.extend(frame_zero[..width].iter().copied());
                }
            }
            ctx.state.edges[edge_index].flush_remaining -= push;
            *acted = true;
        }
        if ctx.state.edges[edge_index].flush_remaining == 0
            && ctx.state.edges[edge_index].queue.is_empty()
        {
            ctx.state.edges[edge_index].eof = true;
            *acted = true;
        }
        Ok(())
    }

    /// Commit a retained holdover wave to out-edges or the pending FIFO.
    ///
    /// Commit is per-edge: edges with room accept the wave while full
    /// edges wait, and the holdover clears only after every destination
    /// accepted it. Output nodes commit to their pending FIFO the same
    /// way: a full FIFO retains the wave (backpressure), and an empty
    /// one always fits it because every cap covers a full single
    /// emission plus compensation.
    fn graph_drain_commit_holdover(
        ctx: &mut GraphDrainRoundCtx<'_>,
        node_id: NodeId,
        round: &mut GraphDrainRound,
        bufs: &mut ProcessBuffers<f32>,
    ) -> Result<(), String> {
        let held = ctx.state.holdover_frames[node_id];
        if held == 0 {
            return Ok(());
        }
        if ctx.is_output_node[node_id] {
            let output_channels = ctx.output_channels;
            let out_pos = ctx
                .output_nodes
                .iter()
                .position(|&id| id == node_id)
                .ok_or("graph drain lost its output node")?;
            let samples = held.saturating_mul(output_channels);
            // Every output appends post-delay frames to its own pending
            // FIFO in emission order; the round end emits the aligned
            // prefix. Scheduling rounds never define sample positions.
            // A full FIFO retains the wave (backpressure) exactly like
            // a full edge queue; the wave fits once emission drains
            // room, independent of total tail duration. Room is checked
            // before the scratch copy and the stateful delay feed: the
            // delay swap advances history, so feeding a wave that never
            // queues would make its retry release wrong frames and lose
            // real ones.
            let cap = ctx.plan.output_queue_cap_frames[out_pos].saturating_mul(output_channels);
            if ctx.state.output_queues[out_pos].len() + samples > cap {
                #[cfg(test)]
                {
                    ctx.state.output_commit_retries += 1;
                }
                return Ok(());
            }
            ensure_len(&mut bufs.scratch_output, samples);
            let wave = &ctx.state.holdover_data[node_id][..samples];
            let delayed = &mut bufs.scratch_output[..samples];
            delayed.copy_from_slice(wave);
            if let Some(delay) = bufs.compensation_delays.output_delays[out_pos].as_mut() {
                for frame in delayed.chunks_exact_mut(output_channels) {
                    delay.process_frame_in_place(frame);
                }
            }
            let queue = &mut ctx.state.output_queues[out_pos];
            queue.extend(bufs.scratch_output[..samples].iter().copied());
            ctx.state.holdover_frames[node_id] = 0;
            round.acted = true;
            return Ok(());
        }
        for &edge_index in &ctx.successors[node_id] {
            if ctx.state.edge_committed[edge_index] {
                continue;
            }
            let plan = ctx.plan.edge_plan[edge_index];
            if plan.routed_channels == 0 {
                ctx.state.edge_committed[edge_index] = true;
                continue;
            }
            let width = plan.routed_channels;
            let need = held.saturating_mul(width);
            let cap = plan.queue_cap_frames.saturating_mul(width);
            if ctx.state.edges[edge_index].queue.len() + need > cap {
                continue;
            }
            let edge = &ctx.edges[edge_index];
            let source_channels = ctx.nodes[&edge.from_node].output_channels();
            ensure_len(&mut bufs.delay_scratch, width.saturating_mul(2).max(1));
            let has_delay = bufs.compensation_delays.delays[edge_index].is_some();
            for frame in 0..held {
                // Map the source wave into the first scratch row. Out-of-range
                // selections read as silence, exactly like the process merge.
                {
                    let wave = &ctx.state.holdover_data[node_id];
                    let row = &mut bufs.delay_scratch[..width];
                    for (lane, slot) in row.iter_mut().enumerate() {
                        let source_lane = match &edge.channel_map {
                            Some(map) => map.get(lane).copied().unwrap_or(usize::MAX),
                            None => lane,
                        };
                        *slot = wave
                            .get(
                                frame
                                    .saturating_mul(source_channels)
                                    .saturating_add(source_lane),
                            )
                            .copied()
                            .unwrap_or_default();
                    }
                }
                if has_delay {
                    let (row_in, row_out) = bufs.delay_scratch.split_at_mut(width);
                    let delay = bufs.compensation_delays.delays[edge_index]
                        .as_mut()
                        .ok_or("graph drain lost its compensation delay")?;
                    delay.process_frame(&row_in[..width], &mut row_out[..width]);
                    ctx.state.edges[edge_index]
                        .queue
                        .extend(row_out[..width].iter().copied());
                } else {
                    ctx.state.edges[edge_index]
                        .queue
                        .extend(bufs.delay_scratch[..width].iter().copied());
                }
            }
            ctx.state.edge_committed[edge_index] = true;
            round.acted = true;
        }
        let done = ctx.successors[node_id]
            .iter()
            .all(|&edge_index| ctx.state.edge_committed[edge_index]);
        if done {
            ctx.state.holdover_frames[node_id] = 0;
            for &edge_index in &ctx.successors[node_id] {
                ctx.state.edge_committed[edge_index] = false;
            }
        }
        Ok(())
    }

    /// Drain one node whose input is complete, at most once per round.
    ///
    /// Preparation, bound snapshot, and quota charging mirror the chain
    /// path exactly: `begin_drain` runs once, the bound is queried once,
    /// quota is charged only for `Ok` calls, and unknown bounds fall back
    /// to 4096 successful calls.
    fn graph_drain_native(
        ctx: &mut GraphDrainRoundCtx<'_>,
        node_id: NodeId,
        round: &mut GraphDrainRound,
        bufs: &mut ProcessBuffers<f32>,
    ) -> Result<(), String> {
        let node = &ctx.nodes[&node_id];
        if node.bypassed {
            ctx.state.phases[node_id] = GraphNodeDrainPhase::Complete;
            Self::graph_drain_arm_edge_flush(ctx, node_id);
            return Ok(());
        }
        let input_rate = ctx.node_input_sample_rates[node_id];
        let drain_capacity = ctx.plugins[node_id]
            .as_ref()
            .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?
            .drain_output_frames_max();
        let drain_samples = drain_capacity
            .checked_mul(node.output_channels())
            .ok_or("Host native drain capacity overflow")?;
        if input_rate == 0.0 || node.input_channels() == 0 || node.output_channels() == 0 {
            return Err("Host native drain requires nonzero rate and channels".into());
        }
        let GraphNodeDrainPhase::Draining {
            prepared,
            remaining_calls,
            grant_tail,
        } = &mut ctx.state.phases[node_id]
        else {
            return Err(format!(
                "graph drain lost the draining phase of node {node_id}"
            ));
        };
        let context = ProcessContext::new(f64::from(input_rate), 0)
            .with_sample_position(ctx.node_input_positions[node_id]);
        if !*prepared {
            ctx.plugins[node_id]
                .as_mut()
                .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?
                .begin_drain(&context)?;
            *prepared = true;
            round.acted = true;
        }
        if remaining_calls.is_none() {
            let plugin = ctx.plugins[node_id]
                .as_ref()
                .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?;
            let bound = plugin
                .drain_call_bound()
                .map_or(UNKNOWN_DRAIN_CALL_LIMIT, std::num::NonZeroU64::get);
            *grant_tail = Some(plugin.tail_length());
            *remaining_calls = Some(bound);
        }
        let mut remaining = remaining_calls.unwrap_or(0);
        if remaining == 0 {
            let plugin = ctx.plugins[node_id]
                .as_ref()
                .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?;
            let grant = grant_tail.unwrap_or(TailLength::Unknown);
            if let Some((budget, current)) = drain_quota_refresh(plugin.as_ref(), grant) {
                *grant_tail = Some(current);
                *remaining_calls = Some(budget);
                remaining = budget;
            } else {
                return Err(format!(
                    "plugin '{}' drain did not converge within its call bound",
                    node.name
                ));
            }
        }
        let holdover = &mut ctx.state.holdover_data[node_id];
        if holdover.len() < drain_samples {
            return Err(format!(
                "graph drain holdover for node {node_id} is smaller than its declared drain capacity"
            ));
        }
        let result = ctx.plugins[node_id]
            .as_mut()
            .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?
            .drain(&mut holdover[..drain_samples], &context)?;
        if let GraphNodeDrainPhase::Draining {
            remaining_calls, ..
        } = &mut ctx.state.phases[node_id]
        {
            *remaining_calls = Some(remaining - 1);
        }
        round.acted = true;
        if result.frames > drain_capacity {
            return Err("Plugin drain exceeded its declared output capacity".into());
        }
        ctx.state.holdover_frames[node_id] = result.frames;
        if result.complete {
            ctx.state.phases[node_id] = GraphNodeDrainPhase::Complete;
            Self::graph_drain_arm_edge_flush(ctx, node_id);
        }
        Self::graph_drain_commit_holdover(ctx, node_id, round, bufs)
    }

    /// Arm compensation flushing on every out-edge of a completed node,
    /// plus the output-delay flush when the node itself is an output.
    fn graph_drain_arm_edge_flush(ctx: &mut GraphDrainRoundCtx<'_>, source: NodeId) {
        for &edge_index in &ctx.successors[source] {
            if !ctx.state.edges[edge_index].eof {
                ctx.state.edges[edge_index].flush_remaining =
                    ctx.plan.edge_plan[edge_index].comp_frames;
            }
        }
        if ctx.is_output_node[source]
            && let Some(out_pos) = ctx.output_nodes.iter().position(|&id| id == source)
        {
            ctx.state.output_flush_remaining[out_pos] = ctx.plan.output_comp_frames[out_pos];
        }
    }

    /// Consume one aligned input slice, or transition to draining.
    ///
    /// A node consumes only when every data-carrying non-EOF incoming edge
    /// holds at least one frame; EOF edges pad with silence. Frame index is
    /// the timestamp: queues hold post-compensation frames and all join
    /// inputs share one rate, so equal indices align key and program data
    /// after node latency. Bypassed nodes pass the slice through unchanged.
    fn graph_drain_consume(
        ctx: &mut GraphDrainRoundCtx<'_>,
        node_id: NodeId,
        round: &mut GraphDrainRound,
        bufs: &mut ProcessBuffers<f32>,
    ) -> Result<(), String> {
        if Self::graph_drain_input_complete(ctx, node_id) {
            // Audio input is finished; flag it so later rounds treat it
            // as EOF without recomputing. Key edges stay unflagged:
            // their sources may still produce surplus to discard.
            for (edge, &edge_index) in ctx.predecessors[node_id]
                .iter()
                .zip(ctx.predecessor_edge_indices[node_id].iter())
            {
                if edge.edge_type == EdgeType::Audio {
                    ctx.state.edges[edge_index].eof = true;
                }
            }
            if ctx.nodes[&node_id].bypassed {
                // Bypassed nodes complete here, so their flush arms
                // here. Live nodes arm at completion instead (native):
                // arming at transition would let the every-round
                // output flush push before the tail finishes, and the
                // completion re-arm would then flush twice.
                ctx.state.phases[node_id] = GraphNodeDrainPhase::Complete;
                round.acted = true;
                Self::graph_drain_arm_edge_flush(ctx, node_id);
                return Ok(());
            }
            ctx.state.phases[node_id] = GraphNodeDrainPhase::Draining {
                prepared: false,
                remaining_calls: None,
                grant_tail: None,
            };
            round.acted = true;
            return Self::graph_drain_native(ctx, node_id, round, bufs);
        }
        let Some(frames) = Self::graph_drain_aligned_frames(ctx, node_id) else {
            return Ok(());
        };
        let node = &ctx.nodes[&node_id];
        let input_channels = node.input_channels();
        let output_channels = node.output_channels();
        let bypassed = node.bypassed;
        let input_samples = frames
            .checked_mul(input_channels)
            .ok_or("graph drain input size overflow")?;
        ensure_len(&mut bufs.scratch_input, input_samples);
        bufs.scratch_input[..input_samples].fill(0.0);
        Self::graph_drain_assemble_input(
            ctx,
            node_id,
            frames,
            &mut bufs.scratch_input[..input_samples],
            &mut round.acted,
        );
        round.acted = true;
        if bypassed {
            let holdover = &mut ctx.state.holdover_data[node_id];
            if holdover.len() < input_samples {
                return Err(format!(
                    "graph drain holdover for node {node_id} is smaller than its bypass wave"
                ));
            }
            holdover[..input_samples].copy_from_slice(&bufs.scratch_input[..input_samples]);
            ctx.state.holdover_frames[node_id] = frames;
            ctx.node_input_positions[node_id] =
                Self::advance_sample_position(ctx.node_input_positions[node_id], frames)?;
            return Self::graph_drain_commit_holdover(ctx, node_id, round, bufs);
        }
        let plugin = ctx.plugins[node_id]
            .as_mut()
            .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?;
        let capacity = plugin.output_frames_for_input(frames);
        let output_samples = capacity
            .checked_mul(output_channels)
            .ok_or("graph drain output size overflow")?;
        let holdover = &mut ctx.state.holdover_data[node_id];
        if holdover.len() < output_samples {
            return Err(format!(
                "graph drain holdover for node {node_id} is smaller than its declared process capacity"
            ));
        }
        let input_rate = ctx.node_input_sample_rates[node_id];
        let context = ProcessContext::new(f64::from(input_rate), frames)
            .with_sample_position(ctx.node_input_positions[node_id]);
        // The holdover and scratch borrows below are disjoint by construction:
        // rebuild the slices after the plugin borrow to satisfy the checker.
        let produced = {
            let holdover = &mut ctx.state.holdover_data[node_id];
            let plugin = ctx.plugins[node_id]
                .as_mut()
                .ok_or_else(|| format!("graph drain is missing plugin {node_id}"))?;
            plugin.process(
                &bufs.scratch_input[..input_samples],
                &mut holdover[..output_samples],
                &context,
            )?
        };
        if produced > capacity {
            return Err(format!(
                "plugin '{}' process exceeded its declared output capacity",
                ctx.nodes[&node_id].name
            ));
        }
        ctx.state.holdover_frames[node_id] = produced;
        ctx.node_input_positions[node_id] =
            Self::advance_sample_position(ctx.node_input_positions[node_id], frames)?;
        Self::graph_drain_commit_holdover(ctx, node_id, round, bufs)
    }

    /// True when an edge can never carry another frame.
    ///
    /// Flagged EOF and zero-width (inert) edges qualify immediately. An
    /// unflagged edge qualifies when its source is complete, its queue is
    /// empty, its compensation flush is exhausted, and no wave is still
    /// retained for it: either the source holds nothing, or this edge
    /// already accepted the retained wave (other edges may lag). The
    /// holdover clause is load-bearing: without it the flag can be set
    /// while a wave is still owed, stranding that wave in an EOF queue
    /// no join will ever consume again.
    fn graph_drain_edge_finished(ctx: &GraphDrainRoundCtx<'_>, edge_index: usize) -> bool {
        if ctx.plan.edge_plan[edge_index].routed_channels == 0 {
            return true;
        }
        let edge = &ctx.state.edges[edge_index];
        if edge.eof {
            return true;
        }
        if !edge.queue.is_empty() || edge.flush_remaining != 0 {
            return false;
        }
        let from = ctx.edges[edge_index].from_node;
        if ctx.state.phases[from] != GraphNodeDrainPhase::Complete {
            return false;
        }
        ctx.state.holdover_frames[from] == 0 || ctx.state.edge_committed[edge_index]
    }

    /// True when no more audio input can ever arrive at `node_id`.
    ///
    /// Audio edges alone determine join length: a node becomes
    /// input-complete when every audio in-edge is finished. Key
    /// (sidechain) edges neither gate this transition nor extend the
    /// join; their surplus past audio end is discarded, never padded
    /// into program frames.
    fn graph_drain_input_complete(ctx: &GraphDrainRoundCtx<'_>, node_id: NodeId) -> bool {
        ctx.predecessors[node_id]
            .iter()
            .zip(ctx.predecessor_edge_indices[node_id].iter())
            .all(|(edge, &edge_index)| {
                edge.edge_type == EdgeType::Sidechain
                    || Self::graph_drain_edge_finished(ctx, edge_index)
            })
    }

    /// Aligned consumable frames, or `None` when a branch must catch up.
    ///
    /// Finished edges (audio or key) pad with silence. Live edges of
    /// either kind gate consumption: a non-EOF key branch that has not
    /// delivered its frames at these timestamps is awaited, never
    /// zero-filled early. Only confirmed key EOF permits zero padding
    /// while program remains.
    fn graph_drain_aligned_frames(ctx: &GraphDrainRoundCtx<'_>, node_id: NodeId) -> Option<usize> {
        let mut frames = ctx.plan.node_plan[node_id].process_quantum;
        let mut live = false;
        for &edge_index in &ctx.predecessor_edge_indices[node_id] {
            let width = ctx.plan.edge_plan[edge_index].routed_channels;
            if width == 0 {
                continue;
            }
            if Self::graph_drain_edge_finished(ctx, edge_index) {
                continue;
            }
            live = true;
            let queued = ctx.state.edges[edge_index].queue.len() / width;
            if queued == 0 {
                return None;
            }
            frames = frames.min(queued);
        }
        if live { Some(frames.max(1)) } else { None }
    }

    /// Assemble one aligned input slice from edge queues, popping consumed
    /// frames. Channel maps were applied at production; offsets and the
    /// sidechain packing order mirror the process-time merge exactly,
    /// including the running key-bus offset sequence. Finished edges pad
    /// with silence; live edges always hold the consumed frames because
    /// alignment gated on them.
    fn graph_drain_assemble_input(
        ctx: &mut GraphDrainRoundCtx<'_>,
        node_id: NodeId,
        frames: usize,
        assembled: &mut [f32],
        acted: &mut bool,
    ) {
        let node = &ctx.nodes[&node_id];
        let input_channels = node.input_channels();
        let has_sidechain = ctx.predecessors[node_id]
            .iter()
            .any(|edge| edge.edge_type == EdgeType::Sidechain);
        let primary = if has_sidechain && input_channels > node.output_channels() {
            node.output_channels()
        } else {
            input_channels
        };
        let mut sidechain_offset = primary;
        for (edge, &edge_index) in ctx.predecessors[node_id]
            .iter()
            .zip(ctx.predecessor_edge_indices[node_id].iter())
        {
            let width = ctx.plan.edge_plan[edge_index].routed_channels;
            let source_channels = ctx.nodes[&edge.from_node].output_channels();
            let dest_offset = match edge.edge_type {
                EdgeType::Audio => edge.destination_offset,
                EdgeType::Sidechain => {
                    if sidechain_offset >= input_channels {
                        continue;
                    }
                    let offset = sidechain_offset;
                    let requested = edge
                        .channel_map
                        .as_ref()
                        .map_or(source_channels, |map| map.len());
                    sidechain_offset = (sidechain_offset + requested).min(input_channels);
                    offset
                }
            };
            let available = match edge.edge_type {
                EdgeType::Audio => primary.saturating_sub(dest_offset),
                EdgeType::Sidechain => input_channels.saturating_sub(dest_offset),
            };
            if available == 0 || width == 0 {
                continue;
            }
            let lanes = width.min(available);
            let flagged = ctx.state.edges[edge_index].eof;
            let finished = flagged || Self::graph_drain_edge_finished(ctx, edge_index);
            let queue = &mut ctx.state.edges[edge_index].queue;
            for frame in 0..frames {
                for lane in 0..lanes {
                    let sample = if finished {
                        0.0
                    } else {
                        queue.pop_front().unwrap_or_default()
                    };
                    let dst = frame * input_channels + dest_offset + lane;
                    if let Some(slot) = assembled.get_mut(dst) {
                        *slot += sample;
                    }
                }
                // Discard routed lanes past the destination window so queue
                // and frame counts stay aligned.
                if !finished {
                    for _ in lanes..width {
                        queue.pop_front();
                    }
                }
            }
            // A finished edge with an emptied queue never carries another
            // frame; flagging it now saves a round. The holdover clause
            // below is what keeps a retained wave from stranding in a
            // flagged queue: the flag waits until the source holds
            // nothing more for this edge.
            if !flagged
                && queue.is_empty()
                && ctx.state.edges[edge_index].flush_remaining == 0
                && ctx.state.phases[edge.from_node] == GraphNodeDrainPhase::Complete
                && (ctx.state.holdover_frames[edge.from_node] == 0
                    || ctx.state.edge_committed[edge_index])
            {
                ctx.state.edges[edge_index].eof = true;
                *acted = true;
            }
        }
    }

    /// Feed armed output-delay zeros after the last emission.
    ///
    /// Output delays hold process-time and drain-wave history that must
    /// release after the final real frame; the flush arms when its
    /// output node completes and runs once the retained wave cleared.
    /// Single outputs sum straight into the caller buffer, which stays
    /// ordered because a single sink completes only after every node.
    /// Concurrent outputs queue their flush behind their waves so the
    /// round-end consume aligns every output by timestamp.
    fn graph_drain_output_flush(
        ctx: &mut GraphDrainRoundCtx<'_>,
        round: &mut GraphDrainRound,
        bufs: &mut ProcessBuffers<f32>,
    ) -> Result<(), String> {
        let output_channels = ctx.output_channels;
        if output_channels == 0 {
            return Ok(());
        }
        for (out_pos, &node_id) in ctx.output_nodes.iter().enumerate() {
            let remaining = ctx.state.output_flush_remaining[out_pos];
            if remaining == 0 {
                continue;
            }
            if ctx.state.holdover_frames[node_id] > 0 {
                continue;
            }
            if bufs.compensation_delays.output_delays[out_pos].is_none() {
                ctx.state.output_flush_remaining[out_pos] = 0;
                continue;
            }
            // Flush appends behind already-queued waves in the same
            // pending FIFO, preserving absolute emission order. Only
            // the room available is pushed; the remainder stays armed
            // for later rounds, exactly like edge compensation flush.
            let cap = ctx.plan.output_queue_cap_frames[out_pos].saturating_mul(output_channels);
            let queued = ctx.state.output_queues[out_pos].len();
            let room_frames = cap.saturating_sub(queued) / output_channels;
            let push = remaining.min(room_frames);
            if push == 0 {
                continue;
            }
            let samples = push.saturating_mul(output_channels);
            ensure_len(&mut bufs.scratch_output, samples);
            bufs.scratch_output[..samples].fill(0.0);
            if let Some(delay) = bufs.compensation_delays.output_delays[out_pos].as_mut() {
                for frame in bufs.scratch_output[..samples].chunks_exact_mut(output_channels) {
                    delay.process_frame_in_place(frame);
                }
            }
            let queue = &mut ctx.state.output_queues[out_pos];
            queue.extend(bufs.scratch_output[..samples].iter().copied());
            ctx.state.output_flush_remaining[out_pos] -= push;
            round.acted = true;
        }
        Ok(())
    }

    /// Consume one aligned slice from concurrent output queues.
    ///
    /// Frame index is the timestamp across outputs. Outputs that can
    /// still emit gate consumption: a slice covers only frames every
    /// live output already holds. Exhausted outputs pad with silence
    /// past their final frame, exactly like EOF edges at a join.
    fn graph_drain_consume_outputs(
        ctx: &mut GraphDrainRoundCtx<'_>,
        output: &mut [f32],
        round: &mut GraphDrainRound,
    ) {
        let output_channels = ctx.output_channels;
        if output_channels == 0 || ctx.output_nodes.is_empty() {
            return;
        }
        // Frame index is the timestamp across outputs. A single output
        // emits everything pending (bounded by the session per-call
        // bound); concurrent outputs emit the common prefix every live
        // output already holds, with exhausted outputs padding silence
        // past their final frame, exactly like EOF edges at a join.
        let mut frames = ctx.session_output_bound;
        if ctx.output_nodes.len() == 1 {
            let queued = ctx.state.output_queues[0].len() / output_channels;
            frames = frames.min(queued);
        } else {
            let mut live = false;
            let mut longest = 0usize;
            for (out_pos, &node_id) in ctx.output_nodes.iter().enumerate() {
                let exhausted = ctx.state.phases[node_id] == GraphNodeDrainPhase::Complete
                    && ctx.state.holdover_frames[node_id] == 0
                    && ctx.state.output_flush_remaining[out_pos] == 0;
                let queued = ctx.state.output_queues[out_pos].len() / output_channels;
                longest = longest.max(queued);
                if exhausted {
                    continue;
                }
                live = true;
                if queued == 0 {
                    return;
                }
                frames = frames.min(queued);
            }
            if !live {
                frames = frames.min(longest);
            }
        }
        if frames == 0 {
            return;
        }
        // Drain rounds write the caller buffer only here, so the slice
        // starts silent and each output adds its aligned share.
        output[..frames * output_channels].fill(0.0);
        round.frames = frames;
        round.acted = true;
        for queue in &mut ctx.state.output_queues {
            let take = (queue.len() / output_channels).min(frames) * output_channels;
            for slot in output[..take].iter_mut() {
                *slot += queue.pop_front().unwrap_or_default();
            }
        }
    }

    /// Advance one bounded EOF step for a prepared terminal sink route.
    pub fn drain_to_sink(&mut self) -> Result<SinkDrainResult, SinkDrainError> {
        match self.terminal_sink_lifecycle {
            None => return Err(SinkDrainError::ModeDisabled),
            Some(TerminalSinkLifecycle::Complete) => {
                return Ok(SinkDrainResult {
                    source_tail_frames_handed_to_sink: 0,
                    complete: true,
                });
            }
            Some(TerminalSinkLifecycle::ResetRequired) => {
                return Err(SinkDrainError::ResetRequired(
                    "the previous operation had uncertain progress".into(),
                ));
            }
            Some(TerminalSinkLifecycle::Running | TerminalSinkLifecycle::Draining) => {}
        }

        let first_entry = self.terminal_sink_lifecycle == Some(TerminalSinkLifecycle::Running);
        if !self.built {
            return Err(SinkDrainError::GraphNotBuilt);
        }
        let drain_horizon = self
            .chain_envelope_prep_frames()
            .unwrap_or_else(|| self.drain_output_frames_max());
        self.preflight_process_horizon(drain_horizon)
            .map_err(SinkDrainError::InvalidRoute)?;
        if first_entry {
            if !self.queues.graph_mutation_rx.is_empty() {
                return Err(SinkDrainError::PendingGraphMutation);
            }
            if !self.built {
                return Err(SinkDrainError::GraphNotBuilt);
            }
            let (_, source_id) = self
                .validate_terminal_sink_graph()
                .map_err(SinkDrainError::InvalidRoute)?;
            self.validate_terminal_sink_tail_source(source_id)?;

            let mut events = std::mem::take(&mut self.queues.parameter_event_scratch);
            self.drain_parameter_events_into(&mut events);
            for event in events.drain(..) {
                let _ = self.apply_parameter_event(event);
            }
            self.queues.parameter_event_scratch = events;
            self.drain_state = DrainState::default();
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::Draining);
            self.terminal_sink_source_complete = self.chain_nodes.len() == 1;
        }

        let (sink_id, source_id) = self
            .validate_terminal_sink_graph()
            .map_err(SinkDrainError::InvalidRoute)?;
        self.validate_terminal_sink_tail_source(source_id)?;
        let sink_rate = self.node_input_sample_rates[sink_id];
        let sink_position = self.node_input_positions[sink_id];
        let sink_state = self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or("terminal sink contract disappeared")?
            .queue_state();
        if sink_state.pending_frames > 0 {
            if let Err(cause) = self.service_terminal_sink_pending(sink_id) {
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkDrainError::ResetRequired(format!(
                    "terminal sink service failed: {cause:?}"
                )));
            }
            return Ok(SinkDrainResult {
                source_tail_frames_handed_to_sink: 0,
                complete: false,
            });
        }

        if self.terminal_sink_source_complete {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::Complete);
            return Ok(SinkDrainResult {
                source_tail_frames_handed_to_sink: 0,
                complete: true,
            });
        }

        let Some(source_id) = source_id else {
            self.terminal_sink_source_complete = true;
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::Complete);
            return Ok(SinkDrainResult {
                source_tail_frames_handed_to_sink: 0,
                complete: true,
            });
        };
        let source_index = self
            .chain_nodes
            .iter()
            .position(|&node_id| node_id == source_id)
            .ok_or("terminal tail source is missing from the chain")?;
        let source_rate = self.node_input_sample_rates[source_id];
        let source_position = self.node_input_positions[source_id];
        let maximum_frames = self.plugins[source_id]
            .as_ref()
            .ok_or("terminal tail source plugin is missing")?
            .drain_output_frames_max();
        let sink_context = ProcessContext::new(f64::from(sink_rate), maximum_frames)
            .with_sample_position(sink_position);
        let sink = self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or("terminal sink contract disappeared")?;
        let queue_state = sink.queue_state();
        if queue_state
            .pending_frames
            .saturating_add(queue_state.free_prepared_frames)
            > queue_state.capacity_frames
        {
            return Err("terminal sink queue state has invalid frame geometry".into());
        }
        sink.preflight_append(maximum_frames, &sink_context)
            .map_err(SinkDrainError::SinkPreflight)?;

        let channels = self.nodes[&source_id].output_channels();
        let maximum_samples = maximum_frames
            .checked_mul(channels)
            .ok_or("terminal tail scratch size overflow")?;
        let buffers = self
            .process_buffers
            .as_mut()
            .ok_or("terminal sink processing buffers are not prepared")?;
        if buffers.scratch_output.capacity() < maximum_samples {
            return Err(SinkDrainError::ScratchCapacity {
                required_samples: maximum_samples,
                available_samples: buffers.scratch_output.capacity(),
            });
        }
        buffers.scratch_output.resize(maximum_samples, 0.0);

        if self.drain_state.active_node != Some(source_id) {
            self.drain_state.active_node = Some(source_id);
            self.drain_state.prepared = false;
            self.drain_state.remaining_calls = None;
            self.drain_state.quota_grant_tail = None;
        }
        let source_context =
            ProcessContext::new(f64::from(source_rate), 0).with_sample_position(source_position);
        if !self.drain_state.prepared {
            let prepared = self.plugins[source_id]
                .as_mut()
                .unwrap()
                .begin_drain(&source_context);
            if let Err(error) = prepared {
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkDrainError::ResetRequired(format!(
                    "terminal tail preparation failed: {error}"
                )));
            }
            self.drain_state.prepared = true;
        }
        if self.drain_state.remaining_calls.is_none() {
            let plugin = self.plugins[source_id].as_ref().unwrap();
            let bound = plugin
                .drain_call_bound()
                .map_or(UNKNOWN_DRAIN_CALL_LIMIT, std::num::NonZeroU64::get);
            self.drain_state.quota_grant_tail = Some(plugin.tail_length());
            self.drain_state.remaining_calls = Some(bound);
        }
        let mut remaining = self.drain_state.remaining_calls.unwrap_or(0);
        if remaining == 0 {
            let plugin = self.plugins[source_id].as_ref().unwrap();
            let grant_tail = self
                .drain_state
                .quota_grant_tail
                .unwrap_or(TailLength::Unknown);
            if let Some((budget, current)) = drain_quota_refresh(plugin.as_ref(), grant_tail) {
                self.drain_state.quota_grant_tail = Some(current);
                self.drain_state.remaining_calls = Some(budget);
                remaining = budget;
            } else {
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkDrainError::ResetRequired(
                    "terminal tail source exhausted its drain call bound".into(),
                ));
            }
        }

        let result = self.plugins[source_id].as_mut().unwrap().drain(
            &mut self.process_buffers.as_mut().unwrap().scratch_output[..maximum_samples],
            &source_context,
        );
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkDrainError::ResetRequired(format!(
                    "terminal tail drain failed: {error}"
                )));
            }
        };
        self.drain_state.remaining_calls = Some(remaining - 1);
        if result.frames > maximum_frames {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
            return Err(SinkDrainError::ResetRequired(
                "terminal tail source exceeded its declared capacity".into(),
            ));
        }
        let tail_samples = result
            .frames
            .checked_mul(channels)
            .ok_or("terminal tail output size overflow")?;
        let mut handed_frames = 0;
        if result.frames > 0 {
            let next_sink_position = Self::advance_sample_position(sink_position, result.frames)
                .map_err(|error| {
                    self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                    SinkDrainError::ResetRequired(error)
                })?;
            let append_context = ProcessContext::new(f64::from(sink_rate), result.frames)
                .with_sample_position(sink_position);
            let append = self.plugins[sink_id]
                .as_mut()
                .unwrap()
                .terminal_sink_mut()
                .ok_or("terminal sink contract disappeared")?
                .append_preflighted(
                    &self.process_buffers.as_ref().unwrap().scratch_output[..tail_samples],
                    &append_context,
                );
            if let Err(SinkAppendFailure::ContractViolation) = append {
                self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::ResetRequired);
                return Err(SinkDrainError::ResetRequired(
                    "terminal sink append violated its preflight contract".into(),
                ));
            }
            self.node_input_positions[sink_id] = next_sink_position;
            handed_frames = result.frames;
        }
        if result.complete {
            self.terminal_sink_source_complete = true;
            self.drain_state.completed_prefix = source_index + 1;
            self.drain_state.active_node = None;
        }
        let pending = self.plugins[sink_id]
            .as_ref()
            .and_then(|plugin| plugin.terminal_sink())
            .ok_or("terminal sink contract disappeared")?
            .queue_state()
            .pending_frames;
        if self.terminal_sink_source_complete && pending == 0 {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::Complete);
            return Ok(SinkDrainResult {
                source_tail_frames_handed_to_sink: handed_frames,
                complete: true,
            });
        }
        Ok(SinkDrainResult {
            source_tail_frames_handed_to_sink: handed_frames,
            complete: false,
        })
    }

    pub(super) fn process_with_parameter_events(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        events: &mut Vec<ParameterEvent>,
        pad_short_blocks: bool,
    ) -> Result<usize, String> {
        if events.is_empty() {
            return self.process_block_without_parameter_events(
                input,
                output,
                self.automation_state.playback_position as u64,
                pad_short_blocks,
            );
        }

        if events.iter().any(|event| event.sample_offset > 0)
            && self.can_split_parameter_event_block(input, output)
        {
            return self.process_split_parameter_events(
                input,
                output,
                events,
                self.automation_state.playback_position as u64,
                pad_short_blocks,
            );
        }

        for event in events.drain(..) {
            let _ = self.apply_parameter_event(event);
        }
        self.process_block_without_parameter_events(
            input,
            output,
            self.automation_state.playback_position as u64,
            pad_short_blocks,
        )
    }

    fn process_with_parameter_events_to_sink(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        events: &mut Vec<ParameterEvent>,
        block_start_sample: u64,
    ) -> Result<usize, String> {
        if events.is_empty() {
            // Unpadded: the sink route demands exact identity frame counts,
            // so short production must fail loudly downstream, never pad.
            return self.process_block_without_parameter_events(
                input,
                output,
                block_start_sample,
                false,
            );
        }
        let input_channels = self.input_channels();
        if input_channels == 0 || !input.len().is_multiple_of(input_channels) {
            return Err("terminal sink input lost whole-frame geometry".into());
        }
        let frames = input.len() / input_channels;
        if output.len() != input.len() {
            return Err("terminal sink staging does not match the admitted input block".into());
        }
        events.sort_by_key(|event| event.sample_offset);
        events.reverse();

        let mut frame_cursor = 0;
        let mut processed_frames = 0;
        while events.last().is_some_and(|event| event.sample_offset == 0) {
            let event = events.pop().unwrap();
            let _ = self.apply_parameter_event(event);
            self.validate_terminal_sink_input_geometry(frames)
                .map_err(|error| format!("parameter update invalidated sink route: {error}"))?;
        }

        while frame_cursor < frames {
            let next_event_frame = events
                .last()
                .map_or(frames, |event| event.sample_offset.min(frames));
            if next_event_frame > frame_cursor {
                let in_start = frame_cursor * input_channels;
                let in_end = next_event_frame * input_channels;
                let segment_frames = self.process_block_without_parameter_events(
                    &input[in_start..in_end],
                    &mut output[in_start..in_end],
                    block_start_sample + frame_cursor as u64,
                    false,
                )?;
                processed_frames += segment_frames;
                frame_cursor = next_event_frame;
            }
            while events
                .last()
                .is_some_and(|event| event.sample_offset <= frame_cursor)
            {
                let event = events.pop().unwrap();
                let _ = self.apply_parameter_event(event);
                self.validate_terminal_sink_input_geometry(frames - frame_cursor)
                    .map_err(|error| format!("parameter update invalidated sink route: {error}"))?;
            }
        }
        while let Some(event) = events.pop() {
            let _ = self.apply_parameter_event(event);
            self.validate_terminal_sink_input_geometry(0)
                .map_err(|error| format!("parameter update invalidated sink route: {error}"))?;
        }
        Ok(processed_frames)
    }

    pub(super) fn can_split_parameter_event_block(&self, input: &[f32], output: &[f32]) -> bool {
        if !self.automation_state.automation.is_empty()
            || !self.cached_frames_identity
            || !self.cached_rate_identity
        {
            return false;
        }
        let input_channels = self.input_channels();
        if input_channels == 0 || !input.len().is_multiple_of(input_channels) {
            return false;
        }
        let frames = input.len() / input_channels;
        output.len() >= frames * self.output_channels()
    }

    pub(super) fn process_split_parameter_events(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        events: &mut Vec<ParameterEvent>,
        block_start_sample: u64,
        pad_short_blocks: bool,
    ) -> Result<usize, String> {
        let input_channels = self.input_channels();
        let output_channels = self.output_channels();
        let frames = input.len() / input_channels;
        events.sort_by_key(|event| event.sample_offset);
        events.reverse();

        let mut frame_cursor = 0;
        let mut processed_frames = 0;

        while events.last().is_some_and(|event| event.sample_offset == 0) {
            let event = events.pop().unwrap();
            let _ = self.apply_parameter_event(event);
        }

        while frame_cursor < frames {
            let next_event_frame = events
                .last()
                .map_or(frames, |event| event.sample_offset.min(frames));

            if next_event_frame > frame_cursor {
                let in_start = frame_cursor * input_channels;
                let in_end = next_event_frame * input_channels;
                let out_start = frame_cursor * output_channels;
                let out_end = next_event_frame * output_channels;
                let segment_frames = self.process_block_without_parameter_events(
                    &input[in_start..in_end],
                    &mut output[out_start..out_end],
                    block_start_sample + frame_cursor as u64,
                    pad_short_blocks,
                )?;
                processed_frames += segment_frames;
                frame_cursor = next_event_frame;
            }

            while events
                .last()
                .is_some_and(|event| event.sample_offset <= frame_cursor)
            {
                let event = events.pop().unwrap();
                let _ = self.apply_parameter_event(event);
            }
        }

        while let Some(event) = events.pop() {
            let _ = self.apply_parameter_event(event);
        }

        Ok(processed_frames)
    }

    pub(super) fn process_block_without_parameter_events(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        block_start_sample: u64,
        pad_short_blocks: bool,
    ) -> Result<usize, String> {
        if self.nodes.is_empty() {
            output.copy_from_slice(input);
            return Ok(input.len() / self.input_channels());
        }
        let nf = input.len() / self.input_channels();
        // Envelope sizing covers every residual state, so same-size
        // blocks never re-grow node buffers on straddle; unknown
        // envelopes keep live sizing exactly.
        let max_of = self
            .output_frames_envelope(nf)
            .unwrap_or_else(|| self.output_frames_for_input(nf));
        let out_ch = self.output_channels();
        self.apply_automation_for_block(nf);
        if self.terminal_sink_lifecycle.is_some() {
            self.validate_terminal_sink_input_geometry(nf)
                .map_err(|error| format!("automation invalidated sink route: {error}"))?;
        }
        let compiled_plan = std::mem::take(&mut self.compiled_plan);
        if self.terminal_sink_lifecycle.is_none()
            && let CompiledRenderPlan::LinearF32(ref plan) = compiled_plan
        {
            let result =
                self.process_compiled_linear_f32_plan(plan, input, output, block_start_sample);
            self.compiled_plan = compiled_plan;
            return result;
        }
        self.compiled_plan = compiled_plan;
        let stage_block_start_sample = block_start_sample;

        // Use BufferGuard to guarantee process_buffers are returned even on early ?-return
        let mut guard = BufferGuard::take(&mut self.process_buffers);
        let bufs = guard.get_mut();
        for nb in bufs.node_buffers.iter_mut().flatten() {
            nb.ensure_capacity(nf.max(max_of));
            nb.clear();
        }
        ensure_len(&mut bufs.scratch_input, input.len());
        let mut cf;

        for stage in &self.stages {
            if let Some(parallel_result) = Self::process_stage_parallel(
                self.terminal_sink_lifecycle.is_none()
                    && self.config.parallel_enabled
                    && self.cached_frames_identity
                    && self.cached_rate_identity
                    && !self.has_variable_frame_plugin,
                stage,
                input,
                self.config.sample_rate,
                nf,
                stage_block_start_sample,
                &mut self.plugins,
                &self.nodes,
                &self.predecessors,
                &self.is_input_node,
                &self.bypassed,
                &self.cached_parallel_node_costs,
                bufs,
            ) {
                parallel_result?;
                for &nid in &stage.nodes {
                    self.node_input_positions[nid] =
                        Self::advance_sample_position(block_start_sample, nf)?;
                }
                continue;
            }

            for &nid in &stage.nodes {
                let node = &self.nodes[&nid];
                let cf = Self::node_input_frames(nid, nf, &self.predecessors, &bufs.node_buffers)?;
                if self.is_input_node[nid] {
                    self.node_input_positions[nid] = block_start_sample;
                }
                let in_len = if self.is_input_node[nid] {
                    ensure_len(&mut bufs.scratch_input, input.len());
                    bufs.scratch_input[..input.len()].copy_from_slice(input);
                    input.len()
                } else {
                    let il = Self::merge_inputs_into(
                        node,
                        &self.predecessors,
                        &bufs.node_buffers,
                        cf,
                        &mut bufs.merge_buffer,
                        &mut bufs.channel_map_buffer,
                        &mut bufs.delay_scratch,
                        &mut bufs.compensation_delays,
                        &mut bufs.merge_queues,
                        &bufs.merge_queue_caps,
                    )
                    .map_err(|e| {
                        self.merge_overflow_poisoned = true;
                        crate::rate_limited_log!(
                            error,
                            5,
                            "host: merge_inputs_into failed for node {} '{}': {e}",
                            nid,
                            node.name
                        );
                        e
                    })?;
                    ensure_len(&mut bufs.scratch_input, il);
                    bufs.scratch_input[..il].copy_from_slice(&bufs.merge_buffer[..il]);
                    il
                };
                // Merge destinations consume the retention join, not the
                // buffer min-prefix: re-derive the frame count from the exact
                // merged samples.
                let cf = if !self.is_input_node[nid] && self.predecessors[nid].len() >= 2 {
                    in_len / node.input_channels().max(1)
                } else {
                    cf
                };
                let terminal_sink_id = self.chain_nodes.last().copied();
                if self.terminal_sink_lifecycle.is_some() && terminal_sink_id == Some(nid) {
                    if in_len != output.len() {
                        return Err(format!(
                            "terminal sink source produced {} samples; expected {}",
                            in_len,
                            output.len()
                        ));
                    }
                    output.copy_from_slice(&bufs.scratch_input[..in_len]);
                    self.node_input_positions[nid] =
                        Self::advance_sample_position(self.node_input_positions[nid], cf)?;
                    continue;
                }
                if self.bypassed[nid] {
                    // Bypassed: pass input directly to output buffer
                    bufs.node_buffers[nid]
                        .as_mut()
                        .unwrap()
                        .write(&bufs.scratch_input[..in_len]);
                    cf
                } else {
                    let p = self.plugins[nid].as_mut().unwrap();
                    let context =
                        ProcessContext::new(f64::from(self.node_input_sample_rates[nid]), cf)
                            .with_sample_position(self.node_input_positions[nid]);
                    let mof = Self::plugin_output_frames_for_input_isolated(
                        p.as_ref(),
                        nid,
                        &node.name,
                        cf,
                    );
                    let ol = mof * node.output_channels();
                    // Adapters own any input-stride work storage; Plugin output
                    // always uses the declared output channel count.
                    let process_output_len = ol;
                    ensure_len(&mut bufs.scratch_output, process_output_len);
                    if self.terminal_sink_lifecycle.is_some() {
                        self.terminal_sink_producer_started = true;
                    }
                    let out_frames = if self.terminal_sink_lifecycle.is_some() {
                        // An admitted terminal-sink block cannot use the
                        // ordinary passthrough fallback: that would hide a
                        // producer failure after it may have advanced state,
                        // leaving the caller unable to know whether retrying
                        // the input would duplicate samples.
                        match catch_unwind(AssertUnwindSafe(|| {
                            p.process(
                                &bufs.scratch_input[..in_len],
                                &mut bufs.scratch_output[..process_output_len],
                                &context,
                            )
                        })) {
                            Ok(Ok(frames)) if frames == cf => frames,
                            Ok(Ok(frames)) => {
                                return Err(format!(
                                    "terminal sink producer '{}' (node {nid}) returned {frames} frames; expected {cf}",
                                    node.name
                                ));
                            }
                            Ok(Err(error)) => {
                                return Err(format!(
                                    "terminal sink producer '{}' (node {nid}) process failed: {error}",
                                    node.name
                                ));
                            }
                            Err(payload) => {
                                return Err(format!(
                                    "terminal sink producer '{}' (node {nid}) panicked: {}",
                                    node.name,
                                    panic_payload_description(payload.as_ref())
                                ));
                            }
                        }
                    } else {
                        let outcome = Self::process_plugin_f32_isolated(
                            p.as_mut(),
                            node,
                            &bufs.scratch_input[..in_len],
                            &mut bufs.scratch_output[..process_output_len],
                            &context,
                        );
                        if let Some(returned) = outcome.over_reported {
                            return Err(format!(
                                "plugin '{}' process returned {} frames, exceeding its {}-frame declared output capacity",
                                node.name, returned, mof
                            ));
                        }
                        outcome.frames
                    };
                    bufs.node_buffers[nid]
                        .as_mut()
                        .unwrap()
                        .write(&bufs.scratch_output[..out_frames * node.output_channels()]);
                    out_frames
                };
                self.node_input_positions[nid] =
                    Self::advance_sample_position(self.node_input_positions[nid], cf)?;
            }
        }
        if self.terminal_sink_lifecycle.is_none() {
            cf = Self::terminal_output_frames(&self.output_nodes, &bufs.node_buffers)?;
            Self::collect_output_from_buffers(
                &self.output_nodes,
                &mut bufs.node_buffers,
                &mut bufs.compensation_delays,
                output,
                cf,
            )
            .map_err(|e| {
                crate::rate_limited_log!(error, 5, "host: collect_output_from_buffers failed: {e}");
                e
            })?;
            if pad_short_blocks
                && cf < nf
                && self.has_variable_frame_plugin
                && self.cached_rate_identity
            {
                output[cf * out_ch..].fill(0.0);
                cf = nf;
            }
        } else {
            // The explicit sink route is admitted only for identity frame
            // geometry, so consumed input frames are the host's frame result.
            cf = nf;
        }
        // Advance playback position for automation
        self.automation_state.playback_position = self
            .automation_state
            .playback_position
            .checked_add(nf)
            .ok_or("Graph playback position exceeds addressable frames")?;

        // BufferGuard's Drop impl returns bufs to self.process_buffers when
        // `guard` falls out of scope here.
        Ok(cf)
    }

    pub(super) fn process_compiled_linear_f32_plan(
        &mut self,
        plan: &CompiledLinearPlan,
        input: &[f32],
        output: &mut [f32],
        block_start_sample: u64,
    ) -> Result<usize, String> {
        let input_channels = plan.input_channels;
        let mut guard = BufferGuard::take(&mut self.process_buffers);
        let bufs = guard.get_mut();

        let mut current_source = CompiledLinearSource::ExternalInput;
        let mut current_len = input.len();
        let mut current_frames = input.len() / input_channels;
        let mut active_gain_region: Option<(usize, f32)> = None;

        let mut idx = 0;
        while idx < plan.ops.len() {
            if active_gain_region.is_none() {
                active_gain_region =
                    Self::compiled_linear_gain_region(plan, idx, &self.plugins, &self.bypassed);
            }

            if active_gain_region.is_some()
                && Self::compiled_op_static_gain(plan, idx, &self.plugins, &self.bypassed).is_some()
            {
                idx += 1;
                if let Some((region_end, gain)) = active_gain_region
                    && idx == region_end
                {
                    current_source = Self::apply_compiled_region_gain(
                        bufs,
                        input,
                        output,
                        current_source,
                        current_len,
                        gain,
                        region_end == plan.ops.len(),
                    )?;
                    active_gain_region = None;
                }
                continue;
            }

            let op = &plan.ops[idx];
            let nid = op.node_id;
            let node = self
                .nodes
                .get(&nid)
                .ok_or_else(|| format!("Missing node {nid} during compiled f32 processing"))?;
            let region_writes_final_gain = active_gain_region
                .is_some_and(|(region_end, gain)| region_end == plan.ops.len() && gain != 1.0);
            let is_last = idx + 1 == plan.ops.len() && !region_writes_final_gain;
            if !is_last
                && op.kind == CompiledOpKind::AnalyzerTap
                && !self.bypassed.get(nid).copied().unwrap_or(false)
            {
                let context =
                    ProcessContext::new(f64::from(self.config.sample_rate), current_frames)
                        .with_sample_position(block_start_sample);
                let tap_input = match current_source {
                    CompiledLinearSource::ExternalInput => &input[..current_len],
                    CompiledLinearSource::ScratchInput => &bufs.scratch_input[..current_len],
                    CompiledLinearSource::ScratchOutput => &bufs.scratch_output[..current_len],
                };
                if Self::process_analyzer_tap_f32_isolated(
                    self.plugins[nid].as_mut().unwrap().as_mut(),
                    node,
                    tap_input,
                    &context,
                ) {
                    idx += 1;
                    continue;
                }
            }
            let output_frames = if self.bypassed[nid] {
                current_frames
            } else {
                let plugin = self.plugins[nid].as_ref().unwrap();
                Self::plugin_output_frames_for_input_isolated(
                    plugin.as_ref(),
                    nid,
                    &node.name,
                    current_frames,
                )
            };
            let output_len = output_frames * node.output_channels();

            let frames = match (is_last, current_source) {
                (true, CompiledLinearSource::ExternalInput) => {
                    if output.len() < output_len {
                        // Loose live declaration (or probe coincidence): the
                        // caller staged from the host query, which understates
                        // this final op's need. Run into grown scratch and
                        // commit production exactly — staged-commit parity.
                        // The over-report guard inside process_f32_node fires
                        // first on genuine lies; production genuinely
                        // exceeding caller capacity stays a loud error below,
                        // never silently discarded.
                        ensure_len(&mut bufs.scratch_output, output_len);
                        let frames = Self::process_f32_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            op.kind,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &input[..current_len],
                            &mut bufs.scratch_output[..output_len],
                            self.config.sample_rate,
                            block_start_sample,
                            current_frames,
                        )?;
                        let produced = frames.saturating_mul(node.output_channels());
                        if produced > output.len() {
                            return Err(format!(
                                "f32 output too small: need {produced} samples, got {}",
                                output.len()
                            ));
                        }
                        output[..produced].copy_from_slice(&bufs.scratch_output[..produced]);
                        frames
                    } else {
                        Self::process_f32_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            op.kind,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &input[..current_len],
                            &mut output[..output_len],
                            self.config.sample_rate,
                            block_start_sample,
                            current_frames,
                        )?
                    }
                }
                (true, CompiledLinearSource::ScratchInput) => {
                    if output.len() < output_len {
                        // Loose live declaration (or probe coincidence): the
                        // caller staged from the host query, which understates
                        // this final op's need. Run into grown scratch and
                        // commit production exactly — staged-commit parity.
                        // The over-report guard inside process_f32_node fires
                        // first on genuine lies; production genuinely
                        // exceeding caller capacity stays a loud error below,
                        // never silently discarded.
                        ensure_len(&mut bufs.scratch_output, output_len);
                        let frames = Self::process_f32_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            op.kind,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &bufs.scratch_input[..current_len],
                            &mut bufs.scratch_output[..output_len],
                            self.config.sample_rate,
                            block_start_sample,
                            current_frames,
                        )?;
                        let produced = frames.saturating_mul(node.output_channels());
                        if produced > output.len() {
                            return Err(format!(
                                "f32 output too small: need {produced} samples, got {}",
                                output.len()
                            ));
                        }
                        output[..produced].copy_from_slice(&bufs.scratch_output[..produced]);
                        frames
                    } else {
                        Self::process_f32_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            op.kind,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &bufs.scratch_input[..current_len],
                            &mut output[..output_len],
                            self.config.sample_rate,
                            block_start_sample,
                            current_frames,
                        )?
                    }
                }
                (true, CompiledLinearSource::ScratchOutput) => {
                    if output.len() < output_len {
                        // Loose live declaration (or probe coincidence): the
                        // caller staged from the host query, which understates
                        // this final op's need. Run into the grown sibling
                        // scratch (input lives in scratch_output) and commit
                        // production exactly — staged-commit parity. The
                        // over-report guard inside process_f32_node fires
                        // first on genuine lies; production genuinely
                        // exceeding caller capacity stays a loud error below,
                        // never silently discarded.
                        ensure_len(&mut bufs.scratch_input, output_len);
                        let frames = Self::process_f32_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            op.kind,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &bufs.scratch_output[..current_len],
                            &mut bufs.scratch_input[..output_len],
                            self.config.sample_rate,
                            block_start_sample,
                            current_frames,
                        )?;
                        let produced = frames.saturating_mul(node.output_channels());
                        if produced > output.len() {
                            return Err(format!(
                                "f32 output too small: need {produced} samples, got {}",
                                output.len()
                            ));
                        }
                        output[..produced].copy_from_slice(&bufs.scratch_input[..produced]);
                        frames
                    } else {
                        Self::process_f32_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            op.kind,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &bufs.scratch_output[..current_len],
                            &mut output[..output_len],
                            self.config.sample_rate,
                            block_start_sample,
                            current_frames,
                        )?
                    }
                }
                (false, CompiledLinearSource::ExternalInput) => {
                    ensure_len(&mut bufs.scratch_output, output_len);
                    let frames = Self::process_f32_node(
                        self.plugins[nid].as_mut().unwrap().as_mut(),
                        node,
                        op.kind,
                        self.bypassed.get(nid).copied().unwrap_or(false),
                        &input[..current_len],
                        &mut bufs.scratch_output[..output_len],
                        self.config.sample_rate,
                        block_start_sample,
                        current_frames,
                    )?;
                    current_source = CompiledLinearSource::ScratchOutput;
                    frames
                }
                (false, CompiledLinearSource::ScratchInput) => {
                    ensure_len(&mut bufs.scratch_output, output_len);
                    let frames = Self::process_f32_node(
                        self.plugins[nid].as_mut().unwrap().as_mut(),
                        node,
                        op.kind,
                        self.bypassed.get(nid).copied().unwrap_or(false),
                        &bufs.scratch_input[..current_len],
                        &mut bufs.scratch_output[..output_len],
                        self.config.sample_rate,
                        block_start_sample,
                        current_frames,
                    )?;
                    current_source = CompiledLinearSource::ScratchOutput;
                    frames
                }
                (false, CompiledLinearSource::ScratchOutput) => {
                    ensure_len(&mut bufs.scratch_input, output_len);
                    let frames = Self::process_f32_node(
                        self.plugins[nid].as_mut().unwrap().as_mut(),
                        node,
                        op.kind,
                        self.bypassed.get(nid).copied().unwrap_or(false),
                        &bufs.scratch_output[..current_len],
                        &mut bufs.scratch_input[..output_len],
                        self.config.sample_rate,
                        block_start_sample,
                        current_frames,
                    )?;
                    current_source = CompiledLinearSource::ScratchInput;
                    frames
                }
            };

            current_frames = frames;
            current_len = frames * node.output_channels();
            idx += 1;
            if let Some((region_end, gain)) = active_gain_region
                && idx == region_end
            {
                current_source = Self::apply_compiled_region_gain(
                    bufs,
                    input,
                    output,
                    current_source,
                    current_len,
                    gain,
                    region_end == plan.ops.len(),
                )?;
                active_gain_region = None;
            }
        }

        for op in &plan.ops {
            self.node_input_positions[op.node_id] =
                Self::advance_sample_position(block_start_sample, input.len() / input_channels)?;
        }
        self.automation_state.playback_position = self
            .automation_state
            .playback_position
            .checked_add(input.len() / input_channels)
            .ok_or("Graph playback position exceeds addressable frames")?;
        Ok(current_frames)
    }

    fn compiled_linear_gain_region(
        plan: &CompiledLinearPlan,
        start: usize,
        plugins: &[Option<Box<dyn Plugin>>],
        bypassed: &[bool],
    ) -> Option<(usize, f32)> {
        let first = plan.ops.get(start)?;
        if first.barrier.is_some()
            || first.metadata_boundary
            || !first.linear
            || !first.time_invariant_for_block
            || !first.can_absorb_input_gain
            || !first.can_absorb_output_gain
            || first.input_channels != first.output_channels
        {
            return None;
        }

        let mut end = start;
        let mut gain = 1.0_f32;
        let mut saw_static_gain = false;
        while let Some(op) = plan.ops.get(end) {
            if op.barrier.is_some()
                || op.metadata_boundary
                || !op.linear
                || !op.time_invariant_for_block
                || !op.can_absorb_input_gain
                || !op.can_absorb_output_gain
                || op.input_channels != op.output_channels
            {
                break;
            }
            if let Some(op_gain) = Self::compiled_op_static_gain(plan, end, plugins, bypassed) {
                saw_static_gain = true;
                gain *= op_gain;
            } else if op.kind == CompiledOpKind::ApplyGain {
                break;
            }
            end += 1;
        }
        (saw_static_gain && end > start).then_some((end, gain))
    }

    fn compiled_op_static_gain(
        plan: &CompiledLinearPlan,
        idx: usize,
        plugins: &[Option<Box<dyn Plugin>>],
        bypassed: &[bool],
    ) -> Option<f32> {
        let op = plan.ops.get(idx)?;
        if op.kind != CompiledOpKind::ApplyGain {
            return None;
        }
        let nid = op.node_id;
        if bypassed.get(nid).copied().unwrap_or(false) {
            return Some(1.0);
        }
        plugins
            .get(nid)
            .and_then(|plugin| plugin.as_ref())
            .and_then(|plugin| plugin.compile_metadata().static_gain)
    }

    fn apply_compiled_region_gain(
        bufs: &mut ProcessBuffers<f32>,
        input: &[f32],
        output: &mut [f32],
        current_source: CompiledLinearSource,
        current_len: usize,
        gain: f32,
        is_final: bool,
    ) -> Result<CompiledLinearSource, String> {
        if is_final {
            if output.len() < current_len {
                return Err(format!(
                    "f32 output too small: need {current_len} samples, got {}",
                    output.len()
                ));
            }
            match current_source {
                CompiledLinearSource::ExternalInput => {
                    Self::write_scaled_f32(&input[..current_len], &mut output[..current_len], gain)
                }
                CompiledLinearSource::ScratchInput => Self::write_scaled_f32(
                    &bufs.scratch_input[..current_len],
                    &mut output[..current_len],
                    gain,
                ),
                CompiledLinearSource::ScratchOutput => Self::write_scaled_f32(
                    &bufs.scratch_output[..current_len],
                    &mut output[..current_len],
                    gain,
                ),
            }
            return Ok(current_source);
        }

        match current_source {
            CompiledLinearSource::ExternalInput => {
                ensure_len(&mut bufs.scratch_output, current_len);
                Self::write_scaled_f32(
                    &input[..current_len],
                    &mut bufs.scratch_output[..current_len],
                    gain,
                );
                Ok(CompiledLinearSource::ScratchOutput)
            }
            CompiledLinearSource::ScratchInput => {
                ensure_len(&mut bufs.scratch_output, current_len);
                Self::write_scaled_f32(
                    &bufs.scratch_input[..current_len],
                    &mut bufs.scratch_output[..current_len],
                    gain,
                );
                Ok(CompiledLinearSource::ScratchOutput)
            }
            CompiledLinearSource::ScratchOutput => {
                ensure_len(&mut bufs.scratch_input, current_len);
                Self::write_scaled_f32(
                    &bufs.scratch_output[..current_len],
                    &mut bufs.scratch_input[..current_len],
                    gain,
                );
                Ok(CompiledLinearSource::ScratchInput)
            }
        }
    }

    fn write_scaled_f32(input: &[f32], output: &mut [f32], gain: f32) {
        debug_assert!(output.len() >= input.len());
        if (gain - 1.0).abs() <= f32::EPSILON {
            output[..input.len()].copy_from_slice(input);
        } else {
            for (dst, src) in output.iter_mut().zip(input.iter()) {
                *dst = *src * gain;
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "plugin process signature mirrors the underlying audio graph API"
    )]
    pub(super) fn process_f32_node(
        plugin: &mut dyn Plugin,
        node: &GraphNode,
        op_kind: CompiledOpKind,
        bypassed: bool,
        input: &[f32],
        output: &mut [f32],
        sample_rate: f64,
        sample_position: u64,
        num_frames: usize,
    ) -> Result<usize, String> {
        if bypassed {
            output[..input.len()].copy_from_slice(input);
            return Ok(num_frames);
        }
        let context =
            ProcessContext::new(sample_rate, num_frames).with_sample_position(sample_position);
        if let Some(frames) = plugin_compiled_op(op_kind).and_then(|compiled_op| {
            Self::process_compiled_plugin_f32_isolated(
                plugin,
                node,
                compiled_op,
                input,
                output,
                &context,
            )
        }) {
            return Ok(frames);
        }
        let outcome =
            Self::process_plugin_f32_isolated(plugin, node, input, &mut *output, &context);
        if let Some(returned) = outcome.over_reported {
            let staged = output.len() / node.output_channels().max(1);
            return Err(format!(
                "plugin '{}' process returned {} frames, exceeding its {}-frame declared output capacity",
                node.name, returned, staged
            ));
        }
        Ok(outcome.frames)
    }

    pub(super) fn apply_automation_for_block(&mut self, nf: usize) {
        // Apply automation: evaluate curves at current position and set parameters.
        // `eval_curve` interprets (sample, num_frames) as a position within a window,
        // so we use each automation's relative position and advance it by nf each call.
        if self.automation_state.automation.is_empty() {
            return;
        }

        self.automation_scratch.clear();
        for (idx, slot) in self.automation_state.automation.iter().enumerate() {
            let auto = &slot.automation;
            if let Some(curve) = auto.curve.as_ref() {
                let total_frames = match curve {
                    crate::automation::AutomationCurve::Step {
                        values,
                        samples_per_step,
                    } => {
                        if *samples_per_step > 0 {
                            values.len() * *samples_per_step
                        } else {
                            values.len() * nf
                        }
                    }
                    crate::automation::AutomationCurve::Linear { values } => {
                        values.len().max(1) * nf
                    }
                    crate::automation::AutomationCurve::Bezier { points } => {
                        points.last().map_or(nf, |p| p.position.max(nf))
                    }
                    crate::automation::AutomationCurve::Exponential { values, .. } => {
                        values.len().max(1) * nf
                    }
                };
                let pos = auto.position.min(total_frames.saturating_sub(1));
                let val = automation_utils::eval_curve(curve, pos, total_frames);
                self.automation_scratch.push((idx, val));
            }
        }
        for i in 0..self.automation_scratch.len() {
            let (idx, val) = self.automation_scratch[i];
            let slot = &mut self.automation_state.automation[idx];
            if let Some(p) = self.plugins[slot.node_id].as_mut() {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    p.set_parameter(slot.param_id.clone(), ParameterValue::Float(val))
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        crate::rate_limited_log!(
                            warn,
                            5,
                            "host: automation parameter update failed for node {}: {err}",
                            slot.node_id
                        );
                    }
                    Err(payload) => {
                        let reason = panic_payload_description(payload.as_ref());
                        crate::rate_limited_log!(
                            error,
                            5,
                            "host: plugin at node {} panicked during automation parameter update: {}",
                            slot.node_id,
                            reason
                        );
                    }
                }
            }
            slot.automation.last_value = val;
            slot.automation.position += nf;
        }
    }

    /// Process an f64 buffer through the host.
    ///
    /// Native f64 simple-chain and DAG paths are used when every active plugin
    /// declares `supports_f64()`. Graphs containing f32-only plugins use a
    /// scratch-backed f32 compatibility bridge.
    pub fn process_f64(&mut self, input: &[f64], output: &mut [f64]) -> Result<usize, String> {
        if self.merge_overflow_poisoned {
            return Err(
                "host merge retention overflowed; reset the host before further processing".into(),
            );
        }
        if self.terminal_sink_lifecycle.is_some() || self.has_advertised_terminal_sink() {
            return Err(
                "terminal sink graphs do not support process_f64; use process_to_sink".into(),
            );
        }
        self.drain_graph_mutations()?;
        if !self.built {
            self.build()?;
        }
        self.preflight_process_horizon(input.len() / self.input_channels())?;
        if let Some((index, sample)) = input
            .iter()
            .copied()
            .enumerate()
            .find(|(_, sample)| !sample.is_finite())
        {
            output.fill(0.0);
            return Err(format!(
                "host input contains non-finite sample at index {index}: {sample}"
            ));
        }
        let mut events = std::mem::take(&mut self.queues.parameter_event_scratch);
        self.drain_parameter_events_into(&mut events);
        let result = self.process_f64_with_parameter_events(input, output, &mut events);
        self.queues.parameter_event_scratch = events;
        if result.is_ok() && !input.is_empty() {
            self.drain_state = DrainState::default();
            self.graph_drain_state.reset_dynamic();
        }
        result
    }

    pub(super) fn process_f64_with_parameter_events(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        events: &mut Vec<ParameterEvent>,
    ) -> Result<usize, String> {
        if events.is_empty() {
            return self.process_f64_without_parameter_events(
                input,
                output,
                self.automation_state.playback_position as u64,
            );
        }

        if events.iter().any(|event| event.sample_offset > 0)
            && self.can_split_parameter_event_block_f64(input, output)
        {
            return self.process_f64_split_parameter_events(
                input,
                output,
                events,
                self.automation_state.playback_position as u64,
            );
        }

        for event in events.drain(..) {
            let _ = self.apply_parameter_event(event);
        }
        self.process_f64_without_parameter_events(
            input,
            output,
            self.automation_state.playback_position as u64,
        )
    }

    pub(super) fn can_split_parameter_event_block_f64(
        &self,
        input: &[f64],
        output: &[f64],
    ) -> bool {
        if !self.automation_state.automation.is_empty()
            || !self.cached_frames_identity
            || !self.cached_rate_identity
        {
            return false;
        }
        let input_channels = self.input_channels();
        if input_channels == 0 || !input.len().is_multiple_of(input_channels) {
            return false;
        }
        let frames = input.len() / input_channels;
        output.len() >= frames * self.output_channels()
    }

    pub(super) fn process_f64_split_parameter_events(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        events: &mut Vec<ParameterEvent>,
        block_start_sample: u64,
    ) -> Result<usize, String> {
        let input_channels = self.input_channels();
        let output_channels = self.output_channels();
        let frames = input.len() / input_channels;
        events.sort_by_key(|event| event.sample_offset);
        events.reverse();

        let mut frame_cursor = 0;
        let mut processed_frames = 0;

        while events.last().is_some_and(|event| event.sample_offset == 0) {
            let event = events.pop().unwrap();
            let _ = self.apply_parameter_event(event);
        }

        while frame_cursor < frames {
            let next_event_frame = events
                .last()
                .map_or(frames, |event| event.sample_offset.min(frames));

            if next_event_frame > frame_cursor {
                let in_start = frame_cursor * input_channels;
                let in_end = next_event_frame * input_channels;
                let out_start = frame_cursor * output_channels;
                let out_end = next_event_frame * output_channels;
                let segment_frames = self.process_f64_without_parameter_events(
                    &input[in_start..in_end],
                    &mut output[out_start..out_end],
                    block_start_sample + frame_cursor as u64,
                )?;
                processed_frames += segment_frames;
                frame_cursor = next_event_frame;
            }

            while events
                .last()
                .is_some_and(|event| event.sample_offset <= frame_cursor)
            {
                let event = events.pop().unwrap();
                let _ = self.apply_parameter_event(event);
            }
        }

        while let Some(event) = events.pop() {
            let _ = self.apply_parameter_event(event);
        }

        Ok(processed_frames)
    }

    pub(super) fn process_f64_without_parameter_events(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        block_start_sample: u64,
    ) -> Result<usize, String> {
        if self.nodes.is_empty() {
            output.copy_from_slice(input);
            return Ok(input.len() / self.input_channels());
        }
        if self.can_process_f64_chain_native() {
            return self.process_f64_chain_native(input, output, block_start_sample);
        }
        if self.can_process_f64_graph_native() {
            return self.process_f64_graph_native(input, output, block_start_sample);
        }
        let mut input_scratch = std::mem::take(&mut self.config.f64_input_scratch);
        let mut output_scratch = std::mem::take(&mut self.config.f64_output_scratch);

        ensure_len(&mut input_scratch, input.len());
        for (dst, &src) in input_scratch[..input.len()].iter_mut().zip(input.iter()) {
            *dst = src as f32;
        }

        ensure_len(&mut output_scratch, output.len());
        let in_len = input.len();
        let out_len = output.len();
        let result = self.process_block_without_parameter_events(
            &input_scratch[..in_len],
            &mut output_scratch[..out_len],
            block_start_sample,
            true,
        );
        let frames = match result {
            Ok(frames) => frames,
            Err(err) => {
                self.config.f64_input_scratch = input_scratch;
                self.config.f64_output_scratch = output_scratch;
                return Err(err);
            }
        };
        for (dst, &src) in output.iter_mut().zip(output_scratch[..out_len].iter()) {
            *dst = src as f64;
        }

        self.config.f64_input_scratch = input_scratch;
        self.config.f64_output_scratch = output_scratch;
        Ok(frames)
    }

    pub(super) fn can_process_f64_graph_native(&self) -> bool {
        !self.nodes.is_empty()
            && self.nodes.keys().copied().all(|nid| {
                self.bypassed.get(nid).copied().unwrap_or(false)
                    || self.plugins[nid].as_ref().is_some_and(|plugin| {
                        let node = &self.nodes[&nid];
                        Self::plugin_supports_f64_isolated(plugin.as_ref(), nid, &node.name)
                    })
            })
    }

    pub(super) fn can_process_f64_chain_native(&self) -> bool {
        if self.chain_nodes.is_empty() || self.chain_nodes.len() != self.nodes.len() {
            return false;
        }
        if self.edges.len() != self.chain_nodes.len().saturating_sub(1) {
            return false;
        }
        for pair in self.chain_nodes.windows(2) {
            let from = pair[0];
            let to = pair[1];
            let Some(edge) = self
                .edges
                .iter()
                .find(|e| e.from_node == from && e.to_node == to)
            else {
                return false;
            };
            if edge.edge_type != EdgeType::Audio
                || edge.channel_map.is_some()
                || edge.destination_offset != 0
            {
                return false;
            }
        }
        self.chain_nodes.iter().all(|&nid| {
            self.bypassed.get(nid).copied().unwrap_or(false)
                || self.plugins[nid].as_ref().is_some_and(|plugin| {
                    let node = &self.nodes[&nid];
                    Self::plugin_supports_f64_isolated(plugin.as_ref(), nid, &node.name)
                })
        })
    }

    pub(super) fn process_f64_graph_native(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        block_start_sample: u64,
    ) -> Result<usize, String> {
        let nf = input.len() / self.input_channels();
        // Envelope sizing covers every residual state, so same-size
        // blocks never re-grow node buffers on straddle; unknown
        // envelopes keep live sizing exactly.
        let max_of = self
            .output_frames_envelope(nf)
            .unwrap_or_else(|| self.output_frames_for_input(nf));
        let out_ch = self.output_channels();
        self.apply_automation_for_block(nf);

        let mut guard = BufferGuard::take(&mut self.process_buffers_f64);
        let bufs = guard.get_mut();
        for nb in bufs.node_buffers.iter_mut().flatten() {
            nb.ensure_capacity(nf.max(max_of));
            nb.clear();
        }
        ensure_len(&mut bufs.scratch_input, input.len());
        let mut cf;

        for stage in &self.stages {
            for &nid in &stage.nodes {
                let node = &self.nodes[&nid];
                let cf = Self::node_input_frames(nid, nf, &self.predecessors, &bufs.node_buffers)?;
                if self.is_input_node[nid] {
                    self.node_input_positions[nid] = block_start_sample;
                }
                let in_len = if self.is_input_node[nid] {
                    ensure_len(&mut bufs.scratch_input, input.len());
                    bufs.scratch_input[..input.len()].copy_from_slice(input);
                    input.len()
                } else {
                    let il = Self::merge_inputs_into(
                        node,
                        &self.predecessors,
                        &bufs.node_buffers,
                        cf,
                        &mut bufs.merge_buffer,
                        &mut bufs.channel_map_buffer,
                        &mut bufs.delay_scratch,
                        &mut bufs.compensation_delays,
                        &mut bufs.merge_queues,
                        &bufs.merge_queue_caps,
                    )
                    .map_err(|e| {
                        self.merge_overflow_poisoned = true;
                        crate::rate_limited_log!(
                            error,
                            5,
                            "host: f64 merge_inputs_into failed for node {} '{}': {e}",
                            nid,
                            node.name
                        );
                        e
                    })?;
                    ensure_len(&mut bufs.scratch_input, il);
                    bufs.scratch_input[..il].copy_from_slice(&bufs.merge_buffer[..il]);
                    il
                };
                // Merge destinations consume the retention join, not the
                // buffer min-prefix: re-derive the frame count from the exact
                // merged samples.
                let cf = if !self.is_input_node[nid] && self.predecessors[nid].len() >= 2 {
                    in_len / node.input_channels().max(1)
                } else {
                    cf
                };
                if self.bypassed[nid] {
                    bufs.node_buffers[nid]
                        .as_mut()
                        .unwrap()
                        .write(&bufs.scratch_input[..in_len]);
                    cf
                } else {
                    let plugin = self.plugins[nid].as_mut().unwrap();
                    let context =
                        ProcessContext::new(f64::from(self.node_input_sample_rates[nid]), cf)
                            .with_sample_position(self.node_input_positions[nid]);
                    let max_output_frames = Self::plugin_output_frames_for_input_isolated(
                        plugin.as_ref(),
                        nid,
                        &node.name,
                        cf,
                    );
                    let output_len = max_output_frames * node.output_channels();
                    // Adapters own any input-stride work storage; Plugin output
                    // always uses the declared output channel count.
                    let process_output_len = output_len;
                    ensure_len(&mut bufs.scratch_output, process_output_len);
                    let outcome = Self::process_plugin_f64_isolated(
                        plugin.as_mut(),
                        node,
                        &bufs.scratch_input[..in_len],
                        &mut bufs.scratch_output[..process_output_len],
                        &context,
                    );
                    if let Some(returned) = outcome.over_reported {
                        return Err(format!(
                            "plugin '{}' process returned {} frames, exceeding its {}-frame declared output capacity",
                            node.name, returned, max_output_frames
                        ));
                    }
                    let frames = outcome.frames;
                    bufs.node_buffers[nid]
                        .as_mut()
                        .unwrap()
                        .write(&bufs.scratch_output[..frames * node.output_channels()]);
                    frames
                };
                self.node_input_positions[nid] =
                    Self::advance_sample_position(self.node_input_positions[nid], cf)?;
            }
        }

        cf = Self::terminal_output_frames(&self.output_nodes, &bufs.node_buffers)?;
        Self::collect_output_from_buffers(
            &self.output_nodes,
            &mut bufs.node_buffers,
            &mut bufs.compensation_delays,
            output,
            cf,
        )
        .map_err(|e| {
            crate::rate_limited_log!(
                error,
                5,
                "host: f64 collect_output_from_buffers failed: {e}"
            );
            e
        })?;
        if cf < nf && self.has_variable_frame_plugin && self.cached_rate_identity {
            output[cf * out_ch..].fill(0.0);
            cf = nf;
        }
        self.automation_state.playback_position = self
            .automation_state
            .playback_position
            .checked_add(nf)
            .ok_or("Graph playback position exceeds addressable frames")?;

        Ok(cf)
    }

    pub(super) fn process_f64_chain_native(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        block_start_sample: u64,
    ) -> Result<usize, String> {
        let mut scratch_a = std::mem::take(&mut self.config.f64_chain_scratch);
        let mut scratch_b = std::mem::take(&mut self.config.f64_chain_scratch_alt);

        ensure_len(&mut scratch_a, input.len());
        scratch_a[..input.len()].copy_from_slice(input);

        let mut current_in_a = true;
        let mut current_len = input.len();
        let mut current_frames = input.len() / self.input_channels();
        let mut current_rate = self.config.sample_rate;

        for idx in 0..self.chain_nodes.len() {
            let nid = self.chain_nodes[idx];
            if idx == 0 {
                self.node_input_positions[nid] = block_start_sample;
            }
            let node = self
                .nodes
                .get(&nid)
                .ok_or_else(|| format!("Missing node {nid} during f64 processing"))?;
            let is_last = idx + 1 == self.chain_nodes.len();
            let output_frames = if self.bypassed[nid] {
                current_frames
            } else {
                let plugin = self.plugins[nid].as_ref().unwrap();
                Self::plugin_output_frames_for_input_isolated(
                    plugin.as_ref(),
                    nid,
                    &node.name,
                    current_frames,
                )
            };
            let output_len = output_frames * node.output_channels();

            let frames = if is_last {
                if output.len() < output_len {
                    // Loose live declaration (or probe coincidence): the
                    // caller staged from the host query, which understates
                    // this final op's need. Run into the grown sibling
                    // scratch and commit production exactly. The over-report
                    // guard inside process_f64_node fires first on genuine
                    // lies; production genuinely exceeding caller capacity
                    // stays a loud error below, never silently discarded.
                    let frames = if current_in_a {
                        ensure_len(&mut scratch_b, output_len);
                        Self::process_f64_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &scratch_a[..current_len],
                            &mut scratch_b[..output_len],
                            current_rate,
                            self.node_input_positions[nid],
                            current_frames,
                        )?
                    } else {
                        ensure_len(&mut scratch_a, output_len);
                        Self::process_f64_node(
                            self.plugins[nid].as_mut().unwrap().as_mut(),
                            node,
                            self.bypassed.get(nid).copied().unwrap_or(false),
                            &scratch_b[..current_len],
                            &mut scratch_a[..output_len],
                            current_rate,
                            self.node_input_positions[nid],
                            current_frames,
                        )?
                    };
                    let produced = frames.saturating_mul(node.output_channels());
                    if produced > output.len() {
                        self.config.f64_chain_scratch = scratch_a;
                        self.config.f64_chain_scratch_alt = scratch_b;
                        return Err(format!(
                            "f64 output too small: need {produced} samples, got {}",
                            output.len()
                        ));
                    }
                    if current_in_a {
                        output[..produced].copy_from_slice(&scratch_b[..produced]);
                    } else {
                        output[..produced].copy_from_slice(&scratch_a[..produced]);
                    }
                    frames
                } else if current_in_a {
                    Self::process_f64_node(
                        self.plugins[nid].as_mut().unwrap().as_mut(),
                        node,
                        self.bypassed.get(nid).copied().unwrap_or(false),
                        &scratch_a[..current_len],
                        &mut output[..output_len],
                        current_rate,
                        self.node_input_positions[nid],
                        current_frames,
                    )?
                } else {
                    Self::process_f64_node(
                        self.plugins[nid].as_mut().unwrap().as_mut(),
                        node,
                        self.bypassed.get(nid).copied().unwrap_or(false),
                        &scratch_b[..current_len],
                        &mut output[..output_len],
                        current_rate,
                        self.node_input_positions[nid],
                        current_frames,
                    )?
                }
            } else if current_in_a {
                ensure_len(&mut scratch_b, output_len);
                let frames = Self::process_f64_node(
                    self.plugins[nid].as_mut().unwrap().as_mut(),
                    node,
                    self.bypassed.get(nid).copied().unwrap_or(false),
                    &scratch_a[..current_len],
                    &mut scratch_b[..output_len],
                    current_rate,
                    self.node_input_positions[nid],
                    current_frames,
                )?;
                current_in_a = false;
                frames
            } else {
                ensure_len(&mut scratch_a, output_len);
                let frames = Self::process_f64_node(
                    self.plugins[nid].as_mut().unwrap().as_mut(),
                    node,
                    self.bypassed.get(nid).copied().unwrap_or(false),
                    &scratch_b[..current_len],
                    &mut scratch_a[..output_len],
                    current_rate,
                    self.node_input_positions[nid],
                    current_frames,
                )?;
                current_in_a = true;
                frames
            };

            self.node_input_positions[nid] =
                Self::advance_sample_position(self.node_input_positions[nid], current_frames)?;
            current_frames = frames;
            current_len = frames * node.output_channels();
            current_rate = self.node_output_sample_rates[nid];
        }

        self.config.f64_chain_scratch = scratch_a;
        self.config.f64_chain_scratch_alt = scratch_b;
        self.automation_state.playback_position = self
            .automation_state
            .playback_position
            .checked_add(input.len() / self.input_channels())
            .ok_or("Graph playback position exceeds addressable frames")?;
        Ok(current_frames)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "plugin process signature mirrors the underlying audio graph API"
    )]
    pub(super) fn process_f64_node(
        plugin: &mut dyn Plugin,
        node: &GraphNode,
        bypassed: bool,
        input: &[f64],
        output: &mut [f64],
        sample_rate: f64,
        sample_position: u64,
        num_frames: usize,
    ) -> Result<usize, String> {
        if bypassed {
            output[..input.len()].copy_from_slice(input);
            return Ok(num_frames);
        }
        let context =
            ProcessContext::new(sample_rate, num_frames).with_sample_position(sample_position);
        let outcome =
            Self::process_plugin_f64_isolated(plugin, node, input, &mut *output, &context);
        if let Some(returned) = outcome.over_reported {
            let staged = output.len() / node.output_channels().max(1);
            return Err(format!(
                "plugin '{}' process returned {} frames, exceeding its {}-frame declared output capacity",
                node.name, returned, staged
            ));
        }
        Ok(outcome.frames)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "parallel stage dispatch needs the full graph context; a struct would just move the fields"
    )]
    pub(super) fn process_stage_parallel(
        parallel_enabled: bool,
        stage: &ProcessingStage,
        input: &[f32],
        sample_rate: f64,
        cf: usize,
        sample_position: u64,
        plugins: &mut [Option<Box<dyn Plugin>>],
        nodes: &HashMap<NodeId, GraphNode>,
        predecessors: &[Vec<GraphEdge>],
        is_input_node: &[bool],
        bypassed: &[bool],
        parallel_node_costs: &[u32],
        bufs: &mut ProcessBuffers<f32>,
    ) -> Option<Result<usize, String>> {
        if !parallel_enabled
            || !Self::should_parallelize_stage(stage, cf, nodes, parallel_node_costs)
        {
            return None;
        }

        for &nid in &stage.nodes {
            if nid >= plugins.len() || nid >= bufs.node_buffers.len() {
                return None;
            }
            if !is_input_node.get(nid).copied().unwrap_or(false) {
                let preds = predecessors.get(nid)?;
                if preds.len() != 1 {
                    return None;
                }
                let edge = &preds[0];
                if edge.edge_type != EdgeType::Audio
                    || edge.channel_map.is_some()
                    || edge.destination_offset != 0
                {
                    return None;
                }
            }
        }

        if bufs.parallel_scratch.len() < plugins.len()
            || bufs.parallel_results.capacity() < stage.nodes.len()
        {
            return None;
        }

        let plugins_addr = plugins.as_mut_ptr() as usize;
        let node_buffers_addr = bufs.node_buffers.as_mut_ptr() as usize;
        let scratch_addr = bufs.parallel_scratch.as_mut_ptr() as usize;
        stage
            .nodes
            .par_iter()
            .map(|&nid| {
                let node = nodes
                    .get(&nid)
                    .ok_or_else(|| format!("Missing node {nid} during parallel stage"))?;
                // SAFETY: `stage.nodes` is produced by topological sorting and contains
                // each node at most once. This closure mutates only the plugin, output
                // buffer, and scratch slot for its own `nid`. Reads are limited to
                // predecessor node buffers from earlier stages; this fast path rejects
                // merge nodes, so it never reads a buffer written by the same stage.
                unsafe {
                    let plugin_slot =
                        &mut *((plugins_addr as *mut Option<Box<dyn Plugin>>).add(nid));
                    let node_buffer_slot =
                        &mut *((node_buffers_addr as *mut Option<NodeBuffer<f32>>).add(nid));
                    let (scratch_input, scratch_output, merge_buffer) =
                        &mut *((scratch_addr as *mut (Vec<f32>, Vec<f32>, Vec<f32>)).add(nid));

                    let in_len = if is_input_node.get(nid).copied().unwrap_or(false) {
                        ensure_len(scratch_input, input.len());
                        scratch_input[..input.len()].copy_from_slice(input);
                        input.len()
                    } else {
                        let edge = &predecessors[nid][0];
                        let source = (&*((node_buffers_addr as *const Option<NodeBuffer<f32>>)
                            .add(edge.from_node)))
                            .as_ref()
                            .ok_or_else(|| {
                                format!("Missing predecessor buffer for node {}", edge.from_node)
                            })?;
                        let source_data = source.read();
                        let input_len = cf * node.input_channels();
                        ensure_len(merge_buffer, input_len);
                        merge_buffer[..input_len].fill(0.0);
                        let route_channels = source.num_channels.min(node.input_channels());
                        for frame in 0..cf {
                            let src = frame * source.num_channels;
                            let dst = frame * node.input_channels();
                            let src_end = (src + route_channels).min(source_data.len());
                            let copied = src_end.saturating_sub(src);
                            if copied > 0 {
                                merge_buffer[dst..dst + copied]
                                    .copy_from_slice(&source_data[src..src_end]);
                            }
                        }
                        ensure_len(scratch_input, input_len);
                        scratch_input[..input_len].copy_from_slice(&merge_buffer[..input_len]);
                        input_len
                    };

                    let out_frames = if bypassed.get(nid).copied().unwrap_or(false) {
                        node_buffer_slot
                            .as_mut()
                            .unwrap()
                            .write(&scratch_input[..in_len]);
                        cf
                    } else {
                        let plugin = plugin_slot.as_mut().unwrap();
                        let context = ProcessContext::new(f64::from(sample_rate), cf)
                            .with_sample_position(sample_position);
                        let max_output_frames = Self::plugin_output_frames_for_input_isolated(
                            plugin.as_ref(),
                            nid,
                            &node.name,
                            cf,
                        );
                        let output_len = max_output_frames * node.output_channels();
                        ensure_len(scratch_output, output_len);
                        let outcome = Self::process_plugin_f32_isolated(
                            plugin.as_mut(),
                            node,
                            &scratch_input[..in_len],
                            &mut scratch_output[..output_len],
                            &context,
                        );
                        if let Some(returned) = outcome.over_reported {
                            return Err(format!(
                                "plugin '{}' process returned {} frames, exceeding its {}-frame declared output capacity",
                                node.name, returned, max_output_frames
                            ));
                        }
                        let frames = outcome.frames;
                        node_buffer_slot
                            .as_mut()
                            .unwrap()
                            .write(&scratch_output[..frames * node.output_channels()]);
                        frames
                    };
                    Ok::<usize, String>(out_frames)
                }
            })
            .collect_into_vec(&mut bufs.parallel_results);

        let mut stage_cf = None;
        for result in bufs.parallel_results.drain(..) {
            match result {
                Ok(frames) => {
                    stage_cf = Some(stage_cf.map_or(frames, |prev: usize| prev.min(frames)));
                }
                Err(err) => return Some(Err(err)),
            }
        }

        stage_cf.map(Ok)
    }

    pub(super) fn should_parallelize_stage(
        stage: &ProcessingStage,
        frames: usize,
        nodes: &HashMap<NodeId, GraphNode>,
        parallel_node_costs: &[u32],
    ) -> bool {
        if stage.nodes.len() < 2 {
            return false;
        }
        Self::stage_work_units(stage, frames, nodes, parallel_node_costs)
            >= MIN_PARALLEL_STAGE_WORK_UNITS
    }

    pub(super) fn stage_work_units(
        stage: &ProcessingStage,
        frames: usize,
        nodes: &HashMap<NodeId, GraphNode>,
        parallel_node_costs: &[u32],
    ) -> usize {
        stage.nodes.iter().fold(0usize, |total, &nid| {
            let channels = nodes
                .get(&nid)
                .map(|node| node.input_channels().max(node.output_channels()).max(1))
                .unwrap_or(1);
            let cost = parallel_node_costs
                .get(nid)
                .copied()
                .unwrap_or(DEFAULT_PARALLEL_NODE_COST) as usize;
            total.saturating_add(frames.saturating_mul(channels).saturating_mul(cost))
        })
    }

    /// Routed channel width of every edge, indexed by `GraphEdge::id`.
    ///
    /// This must match `compute_compensation_delays` exactly, including the
    /// sidechain running-offset sequence, so queues hold exactly the frames
    /// their delay line was built for. Shared by the drain plan and the
    /// process-phase retention sizing so the two can never diverge.
    fn edge_routed_channels(
        nodes: &HashMap<NodeId, GraphNode>,
        edges: &[GraphEdge],
        predecessors: &[Vec<GraphEdge>],
    ) -> Vec<usize> {
        let mut routed = vec![0usize; edges.len()];
        for (node_id, dest) in nodes {
            let has_sidechain = predecessors[*node_id]
                .iter()
                .any(|edge| edge.edge_type == EdgeType::Sidechain);
            let primary = if has_sidechain && dest.input_channels() > dest.output_channels() {
                dest.output_channels()
            } else {
                dest.input_channels()
            };
            let mut sidechain_offset = primary;
            for edge in &predecessors[*node_id] {
                let source_channels = nodes
                    .get(&edge.from_node)
                    .map(|node| node.output_channels())
                    .unwrap_or(0);
                routed[edge.id] = Self::routed_channel_count(
                    dest,
                    edge,
                    source_channels,
                    primary,
                    &mut sidechain_offset,
                );
            }
        }
        routed
    }

    /// Per-edge process retention caps in frames, indexed by edge id.
    ///
    /// Only merge-point edges (destinations with at least two predecessors)
    /// receive nonzero caps. Each cap covers the edge's own compensation
    /// history plus one prepared-scale emission wave from its source and one
    /// from the merge's heaviest branch, so any skew up to a full wave plus
    /// alignment is admitted and sustained divergence fails loudly instead of
    /// dropping. Emissions are fresh-state declared bounds queried with the
    /// panic-isolated build-time helper (the same contract the drain plan
    /// uses); bypassed sources count as full passthrough waves. Every sum is
    /// checked: the build refuses on overflow rather than saturating.
    fn merge_retention_caps(
        &self,
        comp_frames: &[usize],
        prepared_sink_frames: usize,
    ) -> Result<Vec<usize>, String> {
        let mut caps = vec![0usize; self.edges.len()];
        for node_id in self.nodes.keys() {
            let incoming = &self.predecessors[*node_id];
            if incoming.len() < 2 {
                continue;
            }
            let mut waves = Vec::with_capacity(incoming.len());
            let mut emit_max = 0usize;
            for edge in incoming {
                // Envelope preparation per edge: envelopes dominate live
                // values, so each substitution only grows the cap and
                // mixed graphs keep live sizing exactly where unknown.
                let scale = self
                    .path_output_envelope(edge.from_node, prepared_sink_frames)
                    .unwrap_or_else(|| {
                        self.path_output_frames(edge.from_node, prepared_sink_frames)
                    });
                let bypassed = self.bypassed.get(edge.from_node).copied().unwrap_or(false);
                let cached = self.plugins.get(edge.from_node).and_then(Option::as_ref);
                let wave = if bypassed {
                    scale
                } else if let Some(plugin) = cached {
                    Self::plugin_output_frames_envelope_isolated(
                        plugin.as_ref(),
                        edge.from_node,
                        &self.nodes[&edge.from_node].name,
                        scale,
                    )
                    .unwrap_or_else(|| {
                        Self::plugin_output_frames_for_input_isolated(
                            plugin.as_ref(),
                            edge.from_node,
                            &self.nodes[&edge.from_node].name,
                            self.path_output_frames(edge.from_node, prepared_sink_frames),
                        )
                    })
                } else {
                    scale
                };
                waves.push((edge.id, wave));
                emit_max = emit_max.max(wave);
            }
            for (id, wave) in waves {
                let comp = comp_frames.get(id).copied().unwrap_or(0);
                let cap = comp
                    .checked_add(wave)
                    .and_then(|partial| partial.checked_add(emit_max))
                    .ok_or_else(|| {
                        format!(
                            "graph merge retention cap overflows addressable frames on edge {id}"
                        )
                    })?;
                if let Some(slot) = caps.get_mut(id) {
                    *slot = cap;
                } else {
                    return Err(format!(
                        "graph merge retention cap indexes unknown edge {id}"
                    ));
                }
            }
        }
        Ok(caps)
    }

    /// Build (or preserve) the per-edge retention FIFOs for one buffer set.
    ///
    /// Queues are pre-sized to `cap * routed width` samples with checked
    /// arithmetic, so process-phase pushes never allocate. On a no-change
    /// rebuild (`preserve`), old queues move over when the edge set matches
    /// and every depth fits the rebuilt cap; anything else fails loudly
    /// instead of dropping retained audio.
    fn restore_merge_queues<T: AudioSample>(
        old: Option<&Vec<VecDeque<T>>>,
        caps: &[usize],
        routed: &[usize],
        preserve: bool,
    ) -> Result<Vec<VecDeque<T>>, String> {
        let mut queues = Vec::with_capacity(caps.len());
        for (id, &cap) in caps.iter().enumerate() {
            let width = routed.get(id).copied().unwrap_or(0);
            let samples = cap.checked_mul(width).ok_or_else(|| {
                format!("graph merge retention of edge {id} exceeds addressable samples")
            })?;
            if samples > isize::MAX as usize {
                return Err(format!(
                    "graph merge retention of edge {id} needs {samples} samples, beyond addressable storage"
                ));
            }
            let mut queue = VecDeque::with_capacity(samples);
            if preserve {
                if let Some(old_queue) = old.as_ref().and_then(|queues| queues.get(id)) {
                    let old_frames = old_queue.len() / width.max(1);
                    if old_frames > cap {
                        return Err(format!(
                            "graph merge retention of edge {id} holds {old_frames} frames past the rebuilt {cap}-frame cap; reset the host"
                        ));
                    }
                    queue.extend(old_queue.iter().copied());
                } else if old.is_some() {
                    return Err(format!(
                        "graph merge retention lost edge {id} across a rebuild; reset the host"
                    ));
                }
            }
            queues.push(queue);
        }
        Ok(queues)
    }

    /// Per-edge merge routing: (skip, routed width, destination offset).
    ///
    /// Mirrors the single-predecessor merge loop's channel-map/offset
    /// semantics exactly, including the sidechain running-offset fold over the
    /// same edge order, so the retained path routes identically and only the
    /// frame accounting differs. `position` is the edge's index in `edges`.
    fn merge_edge_routing<T: AudioSample>(
        n: &GraphNode,
        edges: &[GraphEdge],
        nbs: &[Option<NodeBuffer<T>>],
        position: usize,
    ) -> (bool, usize, usize) {
        let has_sidechain = edges.iter().any(|e| e.edge_type == EdgeType::Sidechain);
        let primary_channels = if has_sidechain && n.input_channels() > n.output_channels() {
            n.output_channels()
        } else {
            n.input_channels()
        };
        let mut sidechain_offset = primary_channels;
        for prior in &edges[..position] {
            if prior.edge_type != EdgeType::Sidechain {
                continue;
            }
            if sidechain_offset >= n.input_channels() {
                continue;
            }
            let prior_source_channels = nbs[prior.from_node].as_ref().unwrap().num_channels;
            let requested = prior
                .channel_map
                .as_ref()
                .map_or(prior_source_channels, |cm| cm.len());
            sidechain_offset = (sidechain_offset + requested).min(n.input_channels());
        }
        let edge = &edges[position];
        let dest_offset = match edge.edge_type {
            EdgeType::Audio => edge.destination_offset,
            EdgeType::Sidechain => {
                if sidechain_offset >= n.input_channels() {
                    return (true, 0, 0);
                }
                sidechain_offset
            }
        };
        let available_dest_channels = match edge.edge_type {
            EdgeType::Audio => primary_channels.saturating_sub(dest_offset),
            EdgeType::Sidechain => n.input_channels().saturating_sub(dest_offset),
        };
        if available_dest_channels == 0 {
            return (true, 0, 0);
        }
        let source_channels = nbs[edge.from_node].as_ref().unwrap().num_channels;
        let width = match edge.channel_map.as_ref() {
            Some(cm) => cm.len().min(available_dest_channels),
            None => source_channels.min(available_dest_channels),
        };
        (false, width, dest_offset)
    }

    /// Route one edge's full actual emission through compensation into its queue.
    ///
    /// The channel routing mirrors the single-predecessor merge loop
    /// frame-for-frame (including pad-on-short reads); the delay half mirrors
    /// `apply_compensation_and_sum_at`, except the delayed output lands in the
    /// retention FIFO instead of the mix. Admission (pass 1 of the retained
    /// merge) guarantees the push fits the pre-sized capacity, so this never
    /// allocates.
    #[allow(
        clippy::too_many_arguments,
        reason = "retained edge push: routing plus delay halves share the merge scratch set"
    )]
    fn push_compensated_edge_frames<T: AudioSample>(
        edge: &GraphEdge,
        width: usize,
        num_frames: usize,
        src: &[T],
        src_channels: usize,
        queue: &mut VecDeque<T>,
        routed: &mut Vec<T>,
        delay_scratch: &mut Vec<T>,
        delays: &mut CompensationDelays<T>,
    ) -> Result<(), String> {
        if width == 0 || num_frames == 0 {
            return Ok(());
        }
        let total = num_frames.checked_mul(width).ok_or_else(|| {
            format!(
                "graph merge retention frame expansion overflows on edge {}",
                edge.id
            )
        })?;
        ensure_len(routed, total);
        if let Some(cm) = edge.channel_map.as_ref() {
            for f in 0..num_frames {
                for (di, &si) in cm.iter().take(width).enumerate() {
                    let dst = f * width + di;
                    let s = f * src_channels + si;
                    routed[dst] = src.get(s).copied().unwrap_or_default();
                }
            }
        } else if width == src_channels && src.len() >= total {
            routed[..total].copy_from_slice(&src[..total]);
        } else {
            for f in 0..num_frames {
                let s = f * src_channels;
                let dst = f * width;
                let src_end = (s + width).min(src.len());
                let copied = src_end.saturating_sub(s);
                if copied > 0 {
                    routed[dst..dst + copied].copy_from_slice(&src[s..src_end]);
                }
                if copied < width {
                    routed[dst + copied..dst + width].fill(T::default());
                }
            }
        }
        if let Some(delay_buf) = delays.get_mut_edge(edge.id) {
            let needed = total + width;
            if delay_scratch.len() < needed {
                delay_scratch.resize(needed, T::default());
            }
            let (frame_part, silence) = delay_scratch.split_at_mut(total);
            silence[..width].fill(T::default());
            for f in 0..num_frames {
                let start = f * width;
                delay_buf.process_frame(
                    &routed[start..start + width],
                    &mut frame_part[start..start + width],
                );
            }
            queue.extend(frame_part[..total].iter().copied());
        } else {
            queue.extend(routed[..total].iter().copied());
        }
        Ok(())
    }

    /// Merge-point join with lossless per-edge retention.
    ///
    /// Every participating edge pushes its full actual emission through
    /// routing and compensation into its FIFO; the join then consumes the
    /// minimum queued depth and sums it into the merge buffer. Admission is
    /// atomic: every edge is cap-checked before any queue or delay mutates, so
    /// an overflow error leaves no partial block behind (the caller poisons
    /// the host, and only `reset` recovers). Edges routed nowhere keep the
    /// pre-existing skip: they neither queue nor gate the join.
    #[allow(
        clippy::too_many_arguments,
        reason = "retained merge path: same scratch buffers as the single-predecessor merge"
    )]
    fn merge_inputs_retained<T: AudioSample>(
        n: &GraphNode,
        preds: &[Vec<GraphEdge>],
        nbs: &[Option<NodeBuffer<T>>],
        mb: &mut Vec<T>,
        cmb: &mut Vec<T>,
        delay_scratch: &mut Vec<T>,
        compensation_delays: &mut CompensationDelays<T>,
        merge_queues: &mut [VecDeque<T>],
        merge_caps: &[usize],
    ) -> Result<usize, String> {
        let edges = &preds[n.id];
        // Pass 1 (pure): admit every edge before any queue or delay mutates.
        for (position, edge) in edges.iter().enumerate() {
            let (skip, width, _) = Self::merge_edge_routing(n, edges, nbs, position);
            if skip || width == 0 {
                continue;
            }
            let source = nbs[edge.from_node].as_ref().unwrap();
            let incoming = source.actual_len / source.num_channels.max(1);
            let queue = merge_queues
                .get(edge.id)
                .ok_or_else(|| format!("graph merge retention indexes unknown edge {}", edge.id))?;
            let queued = queue.len() / width.max(1);
            let cap = merge_caps.get(edge.id).copied().unwrap_or(0);
            let admitted = queued
                .checked_add(incoming)
                .is_some_and(|total| total <= cap);
            if !admitted {
                return Err(format!(
                    "graph merge retention overflow: edge {} into node {} holds {queued}/{cap} frames with {incoming} incoming; sustained branch divergence (reset the host)",
                    edge.id, n.id
                ));
            }
        }
        // Pass 2: route full actual emissions through compensation into queues.
        for (position, edge) in edges.iter().enumerate() {
            let (skip, width, _) = Self::merge_edge_routing(n, edges, nbs, position);
            if skip {
                continue;
            }
            let source = nbs[edge.from_node].as_ref().unwrap();
            let data = source.read();
            let incoming = source.actual_len / source.num_channels.max(1);
            let queue = merge_queues
                .get_mut(edge.id)
                .ok_or_else(|| format!("graph merge retention indexes unknown edge {}", edge.id))?;
            Self::push_compensated_edge_frames(
                edge,
                width,
                incoming,
                data,
                source.num_channels,
                queue,
                cmb,
                delay_scratch,
                compensation_delays,
            )?;
        }
        // Pass 3: consume the minimum queued depth and sum it.
        let mut consume = usize::MAX;
        for (position, edge) in edges.iter().enumerate() {
            let (skip, width, _) = Self::merge_edge_routing(n, edges, nbs, position);
            if skip {
                continue;
            }
            let queue = merge_queues
                .get(edge.id)
                .ok_or_else(|| format!("graph merge retention indexes unknown edge {}", edge.id))?;
            consume = consume.min(queue.len() / width.max(1));
        }
        let consume = if consume == usize::MAX { 0 } else { consume };
        let out_samples = consume.checked_mul(n.input_channels()).ok_or_else(|| {
            format!(
                "graph merge retention join overflows on node {} ({} frames)",
                n.id, consume
            )
        })?;
        ensure_len(mb, out_samples);
        mb[..out_samples].fill(T::default());
        for (position, edge) in edges.iter().enumerate() {
            let (skip, width, dest_offset) = Self::merge_edge_routing(n, edges, nbs, position);
            if skip || width == 0 || consume == 0 {
                continue;
            }
            let queue = merge_queues
                .get_mut(edge.id)
                .ok_or_else(|| format!("graph merge retention indexes unknown edge {}", edge.id))?;
            let take = consume.checked_mul(width).ok_or_else(|| {
                format!(
                    "graph merge retention join overflows on edge {} ({} frames)",
                    edge.id, consume
                )
            })?;
            ensure_len(cmb, take);
            let (front, back) = queue.as_slices();
            let from_front = front.len().min(take);
            cmb[..from_front].copy_from_slice(&front[..from_front]);
            cmb[from_front..take].copy_from_slice(&back[..take - from_front]);
            drop(queue.drain(..take));
            Self::sum_interleaved_at(
                &cmb[..take],
                mb,
                width,
                n.input_channels(),
                dest_offset,
                consume,
            );
        }
        Ok(out_samples)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "internal graph wiring helper: all arguments are distinct scratch buffers"
    )]
    pub(super) fn merge_inputs_into<T: AudioSample>(
        n: &GraphNode,
        preds: &[Vec<GraphEdge>],
        nbs: &[Option<NodeBuffer<T>>],
        nf: usize,
        mb: &mut Vec<T>,
        cmb: &mut Vec<T>,
        delay_scratch: &mut Vec<T>,
        compensation_delays: &mut CompensationDelays<T>,
        merge_queues: &mut [VecDeque<T>],
        merge_caps: &[usize],
    ) -> Result<usize, String> {
        let any_participating = preds[n.id].iter().enumerate().any(|(position, _)| {
            let (skip, width, _) = Self::merge_edge_routing(n, &preds[n.id], nbs, position);
            !skip && width > 0
        });
        if preds[n.id].len() >= 2 && any_participating {
            return Self::merge_inputs_retained(
                n,
                preds,
                nbs,
                mb,
                cmb,
                delay_scratch,
                compensation_delays,
                merge_queues,
                merge_caps,
            );
        }
        let is = nf * n.input_channels();
        ensure_len(mb, is);
        mb[..is].fill(T::default());
        let has_sidechain = preds[n.id]
            .iter()
            .any(|e| e.edge_type == EdgeType::Sidechain);
        let primary_channels = if has_sidechain && n.input_channels() > n.output_channels() {
            n.output_channels()
        } else {
            n.input_channels()
        };
        let mut sidechain_offset = primary_channels;
        for e in &preds[n.id] {
            let sb = nbs[e.from_node].as_ref().unwrap();
            let sd = sb.read();
            let dest_offset = match e.edge_type {
                EdgeType::Audio => e.destination_offset,
                EdgeType::Sidechain => {
                    if sidechain_offset >= n.input_channels() {
                        continue;
                    }
                    let offset = sidechain_offset;
                    let requested = e
                        .channel_map
                        .as_ref()
                        .map_or(sb.num_channels, |cm| cm.len());
                    sidechain_offset = (sidechain_offset + requested).min(n.input_channels());
                    offset
                }
            };
            let available_dest_channels = match e.edge_type {
                EdgeType::Audio => primary_channels.saturating_sub(dest_offset),
                EdgeType::Sidechain => n.input_channels().saturating_sub(dest_offset),
            };
            if available_dest_channels == 0 {
                continue;
            }
            if let Some(ref cm) = e.channel_map {
                let mapped_channels = cm.len().min(available_dest_channels);
                let ms = nf * mapped_channels;
                ensure_len(cmb, ms);
                for f in 0..nf {
                    for (di, &si) in cm.iter().take(mapped_channels).enumerate() {
                        let dst = f * mapped_channels + di;
                        let src = f * sb.num_channels + si;
                        cmb[dst] = sd.get(src).copied().unwrap_or_default();
                    }
                }
                Self::apply_compensation_and_sum_at(
                    e,
                    mapped_channels,
                    nf,
                    &cmb[..ms],
                    mb,
                    n.input_channels(),
                    dest_offset,
                    delay_scratch,
                    compensation_delays,
                );
            } else {
                let route_channels = sb.num_channels.min(available_dest_channels);
                let ms = nf * route_channels;
                if route_channels == sb.num_channels && sd.len() >= ms {
                    Self::apply_compensation_and_sum_at(
                        e,
                        route_channels,
                        nf,
                        &sd[..ms],
                        mb,
                        n.input_channels(),
                        dest_offset,
                        delay_scratch,
                        compensation_delays,
                    );
                } else {
                    ensure_len(cmb, ms);
                    for f in 0..nf {
                        let src = f * sb.num_channels;
                        let dst = f * route_channels;
                        let src_end = (src + route_channels).min(sd.len());
                        let copied = src_end.saturating_sub(src);
                        if copied > 0 {
                            cmb[dst..dst + copied].copy_from_slice(&sd[src..src_end]);
                        }
                        if copied < route_channels {
                            cmb[dst + copied..dst + route_channels].fill(T::default());
                        }
                    }
                    Self::apply_compensation_and_sum_at(
                        e,
                        route_channels,
                        nf,
                        &cmb[..ms],
                        mb,
                        n.input_channels(),
                        dest_offset,
                        delay_scratch,
                        compensation_delays,
                    );
                }
            }
        }
        Ok(is)
    }

    fn node_input_frames<T: AudioSample>(
        id: NodeId,
        source_frames: usize,
        predecessors: &[Vec<GraphEdge>],
        buffers: &[Option<NodeBuffer<T>>],
    ) -> Result<usize, String> {
        let mut frames = None;
        for edge in &predecessors[id] {
            let buffer = buffers[edge.from_node].as_ref().unwrap();
            let count = buffer.actual_len / buffer.num_channels;
            // Common-prefix behavior for input and single-predecessor nodes.
            // Merge points (>= 2 predecessors) retain per-edge queues instead
            // (`merge_inputs_retained`); the caller re-derives their frame
            // count from the exact merged samples.
            frames = Some(frames.map_or(count, |previous: usize| previous.min(count)));
        }
        Ok(frames.unwrap_or(source_frames))
    }

    fn terminal_output_frames<T: AudioSample>(
        outputs: &[NodeId],
        buffers: &[Option<NodeBuffer<T>>],
    ) -> Result<usize, String> {
        let mut frames = None;
        for &id in outputs {
            let buffer = buffers[id].as_ref().unwrap();
            let count = buffer.actual_len / buffer.num_channels;
            frames = Some(frames.map_or(count, |previous: usize| previous.min(count)));
        }
        Ok(frames.unwrap_or(0))
    }

    /// Apply latency compensation delay (if any) to `src_data` for the given edge,
    /// then sum the result into `dest`. If no compensation is needed, sums directly.
    #[allow(
        clippy::too_many_arguments,
        reason = "internal latency-compensation helper: scratch buffers and destination slices are separate concerns"
    )]
    pub(super) fn apply_compensation_and_sum_at<T: AudioSample>(
        edge: &GraphEdge,
        channels: usize,
        num_frames: usize,
        src_data: &[T],
        dest: &mut [T],
        dest_channels: usize,
        dest_offset: usize,
        delay_scratch: &mut Vec<T>,
        compensation_delays: &mut CompensationDelays<T>,
    ) {
        if let Some(delay_buf) = compensation_delays.get_mut_edge(edge.id) {
            // Process frame-by-frame through the compensation delay.
            // delay_scratch is split: first `total` samples for output,
            // next `channels` samples as a reusable silence frame.
            let total = num_frames * channels;
            let needed = total + channels;
            if delay_scratch.len() < needed {
                delay_scratch.resize(needed, T::default());
            }
            // Zero the silence frame region
            delay_scratch[total..total + channels].fill(T::default());
            for f in 0..num_frames {
                let start = f * channels;
                let end = start + channels;
                if end <= src_data.len() {
                    // Split delay_scratch so we can read silence region while writing frame region
                    let (frame_part, silence_part) = delay_scratch.split_at_mut(total);
                    let _ = silence_part; // unused in this branch
                    delay_buf.process_frame(&src_data[start..end], &mut frame_part[start..end]);
                } else {
                    // Partial/missing frame: feed silence from the tail of delay_scratch
                    let (frame_part, silence_part) = delay_scratch.split_at_mut(total);
                    delay_buf.process_frame(&silence_part[..channels], &mut frame_part[start..end]);
                }
            }
            Self::sum_interleaved_at(
                &delay_scratch[..total],
                dest,
                channels,
                dest_channels,
                dest_offset,
                num_frames,
            );
        } else {
            Self::sum_interleaved_at(
                src_data,
                dest,
                channels,
                dest_channels,
                dest_offset,
                num_frames,
            );
        }
    }

    pub(super) fn sum_interleaved_at<T: AudioSample>(
        src_data: &[T],
        dest: &mut [T],
        channels: usize,
        dest_channels: usize,
        dest_offset: usize,
        num_frames: usize,
    ) {
        let total = num_frames * channels;
        if dest_offset == 0
            && channels == dest_channels
            && src_data.len() >= total
            && dest.len() >= total
        {
            T::scale_add(&mut dest[..total], &src_data[..total]);
            return;
        }

        for frame in 0..num_frames {
            let src = frame * channels;
            let dst = frame * dest_channels + dest_offset;
            for ch in 0..channels {
                if src + ch < src_data.len() && dst + ch < dest.len() {
                    dest[dst + ch] += src_data[src + ch];
                }
            }
        }
    }

    pub(super) fn collect_output_from_buffers<T: AudioSample>(
        ons: &[NodeId],
        nbs: &mut [Option<NodeBuffer<T>>],
        delays: &mut CompensationDelays<T>,
        out: &mut [T],
        _nf: usize,
    ) -> Result<(), String> {
        if ons.len() == 1 {
            let d = nbs[ons[0]].as_ref().unwrap().read();
            let l = d.len().min(out.len());
            out[..l].copy_from_slice(&d[..l]);
        } else {
            out.fill(T::default());
            for (output_index, &id) in ons.iter().enumerate() {
                let buffer = nbs[id].as_mut().unwrap();
                let d = &mut buffer.data[..buffer.actual_len];
                if let Some(delay) = delays.output_delays[output_index].as_mut() {
                    for frame in d.chunks_exact_mut(buffer.num_channels) {
                        delay.process_frame_in_place(frame);
                    }
                }
                let l = d.len().min(out.len());
                T::scale_add(&mut out[..l], &d[..l]);
            }
        }
        Ok(())
    }

    /// Verify one edge's EOS queue can absorb a retention handoff.
    ///
    /// The build pre-sizes every EOS queue for its drain wave plus retention,
    /// so any shortfall is a loud internal error, never a silent reallocation.
    fn check_retention_handoff_capacity(
        plan: &GraphDrainPlan,
        state: &GraphDrainState,
        id: usize,
        add: usize,
    ) -> Result<(), String> {
        let edge_plan = plan
            .edge_plan
            .get(id)
            .ok_or_else(|| format!("graph drain retention handoff indexes unknown edge {id}"))?;
        let room = edge_plan
            .queue_cap_frames
            .checked_mul(edge_plan.routed_channels)
            .ok_or_else(|| format!("graph drain retention capacity overflows on edge {id}"))?;
        let held = state
            .edges
            .get(id)
            .map(|edge| edge.queue.len())
            .unwrap_or(0);
        let total = held
            .checked_add(add)
            .ok_or_else(|| format!("graph drain retention size overflows on edge {id}"))?;
        if total > room {
            return Err(format!(
                "graph drain retention handoff of {add} samples exceeds prepared {room} on edge {id}"
            ));
        }
        Ok(())
    }

    /// Move process-phase merge retention into the graph EOS edge queues.
    ///
    /// Runs at the top of every graph drain call; it is a no-op unless a
    /// process-phase queue holds frames. Only `process` fills those queues
    /// and the first drain moves them out, so mid-drain calls find them
    /// empty. Retention lands ahead of native tails in EOS queues the build
    /// pre-sized for the sum, so the unchanged scheduler drains it first
    /// without reallocating. f64 retention crosses the documented f32-only
    /// drain bridge with the same `as f32` cast as the f64 fallback input
    /// path. Both precisions holding frames means a mixed-precision stream
    /// without an intervening reset, which fails loudly.
    fn transfer_process_retention_to_drain(&mut self) -> Result<(), String> {
        let f32_held: usize = self
            .process_buffers
            .as_ref()
            .map(|buffers| buffers.merge_queues.iter().map(VecDeque::len).sum())
            .unwrap_or(0);
        let f64_held: usize = self
            .process_buffers_f64
            .as_ref()
            .map(|buffers| buffers.merge_queues.iter().map(VecDeque::len).sum())
            .unwrap_or(0);
        if f32_held > 0 && f64_held > 0 {
            return Err(
                "graph drain refused mixed-precision process retention without an intervening reset"
                    .into(),
            );
        }
        if f32_held == 0 && f64_held == 0 {
            return Ok(());
        }
        if f32_held > 0 {
            let buffers = self.process_buffers.as_mut().ok_or_else(|| {
                "graph drain lost process buffers with retention held".to_string()
            })?;
            for (id, queue) in buffers.merge_queues.iter().enumerate() {
                if !queue.is_empty() {
                    Self::check_retention_handoff_capacity(
                        &self.graph_drain_plan,
                        &self.graph_drain_state,
                        id,
                        queue.len(),
                    )?;
                }
            }
            for (id, queue) in buffers.merge_queues.iter_mut().enumerate() {
                if queue.is_empty() {
                    continue;
                }
                let dst = self
                    .graph_drain_state
                    .edges
                    .get_mut(id)
                    .map(|edge| &mut edge.queue)
                    .ok_or_else(|| {
                        format!("graph drain retention handoff indexes unknown edge {id}")
                    })?;
                dst.extend(queue.drain(..));
            }
        }
        if f64_held > 0 {
            let buffers = self.process_buffers_f64.as_mut().ok_or_else(|| {
                "graph drain lost process buffers with retention held".to_string()
            })?;
            for (id, queue) in buffers.merge_queues.iter().enumerate() {
                if !queue.is_empty() {
                    Self::check_retention_handoff_capacity(
                        &self.graph_drain_plan,
                        &self.graph_drain_state,
                        id,
                        queue.len(),
                    )?;
                }
            }
            for (id, queue) in buffers.merge_queues.iter_mut().enumerate() {
                if queue.is_empty() {
                    continue;
                }
                let dst = self
                    .graph_drain_state
                    .edges
                    .get_mut(id)
                    .map(|edge| &mut edge.queue)
                    .ok_or_else(|| {
                        format!("graph drain retention handoff indexes unknown edge {id}")
                    })?;
                dst.extend(queue.drain(..).map(|sample| sample as f32));
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.drain_state = DrainState::default();
        self.graph_drain_state.reset_dynamic();
        self.terminal_sink_source_complete = false;
        self.terminal_sink_producer_started = false;
        if self.terminal_sink_lifecycle.is_some() {
            self.terminal_sink_lifecycle = Some(TerminalSinkLifecycle::Running);
        }
        if let Err(error) =
            self.reanchor_node_positions(self.automation_state.playback_position as u64)
        {
            self.built = false;
            log::error!("Graph reset invalidated its timeline: {error}");
        }
        for &id in self.nodes.keys() {
            if let Some(p) = self.plugins[id].as_mut() {
                p.reset();
            }
        }
        if let Some(buffers) = self.process_buffers.as_mut() {
            buffers.compensation_delays.reset();
            for queue in &mut buffers.merge_queues {
                queue.clear();
            }
        }
        if let Some(buffers) = self.process_buffers_f64.as_mut() {
            buffers.compensation_delays.reset();
            for queue in &mut buffers.merge_queues {
                queue.clear();
            }
        }
        self.merge_overflow_poisoned = false;
    }
    pub fn total_latency_samples(&self) -> usize {
        if let Some(cached) = self.cached_latency {
            return cached;
        }
        self.compute_latency()
    }
    /// Conservative active-graph queued-work horizon in host-input frames.
    ///
    /// `build()` computes the exact cross-rate value. Before the first build,
    /// this returns a conservative same-rate sum; production activation
    /// builds the host off-thread before consuming this contract.
    pub fn realtime_quantum_frames(&self) -> usize {
        if self.built {
            self.config.realtime_quantum_frames
        } else {
            self.plugins
                .iter()
                .flatten()
                .map(|plugin| plugin.realtime_quantum_frames().max(1))
                .fold(0usize, usize::saturating_add)
                .max(1)
        }
    }
    /// Use a common integer clock to retain fractions across consecutive converters.
    /// Clock construction and all latency calculations run during graph setup.
    fn latency_clock_rate(&self) -> Result<ExactClock, String> {
        ExactClock::new(
            std::iter::once(self.config.sample_rate)
                .chain(
                    self.nodes
                        .keys()
                        .map(|&id| self.node_input_sample_rates[id]),
                )
                .chain(
                    self.nodes
                        .keys()
                        .map(|&id| self.node_output_sample_rates[id]),
                )
                .map(ExactRate::new)
                .collect::<Result<Vec<_>, _>>()?,
        )
    }

    fn latency_ticks_to_frames(ticks: u128, clock: ExactClock, rate: f64) -> Result<usize, String> {
        clock.ticks_to_frames_ceil(ticks, ExactRate::new(rate)?)
    }

    pub(super) fn compute_latency(&self) -> usize {
        self.output_nodes
            .iter()
            .map(|&id| self.path_latency(id))
            .max()
            .unwrap_or(0)
    }
    pub(super) fn path_latency(&self, id: NodeId) -> usize {
        (|| {
            let clock = self.latency_clock_rate()?;
            let rate = self
                .node_output_sample_rates
                .get(id)
                .copied()
                .unwrap_or(self.config.sample_rate);
            Self::latency_ticks_to_frames(self.path_latency_ticks(id, clock)?, clock, rate)
        })()
        .unwrap_or(usize::MAX)
    }

    fn path_latency_ticks(&self, id: NodeId, clock: ExactClock) -> Result<u128, String> {
        let own = if self.nodes.get(&id).is_some_and(|node| node.bypassed) {
            0
        } else {
            self.plugins[id].as_ref().unwrap().latency_samples()
        };
        let rate = ExactRate::new(
            self.node_output_sample_rates
                .get(id)
                .copied()
                .unwrap_or(self.config.sample_rate),
        )?;
        let own_ticks = (own as u128)
            .checked_mul(clock.ticks_per_sample(rate)?)
            .ok_or_else(|| "Graph latency exceeds the exact clock range".to_owned())?;
        let edges = self.predecessors.get(id).unwrap_or(&self.edges);
        let mut predecessors = edges.iter().filter(|edge| edge.to_node == id);
        let Some(first) = predecessors.next() else {
            return Ok(own_ticks);
        };
        let first_ticks = self.path_latency_ticks(first.from_node, clock)?;
        if predecessors.next().is_none() {
            return own_ticks
                .checked_add(first_ticks)
                .ok_or_else(|| "Graph latency exceeds the exact clock range".to_owned());
        }
        let input_rate = self
            .node_input_sample_rates
            .get(id)
            .copied()
            .unwrap_or(self.config.sample_rate);
        let max_frames =
            edges
                .iter()
                .filter(|edge| edge.to_node == id)
                .try_fold(0_usize, |maximum, edge| {
                    let frames = Self::latency_ticks_to_frames(
                        self.path_latency_ticks(edge.from_node, clock)?,
                        clock,
                        input_rate,
                    )?;
                    Ok::<_, String>(maximum.max(frames))
                })?;
        let predecessor_ticks =
            edges
                .iter()
                .filter(|edge| edge.to_node == id)
                .try_fold(0_u128, |maximum, edge| {
                    let ticks = self.path_latency_ticks(edge.from_node, clock)?;
                    let frames = Self::latency_ticks_to_frames(ticks, clock, input_rate)?;
                    let compensation = (max_frames - frames) as u128;
                    let compensation_ticks = compensation
                        .checked_mul(clock.ticks_per_sample(ExactRate::new(input_rate)?)?)
                        .ok_or_else(|| {
                            "Graph compensation exceeds the exact clock range".to_owned()
                        })?;
                    let candidate = ticks.checked_add(compensation_ticks).ok_or_else(|| {
                        "Graph compensation exceeds the exact clock range".to_owned()
                    })?;
                    Ok::<_, String>(maximum.max(candidate))
                })?;
        own_ticks
            .checked_add(predecessor_ticks)
            .ok_or_else(|| "Graph latency exceeds the exact clock range".to_owned())
    }
    /// Compute the cumulative latency from graph inputs to each node, then create
    /// compensation delay buffers for edges feeding into merge points where path
    /// latencies differ. This ensures all paths through the DAG are time-aligned
    /// at merge points.
    pub(super) fn compute_compensation_delays<T: AudioSample>(
        &mut self,
        num_slots: usize,
    ) -> Result<CompensationDelays<T>, String> {
        // Step 1: Compute cumulative latency from inputs to each node using topological order.
        // For each node, the cumulative latency is:
        //   node's own latency + max(cumulative latency of predecessors)
        self.node_latency_from_input = vec![0; num_slots];
        let clock = self.latency_clock_rate()?;
        let mut latency_ticks = vec![0_u128; num_slots];

        // Process in topological order (stages are already computed)
        for stage in &self.stages {
            for &nid in &stage.nodes {
                let own_latency = if self.bypassed.get(nid).copied().unwrap_or(false) {
                    0
                } else {
                    self.plugins[nid]
                        .as_ref()
                        .map(|p| p.latency_samples())
                        .unwrap_or(0)
                };

                let max_pred_frames = self.predecessors[nid]
                    .iter()
                    .map(|edge| self.node_latency_from_input[edge.from_node])
                    .max()
                    .unwrap_or(0);
                // Integer compensation can add a fraction beyond the longest
                // unrounded path. Carry that actual added delay into later
                // rate conversions instead of discarding it at this merge.
                let input_tick_scale =
                    clock.ticks_per_sample(ExactRate::new(self.node_input_sample_rates[nid])?)?;
                let max_pred_ticks =
                    self.predecessors[nid]
                        .iter()
                        .try_fold(0_u128, |maximum, edge| {
                            let compensation =
                                max_pred_frames - self.node_latency_from_input[edge.from_node];
                            let ticks = (compensation as u128)
                                .checked_mul(input_tick_scale)
                                .and_then(|delay| latency_ticks[edge.from_node].checked_add(delay))
                                .ok_or_else(|| {
                                    "Graph compensation exceeds the exact clock range".to_string()
                                })?;
                            Ok::<_, String>(maximum.max(ticks))
                        })?;
                let rate = self.node_output_sample_rates[nid];
                latency_ticks[nid] = (own_latency as u128)
                    .checked_mul(clock.ticks_per_sample(ExactRate::new(rate)?)?)
                    .and_then(|own_ticks| own_ticks.checked_add(max_pred_ticks))
                    .ok_or_else(|| "Graph latency exceeds the exact clock range".to_string())?;
                self.node_latency_from_input[nid] =
                    Self::latency_ticks_to_frames(latency_ticks[nid], clock, rate)?;
            }
        }

        // Step 2: For each merge point (node with multiple predecessors), compute
        // compensation delays for shorter paths.
        let mut delays = CompensationDelays::<T>::new(&self.edges);

        for stage in &self.stages {
            for &nid in &stage.nodes {
                let preds = &self.predecessors[nid];
                if preds.len() < 2 {
                    continue; // Not a merge point
                }

                // Find the max cumulative latency among all predecessors
                let max_pred_latency = preds
                    .iter()
                    .map(|e| self.node_latency_from_input[e.from_node])
                    .max()
                    .unwrap_or(0);

                // For each predecessor with lower latency, create a compensation delay
                let dest_node = self
                    .nodes
                    .get(&nid)
                    .expect("stage node should exist in graph");
                let has_sidechain = preds.iter().any(|e| e.edge_type == EdgeType::Sidechain);
                let primary_channels =
                    if has_sidechain && dest_node.input_channels() > dest_node.output_channels() {
                        dest_node.output_channels()
                    } else {
                        dest_node.input_channels()
                    };
                let mut sidechain_offset = primary_channels;
                for edge in preds {
                    let pred_latency = self.node_latency_from_input[edge.from_node];
                    let compensation = max_pred_latency - pred_latency;
                    if compensation > 0 {
                        let pred_channels = self
                            .nodes
                            .get(&edge.from_node)
                            .map(|n| n.output_channels())
                            .unwrap_or(2);
                        let delay_channels = Self::routed_channel_count(
                            dest_node,
                            edge,
                            pred_channels,
                            primary_channels,
                            &mut sidechain_offset,
                        );
                        if delay_channels == 0 {
                            continue;
                        }
                        delays.set(edge.id, DelayBuffer::new(compensation, delay_channels))?;
                    } else if edge.edge_type == EdgeType::Sidechain {
                        let pred_channels = self
                            .nodes
                            .get(&edge.from_node)
                            .map(|n| n.output_channels())
                            .unwrap_or(2);
                        let _ = Self::routed_channel_count(
                            dest_node,
                            edge,
                            pred_channels,
                            primary_channels,
                            &mut sidechain_offset,
                        );
                    }
                }
            }
        }

        // The host output is another merge point, even when no plugin joins
        // the terminal branches. Align these paths before summing them.
        let output_latency = self
            .output_nodes
            .iter()
            .map(|&id| self.node_latency_from_input[id])
            .max()
            .unwrap_or(0);
        delays.output_delays = self
            .output_nodes
            .iter()
            .map(|&id| {
                let compensation = output_latency - self.node_latency_from_input[id];
                (compensation > 0)
                    .then(|| DelayBuffer::new(compensation, self.nodes[&id].output_channels()))
            })
            .collect();

        Ok(delays)
    }

    pub(super) fn routed_channel_count(
        dest_node: &GraphNode,
        edge: &GraphEdge,
        source_channels: usize,
        primary_channels: usize,
        sidechain_offset: &mut usize,
    ) -> usize {
        let available_dest_channels = match edge.edge_type {
            EdgeType::Audio => primary_channels.saturating_sub(edge.destination_offset),
            EdgeType::Sidechain => {
                if *sidechain_offset >= dest_node.input_channels() {
                    return 0;
                }
                let available = dest_node.input_channels().saturating_sub(*sidechain_offset);
                let requested = edge
                    .channel_map
                    .as_ref()
                    .map_or(source_channels, |cm| cm.len());
                *sidechain_offset = (*sidechain_offset + requested).min(dest_node.input_channels());
                available
            }
        };

        if available_dest_channels == 0 {
            return 0;
        }

        edge.channel_map.as_ref().map_or_else(
            || source_channels.min(available_dest_channels),
            |cm| cm.len().min(available_dest_channels),
        )
    }

    pub(super) fn has_cycle(&self) -> bool {
        let mut v = HashSet::new();
        let mut r = HashSet::new();
        for &id in self.nodes.keys() {
            if self.cycle_util(id, &mut v, &mut r) {
                return true;
            }
        }
        false
    }
    pub(super) fn cycle_util(
        &self,
        id: NodeId,
        v: &mut HashSet<NodeId>,
        r: &mut HashSet<NodeId>,
    ) -> bool {
        if r.contains(&id) {
            return true;
        }
        if v.contains(&id) {
            return false;
        }
        v.insert(id);
        r.insert(id);
        for e in &self.edges {
            if e.from_node == id && self.cycle_util(e.to_node, v, r) {
                return true;
            }
        }
        r.remove(&id);
        false
    }
    pub(super) fn compute_io_nodes(&mut self) {
        let mut hi = HashSet::new();
        let mut ho = HashSet::new();
        for e in &self.edges {
            hi.insert(e.to_node);
            ho.insert(e.from_node);
        }
        self.input_nodes = self
            .nodes
            .keys()
            .filter(|id| !hi.contains(id))
            .copied()
            .collect();
        self.output_nodes = self
            .nodes
            .keys()
            .filter(|id| !ho.contains(id))
            .copied()
            .collect();
    }
    pub(super) fn compute_stages(&mut self) -> Result<(), String> {
        let mut deg: HashMap<NodeId, usize> = self.nodes.keys().map(|&id| (id, 0)).collect();
        for e in &self.edges {
            *deg.get_mut(&e.to_node).unwrap() += 1;
        }
        let mut q: VecDeque<NodeId> = deg
            .iter()
            .filter(|&(_, &d)| d == 0)
            .map(|(&id, _)| id)
            .collect();
        self.stages.clear();
        let mut count = 0;
        while !q.is_empty() {
            let mut s = ProcessingStage::new();
            for _ in 0..q.len() {
                let id = q.pop_front().unwrap();
                s.add_node(id);
                count += 1;
                for e in &self.edges {
                    if e.from_node == id {
                        let d = deg.get_mut(&e.to_node).unwrap();
                        *d -= 1;
                        if *d == 0 {
                            q.push_back(e.to_node);
                        }
                    }
                }
            }
            if !s.is_empty() {
                self.stages.push(s);
            }
        }
        if count != self.nodes.len() {
            Err("Cycle".into())
        } else {
            Ok(())
        }
    }
    pub fn num_stages(&self) -> usize {
        self.stages.len()
    }
    pub fn stage_info(&self, idx: usize) -> Option<Vec<String>> {
        self.stages.get(idx).map(|s| {
            s.nodes
                .iter()
                .filter_map(|id| self.nodes.get(id))
                .map(|n| n.name.clone())
                .collect()
        })
    }
}

impl DawHost {
    /// Live remaining tail in host-output frames.
    ///
    /// Total future emission with no further input, folded from current
    /// stream state: retained waves (process merge queues, drain edge
    /// queues, node holdover, output queues), pending compensation flushes,
    /// and per-node tails, composed through each plugin's live
    /// [`Plugin::output_frames_for_input`] transfer declaration. A
    /// `Finite` answer is a conservative bound: it is exact only when
    /// every transfer declaration is exact at the folded quanta and every
    /// tail exact (quiescent transfer-exact topologies: delays, gains,
    /// identity, residual-exact bursts — fresh or fully-armed drain
    /// state). Resampler transfers over-approximate through chunk
    /// ceilings and call padding, so converting folds are bounds, never
    /// counts. It over-approximates mid-drain partial progress (a
    /// draining node contributes its full live tail) and un-armed
    /// compensation (full plan length until the arming transition fires,
    /// then the exact remainder). It never undercounts: every in-flight
    /// frame is counted
    /// in exactly one store (retention transfer and holdover commit are
    /// moves, and the query cannot interleave them), joins pace by the
    /// minimum over non-EOF data inputs plus EOF-side content, sidechain
    /// edges carry control rather than audio frames, and bypassed nodes
    /// pass input through with no tail, mirroring drain dispatch, which
    /// this mirrors with the same chain predicate.
    ///
    /// `Unknown` is honest, never a guess: refused drains (merge-overflow
    /// poison, mixed-precision retention), terminal-sink routing (a side
    /// channel this fold does not model), unbuilt graphs (no plan), an
    /// unprovable node tail without a support bound, any structural
    /// mismatch, or any arithmetic overflow (checked throughout, never
    /// saturated). A node reporting `Infinite` propagates `Infinite`
    /// (termination semantics: its drain never completes). The answer is
    /// instantaneous: later `process`/`drain` calls change stream state,
    /// so callers re-query after advancing the stream.
    ///
    /// Realtime: the chain fold keeps one accumulator and allocates
    /// nothing; the graph fold reuses pre-sized per-node scratch and
    /// allocates only when the topology grew without a fresh derivation.
    /// Both call each node's `tail_length`/`tail_support`
    /// (assumed cheap and side-effect free) and never mutate stream state.
    pub fn tail_length(&self) -> TailLength {
        if self.merge_overflow_poisoned || self.terminal_sink_lifecycle.is_some() {
            return TailLength::Unknown;
        }
        if self.nodes.is_empty() {
            // No nodes, no edges, no outputs: drain completes immediately
            // with zero frames, so the tail is exactly zero (covers both
            // fresh and `PathConfig::None` empty hosts).
            return TailLength::Finite(0);
        }
        if self.is_topologically_linear_chain() || self.is_chain_topology_for_drain() {
            return self.chain_remaining_tail();
        }
        if !self.built {
            return TailLength::Unknown;
        }
        self.graph_remaining_tail()
    }

    /// State-independent tail support in host-output frames, if provable.
    ///
    /// Upper bound over ALL reachable stream states (not just the current
    /// one): drain queues at capacity, merge retention at cap, holdover at
    /// each node's drain envelope, compensation at full plan length, node
    /// tails at [`Plugin::tail_support`], composed through each plugin's
    /// [`Plugin::output_frames_envelope`] transfer bound. This dominates
    /// TRUE future emission in every state — term by term (caps over live
    /// queues, holdover envelopes over retained waves, full plan
    /// compensation over armed/unarmed flushes, per-node support over
    /// per-node emission, envelope transfers over production, with
    /// envelope monotonicity lifting larger-quantum envelopes over
    /// smaller-quantum production) — never by comparison against live
    /// tail values, which it need not dominate. Hosts compose it for
    /// in-flight content. `None` is honest:
    /// any node without the needed envelopes/support, an unbuilt graph,
    /// terminal-sink routing, structural mismatch, or arithmetic overflow.
    /// Phases are deliberately unread (support covers every phase: joins
    /// take twice the maximum over data inputs, which dominates every
    /// armed/unarmed minimum-plus-maximum partition).
    pub fn tail_support(&self) -> Option<u64> {
        if self.terminal_sink_lifecycle.is_some() {
            return None;
        }
        if self.nodes.is_empty() {
            return Some(0);
        }
        if self.is_topologically_linear_chain() || self.is_chain_topology_for_drain() {
            return self.chain_tail_support();
        }
        if !self.built {
            return None;
        }
        self.graph_tail_support()
    }

    /// Chain fold: completed stages emit nothing more; each live stage maps
    /// the running wave through its transfer declaration, then adds its own
    /// live tail (which later stages fold as input). Bypassed stages are
    /// identity with no tail, exactly as chain drain skips them.
    fn chain_remaining_tail(&self) -> TailLength {
        let mut acc: u64 = 0;
        for &node_id in self
            .chain_nodes
            .iter()
            .skip(self.drain_state.completed_prefix)
        {
            let Some(node) = self.nodes.get(&node_id) else {
                return TailLength::Unknown;
            };
            if node.bypassed {
                continue;
            }
            let Ok(input_frames) = usize::try_from(acc) else {
                return TailLength::Unknown;
            };
            let Some(plugin) = self.plugins.get(node_id).and_then(|slot| slot.as_ref()) else {
                return TailLength::Unknown;
            };
            let produced = plugin.output_frames_for_input(input_frames);
            let Ok(produced) = u64::try_from(produced) else {
                return TailLength::Unknown;
            };
            let tail = match plugin.tail_length() {
                TailLength::Finite(frames) => frames,
                TailLength::Infinite => return TailLength::Infinite,
                TailLength::Unknown => match plugin.tail_support() {
                    Some(bound) => bound,
                    None => return TailLength::Unknown,
                },
            };
            let Some(next) = produced.checked_add(tail) else {
                return TailLength::Unknown;
            };
            acc = next;
        }
        TailLength::Finite(acc)
    }

    /// Chain support: the full chain (prefix ignored: support covers every
    /// drain phase) through transfer envelopes plus support tails.
    fn chain_tail_support(&self) -> Option<u64> {
        let mut acc: u64 = 0;
        for &node_id in &self.chain_nodes {
            let node = self.nodes.get(&node_id)?;
            if node.bypassed {
                continue;
            }
            let plugin = self.plugins.get(node_id)?.as_ref()?;
            if plugin.tail_length() == TailLength::Infinite {
                // Infinite live with finite support is incoherent (a
                // proven emission bound contradicts unproven
                // termination): an Infinite node keeps whole-chain
                // support honestly `None`.
                return None;
            }
            let input_frames = usize::try_from(acc).ok()?;
            let produced = plugin.output_frames_envelope(input_frames)?;
            let support = plugin.tail_support()?;
            acc = u64::try_from(produced).ok()?.checked_add(support)?;
        }
        Some(acc)
    }

    /// Live tail of one node in its own output-rate frames. Bypassed nodes
    /// contribute nothing (drain never calls them); otherwise the live
    /// `tail_length`, falling back to the state-independent `tail_support`
    /// bound (which bounds true emission in the current state too, since
    /// support covers every state) when live is `Unknown`.
    fn node_live_tail_frames(&self, node_id: NodeId) -> TailLength {
        let Some(node) = self.nodes.get(&node_id) else {
            return TailLength::Unknown;
        };
        if node.bypassed {
            return TailLength::Finite(0);
        }
        let Some(plugin) = self.plugins.get(node_id).and_then(|slot| slot.as_ref()) else {
            return TailLength::Unknown;
        };
        match plugin.tail_length() {
            TailLength::Finite(frames) => TailLength::Finite(frames),
            TailLength::Infinite => TailLength::Infinite,
            TailLength::Unknown => match plugin.tail_support() {
                Some(bound) => TailLength::Finite(bound),
                None => TailLength::Unknown,
            },
        }
    }

    /// Live content pending on one data edge in destination-clock frames:
    /// drain queue plus process merge retention (both precisions; mixed
    /// retention refuses drain, so at most one holds content), the
    /// source's uncommitted holdover wave (committed edges already hold
    /// their copy in the queue), and pending compensation (exact armed
    /// remainder once the source completed, else the deterministic future
    /// arm length from the plan). Both queue stores divide by the plan's
    /// routed width: `edge_routed_channels` sizes process retention and
    /// the drain plan alike so the two can never diverge, and the
    /// retention transfer moves samples unchanged.
    fn graph_edge_tail_content(&self, edge_index: usize) -> Option<u64> {
        let edge_plan = self.graph_drain_plan.edge_plan.get(edge_index)?;
        let edge = self.edges.get(edge_index)?;
        let drain_len = self
            .graph_drain_state
            .edges
            .get(edge_index)
            .map(|state| state.queue.len())
            .unwrap_or(0);
        let merge_len = self
            .process_buffers
            .as_ref()
            .and_then(|buffers| buffers.merge_queues.get(edge.id))
            .map(VecDeque::len)
            .unwrap_or(0)
            .checked_add(
                self.process_buffers_f64
                    .as_ref()
                    .and_then(|buffers| buffers.merge_queues.get(edge.id))
                    .map(VecDeque::len)
                    .unwrap_or(0),
            )?;
        let total_len = drain_len.checked_add(merge_len)?;
        let width = edge_plan.routed_channels;
        // `checked_div` answers `None` exactly at zero width: the empty
        // store still folds to 0 frames, while a nonempty store at zero
        // width is unprovable content.
        let content = match total_len.checked_div(width) {
            Some(frames) => frames,
            None if total_len == 0 => 0,
            None => return None,
        };
        let committed = self
            .graph_drain_state
            .edge_committed
            .get(edge_index)
            .copied()?;
        let holdover = if committed {
            0
        } else {
            self.graph_drain_state
                .holdover_frames
                .get(edge.from_node)
                .copied()
                .unwrap_or(0)
        };
        let from_complete = self.graph_drain_state.phases.get(edge.from_node)
            == Some(&GraphNodeDrainPhase::Complete);
        let comp = if from_complete {
            self.graph_drain_state
                .edges
                .get(edge_index)
                .map(|state| state.flush_remaining)
                .unwrap_or(0)
        } else {
            edge_plan.comp_frames
        };
        u64::try_from(content)
            .ok()?
            .checked_add(u64::try_from(holdover).ok()?)?
            .checked_add(u64::try_from(comp).ok()?)
    }

    /// Graph live fold in forward topological order. Each edge carries
    /// its stored content (queues, uncommitted holdover share, pending
    /// compensation) plus its source's future output; each node maps its
    /// input (minimum over non-EOF data inputs, which gate consumption,
    /// plus the maximum over EOF-side content, which rides past gating)
    /// through its transfer declaration, then adds its live tail.
    /// Completed nodes emit nothing more (their past lives in the stores).
    /// Sidechain inputs add no audio frames but an unprovable key source
    /// poisons control-dependent audio. Output nodes additionally emit
    /// their holdover wave, output queue, and output compensation. The
    /// host tail is the maximum over outputs (each in its own clock;
    /// exact for single-output and same-rate graphs). `None` scratches
    /// poison only dependent outputs (per-output isolation limits
    /// propagation), but the host answer is all-or-nothing: any
    /// unresolvable output collapses the maximum to `Unknown`.
    fn graph_remaining_tail(&self) -> TailLength {
        let num_slots = self.predecessors.len();
        let f32_held: usize = self
            .process_buffers
            .as_ref()
            .map(|buffers| buffers.merge_queues.iter().map(VecDeque::len).sum())
            .unwrap_or(0);
        let f64_held: usize = self
            .process_buffers_f64
            .as_ref()
            .map(|buffers| buffers.merge_queues.iter().map(VecDeque::len).sum())
            .unwrap_or(0);
        if f32_held > 0 && f64_held > 0 {
            return TailLength::Unknown;
        }
        let mut scratch = self.tail_fold_scratch.borrow_mut();
        if scratch.len() < num_slots {
            scratch.resize(num_slots, None);
        }
        scratch[..num_slots].fill(None);
        for &node_id in self.graph_drain_plan.topo_order.iter() {
            if node_id >= num_slots {
                continue;
            }
            let completed =
                self.graph_drain_state.phases.get(node_id) == Some(&GraphNodeDrainPhase::Complete);
            if completed {
                // Past emissions live in the edge/output stores now; no
                // future flows out regardless of stale input readings.
                scratch[node_id] = Some(0);
                continue;
            }
            let tail_frames = match self.node_live_tail_frames(node_id) {
                TailLength::Finite(frames) => Some(frames),
                TailLength::Infinite => return TailLength::Infinite,
                TailLength::Unknown => None,
            };
            let mut gated_min: Option<u64> = None;
            let mut eof_max: u64 = 0;
            let mut poisoned = tail_frames.is_none();
            if let Some(preds) = self.predecessors.get(node_id) {
                for (position, edge) in preds.iter().enumerate() {
                    let Some(edge_index) = self
                        .predecessor_edge_indices
                        .get(node_id)
                        .and_then(|indices| indices.get(position))
                        .copied()
                    else {
                        poisoned = true;
                        continue;
                    };
                    if edge.edge_type == EdgeType::Sidechain {
                        let key_known = scratch
                            .get(edge.from_node)
                            .and_then(|slot| slot.as_ref())
                            .is_some();
                        if !key_known {
                            poisoned = true;
                        }
                        continue;
                    }
                    let eof = self
                        .graph_drain_state
                        .edges
                        .get(edge_index)
                        .map(|state| state.eof)
                        .unwrap_or(false);
                    let stored = self.graph_edge_tail_content(edge_index);
                    let from_out = scratch.get(edge.from_node).copied().flatten();
                    let (Some(stored), Some(from_out)) = (stored, from_out) else {
                        poisoned = true;
                        continue;
                    };
                    let Some(content) = stored.checked_add(from_out) else {
                        poisoned = true;
                        continue;
                    };
                    if eof {
                        eof_max = eof_max.max(content);
                    } else {
                        gated_min = Some(gated_min.map_or(content, |min| min.min(content)));
                    }
                }
            }
            let node_out = (!poisoned).then(|| {
                let input = gated_min.unwrap_or(0).checked_add(eof_max)?;
                let bypassed = self.nodes.get(&node_id).is_some_and(|node| node.bypassed);
                let produced = if bypassed {
                    input
                } else {
                    let input_frames = usize::try_from(input).ok()?;
                    let plugin = self.plugins.get(node_id)?.as_ref()?;
                    u64::try_from(plugin.output_frames_for_input(input_frames)).ok()?
                };
                produced.checked_add(tail_frames?)
            });
            scratch[node_id] = node_out.flatten();
        }
        if self.output_nodes.is_empty() {
            return TailLength::Finite(0);
        }
        let mut longest: Option<u64> = Some(0);
        for (pos, &node_id) in self.output_nodes.iter().enumerate() {
            let total = (|| -> Option<u64> {
                let out = scratch.get(node_id).copied().flatten()?;
                let holdover = self
                    .graph_drain_state
                    .holdover_frames
                    .get(node_id)
                    .copied()
                    .unwrap_or(0);
                let node = self.nodes.get(&node_id)?;
                let channels = node.output_channels();
                let queued = self
                    .graph_drain_state
                    .output_queues
                    .get(pos)
                    .map(VecDeque::len)
                    .unwrap_or(0);
                // `checked_div` answers `None` exactly at zero width: the
                // empty queue still folds to 0 frames, while a queued
                // backlog at zero width is unprovable content.
                let queue_frames = match queued.checked_div(channels) {
                    Some(frames) => frames,
                    None if queued == 0 => 0,
                    None => return None,
                };
                let completed = self.graph_drain_state.phases.get(node_id)
                    == Some(&GraphNodeDrainPhase::Complete);
                let comp = if completed {
                    self.graph_drain_state
                        .output_flush_remaining
                        .get(pos)
                        .copied()
                        .unwrap_or(0)
                } else {
                    self.graph_drain_plan.output_comp_frames.get(pos).copied()?
                };
                out.checked_add(u64::try_from(holdover).ok()?)?
                    .checked_add(u64::try_from(queue_frames).ok()?)?
                    .checked_add(u64::try_from(comp).ok()?)
            })();
            match total {
                Some(frames) => longest = longest.map(|max| max.max(frames)),
                None => longest = None,
            }
        }
        match longest {
            Some(frames) => TailLength::Finite(frames),
            None => TailLength::Unknown,
        }
    }

    /// Support content of one data edge: drain queue cap plus merge
    /// retention cap, holdover at the source's drain envelope (one
    /// retained wave), and full plan compensation (covers armed and
    /// unarmed alike).
    fn graph_edge_support_content(&self, edge_index: usize) -> Option<u64> {
        let edge_plan = self.graph_drain_plan.edge_plan.get(edge_index)?;
        let edge = self.edges.get(edge_index)?;
        let merge_cap = self
            .process_buffers
            .as_ref()
            .and_then(|buffers| buffers.merge_queue_caps.get(edge.id))
            .copied()
            .or_else(|| {
                self.process_buffers_f64
                    .as_ref()
                    .and_then(|buffers| buffers.merge_queue_caps.get(edge.id))
                    .copied()
            })?;
        let plugin = self.plugins.get(edge.from_node)?.as_ref()?;
        let holdover = plugin.drain_frames_envelope()?;
        u64::try_from(edge_plan.queue_cap_frames)
            .ok()?
            .checked_add(u64::try_from(merge_cap).ok()?)?
            .checked_add(u64::try_from(holdover).ok()?)?
            .checked_add(u64::try_from(edge_plan.comp_frames).ok()?)
    }

    /// Graph support fold: same shape as the live fold but phase-agnostic
    /// (caps, envelopes, full compensation, support tails throughout).
    /// Joins take twice the maximum over data inputs, which dominates the
    /// live minimum-plus-maximum under every armed/unarmed partition.
    /// Edges carry stored caps plus the source's support output, folded
    /// forward like the live flow.
    fn graph_tail_support(&self) -> Option<u64> {
        let num_slots = self.predecessors.len();
        let mut scratch = self.tail_fold_scratch.borrow_mut();
        if scratch.len() < num_slots {
            scratch.resize(num_slots, None);
        }
        scratch[..num_slots].fill(None);
        for &node_id in self.graph_drain_plan.topo_order.iter() {
            if node_id >= num_slots {
                continue;
            }
            let node_out = (|| -> Option<u64> {
                let node = self.nodes.get(&node_id)?;
                let tail = if node.bypassed {
                    0
                } else {
                    let plugin = self.plugins.get(node_id)?.as_ref()?;
                    if plugin.tail_length() == TailLength::Infinite {
                        // See `chain_tail_support`: Infinite nodes stay
                        // unsupported, keeping whole-graph support honest.
                        return None;
                    }
                    plugin.tail_support()?
                };
                let mut widest: u64 = 0;
                if let Some(preds) = self.predecessors.get(node_id) {
                    for (position, edge) in preds.iter().enumerate() {
                        let edge_index = self
                            .predecessor_edge_indices
                            .get(node_id)
                            .and_then(|indices| indices.get(position))
                            .copied()?;
                        let from_out = scratch.get(edge.from_node).copied().flatten()?;
                        if edge.edge_type == EdgeType::Sidechain {
                            continue;
                        }
                        let stored = self.graph_edge_support_content(edge_index)?;
                        widest = widest.max(stored.checked_add(from_out)?);
                    }
                }
                let input = widest.checked_add(widest)?;
                let produced = if node.bypassed {
                    input
                } else {
                    let input_frames = usize::try_from(input).ok()?;
                    let plugin = self.plugins.get(node_id)?.as_ref()?;
                    u64::try_from(plugin.output_frames_envelope(input_frames)?).ok()?
                };
                produced.checked_add(tail)
            })();
            scratch[node_id] = node_out;
        }
        if self.output_nodes.is_empty() {
            return Some(0);
        }
        let mut longest: u64 = 0;
        for (pos, &node_id) in self.output_nodes.iter().enumerate() {
            let out = scratch.get(node_id).copied().flatten()?;
            let plugin = self.plugins.get(node_id)?.as_ref()?;
            let holdover = plugin.drain_frames_envelope()?;
            let queue_cap = self
                .graph_drain_plan
                .output_queue_cap_frames
                .get(pos)
                .copied()?;
            let comp = self.graph_drain_plan.output_comp_frames.get(pos).copied()?;
            longest = longest.max(
                out.checked_add(u64::try_from(holdover).ok()?)?
                    .checked_add(u64::try_from(queue_cap).ok()?)?
                    .checked_add(u64::try_from(comp).ok()?)?,
            );
        }
        Some(longest)
    }
}

impl Host for DawHost {
    fn add_plugin(&mut self, p: Box<dyn Plugin>) -> Result<(), String> {
        self.add_plugin(p)
    }
    fn remove_plugin(&mut self, i: usize) -> Result<Box<dyn Plugin>, String> {
        self.remove_plugin(i)
    }
    fn plugin_count(&self) -> usize {
        self.plugin_count()
    }
    fn get_plugin(&self, i: usize) -> Option<&dyn Plugin> {
        self.get_plugin(i)
    }
    fn set_plugin_parameter(
        &mut self,
        i: usize,
        param_id: &str,
        value: super::super::parameters::ParameterValue,
    ) -> Result<(), String> {
        DawHost::set_plugin_parameter(self, i, param_id, value)
    }
    fn input_channels(&self) -> usize {
        self.input_channels()
    }
    fn output_channels(&self) -> usize {
        self.output_channels()
    }
    fn process(&mut self, i: &[f32], o: &mut [f32]) -> Result<usize, String> {
        self.process(i, o)
    }
    fn process_f64(&mut self, i: &[f64], o: &mut [f64]) -> Result<usize, String> {
        self.process_f64(i, o)
    }
    fn drain_output_frames_max(&self) -> usize {
        self.drain_output_frames_max()
    }
    fn drain(&mut self, output: &mut [f32]) -> Result<PluginDrainResult, String> {
        self.drain(output)
    }
    fn reset(&mut self) {
        self.reset()
    }
    fn total_latency_samples(&self) -> usize {
        self.total_latency_samples()
    }
    fn realtime_quantum_frames(&self) -> usize {
        self.realtime_quantum_frames()
    }
    fn get_plugin_data(&self, i: usize) -> Option<Arc<dyn Any + Send + Sync>> {
        let &node_id = self.chain_nodes.get(i)?;
        self.plugins.get(node_id)?.as_ref()?.get_data()
    }
    fn take_analyzer_contention_stats(&mut self) -> Vec<(usize, u64, u64)> {
        let mut stats = Vec::new();
        for (i, &node_id) in self.chain_nodes.iter().enumerate() {
            if let Some(Some(plugin)) = self.plugins.get_mut(node_id) {
                let (contention, updates) = plugin.take_cache_contention_stats();
                if updates > 0 {
                    stats.push((i, contention, updates));
                }
            }
        }
        stats
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn poll_isolated_external_plugin_workers(&mut self) -> Vec<IsolatedExternalPluginWorkerReport> {
        DawHost::poll_isolated_external_plugin_workers(self)
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn ensure_isolated_external_plugin_workers_running(
        &mut self,
    ) -> Vec<IsolatedExternalPluginWorkerReport> {
        DawHost::ensure_isolated_external_plugin_workers_running(self)
    }
}
