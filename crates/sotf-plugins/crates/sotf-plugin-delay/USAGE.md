# Delay

## Overview

An audio delay effect with adjustable delay time, feedback, and dry/wet mix. Use it for echo effects, slapback, doubling, or creative rhythmic patterns. Supports fractional-sample delay via four-point Lagrange interpolation.

## Features

### Delay Line

A feedback delay line that stores audio and plays it back after a configurable time. Feedback sends the delayed signal back into the buffer, creating repeating echoes that decay over time.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Delay Time | 0.0 to 5000 | 100 | ms | Time between the dry signal and the first echo (0 = passthrough) |
| Feedback | -95 to 95 | 30 | % | Amount of delayed signal fed back into the delay buffer. Higher = more repeats; negative values invert the recirculated phase |
| Mix | 0 to 100 | 50 | % | Dry/wet blend. 0% = dry only, 100% = delayed only |

### Smooth Parameter Changes

Delay time, feedback, and mix are smoothed to avoid hard discontinuities during
real-time adjustment. The default delay-time path uses four-point Lagrange
interpolation with tape-style behavior, so automation or LFO modulation can
intentionally produce Doppler/pitch glide. The structural `pitch_preserving`
mode instead switches nonidentical delay targets through silence over 20 ms
while both read heads remain stationary; LFO rate and depth must be zero in
that mode.

The LFO uses bounded one-sided clamping at the minimum and maximum delay: the
feasible half-cycle remains active while only the portion outside the declared
delay range is clipped. Allpass enable/bypass and coefficient changes crossfade
over 20 ms, and the filter state runs continuously to avoid stale-tail clicks.

Per-channel mode is reserved for pure RoomEQ routing delays: mix is fixed wet,
feedback and LFO are zero, and allpass and pitch-preserving modes are disabled.
Its delay memory is sized from the declared per-channel automation maximum
rather than the global five-second effect range. Effect settings are rejected
at construction, and runtime writes that deviate from the pure routing values
are rejected on both the single and batch paths without changing the accepted
configuration or populated history.

Fractional delays below two samples use a causal four-tap stencil: taps the
ring cannot yet supply are solved implicitly through the current frame, which
is exact with and without feedback, while the sub-one-sample future tap is
extrapolated as the current frame value. The stencil is continuous at every
integer boundary and preserves the closed-loop DC gain.

## Demos

### Demo: Slapback Echo

**Scenario:** Adding a single quick echo to a vocal or guitar for a rockabilly/vintage feel.
**Before:** Dry vocal or guitar track.
**After:** A single tight echo ~80 ms after each note, adding depth without clutter.
**Config:**
```json
{
  "delay_ms": 80.0,
  "feedback": 0.0,
  "mix": 0.3
}
```

### Demo: Rhythmic Echo

**Scenario:** Creating a rhythmic delay pattern synced to a tempo (e.g., 120 BPM = 500 ms per beat).
**Before:** A synth lead or percussion loop.
**After:** Repeating echoes that decay over several beats, creating rhythmic interest.
**Config:**
```json
{
  "delay_ms": 500.0,
  "feedback": 0.5,
  "mix": 0.4
}
```

### Demo: Ambient Wash

**Scenario:** Long delay with high feedback for ambient/drone textures.
**Before:** A simple sustained sound or pad.
**After:** Dense, evolving echoes that build into an ambient wash.
**Config:**
```json
{
  "delay_ms": 1200.0,
  "feedback": 0.8,
  "mix": 0.6
}
```

## Presets

### Slapback
**Use case:** Quick single echo for vocals, guitar, snare
```json
{
  "delay_ms": 80.0,
  "feedback": 0.0,
  "mix": 0.3
}
```
**Tips:** Keep feedback at 0 for a single reflection. Adjust delay between 50-120 ms to taste.

### Stereo Doubling
**Use case:** Subtle thickening effect
```json
{
  "delay_ms": 20.0,
  "feedback": 0.0,
  "mix": 0.5
}
```
**Tips:** Very short delays (10-30 ms) create a chorus/doubling effect rather than a distinct echo.

### Quarter Note Echo (120 BPM)
**Use case:** Rhythmic delay synced to tempo
```json
{
  "delay_ms": 500.0,
  "feedback": 0.4,
  "mix": 0.35
}
```
**Tips:** Adjust delay time to match your tempo: delay_ms = 60000 / BPM for quarter notes.

### Long Ambient Tail
**Use case:** Ambient textures and soundscapes
```json
{
  "delay_ms": 2000.0,
  "feedback": 0.75,
  "mix": 0.5
}
```
**Tips:** Feedback above 0.8 creates very long decay — be careful not to build up to clipping.

## Tips & Best Practices

- Feedback is capped at 95%; this does not prevent overload. A coherent DC input can approach 20 times its input level at +95% feedback. Leave headroom or limit downstream.
- For tempo-synced delays: quarter note = 60000/BPM ms, eighth note = 30000/BPM ms.
- Use mix at 100% in a send/return configuration; use 20-50% when inserted directly.
- Very short delays (< 30 ms) create comb filtering / chorus effects rather than distinct echoes.
- Delay time changes are smoothed — you can automate delay time without clicks.

## Offline tails and tempo changes

Native `drain` emits bounded nonrecursive delay support. Once feedback has
been used, tail metadata remains `Infinite` until reset, and native drain can
return complete without rendering recursive repeats. Offline callers that
want those repeats must continue normal processing with silence **before**
native drain, under an explicit tail policy.

Choose a maximum duration and report when that limit truncates the render.
An optional energy cutoff needs an observation window covering the longest
possible echo gap; a silent callback alone cannot establish that the tail is
finished. Freeze automation during that check. Modulation, allpass feedback,
longer routes and other plugins can require a larger window or a fixed-duration
policy. For the static 10 ms impulse regression at feedback ±0.95, a 10 ms
window with summed squared samples below `1e-8` leaves less than `1e-7`
unrendered energy; a three-second duration cap bounds the work. This is a
measured test policy, not a universal recursive-tail bound.

Tempo conversion belongs to the caller: quarter notes use `60000 / BPM` ms
and sixteenth notes use `15000 / BPM` ms. Set `delay_ms` at the intended
absolute sample when tempo changes. Tape mode glides pitch as the read head
moves; pitch-preserving mode keeps stationary heads and switches through a
brief dip. Both modes are tested through the host across abrupt tempo changes
and varied callback partitions. Changing modes after initialization requires
rebuilding the plugin.

## Signal Flow

```
Input ──┬──────────────────────────────────────┐
        │                                      │ × (1 - mix)
        ▼                                      │
   Delay Buffer ◄── feedback ── Delayed Out    │
        │                           │          │
        │                           │ × mix    │
        │                           ▼          ▼
        │                        Mix Blend ── Output
```
