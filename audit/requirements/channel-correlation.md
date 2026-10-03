# Channel Correlation: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-host`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-host/src/analyzer_channel_correlation](../../crates/sotf-plugins/crates/sotf-host/src/analyzer_channel_correlation).
- Coordination group: **Shared host analyzers**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Centered exponentially weighted Pearson matrix and corrected standalone ingestion/realtime storage exist. The standalone wrapper and embedded LoudnessData path are separate.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **CHANNEL-CORRELATION-R1** — AUDIT/INTEGRATE: Decide and complete intended standalone registry/UI/native exposure without confusing it with embedded loudness correlation.
- [ ] **CHANNEL-CORRELATION-R2** — VERIFY: Full multichannel ingestion, numerical correlation and retained publication at supported widths.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **CHANNEL-CORRELATION-A1** — Independent centered Pearson recurrence with DC offsets, unequal gains, quadrature/antiphase and near-silence.
- [ ] **CHANNEL-CORRELATION-A2** — Callbacks exceeding former 96000-sample capacity at non-divisor channel widths; compare all channels/sample history.
- [ ] **CHANNEL-CORRELATION-A3** — Cold callbacks/reset with held readers, publication contention and no dropped/realigned frames.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Complete interleaved frames → standalone or embedded correlation path → actual host/UI snapshot.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-host/src/analyzer_channel_correlation` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

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

- [audit/correlation-stream.md](../../audit/correlation-stream.md)
- [audit/correlation-realtime.md](../../audit/correlation-realtime.md)
- [audit/metering-effects.md](../../audit/metering-effects.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
