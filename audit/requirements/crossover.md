# Crossover: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-crossover`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-crossover](../../crates/sotf-plugins/crates/sotf-plugin-crossover).
- Coordination group: **Split/merge/shared native layouts**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

LR12/24/48, Butterworth orders 1–8, Bessel2, FIR and explicit band counts exist. LR24 multiway correction and bounded 11-layout loaded routing are accepted.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **CROSSOVER-R1** — INTEGRATE: Finish wider live-manager/application routes and realtime deadline evidence for output-width/topology changes.
- [ ] **CROSSOVER-R2** — VERIFY: Close all supported family/order/per-channel/multiway/rate combinations through controls, restored state and actual split/merge.
- [ ] **CROSSOVER-R3** — AUDIT: Revalidate feature comparison before adding any further families/slopes; do not reimplement already accepted AUD141/AUD142 work.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **CROSSOVER-A1** — Independent complex branch and summed response, polarity and allpass phase for overlapping three/four-band splits, not only distant sine magnitudes.
- [ ] **CROSSOVER-A2** — Explicit/dormant band-count state, impossible cutoffs and invalid actual-rate requests reject without changing the live graph.
- [ ] **CROSSOVER-A3** — Actual loaded CLAP/VST3 output buses at named layouts and complete nonzero waveform comparison through BandMerge.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Typed settings → Crossover W→bands×W → independently processed bands → BandMerge → W-channel output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-crossover` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-crossover --lib --tests
cargo clippy --offline --locked -p sotf-plugin-crossover --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-crossover`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/reviews/AUD142-astra.md](../../audit/reviews/AUD142-astra.md)
- [audit/reviews/AUD142-ffi-astra.md](../../audit/reviews/AUD142-ffi-astra.md)
- [audit/crossover-multiway-recombination.md](../../audit/crossover-multiway-recombination.md)
- [audit/crossover-filter-choice-gap.md](../../audit/crossover-filter-choice-gap.md)
- [crates/sotf-plugins/crates/sotf-plugin-crossover/README.md](../../crates/sotf-plugins/crates/sotf-plugin-crossover/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
