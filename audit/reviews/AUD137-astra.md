# AUD137 Astra design review

Status: **ACCEPTED for the bounded identity-frame, same-rate serial child scope**.

## Final revision acceptance

Reviewed and verified all nine current entries of
`/tmp/sotf-aud137-review-fix-final-end.sha256`, aggregate
`5d35cc3b55868eb1078af9dbabe9a5698211c6134a711f954110d0fa033a0425`.
Both P1 findings below are closed:

- Identity admission now requires an explicit conservative plugin capability,
  forwarded by the parametric adapter, and the host checks every active node's
  negotiated input/output rate. Actual production Delay opts in. No sampled
  inference or ordinary fast-path change is used. Adversarial size-83 and
  compensating 48→96→48 routes reject before child begin/drain calls.
- Drain checks completion before emission and includes the remaining dry
  timeline in its chunk clamp. Exact-count regressions cover empty/no-tail
  completion with nonzero declared capacity and a three-frame dry-only suffix.

Final package log passes 131 tests with four manual utilities ignored (including
the separately executed replay); explicit
seven-control byte replay passes 1/1. Strict ABCompare, host and Delay all-target
Clippy passes. Logs are `/tmp/sotf-aud137-review-fix-package.log`,
`/tmp/sotf-aud137-review-fix-replay-final.log`, and
`/tmp/sotf-aud137-review-fix-clippy.log`. Earlier independent full-vector FIR,
serial Plugin/Rack/Graph, terminal child, poisoned failure, frozen controls and
prepared heap evidence remains part of this scoped result.

Unsupported geometry, undeclared child capabilities, general nested/branching
graphs and active/prior recursive band-mask EOF remain explicitly outside this
acceptance. `TailLength::Unknown` stays truthful; this is not whole-audit closure
or a new full-workspace gate. No reviewer Cargo or production edits.

## Historical implementation findings, resolved

Reviewed frozen source manifest `bdad348d11d74759da66e6b290fff949abd8a17499dd4fdb3fca8ee69f620ee1`
and report. Current files verify against all five entries. Package evidence is
127 passed / 4 manual ignored, strict lint clean and explicit baseline replay
1/1. Independent FIR vectors, serial Plugin/Rack/Graph coverage, failure
poisoning and prepared heap guards are useful, but do not close these findings:

1. **P1 — sampled geometry admission is not a proof.**
   `validate_drain_host` probes only six input lengths. An arbitrary admitted
   factory can differ at any unprobed length or have compensating internal rate
   changes while aggregate probes pass. Require a reliable per-node capability
   or constrained known geometry contract covering all accepted callbacks and
   reject unsupported paths before mutation. Add an adversarial child differing
   at an unprobed size. Do not simply add more sample points.
2. **P1 — drain can emit capacity-sized excess silence.**
   After pumping children, frame selection starts at full capacity and only
   reduces for nonempty child queues or nonzero child alignment remainders.
   Both children may report complete with zero frames and no alignment state,
   yet the implementation emits a full capacity block before noticing complete.
   A dry-only final remainder shorter than capacity is likewise over-emitted.
   Check true completion before emission and bound each chunk by the actual
   remaining composed timeline, including dry-only state. Require exact-length
   tests for a no-tail child with nonzero declared capacity, zero-frame final
   completion, and a short final dry-only fragment; include empty-stream policy.

Child cursor pairing, prepared staging and reset-required errors otherwise
follow the accepted design on inspected paths. No additional source finding
was identified in this pass. No reviewer Cargo or production edits. Fix these
cases and rerun affected numerical/lifecycle plus scoped package/lint gates;
whole-feature acceptance is pending.

Revised proposal `b10a82638b1caae6bcdf6495d77bedf1aa23e543cc78864ffb27c4b3bc77df00`
closes the findings below: explicit reset is required after completion or partial
failure; controls freeze; preflight is transactional; current or previous active
band-mask use rejects EOF until reset. Seven meaningful pre-edit mixer controls
are captured, with explicit replay passing 1/1 in
`/tmp/sotf-aud137-preedit-mixer-controls-replay.log`. AutoGain movement is
compatibility evidence, not independent quality evidence. Bounded implementation
may proceed; recursive band-mask EOF remains an explicit unsupported follow-up.

The public 8-to-72-frame red case and exact 48-frame analytic shift demonstrate
nonzero lost child output. Same-rate child draining with independent output
cursors and prepared bounded staging is appropriate. Captured ordinary mixer
vectors must precede shared-kernel edits. Rack and Graph routes, unequal child
latencies/tails and independent full-vector oracles remain required.

## Historical findings, resolved by revised design

1. Proposal item 4 incorrectly equates clearing DawHost drain bookkeeping with
   a fresh child stream. `DawHost::process` clears only its own cursor after
   successful processing. Terminal-locking children such as SpeechDenoiser
   reject new input first; arbitrary recursive child state is not reset.
   Choose an explicit-reset-required parent policy or genuinely reset all
   child/outer state after complete preflight using prepared allocation-free
   operations. Test a terminal-locking child, not only Delay.
2. Freeze structural/path/band-mask changes once drain starts. If one child
   advances and another fails, mark parent reset-required; do not permit a
   retry to silently duplicate or discard timeline data. Validation errors
   before advancement remain transactional. Define zero-frame calls similarly.
3. Active recursive band-mask rejection before either child advances is an
   acceptable bounded unsupported case, retained as a named follow-up. State
   exactly what disabling/resetting the mask after rejection discards; Unknown
   metadata alone never licenses truncating recursive response.

The child/composition completion policy must remain distinct from mathematical
finite support. No generic unequal-rate queues or manager protocol changes are
authorized. Source-only review; no Cargo or production edits.
