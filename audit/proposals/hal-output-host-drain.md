# AUD138: Host-level HAL Output handoff proposal

Status: Astra accepted the direct-plugin/EOF design at
`bedb455fd4ba7abf835d10b537d3a1e93da0e44612a2864a830a0d56f4b3bf9a` and the
ordinary-input amendment at
`a85b2ee6cd86e4d98d5edbf1efb0ee5a397b433c1c69c3f0d1f5fdcc59cc33dd`. The
bounded `DawHost` implementation is in progress and awaits implementation
review. This scope does not enable HAL Output in engine playback or claim
physical playback.

## Current source and red evidence

The public `DawHost` can assemble a serial chain ending in HAL Output using the
portable fake-writer seam. Its ordinary `process` route is not valid for that
chain today:

- HAL declares two input channels and zero output channels
  (`sotf-plugin-hal-output/src/lib.rs`, `Plugin::output_channels`). Its process
  method returns consumed input frames after writing or retaining those
  samples.
- `DawHost::process` eventually calls `terminal_output_frames`, which divides
  `NodeBuffer.actual_len` by `num_channels` at
  `sotf-host/src/host/daw_host.rs:3760-3767`. For this sink, the divisor is
  zero. The panic can happen after HAL has accepted a prefix.
- `DawHost::drain` first calls `is_topologically_linear_chain` at
  `daw_host.rs:2030-2048`. That helper also requires every node's input and
  output channel counts to match at lines 864-883, so it rejects the terminal
  sink before reaching its separate host-output-zero guard at lines 2050-2054
  or per-node zero-channel guard at lines 2092-2095. If admitted, the current
  tail route drains an upstream node and passes its output to each downstream
  plugin's ordinary `process` method at lines 2130-2170. It returns the final
  plugin's frame count at lines 2178-2195; for HAL this is consumed input, not
  emitted output.
- The equal-channel topology helper also gates
  `can_process_f32_linear_chain` at lines 854-862. Do not weaken that shared
  predicate: AUD140 separately covers channel-changing serial drain
  eligibility. The sink route needs its own narrowly checked topology.
- `Plugin::reset` defaults to a no-op (`sotf-host/src/plugin.rs:220`). The
  accepted direct HAL stage covers successful reinitialization cancellation,
  not generic host reset.
- The actual engine route remains unsupported. `EngineConfig` rejects zero
  output channels (`sotf-engine/src/types/config/engine_config.rs:168-170`),
  and `PreparedHostUpdate::prepare` rejects a zero-output host
  (`sotf-engine/src/engine/types.rs:148-156`). The daemon currently strips
  legacy HAL graph nodes. Do not remove these admission checks for this work.

### Executed pre-production host probes

The HAL crate's public-host regressions use `DawHost`, a finite four-frame
delay fixture, and the existing injected `FakeWriter`; no macOS hardware is
needed. The fixture is an independent exact delay/marker oracle, not the
production Delay crate or a claim about all DSP plugins.

`/tmp/sotf-aud138-host-drain-red-clean.log` (exit 101, 3 tests, 3 failed),
SHA-256 `324473ea7940ca16e83c9b91187572f0c3d272e375f68a802103d742bc5d9d67`:

1. `public_daw_host_sink_process_does_not_panic_after_handoff` caught the
   divide-by-zero panic. Before the panic, the fake writer accepted 4 of 24
   expected interleaved samples; the assertion verifies they equal the exact
   expected prefix. The finite-delay marker suffix is nonzero and absent from
   that accepted prefix.
2. `public_daw_host_sink_drain_preflights_tail_before_any_source_consumption`
   got the current error `end-of-stream drain currently requires a linear
   plugin graph`; it did not reach the desired capacity check. After removing
   the sink, `DawHost::drain` recovered all four source-tail frames exactly as
   `[0, 0, 0, 0, 0.25, -0.5, 0.75, -1.0]`. This proves the current topology
   rejection precedes source drain. It does not prove a sink-aware capacity
   preflight exists.
3. `public_daw_host_reset_does_not_replay_pre_reset_pending_samples` accepted
   the old stream's six-frame prefix, reset the host without transport calls,
   then accepted the two-frame old pending suffix before the new stream. The
   observed total was 32 samples versus 28 expected; the four extra samples
   were the pre-reset suffix. Both current `process` calls panic only after
   their writer calls, and the test catches those panics.

`/tmp/sotf-aud138-hal-reset-red.log` (exit 101, 1 test, 1 failed), SHA-256
`b82021b926313edaac51a75791d526bcbb4e716615781d75fd410c6728f4e7b0`:

- `hal_output_reset_cancels_pending_samples_and_counts_drops_without_transport_io`
  directly probes the HAL plugin. After a partial write, `Plugin::reset` left
  two frames queued and left `dropped_frames` at zero. The no-allocation and
  no-deallocation guard passed; the fake writer's write, fill, flush, reconnect,
  cipher-reload, and ready counts were unchanged.

These logs preserve current failures; no assertion is represented as passing
future sink behavior. The pre-edit source snapshot is
`/tmp/sotf-aud138-host-drain-preedit/hal-output.rs`, SHA-256
`4905e63a57c8c20a615dfd6cea5974440b04296ccb32da493a0df0a37a6bcb1c`, and
`daw_host.rs`, SHA-256
`3669edc4a69c5f7bfb894a3fdbd6dc8ca6242425b3dbaa8d9d69da31537bf40e`.
The current HAL file's prefix through `#[cfg(test)]` is byte-identical to the
captured source; this stage changed only its test module. The old exploratory
log `/tmp/sotf-aud138-host-drain-red.log` is not evidence for these boundaries.

## Proposed narrow host contract

Add explicit `DawHost` APIs for sink processing and sink EOF drain, rather than
making ordinary output-buffer processing infer a sink from a zero divisor.
Names below are proposals; preserve the field semantics if the names change.

```rust
pub struct SinkProcessResult {
    pub input_frames_consumed: usize,
    pub pending_sink_frames: usize,
}

pub struct SinkDrainResult {
    pub source_tail_frames_handed_to_sink: usize,
    pub complete: bool,
}

pub struct SinkQueueState {
    pub pending_frames: usize,
    pub free_prepared_frames: usize,
    pub capacity_frames: usize,
}

pub enum SinkTailPreflightError {
    Unavailable,
    InvalidGeometry,
    CapacityExceeded { required_frames: usize, free_frames: usize },
}

pub enum SinkPreflightError {
    PendingGraphMutation,
    GraphNotBuilt,
    InputExceedsCapacity { input_frames: usize, capacity_frames: usize },
    Sink(SinkTailPreflightError),
}

pub enum SinkFailure {
    ControlInvalidatedRoute,
    UpstreamProcess,
    InvalidProducedFrameCount,
    AppendContractViolation,
    ServiceContractViolation,
}

pub enum SinkGraphSettlementResult {
    ServiceOnly { pending_sink_frames: usize },
    Settled,
}

pub enum SinkGraphSettlementError {
    NotRunning,
    ResetRequired(SinkFailure),
    Graph(String),
}

pub enum SinkAppendFailure {
    ContractViolation,
}

pub trait TerminalSink: Send {
    fn queue_state(&self) -> SinkQueueState;
    fn preflight_append(
        &self,
        maximum_frames: usize,
        context: &ProcessContext,
    ) -> Result<(), SinkTailPreflightError>;
    fn append_preflighted(
        &mut self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Result<(), SinkAppendFailure>;
}

pub trait Plugin: Send {
    fn terminal_sink(&self) -> Option<&dyn TerminalSink> { None }
    fn terminal_sink_mut(&mut self) -> Option<&mut dyn TerminalSink> { None }
    // Existing plugin methods follow.
}

impl DawHost {
    pub fn settle_terminal_sink_graph_mutations(
        &mut self,
    ) -> Result<SinkGraphSettlementResult, SinkGraphSettlementError>;
    pub fn process_to_sink(
        &mut self,
        input: &[f32],
    ) -> Result<SinkProcessResult, SinkProcessError>;
    pub fn drain_to_sink(&mut self) -> Result<SinkDrainResult, SinkDrainError>;
}
```

