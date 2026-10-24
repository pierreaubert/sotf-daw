# plugins-denoiser

Shared denoiser DSP building blocks for SOTF plugins. Extracted from `sotf-plugin-denoiser` so the dedicated declick, hiss-reducer, and speech-denoiser plugins can reuse the same primitives without duplicating logic.

## Modules

- `transient` — eight-sample-lookahead robust click detection and interpolation
  (used by `sotf-plugin-declick`).
- `hiss` — zero-latency persistent low-level high-band expander with warm
  bypass and smoothed cutoff automation (used by `sotf-plugin-hiss-reducer`).
- `rnnoise` — `RnnoiseBackend` wrapping `nnnoiseless` for RNNoise voice denoising (used by `sotf-plugin-speech-denoiser`).

## Testing

```bash
cargo check -p plugins-denoiser && cargo clippy -p plugins-denoiser
cargo test -p plugins-denoiser
```

## Important Notes

- Pure DSP: no `Plugin` trait impls live here — those are in the per-plugin crates that depend on this one.
- Pre-allocate all working buffers in builders; never allocate on the audio path.
- RNNoise's linked stereo detector must be permutation-invariant over complete
  streams, including negative correlation and exact equal-energy ties. Preserve
  the original channels when applying shared masks.
- Each block is independently testable — keep public APIs narrow and free of plugin-host concerns.

- Select the physical negative-correlation detector reference once per stream,
  using dominant energy and a canonical positive-peak initial tie; retain it
  across later energy crossings. Reset and model installation clear it. Keep
  positive-correlation mixing and original channel polarities unchanged.
- Preserve the frozen stateless quality control, independent direct-model clock
  reference, 36 recorded crossing/control cases, exact stream swaps and reset.
  Unknown training overlap and remaining correlation-sign transitions must not
  be presented as held-out/listening/native quality acceptance.
