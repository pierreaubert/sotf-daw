# Crossfeed: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Max (implementer lane, claimed 2026-10-02)** — owned-crate D1/D2/D3 patch plus V1/V2/V3/V6 tests; shared consumers via handoff; no requirement checkbox closed by this claim.

- Package: `sotf-plugin-crossfeed`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-crossfeed](../../crates/sotf-plugins/crates/sotf-plugin-crossfeed).
- Coordination group: **Stereo utilities/shared AutoGain**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Bauer, Meier, multiband and compact parametric HRTF modes, yaw/ITD and causal AutoGain corrections exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **CROSSFEED-R1** — VERIFY: Independent complex transfer for each mode and physical HRTF approximation limits.
- [ ] **CROSSFEED-R2** — AUDIT/IMPLEMENT: Remaining controls/consumer requirements; retain the documented compact model rather than claiming personalized measured-HRTF equivalence.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **CROSSFEED-A1** — Independent 2×2 frequency/phase matrix, ITD arrival, mono fold and antiphase extremes.
- [ ] **CROSSFEED-A2** — AutoGain causal/partition/settled-level checks with meaningful smoothing, and exact disabled controls.
- [ ] **CROSSFEED-A3** — Mode/preset transitions, sample-rate changes, dry mix and native/app restoration.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Stereo source → selected crossfeed mode → aligned compensation/mix → headphone output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-crossfeed` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-crossfeed --lib --tests
cargo clippy --offline --locked -p sotf-plugin-crossfeed --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-crossfeed`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [audit/crossfeed-autogain-clock.md](../../audit/crossfeed-autogain-clock.md)
- [audit/crossfeed-autogain-clock-independent-review.md](../../audit/crossfeed-autogain-clock-independent-review.md)
- [crates/sotf-plugins/crates/sotf-plugin-crossfeed/README.md](../../crates/sotf-plugins/crates/sotf-plugin-crossfeed/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
