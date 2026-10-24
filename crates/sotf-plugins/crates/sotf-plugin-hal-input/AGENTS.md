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

- `native_transport` is an ignored, explicit isolated target. Require a fresh
  owned SOTF_HAL_AUDIT_FIXTURE_ROOT; spawn child overrides for shared mapping,
  key and runtime paths before calling production constructors. Never run the
  full native test suite as a substitute for this isolated target.
- The native evidence uses a dirty driver dependency snapshot. Keep its hashes
  and do not claim clean-driver, Swift callback or hardware timing validation.
- Shared driver_ready is not process liveness. Abrupt writer death can leave
  connected=true with counted silence; automatic supervisor recovery is open.

- `service_transport` is an explicit non-RT liveness check: failed broker/key/
  format preparation latches callbacks disconnected without consuming the old
  ready mapping. Reset and initialize cannot bypass it; successful validated
  replacement clears it. Keep transactional `refresh_transport` refusal behavior
  separate. Automatic service scheduling and physical source-clock checks remain
  open; never derive process death from silence alone.

- Healthy service probes must retain the current reader's decrypted suffix and
  arming/counter state. The driver mapping identity check is control-only and
  performs filesystem metadata work; never call it from reader_observation or
  process. A new mapping or faulted reader still requires validated replacement.