The `terminal_sink` accessors are the explicit opt-in; `output_channels() == 0`
alone does not opt a plugin into this route. `queue_state` is a value snapshot
of already prepared, frame-aligned storage. It performs no allocation, writer
call, format adoption, or other state change. `preflight_append` is read-only:
it validates initialization/writer availability, cached rate/channel geometry,
queue invariants, configuration-change state, and whether the conservative
frame maximum fits in free prepared storage. It must not reconnect, clear a
configuration flag, prepare capacity, or adopt external state. A disconnected,
key-mismatched, or backpressured writer may still pass admission when the local
queue can retain the complete tail; completion then waits for control recovery
and transport progress.

`append_preflighted` may return an error type for defensive reporting, but it
has a stronger implementation contract: immediately after successful
preflight and with no intervening host/transport adoption, appending at most
the validated maximum is guaranteed to copy all aligned samples into existing
queue capacity, without writer calls, allocation, deallocation, or partial
success. The implementation has no normal failure path after this validation;
`ContractViolation` exists to make an invariant breach fail closed. It must not
read/adopt async state. The host holds exclusive mutable plugin access from
preflight through upstream processing and append; only shared writer status
may change asynchronously, and that status cannot alter the reserved local
queue.
If the append hook nevertheless reports a contract violation after the
producer advances, the host must treat its source tail as uncertain and enter
`ResetRequired`; it must never retry that source tail.

The new methods opt into a terminal sink explicitly. They require a prepared,
single serial chain ending in a plugin that advertises a sink contract; the
sink must have nonzero input channels, zero output channels, and consume the
upstream width. For this first host implementation, keep preceding nodes
channel-preserving with prepared, fixed-rate frame geometry, and allow at most
one active finite-tail producer immediately before the sink; earlier nodes
must report no native tail. This avoids composing a second stateful downstream
processor after source drain. Channel-changing serial nodes remain outside
this gate pending AUD140; arbitrary DAGs, branches, sidechains, mixed-rate
queues, and the generic unequal-rate queue/manager proposal remain out of
scope. Keep `can_process_f32_linear_chain` and its existing equal-channel
fast-path rule unchanged. For the upstream prefix's fixed frame-identity
requirement, reuse the accepted AUD137 capabilities: each participating
`Plugin` must advertise `guarantees_identity_frame_geometry()`, and
`DawHost::has_identity_frame_geometry()` must confirm the active nonbypassed
nodes also have equal negotiated input/output sample rates. Do not introduce a
second geometry flag.
The terminal sink opt-in and queue-admission hooks remain separate.

### Sink-mode ownership and command admission

Choose terminal-sink mode before exporting either asynchronous command
producer. In current source, `take_parameter_event_sender()` takes
`queues.parameter_event_tx`, and `take_graph_mutation_sender()` takes
`queues.graph_mutation_tx` (`sotf-host/src/host/daw_host.rs:1367-1406`). Those
`Option` slots are the host's ownership record: if either is already `None`,
the producer may be held by an external sender and sink-mode activation must
fail. Add `DawHost::enable_terminal_sink_mode(&mut self)` which succeeds only
while both producer slots are still owned by the host. Once enabled, both
`take_*_sender` methods must refuse extraction for this host's lifetime. This
does not revoke an existing sender and does not add a shared gate, generation
counter, or concurrency protocol; it rejects sink mode when exclusive host
ownership can no longer be established.

The published `topology_handle()` is an immutable snapshot and does not expose
graph mutation. Current `DawHost` exposes no mutable plugin accessor. The
remaining host-local commands require `&mut self`: graph queue/direct mutation,
bypass, immediate parameter changes, automation edits, and playback-position
changes. They can coexist with sink mode while `Running`, but every mutating
entry point must reject with a lifecycle error after the host transitions to
`SinkDraining`, `SinkComplete`, or `ResetRequired`. This includes the public
`queue_add_*`/`queue_remove_plugin`, graph edit and bypass APIs,
`set_plugin_parameter_immediate`, parameter queue APIs, automation setters and
clearers, and playback-position setters/resetters. The host's exclusive
`&mut self` borrow serializes these calls with drain entry; since sink mode
forbids exported sender handles, there is no concurrent producer racing the
boundary. `DawHost::reset` remains the one lifecycle operation allowed to
cancel/reset a frozen stream and reopen it. Do not require or test a command
enqueue race through cloned handles in sink mode; such handles cannot be
exported after activation.

### Amendment: ordinary input admission and backpressure

The original proposal only prevented new overflow drops while admitting an EOF
tail. The host ordinary route has the same risk: `HalOutputPlugin::process`
returns the full consumed-frame count even when a partial writer leaves fewer
prepared queue slots than the new input block needs. The host route must not
turn that behavior into a new whole-chain data loss. Existing callers that use
the plugin directly keep the current counted-overflow contract; this amendment
applies only to `DawHost::process_to_sink`.

Add a bounded `TerminalSink::service_pending` hook that returns the
post-service `SinkQueueState` and makes at most two writer attempts per call.
It changes only the sink's already prepared pending queue and cached transport
state; it never advances a producer, changes graph or parameter state,
allocates, adopts format/capacity, or discards queued samples. A recoverable
transport-availability wait is reported through the unchanged or reduced
pending-frame count. A queue-contract violation is an error and fails the host
closed. The host calls this hook at most once per `process_to_sink` or
`drain_to_sink` call.

The hook has the shape
`service_pending(&mut self, context: &ProcessContext) -> Result<SinkQueueState, SinkServiceFailure>`;
the only error is a sink contract violation. Ordinary unavailable or
backpressured transport is represented by the returned pending count, never
as an ambiguous input-processing error. Any `SinkServiceFailure` enters
`ResetRequired`, whether it occurs before or after input admission. Recoverable
transport waits remain successful results with pending frames.

