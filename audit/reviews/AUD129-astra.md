# AUD129 independent validation — Upmixer minimum FFT geometry

Validator: Astra, medium. Implementer: dedicated Upmixer Luna, xhigh.
Status: **accepted for minimum FFT geometry and HR ring capacity** (2026-09-28).

## Proposal review

Reviewed `audit/proposals/upmixer-minimum-fft.md` and the actual constructor,
factory, WOLA window and streaming shift. The current power-of-two precondition
admits one; the factory also maps zero/one to one. `hop_size = fft_size / 2`
then gives zero, and the full-frame processing loop's shift cannot consume
input. This is a source-derived progress defect; no hanging process was run.

The proposed minimum two is mathematically valid for neutral WOLA: its periodic
sqrt-Hann window is `[0, 1]`, whose squared overlapping windows at hop one sum
to unity. This establishes geometry, not useful spatial-frequency resolution
or full product quality at a two-sample transform. Direct construction can
retain its existing panic precondition; the parameter factory can preserve
upward rounding while mapping values below two to two.

## Required refinements sent to Luna

- Document geometry versus useful spectral/spatial quality clearly.
- Verify direct bounds zero/one and factory zero/one/two/non-power three;
  preserve the low-latency override to 1024. Do not process the defective N=1.
- Test N=2 DC/Nyquist behavior and window phases with an independent neutral
  delayed-impulse/dense or DC/Nyquist oracle, declared latency, frame counts
  and complete EOS. Finite-output checks alone are insufficient.
- Inspect HR-direct's separate 512-point transform when the main transform is
  smaller, and causal AutoGain assumptions. Include representative HR on/off,
  AutoGain on/off and surround/height public routes; track any independently
  discovered limitation instead of claiming all feature combinations work.
- Retain AUD118's N=64/128 tests and existing AutoGain/EOS contracts. Run focused
  tests, fresh-thread heap checks and strict lint through the shared Cargo queue.
- Record source hashes and exact gate snapshots. Metering Luna owns the shared
  ledger; the Upmixer worker owns its plugin and unique proposal/report only.

No design blocker remains once these acceptance refinements are incorporated.
Production and executed evidence still require independent review.

## First implementation review

The worker reproduced an additional HR ring-capacity defect: at main N=32,
the old four-main-FFT ring holds 128 frames, less than a 256-frame HR hop.
The fix prepares `4 * max(main_fft_size, hr_fft_size)` frames at construction,
layout reconfiguration and FFT resize. Inspected all three sites: the rule is
power-of-two and covers the complete 512-frame HR block as well as its hop;
ordinary main sizes at least 512 retain the old capacity. Bounds/factory logic
and this preparation-only correction have no identified source blocker.

Reviewed the every-phase neutral impulse oracle (N=2/4/8/16/32/64/512) and the
N=2 independent delayed DC/Nyquist/dense signal with exact EOS frame count.
These run actual forward/inverse transforms and WOLA with neutralized routing,
using a 2e-6 absolute error limit. Fresh-thread small-size heap tests process
512 HR-enabled frames before and after reset, asserting zero alloc/free.

**Requested evidence refinement:** public N=2–16 route fixtures currently use
`n * 8 + 13` frames, less than one HR hop; all N=2–32 fixtures are shorter than
the AutoGain measurement update. Extend meaningful public HR/AutoGain/reset
fixtures beyond both clocks (for example sample_rate/10 + 512 frames). Toggle
flags alone do not establish those routes executed. The separate 512-frame
heap test does exercise HR buffer capacity, but does not replace active public
route equivalence checks.

**Requested timing disposition:** inspect/measure the fixed 512-point HR path
against main transforms below 512. The existing alignment delay saturates to
zero in that range. Supply an independent neutral HR-only impulse timing probe
and record any separate alignment limitation rather than inferring alignment
from finite output and reset equivalence. A distinct confirmed issue can remain
separate from this bounded geometry/capacity fix if explicitly tracked.

Inspected green focused/package/lint logs and matching pre-format package
start/end manifests. Final formatted-source gates and the requested refinements
remain pending. No overlapping Cargo job was started by the validator.

## Second implementation review

The revised public matrix now extends beyond the AutoGain update interval and
the HR transform (`sample_rate / 10 + 512 + N` frames), retaining both modes,
layouts, HR/AutoGain toggles and reset/fresh comparisons. This addresses the
short-fixture finding. Final package evidence records 163 passing tests and
strict all-target Clippy success.

The new internal HR impulse probe does not yet settle scheduler alignment:
it calls two `process_hr_block` operations directly before `mix_hr_output`,
bypassing real-time HR readiness relative to main output. Its frame-zero ring
peak and main API peak at N use different clocks. Requested a synchronized
stream-path probe with a nonzero HR contribution, callback partition coverage
and finite EOS. Also requested an amplitude assertion so an all-zero internal
probe cannot pass solely through its argmax. A confirmed separate timing defect
may be tracked separately from the minimum-geometry/capacity correction.

The bounded production correction still has no identified source blocker;
final acceptance awaits this concrete timing disposition. No validator Cargo
job was started.

## Synchronized HR follow-up

Reviewed the revised stream-path probe: matched neutral plugins retain identical
HR-enabled main transient state; the reference only clears cached HR channel
writes. Subtracting their outputs isolates a nonzero HR contribution. Both
irregular and 512-frame callbacks run 1024 input frames plus finite EOS. The
reported contribution arrives at frame 512, while the main impulse peaks at
the declared N=2/32 latency: respective offsets 510/480 frames. This is concrete
scheduler-level defect evidence, unlike the earlier internal-ring comparison.

Requested executed final evidence and separate issue tracking through the
shared ledger owner. No production alignment fix belongs to this bounded
geometry/capacity patch. Its acceptance can proceed once that distinct
limitation is recorded and final source gates are verified.

## Final scoped acceptance

Accepted AUD129's constructor/factory minimum and prepared HR ring-capacity
corrections. Verified final package log: 164 passed, zero failures; strict
all-target Clippy passed. The worker reports package format check passed.
Start/end manifests both hash to
`d0ef11f8e20a005c3483bdefee80b9dea3cb945c498938c64e15c5fb18f06835`;
independent `sha256sum --check --quiet` verified current files against that
manifest. Evidence is package-scoped, not a new whole-workspace result.

The final named characterization log confirms nonzero HR contributions
(0.0005751852 at N=2 and 0.0033707132 at N=32), first/peak frame 512 under both
callback partitions. AUD130 now records the separate observed offset, with
intended timing and severity still requiring investigation. This acceptance
does not establish HR alignment, full spatial quality at tiny transforms,
or CPU cost. No production alignment correction was made.

Reviewed report: `audit/upmixer-minimum-fft.md`. Executed evidence logs:
`/tmp/sotf-aud129-upmixer-package-accepted.log`,
`/tmp/sotf-aud129-upmixer-clippy-accepted.log`, and
`/tmp/sotf-aud129-aud130-hr-probe-final.log`.
The validator reviewed source, independent numerical expectations, manifests
and execution logs; no duplicate Cargo job was run.
