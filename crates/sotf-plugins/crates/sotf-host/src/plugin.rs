use crate::parameters::{Parameter, ParameterId, ParameterValue};
use std::any::Any;
use std::sync::Arc;

pub(crate) mod bounded_in_place;
mod in_place_plugin;
mod in_place_plugin_adapter;
mod loop_range;
mod midi_event;
mod midi_message;
mod misc;
mod note_expression_event;
mod parameter_event;
mod plugin_info;
mod process_context;
#[cfg(test)]
mod tests;
mod time_signature;
mod transport_info;
mod types;

pub use in_place_plugin::*;
pub use in_place_plugin_adapter::*;
pub use loop_range::*;
pub use midi_event::*;
pub use midi_message::*;
pub use note_expression_event::*;
pub use parameter_event::*;
pub use plugin_info::*;
pub use process_context::*;
pub use time_signature::*;
pub use transport_info::*;
pub use types::*;

/// Bound on a plugin's response after its input becomes zero.
///
/// This counts output-rate frames from the last input sample, including emitted
/// delay and filter support once. It is independent of per-call drain capacity
/// and does not count down as silence is processed. Bounds cover retained state,
/// transitions and internal modulation with no future input, events or changes.
/// An unaudited bound must remain `Unknown`; it must not be treated as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailLength {
    /// A conservative finite bound in output-rate frames. Zero is memoryless.
    Finite(u64),
    /// Recursive or autonomous output without a proven finite termination bound.
    Infinite,
    /// The plugin has not established a response bound.
    Unknown,
}

/// Validate the common realtime block contract before any plugin state advances.
///
/// The host contract is deliberately strict: buffers contain exactly the
/// interleaved samples described by `context` and channel metadata, and input
/// samples must be finite. Rejected blocks leave output and plugin state
/// untouched. Individual plugins therefore do not need to rediscover malformed
/// buffers after partially updating filters, delay lines, or stochastic state.
pub fn validate_process_block_f32(
    input: &[f32],
    output: &[f32],
    context: &ProcessContext<'_>,
    input_channels: usize,
    output_channels: usize,
) -> PluginResult<()> {
    validate_process_block_lengths(
        input.len(),
        output.len(),
        context,
        input_channels,
        output_channels,
    )?;
    if let Some(index) = input.iter().position(|sample| !sample.is_finite()) {
        return Err(format!(
            "plugin input contains non-finite sample at interleaved index {index}"
        ));
    }
    Ok(())
}

/// f64 counterpart of [`validate_process_block_f32`].
pub fn validate_process_block_f64(
    input: &[f64],
    output: &[f64],
    context: &ProcessContext<'_>,
    input_channels: usize,
    output_channels: usize,
) -> PluginResult<()> {
    validate_process_block_lengths(
        input.len(),
        output.len(),
        context,
        input_channels,
        output_channels,
    )?;
    if let Some(index) = input.iter().position(|sample| !sample.is_finite()) {
        return Err(format!(
            "plugin input contains non-finite sample at interleaved index {index}"
        ));
    }
    Ok(())
}

fn validate_process_block_lengths(
    input_len: usize,
    output_len: usize,
    context: &ProcessContext<'_>,
    input_channels: usize,
    output_channels: usize,
) -> PluginResult<()> {
    if input_channels == 0 || output_channels == 0 {
        return Err(format!(
            "plugin channel counts must be non-zero, got input={input_channels} output={output_channels}"
        ));
    }
    let expected_input = context
        .num_frames
        .checked_mul(input_channels)
        .ok_or_else(|| "plugin input buffer length overflow".to_string())?;
    let expected_output = context
        .num_frames
        .checked_mul(output_channels)
        .ok_or_else(|| "plugin output buffer length overflow".to_string())?;
    if input_len != expected_input || output_len != expected_output {
        return Err(format!(
            "plugin expected input={expected_input} output={expected_output} samples for {} frames x {input_channels}/{output_channels} channels, got input={input_len} output={output_len}",
            context.num_frames
        ));
    }
    Ok(())
}

