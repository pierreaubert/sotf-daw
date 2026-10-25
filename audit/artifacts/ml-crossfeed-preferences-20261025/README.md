# Crossfeed offline preference assistant

Issue 10. Implements opt-in per-listener learning over explicitly auditioned
native Crossfeed settings. A regularized Bradley–Terry utility model includes
presentation-side bias. Programme groups receive equal weight; grouped bootstrap
stability and leave-one-programme-out prediction govern abstention. The CLI
returns an auditioned parameter set unchanged for review/import and leaves the
active plugin untouched. There are no pretrained weights or audio-thread calls.
The model/usage guide is `crates/sotf-plugins/models/crossfeed-preferences`.

All committed choices are synthetic and explicitly labelled `synthetic_validation`.
No user listening observations, recordings or external model weights were used.
This establishes implementation correctness, **not listener benefit or calibrated
confidence**. Controlled independent listening, policy calibration, repeatability,
application review/import UI and real-host acceptance remain pending.

Final functional CLI outputs are `functional-final-*`, with command records in
`functional-final-commands.json`: consistent choices export the exact Bauer
candidate; ties, contradictory programmes and presentation-side bias abstain
without a preset. These reports propagate the synthetic data-origin field.
Earlier `consistent/`, `ties/`, `heterogeneous/` and `side-bias/` outputs preserve
the initial functional checkpoint before origin metadata was added to reports;
the input sessions already declared their synthetic origin and decisions match.
Native fixtures are byte-identical to the final CLI session/preset/report and
are stored inside the crate so packaging has no external fixture dependency.

`pre-evaluation-source.json` pins fitter/evaluator bytes before fresh simulation.
`fresh-synthetic/protocol.json` records fixed seeds, policy and a 16-session
logistic-choice simulation (8 strong and 8 no-preference sessions). Each session
fits its own utility model on eight programme groups and predicts eight separate
held-out programme groups. It uses the model's assumed family and is not an
independent model of real listeners. Accepted correct strong cases: 7/8; one
abstention; wrong strong recommendations: 0; no-preference recommendations: 0/8.
A simple pooled-vote ranking is recorded without a superiority claim. Policies
were not tuned on these results. The 90% bootstrap stability rule is not a 90%
probability of user benefit.

`independent-analysis.json` reconstructs the likelihood and fits it with BFGS and
finite differences, independently of the production objective/Jacobian/Hessian.
It checks 16 utility fits, 128 held-programme fits and 80 bootstrap fits (five
per session), plus statistics of all stored bootstrap utility arrays and exact
accepted/rejected export behavior. Maximum utility discrepancy: 9.24e-8;
bootstrap utility discrepancy: 1.02e-7; held-programme loss discrepancy: 3.38e-9.
This is numerical agreement, not empirical confidence calibration.

Required successful gates:
- `python-checks.json`: eight Python tests, focused Ruff lint and format checks.
- `native-tests.json` / `native-tests.stdout.txt`: full QA-feature Crossfeed suite,
  128 tests passed, including existing transfer, reset, automation and heap guards.
- `native-final.json`: two new native import/streaming tests and strict Clippy
  after moving byte-identical fixtures into the crate. Four rates and callbacks
  1/31/257/1024 frames include partial final blocks; error bound is 1e-6.
- `analysis-command.json`: independent statistical fit/export checks.
- `final-source-checks.json`: final analyzer/test source lint/format verification.

Resolved historical checks remain: `native-tests-r1.*` failed because existing
release integration tests require feature `qa`; the final command includes it.
`python-checks-r2.json` records a format-only failure in the added CLI test after
its eight tests and lint passed; final formatting and checks pass. No response,
statistical or native threshold was changed to resolve either command issue.

Reproduce from repository root with the ML Python environment (NumPy/SciPy):

```sh
python -m unittest discover -s scripts/ml -p test_crossfeed_preferences.py
python scripts/ml/analyze_crossfeed_preferences.py --artifacts audit/artifacts/ml-crossfeed-preferences-20261025
python scripts/ml/evaluate_crossfeed_preferences.py --template audit/artifacts/ml-crossfeed-preferences-20261025/consistent-session.json --output /path/to/new-validation-directory
cargo test -p sotf-plugin-crossfeed --features qa --release --offline --locked -j 2
```

The fitter accepts at most 2 MiB of JSON, validates native bounds and exact
candidate identities, and hashes/parses the same session buffer. It refuses
existing output directories; rejected sessions write a report only. Context and
protocol fields are self-reported and must describe genuinely controlled,
level-matched stereo/headphone auditions. Ties are half-win soft labels. Repeated
programme IDs do not inflate independent-group count; relabelling correlated
material can still invalidate the uncertainty estimate.

Method references: Bradley and Terry (1952),
https://doi.org/10.1093/biomet/39.3-4.324; Efron (1979),
https://doi.org/10.1214/aos/1176344552. Grouping, ridge, side-bias correction and
policy thresholds are implementation choices, not validated by these papers.
All source/synthetic fixtures are GPL-3.0-or-later. Source and artifact manifests
pin byte hashes; no listening data is redistributed. The full ML audit remains
in progress.
