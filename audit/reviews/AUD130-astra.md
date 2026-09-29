# AUD130 independent design review

Status: **bounded N<512 source-tag correction accepted**. N>=512 timing remains open.
Validator: Astra medium. Implementer: Upmixer Luna xhigh.

The source contract calls for synchronized additive HR/main paths and matching
main-analysis gain schedules. AUD129's synchronized nonzero-contribution probe
establishes the sub-512 arrival discrepancy. A stable latency policy independent
of HR toggles is sensible because host graph latency is cached.

The proposed `512 - N` main-input delay is not yet approved:

- Delaying main analysis also delays transient and HR gain decisions relative
  to the unchanged HR input. Neutral waveform alignment alone cannot establish
  correct transient routing and gain timing.
- Increasing startup padding while adding input delay risks counting delay
  twice. Derive emitted-stream indices through input prefix, WOLA/discard,
  startup padding, and HR mixing before selecting compensation amounts.
- Current HR first readiness at frame 512 was measured with the current main
  scheduler; it is not established as invariant after altering that scheduler.

The planned independent phase/arrival characterization may proceed before
production edits. Add a nonstationary transient/gain fixture and a written
timeline derivation. An output-side main delay may preserve analysis timing,
but would need explicit queue-capacity and gain-index alignment analysis; it
is an option to investigate, not an approved substitute design.

Preserve AUD129 minimum/capacity guarantees, AutoGain causal reference,
finite-stream frame accounting, reset/bypass semantics, allocation freedom
and unchanged N>=512 outputs. No production correction accepted yet.

## Output-side candidate revision

The revised proposal leaves analysis and startup padding N unchanged, delaying
main audio and its corresponding HR gain by D=512-N at the existing drain.
This is coherent in principle and avoids the earlier input-analysis shift and
double-padding risk. Characterization may proceed.

Required additions: a nonstationary transient/gain pairing test; midstream HR
enable/disable and resumed-input/EOS coverage; and explicit queue/HR read-clock
derivation. Mixing must read the delayed gain, not the old main-ring position.
Unavailable HR frames and initially zero queue contents must not advance the
wrong clock. Independent phase/arrival evidence remains required before
production implementation approval.

## Consolidated characterization review

Fixed-gain one-frame-callback probes and independent settled-tone projections
now agree on HR delay 511 below N=512, rather than the earlier dynamic-gain
peak at 512. Two tones corroborate the phase. Above 512, measured excess
delays are 256/768 frames at N=1024/2048. Removing the existing HR input delay
in a test-only candidate aligns impulse peaks but fails the fixed 1% residual
ceiling at N=1024; that extension is not approved.

Remaining blocker to a constant `511-N` production delay: fixed-gain timing
was measured with one-frame callbacks and per-callback envelope reseeding.
Require stabilization independent of callback partition and impulse/phase
comparison across one-frame, 512-frame and irregular callbacks. The earlier
N=256 partition sensitivity and 511/512 distinction may reflect scheduling.
Derive the synthetic source gain's index N from actual startup discard and
ring mapping rather than assuming its correspondence to source frame zero.

The stale partial HR input after off/on is a separately confirmed lifecycle
sub-finding requiring a reset/resume policy. Above-512 timing and lifecycle
findings should be tracked separately from bounded sub-512 correction. No
production timing fix is approved by the present evidence alone.

## Source-tag scheduler design decision

Prepared constant-gain partition matrix confirms the untagged FIFO defect:
sub-512 HR peak placement varies with callbacks (including 511/512 and
N=256 irregular peak 427). Padding/capacity-only simulation also fails
partition invariance. Therefore constant `511-N` output compensation is
rejected as a general solution.

Approved implementation of the revised bounded N<512 scheduler with explicit
source-frame tags and fixed 512-frame reported/emitted latency. Required
invariants: post-discard tags derive from accepted source positions; output
frame t corresponds to source t-512; negative tags produce initial silence;
main/HR/gain pairing consumes matching tags only. Derive HR readiness from
the fixed window/hop/prefix, and stop to revise if a frame cannot be available
at its declared output time rather than silently dropping HR contribution.

Acceptance must include one-frame, irregular, 512-frame and multi-second
callbacks, final-sample/EOS accounting, causal gain/AutoGain reference,
allocation freedom, reset/bypass and sub-512 HR toggle without stale-tag replay.
N>=512 must remain byte-identical; its delay correction and broader lifecycle
work remain separate. This approves implementation, not a passing result.

## First implementation review

Inspected source-tag generation, accepted-input output credits, pre-drain
readiness validation, matching-tag mixing, HR resume history clearing, AutoGain
latency and finite drain changes. The bounded N<512 design is recognizable;
no concrete production blocker identified in this pass.

Requested three acceptance evidence fixes:

- The corrected partition test still overwrites ring capacity and startup
  padding inside its fixture. Remove candidate-era overrides and assert actual
  constructor preparation so production regressions cannot be masked.
- New small-FFT AutoGain coverage checks finite values and cursor position but
  lacks a causal delayed-input numerical reference. Extend AUD115's independent
  reference to N=2/256 with the new 512-frame latency and disabled warming.
- Above-512 unchanged sample claim requires captured baseline digests or an
  isolated prior-path equivalence control. Stable peak positions alone do not
  prove byte-identical output.

Reported package 173 passing/2 intentionally ignored and strict lint evidence
does not close these oracle gaps. Final implementation acceptance pending;
coordinate subsequent source changes with sibling UI gate snapshots.

## Final bounded acceptance

Accepted after the three evidence corrections: partition fixture now asserts
constructor-prepared capacity/padding; N=2/256 AutoGain identity and reference
checks cover disabled warming then enabling at the new 512-frame latency;
N=512/1024/2048 compare full output arrays against an isolated reconstructed
pre-AUD130 prepared-gain mixer across three callback partitions, with nonzero
HR contribution and sample digests.

Verified final package result: 175 passed, 2 ignored rejected candidate tests,
zero failures; strict all-target Clippy passed. Worker reports format check
passed. Matching start/end manifest SHA
`f5b180b05ff15981b20ea2e5ad306bf0001daafc34dba5258bc74a6394eb45f9`
was independently compared and current crate/lock files verified against it.
Logs: `/tmp/sotf-aud130-final-accepted-package.log`,
`/tmp/sotf-aud130-final-accepted-clippy.log` and
`/tmp/sotf-aud130-prepared-baseline-digests.log`.

Limitations: the >=512 control reconstructs only the mixer while sharing
current analysis/preparation/drain code; it is not an archived whole-plugin
prechange binary or output. Source review supports preservation of that branch.
Above-512 alignment remains unresolved; no general HR quality or CPU bound
claim is accepted. This package-scoped result is not a new workspace-wide gate.
The bounded correction includes sub-512 resume history clearing required to
prevent stale source tags. Broader lifecycle policy is separately tracked.

The implementation owner should retain these limits in `audit/upmixer-hr-timing.md`
and coordinate the above-512 follow-up with the shared ledger owner. No duplicate
Cargo gate was run by the validator.