/// Result of one allocation-free end-of-stream drain step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginDrainResult {
    /// Output-rate frames written by this step.
    pub frames: usize,
    /// `true` when the plugin's declared drain policy has no remaining output
    /// for the finalized stream. A plugin may explicitly cut off residual
    /// recursive state for an `Unknown` response; completion does not make its
    /// [`TailLength`] finite.
    pub complete: bool,
}

impl PluginDrainResult {
    pub const COMPLETE: Self = Self {
        frames: 0,
        complete: true,
    };
}

/// Snapshot of a terminal sink's already prepared frame queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkQueueState {
    /// Frames retained locally and not yet accepted by the sink transport.
    pub pending_frames: usize,
    /// Frames that can be appended without allocating or invoking the writer.
    pub free_prepared_frames: usize,
    /// Total prepared queue capacity in frames.
    pub capacity_frames: usize,
}

/// The prepared transport geometry owned by a terminal sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkTransportFormat {
    /// Prepared sample rate in hertz.
    pub sample_rate: u32,
    /// Number of interleaved input channels.
    pub channels: usize,
    /// Prepared transport ring capacity in frames.
    pub buffer_frames: usize,
}

/// A control-thread plan for replacing a terminal sink's prepared ring size
/// without dropping its locally retained samples or resetting its host route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkTransportRepreparePlan {
    /// Ring geometry currently prepared by the sink.
    pub prepared_format: SinkTransportFormat,
    /// Ring geometry currently reported by the transport.
    pub target_format: SinkTransportFormat,
    /// Pending samples retained locally when this plan was observed.
    pub pending_frames: usize,
    /// Logical queue bound after resize. It can exceed the physical ring while
    /// the existing pending queue is larger than a requested shrink.
    pub queue_capacity_frames: usize,
    /// Sink latency after adopting the target ring size.
    pub latency_samples: usize,
    /// The writer's configuration-change flag was asserted for this plan.
    /// Sinks without a monotonic generation must keep it asserted through the
    /// final format/readiness check and clear it only at commit.
    pub configuration_changed: bool,
}

/// Why a control-thread sink recovery is waiting for the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkTransportWaitingReason {
    /// The transport is not currently connected.
    Disconnected,
    /// The transport has no usable encryption key.
    KeyMismatch,
    /// The transport format could not be read.
    FormatUnavailable,
    /// The transport is still reporting a configuration change.
    ConfigurationChanged,
}

/// Readiness returned by control-thread recovery of a terminal sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkTransportRecoveryStatus {
    /// The original prepared format is ready for bounded service again.
    Ready,
    /// Recovery completed without changing local queues, but service must wait.
    Waiting(SinkTransportWaitingReason),
}

/// Failure while recovering a terminal sink without rebuilding its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkTransportRecoveryError {
    /// The sink does not provide control-thread recovery.
    Unsupported,
    /// The active transport geometry differs from the geometry prepared by the host.
    NeedsReprepare {
        /// Geometry whose queue and latency the host prepared.
        expected: SinkTransportFormat,
        /// Current transport geometry, when it could be queried.
        actual: Option<SinkTransportFormat>,
    },
    /// The transport or local queue changed after a reprepare plan was observed.
    StalePlan,
    /// The transport operation failed without changing the host-owned queue.
    Transport(String),
    /// A staged sink or host buffer allocation failed before geometry commit.
    Preparation(String),
    /// The sink violated the promise to preserve its local queue and geometry.
    ContractViolation,
}

impl std::fmt::Display for SinkTransportRecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("terminal sink recovery is unsupported"),
            Self::NeedsReprepare { expected, actual } => {
                write!(
                    formatter,
                    "terminal sink format changed; expected {expected:?}, got {actual:?}"
                )
            }
            Self::StalePlan => formatter.write_str("terminal sink reprepare plan is stale"),
            Self::Transport(error) => write!(formatter, "terminal sink recovery failed: {error}"),
            Self::Preparation(error) => {
                write!(
                    formatter,
                    "terminal sink reprepare could not stage buffers: {error}"
                )
            }
            Self::ContractViolation => {
                formatter.write_str("terminal sink recovery changed prepared host-owned state")
            }
        }
    }
}

