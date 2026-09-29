# Limiter audio-path oversampling (AUD-010)

Current status: prepared limiter-local 2x/4x processing is implemented; see
[the implementation and verification report](limiter-oversampling-implementation.md)
and [native/factory propagation evidence](limiter-oversampling-propagation.md).
MIDI and IAMF remain excluded.

The investigation below records the original 2026-09-28 baseline before that
implementation. Its measurements and historical conclusions are preserved;
references to the then-current limiter describe that baseline.

## Comparison and acceptance criteria

[FabFilter Pro-L 2's oversampling documentation](https://www.fabfilter.com/help/pro-l/using/oversampling),
checked 2026-09-28, distinguishes running the limiting process at a higher rate
from true-peak detection. It also explicitly requires its final downsampled output
to respect the chosen sample/true-peak ceiling. These are separate acceptance
criteria; a detector-only option does not address all nonlinear aliasing.

SOTF currently has rate-dependent interpolated input detection and optional
predictive output ISP correction, all operating around a base-rate gain stage.
The generic host oversampler can already run that gain stage at 2x or 4x, but a
simple wrapper is not sufficient to provide a new ceiling-preserving mode.

## Reproducible experiment

Source: `/tmp/sotf-limiter-oversampling-probe.rs`.
Log: `/tmp/sotf-limiter-oversampling-probe.log`.
The source was temporarily run as the limiter crate's `audit_oversampling`
example, then removed from the crate. It uses existing production limiter and
`AutoOversampledPlugin` APIs; no production algorithm was changed.

All cases use mono 48 kHz, ceiling -12 dBFS and release 10 ms. Tone input is
amplitude 0.9, phase 0.37, 96,000 samples. The last 48,000 samples form a coherent
one-second window. Independent f64 sine/cosine projection measures the fundamental
and folded third harmonic, without production FFT or detector helpers. Processing
uses 257-frame callbacks. The tone experiment has zero lookahead and ISP off.

| Input tone | Measured frequency | 1x spur dBc | 2x spur dBc | 4x spur dBc |
|---|---|---:|---:|---:|
| 7 kHz | 21 kHz (ordinary harmonic, control) | -56.727 | -58.161 | -58.327 |
| 11 kHz | 15 kHz (folded) | -53.857 | -67.071 | -84.264 |
| 17 kHz | 3 kHz (folded) | -56.951 | -92.531 | -83.587 |
| 21 kHz | 15 kHz (folded) | -50.762 | -57.359 | -62.163 |

These measurements establish improvement in the selected aliases, not a universal
factor ranking, perceptual quality claim, or comparison with proprietary DSP.

A separate burst matrix uses 1/2/3/5/13/31/127 samples and five impulse, alternating,
sinusoidal, asymmetric and two-tone patterns in zero-padded 16,384-sample streams.
The peak below is measured directly on the final emitted samples, so it does not
depend on a disputed reconstruction kernel. ISP-on cases use 0.25 ms lookahead;
ISP-off cases use zero lookahead.

| Existing ISP mode | 1x maximum | 2x maximum | 4x maximum |
|---|---:|---:|---:|
| Off | -12.000000 dBFS | -10.072892 dBFS | -10.083165 dBFS |
| On inside oversampler | -12.000000 dBFS | -11.912005 dBFS | -11.983037 dBFS |

Thus simply setting a preferred oversampling factor would violate the existing
sample ceiling after downsampling. Enabling the existing ISP stage only inside
the higher-rate plugin also fails to establish a final base-rate ceiling.

## Implementation requirements

- Preserve the current 1x default and old parameter indices/preset behavior.
- Treat the audio oversampling choice as structural; allocate filters, scratch
  and final-output protection before processing, and report their complete latency.
- Protect the final downsampled output at its actual sample rate. Do not relabel
  the inner true-peak detector as a guarantee about samples after another filter.
- Apply dry/wet mixing once with matched delay and explicitly scope any ceiling
  guarantee to fully wet output, as the current ISP contract already does.
- Verify the complete proposed chain for alias reduction, final sample and
  independently reconstructed peaks, startup and EOS, reset, variable callback
  partitions, automation timing, native/FFI configuration, and cold allocations
  and frees. Do not substitute a blanket output attenuation for this evidence.
