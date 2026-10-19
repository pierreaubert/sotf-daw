# sotf-plugin-spectral-compressor

Spectral compressor — per-bin frequency domain dynamics processing.

## What It Does

Applies dynamic range compression to local frequency regions in the STFT domain.
It can control resonances more selectively than a small number of crossover
bands. Analysis windows and frequency smoothing spread attenuation to nearby
frequencies; transient shape and phase can also change during gain transitions.

## Features

- **Per-bin compression**: Independent dynamics for each frequency bin
- **STFT-based**: Operates in the frequency domain via Short-Time Fourier Transform
- **Fine frequency control**: More regions than a typical multiband compressor
- **Spectral taming**: Reduces resonances with adjustable neighboring-bin smoothing
- **Targeted processing**: All, tonal, or transient spectral components
- **Adaptive threshold**: 500 ms programme-relative estimator with deterministic priming
- **Channel linking**: Continuous independent-to-maximum detector linking for image stability

The processor uses periodic Hann analysis/synthesis at 75% overlap and reports
one FFT frame of latency. FFT size is structural and requires host graph rebuild.
Target mode is an integer choice (`All`, `Tonal`, `Transient`); classification
masks and envelopes are independent per channel. Construction rejects unknown,
non-finite, and out-of-range state. Initialized processing supports blocks up to
16384 frames without heap allocation.

Threshold is defined as local narrowband coherent amplitude. The detector sums
five calibrated Hann bins before compression, so equal tones remain stable across
FFT sizes and fractional-bin alignment. Broadband per-bin readings intentionally
follow FFT analysis bandwidth; this control is not an SPL, LUFS, or PSD threshold.

Spectral smoothing is a reversal-invariant, edge-normalized box kernel. Its
0–100% amount maps to a radius of 0–12 bins (bin width is `sample_rate / FFT size`).
Adaptive threshold uses `exp(-hop/(0.5*sample_rate))`, primes from the first valid
spectrum, and reprimes on reset or enable. `channel_link` blends each independent
gain-reduction value toward the maximum across all channels without mixing audio.

## Dynamics clock

With FFT length `N` and hop `H=N/4`, detectors and gain envelopes update once
per completed analysis window. Attack/release coefficients are
`exp(-H / (sample_rate * time_seconds))`, applied to gain reduction in dB.
They describe the temporal envelope's exponential time constant, not a
sample-domain peak clamp or an exact output rise time: windowing and overlap-add
also shape the audible transition. Startup's zero-padded windows advance this
same clock. Tonal/transient classification masks apply on the following hop.

Each hop applies the three-bin median and symmetric frequency smoother to a
scratch copy of the temporal envelope. Their result controls that hop's audio
without feeding back into attack/release history. This prevents repeated
frequency diffusion from silently changing the envelope trajectory. A spectral
smoothing change immediately affects the next completed hop's gain field.
Threshold control smoothing advances by `H` frames per hop; Mix smoothing
advances once per output frame. Both use a 20 ms control time constant.

## Architecture

```
src/
├── lib.rs                              # crate surface
├── params.rs                           # canonical specs and layout
└── lib/
    ├── spectral_compressor_plugin.rs   # host contract and streaming DSP
    ├── spectral_compressor_plugin_params.rs # strict serialized state
    ├── stft_state.rs                   # prepared circular WOLA state
    ├── misc.rs                         # compression/calibration helpers
    └── tests.rs                        # DSP property regressions
```

## Testing

```bash
cargo test -p sotf-plugin-spectral-compressor
cargo test -p sotf-plugins --test audit_spectral_compressor_host
cargo run -p sotf-plugin-spectral-compressor --features qa --bin qa-spectral-compressor
```

`audit_dynamics.rs` compares complete streamed and drained output with an
independent f64, full-complex offline WOLA model. It derives periodic Hann
windows, five-bin detector calibration, soft-knee gain, channel linking,
hop-rate attack/release, median/box filtering and synthesis without production
DSP helpers or the circular scheduler. All three FFT lengths and 44.1/48/96/192
kHz cover isolated tones, fractional-bin tones, level steps, broadband bursts,
silence boundaries and first/final impulses, with a fixed 3e-6 peak error bound.

Separate tests assert delayed unity/COLA reconstruction, linked-channel ratios
and polarity, independent quiet-channel identity, and quiet-carrier sidebands
below -60 dBc beside a modulated distant resonance. That pumping bound applies
to the documented synthetic case, not arbitrary programme material. The actual
factory/host test crosses off-hop control boundaries, target modes and adaptive
threshold settings with different callback sizes.

## License

Part of the SOTF (Sound of the Future) project.

## Finite streams

`drain` emits the zero-input continuation of the current spectral processing,
including dry-path delay and all remaining overlap-add windows. With FFT size
`N`, hop `H=N/4`, and `T>0` accepted input frames, the suffix has
`2N + floor((T-1)/H)*H - T` frames. The native tail declaration is the
phase-independent maximum `2N-1`. Envelope and classifier history can change
gain, but cannot generate audio once every input-containing window has ended.

Drain uses a prepared one-hop cache, accepts any positive destination capacity
aligned to the channel count, and leaves unused destination samples untouched.
An empty stream completes without output. The first successful drain closes
the stream; further nonempty input and parameter changes require `reset` or
successful reinitialization. Invalid capacities and sample rates consume no
audio. Completed calls remain complete, without allocation or deallocation.

Startup includes the three zero-padded negative-time windows at origins
`-3H`, `-2H`, and `-H`. Negative synthesis samples are discarded explicitly;
positive samples receive all four Hann-square contributions before output
begins at the unchanged `N`-frame delay. Unity processing therefore preserves
every input sample, including the first and final samples of very short streams.
Priming windows also advance the existing detector/envelope rules; compression
at startup can differ from steady state because those windows contain silence.
