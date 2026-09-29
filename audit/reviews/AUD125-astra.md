# AUD125 independent proposal review

Validator: Astra medium. Status: **core/API implementation accepted**;
application display remains open.

Read the proposal and actual `LoudnessMonitor::add_frames` boundary loop.
Verified EBU Tech 3341 v4.0 sections 2.1/2.2: maximum M/S display and reset
with Integrated measurement, 400 ms/3 s ungated windows, Short-term updates
at least 10 Hz. Source: https://tech.ebu.ch/files/live/sites/tech/files/shared/tech/tech3341v4_0.pdf

Accumulating inside the processing loop is necessary: publication occurs only
after a callback, which can cross several observation boundaries. Finite Option
fields, independent scalar latches and retained-reader publication are coherent.
The core/API stage does not close the required UI display work.

Required refinements sent before implementation:

- Define maxima over the fixed epoch-aligned observation grid. Do not claim
  continuous-time maxima or maxima over arbitrary snapshot-query instants.
  Exercise off-grid ends and confirm TP-only final drain adds no M/S windows.
- Check full-window eligibility against sample/window geometry, including a
  rate not divisible by ten (11025 Hz), and disclose inherited rounding limits.
- Require LRA on/off parity plus explicit wide-layout weighted/LFE coverage.
- Preserve query-error behavior; errors must not invent finite maxima.
- Supply matched before/after CPU evidence, including LRA disabled and a wide
  layout where Short-term queries traverse multiple mono meters.

Independent fixed-grid current-reading reduction plus absolute stereo tone
calibration is suitable evidence; neither should consume the new maximum fields.
No production Rust or sibling application source was edited by the validator.

Revised proposal incorporates grid/rounding, errors, finalization and baseline
requirements. One remaining coverage correction: explicit 5.1 uses the single
seven-channel adapter, whereas input widths above six use per-channel mono
aggregation. Requested an explicit 7.1 or 7.1.4 numerical parity case and
LRA-disabled CPU case in addition to the independent 5.1 weight/LFE reference.
No further design blocker remains once that coverage is included.

Final proposal check confirms explicit 7.1.4 mono-aggregation numerical,
LRA-policy parity and CPU evidence are now required. Core/API design accepted;
implementation may proceed after baseline capture. Feature closure still needs
independent implementation review and the separate application display stage.

## First implementation review

Inspected scalar API/default/copy fields and the processing-loop update. The
boundary elapsed-frame guards, finite-only latch updates, reuse of LRA queries,
reset and snapshot preparation are consistent with the proposal. Focused log
records eight passing maxima tests; no validator Cargo job was started.

Requested evidence corrections before acceptance:

- Wide-layout reference must use a test-owned role-weight table, rather than
  production `role.bs1770_weight()`, to validate weights independently.
- Add exact 48 kHz eligibility checks at 19199/19200 and 143999/144000 frames;
  detailed current coverage only exercises 11025 Hz.
- Establish finite M and S maxima before disable/enable and allocator/lifetime
  assertions; short existing fixtures with only None can pass vacuously.
- Exercise an increasing maximum while all publication generations are held,
  then release and verify recovery within the same epoch, in addition to the
  existing reset-to-lower sequence.

Matched CPU and broader final gates remain outstanding. The implementer
reported broad Cargo.lock churn atop a pre-existing dirty file; parent owns
that disposition, and no guessed restore is appropriate.

## Revised source review

The four evidence findings are addressed: test-owned role weights, explicit
48 kHz boundary assertions, populated M/S lifecycle fixtures, and same-epoch
blocked-publication recovery after four seconds each of low/high audio. The
new allocator fixture retains three distinct generations, proves publication
stays stale, releases readers and publishes the increased maxima with empty
input. Focused execution reported 9 maxima and 7 nested-realtime tests passing.
No further source blocker identified. Implementer cleared to run matched CPU,
strict lint and broad gates; final evidence and lockfile disposition pending.

## Final core/API acceptance

Accepted the stable implementation after inspecting the private
`LoudnessMaximums` cache-construction refactor and final execution evidence:
9 maxima tests, 7 nested realtime tests, strict all-target host Clippy, and
6020 passing workspace tests with 11 skipped in 268.330 seconds. MIDI/IAMF
excluded, FFI included. Independently compared the start/end source manifests
and verified current batch files against `/tmp/sotf-aud125-batch-end2.sha256`.

The first workspace run failed a convolution reset test; its isolated rerun
and repeated full workspace gate passed. This sequence is disclosed; no cause
is established by the reruns. No validator Cargo gate was duplicated.

Matched saved baseline CPU evidence reports +4.57% stereo/LRA-on and +5.96%
7.1.4/LRA-off (approximately 1.44 and 9.33 microseconds per 10 ms callback),
with other cases unchanged, within noise, or faster. These bounded measured
costs are accepted and disclosed; not worst-callback latency or publication
cost. Requested report headings say Criterion time estimate rather than
unverified median. Lockfile disposition is recorded separately and remains
unchanged under locked gates.

Acceptance is core/API only: fixed-grid maxima, inherited non-divisible-rate
window rounding, and no EBU certification claim. Required TUI/GPUI maximum M/S
display remains open. Final report: `audit/maximum-ms-loudness.md`.

## Application display proposal

Reviewed `audit/proposals/maximum-ms-loudness-ui.md`: design accepted for the
reachable TUI and Studio LoudnessMonitor surfaces. The proposal preserves
current bars, reads nullable maxima directly, uses one-decimal redraw precision,
tests maximum-only updates/reset and real query paths, and records temporary
versus restored lock provenance.

Additional required fixtures: finite maxima with false current M/S validity
flags, and vertical pressure from stacked TUI rows (width-only fitting does
not prove current bars remain visible). No further design blocker; proceed
under existing sibling authorization, with implementation acceptance pending.

## Application implementation source review

Snapshot/query/redraw mapping is coherent. Found unconditional TUI summary
rendering could cross the inner height and overwrite the bottom border.
Revised source now guards each complete summary with a saturating height
check; correction accepted. Requested bottom-border interior assertions in
the regression, because corners/side borders alone miss the original write.
Full-content 24x10/12 and redraw/localization focused checks reportedly pass;
GPUI runtime/compile evidence is pending a stable Upmixer dependency snapshot.

## Final application display acceptance

Accepted UI source and executed evidence: TUI render/height/borders 3/3,
redraw 1/1, GPUI translations 1/1, compact and mounted Studio rendering plus
live nullable routes 3/3, and all-target GPUI dev-api compilation. The final
gap reduction keeps the mounted panel within 720x900; summaries preserve
finite values despite false current flags and correctly show empty/reset
placeholders. Current sibling files independently verified against manifest
`d9b5e6f5ddfad6d2d5ccf3439b5d91ed5b365dc62f17f2b7cb7b3aa5f178372c`.

Tests used sibling temporary lock
`87f37029677509822f0164117da1a37fcf39d72a2523908b1b036f0b46bd554a`,
with accepted frozen Upmixer package manifest `f5b180b…eb45f9`. Requested exact
original sibling lock restoration to `2c87468c…335a31f` as final housekeeping;
no further Cargo needed. Preserve tested-versus-restored resolution distinction.

Core/API and application display acceptance are complete; original-lock
restoration confirmation remains before closing the batch handoff. No EBU Mode
or certification claim follows. Suggested next proposal: AUD126 pause/continue,
with explicit separation of I/LRA measurement clocks and live M/S/TP behavior.

Final housekeeping verified: sibling original lock restored exactly to
`2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`.
No Cargo ran after restoration. AUD125 core and UI handoff is complete with
the tested-versus-restored lock qualification above.
