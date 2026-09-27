# sotf-plugin-analog-common

Shared building blocks for the analog plugin family (`analog-eq`,
`analog-compressor`, `analog-limiter`). Not a plugin itself.

## What It Holds

- `AnalogColorStage`: one `math-analog` model (Harmonics, Static,
  Hammerstein, Tape, Transformer, Console Preamp) behind a uniform
  drive/color/character/trim surface, processing interleaved audio in place
- Shared parameter specs so all three plugins expose identical analog blocks
- 0 VU = −18 dBFS drive calibration
