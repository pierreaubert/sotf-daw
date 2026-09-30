# AUD144 r3: directional FletcherMunson preset migration

Status: implementation and local verification complete; Astra review remains
pending.

## Compatibility rule

Preset identity matching now receives the import target and the saved preset
type explicitly. Factory aliases still match in either direction. The
backward-compatibility exception accepts a saved `FletcherMunson` preset when
the import target is `LoudnessCompensation`. The reverse direction remains
refused by the selected migration contract. The earlier r1/r2 evidence did not
exercise that reverse direction, so its refusal is a policy choice rather than
a previously measured behavior.

The alias unit regression also keeps `Compressor` and
`MultibandCompressor` separate. The public C-API regression exports a genuine
FletcherMunson preset after setting non-default gains through the normalized
setter, imports it into both plugin spellings, verifies equal saved state and
exact continuation audio, checks the settings affect output, and checks the
selected one-way boundary.

## Verification

All commands used `TMPDIR=/tmp`, the shared Cargo target, offline locked Cargo,
and the shared Cargo flock. Exit codes are recorded in `execution-receipt.json`.

- Focused AUD144 tests: 3 passed.
- Full `plugins-ffi` library tests: 82 passed, 1 ignored.
- Strict all-target `plugins-ffi` Clippy: passed with warnings denied.
- `plugins-ffi` library build: passed. The selected 340-path source manifest
  is byte-identical before and after the first build. After a test-only
  rustfmt adjustment, the library was rebuilt and remained byte-identical to
  the immutable r3 copy. Its broader 340-path manifest changed only for
  Band's concurrent `native_vst3_host.rs` integration-test edit, which is not
  compiled by `plugins-ffi --lib`. The other 339 selected hashes match exactly;
  the filtered 339-path start/end manifests are byte-identical.
- The unchanged preset-envelope ctypes probe passed: all nine invalid envelope
  variants return `-8`, preserve serialized state and populated continuation,
  while both valid controls are accepted. Valid imports intentionally replace
  prepared processing history; the probe compares them with fresh prepared
  output.
- The unchanged FletcherMunson migration ctypes probe passed: import return
  code 0, equal saved state, bit-exact 9,464-sample output, and default
  sensitivity `0.2791688293`.

Root independently checked the library and probe identities, source manifest,
all saved waveform hashes and lengths, refusal cases, valid controls, and
migration output in `root-probe-verification.json`.

## Preserved evidence and limits

The three changed FFI Rust files are copied byte-for-byte under both the
initial `sources/` checkpoint and the final post-rustfmt `sources-r2/`
checkpoint. The final current `plugin_factory.rs` hash is recorded in the
receipt. The first and post-rustfmt test/lint/build logs and manifests are
retained separately; the broader post-rustfmt manifest mismatch is preserved
rather than described as a clean 340/340 interval.
The immutable r3 cdylib is
`crates/sotf-plugins/target/audit-artifacts/aud144-preset-envelope-r3/libplugins_ffi.so`.
The probes remain at `audit/tools/`; their unchanged hashes are in the receipt.
Audio vectors and result JSON files are preserved beside this report.

The manifest covers 340 selected workspace sources and manifests, not every
transitive dependency source byte. The cdylib build did not change the C header.
These checks establish the preset import contract through the C ABI; they do
not claim AU runtime or callback allocation behavior.
