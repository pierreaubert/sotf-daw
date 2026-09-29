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
