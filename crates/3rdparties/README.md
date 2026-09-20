# crates/3rdparties

Vendored third-party forks used by this workspace via the root
`[patch.crates-io]` section. Mirrored from `sotf/crates/3rdparties` so
`sotf-daw` builds standalone, without depending on the sibling `sotf`
checkout (outside the allowed `streaming` / `testkit` edges).

- `nnnoiseless` — RNNoise port used by `plugins-denoiser`. Its
  `tests/testing.raw` and `tests/reference_output.raw` fixtures are
  embedded by `plugins-denoiser` tests via `include_bytes!`, so the
  directory must stay at this path.
- `coreaudio-rs` — CoreAudio bindings used by `sotf-engine` on macOS.

When pulling upstream fixes, apply them in both mirrors (or re-mirror)
and keep the two copies in sync.
