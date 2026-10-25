# Experimental offline limiter release initializer

This small locally trained model proposes a native Limiter release setting from
an explicit, aligned dry/reference audio pair. Native processing, refinement,
waveform checks and sample-ceiling enforcement remain deterministic. It does not
choose an artistically preferred release from an unpaired recording.

**General adoption was rejected.** On a fresh equal-budget comparison, an analytic
recovery initializer qualified 20/20 synthetic probes, while the learned
initializer qualified 19/20. Median attempted native evaluations were 15.5 versus
17. The model failed the short-context probe. It remains an explicit offline
experiment; neither live DSP nor plugin defaults changed.

The initial combined neural/classical fitter qualified all 20 excited synthetic
probes and rejected all five ambiguity/mismatch controls. Its classical grid and
analytic recovery seed provide fallback when a learned proposal fails. Passing
the combined fitter does not establish a learned-model advantage. The additional
comparison used 20 new signal identities, with unchanged weights, features,
tolerances and optimizer settings, after the initial analytic raw estimates
proved more accurate than the neural ones.

The model has 32 chronological envelope/recovery features, 32 tanh hidden units,
one bounded output and 1,089 parameters. Output is `10**(2+tanh(output))` ms,
bounded to 10–1000 ms. Training used 256 synthetic signal identities, with two
release variants per identity, plus 48 disjoint development identities. The best
development checkpoint was epoch 299. No recordings or external weights were
used. Weights are licensed GPL-3.0-or-later; provenance and SHA-256 are in
`manifest.json` and the audit training report.

Use the CLI explicitly:

```sh
python scripts/ml/fit_limiter_release.py \
  --target /path/to/target.json \
  --renderer /path/to/qa-limiter-render \
  --model crates/sotf-plugins/models/limiter-release/model.json \
  --model-sha256 3c43c23e701a252af29272c731fd6ab92475c717cf7b4b4d849648672de1d999 \
  --work /path/to/new-native-work --output /path/to/new-output
```

Omit both model arguments for classical-only fitting. Supplying a model requires
its explicit expected checksum; malformed, oversized or mismatched models are
rejected before output creation. Model hashing and parsing use the same once-read
bytes. No active playback is changed. Output directories must be new. Accepted
fits produce `report.json` and a native constructor `preset.json`; abstentions
produce a report with null parameters, no preset, and exit status 2.

Target format:

```json
{
  "format": "sotf-limiter-release-target-v1",
  "alignment": "latency_removed",
  "sample_rate": 48000,
  "frames": 192000,
  "channels": 1,
  "dry": "dry.f32le",
  "reference": "reference.f32le",
  "fixed_params": {
    "threshold_db": -6.0,
    "lookahead_ms": 5.0,
    "soft": false,
    "true_peak": false,
    "isp_mode": false,
    "dual_release": false,
    "mix": 1.0,
    "feed_forward": true,
    "link_amount": 1.0,
    "oversampling": 0
  }
}
```

Paths are relative to the target JSON or absolute. Audio is raw little-endian
float32, interleaved by channel, with exactly `frames*channels` finite samples
within ±32. The arrays must already be aligned without a latency prefix. Supply
2–8 seconds at 44.1–192 kHz, 1–8 channels, and at most four million interleaved
samples. The only supported fitting profile is 1x, hard, fully wet, fully linked,
single release, with `feed_forward: true`, no true-peak/ISP, and no soft/dual mode.
Threshold is bounded to −20..0 dB; lookahead to 0..20 ms. The admitted domain is
broader than the synthetic validation coverage; other native profiles remain
unimplemented in this prototype.

Each neural and classical search has at most 24 attempted native evaluations by
default. A shared 33-point logarithmic scan and ±10% release probes check
ambiguity separately. Qualification requires relative waveform RMS ≤0.002 and
maximum absolute sample error ≤0.01, followed by a fresh native preset reopen
with a different callback size. Sampled alternatives separated by at least 10%
in release and within best RMS+0.001 cause abstention. This finite diagnostic is
an abstention heuristic, not a global continuous identifiability guarantee or
calibrated confidence interval.

Every actual native render checks the complete raw output against
`10**(threshold_db/20)+2e-6`, including drain samples, under the existing native
hard ceiling. This is a sample-peak check, not independent inter-sample peak
certification. Latency and complete finite tails are reported by the native
renderer; fitting compares the input-duration segment after removing latency.
Nominal release can differ while float32 coefficients yield the same waveform,
particularly at high rates, so control identity is not guaranteed by a good fit.

Evidence: `audit/artifacts/ml-limiter-release-20261025`. Raw arrays are retained
outside Git; generator recipes, split identities, hashes, features, weights,
reports and independent reconstruction checks are archived. Natural recordings,
independently calibrated tolerances, perception, other limiter configurations,
host/UI integration and analog-hardware coloration remain open. No general
programme, preference, hardware-emulation or realtime inference claim is made.