For a nonempty input block, `process_to_sink` first validates the graph, input
geometry and samples, then reads `queue_state` and runs read-only sink
preflight for the *entire* input frame count. This must happen before it dequeues
parameter events or invokes any upstream plugin. If the block is larger than
the sink's total prepared capacity, it returns a clear capacity error without
advancing the graph; callers split it into smaller blocks. If the block fits
the total queue but not current free capacity because older samples remain
pending, the host runs one bounded `service_pending` phase and returns
`input_frames_consumed: 0` with the remaining pending-frame count. It does this
even if that phase empties the queue, so one host call never services the old
queue and writes the new block in the same call. The caller retries the exact
same input slice on a later scheduled call. No busy wait occurs, and no queued
parameter event or producer frame is consumed by this service-only result.

When the whole block fits free prepared capacity, the host processes the
upstream prefix and appends the complete final interleaved block with the
already reviewed nonfallible `append_preflighted` hook. It skips the sink's
ordinary `Plugin::process` callback on this explicit route: that callback can
adopt asynchronous transport state and partially write before queueing, so a
preceding capacity check would not reserve a stable append. Between preflight
and append, the host keeps exclusive mutable access to the plugin; external
writer/configuration changes cannot alter the local prepared queue. After
append succeeds, it may run one bounded service phase and returns the full
consumed-frame count plus the current pending count. A blocked or recoverably
unavailable transport therefore leaves the admitted input retained, and the
caller must not replay that block.

If upstream processing returns an error or a frame count other than the
validated identity-geometry count, upstream state may already have advanced;
the host enters `ResetRequired` and refuses further input/drain until reset.
If `append_preflighted` violates its contract after upstream processing, the
host also enters `ResetRequired` and never retries that block.

#### Exact admission, control, and error semantics

Use typed errors so a caller can distinguish a retryable preflight refusal
from an aborted stream:

```rust
pub enum SinkInputDisposition {
    NotAdmitted,
    Admitted { frames: usize },
    Indeterminate,
}

pub enum SinkProcessError {
    RetryablePreflight(SinkPreflightError),
    ResetRequired {
        cause: SinkFailure,
        input: SinkInputDisposition,
    },
}

pub enum SinkDrainError {
    RetryablePreflight(SinkPreflightError),
    ResetRequired(SinkFailure),
}
```

For `SinkProcessError`, `RetryablePreflight` leaves the host in `Running` and
proves that this call did not invoke an upstream producer or consume
parameter/automation events. Examples are a pending graph mutation, an
unsettled graph, an input larger than total prepared capacity, or invalid
prepared geometry. Do not apply this lifecycle guarantee to
`SinkDrainError::RetryablePreflight`: before first EOF entry, a graph/topology
refusal leaves the host in `Running` with boundary events untouched; after the
EOF boundary is applied and lifecycle moves to `SinkDraining`, capacity or
transport preflight may retry only in `SinkDraining`. The boundary parameter
snapshot stays applied exactly once and controls stay frozen; only explicit
reset returns to `Running`. If a block fits total capacity but not current
free capacity, service the old queue once and return
`Ok(SinkProcessResult { input_frames_consumed: 0, pending_sink_frames })`,
even if service empties the queue. The caller retries the exact same input
slice on a later scheduled call. No input, control event, or clock advances on
that service-only result. Oversized input returns a typed retryable capacity
error so the caller can split it.

Before validation, control dequeue, or sink service, check whether queued
graph mutations are pending. If so, return
`RetryablePreflight(PendingGraphMutation)` and leave the queue untouched; do
not call `drain_graph_mutations` as a side effect of a sink call. If no queue
is pending but the host plan is unbuilt, return `GraphNotBuilt`; sink callbacks
must not lazily build or allocate. A service-only call never consumes graph
edits, parameter events, automation cursors, plugin sample positions, or the
host's automation/playback frame position.

### Explicit graph settlement while Running

Keep graph-queue commands available while the sink lifecycle is `Running`.
Add a narrow control-thread method:

```rust
impl DawHost {
    pub fn settle_terminal_sink_graph_mutations(
        &mut self,
    ) -> Result<SinkGraphSettlementResult, SinkGraphSettlementError>;
}
```

It is valid only for an enabled terminal-sink host in `Running`, while both
command producers remain host-owned. If no graph mutations are pending and the
graph is already built, it only validates the current terminal-sink route and
returns `Settled`. If mutations are pending or a prior error left the graph
unbuilt, it first checks the sink's pending-frame count. When sink frames
remain, it makes one bounded `service_pending` call and returns
`ServiceOnly { pending_sink_frames }`, even if that call empties the sink
queue. It does not apply graph mutations in the same call as service. The
caller repeats settlement after transport progress. This preserves admitted
audio before a queued edit can rebuild or reinitialize the sink, and gives
external backpressure no finite completion guarantee.

Only when the sink pending queue is empty does settlement apply queued graph
mutations. It keeps `built` false, calls the private
`drain_graph_mutations()`, then calls `build()` and validates that the result
is still a supported terminal-sink route. It returns `Settled` only after all
those steps succeed. It does not
change the ordinary public `build()` semantics: `build()` recompiles the graph
already applied in memory and does not consume queued mutations. This
settlement method is a control-thread operation because applying
edits/building can allocate or initialize plugins. It does not consume input
or parameter events, advance source or automation clocks, or move the sink
lifecycle out of `Running`. A `SinkServiceFailure` from its service step is a
contract failure and returns `ResetRequired`; recoverable transport waits
return `ServiceOnly` with retained pending frames.

At source SHA
`9b7bccab1760b536cb609b83709c648c995872f29ae804a472d0a52d1033bed8`,
`build()` at `sotf-host/src/host/daw_host.rs:655` does not drain graph
mutations; `drain_graph_mutations()` at line 2293 is `pub(super)`;
`process_to_sink()` at line 2417 currently drains that queue and lazily calls
`build()`; and ordinary `process()` at line 2388 rejects an advertised sink
before its own graph-mutation drain at line 2391. The amendment must remove
those implicit control operations from `process_to_sink`. The explicit
settlement method is the supported route for queued edits in sink mode; do not
alter ordinary `build()` or ordinary process semantics.

Preserve the existing queue's partial-error behavior; this is not a
transactional batch. `drain_graph_mutations()` pops and applies one command at
a time. If a command fails, earlier successful commands stay applied, the
failing command has already been popped, and later commands remain queued;
the method returns that error and does not call `build()`. Keep `built` false
until a subsequent settlement completes a successful build. If `build()`
fails after the queue drains, the graph edits remain applied and `built`
remains false; the caller must repair the graph and call settlement again.
No audio is admitted on either error. `process_to_sink` and first-entry
`drain_to_sink` continue to refuse while mutations are pending or the graph is
unbuilt. This uses the existing host-owned queue and exclusive `&mut` access;
it adds no sender gate, generation counter, atomic batch, or general queue
protocol.