impl std::error::Error for SinkTransportRecoveryError {}

/// A transactional refusal to admit an upstream terminal-tail chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkTailPreflightError {
    /// The sink transport object is not available.
    Unavailable,
    /// The sink's current format or queue invariants do not match the request.
    InvalidGeometry,
    /// The largest atomic chunk does not fit in already prepared storage.
    CapacityExceeded {
        /// Maximum frames the producer may return in one drain call.
        required_frames: usize,
        /// Frames currently free in the prepared queue.
        free_frames: usize,
    },
}

/// Failure of the append operation after a successful sink preflight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkAppendFailure {
    /// The producer or sink violated the preflighted geometry/capacity contract.
    ContractViolation,
}

/// A terminal sink violated its bounded pending-service contract.
///
/// Temporary transport unavailability and backpressure are represented by a
/// successful queue snapshot with frames still pending. This error is reserved
/// for an invalid queue or writer contract and requires the host to stop and
/// recover the stream explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkServiceFailure {
    /// The sink could not preserve queue invariants while servicing pending data.
    ContractViolation,
}

/// Prepared, allocation-free handoff for a terminal sink.
///
/// `preflight_append` is read-only. After it succeeds, the host keeps exclusive
/// mutable access to the plugin while the producer advances and
/// `append_preflighted` runs. Implementations must accept every aligned block
/// up to the preflighted maximum without a writer call, allocation,
/// deallocation, or partial append.
pub trait TerminalSink: Send {
    /// Return the current prepared queue occupancy and capacity.
    fn queue_state(&self) -> SinkQueueState;

    /// Return the physical transport geometry prepared by this sink when it
    /// differs from the local logical queue bound.
    fn prepared_transport_format(&self) -> Option<SinkTransportFormat> {
        None
    }

    /// Check a conservative maximum chunk before an upstream producer mutates.
    fn preflight_append(
        &self,
        maximum_frames: usize,
        context: &ProcessContext,
    ) -> Result<(), SinkTailPreflightError>;

    /// Append a previously preflighted block to retained sink storage.
    fn append_preflighted(
        &mut self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Result<(), SinkAppendFailure>;

    /// Make at most two writer attempts for already retained data.
    ///
    /// This may change only the prepared pending queue and cached transport
    /// diagnostics. It must not advance a producer, allocate, adopt transport
    /// format/capacity, reconnect, or discard pending samples. Recoverable
    /// backpressure is reported in the returned queue state.
    fn service_pending(
        &mut self,
        context: &ProcessContext,
    ) -> Result<SinkQueueState, SinkServiceFailure>;

    /// Recover transport state on a control thread without rebuilding the host.
    ///
    /// Implementations must retain their local pending samples and prepared
    /// queue geometry. They must not silently adopt a different sample rate,
    /// channel count, or ring capacity. A transport ring may be flushed while
    /// reconnecting; this does not authorize dropping the sink's local queue.
    fn recover_transport(
        &mut self,
        _expected: SinkTransportFormat,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        Err(SinkTransportRecoveryError::Unsupported)
    }

    /// Observe a proposed ring-size-only reprepare on a control thread.
    ///
    /// The returned plan is a value snapshot. The host stages all fallible
    /// buffers before passing it to [`Self::reprepare_transport`], and the sink
    /// must reject a stale plan without changing its local queue geometry.
    fn reprepare_plan(&self) -> Result<SinkTransportRepreparePlan, SinkTransportRecoveryError> {
        Err(SinkTransportRecoveryError::Unsupported)
    }

    /// Adopt a previously observed ring-size plan after staging its resources.
    ///
    /// Implementations preserve every locally pending sample on success and
    /// failure. A transport ring may be flushed as part of the discontinuity;
    /// the host-owned queue and its geometry are committed only after the
    /// target format and readiness are revalidated.
    fn reprepare_transport(
        &mut self,
        _plan: SinkTransportRepreparePlan,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        Err(SinkTransportRecoveryError::Unsupported)
    }
}

/// Core plugin trait
///
/// Plugins process audio samples in an interleaved format where samples are
/// organized as [L0, R0, L1, R1, ...] for stereo, or more generally
/// [C0_F0, C1_F0, C2_F0, ..., C0_F1, C1_F1, C2_F1, ...] for multi-channel.
///
/// Each plugin can process N input channels and produce P output channels,
/// allowing for flexible channel configuration (e.g., stereo to mono,
/// mono to stereo, surround processing, etc.).
pub trait Plugin: Send {
    /// Optional prepared handoff contract for a zero-output terminal sink.
    fn terminal_sink(&self) -> Option<&dyn TerminalSink> {
        None
    }

