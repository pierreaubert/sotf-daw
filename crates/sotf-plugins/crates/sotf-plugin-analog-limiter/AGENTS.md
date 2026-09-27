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
- Signal flow: limiter core first, analog color stage second, both in place
- Reported latency is the core's lookahead latency (color adds none)
- Carries an opinionated subset of core params; `isp_mode`,
  `dual_release`, `link_amount`, `feed_forward` stay at core defaults
- Default state limits at −0.1 dB with color at 0%
