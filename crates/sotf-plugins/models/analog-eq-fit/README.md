# Offline Analog EQ fitting

`scripts/ml/fit_analog_eq_hardware.py` fits a reviewed native Analog EQ preset to
paired dry/wet captures. A Gaussian process learns the native waveform objective
offline and selects expected-improvement proposals. The final preset uses the
existing deterministic plugin. There is no inference in audio processing.

The complete 15-field native preset is the template. `free` names 1–14 numeric
controls with explicit `minimum`, `maximum`, and `scale` (`linear` or positive
`log`). `models` lists canonical native models, including the template model.
All 14 numeric controls and six models are supported; categories have independent
Matérn 5/2 covariance. Analog stage controls are rounded to native f32 with free
bounds rounded inward. EQ coefficients retain their native f64 parameter values.
Unspecified controls stay fixed. A free parameter is a search variable, not an
identified physical knob.

Use `sotf-analog-eq-hardware-fit-v1` as `format`, `quiescent` as `start_state`,
`capture_manifest` and `capture_manifest_sha256` for the calibrated capture JSON,
and `renderer_sha256` for the native QA executable. The generated fit specs from
`analog_eq_cases.py` demonstrate the exact schema. Capture requirements and
calibration conventions are documented in [analog-hardware](../analog-hardware/README.md).
At least eight train, two validation and two test source groups are required.
Hashes, calibration, rights, hardware configuration, excitation, headroom and
alignment are checked through that contract. Recorded levels and sample alignment
are preserved; no resampling, fitted gain, DC removal or delay correction occurs.
Declarations require supporting capture evidence. Recorded quiescent starts must
match fresh native state.

```sh
cargo build -p sotf-plugin-analog-eq --features qa --bin qa-analog-eq-render --release
python scripts/ml/fit_analog_eq_hardware.py \
  --spec /absolute/path/fit-spec.json \
  --renderer /absolute/path/qa-analog-eq-render \
  --output /absolute/path/new-fit-directory --budget 128
```

The 40–256 budget limits unique native objective evaluations. Each source group
has equal weight, with equal record weighting within a group. Loss is squared
relative wet RMS error. Search reads training PCM only. Both methods use bounded
Nelder–Mead refinement in their best observed model; `--method classical` uses
Sobol proposals in place of GP expected improvement. The learned GP has a fixed
length scale of 0.25 and jitter of 1e-6 on normalized log10 loss. Its uncertainty
is a search heuristic, not a calibrated confidence interval. Search convergence
status is recorded separately from qualification.

`candidate.json` freezes the complete preset before held PCM is read. Every held
record must meet relative RMS error ≤0.01, absolute error ≤0.01 and output sample
peak ≤0.999, including a 4096-sample zero suffix for peak observation. The suffix
is a finite observation, not proof of an exhausted recursive tail. Reopening the
native preset with 17-frame callbacks must agree within 1e-7. Renderer and
candidate hashes are rechecked before export. Only an accepted candidate produces
`preset.json`; a rejected fit retains diagnostics and exits 2. Existing output
directories are refused. No preset is automatically applied. `surrogate.json`
stores offline observations and search policy, not a runtime audio model.

The committed `preset.synthetic.json` is an actual accepted generated-fixture
export for factory regression testing. It is not a hardware preset or a default.
At a 128-objective cap, classical and GP searches both accepted the matched
fixture (127 unique objectives each) and rejected its out-of-family ten-sample
delayed variant (126/127 objectives). Worst held relative RMS was 4.20e-5 for
classical and 1.58e-5 for GP; delayed errors exceeded 1.12 for both. All native
reopens were exact. Both refinements reached their evaluation limits. This tests
three free controls and two models over 12 generated sources; it establishes no
general GP advantage, measured-hardware accuracy or parameter identifiability.

Evidence: [audit](../../../../audit/ml-analog-eq-fit-20261025.md),
[manifest](manifest.json). Real calibrated hardware recordings, broader control
and model search, level/rate/configuration shifts and listening review remain.
