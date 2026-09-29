# AUD126 independent design review

Status: **core/API and reachable UI controls accepted** (scoped AUD126).
Validator: Astra medium. No production edits reviewed.

Live M/S, maxima, peaks and correlation while pausing only I/LRA is consistent
with the coupled I/LRA controls in Tech 3341 section 2.2. A second prepared
meter lane isolates paused audio from I/LRA, preserving an active-time clock.
Concatenated A+C is an appropriate independent pause-exclusion reference.

Required proposal corrections/refinements:

- Acceptance item 3 contradicts itself: reset preserves Running/Paused versus
  full plugin reset begins Running. Make reset preserve state; reserve Running
  restart for explicit start, reinitialization and destructive enabled epoch.
- Define pause/continue while disabled and parameter/getter consistency.
  Repeated setters must not secretly restart measurements.
- I/LRA validity, warming, counts/capacity and whole-program overflow status
  derive from active-lane history, never live frames. Test pause before 400 ms
  and 3 s followed by a long paused interval.
- Define state/epoch behavior for spatial/cache and integrated-mode structural
  reconfiguration; cover serialization/default/update_from propagation.
- State that K-filter history is preserved on the active-time axis across a
  pause; verify a discontinuous resume signal against concatenated A+C.
- Avoid duplicate unused integrated history in the live lane if supported by
  the backend. Report real prepared storage and matched pre-edit CPU baseline.

No further architectural blocker identified. Core acceptance remains separate
from required reachable UI controls; no EBU certification claim follows.

Revised proposal resolves reset/disabled/reconfiguration policies and specifies
active-lane eligibility and preserved K-filter state. Pre-edit CPU baseline
and matching source/lock manifests are recorded in the proposal. Design
accepted. Clarify that getters agree with immediate control state, while
retained snapshots may lag until successful best-effort publication; test that
lag rather than promise every historical snapshot always matches live state.

## First implementation findings

- **P1:** `publish_integrated_control_state` changes only a bool in an arbitrary
  prepared spare then publishes it. The spare can contain older/cold I/LRA,
  peaks and maxima, causing immediate telemetry regression. Copy authoritative
  published data into the writable candidate before changing state, without
  consuming interval queries. Test populated distinct generations and retained
  strong/nested-Weak pressure, including clear-pending reset semantics.
- **P1:** disabling while Paused calls reset that preserves Paused; approved
  destructive enabled transitions must enter Running. Current fixture disables
  only from Running and misses this case. Correct monitor, parameter cache and
  published flag together.
- Repeated direct pause/continue should be publication no-ops, matching setter
  semantics. Allocator checks currently guard process only; include transitions
  and reset, plus the wide/LRA lane.
- Add one-frame and multi-second callback partitions and a pause at a real
  partial sub-block boundary. Current concatenation checks are useful but do
  not cover the promised extreme partitions/partial interval.

No validator Cargo gate was run; implementation owner holds the shared target.

## Final candidate review

The stale-spare control publication and Paused-to-disabled findings are fixed;
repeated direct controls now return without publication. New tests compare full
telemetry and cover retained weak storage, partial blocks and guarded controls.
Verified 6042 workspace passes/13 skips and strict host lint; current files and
matching start/end manifest `5ad0cf…72ce` verified. CPU and total prepared
heap-request evidence were provided, with suitable scope caveats.

**Remaining numerical compatibility finding:** active I/LRA observation admission
now uses exact elapsed 400 ms/3 s thresholds instead of the old completed
four/thirty-sub-block conditions. At 11025 Hz the first historical I block at
4408 frames and LRA observation at 33060 frames are skipped, since the new
checks require 4410/33075 frames and next observe only on the following grid
boundary. The accepted proposal preserves inherited floor-grid geometry.
Requested restoration of legacy active-lane block admission (separate from
published validity and M/S maximum eligibility), plus independent first-window
counts and never-paused compatibility. New-monitor A+C comparisons alone miss
this shared regression. Any intentional policy change requires explicit review.

Requested an on-disk final report with CPU/storage/gates/manifests, beyond the
proposal's pre-edit baseline. Core/API acceptance remains pending this finding.

Revised final source restores four/thirty completed active-block admission and
integrated validity. Independent boundary/count tests now cover the 11025 Hz
first I/LRA observations. Verified final workspace 6043 passed/13 skipped in
268.544 s, strict lint, equal manifests and current-file checks against
`fc8ef98a…459e12c`. Numerical finding closed; no production blocker remains.

