# Optional CREPE-tiny pitch assistance

`crepe-tiny.onnx` contains the pretrained tiny weights distributed in
[torchcrepe 0.0.24](https://github.com/maxrmorrison/torchcrepe). The original
[CREPE](https://github.com/marl/crepe) and torchcrepe notices are included.
`manifest.json` records the exact weights, ONNX, parity, and exporter hashes.
No new training has been performed. Upstream training overlap with recordings
available to this project is unknown.

Build with `sotf-plugin-pnd/neural-pitch`, `sotf-plugins/neural-pitch`, or
`sotf-engine/neural-pitch`. In `PndPluginParams` or engine `Pnd` settings, set
`neural_pitch` to:

```json
{
  "model_path": "crates/sotf-plugins/models/crepe-tiny/crepe-tiny.onnx",
  "sha256": "57209bf565bce9de445b31a175294dd2f297432864599c6aa237a895353918d3",
  "min_confidence": 0.8,
  "max_age_ms": 150.0,
  "max_deviation_cents": 100.0,
  "source_channel": 0
}
```

Also supply an explicit `reference_frequency_hz`. This is for a known,
monophonic fundamental between 32.7 and 2006 Hz, not automatic polyphonic
tuning. Neural estimates are never folded by an octave to force agreement.
The default mode and old presets remain classical. Builds without the feature
reject an explicit neural configuration instead of silently ignoring it.

The worker resamples a complete causal 64 ms source window (16–192 kHz) to
1024 samples at 16 kHz with a Blackman-windowed sinc low-pass, removes DC,
and normalizes using sample standard deviation. Windows below -60 dBFS
centered RMS are treated as silence before normalization. The network returns
360 activations. Local weighted cents around the peak decode frequency;
activation confidence is not a calibrated probability. Inputs are submitted
every 10 ms; a two-slot bounded queue sheds old frames. The worker can allocate;
the callback copies fixed-size buffers and never waits for inference.

Fresh, confident estimates within the configured deviation bound may replace
the classical observation. All other results fall back to classical analysis.
Age includes the entire analysis window, plus queued/inference delay, measured
in source samples. Model/reset/reference changes invalidate prior results.
EOF freezes the correction decision and never feeds artificial silence to the
model. Loading, reinitialization, and destruction belong on the control thread.
The existing synthesis latency is unchanged; analysis response has at least
64 ms of observation history plus worker turnaround. `PndData.neural_pitch_used`
reports the source of the last analysis decision.

Reproduce the conversion with `scripts/ml/export_crepe.py` in an isolated
environment from `scripts/ml/crepe-export-requirements.txt`. The parity fixture
compares all 360 activations from four fixed normalized noise frames against
the source PyTorch model. These fixtures and synthetic pitch tests establish
implementation behavior, not held-out quality, vibrato intent, listener
acceptance, or deadline capacity under a real DAW workload.
