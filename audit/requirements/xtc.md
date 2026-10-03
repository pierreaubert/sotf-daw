# Xtc: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-xtc`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-xtc](../../crates/sotf-plugins/crates/sotf-plugin-xtc).
- Coordination group: **Spatial/shared SOFA**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Regularized transaural inversion, geometry/SOFA, RoomEQ filters, gain limits, aligned bypass and staged initialization/drain corrections exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **XTC-R1** — VERIFY/IMPROVE: Cancellation against independent measured ear transfer functions, not the same model used for filter design.
- [ ] **XTC-R2** — INTEGRATE: Complete head/geometry/resource and RoomEQ settings through consumers with failure-safe preparation.
- [ ] **XTC-R3** — AUDIT: Remaining robustness/control capabilities against declared scope; document spatial sweet-spot limits.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **XTC-A1** — Independent 2×2 inversion and full emitted response against separate measured transfer functions.
- [ ] **XTC-A2** — Head-position/room-error grids, gain amplification, ear cancellation, ITD/ILD and limiter tradeoffs.
- [ ] **XTC-A3** — Aligned bypass/AutoGain, resource transitions, physical SOFA delays/resampling and complete finite-generation EOF.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Room/SOFA/geometry → inversion/preparation → XTC → independent speaker-to-ear response → measured ear output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-xtc` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-xtc --lib --tests
cargo clippy --offline --locked -p sotf-plugin-xtc --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-xtc`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/xtc-staged-initialize.md](../../audit/xtc-staged-initialize.md)
- [audit/xtc-aligned-bypass.md](../../audit/xtc-aligned-bypass.md)
- [audit/xtc-finite-generation.md](../../audit/xtc-finite-generation.md)
- [audit/xtc-autogain-clock.md](../../audit/xtc-autogain-clock.md)
- [audit/spatial-plugins.md](../../audit/spatial-plugins.md)
- [crates/sotf-plugins/crates/sotf-plugin-xtc/README.md](../../crates/sotf-plugins/crates/sotf-plugin-xtc/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
