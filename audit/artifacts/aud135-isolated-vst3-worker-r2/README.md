# AUD135 isolated VST3 worker checkpoint

This packet records the isolated-worker test and focused Clippy rerun against
the current host test source. It does not cover the requested engine update or
EOF routes.

## Executed gates

Both commands used `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`,
`TMPDIR=/tmp`, `CARGO_NET_OFFLINE=true`, and the shared lock
`/tmp/sotf-daw-audit-cargo.lock`.
The workspace `Cargo.lock` hash after these `--locked` runs is
`db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`.

1. Worker callback gate, exec session `2782`, exit 0, 1 passed:

   ```text
   SOTF_TEST_AMBISONICS_VST3_PLUGIN=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud135-native-late-restore-r1/Ambisonics.vst3
   cargo test --offline --locked -p sotf-host --features external-plugin-vst3 --test native_vst3_host isolated_vst3_order_seven_late_native_load_failure_preserves_populated_worker -- --ignored --exact --nocapture
   ```

   The environment value is the VST3 bundle root; its inner Linux library is
   `Contents/x86_64-linux/ambisonics.so`.

2. Focused strict test Clippy, exec session `10540`, exit 0:

   ```text
   cargo clippy --offline --locked -p sotf-host --features external-plugin-vst3 --test native_vst3_host -- -D warnings
   ```

The full command transcripts are in `logs/worker-current-r3.log` and
`logs/clippy-current-r3.log`. Their SHA-256 values are respectively
`70c827a2cafd867111a6ae739d457c5955b7b0824f2cd3d23684fa325d848d76` and
`fa48f3277ce3578902c83e9b614cc79f72910ad126add89f7f02c27f2324c12e`.

## Source and artifact identity

The source copied to `source-executed/native_vst3_host.rs` is the source used by
both current gates: SHA-256
`6617421b150507482cb702cebec24c9e17c45831c71e7181c75df5316ace723b`. The
working-tree test file has the same hash. The VST3 backend source copied to
`source-current/vst3_backend.rs` and the working-tree backend both have SHA-256
`41b56cbcb96c240370273e06358f19ed29d0e674f1fbcf2a49b7abc8cab44b63`.

`source-historical-r1/native_vst3_host.rs` preserves the test source from the
earlier session 55414 pass, SHA-256
`9acc959d76077dd27ed5fc05c1cac29724dddd80655779663d5c2d8bd246130b`. The only
change between that file and the current test is a test-helper Clippy cleanup
from `% 2 == 0` to `.is_multiple_of(2)`. The earlier temporary log is no longer
available, so session 55414 is historical corroboration; the durable evidence
for the current source is the pair of logs above.

The loaded artifact used by the current test is
`crates/sotf-plugins/target/audit-artifacts/aud135-native-late-restore-r1/libplugins_nih.so`.
Its SHA-256 is
`7dfc7b96519b0d884a3b06633c009e3024f0a5a46a418236f43596699f74a3b8`. The
same bytes are present at the VST3 bundle's inner library path. The passing
test covers the order-7 64-input/16-output worker, late candidate native-load
refusal, live/twin continuation, persisted-state sensitivity, and a valid
retry. It is not an engine commit or EOF test.

The earlier `.so`-as-descriptor-path attempt failed format validation before
plugin loading. Its temporary log was also unavailable; it is retained only as
a fixture-path diagnostic, not as a plugin failure or current gate.

This is a selected-source and artifact packet, not a full transitive source
manifest. See `SHA256SUMS` for packet file hashes.
