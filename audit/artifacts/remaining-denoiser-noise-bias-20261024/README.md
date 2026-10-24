# Noise estimation and common-change diagnostic checkpoint

**Decision: NO-GO for production state rebase.** Shipping Denoiser source and index match `21806c0` exactly. Both diagnostic test modules and their registrations were archived and removed from the active crate. No new estimator, gain law, defaults, parameters or production allocation were retained.

## What the noise-only baseline establishes

The fixed 64-seed, two-FFT, two-channel, five-window diagnostic records 1,280 rows. Pure uniform noise is generated directly; channel 0 has a 40 dB up/down level step and channel 1 is stationary. Raw PSD is checked against the independent first moment `E|X[k]|²=sum(a[n]²*w[n]²/3)` for `w[n]=sin(pi*n/N)`. Direct-f64 covariance/bin-pair sums verify the stated uniform-noise band-mean variance separately from production FFTs. Finite PRNG/f32 samples approximate the population model; these are diagnostic ensembles, not held-out acceptance data.

Stationary-control estimated PSD/analytic truth is about 0.97–1.02. Quality-mode upward-step estimates are only 0.1155 at source-window center 320 ms, 0.1485 at 469 ms and 0.1512 at 619 ms, while the known-H0 quiet-clock recurrence predicts 0.5471/0.8964/0.9763. The false-presence behavior therefore needs discrimination/tracking evidence, not a universal noise multiplier or an assumed calibrated posterior.

The downward-step ratios are **not independently classified as bugs**. At the actual source-window ages, the quiet H0 recurrence predicts 1,720.50x/23.864x after the 40 dB fall, versus measured 1,514.30x/23.593x. Low latency closely matches ordinary smoothing memory (tau 25.31 ms); quality has tau 101.24 ms and an already underestimated pre-fall state. Both center age and window-end age are recorded. Ordinary alpha=.9 and quiet-fast alpha=.9² references are retained.

A separate single-step Gaussian-power calculation demonstrates sample-selection bias in adaptive averaging; it is not a full MCRA fixed point or a proposed compensation factor. The primary fixed-prior SPP reference was also not ported: its mean-p anti-stall cap cannot activate during the declared 500 ms/40 dB scalar step. Its inspected BSD-style notice/provenance are retained, and no author MATLAB implementation was vendored into production.

## Independent Gaussian score and explicit reference lifecycle

For independent proper complex Gaussian coefficients from **real white Gaussian** source noise, one raw disjoint-window ratio obeys `R=Pnew/Pref`, `F(R)=R/(1+R)`. Thus `T=(Pnew-Pref)/(Pnew+Pref)` is Uniform[-1,1]. The sum's Irwin–Hall tail is evaluated with exact rational arithmetic. Every-other proper interior bins in 100–4000 Hz give 83/21 bins at FFT2048/512. The fixed illustrative hourly familywise budget is 0.001 via a per-frame union bound; thresholds are 0.3548100590648318/0.6858900300278671, derived before empirical fixture measurement.

These exact claims do not transfer to uniform noise: its fourth cumulant creates dependence despite nonadjacent covariance decorrelation. Colored/voiced/unvoiced controls are likewise empirical. The bounded score is a likelihood direction under H0, **not** an H1-shape goodness-of-fit or proof of speech identity.

The diagnostic reference is one raw pre-gain periodogram from exactly four hops earlier. It advances deterministically, never uses averaged/minimum/adaptively selected PSD, and is never rebased. The observed stream starts only when full active source windows have valid raw snapshots; first comparisons occur after four additional valid frames. Neighboring tests may share/overlap source data; no across-test independence is assumed. The union bound uses only valid unconditional Gaussian marginals. Reference IDs, reset, callback grouping, applied-gain independence and zero allocation/free in the QA-scoped lifecycle test pass. These are test-only lifecycle checks, not a deployed host integration.

## Fixed empirical go/no-go result

The unchanged plan requires zero events on wanted/stationary/colored controls and at least 61/64 timely detections of a 40 dB common-noise increase within two FFT-window durations. The 1,424 reports comprise 64 fixed seeds across 11 synthetic conditions and eight existing recorded clips in two FFT modes. Existing recorded s/sh/p annotations are reused; no annotation or threshold was selected from detector output. Clean recordings receive a fixed low Gaussian bed to avoid zero-reference singularities.

All six required positive groups detect **64/64** seeds within the deadline, with zero events outside the physically affected reference/window horizons. Stationary Gaussian, uniform and colored AR(.95/.995) controls have zero events. Nevertheless wanted controls produce **1,257 false events across 221 reports**, including 15/16 recorded clip/mode reports, both synthetic unvoiced modes, and 13 quality-mode events centered inside existing annotated fricative intervals. These failures conclusively reject directly using this score to rebase a production noise tracker. No threshold tuning or new hypothesis followed these results.

The first compile attempt had only local scaffolding mistakes (parameter f64 versus existing f32, missing heap-helper label); its raw failure is preserved. The corrected two focused tests pass; this does not override the failed production-qualification gate.

## Archive and verification

`validation.json` preserves all 1,280 PSD observations; `ratio-validation.json` all 1,424 score reports and reference-lifecycle results. `analysis.json`, `physical-clock-h0-reference.json`, `ratio-analysis.json` and `recorded-false-alarm-summary.json` retain signed observations and all false events. `measured-source/`, `bias-only-registration.rs` and both source-hash manifests freeze the actual measured modules/registrations. `restoration.json` verifies all 46 active and indexed crate files against `21806c0`, with no untracked crate files. No trial-owned staged delta needed clearing; unrelated index entries were preserved.

The already validated restored checkpoint has 134 passing tests, four existing ignores and strict Clippy exit zero under `remaining-denoiser-snr-risk-20261024`. No full rebuild was repeated after byte-identical restoration. All seven independent oracle/analysis scripts replay successfully; their outputs are in `independent-replay.json`.

Run `rtk proxy python3 audit/artifacts/remaining-denoiser-noise-bias-20261024/verify.py` to verify this checkpoint and current restoration. Use `--frozen` only when intentionally checking the archived checkpoint after later source work.

## Remaining concrete gate

Any future rebase proposal needs an independently specified discriminator that rejects the fixed wanted-voiced/unvoiced/instrument controls while retaining the fixed positive/noise controls and valid reference lifecycle. This checkpoint does not select that model. Only after that qualification may a separately predeclared production candidate face the unchanged vowel >=0.5, 36 SDR >=-0.25 dB, settled/noise-step <=1 dB, profile/zero/heap/latency/EOF gates. Main bootstrap contamination and vowel loss remain open. AAE wall/native/held-out scopes and Speech broader onset/instrument/multilingual/native scopes also remain partial. No deadline, held-out or listening acceptance is claimed.
