# Binaural: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-binaural`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-binaural](../../crates/sotf-plugins/crates/sotf-plugin-binaural).
- Coordination group: **Spatial/shared SOFA**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

SOFA convolution/resampling, head tracking, room reflections, signed/fractional delay support and bounded lifecycle/EOF fixes exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **BINAURAL-R1** — AUDIT/IMPLEMENT: Full per-reflection HRTF/ITD treatment remains a comparison dimension versus the documented broadband-ILD approximation; establish its intended scope and implement confirmed gap.
- [ ] **BINAURAL-R2** — VERIFY: Measured-SOFA corpus, head-motion trajectories, localization/externalization and native/device routes.
- [ ] **BINAURAL-R3** — INTEGRATE: Persist geometry, SOFA/resource identity and room modes with transactional asynchronous changes.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **BINAURAL-A1** — Independent four-path/direct convolution, physical-time IR resampling and signed/fractional delay phase/arrival.
- [ ] **BINAURAL-A2** — Known head trajectories and reflection geometries: continuity, angular interpolation, ITD/ILD and stale generation refusal.
- [ ] **BINAURAL-A3** — Long tails, late reflections, bypass/reset, failure preserving prior audio and cold callback reclamation checks.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Speaker layout + SOFA/head pose/room → async prepared binaural filters → stereo host/headphone output and EOF.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-binaural` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-binaural --lib --tests
cargo clippy --offline --locked -p sotf-plugin-binaural --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-binaural`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/spatial-plugins.md](../../audit/spatial-plugins.md)
- [audit/sofa-fractional-delay.md](../../audit/sofa-fractional-delay.md)
- [audit/sofa-resampling.md](../../audit/sofa-resampling.md)
- [audit/spatial-drain-work-bounds.md](../../audit/spatial-drain-work-bounds.md)
- [crates/sotf-plugins/crates/sotf-plugin-binaural/README.md](../../crates/sotf-plugins/crates/sotf-plugin-binaural/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
