# Gain: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-gain`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-gain](../../crates/sotf-plugins/crates/sotf-plugin-gain).
- Coordination group: **Independent utility**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Global/per-channel dB gain, smoothing and compiled/SIMD paths exist; no material feature omission was established in the utility audit.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [x] **GAIN-R1** — AUDIT: Revalidate declared utility scope and current host/control integration; implement only confirmed missing behavior. Disposition recorded 2026-10-01 (see Execution evidence; no missing behavior, no production change).
- [ ] **GAIN-R2** — VERIFY: Strengthen independent numerical and automation precision where current tests reuse production conversions. Suite authored, not executed (see Execution evidence).

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **GAIN-A1** — Independent f64 10^(dB/20) sweep at range endpoints, subnormal/small values, channel gains and signed input.
- [ ] **GAIN-A2** — Closed-form smoothing and exact event offsets, compiled/scalar parity and callback partitions.
- [ ] **GAIN-A3** — Unity/mute/overrange behavior, no hidden clamp and complete parameter/preset consumer round trip.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Per-channel automation → Gain → downstream meter/output; compare direct and factory/compiled/native paths.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-gain` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-gain --lib --tests
cargo clippy --offline --locked -p sotf-plugin-gain --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-gain`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Execution evidence — Muse 2026-10-01 (single session, no subagents)

Owner: `Muse: gain` (claim already present in README; table untouched).
Work record: [progress](../muse-parallel-2026-10-01/gain/progress.md),
[scoped issue](../muse-parallel-2026-10-01/gain/issue.md),
[result](../muse-parallel-2026-10-01/gain/result.md).

- [x] **GAIN-R1** — AUDIT disposition recorded (source evidence, no
  behavior change): global/per-channel dB gain, smoothing, SIMD settled
  kernels, compiled `ApplyGain` op with static-gain fusion metadata, and
  integration across facade factory (`"gain"`), bridge factory, FFI
  parameter map, NIH feature gate, params re-export, and `DawHost` are
  all present in current source. No confirmed missing behavior; no
  production edit made. Feature table in `result.md`.
- [ ] **GAIN-R2 / GAIN-A1 / GAIN-A2 / GAIN-A3** — Independent suite
  authored at
  [tests/accuracy.rs](../../crates/sotf-plugins/crates/sotf-plugin-gain/tests/accuracy.rs)
  (16 tests: f64 `10^(dB/20)` sweep incl. endpoints/subnormals/signed
  input, closed-form smoother oracle at 44.1/48/96/192 kHz, exact event
  offsets, compiled/scalar parity, partition invariance, bit-exact
  unity/static-gain/round-trip checks, host automation chain). Pre-declared
  bounds: 1e-5 relative + 1e-38 absolute floor (conversion), 5e-4
  absolute (ramps). NOT EXECUTED — shell/Cargo blocked in-session
  (`bwrap: execvp /dev/.tbh-linux-sandbox: No such file or directory`);
  coordinator must run the focused gates below and record worst-case errors.
- [ ] Whole-chain acceptance — partial: per-channel automation → Gain →
  output through a real `DawHost` plan is covered by an authored (unrun)
  test; save/reload through shared engine/FFI/native adapters needs the
  shared owner. No shared patch required (no production change).
- [ ] Astra medium review — pending coordinator gate; not claimed.

```bash
flock /tmp/sotf-daw-audit-cargo.lock env \
  TMPDIR=/tmp CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo test --offline --locked -p sotf-plugin-gain --lib --tests
flock /tmp/sotf-daw-audit-cargo.lock env \
  TMPDIR=/tmp CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo clippy --offline --locked -p sotf-plugin-gain --all-targets -- -D warnings
```

## Fix round R1 evidence — Muse 2026-10-01 (authored, not executed)

Independent review (`audit/muse-parallel-2026-10-01/gain/review.md`,
verdict: changes required) plus `validation-r1` clippy failures addressed;
`review.md` immutable. Records:
[fix result](../muse-parallel-2026-10-01/gain/fix-r1-result.md),
[verification request](../muse-parallel-2026-10-01/gain/verification-request.md).

- **F1 (clippy)** — fixed: approx-constant literals in `tests/accuracy.rs`
  replaced (`3.14159`→`3.25`, `-2.71828`→`-2.75`, `-3.14159`→`-3.25`).
- **F2 (mode fidelity)** — fixed in production: `apply_values` restores
  global-mode snapshots as global state (new `global_snapshot_gain_db`
  helper); round-trip test extended with `is_per_channel()` and
  `compile_metadata()` equality; new dedicated regression test.
- **F3 (transactional restore)** — fixed in production: `apply_values`
  pre-validates the whole set (out-of-range/non-finite/unknown/OOB entries
  now `Err` with zero mutation; smoothing clamp removed); new 7-case
  mid-ramp-history regression test. No shared caller depends on the old
  clamp/no-op behavior.
- **F4 (bulk setter)** — documented snap semantics on `set_channel_gains`
  (review's allowed alternative; no callers, consistent with construction)
  with a pinning test.
- **F5 (citation)** — `result.md` now cites `plugin_factory.rs:463`.
- **F6 (worst-case errors)** — bounds/assertions untouched; all 7 oracle
  tests print `GAIN-WORST` diagnostics for `--nocapture` capture.

No DSP, default, ID, range, schema, preset-format, or version change. No
requirement checkbox is marked passing by this round: coordinator must run
the gates in
`verification-request.md` (focused tests, clippy, `--nocapture` worst-case
capture, `qa-gain`) and record raw logs plus measured errors.

## Starting evidence

- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [crates/sotf-plugins/crates/sotf-plugin-gain/README.md](../../crates/sotf-plugins/crates/sotf-plugin-gain/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
