# sotf-plugin-saturation

Saturation / Harmonic Exciter — adds warmth and harmonic richness.

## What It Does

Adds harmonic overtones through deterministic nonlinear waveshaping. The Tube
and Tape labels describe static, tube- and tape-flavoured curves; they are not
physical circuit, bias, hysteresis, or magnetic-head models.

## Features

- **Multiple saturation curves**: Soft clip, odd-symmetric Tube-style curve,
  memoryless Tape-style sigmoid, split-band Exciter, and a bounded Asymmetric
  curve with controlled even harmonics
- **Alias suppression**: Soft Clip supports first-order antiderivative
  antialiasing (ADAA); all modes can request 2x or 4x host oversampling
- **Drive control**: From subtle warmth to heavy distortion
- **Mix control**: Blend dry and saturated signal

## Aliasing and parallel mixing

The ADAA flag changes **Soft Clip only**. Tube, Tape, Asymmetric, and Exciter
retain their own direct nonlinear curves with either flag setting. No mode is
guaranteed alias-free. The oversampling parameter requests the host's up/down
sampling wrapper; calling the saturation kernel directly does not oversample.

In Soft Clip's small-signal limit, ADAA averages adjacent driven samples.
Its response contains `(1 + z^-1) / 2`: half a sample of phase delay and a
frequency-dependent magnitude reduction. The dry path uses the current input
sample, so intermediate Mix settings include the complex sum of these paths.
The native plugin reports zero integer latency; enabling host oversampling
adds the wrapper's reported latency. ADAA therefore also changes coloration,
particularly near Nyquist, rather than acting as a transparent quality switch.

Successful initialization starts a fresh stream: crossover, DC, ADAA, and
envelope histories are cleared, and control smoothers settle at the configured
values. Live parameter edits continue to use per-frame smoothing.

## Asymmetric model contract

The Asymmetric mode is an explicit memoryless family, not analog circuit
emulation. It subtracts the zero-input value from a bias-shifted `tanh`, then
normalizes the positive and negative rails independently to +1 and -1. Tone
maps the dimensionless bias from 0.08 to 0.40. The result is zero at zero input,
bounded, deterministic, and intentionally produces even harmonics. Use 2x or
4x host oversampling for alias suppression and the DC Block option when
programme-dependent offset is undesirable.

## Architecture

```
src/
├── lib.rs     # SaturationPlugin implementation
└── params.rs  # Parameter definitions
```

## Testing

```bash
cargo test -p sotf-plugin-saturation
cargo test -p sotf-plugins --test audit_saturation_host
```

The independent spectral reference samples each continuous curve at 32,768
phase points, checks convergence against 16,384 points, then explicitly folds
its harmonic series into a 4,096-sample base-rate spectrum. Exciter's reference
uses an independently derived bilinear fourth-order crossover response. The
sweep covers all five modes, four coherent frequencies, drives 1/4/12, and
44.1/48/96 kHz. It measures folded harmonics separately from legitimate
in-band harmonics; aliases coincident with legitimate harmonics are excluded
from that energy metric.

Additional checks cover ADAA's complex dry/wet response, numerical interval
integration, DC rejection, reinitialization, automation, reset, and callback
partitioning through 192 kHz. Real factory/host tests require at least 3 dB
less folded distortion relative to the fundamental with 2x and 4x oversampling
at the tested high-frequency, high-drive points. These are bounded regression
cases, not universal alias-suppression guarantees.

## License

Part of the SOTF (Sound of the Future) project.
