# AUD116: spectrum band endpoint power

## Reproduction

The original metering audit derived an endpoint discrepancy. The current public probe `/tmp/sotf-spectrum-endpoint-probe.rs` compares the sum of published band powers to an independent f64 Hann-weighted time-domain mean square (scaled by two to preserve the existing full-scale-sine reference). The signal is mono, N 4096 at 40 kHz, periodic Hann, zero display smoothing and 100 bands spanning 10 Hz through Nyquist. Audio passthrough is checked exactly.

This is a band-energy correction, independent of the existing correctly normalized coherent peak display. Ordinary interior full-scale coherent sinusoidal tones retain their current 0 dB band reference; full-scale alternating Nyquist samples have twice that mean-square energy and should measure +3.0103 dB on that same reference. Calling every full-scale sequence 0 dB would mix peak and power calibration.

The actual current probe contradicts the earlier source-only estimate of +1.2494 dB: Nyquist measures **−1.759465287 dB**, versus independent **+3.010299957 dB**. Inspection explains the extra loss: the mapping accepts `freq <= max_freq` but then drops exact equality because its normalized index equals `num_bins`. The Nyquist line is absent entirely; the existing peak test passes using its adjacent Hann line. The adjacent cosine case is also low (−0.791598041 versus +0.669467878 dB); its sine-phase control already agrees within 0.000215 dB.

## Proposed minimal correction

Include a line exactly at the validated maximum frequency in the final display band by bounding its computed band index after the existing inclusive range check. Keep the current per-line maximum over channels and coherent peak-magnitude calibration. When accumulating line power into display bands, multiply the Nyquist endpoint's calibrated squared amplitude by two before the existing Hann ENBW division. The endpoint's current squared-amplitude calibration is one quarter of an interior bin's; one-sided power needs one half. The weighting changes only bands containing Nyquist. The mapping repair can also include a previously dropped upper-bound line below Nyquist and thereby correctly update the selected-range peak. Other interior arithmetic, smoothing and callback/window schedule remain unchanged.

A real signal's interior positive/negative frequencies each contribute power, whereas DC and Nyquist have no separate conjugate partner. The primary [MATLAB periodogram documentation](https://www.mathworks.com/help/signal/ref/periodogram.html) describes this endpoint weighting. [SciPy periodogram documentation](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.periodogram.html) documents the window-square versus coherent-window normalization relationship. The factor above follows from applying those conventions to the existing sine-referenced integrated bands, not from treating this display as an unqualified PSD.

## Acceptance and limits

First preserve a permanent failing independent time-domain Parseval oracle for Nyquist, adjacent-bin cosine and sine, mixed/high-frequency fixtures and ordinary interior controls. Include phases because window-weighted power near an endpoint depends on phase; do not assert that every finite-window near-Nyquist tone has exact 0 dB energy. Verify calibrated full-scale Nyquist peak stays 0 dB, an exact non-Nyquist upper-bound peak is included, exact passthrough and relevant multichannel line-max behavior. Use existing host tests/Clippy and source review. Document actual units on public spectrum fields.

No promise of arbitrary callback partition invariance, new DC display, new window/FFT modes, PSD units or changed multichannel energy summation is made. Spectrum's current maximum-across-channels aggregation is retained explicitly. Do not edit cache ownership in this numerical correction.
