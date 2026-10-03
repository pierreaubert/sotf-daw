//! R17: terminal-sink reprepare sizing across ring growth.
//!
//! The reprepare path (`reprepare_terminal_sink_transport`) re-stages every
//! host buffer on a control thread after a physical ring-size change. Its
//! shared-scratch term sized from live declarations while every sibling
//! preparation (build scratch, node buffers, merge caps, parallel slots)
//! already uses envelope-or-live, so a reprepare landing in a low-live
//! stream state could under-size scratch for a later residual peak.
//!
//! Two facts bound that gap; the tests below pin each, plus a retention variant:
//!
//! 1. Admissible sink routes are identity-geometry by declaration: every
//!    node must report `guarantees_identity_frame_geometry` at equal rates
//!    (`validate_terminal_sink_graph`), and every non-sink node must also
//!    declare `output_frames_for_input(n) == n` for each admitted block
//!    (`validate_terminal_sink_input_geometry`). Those gates pin live == n;
//!    value-coincidence pre/post fix additionally requires exact envelopes
//!    (a loose-but-valid envelope such as `2n` passes both gates while
//!    sizing the extents differently). In-tree publishers publish exact
//!    identity on identity routes, so the coincidence holds for every
//!    route buildable today — and safety holds regardless, since
//!    envelopes dominate live values (growth-only). The lifecycle test
//!    pins the exact
//!    prepared capacities across a ring doubling plus allocation-free
//!    post-reprepare blocks (odd and MAX_BLOCK) with bitwise content.
//!    A retention variant reprepares with a non-empty pending span on a
//!    latency-bearing route and pins span preservation, latency
//!    stability, and delayed-bitwise continuity.
//! 2. Genuinely residual-walking geometry is refused the route loudly.
//!    The refusal test drives a chunk-blocked expander with F1-exact
//!    declaration arithmetic directly (proving it really walks, so the
//!    refusal is correct rather than vacuous), then proves the host
//!    rejects it from both sink processing and reprepare, naming the
//!    identity gate.
//!
//! The extents correction itself (envelope-or-live, matching the build
//! path) is justified by uniformity with its five sibling sites and by
//! the over-`MAX_BLOCK` behavior after ring growth, where live sizing
//! could re-grow once per residual peak instead of preparing once. The
//! contracted domain (blocks within `MAX_BLOCK`) is additionally covered
//! by reprepare retention, which never shrinks staged storage.

use super::super::{
    daw_host::{DawHost, SinkPreflightError, SinkProcessError, TerminalSinkRecoveryError},
    graph_edge::GraphEdge,
};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginInfo, ProcessContext, SinkAppendFailure, SinkQueueState, SinkServiceFailure,
    SinkTailPreflightError, SinkTransportFormat, SinkTransportRecoveryError,
    SinkTransportRecoveryStatus, SinkTransportRepreparePlan, TerminalSink,
};
use crate::test_utils::measure_heap_activity;
use std::sync::{Arc, Mutex, MutexGuard};

/// Honest identity-geometry gain stage: every valid input frame count is
/// preserved by processing, so it declares (and proves) the guarantee the
/// terminal-sink route requires. The x2.0 factor is bit-exact in f32 and
/// lets the lifecycle test verify content end to end.
struct IdentityGain {
    channels: usize,
    factor: f32,
}

impl IdentityGain {
    fn new(channels: usize) -> Self {
        assert!(channels > 0, "identity gain needs at least one channel");
        Self {
            channels,
            factor: 2.0,
        }
    }
}

impl Plugin for IdentityGain {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("IdentityGain", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<usize, String> {
        assert!(
            input.len().is_multiple_of(self.channels),
            "identity gain needs whole input frames"
        );
        assert!(
            output.len() >= input.len(),
            "identity gain output must fit the input block"
        );
        for (o, &i) in output.iter_mut().zip(input.iter()) {
            *o = i * self.factor;
        }
        Ok(input.len() / self.channels)
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }
    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }
    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }
}

/// Honest identity-geometry pure delay: every block emits exactly as many
/// frames as it consumes (only shifted in time), so the terminal-sink
/// route admits it while its delay line carries real history. The ring is
/// pre-sized; processing allocates nothing.
struct IdentityDelay {
    channels: usize,
    delay_frames: usize,
    ring: Vec<f32>,
    cursor: usize,
}

