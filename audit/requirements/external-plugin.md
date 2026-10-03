# External Plugin: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-host`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-host/src/external_plugin.rs](../../crates/sotf-plugins/crates/sotf-host/src/external_plugin.rs).
- Coordination group: **Shared native host/engine**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Direct/isolated CLAP/VST3 hosting, typed saved setup, worker recovery and bounded native layout/state/tail fixes exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **EXTERNAL-PLUGIN-R1** — INTEGRATE: Finish wider manager/application/platform and packaged editor routes, truthful layout/precision/tail negotiation and state lifecycle.
- [ ] **EXTERNAL-PLUGIN-R2** — VERIFY: Actual loaded external binaries and isolated-worker behavior under refusal, failure, replacement and EOF.
- [ ] **EXTERNAL-PLUGIN-R3** — COORDINATE: Engine format/quiescence protocol and unequal-branch queue redesign remain separately constrained; do not broaden this assignment without resolving their scoped design.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **EXTERNAL-PLUGIN-A1** — Actual CLAP/VST3 callback metadata, bus arrangements/masks, inactive/short auxiliary buffers and speaker permutations.
- [ ] **EXTERNAL-PLUGIN-A2** — Loaded nonzero reference audio across format/rate/width changes; failed prepare preserves committed instance and later retry works.
- [ ] **EXTERNAL-PLUGIN-A3** — Isolated pending audio, startup delay, exact bounded EOF, worker timeout/recovery, cold heap and callback deadlines.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Descriptor + persisted typed setup → factory/planner → direct or isolated native host → engine/device/export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-host/src/external_plugin.rs` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

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

- [audit/support-layers.md](../../audit/support-layers.md)
- [audit/reviews/AUD135-astra.md](../../audit/reviews/AUD135-astra.md)
- [audit/reviews/AUD142-astra.md](../../audit/reviews/AUD142-astra.md)
- [audit/reviews/AUD143-astra.md](../../audit/reviews/AUD143-astra.md)
- [audit/engine-drivers.md](../../audit/engine-drivers.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
