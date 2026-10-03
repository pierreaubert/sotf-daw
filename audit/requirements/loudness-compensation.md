# Loudness Compensation: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-loudness-compensation`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-loudness-compensation](../../crates/sotf-plugins/crates/sotf-plugin-loudness-compensation).
- Coordination group: **Metering/shared AutoGain**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Manual/ISO 226:2003 contour EQ, calibration and causal Pre/Post AutoGain exist; equation, feedback and rejected-calibration mutation fixes are recorded. FletcherMunson is a compatibility name.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **LOUDNESS-COMPENSATION-R1** — VERIFY/IMPROVE: Independent fitted-bank error against published contours with an explicit approximation budget and truthful applicability/extrapolation labels.
- [ ] **LOUDNESS-COMPENSATION-R2** — AUDIT: Current-edition contours, calibration workflow and remaining control parity; edition changes need explicit versioned semantics.
- [ ] **LOUDNESS-COMPENSATION-R3** — VERIFY: Mixed parameter-batch transaction behavior, saved calibration and all app/native aliases.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **LOUDNESS-COMPENSATION-A1** — Published rounded 20/40/60/80-phon SPL checkpoints and contour differences, then independently measured bank frequency response.
- [ ] **LOUDNESS-COMPENSATION-A2** — Pre/Post boosts/cuts and varying input: independent RMS/LUFS convergence, causal updates and partition equality.
- [ ] **LOUDNESS-COMPENSATION-A3** — Invalid calibration/mode batch retains state/history; never claim ISO 532 broadband loudness certification from pure-tone contour EQ.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Calibration/reference level → contour/filter preparation → Pre/Post compensation → final output and truthful meters.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-loudness-compensation` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-loudness-compensation --lib --tests
cargo clippy --offline --locked -p sotf-plugin-loudness-compensation --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-loudness-compensation`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/metering-effects.md](../../audit/metering-effects.md)
- [AUDIT.md](../../AUDIT.md)
- [crates/sotf-plugins/crates/sotf-plugin-loudness-compensation/README.md](../../crates/sotf-plugins/crates/sotf-plugin-loudness-compensation/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
