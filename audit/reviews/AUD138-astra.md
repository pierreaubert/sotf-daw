# AUD138 Astra design review

Status: **bounded direct plugin and exclusive-owner serial host process/drain, recovery and preserving ring-size reprepare ACCEPTED. Engine/application admission and physical playback remain open.**

## Multi-chunk closure and bounded host verdict — 2026-09-30

**Accepted for the bounded host/recovery/reprepare scope.** The final test-only revision closes the sole r2 condition. HAL SHA `b81d7042ce4766cb8252c477539c0318a079f92e481f266929da3f147d027346`; selected source/lock manifest aggregate `a8e77d733f79609cdc3d73d268cdfe827add0a2b0872fdfa1c26030d0619f79f`. Production prefix and the three host sources remain unchanged from r2.

The new public fixture owns a six-frame finite tail emitted in three two-frame chunks. Before resize it asserts one begin, one drain, cursor two, remaining calls two, producer incomplete, and nonzero queued tail. Both growth and shrink preserve this state. Queue-only service calls do not advance the producer; the next source call advances to cursor four; completion occurs after exactly three source drains and one begin. Full accepted samples match an unresized twin and separately constructed delayed input. This now covers both active-source and the earlier source-complete/pending-sink cases.

Inspected terminal focused result: `/tmp/sotf-aud138-multichunk-reprepare-focused-r3.log`, 1 passed, SHA `bf9f83ec1a7aa0075c8cb554c8e20a47f81419df0ea98cc63151fca3b183c815`; strict HAL all-target Clippy `/tmp/sotf-aud138-multichunk-reprepare-clippy-r1.log`, SHA `8d34be7b97bb6470481527f39d69c55b6ccd89b7c883df0b68b511f1b7f2c71c`. The preceding full 71-test run remains historical; no full 72-test rerun is claimed or required for this focused test-only addition.

Earlier five host corrections are also closed within this bounded verdict: the current validator explicitly rejects Unknown tails and requires earlier producers to report finite zero tails; it validates host entrance and every adjacent width; the post-DSP path appends the reserved block without repeating asynchronous transport preflight; typed drain refuses unbuilt graphs without a lazy build; and public exact-geometry recovery plus preserving reprepare reaches the owned sink without removal/reset. The prior executed correction, failure/no-replay, ordinary fallback and AUD140 route evidence is retained in the report and archives. This acceptance does not enlarge the route beyond its declared identity-frame, same-rate, channel-preserving serial topology with one final finite-tail producer and an opted-in terminal sink.

No Rust edits or Cargo runs were made by this review. No whole-current-workspace, generic branch queue, consuming-engine scheduler, macOS transport, or physical playback acceptance is implied. Historical revision-required sections below describe superseded snapshots.

## Preserving reprepare r2 re-review — 2026-09-30

**Three findings closed; one narrow lifecycle evidence condition remains before scoped reprepare acceptance.** No new production defect was found in r2. Historical findings below are retained for provenance, not presented as all still open.

Reviewed HAL SHA `27d7a4a452b580b0250ccce2b1994bade1e7b7c64516e3371a847c004aa7d04f`; host API/implementation/reexports remain at the four-file r1 hashes below. Root's selected-source archive is `target/audit-artifacts/aud138-preserving-reprepare-r2/`, archive SHA `d96028a7e5fc577bd74921d05124d9c370117a3da16e341f3a37acceac6bc521`. This preserves selected current bytes, not a whole-transitive run-bound manifest. I inspected current source and terminal focused 9/9, HAL 71/71 and strict all-target HAL lint logs. No Cargo or Rust changes were made during review.

