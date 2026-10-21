# sotf-plugin-hal-input

macOS HAL input plugin — reads audio from CoreAudio HAL driver.

## Architecture

- `lib.rs` — Main plugin struct, implements `Plugin` trait
- `params.rs` — Parameter definitions and JSON deserialization


## Key Public API

- Main plugin struct implementing `sotf_host::plugin::Plugin`
- Plugin parameters via `params.rs`

## Testing

```bash
cargo test -p sotf-plugin-hal-input
```

## Important Notes

- macOS only — requires CoreAudio HAL driver (sotf-macos-hal)
- Plugin trait (not ParametricInPlacePlugin) — generates output without input

- Reader observations before and after each nonempty read gate publication of
  the entire callback. Do not claim these are an atomic native epoch fence.
- Successful silence counts missing frames; error returns do not. Startup and
  transport recovery remain distinct from an armed-stream underrun.
- Empty callbacks observe readiness without consuming transport frames.
- Use `--features sotf-host/qa` for the allocation assertions. Native `hal`
  compile checks do not establish runtime transport correctness; constructors
  can access live shared memory, so use an isolated fixture for native tests.
