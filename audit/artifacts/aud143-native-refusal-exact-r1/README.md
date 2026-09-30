# AUD143 exact late-refusal continuation check

This focused test-only update closes Astra's late-refusal history-evidence finding for the frozen loaded BandSplit host fixtures.

## Evidence

- The CLAP and VST3 late-refusal cases now compare the complete live and synchronized-twin continuation vectors with equal-length checks, finite-sample checks, and exact `f32` value equality. The twin uses the same frozen plugin binary/state and receives the same two warmup blocks as the live instance.
- The cold sensitivity requirement remains `cold_error > 1e-6`; no DSP tolerance or cold threshold was changed.
- Focused command: see `command.txt`. It ran as session `57859`, exit `0`: 2 passed, 0 failed, 3 filtered out. The Cargo lock was released at terminal.
- Test log SHA-256: `5033d817f5a66442a4fd712b6149fb3f079b74ff426fb6a9f4627219b4d85ef5`.
- The source frozen for the command is `crates/sotf-plugins/tests/native_bandsplit_external_host.rs`, SHA-256 `e00a9dba1ee29c8f09b7b7819d2543fd7ee2cbdf22cb81f8109698bbbc2f2811`. `source-before.sha256` and `source-after.sha256` are equal; both manifest files have SHA-256 `9247f2fffdc58d99d8228114efa5f7de3a8f589a09a7b545f2f6b497f02c4392`.
- The two host artifacts were unchanged across the run. Both `BandSplit.clap` and `BandSplit.vst3/Contents/x86_64-linux/BandSplit.so` have SHA-256 `6f1803cbad160f59f589569e600e10ffc7dc1a5cf5986b68e4eff123a5ef838b`; pre/post bundle identity manifests are equal and each has SHA-256 `d94c071dd0ff347cdfd02260f2dd311a1c27e6050d539689be7ad165794989be`.
- `source-change.patch` records the bounded test change relative to `audit/artifacts/aud143-native-buffer-reuse-r1/sources/native_bandsplit_external_host.rs` (SHA-256 `d98f1d6091b66c92e03a17eb470baf3b98cb4639fdfbe8e4f270fdd16e687141`). The captured post-change test source is also retained in this directory.

No production code or plugin artifact was changed for this evidence pass. The result establishes exact synchronized continuation equality for these frozen CLAP/VST3 binaries and this late-refusal fixture; it does not generalize to other plugin builds or host implementations.
