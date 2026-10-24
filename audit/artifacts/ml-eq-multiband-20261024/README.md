# Multi-band EQ implementation and negative adoption result

The new offline pipeline trains a real six-band inverse-response model,
verifies model identity, compares bounded neural/classical refinements, and
exports only certified native parameters. The audio renderer is unchanged.
The earlier single-peak experiment and its model remain frozen.

- Full run: 8192 training, 1024 development and 1024 initial-test curves;
  Adam 0.001, 100 epochs, two CPU threads; selected epoch 98 by development.
  Training took about 59.45 seconds. Recipe/checkpoint/weights/data hashes
  are retained with all split identities and per-epoch metrics.
- The initial test seed overlapped the ten-example smoke. That initial result
  is retained as preliminary, not presented as final held-out evidence.
  `fresh-holdout.json` freezes 1024 new identities excluding all earlier splits;
  its unrefined MSE is 0.451479 dB² versus flat 23.864971 dB². No weights or
  checkpoint selection changed after that evaluation. `family-breakdown.json`
  reproduces the aggregate and disaggregates peak-only, shelf-only and mixed.
- `candidate-summary.json`: neural seeds qualified in 7/18 fresh fits,
  classical edge seeds in 15/18, combined pipeline in 16/18. Neural median
  residual calls 1000, edge baseline 631. Solver-only observed timings are not
  controlled speed results. The model is explicitly experimental; this does
  not support production adoption or a neural speed/quality advantage.
- Four sample-rate-shift fits and three predeclared native fixtures qualified.
  The jagged out-of-family target was rejected with `parameters: null`.
  Two in-family failures are preserved in `refinement-comparison.json`.
- Eleven Python checks pass across the new and old EQ tools. The new independent
  analog/bilinear checks cover both shelf conventions and peak coefficients,
  gradients, split identities, zero gain, constant gain, invalid inputs and
  preservation of existing training outputs. Corrupt model hashes are rejected.
- Two native tests pass. The six-filter cascades render in uneven 137-frame
  blocks at 44.1/48/96 kHz, comparing magnitude and complex phase response with
  independent analog prototypes. Worst errors: 4.91458e-7 dB and 3.30776e-8.
  Strict native Clippy and Python Ruff pass.

The parameter acceptance fixture was declared before full training and is
excluded from all train/development/evaluation identities. All claims remain
limited to synthetic filter-response estimation and supplied sample checks.
Measured/noisy responses, refinement-aware improvements, controlled end-to-end
cost comparisons, and user-facing review/import remain open. No held-out
recordings, listening, real DAWs or unavailable Linux/Windows hosts were tested.
