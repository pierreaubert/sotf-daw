# AUD137: ABCompare finite child and alignment drain

**Status:** bounded implementation corrections complete; Astra final
implementation review pending. The design was accepted before production edits.

## Change

ABCompare now drains its two same-rate child hosts into preallocated per-child
staging buffers, preserves each child’s output cursor, and pairs only frames at
the same stream position. It carries those frames through the existing A/B and
dry alignment delays and mixer until the child queues and delay rings are
empty. Drain preflight checks channel count, path sample rate, per-node sample
rates and declared frame geometry, and child capacity before either child
advances. `Plugin::guarantees_identity_frame_geometry` defaults to `false`;
the parametric adapter forwards an explicit opt-in. Every active node must opt
in and have matching negotiated input and output rates, so compensating rate
changes across a path are rejected. The built-in Delay plugin is the only
production plugin currently verified and opted in. Other child types remain
unsupported for this drain path until their all-block geometry contract is
verified and they opt in.

Drain returns `COMPLETE` with zero frames once both child streams are complete,
their queues are empty, and all A/B and dry alignment delays are empty. Each
output chunk is clamped to the shortest remaining child queue or alignment
delay, including the dry-only remainder; it does not pad the last chunk with
unused capacity zeros.

The plugin uses accepting, draining, complete, and reset-required states.
Starting drain freezes controls and positive-frame processing. A completed
stream, or a partial child-operation failure after either child has advanced,
requires explicit reset before more input. Invalid preflight is transactional.
Active or prior recursive band-mask history rejects drain because its response
cannot be proven finite; a reset discards that stream. Nonlinear, branching,
channel-remapped, or otherwise non-linear Graph routes are rejected before
child draining starts. Supported Rack and Graph composition is serial. The
plugin’s `TailLength` remains `Unknown`.

## Evidence

The pre-edit public Delay reproducer accepted 8 stereo frames but returned only
those 8 process frames and no drain output; the expected stream was 72 frames,
with a nonzero omitted suffix. The preserved public regression now passes.
The pre-edit expected-red run is `/tmp/sotf-aud137-public-red-final.log`
(exit 101, SHA-256
`0e43fef081bea3203a459952a37671e90bc8468b22d1698b0a851080f18512da`). Its
saved input, process output, empty drain output, and expected full vector are
listed in the proposal.
Before edits, a separate dense 128-frame ordinary-process control matched its
independent 48-frame delay reference byte-for-byte and had output peak 0.75.
Seven pre-edit mixer outputs (pure A, pure B, center mix, difference, phase
inversion, bypass, and enabled AutoGain) were also captured and replayed
byte-for-byte after the mixer edits.

The new composition suite compares full vectors to an independent f64 FIR
oracle with per-sample absolute error bound `1e-6`. It covers public Plugin,
Rack, and linear Graph paths, including two serial stages with different
latencies and tails; pure A/B, center, difference, phase inversion, and bypass;
zero-output child progress; the final marker and terminal completion; and an
outer DawHost route. It checks exact declared drain capacity and transactional
rejection for undersized or misaligned destinations, completion/reset
behavior, reset against a fresh instance, and reset-required behavior after a
partial child failure. An enabled AutoGain route is compared with ordinary
zero-input continuation across mix and bypass transitions. This proves
continuation alignment for the exercised fixture; it is not an independent
loudness-algorithm oracle.

The allocator test observes zero allocations and zero deallocations in the
prepared process, begin-drain, full drain, repeated terminal drain, and reset
phases. The active recursive-mask case verifies preflight rejection before
child advancement. Mask changes are structural, so this test does not claim a
runtime mask-off transition; the policy is to reset and discard any stream
whose mask was active. Arbitrary branching/remapped Graphs, unequal-rate
children, recursive child responses, and broader nested graph forms remain
outside this result.

## Reproduction

The pre-fix correction gate reproduced all three review findings: frame geometry
was admitted, an empty/no-tail stream emitted 17 frames, and a dry-only final
remainder of 3 frames emitted 16. After correction, the focused composition
suite passed 15/15, the full ABCompare package passed 131 tests with 4 manual
utilities ignored, strict Clippy passed for ABCompare, host, and Delay, and the
pre-edit mixer-control byte replay passed 1/1. The replay loads the preserved
pre-edit audio directory through `SOTF_AUDIT_BASELINE_DIR`.

