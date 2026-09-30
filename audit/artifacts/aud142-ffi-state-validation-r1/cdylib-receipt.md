# AUD142 FFI cdylib r1

Build command: `cargo build --offline --locked -p plugins-ffi --lib`

Execution used `TMPDIR=/tmp`, `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`, and `SOTF_FFI_SKIP_HEADER_SYNC=1` to avoid generated-header writes. Cargo was serialized with `/tmp/sotf-daw-audit-cargo.lock`.

Build exited 0. The selected source set contains 342 files; `build-source-start.sha256` and `build-source-end.sha256` are byte-identical. The selected set reuses the AUD144 r3 build source inventory and adds current FFI and Crossover source files. Upmixer's just-made DynamicEQ source edit is included. This is a source-stability receipt for the listed inputs, not a full workspace hash.

Immutable library copy: `crates/sotf-plugins/target/audit-artifacts/aud142-ffi-state-r1/libplugins_ffi.so`. Verify its SHA-256 with `cdylib-artifact-sha256.txt`.

The matching full FFI lib tests and strict all-target FFI Clippy logs are in this directory. The independent unchanged public C-ABI probe has not yet been run against this r1 library.
