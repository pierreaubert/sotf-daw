// Rust guideline compliant 2026-02-21
pub use super::config::*;
use super::delay_line::DelayLine;
use super::factory::{build_path_from_config, build_path_from_config_with_factory};
use super::types::ABCompareData;
use math_audio_iir_fir::{Biquad, BiquadCoefficients, BiquadFilterType};
use sotf_host::analyzer::RealTimeCache;
use sotf_host::auto_gain::{AutoGain, AutoGainLoudnessType, AutoGainParams};
use sotf_host::host::DawHost;
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::smoothing::Smoother;
use std::any::Any;
use std::sync::Arc;

pub(super) struct TransitionSmoothers {
    pub(super) mix: Smoother,
    pub(super) bypass: Smoother,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainLifecycle {
    Accepting,
    Draining,
    Complete,
    ResetRequired,
}

#[derive(Debug, Default, Clone, Copy)]
struct ChildDrainProgress {
    queued_frames: usize,
    consumed_frames: usize,
    complete: bool,
}

impl ChildDrainProgress {
    fn queued_remaining(self) -> usize {
        debug_assert!(self.consumed_frames <= self.queued_frames);
        self.queued_frames - self.consumed_frames
    }
}

/// Last two inputs and outputs of one band-mask biquad: the observable-state
/// history from which the rigorous flush length is derived at drain time.
/// The mask biquads run Direct Form I (`use_tdf2: false`), so recorded I/O
/// pairs are the true filter state registers — including across mid-stream
/// coefficient retunes, which preserve DF-I state by design.
#[derive(Debug, Clone, Copy, Default)]
struct MaskStageHistory {
    in1: f64,
    in2: f64,
    out1: f64,
    out2: f64,
}

impl MaskStageHistory {
    fn is_silent(self) -> bool {
        self.in1 == 0.0 && self.in2 == 0.0 && self.out1 == 0.0 && self.out2 == 0.0
    }

    fn observe(&mut self, input: f64, output: f64) {
        self.in2 = self.in1;
        self.in1 = input;
        self.out2 = self.out1;
        self.out1 = output;
    }
}

/// Drain topology class of one nested path, classified once on the control
/// thread at construction. Path configs are structural parameters (rejected
/// by `set_parameter`), so the class cannot change at runtime and
/// `begin_drain` can rely on the cached value while staying allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathTopology {
    /// Empty, single-plugin, rack, or linear-graph path.
    Linear,
    /// Connected single-source/single-sink DAG with direct edges. The host's
    /// graph-drain scheduler composes branches; ABCompare keeps its per-child
    /// black-box staging and cursor pairing on the composed host output.
    Dag,
    /// Multi-sink graph the factory joins behind one transparent output
    /// node. The built host is single-sink: the host's fan-in merge
    /// (per-edge retention plus minimum-depth summation) composes
    /// heterogeneous per-call production losslessly through the joined
    /// output, in process and at EOF alike.
    JoinedMultiSink,
    /// Multi-source single-sink, cyclic, remapped, or otherwise unsupported
    /// graph. Drain refuses these before either child advances; process still
    /// runs them (single-sink collection is exact, and the pre-existing
    /// multi-source shapes are host-defined).
    Unsupported,
}

/// Classify a path config for process and drain. Accepts everything the
/// accepted linear validator accepted, plus connected single-source/
/// single-sink DAGs with direct edges, plus factory-joined multi-sink
/// graphs. Control thread only: reachability and cycle checks may
/// allocate.
fn classify_path_topology(config: &PathConfig) -> PathTopology {
    let PathConfig::Graph { nodes, edges } = config else {
        return PathTopology::Linear;
    };
    if nodes.is_empty() {
        return if edges.is_empty() {
            PathTopology::Linear
        } else {
            PathTopology::Unsupported
        };
    }
    for edge in edges {
        if edge.channel_map.is_some() || edge.destination_offset != 0 {
            return PathTopology::Unsupported;
        }
        if edge.from == edge.to {
            return PathTopology::Unsupported;
        }
        let endpoints_known = nodes.iter().any(|node| node.id == edge.from)
            && nodes.iter().any(|node| node.id == edge.to);
        if !endpoints_known {
            return PathTopology::Unsupported;
        }
    }
    let sources: Vec<&str> = nodes
        .iter()
        .filter(|node| !edges.iter().any(|edge| edge.to == node.id))
        .map(|node| node.id.as_str())
        .collect();
    let sinks: Vec<&str> = nodes
        .iter()
        .filter(|node| !edges.iter().any(|edge| edge.from == node.id))
        .map(|node| node.id.as_str())
        .collect();
    if sinks.len() > 1 {
        // The factory joins every multi-sink graph behind one transparent
        // 0 dB output node (converters first for off-clock sinks), so the
        // built host is single-sink regardless of config shape.
        return PathTopology::JoinedMultiSink;
    }
    if sources.len() != 1 || sinks.len() != 1 {
        return PathTopology::Unsupported;
    }
    // Every node must be reachable from the source (forward BFS).
    let mut reached: Vec<&str> = vec![sources[0]];
    let mut cursor = 0;
    while cursor < reached.len() {
        let current = reached[cursor];
        cursor += 1;
        for edge in edges.iter().filter(|edge| edge.from == current) {
            if !reached.contains(&edge.to.as_str()) {
                reached.push(edge.to.as_str());
            }
        }
    }
    if reached.len() != nodes.len() {
        return PathTopology::Unsupported;
    }
    // Every node must reach the sink (reverse BFS) and the graph must be
    // acyclic (Kahn's algorithm over in-degrees).
    let mut reaches_sink: Vec<&str> = vec![sinks[0]];
    let mut reverse_cursor = 0;
    while reverse_cursor < reaches_sink.len() {
        let current = reaches_sink[reverse_cursor];
        reverse_cursor += 1;
        for edge in edges.iter().filter(|edge| edge.to == current) {
            if !reaches_sink.contains(&edge.from.as_str()) {
                reaches_sink.push(edge.from.as_str());
            }
        }
    }
    if reaches_sink.len() != nodes.len() {
        return PathTopology::Unsupported;
    }
    let mut in_degree: Vec<(&str, usize)> = nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                edges.iter().filter(|edge| edge.to == node.id).count(),
            )
        })
        .collect();
    while let Some(position) = in_degree.iter().position(|(_, degree)| *degree == 0) {
        let (id, _) = in_degree.remove(position);
        for edge in edges.iter().filter(|edge| edge.from == id) {
            if let Some((_, degree)) = in_degree
                .iter_mut()
                .find(|entry| entry.0 == edge.to.as_str())
            {
                *degree = degree.saturating_sub(1);
            }
        }
    }
    if !in_degree.is_empty() {
        return PathTopology::Unsupported;
    }
    let linear_degrees = nodes.iter().all(|node| {
        edges.iter().filter(|edge| edge.to == node.id).count() <= 1
            && edges.iter().filter(|edge| edge.from == node.id).count() <= 1
    });
    if linear_degrees && edges.len() == nodes.len().saturating_sub(1) {
        PathTopology::Linear
    } else {
        PathTopology::Dag
    }
}

/// A/B Comparison Plugin
///
/// Allows fair comparison between two audio processing chains with automatic
/// loudness matching. Each path (A or B) can be a single plugin, a rack
/// (linear chain), or a full graph.
pub struct ABComparePlugin {
    // Configuration
    pub(super) num_channels: usize,
    pub(super) sample_rate: f64,

    /// External plugin factory -- when set, supports all plugin types.
    /// Falls back to the built-in limited factory when None.
    pub(super) plugin_factory: Option<sotf_host::PluginFactoryFn>,

    // Processing paths - use DawHost for flexibility
    pub(super) host_a: DawHost,
    pub(super) host_b: DawHost,

    // Path configurations (stored for runtime changes)
    pub(super) path_a_config: PathConfig,
    pub(super) path_b_config: PathConfig,

    // Auto-gain for matching B to A's loudness
    // Uses A's output as "input reference" and B's output as "output to compensate"
    // Also provides loudness and peak data for both paths
    pub(super) auto_gain: AutoGain,

    // State
    pub(super) mix_mode: MixMode,
    pub(super) mix: f32,
    /// Crossfades path selection and latency-aligned bypass without expanding
    /// the already-large outer plugin state.
    pub(super) transition_smoothers: TransitionSmoothers,
    pub(super) selected_path: i32,
    pub(super) bypass: bool,
    pub(super) mix_transition_ms: f32,

    // Phase inversion
    pub(super) phase_invert: [bool; 2],

    // Difference mode (A - B)
    pub(super) difference_mode: bool,

    // Latency compensation delay lines
    pub(super) delay_a: DelayLine,
    pub(super) delay_b: DelayLine,
    /// Dry delay keeps bypass aligned with the latency reported to the host.
    pub(super) delay_dry: DelayLine,

    // Internal buffers
    pub(super) buffers: [Vec<f32>; 2],
    /// One bounded child-drain result per path; each buffer holds one declared
    /// `DawHost::drain_output_frames_max()` result.
    drain_buffers: [Vec<f32>; 2],
    drain_children: [ChildDrainProgress; 2],
    drain_delay_remaining: [usize; 3],
    drain_lifecycle: DrainLifecycle,
    band_mask_used_since_reset: bool,
    /// Retained same-clock child output across `process` calls for children
    /// whose per-call production varies (bursts). Valid samples are
    /// `[process_queue_start[i]..process_queues[i].len())`; preallocated at
    /// construction/initialize, never grown on the audio thread.
    process_queues: [Vec<f32>; 2],
    process_queue_start: [usize; 2],
    /// Retained delayed dry output, paired by stream position with the child
    /// queues. Same valid-range convention as `process_queues`.
    dry_queue: Vec<f32>,
    dry_queue_start: usize,
    /// Drain topology class per path, classified at construction (path
    /// configs are structural and cannot change at runtime).
    path_topology: [PathTopology; 2],
    /// Actual frames emitted by the last successful `process` call.
    last_process_frames: usize,

    // Band mask (bandpass filter for isolating frequency range in comparison)
    pub(super) band_mask_low_hz: f32,
    pub(super) band_mask_high_hz: f32,
    /// Per-channel highpass filters (one per channel) for band mask low cutoff
    pub(super) band_mask_hp: Vec<Biquad>,
    /// Per-channel lowpass filters (one per channel) for band mask high cutoff
    pub(super) band_mask_lp: Vec<Biquad>,
    /// Observable-state history per band-mask biquad per channel, recorded
    /// while the mask is active. Preallocated at construction; drives the
    /// rigorous residual flush length, never grown on the audio thread.
    mask_hist_hp: Vec<MaskStageHistory>,
    mask_hist_lp: Vec<MaskStageHistory>,
    /// Maximum absolute wet sample fed into the mask highpass stage this
    /// stream. Sets the relative residual threshold; zero means zero state.
    mask_wet_peak: f64,
    /// Remaining mask residual flush frames once child/ring/dry content is
    /// exhausted. `None` before the flush length is derived from live state.
    mask_flush_remaining: Option<usize>,

    // Cached peak values
    pub(super) last_peaks: [f64; 2],
    /// Cached unity-preserving gain for the empty-path fast path.
    pub(super) empty_path_fast_gain: f32,

    pub(super) cache: RealTimeCache<ABCompareData>,
    /// Frames elapsed since the last loudness target and diagnostic update.
    pub(super) cache_update_counter: usize,
    pub(super) cached_parameters: Vec<Parameter>,
}

impl ABComparePlugin {
    const MAX_REALTIME_FRAMES: usize = 48_000;

    /// Per-queue retention bound in frames for same-clock variable production.
    ///
    /// Proof sketch: every admitted path produces the outer clock with total
    /// output converging to total input (count-preserving resampling and
    /// chunking; drops fail the capacity checks loudly). Per call at most
    /// `host.output_frames_for_input(num_frames)` appends, and at most
    /// `num_frames` emits, so retention grows only by sibling divergence,
    /// bounded by one maximum converter/chunk burst (256 input frames x
    /// ratio plus rubato jitter: single-digit thousands of frames). Two
    /// maximum blocks (96k frames) therefore cover one worst-case retained
    /// block plus one worst-case burst with an order of magnitude of margin.
    /// Exceeding this bound is a loud process error, never a silent drop;
    /// raising it only grows preallocated memory.
    const PROCESS_QUEUE_FRAMES: usize = 2 * Self::MAX_REALTIME_FRAMES;

    /// Per-path child-drain staging capacity in frames, prepared once on the
    /// control thread at initialize and never grown on the audio thread.
    ///
    /// Must cover one maximum host drain call at ANY stream position: node
    /// drain bounds are stream-state-dependent (a fresh resampler reports 0
    /// with nothing to flush, then its full per-chunk bound mid-stream),
    /// so sizing staging from the fresh bound underprepares real converter
    /// paths (R2 gate failure). One maximum realtime block matches the
    /// plugin's own per-call drain ceiling (`drain_capacity_frames` min) and
    /// process staging (`buffers`); in-tree per-call drain production is
    /// single-digit thousands of frames, leaving an order of magnitude of
    /// margin. Hosts whose live bound exceeds this staging are refused
    /// loudly at `begin_drain` before any child advances, and any mid-drain
    /// bound growth fails closed through the host's own capacity error plus
    /// the post-call actual guard below — never silent truncation.
    /// Raising this only grows preallocated memory (~384 KB per stereo path).
    const DRAIN_STAGING_FRAMES: usize = Self::MAX_REALTIME_FRAMES;

    /// Compact a staging queue and verify room for `need_samples` more.
    ///
    /// Compaction moves retained samples to the front with `copy_within`
    /// (no allocation); the queue never grows on the audio thread.
    fn ensure_queue_spare(
        queue: &mut Vec<f32>,
        start: &mut usize,
        need_samples: usize,
    ) -> Result<(), String> {
        if *start > 0 {
            let len = queue.len();
            debug_assert!(*start <= len);
            queue.copy_within(*start..len, 0);
            queue.truncate(len - *start);
            *start = 0;
        }
        let end = queue
            .len()
            .checked_add(need_samples)
            .ok_or_else(|| "A/B Compare process staging sample count overflow".to_string())?;
        if end > queue.capacity() {
            return Err(format!(
                "A/B Compare process staging overflow: need {end} samples, capacity {}",
                queue.capacity()
            ));
        }
        Ok(())
    }

    /// Valid queued frames for one child path.
    fn process_queue_frames(&self, child_index: usize) -> usize {
        let valid = self.process_queues[child_index]
            .len()
            .saturating_sub(self.process_queue_start[child_index]);
        debug_assert!(valid.is_multiple_of(self.num_channels));
        valid / self.num_channels
    }

    /// Valid queued frames of delayed dry output.
    fn dry_queue_frames(&self) -> usize {
        let valid = self.dry_queue.len().saturating_sub(self.dry_queue_start);
        debug_assert!(valid.is_multiple_of(self.num_channels));
        valid / self.num_channels
    }

    /// Proven-zero child future: an exact-zero live tail, or a
    /// live-Unknown tail with zero support (support bounds true
    /// emission, so zero support means the child emits nothing more).
    fn child_future_is_zero(host: &DawHost) -> bool {
        match host.tail_length() {
            TailLength::Finite(0) => true,
            TailLength::Unknown => host.tail_support() == Some(0),
            _ => false,
        }
    }

    /// Advance a queue head past `samples`, resetting an emptied queue so the
    /// next append starts at offset zero without compaction.
    fn consume_queue(queue: &mut Vec<f32>, start: &mut usize, samples: usize) {
        *start += samples;
        debug_assert!(*start <= queue.len());
        if *start == queue.len() {
            queue.clear();
            *start = 0;
        }
    }

