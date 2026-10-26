# Synthetic onset-assist candidate

This project-generated GPL-3.0-or-later model is an unadmitted QA experiment.
It is not enabled by production parameters. Its format is
`sotf-transient-onset-assist-v1`: six causal native features, eight tanh units,
one sigmoid output, and65 bounded coefficients.

SHA256 of `model.synthetic.json`:
`781433352e2fe070fe640909b0ebe422e785cd2523b55b3606228ea53bd0c88c`.

Train with `scripts/ml/train_onset_assist.py --renderer PATH --output NEW_DIR`.
The renderer must be built with the transient-shaper crate's `qa` feature;
the trainer freezes its own executable copy. Training/validation/final source
identities, parameters and hashes are recorded in
`audit/artifacts/ml-onset-assist-20261026/training-report.json`.
`parity.synthetic.json` contains17 independent Python inference references.

The optimizer reached its300-iteration limit. Steady-only false-positive rate
and slow-attack recall fail quality expectations. Do not infer calibration,
recorded-material quality, stereo generalization or admission from these assets.
See `audit/ml-onset-assist-20261026.md` for results and remaining requirements.
