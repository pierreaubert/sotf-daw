# Mono To Stereo: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-mono-to-stereo`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-mono-to-stereo](../../crates/sotf-plugins/crates/sotf-plugin-mono-to-stereo).
- Coordination group: **Stereo utilities**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Allpass decorrelation, width, optional Haas and duplicate shortcut exist; hostile final-input state poisoning was fixed.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **MONO-TO-STEREO-R1** — VERIFY: Frequency/phase, mono compatibility and image behavior across the documented modes.
- [ ] **MONO-TO-STEREO-R2** — AUDIT/IMPLEMENT: Any remaining feature/control gaps without assuming decorrelation must preserve waveform identity.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **MONO-TO-STEREO-A1** — Independent allpass complex transfer and L/R energy/causality; Haas delay and mono-fold combing are measured separately.
- [ ] **MONO-TO-STEREO-A2** — Width-zero→nonzero after final-sample NaN/Inf, finite continuation and reset versus sanitized reference.
- [ ] **MONO-TO-STEREO-A3** — Actual 1→2 host/native negotiation, asymmetric buffers and complete EOF.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Mono input → 1→2 widening/Haas → stereo host output and explicit mono compatibility check.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-mono-to-stereo` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-mono-to-stereo --lib --tests
cargo clippy --offline --locked -p sotf-plugin-mono-to-stereo --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-mono-to-stereo`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [audit/channel-changing-host-eof.md](../../audit/channel-changing-host-eof.md)
- [crates/sotf-plugins/crates/sotf-plugin-mono-to-stereo/README.md](../../crates/sotf-plugins/crates/sotf-plugin-mono-to-stereo/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
