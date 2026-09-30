# AUD139 Crossover default-sync regression

## Finding and correction

The prior full NIH run used `DynamicParams::from_infos` in `all_wrapper_defaults_sync_to_initialized_plugins`. That generic constructor has no plugin identity, so it cannot apply Crossover's fixed-schema policy for dormant cutoffs. The actual NIH wrapper constructs `DynamicParams::from_infos_for_plugin(plugin_type, infos)`. That policy leaves fixed-schema cutoff values available in native state, but skips global cutoff IDs that are absent from the currently prepared topology. Runtime plugin metadata stays topology-specific, and invalid structural writes remain rejected.

The test now uses the same plugin-aware constructor as the wrapper. No production source changed for this correction. It does not filter Crossover out or weaken the synchronization assertion.

## Verification

- Focused default-sync test: 1 passed, 0 failed (`focused-default-sync.log`, SHA-256 `33e6c1091b3cea85d89a30d75c3864d4c597121ed2a1c75b48e113ecb0ffcca1`).
- Full NIH library suite with `--no-default-features --features dynamic-eq`: 118 passed, 0 failed, 1 ignored (`full-nih-lib.log`, SHA-256 `37dae05de5d2dfa1a861aeb8c4e089f3ba431286d8691c04e227ac7767e4d866`).
- Strict all-target Clippy with the same feature selection: passed (`strict-clippy.log`, SHA-256 `381a5d955dc379e3e717857cc84cca2e188ed4b8a611c1d6a70454459c41de05`).
- `rustfmt --edition 2024 --check crates/sotf-plugins/crates/plugins-nih/src/params_default_sync_tests.rs`: passed.
- Commands are retained in the adjacent `*-command.txt` files. All Cargo commands used `/tmp/sotf-daw-audit-cargo.lock`, offline locked mode, and the shared warm target.
- `source-before.sha256` and `source-after.sha256` match exactly. They bind the test, NIH parameters/configuration, native Crossover schema, Crossover runtime sources, and workspace `Cargo.lock`. The selected test file is copied under `selected-source/`; its tested SHA-256 is `41c739e4dd9aa134b74b4c49f60a2a9e4c907e888908960a9dc307b2603b1bb5`.
- Workspace lock SHA-256 is `db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`; the exact lock copy is in `selected-source/Cargo.lock`.

The preceding failure is preserved at `audit/artifacts/aud139-vst3-restart-r1/logs/sotf-aud139-native-dynamiceq-full-lib-r1.log` (SHA-256 `c6d01c990a388e911b83790f58dea209e4db3437d92b7abd7f129661a85df824`): 117 passed, 1 failed, 1 ignored, with the Crossover `Unknown parameter: frequency_2` test failure. This packet supersedes that result for the current feature-scoped NIH source set.
