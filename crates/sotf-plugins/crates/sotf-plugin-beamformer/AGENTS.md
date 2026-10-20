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
- `tests/audit_array_robustness.rs` uses independent physical steering and an f64 diffuse reference. Public moving-source/diffuse-mixture coverage is in `tests/audit_moving_sources.rs` at 16/48/96 kHz; report target distortion separately from residual-noise error.

## GSC target protection

- Prepare 10 ms spatial-power and 5 ms cancellation-fade timing from the actual clock. The legacy core constructor retains its 48 kHz default; plugin construction/reinitialization uses `new_with_sample_rate`.
- Freeze weights, power detectors and cancellation mix during drain. Ordinary zero input may evolve detectors without extending finite audio support.
- Bound the complete microphone FIR energy using the blocking Gram identity; scalar limits preserve fixed summation and avoid realtime allocation.
- Reject invalid evidence before updating recursive detector state, so finite input resumes without reset.