impl IdentityDelay {
    fn new(channels: usize, delay_frames: usize) -> Self {
        assert!(channels > 0, "delay needs at least one channel");
        assert!(delay_frames > 0, "delay needs a nonzero span");
        let span = channels
            .checked_mul(delay_frames)
            .expect("delay ring extent fits addressable samples");
        Self {
            channels,
            delay_frames,
            ring: vec![0.0; span],
            cursor: 0,
        }
    }
}

impl Plugin for IdentityDelay {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("IdentityDelay", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<usize, String> {
        assert!(
            input.len().is_multiple_of(self.channels),
            "delay needs whole input frames"
        );
        assert!(
            output.len() >= input.len(),
            "delay output must fit the input block"
        );
        let span = self.ring.len();
        for (slot, &sample) in output.iter_mut().zip(input.iter()) {
            *slot = self.ring[self.cursor];
            self.ring[self.cursor] = sample;
            self.cursor += 1;
            if self.cursor == span {
                self.cursor = 0;
            }
        }
        Ok(input.len() / self.channels)
    }
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.cursor = 0;
    }
    fn latency_samples(&self) -> usize {
        self.delay_frames
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }
    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }
    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }
}

/// Shared control/observation state for [`ReprepareTestSink`].
///
/// The test stages a transport ring change through `target_buffer_frames`
/// and reads back the committed geometry plus the recorded stream. The
/// record buffer is pre-sized at construction (a real sink's prepared
/// storage); appends within it allocate nothing, and anything past the
/// exact bound fails loudly instead of growing.
struct SinkShared {
    target_buffer_frames: Option<usize>,
    prepared_buffer_frames: usize,
    capacity_frames: usize,
    pending_frames: usize,
    record_cap_samples: usize,
    recorded: Vec<f32>,
    retain_on_service: bool,
}

/// First in-tree [`TerminalSink`] double: a zero-output plugin with a
/// bounded prepared queue, a healthy transport (one service step drains
/// everything retained), and a scripted ring-size change for reprepare.
///
/// The queue invariant `pending + free == capacity` always holds. The
/// double publishes no process envelope, so the corrected extents term
/// takes its live-fallback branch here while the source above takes the
/// envelope branch: one lifecycle covers both.
struct ReprepareTestSink {
    channels: usize,
    sample_rate: u32,
    shared: Arc<Mutex<SinkShared>>,
}

impl ReprepareTestSink {
    /// Build a sink with `ring_frames` of prepared queue and an exact
    /// `record_cap_samples` content record. Returns the plugin plus the
    /// shared handle the test uses to script and observe it.
    fn new(
        channels: usize,
        sample_rate: u32,
        ring_frames: usize,
        record_cap_samples: usize,
    ) -> (Self, Arc<Mutex<SinkShared>>) {
        assert!(channels > 0, "sink needs at least one channel");
        assert!(ring_frames > 0, "sink needs a nonzero prepared ring");
        let shared = Arc::new(Mutex::new(SinkShared {
            target_buffer_frames: None,
            prepared_buffer_frames: ring_frames,
            capacity_frames: ring_frames,
            pending_frames: 0,
            record_cap_samples,
            recorded: Vec::with_capacity(record_cap_samples),
            retain_on_service: false,
        }));
        (
            Self {
                channels,
                sample_rate,
                shared: Arc::clone(&shared),
            },
            shared,
        )
    }

    fn lock(&self) -> MutexGuard<'_, SinkShared> {
        self.shared.lock().unwrap()
    }
}

