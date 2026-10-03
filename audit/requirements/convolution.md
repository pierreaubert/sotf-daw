# Convolution: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-convolution`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-convolution](../../crates/sotf-plugins/crates/sotf-plugin-convolution).
- Coordination group: **Convolution/shared native resources**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Uniform/nonuniform partitioning, async IR preparation/resampling and four-path true stereo exist. Bounded Linux CLAP/VST3 resource/editor/state checkpoints are accepted.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **CONVOLUTION-R1** — INTEGRATE: Finish packaged host-loaded editor/file selection, changed-geometry activation, native teardown and other supported platforms.
- [ ] **CONVOLUTION-R2** — INTEGRATE: Complete IR resource persistence/recovery across missing files, failure/retry and fresh host instances while retaining committed audio.
- [ ] **CONVOLUTION-R3** — AUDIT: Compare IR/channel routing, partition/latency and resource capabilities against primary convolution references; add only verified remaining functionality.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **CONVOLUTION-A1** — Independent direct convolution for mono/stereo/true-stereo and long asymmetric IRs, nonzero final taps and irregular blocks.
- [ ] **CONVOLUTION-A2** — Actual loaded native editor/state lifecycle with host-serviced restart, scalar automation while loading, failed IR adoption and retained old history.
- [ ] **CONVOLUTION-A3** — Cold process/drain/reset heap checks, resampled IR physical timing and complete exported tails.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

IR selection → resource preparation → native/engine commit → four-path convolution → EOF/export and fresh-state restoration.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-convolution` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-convolution --lib --tests
cargo clippy --offline --locked -p sotf-plugin-convolution --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-convolution`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/true-stereo-convolution.md](../../audit/true-stereo-convolution.md)
- [audit/reviews/AUD134-astra.md](../../audit/reviews/AUD134-astra.md)
- [audit/handoffs/aud134-native-ir-resource-contract.md](../../audit/handoffs/aud134-native-ir-resource-contract.md)
- [audit/delay-convolution-finite-stream.md](../../audit/delay-convolution-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-convolution/README.md](../../crates/sotf-plugins/crates/sotf-plugin-convolution/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
