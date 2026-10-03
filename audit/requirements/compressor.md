# Compressor: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse compressor family — muse-spark-1.3-contributor, max effort**.

- Package: `sotf-plugin-multiband-compressor`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-multiband-compressor](../../crates/sotf-plugins/crates/sotf-plugin-multiband-compressor).
- Coordination group: **Shared compressor implementation**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

The broadband Compressor is an alias/configuration of sotf-plugin-multiband-compressor. Range/hold and dynamics corrections exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **COMPRESSOR-R1** — VERIFY/INTEGRATE: Broadband schema, presets and native/app controls must remain distinct from multiband defaults despite shared implementation.
- [ ] **COMPRESSOR-R2** — AUDIT/IMPLEMENT: Current professional detector, sidechain, timing and channel capabilities; implement confirmed remaining gaps with the shared owner.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **COMPRESSOR-A1** — Independent hard/soft-knee ratio/range law and attack/hold/release timing over input levels and rates.
- [ ] **COMPRESSOR-A2** — Broadband factory preset versus explicitly equivalent core configuration, including rejected unsupported legacy controls.
- [ ] **COMPRESSOR-A3** — Lookahead/dry alignment, linked channels, automation, final held samples and callback allocation bounds.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Broadband Compressor app/native settings → shared compressor DSP → latency-compensated host/output.

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

The package commands also exercise the multiband implementation; keep broadband-specific factory/native regressions as separate named cases.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Muse max review; separate Muse max implementation session fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Implementation evidence (2026-10-01, compressor-family lane)

- RMS detection (10 ms window, Gate/Expander precedent) and sidechain HPF
  (0..200 Hz + 2nd/4th Butterworth) implemented per band per channel,
  additive with Peak + HPF-disabled defaults; the legacy Peak path is
  bit-preserved. `sidechain_hpf_hz` spec default 80 -> 0 Hz (disabled); no
  accepted preset carried the key (all explicit values were rejected).
- External sidechain and program-dependent release remain loudly rejected
  with recorded reasons (single-width bus; no reference design).
- New `tests/compressor_accuracy.rs` (10 tests, bounds predeclared in-file):
  f64 static knee/ratio/range law, absolute attack/hold/release timing over
  44.1/48/96/192 kHz, RMS/HPF oracles, lookahead/dry alignment, LR4-prototype
  adjacent-multitone phase/sum, stereo image, broadband preset equivalence,
  legacy ID/default compatibility, automation/EOF/RT.
- Broadband-vs-multiband factory trace + engine/native patch handoffs:
  `audit/muse-parallel-2026-10-01/compressor-family/shared-patches/`.
  Gates NOT executed (shell disabled); boxes stay unchecked until the
  coordinator runs `verification-request.md`. No SOTA claim.

## Fix-compat evidence (2026-10-01, compressor-family lane)

- Root review rejected the v1 HPF default change (80 -> 0 Hz + always
  forward): it would silently activate an 80 Hz HPF in previously accepted
  engine presets. V1 was NOT landed.
- Fix: additive `sidechain_hpf_enabled` flag (default false, appended
  PARAMS idx 18 / GLOBAL_PARAMS idx 22); HPF spec default restored to
  legacy 80 Hz with original range/labels; HPF active <=> enabled AND
  hz > 0. Old core/engine/native saves without the flag render legacy
  audio; 80 Hz with enabled is a working new-user setting (no
  value special-case). RMS, timing bounds, Peak default, and
  program/external rejects preserved. See `fix-compat-issue.md`,
  `fix-compat-result.md`, v2 `shared-patches/`, and
  `fix-compat-verification-request.md` (gates NOT executed here).

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/dynamics-finite-stream.md](../../audit/dynamics-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-multiband-compressor/README.md](../../crates/sotf-plugins/crates/sotf-plugin-multiband-compressor/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
