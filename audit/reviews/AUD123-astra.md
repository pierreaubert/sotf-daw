# AUD123 independent validation

Validator: Astra, medium reasoning effort. Implementation: Luna, xhigh.
Status: **ACCEPTED for the scoped AUD123 rate-extension batch** (2026-09-28).
The full audit remains open.

## Requirements reviewed

- Preserve exact published 12-tap arithmetic at 48/96 kHz.
- Select a bounded power-of-two interpolation factor reaching at least 192 kHz,
  with minimum 2x and maximum 32x, for accepted audio rates from 8 kHz.
- Keep lower diagnostic rates explicitly unavailable; verify the backend's
  actual accepted upper boundary and transitions between interpolation factors.
- Verify analytic EBU Tech 3341 cases 15–19, independent finite convolution,
  dense/impulse/DC inputs, arbitrary callback and query intervals, channel
  counts 1/2/6/24, reset/reinitialization and retained snapshot readers.
- Finalize all actual filter support without emitting audio, adding audio
  latency, or advancing the other meter clocks. Repeated finalization is stable.
- Prepare storage outside realtime processing; measure cold process, reset and
  finalization allocations/frees, plus processing and worst-case drain cost.
- Inspect executed focused tests, strict host lint and the final workspace gate.
  Preserve MIDI/IAMF exclusions, unrelated dirty files and prior approval blocks.