- **Finding 1 closed:** the priming operation is now a Result-returning closure, so its `?` reaches cleanup. The fake physically records three priming frames then reports an invalid count, exercising `write_frames`' error path; the test checks the partial fill was flushed, PrimingFailed, no Ready publication, exact retained queue/latency/source state, and successful public retry. This accurately exercises a write-wrapper error (the writer API returns a count), not a separately fallible OS-write API.
- **Finding 3 closed:** Nth-format-read mutations hit the host's second plan observation before quiescing; format/config/key/connection changes preserve local state and do not flush/reload/publish readiness. During-prime mutations exercise final verification. A thread-local test-only staging failure occurs after earlier staging succeeds. Each retries through the public host API and checks exact programme delivery. Invalid rate/channel/zero geometry, queued graph, unbuilt and Complete cases add refusal evidence. These tests are deterministic protocol checks, not real-OOM or physical-device concurrency proof.
- **Finding 4 closed:** the large case actually processes 8,193 frames and an 8,192-frame tail at 16 channels under allocation/deallocation guards with preallocated full-vector capture. Shrink 8→4 retains six pending frames and services backlog plus tail under the same guards. The existing HAL constructor rejects widths above 16, and this serial route preserves width; the documented 16-channel maximum is therefore a real admission limit, not a test-only restriction.
- **Finding 2 partially closed:** both growth/shrink now occur after source begin/drain, compare nonzero complete output with a no-resize twin, and show begin/drain called once. However, `assert_started_drain_reprepare_preserves_stream` explicitly asserts `source_before.drained` is true. Both cases exercise source-complete with sink-pending data. They do not exercise reprepare while the producer still owns an un-emitted suffix and the host active-node/call-quota state must resume it.

**Only remaining acceptance action:** add a test-owned finite producer that returns a nonempty partial tail with `complete=false` and later emits the rest. Reprepare with that partial tail pending, for growth and shrink; assert the producer is incomplete before reprepare, begin is not repeated, the remaining drain calls progress (not replay), and the full accepted vector equals an unresized twin and independent expected samples. Keep this test-only unless it reveals a defect. A focused executed test and affected strict lint are sufficient; no broad rerun is requested for this addition.

Direct-plugin acceptance remains intact. Engine/application sink admission, physical playback and broad manager queues remain outside this checkpoint.

## Preserving ring-size reprepare review — 2026-09-30

**Revision required for the new reprepare stage.** Direct-plugin acceptance remains intact; this does not reject the bounded design or reopen AUD140. No Rust edits or Cargo runs were made. Current source hashes were independently checked:

- `plugin.rs`: `9f704ca257ae03394bcd304098c8ad9e2ad5eebef44e931ffa464a637ae59fcd`
- `host/daw_host.rs`: `b51f44b6515ce3e93e833be6c45fb2d1a5152cb5ac6944b61eb320436261463a`
- host `lib.rs`: `d5a656f6f24aaf044857be05ba9ec843ae1adbe1914955afe4fc5a40e4fc959f`
- HAL `lib.rs`: `0a5d38de2df9131b95a08248be6c1a551ccf10cc0d0081fdacc7b54437f3e9db`

Root archived this source and receipts at `target/audit-artifacts/aud138-preserving-reprepare-r1/` under `crates/sotf-plugins`; source archive SHA `1bfd644dc649bb195716c2a90272aee63f223ac6ddbd48c07eb7a91aedb02514`. I inspected the terminal results: HAL 65 passed, host 551 passed/1 ignored, both strict all-target Clippy checks passed. Behavior runs precede the reported host style-only condition cleanup; final lint covers current source. These gates do not supply the missing scenarios below.

### Source assessment

The host stages f32/f64 node and scratch storage before calling the sink commit, copies retained node data/lengths, and moves compensation delay objects only after success. The sink stages a copy of the pending deque and uses `max(new physical ring frames, pending frames)` for logical capacity. Host cached latency is recomputed after the successful swap. This is a plausible preserving control-thread design, without source reinitialization or broad manager changes. I found no basis to claim that ring growth itself allocates inside the callback: the staging and input admission bounds must be tested at their actual extents.

### Required corrections and evidence

