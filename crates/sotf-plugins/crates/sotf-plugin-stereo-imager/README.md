# sotf-plugin-stereo-imager

Multi-band M/S stereo width control.

## What It Does

Controls the stereo width of audio using Mid/Side processing. The signal is split into frequency bands, and each band's stereo width can be independently adjusted — from mono (0%) through natural (100%) to hyper-wide (200%+). Useful for widening the high frequencies while keeping bass centered, or for collapsing problematic stereo information.

## Features

- **M/S processing**: Mid/Side encoding for precise width control
- **Multi-band**: Independent width per frequency band
- **Width range**: From mono (collapsed) to hyper-wide
- **Frequency-dependent**: Widen highs while keeping bass centered

The plugin is strictly stereo. Its complementary first-order crossovers use
6 dB/octave slopes. The three side bands are a lowpass, the difference between
the upper and lower lowpasses, and the upper highpass. They sum to the original
side signal, so neutral widths are sample-transparent at every mix and all-zero
band widths produce mono. Mono Bass applies the complementary first-order
highpass to the side signal while leaving the mid signal unchanged.

This topology preserves zero latency and neutral transparency. It has gentler
band isolation than LR24 crossovers; steeper phase-aligned or linear-phase modes
are not implemented. Earlier builds subtracted phase-rotated LR24 outputs from
the dry side, which could increase bass width and prevent complete mono collapse.

Construction validates finite ranges and strict crossover ordering; initialization
and automation also require the upper crossover below Nyquist. Processing performs
bounded work per frame and uses no heap allocation or callback-sized scratch buffer.

## Image and automation behavior

The processor preserves `M = (L + R) / 2` and changes only
`S = (L - R) / 2`. Consequently the mono fold is unchanged, within floating-point
rounding, even while controls move. A mono source remains mono; widening cannot
create independent spatial information. Increasing side energy can make measured
L/R correlation negative. This is an intentional width tradeoff, and it does not
mean that the mono fold gains the widened side signal.

Mono Bass suppresses the low side band using the same gentle 6 dB/octave
complementary split. With otherwise neutral controls and full wet mix, the side
response is a first-order highpass at Low-Mid Freq: approximately -3 dB at that
cutoff, rather than a brick-wall mono region. Partial dry mix retains some of the
original bass side signal. Global width and band widths multiply; their maximum
combined side gain can exceed unity, so this effect does not impose a peak ceiling.

Controls follow 10 ms one-pole trajectories on the sample clock; cutoff
coefficients update every 16 samples. Rounding-stalled controls finish at their
exact target on a real sample. Zero-frame callbacks do not advance time. Exact
zero mix and neutral widths preserve input bits while filter histories continue
following the source. Tiny nonzero mix settings remain active. Changing back to
wet therefore uses current crossover history. The first direct sample has zero
algorithmic delay; recursive crossover decay is not finite mathematical support.
Reset and successful rate preparation clear histories and adopt current targets.
Host bypass is separate from the plugin's dry mix and may suspend processing.

## Architecture

```
src/
├── lib.rs     # StereoImagerPlugin implementation
└── params.rs  # Parameter definitions
```

## Testing

```bash
cargo test -p sotf-plugin-stereo-imager
```

## License

Part of the SOTF (Sound of the Future) project.