The public regression first admits a distinct marker block and leaves it
retained with a zero-write script. While `Running`, queue a harmless valid
graph edit that preserves the supported sink route, then submit a second
distinct block. The call returns `PendingGraphMutation`; verify no writer
attempt, source/parameter/automation clock advance, or change to the first
retained marker. Calling ordinary `build()` alone must leave that refusal in
place. Call `settle_terminal_sink_graph_mutations()` with the writer still
blocked; it returns `ServiceOnly`, leaves graph edits queued, and preserves
the first marker. Make the writer ready and call settlement again; that call
services the old marker and returns `ServiceOnly { pending_sink_frames: 0 }`
without applying graph edits. Call settlement once more to apply/build the
edit and get `Settled`, then resubmit the exact second block. The accepted
sample oracle must contain the first and second blocks exactly once, with no
duplicate from either refusal. A second test injects a failing queued mutation
between valid mutations: settlement reports the error, retains earlier
application, consumes the failing command, leaves the later suffix queued and
the graph unbuilt, and refuses audio until a later successful settlement.
Frozen `SinkDraining`, `SinkComplete`, and `ResetRequired` states reject this
control method; explicit reset is required before it can run again.

For nonempty input, validate the current graph and interleaved samples, then
preflight the entire block against free prepared sink capacity before
dequeueing parameters or invoking an upstream plugin. Keep parameter events
queued through all graph, geometry, size, and free-space refusals. Once a
block has enough free capacity, apply its parameter events and automation at
their normal block/sub-block boundaries. Do not assume such controls preserve
geometry: after each control update and before the next producer callback,
revalidate the active topology, every node's channel geometry and negotiated
rates, the AUD137 identity-frame capability, terminal-sink opt-in, and that
free prepared capacity still covers the whole not-yet-appended input block.
After the upstream prefix produces the final interleaved block, run the same
validation again immediately before `append_preflighted`. The ordinary sink
`Plugin::process` callback stays skipped, so asynchronous format/key/service
changes cannot adopt configuration or steal local queue capacity between
revalidation and append.

If a control update invalidates the route before any producer callback, enter
`ResetRequired` with `input: NotAdmitted`; the event has been consumed and is
not rolled back, so reset is required before a new programme. If a producer
callback or append may already have advanced or partially stored the block,
enter `ResetRequired` with `input: Indeterminate`. This abort result never
permits retrying that block. On successful complete append, advance the host
and producer audio clocks exactly once for the admitted frame count. Any
reset-required result poisons the current stream; the caller must not retry
input or drain on that host. After explicit reset, only a new programme may
start. `NotAdmitted` means the source plugin did not process that block; it is
accounting information, not permission to retry on the poisoned host.

An empty input call has a separate exact rule: validate the prepared route
and queue state; if graph mutations are pending, return the retryable graph
error and do not service. Otherwise, if pending frames are nonzero, call
`service_pending` exactly once; if the queue is empty, do not call the writer.
Return zero consumed with the current pending count. Empty input never
dequeues or applies controls and never advances source, automation, or host
playback clocks. A `SinkServiceFailure` during empty-input service still
enters `ResetRequired` with `input: NotAdmitted`.

`SinkServiceFailure` before input admission returns
`ResetRequired { input: NotAdmitted, .. }`. After a full append it returns
`ResetRequired { input: Admitted { frames: input_frames }, .. }`: the block
was queued exactly once, and the caller must not submit it again, even after
reset. If the producer/append contract leaves partial progress possible, use
`Indeterminate`; the caller must abort that programme rather than replay or
reconstruct its block. Recoverable writer, key, connection, or service waits
are not `SinkServiceFailure`; they return `Ok` with the full admitted count
and current pending count. The first EOF-boundary call still applies queued
boundary events once, then freezes controls as in the original proposal;
service-only EOF calls do not dequeue events or advance source clocks.

Extend `SinkProcessResult` with `pending_sink_frames` alongside
`input_frames_consumed`. For nonempty input, a zero consumed count means no
upstream or parameter-event work was performed and the caller must retry the
same slice later. A positive count means the block has been appended exactly
once; any nonzero pending count is retained sink work, not a request to replay
the block. Since this route bypasses `HalOutputPlugin::process`,
`append_preflighted` increments `requested_frames` exactly once by the full
frame count only after complete append. Service-only calls, empty input,
refusals, and zero-consumed retries do not increment it. `written_frames`
counts only frames accepted by the writer/ring. Host admission and transport
waits add no `dropped_frames`; preserve existing direct-plugin overflow/drop
behavior and explicit pending-cancellation accounting. On an indeterminate
contract failure, poison the programme and do not interpret partial telemetry
as a successful request.

#### Executed ordinary-input red regressions

`/tmp/sotf-aud138-hal-input-admission-red.log` (exit 101), SHA-256
`571a469283ec7c3190310647874e50949f83a6a022c953f82ef3f85462735f01`, runs
the two new public host tests against the pre-amendment implementation:

- `public_sink_input_refuses_over_capacity_before_advancing_source` used a
  four-frame block and a two-frame pending queue. The current route reported
  four frames consumed after one writer attempt. The exact source marker
  changed from an all-zero, no-input delay line to the four input marker
  samples, so the intended preflight-refusal assertion failed before claiming
  any later delivery outcome.
- `public_sink_input_waits_for_pending_capacity_then_retries_distinct_block_once`
  filled a four-frame queue with writer responses `[0, 0]`, then tried a
  distinct one-frame block `[9.0, -9.0]`. The current route reported one frame
  consumed; the source marker advanced from write cursor 0 to 1 and changed
  its first delayed frame to `[9.0, -9.0]`. The zero-consumed/retry assertion
  failed. The test includes the later exact-stream oracle for the amended
  implementation, but that oracle was unreachable in this red run.

This red run proves the current host route advances upstream under an
over-capacity or full-queue condition. It does not report the internal HAL
`dropped_frames` counter through a public downcast seam. Existing direct-plugin
overflow tests remain the evidence for that counter; the host fix must preserve
that behavior and prevent these new route-level losses.

#### Required ordinary-input amendment regressions

Keep these as public `DawHost` tests using distinct successive marker blocks
and the portable fake writer. Assert exact interleaved accepted samples, the
source marker/cursor, queued controls, host frame position, and sink telemetry
at each boundary:

- Convert both recorded ordinary-input reds to green. The over-capacity block
  returns `RetryablePreflight(InputExceedsCapacity)` with source/controls/
  clocks and request/drop telemetry unchanged. Split that same source into
  blocks that fit and prove their concatenated output is exact. For the full
  queue case, service the old queue and return `Ok(consumed = 0)` even when it
  empties; verify the distinct new marker did not reach the producer. Submit
  that same marker on the next call, then prove the final stream contains each
  old and new sample exactly once, with no skipped or replayed frame.
- Queue a parameter event, an automation event, and an automation boundary.
  Repeat both a graph/size refusal and a full-queue service-only call; event
  counts, automation cursor, node sample positions, and host automation/playback
  frame position stay fixed. Admit the exact block once and verify controls
  apply at their original split points exactly once. Queue a graph mutation
  separately and verify a sink call returns `PendingGraphMutation` without
  draining it or servicing the sink; settle with
  `settle_terminal_sink_graph_mutations()` while Running, then run the input
  case. If old sink frames remain, verify settlement returns service-only
  until they are handed off, then applies the edit on a later control call.
