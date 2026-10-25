# Optional offline convolution IR experiment: failed quality gate

This checkpoint implements a project-trained offline tail mask, a separate
float32 WAV export/report CLI, and a QA-only native convolution renderer. The
convolution runtime and default IRs are unchanged. No output is automatically
loaded. The current model is rejected for general adoption.

Training: 256 generated scenes; development: 48 independently seeded scenes.
Development weighted mask loss selects weights. The model/hash is frozen before
creation of 48 final and 30 shift scenes. All 382 input hashes are distinct.
There are no external or measured IRs, recordings, or pretrained weights.
`experiment/protocol.json`, `training-sources.json`, `training.json`, and frozen
`evaluation-source` copies preserve the initial experiment. Individual case
records include all clean/noisy/export hashes and metric results. Float arrays
remain in `/Volumes/home_tmp/tmp/ml-ir-denoise-20261025`, not in Git.

The 8→24 tanh→1 sigmoid network has 241 parameters, with gain 0.05..1. It uses
20 ms energy, neighboring energy, fitted decay trend, time, color and noise
features. All methods preserve original f32 samples through the first 50 ms
after a supplied arrival, use a supplied noise-only prefix, and retain frame
count, sample rate, channel order, delay and level. A common time mask links
channels. No normalization or resampling occurs. Missing/malformed model,
invalid noise analysis or failed response checks retain original f32 samples.

The neural, smoothed Wiener and fitted-decay candidates use identical protection
and response limits: relative IR change <=5%, <=1 dB/10 degrees change at 192
log-frequency points above -20 dB per-channel peak. Those bounds do not establish
structure preservation. Case 52 removes a weak late reflection while satisfying
the bounds; its reconstruction SNR falls 9.0735 dB. This is a genuine quality
failure, not an implementation or native rendering failure.

Final results, including fallback cases:

| Method | Accepted / 48 | Median improvement over unchanged IR |
| --- | ---: | ---: |
| Neural | 36 | 3.2285 dB |
| Smoothed Wiener | 34 | 3.4365 dB |
| Fitted decay | 34 | 3.4382 dB |

The neural model beats the better classical method in only 2/48 final scenes.
It beats that comparison in 5/30 shifts, but shift median improvement is zero
and worst change is -9.0735 dB. Fitted decay also loses 1.7672 dB in its worst
shift. Do not substitute either learned or fitted decay attenuation automatically.
These synthetic comparisons do not establish measured-room, listening or broader
rate/channel acceptance.

`independent-quality-final.json` independently recomputes every quality metric,
response limit and channel-link/protected/fallback invariant using SciPy full
complex FFTs. Metric error is zero. Maximum response discrepancy is 2.11e-6
because the original NumPy candidate transform retained complex64; the final
helper explicitly transforms both operands as f64. `reproduce.json` verifies
that the final trainer regenerates identical model bytes, all 156 source arrays,
all 234 exported arrays and all admission decisions. It also tests automatic
source snapshots. Frozen initial results were not overwritten or tuned against.

`native-verification.json` records 36 public-plugin UPC renders: selected model
and missing-model WAV exports, mono/stereo at 16/48 kHz, callbacks 1/193/1024.
After compensating exactly 1024 samples, maximum native IR error is 5.97e-8.
The callback allocation guard remains active on every render. This validates
float32 WAV loading, channel order and filtering, not denoising quality or real
host deadlines. All 234 original exports are independently replayed first.

`model-eol-check.json` verifies raw model identity under Git autocrlf filtering.
The asset SHA is `a4368ca93edb1a8029730474955ec65ad28724146acc595af036ed80c6001e39`.
Five focused Python tests, Python style/format, focused Rust formatting, native
build and strict Clippy pass. Command JSON records and text logs include status
and durations; trailing blank lines in logs are trimmed. `source-sha256.json`
hashes final source/model/docs; `artifact-sha256.json` hashes all other artifacts.

Reproduction and limitations are documented in the model README. Further work
must protect late structure using separate development material and evaluate new
independent final data, measured IR/noise recordings, rates/channels and listening.
The Convolution proposal and the complete 31-plugin/5-program ML audit remain
in progress.
