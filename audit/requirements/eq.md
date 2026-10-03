# Eq: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-eq`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-eq](../../crates/sotf-plugins/crates/sotf-plugin-eq).
- Coordination group: **EQ integration**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Ordered per-band Stereo/Left/Right/Mid/Side DSP, explicit pairs, Biquad/SVF/Warped/Kautz, engine conversion and bounded manager transactions exist. Independent base/advanced/multirate core evidence and response-domain corrections have scoped Astra acceptance. Native wrapper and GPUI receipt work are unfinished; see CHECKPOINT.md.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **EQ-R1** — INTEGRATE: Finish native CLAP/VST3 placement and pair Apply, 11 named layouts through 16 channels, host-to-DSP channel permutations, saved-state migration, restart/refusal/retry and committed-state serialization. Retain the old 125 parameter IDs; validate the documented raw-order-to-choice migration.
- [ ] **EQ-R2** — INTEGRATE: Append FFI placement/pair metadata after the existing 105 addresses; retain 32-channel core/FFI routes. Make new full presets retain pairs, band order and full advanced/Kautz configuration; preserve legacy raw partial-state semantics.
- [ ] **EQ-R3** — INTEGRATE: Finish accepted/staged graph receipt ownership, original-band/node targeting, visible Retry/correction/discard behavior, and mounted placement/pair controls. Restore accepted audio state after failure without losing newer band/rack edits.
- [ ] **EQ-R4** — IMPLEMENT: Converter-inclusive oversampled complex response and chart/cache integration; converters surround the whole ordered matrix once, including unpaired channels and queue phase.
- [ ] **EQ-R5** — IMPLEMENT: Integrated dynamic/spectral bands remain a separate feature slice. Specify detector, gain law, phase/latency and interactions with existing static/advanced bands; coordinate common algorithms with Dynamic EQ and Spectral Compressor.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **EQ-A1** — Use frozen independent matrix and full-waveform references for noncommuting Left→Mid versus Mid→Left, reversed/disjoint pairs and unpaired channels.
- [ ] **EQ-A2** — Check active ordinary SVF order restrictions, base-rate SVF with selected factors 1/2/4, wide advanced parameter domains, Kautz dry contribution and Warped phase.
- [ ] **EQ-A3** — Load fresh actual CLAP/VST3 binaries and exercise host callbacks, failed populated restore, correction/retry and complete nonzero audio; helper tests alone do not close integration.
- [ ] **EQ-A4** — Run mounted receipt tests for add/remove-before-target, unrelated NodeID add/remove, stale A success/B rejection, subsequent C rejection, immediate dispatch failure and eventual success.
- [ ] **EQ-A5** — Measure advanced oversampled transfer, automation, cold process/drain/reset heap activity and finite-stream delivery. Preserve frozen legacy audio.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Persisted application graph → real manager → EQ → downstream gain/output; separately bridge/FFI and loaded CLAP/VST3 at stereo and supported multichannel widths.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-eq` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-eq --lib --tests
cargo clippy --offline --locked -p sotf-plugin-eq --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-eq`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/reviews/AUD145-astra.md](../../audit/reviews/AUD145-astra.md)
- [audit/proposals/eq-band-placement.md](../../audit/proposals/eq-band-placement.md)
- [audit/handoffs/aud145-native-eq-placement.md](../../audit/handoffs/aud145-native-eq-placement.md)
- [audit/handoffs/aud145-oversampled-response-implementation.md](../../audit/handoffs/aud145-oversampled-response-implementation.md)
- [audit/handoffs/aud145-player-response-review.md](../../audit/handoffs/aud145-player-response-review.md)
- [audit/handoffs/aud145-eq-receipt-stale-success.md](../../audit/handoffs/aud145-eq-receipt-stale-success.md)
- [crates/sotf-plugins/crates/sotf-plugin-eq/README.md](../../crates/sotf-plugins/crates/sotf-plugin-eq/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
