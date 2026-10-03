# Matrix: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-matrix`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-matrix](../../crates/sotf-plugins/crates/sotf-plugin-matrix).
- Coordination group: **Routing/shared geometry**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Dense/sparse signed routing, polarity, master/output gain/mute/solo/dim and smoothing exist; no concrete missing advertised behavior was found.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **MATRIX-R1** — VERIFY: High-dynamic-range cancellation precision and complete structural layout/state integration.
- [ ] **MATRIX-R2** — AUDIT: Confirm routing/control parity within Matrix scope; do not silently turn it into a separate spatial renderer.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **MATRIX-A1** — Independent f64 matrix multiplication over dense/sparse and rectangular shapes, opposite-sign cancellation and unused channels.
- [ ] **MATRIX-A2** — Closed-form polarity/gain ramps, rejected dimensions, exact channel order and full output overwrite.
- [ ] **MATRIX-A3** — Restored matrix state, compiled/native paths and channel-changing host EOF/latency.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Persisted rectangular matrix → negotiated input/output channels → downstream processor/output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-matrix` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-matrix --lib --tests
cargo clippy --offline --locked -p sotf-plugin-matrix --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-matrix`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [audit/channel-changing-host-eof.md](../../audit/channel-changing-host-eof.md)
- [crates/sotf-plugins/crates/sotf-plugin-matrix/README.md](../../crates/sotf-plugins/crates/sotf-plugin-matrix/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
