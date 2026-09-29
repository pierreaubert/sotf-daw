# Resampler dynamic clock and lifecycle audit — AUD-059 proposal

Date: 2026-09-28. Read-only investigation; no production or test source changes in
the repository. Existing Resampler working-tree edits belong to earlier audit
work. MIDI/IAMF and graph scheduling changes are excluded. Spectral agent owns
AUD-058 backend ramp sizing; this report concerns the wrapper's programme-end
accounting, equal-rate path changes, and EOF transactionality.

## Executed public API evidence

Probe: `/tmp/sotf-resampler-lifecycle-probe.rs`; executable of the same basename;
results: `/tmp/sotf-resampler-lifecycle-probe.log`. Built with `rustc --edition
2024 -O`, linking existing production rlibs, without Cargo or repository mutation.
Linked artifact paths are in `/tmp/sotf-resampler-lifecycle-linked-artifacts.txt`.
Inspected-source hashes/timestamps are in
`/tmp/sotf-resampler-lifecycle-source-manifest.json`; source predates the linked
build. Assertions establish the existing failures. The final probe exits0.

### 1. Pending input is counted at a ratio that never processes it

Across Fast/Medium/High and44.1/48/96kHz (18cases): chunk1024,1000real frames,
only the final frame contains a0.5impulse. InstanceA accepts900frames under one
ratio, changes ratio without ramp, then accepts100. No backend call occurs yet.
InstanceB chooses the final ratio first, then accepts the identical1000frames.
Both backend histories and submitted samples are identical; every overlapping
output sample is bit-exact. Their different output lengths therefore cannot be
attributed to an interpolation or ramp difference.

| Ratio before→after | Quality | A output frames | Equivalent B frames | Error |
| --- | --- | ---: | ---: | ---: |
| 1→2 | Fast | 1164 | 2064 | -900 |
| 1→2 | Medium | 1228 | 2128 | -900 |
| 1→2 | High | 1356 | 2256 | -900 |
| 2→1 | Fast | 1932 | 1032 | +900 |
| 2→1 | Medium | 1964 | 1064 | +900 |
| 2→1 | High | 2028 | 1128 | +900 |

The upward case emits only zeros and entirely discards the final impulse. The
Medium equivalent stream peaks0.447353 at output2125. Reverse changes append900
unnecessary zeros. Counts are identical at all three tested sample rates.

Cause: `process()` adds `accepted_frames*current_target_ratio` to
`expected_signal_frames`, including input retained in `residual_input`. A later
ratio update changes how that retained input is actually processed, but never
corrects the accumulated duration.

### 2. Ramps also lose the last impulse with no pending partial block

At48k, chunk512: process512zeros at ratio1.1, ramp to1.0, then process512frames
with a0.5impulse at the final sample. Compare finite drain to a second identical
resampler given the same programme/ratio schedule followed by4096explicit zero
frames. This deterministic zero continuation is a waveform-support probe, not
an independent implementation of backend interpolation. It is valid here
because Resampler has no adaptive coefficient training. All shared samples are
bit-exact. Backend AUD-058's large-ramp panic is avoided by this small change.

| Quality | process+drain length | Continuation's final impulse peak | Peak amplitude | Largest drained amplitude |
| --- | ---: | ---: | ---: | ---: |
| Fast | 1108 | 1129 | 0.344849 | 0.00001018 |
| Medium | 1140 | 1161 | 0.374797 | 0.00080206 |
| High | 1204 | 1225 | 0.388122 | 0.00262885 |

Every final peak is21frames beyond the reported end. Reverse1→1.1 ramps also
miscount duration; observed drain ends29frames after the marker peak. That number
includes the legitimate small post-peak support and is not claimed to be an
exact29-frame overcount.

Reason: the backend ramps reciprocal ratio across its next output block; target
ratio times accepted input length is not the integrated conversion clock. The
existing integration test `ratio_ramps_use_cumulative_stream_duration_when_draining`
repeats the production target-ratio sum in its expectation and therefore misses
this failure.

### 3. Equal-rate dynamic disable drops queued input and changes latency

Nine cases (3qualities×3rates): enable dynamic ratio at nominal1, accept123frames
of a clip with final0.5impulse, disable dynamic mode, then drain. Actual output is
empty, `drain_output_frames_max()==0`, and drain immediately reports completion.
A control instance that stays enabled preserves the impulse:

| Quality | Enabled latency→disabled | Complete control length | Control peak index/amplitude |
| --- | --- | ---: | --- |
| Fast | 1055→0 | 155 | 153 /0.394532 |
| Medium | 1087→0 | 187 | 185 /0.447353 |
| High | 1151→0 | 251 | 249 /0.473513 |

