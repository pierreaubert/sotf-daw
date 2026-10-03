# Limiter: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse limiter — muse-spark-1.3-contributor, max effort**.

- Package: `sotf-plugin-limiter`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-limiter](../../crates/sotf-plugins/crates/sotf-plugin-limiter).
- Coordination group: **Dynamics/shared oversampling**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Predictive final-output protection and actual 2x/4x audio oversampling exist with independent reconstructed true-peak, compatibility and heap evidence.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **LIMITER-R1** — VERIFY: Complete native automation, preset/consumer and wider signal/rate/transition coverage without replacing the existing emitted-output ceiling tests.
- [ ] **LIMITER-R2** — AUDIT/IMPLEMENT: Remaining release/channel/metering features against current primary references; preserve established core behavior and choice mappings.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **LIMITER-A1** — Independent oversampled reconstruction of final emitted audio after nonlinear gain and downsampling; retain existing +0.1 dB ceiling tolerance cases.
- [ ] **LIMITER-A2** — Burst/two-tone/phase/lookahead/release/rate matrix, linked and unlinked channels and threshold automation.
- [ ] **LIMITER-A3** — Native low-level bitwise controls after documented delay; correct dry latency, all EOF suffixes and cold callback bounds.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Engine/native control → audio oversampling → limiter → downsampling/final protection → emitted output/export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-limiter` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-limiter --lib --tests
cargo clippy --offline --locked -p sotf-plugin-limiter --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-limiter`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Muse max review; separate Muse max implementation session fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/limiter-oversampling-accuracy.md](../../audit/limiter-oversampling-accuracy.md)
- [audit/limiter-oversampling-implementation.md](../../audit/limiter-oversampling-implementation.md)
- [audit/limiter-oversampling-propagation.md](../../audit/limiter-oversampling-propagation.md)
- [audit/limiter-engine-controls.md](../../audit/limiter-engine-controls.md)
- [crates/sotf-plugins/crates/sotf-plugin-limiter/README.md](../../crates/sotf-plugins/crates/sotf-plugin-limiter/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
