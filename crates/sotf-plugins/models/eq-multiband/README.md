# Experimental six-band offline EQ initializer

A trained 512 → 256 → 128 → 16 MLP proposes four peaking bands and two S=1
shelves. Inputs combine 511 response samples and the sample-rate feature. It
runs only in the offline Python tool; audio still uses native deterministic
filters. Proposals go through bounded exact refinement, stable-pole checks,
independent SciPy evaluation, supplied-sample RMS <=0.1 dB / maximum <=0.5 dB,
and a sampled dense +/-24 dB response guard. Rejected fits export no parameters.
AutoGain remains disabled. These sampled checks are not continuous guarantees.

Example (Python environment with PyTorch, NumPy and SciPy):

```sh
python scripts/ml/eq_multiband.py fit --weights crates/sotf-plugins/models/eq-multiband/weights.json --target target.json --output proposal.json
python scripts/ml/verify_eq_multiband.py crates/sotf-plugins/models/eq-multiband
```

Targets supply `sample_rate`, ascending `frequencies_hz` covering 20–20000 Hz,
and matching `target_db`. Supported fit rates are 44.1–192 kHz; 44.1/48/96 kHz
are the training rates. Parameters are returned under `parameters` only when
`accepted` is true; consumers must check command status, acceptance and target
identity before review/import. The optimizer compares the neural seed with two
classical seeds and selects a qualifying candidate. It does not apply audio.

The run used 8192 training, 1024 development and 1024 initial-test curves,
selecting epoch 98 by development loss.
The initial test seed overlaps the earlier ten-example smoke test; preserve
`training-report.json` as preliminary. The authoritative `final-holdout.json`
uses 1024 new identities, disjoint from all earlier splits, scored after weights
were frozen: MSE 0.451479 dB² versus flat 23.864971 dB². This is synthetic raw
response accuracy, not measured-room, listening, or refinement superiority.
The frozen recipe and split identities make the model reproducible.

The refinement comparison is a negative result for adoption: neural seeds
qualified in 7/18 fits, classical edge seeds 15/18, and the combined pipeline
16/18. Median solver residual calls were 1000 versus 631 for the edge seed.
Observed solver times exclude initializer/import overhead; no speed advantage is
claimed. This model remains experimental. Better refinement-aware initialization
and broader validation are required. The two rejected in-family cases remain
recorded. A jagged out-of-family target was also rejected without parameters.

All three declared native acceptance fixtures qualified, with their best
candidates selected classically. Native magnitude/phase at 44.1/48/96 kHz matched
independent analog/bilinear references within 4.92e-7 dB / 3.31e-8 complex error.
`validation.json` and `audit/artifacts/ml-eq-multiband-20261024` hold the evidence.
The model is GPL-3.0-or-later and uses only synthetic filter responses.
