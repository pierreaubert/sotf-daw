# sotf-plugin-analog-common

Shared analog coloration stage for the `sotf-plugin-analog-*` family.
Not a factory plugin — a library crate.

## Architecture

- `lib.rs` — everything: `MODEL_NAMES`, shared `ParamSpec` constructors,
  `AnalogColorStage`, unit tests

## Key Public API

- `AnalogColorStage`: uniform drive/color/character/trim surface over one
  `math-analog` model, with chunked in-place interleaved processing
- `MODEL_NAMES`: display names in stable model-id order
- `model/drive/color/character/output_trim_param_spec`: shared `const fn`
  spec constructors for the family PARAMS arrays
- `REFERENCE_LEVEL_DBFS`: 0 VU calibration (−18 dBFS)

## Testing

```bash
cargo test -p sotf-plugin-analog-common
```

## Important Notes

- Console-preamp model maps drive→`input_gain_db` and
  character→`asymmetry` (same ranges); all other models map directly
- Defect controls stay at zero: adding the stage never adds noise/hum
- `set_color(0)` is bit-transparent on every model (pinned by test)
- Blocks larger than the prepared maximum are chunked; no RT allocation
