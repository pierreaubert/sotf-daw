# AUD139 native VST3 Dynamic EQ restart provenance correction (r2)

This packet corrects the source-provenance gap noted in the AUD139 Astra review. The original
`aud139-vst3-restart-r1` packet remains unchanged; this packet adds a fresh focused run bound to
the omitted VST3 production sources.

## Focused run

- Command (executed under `/tmp/sotf-daw-audit-cargo.lock`):
  `TMPDIR=/tmp CARGO_TARGET_DIR=crates/sotf-plugins/target cargo test --offline --locked --manifest-path crates/sotf-plugins/Cargo.toml -p plugins-nih --no-default-features --features dynamic-eq --lib native_dynamic_eq_vst3_restart`
- Terminal result: **2 passed, 0 failed, 0 ignored; 117 filtered out**. Session 8428.
- Log: `logs/vst3-focused.log`.
- The selected input set is `manifests/selected-paths.txt`. It contains 42 entries: every changed Rust source under the vendored NIH crate and `plugins-nih`, plus the root lockfile and the three relevant Cargo manifests. This is a selected-source binding, not a complete transitive compiler-input manifest.
- Focused start/end manifests are byte-identical and have SHA-256 `c59d0a18a727eaab6c747156a512e17a412c8b0de81b396cdd3774b99433c798`. The copied source files independently reproduce the same manifest.

## VST3 production source coverage

The focused source set includes all seven changed VST3-specific production files:

- `crates/3rdparties/nih-plug/src/plugin/vst3.rs` — production opt-in for restart on required parameter changes.
- `crates/3rdparties/nih-plug/src/wrapper/vst3.rs` — factory host-context acquisition and wrapper construction.
- `crates/3rdparties/nih-plug/src/wrapper/vst3/context.rs`
- `crates/3rdparties/nih-plug/src/wrapper/vst3/inner.rs`
- `crates/3rdparties/nih-plug/src/wrapper/vst3/util.rs`
- `crates/3rdparties/nih-plug/src/wrapper/vst3/view.rs`
- `crates/3rdparties/nih-plug/src/wrapper/vst3/wrapper.rs`

The r1 packet already contained `inner.rs`, `view.rs`, and `wrapper.rs`; it omitted the other four
files. All seven exact tested bytes are copied under `selected-source/` and listed in both run
manifests. `manifests/copied-source.sha256` matches the focused start manifest.

## Related NIH test source

The run also binds the stable `plugins-nih/src/params_default_sync_tests.rs` bytes
(`41c739e4dd9aa134b74b4c49f60a2a9e4c907e888908960a9dc307b2603b1bb5`). The focused filter does
not execute that default-sync test. A separate feature-scoped full NIH library run subsequently
passed **118 tests, 0 failed, 1 ignored**. Its log SHA is
`37dae05de5d2dfa1a861aeb8c4e089f3ba431286d8691c04e227ac7767e4d866`; matching selected-source
manifests have SHA `d1016ae9568c5fddf2d45f0db3869da28e1251a2e39467e15731ba4ea523a4e4`. The full run's
packet is `audit/artifacts/aud139-crossover-default-sync-r2/` (index SHA
`48b24439f92ad2f4b2cf8a95fa7bbb8492e8baa0922eb71456997e8e108fc526`). Its strict all-target
Clippy also passed (log SHA
`381a5d955dc379e3e717857cc84cca2e188ed4b8a611c1d6a70454459c41de05`); that separate selected
manifest does not replace this packet's focused VST3 source binding.

The two focused tests exercise deferred/coalesced VST3 structural requests, host-loop dispatch,
refusal and retry, prepared-state continuity, state import across wrapper recreation, and safe
behavior without a usable host run loop. They use the real NIH VST3 COM wrapper with a fake host;
they do not load an exported VST3 bundle or exercise a third-party DAW.

## Prior packet preserved

The original packet index remains SHA-256 `0964bffb5c140fe54c562f65eac97f22293138c8334ddf29534693f3feb8b5f2`, and its receipt remains SHA-256 `1dc2867e45d1affd6ab7fb6b4c059191bc5d62038c552785444c18d990b99dcf`. Its historical full NIH result (117 passed, one unrelated Crossover failure, one ignored) remains recorded there. This r2 focused run does not replace or reinterpret that result.
