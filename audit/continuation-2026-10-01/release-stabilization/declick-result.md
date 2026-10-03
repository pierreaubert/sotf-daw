# Declick release-stabilization result

File-only, UNEXECUTED. Root runs the gates below. Bounded stabilization only:
R47-validated detector + emission behavior restored, experiments deferred with
IDs, no new mechanisms, no accuracy-acceptance claims.

## 1. Exact restore (production `repair.rs`)

Reverted to R47-validated behavior (review-r8 accepted; gates-r47 green
except known corpus-accuracy reds):

- R48 asymmetric-quadratic estimator block + R49 P6 bias correction removed;
  `pending_baseline = baseline` (mixed median) restored. Marker comment at
  the store site points to DECLK-DEFER-01.
- R49 P1 detrended shape removed: raw bridge/excursion/returned restored
  verbatim, `slope_pre` sup-only at the veto site restored verbatim. Marker
  at the shape block points to DECLK-DEFER-02.
- R48 estimator consts (`QUAD_W_*`, `QUAD_MAX_PRE_FLIPS`) removed with a
  pointer comment (else dead_code). Verified zero remaining references
  (`QUAD_|emit_base|q_hat|past_join|MAX_PRE_FLIPS` clean; the two
  `detrended` hits are pre-existing R36/R29 test mirrors, untouched).
- KEPT: R46 bounded-join arm + M3 comment (R8-accepted), v2 test with F8.1
  assertion (R47 green), R45 NIH 9-controls/restart (no Declick-side NIH
  edits in R48/R49 — nothing to revert), corpus ID3 parser + fail-on-error
  reader + `manual_map` lint fix, ramp linear-exactness test (active: the
  median is linear-exact, still passes).

## 2. Deferred probes (ignored characterization, not deleted)

- `diagnostic_r44_corpus_slot_anatomy` → `#[ignore]` (DECLK-DEFER-03).
- `r48_tone_estimator_holds_corpus_bound` (R49 sens-1/quadrature/rows form
  kept) → `#[ignore]` (DECLK-DEFER-04).
- `r48_quadratic_estimator_is_curvature_exact` (R49 wings/rows form kept) →
  `#[ignore]` (DECLK-DEFER-04).
- `diag_r49_decision_rows` helper kept (ignored tests still compile and
  need it under `--ignored`).
- Corpus gate split (clean separation per PLAN step 2):
  `declick_corpus_music_damage_and_finite` ACTIVE (release invariants:
  damage 0 violations + render finite) and
  `declick_corpus_music_repair_accuracy` `#[ignore]`d (DECLK-DEFER-03),
  sharing `run_corpus_matrix(assert_repair)` — the ignored half is
  behavior-identical to the old failing gate.
- v2 engagement rows removed with the estimator (pointer comment;
  code in the freeze backup). No preexisting R47-green test ignored.

## 3. Deferred IDs (backlog: `../declick-boundary-accuracy-r1/deferred-backlog.md`)

DEFER-01 quad estimator (+P6 bias); DEFER-02 detrended shape (caused the
11 R49 regressions — needs drum/tail-gated care); DEFER-03 strict 0.025
corpus gate; DEFER-04 tone/quadratic probes; DEFER-05 31808 ring-tail
(designed, needs engagement confirmation); DEFER-06 sens-5 envelope +
peak-veto doctrine (carried finding). Full derivations + reactivation
criteria in the backlog; sources frozen at
`/tmp/sotf-release-freeze-20261003`.

## 4. Known corpus limits (documented, not waived)

Multiband piano repair reaches ~0.026-0.056 on loud tonal slots with R47
DSP (coverage complete per v2; residual is median curvature bias, the
DEFER-01 target). Fullband/legacy 0.041177 frozen by design. No claim of
full accuracy acceptance.

## 5. Commands (root runs)

```bash
cargo test -p sotf-plugin-declick --no-fail-fast
cargo test -p sotf-player --test declick_consumers
cargo test -p plugins-nih --lib declick
cargo test --offline --locked -p plugins-nih --lib
cargo clippy -p sotf-plugin-declick --all-targets -- -D warnings
SOTF_TEST_DATA_ROOT=/home/pierre/src/all_of_sotf/sotf/data_tests cargo test -p sotf-plugin-declick --test corpus_quality -- --nocapture
```

Predicted: all green (11 R49 regressions gone by revert-to-validated;
damage/finite + parser green with data; skip-loudly green without).
Characterization (expected red, informational):
`cargo test -p sotf-plugin-declick -- --ignored` (+ data root for corpus).

## 6. Remaining risks

- Revert restores R47-exact code paths, but green is predicted until root
  gates execute — the 11 regressions resolve by construction, not yet by
  measurement.
- rustfmt could not be run (shell disabled); restored spans are R47-verbatim
  (fmt-clean) and new spans follow file style — root lint gate confirms.
- `/tmp` freeze backup is outside the repo; if it must survive reboot,
  root should relocate it before sign-off.
- No other crates touched; sibling AB/Denoiser lanes unaffected.
