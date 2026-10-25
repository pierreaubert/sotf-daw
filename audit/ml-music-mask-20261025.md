# Denoiser musical mask qualification — 2026-10-25

Plugin 19 has an explicit `qa`-only neural music-mask path. The model is trained
on generated polyphonic/percussive music/noise, not speech or recorded music.
Classical MCRA/DD/gain state remains independent; learned gains are selected at
synthesis only, before the existing final zero-reduction endpoint. Missing,
disabled, invalid or unsupported model contexts retain exact native gains.
There is no production factory selection or deployment admission.

The model has six causal spectral/temporal features, eight tanh hidden units and
a sigmoid gain output (65 bounded coefficients). Its SHA256 is
`ab1a996543de22b77b38ea57363a57d23a65319161ba81f4ecce1d5b280fe0a1`.
Preparation validates exact policy/provenance/dimensions/hash. Supported research
contexts are mono/stereo, 512/2048 FFT and 44.1/48/96 kHz. Gain smoothing is fixed
at 5/30 ms, feature memory at 30 ms. Floor/curve/transparency and the final unity
blend remain explicit. The 20 dB shaping reference does not equalize achieved
noise attenuation with classical Wiener processing.

Training/validation/final seeds are disjoint at 32/8/8 source groups. Training
samples 512 foreground-dominant and 512 other bins per source and emphasizes
foreground in its weighted objective. Final gain metrics pool every frame/bin;
the analyzer also reports equal-source averages. Final variants reuse seeds.
The fixed optimization hits its 300-iteration limit. Training analysis includes
zero-padded end frames; native musical state freezes at EOF. No convergence,
arbitrary-music quality or speech-model safety claim follows from these fixtures.

Pooled final gain MSE is worse than the feature-only flatness reference (0.2312
versus 0.1503 matched), with noise-step MSE 0.3905 and clean-only foreground gain
MSE 0.3446. Native waveform results tell a different but limited part of the
story: over nine new generated compositions/conditions at two FFT sizes, steady
SNR is 0.29–26.62 dB above the aggressive classical 20 dB setting. The model also
leaks more noise: lead attenuation 0.75–8.43 dB versus 13.68–19.90 dB classical.
Clean-only neural SNR is about 27.3/29.7 dB, while unfiltered clean input is ideal.
Noise spectral crest, clean-ranked transient error, stereo residual and complete
trailing synthesis energy are retained separately. These diagnostics cannot
certify perceptual preservation or a fair equal-noise-removal advantage.

A supplemental classical/dry output blend calibrates from only the known
noise-only 0.15–0.19 s span, then ramps over 10 ms before later musical scoring.
The model wins eight and loses eight noisy comparisons (−1.87 to +7.98 dB versus
that control). Later noise-energy transfer differs by −3.09 to +0.17 dB, so this
is an approximate attenuation comparison and an oracle noise-span reference,
not a deployed parameter or a model-selection/admission gate. Subsequent clean
target data is not used to choose its coefficient.

Independent reconstruction verifies final gain-score arithmetic, source
identities, model/target/native output hashes, all 36 native waveform metric sets,
the source FFT counters and full finite drain timing. Maximum native metric
discrepancy is 6.22e-15. Native/NumPy parity covers 30 feature/network points at
both FFT sizes: feature error ≤2.67e-15, gain error ≤2.96e-8. The raw sessions are
`/Volumes/home_tmp/tmp/ml-music-mask-native-20261025`, training work is
`/Volumes/home_tmp/tmp/ml-music-mask-20261025`, and supplemental controls are
`/Volumes/home_tmp/tmp/ml-music-mask-controls-20261025`.

Verification passes 138 native tests, six Python/native tests, strict QA Clippy
and default-feature library check. New tests compare every retained classical
estimator/gain array with a native-only instance, including DD and optional modes,
while selected output differs. They cover unity reconstruction, reset, exact
missing/disabled/context fallback, rejected clock/live replacement, frozen model
drain, callback and one-frame drain partitions, and native/NumPy DFT/gradient and
mask parity. Valid process/reset/drain paths are allocation/deallocation guarded.
Initial renderer import and Clippy formatting failures are preserved.

All 18 local stereo probes pass a fixed 80%-of-callback-duration budget over 200
callbacks after 64 warmups. Worst learned callback is 0.687791 ms at 96 kHz/
2048 FFT versus a 1.608333 ms budget. There is zero added model lookahead; native
latency remains 512/2048 samples. Disabled qualification intentionally keeps
feature history and inference warm. This is not complete heap memory accounting
or portable host/deadline admission; timing uses the other native defaults and
does not certify every optional-mode combination.

Real licensed held-out music, instrument/genre/level shifts, listening review,
stereo/transient artifacts, formal parameter/profile semantics, activation UX and
complete memory/platform validation remain. The implementation ledger remains
in progress, with deployment admission false.
