# sotf-plugin-declick

Bounded-lookahead click detection and interpolation plugin, with a legacy
`TransientSuppressor` path plus the owned periodic/multiband repair engine.

## Architecture

- `lib.rs` — `DeclickPluginParams` + `ParametricInPlacePlugin` implementation.
- `repair.rs` — owned robust detection, periodic tracking, complementary bands,
  interpolation and retained boundary state.
- `params.rs` — `PARAMS` array (parameter specs) and registration via `param_specs::find_by_key`.

## Parameters

- `enabled` — bypass toggle.
- `sensitivity` — click-detection sensitivity threshold.
- `link_channels` — linked adjacent-pair versus independent detection.
- `mode`, `bands`, `crossover_hz`, `repair_width` — structural configuration.
- `frequency_skew` — per-band detection bias.
- `audition_residual` — aligned removed-signal audition.

## Features

- `qa` — enables `sotf-host/qa` and the `qa-declick` benchmark binary.

## Testing

```bash
cargo check -p sotf-plugin-declick && cargo clippy -p sotf-plugin-declick
cargo test -p sotf-plugin-declick
cargo run -p sotf-plugin-declick --features qa --bin qa-declick
```

## Important Notes

- Legacy latency is 8 samples; owned paths add configured repair width (0–8).
- Bypass remains latency matched and keeps detector history warm.
- Implements `ParametricInPlacePlugin` — same in/out channel count.
- Parameter registration must appear in `parameter_schema`/`current_values` and be applied in `apply_values`. Missing schema entries cause silent rejection.
- Random/fullband/zero-width preserves the shared legacy suppressor exactly.
  Other configurations use this crate's owned engine.
- `tests/audit_quality.rs` is an always-run clean-reference corpus. Distinguish
  aggressive repair damage from conservative missed clicks; report both.
  Audio fixture licenses/hashes are in `tests/fixtures/recorded`.
- External piano/rock quality tests are explicitly ignored by default and need
  `SOTF_TEST_DATA_ROOT` when selected; they are not silently counted as passes.
