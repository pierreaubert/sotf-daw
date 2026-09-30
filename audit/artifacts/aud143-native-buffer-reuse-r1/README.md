# AUD143 native buffer-reuse checkpoint

Recorded 2026-09-30. This checkpoint adds a workspace-owned VST3 callback
probe for the auxiliary-output buffer reuse correction and binds it to the
already-green packaged BandSplit route. It has not received a separate Astra
review. The earlier prefix archive remains historical; this package includes
the expanded native callback source and the vendored buffer source hash.

## Source scope

`logs/` contains the five gate logs and the two selected-source manifests from
the loaded BandSplit r6 run. `selected-source.sha256` records current hashes of
the touched callback/buffer files, the loaded route test, relevant external
host implementation files, and `Cargo.lock`. It is a selected-source manifest,
not a complete transitive build manifest. `sources/` preserves the exact
callback test, vendored buffer manager, and loaded BandSplit test sources for
this checkpoint.

The callback probe lives in
`crates/sotf-plugins/crates/plugins-nih/tests/native_aux_output.rs`; it does
not change the existing strict `OutputProbe`. A second VST3 plugin allows
inactive audio output buses and processes three callbacks through one wrapper:

| Callback | Auxiliary buffers | Frames | Expected `Buffer::samples()` |
|---|---|---:|---:|
| 1 | Both channel-pointer arrays absent | 17 | 0 for both ports |
| 2 | Mono and stereo buffers supplied while inactive | 33 | 33 for both ports |
| 3 | Both channel-pointer arrays absent again | 9 | 0 for both ports |

The plugin stores sample counts, channel counts, slice-length checks, and
zero-sample checks in preallocated atomics; test assertions run after native
callbacks. The host-side assertions also check that provided inactive buffers
are zero-filled only across the callback frame range and that omitted storage
retains its canary. This new reuse probe is not wrapped in `assert_no_alloc` so
a buffer invariant regression is not obscured by an allocator guard while the
wrapper prepares the callback. The existing CLAP/VST3 prefix and malformed
width tests remain unchanged and retain their allocation guards and canaries.

The buffer implementation represents a missing auxiliary output with zero
samples and empty per-channel slices while retaining the declared channel
slots. Present output buses continue to receive full callback-length slices
that are cleared before plugin processing. This prevents a stale pointer or
old frame count from surviving an absent→present→absent reuse sequence.
The vendored `BufferManager` also contains a direct unit test for this
transition, but that nested `nih_plug` test target was not run: standalone
offline resolution cannot fetch its uncached `baseview` git revision. The
workspace VST3 callback probe above exercises the same production manager
through a native wrapper callback, without changing public production
visibility.

## Executed gates

Commands used `flock -x /tmp/sotf-daw-audit-cargo.lock`,
`CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`,
`TMPDIR=/tmp`, `CARGO_NET_OFFLINE=true`, `--offline --locked`.

- `cargo test -p plugins-nih --features band-split --test native_aux_output -- --nocapture`: 3 passed, including the new reuse probe. Log SHA-256:
  `7c357773401874cfccc7c057d00a0bf0c4a313d1435c40da4804dab94fe29490`.
- `cargo test -p plugins-nih --features band-split --lib native_bandsplit_vst3_callbacks -- --nocapture`: 5 passed. Log SHA-256:
  `daec77cf3d855308490bb462912859fa3fba2386ca0ef0e2a76a8ff7ebdbe634`.
- `cargo test -p plugins-nih --features band-split --lib --tests -- --nocapture`: 111 unit tests passed, 3 native auxiliary-output tests passed, and 1 manual test was ignored (114 passed, 1 ignored). Log SHA-256:
  `6be92fda2743e16d6b559f2ee53cca306ffc4344714e47097898fbdff5416467`.
- `cargo clippy -p plugins-nih --features band-split --all-targets -- -D warnings`: passed. Log SHA-256:
  `f1e48d8d163d8a3dfd21b0929b98249b77c8de0b7b2d80951ea0f2beeaa815a7`.
- Previously executed loaded-host gate: `cargo test -p sotf-plugins --features external-plugin-clap,external-plugin-vst3 --test native_bandsplit_external_host -- --nocapture`: 5 passed. The byte-identical packaged CLAP and VST3 binaries each have SHA-256
  `6f1803cbad160f59f589569e600e10ffc7dc1a5cf5986b68e4eff123a5ef838b`.
  The complete-vector route compares loaded output against a separately
  configured public BandSplit DSP composition and checks full lengths, finite
  samples, distinct stereo pairs, state reload, and `DawHost` delivery. The
  test enforces a maximum sample residual of `2e-5`; it does not print the
  passing residual values. It covers 2/3/4 bands, LR24/LR48, both modes,
  chunked processing, valid saved-state reload, and a valid-envelope late
  candidate restore refusal followed by synchronized live/twin continuation,
  cold sensitivity control, and successful retry.
  Log SHA-256: `bcdeeb9189f518b9378161164c74dd157fed8069e832ea895f1c46c310ab2aee`.
  Start/end selected-source manifest files are preserved in `logs/` and match.

The loaded-host output is separate from DSP accuracy acceptance. It establishes
that the loaded CLAP/VST3 route carries the selected bands through SOTF's
external host and `DawHost`; its DSP reference is independently constructed,
while the independent coefficient/complex-response gates remain documented
in the main AUD143 report. It does not cover AU on Linux.

## Review boundary

This is a selected-source and focused-gate record. It does not claim a full
workspace pass, an AU route, or independent review of the buffer-reuse probe.
The archived short-prefix packet at
`audit/artifacts/aud143-native-prefix-root-r1/` predates the added reuse
probe; this checkpoint includes its fresh 3-test prefix/reuse gate and current
source hashes.
