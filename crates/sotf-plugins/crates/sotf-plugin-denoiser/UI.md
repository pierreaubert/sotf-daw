# Denoiser UI contract

The authoritative layout is `params::consts::LAYOUT`.

- Config: Low Latency.
- Main groups: Reduction (reduction, floor, smoothing, transparency), Timing, Spectral
  Subtraction, Noise Profile, Curve (low/mid/high knobs), and Monitor (audition toggle).
- Analysis tab: polyphonic detection, decision-directed SNR, masking, and smoothing controls.
- MCRA tab: alpha-S, alpha-P, minimum window, and delta.
- Formant tab: enable plus conditionally enabled strength.
- Advanced tab: multi-resolution, harmonic/percussive, spatial enable, and conditionally enabled
  spatial strength.

Low Latency and Multi-Resolution are setup/structural controls and require plugin reconstruction.
Smoothing, Transparency, Formant Strength, Spatial Strength, and the Curve knots are serialized
as normalized fractions and displayed as percentages where the schema applies a `100x` scale.