- Test empty input with an empty queue (zero writer attempts) and with retained
  pending frames (exactly one service phase, at most two writer attempts).
  Both return zero consumed; neither applies queued controls nor advances any
  source/automation/host clock. With pending graph mutation, empty input
  returns the graph preflight error and performs no service.
- Have an upstream test plugin switch the fake writer's connection/key/service
  availability after preflight but before append. The append still retains the
  full block because it does not adopt transport state; return full consumed
  count with pending frames. Restore transport on the control thread, service,
  and prove the exact block appears once. This is a recoverable wait, not a
  `SinkServiceFailure`.
- Inject a contract-only `SinkServiceFailure` before admission and after a
  complete append. Both poison the host and reject later input/drain until
  reset. Assert `NotAdmitted` before append and `Admitted { frames }` after it;
  the latter block is never resubmitted, including after reset. Inject producer
  partial advancement and append-contract violations separately; both return
  `Indeterminate`, fail closed, and never replay or duplicate the marker.
- Make a queued parameter/automation update invalidate channel count or
  negotiated geometry. Revalidation before a producer callback enters
  `ResetRequired` with `NotAdmitted`; confirm the event is not replayed and
  reset is required. Also cover a later sub-block update after earlier producer
  work, which must return `Indeterminate` and abort the programme.
- Have a producer callback invalidate its own reported frame geometry after
  advancing. The final pre-append revalidation must reject the append, enter
  `ResetRequired` with `Indeterminate`, and prevent a retry or duplicate tail.
- Split one admitted input across several automation sub-blocks and count all
  fake-writer attempts for the entire host call: no more than two. Assert
  `requested_frames` counts only complete appended input once, `written_frames`
  counts writer-accepted frames, and refusal/service-only retries add neither
  requests nor drops. Preserve direct-plugin overflow tests unchanged.
- Guard heap allocation and deallocation on admitted, blocked/service-only,
  empty, recovered, and terminal-success paths. Format or error construction
  may allocate on explicit error paths; state that limitation rather than
  claiming those paths are heap-free.

With the amendment, `process_to_sink` validates the complete input and sink
capacity before it dequeues parameter events or invokes upstream plugins. It
then appends the exact final interleaved block through the advertised sink's
prepared queue hook; it does not call the sink's ordinary `Plugin::process` or
`terminal_output_frames`. Its result reports both source input frames consumed
and retained sink frames. Ordinary `process` returns a clear preflight error
for a sink topology before any plugin side effect; callers choose
`process_to_sink` explicitly.

`drain_to_sink` follows a host-owned lifecycle and is one bounded scheduling
step; it never spins while transport availability is external:

1. On first entry, reject without dequeuing if graph mutations are pending;
   callers must settle graph edits with
   `settle_terminal_sink_graph_mutations()` while the stream is still in its
   running state. Then validate the static serial topology and sink opt-in before
   closing input/control admission. Capture and apply the already queued
   EOF-boundary parameter events exactly once, freeze later parameter and graph
   changes, and recompute the producer's drain bound and sink admission after
   those events. This records a stable EOF parameter snapshot even if a later
   capacity check needs a retry. Build and validate all formats, scratch
   capacities, producer bounds, and sink state before calling any native
   producer `begin_drain`.
2. If HAL already has pending frames, call its bounded sink service hook once
   and do not drain upstream in the same host call. Return `complete: false`,
   including when this service call empties the pending queue. The caller
   schedules a new call after transport progress. This keeps one host call at
   no more than the direct plugin's two writer attempts and ensures a blocked
   sink never causes a new tail callback.
3. If pending is empty, preflight the maximum next source-tail chunk against
   the sink's *free prepared pending capacity*. Add narrow optional `Plugin`
   hooks described above. If the tail maximum exceeds free capacity or the
   sink is unavailable/invalid, return a specific preflight error before
   producer `begin_drain`. A capacity refusal leaves producer state untouched;
   it may be retried after a control-thread capacity repair, with controls
   frozen, or explicitly cancelled with reset. After successful complete
   preflight, call `begin_drain` once, then one native `drain`, then append its
   exact output with `append_preflighted`. This avoids a fallible/side-effecting
   writer call after the producer mutates. The host does not feed EOF samples
   through ordinary sink `process`, whose writer can fail after the producer
   has drained.
4. Preserve the existing per-native-plugin `drain_call_bound`, including the
   4096-call fallback and its accounting of zero-output calls. Sink
   availability waiting is a separate external retry and must not relax
   unknown-plugin convergence limits. Every call services at most one native
   drain operation and at most one sink-service phase; no busy wait or
   unbounded loop is permitted.
5. When the upstream source is complete and the sink's retained queue is
   empty, return `complete: true`. The sink API reports source-tail frames
   separately from emitted output frames: a zero-output sink emits zero audio
   frames. Completion means all callback samples were either accepted into
   the driver ring or remain truthfully retained until a later completion; it
   does not mean the device physically played them. Never call `flush_audio`
   or count a discard as completion.

If a declared maximum tail chunk exceeds free sink pending capacity, return a
specific capacity error before native drain. Keep the producer state intact.
The host lifecycle remains frozen at the EOF boundary, so a regression must
not remove or replace the sink after refusal. Prove nonadvancement with a
test-owned producer that exposes `begin_drain_calls`, `drain_calls`, its exact
remaining marker samples, and any consumed cursor: after refusal, both call
counters and the cursor remain zero and the complete marker vector remains
unchanged. Then build a separate comparator host from the same deterministic
producer fixture with sufficient prepared sink capacity and prove it drains
and hands off exactly that marker vector. This preserves the refused host's
frozen lifecycle and does not reset away source evidence. If a later supported
control-thread capacity repair can preserve producer state in place, it may
add a same-host retry test, but it is not required for this acceptance case.
Do not enlarge the queue silently, allocate on the audio thread, split a
plugin's atomic drain output without a plugin contract, or count
ordinary-process overflow drops as repaired. Prior counted HAL overflow drops
remain lost and visible. If the amendment is accepted, the host route admits
ordinary blocks only when the prepared queue can retain them and refuses
oversized blocks or waits with `input_frames_consumed: 0`; it does not change
the direct-plugin overflow behavior or counters.

Before successful preflight, topology, sink-availability, geometry, and
capacity errors are transactional for producer audio: neither `begin_drain`
nor `drain` has run. Boundary parameter events, if any, are applied once as
specified above and are not replayed after a capacity refusal. After producer
`begin_drain` starts, any error from `begin_drain` or `drain`, invalid output
length, or failure of the append contract makes producer advancement
uncertain. Mark the host `ResetRequired`; reject every subsequent drain/input
call until explicit reset cancels the stream. Never retry a possibly partial
producer result or reappend it. Tests must inject a producer error after state
advancement and a sink append-contract error, then prove the next call fails
closed without duplicate samples; reset is the only path back to a new stream.

### Reset and control recovery

