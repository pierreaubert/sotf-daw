# Hal Output: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-hal-output`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-hal-output](../../crates/sotf-plugins/crates/sotf-plugin-hal-output).
- Coordination group: **HAL/macOS shared transport**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Prepared frame-aligned pending output, backpressure, bounded direct/serial-host EOF and preserving ring reprepare are accepted in scoped tests (AUD138).

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **HAL-OUTPUT-R1** — INTEGRATE: Engine/application admission for zero-output terminal sinks and actual native playback.
- [ ] **HAL-OUTPUT-R2** — VERIFY: Physical output completion/latency, driver restart and sustained clock mismatch, beyond writer acceptance.
- [ ] **HAL-OUTPUT-R3** — COORDINATE: Shared engine endpoint and driver lifecycle changes belong to designated integration owners.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **HAL-OUTPUT-A1** — Partial writes/backpressure including multi-chunk upstream tails, final nonzero samples and no duplicate/drop across reprepare.
- [ ] **HAL-OUTPUT-A2** — Native device loopback and key rollover/reconnect stress; timestamps/duration separate from pending-ring fill.
- [ ] **HAL-OUTPUT-A3** — Cold allocation/reset/drain bounds and failure preserving pending accepted audio.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Finite/recursive producer → actual engine terminal sink → shared memory/HAL → native playback completion.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-hal-output` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-hal-output --lib --tests
cargo clippy --offline --locked -p sotf-plugin-hal-output --all-targets -- -D warnings
```

Native device gates require macOS and real driver/runtime evidence; Linux unit tests must be reported separately.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/hal-output-finite-stream.md](../../audit/hal-output-finite-stream.md)
- [audit/reviews/AUD138-astra.md](../../audit/reviews/AUD138-astra.md)
- [audit/remaining-finite-stream-reconciliation.md](../../audit/remaining-finite-stream-reconciliation.md)
- [crates/sotf-plugins/crates/sotf-plugin-hal-output/README.md](../../crates/sotf-plugins/crates/sotf-plugin-hal-output/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
