# Dynamic Eq: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: recovery session `01a0f6c9` (2026-10-01).
Claim recorded in `audit/muse-parallel-2026-10-01/dynamic-eq/`; the README
ownership table is intentionally untouched per the parallel-worker rule.
In-crate R1/R2/A1/A2/A3 implemented; R3 handed to shared owners via concrete
patch; whole-chain acceptance pending shared integration and platform lanes.
Fix-r1 (2026-10-01, session `01a0f708`): every actionable owned finding in
the independent Muse review (`../muse-parallel-2026-10-01/dynamic-eq/review.md`)
is fixed in source (4 compile errors, empty-pairs doc/probe, audio timing
test, QA smoke, R4 provenance); coordinator rerun pending. Nothing here
claims passing tests or reviewed completion.
Fix-r2 (2026-10-01, session `01a0f75e`): validation-r2 ran red (69 passed,
2 failed, clippy clean); both failures are repaired as owned test-oracle
corrections with mathematical evidence and new prefix-bound regressions
(see `../muse-parallel-2026-10-01/dynamic-eq/fix-r2-result.md`) — no
production change, no weakened bound. Coordinator rerun pending.

- Package: `sotf-plugin-dynamic-eq`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-dynamic-eq](../../crates/sotf-plugins/crates/sotf-plugin-dynamic-eq).
- Coordination group: **EQ integration**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Eight peak/low-shelf/high-shelf bands and bounded independent shelf/core/native callback evidence exist (AUD139). Shelf implementation and matched CPU work must not be restarted from older inventory rows.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [x] **DYNAMIC-EQ-R1** — IMPLEMENT (in-crate done 2026-10-01; chain gates pending): Per-band Stereo/Left/Right/Mid/Side selection with explicit pair geometry and deterministic band order. Evidence: `DynEqPlacement`, `ROUTING_PARAMS`, `stereo_pairs`, routed detection/processing in `src/`; tests `src/lib/tests/routing.rs`, `tests/routing_tilt_public.rs`. Empty-pairs bypass documented in the crate README "Supported configurations" section with a probe test (fix-r1).
- [x] **DYNAMIC-EQ-R2** — IMPLEMENT (tilt in-crate done 2026-10-01; spectral via coordination note; coordinator ruling on coordination-only spectral requested per review P1-4): Tilt and spectral dynamics with explicit detector/response and cut/boost laws; coordinate reusable DSP with EQ and Spectral Compressor. Evidence: `DynEqShape::Tilt`, `design_tilt_coefficients`, law docs in crate README; tests `src/lib/tests/tilt.rs`; STFT-stays-in-SpectralCompressor note in `../muse-parallel-2026-10-01/dynamic-eq/shared-integration-patch.md`.
- [ ] **DYNAMIC-EQ-R3** — INTEGRATE: Finish packaged native, consuming-host rebuild, mounted band controls/curves, presets and applicable AU routes. Session status: concrete patches + sample preset + task list delivered in `../muse-parallel-2026-10-01/dynamic-eq/shared-integration-patch.md`; shared files untouched (ownership rule).
- [x] **DYNAMIC-EQ-R4** — AUDIT (done 2026-10-01): Compare per-band timing, external sidechain and above/below-threshold quadrants against current primary manuals; record which are already supported before implementing genuine gaps. Evidence: `../muse-parallel-2026-10-01/dynamic-eq/audit-r4-manual-comparison.md` (Pro-Q 4 + Nova GE manuals; follow-ups DYN-EQ-F1/F2/F3 recorded, not silently implemented).

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [x] **DYNAMIC-EQ-A1** — (tests written 2026-10-01; execution unverified, shell disabled): Independent complex held-band/blended response, absolute detector threshold and gain-reduction sweeps for peak/shelves/tilt and new modes. Evidence: `src/lib/tests/tilt.rs` (analog-prototype complex reference, held recurrence, full-band reference, absolute DC sweeps, DC-step timing); peak/shelf references preserved in `src/lib/tests/shelves.rs`.
- [x] **DYNAMIC-EQ-A2** — (tests written 2026-10-01; validation-r2 exposed 2 oracle failures, corrected fix-r2 with saturation/settling evidence; rerun pending, shell disabled): Cut/boost, attack/release, linking and per-band routing under automation at 44.1/48/96 kHz; compare emitted audio, not only meter values. Evidence: `src/lib/tests/routing.rs` (bit-exact isolation, M/S laws, pair geometry, order determinism, automation at 3 rates with 150 ms settling extension, linked-vs-unlinked emitted audio, empty-pairs bypass probe) + `src/lib/tests/tilt.rs` (meter timing + emitted-audio 63%/37% timing confirmation with 5 ms attack settling window and prefix-bound regressions).
- [x] **DYNAMIC-EQ-A3** — (legacy preserved + new retry tests 2026-10-01; CPU rerun requested): Keep legacy peak waveform compatibility, failed structural restore history, restart/retry and matched before/after CPU evidence. Evidence: `tests/aud139_*` fixtures untouched (only additive `stereo_pairs: None` literal fields); new-field rejection/retry in `src/lib/tests/routing.rs`; CPU rerun in `../muse-parallel-2026-10-01/dynamic-eq/verification-request.md`.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Application band settings/preset → engine factory → Dynamic EQ → host/output, plus actual packaged CLAP/VST3 and applicable AU consumers.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-dynamic-eq` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-dynamic-eq --lib --tests
cargo clippy --offline --locked -p sotf-plugin-dynamic-eq --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-dynamic-eq`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent review (Muse `muse-spark-1.3-contributor` per COMMON.md override; report in `../muse-parallel-2026-10-01/dynamic-eq/review.md`): findings P0-1/P1-3/P1-5/P2-7/P2-8/P2-10/P2-11 fixed in owned source (see `fix-r1-result.md`); P0-2 shared patches and P1-4 ruling outstanding; rerun of affected gates pending. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/reviews/AUD139-astra.md](../../audit/reviews/AUD139-astra.md)
- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/handoffs/aud139-native-checkpoint-review.md](../../audit/handoffs/aud139-native-checkpoint-review.md)
- [crates/sotf-plugins/crates/sotf-plugin-dynamic-eq/README.md](../../crates/sotf-plugins/crates/sotf-plugin-dynamic-eq/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
