# Unchanged upstream DFN2 mono reference

`input.f32le`, `waveform.f32le`, and `scores.f32le` are little-endian float32.
There are 64 mono hops of 480 frames at 48 kHz. Inputs are synthetic silence,
harmonic/noise mixtures, a noisy tone, and impulses. These are regression stimuli,
not training examples or held-out enhancement-quality evidence.

The reference runner compiled the byte-unchanged `lib.rs`, `tract.rs`, and
`transforms.rs` from upstream v0.3.1 revision
`af5d24a125cc20759f483d4e354fdf70f7951024`, with Tract pinned to the revision in
that release's lockfile. `provenance.json` identifies all source, model, input,
output, generator, manifest and dependency-lock hashes, plus compiler/platform.
The wrapper uses the upstream default mono runtime policy and allocator. The
minimal inference manifest omits training/dataset dependencies; it does not edit
upstream Rust sources. No reference-source changes are compiled into production.

Replay with `scripts/ml/deepfilter_reference.py`, using an empty work directory
outside the repository and an external Cargo target cache. `--source-dir` can
supply the original release source directory after mandatory hash verification;
omitting it fetches those exact files. Builds and cached replays use normal
Cargo resolution. The original build and cached replay logs are retained
as historical evidence.

The native `mono_port_matches_unchanged_upstream_v031_golden_audio` test checks
fixture hashes and compares the local adapter. Local maximum waveform and score
errors are zero. This proves the exercised mono port trajectory, not every input,
model quality, stereo quality, or cross-platform parity.
