# Transient Shaper: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-transient-shaper`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-transient-shaper](../../crates/sotf-plugins/crates/sotf-plugin-transient-shaper).
- Coordination group: **Dynamics**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Linked attack/sustain envelope processing, sensitivity, smoothing, neutral behavior and zero-audio-tail support exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **TRANSIENT-SHAPER-R1** — VERIFY: Establish independent two-envelope attack/sustain accuracy and level/rate behavior.
- [ ] **TRANSIENT-SHAPER-R2** — AUDIT/IMPLEMENT: Confirm any remaining control/sidechain/channel requirements using the intended model; do not claim exact hardware equivalence from an SPL-inspired design.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **TRANSIENT-SHAPER-A1** — Independent f64 envelope recurrences for impulses, attacks, exponential decays and sustained plateaus.
- [ ] **TRANSIENT-SHAPER-A2** — Scale invariance above sensitivity, gain clamps and distortion, channel link and callback partitions.
- [ ] **TRANSIENT-SHAPER-A3** — Neutral overrange transparency, zero continuation while envelopes remain active, preset/automation timing.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Transient program → Shaper envelope/gain stages → actual engine/native output, including bypass and automation.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-transient-shaper` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-transient-shaper --lib --tests
cargo clippy --offline --locked -p sotf-plugin-transient-shaper --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-transient-shaper`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/zero-audio-tail-support.md](../../audit/zero-audio-tail-support.md)
- [crates/sotf-plugins/crates/sotf-plugin-transient-shaper/README.md](../../crates/sotf-plugins/crates/sotf-plugin-transient-shaper/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
