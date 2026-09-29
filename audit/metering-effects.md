# Metering, analysis and loudness-effect audit

Read-only audit, 2026-09-27. No source changes or Cargo/test runs were made for this report. Numerical examples below are independent arithmetic from the inspected equations, not measured plugin output. MIDI and IAMF are excluded.

**AUD121 correction to this report:** the historical true-peak test described
below copied incorrect coefficients from production. It did not establish ITU
calibration. Public tests subsequently reproduced a 4.675 dB overread for a
known 12 kHz/48 kHz tone. Both host and backend tables are corrected against
BS.1770-5 Annex 2, with independent full convolution and analytic tone tests.
See [true-peak coefficient verification](true-peak-coefficients.md). The source
had also mislabeled the interpolation table as Table 2, which describes the
loudness high-pass stage instead. Earlier green test counts remain historical
execution evidence; they do not prove the old true-peak calibration.

**Subsequent implementation checkpoint (AUD035/036):** after the aggregate gate,
the ISO 226:2003 equation and LoudnessCompensation automatic-gain defects below
were reproduced and corrected. Pre-fix regression measured 85.1756 dB SPL
instead of 89.6 dB at 20 Hz/20 phon, a -2.974 dB residual for a -6 dB center
peak EQ, and 0.0987 full-scale callback-partition discrepancy. The corrected
crate passes 110 tests, including 16 published SPL checkpoints, independent
contour differences, Pre/Post correction within 0.03 dB after level changes,
partition error below 1e-6 at 44.1/48 kHz, causal warmup, exact reset and cold
allocation checks. A separate meter preserves truthful raw-input/final-output
telemetry. The controller measures the two sides of the EQ with compensation
on neither side (Post) or both sides (Pre), and updates causally every 50 ms.
The host AutoGain helper was not changed. Subsequent AUD043 also fixes the
single-parameter rejected-calibration mutation: all getters/schema values and
continued DSP history remain unchanged; 112 crate tests and Clippy pass. This
does not establish atomicity for arbitrary mixed parameter batches. Other
bounded findings below remain open. Logs: `/tmp/sotf-loudness-tests.log` and
`/tmp/sotf-loudness-clippy.log`. The rest of this report records the pre-fix audit.

## Inventory and scope

The uncovered standalone crate is `sotf-plugin-loudness-compensation`; `FletcherMunson` is its compatibility name. Spectrum and loudness analyzers are modules of `sotf-host`, not separate crates. No `PitchDetection` implementation was found in the current inventory/search. PND, spatial processors, dynamics, saturation, dither and utility effects already have comparison reports and are not duplicated here. TokenSave exploration was followed by current source reads because its graph was rebuilding.

All paths below are relative to `crates/sotf-plugins/crates/` unless stated otherwise.

## Capabilities and evidence

| Crate/module | Implemented capabilities | Strongest existing numerical evidence | Missing evidence or comparison dimensions |
|---|---|---|---|
| `sotf-plugin-loudness-compensation` | Manual shelving; ISO 226:2003 contour differences at 29 frequencies; fitted IIR bank; measured-calibration Auto mode; reference/playback levels; headroom normalization; pre/post automatic gain; smooth transitions. | `src/lib/tests.rs:1606` measures fitted-bank response against internal contour targets; validation, automation and realtime tests exist. | The contour equation is wrong, and fitting against its own `iso_deltas` cannot detect that. Fit acceptance is loose: RMS 4 dB, maximum 10 dB. Auto-gain convergence is not established by the current 13 dB error allowance. Detailed defects below. ISO 226:2023 would be an optional edition update; pure-tone contour EQ does not establish ISO 532 broadband loudness compliance. |
| `sotf-host::analyzer_loudness_monitor` | K weighting; momentary/short-term/integrated loudness; absolute/relative gates; rolling or exact bounded whole-program history; explicit channel roles including LFE exclusion; true peaks; sample peaks; correlation; validity/warmup/error reporting. Unsupported 192 kHz true-peak compliance and unknown layouts are explicitly marked. | `src/analyzer_loudness_monitor.rs:1400`: independent f64 implementation of ITU Table 2 true-peak FIR, four rates, error below 1e-14. Internal gating and history-cap tests; `tests/test_analyzer_plugins.rs:443` 1 kHz stereo sine within 0.2 LU; channel-role/permutation checks; `:671` exact whole-program/rolling and callback partitions within 1e-12. Cold/held-reader/reset allocation coverage exists in facade realtime tests. | No Loudness Range/LRA output was found. LRA is a useful separate EBU Tech 3342 dimension, not an implied requirement for every meter. I found no use of the official EBU conformance audio corpus; internal reference tests alone do not establish full certification. Whole-program history has an explicit finite capacity. |
| `sotf-host::analyzer_spectrum` | Fixed 4096-point periodic-Hann FFT; 8–120 logarithmic bands; per-line maximum across channels avoids antiphase cancellation/silent-channel dilution; elapsed-time smoothing; calibrated peak display; recent-window policy for oversized callbacks; reset/sanitization/realtime publication. | `src/analyzer_spectrum.rs:849` interior Hann-band normalization within 0.1 dB; `:1061` interior full-scale coherent tone; `:1111` Nyquist peak; antiphase/disjoint-channel/silent-channel tests; `:874` physical decay-time checks. | Nyquist **band** normalization is not tested and appears inconsistent with the interior-band convention; see below. Latest-window processing is intentionally not arbitrary-signal callback-partition invariant. Adjustable FFT/window/hop, PSD units, per-channel or M/S traces and peak hold are comparison dimensions, not verified advertised omissions. |
| `sotf-host::analyzer_channel_correlation` | Centered, exponentially weighted Pearson correlation, f64 moments, multichannel matrix, partial interleaved-frame handling. | `channel_correlation_monitor.rs:235` coherent/quadrature/antiphase; `:437` DC-offset/unequal-gain invariance; `:458` exact partition check; symmetry and more than 32 channels. | The optional plugin wrapper has a fixed scalar queue and can drop/realign samples on oversized callbacks. Core-monitor tests do not cover that wrapper. Wrapper documentation says it is not registered in the engine; the loudness monitor's embedded path is separate. |
| `sotf-host::auto_gain` and `multichannel_auto_gain` | Input/output LUFS comparison, gain bounds, smoothing and momentary/short-term choices; multichannel helper compares a geometry-derived stereo fold against stereo reference. | `auto_gain.rs:425` convergence tests for the helper's intended uncompensated measurement; multichannel tests exercise disabled/stereo/5.1/mono/binaural construction and finite processing. | LoudnessCompensation feeds the wrong measurement into the absolute-gain helper. Multichannel tests lack analytical gain and arbitrary-partition oracles. Geometry-based fold-down loudness can cancel opposite-phase channels and is not equivalent to BS.1770 multichannel energy summation; this is an explicit objective limitation, not necessarily a defect. |