Final report still required on disk. Latest CPU run has running estimates
36.894/36.739 us stereo, 94.879/89.872 us 5.1, 191.55/194.56 us 7.1.4
(LRA on/off), with detected baseline regressions about 10.2–29.3%. Requested
these latest figures and frequency-variation limits, not superseded earlier
numbers. No further Cargo gate requested; acceptance awaits report completion.

## Final scoped acceptance

Reviewed completed `audit/coupled-integrated-lra-pause.md`: semantics, numerical
fixes, exact commands/manifests, total prepared heap-request bytes and latest
CPU estimates/variation are accurately qualified. Core/API accepted. All
three production findings are resolved, and final 6043-test workspace/strict
lint evidence matches reviewed source. No further Cargo required.

Running dual-lane overhead is measured at roughly 10.2–29.3% for this fixture;
largest observed core time is 194.56 us per 10 ms callback. Accepted as disclosed
feature cost, not a worst-case latency guarantee. Prepared memory measurements
are total monitor allocation requests, not incremental baseline cost or RSS.

AUD126 remains open for reachable application Start/Pause/Continue/Reset
controls and their state/lifecycle tests. AUD127 and AUD128 also remain open;
no EBU Mode or certification claim is made.

## UI/control proposal review

Transient false-default/false-getter Start/Reset commands and preset exclusion
are aligned, as are per-monitor targeting and reachable control tests. The
proposed generation-only pending acknowledgement still has blockers:

- Same-state Pause/Continue does not increment generation but UI waits for a
  different generation, causing permanent pending without an explicit no-op
  acknowledgement or disabling that redundant action.
- Any generation change relative to a stale published snapshot can come from
  an earlier control, falsely acknowledging a newly queued Start/Reset.
  Require a correlated request token echoed after application or an actual
  applied-generation receipt; test delayed old publication after a new request.
- Runtime instance incarnation must be part of identity: graph-node stability
  alone cannot distinguish replacement meters whose generation restarts.

Define supersession/retry/recreation behavior without broad manager protocol
changes. Correct DAW paths in proposal (sibling `../sotf-daw`, not nested under
the application repository). UI/control production edits not yet approved.

Revised exact request-ID receipt protocol resolves the generation/no-op
acknowledgement blockers. UI/control design accepted, with required bounded
ASCII/checked-ID parsing before mutation, duplicate-ID non-replay semantics,
explicit older-ID policy and receipt persistence across ordinary measurement
reset/Start. Runtime instance replacement clears identity/receipt and cancels
pending UI requests. Only one pending request per instance; explicit retry
uses a fresh ID. Test malformed/duplicate/no-op/delayed old publication and
replacement behavior. Existing manager/queue protocol scope remains unchanged;
implementation must return for review if reliable route identity is unavailable.

## UI/control implementation review — revisions required

The bounded instance/request parser, duplicate-operation guard and snapshot
receipt plumbing are coherent on source inspection. Displayed-snapshot identity
must select the analyzer role actually shown by the Studio panel; the visible
user graph node is not necessarily that analyzer. Core acceptance stands.

- **P1:** GPUI Retry remains enabled after acknowledgement and replacement,
  and combines an old operation with the current runtime ID. TUI similarly
  falls back to an unqualified last operation. An old destructive Reset/Start
  can consequently reach a replacement monitor. Retry must retain the original
  identity and be available only for unresolved/failed same-instance requests;
  acknowledged, superseded and replaced requests are terminal. Add regressions.
- **P2:** Missing snapshot/target paths hide or defer replacement cancellation.
  Preserve an explicit terminal cancellation indication and prevent transfer
  when the target disappears or changes.
- **Evidence:** The mounted HTTP click fixture manually injects instance 77 and
  receipt/state updates. It proves rendered routing, not actual host application
  and receipt readback. Add a real host/Player command-to-publication test with
  distinct input/output instances, retaining the fixture for delayed receipts.
  TUI tests must exercise the actual documented key dispatcher, not only helpers.
- **Realtime qualification:** The borrowed parser can be allocation-free, but
  the public owned String setter drops payload storage; the existing processing
  command route parses strings and sends responses between blocks. Add guarded
  valid borrowed command/mutation/publication tests with retained readers and
  disclose owned transport allocation. Do not claim an allocation-free complete
  String transport or expand into the separately blocked manager redesign.

