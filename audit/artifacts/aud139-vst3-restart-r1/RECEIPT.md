# AUD139 native VST3 Dynamic EQ restart checkpoint

This packet captures the Dynamic EQ VST3 lifecycle slice for Astra review. It includes the
current selected Rust sources, the Cargo lock, terminal logs, and selected-source manifests.

## Current evidence

- `cargo test --offline --locked --manifest-path crates/sotf-plugins/Cargo.toml -p plugins-nih --no-default-features --features dynamic-eq --lib native_dynamic_eq_vst3_restart`: **2 passed** in `logs/sotf-aud139-native-dynamiceq-vst3-r13.log`.
- `cargo clippy --offline --locked --manifest-path crates/sotf-plugins/Cargo.toml -p plugins-nih --no-default-features --features dynamic-eq --all-targets -- -D warnings`: **passed** in `logs/sotf-aud139-native-dynamiceq-clippy-r2.log`.
- The selected source set was byte-identical across the focused VST3 run: `manifests/sotf-aud139-native-dynamiceq-vst3-r13-start.sha256` and `...-end.sha256` have the same bytes and SHA-256 `0c6be2c3deaab3f71b81f0e36e26b563c4156034b396a0c06b23b652fee2ceb3`.
- `Cargo.lock` SHA-256: `db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`.

The focused tests cover deferred/coalesced LowShelf shape+slope requests; non-automatable metadata;
main-thread callback dispatch; refusal and same-value retry; old prepared Peak waveform continuity;
failed 8 kHz initialization preserving requested values; successful 48 kHz reactivation; complete
stereo output equality with an independently configured DynamicEQ core; saved state across a full
wrapper recreation; no callback without a usable host loop; failed IRunLoop registration; and saved
control restoration into a later loop-capable instance.

## Full library result and scope

The feature-scoped full NIH library run is in `logs/sotf-aud139-native-dynamiceq-full-lib-r1.log`:
**117 passed, 1 failed, 1 ignored**. Both VST3 tests passed in that run. The sole failure is the
existing Crossover default-sync test, which reports `Unknown parameter: frequency_2`; it is outside
this DynamicEQ VST3 change. The full-library run preceded only a test-helper Clippy cleanup that
replaced default-then-field assignment with an equivalent struct initializer; the final focused
run and strict Clippy both cover the resulting source.

## Review boundary

The test calls the VST3 COM interfaces through the real NIH wrapper and a fake host. The Linux
`IRunLoop` callback, refusal, and registration paths execute here; this is not a load test in a
third-party DAW or an exported `.vst3` bundle test. Windows/macOS dispatch was not executed. The
SOTF consuming-host control path and product UI remain separate follow-up work.

`logs/sotf-aud139-native-dynamiceq-vst3-r9-gdb.log` records the earlier fake-host lifetime crash;
the restored handler owner was moved ahead of its wrapper instance, and the final test is green.
`logs/sotf-aud139-native-dynamiceq-vst3-r11.log` records an obsolete test expectation that a
pre-activation `set_state` should issue a restart; the final test instead verifies that a later
supported instance applies the saved state during initialization.