1. **P1 — actual error-path discrepancy in priming.** In HAL `reprepare_prepared_transport`, `let prime_result = { ... write_frames(...).map_err(...)?; ... };` is a plain block. A writer `Err` returns from the entire function, bypassing the following cleanup and `PrimingFailed` assignment. The current short-write test (`prime_write = Some(0)`) exercises only the other branch. Capture the complete priming operation in a Result-producing closure/helper, or handle the error explicitly, so write errors and short writes both discard partial priming and publish the intended non-ready failure state. Add an injected writer error after recording partial priming; prove no local queue/cached geometry/latency/source cursor change, no programme replay, and successful public retry. `quiesce_transport` already sets Servicing and engine-ready false, so this finding is not a claim that this path currently advances a producer after failure.

2. **P1 — required already-Draining transaction evidence is absent.** Current growth/shrink tests reprepare while Running, then begin draining. Add a public route that has actually called producer begin/drain and retains part of its tail at the sink before growth and shrink. Verify exact final accepted programme, no repeated begin or tail consumption, unchanged host lifecycle/cursor/source-complete state across reprepare, and final completion. Include the source-complete-but-pending case. Assertions on source observations and final audio should supplement private host-state checks, not be replaced by them.

3. **P1 — stale-plan/final-readiness rejection is untested.** Add deterministic fake-writer mutations between plan observation and commit, and during priming: changed full format, cleared config flag, key loss/disconnection, and a writer error. Prove old local geometry, logical queue contents, cached latency and stream state remain intact on refusal; no Ready publication; then use the public API to recover and deliver exactly once. Cover unsupported rate/channel and invalid geometry plus unbuilt/pending-graph/terminal lifecycle refusals through the new API. Allocation/preparation failure can use deterministic checked-overflow or a targeted test seam rather than inducing real OOM. These are missing proof of the proposed transaction, not a demonstrated broad race corruption.

4. **P2 — the large-capacity heap test does not reach enlarged storage.** Its 16-channel/16,384-frame ring processes one frame and drains a four-frame tail. Exercise an admitted block exceeding the old 8,192-frame bound and a matching large source output under both allocation and deallocation guards, with preallocated observations and a nonzero full-vector oracle. Repeat successful shrink/retained-backlog service. Include an admitted channel width above 32, or document and assert the actual route's narrower supported limit; no arbitrary restriction should be added merely to satisfy the test. The current one-frame case can remain a smoke test.

Update the report with this reprepare snapshot and the eventual closures; its current body still primarily documents the earlier exact-geometry checkpoint. Retain the earlier archive and clarify which source each log tested. Neither ring priming nor flushing proves physical playback; engine/application sink admission and macOS execution remain open.

## Host implementation review: revision required

### Control-recovery API design requirements (proposal refinement pending)

An explicit opt-in `TerminalSink::recover_transport` and control-thread
`DawHost::recover_terminal_sink_transport` is appropriately narrow. Restrict it
to built Running/Draining routes, preserving pending samples, drain cursor,
source-complete flag and lifecycle on success and recoverable failure. Default
Unsupported keeps generic plugins outside this capability.

Do not translate existing `HalOutputPlugin::service_transport() -> Ok(())`
directly into Ready: its final state may still be Disconnected or KeyMismatch.
Return readiness from actual final connection/key/configuration state. Also do
not delegate its format adoption blindly. `validate_transport_format` changes
pending capacity, target fill and reported device latency with buffer_frames.
Shrink can leave retained length above the new logical queue capacity; growth
changes the queue's advertised admission bound and host cached latency. Existing
host staging checks can refuse oversized input, so growth alone is not proof of
a callback allocation defect.

The simplest bounded policy is exact original rate/channels/ring size and
unchanged local queue capacity/latency. Control-thread reconnect/remap can occur,
but inspect the resulting format before adopting local geometry or priming.
On mismatch return an explicit reprepare-required result, retain pending and
old local geometry/lifecycle, and do not silently build/reset. Alternatively,
specify an equally concrete fixed admission cap and latency reconciliation;
general host replacement is outside this scope. Post-operation readiness must
not claim stability against future external changes; later service gates still
apply.