The host sink lifecycle is `Running -> SinkDraining -> SinkComplete`, with a
separate `ResetRequired` terminal state after uncertain producer advancement.
`process_to_sink` accepts input only in `Running`; a call after drain begins is
an error and cannot clear or restart native drain quotas. `drain_to_sink` in
`SinkComplete` returns the same completed status without calling any child
plugin. The host does not infer a new programme from another process callback.

The transition into `SinkDraining` is serialized with host-local graph and
parameter command admission by exclusive `&mut DawHost` access. Sink mode is
only available before either external sender is taken, and then prevents both
sender exports. Thus no cloned external sender can race drain entry. Keep
normal-host sender APIs and their queue protocol unchanged; this contract does
not authorize a new gate, generation counter, or broader queue redesign.

Before the first transition to `SinkDraining`, pending graph mutations cause a
transactional error and remain queued for the caller to settle while running.
At that boundary, already queued parameter events are applied exactly once;
the final producer bound and sink capacity are then recomputed. Host-local
parameter events queued afterward and all graph edits are rejected by the
host's command APIs while draining, complete, or reset-required. A capacity
refusal after the boundary leaves this parameter snapshot and lifecycle frozen;
it does not reapply controls or advance a producer. This is stricter than direct
`Plugin::drain`, which remains a point-in-time queue operation.

Implement HAL `Plugin::reset` as allocation-free cancellation of only the
plugin's currently pending samples: clear the retained queue without releasing
its allocation and add exactly those pending frames to `dropped_frames`.
Reset must not call writer methods, flush the ring, reconnect, reload cipher
state, toggle engine readiness, or claim ring-accepted data was physically
played. `DawHost::reset` resets all plugin state and drain bookkeeping, so the
next stream cannot replay the old pending suffix. Any control-thread format or
service recovery must preserve pending samples unless a separately explicit
reset/reinitialization policy succeeds; a failed format/capacity preflight
cannot clear them. Ring-accepted data follows the driver's own transport
lifecycle and is outside pending-only reset accounting.

Control-thread transport service may resume a retained queue only when it
preserves that queue on both success and failure. Failed format/capacity
preparation leaves pending samples and the frozen sink lifecycle intact. An
explicit successful reinitialization that cancels queue contents aborts the
current host stream and moves it to `ResetRequired`; the counted cancellation
does not permit replaying the producer tail. Only `DawHost::reset` resets every
producer and sink plugin, clears the uncertain tail/queue state, and returns
the host to `Running`. A new programme after `SinkComplete` likewise requires
explicit reset so terminal child state is not accidentally reused.

## Required public acceptance tests

Use fresh hosts and the portable fake writer, and keep the finite marker oracle
independent of accepted writer accounting:

- A ready-writer `process_to_sink` followed by `drain_to_sink` delivers the
  complete exact ordinary-plus-finite-tail vector. The terminal drain reports
  zero emitted frames and completes only after all expected samples are
  accepted by the fake writer.
- A scripted partial/zero writer sequence leaves pending samples retained.
  A blocked call returns immediately, attempts no more than two writes, does
  not advance the finite producer, and a later writable retry delivers every
  expected sample once.
- The small-capacity case declares a four-frame producer tail with only two
  free sink frames. It returns the capacity error before `begin_drain` or
  `drain`; assert test-owned counters are still zero and the exact source
  marker vector and cursor are unchanged. Keep this host frozen with its sink
  attached. A separately constructed comparator with the same deterministic
  tail and enough prepared capacity must hand off that exact vector. Do not
  reset/remove the sink or inflate fake capacity to inspect the refused source.
- Reset tests cover both direct HAL telemetry (exact pending drop count,
  pending empty, no allocation/deallocation or transport calls) and public
  `DawHost::reset` (no stale pre-reset marker appears after next-stream input;
  writer-accepted ring prefix and transport counters stay unchanged by reset).
- Add short-write, zero-write then recovery, wrapped pending queue, queue-full,
  unavailable-writer, format-mismatch, and failure-before-source-consumption
  cases. Preserve current accepted direct-plugin tests and ordinary processing
  controls. No macOS device or physical-playback assertion is required.
- Inject a native producer that mutates during `begin_drain` or `drain` and then
  returns an error, plus a deliberately broken sink append hook. Each case
  enters `ResetRequired`; repeated drain/input calls must fail without invoking
  the producer or accepting any duplicate tail. An explicit host reset must
  clear both plugin states before a new marker stream can run.
- Exercise lifecycle transitions: pending graph edits are rejected without
  being consumed before drain entry; queued boundary parameters apply once;
  later input, parameter events, and graph mutations are rejected while
  draining and after completion; only reset opens the host for another stream.
  Verify sink-mode activation fails after either sender has been extracted,
  succeeds before extraction, and blocks later sender extraction. Verify every
  host-local mutator rejects after drain entry. No enqueue race test is needed:
  external sender handles are excluded by the opt-in precondition, and direct
  mutations require the same exclusive `&mut DawHost` borrow as draining.

Until a separately reviewed engine scheduler and output-frame contract exist,
`EngineConfig`, `PreparedHostUpdate`, engine EOS polling, and daemon routing stay
unchanged. This proposal does not establish an application playback path.

## Implementation checkpoint (2026-09-29)

The current direct HAL and host sink implementation includes the accepted
ordinary-input admission contract and preserves AUD140's separate
channel-changing drain validator. `process_to_sink` preflights capacity before
control or producer work, reports zero-consumed service retries, and appends
through the prepared queue. Its source-plugin callback now propagates errors
and contains panics only in terminal-sink mode; uncertain post-admission
progress poisons the host. Ordinary host processing continues to use its
existing isolated passthrough fallback. HAL service while unavailable retains
the admitted queue and does not reconnect, reload keys, flush, or change
configuration. Reset counts pending-frame cancellation without transport I/O.

Portable verification so far:

- `/tmp/sotf-aud138-hal-lib-full4.log` (exit 0, 54 passed, 0 failed), SHA-256
  `6b1d0d589bdfbc15e5af8997b8331045f6d90619e8d5d494ed719a69b9285175`. This
  run covered source error, appended-sink and service contract failures,
  panic-after-process poison/reset recovery, service-only control/clock
  preservation, full-queue refusal/retry, graph settlement/partial repair,
  bounded writer calls, direct reset telemetry, and heap guards. Source hashes
  for this exact run were `plugin.rs=113ba4529a5bf1ea58bad10baae8b781855c96fcc8d3bfaf0d3a5f19d998f469`,
  `daw_host.rs=e6926e78698c7191108253b37fbcee9e6ed8fbf5eed164336ea67f41a178ee5d`,
  and HAL `lib.rs=6b6d26f2430b18b4bf3ca19ebcba7449752ee9aef44241077d17befdab51ba76`.
- `/tmp/sotf-aud138-hal-panic-focused1.log` (exit 0, 1 passed), SHA-256
  `ea81201e99bb3186002221f22139165f6a2e945aa8b5119e2a0b3bf2f1f4619a`.
  The callback panics after recording its side effect; the host returns
  `ResetRequired/Indeterminate`, refuses replay, then accepts the marker once
  after explicit reset.
