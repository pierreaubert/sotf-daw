# sotf-plugin-aec

Acoustic Echo Cancellation — PBFDAF with two-path and post-filter.

## Architecture

- `lib.rs` — Main `AecPlugin`, implements `Plugin` trait
- `params.rs` — Parameter definitions and JSON deserialization

## Key Public API

- `AecPlugin` implementing `sotf_host::plugin::Plugin`

## Testing

```bash
cargo test -p sotf-plugin-aec
```

## Important Notes

- PBFDAF (Partitioned Block Frequency Domain Adaptive Filter) algorithm
- Two-path architecture for robust convergence
- Post-filter for residual echo suppression
- Plugin trait (not ParametricInPlacePlugin) — requires both microphone and reference signals

## Production audit validation

- `tests/audit_echo_paths.rs` uses independent time-domain echo fixtures and latency-aligned metrics; do not count near-end energy as ERLE.
- Background adaptation remains active; foreground promotion requires coherent, sustained improvement (133 ms initial, 25 ms thereafter).
- Drift coverage is bounded tracking, with a 3 dB floor at ±100 ppm, not clock synchronization.
- Release tests require `--features qa` for the existing callback allocation instrumentation.
