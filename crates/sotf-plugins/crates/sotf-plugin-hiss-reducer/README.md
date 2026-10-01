# sotf-plugin-hiss-reducer

SOTF high-frequency noise reducer with a zero-latency default and an optional
fixed-latency spectral restoration mode.

The plugin wraps `plugins_denoiser::hiss::HissReducer` in the SOTF host trait.
It is deliberately a lightweight time-domain high-band downward expander, not
an STFT noise estimator: the Threshold control is an absolute dBFS high-band
level, not an SNR value. `Spectral mode` instead selects a 1024-point WOLA
minimum-statistics estimator for stationary hiss. It is structural because it
changes the plugin latency from 0 to exactly 1024 samples.

## DSP contract

- Interleaved, in-place, channel-independent processing with zero algorithmic
  latency and `O(frames × channels)` bounded work.
- An exact-mapped complementary one-pole split defines the high band. The
  shallow 6 dB/octave transition is intentional; live cutoff changes ramp over
  5 ms and the visible cutoff is limited to `min(16 kHz, 0.45 × sample rate)`.
- Fast (5 ms) and slow (100 ms) high-band power envelopes identify persistent
  low-level energy. A 30 ms persistence requirement, threshold hysteresis,
  20 ms hold, continuous reduction depth, and 1/50 ms gain attack/release avoid
  waveform-cycle modulation and binary gain clicks.
- Live bypass keeps analysis/filter state warm and crossfades over 5 ms. A
  plugin initialized disabled is exactly dry, as is settled bypass or zero
  Strength.
- Processing is allocation-free. Non-finite input is replaced with silence and
  decaying filter/envelope state snaps to zero before becoming subnormal.
- Spectral mode uses a periodic Hann window, 256-sample hop, and minimum
  statistics over eight slots spanning about 512 ms at every sample rate.
  Above Frequency, a calibrated high-band RMS Threshold gates 15/50 ms
  attack/release-smoothed Wiener gains; three-bin tonal-main-lobe protection
  preserves sustained narrowband programme. A 5 ms aligned wet/dry ramp keeps
  live bypass click-free. Disabled spectral processing stays latency-correct
  by emitting 1024-sample delayed dry audio; callbacks always return their
  requested frame count.

The plugin must be initialized at a supported nonzero sample rate before
processing, and each `ProcessContext` must use that same rate.

## Finite spectral streams

Spectral mode implements `drain`: after `T>0` accepted frames it returns
`2048 + floor((T-1)/256)*256 - T` frames of zero-input continuation, including
remaining transform overlap and the aligned dry path. The native tail bound is
2047 frames, and the prepared drain cache holds one 256-frame hop. Any positive
channel-aligned destination capacity is accepted, and unused samples stay
untouched. Empty streams complete immediately.

The first successful spectral drain closes the stream; nonempty input and
parameter changes require reset or reinitialization. Invalid rates/capacities
consume no history, completion is stable, and callbacks allocate or free no
storage. Existing startup timing remains unchanged.

Conventional mode retains its existing unknown-tail declaration and no-drain
behavior. Its recursive lowpass can produce audio after input ends while
reduction is active; spectral finite-stream support does not cover that mode.

## Noise profile capture (R1)

`Learn Noise` captures a 1 s noise-only reference with the same exact-mapped
one-pole high-band split the time-domain reducer uses, storing one RMS floor
per channel (`NoiseProfileData`, format v1). `Use Profile` enables it;
`Clear Profile` discards it. Capture is frozen during drain, reset discards a
partial capture while preserving the stored profile, and all trigger/process/
reset paths are allocation-free (pre-allocated buffers).

In time-domain mode an enabled profile moves the effective threshold to the
louder of the user threshold and the measured floor plus 6 dB headroom, so
hiss louder than the user threshold still engages reduction. On multichannel
material the single backend threshold follows the loudest channel floor
(maximum across channels), so a quiet channel inherits a louder channel's
threshold and may over-reduce split program; this is a deliberate compromise
forced by the shared single-threshold backend. The default (profile off) is
bit-identical to previous behavior.

