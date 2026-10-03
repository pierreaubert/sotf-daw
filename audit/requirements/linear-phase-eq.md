# Linear Phase Eq: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-linear-phase-eq`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq](../../crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq).
- Coordination group: **EQ integration**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Linear/minimum-phase FIR design, dry alignment, partitioned convolution and independent 960 single-filter/27 multiband analytical cases exist; finite-stream work is recorded.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [x] **LINEAR-PHASE-EQ-R1** — IMPLEMENT: Per-band channel/Mid/Side selection with explicit pairs, persistence, channel-aware matrix response and controls.
- [ ] **LINEAR-PHASE-EQ-R2** — IMPLEMENT: Dynamic bands with a defined update/crossfade strategy and truthful phase/latency contract; avoid silently substituting minimum-phase processing.
- [ ] **LINEAR-PHASE-EQ-R3** — INTEGRATE: Propagate new topology, pair and dynamic settings through engine, native wrappers, FFI, presets and charts.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **LINEAR-PHASE-EQ-A1** — Independent FIR convolution and complex response/group delay for single/multiple bands, minimum/linear modes and every supported tap count.
- [x] **LINEAR-PHASE-EQ-A2** — Dynamic coefficient updates must preserve continuity, bounded callback work, dry alignment and startup/EOF output across irregular blocks.
- [x] **LINEAR-PHASE-EQ-A3** — Noncommuting channel placements, reset/reprepare/refusal and old static preset/audio compatibility.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Typed settings → FIR design/preparation → async/native host processing → downstream processor → EOF/export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-linear-phase-eq --lib --tests
cargo clippy --offline --locked -p sotf-plugin-linear-phase-eq --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-linear-phase-eq`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/fir-eq-finite-stream.md](../../audit/fir-eq-finite-stream.md)
- [AUDIT.md](../../AUDIT.md)
- [crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq/README.md](../../crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Audit implementation evidence (2026-10-01, Muse session, recovery mode)

Implementation is complete in owned files; coordinator verification runs are
pending (this session had no shell access). Evidence bundle:
`audit/muse-parallel-2026-10-01/linear-phase-eq/` (`progress.md`, `issue.md`,
`shared-r3-patch.md`, `result.md`, `verification-request.md`).

- R1: `LinearPhaseEqBandPlacement` + `BandConfig.placement` +
  `LinearPhaseEqPluginParams.stereo_pairs` (`src/lib/types.rs`), pair
  validation + ordered cascade route (`src/lib/ordered.rs`), route selection
  and truthful latency/support (`src/lib/linear_phase_eq_plugin.rs`),
  append-only `band_{i}_placement` params + `BAND_TEMPLATE` entry
  (`src/params.rs`), `channel_complex_response` /
  `channel_group_delay_samples` chart APIs.
- R2: `snapshot_config` / `prepare_band_update` (off-thread design) /
  `commit_prepared_update` (allocation-free prime + fixed 513-frame exact-0/1
  crossfade) / `take_retired_route` / `update_in_progress`
  (`src/lib/linear_phase_eq_plugin.rs`, `src/lib/types.rs`). Phase and
  latency are fingerprinted and invariant; topology/count/placement/length/
  phase/auto-gain changes stay structural.
- R3: shared patch + integration instructions delivered at
  `audit/muse-parallel-2026-10-01/linear-phase-eq/shared-r3-patch.md`
  (FFI count test + state merge, engine settings + converter, NIH builder,
  chart API, preset/dynamic notes). No shared files edited; application by
  the shared owner is pending, so R3 stays open.
- A1: `tests/analytic_response.rs` additions (all 4 tap counts × both phases
  × 44.1–192 kHz: 0.05 dB magnitude, complex real-positive 1e-4, linear
  group-delay slope < 0.5 samples, min-phase peak characterization;
  multiband group-delay flatness on every tap count) plus per-channel
  response-vs-streamed-impulse cross-checks (`src/lib/placement_tests.rs`).
- A2: blend-reference continuity/post-blend exactness, block-partition
  invariance, dry exactness, EOF exactness, refusal atomicity, reset/rate
  lifecycle, and (0,0) alloc/free accounting for commit+xfade+drain+reset
  (`src/lib/dynamic_tests.rs`, `tests/dynamic_updates.rs`).
- A3: exact ordered-cascade references incl. order noncommutation with a
  setup guard, refusal/reset/reprepare coverage, legacy preset/audio
  compatibility (`src/lib/placement_tests.rs`, `tests/channel_placement.rs`).
- Whole-chain acceptance and the Astra review gate remain for the
  coordinator: owned-crate gates first (`verification-request.md`), then the
  shared-owner patch application + workspace gates.

## Fix round R1 (2026-10-01, Muse session, recovery mode)

Independent Muse review (`audit/muse-parallel-2026-10-01/linear-phase-eq/review.md`,
replacing Astra per current user instruction) found the crate non-compiling
plus owned DSP/test gaps; dispositions are in
`audit/muse-parallel-2026-10-01/linear-phase-eq/fix-r1-result.md`. All
actionable owned findings were fixed in owned files only (no shared edits):
compile errors, single-channel-excitation response semantics, full-support
priming + history capacity, dead commit-path FFT removal, independent
post-blend/drain legs, full A1/A3 matrices, chart-during-blend contract, and
xfade naming. R2 is narrowed to DSP-level in owned docs (no host automation
path adopts the update API yet; host band controls stay structural with
truthful refusal); R3 and whole-chain acceptance remain open pending the
shared owner. Coordinator rerun pending; no test passes are claimed here.

## Fix round R2 (2026-10-01, Muse session, recovery mode)

`validation-r2` stayed red on mechanical owned-crate issues only: one E0277
(`unwrap_err` on the non-`Debug` plugin type in
`tests/channel_placement.rs`) plus 4 lib and 9 lib-test clippy lints
(`too_many_arguments`, `assign_op_pattern`, `needless_range_loop`,
`explicit_auto_deref`, `type_complexity`, `chunks_exact_to_as_chunks`).
All 14 are fixed in owned files with behavior-preserving edits (see
`audit/muse-parallel-2026-10-01/linear-phase-eq/fix-r2-result.md`): no
bound, tolerance, oracle, reference or test-inventory change, no shared
edits. Checkboxes unchanged; coordinator rerun pending, no passes claimed.

## Fix round R3 (2026-10-01, Muse session, recovery mode)

`validation-r3` reported 5 owned-crate test failures with clippy green: a
real realtime violation (commit freed the prepared base snapshot: 2 frees),
two placement-test setup gaps (pre-latency comparison window, disjoint-band
noncommutation pair), and two analytic-test setup gaps (192 kHz band above
the 20 kHz ceiling, 100 Hz magnitude probe inside the short-FIR main lobe
at 96 kHz/1024 taps, measured 0.061 dB vs the 0.05 dB bound). All 5 are
fixed in owned files (see
`audit/muse-parallel-2026-10-01/linear-phase-eq/fix-r3-result.md`): the
base snapshot is stashed into the target banks for off-thread reclamation
(signatures unchanged, error paths documented as control-thread events);
test setups corrected with Fourier/MIMO evidence; no bound weakened, no
case discarded, no shared edits. Checkboxes unchanged; coordinator rerun
pending, no passes claimed.

## RT-refusal follow-through (2026-10-01, Muse session, recovery mode)

The coordinator ruled the R3 success-only stash insufficient: a rejected
commit still consumed/dropped `PreparedBandUpdate` and allocated a `String`
error on the calling thread. This round adds the ownership-preserving
realtime entrypoint `try_commit_prepared_update(&mut Option<...>)` with the
allocation-free `Copy` typed error `CommitRefusal` (every refusal retains
the prepared update for correction/retry; all validations run before
priming or live-state mutation), keeps `commit_prepared_update` as the
explicit control-thread wrapper, and proves success plus every refusal at
(0,0) alloc/free with populated history plus retry-after-refusal
regressions through the real error branch (see
`audit/muse-parallel-2026-10-01/linear-phase-eq/rt-refusal-result.md` and
`host-adoption-rt-commit.md`). No bound weakened, no case discarded, no
shared edits. Checkboxes unchanged; coordinator rerun pending, no passes
claimed.

## Fix round R5 (2026-10-01, Muse session, recovery mode)

`validation-r5` reported 4 owned-crate failures with clippy green. Three are
fixed in owned tests only (see
`audit/muse-parallel-2026-10-01/linear-phase-eq/fix-r5-result.md`): the
inactive-slot guard gets a -18 dB Mid cut, the noncommutation guard gets a
2 kHz Left-cut/Mid-boost ±18 dB pair at the stimulus fundamental, and the
retry test reclaims the twin retired route — all bounds unchanged, no
production edits. The fourth (multiband 96 kHz / 1024 taps / 500 Hz,
-0.246 dB vs 0.05 dB) is diagnosed with independent Fourier derivation as a
short-FIR resolution limit (2.67 bins across the Q2 peak) and left failing
with exact evidence; no skip, loosening, tap/latency change, or case removal.
Independent `review-rt.md` (pre-fix source) is incorporated. Checkboxes
unchanged; coordinator rerun pending, no passes claimed.

## Current contradictory evidence supersedes historical completion marks

R2 remains incomplete: the prepared update/crossfade API exists at DSP level, but live host automation does not adopt it. A1 remains incomplete: the original 96 kHz/1024-tap multiband linear-phase case fails the 0.05 dB response bound. R10 executed an independent dense witness search; its best error was 0.24094 dB, with no passing candidate. This result does not prove infeasibility and does not authorize changing taps, phase, latency or tolerances. See audit/continuation-2026-10-01/linear-fir-r10/witness-r10.json and gate-results.json. Historical DSP evidence for other checked items does not close R3 or whole-chain acceptance.
