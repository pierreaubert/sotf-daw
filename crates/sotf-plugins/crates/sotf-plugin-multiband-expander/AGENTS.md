# sotf-plugin-multiband-expander

Broadband (1 band) and multiband (2-5 bands) dynamic range expansion.

## Architecture

- `lib.rs` — Main plugin struct, implements `ParametricInPlacePlugin` trait
- `params.rs` — Parameter definitions and JSON deserialization


## Key Public API

- Main plugin struct implementing `sotf_host::plugin::ParametricInPlacePlugin`
- Plugin parameters via `params.rs`

## Testing

```bash
cargo test -p sotf-plugin-multiband-expander
```

## Important Notes

- Uses cascaded LR4 crossovers with f64 coefficients, state and split
  intermediates, casting final bands to f32. Preserve this precision at low
  cutoffs/high rates. Three-plus-band sums are not generally flat all-pass
  responses; validate against the cascade, not a flat response assumption.
- Re-exports `CROSSOVER_PRESETS` from sotf-host for standard frequency splits
- Each band has independent threshold, ratio, attack, release parameters
- `expander` is a true one-band factory path; `multiband_expander` requires 2-5 bands.
- Canonical and bridge `expander` force `time_domain`; multiband routes retain
  mode. Bridge construction must use `try_from_params`, not the legacy
  infallible constructor. Direct Rust facade aliases retain multiband defaults.
- `num_bands` and `processing_mode` are structural and must never allocate in a live setter.
- Spectral mode is dual-Hann, 75% overlap, `N/4` hop, and `N` samples latency.
- Spectral detection is linked: FFT all channels, collect per-bin maxima into
  preallocated `linked_bin_norms`, then apply shared detection to every channel.
  Do not synthesize one channel before collecting the others' spectra.
- Normalize constructor detector/mode strings to lowercase; validators accept
  case-insensitive labels, so processing comparisons must use the same form.
- Sidechain HPF affects detector samples only; lookahead applies uniformly to bypassed bands.
- `tests/audit_reconstruction.rs` uses independent f64 Tustin/DF-I references
  for time mode and a pure delayed-input reference for spectral unity. Keep
  fixed bounds and callback heap guards; release heap checks require QA.
- `tests/audit_dynamics.rs` uses f64 detector/state and full-complex FFT/linear
  OLA references for nonlinear quiet startup, chatter and per-band thresholds.
