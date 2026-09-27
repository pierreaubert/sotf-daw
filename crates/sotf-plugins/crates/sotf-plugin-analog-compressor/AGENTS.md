# sotf-plugin-analog-compressor

Analog compressor: single-band feed-forward compressor into a shared
`math-analog` coloration stage.

## Architecture

- `lib.rs` — module wiring and re-exports
- `params.rs` — parameter specs, UI layout, serializable params
- `analog_compressor.rs` — `AnalogCompressorPlugin`, implements `ParametricInPlacePlugin`
- Dynamics core is native (linked `EnvelopeFollower` + soft-knee gain
  computer + static/auto makeup + parallel mix), built on host primitives
- Analog color via `sotf-plugin-analog-common` (`AnalogColorStage`)

## Key Public API

- `AnalogCompressorPlugin` implementing `ParametricInPlacePlugin`
- `AnalogCompressorPlugin::from_params` / `try_from_params`
- `AnalogCompressorPluginParams` implementing `PluginParamDef` (`PLUGIN_TYPE_KEY = "analog_compressor"`)

## Testing

```bash
cargo test -p sotf-plugin-analog-compressor
```

## Important Notes

- ParametricInPlacePlugin — same channel count in/out
- Detection is linked: the hottest channel drives one shared envelope
  (stereo-bus behavior, no extra link knob)
- Signal flow: compressor core first, analog color stage second, both in place
- Zero latency; default state compresses gently (threshold −18 dB) with color at 0%
