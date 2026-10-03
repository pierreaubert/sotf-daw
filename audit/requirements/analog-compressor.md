# Analog Compressor: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-analog-compressor`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-analog-compressor](../../crates/sotf-plugins/crates/sotf-plugin-analog-compressor).
- Coordination group: **Shared analog family**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Compressor plus six-model coloration; shared control retention has been corrected.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **ANALOG-COMPRESSOR-R1** — VERIFY: Combined dynamics/color/trim behavior and complete consumer controls; compare professional analog-model features before implementing verified gaps.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **ANALOG-COMPRESSOR-A1** — Independent compressor law/timing with color off, then THD/IMD/aliasing with color on.
- [ ] **ANALOG-COMPRESSOR-A2** — Model update-order/preset equivalence, linked channels, latency and final emitted gain rather than pre-color telemetry.
- [ ] **ANALOG-COMPRESSOR-A3** — Cold lifecycle heap checks, automation, rate changes and finite/recursive-tail policy.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Native/application preset → dynamics or EQ → analog color/trim → final output; coordinate analog-common ownership.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-analog-compressor` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-analog-compressor --lib --tests
cargo clippy --offline --locked -p sotf-plugin-analog-compressor --all-targets -- -D warnings
```

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/dynamics-finite-stream.md](../../audit/dynamics-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-analog-compressor/README.md](../../crates/sotf-plugins/crates/sotf-plugin-analog-compressor/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