impl Plugin for ReprepareTestSink {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("ReprepareTestSink", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        0
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        _input: &[f32],
        _output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<usize, String> {
        // The sink-mode render routes the merged source block around this
        // node straight into staging, then hands it over through
        // `append_preflighted`; a render call here would mean the host
        // lost the terminal-node exemption, so refuse loudly.
        Err("terminal sink node never renders audio".into())
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn output_frames_for_input(&self, _input_frames: usize) -> usize {
        0
    }
    fn guarantees_identity_frame_geometry(&self) -> bool {
        // The sink preserves every admitted input frame into its prepared
        // queue (append takes exactly the preflighted block); it emits
        // nothing downstream, and the host exempts this node from the
        // per-block frame check for exactly that reason.
        true
    }
    fn terminal_sink(&self) -> Option<&dyn TerminalSink> {
        Some(self)
    }
    fn terminal_sink_mut(&mut self) -> Option<&mut dyn TerminalSink> {
        Some(self)
    }
}

impl TerminalSink for ReprepareTestSink {
    fn queue_state(&self) -> SinkQueueState {
        let shared = self.lock();
        assert!(
            shared.pending_frames <= shared.capacity_frames,
            "sink queue over-retained: {} pending over {} capacity",
            shared.pending_frames,
            shared.capacity_frames
        );
        SinkQueueState {
            pending_frames: shared.pending_frames,
            free_prepared_frames: shared.capacity_frames - shared.pending_frames,
            capacity_frames: shared.capacity_frames,
        }
    }

    fn prepared_transport_format(&self) -> Option<SinkTransportFormat> {
        let shared = self.lock();
        Some(SinkTransportFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            buffer_frames: shared.prepared_buffer_frames,
        })
    }

    fn preflight_append(
        &self,
        maximum_frames: usize,
        _context: &ProcessContext,
    ) -> Result<(), SinkTailPreflightError> {
        let state = self.queue_state();
        if maximum_frames > state.free_prepared_frames {
            return Err(SinkTailPreflightError::CapacityExceeded {
                required_frames: maximum_frames,
                free_frames: state.free_prepared_frames,
            });
        }
        Ok(())
    }

    fn append_preflighted(
        &mut self,
        input: &[f32],
        _context: &ProcessContext,
    ) -> Result<(), SinkAppendFailure> {
        assert!(
            input.len().is_multiple_of(self.channels),
            "sink append needs whole frames"
        );
        let mut shared = self.lock();
        let frames = input.len() / self.channels;
        let free = shared.capacity_frames - shared.pending_frames;
        assert!(
            frames <= free,
            "sink append of {frames} frames exceeds {free} free prepared frames"
        );
        assert!(
            shared.recorded.len() + input.len() <= shared.record_cap_samples,
            "sink record cap {} samples exhausted",
            shared.record_cap_samples
        );
        shared.recorded.extend_from_slice(input);
        shared.pending_frames += frames;
        Ok(())
    }

    fn service_pending(
        &mut self,
        _context: &ProcessContext,
    ) -> Result<SinkQueueState, SinkServiceFailure> {
        // Healthy-transport model: one bounded service step drains every
        // retained frame, so post-service pending is deterministically 0 —
        // unless the test stages retention, in which case the span stays.
        if !self.lock().retain_on_service {
            self.lock().pending_frames = 0;
        }
        Ok(self.queue_state())
    }

    fn reprepare_plan(&self) -> Result<SinkTransportRepreparePlan, SinkTransportRecoveryError> {
        let shared = self.lock();
        let Some(target) = shared.target_buffer_frames else {
            return Err(SinkTransportRecoveryError::Unsupported);
        };
        Ok(SinkTransportRepreparePlan {
            prepared_format: SinkTransportFormat {
                sample_rate: self.sample_rate,
                channels: self.channels,
                buffer_frames: shared.prepared_buffer_frames,
            },
            target_format: SinkTransportFormat {
                sample_rate: self.sample_rate,
                channels: self.channels,
                buffer_frames: target,
            },
            pending_frames: shared.pending_frames,
            queue_capacity_frames: target.max(shared.pending_frames),
            latency_samples: 0,
            configuration_changed: true,
        })
    }

    fn reprepare_transport(
        &mut self,
        plan: SinkTransportRepreparePlan,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        // Stale-plan guard: the committed plan must equal a fresh
        // observation, else the transport moved under the host staging.
        let fresh = self.reprepare_plan()?;
        if fresh != plan {
            return Err(SinkTransportRecoveryError::StalePlan);
        }
        let mut shared = self.lock();
        shared.capacity_frames = plan.queue_capacity_frames;
        shared.prepared_buffer_frames = plan.target_format.buffer_frames;
        shared.target_buffer_frames = None;
        Ok(SinkTransportRecoveryStatus::Ready)
    }
}

/// Chunk-blocked frame expander with F1-exact declaration arithmetic.
///
/// Live production walks with the sub-chunk residual (`((n + residual) /
/// CHUNK) * OUT_PER_CHUNK`), so one input size declares anywhere from
/// the aligned count up to one chunk more. The published envelope is the
/// residual-free maximum. Sub-chunk input buffers entirely (production
/// 0), exactly like a real chunked converter below its first block
/// boundary. Deliberately not identity-guaranteed: this is the geometry
/// the sink route must refuse.
struct ChunkWalkFixture {
    channels: usize,
    residual: usize,
    emitted_frames: u64,
}

impl ChunkWalkFixture {
    const CHUNK: usize = 1000;
    const OUT_PER_CHUNK: usize = 4005;