Primary sources checked directly:
[ITU-R BS.1770-5 Annex 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf)
and [EBU Tech 3341](https://tech.ebu.ch/docs/tech/tech3341.pdf).
The proposed rate policy is consistent with the former's 192 kHz guideline.
The EBU tone expectations are -6 or +3 dBTP, with +0.2/-0.4 dB tolerance;
fixtures must include the specified fades. No proposal blocker found.

## Review protocol

Luna supplies stable changed paths, exact commands/logs, independent oracles,
realtime and CPU evidence and limitations. Review source only after that handoff;
do not duplicate an active Cargo gate. Production fixes remain Luna's work.
Concrete findings return to Luna; acceptance requires rechecking the fixes.

## Next-batch investigation retained

After AUD123, inspect programme true-peak maxima, maximum M/S loudness and
simultaneous I/LRA pause/continue against EBU Tech 3341 sections 2.1, 2.2 and 2.8.
Section 2.4 also requires an instability indication during the first 60 seconds
of LRA measurement. The current `loudness_range.rs::query` marks the first
surviving observation `Valid`; existing observation counts may support a UI
interpretation, but the AUD117 report explicitly leaves UI integration out.
This is a scoped investigation finding, not a claim that AUD123 must implement
those features. Authentic corpus execution and EBU transient cases 20–23 remain
separate evidence work. Further next-batch investigation is paused until AUD123
is ready for validation.

## Revision 1 source review

Luna handed off a stable kernel and tests before running Cargo. The kernel
selects published 12-tap versus prepared 64-tap storage outside processing;
sample processing, reset and finish contain no heap operations by inspection.
The 63 zero advances cover the 64-coefficient history and finish clears that
history. The published-path accumulation order preserves the prior structure.
Executed allocation counters and exact old-output controls are still required.

Findings sent to Luna:

1. **P1 compile blocker:** the long-sinc oracle negated a `usize` at its starting
   time bound. Luna reported correcting the bounds to signed arithmetic.
2. **P1 stale oracle assumptions:** `true_peak_calibration.rs` and
   `true_peak_finite_stream.rs` still used the old 44.1/88.2 kHz phase counts and
   11-zero support. Preserve the 48/96 kHz published oracle; adapt the changed
   paths and keep new high-ratio lifecycle coverage.
3. **P1 acceptance evidence:** a final-impulse peak range of 0.45–0.51 cannot
   establish complete convolution or interval timing. Add direct offline
   convolution checks across dense input, impulse offsets, callback/query
   boundaries and factors 8/16/32. This implementation-equivalence oracle must
   remain backed by independent analytic and longer-window quality oracles.
4. **P2 oracle extent:** the 129-sample Lanczos oracle searched only +/-32
   source intervals beyond the input; use its full +/-64 reconstruction support
   if claiming complete finite reconstruction.
5. **P2 public documentation:** `analyzer.rs::true_peak_is_compliant` still says
   only 48 kHz is verified and other rates are approximate. Update the host
   policy without implying authentic-programme certification or changing the
   separate math-dsp detector's scope. The obsolete 88.2-kHz phase comment was
   also reported corrected by Luna.

The production phase/support review has identified no correctness blocker so
far. This is not acceptance: revised tests, CPU/heap evidence, focused gates,
strict lint and workspace execution remain pending.

## Revision 2 source review

The signed long-sinc bounds now cover the full radius-64 support; this oracle
also exercises factor 16. Old published-table tests preserve 48/96 kHz and use
four phases at 88.2 kHz. The new direct offline convolution sums by source/output
indices, checks queried intervals and the complete tail for 64 impulse offsets
and dense input at factors 8/16/32. Its coefficient formula describes the same
design as production, so its role is state/support equivalence; the separate
Lanczos and analytic-tone tests supply independent accuracy expectations.
The public field documentation now describes host rate support without claiming
external certification. Findings 1–5 above are addressed in source.

Requested the remaining public semantic lifecycle checks at custom-kernel
rates (8/12/44.1 kHz): reset/reinitialize, disable/enable and strong/Weak retained
reader recovery. Existing semantic tests were still at 48 kHz; the 44.1-kHz heap
test alone establishes allocation behavior, not the recovered final values.
Ordinary zero-continuation control can verify those values independently of
the explicit finalization path. No source correctness blocker identified.
Executed gates, exact old-output controls, CPU/drain measurements and allocation
evidence remain pending.

## Revision 3 execution and lifecycle review

Inspected `/tmp/sotf-aud123-focused.log`: 548 library, four calibration, three
heap, 11 finite-stream and four rate tests passed. The heap tests assert zero
allocations and frees for cold processing, repeated finalization/query and
reset, at 1/2/6/24 channels including the accepted maximum rate. The full host
log `/tmp/sotf-aud123-host.log` also ends successfully with eight ignored docs.
These logs predate the remaining lifecycle expectation refinement below.

The public lifecycle matrix now exercises custom kernels at 8/12/44.1 kHz.
However, held-reader recovery only asserted a finite peak at those rates; that
would not reject the historical finite-stream underread. Requested comparison
with ordinary 63-zero continuation, or the analytic unit-impulse peak, for the
initial and recovered custom-rate values. This remains an evidence blocker.
Luna is preparing CPU/finalization measurements; no overlapping Cargo command
was started by the validator.

## Revision 4 lifecycle and cost review

Custom-rate initial/recovered final peaks now compare with an ordinary
63-zero-continuation control to 2e-12 dB. The 48-kHz published-table oracle is
preserved. Inspected the updated source and successful 11-test execution in
`/tmp/sotf-aud123-lifecycle.log`; the lifecycle evidence finding is resolved.
`/tmp/sotf-aud123-clippy.log` ends successfully.

Inspected the new Criterion benchmark source and `/tmp/sotf-aud123-cpu.log`.
It measures 128-frame ingestion separately from reset/publication and measures
finish plus low-level snapshot update separately. Candidate estimates include
3.374 ms ingestion and 1.659 ms finish/update at 8 kHz/24 channels, and 0.904 ms
and 0.440 ms respectively at 44.1 kHz/24 channels. These are local benchmark
estimates, not individual-call worst-case bounds or portable realtime guarantees.
Requested hardware/profile/variation reporting, matched pre-AUD123 48/96-kHz
CPU/exact-output controls, and unchanged remaining telemetry evidence. Candidate
timings alone cannot establish before/after performance preservation.

### Matched-control finding

Luna reported that the requested alternating old/new kernel comparison found
15–19% processing overhead at 48/96 kHz with 24 channels and 14–20% drain
overhead. Per-sample enum dispatch was identified as the cause. Luna is hoisting
dispatch outside sample loops and will rerun identical output-checked controls.
This supersedes the previously reviewed kernel snapshot; final source and gates
must be reviewed again. No acceptance has been issued.

## Revision 5 dispatch review

Inspected the revised production code: enum selection is outside each sample
loop; small FIR helpers and their call paths are inlined. Published sums retain
the same order. The matched control is a reconstructed pre-AUD123 kernel in the
same test executable, using the same coefficient table. Independent published
table tests remain the accuracy oracle; this control addresses exact refactor
preservation and local kernel cost, not historical whole-monitor speedup.

The first post-fix control log reports process ratios of 0.895/0.928 and drain
ratios 0.888/0.898 at 48/96 kHz, 24 channels. Requested two diagnostic refinements:
consume the second input interval before asserting exact drain-only values,
and alternate drain trial order too (processing already alternates). This
prevents earlier interval maxima from masking the tail and makes the stated
timing protocol accurate. Final rerun and aggregate gate remain pending.

Expanded finish-only controls now compare every other serialized telemetry
field across 8/12/44.1/48/88.2/96 kHz and 1/2/6/24 channels, with complete-support
peak expectations. Source review confirms this extension addresses the prior
unrelated-telemetry evidence request.

## Final focused evidence review

Inspected `/tmp/sotf-aud123-focused-final2.log`: 549 library tests passed, one
manual timing diagnostic ignored; calibration 4, heap 3, finite-stream 11 and
rates 4 passed. `/tmp/sotf-aud123-clippy-final.log` passes strict all-target host
lint. Scoped `git diff --check` passes. The final control now consumes/asserts
both input intervals before asserting exact tail equality, and alternates
process and drain order. `/tmp/sotf-aud123-kernel-cpu-control-final.log` passes.

Final process ratios candidate/reference are 1.054/0.978 at 48 kHz for 1/24
channels, and 1.031/1.017 at 96 kHz; drain ratios span 0.957–0.975. The reference
is a reconstructed kernel in the same opt-level-1 test executable. Scheduling
variation is visible. No material 24-channel regression is established by this
limited local comparison, and no historical whole-monitor comparison is claimed.

Reviewed the final optimized Criterion output and report: 24-channel 128-frame
ingestion uses 3.701 ms at 8 kHz and 0.969 ms at 44.1 kHz; finish/update estimates
are 1.817 ms and 0.479 ms. Hardware is AMD Threadripper PRO 3995WX, Linux x86_64.
The supported maximum rate is numerically/allocation tested but not profiled.
Requested explicit prepared-memory bounds, precise tolerance/CI terminology,
and reproducible commands in the report; these are documentation refinements.

The implementation plan originally required the workspace gate after focused
tests and lint. Requested it on the stable final source. Luna started the
documented offline workspace nextest run, excluding MIDI/IAMF and including
FFI; log `/tmp/sotf-aud123-workspace-nextest-final.log`. Acceptance awaits that
result and final report verification. No validator Cargo job overlaps it.

## Acceptance

Inspected the final workspace log: **5,997 tests passed, 11 skipped**, 268.549 s;
the full-history heap stress test finished in 267.149 s. MIDI/IAMF are excluded
and FFI is included. Report corrections now state the interpolation-rate units,
published-oracle tolerance, prepared-memory bounds, exact commands and the
scope of timing confidence intervals. No unresolved AUD123 correctness or
required execution finding remains.

Acceptance combines source inspection, independent primary-source expectations,
analytic and Lanczos quality oracles, direct complete-convolution/state oracles,
public lifecycle and retained-reader controls, cold zero-allocation/free checks,
exact 48/96-kHz reconstructed-kernel equality, measured local costs, strict lint,
and the final workspace gate. The validator inspected the executed logs and
test implementations; it did not duplicate the implementer's Cargo runs.

Reviewed source SHA-256 values:

- `src/analyzer_loudness_monitor.rs`:
  `204a2bee9e9ae3a7a18fc827fb94e5b8bf22e220b57acba9aec3ab237fb2b753`
- `src/analyzer.rs`:
  `663308f93489116163953552cff46cbad2a69669bc476d2af3a478cc0763e379`
- `tests/true_peak_rates.rs`:
  `0e4fe0260d45c4843e4a1037963c1e0465869e147fd30e8280e40e696245e994`
- `tests/true_peak_finite_stream.rs`:
  `d2b79608566616bffd6a6525634290c3341d625deaebaf4471702cad09d42fea`

Paths above are relative to `crates/sotf-plugins/crates/sotf-host`.

### Limits retained

The authentic external corpus and arbitrary programmes/rates are not certified.
The accepted 2,822,400-Hz maximum has numerical/allocation evidence but no CPU
profile. The historical CPU comparison is reconstructed kernel-only; absolute
whole-monitor timings and their sizeable low-rate costs are reported separately.
Confidence intervals are not worst individual callback or device deadlines.
The separate math-dsp detector's 48-kHz reference scope is unchanged.

### Next authorized batch

Proceed with a source-grounded metering requirements investigation: programme
maximum true peak, maximum momentary/short-term loudness and their reset epochs,
simultaneous integrated/LRA pause/continue, and first-60-second LRA stability
indication. Reconcile current source and wrappers before creating local issue
rows/proposals, then implement one coherent confirmed gap with Luna and validate
it here. Do not reopen resolved historical findings or bypass existing approval
blocks. Full external corpus and transient-case evidence remain explicit work.