Cause: `is_unity_passthrough()` depends immediately on the user flag, without
checking residual or filter history. This is an audio-path/latency transition,
not just a ratio-automation permission flag.

### 4. Re-enable emits older buffered input after newer bypass audio

Three cases at48k: accept123frames dynamically (last sample0.5); disable and
process8new0.75samples; re-enable and process901zeros to complete the old1024
frame chunk. The newer source frames123..130 are emitted immediately at output
0..7, then the older source122 impulse emerges at161/193/257 for Fast/Medium/High.
Thus disabling does not clear old residual history; later enabling resurrects
it out of chronological order.

### 5. Rejected drain latches EOF; mutation and clock guards are absent

- With one pending real frame, `drain(&mut[])` returns a capacity error but has
  already stored `drain_target_frames`. The next otherwise-valid real-input
  `process()` rejects the stream as finalized. This violates retry/continuation
  transactionality.
- `set_ratio(1.5,false)` succeeds after completed drain; mid-drain ratio changes
  are likewise unguarded while the stored output target remains frozen.
- A48k-initialized plugin accepts a44.1k process context (`Ok(0)` with one pending
  frame), then accepts a44.1k drain context and emits65frames. `initialize()` checks
  the host rate, but the actual process/drain entries do not.

## Proposed scoped lifecycle semantics (review before implementation)

### Equal input/output rates

1. Keep bit-exact zero-latency bypass when dynamic mode is disabled at the start
   of a fresh stream.
2. Once any positive input has been accepted, reject real dynamic-mode changes
   until `reset()`; reject before touching the flag, ratio, residual, counters,
   filter state, output, or latency. Same-value writes can remain idempotent.
3. Allow setup changes after `initialize()` if the stream is still fresh; the
   existing `initialized` boolean alone is not the relevant boundary. A caller
   must configure before activating processing and renegotiate declared latency.
   Current `initialize()` preserves old state, so calling it alone does not erase
   accepted-input history or authorize a path change.
4. On a legal fresh-stream path change, reset the dormant backend to nominal
   ratio as well. This prevents a pre-input enable/set-ratio/disable/enable cycle
   retaining a queued nonnominal ramp. All storage is already prepared.
5. Mark the instance's runtime `dynamic_ratio` descriptor Structural when input
   and output rates match. This is conservative host guidance to reconfigure and
   renegotiate latency; it need not make direct API setup stricter than rule3.
   Static `params.rs` cannot encode per-instance rates: document the conditional
   contract and retain the runtime descriptor as the documented source of truth.

### Unequal rates

Retain dynamic enable/disable as a realtime permission change because both modes
already use the same prepared backend. Disabling returns the ratio target to
nominal through that backend; it never diverts into passthrough or resets signal
history. Propagate any backend error before committing public state. Retain the
existing next-backend-chunk application semantics for ratio events, including
already-buffered real input; do not silently promise sample-accurate ratio events
at the original host callback boundary.

### EOF validation and freeze

Validate callback rate and destination capacity before storing an EOF latch or
mutating DSP. Compute a candidate target/bound without committing it; commit only
once a valid drain step begins. Freeze ratio/mode changes while draining or after
completion until reset. Capacity errors must allow both identical retry and
ordinary real-input continuation when no valid drain step has begun. Preserve
idempotent completion and output canaries. Input arithmetic should stay checked,
as in adjacent plugin contracts, when touching these entry validations.

## Exact EOS accounting proposal — coordinate with AUD-058

Do not repair this by averaging the user targets, retroactively scaling all
accepted input, or treating output latency as programme duration. The accounting
must follow the backend's actual source-position trajectory and submitted input
chunks. Already accepted residual input has not advanced that trajectory.

The smallest useful backend extension appears to be a read-only interpolation
cursor relative to the next input block. Async already exposes the **current**
(not target) ratio through `resample_ratio()`; the wrapper knows its requested
target and can query the corrected `output_frames_next()` from AUD-058. The
backend's last_index is currently private. Candidate API:

`Async::last_input_index(&self) -> f64`

Its docs must identify it as the last emitted interpolation-kernel position,
relative to the next input block origin, including negative filter-history
positions. It is not a physical group-delay declaration. An alternative is
`next_output_input_position(output_index)` to keep trajectory evaluation inside
the backend and avoid duplicating its floating-point recurrence. Spectral agent
is reviewing the minimal robust surface; no API or repository changes yet.

With a proven cursor/trajectory, track integer input frames actually submitted
per backend chunk separately from total accepted real frames. Freeze the source
programme endpoint at valid EOF. Pump residual/zero chunks and retain exactly the
outputs selected by a documented source-coordinate endpoint rule. That rule must
be proven against pinned fixed-rate completion counts and independent ramp
trajectories; any unavoidable fractional endpoint rounding difference (at most
one output sample) must be explicit and justified, not hidden by loose tests.
Keep `signal_delay_samples()` and realtime `latency_samples()` separate: these
are physical-delay/PDC contracts, not substitutes for variable-rate stream
position. No host graph/PDC changes are proposed here.