    fn new(channels: usize) -> Self {
        assert!(channels > 0, "chunk fixture needs at least one channel");
        Self {
            channels,
            residual: 0,
            emitted_frames: 0,
        }
    }

    /// Live declaration at the current residual. Saturating: declaration
    /// arithmetic never wraps, and the test domain sits far below
    /// saturation, so saturation is unreachable rather than load-bearing.
    fn declare(&self, input_frames: usize) -> usize {
        (input_frames.saturating_add(self.residual) / Self::CHUNK)
            .saturating_mul(Self::OUT_PER_CHUNK)
    }

    /// Residual-free upper bound over every stream state, with checked
    /// arithmetic (overflow answers unknown, per the envelope contract).
    fn envelope_for(input_frames: usize) -> Option<usize> {
        input_frames
            .checked_add(Self::CHUNK - 1)?
            .checked_div(Self::CHUNK)?
            .checked_mul(Self::OUT_PER_CHUNK)
    }
}

impl Plugin for ChunkWalkFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("ChunkWalkFixture", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<usize, String> {
        assert!(
            input.len().is_multiple_of(self.channels),
            "chunk fixture needs whole input frames"
        );
        let produced = self.declare(input.len() / self.channels);
        let need = produced
            .checked_mul(self.channels)
            .ok_or("chunk fixture declared extent overflow")?;
        assert!(
            output.len() >= need,
            "chunk fixture output must fit {produced} declared frames"
        );
        for frame in 0..produced {
            let sample = (self.emitted_frames.wrapping_add(frame as u64) % 997) as f32 * 0.25;
            output[frame * self.channels..(frame + 1) * self.channels].fill(sample);
        }
        self.residual = (self.residual + input.len() / self.channels) % Self::CHUNK;
        self.emitted_frames = self.emitted_frames.wrapping_add(produced as u64);
        Ok(produced)
    }
    fn reset(&mut self) {
        self.residual = 0;
        self.emitted_frames = 0;
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        self.declare(input_frames)
    }
    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Self::envelope_for(input_frames)
    }
}

/// Deterministic content pattern: exact in f32, position-sensitive,
/// finite (the sink path rejects non-finite input before admission).
fn pattern_sample(global_sample: usize) -> f32 {
    (global_sample % 997) as f32 * 0.25
}

fn pattern_block(frames: usize, channels: usize, start_sample: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|index| pattern_sample(start_sample + index))
        .collect()
}

