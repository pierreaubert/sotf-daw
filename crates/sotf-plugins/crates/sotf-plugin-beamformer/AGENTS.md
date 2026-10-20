# sotf-plugin-beamformer

Beamformer plugin — MVDR, superdirective, and GSC beamformers.

## Architecture

- `lib.rs` — Main `BeamformerPlugin`, implements `Plugin` trait
- `mvdr.rs` — Minimum Variance Distortionless Response beamformer
- `superdirective.rs` — Superdirective beamformer
- `gsc.rs` — Generalized Sidelobe Canceller
- `steering.rs` — Steering vector computation
- `params.rs` — Parameter definitions

## Key Public API

- `BeamformerPlugin` implementing `Plugin`

## Testing

```bash
cargo test -p sotf-plugin-beamformer
```

## Important Notes

- Plugin trait — changes channel count (multi-mic input → mono/stereo output)
- Multiple beamforming algorithms: MVDR, superdirective, GSC
- Steering vectors define the look direction
- Coordinate convention: 0° broadside, ±90° endfire for the linear x-axis array.
- Microphone geometry, steering angle, and algorithm are structural. Rebuild;
  do not allocate or replace adaptive state in a live setter.

## Weight robustness audit

- MVDR and superdirective cap squared weight norm at one for physical steering, preserving look response by shrinking the orthogonal component. Low-norm weights stay exact.
- This is a 0 dB white-noise-gain floor, with a documented diffuse-directivity tradeoff; it is not a GSC guarantee.
- `tests/audit_array_robustness.rs` uses independent physical steering and an f64 diffuse reference. Moving-source/public diffuse-mixture coverage remains outstanding.
