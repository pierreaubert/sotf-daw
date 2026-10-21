# sotf-plugin-hal-output

macOS HAL output plugin — writes audio to CoreAudio HAL driver.

## Architecture

- `lib.rs` — Main plugin struct, implements `Plugin` trait
- `params.rs` — Parameter definitions and JSON deserialization


## Key Public API

- Main plugin struct implementing `sotf_host::plugin::Plugin`
- Plugin parameters via `params.rs`

## Testing

```bash
cargo test -p sotf-plugin-hal-output
```

## Important Notes

- macOS only — requires CoreAudio HAL driver (sotf-macos-hal)
- Plugin trait (not ParametricInPlacePlugin) — consumes input without producing output

- Process and drain share one bounded complete-frame flush. Preserve the direct
  path for frame-aligned slices; use prepared staging when a frame crosses the
  VecDeque wrap. Never let a short physical front fragment starve logical audio.
- Count each validated queued-write prefix before attempting the next batch.
  Failed current process input stays caller-owned and does not increment the
  request counter. Success counts all owned input, including explicit overflow
  drops. Keep requested = written + pending + dropped with saturating totals.
- Writer-contract errors enter FormatError and gate further writes until control
  recovery. Never remove or count a batch on an invalid report. Control recovery
  quiesces transport before retry; physical delivery of that batch remains unknown.
- Control recovery preserves pending FIFO data; reset/successful initialize
  explicitly cancel it. Priming silence is outside input ownership counters.
- Keep frame-order/conservation, split-wrap layouts, partial-error prefixes,
  recovery and allocation/free tests. Do not infer native device safety from a
  fake transport; native-feature checks compile without opening the live device.
- ConfigurationChanged is a local latch, like FormatError. Clearing the shared
  notification from another consumer does not validate this output's format or
  authorize writes. Require explicit control-side format validation/recovery.

- Explicit service, prepared recovery and reprepare must re-open the current
  mapping even when a retired header still says driver_ready. Native reconnect
  retires old engine readiness and gates the new mapping before prime/validation.
- `native_transport` is an ignored isolated target; require a fresh owned fixture
  root and child mapping/key/runtime overrides. Never substitute the full native
  suite, whose other constructors may open live default transport.
- Preserve dirty dependency provenance and distinguish mapped-ring checks from
  Swift/device/deadline certification. A raw rate setter alone does not publish
  config_changed. Callback probes now gate settled unnotified geometry changes;
  concurrent changes during a write still require an atomic epoch fence.

- Private callback_format probes must be allocation-free and separate from
  control metadata queries. Gate process, pending drain and sink preflight on
  mismatched/unavailable prepared geometry; preserve ownership and sticky state.
  Never describe separately observed atomic fields as a coherent epoch snapshot.
