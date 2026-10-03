# Band Merge: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-band-merge`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-band-merge](../../crates/sotf-plugins/crates/sotf-plugin-band-merge).
- Coordination group: **Split/merge/shared native layouts**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Band-major summing, per-band gains/mutes, smoothing, layout validation and zero-audio-tail support exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **BAND-MERGE-R1** — VERIFY: Complete split/merge numerical and actual host-route evidence across all supported layouts and structural band counts.
- [ ] **BAND-MERGE-R2** — AUDIT: No new standalone feature omission is established by the current utility audit; inspect current contracts before inventing extensions.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **BAND-MERGE-A1** — Independent f64 sum/cancellation oracle, signed gains, muted bands and gain-ramp closed forms.
- [ ] **BAND-MERGE-A2** — Full complex and waveform reconstruction with BandSplit/Crossover, including nonneutral processing and channel leakage.
- [ ] **BAND-MERGE-A3** — Exact input/output width validation, rejected reconfiguration, bypass and EOF without losing the final band samples.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Band-major splitter output → per-band processing → BandMerge → output; use real negotiated widths.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-band-merge` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-band-merge --lib --tests
cargo clippy --offline --locked -p sotf-plugin-band-merge --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-band-merge`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [audit/zero-audio-tail-support.md](../../audit/zero-audio-tail-support.md)
- [audit/crossover-multiway-recombination.md](../../audit/crossover-multiway-recombination.md)
- [crates/sotf-plugins/crates/sotf-plugin-band-merge/README.md](../../crates/sotf-plugins/crates/sotf-plugin-band-merge/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
