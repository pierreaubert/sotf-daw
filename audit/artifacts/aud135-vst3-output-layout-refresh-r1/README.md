# AUD135 VST3 output-layout refresh checkpoint

## Source change

The VST3 Ambisonics reconfiguration path now refreshes its cached main-output
bus count, width, and active mask when the negotiated target changes. It also
clears the BandSplit-only layout marker. Previously, a 16-channel target could
be changed to an 8-channel target while `output_bus_widths[0]` still described
16 channels. The next process call then supplied a 16-channel bus with no
matching output pointers, and the NIH VST3 wrapper rejected it with
`kResultFalse`.

The loaded fixture retains the single-band state-preservation, parameter
sensitivity, and valid-retry assertions. Its deliberate dual-band path also
processes after expanding to a wide layout, shrinking to 7.1, and expanding
again. The later valid-envelope candidate-load failure compares complete state
and output against a synchronized twin, includes a cold-history sensitivity
control, and verifies a successful retry. These assertions exercise separate
contracts; the single-band route does not claim recursive history.

## Current green source and artifact

- `source/native_ambisonics_host_order7.rs`: SHA-256
  `b242fe9782a17137205ef37b3854c05e1153256e7ef7da97b18d675bf6588883`.
- `source/vst3_backend.rs`: SHA-256
  `41b56cbcb96c240370273e06358f19ed29d0e674f1fbcf2a49b7abc8cab44b63`.
- Fresh exported library: `crates/sotf-plugins/target/audit-artifacts/aud135-native-late-restore-r1/libplugins_nih.so`, SHA-256
  `7dfc7b96519b0d884a3b06633c009e3024f0a5a46a418236f43596699f74a3b8`.
- Earlier exported library: `crates/sotf-plugins/target/audit-artifacts/aud135-native-host-restore-fix/libplugins_nih.so`, SHA-256
  `54c29a62f65c93ee65aa257b521089c8510d385388229413d4669602a00de20a`.

The old and fresh native binaries remain at those target paths; they are not
duplicated into this source archive. The current green loaded run uses the
fresh binary through its `.clap` and `.vst3` format aliases.

## Commands and results

All Cargo commands below ran offline and locked under
`flock -x /tmp/sotf-daw-audit-cargo.lock`, with
`CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`
and `TMPDIR=/tmp`.

Fresh library build:

```sh
cargo build --offline --locked -p plugins-nih --features ambisonics
```

Focused VST3 route after the cache correction:

```sh
SOTF_TEST_AMBISONICS_VST3_PLUGIN=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud135-native-late-restore-r1/ambisonics.vst3 \
cargo test --offline --locked -p sotf-plugins --features 'external-plugin-clap external-plugin-vst3' \
  --test native_ambisonics_host_order7 -- --exact \
  exported_vst3_order_seven_audio_matches_direct_decoder_and_saved_state --ignored --nocapture
```

Result: 1 passed, 0 failed. Full combined loaded CLAP and VST3 route:

```sh
SOTF_TEST_AMBISONICS_CLAP_PLUGIN=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud135-native-late-restore-r1/ambisonics.clap \
SOTF_TEST_AMBISONICS_VST3_PLUGIN=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud135-native-late-restore-r1/ambisonics.vst3 \
cargo test --offline --locked -p sotf-plugins --features 'external-plugin-clap external-plugin-vst3' \
  --test native_ambisonics_host_order7 -- --ignored --nocapture
```

Result: 2 passed, 0 failed. Strict host lint:

```sh
cargo clippy --offline --locked -p sotf-host --all-targets \
  --features 'external-plugin-clap external-plugin-vst3' -- -D warnings
```

Result: passed. Logs and hashes:

- `logs/sotf-aud135-native-late-restore-build-r1.log`: `28bbc2538eed2335c13d19c26558d18c9567bb41ca6e0faae52d811de28ce669`.
- `logs/sotf-aud135-native-layout-refresh-vst3-r3.log`: `18ea1d9a3e29d71fb1bf12a26a887daea9fc367cbbe16b479e7a05d98c029cc5`.
- `logs/sotf-aud135-native-layout-refresh-host-r1.log`: `2148234eebac9d5025f4996dab44ba02067a53bc6dab3a1fbac315c33b85f4b5`.
- `logs/sotf-aud135-native-layout-refresh-host-clippy-r1.log`: `8a26ab1ed59b36274f633bf77bfb1af830e14fb2b777a7ec4081c1facaffc547`.

## Preserved failure comparison

The VST3 process failure was reproduced with the same strengthened host-test
source against both the fresh `7dfc7b96…` library and the archived
`54c29a62…` library. Both failed while processing immediately after the
order-seven 7.1 reconfiguration, before reaching the late-candidate section.
This comparison showed that the failure was not unique to the refreshed
artifact. The source hash was recorded by the run owner only as the prefix
`63dd9c…`; the exact prior source bytes and a complete source manifest were
not retained, so this is owner-observed same-source evidence rather than a
reproducible archived source pair.

- `logs/sotf-aud135-native-late-restore-vst3-diag-r1.log` (fresh library), SHA-256
  `353bcdee50ebe11919d487963cee865afbe24363439c8f2c69b257646f3a10a6`.
- `logs/sotf-aud135-native-late-restore-vst3-oldartifact-r1.log` (archived library), SHA-256
  `8362d2abb8085b63f11bf416a22f3ec0fdf72f17b508acc54b3d198684a7b139`.
- `logs/sotf-aud135-native-late-restore-host-r3.log` preserves the earlier
  combined attempt, SHA-256
  `0e1e40aa5ad5e02af664408c92f24d02f0a6f81d951f27311b80b1f66ef9f141`.
- `logs/sotf-aud135-native-late-restore-vst3-r2.log` preserves the diagnostic
  reduction attempt, SHA-256
  `29f9a85027aa18340ec9e8cf4b27e2558837a65e0125b131c380095a5ee9e514`.

Earlier fixture-assertion corrections are retained in
`logs/sotf-aud135-native-late-restore-host-r1.log` and `-r2.log`; those are not
additional native-process failures. No full transitive source manifest was
captured for the new red/green series. The current test and VST3 backend bytes
are archived here and their hashes are recorded above.

This checkpoint closes the output-cache defect and verifies loaded CLAP/VST3
reconfiguration on the tested Linux artifacts. It does not close AUD135's
isolated-worker, engine/UI, or EOF routes.