Required public evidence: disconnected owned sink → EOF → control recovery →
exact suffix delivery; key still unavailable is not Ready; growth and shrink
with queued samples preserve bytes and valid queue accounting on refusal,
leave cached latency unchanged, then recover/deliver after restoring supported
format. Failed reconnect/format/key preparation also preserves pending and drain
state. Explicit control recovery may flush the driver ring; neither that flush
nor priming silence counts as delivery/physical playback of programme samples.
No final design verdict on an as-yet-unfrozen revision is claimed here.

Reviewed the three frozen files under
`target/audit-baselines/aud138-implemented-final/source/` (beneath
`crates/sotf-plugins`), independently verifying manifest aggregate
`48cea9b36986950901d010d2d8df19ef1cdd7eecb1d77148674a746edac9f412`.
Host SHA is `1f2944f5a6c696e7cdc8e424cd7ecd31f12b14e98f806431a24d96c223dee9db`.
Direct-plugin acceptance remains intact; host-stage acceptance needs these fixes:

1. **P1 — enforce the bounded tail contract.**
   `validate_terminal_sink_tail_source` checks only earlier nodes' maximum drain
   chunk, not their `TailLength`, and never checks the final producer's tail
   metadata. An identity-frame recursive/Unknown plugin with default zero drain
   capacity can therefore be silently completed. Require no tail on earlier
   active nodes and finite/none on the final producer, as the approved scope
   specifies. Add Unknown-with-zero-capacity adversaries at both positions;
   reject before begin/drain, preserving subsequent ordinary audio/state.
2. **P1 — validate every adjacent width and the host entrance.**
   `validate_terminal_sink_graph` proves each upstream node preserves its own
   width and checks only the final source→sink width. It does not require the
   first node's input to match the host or each intermediate edge's widths to
   match. A serial 2→2 node followed by 4→4 and a 4-channel sink can pass these
   checks. Require exact host-input/first-node and every neighboring output/input
   equality before controls or DSP. Add public mismatched-entrance and interior
   fixtures with no producer/writer advancement and unchanged queued markers.
3. **P1 — retain admitted audio across a post-preflight configuration flag.**
   After upstream processing, `process_to_sink` repeats the full HAL preflight,
   which reads `writer.config_changed()`. A flag raised by the producer/shared
   transport now aborts with Indeterminate before storing the produced block,
   although its local prepared queue reservation is unchanged. Separate the
   post-control/local geometry-capacity revalidation from asynchronous transport
   admission checks. Once processing starts against valid prepared storage,
   append the complete block and let bounded service retain it until recovery.
   Add a config-flag flip after initial preflight, then exact later delivery,
   with full consumed count and no adoption/recovery I/O on the callback.
4. **P2 — finish the specified EOF API and unbuilt refusal.**
   `drain_to_sink` still returns `String` and calls `build()` on first entry if
   unbuilt. The accepted amendment specifies typed preflight/reset-required
   errors and explicit control-thread settlement; implicit build may allocate,
   initialize plugins and bypass that boundary. Return the declared typed drain
   error, refuse unbuilt/pending graphs without side effects, and test public
   settlement followed by retry. Preserve frozen SinkDraining state after an
   EOF capacity refusal; do not turn it back into Running.

5. **P1 — expose a reachable control recovery route for the owned sink.**
   Follow-up ownership tracing confirms `HalOutputPlugin::service_transport`
   needs mutable concrete-plugin access. DawHost exposes only immutable
   `get_plugin`, while `TerminalSink` exposes no control recovery hook. Removing
   the boxed sink is forbidden with pending audio and after EOF freeze. Once
   bounded drain observes Disconnected/KeyMismatch/ConfigurationChanged, later
   service returns early on that cached state even if external flags recover.
   Thus the public boxed-host route cannot perform the required recovery.
   Add a narrowly scoped explicit control-thread sink recovery hook/API that
   preserves pending on success/failure and preserves the EOF lifecycle, then
   prove public process/drain → unavailable → control recovery → exact suffix
   delivery without private downcasts/removal/reset. This must not become a
   general mutable-plugin accessor or callback reconnect path. Distinguish any
   explicit ring flush during control recovery from delivered/played samples.

