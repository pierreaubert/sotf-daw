# sotf-plugin-multiband-compressor

Multiband dynamic range compression (2-5 bands).

## Architecture

- `lib.rs` — Main plugin struct, implements `ParametricInPlacePlugin` trait
- `params.rs` — Parameter definitions and JSON deserialization


## Key Public API

- Main plugin struct implementing `sotf_host::plugin::ParametricInPlacePlugin`
- Plugin parameters via `params.rs`

## Testing

```bash
cargo test -p sotf-plugin-multiband-compressor
```

## Important Notes

- Uses cascaded LR4 crossovers with f64 coefficients, state and split
  intermediates; cast to f32 only at band buffers. Preserve precision at low
  cutoffs/high sample rates. Three-plus-band sums are not generally flat
  all-pass responses: later crossover phase is absent from earlier low bands.
- Re-exports `CROSSOVER_PRESETS` from sotf-host for standard frequency splits
- Each band has independent threshold, ratio, attack, release, knee, makeup gain
- `tests/audit_signal_paths.rs` validates against independent DF-I/Tustin and
  dynamics references with fixed bounds; do not substitute production filter
  helpers into the oracle. Release heap checks require `--features qa`.

## Explicit recursive render policy

- Native render jobs may call `set_recursive_tail_render_frames(Some(N))` before
  EOS. N counts output frames after the final input, including lookahead. It is
  deliberate truncation, not finite mathematical support or silence detection.
- Default None preserves legacy recursive zero-frame COMPLETE behavior. Exact
  one-band/settled-dry support overrides the recursive-only policy.
- Keep the chosen extent frozen in `drain_total_frames` until reset, even if a
  wet-to-dry smoother settles during drain. Capacity metadata must stay valid.
- Reset/reinitialize preserve configured frame count without rescaling; policy
  changes during EOS require reset. The setting is not a serialized parameter.
- `tests/recursive_render.rs` checks zero continuation, cutoff, finite precedence,
  partial capacity/refusal, reset, wet-to-dry freeze and native adapter forwarding.
