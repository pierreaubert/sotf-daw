# Declick deferred backlog (release-stabilization)

R48/R49 accuracy research, deferred per user release priority (2026-10-03).
Nothing here is silently dropped: each item names the experiment, the key
derivations, why it deferred, and reactivation criteria. Pre-revert sources
are frozen at `/tmp/sotf-release-freeze-20261003` (root-readable); lane
derivations live in `consumers-r46-result.md` through `consumers-r49-result.md`
plus `review-r8.md` (R46 acceptance) in this directory.

Ignored characterization tests keep running the deferred stimuli on demand:
`cargo test -p sotf-plugin-declick -- --ignored` (add
`SOTF_TEST_DATA_ROOT=/home/pierre/src/all_of_sotf/sotf/data_tests` for corpus).

## DECLK-DEFER-01 — asymmetric-quadratic reconstructive estimator (R48 + R49 P6)

- What: band-path-only emission baseline replacing the mixed median with a
  Lagrange quadratic through pre-far/pre-near/post-far quarter medians
  (weights -65/208, 169/144, 65/468; post-near excluded as click-ring zone,
  pre-near ring-free by causality), four fallbacks (drain/startup/past-join/
  flip-count), plus the R49 -0.25q median-bias correction (divided
  differences on the same centers; linear-exactness preserved).
- Derivations: mixed-median curvature bias 20.5q matches a 1kHz tone at
  0.035 exactly (measured gates-r47); LTI closure pred==err to 6 decimals;
  quarter-median +0.25q bias proof; symmetric-4 rejection (post-near ring
  contamination); ratio-gate rejection (conflates peaks with oscillation).
- Measured: piano 3-band worst 0.0555 (R47) with full coverage; estimator
  predicted ~0.005-0.015. Never validated green (R48 gates: 2-band PASS
  0.0216, 3-band 0.025852 single-slot red; tone/quad probes red on
  detector-envelope, not estimator math).
- Why deferred: release needs the R47-validated median, not new estimator
  behavior; remaining work (ring-tail post_far, DEFER-05) is research.
- Reactivate: restore from freeze, re-run estimator + corpus gates, then
  continue with DEFER-05.

## DECLK-DEFER-02 — detrended shape (R49 P1)

- What: `slope_pre` for every core (pure move, sup values identical) with
  bridge and excursion detrended exactly as R29 detrends the veto (reuses
  TONE_DETREND_SPAN; ratios frozen). Target: ring-plus-flank excursion
  overflow (440/ph4/b3-b0 len 9 > 6) and slope-conflated bridges.
- Derivations: b2/b3 split explained by residual-relative bridge
  (shares 0.2/0.115 give allowances 0.15/0.086 straddling bridge ~0.13);
  precision argument (veto untouched + shape-independent backstop, level
  still gates clean, steps/drum verdicts preserved).
- Why deferred: root R49 gates showed 11 OLD regressions from the shape
  change — recall expansion needs drum/tail-gated care post-release.
- Reactivate: restore from freeze, fix the 11 regressions with
  clean-safety pins, re-verify H1/H2/H4/drums/A1.

## DECLK-DEFER-03 — strict real-corpus 0.025 accuracy gate

- What: `declick_corpus_music_repair_accuracy` (ignored;
  `run_corpus_matrix(true)`) plus ignored `diagnostic_r44_corpus_slot_anatomy`.
  Red since R43 (piano slot 13824 ch0 0.117). Damage/finite legs stay ACTIVE
  (`declick_corpus_music_damage_and_finite`) as release invariants.
- Known limit (documented, not waived): multiband piano repair reaches
  ~0.026-0.056 on loud tonal slots with R47 DSP (coverage complete per v2;
  residual is median curvature bias, DEFER-01's target). Fullband/legacy
  0.041177 frozen by design.
- Reactivate: when DEFER-01/05 land; un-ignore and close at 0.025.

## DECLK-DEFER-04 — tone/quadratic/ramp requirement probes

- What: ignored `r48_tone_estimator_holds_corpus_bound` (sens-1 control +
  stereo quadrature + decision rows) and
  `r48_quadratic_estimator_is_curvature_exact` (flat-start/C1-blend hygiene +
  decision rows). Cells, amplitudes, bounds unchanged; characterization only.
- Derivations: detector-envelope map (level-marginal crossings, peak
  veto-trips, shape caps) with per-phase analytic margins; quad floor
  inflation from the false "wings small" R48 claim (admitted test bug,
  fixed by hygiene). The R48 ramp probe is also deferred: the restored
  release path measured errors 0.00225–0.01788 against its experimental
  1e-4 bound. The earlier claim that core median linearity guaranteed this
  end-to-end result was incorrect; original test inputs and bound remain.
- Reactivate: with DEFER-01/02; un-ignore when the detector+estimator
  stack provably covers the probe regimes.

## DECLK-DEFER-05 — 31808 ring-tail correction (designed, unimplemented)

- What: post_far LTI tail subtraction for slow bands (data-driven decay
  from post_near, music-removed via causal pre anchor). Analytic c=0.23
  confirmed two ways (share onset + edge ratio); predicted post_far ring
  ≈ +0.021 on b0-ch0 (≈ +0.003 quad bias, 3x the +0.0009 gap).
- Why deferred: needs engagement-row confirmation that never executed
  (R49 rows were removed with the estimator before gates ran).
- Reactivate: after DEFER-01; confirm magnitudes via restored engagement
  rows, then implement bounded tail subtraction.

## DECLK-DEFER-06 — sens-5 detector envelope + peak-veto doctrine (finding)

- What: carried finding, not a bug. Sens-5 sup bars (frozen legacy) cannot
  see 0.5-clicks on steep/fast loud tones; peak veto-trips trade recall
  for precision with stereo link-cover as the design net (measured corpus
  23488-ch0 mechanism). Synthetic stimuli must respect this envelope;
  corpus piano (slow-dominant) lives inside it.
- Reactivate: only if requirements demand mono loud-tone recall — needs
  veto/threshold surgery with drum-gated precision proof (high risk).
