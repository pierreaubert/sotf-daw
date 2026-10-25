# Experimental offline IR tail mask — rejected for general adoption

This optional experiment estimates a linked time mask for the late part of an
impulse response. It does **not** pass the audit's full measured-response
preservation requirement. The original unprotected model beats the better
classical method in only 2/48 final cases and loses 9.074 dB reconstruction SNR
on one late-reflection shift. An optional local-noise-budget policy now prevents
such large losses in a new synthetic probe, with the model weights unchanged.
Measured-IR and listening acceptance remain open; do not use this as an automatic
IR cleaner. Convolution defaults and realtime DSP are unchanged.

The exported model is project-trained, GPL-3.0-or-later, with no external IRs,
recordings or pretrained weights. SHA-256:
`a4368ca93edb1a8029730474955ec65ad28724146acc595af036ed80c6001e39`.
There are 241 parameters (964 bytes as f32). Offline NumPy inference uses an
8→24 tanh→1 sigmoid network with output bounded to 0.05..1. Model loading checks
identity, file size, format, shapes, finite values and weight bounds. Hashing and
parsing use the same bounded byte read, so a concurrent path replacement cannot
substitute unverified weights. Training
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

## Original unprotected evidence and limitations

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

## Optional local preservation policy

Pass `--preservation-policy` and its matching `--policy-sha256` to constrain the
proposal using the supplied prefix's per-channel noise power. The selected policy
uses nonoverlapping 5 ms windows and a noise fraction of 0.5. For channel energy
`E` in a window and reference-noise budget `B = 0.5 * prefix_noise_power * frames`,
the gain floor is `1 - min(0.95, B/E)`. A common linked floor is the largest channel
floor. Removed energy in each channel/window is therefore at most `B` in exact
arithmetic. The f32 result is checked again; a budget violation rejects the
candidate. A zero-noise channel conservatively forces unchanged linked output.
The first 50 ms after arrival and preceding samples remain exact.

This is a noise-reference-relative constraint, not a bound on unknown clean IR
error. A bad or unrepresentative prefix can still invalidate the assumption.
Protection does not identify the true clean decay, provide acoustic calibration,
or guarantee every frequency or perceptual feature. Unprotected behavior remains
available for reproducibility when the policy is omitted.

Sixty **new** development scenes select window/fraction independently for the
learned, Wiener and decay methods; all three select 5 ms / 0.5. Policies are saved
and hashed before generating 72 fresh final scenes and 32 fresh shifts. Their
source hashes are disjoint from all 382 prior and 60 development inputs. The new
probe expands to 16/44.1/48/96 kHz and adds late packets, truncation, nonstationary
prefixes and linked four-channel signals. Four-channel paths are correlated
copies, not independent spatial measurements.

All 104 guarded learned exports avoid reconstruction SNR loss in this probe,
including 32 exact fallbacks (eight due to the nonstationary prefix). Accepted
exports number 72. Median improvement over unchanged input is 1.525 dB in the
72 final cases and 1.962 dB in the 32 shifts. Learned output beats the better
independently calibrated classical method in only 6/72 final and 6/32 shift cases.
This supports the protection rule more strongly than general ML adoption.
Unprotected learned output loses up to 8.905 dB on the same new cases.

Independent analysis verifies each per-channel/window budget and all sampled
response metrics. With mixture-derived decisions held fixed on known clean IRs,
worst conditional full-IR projection loss is 0.00805 dB and worst conditional
late-structure loss is 0.02514 dB. These are generated-data results. The largest
removed-energy/budget ratio is 0.94786, below one. All eight nonstationary-prefix
cases retain original samples exactly. Seventy-two native UPC renders verify
36 guarded/invalid-policy/missing-model WAV exports at four sample rates and
callbacks 193/1024. Maximum error after compensating native latency is 5.97e-8.
All 234 legacy export arrays remain identical when the policy is omitted.

Evidence: `audit/artifacts/ml-ir-preservation-20261025`. Policy SHA-256:
`1c29da25a280f3b82e2fd77e5a6d22c71bb80131e449378e7153ae3594f2d14b`.

