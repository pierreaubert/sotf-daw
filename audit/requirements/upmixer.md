# Upmixer: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-upmixer`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-upmixer](../../crates/sotf-plugins/crates/sotf-plugin-upmixer).
- Coordination group: **Spatial/shared geometry/inference**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Direct/ambient decomposition, VBAP, decorrelation, height/transient controls, multi-source and dual-resolution paths exist. Small FFT and above-512 HR timing corrections are accepted in bounded scopes.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **UPMIXER-R1** — VERIFY/IMPROVE: Independent dialogue placement, spatial stability, downmix compatibility and held-out vocal-model accuracy.
- [ ] **UPMIXER-R2** — INTEGRATE: Remaining custom/native/device routes and full mode/state/latency handling.
- [ ] **UPMIXER-R3** — VERIFY: Golden-dependent gates must explicitly require assets or be marked unavailable; absent files must not silently pass.
- [ ] **UPMIXER-R4** — AUDIT: Resolve genuine remaining feature gaps after current primary-source comparison; preserve accepted timing fixes and raw-mode compatibility.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **UPMIXER-A1** — Independent coherent/antiphase/quadrature/diffuse multichannel matrix and known-direction signals through supported layouts.
- [ ] **UPMIXER-A2** — Source-time alignment across low/high resolution, FFT64/128 and >=512 paths, attacks and complete streams.
- [ ] **UPMIXER-A3** — Measured dialogue leakage, downmix error and vocal inference on held-out material; matched CPU where reproducible baseline exists.
- [ ] **UPMIXER-A4** — AutoGain partition/causality, resource publication, reset/refusal and native callback heap bounds.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Stereo program/model → Upmixer → named surround/height layout → device/downmix comparison and complete EOF.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-upmixer` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-upmixer --lib --tests
cargo clippy --offline --locked -p sotf-plugin-upmixer --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-upmixer`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/spatial-plugins.md](../../audit/spatial-plugins.md)
- [audit/upmixer-small-fft.md](../../audit/upmixer-small-fft.md)
- [audit/spatial-autogain-clock-proof.md](../../audit/spatial-autogain-clock-proof.md)
- [audit/spatial-drain-work-bounds.md](../../audit/spatial-drain-work-bounds.md)
- [audit/IMPLEMENTATION_PLAN.md](../../audit/IMPLEMENTATION_PLAN.md)
- [crates/sotf-plugins/crates/sotf-plugin-upmixer/README.md](../../crates/sotf-plugins/crates/sotf-plugin-upmixer/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
