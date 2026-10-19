# Band Merge

## Overview

Sums multiple frequency bands back into a single channel stream. The inverse of Band Split — takes N×B input channels (N output channels × B bands) and sums corresponding band channels together to reconstruct the original channel count.

## Features

### Configuration

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Bands | 2 to 8 | 2 | — | Number of bands to merge (must match the splitting plugin) |

### Channel Layout

For B bands and N output channels, the input has B×N channels:
- Input channels 0..N-1: Band 0
- Input channels N..2N-1: Band 1
- ...
- Input channels (B-1)×N..B×N-1: Band B-1

Each output channel is the sum of all corresponding band channels.

## Demos

### Demo: Recombining After Parallel Processing

**Scenario:** After Band Split + per-band processing, merge bands back together.
**Before:** 4-channel split signal (low-L, low-R, high-L, high-R).
**After:** 2-channel stereo with per-band processing baked in.
**Config:**
```json
{
  "bands": 2
}
```

## Presets

### Two-Band Merge
**Use case:** Merge output from a single Band Split
```json
{
  "bands": 2
}
```
**Tips:** Standard pairing with Band Split.

## Tips & Best Practices

- The band count must match the number of bands from the splitting plugin.
- Band Merge sums the supplied bands. A two-band Linkwitz-Riley split is
  magnitude-complementary but phase-shifted. Band Split's legacy multiband
  cascade has unequal branch phase and can change summed magnitude. Its
  `phase_compensated` mode matches later crossover phase in each earlier band,
  giving unity summed magnitude with frequency-dependent allpass group delay.
  Neither mode reconstructs the original waveform without phase rotation.
- No gain scaling is applied — if per-band processing added gain, the sum may clip.
- Denormals are flushed after merging to prevent CPU spikes.

## Host and realtime contract

- Initialize before processing. Callback rate must match initialization and both
  buffers must have exact checked lengths.
- Band count is limited to 2–8 and changes require graph replacement; exact
  no-op writes succeed. Presets reject unknown fields and invalid gains.
- Gain and mute transitions use one allocation-free 10 ms sample-rate-aware
  smoother. Reset snaps each band to its configured mute/gain target.
- Reading `reconstruction_error_db` arms one callback to compute normalized
  RMS(`output - unity-band-sum`) without logging or allocation.

## Signal Flow

```
Input (B × N channels) → Sum per output channel:
  output[ch] = input[ch] + input[ch + N] + input[ch + 2N] + ... + input[ch + (B-1)×N]
→ Output (N channels)
```

## Declared band-order validation

The native `BandMergePlugin::process_with_band_order` entry point accepts the
original splitter band index for each input group and rejects any sequence other
than `0..bands`. Wrong counts, reversed/permuted groups, duplicates and out-of-range
labels fail before output, gain smoothers or diagnostic state change. Successful
validation and merging allocate nothing. Each group still contains the original
output-channel order, and the caller must supply truthful labels.

The ordinary `Plugin::process` interface carries untagged PCM. It cannot infer
whether a caller swapped two bands or channels. Hosts using that interface must
preserve the splitter's frame-interleaved, band-major layout when wiring the graph.
The gain-weighting diagnostic cannot detect a routing permutation at unity gains;
use the labelled entry point when routing provenance is available.
