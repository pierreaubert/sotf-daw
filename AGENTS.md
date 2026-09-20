# sotf-daw

DAW core workspace: audio engine, plugin family and host, MIDI integration,
IAMF support, and driver transport. `sotf` (apps, player, services) and
`sotf-systemwide` (daemon) both depend on this workspace.

## Sub-crates

- **`sotf-engine`** (`sotf_audio`) -- decode, process, playback, manager
  runtime. Optional `streaming`/`hls` features wire in `sotf-streaming`,
  which stays in `../sotf`; the integration is fully `cfg(feature)`-gated
  with no-op fallbacks.
- **`sotf-plugins`** -- facade over `sotf-host` (internal/external plugin
  host), ~40 `sotf-plugin-*` DSP crates, and the `plugins-bridge`,
  `plugins-denoiser`, `plugins-ffi`, `plugins-gpui`, `plugins-nih`,
  `plugins-spatial` layers.
- **`sotf-midi`** (`sotf_audio_player_midi`) -- MIDI device management.
- **`sotf-iamf`** (`sotf_iamf`) -- IAMF decoder (uses `sotf-host` +
  `sotf-plugin-ambisonics`).
- **`driver-common`** (`driver_common`) -- platform-agnostic `AudioDriver`
  trait and `NullDriver` fallback. No platform-specific code.
- **`driver-hal`** (`driver_hal`) -- macOS-only shared-memory bridge to the
  Swift CoreAudio HAL driver (ChaCha20-Poly1305, `/tmp/sotf-{uid}/audio.shm`).

## Dependency rules

This workspace must stay independent of `sotf` and `sotf-systemwide`:

- Normal (non-dev, non-optional) dependencies: daw crates only, plus
  external crates.io/git dependencies. No `../sotf` or `../sotf-systemwide`
  paths.
- Allowed exceptions, both resolving from the sibling `../sotf` checkout in
  the `all_of_sotf` layout: the optional `sotf-engine[streaming]` edge to
  `sotf-streaming`, and dev-dependencies on `sotf-testkit`/`sotf-test`.
- `[patch.crates-io]` mirrors the vendored forks (`nnnoiseless`,
  `coreaudio-rs`) and the Zed `wgpu` fork pins from `sotf`; patches are
  root-workspace configuration and must be repeated here, not inherited.
- `gpui-toolkit` git dependencies are pinned to a `rev` (not floating
  `branch = "main"`); bump deliberately in step with `sotf`'s lockfile.

## Testing

```bash
just check    # workspace check (excludes plugins-ffi, see Justfile)
just lint     # workspace clippy, warnings denied
just test     # workspace tests (excludes plugins-ffi)
just qa       # qa-plugins + qa-engine gates
cargo test -p driver-common --lib
cargo test -p driver-hal --lib
just --list   # full recipe list, incl. plugins-check, qa-engine, ...
```

`plugins-ffi`'s build script shells out to a nested `cargo metadata`, which
needs registry write access unavailable in some sandboxes; check it on a
normal host when touching FFI.