Aligned portions: sender ownership gates prevent both exports in sink mode;
prepared append avoids ordinary overflow; service-only input preserves queued
controls and source clocks; source Err/panic and count mismatch propagate through
the sink-only path while ordinary fallback remains separate. Graph settlement
defers edits while retained frames remain and discloses partial application.
Build already scales node/scratch storage to prepared sink frame capacity, so
the provisional concern about blocks above 8192 frames alone is withdrawn.

The availability-flip fixture is meaningful: its real upstream callback changes
connection/key flags after processing, and assertions prove admitted input stays
queued with zero writer attempts and no recovery I/O. It is not merely initially
unavailable transport. It does not cover a configuration flag raised at that
point or subsequent actual recovery delivery; those remain in item 3's test.

Verified terminal evidence: HAL 54 tests passed; strict default HAL all-target
Clippy completed; ordinary fallback regression 1 passed; AUD140 regression
13 passed / 3 manual ignored. Heap tests cover successful cold/repeat/service/
reset paths, not formatted errors. No new broad workspace, real engine sink
admission or physical playback claim is supported. No reviewer Cargo or Rust
edits. These findings concern the new host sink route, not accepted AUD140.

## Ordinary-input admission amendment review

**Bounded amendment design ACCEPTED** at proposal SHA
`a85b2ee6cd86e4d98d5edbf1efb0ee5a397b433c1c69c3f0d1f5fdcc59cc33dd`.
The explicit Running-only `settle_terminal_sink_graph_mutations` closes the
public recovery gap. It services retained sink frames once and defers edits to
a later call, including when service empties the queue. Only an empty pending
queue permits queued mutations followed by build and route validation. Ordinary
`build()` semantics remain unchanged. Frozen EOF and reset-required states reject
settlement; a sink contract failure poisons the stream.

The proposal correctly discloses partial graph-error semantics: earlier commands
may remain applied, the failing command is consumed, later commands remain
queued, and audio stays refused until a successful repair/build. This is a
control-thread operation with allocation permitted, not a realtime transactional
graph replacement. Its explicit public refusal/service/settlement/resume oracle,
partial-error repair test, and frozen-state rejection are required implementation
gates. Preserve exact retained markers through the intermediate ordinary-build
negative control as well; that call alone cannot be credited as settlement.

Together with the typed consumption/error, empty-input, control revalidation,
clock and telemetry rules reviewed below, this is sufficient to proceed with
the bounded amendment. Host implementation acceptance remains pending executed
ordinary-plus-EOF waveform, error, control, capacity and heap evidence. Direct
plugin acceptance is unchanged. General engine/application admission, physical
playback and broad queue/protocol changes remain open and unauthorized here.
No Cargo or production edits for this design re-review.

### Historical final API finding (closed above)

Re-review of proposal SHA
`5e5e92f2b09e821beb6cdf0bdc5de3a8d7ffd4d233285a5201c1c2b212fdc766`:
the typed input disposition, contract-only service errors, empty-input policy,
control/sub-block revalidation, telemetry and clock rules resolve the prior
two contract findings. **One concrete public-API correction remains before
implementing the full amendment.**

The proposal says callers settle queued graph edits using public `build()`.
In inspected host SHA
`9b7bccab1760b536cb609b83709c648c995872f29ae804a472d0a52d1033bed8`,
`build()` does not drain graph mutations. `drain_graph_mutations()` is
`pub(super)`, and ordinary `process` rejects terminal-sink graphs before that
drain. Therefore the advertised recovery leaves the pending queue unchanged
and subsequent sink calls keep refusing. Specify a bounded explicit
control-thread settlement API while Running (apply queued edits, then build,
with existing nontransactional graph-error semantics disclosed), or reject
queued graph APIs in sink mode and require supported direct edits instead.
Do not change ordinary `build()` semantics implicitly. Add a public test that
actually resolves a queued-edit refusal and resumes input without replaying
previously admitted audio; frozen EOF states must reject settlement.