#[test]
fn terminal_sink_reprepare_grows_ring_without_rt_growth() {
    const CHANNELS: usize = 32;
    const RATE: u32 = 48_000;
    const BUILD_RING: usize = 8192;
    const GROWN_RING: usize = 16384;
    // Build scratch: the identity path at seed 8192 is 8192 frames times
    // the fixed x32 build multiplier (no fan-in, and the doubles publish
    // no drain envelopes, so neither merge-join nor chain drain prep
    // adds above it).
    const BUILD_SCRATCH: usize = 262_144;
    // Reprepare scratch: the envelope path at seed 16384 is 16384 frames
    // times 32 real channels, which dominates the retained build scratch,
    // so the corrected term's value is exactly observable here. Identity
    // graphs declare live == envelope, so this pins the lifecycle
    // arithmetic rather than distinguishing pre/post fix; the module proof
    // and the refusal test carry the sizing argument.
    const GROWN_SCRATCH: usize = 524_288;
    // Delay scratch is graph samples plus channel maximum on both paths
    // (the build adds a literal 32, the reprepare the observed maximum).
    const BUILD_DELAY: usize = 262_176;
    const GROWN_DELAY: usize = 524_320;
    const PRE_FRAMES: usize = 1000;
    const ODD_FRAMES: usize = 1999;
    const MAX_FRAMES: usize = 8192;

    let mut host = DawHost::new(CHANNELS, RATE);
    host.enable_terminal_sink_mode().unwrap();
    let source = host
        .add_node("source".to_string(), Box::new(IdentityGain::new(CHANNELS)))
        .unwrap();
    let record_cap = (PRE_FRAMES + ODD_FRAMES + MAX_FRAMES) * CHANNELS;
    let (sink, shared) = ReprepareTestSink::new(CHANNELS, RATE, BUILD_RING, record_cap);
    let sink_id = host.add_node("sink".to_string(), Box::new(sink)).unwrap();
    host.add_edge(GraphEdge::new(source, sink_id)).unwrap();
    host.build().unwrap();

    assert!(
        host.has_identity_frame_geometry(),
        "identity gain plus sink must satisfy the route gate"
    );
    {
        let buffers = host.process_buffers.as_ref().unwrap();
        assert_eq!(
            buffers.scratch_input.len(),
            BUILD_SCRATCH,
            "build sizes shared scratch"
        );
        assert_eq!(
            buffers.delay_scratch.len(),
            BUILD_DELAY,
            "build sizes delay scratch"
        );
        let buffers_f64 = host.process_buffers_f64.as_ref().unwrap();
        assert_eq!(
            buffers_f64.scratch_input.len(),
            BUILD_SCRATCH,
            "build sizes f64 shared scratch"
        );
        assert_eq!(
            buffers_f64.delay_scratch.len(),
            BUILD_DELAY,
            "build sizes f64 delay scratch"
        );
    }

    // A live route first: the reprepare below must preserve a running
    // programme, not a fresh build.
    let pre = pattern_block(PRE_FRAMES, CHANNELS, 0);
    let result = host.process_to_sink(&pre).unwrap();
    assert_eq!(
        result.input_frames_consumed, PRE_FRAMES,
        "pre block consumed"
    );
    assert_eq!(
        result.pending_sink_frames, 0,
        "service drains the pre block"
    );

    shared.lock().unwrap().target_buffer_frames = Some(GROWN_RING);
    let status = host.reprepare_terminal_sink_transport().unwrap();
    assert_eq!(
        status,
        SinkTransportRecoveryStatus::Ready,
        "ring growth commits"
    );
    assert!(
        host.has_identity_frame_geometry(),
        "route gate holds after reprepare"
    );
    {
        let shared = shared.lock().unwrap();
        assert_eq!(
            shared.capacity_frames, GROWN_RING,
            "queue adopts the grown ring"
        );
        assert_eq!(
            shared.prepared_buffer_frames, GROWN_RING,
            "transport adopts the grown ring"
        );
        assert_eq!(
            shared.pending_frames, 0,
            "no retained audio across reprepare"
        );
    }
    {
        let buffers = host.process_buffers.as_ref().unwrap();
        assert_eq!(
            buffers.scratch_input.len(),
            GROWN_SCRATCH,
            "reprepare grows shared scratch to the envelope term"
        );
        assert_eq!(
            buffers.delay_scratch.len(),
            GROWN_DELAY,
            "reprepare grows delay scratch"
        );
        let buffers_f64 = host.process_buffers_f64.as_ref().unwrap();
        assert_eq!(
            buffers_f64.scratch_input.len(),
            GROWN_SCRATCH,
            "reprepare grows f64 shared scratch"
        );
        assert_eq!(
            buffers_f64.delay_scratch.len(),
            GROWN_DELAY,
            "reprepare grows f64 delay scratch"
        );
    }
    println!(
        "sink reprepare: ring {BUILD_RING} -> {GROWN_RING}, \
         scratch {BUILD_SCRATCH} -> {GROWN_SCRATCH}"
    );

    // Post-reprepare blocks (odd, then MAX_BLOCK) must not grow: the
    // no-alloc ceiling applies after reprepare exactly as after build.
    let odd = pattern_block(ODD_FRAMES, CHANNELS, PRE_FRAMES * CHANNELS);
    let mut odd_consumed = 0;
    let mut odd_pending = 0;
    let (odd_allocs, odd_deallocs) = measure_heap_activity(|| {
        let result = host.process_to_sink(&odd).unwrap();
        odd_consumed = result.input_frames_consumed;
        odd_pending = result.pending_sink_frames;
    });
    println!(
        "sink post-reprepare odd block {ODD_FRAMES}: consumed {odd_consumed}, \
         heap ({odd_allocs}, {odd_deallocs})"
    );
    assert_eq!(
        (odd_allocs, odd_deallocs),
        (0, 0),
        "odd block must not grow after reprepare"
    );
    assert_eq!(odd_consumed, ODD_FRAMES, "odd block consumed");
    assert_eq!(odd_pending, 0, "service drains the odd block");

    let max_start = (PRE_FRAMES + ODD_FRAMES) * CHANNELS;
    let max = pattern_block(MAX_FRAMES, CHANNELS, max_start);
    let mut max_consumed = 0;
    let mut max_pending = 0;
    let (max_allocs, max_deallocs) = measure_heap_activity(|| {
        let result = host.process_to_sink(&max).unwrap();
        max_consumed = result.input_frames_consumed;
        max_pending = result.pending_sink_frames;
    });
    println!(
        "sink post-reprepare max block {MAX_FRAMES}: consumed {max_consumed}, \
         heap ({max_allocs}, {max_deallocs})"
    );
    assert_eq!(
        (max_allocs, max_deallocs),
        (0, 0),
        "max block must not grow after reprepare"
    );
    assert_eq!(max_consumed, MAX_FRAMES, "max block consumed");
    assert_eq!(max_pending, 0, "service drains the max block");

    // Whole-stream content, one pure function of the global sample index,
    // so block splits cannot hide a drop, duplication, or reorder.
    {
        let shared = shared.lock().unwrap();
        assert_eq!(
            shared.recorded.len(),
            record_cap,
            "sink recorded every frame"
        );

        for (index, &sample) in shared.recorded.iter().enumerate() {
            assert_eq!(
                sample,
                pattern_sample(index) * 2.0,
                "sink content bit-exact at sample {index}"
            );
        }
    }

    // Counter-liveness probe: the zeros above are only meaningful if the
    // counter observes this binary, so watch it fire on deliberate growth.
    let (probe_allocs, _) = measure_heap_activity(|| {
        let mut probe = Vec::with_capacity(4);
        for value in 0..100u8 {
            probe.push(value);
        }
    });
    assert!(
        probe_allocs > 0,
        "heap counter must observe deliberate growth"
    );
    println!("sink reprepare: heap counter live ({probe_allocs} observed allocs)");
}