No validator Cargo run. Reviewed source through the declared stable handoff;
implementation owner will revise and rerun relevant gates. TokenSave index was
rebuilding, so current source bytes were used for decisive inspection.

Revision source pass: TUI now terminalizes exact acknowledgement, supersession
and changed nonzero runtime IDs; Retry retains the original runtime identity.
Its new actual key-dispatch test applies generated commands to two distinct real
host plugins and reads actual output-only receipts, including host rejection
and retry. This is useful component integration, not a Player/audio-device test.
Missing/zero-ID snapshot reconciliation still returns without cancellation;
requested an explicit disappearance policy and regression before closing it.
Owned public command allocator probe reports zero allocations/two deallocations;
this supports the transport qualification, not a no-heap-operation claim.

Located prior temporary dependency artifact
`/tmp/sotf-aud125-ui-temporary-resolver.lock` with SHA
`3d1581cebf529b878c201e52ebe9beec3099434f5409a1f1206038da3bff8009`,
math-audio revision `cabbc6dc1c3d0c8aad275ac33ec015d174859c89`.
Sent this evidence to the owner to reconstruct and verify the later mimalloc
variant without modifying unrelated dirty analog plugin code. No Cargo run.

Further revision source pass: TUI now cancels both missing and zero-instance
snapshots and clears retry identity; disappearance finding closed. LevelMeters
focus is valid across Library/Queue, so a Queue-only guard would be a regression;
wrong-focus and Configure-context tests are the appropriate non-dispatch checks.
GPUI Retry now requires the same instance and an unacknowledged request ID.

Remaining GPUI status precedence: a stored submission error is returned before
checking runtime replacement or an authoritative exact/newer receipt. A late
successful publication after a transport error remains falsely failed; replacing
that failed target hides cancellation. Requested terminal identity/receipt
outcomes before transport error, with regression assertions. The inspected
mounted fixture still manually assigns receipts; actual GPUI host publication
route evidence remains outstanding. Revised gate results reported by owner:
TUI 365/365 and no-deps strict lint, mounted GPUI route passing; final manifests
and temporary-lock provenance still to be captured after fixes.

GPUI status precedence revision inspected: runtime mismatch and exact/newer
receipt now take precedence over stored submission error, with mounted
late-acknowledgement and replacement regressions. Finding closed.

Accepted a bounded layered verification strategy because the current GPUI test
Player has no engine and the runtime sink selector offers only CPAL. No new
headless engine or manager protocol is required. Remaining bridge: seed mounted
controls from two actual host snapshots, apply the exact recorded UI-generated
engine index/payload to the corresponding real host, and return its actual
published receipt to the panel, checking other-host isolation. Manual retained
receipt fixtures remain supplemental. This does not claim end-to-end execution
through Player/processing transport; final report must retain that limitation.

Final mounted bridge inspected and passing log verified:
`/tmp/sotf-aud126-gpui-live-route-host-bridge-final.log` (1/1). It initializes
two real monitor instances, obtains the exact submitted payload/index through
the live query, applies it unchanged to the output host, and renders its actual
published receipt for Start/Pause/Reset/Continue. Input receipts/state remain
unchanged. This closes the agreed layered integration gap; Player/processing
transport remains explicitly outside this test. All functional findings closed.

Verified TUI final suite 365/365 and package no-deps strict Clippy, plus final
GPUI all-target check log (existing test-helper warnings disclosed). Public
owned command test explicitly asserts zero allocations and two deallocations
under retained publication pressure. Final acceptance awaits evidence manifest,
report and exact original sibling-lock restoration; no new gate requested.


## Final UI acceptance

All functional findings are closed. Reviewed final UI report with explicit
layered transport and heap-operation limits. Independently verified every file
in sibling manifest `bd97ee929fff3fae4e4759298eae442a827e37ca8549451addc9818e1a1b7075`
and host manifest `59ae5f4146e0313536084d18104755a37af8f002af4c2bdbaf3da8e567d6a2f3`.
Sibling Cargo.lock is byte-identical to original
`2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`;
final sibling gates used temporary `7a03b9f561ee929aa189ee70881d84563ab1b1d0036b3a76078d50c033a5af3a`.
Restored-lock builds are not claimed tested. Scoped AUD126 accepted; AUD127/128
and unrelated audit items remain open. Requested stale core-report open-work
paragraph link to the accepted UI report.