All other amendment mechanics are aligned for implementation. Keep the typed
process preflight guarantee scoped to `process_to_sink`: EOF preflight after
the first boundary remains frozen in SinkDraining with controls applied once,
not Running. This is a documentation distinction between the two error enums,
not a request to undo the accepted EOF lifecycle. No Cargo or Rust edits.

### Previous amendment review (findings superseded except as noted above)

Reviewed proposal SHA
`74067730cd2f0a51d5bfbf1993b92dd73300ff09800ec36f3c90ef060bdfbe8e`.
**The bounded admission architecture is aligned; two contract refinements are
required before implementing the amendment.** Earlier direct-plugin acceptance
and the EOF-only design acceptance below remain intact. This is not host-stage
implementation acceptance.

The red log `/tmp/sotf-aud138-hal-input-admission-red.log` establishes both
oversized-block and full-queue upstream advancement. Its later retry waveform
assertions are unreachable. Current `process_to_sink` drains parameter events
and delegates to ordinary processing without sink capacity admission; the
proposed prepared append closes that concrete loss mechanism. Exclusive local
queue ownership and an append that never adopts asynchronous configuration are
sufficient even if writer availability changes after preflight. Recoverable
transport failure must retain admitted input and return its consumed count.

1. Specify the control boundary between capacity admission and append. Applying
   queued parameters or automation must not invalidate the checked topology,
   negotiated geometry, identity capability or prepared queue. Restrict these
   controls to geometry-preserving operations, or revalidate after application
   and before the first producer callback, explicitly describing a failed
   control/revalidation outcome. Service-only calls must leave graph mutations,
   parameter events, automation cursors, node sample positions and host playback
   position untouched. Current source applies graph mutations before input
   validation; the amendment needs an explicit reject/defer rule rather than an
   accidental exception to its unchanged-control promise.
2. Define zero-length input and contract-failure results unambiguously. Recommended
   empty-input policy: validate the prepared route, optionally service pending
   once, return zero consumed/current pending, and do no upstream/control/clock
   work. A `SinkServiceFailure` is declared contract-only, so it must always enter
   `ResetRequired`, including before admission; the later wording permitting
   retry after a service error must refer only to recoverable waits returned as
   `Ok`. After append, an invariant error must never invite replay: either expose
   consumed/unknown status explicitly or document that any poisoned result ends
   the programme and requires cancellation/reset. Ordinary waits remain successful
   results with the full admitted count.

Required implementation evidence: both red cases become green with full exact
delivery; a service call that clears old pending still consumes zero new input;
queued controls and all source clocks stay fixed until admission and then advance
once; empty input follows the declared policy; configuration/key/service changes
between preflight and append preserve the complete block; recoverable post-append
waits report full consumption; contract violations fail closed before and after
admission; total writer attempts remain at most two across automation sub-blocks.
Define host-route requested/written/drop telemetry when bypassing ordinary HAL
`process`, and assert no duplicate request accounting on retries. Guard allocation
and deallocation on admitted, blocked, empty, recovery and terminal paths; qualify
any formatted error allocation rather than claiming all error paths heap-free.

No Rust edits or Cargo. General engine admission, scheduled application retries,
physical playback and generic branch/unequal-rate queues remain outside this
bounded amendment.

## Host-stage proposal review

**Bounded host-stage design accepted** at proposal SHA
`bedb455fd4ba7abf835d10b537d3a1e93da0e44612a2864a830a0d56f4b3bf9a`.
Sink-mode opt-in now requires both command producers remain host-owned and
prevents subsequent extraction. This makes exclusive host borrowing sufficient
for the new sink lifecycle without changing the normal shared queue protocol.
The refused-capacity test keeps its graph frozen and uses explicit producer
counters/cursor/marker state plus a separately prepared adequate-capacity
comparator. These changes close the two residual design findings below.

Implementation may proceed within the stated serial, same-rate, identity-frame
subset using the AUD137 capability. Acceptance still requires public host
waveform/retention and failure tests, all local mutator gates, preserved normal
processing behavior, and prepared heap evidence. Direct-plugin acceptance is
unchanged. Engine admission, scheduler, application routing and physical playback
remain open; no broader protocol change is approved. No Cargo or Rust edits.

