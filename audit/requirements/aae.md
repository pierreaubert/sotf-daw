# Aae: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-aae`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-aae](../../crates/sotf-plugins/crates/sotf-plugin-aae).
- Coordination group: **Spatial/shared AutoGain**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Feed-forward multichannel ambience, VBAP early reflections, modulated FDN/damping, dialogue ducking and causal AutoGain exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **AAE-R1** — VERIFY: Execute recorded validation protocol for room/enhancement quality and recover matched AutoGain CPU evidence where possible.
- [ ] **AAE-R2** — AUDIT/IMPLEMENT: Reconcile remaining reflection/recursive-FDN endpoint behavior and application/native controls.
- [ ] **AAE-R3** — AUDIT: Closed-loop feedback identification/stability is not established by this feed-forward implementation; define scope before claiming or implementing installation-level parity.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **AAE-A1** — Independent reflection arrival/direction and Schroeder RT60/decay/echo-density metrics on actual plugin output.
- [ ] **AAE-A2** — Spatial diffusion, dialogue preservation and gain/phase/AutoGain partition controls; listening evidence separate from metric unit tests.
- [ ] **AAE-A3** — Bounded recursive export policy, state/reset/resource changes and hot/cold callback work; no arbitrary finite-tail claim.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Stereo feed-forward input → early reflections/FDN → multichannel enhancement output and explicitly bounded export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-aae` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-aae --lib --tests
cargo clippy --offline --locked -p sotf-plugin-aae --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-aae`; required features: `qa`.
- `qa-aae-quality`; required features: `qa`.
- `qa-aae-validation`; required features: `none`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/spatial-plugins.md](../../audit/spatial-plugins.md)
- [audit/spatial-autogain-clock-proof.md](../../audit/spatial-autogain-clock-proof.md)
- [audit/auto-gain-meter-cost.md](../../audit/auto-gain-meter-cost.md)
- [crates/sotf-plugins/crates/sotf-plugin-aae/README.md](../../crates/sotf-plugins/crates/sotf-plugin-aae/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
