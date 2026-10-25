# Offline loudness-plugin preference fitting

Issue10. Implements a per-listener spectral-preference GP versus explicitly
supplied identity-reference SPL. It exports a native Manual-mode shape proposal
for a new matched audition, with gain/uncertainty/native-response gates and
abstention. No production manual/ISO/Auto DSP, calibration behavior, defaults,
volume controls or audio-thread inference were changed.

All committed sessions are `synthetic_validation`. There are no real listener
observations, recordings or measured calibration data here. Confirmed calibration
fields in those fixtures describe the synthetic protocol only. Real context,
calibration and listening-policy metadata are self-reported and remain subject
to external measurement verification. This is not phon/sone/LUFS estimation or
an equal-loudness model. The model/usage guide is under
`crates/sotf-plugins/models/loudness-preferences`.

The RBF GP is conditioned on a zero reference anchor, with fixed length10dB,
priorSD3dB and minimum noise0.25dB. It fits three explicit native gains for a
fixed auditioned shelf/peak shape. Programme-level cells aggregate repeats
without shrinking the noise floor, and each programme has equal total nominal
likelihood weight. Conditional variance is combined with leave-programme-out
prediction errors and200 programme bootstrap fits. A zero/zero-variance anchor
is imposed by definition; it does not prove certain human preference.

`protocol-initial.json` and `initial-validation-source.py` preserve the first
frozen validation source. Review then found that reference-only programmes could
supply the programme count. A validation-only coverage rule now requires each
programme to include at least3 non-reference levels. `support-guard-replay.json`
proves all24 original model reports and all5 functional core reports remain
exactly unchanged. It changes admission for unsupported designs, not kernel,
learning equations, policy constants or the original measured results. The
current `protocol.json` pins the final source and original kernel/policy values.

Fresh simulation:8 generated sessions, two correlated level queries each, plus
4 contradictory and4 high-uncertainty sessions. All16 consistent queries passed
statistical admission; all8 negative controls abstained. Known-mean median error
was0.04381dB GP versus0.09007dB anchored linear, with identity also recorded.
This small fixed synthetic function family cannot establish real listener
benefit, general GP superiority or human-confidence coverage. No thresholds
were tuned on these results.

Functional CLI cases (`functional-commands.json`): consistent and reference
sessions export; contradictory, uncertain and out-of-span sessions abstain with
no preset. Invalid calibration/quantity metadata fails before creating output;
rerunning against an existing export refuses overwrite. The proposed preset
preserves the native peak-normalization policy and disables AutoGain/Auto
calibration. External audition matching is not applied by the exporter and must
be repeated before evaluating the new shape.

Native qualification renders131072samples/channel of the actual2-channel manual
impulse under callback allocation guards. It checks equal channels, zero
algorithmic latency, finite output, negligible tail, complex agreement with an
independent analog/bilinear five-biquad cascade, and dense±12dB response bounds.
It tests20Hz–20kHz and the native normalization grid. Frequency-domain peak
normalization is not an instantaneous ceiling or an acoustic-SPL guarantee.
The consistent fixture's max complex error is1.54e-8 and magnitude error1.71e-7dB.

Required successful gates:
- `native-build.json`:release build and strict Clippy of QA-only renderer.
- `native-tests.json` / `.stdout.txt`:118 native plugin tests, including unchanged
  ISO reference, normalization, calibration and realtime lifecycle behavior.
- `python-tests.json`:9 posterior/group/calibration/abstention/native tests;
  native responses checked at44.1/48/96/192kHz with partial callbacks.
- `final-source-checks.json`:focused Ruff/rustfmt checks.
- `independent-analysis.json`:24 independent dense posterior solves,120 bootstrap
  fits and native preset reopens; max mean discrepancy1.69e-13dB, variance3.56e-15,
  bootstrap3.21e-13dB. Reopens preserve native response at different callbacks.
- `invalid-cli-validation.json` and `support-guard-replay.json`:contract,
  overwrite and support-rule regression evidence.

The earlier successful7/8-test checkpoints are retained in `python-tests-r1/r2`.
No test or admission bound was loosened. Source snapshots/hashes cover fitting,
validation and the native renderer; generated impulse files stay in the external
cache. Their request/report evidence and package versions are recorded.

Reproduce from repository root in the ML Python environment:

```sh
python -m unittest discover -s scripts/ml -p test_loudness_preferences.py
python scripts/ml/evaluate_loudness_preferences.py --artifacts /path/to/fresh-directory-containing-protocol
python scripts/ml/analyze_loudness_preferences.py --artifacts audit/artifacts/ml-loudness-preferences-20261025 --renderer /path/to/qa-loudness-preference --work /path/to/new-reopen-directory
```

Set `SOTF_LOUDNESS_PREFERENCE_RENDERER` to include native tests. The model README
contains the actual-session format and CLI. Real controlled listening and
uncertainty calibration, broader contextual shifts, UI/import and any live
volume-dependent personal curve remain unfinished. Source/synthetic fixtures
are GPL-3.0-or-later; no external audio, human data or pretrained weights are
redistributed. The complete ML audit remains active.
