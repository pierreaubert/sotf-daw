# Multiband Expander: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-multiband-expander`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-multiband-expander](../../crates/sotf-plugins/crates/sotf-plugin-multiband-expander).
- Coordination group: **Shared expander implementation**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Broadband/multiband time and spectral expansion share this crate. Conventional ratio law, centered knee and spectral stream/drain corrections exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **MULTIBAND-EXPANDER-R1** — AUDIT/IMPLEMENT: Compare remaining per-band detection, linking, sidechain and expansion-mode capabilities; implement confirmed gaps.
- [ ] **MULTIBAND-EXPANDER-R2** — VERIFY: Complete phase/recombination and absolute spectral gain-law evidence plus all consumers.
- [ ] **MULTIBAND-EXPANDER-R3** — COORDINATE: Same implementation as Expander; avoid separate agents editing the same DSP modules.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **MULTIBAND-EXPANDER-A1** — Independent downward/upward laws as supported, soft-knee continuity, range floor and timed hold/release.
- [ ] **MULTIBAND-EXPANDER-A2** — Spectral DC/interior/Nyquist detector calibration and current/previous-mask timing, actual final output gain and all hop phases.
- [ ] **MULTIBAND-EXPANDER-A3** — Full multiband reconstruction, dry latency, cold lifecycle heap checks and finite cached continuation.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Typed broadband/multiband settings → time/spectral engine → recombination → host/export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-multiband-expander` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-multiband-expander --lib --tests
cargo clippy --offline --locked -p sotf-plugin-multiband-expander --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-multiband-expander`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/multiband-expander-soft-knee.md](../../audit/multiband-expander-soft-knee.md)
- [audit/multiband-expander-spectral-finite-stream.md](../../audit/multiband-expander-spectral-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-multiband-expander/README.md](../../crates/sotf-plugins/crates/sotf-plugin-multiband-expander/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
