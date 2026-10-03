# crates/3rdparties

Vendored third-party forks owned by this workspace via the root
`[patch.crates-io]` section. `sotf` and `sotf-systemwide` patch to these
paths, so their builds use the same implementation as `sotf-daw`.

- `nnnoiseless` — RNNoise port with checked model loading, used by
  `plugins-denoiser`. Its
  `tests/testing.raw` and `tests/reference_output.raw` fixtures are
  embedded by `plugins-denoiser` tests via `include_bytes!`, so the
  directory must stay at this path.
- `coreaudio-rs` — CoreAudio bindings used by `sotf-engine` on macOS.

The shared Rubato fork lives in `math-audio/crates/3rdparties/rubato`.
