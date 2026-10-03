# Beamformer: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-beamformer`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-beamformer](../../crates/sotf-plugins/crates/sotf-plugin-beamformer).
- Coordination group: **Spatial/adaptive**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

ULA MVDR, superdirective and GSC processing, covariance recovery and bounded native tail support exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **BEAMFORMER-R1** — AUDIT/IMPLEMENT: Establish scope for arbitrary-array calibration and moving-source tracking; these are unimplemented comparison dimensions, not proven defects in the ULA contract.
- [ ] **BEAMFORMER-R2** — VERIFY/IMPROVE: Robustness to microphone gain/phase error, reverberation, wind and nonstationary noise.
- [ ] **BEAMFORMER-R3** — INTEGRATE: Persist geometry/adaptive controls and expose truthful mono/output/tail metadata through consumers.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **BEAMFORMER-A1** — Independent steering and distortionless constraints, white-noise gain/directivity, fractional-delay broadband error.
- [ ] **BEAMFORMER-A2** — Known target/interferer directions and measured/controlled room data: SNR improvement versus target distortion.
- [ ] **BEAMFORMER-A3** — Adaptation/recovery time after overload or direction change, tail waveform and heap bounds; finite output alone is insufficient.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Multimicrophone channels + geometry → adaptive beamformer → mono speech/output with correct native layout.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-beamformer` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-beamformer --lib --tests
cargo clippy --offline --locked -p sotf-plugin-beamformer --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-beamformer`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/beamformer-numerical-recovery.md](../../audit/beamformer-numerical-recovery.md)
- [audit/beamformer-covariance-recovery.md](../../audit/beamformer-covariance-recovery.md)
- [audit/beamformer-native-tail-support.md](../../audit/beamformer-native-tail-support.md)
- [audit/spatial-plugins.md](../../audit/spatial-plugins.md)
- [crates/sotf-plugins/crates/sotf-plugin-beamformer/README.md](../../crates/sotf-plugins/crates/sotf-plugin-beamformer/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