### Required verification before source integration

- Independent reciprocal-ratio recurrence/count oracle using AUD-058's exact
  chunk sizing; both ramp directions, multiple chunks, fractional ratios and
  partial blocks. Compare final marker position and complete emitted support,
  not just production's own duration expression.
- The18pending-input equivalence cases above become equal-waveform/equal-count
  regressions; dense input plus first/final impulses must survive.
- Equal-rate mode change after residual and after complete backend blocks must
  reject transactionally; reset permits the change and yields fresh behavior.
  Include toggles before any input, with a queued ratio update, and unchanged
  setter calls. Confirm bypass remains bit-exact and zero latency when fresh.
- Unequal-rate toggle keeps history and follows the same deterministic ramp
  trajectory as an equivalent explicit ratio update.
- Capacity/rate errors before and during drain preserve retry results; invalid
  first drain permits continuation; ratio/mode mutation during EOS rejects.
- Cold callbacks, setters, drain, reset, and maximum-channel/preallocated bounds
  retain zero allocations/deallocations. Preserve static-rate and offline
  physical-delay regression matrices without changing their oracles casually.

No implementation is authorized by this report itself; root is reviewing the
plan and coordinating the backend API.


## Reviewed lifecycle implementation — verified checkpoint

Root authorized the lifecycle subset after reviewing this report. Production
changes are confined to ResamplerPlugin/parameter documentation; no backend,
Cargo, vendor, source-position accounting, `expected_signal_frames`, physical
delay, or realtime PDC changes were made.

Implemented:

- Equal-rate mode transitions reject after accepted input or valid EOF, before
  mutation. Same-value mode writes stay idempotent, including after EOS. Fresh
  transitions reuse `reset()` to clear dormant ratio ramps and backend history.
- Runtime mode metadata is Structural for equal rates and Realtime otherwise.
  Static schema documentation explains the conditional contract. `initialize()`
  remains rate validation without clearing accepted-input history.
- Unequal-rate disable applies the nominal ramp on the existing backend and
  propagates errors before committing the public flag/ratio.
- Process/drain callback-rate validation occurs before state mutation. Input
  sample-count multiplication is checked.
- Drain computes its target without storing an EOF latch before capacity
  validation. Valid completion (including no-input completion) still finalizes;
  ratio changes and real quality/mode changes then require reset. Idempotent
  quality/mode updates do not mutate the frozen trajectory.

Evidence: the six initial public regressions had five failures on the original
code (`/tmp/sotf-resampler-lifecycle-red.log`), including the dormant-ramp toggle
sequence reaching the known unmodified backend index panic. All seven final
lifecycle tests pass, including72 same-rate rate/quality/mode/history cases,
6 fresh/reset toggle sequences,6 unequal-rate backend-equivalence cases,
transactional pre/mid-drain errors, no-input completion and conditional schema.
Four fresh audio-thread cases (one/eight channels, equal/unequal rates) measure
zero allocations and zero deallocations across legal setters, process, drain,
and reset. Allocation checks use prepared parameter IDs retained outside each
measured callback.

Full `cargo test -p sotf-plugin-resampler`: **82 passed,1 ignored**. The ignored
case is the unchanged, previously documented AUD-018 dynamic antialiasing gap;
this task added no ignored tests. Log: `/tmp/sotf-resampler-lifecycle-full.log`.
Warnings-denied all-targets Clippy passes:
`/tmp/sotf-resampler-lifecycle-clippy.log`. Scoped `git diff --check` passes.
README/CHANGELOG document both the corrected lifecycle and still-open dynamic
EOF-count limitation. The original public probe log is historical failure
proof; its assertions intentionally describe the pre-fix lifecycle behavior.

### Backend cursor coordination update

Spectral agent/root approved an isolated design for
`input_positions_next() -> owned zero-allocation Clone + ExactSizeIterator`
sharing exactly the repeated f64 step/index additions used by sinc/poly DSP.
The snapshot predicts the currently planned block and must be taken immediately
before processing; setters/reset invalidate its predictive meaning. Positions
are raw interpolation anchors relative to the next input-block origin and can
be negative (sinc FIR-window starts, not filter centers or delay declarations).
`last_input_index()` may accompany it for global-base accounting. Backend API
implementation/validation remains with spectral; integrating it into EOS
accounting requires separate root review. The mathematical quadratic expression
alone is insufficient at integer boundaries because repeated floating-point
addition can round differently.
