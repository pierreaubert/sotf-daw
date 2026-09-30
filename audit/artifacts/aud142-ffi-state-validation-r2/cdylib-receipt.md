# AUD142 FFI cdylib r2

Build command: `cargo build --offline --locked -p plugins-ffi --lib`

Execution used `TMPDIR=/tmp`, `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`, and `SOTF_FFI_SKIP_HEADER_SYNC=1` to avoid generated-header writes. Cargo was serialized with `/tmp/sotf-daw-audit-cargo.lock`.

Build exited 0. The selected source set contains 342 files; `build-source-start.sha256` and `build-source-end.sha256` are byte-identical. The selected set reuses the AUD144 r3 build source inventory and adds current FFI and Crossover source files. It includes the Crossover preset-only topology check, final C-ABI state regression source, and the DynamicEQ change present at build time. This is a stability receipt for the listed inputs, not a full workspace hash.

Immutable library copy: `crates/sotf-plugins/target/audit-artifacts/aud142-ffi-state-r2/libplugins_ffi.so`. Verify its SHA-256 with `cdylib-artifact-sha256.txt`.

The FFI library suite passed 91 tests with 1 ignored after the preset topology implementation. The subsequently added same-layout preset assertion changed only the test fixture; the focused test then passed 7/7, including matching-layout preset acceptance, two-way preset refusal on a four-way handle, and raw partial-merge retention of inactive split frequencies. Strict all-target FFI Clippy passed with warnings denied. Logs are preserved in this directory.

The unchanged external state and full-preset probes have not yet been executed against this r2 library; root owns those runs.
