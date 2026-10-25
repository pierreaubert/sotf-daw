# Frozen measured-reference IR probe and export decay diagnostics

This checkpoint tests the existing model and selected local-budget policies on
recorded IR structure. It adds before/after observed decay diagnostics to the
separate offline export report. No model, mask policy, admission threshold,
convolution runtime or default IR changes from these results. The model remains
experimental; measured decay preservation and general ML adoption are unproven.

The pinned local AIR v1.4 archive supplies six paired-channel rooms (12 MAT
members). Their original h_air doubles are cast to f32 and preceded by 80 ms
zeros. Original recording noise/artifacts remain in the target. Controlled white
and colored stationary noise is added at 20/40/60 dB SNR, with a 60 ms reference
prefix containing added noise only. Approximate prior direct-arrival annotations
are tested at +/-0.5 ms. There are 36 unique noisy measurements and 108 correlated
arrival variants, not 108 independent recordings. Rooms were previously selected
for AAE evaluation; they were never used in this IR model's synthetic-only
training, but are not globally untouched data.

The probe is deliberately restricted: 48 kHz paired recordings, artificial added
noise and a supplied prefix. It does not identify unknown clean-room truth,
actual acquisition noise, acoustic calibration, true-stereo routing or listening
suitability. The artificial prefix is retained in test exports; these WAVs are
validation fixtures, not replacements to load as original-delay user IRs.

| Method | Admitted / 108 | Median added-noise recovery SNR improvement | Worst improvement |
| --- | ---: | ---: | ---: |
| Learned | 72 | 0.1780 dB | 0 dB |
| Wiener | 72 | 0.1755 dB | 0 dB |
| Fitted decay | 72 | 0.1534 dB | 0 dB |

All methods retain all 36 cases at 20 dB SNR unchanged. Results include those
fallbacks. Grouping the three arrival variants within each unique noisy
measurement, learned output beats the better classical result in 13/36 groups;
median advantage is zero and deltas range from -0.02696 to +0.03010 dB. The larger
40/108 case win count is correlated and is not population accuracy or statistical
significance. These gains do not support general learned adoption.

The weak conditional normalized decay profile changes by as much as 2.4107 dB
(stairway, 40 dB white noise, +0.5 ms annotation). At that case, reconstruction
SNR improves by 2.2097 dB and observed profile error improves, but held-mask
reference decay still changes. All three methods have the same worst conditional
change because their common deterministic floor can dominate. No extra
preservation criterion was tuned from this final set.

The new CLI `observed_decay_change` reports normalized observed energy-profile
RMSE, maximum difference and point count, without a clean target. It is a review
diagnostic, not an additional admission rule. Noise suppression can change this
shape. Waveform/complex response measurements separately retain absolute level
and phase; there is no alignment or gain fitting. Invalid model/policy fallback
reports zero observed change. Source IRs are never overwritten or automatically
loaded. Future training snapshots include the new report dependency.

`independent-quality-final.json` verifies archive/member identities, reconstructs
all paired f32 references and all 36 controlled-noise signals, verifies shared
arrival variants, and recomputes all masks, budgets, waveform/complex transfer
and energy-profile metrics using SciPy full FFTs. Maximum metric discrepancy is
1.25e-13; largest removed-energy/budget ratio is 0.94998. The profile uses backward
integrated energy normalized at arrival and reference points above -40 dB;
transfer measurements use fixed log-grid points above -20 dB per-channel peak.
Conditional masks are held fixed on the known recorded reference, not unknown
noise-free IRs. Source WAV/noise artifacts remain in that reference.

`native-verification.json` replays all 324 outputs, masks and decisions exactly
with the final helper. Twenty-four allocation-guarded public-plugin UPC renders
check valid-policy and invalid-policy exports from all six rooms, callbacks
193/1024, full wet and unity gain. Maximum native error is 5.97e-8 after exactly
1024 samples of UPC latency. Every new observed-decay report is also checked.
The unchanged native binary/source match the previous verified build. This is
filter-realization evidence, not native DAW deadline or denoising acceptance.

Fifteen Python analytic, regression, fallback and file-identity tests pass, as do
Python style/format checks. Command/status/duration JSON records have matching
text logs (trailing blank lines trimmed). `experiment/protocol.json` and frozen
`evaluation-source` copies preserve the pre-inference specification. Only code,
metadata, hashes and metrics are committed. Float arrays stay in
`/Volumes/home_tmp/tmp/ml-ir-measured-20261025`, native WAVs/outputs in the separate
`ml-ir-measured-native-20261025` directory.

AIR provenance: Aachen Impulse Response Database v1.4, RWTH Aachen University,
Institute of Communication Systems and Data Processing, associated with Marco
Jeub; archive readme copyright 2011. Archive SHA-256:
`4baa3a0ea31dc72647ff61f53b0bf0b962876de31b6b0569ebc504987898aa02`.
The historical archive has no license file. Reconcile terms before distributing
its source or derived audio; none is committed or used for training here.

Reproduce with the commands in `evaluate.json`, `independent-analysis.json` and
`verify.json`, using new output paths. Original data and final evidence were not
overwritten. `source-sha256.json` hashes current runtime/model/docs/verification;
`artifact-sha256.json` hashes every other file in this bundle. The full 31-plugin
and five-program ML audit remains in progress.
