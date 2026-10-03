# Multiband Compressor: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse compressor family — muse-spark-1.3-contributor, max effort**.

- Package: `sotf-plugin-multiband-compressor`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-multiband-compressor](../../crates/sotf-plugins/crates/sotf-plugin-multiband-compressor).
- Coordination group: **Shared compressor implementation**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Broadband and multiband compressor share this crate; range/hold and bounded dynamics/drain fixes exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **MULTIBAND-COMPRESSOR-R1** — AUDIT/IMPLEMENT: Compare multiband detector, linking, phase, sidechain and band-control capabilities with current primary references; implement confirmed missing capabilities.
- [ ] **MULTIBAND-COMPRESSOR-R2** — VERIFY: Full recombination/dynamics accuracy and application/native parameter/preset coverage, including interaction with new crossover work.
- [ ] **MULTIBAND-COMPRESSOR-R3** — COORDINATE: Same source as Compressor; assign both files to one owner or disjoint agreed modules.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **MULTIBAND-COMPRESSOR-A1** — Independent static knee/ratio/range law per band and absolute attack/hold/release timing across rates.
- [ ] **MULTIBAND-COMPRESSOR-A2** — Multitone signals crossing adjacent bands: independent crossover phase/sum, gain envelopes, stereo image and linked/unlinked behavior.
- [ ] **MULTIBAND-COMPRESSOR-A3** — Neutral and bypass transparency with reported delay, lookahead/EOF, dense automation and legacy state/ID compatibility.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Persisted multiband controls → splitting/detection/gain/recombination → host latency compensation → output.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-multiband-compressor` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-multiband-compressor --lib --tests
cargo clippy --offline --locked -p sotf-plugin-multiband-compressor --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-multiband-compressor`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Muse max review; separate Muse max implementation session fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Implementation evidence (2026-10-01, compressor-family lane)

- Per-band/per-channel RMS detection + sidechain HPF implemented (additive;
  Peak + HPF-disabled defaults; legacy path bit-preserved). Crossover
  topology deliberately unchanged; the actual cascaded-LR4 complex sum is
  pinned by an independent prototype oracle (no unity assumption, no fitted
  phase, no default-audio change). HPF spec default 80 -> 0 Hz (disabled);
  external sidechain and program-dependent release remain loudly rejected
  with recorded reasons.
- New `tests/compressor_accuracy.rs` covers the multiband accuracy items:
  per-band static law with range, absolute timing, adjacent multitone vs
  LR4 prototype + solo/sum closure, linked/unlinked image, lookahead/EOF,
  dense automation, legacy compatibility, cold RT guard.
- Shared handoffs in
  `audit/muse-parallel-2026-10-01/compressor-family/shared-patches/`; gates
  NOT executed (shell disabled), boxes unchecked pending
  `verification-request.md`. No SOTA claim.

## Fix-compat evidence (2026-10-01, compressor-family lane)

- V1 HPF default change rejected by root review (silent old-save audio
  change); NOT landed. Fix: additive `sidechain_hpf_enabled` (default
  false, appended idx 22); HPF spec default restored to 80 Hz; HPF
  active <=> enabled AND hz > 0. Legacy multiband state without the new
  keys renders legacy audio; crossover topology unchanged; RMS, timing
  bounds, Peak default, and unsupported-control rejects preserved. See
  lane `fix-compat-issue.md`, `fix-compat-result.md`, v2
  `shared-patches/`, `fix-compat-verification-request.md` (not executed).

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/dynamics-finite-stream.md](../../audit/dynamics-finite-stream.md)
- [audit/dynamics-drain-work-bounds.md](../../audit/dynamics-drain-work-bounds.md)
- [crates/sotf-plugins/crates/sotf-plugin-multiband-compressor/README.md](../../crates/sotf-plugins/crates/sotf-plugin-multiband-compressor/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
