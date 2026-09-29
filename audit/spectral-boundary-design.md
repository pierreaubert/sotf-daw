# SpectralCompressor boundary detector: mathematical design review

2026-09-27. Read-only design review; no SpectralCompressor source or tests changed.
The independent experiment is `/tmp/sotf-spectral-boundary-oracle.py`; results
are `/tmp/sotf-spectral-boundary-oracle.json`. It uses f64 direct DFT sums and an
analytic window transform, with no SOTF imports or production FFT routines.

## Current contract and defect

The README defines threshold as local narrowband coherent amplitude. Source
`crates/sotf-plugins/crates/sotf-plugin-spectral-compressor/src/lib/spectral_compressor_plugin.rs:182`
uses an unnormalized real DFT of a periodic-Hann-windowed frame, scales interior
line magnitudes by `4/N` and DC/Nyquist by `2/N`, sums a five-bin neighborhood,
and divides its squared magnitude by 1.5. Envelopes are then median filtered,
optionally smoothed across frequency, and applied to the complex spectrum.

For a sinusoid whose two spectral images are well separated, Hann coherent gain
is 1/2 and the three coherent line powers have proportions 1, 1/4, 1/4. This
explains the existing 1.5 divisor. It does not justify using that divisor after
the images overlap at a spectrum boundary. Correct one-sided **power** scaling
adds negative-frequency power, rather than squaring the doubled amplitude
scaling indiscriminately. [SciPy's official periodogram scaling reference](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.periodogram.html)

## Convention A: the advertised coherent-amplitude objective

For an isolated resolvable real tone
`x[n] = a*cos(2*pi*nu*n/N) + b*sin(2*pi*nu*n/N)`, use peak amplitude
`A = hypot(a,b)` and `20*log10(A)` dBFS. At exact DC or Nyquist the sine basis
vanishes: amplitude means the absolute **observable** constant/alternating
coefficient. An unobservable endpoint quadrature must not be interpreted as
additional amplitude. For mixtures/noise, retain a declared local spectral
energy proxy; there is no unique sinusoidal peak amplitude to recover.

This preserves the documented interior-tone threshold. It is not an RMS, PSD,
LUFS, SPL or true-peak convention. In particular, changing to a standard
one-sided PSD estimator and multiplying everything by sqrt(2) would create a
different threshold convention at DC/Nyquist and does not solve this contract.

## Exact finite-N equations

Define the forward DFT with `exp(-i*2*pi*k*n/N)`, no normalization, and
`w[n] = 0.5 - 0.5*cos(2*pi*n/N)` for `n=0..N-1`.

```text
D_N(t) = exp(-i*pi*t*(N-1)/N) * sin(pi*t) / sin(pi*t/N)
W_N(t) = 0.5*D_N(t) - 0.25*D_N(t-1) - 0.25*D_N(t+1)
Y[k]   = A/2 * { exp(i*phi)*W_N(k-nu) + exp(-i*phi)*W_N(k+nu) }
```

Use the limiting value at removable Dirichlet singularities. Squaring produces
the phase-sensitive cross term
`A²/2 * Re(exp(i*2*phi)*W_N(k-nu)*conj(W_N(k+nu)))`.
The overlapping images are the mechanism, not merely an endpoint counting error.
The same two-image model is the starting point of Borkowski and Kania's primary
[2016 amplitude/phase estimation paper, equations 2–4](https://www.journals.pan.pl/Content/90377/PDF/10.1515-2016-0013-paper_02.pdf).
Their application uses a sinusoidal model and interference control; it is not
evidence that an unconditional single-tone fit handles arbitrary audio mixtures.

### Why a new endpoint factor/divisor cannot work

Let the current endpoint amplitude scaling be multiplied by `c` and replace
1.5 by a common divisor `B`. For DC of amplitude A:

```text
detector² / A² = (c² + 1) / B
```

For bin-1 cosine phase phi, the normalized magnitudes are
`c*A*abs(cos(phi))/2`, `A`, and `A/2` in bins 0, 1 and 2:

```text
detector² / A² = [1.25 + c²*cos²(phi)/4] / B
```

Phase independence requires `c=0`; bin 1 then requires `B=1.25`, while DC
requires `B=1`. Both cannot hold. A special value at detector index 0 is also
insufficient: constant input has nonzero Hann spectrum at **both** bins 0 and 1,
and downstream median filtering reads neighboring detectors.

## Independent numerical evidence

The direct-DFT oracle sweeps N=1024/2048/4096, 32 phases, and frequencies
0, 0.1, 0.125, 0.25, 0.37, 0.5, 0.73, 1, 1.17, 1.5, 1.91, 2, 2.37,
2.75 and 3 bins. DC has one observable phase. This is 1,347 cases, plus their
Nyquist mirrors. The table reports maximum local detector level relative to
the known unit-amplitude input, across phases, at N=4096.

| Distance from boundary, bins | Current minimum error, dB | Current maximum error, dB |
|---:|---:|---:|
| 0 | +1.249387 | +1.249387 |
| 0.1 | -16.242758 | +1.199571 |
| 0.25 | -10.242020 | +1.107858 |
| 0.5 | -4.771364 | +0.742043 |
| 0.73 | -2.271668 | +0.335819 |
| 1 | -0.791812 | 0 |
| 1.5 | -0.084430 | -0.000089 |
| 2 | 0 (roundoff) | 0 (roundoff) |
| 2.37 | -0.002346 | -0.000999 |

The exact window formula agrees with direct DFT to `5.0e-16*N`. Nyquist
modulation/mirroring agrees to `3.9e-13*N`. Given the true frequency, the
two-quadrature least-squares estimate below recovers amplitude to `3.4e-15`.
When frequency is estimated with a fixed 49-point grid and 24 golden-section
refinements, maximum amplitude error is `2.95e-6` and frequency error is
`2.99e-7` bins in this clean-tone experiment. These figures validate the
mathematics; they are not realtime performance or noisy-audio validation.

## Convention B: explicit finite-record energy, valid for mixtures and noise

The smaller, model-free alternative is to define the detector from a genuine
Hann-weighted local band energy. Let `B[k]` be the existing five-bin neighborhood
clipped to the real spectrum, and `eta[j]` be 1 at DC/Nyquist and 2 elsewhere:

```text
E[k] = sum(j in B[k], eta[j] * |Y[j]|²) / (N * sum(n, w[n]²))
D_rms[k] = sqrt(E[k])
D_sine_equivalent[k] = sqrt(2*E[k])
```

With all bins included, Parseval gives exactly
`E = sum(w[n]²*x[n]²)/sum(w[n]²)` for **any** finite real input. No single-tone
assumption or unstable inversion is involved. The local result is the defined
spectral neighborhood's contribution to that energy. Fractional-bin phase
dependence is then a real finite-record/window effect, not a claim that latent
sinusoidal amplitude was recovered.

For periodic Hann, `sum(w²)=3N/8`. The sine-equivalent version keeps the existing
interior sinusoid and noise scaling. In current calibrated-magnitude variables,
it can be implemented by doubling the **power contribution** of an endpoint
whenever that endpoint is included in a neighborhood; all interior terms and
the divisor 1.5 remain unchanged. Apply the same formula to every neighborhood,
including the detector beside an endpoint. No new scratch, estimator or
allocations are required; it adds at most endpoint checks/additions to the
existing five-term sums. This is a justified weighting **for the new energy
convention**, not a solution to Convention A.

Independent f64 direct-DFT/Parseval verification for DC, Nyquist, coherent
tones, a DC-plus-fractional-tone mixture, and noise at N=128/256/512 agrees with
time-domain weighted energy within `1.02e-14` relative error. Script and output:
`/tmp/sotf-spectral-energy-oracle.py` and `.json`.

| Unit-amplitude input | Sine-equivalent detector, dB | RMS detector, dB |
|---|---:|---:|
| DC / Nyquist | +3.010300 | 0 |
| Bin-1 cosine | +0.669468 | -2.340832 |
| Bin-1 sine | -0.791812 | -3.802112 |
| Coherent interior tone | 0 | -3.010300 |

For bin 1, the sine-equivalent result is exactly
`D²=A²*(1+cos(2*phi)/6)`. For white noise with variance sigma²,
`expected D² = 2*sigma²*sum(eta over B)/N`; bandwidth and truncated endpoint
neighborhoods therefore have explicit interpretations.

**Compatibility cost:** this changes boundary behavior. Under sine-equivalent
energy, DC with A=0.1 and threshold -20 dB is 3.0103 dB over threshold, implying
2.257725 dB steady gain reduction at ratio 4 and zero knee. It must not be
called an amplitude-correct DC result. Using the RMS version makes DC read its
physical amplitude but shifts all interior sine thresholds by 3.0103 dB; preset
semantics would then need an explicit migration or a detector choice. Neither
variant simultaneously preserves all of Convention A's desired special cases.

For this alternative, validate final WOLA gain against the **declared energy**
gain law, not a phase-invariant sinusoidal amplitude oracle. Verify all active
Hann lines receive the expected gain through the median stage, and compare a
nonstationary independent frame-energy/envelope reference. This is the best
bounded implementation option if the product explicitly adopts energy semantics.

## Deferred model-based alternative

Possible larger prototype: **complex, boundary-only coherent-component calibration**.
Keep the ordinary five-bin detector in the interior and for observations that
do not fit a reliable isolated component. Do not infer correction from magnitudes
alone or apply an unconditional amplitude estimate to arbitrary program material.

1. For each edge, inspect six complex bins (0..5, or their Nyquist mirror).
   For candidate frequency nu, form complex templates
   `C=(W(k-nu)+W(k+nu))/2` and `S=(W(k-nu)-W(k+nu))/(2i)`.
2. Stack real and imaginary observations into a real two-column system. Solve
   the 2x2 normal equations for `a,b`, retaining a rank-one DC/Nyquist case.
   Amplitude is `hypot(a,b)`. Retain residual energy and conditioning.
3. Restrict frequency search to the first three bins. Precompute the coarse
   projection templates during initialization; use a fixed iteration count for
   optional refinement. Mirror the same operation at Nyquist. No allocations,
   logging or data-dependent iteration count are needed on the callback.
4. Generate the fitted unit-amplitude spectrum, including phase and both images.
   Compute its peak local-energy proxy with exactly the existing neighborhood.
   Its reciprocal is the calibration factor. Apply one factor consistently
   across the affected boundary detector neighborhoods (including indices 0/1
   for DC and the symmetric Nyquist pair), before the existing median/envelope
   stages. Taper to unity where image interference is negligible; preserve
   interior behavior. Calibrating only detector index 0 is expressly insufficient.
5. Gate/blend the correction by model residual and conditioning. A conservative
   first contract can cover DC/Nyquist and isolated tones at least 0.5 bins from
   the edge, with correction bounded to 0.5..2 in amplitude. The measured largest
   required correction at 0.5 bins is 1.7321. The cap and frequency limitation
   must be documented rather than called a complete all-frequency calibration.

Why guards are necessary: in a separate deterministic check, a mixture of
tones at 0.73 and 1.5 bins drives an unconstrained one-tone fit toward frequency
`4.5e-6` and amplitude about 24,879, while its residual power fraction is 0.0204.
DC plus a half-amplitude bin-1 sine gives residual fraction 0.00358. White noise
gives 0.324. A 0.73-bin tone plus white noise at amplitude 0.01 gives residual
fraction about `1.1e-7` in those six bins. A proposed residual threshold is an
engineering policy that still needs mixture/noise and smooth-transition tests.

The two-column condition number rises from 2.81 at 0.5 bins to 14.5 at 0.1,
145 at 0.01 and 1,453 at 0.001. For a phase that places a zero crossing near the
window center, little of the tone's latent full-cycle amplitude is observed.
Recovering that amplitude amplifies uncertainty; an energy fallback is required.
The exact finite-record fallback statistic and confidence crossfade must be
documented. Merely capping an unstable fit without identifying its validity
would hide the limitation.

## Required implementation gates before shipping

- Independent direct-DFT amplitude and static gain-law tests: DC, Nyquist,
  coherent bin 1 with all phases, fractional frequencies 0.5..3, both edges,
  N=1024/2048/4096 and relevant sample rates. Include unsupported sub-half-bin
  cases to verify the documented bounded fallback.
- Verify final WOLA audio gain, not only maximum `detector_gr`. A DC/Nyquist
  input at threshold with zero knee should have no settled gain reduction.
  Above threshold, compare with `(1-1/ratio)*(level-threshold)` at every
  nonzero Hann line and on the final settled waveform.
- Mixtures, DC offsets, noise, silence, nonfinite sanitization, and transitions
  across the confidence/frequency limits must not cause spurious extreme gain
  reduction. Include steady tones with moving phase across successive 75%-overlap
  hops, and parameter automation.
- Preserve adaptive/linked behavior, reset and callback partition invariance;
  measure worst-case callback work and zero allocation with both edges active.

This is a scoped proposal, not a completed production correction. The root
requested that SpectralCompressor production remain unchanged. Preserve the
current measured behavior and limitations in the audit. Convention B is simpler
and well defined for arbitrary audio, but changing documentation alone would
not turn the current detector into that estimator. Convention A needs more
than endpoint weights, and its tone estimator remains deferred.
