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

- Uses LR4 crossovers for phase-coherent band splitting
- Re-exports `CROSSOVER_PRESETS` from sotf-host for standard frequency splits
- Each band has independent threshold, ratio, attack, release, knee, makeup gain

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
