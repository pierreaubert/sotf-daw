# Multi-band EQ inverse estimator

- Objective: initialize exact offline response fitting for four peaks plus two S=1 shelves; preserve exact native rendering.
- Method: supervised inverse regression with differentiable float64 native-response MSE and 0.1 normalized parameter loss. Direct gradients suit this small differentiable regressor; no LLM policy optimization is involved.
- Prior: single-peak experiment and its literature review remain frozen. A new architecture is necessary for the 512 input/16 output contract; the old checkpoint is incompatible.
- Data: synthetic effective filter identities, disjoint train/development/test seeds, 44.1/48/96 kHz. Flat is a deterministic no-op, excluded from split metrics. Inactive parameters are canonicalized before identity hashing.
- Selection: development-only best checkpoint; sealed test scored after reloading the selected checkpoint. No tracker environment is configured; local epoch metrics are streamed.
- Smoke: 10 train + 10 development + 10 test, 30 epochs; serialized checkpoint reload succeeds and held-out response score is finite/nonzero. Raw test MSE 3.73409254 dB² versus flat 32.61046594; no generalization claim from this smoke.
- Full run: 8192 train, 1024 development, 1024 sealed test; Adam 0.001; batch 128; 100 epochs maximum; patience 20; CPU 2 threads; seed 20261024 plus split offsets 301/701/1103, weights seed 20261025.
- Acceptance: independent analog-bilinear physics; bounded exact refinement, independent SciPy evaluation, stable poles, sampled RMS <=0.1 dB and max <=0.5 dB, sampled dense gain <=24 dB. Rejection emits no filter parameters; no response/speed/quality advantage claim without the comparisons.
