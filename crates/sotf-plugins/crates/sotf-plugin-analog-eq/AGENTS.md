# sotf-plugin-analog-eq

Analog EQ: fixed 4-band parametric EQ (low-shelf, 2 peaks, high-shelf) into a
shared `math-analog` coloration stage.

## Architecture

- `lib.rs` — module wiring and re-exports
- `params.rs` — parameter specs, UI layout, serializable `AnalogEqParams`
- `analog_eq.rs` — `AnalogEqPlugin`, implements `ParametricInPlacePlugin`
- Analog color via `sotf-plugin-analog-common` (`AnalogColorStage`)

## Key Public API

- `AnalogEqPlugin` implementing `ParametricInPlacePlugin`
- `AnalogEqPlugin::from_params` / `try_from_params`
- `AnalogEqParams` implementing `PluginParamDef` (`PLUGIN_TYPE_KEY = "analog_eq"`)

## Testing

```bash
cargo test -p sotf-plugin-analog-eq
```

## Important Notes

- ParametricInPlacePlugin — same channel count in/out
- Signal flow: biquad core first, analog color stage second, both in place
- Coefficient updates are click-free (`Biquad::update_params` keeps delay state)
- Zero added latency (biquads + color stage report 0)
- Default state is a flat EQ with color at 0% (transparent)
- Model selection is fail-closed: unknown names are rejected, never guessed
