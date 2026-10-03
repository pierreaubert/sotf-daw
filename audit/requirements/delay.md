# Delay: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-delay`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-delay](../../crates/sotf-plugins/crates/sotf-plugin-delay).
- Coordination group: **Independent utility**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Fractional delay, feedback/allpass feedback, LFO and optional fixed-tap pitch-preserving switching exist; bounded finite/recursive drain contracts are documented.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **DELAY-R1** — VERIFY: Independent high-frequency fractional-delay magnitude/phase and closed-loop feedback bounds.
- [ ] **DELAY-R2** — AUDIT/IMPLEMENT: Remaining routing/modulation/timing features within the documented scope; preserve intentionally incompatible mode combinations.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **DELAY-A1** — Analytic four-point Lagrange transfer and feedback pole/gain checks; use frequencies beyond interpolation-polynomial exactness.
- [ ] **DELAY-A2** — Impulse arrival, per-route delay, LFO/transition pitch and level effects, dry/wet phase and reported latency.
- [ ] **DELAY-A3** — Zero-feedback exact tail vs recursive explicit rendering policy, reset and automated changes over callback boundaries.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Timed source/control events → Delay → latency-compensated host → finite or explicitly duration-bounded export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-delay` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-delay --lib --tests
cargo clippy --offline --locked -p sotf-plugin-delay --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-delay`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [audit/delay-convolution-finite-stream.md](../../audit/delay-convolution-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-delay/README.md](../../crates/sotf-plugins/crates/sotf-plugin-delay/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Recovery evidence (Muse 2026-10-01, unverified)

No shell in the recovery session: nothing was compiled or run, no box is
checked, and Astra review stays a coordinator gate. README ownership claim
is left to the coordinator (README table not editable by this worker).

- Scoped issue, progress, verification request, shared-integration notes,
  and full result: [audit/muse-parallel-2026-10-01/delay/](../muse-parallel-2026-10-01/delay/result.md)
  (with `issue.md`, `progress.md`, `verification-request.md`,
  `shared-integration.md` alongside).
- New tests (only production-tree change):
  [tests/audit_accuracy.rs](../../crates/sotf-plugins/crates/sotf-plugin-delay/tests/audit_accuracy.rs)
  — 11 tests. DELAY-R1/A1: HF Lagrange transfer vs defining-product oracle
  (1/44.1/48/96/192 kHz, per-sample 1e-5 + complex-gain 5e-5 bounds, >5%
  beyond-exactness guard) and closed-loop bounds (echo decay 1e-5, DC gain
  1e-3 rel, comb peak/null 1e-3 rel, allpass energy 2% + peak bound).
  DELAY-A2: dry/wet construction/cancellation/unity, stereo identity,
  bounded-range and per-channel rejection tests. DELAY-A3: automation
  partition invariance, reset-vs-fresh equality, seeded randomized
  partitions (exact). No production source modified.
- DELAY-R2 audit: documented scope fully implemented; no new feature added;
  incompatible combinations stay rejected (see `issue.md`).
- Open finding (not fixed): stale interpolation taps for fractional delays
  with integer part < 2 samples (`delay_plugin.rs:488-509`); proposed patch
  in `result.md`, needs coordinator decision + shell verification.
- Shared patch: none required (no new settings); whole-chain scenario for
  the integrator in `shared-integration.md`.
- Requested gates: `verification-request.md` (focused package tests incl.
  `--nocapture` worst-case lines, clippy `-D warnings`, `qa-delay`).

## Fix-round R1 evidence (Muse 2026-10-01, unverified)

Independent review (`audit/muse-parallel-2026-10-01/delay/review.md`,
replacing Astra at the user's request) found a test-compile blocker
(F1), the confirmed int<2 stale-tap bug (F2), a weak allpass proof
(F3), a per-channel runtime purity gap (F4), and doc drift (F5);
`validation-r1/delay.log` confirmed 6x E0689. All five are fixed in
owned files; see
[fix-r1-result.md](../muse-parallel-2026-10-01/delay/fix-r1-result.md)
for dispositions, math evidence, and files changed. No box is checked:
the coordinator reruns the gates in the updated
`verification-request.md`.

- F1: 7 float accumulators annotated `: f64` in `audit_accuracy.rs`.
- F2: `delay_plugin.rs` solves ring-unavailable taps implicitly through
  the current frame (exact with feedback incl. allpass loops; a plain
  `input` substitution would scale loop gain and destabilize high-
  feedback short delays). New oracle test (0.5/1.25/1.5 samples,
  fb 0/0.5) + DC-gain test (fb to +/-0.95). Rendering for int>=2 is
  bit-identical; no frozen output changed.
- F3: allpass echo-shape taps (0.63/0.459/-0.3213 @960/961/962),
  spread checks, and a direct-feedback discrimination control; old
  2%/2e-3 bounds kept.
- F4: per-channel runtime effect writes rejected transactionally on
  single + batch paths (pure no-ops accepted), schema marks the six
  controls, regression test incl. history retention and exact routing
  impulse. No shared patch required.
- F5: USAGE/UI/README/CHANGELOG corrected to PARAMS/LAYOUT (0.6.0
  unreleased, no version bump).

## Fix-round R2 evidence (Muse 2026-10-01, unverified)

`validation-r2/delay-test.log` ran green: 110/110 (59 lib + 14
`audit_accuracy` + 7 finite_stream + 15 integration + 10 property + 2
realtime_parameters + 2 tail_length + 1 test_basic), confirming R1
regressions and int>=2 bit-stability. The sole remaining R2 failure
was one clippy `unusual_byte_groupings` error on the `0x5EED_F1` PRNG
seed (`validation-r2/delay-clippy.log`); fixed by regrouping to
`0x005E_EDF1` (same value, bit-identical LCG stream, no
bound/oracle change). See
[fix-r2-result.md](../muse-parallel-2026-10-01/delay/fix-r2-result.md).
No box is checked: the coordinator reruns clippy plus the still-
uncaptured `--nocapture` worst-case lines and `qa-delay` via the
updated `verification-request.md`.
