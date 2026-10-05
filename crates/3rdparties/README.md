# crates/3rdparties

Vendored third-party forks owned by this workspace via the root
`[patch.crates-io]` section. `sotf` and `sotf-systemwide` patch to these
paths, so their builds use the same implementation as `sotf-daw`.

- `nnnoiseless` — RNNoise port with checked model loading, used by
  `plugins-denoiser`. Its
  `tests/testing.raw` and `tests/reference_output.raw` fixtures are
  embedded by `plugins-denoiser` tests via `include_bytes!`, so the
  directory must stay at this path.

`sotf-engine` uses the pinned external `coreaudio-rs` fork in the workspace
`[patch.crates-io]`. The removed local 0.13.1 copy and its licenses remain
available in Git history; the external 0.14.2 fork retains both license files.

The shared Rubato fork lives in `math-audio/crates/3rdparties/rubato`.