#[test]
fn terminal_sink_reprepare_preserves_retained_span_and_latency() {
    const CHANNELS: usize = 2;
    const RATE: u32 = 48_000;
    const BUILD_RING: usize = 8192;
    const GROWN_RING: usize = 16384;
    const DELAY_FRAMES: usize = 48;
    // Retention dominance: with two channels the reprepare term stays
    // below the x32 build scratch, so the pins below prove the old
    // storage (and its content) survives the swap unchanged.
    const SCRATCH: usize = 262_144;
    const FIRST_FRAMES: usize = 1000;
    const SECOND_FRAMES: usize = 2000;
    const THIRD_FRAMES: usize = 1500;
    const FOURTH_FRAMES: usize = 8192;

    let mut host = DawHost::new(CHANNELS, RATE);
    host.enable_terminal_sink_mode().unwrap();
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(IdentityDelay::new(CHANNELS, DELAY_FRAMES)),
        )
        .unwrap();
    let record_cap = (FIRST_FRAMES + SECOND_FRAMES + THIRD_FRAMES + FOURTH_FRAMES) * CHANNELS;
    let (sink, shared) = ReprepareTestSink::new(CHANNELS, RATE, BUILD_RING, record_cap);
    let sink_id = host.add_node("sink".to_string(), Box::new(sink)).unwrap();
    host.add_edge(GraphEdge::new(source, sink_id)).unwrap();
    host.build().unwrap();

    assert!(
        host.has_identity_frame_geometry(),
        "delay plus sink must satisfy the route gate"
    );
    let latency = host.total_latency_samples();
    assert_eq!(latency, DELAY_FRAMES, "route latency is the delay span");
    {
        let buffers = host.process_buffers.as_ref().unwrap();
        let scratch = buffers.scratch_input.len();
        let buffers_f64 = host.process_buffers_f64.as_ref().unwrap();
        let scratch_f64 = buffers_f64.scratch_input.len();
        assert_eq!(scratch, SCRATCH, "build sizes shared scratch");
        assert_eq!(scratch_f64, SCRATCH, "build sizes f64 scratch");
    }

    // Retaining transport: service keeps every appended span, so the
    // reprepare below must preserve a genuinely non-empty retention.
    shared.lock().unwrap().retain_on_service = true;
    let first = pattern_block(FIRST_FRAMES, CHANNELS, 0);
    let result = host.process_to_sink(&first).unwrap();
    let consumed1 = result.input_frames_consumed;
    let pending1 = result.pending_sink_frames;
    assert_eq!(consumed1, FIRST_FRAMES, "first block consumed");
    assert_eq!(pending1, FIRST_FRAMES, "first block retained");

    let second_start = FIRST_FRAMES * CHANNELS;
    let second = pattern_block(SECOND_FRAMES, CHANNELS, second_start);
    let result = host.process_to_sink(&second).unwrap();
    let consumed2 = result.input_frames_consumed;
    let pending2 = result.pending_sink_frames;
    let retained = FIRST_FRAMES + SECOND_FRAMES;
    assert_eq!(consumed2, SECOND_FRAMES, "second block consumed");
    assert_eq!(pending2, retained, "retention accumulates");
    let pending_before = shared.lock().unwrap().pending_frames;
    assert_eq!(pending_before, retained, "pending spans both pre blocks");

    shared.lock().unwrap().target_buffer_frames = Some(GROWN_RING);
    let status = host.reprepare_terminal_sink_transport().unwrap();
    assert_eq!(
        status,
        SinkTransportRecoveryStatus::Ready,
        "ring growth commits"
    );
    let pending_after = shared.lock().unwrap().pending_frames;
    assert_eq!(pending_after, pending_before, "span preserved");
    let capacity = shared.lock().unwrap().capacity_frames;
    let transport = shared.lock().unwrap().prepared_buffer_frames;
    assert_eq!(capacity, GROWN_RING, "queue adopts the grown ring");
    assert_eq!(transport, GROWN_RING, "transport adopts the grown ring");
    let latency = host.total_latency_samples();
    assert_eq!(latency, DELAY_FRAMES, "latency survives reprepare");
    {
        let buffers = host.process_buffers.as_ref().unwrap();
        let scratch = buffers.scratch_input.len();
        let buffers_f64 = host.process_buffers_f64.as_ref().unwrap();
        let scratch_f64 = buffers_f64.scratch_input.len();
        assert_eq!(scratch, SCRATCH, "reprepare retains shared scratch");
        assert_eq!(scratch_f64, SCRATCH, "reprepare retains f64 scratch");
    }
    println!("retain reprepare: pending {pending_before} -> {pending_after}");
    println!("retain reprepare: latency {DELAY_FRAMES}, scratch {SCRATCH}");

    let third_start = retained * CHANNELS;
    let third = pattern_block(THIRD_FRAMES, CHANNELS, third_start);
    let mut consumed3 = 0;
    let mut pending3 = 0;
    let (allocs3, deallocs3) = measure_heap_activity(|| {
        let result = host.process_to_sink(&third).unwrap();
        consumed3 = result.input_frames_consumed;
        pending3 = result.pending_sink_frames;
    });
    let retained3 = retained + THIRD_FRAMES;
    assert_eq!(consumed3, THIRD_FRAMES, "third block consumed");
    assert_eq!(pending3, retained3, "retention still accumulates");
    assert_eq!((allocs3, deallocs3), (0, 0), "third block silent");
    println!("retain block {THIRD_FRAMES}: heap ({allocs3}, {deallocs3})");

    let fourth_start = retained3 * CHANNELS;
    let fourth = pattern_block(FOURTH_FRAMES, CHANNELS, fourth_start);
    let mut consumed4 = 0;
    let mut pending4 = 0;
    let (allocs4, deallocs4) = measure_heap_activity(|| {
        let result = host.process_to_sink(&fourth).unwrap();
        consumed4 = result.input_frames_consumed;
        pending4 = result.pending_sink_frames;
    });
    let retained4 = retained3 + FOURTH_FRAMES;
    assert_eq!(consumed4, FOURTH_FRAMES, "fourth block consumed");
    assert_eq!(pending4, retained4, "retention reaches the max block");
    assert_eq!((allocs4, deallocs4), (0, 0), "max block silent");
    println!("retain block {FOURTH_FRAMES}: heap ({allocs4}, {deallocs4})");

    // Service the retained span out through the normal empty-input path
    // now that retention is off: the whole stream must still be intact.
    shared.lock().unwrap().retain_on_service = false;
    let result = host.process_to_sink(&[]).unwrap();
    let consumed = result.input_frames_consumed;
    let pending = result.pending_sink_frames;
    assert_eq!(consumed, 0, "service call consumes nothing");
    assert_eq!(pending, 0, "retained span services out");
    {
        let shared = shared.lock().unwrap();
        let recorded = &shared.recorded;
        assert_eq!(recorded.len(), record_cap, "sink recorded every frame");
        let delay_samples = DELAY_FRAMES * CHANNELS;
        for (index, &sample) in recorded.iter().enumerate() {
            let expected = if index < delay_samples {
                0.0
            } else {
                pattern_sample(index - delay_samples)
            };
            assert_eq!(sample, expected, "delayed content at {index}");
        }
    }
}