In spectral mode an enabled profile replaces the minimum-statistics
per-bin noise with the value derived from that channel's floor: the
broadband floor is converted to an FFT-bin aggregate by exactly inverting
the live Parseval formula, then spread white across the bins at/above the
live cutoff. The live gate still applies, so loud program stays untouched.
Engaging a spectral profile deepens reduction versus live minima estimation
(the unbiased reference suppresses deeper than the bias-shallow minima —
intended behavior); the curve remains the per-band limit. This white spread
is a coarse v1 approximation, not a captured colored spectrum: colored hiss
is under/over-estimated per bin, and per-band measured capture stays an
open requirement.

Floors are broadband references measured at the capture rate and cutoff:
reusing a profile at another rate or after a cutoff change shifts the
measured band (white hiss reads about +1.8 dB from 48 kHz to 96 kHz at a
fixed 4 kHz cutoff by one-pole variance arithmetic). The 6 dB margin absorbs
this for engagement while depth shifts roughly ±2 dB. Profiles are preserved
across reinitialization deliberately; re-capture after changing the sample
rate or cutoff for critical work.

## Reduction curve and channel linking (R2)

Three controls (`Curve Low/Mid/High`, 0..1, default 1.0) scale the spectral
per-bin maximum reduction at fixed log-spaced anchors (1/4/12 kHz, clamped
outside, log-interpolated between). `Link` (`Independent`/`Linked`, default
`Independent`) selects per-channel versus shared detectors. Both round-trip
through the registry, getters/setters, schema, presets, and JSON state; the
flat curve and independent link preserve legacy behavior exactly. The curve
is applied in spectral mode only (the single-band time-domain detector has
no per-frequency hook, by design).

Link semantics differ by mode, and both preserve the stereo image through
equal gains: the spectral link vetoes on split program (reduction engages
only while every channel is quiet, so a loud channel spares the quiet
channel's hiss), while the time-domain link drags (the shared maximum
reduction depth pulls the loud channel down with the quiet one).

## Transient guard (spectral, opt-in)

`Transient Guard` (default off) enables the backend broadband-onset guard
in spectral mode: each hop compares instantaneous high-band power against
a guard-local recent-mean reference, and a confirmed onset lifts per-bin
targets toward unity with fast gain rise for that hop plus a short hold
covering the impulse span. Stationary hiss and tones track the reference
and never trip the detector, so engaged suppression between transients
is unchanged; linked channels share the onset decision. The reference
seeds once the analysis window fills, so transients in the first ~21 ms
after reset/initialization are unprotected by design (pinned as a blind
window, not as preservation). The flag is stored in both modes but
applied spectrally only; time-domain audio is bit-identical with it on
or off. Old presets, profiles, and defaults never enable it.

Product contract: the guard protects broadband onsets at or above the
proven unit-impulse detection floor on stationary hiss beds near the
-30 dBFS operating threshold (strength 0.85, 48 kHz mono/stereo pinned;
44.1 kHz smoke covered). Quieter onsets below ~0.5 amplitude on the same
bed may not reach the 2.0x firing ratio and stay unprotected; louder
program that closes the live gate stays untouched with or without the
guard. Silence-to-hiss transitions legitimately fire (broadband rise)
with settled suppression recovering below -2 dB. Per-band colored
profiles are out of scope: the v1 floor is spread white, and guard
behavior on strongly colored beds is characterized only for stationary
lowpassed hiss (no fire).

## Mode applicability (R3)

| Setting | Time-domain | Spectral |
|---|---|---|
| Capture/learn/clear/use controls | Full (threshold follows floor + 6 dB) | Full (per-bin noise from floor) |
| Reduction curve | Stored, not applied (spectral-only) | Full (per-bin maximum reduction) |
| Link mode | Full (shared max-depth detectors) | Full (shared targets + quiet-everywhere gate) |
| Transient guard | Stored, not applied (spectral-only) | Full (opt-in onset guard) |

Old five-field presets load with the new compatible defaults
(`Use Profile` off, flat curve, `Independent`, guard off, no profile).
Corrupt profile blobs fail loading transactionally; well-formed blobs for
another channel count are dropped as inapplicable. `Params::VERSION` stays
2 (append-only).
