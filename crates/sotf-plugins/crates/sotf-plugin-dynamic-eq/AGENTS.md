# sotf-plugin-dynamic-eq

Dynamic EQ — frequency-selective dynamics processing.

## Architecture

- `lib.rs` — Main `DynamicEqPlugin`, implements `ParametricInPlacePlugin` trait
- `params.rs` — Parameter definitions

## Key Public API

- `DynamicEqPlugin` implementing `ParametricInPlacePlugin`

## Testing

```bash
cargo test -p sotf-plugin-dynamic-eq
```

## Important Notes

- Combines parametric EQ with dynamics (each band activates only when signal crosses threshold)
- ParametricInPlacePlugin — same channel count in/out
- Each band has: frequency, Q, gain, threshold, ratio, attack, release, shape, placement
- Shapes: Peak, LowShelf, HighShelf, Tilt (pivot; full-band detector; Q/slope ignored)
- Placement: Stereo/Left/Right/Mid/Side within stereo_pairs (default [[0, 1]] on stereo)
- Filter/topology/routing controls are structural; only dynamics and mix automate live.
