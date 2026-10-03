# Ambisonics: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse `ambisonics`** (claimed in README; table untouched).

- Package: `sotf-plugin-ambisonics`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-ambisonics](../../crates/sotf-plugins/crates/sotf-plugin-ambisonics).
- Coordination group: **Spatial/shared geometry**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Named-layout ACN/SN3D orders 1–7, mode matching/AllRAD, max-rE, dual-band processing and bounded native/engine evidence exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **AMBISONICS-R1** — IMPLEMENT: User-defined loudspeaker layouts with geometry validation, persistence, decoder preparation and application/native activation.
- [ ] **AMBISONICS-R2** — AUDIT: Establish requirements and current support for layout/decoder export, imaginary speakers, N3D/FuMa conversion and HOA rotation using primary references; implement confirmed scope gaps without assuming they already exist.
- [ ] **AMBISONICS-R3** — INTEGRATE: Finish wider native/application/platform routes, including unsupported CLAP speaker-role decisions and EOS/manager coverage.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **AMBISONICS-A1** — Independent ACN/SN3D spherical-harmonic and decoder matrices through order 7; full-sphere direction, energy/velocity vectors and conditioning.
- [ ] **AMBISONICS-A2** — Custom valid/degenerate layouts, LFE exclusion, pair/role permutations and failed preparation preserving the old decoder.
- [ ] **AMBISONICS-A3** — 64-channel sources through actual persisted graph, loaded native plugin and engine output; distinguish matrix zero-tail from recursive dual-band behavior.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

64-channel HOA file → persisted custom/named decoder setup → native/internal host → speaker channels → EOF.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-ambisonics` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-ambisonics --lib --tests
cargo clippy --offline --locked -p sotf-plugin-ambisonics --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-ambisonics`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/ambisonics-orders-4-through-7.md](../../audit/ambisonics-orders-4-through-7.md)
- [audit/native-ambisonics-orders-1-through-7.md](../../audit/native-ambisonics-orders-1-through-7.md)
- [audit/reviews/AUD135-astra.md](../../audit/reviews/AUD135-astra.md)
- [audit/ambisonics-tail-support.md](../../audit/ambisonics-tail-support.md)
- [crates/sotf-plugins/crates/sotf-plugin-ambisonics/README.md](../../crates/sotf-plugins/crates/sotf-plugin-ambisonics/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Implementation status (Muse, 2026-10-01; no executed gates)

Shell/Cargo is disabled in this session, so no checkbox above is marked passing
and no numerical error is reported as measured. Implementation is complete
within owned paths; verification is requested from the coordinator.

Fix round R1 (independent Muse review findings F1–F15/O1–O7): all actionable
owned findings are addressed in source (see
`audit/muse-parallel-2026-10-01/ambisonics/fix-r1-result.md`); shared patches
02/03 corrected, patch 04 extended with the save contract, patch 05 added.
Still unverified: the coordinator reruns all gates.

- **AMBISONICS-R1** (implemented, unverified): custom layouts in
  `src/custom_layout.rs` (`CustomSpeaker`/`CustomLayout` validation,
  `CustomDecoderConfig` serde persistence), `DecodeMatrix::build_for_custom` /
  `build_allrad_for_custom`, `AmbisonicsDecoderPlugin::new_custom` with
  transactional preparation, appended `"custom"` target choice (named indices
  0–7 unchanged, `Params` otherwise untouched). Engine/bridge/FFI/native
  activation specified as shared patches in
  `audit/muse-parallel-2026-10-01/ambisonics/shared-patches/`.
- **AMBISONICS-R2** (audit complete, export implemented): dispositions in
  `audit/muse-parallel-2026-10-01/ambisonics/issue.md`. Layout export
  (`export_json`/`import_json`) and decoder export (`DecodeMatrix::export`)
  implemented. Imaginary-speaker authoring deferred (bounded nearest-speaker
  VBAP fallback already covers out-of-hull directions); N3D/FuMa conversion
  and HOA rotation out of scope (input-processor / scene-manipulation
  concerns, not decoder scope).
- **AMBISONICS-R3** (shared, open): no owned native/application changes made.
  CLAP wide-role rejection follows the accepted AUD135 decision; engine EOS /
  manager coverage and custom native activation are specified in the shared
  patches for the integration lane.
- **AMBISONICS-A1** (tests authored, not run): `tests/decode_vectors.rs` —
  closed-form FOA oracle, full-sphere energy/velocity-vector direction
  (cosine ≥ 0.9999, fixed a priori from symmetry), AllRAD hemisphere
  inequalities, octahedron/icosahedron conditioning (rank exact, cond < 4.0).
- **AMBISONICS-A2** (tests authored, not run): `tests/custom_layouts.rs` —
  custom-vs-named matrix equivalence (≤ 2e-6), degenerate bounded-or-reject
  contract, LFE exclusion at any channel, permutation, failed-preparation
  preservation, JSON persistence roundtrip, linearity.
- **AMBISONICS-A3** (in-crate tests authored, chain open): order-7 64-channel
  custom basis/dense/tail tests in `tests/custom_layouts.rs`; loaded-native
  and engine-output legs remain shared/coordinator gates.

See `audit/muse-parallel-2026-10-01/ambisonics/result.md` for files, tolerances,
risks and the exact verification request.
