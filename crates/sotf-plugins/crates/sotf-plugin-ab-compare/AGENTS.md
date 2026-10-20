# sotf-plugin-ab-compare

A/B comparison plugin for blind testing.

## Architecture

- `lib.rs` — Main plugin struct, implements `Plugin` trait
- `params.rs` — Parameter definitions and JSON deserialization
- `config.rs` — Comparison configuration

## Key Public API

- Main plugin struct implementing `sotf_host::plugin::Plugin`
- Plugin parameters via `params.rs`

## Testing

```bash
cargo test -p sotf-plugin-ab-compare
```

## Important Notes

- Implements Plugin trait (not ParametricInPlacePlugin) — may route audio differently based on A/B state
- Used for blind comparison testing between two signal paths
- Built-in `compressor` always selects one band, matching canonical/bridge
  identity. `multiband_compressor` honors serialized topology. External
  factories are authoritative; do not normalize their parameters here.
- `plugins-bridge/tests/compressor_factories.rs` checks fallback plugin/rack/graph
  audio, metadata and rejection against bridge/native construction.
