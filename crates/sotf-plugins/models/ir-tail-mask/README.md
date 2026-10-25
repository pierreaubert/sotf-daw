# Experimental offline IR tail mask — rejected for general adoption

This optional experiment estimates a linked time mask for the late part of an
impulse response. It does **not** pass the audit's measured-response preservation
requirement. On generated final scenes it beats the better classical method in
only 2/48 cases. A late-reflection shift loses 9.074 dB of clean-IR reconstruction
SNR despite meeting the current response bounds. Do not use this as an automatic
IR cleaner. Convolution defaults and realtime DSP are unchanged.

The exported model is project-trained, GPL-3.0-or-later, with no external IRs,
recordings or pretrained weights. SHA-256:
`a4368ca93edb1a8029730474955ec65ad28724146acc595af036ed80c6001e39`.
There are 241 parameters (964 bytes as f32). Offline NumPy inference uses an
8→24 tanh→1 sigmoid network with output bounded to 0.05..1. Model loading checks
identity, file size, format, shapes, finite values and weight bounds. Training
uses Torch only; prediction parity maximum error is 1.04e-7.

## Offline contract

Use a float32 WAV and explicitly identify the direct-path arrival and a preceding
noise-only interval. They are measurement inputs, not inferred calibration.
The first 50 ms after arrival, and all preceding samples, remain bit-identical
as f32. There is no trimming, shift, resampling or normalization. One common
sample mask is applied to every channel; this preserves instantaneous channel
ratios, not complete inter-channel transfer functions.

Twenty-millisecond block features describe observed/noise power ratio, neighboring
blocks, an offline fitted exponential trend, elapsed time, fitted slope, a
first-difference power/color statistic and absolute noise level. A proposed mask
is interpolated in time with early samples protected. The prefix must contain
nonzero stationary noise; a simple half-prefix power check rejects large changes.
This check cannot establish that the interval contains noise alone. A provided
arrival or noise prefix can be wrong.

A candidate is admitted only if the early f32 samples are unchanged, relative IR
change is at most 5%, and magnitude/phase changes stay below 1 dB/10 degrees at
192 fixed log-frequency grid points above -20 dB per-channel peak. Finite-record
DFTs are unnormalized forward and normalized inverse, with zero padding to the
next power of two. There is no phase alignment or gain normalization. This is a
sampled response bound; it does not cover every frequency, deep nulls, or weak
late reflections. The negative shift result proves these checks insufficient.

Failure to load/validate the model or analyze the prefix exports original f32
samples. A rejected candidate also exports original samples. Output and report
paths must be new; the source IR is never overwritten. Missing model does not
silently select another model. The report states whether the learned proposal
was used and always records `adoption_recommended: false`. Review/listening and
an explicit user choice are required before loading any exported IR into a host.

The helper accepts bounded 8..96 kHz, 1..32-channel float32 input, up to 30 seconds
subject to 1,440,000 frames and 8,000,000 total samples. Evaluation covers only
mono/stereo at 16/48 kHz, with 1.5-second generated final IRs and two-second shifts.
Other accepted file geometries do not have model quality validation. Memory and
file I/O occur offline; there is no additional runtime latency or inference task
in the convolution processor.

## Evidence and limitations

Training uses 256 generated scenes; 48 disjoint development scenes select the
weights. Model bytes/hash are frozen before generating 48 final scenes and 30
shifts. There are 382 distinct input hashes. Final scenes contain decaying noise
fields and sparse early reflections; shifts add late reflections, two decay
slopes, ringing, colored noise or a stepped noise floor. These are simulated IRs,
not measured rooms or a validated reverberation-time corpus.

The neural method, smoothed Wiener attenuation, and fitted-decay attenuation all
use the same early protection, response checks and fallback. Metrics include
fallback cases. Final neural median improvement over unchanged input is 3.229 dB,
versus 3.436 dB for Wiener and 3.438 dB for decay. The neural shift median is 0 dB;
worst change is -9.074 dB. Decay also fails on one shift (-1.767 dB), so it is not
a universally accepted substitute. No final case was used to tune the model or
response limits. Subsequent development needs separate data and a new final test.

`audit/artifacts/ml-ir-denoise-20261025` preserves protocol, frozen training sources,
source hashes, all cases, training trace and independent SciPy FFT/quality checks.
The independent reducer confirms every metric, with maximum response discrepancy
2.11e-6 from the initial NumPy float32 FFT. The current helper explicitly uses
f64 for both response transforms. Replaying all 234 export arrays confirms this
numerical change and later report/schema checks did not change any final output
or admission decision. Frozen evidence remains unchanged.

A QA-only native executable loads reviewed WAV exports into the public convolution
plugin at full wet/unity gain. Thirty-six guarded native renders cover learned
and missing-model exports at 16/48 kHz, mono/stereo, and callbacks 1/193/1024.
After compensating the normal 1024-sample UPC latency, maximum IR error is 5.97e-8.
This validates loading/filter realization, not the denoiser's quality, true-stereo
routing, other rates, native host deadlines, or measured response preservation.

## Reproduction

Install `scripts/ml/ir-denoise-requirements.txt`; add `torch==2.14.1` for training.
Preserve existing output directories when rerunning experiments.

```sh
python scripts/ml/train_ir_denoise.py --output ./ir-experiment \
  --temporary ./ir-arrays --epochs 250
python scripts/ml/ir_denoise.py --input measured.wav --output reviewed.wav \
  --report reviewed.json --model crates/sotf-plugins/models/ir-tail-mask/model.json \
  --model-sha256 a4368ca93edb1a8029730474955ec65ad28724146acc595af036ed80c6001e39 \
  --arrival-ms 80 --noise-prefix-ms 60
cargo build --release -p sotf-plugin-convolution --features qa --bin qa-convolution-ir
python scripts/ml/verify_ir_denoise.py --evidence ./ir-experiment \
  --renderer target/release/qa-convolution-ir --temporary ./ir-native-check \
  --output ./ir-native-check.json
python -m unittest discover -s scripts/ml -p 'test_ir_denoise.py' -v
```

The trainer saves source copies in its `evaluation-source` directory before
generating data. Run `analyze_ir_denoise.py --evidence ./ir-experiment
--output ./ir-analysis.json` to independently recompute results from those arrays.
Measured-room structure preservation, broader domains, listening and application
review/import remain open. The Convolution ML proposal remains in progress.
