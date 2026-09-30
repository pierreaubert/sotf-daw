# AUD142 Crossover FFI metadata and constructor precedence focused gate

- Command environment: `TMPDIR=/tmp`, `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`
- Offline locked Cargo commands, serialized by `/tmp/sotf-daw-audit-cargo.lock`:
  - `cargo test --offline --locked -p sotf-plugin-crossover --lib test_complete_channel_modes_make_global_output_dormant -- --nocapture`
  - `cargo test --offline --locked -p plugins-ffi --lib aud142_crossover_route_tests -- --nocapture`
- Result: exit 0; constructor precedence 1/1, FFI metadata/family tests 2/2.
- The unit gate covers complete explicit channel modes with dormant `output=both`, empty/short legacy scalar fallbacks, malformed explicit modes, and overlong mode rejection.
- The FFI gate covers preserved per-instance ID order for two-way, multiway, FIR, and per-channel instances; all 13 canonical family choice labels and normalized runtime values on constructed instances; title-case display labels over lowercase internal values; and unchanged linear cutoff normalization at 0.25, 0.5, and 0.75.
- Full command output and hashes for the source, crate manifests, workspace lock, and log are in this directory.
