# Limiter oversampling: bridge, FFI, and native activation verification

## Result

No propagation defect found. Six new focused tests pass, and all three strict
Clippy commands pass. Production restoration logic was left unchanged.

Added files:

- `crates/sotf-plugins/crates/plugins-bridge/tests/limiter_oversampling.rs`
- `crates/sotf-plugins/crates/plugins-ffi/src/lib/limiter_oversampling_tests.rs`
- `crates/sotf-plugins/crates/plugins-nih/src/limiter_oversampling_tests.rs`

The only shared-file edits are `cfg(test)` module declarations in FFI and NIH
`src/lib.rs`. No engine, DSP production, manifest, factory, parameter mapping,
or wrapper lifecycle behavior changed.

## Source mapping confirmed

- The bridge's Limiter branch deserializes `LimiterPluginParams` and constructs
  the real adapted limiter. Choice index 10 is integer-valued, structural, and
  defaults to zero; all ten prior external IDs retain their order.
- `prepare_standalone_plugin` wraps only a requested host oversampling
  preference. The new limiter returns no preference because it owns its
  oversampling and final protection, so native preparation must not add a
  second stage.
- FFI restoration constructs a separate instance, reapplies the current state
  and incoming overlay before preparation/initialization, then commits only
  after success. The existing generic logic carries the new integer correctly.
- NIH constructor configuration forwards generic structural integers before
  initialization. Its existing structural fingerprint prevents live changes
  from silently reaching the old DSP; initialization reconstructs the chosen
  factor and announces actual latency through `InitContext`.

## Independent public evidence

**Bridge: 2 tests, 27 rate/channel/factor configurations.**

- Index 10 maps normalized 0/0.5/1 to integer 0/1/2. Same-value active writes
  succeed; changed active values fail without changing persisted parameters.
- Old constructor JSON with the field absent equals explicit 1x output exactly.
- Raw factory, standalone-prepared factory, and save/reconstruct/initialize
  paths produce identical complete low-level marker waveforms.
- 44.1/48/96 kHz and mono/stereo/6-channel fixtures independently
  assert first and frame-2047 impulse peaks at floor(Fs*2 ms), with exactly one
  additional 512-frame delay for 2x/4x. Thus two-ms delay is 88/96/192 frames at
  1x and 600/608/704 at the higher factors. Higher factors must differ from 1x
  audio; metadata alone cannot satisfy this check.

**C FFI: 2 tests, 18 valid restoration configurations and 24 invalid cases.**

- Actual `plugin_create`, `plugin_save_state`/`plugin_load_state`,
  `plugin_export_preset_json`/`plugin_import_preset_json`, `plugin_process`,
  normalized getter/setter, and JSON latency entry points are exercised.
- Both state formats retain the integer factor across reconstruction at three
  rates, match a fresh configured handle's full output, preserve old-JSON 1x,
  and produce the independently expected physical marker delay.
- Partial empty state preserves the live factor and its latency.
- Live changed-factor setters and invalid preset values 3, −1, 0.5, and string
  `2x` are rejected. Invalid documents also propose a different lookahead first.
  Persisted state, latency, and the full next waveform match the untouched warm
  reference, proving that original delay/envelope history survives rejection.

**NIH: 2 tests, 9 activation configurations plus 3 active-change lifecycles.**

- The actual `sotf_nih_plugin!` expansion runs with restored `DynamicParams`,
  real NIH buffers, an initialization latency context, and its production
  `process_with_transport` body. Integer choice, no additional host preference,
  inner metadata, announced latency, and first/late emitted marker peaks agree.
- Old/default choice matches explicit 1x output. Reset and reactivation repeat
  the complete waveform exactly while retaining the chosen factor.
- Changing the persisted structural choice while active returns an error and
  zeros the output. Restoring the accepted choice resumes the untouched DSP
  history. Reactivation then adopts the new choice and matches a fresh wrapper.

## Verification

```text
cargo test -p plugins-bridge --test limiter_oversampling
cargo test -p plugins-ffi --lib limiter_ffi_
cargo test -p plugins-nih --no-default-features --features limiter --lib limiter_nih_
cargo clippy -p plugins-bridge --test limiter_oversampling -- -D warnings
cargo clippy -p plugins-ffi --tests -- -D warnings
cargo clippy -p plugins-nih --no-default-features --features limiter --tests -- -D warnings
```

Each test command: **2 passed, 0 failed, 0 ignored**. Strict Clippy, focused
rustfmt, and scoped `git diff --check` passed. The existing vendored NIH
`unused import: unsafe_clap_call` dependency warning remains visible; it is not
introduced by these test-only changes and does not fail these package gates.

Logs are `/tmp/sotf-limiter-{bridge,ffi,nih}-propagation.log` and the corresponding
`-propagation-clippy.log` files. Commands used the shared target and the approved
spacious compiler TMPDIR.

## Limits

NIH tests use actual generated wrapper activation and processing with restored
parameter objects; they do not exercise CLAP/VST3 binary preset serialization or
a commercial DAW. FFI tests execute the portable C entry points on Linux, not
macOS AudioUnit hardware. No new heap-safety claim is made here; independent DSP
allocation/deallocation, automation, EOS, and telemetry gates belong to the
limiter implementation owner. Existing hidden structural-control visibility and
host reactivation requirements are unchanged.
