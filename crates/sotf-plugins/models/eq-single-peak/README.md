# Experimental offline single-peak EQ initializer

This model proposes frequency, Q, and gain for **one peaking biquad** from 128
log-spaced magnitude samples. It does not implement arbitrary multiband EQ,
hardware coloration, loudness normalization, or any realtime inference path.
The coefficients and audio remain the existing deterministic EQ implementation.

Train and fit with `scripts/ml/eq_assistant.py` in the isolated environment
described by `scripts/ml/eq-requirements.txt`:

```sh
python scripts/ml/eq_assistant.py train OUTPUT
python scripts/ml/eq_assistant.py fit \
  crates/sotf-plugins/models/eq-single-peak/weights.json target.json proposal.json
```

A target contains `sample_rate`, `frequencies_hz`, and `target_db` arrays. It must
cover 20 Hz–20 kHz at a source rate of at least 44.1 kHz, remain below Nyquist,
and stay within ±12 dB. The model's training family has centers 50 Hz–10 kHz,
Q 0.35–8, nonzero gains of magnitude 1–12 dB, and sample rate 48 kHz. Other
rates and material require separate distribution-shift validation.

The bounded 128→128→64→3 MLP predicts log frequency, log Q, and gain. Training
uses differentiable float64 RBJ response MSE plus 0.1 normalized parameter MSE.
Its 12,000 training, 1,500 validation, and 1,500 test parameter identities are
disjoint and recorded in `data-manifest.json`. Validation selects the best
checkpoint; the test set is evaluated once after reloading that checkpoint.
`recipe.py` preserves the exact training recipe. `best.pt`, portable
`weights.json`, and their hashes are retained. Data are generated filter
responses; no external recordings or hardware measurements were used.

The learned initializer is always followed by bounded exact least-squares
refinement. If it fails, a classical peak-based initialization is tried. A
proposal is exported only when refinement converges, poles are stable, sampled
RMS error is ≤0.1 dB, sampled maximum error is ≤0.5 dB, and independent SciPy
coefficient evaluation agrees. Otherwise the tool reports rejection and emits
no plugin parameters. Flat targets produce a canonical no-op. Exported
parameters disable AutoGain and retain the ordinary biquad realization.

This is a **sampled** qualification of the supplied target points. Sparse input
cannot prove the response between samples. Native impulse rendering of the
recorded 512.3 Hz/Q2.3/−7.2 dB proposal is independently checked against a
bilinear analog prototype in `sotf-plugin-eq/tests/ml_offline_proposal.rs`.

The frozen model's synthetic test response MSE is 0.0278128 dB² before exact
refinement. The 99.34% explained response fraction compares with a flat curve,
not a classical fitter. A further frozen comparison uses 32 fresh parameter
identities at 44.1, 48, and 96 kHz, evaluating 8,193 log-spaced points plus
DC/Nyquist and center-adjacent features. Both learned and classical half-bandwidth
initializations refine successfully: maximum dense error is below 1.52e-10 dB
for each method, with the same median 24 residual calls. These observations
establish no ML speed advantage. Controlled cold-load/multistart cost,
zero/near-zero gain, noise, shelves, multipeak targets, and measured-response
generalization remain acceptance work. This artifact does not complete the
audit's full EQ or ML scope.

The design follows response-domain training motivated by
[Nercessian, DAFx 2020](https://www.dafx.de/paper-archive/details/lpNI_8szg2jo_eJO1RnKxw),
with coefficients defined by the
[Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/).
This project's model, generator, and tooling use GPL-3.0-or-later.
