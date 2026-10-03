# Expander: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-multiband-expander`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-multiband-expander](../../crates/sotf-plugins/crates/sotf-plugin-multiband-expander).
- Coordination group: **Shared expander implementation**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

The broadband Expander shares sotf-plugin-multiband-expander. Conventional downward-ratio and soft-knee corrections exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **EXPANDER-R1** — VERIFY/INTEGRATE: Preserve broadband parameter/schema/preset behavior independently of multiband settings.
- [ ] **EXPANDER-R2** — AUDIT/IMPLEMENT: Remaining detector/linking/mode features with the shared expander owner.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **EXPANDER-A1** — Independent absolute expansion slope, continuous knee, range and attack/hold/release timing.
- [ ] **EXPANDER-A2** — Factory broadband configuration, linked/unlinked channels and actual controls through consumers.
- [ ] **EXPANDER-A3** — Dry/lookahead alignment, all accepted EOF samples, automation and cold heap checks.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Broadband Expander settings → shared expander DSP → actual host/native output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-multiband-expander` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-multiband-expander --lib --tests
cargo clippy --offline --locked -p sotf-plugin-multiband-expander --all-targets -- -D warnings
```

The package commands also exercise the multiband implementation; keep broadband-specific factory/native regressions as separate named cases.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/multiband-expander-soft-knee.md](../../audit/multiband-expander-soft-knee.md)
- [crates/sotf-plugins/crates/sotf-plugin-multiband-expander/README.md](../../crates/sotf-plugins/crates/sotf-plugin-multiband-expander/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