The metering comparison dimensions come from [ITU-R BS.1770-5](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf), [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341), [EBU Tech 3342](https://tech.ebu.ch/publications/tech3342) and the [official EBU test set](https://tech.ebu.ch/publications/ebu_loudness_test_set). Passing local tests is not presented as complete conformance to these documents.

## Ranked concrete defects

### 1. Incorrect advertised ISO 226:2003 equation

**Verified source defect:** `sotf-plugin-loudness-compensation/src/iso226.rs:60` subtracts `1.585` and leaves the threshold term unexponentiated. The published 2003 equation is:

```text
Af = 4.47e-3 * (10^(0.025 * LN) - 1.15)
     + [0.4 * 10^((Tf + LU)/10 - 9)]^alpha_f
Lp = (10/alpha_f) * log10(Af) - LU + 94
```

This is reproduced in section 2.3, printed page 4 of the standard authors' [2024 revision paper](https://www.jstage.jst.go.jp/article/ast/45/1/45_e23.66/_pdf/-char/en), and in clause 4.1 of the [ISO 226:2003 preview](https://cdn.standards.iteh.ai/samples/34222/d93363dbdafa470aab734f04d091065b/ISO-226-2003.pdf). The wrong expression is a defect in the declared legacy algorithm, independent of whether a 2023 edition mode is added.

Independent checkpoints from ISO 226:2003 Annex B, Table B.1 are below. These are published output values, not values generated by production code. Source: [standard text, Table B.1](https://standards.iteh.ai/catalog/standards/iso/f15d18f8-69b0-4f46-a648-1bb26caca757/iso-226-2003).

| Loudness (phon) | SPL at 20 Hz (dB) | SPL at 100 Hz (dB) | SPL at 1 kHz (dB) | SPL at 12.5 kHz (dB) |
|---|---:|---:|---:|---:|
| 20 | 89.6 | 48.4 | 20.0 | 33.0 |
| 40 | 99.9 | 64.4 | 40.0 | 51.5 |
| 60 | 109.5 | 78.7 | 60.0 | 68.6 |
| 80 | 119.0 | 92.5 | 80.0 | 85.4 |

At 1 kHz, direct arithmetic with the **current wrong expression** yields 7.9286, 37.0145, 59.1190 and 79.7348 dB respectively. Existing `iso226.rs:151` tests check only 60/80 phon with a 2 dB tolerance, so both pass. Other sign/monotonicity/equal-level tests cannot validate the standard.

Recommended independent gate: published rounded SPL checkpoints with 0.06 dB allowance for 0.1 dB table rounding, then a broader source-derived contour fixture. Test the compensation differences separately; for example, the rounded 20-vs-80 phon checkpoints imply roughly +30.6 dB at 20 Hz and +15.9 dB at 100 Hz relative to 1 kHz. Independently measure the fitted filter bank after correcting its target and state a separate approximation-error budget. Never use the production target array as the only standard oracle.

**Domain/documentation issue:** the code advertises 20–90 phon over all 29 frequencies. Clause 4.1 gives an 80 phon upper applicability limit for 5–12.5 kHz; 90 phon applies through 4 kHz. An extrapolated upper treble contour should be identified as extrapolation. [ISO preview, clause 4.1](https://cdn.standards.iteh.ai/samples/34222/d93363dbdafa470aab734f04d091065b/ISO-226-2003.pdf)

The current edition is [ISO 226:2023](https://www.iso.org/standard/83117.html). Its scope concerns normal listeners and pure tones in specified free-field conditions; contour-based playback EQ alone is not a calibrated general loudness model. Edition selection and individual hearing profiles are possible extensions, not prerequisites for correcting the present equation.

### 2. LoudnessCompensation auto-gain converges to half the needed correction

**Verified measurement routing plus algebraic consequence; not rendered in this read-only audit.** `src/lib/loudness_compensation_plugin.rs:926–990` measures output **after** compensation for both Pre and Post. `sotf-host/src/auto_gain.rs:246–275` then sets an **absolute** gain target from input LUFS minus this corrected output LUFS.

For stationary filter gain E dB and compensation G dB, that update is `G_target = -(E + G)`. Its steady state is `G = -E/2`, leaving E/2 dB uncorrected. A +6 dB response therefore predicts +3 dB residual, rather than unity. Corrected-output telemetry and the controller's uncompensated-output measurement must be distinct, or the controller must genuinely accumulate an error correction with appropriate dynamics. A before/after label change is insufficient.

Tests at `src/lib/tests.rs:635–720` discuss this routing but permit up to 13 dB input/output difference and mainly verify freshness/finiteness. The comment that uncompensated-output measurement would cause positive feedback contradicts the helper's absolute-target law. Required oracle: steady sine at an independently measured nonzero filter gain, Pre and Post, cuts and boosts, actual output RMS and cached output LUFS; then varying callback partitions and automation. Refreshing only once at callback end is a second source of partition dependence.

### 3. Rejected Auto calibration update changes live state

**Verified source defect:** `src/lib/loudness_compensation_plugin.rs:826–836` assigns `auto_calibrated = false` before checking that Auto mode is active. It then returns `Err`. The live getter is changed even though the operation failed, while cached parameter rebuilding is skipped. The broader `apply_values` loop also commits keys before all values/cross-parameter constraints are validated, making combined calibration/mode updates order-sensitive.

Required regression: enter calibrated Auto mode, reject calibration removal, and assert unchanged getters, parameter cache and audio history. A valid batch containing calibration plus Auto mode should have deterministic behavior independent of map iteration. Transactional FFI preset staging does not repair this direct plugin setter.

## Additional bounded findings

- **Correlation wrapper sample loss/channel alignment:** `sotf-host/src/analyzer_channel_correlation/channel_correlation_plugin.rs:38` allocates a 96,000-**sample** ring regardless of channel count; `:115–149` pushes the whole callback before draining and ignores excess samples. At seven channels, 96,000 is not frame-aligned, so dropping the remaining input joins a partial old frame to the next callback's first channels. This does not affect the separate embedded-monitor path. Reproduce with a callback exceeding capacity and seven channels, comparing against the direct monitor.
- **Spectrum endpoint evidence gap with a derived discrepancy:** `analyzer_spectrum.rs:520–546` uses 4/N interior amplitude normalization, 2/N at Nyquist, then divides all summed band power by Hann ENBW 1.5. For a full-scale alternating Nyquist sequence, periodic Hann gives Nyquist magnitude N/2 and its adjacent bin N/4. The two calibrated squared magnitudes are both 1, so their shared band displays `10*log10(2/1.5) = +1.2494 dB`, while the peak displays 0 dB. `:1111` checks only the peak. Clarify the band's peak-equivalent versus RMS convention and add an independent endpoint/near-endpoint energy oracle before choosing a correction.
- **Multichannel helper callback capacity:** `multichannel_auto_gain.rs:122` has a debug assertion against an 8192-frame scratch allocation, followed by `resize` in release. Direct callers exceeding that size can panic in debug or allocate in release. Whether normal Upmixer/AAE host paths can reach this with oversized callbacks was not established in this pass; do not report an observed product-path failure without that caller check.
- **Multichannel helper scheduling:** it consumes an entire callback and changes gain before applying that gain to the same callback. The source has the same scheduling concern addressed for ABCompare, but no reproduction was run here. An independent gain/partition oracle is needed before claiming equivalence across buffer sizes.

## Suggested next scope

Correct and independently test the ISO expression first; repair LoudnessCompensation control measurement and sample scheduling next; make its calibration setter transactional. Keep metering conformance, optional current-edition support, and analyzer endpoint conventions separately tracked. No ISO 532 compliance claim is warranted from these fixes.