    fn validate_params(num_channels: usize, params: &ABComparePluginParams) -> Result<(), String> {
        if num_channels == 0 {
            return Err("A/B Compare requires at least one channel".into());
        }
        fn finite_range(name: &str, value: f32, min: f32, max: f32) -> Result<(), String> {
            if value.is_finite() && (min..=max).contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "{name} must be finite and in {min}..={max}, got {value}"
                ))
            }
        }
        finite_range("mix", params.mix, -1.0, 1.0)?;
        if !(0..=1).contains(&params.selected_path) {
            return Err(format!(
                "selected_path must be 0 or 1, got {}",
                params.selected_path
            ));
        }
        finite_range("gain_smoothing_ms", params.gain_smoothing_ms, 1.0, 500.0)?;
        finite_range("max_auto_gain_db", params.max_auto_gain_db, 0.0, 24.0)?;
        finite_range("mix_transition_ms", params.mix_transition_ms, 1.0, 500.0)?;
        finite_range("band_mask_low_hz", params.band_mask_low_hz, 20.0, 20_000.0)?;
        finite_range(
            "band_mask_high_hz",
            params.band_mask_high_hz,
            20.0,
            20_000.0,
        )?;
        if params.band_mask_low_hz >= params.band_mask_high_hz {
            return Err("band mask low cutoff must be below high cutoff".into());
        }
        Ok(())
    }

    fn validate_for_sample_rate(
        params: &ABComparePluginParams,
        sample_rate: f64,
    ) -> Result<(), String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("A/B Compare sample rate must be finite and positive".into());
        }
        let nyquist = sample_rate as f32 * 0.5;
        if params.band_mask_low_hz > Self::BAND_MASK_MIN_HZ + Self::BAND_MASK_EDGE_EPSILON
            && params.band_mask_low_hz >= nyquist
        {
            return Err(format!(
                "band_mask_low_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        if params.band_mask_high_hz < Self::BAND_MASK_MAX_HZ - Self::BAND_MASK_EDGE_EPSILON
            && params.band_mask_high_hz >= nyquist
        {
            return Err(format!(
                "band_mask_high_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        Ok(())
    }

    /// Create a new A/B Compare plugin with default settings
    pub fn new(num_channels: usize) -> Result<Self, String> {
        Self::from_params(num_channels, ABComparePluginParams::default())
    }

    /// Set the external plugin factory, enabling all plugin types in sub-racks.
    /// Call this after construction but before initialize() or processing.
    pub fn set_plugin_factory(&mut self, factory: sotf_host::PluginFactoryFn) {
        self.plugin_factory = Some(factory);
    }

    /// Create from parameters
    pub fn from_params(num_channels: usize, params: ABComparePluginParams) -> Result<Self, String> {
        Self::from_params_internal(num_channels, 48_000, params, None)
    }

    /// Construct initial paths with the authoritative factory already installed.
    pub fn from_params_with_factory<S: Into<f64>>(
        num_channels: usize,
        sample_rate: S,
        params: ABComparePluginParams,
        factory: sotf_host::PluginFactoryFn,
    ) -> Result<Self, String> {
        Self::from_params_internal(num_channels, sample_rate.into(), params, Some(factory))
    }

    fn from_params_internal(
        num_channels: usize,
        sample_rate: f64,
        params: ABComparePluginParams,
        factory: Option<sotf_host::PluginFactoryFn>,
    ) -> Result<Self, String> {
        Self::validate_params(num_channels, &params)?;
        Self::validate_for_sample_rate(&params, sample_rate)?;

        let host_a = if factory.is_some() {
            build_path_from_config_with_factory(&params.path_a, num_channels, sample_rate, factory)?
        } else {
            build_path_from_config(&params.path_a, num_channels, sample_rate)?
        };
        let host_b = if factory.is_some() {
            build_path_from_config_with_factory(&params.path_b, num_channels, sample_rate, factory)?
        } else {
            build_path_from_config(&params.path_b, num_channels, sample_rate)?
        };

        // Create AutoGain for matching B's loudness to A's loudness
        // A's output is the "input reference", B's output is "what to compensate"
        let auto_gain_params = AutoGainParams {
            enabled: params.auto_gain_enabled,
            loudness_type: params.loudness_type,
            max_gain_db: params.max_auto_gain_db,
            smoothing_ms: params.gain_smoothing_ms,
        };
        let auto_gain = AutoGain::new(num_channels, sample_rate, auto_gain_params)?;

        let mix_smoother = Smoother::new(params.mix, params.mix_transition_ms, sample_rate);
        let bypass_value = if params.bypass { 1.0 } else { 0.0 };
        let bypass_smoother = Smoother::new(bypass_value, params.mix_transition_ms, sample_rate);

        let band_mask_low_hz = params.band_mask_low_hz.clamp(20.0, 20000.0);
        let band_mask_high_hz = params.band_mask_high_hz.clamp(20.0, 20000.0);
        let q = 1.0 / std::f64::consts::SQRT_2;
        let band_mask_hp: Vec<Biquad> = (0..num_channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Highpass,
                    band_mask_low_hz as f64,
                    sample_rate as f64,
                    q,
                    0.0,
                )
            })
            .collect();
        let band_mask_lp: Vec<Biquad> = (0..num_channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Lowpass,
                    band_mask_high_hz as f64,
                    sample_rate as f64,
                    q,
                    0.0,
                )
            })
            .collect();

        let queue_samples = Self::PROCESS_QUEUE_FRAMES
            .checked_mul(num_channels)
            .ok_or_else(|| "A/B Compare process staging size overflow".to_string())?;
        let mut process_queues = [Vec::new(), Vec::new()];
        for queue in process_queues.iter_mut() {
            queue.try_reserve_exact(queue_samples).map_err(|error| {
                format!("A/B Compare process staging allocation failed: {error}")
            })?;
        }
        let mut dry_queue = Vec::new();
        dry_queue
            .try_reserve_exact(queue_samples)
            .map_err(|error| format!("A/B Compare dry staging allocation failed: {error}"))?;
        let path_topology = [
            classify_path_topology(&params.path_a),
            classify_path_topology(&params.path_b),
        ];

        let mut p = Self {
            num_channels,
            sample_rate,
            plugin_factory: factory,
            host_a,
            host_b,
            path_a_config: params.path_a,
            path_b_config: params.path_b,
            auto_gain,
            mix_mode: params.mix_mode,
            mix: params.mix,
            transition_smoothers: TransitionSmoothers {
                mix: mix_smoother,
                bypass: bypass_smoother,
            },
            selected_path: params.selected_path,
            bypass: params.bypass,
            mix_transition_ms: params.mix_transition_ms,
            phase_invert: [params.phase_invert_a, params.phase_invert_b],
            difference_mode: params.difference_mode,
            band_mask_low_hz,
            band_mask_high_hz,
            band_mask_hp,
            band_mask_lp,
            mask_hist_hp: vec![MaskStageHistory::default(); num_channels],
            mask_hist_lp: vec![MaskStageHistory::default(); num_channels],
            mask_wet_peak: 0.0,
            mask_flush_remaining: None,
            delay_a: DelayLine::new(),
            delay_b: DelayLine::new(),
            delay_dry: DelayLine::new(),
            buffers: [
                vec![0.0; Self::MAX_REALTIME_FRAMES * num_channels],
                vec![0.0; Self::MAX_REALTIME_FRAMES * num_channels],
            ],
            drain_buffers: [Vec::new(), Vec::new()],
            drain_children: [ChildDrainProgress::default(); 2],
            drain_delay_remaining: [0; 3],
            drain_lifecycle: DrainLifecycle::Accepting,
            band_mask_used_since_reset: false,
            process_queues,
            process_queue_start: [0; 2],
            dry_queue,
            dry_queue_start: 0,
            path_topology,
            last_process_frames: 0,
            last_peaks: [0.0; 2],
            empty_path_fast_gain: 0.0,
            cache: RealTimeCache::new(ABCompareData::default()),
            cache_update_counter: 0,
            cached_parameters: Vec::new(),
        };
        p.recompute_empty_path_fast_gain();
        p.rebuild_cached_parameters();
        Ok(p)
    }

    /// Identical paths must remain at unity for every crossfade position.
    pub(super) fn recompute_empty_path_fast_gain(&mut self) {
        self.empty_path_fast_gain = 1.0;
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = vec![
            Parameter::new_float("mix", "A/B Mix", self.mix, -1.0, 1.0)
                .with_description("Mix between A and B: -1.0 = A, 0.0 = 50/50, +1.0 = B")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_int(
                "mix_mode",
                "Mix Mode",
                match self.mix_mode {
                    MixMode::Potentiometer => 0,
                    MixMode::Binary => 1,
                },
                0,
                1,
            )
            .with_description("0 = Potentiometer (continuous), 1 = Binary (A/B switch)")
            .with_group("Mix Control")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_int("selected_path", "Selected Path", self.selected_path, 0, 1)
                .with_description("0 = A, 1 = B (only used in binary mode)")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_bool("bypass", "Bypass", self.bypass)
                .with_description("Bypass A/B processing, output original input")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_bool(
                "auto_gain_enabled",
                "Auto Gain",
                self.auto_gain.is_enabled(),
            )
            .with_description("Automatically match loudness between A and B")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_int(
                "loudness_type",
                "Loudness Type",
                match self.auto_gain.loudness_type() {
                    AutoGainLoudnessType::Momentary => 0,
                    AutoGainLoudnessType::ShortTerm => 1,
                },
                0,
                1,
            )
            .with_description("0 = Momentary (400ms), 1 = Short-term (3s)")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "max_auto_gain_db",
                "Max Auto Gain",
                self.auto_gain.max_gain_db(),
                0.0,
                24.0,
            )
            .with_description("Maximum loudness correction in dB")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::FineTuning),
            Parameter::new_float(
                "gain_smoothing_ms",
                "Gain Smoothing",
                self.auto_gain.smoothing_ms(),
                1.0,
                500.0,
            )
            .with_description("Auto-gain smoothing time in milliseconds")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::FineTuning),
            Parameter::new_float(
                "mix_transition_ms",
                "Mix Transition",
                self.mix_transition_ms,
                1.0,
                500.0,
            )
            .with_description("A/B transition smoothing time in milliseconds")
            .with_group("Timing")
            .with_importance(ParameterImportance::FineTuning),
            Parameter::new_bool("phase_invert_a", "Phase Invert A", self.phase_invert[0])
                .with_description("Invert phase of path A output (multiply by -1.0)")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("phase_invert_b", "Phase Invert B", self.phase_invert[1])
                .with_description("Invert phase of path B output (multiply by -1.0)")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("difference_mode", "Difference Mode", self.difference_mode)
                .with_description("Output A - B instead of crossfade mix")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "band_mask_low_hz",
                "Band Mask Low",
                self.band_mask_low_hz,
                20.0,
                20000.0,
            )
            .with_description("Highpass cutoff for band-masking the comparison output (Hz)")
            .with_group("Band Mask")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "band_mask_high_hz",
                "Band Mask High",
                self.band_mask_high_hz,
                20.0,
                20000.0,
            )
            .with_description("Lowpass cutoff for band-masking the comparison output (Hz)")
            .with_group("Band Mask")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_string(
                "path_a_config",
                "Path A Config",
                serde_json::to_string(&self.path_a_config)
                    .unwrap_or_else(|_| r#"{"type":"None"}"#.to_string()),
            )
            .with_description("JSON configuration for path A")
            .with_group("Configuration")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_string(
                "path_b_config",
                "Path B Config",
                serde_json::to_string(&self.path_b_config)
                    .unwrap_or_else(|_| r#"{"type":"None"}"#.to_string()),
            )
            .with_description("JSON configuration for path B")
            .with_group("Configuration")
            .with_importance(ParameterImportance::Critical),
        ];
        sotf_host::param_bridge::apply_spec_update_modes(
            &mut self.cached_parameters,
            crate::params::PARAMS,
        );
    }

    #[cfg(test)]
    pub(super) fn rebuild_path_a(&mut self) -> Result<(), String> {
        self.host_a = build_path_from_config_with_factory(
            &self.path_a_config,
            self.num_channels,
            self.sample_rate,
            self.plugin_factory,
        )?;
        self.update_latency_compensation()
    }

    #[cfg(test)]
    pub(super) fn rebuild_path_b(&mut self) -> Result<(), String> {
        self.host_b = build_path_from_config_with_factory(
            &self.path_b_config,
            self.num_channels,
            self.sample_rate,
            self.plugin_factory,
        )?;
        self.update_latency_compensation()
    }

    /// Minimum audible frequency (Hz). Band mask low values at or below this
    /// are treated as "no highpass filtering".
    pub(super) const BAND_MASK_MIN_HZ: f32 = 20.0;

    /// Maximum audible frequency (Hz). Band mask high values at or above this
    /// are treated as "no lowpass filtering".
    pub(super) const BAND_MASK_MAX_HZ: f32 = 20000.0;

    /// Half-step epsilon (Hz) used when comparing band mask edges to the
    /// parameter limits. A value equal to the parameter minimum/maximum
    /// means "full range" — we accept anything within 0.5 Hz of those limits
    /// so that floating-point serialise/deserialise round-trips (e.g. JSON)
    /// cannot accidentally activate the filter chain.
    pub(super) const BAND_MASK_EDGE_EPSILON: f32 = 0.5;

    /// Returns true if the band mask range is narrower than the full audible
    /// spectrum, i.e. if the biquad filter pair should be applied.
    pub(super) fn band_mask_active(&self) -> bool {
        self.band_mask_low_hz > Self::BAND_MASK_MIN_HZ + Self::BAND_MASK_EDGE_EPSILON
            || self.band_mask_high_hz < Self::BAND_MASK_MAX_HZ - Self::BAND_MASK_EDGE_EPSILON
    }

    #[allow(dead_code)]
    pub(super) fn has_empty_paths(&self) -> bool {
        matches!(self.path_a_config, PathConfig::None)
            && matches!(self.path_b_config, PathConfig::None)
    }

    #[allow(dead_code)]
    pub(super) fn can_use_empty_path_fast_path(&self) -> bool {
        self.has_empty_paths()
            && self.mix_mode == MixMode::Potentiometer
            && !self.phase_invert[0]
            && !self.phase_invert[1]
            && !self.difference_mode
            && !self.band_mask_active()
            && self.auto_gain.is_unity_gain_stable()
            && (self.transition_smoothers.mix.current() - self.transition_smoothers.mix.target())
                .abs()
                < 1e-5
    }

    #[allow(dead_code)]
    pub(super) fn process_empty_path_fast(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_frames: usize,
    ) -> Result<(), String> {
        let mut frame = 0;
        while frame < num_frames {
            let count = self.frames_until_measurement().min(num_frames - frame);
            let samples = &input[frame * self.num_channels..(frame + count) * self.num_channels];
            if self.auto_gain.is_enabled() {
                self.auto_gain.ingest_input(samples)?;
                self.auto_gain.ingest_output(samples)?;
            }
            self.auto_gain.next_n(count);
            self.finish_measurement_segment(count);
            frame += count;
        }

        let gain = self.empty_path_fast_gain;

        if (gain - 1.0).abs() < 1e-6 {
            output.copy_from_slice(input);
        } else {
            for (out, &sample) in output.iter_mut().zip(input.iter()) {
                *out = sample * gain;
            }
        }

        Ok(())
    }

    fn frames_until_measurement(&self) -> usize {
        // Refresh at 20 Hz on the sample clock, independently of callback size.
        let interval = (self.sample_rate / 20.0).floor().max(1.0) as usize;
        interval - self.cache_update_counter
    }

    fn finish_measurement_segment(&mut self, frames: usize) {
        debug_assert!(frames <= self.frames_until_measurement());
        self.cache_update_counter += frames;
        if self.frames_until_measurement() != 0 {
            return;
        }
        self.cache_update_counter = 0;
        // The segment has already been rendered with its previous gain target.
        // These statistics may change only the samples following this boundary.
        if self.auto_gain.is_enabled() {
            self.auto_gain.refresh_input_measurement();
            self.auto_gain.refresh_output_measurement();
            self.last_peaks[0] = self.auto_gain.last_input_peak();
            self.last_peaks[1] = self.auto_gain.last_output_peak();
        }
        let data = ABCompareData {
            loudness_a_lufs: self.auto_gain.last_input_lufs(),
            loudness_b_lufs: self.auto_gain.last_output_lufs(),
            auto_gain_db: self.auto_gain.current_gain_db(),
            peak_a: self.last_peaks[0],
            peak_b: self.last_peaks[1],
            current_mix: self.transition_smoothers.mix.current(),
            bypass_active: self.bypass,
        };
        self.cache.update(|d| *d = data);
    }

    /// Rebuild the bandpass filter pair for the current band mask settings.
    pub(super) fn rebuild_band_mask_filters(&mut self) {
        let q = 1.0 / std::f64::consts::SQRT_2;
        let sr = self.sample_rate;
        if self.band_mask_hp.len() == self.num_channels {
            // Update coefficients in place — preserves filter delay state (click-free)
            for f in &mut self.band_mask_hp {
                f.update_params(
                    BiquadFilterType::Highpass,
                    self.band_mask_low_hz as f64,
                    sr,
                    q,
                    0.0,
                );
            }
            for f in &mut self.band_mask_lp {
                f.update_params(
                    BiquadFilterType::Lowpass,
                    self.band_mask_high_hz as f64,
                    sr,
                    q,
                    0.0,
                );
            }
        } else {
            // First time: create filters from scratch
            self.band_mask_hp = (0..self.num_channels)
                .map(|_| {
                    Biquad::new(
                        BiquadFilterType::Highpass,
                        self.band_mask_low_hz as f64,
                        sr,
                        q,
                        0.0,
                    )
                })
                .collect();
            self.band_mask_lp = (0..self.num_channels)
                .map(|_| {
                    Biquad::new(
                        BiquadFilterType::Lowpass,
                        self.band_mask_high_hz as f64,
                        sr,
                        q,
                        0.0,
                    )
                })
                .collect();
        }
    }

    fn reset_band_mask_filter_state(&mut self) {
        let q = 1.0 / std::f64::consts::SQRT_2;
        let sample_rate = self.sample_rate;
        for filter in &mut self.band_mask_hp {
            *filter = Biquad::new(
                BiquadFilterType::Highpass,
                self.band_mask_low_hz as f64,
                sample_rate,
                q,
                0.0,
            );
        }
        for filter in &mut self.band_mask_lp {
            *filter = Biquad::new(
                BiquadFilterType::Lowpass,
                self.band_mask_high_hz as f64,
                sample_rate,
                q,
                0.0,
            );
        }
    }

    /// Relative residual ratio for the band-mask drain flush.
    ///
    /// The proven exact-arithmetic unemitted remainder stays below program
    /// peak x 2^-24: 144 dB below peak is beneath the 24-bit
    /// converter/measurement floor, the widest standard downstream format.
    /// Relative (not absolute) so quiet programs do not force unbounded
    /// flushes for inaudible residue. The emitted flush frames are exact
    /// filter outputs; this ratio bounds only what stays unemitted.
    ///
    /// Float note: the f64 DF-I derivation mirrors the filter's own update
    /// order, so the first two derived frames are bitwise exact and the modal
    /// tail is a bound with realistic rounding margins (worst real corner
    /// ~33x under threshold, typical masks orders more; see the lane's
    /// fix-r2-result for the margin analysis). A rigorous adversarial
    /// worst-case float proof is impossible for high-conditioning corners
    /// and is explicitly out of scope; unprovable exact-arithmetic cases
    /// (unstable coefficients, unit poles, unbounded lengths) refuse loudly
    /// instead of truncating.
    const MASK_RESIDUAL_RATIO: f64 = 1.0 / 16_777_216.0;

    /// Rounding inflation for the modal envelope (Wilkinson-style a priori
    /// bound: the ~100-flop derivation accumulates far less than 1024eps, so
    /// the inflated envelope strictly covers exact arithmetic).
    const MASK_ENVELOPE_SLACK: f64 = 1024.0 * f64::EPSILON;

    /// Closed-form modal envelope of one biquad: returns `(rho, kappa, h1)`
    /// with `||A^n|| <= kappa * rho^n` for the companion state matrix and
    /// `|h[k]| <= h1 * rho^k` for the impulse response.
    ///
    /// Poles come from `z^2 + a1*z + a2 = 0` (a0 is normalized to 1); the
    /// conditioning is the exact 2x2 eigenvector conditioning, with an exact
    /// Jordan form for repeated poles (midpoint radius, the canonical choice
    /// between pole radius and unity). Errors only when the Jury stability
    /// test fails or the radius sits too close to unity for f64 to bound.
    fn biquad_modal_envelope(coeffs: &BiquadCoefficients<f64>) -> Result<(f64, f64, f64), String> {
        let (a1, a2) = (coeffs.a1, coeffs.a2);
        if !(a2.abs() < 1.0 && 1.0 + a1 + a2 > 0.0 && 1.0 - a1 + a2 > 0.0) {
            return Err("A/B Compare band-mask coefficients are unstable; drain refused".into());
        }
        let disc = a1 * a1 - 4.0 * a2;
        let (mut rho, mut kappa) = if disc < 0.0 {
            // Complex conjugate pair: |p|^2 = a2, |p1 - p2| = sqrt(-disc).
            let rho = a2.sqrt();
            let separation = (-disc).sqrt();
            (rho, 2.0 * (1.0 + rho) / separation)
        } else if disc > 0.0 {
            let root = disc.sqrt();
            let lambda1 = (-a1 + root) / 2.0;
            let lambda2 = (-a1 - root) / 2.0;
            let rho = lambda1.abs().max(lambda2.abs());
            let v_norm = (lambda1.abs() + lambda2.abs()).max(2.0);
            let vi_norm = (1.0 + lambda1.abs()).max(1.0 + lambda2.abs()) / root;
            (rho, v_norm * vi_norm)
        } else {
            // Repeated pole: exact Jordan bound folded onto the midpoint
            // radius. max_n n*r^(n-1) = -1/(e*r*ln r) for r in (0, 1).
            let lambda = -a1 / 2.0;
            let rho = lambda.abs();
            let mid = (1.0 + rho) / 2.0;
            let ratio = rho / mid;
            let peak = -1.0 / (std::f64::consts::E * ratio * ratio.ln());
            let jordan = ((-a1 - lambda).abs() + a2.abs()).max(1.0 + lambda.abs());
            // kappa = 1 + peak*jordan/mid: the Jordan triangle needs the +1
            // so mid^n dominates BOTH rho^n and n*rho^(n-1)*||N|| (D2).
            (mid, 1.0 + peak * jordan / mid)
        };
        rho = rho * (1.0 + Self::MASK_ENVELOPE_SLACK) + Self::MASK_ENVELOPE_SLACK;
        kappa *= 1.0 + Self::MASK_ENVELOPE_SLACK;
        // NaN stays rejected: comparisons are false for NaN, so the
        // `is_finite` disjuncts carry the NaN/non-finite rejection explicitly.
        if rho >= 1.0 || !rho.is_finite() || !kappa.is_finite() {
            return Err("A/B Compare band-mask pole radius is too close to unity to bound".into());
        }
        // Observable-canonical input vector: h[k] = C*A^(k-1)*B gives the
        // modal impulse constant below (h[0] = b0 covered by the max).
        let b1 = coeffs.b1 - a1 * coeffs.b0;
        let b2 = coeffs.b2 - a2 * coeffs.b0;
        let h1 = coeffs.b0.abs().max(kappa * (b1.abs() + b2.abs()) / rho);
        Ok((rho, kappa, h1))
    }

    /// Zero-input frames for one biquad to decay below `threshold`, given its
    /// recorded observable history. The first two frames use the exact
    /// feedthrough from recorded input history; the remainder uses the modal
    /// envelope. Returned frames are EMITTED exactly; the proof covers what
    /// stays beyond them.
    fn biquad_free_frames(
        coeffs: &BiquadCoefficients<f64>,
        envelope: (f64, f64, f64),
        history: MaskStageHistory,
        threshold: f64,
    ) -> Result<u64, String> {
        if history.is_silent() {
            return Ok(0);
        }
        let (rho, kappa, _) = envelope;
        let y0 = coeffs.b1 * history.in1 + coeffs.b2 * history.in2
            - coeffs.a1 * history.out1
            - coeffs.a2 * history.out2;
        let y1 = coeffs.b2 * history.in1 - coeffs.a1 * y0 - coeffs.a2 * history.out1;
        let state = y0.abs().max(y1.abs());
        // |y[n]| <= kappa * ||v|| / rho * rho^n for n >= 2, v = [y1, y0].
        let scale = kappa * state / rho * (1.0 + Self::MASK_ENVELOPE_SLACK);
        if scale <= 0.0 {
            // Exact zero state: the homogeneous future is exactly zero.
            return Ok(2);
        }
        if threshold <= 0.0 || !threshold.is_finite() {
            return Err("A/B Compare band-mask residual threshold is not positive".into());
        }
        let frames = if scale <= threshold {
            2
        } else {
            // n > ln(t/M) / ln(rho); ln(rho) < 0 flips the inequality.
            let exact = (threshold / scale).ln() / rho.ln();
            if !exact.is_finite() || exact > u32::MAX as f64 {
                return Err("A/B Compare band-mask flush exceeds its bound".into());
            }
            2 + (exact.floor() as u64 + 1)
        };
        // Fail closed past the bound: clamping would silently truncate an
        // unproven remainder (D3). Unreachable for Butterworth designs.
        if frames > u32::MAX as u64 {
            return Err("A/B Compare band-mask flush exceeds its bound".into());
        }
        Ok(frames)
    }

    /// Checked horizon + tail sum with an explicit bound (D3). Clamping a
    /// past-bound sum would silently truncate an unproven remainder, so sums
    /// past u32::MAX refuse loudly instead.
    fn checked_flush_total(horizon: u64, tail: u64) -> Result<u64, String> {
        let total = horizon.saturating_add(tail);
        if total > u32::MAX as u64 {
            return Err("A/B Compare band-mask flush exceeds its bound".into());
        }
        Ok(total)
    }

    /// Residual flush frames for one channel of the HP->LP mask cascade.
    ///
    /// The highpass tail is bounded to a derived intermediate target that
    /// keeps the lowpass forced response below half the threshold (via the
    /// lowpass l1 bound); the cascade is then simulated exactly to that
    /// horizon in scalar state (no allocation), and the lowpass free response
    /// from the simulated state covers the other half. Triangle inequality
    /// over the two halves proves the unemitted remainder below threshold.
    fn mask_channel_flush_frames(
        hp_coeffs: &BiquadCoefficients<f64>,
        lp_coeffs: &BiquadCoefficients<f64>,
        hp_history: MaskStageHistory,
        lp_history: MaskStageHistory,
        threshold: f64,
    ) -> Result<u64, String> {
        if hp_history.is_silent() && lp_history.is_silent() {
            return Ok(0);
        }
        let hp_envelope = Self::biquad_modal_envelope(hp_coeffs)?;
        let lp_envelope = Self::biquad_modal_envelope(lp_coeffs)?;
        let (lp_rho, _, lp_h1) = lp_envelope;
        // Lowpass l1 bound: |b0| + sum_{k>=1} h1*rho^k.
        let lp_l1 = lp_coeffs.b0.abs() + lp_h1 * lp_rho / (1.0 - lp_rho);
        if !lp_l1.is_finite() {
            return Err("A/B Compare band-mask lowpass bound is not finite".into());
        }
        if lp_l1 == 0.0 {
            // Zero-numerator lowpass: the highpass-forced response is exactly
            // zero, but the free response from nonzero recorded state is not,
            // so derive it at the full threshold instead of returning 0 (D1).
            return Self::biquad_free_frames(lp_coeffs, lp_envelope, lp_history, threshold);
        }
        let hp_target = (threshold / 2.0) / lp_l1;
        if !hp_target.is_finite() || hp_target <= 0.0 {
            return Err("A/B Compare band-mask cascade target is not positive".into());
        }
        let horizon = Self::biquad_free_frames(hp_coeffs, hp_envelope, hp_history, hp_target)?;
        // Exact scalar simulation of the cascade through the horizon; only
        // the running last-two states are retained (no allocation).
        let (mut hp_y1, mut hp_y0) = (hp_history.out1, hp_history.out2);
        let (mut hp_u1, mut hp_u0) = (hp_history.in1, hp_history.in2);
        let (mut lp_y1, mut lp_y0) = (lp_history.out1, lp_history.out2);
        let (mut lp_u1, mut lp_u0) = (lp_history.in1, lp_history.in2);
        for _ in 0..horizon {
            let hp_y = hp_coeffs.b1 * hp_u1 + hp_coeffs.b2 * hp_u0
                - hp_coeffs.a1 * hp_y1
                - hp_coeffs.a2 * hp_y0;
            hp_u0 = hp_u1;
            hp_u1 = 0.0;
            hp_y0 = hp_y1;
            hp_y1 = hp_y;
            let lp_y = lp_coeffs.b0 * hp_y + lp_coeffs.b1 * lp_u1 + lp_coeffs.b2 * lp_u0
                - lp_coeffs.a1 * lp_y1
                - lp_coeffs.a2 * lp_y0;
            lp_u0 = lp_u1;
            lp_u1 = hp_y;
            lp_y0 = lp_y1;
            lp_y1 = lp_y;
        }
        let lp_state = MaskStageHistory {
            in1: lp_u1,
            in2: lp_u0,
            out1: lp_y1,
            out2: lp_y0,
        };
        // Lowpass free response from the simulated state for the other half.
        // Its recorded-input feedthrough is exact simulated audio, and the
        // highpass tail beyond the horizon stays under hp_target by proof.
        let tail = Self::biquad_free_frames(lp_coeffs, lp_envelope, lp_state, threshold / 2.0)?;
        Self::checked_flush_total(horizon, tail)
    }

    /// Proven mask residual flush length over all channels.
    ///
    /// Zero program peak means zero filter state (filters start zeroed and
    /// linear), hence zero flush. Otherwise the threshold is program peak x
    /// 2^-24 (see MASK_RESIDUAL_RATIO), clamped so remainders below f32's
    /// range complete exactly instead of underflowing the proof.
    fn mask_flush_frames_required(&self) -> Result<usize, String> {
        // Inactive mask (never used, or used then deactivated): the mixer
        // never steps inactive filters, so frozen histories are unobservable
        // and no flush is emitted — only real child/ring tails drain (D4).
        // Parameters are frozen during drain (`set_parameter` refuses unless
        // Accepting), so inactive-at-derivation is final.
        if !self.band_mask_active() {
            return Ok(0);
        }
        let peak = self.mask_wet_peak;
        if peak == 0.0 {
            return Ok(0);
        }
        if !peak.is_finite() {
            return Err("A/B Compare band-mask excitation is not finite; drain refused".into());
        }
        let threshold = (peak * Self::MASK_RESIDUAL_RATIO).max(1024.0 * f64::MIN_POSITIVE);
        let hp_coeffs = self.band_mask_hp[0].coefficients();
        let lp_coeffs = self.band_mask_lp[0].coefficients();
        let mut frames: u64 = 0;
        for channel in 0..self.num_channels {
            let channel_frames = Self::mask_channel_flush_frames(
                &hp_coeffs,
                &lp_coeffs,
                self.mask_hist_hp[channel],
                self.mask_hist_lp[channel],
                threshold,
            )?;
            frames = frames.max(channel_frames);
        }
        Ok(frames.min(usize::MAX as u64) as usize)
    }

    /// Coefficient-derived stability gate for mask drain (Jury conditions on
    /// every channel's biquads). Unstable masks cannot bound a residual and
    /// stay refused; Butterworth designs always pass.
    fn validate_mask_stability(&self) -> Result<(), String> {
        for channel in 0..self.num_channels {
            Self::biquad_modal_envelope(&self.band_mask_hp[channel].coefficients())?;
            Self::biquad_modal_envelope(&self.band_mask_lp[channel].coefficients())?;
        }
        Ok(())
    }

    /// Align both paths by delaying the shorter one.
    ///
    /// Returns an error if either host fails to build (which would make latency
    /// queries unreliable and lead to silent phase misalignment). On error,
    /// both delay lines are set to zero so the plugin stays audible while
    /// latency compensation is disabled.
    pub(super) fn update_latency_compensation(&mut self) -> Result<(), String> {
        // Build both hosts so that `total_latency_samples()` reflects the
        // current graph topology. Ignore errors separately so we can report
        // both failures in one message if necessary.
        let err_a = self.host_a.build().err();
        let err_b = self.host_b.build().err();
        if err_a.is_some() || err_b.is_some() {
            // Disable compensation: set both delays to zero so the plugin
            // remains audible rather than silently misaligning the paths.
            self.delay_a.set_delay(0, self.num_channels);
            self.delay_b.set_delay(0, self.num_channels);
            self.delay_dry.set_delay(0, self.num_channels);
            let msg = match (err_a, err_b) {
                (Some(a), Some(b)) => format!(
                    "Latency compensation disabled: host_a build error: {a}; host_b build error: {b}"
                ),
                (Some(a), None) => {
                    format!("Latency compensation disabled: host_a build error: {a}")
                }
                (None, Some(b)) => {
                    format!("Latency compensation disabled: host_b build error: {b}")
                }
                (None, None) => unreachable!(),
            };
            return Err(msg);
        }
        let lat_a = self.host_a.total_latency_samples();
        let lat_b = self.host_b.total_latency_samples();
        if lat_a > lat_b {
            self.delay_a.set_delay(0, self.num_channels);
            self.delay_b.set_delay(lat_a - lat_b, self.num_channels);
        } else {
            self.delay_a.set_delay(lat_b - lat_a, self.num_channels);
            self.delay_b.set_delay(0, self.num_channels);
        }
        self.delay_dry
            .set_delay(lat_a.max(lat_b), self.num_channels);
        Ok(())
    }

    fn mix_prepared_buffers(
        &mut self,
        output_with_dry: &mut [f32],
        num_frames: usize,
    ) -> PluginResult<()> {
        let expected_samples = num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare block sample count overflow".to_string())?;
        if expected_samples > self.buffers[0].len()
            || expected_samples > self.buffers[1].len()
            || output_with_dry.len() != expected_samples
        {
            return Err("A/B Compare mixer received inconsistent frame geometry".into());
        }

        let sign_a: f32 = if self.phase_invert[0] { -1.0 } else { 1.0 };
        let sign_b: f32 = if self.phase_invert[1] { -1.0 } else { 1.0 };
        let band_mask_active = self.band_mask_active();

        let target_mix = match self.mix_mode {
            MixMode::Potentiometer => self.mix,
            MixMode::Binary => {
                if self.selected_path == 0 {
                    -1.0
                } else {
                    1.0
                }
            }
        };
        if (self.transition_smoothers.mix.target() - target_mix).abs() > f32::EPSILON {
            self.transition_smoothers.mix.set_target(target_mix);
            self.recompute_empty_path_fast_gain();
        }

        let mut segment_start = 0;
        while segment_start < num_frames {
            let count = self
                .frames_until_measurement()
                .min(num_frames - segment_start);
            let segment_end = segment_start + count;
            if self.auto_gain.is_enabled() {
                let samples = segment_start * self.num_channels..segment_end * self.num_channels;
                self.auto_gain
                    .ingest_input(&self.buffers[0][samples.clone()])?;
                self.auto_gain.ingest_output(&self.buffers[1][samples])?;
            }
            for frame in segment_start..segment_end {
                let gain_linear = self.auto_gain.next_gain_linear();
                let current_mix = self.transition_smoothers.mix.advance();
                let bypass_mix = self.transition_smoothers.bypass.advance();

                for ch in 0..self.num_channels {
                    let idx = frame * self.num_channels + ch;
                    let dry_sample = output_with_dry[idx];
                    let sample_a = self.buffers[0][idx] * sign_a;
                    let sample_b = self.buffers[1][idx] * gain_linear * sign_b;

                    let mut wet_sample = if self.difference_mode {
                        sample_a - sample_b
                    } else {
                        let mix_01 = (current_mix + 1.0) / 2.0;
                        let gain_a = 1.0 - mix_01;
                        let gain_b = mix_01;
                        sample_a * gain_a + sample_b * gain_b
                    };

                    if band_mask_active {
                        let hp_in = wet_sample as f64;
                        let hp_out = self.band_mask_hp[ch].process(hp_in);
                        let lp_out = self.band_mask_lp[ch].process(hp_out);
                        wet_sample = lp_out as f32;
                        // Observable-state history for the rigorous drain
                        // residual proof (mask-active samples only).
                        self.mask_hist_hp[ch].observe(hp_in, hp_out);
                        self.mask_hist_lp[ch].observe(hp_out, lp_out);
                        let peak = hp_in.abs();
                        if peak > self.mask_wet_peak {
                            self.mask_wet_peak = peak;
                        }
                    }
                    output_with_dry[idx] =
                        wet_sample * (1.0 - bypass_mix) + dry_sample * bypass_mix;
                }
            }
            self.finish_measurement_segment(count);
            segment_start = segment_end;
        }

        Ok(())
    }

    fn pump_child_drain(&mut self, child_index: usize) -> PluginResult<()> {
        if child_index >= self.drain_children.len() {
            return Err("A/B Compare child drain index is out of range".into());
        }
        if self.drain_children[child_index].complete
            || self.drain_children[child_index].queued_remaining() > 0
        {
            return Ok(());
        }

        // Retained process-phase frames precede tail production in stream
        // order; transfer one staging chunk before pumping host drain.
        let retained = self.process_queue_frames(child_index);
        if retained > 0 {
            let staging_frames = self.drain_buffers[child_index].len() / self.num_channels;
            let transfer = retained.min(staging_frames);
            if transfer == 0 {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err("A/B Compare drain staging cannot hold retained process output".into());
            }
            let samples = transfer * self.num_channels;
            let start = self.process_queue_start[child_index];
            let (queue, staging) = (
                &self.process_queues[child_index],
                &mut self.drain_buffers[child_index],
            );
            staging[..samples].copy_from_slice(&queue[start..start + samples]);
            Self::consume_queue(
                &mut self.process_queues[child_index],
                &mut self.process_queue_start[child_index],
                samples,
            );
            self.drain_children[child_index] = ChildDrainProgress {
                queued_frames: transfer,
                consumed_frames: 0,
                complete: false,
            };
            return Ok(());
        }

        self.drain_buffers[child_index].fill(0.0);
        let result = if child_index == 0 {
            self.host_a.drain(&mut self.drain_buffers[child_index])
        } else {
            self.host_b.drain(&mut self.drain_buffers[child_index])
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(format!(
                    "A/B Compare child {child_index} drain failed: {error}"
                ));
            }
        };
        let Some(sample_count) = result.frames.checked_mul(self.num_channels) else {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err("A/B Compare child drain sample count overflow".into());
        };
        if sample_count > self.drain_buffers[child_index].len() {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err("A/B Compare child exceeded prepared drain capacity".into());
        }
        self.drain_children[child_index] = ChildDrainProgress {
            queued_frames: result.frames,
            consumed_frames: 0,
            complete: result.complete,
        };
        Ok(())
    }

    /// Accepted direct process path for identity-geometry hosts.
    ///
    /// Both children are guaranteed to produce exactly `num_frames` at the
    /// outer clock. This is the accepted same-rate path, preserved
    /// bit-identically; actual child counts are additionally verified and
    /// any contract breach fails closed instead of mixing unwritten audio.
    fn process_direct(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_frames: usize,
        expected_samples: usize,
    ) -> Result<usize, String> {
        if expected_samples > self.buffers[0].len() || expected_samples > self.buffers[1].len() {
            return Err(format!(
                "A/B Compare block exceeds prepared realtime capacity: {} frames (max {})",
                num_frames,
                Self::MAX_REALTIME_FRAMES
            ));
        }

        // Always advance the latency-aligned dry path and both nested paths,
        // even while bypass is fully engaged. This makes toggles continuous
        // and prevents stateful nested processors from freezing in bypass.
        output.copy_from_slice(input);
        self.delay_dry.process(output);

        // Process path A
        let produced_a = match self
            .host_a
            .process(input, &mut self.buffers[0][..expected_samples])
        {
            Ok(frames) => frames,
            Err(error) => {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
        };

        // Process path B
        let produced_b = match self
            .host_b
            .process(input, &mut self.buffers[1][..expected_samples])
        {
            Ok(frames) => frames,
            Err(error) => {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
        };

        if produced_a != num_frames || produced_b != num_frames {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(format!(
                "A/B Compare identity paths produced {produced_a}/{produced_b} frames for a {num_frames}-frame block"
            ));
        }

        // Apply latency compensation (delays the shorter path)
        self.delay_a
            .process(&mut self.buffers[0][..expected_samples]);
        self.delay_b
            .process(&mut self.buffers[1][..expected_samples]);

        if let Err(error) = self.mix_prepared_buffers(output, num_frames) {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }
        if self.band_mask_active() {
            self.band_mask_used_since_reset = true;
        }

        self.last_process_frames = num_frames;
        Ok(num_frames)
    }

    /// Staging-queue process path for same-clock variable production.
    ///
    /// Both nested paths produce the outer clock (checked by the caller) but
    /// at least one varies its per-call frame count. Child output is staged
    /// in preallocated FIFO queues and emitted in stream-position order;
    /// retained excess waits for the sibling path. Returns the actual
    /// emitted frames `K <= num_frames`: honest variable output like any
    /// resampler, so the caller must use the returned count — the output
    /// slice beyond `K` frames is untouched. Never drops, pads, or reorders
    /// produced audio. Returning `0` for a nonzero block means a child is
    /// still buffering its chunk; feed more input and retry.
    fn process_queued(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_frames: usize,
    ) -> Result<usize, String> {
        let channels = self.num_channels;
        let input_samples = num_frames
            .checked_mul(channels)
            .ok_or_else(|| "A/B Compare block sample count overflow".to_string())?;
        debug_assert_eq!(input.len(), input_samples);
        debug_assert_eq!(output.len(), input_samples);
        if input_samples > self.buffers[0].len() || input_samples > self.buffers[1].len() {
            return Err(format!(
                "A/B Compare block exceeds prepared realtime capacity: {} frames (max {})",
                num_frames,
                Self::MAX_REALTIME_FRAMES
            ));
        }

        // No observability preflight: both children advance through
        // `process_unpadded`, whose return IS actual production (even zero,
        // even short-on-exact-bound), so silent variable chains pair exactly
        // with no inference and no padding to mistake for audio. This closes
        // the R2 section 5 residual structurally; the host suite pins the
        // silent/latent short-production shape in `process_unpadded_counts`.

        // Staging needs from each child's declared bound (covers upsampling
        // headroom); actual production is verified after each call.
        let need_a = self.host_a.output_frames_for_input(num_frames);
        let need_b = self.host_b.output_frames_for_input(num_frames);
        let need_a_samples = need_a
            .checked_mul(channels)
            .ok_or_else(|| "A/B Compare child A staging sample count overflow".to_string())?;
        let need_b_samples = need_b
            .checked_mul(channels)
            .ok_or_else(|| "A/B Compare child B staging sample count overflow".to_string())?;

        // Verify staging room for both children and dry before either child
        // advances, so capacity errors leave every stream untouched.
        Self::ensure_queue_spare(
            &mut self.process_queues[0],
            &mut self.process_queue_start[0],
            need_a_samples,
        )?;
        Self::ensure_queue_spare(
            &mut self.process_queues[1],
            &mut self.process_queue_start[1],
            need_b_samples,
        )?;
        Self::ensure_queue_spare(
            &mut self.dry_queue,
            &mut self.dry_queue_start,
            input_samples,
        )?;

        // Advance the latency-aligned dry path first (same order as direct).
        let dry_len = self.dry_queue.len();
        self.dry_queue.resize(dry_len + input_samples, 0.0);
        self.dry_queue[dry_len..].copy_from_slice(input);
        self.delay_dry.process(&mut self.dry_queue[dry_len..]);

        // Process path A into queue spare capacity. The unpadded host
        // return is actual production by construction, so it stages
        // directly with no reporting-channel fallback.
        let queue_a_len = self.process_queues[0].len();
        self.process_queues[0].resize(queue_a_len + need_a_samples, 0.0);
        let produced_a = match self
            .host_a
            .process_unpadded(input, &mut self.process_queues[0][queue_a_len..])
        {
            Ok(frames) => frames,
            Err(error) => {
                self.process_queues[0].truncate(queue_a_len);
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
        };

        // Process path B into queue spare capacity.
        let queue_b_len = self.process_queues[1].len();
        self.process_queues[1].resize(queue_b_len + need_b_samples, 0.0);
        let produced_b = match self
            .host_b
            .process_unpadded(input, &mut self.process_queues[1][queue_b_len..])
        {
            Ok(frames) => frames,
            Err(error) => {
                self.process_queues[1].truncate(queue_b_len);
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
        };

        // Post-advance verification: rates are exact once hosts are built
        // (always post-initialize); an unbuilt first block fails closed here
        // instead of mixing a misclocked burst. Every output must match, so
        // a misclocked multi-sink sibling cannot hide behind the first.
        if !self
            .host_a
            .all_output_sample_rates_equal(self.sample_rate, self.sample_rate)
            || !self
                .host_b
                .all_output_sample_rates_equal(self.sample_rate, self.sample_rate)
        {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err("A/B Compare nested path output clock differs from the outer clock".into());
        }
        let produced_a_samples = produced_a
            .checked_mul(channels)
            .ok_or_else(|| "A/B Compare child A output sample count overflow".to_string())?;
        let produced_b_samples = produced_b
            .checked_mul(channels)
            .ok_or_else(|| "A/B Compare child B output sample count overflow".to_string())?;
        if produced_a_samples > need_a_samples || produced_b_samples > need_b_samples {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err("A/B Compare child exceeded its declared output capacity".into());
        }
        self.process_queues[0].truncate(queue_a_len + produced_a_samples);
        self.process_queues[1].truncate(queue_b_len + produced_b_samples);

        // Emit the stream-position-aligned prefix shared by both paths and
        // dry. Retained excess stays queued in order for later blocks.
        let queued_a = self.process_queue_frames(0);
        let queued_b = self.process_queue_frames(1);
        let queued_dry = self.dry_queue_frames();
        let emit = queued_a.min(queued_b).min(queued_dry).min(num_frames);
        if emit == 0 {
            self.last_process_frames = 0;
            return Ok(0);
        }
        // emit <= num_frames and num_frames * channels is checked above.
        let emit_samples = emit * channels;
        {
            let start = self.process_queue_start[0];
            let (queue, buffer) = (&self.process_queues[0], &mut self.buffers[0]);
            buffer[..emit_samples].copy_from_slice(&queue[start..start + emit_samples]);
        }
        self.delay_a.process(&mut self.buffers[0][..emit_samples]);
        {
            let start = self.process_queue_start[1];
            let (queue, buffer) = (&self.process_queues[1], &mut self.buffers[1]);
            buffer[..emit_samples].copy_from_slice(&queue[start..start + emit_samples]);
        }
        self.delay_b.process(&mut self.buffers[1][..emit_samples]);
        {
            let start = self.dry_queue_start;
            output[..emit_samples].copy_from_slice(&self.dry_queue[start..start + emit_samples]);
        }

        if let Err(error) = self.mix_prepared_buffers(&mut output[..emit_samples], emit) {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }
        Self::consume_queue(
            &mut self.process_queues[0],
            &mut self.process_queue_start[0],
            emit_samples,
        );
        Self::consume_queue(
            &mut self.process_queues[1],
            &mut self.process_queue_start[1],
            emit_samples,
        );
        Self::consume_queue(&mut self.dry_queue, &mut self.dry_queue_start, emit_samples);
        if self.band_mask_active() {
            self.band_mask_used_since_reset = true;
        }

        self.last_process_frames = emit;
        Ok(emit)
    }

    fn mask_flush_complete(&self) -> bool {
        match self.mask_flush_remaining {
            None => !(self.band_mask_active() || self.band_mask_used_since_reset),
            Some(remaining) => remaining == 0,
        }
    }

    fn drain_complete(&self) -> bool {
        self.drain_children
            .iter()
            .all(|child| child.complete && child.queued_remaining() == 0)
            && self.drain_delay_remaining.iter().all(|&frames| frames == 0)
            && self.process_queue_frames(0) == 0
            && self.process_queue_frames(1) == 0
            && self.dry_queue_frames() == 0
            && self.mask_flush_complete()
    }

    /// Pending delay-ring flush frames for a drain lane (0/1 child paths,
    /// 2 dry). Before `begin_drain` arms the counters the structural ring
    /// length is the exact future arm value; afterwards the live remainder.
    fn drain_delay_frames(&self, lane: usize) -> usize {
        if self.drain_lifecycle == DrainLifecycle::Accepting {
            match lane {
                0 => self.delay_a.delay_frames(self.num_channels),
                1 => self.delay_b.delay_frames(self.num_channels),
                _ => self.delay_dry.delay_frames(self.num_channels),
            }
        } else if lane < self.drain_delay_remaining.len() {
            self.drain_delay_remaining[lane]
        } else {
            0
        }
    }

    fn drain_capacity_frames(&self) -> usize {
        // Retained process-phase queues need at least one frame of per-call
        // capacity to make progress even when every host/ring bound is zero.
        let queue_floor = usize::from(
            self.process_queue_frames(0) > 0
                || self.process_queue_frames(1) > 0
                || self.dry_queue_frames() > 0,
        );
        // Mask history may emit residual flush frames after all other
        // content exhausts; one frame of per-call capacity keeps that
        // phase progressing even when every host/ring bound is zero.
        let mask_floor = usize::from(self.band_mask_active() || self.band_mask_used_since_reset);
        self.host_a
            .drain_output_frames_max()
            .max(self.host_b.drain_output_frames_max())
            .max(self.delay_a.delay_frames(self.num_channels))
            .max(self.delay_b.delay_frames(self.num_channels))
            .max(self.delay_dry.delay_frames(self.num_channels))
            .max(queue_floor)
            .max(mask_floor)
            .min(Self::MAX_REALTIME_FRAMES)
    }

    fn validate_drain_host(
        &self,
        host: &DawHost,
        drain_buffer: &[f32],
        path_name: &str,
    ) -> PluginResult<()> {
        if host.input_channels() != self.num_channels || host.output_channels() != self.num_channels
        {
            return Err(format!(
                "A/B Compare {path_name} drain requires {} input/output channels, got {}/{}",
                self.num_channels,
                host.input_channels(),
                host.output_channels()
            ));
        }
        if !host.all_output_sample_rates_equal(self.sample_rate, self.sample_rate) {
            return Err(format!(
                "A/B Compare {path_name} drain requires a same-rate child path"
            ));
        }
        // No silence gate (R20 disposition): child drain counts come
        // straight from the host drain return, which reports actual frames
        // on every path (chain, graph, and completion sentinels), so honest
        // silent-variable and compensating paths drain coherently with
        // process. Adversarial geometry is refused through truthful
        // contract validation instead of rejecting all silence: a child
        // that produces past its declared bound fails loudly at the
        // staging/retention guards with the measured evidence (see the
        // lying-geometry tests), never silently.
        let bound_frames = host.drain_output_frames_max();
        let required_samples = bound_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| format!("A/B Compare {path_name} drain capacity overflow"))?;
        if required_samples > drain_buffer.len() {
            return Err(format!(
                "A/B Compare {path_name} drain staging overflow: host bound {bound_frames} frames exceeds prepared staging {} frames",
                drain_buffer.len() / self.num_channels
            ));
        }
        Ok(())
    }
}

impl Plugin for ABComparePlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("A/B Compare", env!("CARGO_PKG_VERSION"), "SotF")
            .with_description("A/B comparison with automatic loudness matching")
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }

    fn output_channels(&self) -> usize {
        self.num_channels
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::boundary(PluginCostClass::External, self.latency_samples())
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }

    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        if let Some(param) = self.cached_parameters.iter().find(|p| p.id == *id) {
            param.validate(value).map_err(|e| format!("{}: {}", id, e))
        } else {
            Err(format!("Unknown parameter: {}", id))
        }
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.drain_lifecycle != DrainLifecycle::Accepting {
            return Err("A/B Compare parameters are frozen until reset after drain begins".into());
        }
        self.validate_parameter(&id, &value)?;
        if crate::params::PARAMS
            .iter()
            .find(|spec| spec.engine_key == id.as_str())
            .is_some_and(|spec| spec.update_mode == UpdateMode::Structural)
        {
            return Err(format!(
                "parameter '{}' is structural; rebuild the outer plugin graph to change it",
                id.as_str()
            ));
        }
        match id.as_str() {
            "mix" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "mix must be a float".to_string())?;
                if v.is_finite() {
                    self.mix = v.clamp(-1.0, 1.0);
                    self.transition_smoothers.mix.set_target(self.mix);
                    self.recompute_empty_path_fast_gain();
                }
            }
            "mix_mode" => {
                let v = value
                    .as_int()
                    .ok_or_else(|| "mix_mode must be an integer".to_string())?;
                self.mix_mode = if v == 0 {
                    MixMode::Potentiometer
                } else {
                    MixMode::Binary
                };
            }
            "selected_path" => {
                let v = value
                    .as_int()
                    .ok_or_else(|| "selected_path must be an integer".to_string())?;
                self.selected_path = v.clamp(0, 1);
                // Update mix target for binary mode
                if self.mix_mode == MixMode::Binary {
                    let target = if self.selected_path == 0 { -1.0 } else { 1.0 };
                    self.transition_smoothers.mix.set_target(target);
                    self.recompute_empty_path_fast_gain();
                }
            }
            "bypass" => {
                self.bypass = value
                    .as_bool()
                    .ok_or_else(|| "bypass must be a boolean".to_string())?;
                self.transition_smoothers
                    .bypass
                    .set_target(if self.bypass { 1.0 } else { 0.0 });
            }
            "auto_gain_enabled" => {
                self.auto_gain.set_enabled(
                    value
                        .as_bool()
                        .ok_or_else(|| "auto_gain_enabled must be a boolean".to_string())?,
                );
            }
            "loudness_type" => {
                let v = value
                    .as_int()
                    .ok_or_else(|| "loudness_type must be an integer".to_string())?;
                let loudness_type = if v == 0 {
                    AutoGainLoudnessType::Momentary
                } else {
                    AutoGainLoudnessType::ShortTerm
                };
                self.auto_gain.set_loudness_type(loudness_type);
            }
            "max_auto_gain_db" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "max_auto_gain_db must be a float".to_string())?;
                if v.is_finite() {
                    self.auto_gain.set_max_gain_db(v.clamp(0.0, 24.0));
                }
            }
            "gain_smoothing_ms" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "gain_smoothing_ms must be a float".to_string())?;
                if v.is_finite() {
                    self.auto_gain.set_smoothing_ms(v.clamp(1.0, 500.0));
                }
            }
            "mix_transition_ms" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "mix_transition_ms must be a float".to_string())?;
                if v.is_finite() {
                    self.mix_transition_ms = v.clamp(1.0, 500.0);
                    self.transition_smoothers
                        .mix
                        .set_time(self.mix_transition_ms, self.sample_rate);
                    self.transition_smoothers
                        .bypass
                        .set_time(self.mix_transition_ms, self.sample_rate);
                }
            }
            "phase_invert_a" => {
                self.phase_invert[0] = value
                    .as_bool()
                    .ok_or_else(|| "phase_invert_a must be a boolean".to_string())?;
            }
            "phase_invert_b" => {
                self.phase_invert[1] = value
                    .as_bool()
                    .ok_or_else(|| "phase_invert_b must be a boolean".to_string())?;
            }
            "difference_mode" => {
                self.difference_mode = value
                    .as_bool()
                    .ok_or_else(|| "difference_mode must be a boolean".to_string())?;
            }
            "band_mask_low_hz" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "band_mask_low_hz must be a float".to_string())?;
                if v.is_finite() {
                    self.band_mask_low_hz = v.clamp(20.0, 20000.0);
                    self.rebuild_band_mask_filters();
                }
            }
            "band_mask_high_hz" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "band_mask_high_hz must be a float".to_string())?;
                if v.is_finite() {
                    self.band_mask_high_hz = v.clamp(20.0, 20000.0);
                    self.rebuild_band_mask_filters();
                }
            }
            "path_a_config" => {
                if let ParameterValue::String(json) = value {
                    let config: PathConfig = serde_json::from_str(&json)
                        .map_err(|e| format!("Invalid path A config JSON: {}", e))?;
                    let mut candidate = super::factory::build_path_from_config_with_factory(
                        &config,
                        self.num_channels,
                        self.sample_rate,
                        self.plugin_factory,
                    )?;
                    candidate
                        .build()
                        .map_err(|e| format!("Invalid path A graph: {e}"))?;
                    self.host_b
                        .build()
                        .map_err(|e| format!("Existing path B graph is invalid: {e}"))?;
                    self.host_a = candidate;
                    self.path_a_config = config;
                    self.update_latency_compensation()?;
                }
            }
            "path_b_config" => {
                if let ParameterValue::String(json) = value {
                    let config: PathConfig = serde_json::from_str(&json)
                        .map_err(|e| format!("Invalid path B config JSON: {}", e))?;
                    let mut candidate = super::factory::build_path_from_config_with_factory(
                        &config,
                        self.num_channels,
                        self.sample_rate,
                        self.plugin_factory,
                    )?;
                    candidate
                        .build()
                        .map_err(|e| format!("Invalid path B graph: {e}"))?;
                    self.host_a
                        .build()
                        .map_err(|e| format!("Existing path A graph is invalid: {e}"))?;
                    self.host_b = candidate;
                    self.path_b_config = config;
                    self.update_latency_compensation()?;
                }
            }
            _ => return Err(format!("Unknown parameter: {}", id.0)),
        }
        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "mix" => Some(ParameterValue::Float(self.mix)),
            "mix_mode" => Some(ParameterValue::Int(match self.mix_mode {
                MixMode::Potentiometer => 0,
                MixMode::Binary => 1,
            })),
            "selected_path" => Some(ParameterValue::Int(self.selected_path)),
            "bypass" => Some(ParameterValue::Bool(self.bypass)),
            "auto_gain_enabled" => Some(ParameterValue::Bool(self.auto_gain.is_enabled())),
            "loudness_type" => Some(ParameterValue::Int(match self.auto_gain.loudness_type() {
                AutoGainLoudnessType::Momentary => 0,
                AutoGainLoudnessType::ShortTerm => 1,
            })),
            "max_auto_gain_db" => Some(ParameterValue::Float(self.auto_gain.max_gain_db())),
            "gain_smoothing_ms" => Some(ParameterValue::Float(self.auto_gain.smoothing_ms())),
            "mix_transition_ms" => Some(ParameterValue::Float(self.mix_transition_ms)),
            "phase_invert_a" => Some(ParameterValue::Bool(self.phase_invert[0])),
            "phase_invert_b" => Some(ParameterValue::Bool(self.phase_invert[1])),
            "difference_mode" => Some(ParameterValue::Bool(self.difference_mode)),
            "band_mask_low_hz" => Some(ParameterValue::Float(self.band_mask_low_hz)),
            "band_mask_high_hz" => Some(ParameterValue::Float(self.band_mask_high_hz)),
            "path_a_config" => serde_json::to_string(&self.path_a_config)
                .ok()
                .map(ParameterValue::String),
            "path_b_config" => serde_json::to_string(&self.path_b_config)
                .ok()
                .map(ParameterValue::String),
            _ => None,
        }
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("A/B Compare graph requires a finite positive sample rate".into());
        }
        let nyquist = sample_rate as f32 * 0.5;
        if self.band_mask_low_hz > Self::BAND_MASK_MIN_HZ + Self::BAND_MASK_EDGE_EPSILON
            && self.band_mask_low_hz >= nyquist
        {
            return Err(format!(
                "band_mask_low_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        if self.band_mask_high_hz < Self::BAND_MASK_MAX_HZ - Self::BAND_MASK_EDGE_EPSILON
            && self.band_mask_high_hz >= nyquist
        {
            return Err(format!(
                "band_mask_high_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        // Prepare every fallible component before committing any live state.
        let mut host_a = build_path_from_config_with_factory(
            &self.path_a_config,
            self.num_channels,
            sample_rate,
            self.plugin_factory,
        )?;
        let mut host_b = build_path_from_config_with_factory(
            &self.path_b_config,
            self.num_channels,
            sample_rate,
            self.plugin_factory,
        )?;
        host_a.build()?;
        host_b.build()?;
        let auto_gain = AutoGain::new(
            self.num_channels,
            sample_rate,
            AutoGainParams {
                enabled: self.auto_gain.is_enabled(),
                loudness_type: self.auto_gain.loudness_type(),
                max_gain_db: self.auto_gain.max_gain_db(),
                smoothing_ms: self.auto_gain.smoothing_ms(),
            },
        )?;
        let latency_a = host_a.total_latency_samples();
        let latency_b = host_b.total_latency_samples();
        let mut delay_a = DelayLine::new();
        let mut delay_b = DelayLine::new();
        let mut delay_dry = DelayLine::new();
        if latency_a > latency_b {
            delay_b.set_delay(latency_a - latency_b, self.num_channels);
        } else {
            delay_a.set_delay(latency_b - latency_a, self.num_channels);
        }
        delay_dry.set_delay(latency_a.max(latency_b), self.num_channels);

        let mut drain_buffers = [Vec::new(), Vec::new()];
        for buffer in drain_buffers.iter_mut() {
            // Fixed staging (see DRAIN_STAGING_FRAMES): the fresh host bound
            // understates mid-stream drain production, so preparation must
            // not size from it. Admission against the live bound happens in
            // validate_drain_host.
            let samples = Self::DRAIN_STAGING_FRAMES
                .checked_mul(self.num_channels)
                .ok_or_else(|| "A/B Compare child drain buffer size overflow".to_string())?;
            buffer.try_reserve_exact(samples).map_err(|error| {
                format!("A/B Compare child drain buffer allocation failed: {error}")
            })?;
            buffer.resize(samples, 0.0);
        }

        let queue_samples = Self::PROCESS_QUEUE_FRAMES
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare process staging size overflow".to_string())?;
        let mut process_queues = [Vec::new(), Vec::new()];
        for queue in process_queues.iter_mut() {
            queue.try_reserve_exact(queue_samples).map_err(|error| {
                format!("A/B Compare process staging allocation failed: {error}")
            })?;
        }
        let mut dry_queue = Vec::new();
        dry_queue
            .try_reserve_exact(queue_samples)
            .map_err(|error| format!("A/B Compare dry staging allocation failed: {error}"))?;

        self.sample_rate = sample_rate;
        self.host_a = host_a;
        self.host_b = host_b;
        self.auto_gain = auto_gain;
        self.delay_a = delay_a;
        self.delay_b = delay_b;
        self.delay_dry = delay_dry;
        self.drain_buffers = drain_buffers;
        self.drain_children = [ChildDrainProgress::default(); 2];
        self.drain_delay_remaining = [0; 3];
        self.drain_lifecycle = DrainLifecycle::Accepting;
        self.band_mask_used_since_reset = false;
        self.process_queues = process_queues;
        self.process_queue_start = [0; 2];
        self.dry_queue = dry_queue;
        self.dry_queue_start = 0;
        self.last_process_frames = 0;

        // Reset mix smoother with new sample rate
        self.transition_smoothers.mix =
            Smoother::new(self.mix, self.mix_transition_ms, sample_rate);
        self.transition_smoothers.bypass = Smoother::new(
            if self.bypass { 1.0 } else { 0.0 },
            self.mix_transition_ms,
            sample_rate,
        );
        self.recompute_empty_path_fast_gain();

        // Fresh zeroed band mask filters for the new stream (a rate change
        // invalidates prior IIR state), with matching zeroed history so the
        // residual proof starts from a consistent state.
        self.reset_band_mask_filter_state();
        self.mask_hist_hp.fill(MaskStageHistory::default());
        self.mask_hist_lp.fill(MaskStageHistory::default());
        self.mask_wet_peak = 0.0;
        self.mask_flush_remaining = None;

        // Pre-allocate processing buffers for max expected frame size (avoids hot-path resize)
        let max_buffer = Self::MAX_REALTIME_FRAMES * self.num_channels;
        for buffer in &mut self.buffers {
            if buffer.len() < max_buffer {
                buffer.resize(max_buffer, 0.0);
            }
        }
        self.cache_update_counter = 0;

        Ok(())
    }

    fn reset(&mut self) {
        // Reset hosts
        self.host_a.reset();
        self.host_b.reset();

        // Reset auto-gain (also resets loudness monitors)
        self.auto_gain.reset();

        // Reset delay lines
        self.delay_a.reset();
        self.delay_b.reset();

        // Reset mix smoother
        self.transition_smoothers.mix.reset(self.mix);
        self.transition_smoothers
            .bypass
            .reset(if self.bypass { 1.0 } else { 0.0 });

        // Reset peak values
        self.last_peaks = [0.0; 2];
        self.cache_update_counter = 0;

        // Reset band mask filters and their residual history.
        self.reset_band_mask_filter_state();
        self.mask_hist_hp.fill(MaskStageHistory::default());
        self.mask_hist_lp.fill(MaskStageHistory::default());
        self.mask_wet_peak = 0.0;
        self.mask_flush_remaining = None;

        // Clear contents without dropping pre-allocated capacity/length.
        self.buffers[0].fill(0.0);
        self.buffers[1].fill(0.0);
        self.drain_buffers[0].fill(0.0);
        self.drain_buffers[1].fill(0.0);
        for queue in self.process_queues.iter_mut() {
            queue.clear();
        }
        self.process_queue_start = [0; 2];
        self.dry_queue.clear();
        self.dry_queue_start = 0;
        self.last_process_frames = 0;
        self.delay_dry.reset();
        self.drain_children = [ChildDrainProgress::default(); 2];
        self.drain_delay_remaining = [0; 3];
        self.drain_lifecycle = DrainLifecycle::Accepting;
        self.band_mask_used_since_reset = false;

        // Update diagnostic cache immediately with reset values
        let data = ABCompareData {
            loudness_a_lufs: f64::NEG_INFINITY,
            loudness_b_lufs: f64::NEG_INFINITY,
            auto_gain_db: 0.0,
            peak_a: 0.0,
            peak_b: 0.0,
            current_mix: self.transition_smoothers.mix.current(),
            bypass_active: self.bypass,
        };
        self.cache.update(|d| {
            *d = data;
        });
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let expected_samples = context
            .num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare block sample count overflow".to_string())?;

        // Verify input/output size
        if input.len() != expected_samples {
            return Err(format!(
                "Input size mismatch: expected {}, got {}",
                expected_samples,
                input.len()
            ));
        }
        if output.len() != expected_samples {
            return Err(format!(
                "Output size mismatch: expected {}, got {}",
                expected_samples,
                output.len()
            ));
        }

        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "A/B Compare was initialized at {} Hz, got a {} Hz process context",
                self.sample_rate, context.sample_rate
            ));
        }
        if let Some(index) = input.iter().position(|sample| !sample.is_finite()) {
            return Err(format!(
                "A/B Compare input contains a non-finite sample at index {index}"
            ));
        }
        if context.num_frames == 0 {
            self.last_process_frames = 0;
            return Ok(0);
        }
        if self.drain_lifecycle != DrainLifecycle::Accepting {
            return Err("A/B Compare requires reset before processing after drain begins".into());
        }

        if self.can_use_empty_path_fast_path() {
            // Empty paths make wet and dry audio identical, but bypass state
            // must still advance by the exact number of rendered samples.
            // Resetting here made a transition restart on every callback and
            // therefore made its duration depend on host block partitioning.
            self.transition_smoothers.bypass.next_n(context.num_frames);
            if let Err(error) = self.process_empty_path_fast(input, output, context.num_frames) {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
            self.last_process_frames = context.num_frames;
            return Ok(context.num_frames);
        }

        // Rate preflight: every nested path output must produce the outer
        // clock (the factory converts off-clock paths at construction;
        // this guards the invariant on the audio thread).
        // Refused before either child advances, so output, child DSP state,
        // and lifecycle are untouched and the caller keeps the accepted
        // stream.
        let off_a = !self
            .host_a
            .all_output_sample_rates_equal(self.sample_rate, self.sample_rate);
        let off_b = !self
            .host_b
            .all_output_sample_rates_equal(self.sample_rate, self.sample_rate);
        if off_a || off_b {
            let paths = if off_a && off_b {
                "paths A and B"
            } else if off_a {
                "path A"
            } else {
                "path B"
            };
            return Err(format!(
                "A/B Compare requires every nested path output at the outer {} Hz clock; off-clock: {}",
                self.sample_rate, paths
            ));
        }

        // Identity hosts always produce exactly num_frames; run the accepted
        // direct path bit-identically. Same-clock variable production takes
        // the staging-queue path, which pairs frames by stream position.
        if self.host_a.has_identity_frame_geometry() && self.host_b.has_identity_frame_geometry() {
            self.process_direct(input, output, context.num_frames, expected_samples)
        } else {
            self.process_queued(input, output, context.num_frames)
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        self.drain_capacity_frames()
    }

    fn tail_length(&self) -> TailLength {
        match self.drain_lifecycle {
            DrainLifecycle::Complete => return TailLength::Finite(0),
            DrainLifecycle::ResetRequired => return TailLength::Unknown,
            DrainLifecycle::Accepting | DrainLifecycle::Draining => {}
        }
        // Mixer maximum over path totals, mirroring `drain`: each path emits
        // its retained process output, staged drain content, child host
        // future, and delay-ring flush in stream order (retention transfer
        // is a move, so the three child-content stores are disjoint), and
        // the mixer paces by the per-call minimum over live paths with
        // exhausted paths contributing silence, so emission continues until
        // the longest path (or dry lane) exhausts. Same-rate children keep
        // every term in the outer clock; nested hosts (converting paths
        // included) compose through their own remaining-tail fold.
        let mut mixed: u64 = 0;
        for child_index in 0..2 {
            let host = if child_index == 0 {
                &self.host_a
            } else {
                &self.host_b
            };
            let child_tail = match host.tail_length() {
                TailLength::Finite(frames) => frames,
                TailLength::Infinite => return TailLength::Infinite,
                TailLength::Unknown => match host.tail_support() {
                    Some(bound) => bound,
                    None => return TailLength::Unknown,
                },
            };
            let path_total = (|| -> Option<u64> {
                let retained = u64::try_from(self.process_queue_frames(child_index)).ok()?;
                let staged =
                    u64::try_from(self.drain_children[child_index].queued_remaining()).ok()?;
                let delay = u64::try_from(self.drain_delay_frames(child_index)).ok()?;
                retained
                    .checked_add(staged)?
                    .checked_add(child_tail)?
                    .checked_add(delay)
            })();
            let Some(path_total) = path_total else {
                return TailLength::Unknown;
            };
            mixed = mixed.max(path_total);
        }
        let dry_total = (|| -> Option<u64> {
            let queued = u64::try_from(self.dry_queue_frames()).ok()?;
            let delay = u64::try_from(self.drain_delay_frames(2)).ok()?;
            queued.checked_add(delay)
        })();
        let Some(dry_total) = dry_total else {
            return TailLength::Unknown;
        };
        mixed = mixed.max(dry_total);
        // The mask flush arms once every other lane is quiet, deriving its
        // length from live filter state at that moment. An armed flush is
        // exact; an inactive mask emits nothing whether or not it was used
        // before (the mixer never steps inactive filters, and parameters
        // are frozen during drain, so inactive-at-derivation is final —
        // later activation is a new state the caller re-queries). An
        // un-armed ACTIVE mask with live history is `Unknown` while
        // content can still drive it -- except with no future content
        // anywhere, when the filter state is frozen and the live
        // derivation IS the final arm value, so it composes exactly.
        let mask_frames = match self.mask_flush_remaining {
            Some(remaining) => {
                let Ok(frames) = u64::try_from(remaining) else {
                    return TailLength::Unknown;
                };
                frames
            }
            None if !self.band_mask_active() => 0,
            None if mixed == 0 => match self.mask_flush_frames_required() {
                Ok(required) => {
                    let Ok(frames) = u64::try_from(required) else {
                        return TailLength::Unknown;
                    };
                    frames
                }
                Err(_) => return TailLength::Unknown,
            },
            None => return TailLength::Unknown,
        };
        match mixed.checked_add(mask_frames) {
            Some(total) => TailLength::Finite(total),
            None => TailLength::Unknown,
        }
    }

    fn tail_support(&self) -> Option<u64> {
        // Unproven: a state-independent mask support needs a uniform flush
        // horizon over all reachable (history, wet-peak) pairs. The live
        // per-state horizon is rigorously derived from pole decay, and the
        // linear filter makes it scale-invariant, but the peak-gain factor
        // bounding normalized state shapes by the wet peak is not yet
        // derived (notably biquad transient overshoot past steady-state
        // gain). Until that coefficient-derived support exists, no finite
        // value dominates every active-mask state, so this stays honestly
        // `None`; live tails compose through `tail_length`.
        None
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        // Mirrors `drain_capacity_frames` with nested host envelopes: each
        // host envelope dominates its live bound, delay lines are
        // configuration-structural, and the state-dependent queue/mask
        // progress floors (0/1) lift to the constant 1. The staging cap
        // composes monotonically, so this dominates the live bound in
        // every state. Any unknown child envelope keeps live sizing.
        let hosts = self
            .host_a
            .drain_frames_envelope()?
            .max(self.host_b.drain_frames_envelope()?);
        Some(
            hosts
                .max(self.delay_a.delay_frames(self.num_channels))
                .max(self.delay_b.delay_frames(self.num_channels))
                .max(self.delay_dry.delay_frames(self.num_channels))
                .clamp(1, Self::MAX_REALTIME_FRAMES),
        )
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        // The direct path emits exactly `num_frames` and the staging-queue
        // path emits `K <= num_frames`, so `input_frames` bounds every
        // success path in every state (same value as the live identity).
        Some(input_frames)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        // Derived work quota, queried after a successful begin_drain per
        // trait order. Every lane contributes its child pumps (child
        // future over child capacity, plus the completion handshake) and
        // its re-emission through this drain's capacity, summed for a
        // sequential worst case — lanes actually drain concurrently, so
        // the sum dominates with room to spare; the dry lane likewise.
        // The mask flush arms only at quiet, so its calls are additive.
        // All checked; unprovable child tails, Infinite children, and
        // arithmetic overflow answer None honestly (the host's finite
        // fallback then trips loudly if it cannot cover the true work).
        // An active-but-unarmed mask returns the to-quiet budget as a
        // partial promise (see below): eager arming covers every quiet
        // state, so unarmed-at-query always means genuinely driven, and
        // the host's single refresh covers the armed remainder.
        if self.drain_lifecycle != DrainLifecycle::Draining {
            return None;
        }
        let capacity_frames = self.drain_capacity_frames();
        if capacity_frames == 0 {
            // No per-call progress is possible; one call either completes
            // (no content) or trips the host quota loudly.
            return std::num::NonZeroU64::new(1);
        }
        let capacity = u64::try_from(capacity_frames).ok()?;
        let mut total: u64 = 1;
        for child_index in 0..2 {
            let host = if child_index == 0 {
                &self.host_a
            } else {
                &self.host_b
            };
            let child_tail = match host.tail_length() {
                TailLength::Finite(frames) => frames,
                TailLength::Infinite => return None,
                TailLength::Unknown => host.tail_support()?,
            };
            let child_total = u64::try_from(self.process_queue_frames(child_index))
                .ok()?
                .checked_add(
                    u64::try_from(self.drain_children[child_index].queued_remaining()).ok()?,
                )?
                .checked_add(child_tail)?
                .checked_add(u64::try_from(self.drain_delay_frames(child_index)).ok()?)?;
            let child_capacity = u64::try_from(host.drain_output_frames_max().max(1)).ok()?;
            let child_pumps = child_total.div_ceil(child_capacity).checked_add(1)?;
            let lane_emit = child_total.div_ceil(capacity);
            total = total.checked_add(child_pumps)?.checked_add(lane_emit)?;
        }
        let dry_total = u64::try_from(self.dry_queue_frames())
            .ok()?
            .checked_add(u64::try_from(self.drain_delay_frames(2)).ok()?)?;
        total = total.checked_add(dry_total.div_ceil(capacity))?;
        let mask_frames = match self.mask_flush_remaining {
            Some(remaining) => u64::try_from(remaining).ok()?,
            // Inactive masks never engage (frozen parameters make
            // inactive-at-derivation final; drain arms a zero flush), so
            // the to-quiet budget accumulated above is already total.
            None if !self.band_mask_active() => 0,
            // Active but unarmed with future content: the flush length
            // is underivable until the quiet-time arm, so return the
            // derived to-quiet budget as a partial promise (see the
            // `drain_call_bound` contract): the host grants one
            // re-queried budget once the mask arms and the tail turns
            // `Finite`. Never zero: `total` carries the handshake.
            None => return std::num::NonZeroU64::new(total),
        };
        total = total.checked_add(mask_frames.div_ceil(capacity))?;
        std::num::NonZeroU64::new(total)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if context.num_frames != 0 {
            return Err("A/B Compare drain preparation requires a zero-frame context".into());
        }
        if self.sample_rate == 0.0 || context.sample_rate != self.sample_rate {
            return Err(format!(
                "A/B Compare drain context must use its initialized sample rate of {} Hz",
                self.sample_rate
            ));
        }

        match self.drain_lifecycle {
            DrainLifecycle::Draining | DrainLifecycle::Complete => return Ok(()),
            DrainLifecycle::ResetRequired => {
                return Err("A/B Compare requires reset after a partial processing failure".into());
            }
            DrainLifecycle::Accepting => {}
        }

        if self.band_mask_active() {
            // Recursive mask history drains through the proven residual
            // flush (R2); only coefficient-unstable masks stay refused.
            // Inactive masks derive zero flush without touching envelopes.
            self.validate_mask_stability()?;
        }
        self.mask_flush_remaining = None;
        for (topology, path_name) in [
            (self.path_topology[0], "path A"),
            (self.path_topology[1], "path B"),
        ] {
            if topology == PathTopology::Unsupported {
                return Err(format!(
                    "A/B Compare {path_name} drain supports only linear Graph paths or connected single-source/single-sink DAGs"
                ));
            }
        }
        self.validate_drain_host(&self.host_a, &self.drain_buffers[0], "path A")?;
        self.validate_drain_host(&self.host_b, &self.drain_buffers[1], "path B")?;

        self.drain_children = [ChildDrainProgress::default(); 2];
        self.drain_delay_remaining = [
            self.delay_a.delay_frames(self.num_channels),
            self.delay_b.delay_frames(self.num_channels),
            self.delay_dry.delay_frames(self.num_channels),
        ];
        // Eager mask arming for the provably-quiet case: when no future
        // content exists anywhere — zero child futures (exact-zero live
        // tails, or live-Unknown with zero support, which bounds true
        // emission at nothing), empty process and staging queues, zero
        // delay rings and dry backlog — the filter state is frozen and
        // the live rigorous derivation IS the final arm value, so derive
        // it now. Behavior-preserving (the drain-time arming check would
        // derive the identical value on the first call from identical
        // state); it lets `drain_call_bound` cover the flush with a
        // derived quota instead of the host's finite unknown fallback,
        // which long 21 Hz tails can exceed. Anything unprovable stays
        // unarmed for the drain-time check. The quiet proof here matches
        // the fold's `mixed == 0` exactly (fresh staging is empty), so an
        // active mask that stays unarmed is always genuinely driven and
        // queries `Unknown` — the combination the quota refresh needs.
        if self.mask_flush_remaining.is_none()
            && (self.band_mask_active() || self.band_mask_used_since_reset)
            && self.drain_delay_remaining == [0, 0, 0]
            && self.process_queue_frames(0) == 0
            && self.process_queue_frames(1) == 0
            && self.dry_queue_frames() == 0
            && Self::child_future_is_zero(&self.host_a)
            && Self::child_future_is_zero(&self.host_b)
            && let Ok(required) = self.mask_flush_frames_required()
        {
            self.mask_flush_remaining = Some(required);
        }
        self.drain_lifecycle = DrainLifecycle::Draining;
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if context.num_frames != 0 || context.sample_rate != self.sample_rate {
            return Err(
                "A/B Compare drain requires a zero-frame context at its initialized rate".into(),
            );
        }
        match self.drain_lifecycle {
            DrainLifecycle::Complete => return Ok(PluginDrainResult::COMPLETE),
            DrainLifecycle::ResetRequired => {
                return Err("A/B Compare requires reset after a partial drain failure".into());
            }
            DrainLifecycle::Accepting => {
                return Err("A/B Compare begin_drain must succeed before drain".into());
            }
            DrainLifecycle::Draining => {}
        }

        if !output.len().is_multiple_of(self.num_channels) {
            return Err(format!(
                "A/B Compare drain output must contain whole {}-channel frames",
                self.num_channels
            ));
        }
        let capacity_frames = self.drain_capacity_frames();
        let required_samples = capacity_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare drain output capacity overflow".to_string())?;
        if output.len() < required_samples {
            return Err(format!(
                "A/B Compare drain output too small: need {required_samples} samples, got {}",
                output.len()
            ));
        }

        for child_index in 0..2 {
            if let Err(error) = self.pump_child_drain(child_index) {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
        }

        if (0..2).any(|index| {
            self.drain_children[index].queued_remaining() == 0
                && !self.drain_children[index].complete
        }) {
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }

        if self.drain_complete() {
            self.drain_lifecycle = DrainLifecycle::Complete;
            return Ok(PluginDrainResult::COMPLETE);
        }

        let mut frames = capacity_frames;
        for child_index in 0..2 {
            let progress = self.drain_children[child_index];
            let queued = progress.queued_remaining();
            if queued > 0 {
                frames = frames.min(queued);
            } else if progress.complete && self.drain_delay_remaining[child_index] > 0 {
                frames = frames.min(self.drain_delay_remaining[child_index]);
            }
        }
        let dry_available = self
            .dry_queue_frames()
            .saturating_add(self.drain_delay_remaining[2]);
        if dry_available > 0 {
            frames = frames.min(dry_available);
        }

        // Mask residual flush engages once all child/ring/dry content is
        // exhausted. The length derives from live filter state at that
        // moment (drain-phase mask inputs join the proof); emitted frames
        // are exact zero-input filter outputs through the shared mixer.
        if self.mask_flush_remaining.is_none()
            && (self.band_mask_active() || self.band_mask_used_since_reset)
        {
            let children_quiet = (0..2).all(|index| {
                self.drain_children[index].complete
                    && self.drain_children[index].queued_remaining() == 0
            }) && self.drain_delay_remaining[0] == 0
                && self.drain_delay_remaining[1] == 0;
            let dry_quiet = self.dry_queue_frames() == 0 && self.drain_delay_remaining[2] == 0;
            let queues_quiet =
                self.process_queue_frames(0) == 0 && self.process_queue_frames(1) == 0;
            if children_quiet && dry_quiet && queues_quiet {
                match self.mask_flush_frames_required() {
                    Ok(required) => self.mask_flush_remaining = Some(required),
                    Err(error) => {
                        self.drain_lifecycle = DrainLifecycle::ResetRequired;
                        return Err(error);
                    }
                }
            }
        }
        // A derived zero-length flush (silent mask state) emits nothing:
        // the mask floor in the per-call capacity must not produce a
        // spurious zero frame. Monotonic drain state (derived only when
        // quiet, children complete, rings/queues consume-only) guarantees no
        // other content exists then, so clamping to zero reaches COMPLETE
        // below instead of emitting.
        if let Some(remaining) = self.mask_flush_remaining {
            frames = frames.min(remaining);
        }

        if frames == 0 {
            if self.drain_complete() {
                self.drain_lifecycle = DrainLifecycle::Complete;
                return Ok(PluginDrainResult::COMPLETE);
            }
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }

        let samples = frames * self.num_channels;
        self.buffers[0][..samples].fill(0.0);
        self.buffers[1][..samples].fill(0.0);
        output[..samples].fill(0.0);

        for child_index in 0..2 {
            let progress = self.drain_children[child_index];
            let queued = progress.queued_remaining();
            let path_buffer = &mut self.buffers[child_index][..samples];
            if queued > 0 {
                let start = progress.consumed_frames * self.num_channels;
                path_buffer
                    .copy_from_slice(&self.drain_buffers[child_index][start..start + samples]);
                if child_index == 0 {
                    self.delay_a.process(path_buffer);
                } else {
                    self.delay_b.process(path_buffer);
                }
                self.drain_children[child_index].consumed_frames += frames;
            } else if self.drain_delay_remaining[child_index] > 0 {
                if child_index == 0 {
                    self.delay_a.process(path_buffer);
                } else {
                    self.delay_b.process(path_buffer);
                }
                self.drain_delay_remaining[child_index] -= frames;
            }
        }

        // Retained delayed dry output (earlier stream positions) precedes
        // the delay-ring flush.
        let from_dry_queue = self.dry_queue_frames().min(frames);
        if from_dry_queue > 0 {
            let queued_samples = from_dry_queue * self.num_channels;
            let start = self.dry_queue_start;
            output[..queued_samples]
                .copy_from_slice(&self.dry_queue[start..start + queued_samples]);
            Self::consume_queue(
                &mut self.dry_queue,
                &mut self.dry_queue_start,
                queued_samples,
            );
        }
        let dry_frames = (frames - from_dry_queue).min(self.drain_delay_remaining[2]);
        if dry_frames > 0 {
            let start_sample = from_dry_queue * self.num_channels;
            let dry_samples = dry_frames * self.num_channels;
            self.delay_dry
                .process(&mut output[start_sample..start_sample + dry_samples]);
            self.drain_delay_remaining[2] -= dry_frames;
        }

        if let Err(error) = self.mix_prepared_buffers(&mut output[..samples], frames) {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }
        if let Some(remaining) = self.mask_flush_remaining.as_mut() {
            *remaining = remaining.saturating_sub(frames);
        }
        if self.drain_complete() {
            self.drain_lifecycle = DrainLifecycle::Complete;
            return Ok(PluginDrainResult {
                frames,
                complete: true,
            });
        }
        Ok(PluginDrainResult {
            frames,
            complete: false,
        })
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }

    fn latency_samples(&self) -> usize {
        // Total latency is the max of both paths
        let latency_a = self.host_a.total_latency_samples();
        let latency_b = self.host_b.total_latency_samples();
        latency_a.max(latency_b)
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_process_frames)
    }
}

#[cfg(test)]
mod mask_proof_tests {
    use super::{ABComparePlugin, MaskStageHistory};
    use math_audio_iir_fir::{Biquad, BiquadCoefficients, BiquadFilterType};
    use sotf_host::plugin::{Plugin, ProcessContext, TailLength};

    #[test]
    fn zero_numerator_lp_derives_free_response() {
        // D1: zero-numerator LP with nonzero recorded state must derive the
        // LP free ring at full threshold, not return 0 (old code silently
        // truncated the free response the proof did not cover).
        let hp = BiquadCoefficients {
            b0: 0.5,
            b1: -1.0,
            b2: 0.5,
            a1: -0.5,
            a2: 0.25,
        };
        let lp = BiquadCoefficients {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: -0.5,
            a2: 0.25,
        };
        let lp_history = MaskStageHistory {
            in1: 0.0,
            in2: 0.0,
            out1: 1.0,
            out2: 0.5,
        };
        let threshold = 1.0e-6;
        let frames = ABComparePlugin::mask_channel_flush_frames(
            &hp,
            &lp,
            MaskStageHistory::default(),
            lp_history,
            threshold,
        )
        .unwrap();
        assert!(frames > 0, "nonzero free state needs nonzero flush");
        // Independent DF-I simulation: the remainder past the derived flush
        // stays below threshold.
        let (mut y1, mut y0) = (lp_history.out1, lp_history.out2);
        for n in 0..frames + 1000 {
            let y = -lp.a1 * y1 - lp.a2 * y0;
            y0 = y1;
            y1 = y;
            if n >= frames {
                assert!(
                    y.abs() < threshold,
                    "remainder {y} at frame {n} exceeds {threshold}"
                );
            }
        }
        // The fix path equals full-threshold LP free frames directly.
        let lp_envelope = ABComparePlugin::biquad_modal_envelope(&lp).unwrap();
        let direct =
            ABComparePlugin::biquad_free_frames(&lp, lp_envelope, lp_history, threshold).unwrap();
        assert_eq!(frames, direct);
    }

    #[test]
    fn repeated_pole_envelope_dominates_matrix_powers() {
        // D2: a1=-1,a2=0.25 (double pole 0.5). Structural: kappa carries the
        // +1 Jordan triangle term. Numeric: ||A^n||_inf <= kappa*mid^n by
        // direct 2x2 powering of the companion matrix.
        let coeffs = BiquadCoefficients {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: -1.0,
            a2: 0.25,
        };
        let (rho, kappa, _) = ABComparePlugin::biquad_modal_envelope(&coeffs).unwrap();
        let lambda = 0.5_f64;
        let mid = (1.0 + lambda) / 2.0;
        let ratio = lambda / mid;
        let peak = -1.0 / (std::f64::consts::E * ratio * ratio.ln());
        let jordan = ((1.0 - lambda).abs() + 0.25).max(1.0 + lambda);
        assert!(
            kappa >= 1.0 + peak * jordan / mid,
            "kappa {kappa} must carry the +1 triangle term"
        );
        let (mut p00, mut p01, mut p10, mut p11) = (1.0_f64, 0.0_f64, 0.0_f64, 1.0_f64);
        for n in 0..2000_u32 {
            let norm = (p00.abs() + p01.abs()).max(p10.abs() + p11.abs());
            let bound = kappa * rho.powi(n as i32);
            assert!(norm <= bound, "||A^{n}|| {norm} exceeds {bound}");
            let (q00, q01) = (p00 - 0.25 * p10, p01 - 0.25 * p11);
            (p00, p01, p10, p11) = (q00, q01, p00, p01);
        }
    }

    #[test]
    fn past_bound_free_frames_refuse() {
        // D3: exact in (u32::MAX-2, u32::MAX] needs frames past u32::MAX; the
        // old .min() clamp silently trimmed up to 3 unproven frames.
        let a2 = 1.0 - 2.0_f64.powi(-30);
        let coeffs = BiquadCoefficients {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2,
        };
        let envelope = ABComparePlugin::biquad_modal_envelope(&coeffs).unwrap();
        let (rho, kappa, _) = envelope;
        let history = MaskStageHistory {
            in1: 0.0,
            in2: 0.0,
            out1: 1.0,
            out2: 0.0,
        };
        let y0 = coeffs.b1 * history.in1 + coeffs.b2 * history.in2
            - coeffs.a1 * history.out1
            - coeffs.a2 * history.out2;
        let y1 = coeffs.b2 * history.in1 - coeffs.a1 * y0 - coeffs.a2 * history.out1;
        let state = y0.abs().max(y1.abs());
        let scale = kappa * state / rho * (1.0 + ABComparePlugin::MASK_ENVELOPE_SLACK);
        let exact_target = u32::MAX as f64 - 1.0;
        let threshold = scale * (rho.ln() * exact_target).exp();
        let error =
            ABComparePlugin::biquad_free_frames(&coeffs, envelope, history, threshold).unwrap_err();
        assert!(
            error.contains("exceeds its bound"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn checked_flush_total_bounds_sums() {
        // D3: the reviewer's exact counterexample — horizon=tail=3e9 must
        // refuse, not clamp to u32::MAX.
        let error = ABComparePlugin::checked_flush_total(3_000_000_000, 3_000_000_000).unwrap_err();
        assert!(
            error.contains("exceeds its bound"),
            "unexpected error: {error}"
        );
        assert_eq!(
            ABComparePlugin::checked_flush_total(u32::MAX as u64, 1).unwrap_err(),
            "A/B Compare band-mask flush exceeds its bound"
        );
        assert_eq!(
            ABComparePlugin::checked_flush_total(u64::MAX, u64::MAX).unwrap_err(),
            "A/B Compare band-mask flush exceeds its bound"
        );
        assert_eq!(ABComparePlugin::checked_flush_total(100, 200).unwrap(), 300);
        assert_eq!(ABComparePlugin::checked_flush_total(0, 0).unwrap(), 0);
        assert_eq!(
            ABComparePlugin::checked_flush_total(u32::MAX as u64, 0).unwrap(),
            u32::MAX as u64
        );
    }

    #[test]
    fn frozen_inactive_mask_tail_is_zero_without_content() {
        // D4-tail: inactive cutoffs + nonzero frozen histories/peak + used
        // flag compose zero mask frames (the mixer never steps inactive
        // filters); with empty paths the whole tail is exactly zero and
        // drain emits nothing. Frozen mid-stream state is unreachable via
        // public API (cutoffs are structural), so this is white-box like
        // the D4 derivation test.
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        assert!(!plugin.band_mask_active());
        plugin.mask_hist_hp.fill(frozen_histories());
        plugin.mask_hist_lp.fill(frozen_histories());
        plugin.mask_wet_peak = 1.0;
        plugin.band_mask_used_since_reset = true;
        let TailLength::Finite(pre) = plugin.tail_length() else {
            panic!("frozen inactive tail must be Finite");
        };
        assert_eq!(pre, 0, "frozen inactive mask contributes no tail");
        let drained = wb_drain(&mut plugin, 48_000);
        assert!(drained.is_empty(), "frozen inactive drain emits nothing");
        let TailLength::Finite(post) = plugin.tail_length() else {
            panic!("post-drain tail must stay Finite");
        };
        assert_eq!(post, 0);
    }

    #[test]
    fn frozen_active_mask_tail_is_exact_without_content() {
        // D4-tail companion: identical frozen state with ACTIVE cutoffs
        // derives a nonzero live horizon; with no future content anywhere
        // the filter state is frozen, so the live derivation IS the final
        // arm value and the pre-drain query equals the drain total exactly.
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.rebuild_band_mask_filters();
        assert!(plugin.band_mask_active());
        plugin.mask_hist_hp.fill(frozen_histories());
        plugin.mask_hist_lp.fill(frozen_histories());
        plugin.mask_wet_peak = 1.0;
        plugin.band_mask_used_since_reset = true;
        let TailLength::Finite(pre) = plugin.tail_length() else {
            panic!("frozen active tail must be Finite");
        };
        assert!(pre > 0, "active frozen state needs a nonzero tail");
        let drained = wb_drain(&mut plugin, 48_000);
        assert_eq!(
            drained.len() / 2,
            pre as usize,
            "drain must emit exactly the queried tail"
        );
        let TailLength::Finite(post) = plugin.tail_length() else {
            panic!("post-drain tail must stay Finite");
        };
        assert_eq!(post, 0);
        println!(
            "frozen active 500/8000 tail: queried {pre}, drained {} frames",
            drained.len() / 2
        );
    }

    fn frozen_histories() -> MaskStageHistory {
        MaskStageHistory {
            in1: 0.1,
            in2: 0.2,
            out1: 0.5,
            out2: 0.25,
        }
    }

    #[test]
    fn inactive_mask_derives_zero_flush_despite_frozen_histories() {
        // D4: inactive cutoffs + nonzero frozen histories/peak derive zero
        // flush (the mixer never steps inactive filters, so frozen state is
        // unobservable). Unreachable via public API today (cutoffs are
        // structural, pinned by `runtime_structural_parameters_require_plugin_rebuild`),
        // so the impossible state is constructed white-box.
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        assert!(!plugin.band_mask_active());
        plugin.mask_hist_hp.fill(frozen_histories());
        plugin.mask_hist_lp.fill(frozen_histories());
        plugin.mask_wet_peak = 1.0;
        plugin.band_mask_used_since_reset = true;
        assert_eq!(plugin.mask_flush_frames_required().unwrap(), 0);
    }

    #[test]
    fn active_mask_derives_nonzero_flush_for_same_histories() {
        // D4 non-vacuity companion: the identical frozen state derives a
        // nonzero flush once cutoffs are active, proving the D4 test above
        // is not vacuous.
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.rebuild_band_mask_filters();
        assert!(plugin.band_mask_active());
        plugin.mask_hist_hp.fill(frozen_histories());
        plugin.mask_hist_lp.fill(frozen_histories());
        plugin.mask_wet_peak = 1.0;
        plugin.band_mask_used_since_reset = true;
        let frames = plugin.mask_flush_frames_required().unwrap();
        assert!(frames > 0, "active frozen state needs nonzero flush");
    }

    #[test]
    fn cutoff_rebuild_preserves_observable_histories() {
        // D7 retune carryover (ABCompare side): rebuilding filters at new
        // cutoffs updates coefficients while preserving the recorded
        // observable histories and peak that mirror DF-I registers.
        // (Runtime retune via set_parameter is refused — cutoffs are
        // structural — so this pins the construction/initialize path
        // contract white-box; DF-I state preservation itself is math-audio's.)
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        plugin.mask_hist_hp.fill(frozen_histories());
        plugin.mask_hist_lp.fill(frozen_histories());
        plugin.mask_wet_peak = 0.75;
        let hp_before = plugin.band_mask_hp[0].coefficients();
        plugin.band_mask_low_hz = 200.0;
        plugin.band_mask_high_hz = 2_000.0;
        plugin.rebuild_band_mask_filters();
        let hp_after = plugin.band_mask_hp[0].coefficients();
        assert_ne!(hp_after.b0, hp_before.b0, "coefficients must update");
        for history in plugin.mask_hist_hp.iter().chain(plugin.mask_hist_lp.iter()) {
            let observed = (history.in1, history.in2, history.out1, history.out2);
            assert_eq!(observed, (0.1, 0.2, 0.5, 0.25));
        }
        assert_eq!(plugin.mask_wet_peak, 0.75);
    }

    /// Stateful HP→LP cascade oracle with retune, mirroring the plugin's
    /// per-sample f64 stage chain (`as f32` per sample) and `update_params`
    /// carryover exactly.
    struct CascadeOracle {
        hp: Vec<Biquad>,
        lp: Vec<Biquad>,
        channels: usize,
    }

    impl CascadeOracle {
        fn new(channels: usize, low_hz: f64, high_hz: f64, sample_rate: f64) -> Self {
            let q = 1.0 / std::f64::consts::SQRT_2;
            Self {
                hp: (0..channels)
                    .map(|_| Biquad::new(BiquadFilterType::Highpass, low_hz, sample_rate, q, 0.0))
                    .collect(),
                lp: (0..channels)
                    .map(|_| Biquad::new(BiquadFilterType::Lowpass, high_hz, sample_rate, q, 0.0))
                    .collect(),
                channels,
            }
        }

        /// Retune with state carryover, mirroring `rebuild_band_mask_filters`.
        fn retune(&mut self, low_hz: f64, high_hz: f64, sample_rate: f64) {
            let q = 1.0 / std::f64::consts::SQRT_2;
            for stage in &mut self.hp {
                stage.update_params(BiquadFilterType::Highpass, low_hz, sample_rate, q, 0.0);
            }
            for stage in &mut self.lp {
                stage.update_params(BiquadFilterType::Lowpass, high_hz, sample_rate, q, 0.0);
            }
        }

        fn feed(&mut self, wet: &[f32]) -> Vec<f32> {
            assert!(wet.len().is_multiple_of(self.channels));
            let mut output = Vec::with_capacity(wet.len());
            for frame in wet.chunks_exact(self.channels) {
                for (channel, &sample) in frame.iter().enumerate() {
                    let hp_out = self.hp[channel].process(f64::from(sample));
                    output.push(self.lp[channel].process(hp_out) as f32);
                }
            }
            output
        }

        fn tail_max(&mut self, extra_frames: usize) -> f64 {
            let zeros = vec![0.0; extra_frames * self.channels];
            self.feed(&zeros)
                .iter()
                .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())))
        }
    }

    fn wb_dense(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|index| {
                let frame = index / channels;
                let channel = index % channels;
                let value = ((frame * (7 + 4 * channel) + 3 * channel) % 23) as f32 - 11.0;
                value / 16.0
            })
            .collect()
    }

    fn wb_process(plugin: &mut ABComparePlugin, input: &[f32], rate: u32) -> Vec<f32> {
        let channels = plugin.num_channels;
        assert!(input.len().is_multiple_of(channels));
        let frames = input.len() / channels;
        let mut block = vec![0.0; input.len()];
        let produced = plugin
            .process(input, &mut block, &ProcessContext::new(rate, frames))
            .unwrap();
        assert_eq!(produced, frames, "unity paths preserve counts");
        block
    }

    fn wb_drain(plugin: &mut ABComparePlugin, rate: u32) -> Vec<f32> {
        plugin.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
        let capacity = plugin.drain_output_frames_max();
        let channels = plugin.num_channels;
        let mut block = vec![0.0; capacity.max(1) * channels];
        let mut output = Vec::new();
        for _ in 0..4096 {
            let result = plugin
                .drain(&mut block, &ProcessContext::new(rate, 0))
                .unwrap();
            output.extend_from_slice(&block[..result.frames * channels]);
            if result.complete {
                return output;
            }
        }
        panic!("white-box drain exceeded the test's bounded call allowance");
    }

    fn assert_coeffs_eq(actual: &BiquadCoefficients, expected: &BiquadCoefficients) {
        assert_eq!(
            (actual.b0, actual.b1, actual.b2, actual.a1, actual.a2),
            (
                expected.b0,
                expected.b1,
                expected.b2,
                expected.a1,
                expected.a2
            )
        );
    }

    #[test]
    fn piecewise_retune_output_matches_stateful_oracle() {
        // D7(b)-honest: cutoffs are structural (runtime `set_parameter` is
        // refused), so the retune path is exercised white-box exactly as the
        // rebuild arms do (fields + rebuild): 64 frames at 500/8000, retune
        // to the reviewer's 200/2000 target, 64 more, drain. Bitwise
        // piecewise oracle, rebuilt coefficients equal fresh construction,
        // retune provably changes output, state provably carries, flush
        // engages, remainder below peak·2^-24.
        const CHANNELS: usize = 2;
        const RATE: u32 = 48_000;
        const RATE_F64: f64 = 48_000.0;
        let mut plugin = ABComparePlugin::new(CHANNELS).unwrap();
        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.initialize(RATE).unwrap();
        assert!(plugin.band_mask_active());

        let input = wb_dense(128, CHANNELS);
        let mut oracle = CascadeOracle::new(CHANNELS, 500.0, 8_000.0, RATE_F64);
        let out1 = wb_process(&mut plugin, &input[..64 * CHANNELS], RATE);
        let exp1 = oracle.feed(&input[..64 * CHANNELS]);
        assert_eq!(out1, exp1, "pre-retune segment bit-exact");

        plugin.band_mask_low_hz = 200.0;
        plugin.band_mask_high_hz = 2_000.0;
        plugin.rebuild_band_mask_filters();
        oracle.retune(200.0, 2_000.0, RATE_F64);
        // Rebuilt coefficients equal fresh construction (closes the
        // shared-mode gap between `update_params` and `Biquad::new`).
        let q = 1.0 / std::f64::consts::SQRT_2;
        assert_coeffs_eq(
            &plugin.band_mask_hp[0].coefficients(),
            &Biquad::new(BiquadFilterType::Highpass, 200.0, RATE_F64, q, 0.0).coefficients(),
        );
        assert_coeffs_eq(
            &plugin.band_mask_lp[0].coefficients(),
            &Biquad::new(BiquadFilterType::Lowpass, 2_000.0, RATE_F64, q, 0.0).coefficients(),
        );

        let out2 = wb_process(&mut plugin, &input[64 * CHANNELS..], RATE);
        let exp2 = oracle.feed(&input[64 * CHANNELS..]);
        assert_eq!(out2, exp2, "post-retune segment bit-exact");
        // Carryover proof: carried state differs from a fresh cascade.
        let fresh2 =
            CascadeOracle::new(CHANNELS, 200.0, 2_000.0, RATE_F64).feed(&input[64 * CHANNELS..]);
        assert_ne!(out2, fresh2, "retune must carry filter state");
        // Retune proof: output differs from the no-retune cascade.
        let no_retune = CascadeOracle::new(CHANNELS, 500.0, 8_000.0, RATE_F64).feed(&input);
        let mut whole_process = out1.clone();
        whole_process.extend_from_slice(&out2);
        assert_ne!(whole_process, no_retune, "retune must change output");

        let tail = wb_drain(&mut plugin, RATE);
        let flush_frames = tail.len() / CHANNELS;
        assert!(flush_frames > 0, "retuned mask flush must engage");
        let mut whole = whole_process;
        whole.extend_from_slice(&tail);
        let mut expected = exp1;
        expected.extend_from_slice(&exp2);
        let flush_zeros = vec![0.0; flush_frames * CHANNELS];
        expected.extend_from_slice(&oracle.feed(&flush_zeros));
        assert_eq!(whole, expected, "retuned whole stream bit-exact");

        let wet_peak = input
            .iter()
            .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
        let threshold = wet_peak * ABComparePlugin::MASK_RESIDUAL_RATIO;
        let tail_max = oracle.tail_max(20_000);
        assert!(
            tail_max < threshold,
            "remainder {tail_max} must stay below {threshold}"
        );
        println!(
            "white-box retune: flush {flush_frames} frames, remainder {tail_max:.3e} below {threshold:.3e}"
        );
    }

    #[test]
    fn deactivate_reactivate_output_matches_piecewise_oracle() {
        // D4-reactivation-honest: the mixer never steps inactive filters, so
        // deactivation freezes state (passthrough output) and reactivation
        // resumes from it. 48 frames at 500/8000, 48 inactive (bitwise
        // passthrough), 32 reactivated (differs from fresh), drain. Bitwise
        // piecewise oracle, flush engages, remainder below peak·2^-24.
        const CHANNELS: usize = 2;
        const RATE: u32 = 48_000;
        const RATE_F64: f64 = 48_000.0;
        let mut plugin = ABComparePlugin::new(CHANNELS).unwrap();
        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.initialize(RATE).unwrap();

        let input = wb_dense(128, CHANNELS);
        let mut oracle = CascadeOracle::new(CHANNELS, 500.0, 8_000.0, RATE_F64);
        let out1 = wb_process(&mut plugin, &input[..48 * CHANNELS], RATE);
        let exp1 = oracle.feed(&input[..48 * CHANNELS]);
        assert_eq!(out1, exp1, "active segment bit-exact");
        assert_ne!(out1, input[..48 * CHANNELS], "active mask must filter");

        plugin.band_mask_low_hz = 20.0;
        plugin.band_mask_high_hz = 20_000.0;
        plugin.rebuild_band_mask_filters();
        assert!(!plugin.band_mask_active());
        let out2 = wb_process(&mut plugin, &input[48 * CHANNELS..96 * CHANNELS], RATE);
        assert_eq!(
            out2,
            input[48 * CHANNELS..96 * CHANNELS],
            "inactive mask must pass through bitwise"
        );

        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.rebuild_band_mask_filters();
        oracle.retune(500.0, 8_000.0, RATE_F64);
        assert!(plugin.band_mask_active());
        let out3 = wb_process(&mut plugin, &input[96 * CHANNELS..], RATE);
        let exp3 = oracle.feed(&input[96 * CHANNELS..]);
        assert_eq!(out3, exp3, "reactivated segment bit-exact");
        let fresh3 =
            CascadeOracle::new(CHANNELS, 500.0, 8_000.0, RATE_F64).feed(&input[96 * CHANNELS..]);
        assert_ne!(out3, fresh3, "reactivation must resume frozen state");

        let tail = wb_drain(&mut plugin, RATE);
        let flush_frames = tail.len() / CHANNELS;
        assert!(flush_frames > 0, "reactivated mask flush must engage");
        let mut whole = out1;
        whole.extend_from_slice(&out2);
        whole.extend_from_slice(&out3);
        whole.extend_from_slice(&tail);
        let mut expected = exp1;
        expected.extend_from_slice(&input[48 * CHANNELS..96 * CHANNELS]);
        expected.extend_from_slice(&exp3);
        let flush_zeros = vec![0.0; flush_frames * CHANNELS];
        expected.extend_from_slice(&oracle.feed(&flush_zeros));
        assert_eq!(whole, expected, "reactivated whole stream bit-exact");

        let wet_peak = input
            .iter()
            .enumerate()
            .filter(|(index, _)| *index < 48 * CHANNELS || *index >= 96 * CHANNELS)
            .fold(0.0_f64, |max, (_, sample)| max.max(f64::from(sample.abs())));
        let threshold = wet_peak * ABComparePlugin::MASK_RESIDUAL_RATIO;
        let tail_max = oracle.tail_max(20_000);
        assert!(
            tail_max < threshold,
            "remainder {tail_max} must stay below {threshold}"
        );
        println!(
            "white-box reactivate: flush {flush_frames} frames, remainder {tail_max:.3e} below {threshold:.3e}"
        );
    }

    #[test]
    fn deactivated_before_drain_emits_only_child_ring_tails() {
        // D4-honest: the reviewer's exact counterexample (active program, then
        // deactivation leaving frozen nonzero histories under full-range
        // coefficients) derives zero mask flush, so drain emits only real
        // child/ring tails. Twin plugins: identical process parts, active
        // twin drains the proven flush, deactivated twin completes empty.
        const CHANNELS: usize = 2;
        const RATE: u32 = 48_000;
        let input = wb_dense(128, CHANNELS);
        let mut active = ABComparePlugin::new(CHANNELS).unwrap();
        active.band_mask_low_hz = 500.0;
        active.band_mask_high_hz = 8_000.0;
        active.initialize(RATE).unwrap();
        let mut deactivated = ABComparePlugin::new(CHANNELS).unwrap();
        deactivated.band_mask_low_hz = 500.0;
        deactivated.band_mask_high_hz = 8_000.0;
        deactivated.initialize(RATE).unwrap();

        let process_active = wb_process(&mut active, &input, RATE);
        let process_deactivated = wb_process(&mut deactivated, &input, RATE);
        assert_eq!(
            process_active, process_deactivated,
            "twins must render identical process parts"
        );

        let tail_active = wb_drain(&mut active, RATE);
        assert!(
            !tail_active.is_empty(),
            "active twin must drain the proven flush"
        );

        deactivated.band_mask_low_hz = 20.0;
        deactivated.band_mask_high_hz = 20_000.0;
        deactivated.rebuild_band_mask_filters();
        assert_eq!(
            deactivated.mask_flush_frames_required().unwrap(),
            0,
            "inactive derivation must be zero despite frozen histories"
        );
        let tail_deactivated = wb_drain(&mut deactivated, RATE);
        assert!(
            tail_deactivated.is_empty(),
            "deactivated twin emits only child/ring tails (none here)"
        );
        println!(
            "white-box deactivate-at-drain: active flush {} frames, deactivated 0",
            tail_active.len() / CHANNELS
        );
    }
}