Re-review of `c089879439c6d572e4ec8c6ebff3ff98fce4a8b8ea32f85184c2278118b0c105`:
the concrete opt-in hooks, guaranteed prepared append, producer error poisoning,
explicit reset and frozen lifecycle resolve the prior three semantic findings.
**Host implementation approval remains conditional on two bounded corrections:**

- A linearizable gate for already exported/cloned command senders reaches the
  shared host queue protocol. This proposal must not implicitly authorize the
  previously excluded concurrency redesign. A conservative bounded option is
  sink opt-in before sender extraction and rejection when senders were exported;
  gate direct exclusive-borrow commands and prohibit extraction in sink mode.
  Otherwise obtain a separately reviewed concrete sink-local admission design.
- The capacity regression still removes the sink after refusal, but the new
  lifecycle forbids graph mutations after the EOF boundary. Prove producer
  nonadvancement with test-owned begin/drain counters and retained marker state,
  then supported capacity repair/control comparison. Do not bypass the freeze
  or reset away the source tail just to reuse the old regression shape.

All other revised scoped contracts are aligned. No production edits or Cargo.

Reviewed `hal-output-host-drain.md`, SHA
`c28682a9d77081e13992eb75ec73ef03a2613528737e1bbeffecaf4ab7caed64`.
Explicit sink APIs, separate consumed/source-tail counts, prepared append-only
handoff and one-step external retries are a sound bounded approach. The existing
equal-width f32 fast-path predicate must remain unchanged. The supported serial
subset (one final finite-tail producer, preceding no-tail same-width/rate nodes)
does not authorize generic drain composition or engine sink admission.

**Design refinements required before host implementation:**

1. Freeze concrete sink-hook signatures and invariants: opt-in capability,
   pending/free prepared frames, validation, and append. Queue-state queries
   must not allocate or adopt asynchronous state. For unchanged validated
   geometry, appending at most the reserved bound must be guaranteed to succeed
   without writer calls or allocation. Validate unavailable/invalid sink state
   before producer mutation, not only after receiving its tail.
2. Capacity/topology preflight must precede any mutating producer `begin_drain`,
   not just `drain`. Specify error handling after producer advancement or a
   violated append contract: enter reset-required state rather than permit
   retries that lose or duplicate accepted tail. Keep transactional preflight
   failures distinct. Test injected partial-operation failure.
3. Specify host lifecycle and control ownership after sink EOF starts. New
   programme input, parameter events and graph changes cannot silently restart
   convergence counters or replace a producer while queued tail remains.
   Reject/freeze such changes until explicit reset/cancellation, or define and
   test an equally precise prepared transition. Completed streams also need
   child-terminal handling; clearing host bookkeeping alone does not reset a
   terminal child.

Preserve accepted direct-plugin point-in-time behavior; the composed host may
impose a stricter stream lifecycle. Source-before-sink staging and separate
external waiting budgets must be demonstrated through public host tests.
No production edits or Cargo performed. Engine admission, scheduler and actual
application playback remain separate open work; do not remove their guards.

## Direct-plugin implementation acceptance

Reviewed current HAL source SHA
`4905e63a57c8c20a615dfd6cea5974440b04296ccb32da493a0df0a37a6bcb1c`
against the preserved pre-edit source and accepted proposal. The logical queue
is copied into prepared frame-aligned staging; each call performs at most two
writer attempts, removes only validated accepted prefixes, and updates written
counters even if a later attempt fails. Zero writes preserve pending frames.
Empty completion remains a point-in-time queue observation; new input preserves
FIFO ordering rather than requiring a terminal reset.

Transport gates prevent implicit callback recovery. Control-thread service no
longer clears pending, and capacity preparation uses the larger of new capacity
and retained length. Failed format preparation retains queued audio. Explicit
successful reinitialization records pending cancellation as drops; preflight
rate/format rejection leaves the queue intact. No new silent EOF discard found.

