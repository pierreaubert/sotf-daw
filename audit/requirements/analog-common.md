# Analog Common: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-analog-common`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-analog-common](../../crates/sotf-plugins/crates/sotf-plugin-analog-common).
- Coordination group: **Shared analog family**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Shared coloration for Analog Compressor/EQ/Limiter; model/control retention correction exists. This is a support crate, not an independently loaded effect.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **ANALOG-COMMON-R1** — VERIFY/IMPROVE: Establish quantitative transfer, THD/IMD, aliasing, DC and latency behavior for every pinned model.
- [ ] **ANALOG-COMMON-R2** — INTEGRATE: Keep retained drive/color/character/trim behavior and update-order compatibility for all three consumers.
- [ ] **ANALOG-COMMON-R3** — COORDINATE: One owner for this shared crate; consumer agents request changes through that owner.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **ANALOG-COMMON-A1** — Independent static/nonlinear transfer and spectral analysis across levels, frequencies and rates; report model-specific error/alias budgets.
- [ ] **ANALOG-COMMON-A2** — Zero-color exact transparency, model switches vs equivalent freshly configured state and nonfinite recovery.
- [ ] **ANALOG-COMMON-A3** — Cold callback allocation/deallocation checks in each actual analog consumer.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Each analog consumer → shared coloration → emitted output; test all consumers after shared changes.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-analog-common` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-analog-common --lib --tests
cargo clippy --offline --locked -p sotf-plugin-analog-common --all-targets -- -D warnings
```

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [AUDIT.md](../../AUDIT.md)
- [crates/sotf-plugins/crates/sotf-plugin-analog-common/README.md](../../crates/sotf-plugins/crates/sotf-plugin-analog-common/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