- Initial strict Clippy `/tmp/sotf-aud138-hal-clippy1.log` exited 101,
  SHA-256 `98d0653e0d3b8a3dc1aead22aac4b5a7a23712300ded2730cbe3e9b7d3d41dca`.
  It found the fixed eager-error cleanup in `daw_host.rs`, HAL modulo checks
  later updated to `is_multiple_of`, and Native-owned channel-layout methods
  later feature-gated.
- Final default-feature strict Clippy `/tmp/sotf-aud138-hal-clippy-final2.log`
  passed with `-D warnings`, SHA-256
  `3aa8da58a245f32cc0209929be7835a7cbd913a30c864071ddc9e09e94b44b23`.
- Final HAL library suite `/tmp/sotf-aud138-hal-lib-final.log` passed all 54
  tests, SHA-256
  `3f6c5058781426a9e51231211663f40d685e32d061b8ab742bcee5e3a69e9e36`.
- Ordinary host fallback test
  `/tmp/sotf-aud138-host-ordinary-fallback2.log` passed 1 test (551 filtered),
  SHA-256 `f552c0ad98c0ccd6742ea458abe7afbe03f9d89653e15b97f7af2cd2238c63af`.
  This confirms ordinary host panic isolation still uses passthrough.
- The named current-source AUD140 test
  `/tmp/sotf-aud138-aud140-current-host.log` passed 13 tests; 3 manual capture
  tests were ignored. SHA-256
  `a226dcdc6c7c8e94d5cc2734927c14e0e79ec5cc4422209765775eb929463029`. It
  exercises the 64-to-16 EOF route and scratch/capacity refusal.
- Current-source facade command `/tmp/sotf-aud141-host-chain.log` passed
  `stereo_multiway_crossover_band_merge_chain_matches_complex_reference`
  (1 passed), SHA-256
  `d69e93a9db367e55f5705f91af981fb830b1e1b427ff18c5e1019a5c7681c7b2`; the
  selected manifest start/end hashes match at
  `8d457192508bcfd6717ed9b4e4bff3cb3298a1070398edbeddce6bfcaff24566`. This
  checks the ordinary stereo serial route and its complex-response oracle; it
  does not exercise the AUD140 64-to-16 channel-changing EOF route.

The current owned source hashes after that one-line cleanup are:
`plugin.rs=113ba4529a5bf1ea58bad10baae8b781855c96fcc8d3bfaf0d3a5f19d998f469`,
`daw_host.rs=1f2944f5a6c696e7cdc8e424cd7ecd31f12b14e98f806431a24d96c223dee9db`,
and HAL `lib.rs=8375555675072a59e8a46b7b37d0e26004f8f1f8580bf6067830f966a72ef2ea`.
The implementation is ready for Astra review. No macOS device, engine playback
route, or physical-playback behavior has been tested or claimed.

## AUD138 review correction proposal (2026-09-30)

The following corrections address the five review findings without changing
ordinary processing, engine/manager routing, or the accepted AUD140 drain
route. Public red regressions are being added before production edits.

### Preflight and typed lifecycle

`DawHost::drain_to_sink` should return a public typed `SinkDrainError`, re-exported
through the existing `host::daw_host::*` surface. An unbuilt graph returns
`GraphNotBuilt` before `build`, plugin initialization, parameter dequeue, or
producer/sink calls. A graph with queued mutations returns
`PendingGraphMutations` without consuming that queue. Only a prepared graph
with the existing terminal-sink contract may enter EOF.

Before the transition from `Running` to `Draining`, validate the complete
prepared route, including host input width to the first plugin, every adjacent
plugin output/input width, same-rate identity geometry, sink width, and tail
metadata. Every plugin before the final producer must report `TailLength::Finite(0)`;
the final producer must report `Finite(_)` or `Finite(0)`. `Unknown` and
`Infinite` are rejected before any `begin_drain` or `drain`. These checks are
read-only, so a rejection leaves input, plugin cursors, writer counters, queued
samples, parameter events, graph mutations, and the `Running` lifecycle
unchanged. The explicit invalid-capacity refusal occurs after EOF entry and
freezes the parameter snapshot/lifecycle in `Draining`; subsequent input and
configuration mutations remain rejected until reset.

### Post-reservation configuration changes

`process_to_sink` keeps its initial writer preflight before producer work. If
the writer configuration flag changes after that preflight while a producer
advances, the host must not call transport service, reconnect, reconfigure, or
make a second configuration-dependent refusal. It must retain the entire
preflighted output block in the already-reserved local pending queue and report
the block consumed once. Later control-thread service handles the configuration
change. A regression flips the flag from the producer callback and checks the
exact retained block and unchanged producer call count across service retries.

### Public control-thread transport recovery

Add an opt-in method to `TerminalSink` with the shape
`recover_transport(&mut self) -> Result<(), SinkTransportRecoveryFailure>`.
The default implementation returns `Unsupported`; it is not called by
`process_to_sink`, `drain_to_sink`, or ordinary audio callbacks. Add
`DawHost::recover_terminal_sink_transport() ->
Result<SinkQueueState, SinkTransportRecoveryError>` as an explicit
control-thread operation on the currently owned sink. It is available only
when sink mode is `Running` or `Draining` and the graph is already built and
valid. It does not call `build`, consume queued graph mutations, process audio,
service or dequeue the local pending queue, alter producer drain state, or
change the host lifecycle. The queue occupancy/capacity and cached host latency
must be unchanged on both success and failure. `Complete`, `ResetRequired`,
unbuilt, and sink-mode-disabled hosts are rejected without side effects.

HAL recovery must distinguish `Ready` from `KeyMismatch`, `Disconnected`, and
configuration/format failure; an `Ok` return from the existing broad
`service_transport` is insufficient because that method can finish in a
non-ready state. The new narrow hook may reconnect and reload transport state
on the control thread, but it may recover only at the exact sample rate,
channel count, and buffer-frame geometry already cached when the host was
built. It must verify geometry before flushing the writer ring, and verify it
again before priming/marking ready. A changed ring size returns
`NeedsReprepare` while preserving plugin pending samples, local cached queue
capacity, and host latency. This proposal does not claim a preserving public
reprepare path for a new device geometry; that remains a separately scoped
follow-up rather than silently accepting stale scratch or latency bounds.

The public recovery regression starts with a finite source block fully held in
the plugin's local pending queue while the writer is unavailable, begins EOF,
then restores the fake writer at the original geometry. Recovery must leave
the exact pending count and `Draining` phase untouched; subsequent bounded
drain calls deliver the exact retained suffix once and complete. Separate
growth/shrink cases must return `NeedsReprepare` before adopting new geometry,
keep the pending marker and cached latency unchanged, and recover successfully
after the fake transport returns to its original format. These assertions cover
the retained plugin queue only: the writer ring may have already accepted data
that cannot be claimed as physically played and may be flushed by recovery.

### Required failure evidence