```sh
python scripts/ml/ir_denoise.py --input measured.wav --output reviewed.wav \
  --report reviewed.json --model crates/sotf-plugins/models/ir-tail-mask/model.json \
  --model-sha256 a4368ca93edb1a8029730474955ec65ad28724146acc595af036ed80c6001e39 \
  --preservation-policy crates/sotf-plugins/models/ir-tail-mask/preservation-policy.json \
  --policy-sha256 1c29da25a280f3b82e2fd77e5a6d22c71bb80131e449378e7153ae3594f2d14b \
  --arrival-ms 80 --noise-prefix-ms 60
```

Policy files have bounded size, exact schema, finite bounded numeric controls
and an explicit raw-byte identity. Missing, malformed or mismatched policy data
exports original f32 samples with a report. Path and identity must be supplied
together. No automatic loading or normalization is introduced.

## Measured-reference added-noise probe

A further frozen probe uses paired channels from six AIR rooms, with no external
IR entering training. It adds controlled white/colored noise at 20/40/60 dB SNR
to the original recorded vectors, cast to f32 and preceded by an artificial
80 ms zero prefix to provide a known added-noise-only reference. There is no
resampling, level normalization, trimming or denoising of the recorded reference.
Original recording noise/artifacts remain in the target. The approximate prior
arrival annotation is tested at +/-0.5 ms. Thirty-six unique noisy measurements
produce 108 correlated arrival cases; they are not 108 independent recordings.
The same six room selections previously served AAE evaluation, so they are not
globally untouched data, but this IR model was trained on generated data only.

All three guarded methods admit 72 cases and retain the 36 cases at 20 dB SNR
unchanged. Learned median reconstruction improvement is 0.1780 dB versus 0.1755 dB
for Wiener and 0.1534 dB for decay. No export loses reconstruction SNR in this
probe. After grouping each noisy measurement's three arrival variants, the
learned method beats the better classical result in 13/36 groups, with median
advantage zero and differences from -0.02696 to +0.03010 dB. This does not support
a general learned advantage.

Conditional masks applied to the known recorded reference change weak normalized
backward-energy profile regions by up to **2.4107 dB** (stairway, 40 dB white-noise
case). Those profile points are within 40 dB of reference integrated energy. The
profile is normalized independently at arrival to measure shape; waveform and
complex-transfer metrics separately retain absolute level and phase. At that
worst conditional case, output reconstruction and observed decay errors still
improve versus added noise. That improvement does not establish identity of the
underlying measured decay. The same maximum conditional loss occurs for all three
methods because the deterministic floor often dominates their proposals.

The CLI now reports `observed_decay_change` before/after normalized energy-profile
RMSE, maximum change and point count. This needs no clean target and is a review
diagnostic, not a new acceptance threshold. Noise removal itself can alter the
observed profile. It neither estimates unknown clean-room ground truth nor changes
the selected model/policy. Missing/invalid-model or policy fallback reports zero
observed change. Automatic loading remains disabled.

`audit/artifacts/ml-ir-measured-20261025` records all cases, source/member hashes,
reference transformations, protocols and independent analysis. The independent
SciPy reducer recreates source pairs and all 36 added-noise signals, checks every
mask/response/budget, and recomputes waveform, phase, magnitude and decay metrics
(maximum discrepancy 1.25e-13). It confirms that three arrival variants share each
source. The final helper replays all 324 outputs/masks/decisions exactly. Twenty-four
native UPC renders cover valid and invalid-policy exports from all six rooms,
with maximum error 5.97e-8 after the usual 1024-sample delay.

The archive is the pinned AIR v1.4 release from RWTH Aachen University (IND),
associated with Marco Jeub. Its historical archive has no license file; reconcile
redistribution terms before distributing its audio. The archive and every derived
IR/WAV/float array remain outside the repository. Only code, source hashes,
metadata and metric results are committed. No source IRs were trained on or
redistributed. Acquisition-noise, independent clean/repeated measurements,
perceptual suitability and general adoption remain open.

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
