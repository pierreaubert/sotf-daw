# sotf-plugin-denoiser

Audio denoising using MCRA noise estimation and Wiener filtering.

## Architecture

- `lib/denoiser_plugin.rs` — plugin state, streaming STFT, parameter and host contracts
- `fft.rs`, `wiener/`, `mcra.rs` — transforms, gain calculation, and noise estimation
- `multi_resolution.rs` — optional aligned 512-point analysis arm
- `noise_profile.rs` — one-second captured-profile state machine
- `masking.rs`, `polyphonic.rs`, `spectral_sub.rs` — optional analysis/reduction modes
- `params/` and `config.rs` — schema, UI layout, serialization, and validation


## Key Public API

- Main plugin struct implementing `sotf_host::plugin::ParametricInPlacePlugin`
- Plugin parameters via `params.rs`

## Testing

```bash
cargo test -p sotf-plugin-denoiser
```

## Important Notes

- MCRA (Minima Controlled Recursive Averaging) for noise floor estimation
- Keep fast-adaptation history per channel. Aggregate UI learning telemetry
  must not influence another channel's estimator or depend on traversal order.
- Prepare the bootstrap hop count from the clock; preserve the 106.6667 ms
  reference observation span with less than one hop of rounding. This is
  separate from signal latency and from frame-based MCRA control settings.
- Wiener filter for noise reduction
- Operates in STFT domain
- FFT topology controls are setup-only; rebuild the plugin to change them.
- The process path must remain allocation-free. Run the QA matrix when changing optional modes.

- Captured profiles own the prepared sample clock and both active FFT grids.
  Accumulate small-FFT periodograms directly into preallocated per-channel
  storage. Do not guess PSD scaling between windows or substitute live small
  noise estimates while a fixed profile is selected.
- Finalize both grids at the main source boundary, including recomputing the
  latest small gains before the large frame blends them. Keep the live estimator
  running independently; a published profile overrides its bootstrap bypass.
- Reset/cancel retain published profiles and discard incomplete capture; clear
  or a real clock change invalidate both grids. Same-clock init retains them;
  rejected init is transactional. EOF padding measures no profile power.
- Refresh primitive cached parameter values on internal profile transitions,
  without rebuilding schema storage. Prepare ParameterId handles before heap
  measurement; callback-time Arc clones are allowed, owned ID construction is not.
- Keep independent f64 periodogram/gain, per-channel capture, profile publication,
  source/EOF partition, lifecycle and allocation/free checks. These are fixed-
  profile correctness checks, not blind speech or held-out quality acceptance.