#[test]
fn terminal_sink_route_refuses_residual_walking_geometry() {
    // The double genuinely walks first: one input size declares a single
    // chunk fresh and two chunks after a sub-chunk feed, so the refusal
    // below is correct rather than vacuous.
    let mut fixture = ChunkWalkFixture::new(2);
    assert_eq!(
        fixture.output_frames_for_input(1500),
        4005,
        "fresh fixture declares one chunk"
    );
    let feed = vec![0.25f32; 600 * 2];
    let mut feed_out = Vec::new();
    let buffered = fixture
        .process(&feed, &mut feed_out, &ProcessContext::new(48_000, 600))
        .unwrap();
    assert_eq!(buffered, 0, "sub-chunk input buffers entirely");
    assert_eq!(
        fixture.output_frames_for_input(1500),
        8010,
        "fed residual declares two chunks"
    );
    assert_eq!(
        ChunkWalkFixture::envelope_for(1500),
        Some(8010),
        "envelope dominates both walked states"
    );
    let feed2 = vec![0.25f32; 1500 * 2];
    let mut feed2_out = vec![f32::NAN; 8010 * 2];
    let produced = fixture
        .process(&feed2, &mut feed2_out, &ProcessContext::new(48_000, 1500))
        .unwrap();
    assert_eq!(produced, 8010, "production honors the fed declaration");
    assert_eq!(
        &feed2_out[..6],
        &[0.0, 0.0, 0.25, 0.25, 0.5, 0.5][..],
        "emitted content follows the frame pattern"
    );

    // The same geometry builds as an ordinary graph (refusal is
    // route-level, not admission-level) but must fail the sink gate.
    let mut host = DawHost::new(2, 48_000);
    host.enable_terminal_sink_mode().unwrap();
    let source = host
        .add_node("source".to_string(), Box::new(ChunkWalkFixture::new(2)))
        .unwrap();
    let (sink, _) = ReprepareTestSink::new(2, 48_000, 8192, 0);
    let sink_id = host.add_node("sink".to_string(), Box::new(sink)).unwrap();
    host.add_edge(GraphEdge::new(source, sink_id)).unwrap();
    host.build().unwrap();
    assert!(
        !host.has_identity_frame_geometry(),
        "walking declarations must trip the route gate"
    );
    let input = vec![0.25f32; 64 * 2];
    assert_eq!(
        host.process_to_sink(&input),
        Err(SinkProcessError::RetryablePreflight(
            SinkPreflightError::Sink(SinkTailPreflightError::InvalidGeometry)
        ),),
        "sink processing must refuse the non-identity route"
    );
    let Err(TerminalSinkRecoveryError::InvalidRoute(message)) =
        host.reprepare_terminal_sink_transport()
    else {
        panic!("reprepare must refuse the non-identity route");
    };
    assert!(
        message.contains("identity frame geometry"),
        "refusal names the gate, got: {message}"
    );
    println!("sink refusal: walking 4005 -> 8010 refused as InvalidGeometry/InvalidRoute");
}
