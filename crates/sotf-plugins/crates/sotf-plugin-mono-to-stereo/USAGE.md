# Mono to Stereo

## Overview

Converts a mono signal into stereo with a causal all-pass cascade. The right channel receives a
frequency-shaped phase response whose magnitude is unity in steady state; the left channel remains
direct. This avoids FFT circular wrap and random-bin pre-echo while retaining optional Haas widening.

## Features

### Stereo Width Control

Controls the right channel's allpass phase response. At width 0 the allpass is
bypassed; L and R are identical when Haas delay is also zero. Width 1 selects
the full configured phase response. Interchannel correlation still depends on
the source spectrum and Haas delay; full width does not promise zero correlation.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Width | 0.0 to 1.0 | 0.5 | — | Stereo width. 0 = mono, 1 = full decorrelation |

### Haas Delay

Adds a small time offset between left and right channels for additional spatial widening based on the Haas effect (precedence effect).

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Haas Delay | 0.0 to 5.0 | 1.5 | ms | Inter-channel delay for Haas-effect widening |

### Decorrelation Band

Defines the frequency range around which the causal all-pass phase rotation is concentrated.
These topology controls, including `freq_dependent`, are applied by rebuilding the plugin graph.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Decor Low | 100 to 500 | 300 | Hz | Lower frequency bound for decorrelation |
| Decor High | 1000 to 5000 | 2000 | Hz | Upper frequency bound for decorrelation |

## Demos

### Demo: Mono Vocal to Stereo

**Scenario:** A mono vocal recording needs to sit in a stereo mix without sounding narrow.
**Before:** Vocal is centered and flat, fighting with other centered elements.
**After:** Vocal has a natural stereo presence, occupying space in the mix without obvious doubling artifacts.
**Config:**
```json
{
  "stereo_width": 0.4,
  "freq_dependent": true
}
```

### Demo: Full Width for Ambient Textures

**Scenario:** A mono synth pad needs to fill the entire stereo field.
**Before:** Synth pad sounds narrow and small.
**After:** Wide, immersive stereo field that fills the speakers.
**Config:**
```json
{
  "stereo_width": 1.0,
  "freq_dependent": true
}
```

### Demo: Subtle Widening for Podcast

**Scenario:** A mono podcast recording needs slight stereo widening for a more spacious feel.
**Before:** Flat mono recording.
**After:** Slightly wider feel without obvious stereo effects — sounds natural on headphones.
**Config:**
```json
{
  "stereo_width": 0.2,
  "freq_dependent": true
}
```

## Presets

### Subtle Width
**Use case:** Gentle widening for mono vocals or dialogue
```json
{
  "stereo_width": 0.3,
  "freq_dependent": true
}
```
**Tips:** Check mono compatibility — fold to mono and ensure no cancellation artifacts.

### Natural Stereo
**Use case:** General-purpose mono-to-stereo conversion
```json
{
  "stereo_width": 0.5,
  "freq_dependent": true
}
```
**Tips:** Good starting point for most material. Adjust width to taste.

### Wide Spread
**Use case:** Full stereo field for pads, synths, or ambient content
```json
{
  "stereo_width": 0.8,
  "freq_dependent": true
}
```
**Tips:** At high width values, check that the sound still collapses to mono gracefully.

### Maximum Width
**Use case:** Effect-level stereo spread
```json
{
  "stereo_width": 1.0,
  "freq_dependent": false
}
```
**Tips:** This selects the full phase response. Check the mono fold for frequency-dependent cancellation.

## Tips & Best Practices

- The causal all-pass path reports zero host-compensated latency. Haas delay remains an intentional right-channel effect.
- Width = 0 and Haas = 0 produce exact duplication (L = R). A nonzero Haas delay still offsets the right channel at zero width.
- Check mono compatibility when using high width values — fold to mono and listen for cancellation.
- The decorrelation band (300-2000 Hz by default) keeps low bass highly correlated.
- Lower the decorrelation low frequency to include more bass in the stereo field (can reduce mono compatibility).
- For headphone listening, lower width values (0.2-0.4) often sound more natural than full width.

## Signal Flow

```
Mono Input ───────────────→ Left (direct)
     │
     └→ Causal all-pass cascade → optional Haas delay → Right

Width changes the stable all-pass coefficients; width 0 uses an exact L=R duplicate fast path.
```

The duplicate fast path requires both zero width and zero Haas delay. Delay
history keeps running in this path so enabling Haas reads recent source audio.
Width ramps, settling and allpass re-entry use the sample clock, independent of
callback sizes. Empty callbacks preserve state. Processing requires the prepared
sample rate; reset clears histories and adopts the current width target.

For a steady tone with right-channel transfer `H`, normalized mono fold is
`(1 + H) / 2`. Unit magnitude in each channel therefore does not ensure a flat
mono sum. For example, a 5 ms Haas delay at zero width puts a fold-down notch
near 100 Hz. Correlation follows phase and source content rather than width
alone. The first transient remains immediate in the direct left channel; the
right onset includes the rounded Haas delay and the causal allpass response.
Reported latency remains zero to preserve that intentional spatial offset.

`DawHost` rejects bypassing this 1-to-2 channel plugin because that would change
graph geometry. For duplicate output, automate width to zero and disable Haas;
the existing width ramp handles the allpass transition. Haas changes select a
new integer delay tap immediately and can introduce a time discontinuity.
