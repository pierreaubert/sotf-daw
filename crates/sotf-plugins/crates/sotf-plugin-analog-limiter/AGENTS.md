# sotf-plugin-analog-limiter

Analog limiter: mastering limiter core into a shared `math-analog`
coloration stage.

## Architecture

- `lib.rs` — module wiring and re-exports
- `params.rs` — parameter specs, UI layout, serializable params
- `analog_limiter.rs` — `AnalogLimiterPlugin`, implements `ParametricInPlacePlugin`
- Limiter core via composition of `sotf-plugin-limiter` (not a fork)
- Analog color via `sotf-plugin-analog-common` (`AnalogColorStage`)

## Key Public API

- `AnalogLimiterPlugin` implementing `ParametricInPlacePlugin`
- `AnalogLimiterPlugin::from_params` / `try_from_params`
- `AnalogLimiterPluginParams` implementing `PluginParamDef` (`PLUGIN_TYPE_KEY = "analog_limiter"`)

## Testing

```bash
cargo test -p sotf-plugin-analog-limiter
```

## Important Notes

- ParametricInPlacePlugin — same channel count in/out
- Signal flow: limiter core first, analog color stage second, final zero-latency ceiling clamp third, all in place
- Reported latency is the core's lookahead latency (color adds none)
- Carries an opinionated subset of core params; `isp_mode`,
  `dual_release`, `link_amount`, `feed_forward` stay at core defaults
- Default state limits at −0.1 dB with color at 0%
- `threshold` is the final emitted sample ceiling at fully wet mix (enforced after color/trim); `true_peak` is input detection only with no strict output true-peak guarantee
- Guard tracks threshold/mix targets immediately (core smooths one-pole 5 ms); color-off output is bit-identical below the ceiling, conversion-epsilon (2e-6) when hot; linking is core-detector-only, color is per-channel