    /// Mutable access to an opted-in terminal sink's prepared handoff contract.
    fn terminal_sink_mut(&mut self) -> Option<&mut dyn TerminalSink> {
        None
    }

    /// Optional typed access for control-thread integrations that need to
    /// discover a concrete plugin wrapper behind `Box<dyn Plugin>`.
    fn as_any(&self) -> Option<&dyn Any> {
        None
    }

    /// Optional mutable typed access for control-thread integrations.
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        None
    }

    /// Get plugin information
    fn info(&self) -> PluginInfo;

    /// Get the number of input channels this plugin expects
    fn input_channels(&self) -> usize;

    /// Get the number of output channels this plugin produces
    fn output_channels(&self) -> usize;

    /// Get the list of parameters this plugin supports
    fn parameters(&self) -> Vec<Parameter>;

    /// Set a parameter value
    /// Returns an error if the parameter doesn't exist or the value is invalid
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()>;

    /// Helper to validate a parameter value against its definition
    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        let params = self.parameters();
        if let Some(param) = params.iter().find(|p| p.id == *id) {
            param.validate(value).map_err(|e| format!("{}: {}", id, e))
        } else {
            Err(format!("Unknown parameter: {}", id))
        }
    }

    /// Get a parameter value
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue>;

    /// Optional opaque native state for out-of-process persistence.
    fn save_opaque_state(&self) -> PluginResult<Vec<u8>> {
        Err("plugin does not expose opaque state".to_string())
    }

    /// Restore opaque native state on the control thread.
    fn load_opaque_state(&mut self, _state: &[u8]) -> PluginResult<()> {
        Err("plugin does not expose opaque state".to_string())
    }

    /// Initialize the plugin with the given sample rate
    /// This is called before any audio processing begins
    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        let _ = sample_rate;
        Ok(())
    }

    /// Reset the plugin state (e.g., clear buffers, reset filters)
    fn reset(&mut self) {
        // Default: no-op
    }

    /// Reset while reporting failures to a host that must keep transport and
    /// DSP state synchronized. Existing plugins inherit their normal reset
    /// behavior; native wrappers can override this to report lifecycle errors.
    fn reset_checked(&mut self) -> PluginResult<()> {
        self.reset();
        Ok(())
    }

    /// Process audio samples
    ///
    /// # Arguments
    /// * `input` - Interleaved input samples [C0_F0, C1_F0, ..., C0_F1, C1_F1, ...]
    ///   Length must be num_frames * input_channels()
    /// * `output` - Interleaved output samples (will be filled by plugin)
    ///   Length must be num_frames * output_channels()
    /// * `context` - Processing context (sample rate, frame count, etc.)
    ///
    /// Input samples must be finite. Implementations must reject malformed or
    /// non-finite blocks before changing output or internal DSP state. The
    /// standard adapters enforce this contract for their wrapped plugins.
    ///
    /// # Returns
    /// Ok(()) on success, Err(message) on failure
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String>;

    /// Maximum output-rate frames written by one [`Plugin::drain`] call.
    ///
    /// Stateful streaming plugins override this together with `drain`. The
    /// value is a capacity bound, not a promise that frames are immediately
    /// available.
    fn drain_output_frames_max(&self) -> usize {
        0
    }

    /// Stream-independent upper bound on one [`Plugin::drain`] call's emission.
    ///
    /// Every `drain` call in every stream state (fresh, mid-stream,
    /// draining, post-reset) emits at most the returned frames; in
    /// particular it covers the maximum over stream states of
    /// [`Plugin::drain_output_frames_max`]. `None` means unknown: the host
    /// keeps live per-call sizing with freeze-plus-refresh. A `Some` value
    /// must be pure, allocation-free, realtime-safe, and computed with
    /// checked arithmetic (overflow answers `None`: unknown is safe, a
    /// saturated guess is not). It may only depend on construction-time
    /// constants, never on stream state, so it survives reset and rebuild
    /// unchanged. This bounds capacity, not progress: pair it with
    /// [`Plugin::drain_call_bound`] for work and [`Plugin::tail_length`]
    /// for content.
    fn drain_frames_envelope(&self) -> Option<usize> {
        None
    }

    /// Prepare current tail metadata for an EOS preflight without starting the
    /// drain itself. The host calls this only after validating the destination
    /// capacity. Wrappers forward it to their inner plugin; asynchronous
    /// plugins may wait, with a bounded timeout, for already accepted input to
    /// finish so the following `tail_length()` query describes that state.
    /// This hook must not accept new input or change output geometry. A
    /// preflight error before drain mutation remains safe to retry.
    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        Ok(())
    }

    /// Refresh metadata that a native format restricts to its serialized
    /// plugin control thread. This is used by the isolated worker after
    /// construction and completed controls/process calls; it is not an audio
    /// callback hook and must not block there.
    fn refresh_control_thread_metadata(&mut self) {}

    /// Prepare bounded end-of-stream work before its call bound is queried.
    ///
    /// Hosts validate the output destination before calling this hook. It must
    /// be allocation-free, bounded, and idempotent until reset or new accepted
    /// input. It must not increase the preflighted output capacity or change
    /// channel layout or output sample rate. Any generated audio stays in
    /// prepared plugin storage for `drain`.
    /// Ordinary plugins need no preparation; wrappers may finish bounded input
    /// padding before querying their child's current-state drain bound.
    ///
    /// # Errors
    /// Validation failures must preserve state. A failure after DSP has advanced
    /// may require reset; implementations must reject retries in that state.
    fn begin_drain(&mut self, _context: &ProcessContext) -> PluginResult<()> {
        Ok(())
    }

    /// Bound successful full-capacity drain calls through the first complete result.
    ///
    /// Query after successful `begin_drain`. The bound includes zero-output
    /// progress and a terminal call, with no new input, reset, or accepted
    /// parameter change. The destination must hold `drain_output_frames_max()`
    /// frames. This is a work bound, coupled to `tail_length` only through
    /// the single-refresh rule below; a maximum output capacity alone does
    /// not prove minimum progress. Queries must not allocate, block, or
    /// adopt asynchronously prepared state.
    ///
    /// A `Some` bound normally covers the drain through its first complete
    /// result — except with deferred arming: when the live tail at grant
    /// time is not `Finite` (a mask or flush the drain derives late, once
    /// driving content exhausts), the bound may instead cover only the
    /// work through the arming transition, and the host grants ONE
    /// re-queried budget once the tail turns `Finite`. The re-queried
    /// budget must dominate true remaining calls (a fresh derivation from
    /// armed state, not a shrunk remainder); hosts never refresh twice,
    /// and any other exhaustion trips loudly as a defect. `None` leaves
    /// the work bound unknown;
    /// hosts enforce a finite fallback allowance under the same
    /// single-refresh rule. Empty/completed state may return a bound of one.
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        None
    }

    /// Advance end-of-stream state without accepting new programme samples.
    ///
    /// Implementations must be object-safe, allocation-free, transactional on
    /// destination-capacity errors, and eventually return `complete = true`.
    /// Hosts call this repeatedly and pass any returned frames through every
    /// downstream plugin before draining the downstream plugin's own tail.
    fn drain(
        &mut self,
        _output: &mut [f32],
        _context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        Ok(PluginDrainResult::COMPLETE)
    }

    /// Process a host-selected compiled operation.
    ///
    /// Returning `None` asks the host to use the regular `process` path. This
    /// keeps compiled plans opportunistic: the host can tag likely-specialized
    /// nodes while the concrete plugin decides whether its current state is
    /// eligible for the optimized operation.
    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        let _ = (op, input, output, context);
        None
    }

    /// Consume an analyzer tap without copying its bit-transparent input into
    /// a separate output buffer. The host uses this only for a non-terminal
    /// compiled analyzer node, where the same upstream buffer remains the
    /// downstream source. Returning `None` requests the ordinary compiled
    /// process path and must be side-effect-free. Once an implementation
    /// returns `Some`, it must have consumed the block exactly once and must
    /// not expect the host to call regular `process()` after an error. A
    /// successful call must be state-equivalent to one regular analyzer
    /// invocation and return exactly `context.num_frames`. Implementations
    /// must be allocation-free and bounded on the realtime thread; their
    /// regular output contract must remain bit-transparent.
    fn process_analyzer_tap_f32(
        &mut self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        let _ = (input, context);
        None
    }

    /// Stable scalar gain that a host compiled plan may fuse with adjacent ops.
    ///
    /// Return `Some(gain)` only when skipping `process()` for this block would
    /// preserve DSP state, for example when gain smoothing is already settled.
    fn compiled_static_gain(&self) -> Option<f32> {
        None
    }

    /// Parameter-sensitive compile/fusion metadata for this plugin state.
    fn compile_metadata(&self) -> types::PluginCompileMetadata {
        let mut metadata =
            types::PluginCompileMetadata::boundary(self.cost_class(), self.latency_samples());
        metadata.static_gain = self.compiled_static_gain();
        metadata
    }

    /// Process f64 audio samples.
    ///
    /// Plugins that need true double-precision processing should override this
    /// and return `supports_f64() == true`. The default bridges through f32 so
    /// callers have a stable API even for existing plugins.
    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let mut input_f32 = vec![0.0; input.len()];
        let mut output_f32 = vec![0.0; output.len()];
        for (dst, &src) in input_f32.iter_mut().zip(input.iter()) {
            *dst = src as f32;
        }
        let frames = self.process(&input_f32, &mut output_f32, context)?;
        for (dst, &src) in output.iter_mut().zip(output_f32.iter()) {
            *dst = src as f64;
        }
        Ok(frames)
    }

    /// Get the processing latency in output-rate frames (if any).
    /// The host converts these units when paths cross sample-rate boundaries.
    /// This is used to compensate for algorithmic delays
    fn latency_samples(&self) -> usize {
        0
    }

    /// Current zero-input response bound; see [`TailLength`].
    /// This scalar query must not allocate, block, or reset processing state.
    fn tail_length(&self) -> TailLength {
        TailLength::Unknown
    }

    /// State-independent zero-input response bound, if established.
    ///
    /// Upper bound, in output-rate frames, on the total future emission
    /// with no further input, taken over ALL reachable states (not just
    /// the current one like [`tail_length`](Self::tail_length)). Hosts
    /// compose it for in-flight content: a wave of `c` frames feeding
    /// this plugin emits at most `output_frames_for_input(c)` during
    /// arrival plus `tail_support` after, so unknown support forces an
    /// `Unknown` host tail wherever content is in flight downstream.
    /// Host support folds dominate TRUE future emission (every support
    /// term bounds truth in the current state too, since support covers
    /// all states) — never by comparison against live tail values, which
    /// support need not dominate. Memoryless plugins (zero in, zero out,
    /// in every state) return `Some(0)`; stream- or state-dependent tails
    /// whose maximum over states is unproven keep the `None` default. A
    /// plugin reporting [`TailLength::Infinite`] must report `None` here:
    /// a finite all-states emission bound contradicts unproven
    /// termination, and hosts treat Infinite nodes as unsupported. This
    /// scalar query must not allocate, block, or reset processing state.
    fn tail_support(&self) -> Option<u64> {
        None
    }

    /// Physical signal delay in concatenated emitted audio, measured in output-rate frames.
    ///
    /// Offline renderers trim this delay after concatenating the frames actually
    /// returned by processing. Include filter group delay and emitted startup
    /// zero padding. Exclude callback buffering that merely postpones when a
    /// plugin returns samples without adding samples to that emitted stream.
    ///
    /// The value must be finite and nonnegative. Fractional frames preserve
    /// a group-delay estimate through cascaded rate converters; the largest
    /// emitted impulse sample can lie at a neighboring integer frame and
    /// depends on resampling phase.
    ///
    /// The default matches [`Plugin::latency_samples`]. Variable-rate plugins
    /// whose realtime scheduling latency includes non-emitting chunk buffering
    /// override this value. Realtime scheduling and graph delay compensation
    /// continue to use `latency_samples`.
    fn signal_delay_samples(&self) -> f64 {
        self.latency_samples() as f64
    }

    /// Minimum input-rate queued-work horizon for worst-case realtime work.
    ///
    /// Plugins may accept smaller and irregular process partitions while
    /// accumulating work internally. A queued realtime scheduler must keep at
    /// least this much input-rate audio ahead of hardware consumption. This
    /// does not manufacture a longer physical callback deadline and is not a
    /// process-buffer shape requirement: every positive block size remains
    /// valid.
    ///
    /// Any input/output FIFO priming introduced to uphold this contract must
    /// be included in [`Plugin::latency_samples`] in the plugin's declared
    /// output-rate domain. The default scalar contract is one frame.
    fn realtime_quantum_frames(&self) -> usize {
        1
    }

    /// Coarse cost category for host scheduling. Override for FFT,
    /// convolution, dynamics, and other non-scalar DSP.
    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Scalar
    }

    /// Check if the plugin supports a specific channel configuration
    /// By default, this checks that input/output match expected values
    fn supports_channel_config(&self, input_channels: usize, output_channels: usize) -> bool {
        input_channels == self.input_channels() && output_channels == self.output_channels()
    }

    /// Get data from the plugin (if it's an analyzer or exposes internal state)
    /// Returns `None` by default for plugins that don't expose data.
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    /// RT diagnostics: returns (contention_count, update_count) from internal
    /// RealTimeCache, then resets counters. Default returns (0, 0).
    fn take_cache_contention_stats(&mut self) -> (u64, u64) {
        (0, 0)
    }

    /// Upper bound on the output frames produced for the given input frames.
    /// Default: returns input unchanged (no frame count change).
    /// Plugins that change frame count (like resamplers) should override this.
    /// Hosts size process destinations and fold in-flight waves through this,
    /// so it must dominate actual production in the queried state (a ceiling
    /// such as chunk-granular maxima, never a floor or an estimate). The
    /// declaration must also be non-decreasing in `input_frames` at fixed
    /// stream state: tail folds query it at over-approximate quanta, and
    /// only monotonicity lets that query dominate production at the
    /// smaller true wave. A non-monotone live declaration undercounts
    /// host tails silently (the fold cannot detect it), so audit every
    /// override — identities, chunk floors/ceilings, and linear maps
    /// qualify; state-shaped curves must be proven or left default.
    ///
    /// Arrival promise (R7-F2): hosts query this at fold time but arrivals
    /// occur later at evolved stream state. A publisher WITHOUT an envelope
    /// promises its live declaration dominates production at every reachable
    /// arrival state, not just the queried state: fixed-state monotonicity
    /// governs quanta, not states, so it never proves varying-state arrival
    /// (a query at carry 0 undercounts an arrival at carry 1). Stateful
    /// publishers whose live varies with stream state must publish
    /// [`Plugin::output_frames_envelope`] (residual/carry-maximized, as the
    /// production resampler envelope does) or keep live arrival-maximized;
    /// hosts size uncovered arrivals from live and cannot detect violation
    /// (silent undercount direction, as with non-monotone declarations).
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    /// Stream-independent upper bound on `process` production and on the
    /// live per-call declaration.
    ///
    /// For every valid `input_frames` and every reachable stream state,
    /// `process`/`process_f64` writes at most the returned frames (both
    /// precisions share frame geometry), and the live
    /// [`Plugin::output_frames_for_input`] declaration at `input_frames`
    /// is at most the returned value too: graph drains size destinations
    /// from live declarations against envelope-sized holdovers, so a
    /// loose live declaration over a tight envelope breaks healthy drains
    /// loudly at the holdover check. The bound must be non-decreasing
    /// in `input_frames`: hosts propagate envelopes through larger derived
    /// quanta, and monotonicity is what lets an envelope queried at a
    /// larger quantum dominate live production at a smaller one. The live
    /// declaration must itself be non-decreasing too (see
    /// [`Plugin::output_frames_for_input`]): tail folds query it at
    /// over-approximate quanta, which is sound only under monotonicity.
    /// `None` means unknown: the host keeps live per-call sizing under the
    /// arrival promise above (envelope-or-Unknown fallback: reservations
    /// prefer the envelope where known, live otherwise).
    /// A `Some` value must be
    /// pure, allocation-free, realtime-safe, and computed with checked
    /// arithmetic (overflow answers `None`). It may only depend on
    /// construction-time constants, never on stream state. Return
    /// `Some(input_frames)` only with proof that every success path writes
    /// at most `input_frames` (frame-preserving plugins qualify exactly;
    /// variable plugins should prefer their tighter proven bound).
    fn output_frames_envelope(&self, _input_frames: usize) -> Option<usize> {
        None
    }

    /// Guarantees identity frame geometry for every supported process block.
    ///
    /// Implementations may return `true` only when every valid input frame
    /// count is preserved by processing. The default is conservative because
    /// checking a few sample sizes cannot prove this property. Hosts that need
    /// identity geometry also verify each node's negotiated sample rate.
    fn guarantees_identity_frame_geometry(&self) -> bool {
        false
    }

    /// Returns the output sample rate given an input rate.
    /// Default: returns input unchanged (no rate change).
    /// Plugins that change sample rate (like resamplers) should override this.
    fn output_sample_rate(&self, input_rate: u32) -> u32 {
        input_rate
    }

    /// Returns the actual number of output frames from the last process() call.
    /// Default: returns None (unknown/not tracked).
    /// Plugins that produce variable output (like resamplers) should override this.
    fn last_output_frames(&self) -> Option<usize> {
        None
    }

    /// Returns the plugin's preferred oversampling factor, if any.
    /// When `Some(n)`, the host may insert oversampling before/after this plugin.
    /// `n` must be 2 or 4.
    fn preferred_oversampling(&self) -> Option<u32> {
        None
    }

    /// Whether this plugin can process in f64 precision.
    /// When true, the host may provide f64 buffers via a future `process_f64()` method.
    fn supports_f64(&self) -> bool {
        false
    }

    /// Reports immediate-only momentary controls.
    ///
    /// The host consults this between blocks on the engine control
    /// path for parameters marked `Structural`: a `true` return admits
    /// that id through `set_plugin_parameter_immediate` without
    /// rebuilding. Never consulted from the audio callback; automation
    /// and queued sample-accurate paths never consult it and keep
    /// rejecting all structural ids. The default is `false` for every id.
    fn supports_immediate_momentary_control(&self, id: &ParameterId) -> bool {
        let _ = id;
        false
    }
}
