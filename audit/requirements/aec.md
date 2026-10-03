# Aec: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-aec`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-aec](../../crates/sotf-plugins/crates/sotf-plugin-aec).
- Coordination group: **Spatial/adaptive clock**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Two-input microphone/reference PBFDAF, foreground/background filters, double-talk behavior and post-filter exist. Learning-rate, process-clock and finite-tail corrections are documented.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **AEC-R1** — AUDIT/IMPLEMENT: Reference-clock drift tracking and independent delay alignment beyond the adaptive tail are outstanding comparison dimensions; specify and implement required supported behavior.
- [ ] **AEC-R2** — VERIFY/IMPROVE: Real speech/music, nonlinear echo devices and changing rooms with independent quality data.
- [ ] **AEC-R3** — INTEGRATE: Preserve reference/program ordering, adaptive settings and error/refusal behavior through actual host routes.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **AEC-A1** — Known independent echo paths: ERLE/convergence time and residual waveform, plus near-end preservation during double talk.
- [ ] **AEC-A2** — Clock offset/drift and delay jumps, foreground/background handover, nonlinear loudspeaker stress and path changes.
- [ ] **AEC-A3** — Absolute learning-rate behavior, wrong-rate calls leave state/output untouched, exact buffer timing and native EOF.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Microphone + independent playback reference → declared 2→1 AEC route → speech output with actual device clock assumptions.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-aec` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-aec --lib --tests
cargo clippy --offline --locked -p sotf-plugin-aec --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-aec`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/aec-constructor-parity.md](../../audit/aec-constructor-parity.md)
- [audit/aec-process-clock.md](../../audit/aec-process-clock.md)
- [audit/aec-native-tail-support.md](../../audit/aec-native-tail-support.md)
- [audit/spatial-plugins.md](../../audit/spatial-plugins.md)
- [crates/sotf-plugins/crates/sotf-plugin-aec/README.md](../../crates/sotf-plugins/crates/sotf-plugin-aec/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
