# AUD104: XTC AutoGain sample clock proposal

2026-09-28. Public reproduction complete; production changes are not yet made.
Scope is XTC scheduling/measurement, tests and documentation. No host AutoGain
arithmetic or public parameter changes are proposed.

## Reproduction

`/tmp/sotf-xtc-autogain-clock-probe.rs` renders the same six-second stepped-level,
three-tone stereo program through real XTC filters: 48/96 kHz, N128/2048,
AutoGain off/on, and callback patterns [1], [512], [8193,137] (24 renders).
The disabled-AutoGain controls are bit-identical across all partitions.
With AutoGain enabled, the 48 kHz/N2048 [512] render differs from [1] by
0.027620405 peak amplitude, with 13.70238 dB signal/error ratio. All mixed large
callbacks finish with nonfinite cached loudness and zero compensation because
every tenth callback samples only a small fraction of program duration.

Log: `/tmp/sotf-xtc-autogain-clock-probe.log`. Build arguments:
`/tmp/sotf-xtc-autogain-clock-build.txt`; executable under `target/audit-tmp`.
The first build used mismatched feature artifacts and failed to link; the
matching existing host/XTC artifacts were used for the successful run.

Source `process_audio` increments a callback counter, ingests input/output only
on the tenth call, refreshes the target, then applies that target retroactively
to the complete caller block. The monitor thus sees a discontinuous, shortened
waveform. Additionally, current input is compared with N-frame delayed output.
The latter needs an independent delayed-identity probe before including its
correction in this issue.

## Proposed invariants

- Both loudness monitors ingest every relevant stereo frame. Input measurement
  should use the original signal delayed by the declared N-frame output clock,
  so neutral delayed identity has no level correction even through level steps.
  Prove the mismatch before production work.
- Refresh derived statistics and the compensation target every
  `max(1, floor(sample_rate/10))` emitted frames. A refresh after boundary frame
  k affects frame k+1 onward; no future block samples alter earlier output.
- Keep the existing AutoGain helper and asymmetric gain recurrence. Calling its
  compensation method with one frame makes its near-unity shortcut and smoother
  bookkeeping independent of external callback boundaries. Do not substitute
  the helper's different `next_gain_linear` recurrence without a separate proof.
- Preserve uncompensated wet-output measurement, limiter after compensation,
  denormal handling before dry/wet blend, warm wet state while disabled, and
  current extra-output behavior (AutoGain only exists for stereo output).
- No added latency, parameter, host protocol, filter arithmetic, async publication
  policy or new callback allocation/free. AutoGain-off waveform remains exact.

## Prepared scheduling approach for review

Factor the existing raw STFT emission into a private segment helper. Bound each
AutoGain-active segment by the next measurement boundary and the contiguous
remaining stereo dry-ring region. Segment length is at most N; before advancing
that ring, its current span is precisely the N-delayed input reference for the
same output span. Feed the existing span directly to AutoGain with no new ring
or scratch allocation. Then ingest the raw wet output, apply the existing gain
method one frame at a time, run the existing limiter, flush wet denormals, and
advance/blend the existing dry ring. Refresh measurement/cache only after the
segment reaches its fixed sample boundary. The no-AutoGain path may retain its
existing whole-block fast path using the same factored arithmetic.

Use explicit frame phase in XTC diagnostics/dynamics instead of a callback
counter. Reset, successful reinitialize and fresh AutoGain construction reset
this phase. Failed initialization/preflight and zero-frame calls preserve it.
Scalar max/smoothing changes do not restart it. Unchanged bool snapshots remain
no-ops. Inspect all creation/enable/disable paths before finalizing this rule.

Ordinary and EOF audio use the same sample clock. Existing canonical-hop drain
cache and finite support remain valid: metering, smoothing and limiter states
cannot synthesize audio after finite wet/dry histories are empty. Partial drain
capacity must not affect the sample phase or frozen controls.

## Required verification

1. Permanent public red partition probe, including irregular/one-frame/oversized
   calls and an AutoGain-off negative control.
2. Independent neutral `[N zeros] + input` oracle with AutoGain enabled across
   level steps, startup and silence; separate fixed diagonal gain matrix tests
   verify expected settled compensation from its analytic amplitude ratio.
3. Exact boundary causality: construct programs identical through a boundary and
   different afterward; prior emitted samples must remain equal. Test final
   frame before/at/after refresh, same-position scalar changes and reversals.
4. Compare meter input/output timing with an independently constructed delayed
   reference using public loudness measurement, separate from the production
   scheduler. Preserve the existing gain-law tolerance and explain FFT error.
5. Ordinary process plus drain versus complete explicit-zero continuation with
   arbitrary process/drain partitions, active gain/limiter/bypass ramps, reset,
   initialization failures and unchanged finite metadata/call-bound checks.
6. Fresh-thread allocation AND deallocation checks through first measurement,
   publication, active gain, resets and EOF. Full XTC tests, strict Clippy,
   release QA and matched-input active-AutoGain CPU measurement. The formerly
   decimated meter now performs more work; quantify it before calling complete.

This intentionally changes AutoGain-enabled waveform to remove caller-size
selection and align measurement clocks. It does not preserve AUD099's captured
all-enabled AutoGain waveform; that prior checkpoint remains historical proof
for its separately scoped bypass correction.

## Delayed-identity evidence

A second public probe uses `bypass_xtc_filters=true` and the independent exact
`[N zeros]+source` reference; all other settings, programs and partitions are
the same. With AutoGain off, peak error is at most 2.24e-8. With AutoGain on,
N2048 at 96 kHz produces 0.000536107 error for one-frame calls and 0.000714958
for 512-frame calls. Thus neutral delay alone creates a measurable gain error.
The reference comparison, source ordering and known N delay support aligning
input/output measurement in this same correction.
Artifacts: `/tmp/sotf-xtc-autogain-identity-probe.rs`, `.log`, and
`/tmp/sotf-xtc-autogain-identity-build.txt`. Production remains unchanged.
