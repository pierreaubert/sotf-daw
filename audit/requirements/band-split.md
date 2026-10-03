# Band Split: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-band-split`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-band-split](../../crates/sotf-plugins/crates/sotf-plugin-band-split).
- Coordination group: **Split/merge/shared native layouts**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

LR24/LR48 splitting and opt-in phase compensation are implemented with scoped native/UI/callback evidence. Reserved legacy VST3 bus and inactive-bus fixes exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **BAND-SPLIT-R1** — VERIFY/INTEGRATE: Complete remaining application/native routes for phase compensation, band count, slope and channel geometry.
- [ ] **BAND-SPLIT-R2** — AUDIT: Reconcile historical uncompensated reconstruction findings with accepted AUD143 fixes; implement only remaining source-confirmed gaps.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **BAND-SPLIT-A1** — Independent full complex split/sum response over crossover boundaries, overlapping bands and compensated versus legacy mode.
- [ ] **BAND-SPLIT-A2** — Preserve frozen legacy audio and IDs; verify active/inactive native buses, channel permutations and refusal/retry.
- [ ] **BAND-SPLIT-A3** — Automated cutoff/band gains, same-rate splitter→merge reconstruction and full final signal delivery.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Application controls → BandSplit → per-band processors → BandMerge, including loaded native auxiliary buses.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-band-split` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-band-split --lib --tests
cargo clippy --offline --locked -p sotf-plugin-band-split --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-band-split`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/band-split-phase-compensation-gap.md](../../audit/band-split-phase-compensation-gap.md)
- [audit/band-split-native-routing.md](../../audit/band-split-native-routing.md)
- [audit/reviews/AUD143-astra.md](../../audit/reviews/AUD143-astra.md)
- [crates/sotf-plugins/crates/sotf-plugin-band-split/README.md](../../crates/sotf-plugins/crates/sotf-plugin-band-split/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
