# AUD116 independent design review

Read-only review of `audit/proposals/spectrum-endpoint-power.md`, the public
probe source/log and current `analyzer_spectrum.rs` mapping/normalization on
2026-09-28. No source edits or builds. No blocker found.

## Independent algebra

Let `Y = DFT(x*w)`, `G = sum(w) = N/2`, and `U = sum(w²) = 3N/8` for the
periodic Hann window. The existing display uses an interior full-scale sine
as its zero-dB integrated-band reference, so the desired single-channel sum is

`P = 2 * sum((x[n]*w[n])²) / U`

`  = 2/(N*U) * (|Y[0]|² + |Y[N/2]|² + 2*sum(interior |Y[k]|²))`.

This is a sum of squared windowed samples, **not the square of their sum**.
The current interior factor `(4/N)² / 1.5 = 32/(3N²)` is correct. The current
endpoint factor `(2/N)² / 1.5 = 8/(3N²)` is half the required `16/(3N²)`.
Therefore multiplying only the Nyquist contribution to integrated bands by
two is correct. Its coherent peak estimate must keep its existing factor.
The usual one-sided endpoint distinction and coherent-window versus
window-square normalization are also documented in the primary
[MATLAB periodogram](https://www.mathworks.com/help/signal/ref/periodogram.html)
and [SciPy periodogram](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.periodogram.html)
references; the factor above is derived specifically for this display's
sine-referenced units.

For unit alternating samples the Hann transform gives endpoint calibrated
power 1 and adjacent interior calibrated power 1. Dropping the endpoint gives
`1/1.5`, or −1.7609 dB before the existing approximate logarithm. Including it
with corrected weight gives `(2+1)/1.5 = 2`, or +3.0103 dB. Endpoint coherent
peak remains 0 dB. This explains the measured red rather than relying on the
earlier source-only +1.2494 dB prediction.

For the adjacent cosine, the three calibrated contributions are 1, 1/4 and
endpoint 1/4; the corrected sum is `(1+1/4+2/4)/1.5 = 7/6`, or +0.66947 dB.
The adjacent sine has no endpoint contribution and gives `5/6`, or −0.79181 dB.
Thus the phase-sensitive oracle is necessary and should not be replaced by a
blanket zero-dB near-Nyquist assertion.

## Mapping and verification scope

`build_bin_to_display` already rejects out-of-range frequencies inclusively,
then incorrectly discards `floor(num_bins)` at exact `max_freq`. Clamping the
computed index to `num_bins-1` after that check is narrowly correct; validated
configurations have a positive band count. It also handles rounding just below
the upper edge without admitting frequencies beyond the requested range.

The proposed matrix is sufficient: Nyquist/adjacent cosine/sine, interior
negative controls, mixed high-frequency support, exact non-Nyquist upper bound,
unchanged passthrough and linewise channel maximum. Keep peak normalization
separate from band weighting and put the new weight after channel maximum so
all existing channel-selection semantics remain unchanged.

The time-domain total-power oracle applies directly only when all material
windowed spectral energy is inside the displayed range. DC is omitted and the
current positive minimum can also omit low bins. Arbitrary broadband fixtures
need a selected-frequency independent DFT oracle or explicit accounting for
omitted energy. Likewise, maximum per FFT line across channels is not the power
of an averaged signal or maximum whole-channel power. Public documentation
should call these sine-referenced integrated bands rather than PSD/dB-per-Hz,
and retain the stated channel aggregation and unsmoothed-oracle conditions.
