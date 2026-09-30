# AUD144 preset-envelope correction, r2

Recorded 2026-09-30. The r1 public red packet remains unchanged.

## Change

`plugin_import_preset_json` now requires an integer `schema_version` of 1, the
exact SOTF preset `ut_type`, and a `plugin_type` that identifies the running
plugin family. These checks happen before the state byte array is read or any
replacement can begin. Family matching covers the explicit aliases accepted by
the bridge factory and the existing direct-format LinearPhaseEQ normalization.
It preserves separate identities for families that share implementation code,
including Compressor/MultibandCompressor and FletcherMunson/
LoudnessCompensation. Unsupported spellings do not become aliases. Preset names
remain editable.

The separate raw `plugin_load_state` path is unchanged. Existing version-1
DynamicEQ preset fixtures remain complete documents. State-merge and limiter
fixtures now use valid envelopes, so those tests continue to reach their
intended state-validation paths. The oversized-input test still verifies that
the document-length guard returns before reading the supplied buffer.

## Verification

All Cargo commands ran offline and locked under `/tmp/sotf-daw-audit-cargo.lock`,
with `TMPDIR=/tmp` and the target directory
`crates/sotf-plugins/target`.

| Gate | Result | Log |
| --- | --- | --- |
| `cargo test --offline --locked -p plugins-ffi --lib aud144_ -- --nocapture` | 2 passed | `focused-r1.log` |
| `cargo test --offline --locked -p plugins-ffi --lib` | 81 passed, 1 ignored | `full-r1.log` |
| `cargo clippy --offline --locked -p plugins-ffi --all-targets -- -D warnings` | passed | `clippy-r1.log` |
| `cargo build --offline --locked -p plugins-ffi --lib` | passed | `build-r1.log` |

Formatting passed with:

```sh
rustfmt --check --edition 2024 \
  crates/sotf-plugins/crates/plugins-ffi/src/plugin_factory.rs \
  crates/sotf-plugins/crates/plugins-ffi/src/lib/plugin.rs \
  crates/sotf-plugins/crates/plugins-ffi/src/lib.rs \
  crates/sotf-plugins/crates/plugins-ffi/src/lib/tests.rs \
  crates/sotf-plugins/crates/plugins-ffi/src/lib/state_tests.rs \
  crates/sotf-plugins/crates/plugins-ffi/src/lib/limiter_oversampling_tests.rs \
  crates/sotf-plugins/crates/plugins-ffi/src/lib/aud144_preset_envelope_tests.rs
```

The build source receipt compares the 340 selected project source and manifest
paths listed in `build-source-start.sha256` and `build-source-end.sha256`;
they match exactly. This is not a snapshot of every transitive dependency.
Header synchronization was skipped.

The immutable r2 library is
`crates/sotf-plugins/target/audit-artifacts/aud144-preset-envelope-r2/libplugins_ffi.so`,
SHA-256 `17ada1bd75f5174e8510d75131ba2ca880538302619eda9c633bc16710caa4f2`.
The immutable r1 library still hashes to
`8954f6abe82bd6685b8f4d3142b06dbb2e1f89f7cdb615b3692497d40587c529`.

Root ran the unchanged public ctypes probe
`audit/tools/aud144_preset_envelope_probe.py` (SHA-256
`107d5b64fb106c3d593749d2c14a9688288da727ecf495d6e2a5ad020cec39ac`)
against the r2 library. Its green output is in `root-green/`; the result file
SHA-256 is
`4e0ca87fc3b41abbbc91cdb1b43bf0ac7cebb8ecfd3ccb36bbf0a00378c6e830`.
All nine preserved invalid-envelope variants return `-8`. Each keeps the full
saved state unchanged and produces a bit-exact 1,272-sample continuation against
the untouched populated twin (maximum residual 0). The cold instance remains
distinct, with maximum sensitivity `0.029786840081214905`. The unchanged and
editable-name controls import successfully. The Rust tests also cover zero and
floating-point schema versions, an unsupported `dynamic-eq` spelling, and
same-family `DynamicEQ`/`dynamic_eq` handles.

The probe evidence covers control-thread C ABI state and audio continuation.
It does not claim callback allocation behavior or AU runtime coverage. The
prior r1 red evidence is the public C-ABI reproduction; no separate Rust red
test run was captured before the validator was written.