Executed evidence inspected: `/tmp/sotf-aud138-hal-green-final.log` passes all
37 tests, including exact accepted sample vectors, blocked/retry completion,
physical wrap splitting a frame, context/destination rejection, unavailable and
over-reporting writer errors, transport gates/recovery, prior drops, lifecycle
ordering and zero allocation/deallocation on first/blocked/retry/terminal calls.
`/tmp/sotf-aud138-hal-clippy.log` finishes strict all-target Clippy successfully.
No reviewer Cargo or Rust edits. This accepts retained queue handoff to the ring,
not guaranteed progress while disconnected or physical playback.

Zero-output host and normal engine admission/retry integration remain unresolved.
Do not remove those guards or broaden manager/unequal-rate queue behavior under
this scoped acceptance. AUD138 whole-chain status remains open.

Revised proposal `3b3d7faea8374821a894b6ec1bd1f862d60f0b98aafe582207744132357252e7`
resolves the six findings below with a retained-queue obligation, at most two
writer attempts, prepared wrap-safe staging, unknown overall backpressure bound,
point-in-time queue lifecycle, explicit cancellation accounting and control-thread
recovery preserving pending. Implementation must retain pending even when recovery
format/capacity preparation fails. Ring flushing is an explicit control reset,
never evidence of EOF delivery. Fix the remaining proposal path typo to
`sotf-host/src/host/daw_host.rs`; no test rerun is needed for that correction.
Direct-plugin implementation may proceed. Whole-chain routing and physical
playback remain open; no generic protocol work is authorized by this acceptance.

The two public-plugin red tests establish premature completion with retained
samples: ready transport accepts only 4/8 samples, and blocked transport still
returns complete. The blocked retry assertion is not reached in the red run.
This is a valid bounded defect; it is not evidence of physical playback.

## Accepted bounded contract and implementation gates

1. Scope the EOF guarantee to retained pending samples. Existing process can
   drop newest complete frames when its queue is full and reports those drops.
   Preserve that telemetry; do not promise recovery of previously dropped input.
   EOF introduces no further silent drops or `flush_audio`-as-success.
2. Bound each call explicitly (existing `flush_pending` makes at most two
   writer attempts). Preserve exact frame order across wrapped VecDeque storage,
   short writes and zero writes. A sink reports zero output frames even when it
   advances ring acceptance. Writer over-report and unavailable writer errors
   must not consume unwritten queue state.
3. External backpressure has no finite overall call bound. Keep
   `drain_call_bound` unknown while pending; never fabricate convergence by
   dropping samples. Completion requires eventual writer availability. The
   shared trait's eventual-completion wording and host fallback limit cannot
   be treated as an unconditional liveness guarantee for a disconnected sink.
4. Honor connection, key, service, priming and format-change gates. No callback
   reconnect, key reload, transport flush, allocation or blocking wait. Specify
   how control-thread recovery resumes the retained queue without discarding it;
   existing `quiesce_transport` clears pending and must not be called implicitly
   to claim successful EOF.
5. Specify initial/empty/terminal drain, new input during and after drain,
   explicit reset/reinitialize cancellation and telemetry ownership. Validate
   initialized sample rate and empty sink destination before mutating state.
   Invalid calls must preserve pending data and caller buffers.
6. Add allocation/deallocation guards for first/repeated/terminal drain and
   scripts covering wrapped queue, alternating short/zero writes, disconnection,
   key/config changes, recovery, prior overflow accounting and reset lifecycle.

## Whole-chain boundary

The current generic DawHost rejects zero-output drain; its source is
`sotf-host/src/host/daw_host.rs`. It also enforces a finite fallback call budget
for unknown bounds. Engine retry scheduling must avoid spinning on incomplete
zero-frame sink drains. Neither that integration nor physical HAL playback is
established by the fake writer. The active daemon strips legacy HAL graph
plugins. Keep the genuine consuming route and scheduled backpressure retry as
explicit separate design/acceptance work; this review authorizes no generic
unequal-rate queue or manager rewrite and cannot close whole-chain AUD138.

Source-only review; no Cargo or production edits.
