# AUD132 independent design review

Status: bounded above512 retiming design accepted; baseline preservation
controls must be captured before production edits. Implementation review pending.
Validator: Astra medium.

Keeping delayed HR analysis `x[t-D]` while correcting its source labels from
-256 to -256-D and dropping256+D startup frames is coherent. It preserves the
HR transform while matching original source time. Conservative readiness
`t+D+512 <= t+N` holds for powers of two N>512, since D=N/2-256. Resume must
clear the delayed input/history and restart tags and prepared gains together.
No main delay or reported-latency change is proposed.

The captured impulse/full-vector retiming simulation supports the candidate;
it is not a modified-plugin result. Rejected input-delay removal remains a
failed experiment, not a passing gate. Exact-bin tone phase alone aliases
512-frame shifts, so impulse/full-sample correspondence is primary evidence.

Required refinements communicated to owner:

- Preserve actual source copies and pre-edit full output for N512 plus an
  accepted sub512 control, not just production hashes or reconstructed mixers.
- Add a larger4096/8192 route and reset/reconfiguration across512/1024, with
  mid-hop HR toggles and finite EOS.
- Preserve accepted-input output credits, tag readiness errors and paired
  nonstationary gain scheduling; compare complete emitted vectors to captured
  baseline retimed by derived D where the controlled fixture permits it.
- A second noninteger-bin phase check can disambiguate periodic delay aliases;
  keep the existing1% residual ceiling and report any limitation explicitly.

No validator Cargo run. Spatial worker owns the current shared Cargo slot;
owner coordinates focused implementation gates and final source provenance.

## Final candidate source review

Production diff is bounded: same tag/gain clock extends above512; constructor,
FFT reconfiguration, reset and resume use H+D discard/tag origin; delay history
clears on resume. N512 remains on the old route and accepted small-route
preservation digests match captures. Current source verifies against0cc6e67e…;
184 package tests/lint/format reported green on equal manifests.

One remaining evidence finding: N8192 retimed old mixed output differs0.2066,
while the no-drain raw queue agrees. Raw equality proves transform/tag origin
but not multi-hop corrected drain/gain output. The same report identifies
safety-cap nonlinearity as the cause of earlier1024/2048 full-output mismatch.
Requested an8192 low-amplitude full-vector/partition comparison with both
routes explicitly below the cap before attributing discrepancy to legacy FIFO.
If mismatch persists, require independent raw-source+prepared-gain multi-hop
output reconstruction through EOS; peak/readiness alone is insufficient.
No threshold relaxation or repeated Cargo by validator requested.

N8192 finding closed. Reduced-level impulse fixture has nonzero HR and full
retimed-vector delta0 plus partitiondelta0 for irregular/512/whole callbacks.
Combined peaks remain below0.024 versus enabled cap1.41249, with final scales1.
Current b3c4d7d5… manifest verifies. This supports complete corrected drain/EOS
output at8192; prior0.2066 discrepancy was nonlinear-cap contamination, not
proof of a legacy FIFO sample error. Requested report correction retaining the
same-run reconstructed-control limitation (no historical8192 archived vector).
Source/evidence cleared for one coordinated final broad gate with AUD131.

## Final scoped acceptance

AUD132 accepted. Verified final coordinated offline/locked workspace gate:
6071 tests passed,15 skipped across359 binaries in269.739s; MIDI/IAMF test
packages excluded, FFI included. Final strict Upmixer lint passes. Whole-tree
start/end manifests and current files match
`1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb`;
Cargo.lock remains `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
Log `/tmp/sotf-aud132-aud131-workspace-final.log` and final lint log independently
checked. This also closes AUD131's pending green workspace requirement.

Corrected report distinguishes the original inconclusive8192 cap-unverified
comparison from the successful reduced-level full-vector comparison, retaining
the same-run reconstructed-control caveat. Requested final status/gate update
and precise20480 total emitted frames including EOS wording. Docs-only updates
need no new Cargo gate. Broader spatial quality, CPU costs and AUD128 corpus
execution remain outside this scoped acceptance.
