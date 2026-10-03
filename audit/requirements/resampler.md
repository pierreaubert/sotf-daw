# Resampler: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse `resampler`** (exclusive: plugin crate, this file, `audit/muse-parallel-2026-10-01/resampler/`; README-table claim deferred to the coordinator to avoid a concurrent shared-file edit).

- Package: `sotf-plugin-resampler`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-resampler](../../crates/sotf-plugins/crates/sotf-plugin-resampler).
- Coordination group: **Clock/host integration**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Fixed/dynamic sinc conversion, prepared cutoff tables, relative-ratio correction and exact finite draining exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **RESAMPLER-R1** — IMPLEMENT: Smooth cutoff/ratio-table transitions without callback allocation or a transient alias burst.
- [ ] **RESAMPLER-R2** — VERIFY/IMPROVE: Quantify narrow transition-band rejection, passband ripple and group delay; improve the filter where declared quality targets are not met.
- [ ] **RESAMPLER-R3** — INTEGRATE: Preserve output sample clock, exact produced lengths and latency through adapters, branching and export; coordinate host queue work separately.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **RESAMPLER-A1** — Independent high-precision resampling/analytic tone oracles for rational/irrational ratios, above-new-Nyquist signals and near-cutoff sweeps.
- [ ] **RESAMPLER-A2** — Changing ratios across callbacks: exact time/count accounting, bounded transition artifacts, partition/reset and finite EOF.
- [ ] **RESAMPLER-A3** — Measure each quality mode and publish its ripple/rejection/CPU tradeoff; equal output duration alone is not accuracy evidence.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Source clock → resampler → downstream filter/host → export header and waveform; mixed-rate branch integration belongs to SHARED.md.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-resampler` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-resampler --lib --tests
cargo clippy --offline --locked -p sotf-plugin-resampler --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-resampler`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [AUDIT.md](../../AUDIT.md)
- [audit/engine-drivers.md](../../audit/engine-drivers.md)
- [audit/abcompare-variable-rate.md](../../audit/abcompare-variable-rate.md)
- [crates/sotf-plugins/crates/sotf-plugin-resampler/README.md](../../crates/sotf-plugins/crates/sotf-plugin-resampler/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Muse implementation status (2026-10-01, unverified — coordinator runs gates)

No shell in the owner session; nothing below was executed by the owner.
Boxes stay unchecked until the coordinator records passing gates. Detail:
[audit/muse-parallel-2026-10-01/resampler/result.md](../muse-parallel-2026-10-01/resampler/result.md),
[verification-request.md](../muse-parallel-2026-10-01/resampler/verification-request.md),
[issue.md](../muse-parallel-2026-10-01/resampler/issue.md).

- **RESAMPLER-R1** (implemented, pending run): opt-in `cutoff_smoothing`
  (default off, Realtime, appended 4th parameter + ParamSpec); asymmetric
  slew in `src/cutoff_bank.rs` (upward one table per backend chunk,
  downward immediate); flag persists across reset/rebuild; changes reject
  after finalization. Bit-exact legacy default audio. Tests:
  `src/tests.rs` slot-trajectory units, `tests/smooth_cutoff.rs` (counts,
  clocks, drain bounds, downward bit-exactness, upward acts-on-audio,
  reconvergence, partition invariance, allocation-free, parameter
  contract), QA Test 7.
- **RESAMPLER-R2** (implemented, pending run): transition width (−3/−60 dB
  span as a fraction of output Nyquist), passband ripple, and stopband
  quality defined and quantified per preset against an independent f64
  analytic DTFT (`tests/filter_response.rs`, `RESAMPLER-FILTER` lines);
  measured-vs-analytic agreement on a 48→24 kHz sweep; no
  existing-preset coefficient change (declared targets are met by current
  tests; near-Nyquist large-decimation weakness stays a documented,
  quantified limitation rather than a relabeled guarantee).
- **RESAMPLER-R3** (in-plugin parts implemented, pending run; branch/export
  shared): count/clock/latency preservation under smoothing covered by
  the new suites; factory/bridge/FFI/native/UI trace with concrete
  shared-owner patches in the owned audit directory; AUD087 mixed-rate
  branching stays with the shared clock/host owner.
- **RESAMPLER-A1** (implemented, pending run): analytic DTFT oracle
  (integer-step ratios), f64 direct-sinc end-to-end oracle (44.1↔48 kHz),
  above-new-Nyquist and near-cutoff sweep points (asserted where
  pre-declared bounds exist, report-only beyond flat-band claims).
- **RESAMPLER-A2** (implemented, pending run): changing-ratio count/clock
  preservation, bounded-transition evidence (HF gradual-release + LF
  undisturbed), partition/reset/finite-EOF coverage in
  `tests/smooth_cutoff.rs`; existing `dynamic_endpoint.rs` matrix
  untouched.
- **RESAMPLER-A3** (implemented, pending run): per-preset ripple/rejection
  tables plus per-quality QA CPU lines; duration equality never used as
  accuracy evidence.
- Whole-chain save/reload execution through shared engine/factory/FFI and
  the independent review remain open; shared patches list precise steps.

## Fix-r5 status (2026-10-01, unverified — coordinator runs gates)

Validation-r5: `resampler-test.log` exit 0 (all lib + integration tests
pass, with one dead-code warning); `resampler-clippy.log` exit 101 with
one real `dead-code` diagnostic (test-only `CutoffBank` helpers compiled
into the non-test lib). Fix-r5 gates those four helpers with `#[cfg(test)]`
(no behavior, numeric, stimulus, tolerance, or default-audio change) and
applies documentation-only reviewer dispositions (slew-contract wording,
oracle-independence disclosure, allocation-claim scoping, shared-validation
correction). Findings requiring new tests, derived bounds, or expanded
coverage (review P1-1, P1-3 test arm, P1-4, P1-5, P2-8; bound provenance
P1-1/P1-2) stay open, and broader quality requirements remain open unless
proved by execution. Boxes stay unchecked. Detail:
[fix-r5-result.md](../muse-parallel-2026-10-01/resampler/fix-r5-result.md).

## Fix-r6 status (2026-10-01, unverified — coordinator runs gates)

Validation-r5 follow-through: `resampler-clippy-after-fix.log` still exit
101 with one real `redundant_closure` diagnostic
(`tests/smooth_cutoff.rs:528`). Fix-r6 replaces that closure with
`f32::from_bits` (no numeric change) and IMPLEMENTS the deferred reviewer
arms with additive stronger tests (existing tests, stimuli, tolerances,
and default audio untouched): P1-1 analytic HF derivation + 0.1 dB LF +
-50 dB coherent residual; P1-2 a-priori error budgets for 0.5 dB / 2e-6 /
0.05 dB; P1-3 asserted 96->24 near-cutoff sweep; P1-4 nominal-0.5/2.0
bank units, nominal-not-1, instant-upward, Fast/Medium downward, 8 ch,
96 kHz; P1-5 preemption, drain-mid-slew audio, per-block counts, latency
equality; P2-8 unified 9..13 window plus full-trajectory checks; P1-6
allocation-free typed refusal (`try_set_*` + `ResamplerControlError`,
String API delegates identically). Boxes stay unchecked. Detail:
[fix-r6-result.md](../muse-parallel-2026-10-01/resampler/fix-r6-result.md).

## Fix-r7 status (2026-10-01, unverified — coordinator runs gates)

Actual validation-r6: full owned tests exit 101 (all others pass,
including hard 96->24, all 8 slew-coverage, both typed-control),
clippy exit 0. Two NEW `slew_artifact_bounds` failures are corrected as
mathematically wrong oracles with independent derivations plus
regression (same bounds, no production change, no weakening): the
coherent fit becomes the true 2x2 least-squares solution for the
non-coherent transition window (same -50 dB bound; old `2/N` bias
`1/(2 sin w)` ~-42.5 dB matches the observed BOTH-modes -42.6 dB), and
the trajectory check becomes floor-aware with a linear floor from the
established 2e-6 alias bound (-108 dB power; same 1 dB above-floor
monotonic, same per-block 9..17 coverage, plus a 20 dB above-floor
measurability assert). Added cumulative-clock uniformity, fixed-2.0
control-leg, and injected-spur regression distinguishing true artifacts
from intended rate/chirp modulation. Boxes stay unchecked. Detail:
[fix-r7-result.md](../muse-parallel-2026-10-01/resampler/fix-r7-result.md).

## Fix-r8 status (2026-10-01, unverified — coordinator runs gates)

Actual validation-r7: owned test AND clippy abort at
`tests/slew_artifact_bounds.rs:703` with E0689 (`max_step_error`
ambiguous float: `.max` exists on both `f32` and `f64`). Fix-r8 is one
explicit type suffix, test-only, with no other change:
`tests/slew_artifact_bounds.rs:697`
`let mut max_step_error = 0.0_f64;` (ground-truth clock is `f64`).
All fix-r7 independent regression oracles (cumulative clock, fixed-2.0
control, injected-spur sensitivity) and every accuracy bound are
retained verbatim; no production-source change. Full-file rescan finds
no other E0689-class ambiguity or new clippy risk. Boxes stay
unchecked. Detail:
[fix-r8-result.md](../muse-parallel-2026-10-01/resampler/fix-r8-result.md).

## Fix-r9 status (2026-10-01, unverified — coordinator runs gates)

Terminal `review-after-r8` (CHANGES REQUIRED, raw-verified after-r8 logs:
2 `slew_artifact_bounds` failures + 1 clippy error) is fixed test-only
with independent derivations plus regression, no production-source
change, no bound weakened, no case removed. F1: trajectory guard uses the
dB check whenever current is measurable, linear bound only when current
is below the floor (passes the legitimate -108.07 to -59.81 dB release;
large above-to-below drops still fail; synthetic sensitivity proof).
F2: false `gain_old.abs() > 0.02` replaced by a 4-phase phase-dependence
proof (spread >0.02 dB, true-LS clean, coherent-only residual >-50 dB);
residual discriminator and -60 dB spur proof kept. F3:
`needless_range_loop` fixed via `enumerate().skip(9).take(4)`. F4:
cumulative clock connected to genuine production (lengths equal for all
29 blocks including the ramp) plus dual-ideal audio fits (clock-derived
and independent uniform, -50 dB, gains within 0.02 dB) with 1% wrong-rate
fault sensitivity; SlewClock/Clock non-independence disclosed. Boxes stay
unchecked. Detail:
[fix-r9-result.md](../muse-parallel-2026-10-01/resampler/fix-r9-result.md).