Keep separate public cases for an unknown-tail plugin before the final producer,
an unknown-tail final producer, a two-to-wrong-width host entrance, and a
correct entrance with an invalid adjacent graph edge. For each case, verify
preflight rejects before producer or writer advancement and that a subsequent
valid ordinary block remains usable. Preserve the existing wrong-geometry
fixtures' actual source markers; do not test only error strings. The
post-reservation writer-flag regression must prove the full processed block is
retained. An unbuilt-host drain regression must show no implicit build or plugin
initialization. Capacity refusal must retain the established frozen-EOF
behavior. The public recovery test must exercise `DawHost`'s hook, not downcast
the owned sink or call `HalOutputPlugin::service_transport` directly.

## AUD138 preserving ring-size reprepare addendum (2026-09-30)

The exact-geometry recovery checkpoint is preserved at
`crates/sotf-plugins/target/audit-baselines/aud138-exact-geometry-recovery/source/`.
Its five-file SHA manifest is the pre-reprepare baseline. Keep
`recover_terminal_sink_transport()` exact-geometry semantics intact for the
ordinary recovery path; this addendum adds a distinct preserving operation for
ring-size-only changes and supersedes the earlier statement that no preserving
reprepare path is in scope.

### Scope and public control operation

Add `DawHost::reprepare_terminal_sink_transport()` for a built, validated,
single serial terminal-sink route in `Running` or `Draining`, with no queued
graph mutations. It is called explicitly from a control thread. It does not
call ordinary `build()`, `Plugin::initialize`, `reset`, or source `begin_drain`
again. It rejects rate/channel changes, unbuilt graphs, pending graph edits,
`Complete`, and `ResetRequired` without consuming audio or changing the
existing graph. Current graph/path and existing checked sample/addressability
limits determine the new staged storage; do not impose an artificial
`MAX_BLOCK_FRAMES` ceiling when the checked planner can represent a larger
valid sink ring.

Keep transport observation and commit separate. The plugin first returns a
value plan containing the actual `(sample_rate, channels, buffer_frames)`, a
freshness token/configuration generation, the prospective logical queue
capacity, and the prospective sink latency. The host accepts only unchanged
sample rate/channels, positive checked ring extents, and a plan computed from
the current pending count and built serial graph. HAL re-reads format,
connection, key, and configuration state immediately before commit and again
after priming; if the plan token or geometry changed, it returns a typed wait
or `NeedsReprepare` without adopting the plan. If the writer API lacks a
monotonic generation, require the existing config-changed flag to remain
asserted until commit, compare the full format at both checks, and never clear
the flag until final readiness is verified.

### Staged resource and commit protocol

All fallible host/sink capacity arithmetic and allocations happen before local
geometry/cache commit. Build a temporary prepared bundle for the prospective
sink capacity using the existing checked `prepared_sink_frames`, path-frame,
channel, and byte/sample arithmetic. The bundle contains both f32 and f64
`ProcessBuffers`: per-node `NodeBuffer`s, scratch input/output, merge/channel
map buffers, delay scratch, compensation delay state, plus the f64-chain,
alternate-chain, f64 input, and f64 output scratch vectors. Keep
`parallel_scratch` and parallel-result sizing on their existing
`MAX_BLOCK_FRAMES`/graph-stage rules, but include them in the prepared bundle
so capacity growth cannot trigger a hidden RT allocation. Also stage the
terminal input staging vector at `logical_queue_capacity_frames * input_channels`
and any compiled-plan/cache values that depend on the prepared geometry. This
is a buffer-only reprepare of the unchanged validated graph; it must not
reinitialize plugins or apply queued graph controls.

On the sink, use checked `try_reserve` for the pending deque, prefill silence,
and drain staging before touching logical queue size, physical capacity,
latency, writer-ready state, or sample contents. Pre-reserving may increase
private backing allocation but must leave queue contents and reported geometry
unchanged. If any arithmetic, host staging, or sink reserve fails, return a
typed preparation error with the old sink/host geometry, cached latency,
queue, source clocks, and EOF state intact.

Once both bundles are ready, quiesce the writer and flush the physical ring as
an explicit transport discontinuity; reload/verify the key and the exact
planned format; prime the new target-fill silence; then re-check final format,
connection, key, config flag, and plan freshness. Only after all checks pass,
commit the sink’s physical ring fields and logical queue bound, swap the staged
host buffers, and recompute/publish cached host latency. These final swaps and
cache writes are infallible. If transport verification or priming fails, do
not commit either local geometry/cache bundle, keep the old pending samples,
and leave the sink not Ready so subsequent process/drain preflight cannot
advance the source before a retry. A physical ring flush can discard samples
already handed to the device but not proven played; tests and reporting must
distinguish that from the local pending queue, which is preserved exactly.

For either growth or shrink, logical queue capacity in frames is
`max(new_ring_frames, pending_frames_at_plan)`, with checked conversion to
samples. Physical transport ring frames and logical local queue capacity are
separate quantities after a shrink with a larger pending backlog. This keeps
`pending + free == capacity` valid without dropping accepted samples. A
shrink-under-backlog therefore has zero free frames at commit; later bounded
service creates capacity as it removes the exact pending prefix. Subsequent
input is admitted only against the prepared logical queue and staged host
buffers.

### Required public regressions

Use the public `DawHost` sink route and the fake writer, not a concrete-plugin
downcast. Queue distinguishable nonzero programme frames while the writer
accepts none, then test:

1. Running with no pending data: grow the ring, reprepare, assert updated queue
   capacity and cached latency, then process ordinary audio without allocation.
2. Running with pending data: grow the ring, reprepare, and drain/service the
   exact held vector once with no gap, duplicate, or silence counted as
   programme. Repeat with shrink where pending frames exceed the new physical
   ring; assert the logical capacity remains at least pending, queue arithmetic
   stays valid, then deliver every frame exactly once over multiple writes.
3. Draining after a finite producer has entered its drain phase: change ring
   size while frames remain pending, prove source/plugin sample positions,
   completed-prefix/active-node/prepared/call quota, source-complete flag and
   `Draining` lifecycle are unchanged, and verify the complete tail vector and
   final `Complete` receipt without a repeated `begin_drain` or replayed tail.
4. Rate/channel change, invalid zero/overflow geometry, plan freshness race,
   unavailable key/format, preparation failure, unbuilt/pending-graph and
   terminal lifecycle refusals. For every precommit failure, compare exact
   queue markers, local queue and transport geometry, cached latency, source
   positions, and EOF state; restoring a supported geometry must permit exact
   delivery.
5. After successful growth and shrink, guard ordinary process and repeated
   drain/service for both allocations and deallocations. Check latency equals
   the actual sink latency change and host cached latency, not merely the
   plugin-reported value. Keep any silence written for target priming out of
   the programme waveform oracle.

No manager, engine queue, or general graph replacement changes are part of this
addendum. All valid same-rate/channel ring sizes that fit checked addressable
resource planning are in scope; a resource preparation failure is a typed,
non-consuming refusal, not a silent downgrade to the old capacity.
