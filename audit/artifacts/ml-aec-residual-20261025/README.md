# Native AEC residual-mask implementation checkpoint

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The model is trained and native prediction is implemented behind the optional
`neural-residual` feature. No learned stage is connected to live AEC audio yet.
The existing adaptive/classical path is unchanged. This is an implementation
checkpoint, not completion of the hybrid AEC or full 31-plugin/five-program goal.

## Authoritative evidence

- `corpus-manifest.json`: 32/16/16 train/development/holdout scenes, source channel
  and native array hashes, seeds, paths, rates, echo-path and timing conventions.
  All 64 microphone/reference input hashes differ. Full binary scene/feature
  arrays stay in the external temporary ML cache and can be regenerated.
- `source/`: original generation/training Python and native exporter preimages.
  Later source-channel integrity checks do not alter generation, fitting or
  prediction equations. `current-*-source-sha256.json` pins final sources.
- `generate.json`, `train.json`, `evaluate.json`: terminal successful commands.
  Weights were selected exclusively on development and frozen before final arrays
  were opened. No final-label tuning, weight rewrite or model replacement occurred.
- `final-reference/report.json`: sixteen synthetic final scenes; adaptive-only,
  classical post-filter and causal neural mask replay, aligned by exactly 256
  samples. Reconstruction independently checks native complex FFT normalization.
- `independent-quality.json`: independent score reduction and conditional neural
  near-end component decomposition. It does not treat mixture power as ERLE.
- `native-parity-r2/report.json`: native/NumPy comparison on 770 frames (12,320
  band gains), including real native features and bounded random/corner probes;
  maximum gain error 4.77e-7. Native prediction allocation assertions are active.
  This revision also verifies source hashes before oracle checks.
- Both parity reports independently recompute near-end oracle gains and importance
  weights using NumPy complex FFT arithmetic for every scene. Fixed tolerances
  are 1e-4 gain and 1e-3 importance; all pass.
- `native-generation-replay.json`: native library/executable rebuilds reproduce
  all five output array hashes exactly on selected 16/48 kHz scenes after the
  non-numerical Clippy `as_chunks` cleanup and separately gated model API addition.
- `native-all-tests-r2.{json,txt}`: 55 library tests plus 13 integration tests pass,
  including the independent echo-path, latency, callback partition and drain QA.
  The additional QA-view lifecycle regression is recorded separately.
- `qa-view-test.*`, `native-clippy-final.*`, `default-check.*`: final lifecycle
  regression, strict all-target Clippy and default-feature build validation.
- `python-checks.json`, `python-check-*.txt`: independent scalar inference,
  invalid input/model and exact ref-silent reconstruction tests, Ruff and format.

The first strict Clippy attempt (`native-clippy.*`) failed on new constant-size
`chunks_exact` calls. The corrected sources pass; replay hashes above establish
unchanged generated data. Early native tests and the initial parity run remain
as chronological evidence, not substitutes for later checks.

## Quality result

The classical post-filter wins 10 of 12 steady echo comparisons. The neural mask
helps both drift cases and the immediate two delay jumps, but has substantially
less steady suppression in other scenes. At 48 kHz balanced double-talk onset,
neural near-target SNR is 0.71 dB versus classical 13.73 dB. Conditional neural
near-component projection loss is at worst about 0.103 dB: the onset problem is
dominated by residual echo, not large near-end attenuation in these fixtures.
Do not rank near-only floating-point noise-floor differences as quality wins.

The outcome is **no automatic adoption**. A bounded worker with stale-result
rejection and host wiring remains required. A future hybrid should keep the
stronger classical path and improve or condition proposals using development
data, followed by a fresh final holdout. Natural speech/device-domain acceptance,
listening and platform runtime budgets are still open.

All training scenes are project-generated voiced/noise signals; no external
recording or pretrained weights entered this model. Model and synthetic data
use GPL-3.0-or-later, as this workspace does. The model README contains exact
feature equations, invocation and remaining implementation scope.
