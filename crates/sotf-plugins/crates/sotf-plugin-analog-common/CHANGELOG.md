# 0.5.0

## Fixed

- Preserve the prepared layout after rejected rate changes, including positive
  `f64` values that underflow to zero in `f32`.
- Use the documented Harmonics fallback for unknown schema default model IDs.

## Validated

- Six-model shared conformance at 44.1/48/96/192 kHz, including zero color,
  drive/trim units, automation, reset, chunking, finite recovery and heap guards.
- Independent coherent alias-energy proxy and model-specific recursive decay
  measurements, with explicit limits on the 768 kHz Console comparison.

## New

- Initial release: `AnalogColorStage` wrapper, shared analog-block `ParamSpec`
  constructors, model names in stable id order, and the −18 dBFS reference
  level shared by the analog plugin family.
