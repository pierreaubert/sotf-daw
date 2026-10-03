# Loudness Monitor: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-host`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs](../../crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs).
- Coordination group: **Shared host analyzers**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

K weighting, M/S/I, LRA, programme maximum true peak and maximum M/S, coupled pause/continue and first-minute LRA indication have scoped accepted core/UI work. AUD123 rate coverage exists.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **LOUDNESS-MONITOR-R1** — VERIFY: Complete official EBU corpus coverage (AUD128), starting with actual archive/version/case mapping and permitted access; no corpus has been acquired by this handoff.
- [ ] **LOUDNESS-MONITOR-R2** — VERIFY/INTEGRATE: Remaining channel-role/rate, finite-stream final publication, native/application and retained-snapshot behavior.
- [ ] **LOUDNESS-MONITOR-R3** — AUDIT: Revalidate full metering feature requirements; do not reimplement accepted maximum/LRA/pause controls from stale report sections.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **LOUDNESS-MONITOR-A1** — Independent published K-weighting, absolute/relative gates, LRA and true-peak cases, including non-divisible rates and programme-end samples.
- [ ] **LOUDNESS-MONITOR-A2** — Actual permitted official audio cases with per-case expected value/tolerance and failure report; synthetic tests are not certification.
- [ ] **LOUDNESS-MONITOR-A3** — Pause/resume/reset/warmup/max latches, LFE exclusion, channel permutations, held readers and cold publication heap behavior.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Multichannel signal/roles → Loudness Monitor → host data → reachable TUI/GPUI/native display and final EOF publication.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

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

- [audit/metering-effects.md](../../audit/metering-effects.md)
- [audit/true-peak-coefficients.md](../../audit/true-peak-coefficients.md)
- [audit/true-peak-finite-stream.md](../../audit/true-peak-finite-stream.md)
- [audit/proposals/ebu-loudness-test-set.md](../../audit/proposals/ebu-loudness-test-set.md)
- [audit/IMPLEMENTATION_PLAN.md](../../audit/IMPLEMENTATION_PLAN.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
