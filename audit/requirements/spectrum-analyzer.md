# Spectrum Analyzer: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-host`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-host/src/analyzer_spectrum.rs](../../crates/sotf-plugins/crates/sotf-host/src/analyzer_spectrum.rs).
- Coordination group: **Shared host analyzers**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Periodic-Hann spectrum, logarithmic bands, channel-max convention, physical-time smoothing and corrected endpoint power exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **SPECTRUM-ANALYZER-R1** — AUDIT/IMPLEMENT: Adjustable FFT/window/hop, PSD units, channel/M/S traces and peak hold are comparison dimensions; verify current support and declare scope before implementing gaps.
- [ ] **SPECTRUM-ANALYZER-R2** — VERIFY/INTEGRATE: Absolute display calibration and renderer/cache behavior through actual host/UI routes.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **SPECTRUM-ANALYZER-A1** — Independent Hann-weighted Parseval energy, coherent/off-bin lines, DC/Nyquist and exact maximum-frequency inclusion.
- [ ] **SPECTRUM-ANALYZER-A2** — Channel-max behavior for antiphase/disjoint/silent channels and declared amplitude versus power/PSD units.
- [ ] **SPECTRUM-ANALYZER-A3** — Physical smoothing, oversized-callback latest-window policy, reset and cold retained-snapshot publication.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Actual plugin-chain samples → analyzer window/bands → retained host snapshot → rendered trace with truthful units.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-host/src/analyzer_spectrum.rs` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-host --lib --tests
cargo clippy --offline --locked -p sotf-host --all-targets -- -D warnings
```

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/spectrum-endpoint-power.md](../../audit/spectrum-endpoint-power.md)
- [audit/metering-effects.md](../../audit/metering-effects.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