These commands used the serialized shared DAW target, offline locked
dependencies, and a temporary directory under that target:

```sh
flock /tmp/sotf-daw-audit-cargo.lock env \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
  CARGO_NET_OFFLINE=true cargo test --offline --locked -p sotf-plugin-ab-compare \
  --test aud137_composition -- --nocapture

flock /tmp/sotf-daw-audit-cargo.lock env \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
  CARGO_NET_OFFLINE=true cargo test --offline --locked -p sotf-plugin-ab-compare --all-targets

flock /tmp/sotf-daw-audit-cargo.lock env \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
  SOTF_AUDIT_BASELINE_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-audio \
  CARGO_NET_OFFLINE=true cargo test --offline --locked -p sotf-plugin-ab-compare \
  --test aud137_finite_stream replay_aud137_pre_edit_mixer_control_vectors \
  -- --ignored --exact --nocapture

flock /tmp/sotf-daw-audit-cargo.lock env \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
  CARGO_NET_OFFLINE=true cargo clippy --offline --locked \
  -p sotf-plugin-ab-compare -p sotf-host -p sotf-plugin-delay --all-targets -- -D warnings
```

| Gate | Result | Log and SHA-256 |
|---|---:|---|
| Pre-fix composition regression | 11 passed, 3 expected failures | `/tmp/sotf-aud137-review-red-composition.log` — `9ca442091717e571ac07ac337b0b46a0731e7bd46e48185454ad3725b6e2b986` |
| Focused composition correction | 15 passed | `/tmp/sotf-aud137-review-fix-focused.log` — `db890e78d817c87078ecb626ac802a7679ee1835f47e1a78e8bdbcfc88228bbc` |
| Package `--all-targets` | 131 passed, 0 failed, 4 ignored manual utilities | `/tmp/sotf-aud137-review-fix-package.log` — `98278e35c96d89c86455036d6dc04e1e55e4025a07ab5312062ea2cf7d6e72c8` |
| Strict ABCompare/host/Delay Clippy | passed | `/tmp/sotf-aud137-review-fix-clippy.log` — `1c081b35c91b09fbb216e4e8ffe4899691783adca7cbf9dd095c2b5b31c09534` |
| Pre-edit mixer-control byte replay | 1 passed | `/tmp/sotf-aud137-review-fix-replay-final.log` — `3cf43635d461aa9ca4db5a4ce895df732241901f8be64cd228aa6ad88b1437ae` |

The preserved pre-edit baseline is source commit
`93027970f412ce47c0cd2b8e4b7b1a5b0e5f0261`. Source copies and their manifest
are under `crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-src/` and
`.../pre-edit-source.sha256` (manifest SHA-256
`1da15762358fef0c2c623645b6cca39e770dd4e3697ed5fd20ef4093cb84e3ab`). The
audio vectors and manifest are under
`crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-audio/` (manifest
SHA-256 `5ba91080ae9685be9665ae54f3667cfa2e18f4c62c2d76a43d207683f5adc983`).
The pre-edit dense process control is separately recorded in
`/tmp/sotf-aud137-preedit-process-control.log` (SHA-256
`621b7bae35a0fd204bc4394daaf24a64d931612502fd962d76b6f756b35bf681`). The
seven-vector pre-edit capture/replay are
`/tmp/sotf-aud137-preedit-mixer-controls.log` and
`/tmp/sotf-aud137-preedit-mixer-controls-replay.log`; their hashes are recorded
in the proposal.

Final tested source and lock hashes are recorded in
`/tmp/sotf-aud137-review-fix-final-end.sha256` (manifest SHA-256
`5d35cc3b55868eb1078af9dbabe9a5698211c6134a711f954110d0fa033a0425`). The
start and end manifests are identical; they cover ABCompare production sources
and tests, the host geometry capability and adapter/query, the Delay opt-in,
and `Cargo.lock` (SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`). Exact
tested copies are preserved under
`crates/sotf-plugins/target/audit-artifacts/aud137/review-fix-source/`; its
`tested-source.sha256` is byte-identical to the final manifest.
