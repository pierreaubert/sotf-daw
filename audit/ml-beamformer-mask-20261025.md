# Beamformer neural covariance masks — 2026-10-25

Plugin 11 now has an explicit `qa`-only native spatial-mask path. A fixed
6→8→1 neural network estimates noise-energy fractions for per-bin covariance
learning. The original native MVDR covariance/weights run independently; missing,
disabled, unsupported, uncertain or invalid model evidence uses those weights.
The existing distortionless/white-noise gain constraints remain authoritative.
There is no production parameter or model activation.

Training uses 64 generated array-scene seeds, validation 16 and final 32; final
shift variants reuse seeds and are correlated. The network/policy are frozen
before final scene generation. Generated complex plane waves and sensor noise
provide energy-ratio labels, not speech identity or real recording validation.
The fixed 300-iteration optimization hit its limit. The model is 65 finite bounded
coefficients with SHA256
`5ed1fe74a6aa11d702c20469a87a209198cb742f3233903f32d12b11729998f7`.

Mask MSE on final generated examples is 0.03727 matched and 0.12571 under
reverberant-target shift; near-interference and sensor-gain shifts also worsen it.
Confident errors remain. The mask-level coherence comparator is per-bin, while
the native baseline gates a whole frame, so it cannot establish audio advantage.

The actual native renderer compares nine waveform conditions, 18 outputs total,
using the frozen model and generated voiced/noise envelopes. Rates are 16/48/96 kHz,
arrays use 2/4/8 microphones, and conditions include matched mixtures, target-only,
near-interference, sensor gain and reverberant target. Steady output SNR changes
versus native classical MVDR range from −0.16122 to +0.30683 dB: four improve,
four regress and target-only is unchanged. Target-only relative RMS error is
1.5063e-7, with native 512-sample latency removed exactly. Mixture projection gain
is not isolated target distortion. No fitted gain/alignment or normalization is
performed. These results do not establish a general reason to deploy the model.

Independent arithmetic reconstructs the four final mask-score sets and all 18
native waveform metrics (maximum discrepancy 2.23e-15), verifies model/output/
target hashes and disjoint training/development/final seeds, and accounts for
the synthetic-prefix exclusion and complete per-bin decision counters. Raw
waveform sessions remain at
`/Volumes/home_tmp/tmp/ml-beamformer-mask-native-20261025`; training work remains
at `/Volumes/home_tmp/tmp/ml-beamformer-mask-20261025`.

Verification: 103 native tests, six Python/native tests, strict QA Clippy and
default-feature library check pass. The mask tests cover independent finite-
difference gradients, global gain/phase invariance, feature/network parity,
weighted covariance recurrence, invalid mask/input rejection, native WNG/look
constraints, missing/disabled/uncertain/unsupported exact fallback, callback and
reset equivalence, frozen bounded drain, rejected live replacement/rate changes,
hash/policy refusals and broadside reconstruction. Probability parity error is
≤2.79e-8; feature parity error ≤3.34e-16. Earlier compile failures
are retained beside final results.

At eight microphones with 193-frame callbacks, 200 measured callbacks per route
and rate passed the fixed 80%-of-duration local budget. At 96 kHz the worst neural
callback was 0.305875 ms versus 1.608333 ms budget; across rates the worst was 0.310209 ms.
Neural p50 at 48 kHz was 0.291083 ms versus 0.125958 ms production. The added algorithmic
lookahead is 0; existing MVDR latency is 512 samples. This is native local evidence,
not complete memory accounting, isolated-kernel profiling or cross-platform
realtime admission.

Natural multilingual speech, independent room/microphone captures, unknown
geometry/motion, frequency-edge behavior, perceptual artifacts and calibrated
confidence remain unproven. Complete memory and host/platform deadline budgets
and activation transitions also remain. The implementation ledger stays in
progress; deployment admission remains false.
