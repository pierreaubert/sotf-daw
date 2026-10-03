# Spectral Compressor: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-spectral-compressor`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-spectral-compressor](../../crates/sotf-plugins/crates/sotf-plugin-spectral-compressor).
- Coordination group: **Spectral dynamics/shared DSP**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Per-bin dynamics, linked channels, delayed dry path and corrected startup/finite-stream synthesis exist. Boundary detector calibration needs explicit interpretation.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **SPECTRAL-COMPRESSOR-R1** — VERIFY/RESOLVE: Define and test absolute DC/Nyquist/near-boundary detector convention and mask timing; implement corrections only against the declared convention.
- [ ] **SPECTRAL-COMPRESSOR-R2** — AUDIT/IMPLEMENT: Remaining spectral shaping/detector controls and integration gaps; coordinate reusable spectral-band behavior with EQ/Dynamic EQ.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **SPECTRAL-COMPRESSOR-A1** — Independent Parseval/window normalization and absolute gain-reduction oracle for coherent/off-bin tones, DC and Nyquist with multiple FFT sizes/phases.
- [ ] **SPECTRAL-COMPRESSOR-A2** — Unity and nonuniform masks: independent complete WOLA output including startup and EOF; no amplitude fitting to conceal normalization errors.
- [ ] **SPECTRAL-COMPRESSOR-A3** — Attack/release, linking, mix alignment, rate/FFT transitions and callback partition invariance.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Signal → FFT detector/mask → overlap-add → aligned dry mix → downstream host/output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-spectral-compressor` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-spectral-compressor --lib --tests
cargo clippy --offline --locked -p sotf-plugin-spectral-compressor --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-spectral-compressor`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/spectral-boundary-design.md](../../audit/spectral-boundary-design.md)
- [audit/declick-spectral-finite-stream.md](../../audit/declick-spectral-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-spectral-compressor/README.md](../../crates/sotf-plugins/crates/sotf-plugin-spectral-compressor/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
