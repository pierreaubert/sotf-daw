# Paired multiband EQ refinement-label experiment

Issue 10, EQ ML proposal. **Negative result: neither checkpoint is adopted.**
The existing model and all product defaults remain unchanged.

The new offline training recipe warm-starts two models from the original frozen
weights. Both receive identical synthetic curves, batches, optimizer steps and
response loss. The control keeps the original generator parameter labels; the
candidate uses parameters obtained by refining the base network's own seeds.
The auxiliary parameter loss is masked for unidentifiable inactive coordinates.
All 256 examples remain in training; all teacher refinements improved response
error. No recorded audio or external model was used.

The 10-example smoke run produced loadable checkpoints and nonzero independent
raw evaluation. Its development rule retained epoch zero in both arms. The full
run used 256 fresh training and 12 development identities, 12 epochs, Adam 1e-4,
and 40-evaluation teacher/development refinement budgets. Both full checkpoints
selected epoch 11. Checkpoint selection ranked development sampled solver
qualification first, residual MSE second, and residual calls third; the original
checkpoint was eligible. Development qualification is a selection proxy; exports
still require all normal stability, dense-gain, accuracy and independent checks.

Final protocol and checkpoint hashes were recorded before final scoring. The
25 target probes use six independent parameter identities, three rates,
correlated rate/noise shifts, and explicit jagged/flat/near-zero controls. The
23 non-flat cases are **not 23 independent rooms or recordings**. Train/dev IDs
are disjoint from smoke, earlier model training/evaluation and final identities.

| Arm | Neural qualifications / 23 | Combined exports / 25 | Median neural residual calls |
|---|---:|---:|---:|
| Original model | 3 | 19 | 2018 |
| Equal-step original-label control | 3 | 19 | 1254 |
| Refined-label candidate | 2 | 19 | 2083 |

Raw response MSE on the six fresh generated cases falls from 0.27220247 dB² to
0.23139379 for the control and 0.23151199 for the candidate. That reduction did
not translate into better fixed-budget neural qualification. Teacher-label
response improvement therefore does not establish useful learned initialization.
The control's lower call count on this small probe does not prove a cold-runtime
advantage. No measured-target, listening or platform claim is made.

The classical searches were exactly identical across all arms. All three
pipelines accepted the same 19 cases, rejected six without controls, and retained
canonical flat behavior. Independent analog prototypes plus SciPy's bilinear
transform checked 1536 teacher sections and 57 accepted exports; 18 rejected
exports exposed no controls. Maximum teacher MSE discrepancy was 7.72e-12 dB².

`methodlog.md` records primary literature and the method decision. `protocol.json`
freezes seeds, solver budgets and source hashes. `full-command.json` and the live
training log record the 62-second paired run. Per-case final targets/proposals
are under `final-evaluation/`. The two archived `weights.json` files are offline
experimental checkpoints, GPL-3.0-or-later like the base. They are not registered
with any product or loaded by default. `teacher-labels.npz` contains synthetic
arrays only. `source/` preserves the exact recipes and dependencies.

Reproduce from repository root in the ML Python environment:

```sh
python scripts/ml/analyze_eq_refined.py --experiment audit/artifacts/ml-eq-refinement-20261025 --artifacts audit/artifacts/ml-eq-refinement-20261025 --base crates/sotf-plugins/models/eq-multiband/weights.json
python -m unittest discover -s scripts/ml -p 'test_eq*.py'
```

For a new training reproduction, use the arguments in `full-command.json` with
a fresh output directory, replacing its external smoke exclusion path with the
committed `smoke-data-manifest.json`. The equal-step seeds and all data-generator
inputs are recorded. For final replay use `evaluation-command.json`, replacing
`--experiment` with this artifact directory and `--output` with a new directory.
The analyzer accepts `--experiment` at either the original training cache or the
committed archive. Hash manifests cover weights, labels, sources and results.
The full ML audit and better refinement-aware initialization remain unfinished.
