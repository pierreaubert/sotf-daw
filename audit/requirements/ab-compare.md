# Ab Compare: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-ab-compare`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-ab-compare](../../crates/sotf-plugins/crates/sotf-plugin-ab-compare).
- Coordination group: **Clock/host integration**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

A/B plugin/rack/graph paths, aligned crossfade/bypass, difference and causal loudness matching exist. Bounded same-rate finite-child composition is accepted (AUD137).

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **AB-COMPARE-R1** — IMPLEMENT: Correct nested variable-rate path composition (AUD087): honor actual rates, produced lengths and timeline alignment without dropped or inserted samples.
- [ ] **AB-COMPARE-R2** — INTEGRATE: Finish branching and active/prior recursive band-mask EOF policies beyond accepted same-rate identity-frame cases.
- [ ] **AB-COMPARE-R3** — COORDINATE: Broad unequal-branch queue and manager protocol rewrites remain explicitly deferred; propose a scoped design and hand off shared changes rather than silently expanding ownership.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **AB-COMPARE-A1** — Independent 48→24 kHz nested resampler constant/impulse oracle and whole-output counts across regular/irregular callbacks.
- [ ] **AB-COMPARE-A2** — Known ±6 dB branch matching magnitude/LUFS and causal level-change timing; equal-path difference cancellation.
- [ ] **AB-COMPARE-A3** — Independently drain children and alignment rings, compare complete output; rejection/reset must preserve the accepted graph.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Nested Plugin/Rack/Graph A and B → path clock/latency alignment → matching/crossfade/difference → outer host EOF.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-ab-compare` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-ab-compare --lib --tests
cargo clippy --offline --locked -p sotf-plugin-ab-compare --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-ab-compare`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/abcompare-variable-rate.md](../../audit/abcompare-variable-rate.md)
- [audit/abcompare-finite-stream.md](../../audit/abcompare-finite-stream.md)
- [audit/reviews/AUD137-astra.md](../../audit/reviews/AUD137-astra.md)
- [crates/sotf-plugins/crates/sotf-plugin-ab-compare/README.md](../../crates/sotf-plugins/crates/sotf-plugin-ab-compare/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
