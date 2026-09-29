# AUD116: spectrum endpoint mapping and band power

Date: 2026-09-28. This corrects spectrum telemetry; audio passthrough and the analysis/window cadence are unchanged.

## Two measured errors

A line exactly at the configured maximum frequency passed the inclusive range check but was later discarded because its logarithmic index equaled the number of display bands. At Nyquist, the adjacent Hann line alone made the existing coherent-peak test pass, concealing the missing endpoint.

Separately, calibrated squared peak amplitudes were summed for band energy without accounting for the endpoint's different conjugate multiplicity. The existing endpoint squared-amplitude factor is one quarter of the interior factor; integrated one-sided energy needs one half. The [primary periodogram documentation](https://www.mathworks.com/help/signal/ref/periodogram.html) describes the one-sided endpoint weighting, and [SciPy's documentation](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.periodogram.html) relates coherent-window and window-square normalizations.

The initial source-only audit predicted a Nyquist band reading of +1.2494 dB. The public probe instead measured **−1.759465287 dB**, because the exact upper-frequency line was absent. An independent time-domain reference gives **+3.010299957 dB** in the existing full-scale-sine band units. The adjacent cosine case measured −0.791598041 versus expected +0.669467878 dB. Its sine-phase control already agreed within 0.000215 dB, and an ordinary interior coherent tone agreed within 0.000001 dB.

## Correction and units

The validated in-range display index is bounded to the last band, including the requested upper endpoint. Nyquist's contribution is multiplied by two only when integrating band power; its coherent peak calibration remains unchanged. Existing per-line maximum across channels, interior line arithmetic, logarithm approximation, band mapping below the upper edge, smoothing and selected-window schedule remain unchanged.

For mono with material spectral support inside the display range, unsmoothed integrated bands represent

`P = 2 * sum((x[n] * w[n])²) / sum(w[n]²)`

before the existing display floor/logarithm rounding, with periodic Hann window w. This retains the existing full-scale interior sine reference. Full-scale alternating Nyquist samples have twice that mean-square energy, hence +3.0103 dB band power and 0 dBFS coherent peak. Near-endpoint finite-window energy depends on phase; it is not generally 0 dB. The public `SpectrumData` field documentation now states these units and channel aggregation explicitly.

## Verification

Permanent `sotf-host/tests/spectrum_power.rs` failed **3/3** before correction and passes **3/3** afterward:

- 21 independent time-domain energy fixtures: seven Nyquist/adjacent/interior/mixed phase-and-amplitude cases at 16/32/40 kHz. Error is below 0.01 dB, accommodating the retained approximate f32 display logarithm.
- Exact non-Nyquist upper-bound inclusion: a coherent 5 kHz tone at 40 kHz retains its central line and lower Hann sideband, giving independent 5/6 selected power and a 0 dBFS coherent peak.
- Full-scale Nyquist coherent peak remains 0 dBFS; antiphase and silent-channel stereo controls exactly match mono under the retained per-line maximum convention.
- Every public probe checks exact audio passthrough.

All **17 existing spectrum unit tests** also pass, including calibration, smoothing, multichannel aggregation, sanitation and recent-window behavior. Logs: `/tmp/sotf-spectrum-power-{red,green}.log` and `/tmp/sotf-spectrum-unit-green.log`. Public baseline probe: `/tmp/sotf-spectrum-endpoint-probe.rs` and matching `.log`. [Independent review](spectrum-endpoint-independent-review.md) confirms the mapping and normalization derivation. The AUD115 gate passed all 682 host tests (including a doctest) and strict all-target Clippy; logs `/tmp/sotf-spatial-autogain-full.log` and `/tmp/sotf-spatial-autogain-clippy-final.log`. The fifteenth workspace gate also passes all 5,942 tests.

This does not add a PSD-per-Hz display, DC coverage, arbitrary callback partition invariance, different multichannel summation, new windows or cache-ownership behavior. Broadband time-domain totals require accounting for spectral energy outside the displayed range; the independent fixtures deliberately keep their material support inside it.
