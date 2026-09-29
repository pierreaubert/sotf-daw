# AUD089 — Denoiser constructor wiring verified and frozen

## Change

Persistent `harmonic_percussive`, `spatial_denoise` and `spatial_strength` now deserialize/serialize through `DenoiserPluginParams`, use the existing schema defaults (`false`, `false`, `0.5`), and reach the runtime model before cached parameter metadata is rebuilt. `try_from_params` validates strength as finite and within 0..=1 before construction. Existing indices 26/27/28 and realtime setters are unchanged.

Production changes are limited to:

1. `crates/sotf-plugins/crates/sotf-plugin-denoiser/src/config.rs`: three persistent fields, schema-derived serde/Default helpers and field documentation.
2. `crates/sotf-plugins/crates/sotf-plugin-denoiser/src/lib/denoiser_plugin.rs`: one constructor validation and three field copies only for AUD089. Earlier changes in this file belong to other accepted audit work and were preserved.

New regression files:

3. `crates/sotf-plugins/crates/sotf-plugin-denoiser/tests/configuration.rs` — five tests.
4. `crates/sotf-plugins/tests/denoiser_configuration.rs` — two tests.
5. `crates/sotf-plugins/crates/plugins-bridge/tests/denoiser_configuration.rs` — two tests.
6. `crates/sotf-engine/tests/denoiser_configuration.rs` — one test; no engine production changes.

The AUD088 HPSS reset implementation and its tests are preserved. No backend DSP, lifecycle, action control, NIH production, host queue or manager protocol changes were made.

## Independent evidence

Original public probe: `/tmp/sotf-denoiser-config-probe.rs`, red output `/tmp/sotf-denoiser-config-red.log`. The facade and bridge previously accepted configured true/true/0.9 but returned false/false/0.5, and accepted invalid strength 1.5 by discarding its unknown JSON key.

New tests verify:

- Old empty JSON defaults and explicit typed config roundtrips.
- Bool/float values and unchanged parameter indices before/after initialization, including mono preserving the valid spatial flag and strength endpoints 0 and 1.
- Out-of-range/nonfinite typed strength, and wrong JSON types, are rejected.
- `learn_noise` and `clear_profile` remain absent from persistent config serialization and are not fired by constructor JSON. Existing named-setter action semantics are unchanged.
- Constructor-configured audio exactly equals the existing named-setter route for separate harmonic/percussive and spatial modes plus their combination, both 512/2048 FFT configurations, with/without multi-resolution, and before/after reset. The deterministic distinct-channel tone/noise/impulse signal uses irregular 1/17/257/63/1024-frame callbacks and 8193 source frames; 12 configuration combinations each run twice. Nonzero output is checked.
- Public facade and bridge constructors retain values and reject malformed/range-invalid configs; bridge saved-state roundtrip matches the complete snapshot.
- Engine `PluginSettings` serialize/deserialize then convert through the real public factory, retaining these controls and rejecting strength above one.

## Verification

All commands completed successfully using the existing shared Cargo target:

- `cargo test -p sotf-plugin-denoiser --all-features`: **83 passed** (67 unit + 5 new configuration + 8 finite-stream + 2 realtime + 1 polyphonic). Log `target/audit-denoiser-config-full.log`.
- `cargo test -p sotf-plugins -p plugins-bridge -p sotf-engine --test denoiser_configuration`: **5 passed** (facade 2, bridge 2, engine 1). Log `target/audit-denoiser-config-routes.log`.
- `cargo clippy -p sotf-plugin-denoiser --all-features --all-targets -- -D warnings`: clean. Log `target/audit-denoiser-config-clippy.log`.
- `cargo clippy -p sotf-plugins -p plugins-bridge -p sotf-engine --test denoiser_configuration -- -D warnings`: clean. Log `target/audit-denoiser-config-routes-clippy.log`.
- Scoped rustfmt and `git diff --check`: clean.

Total focused test count: **88**, including **10 new tests**. No ignores or test-only production paths were introduced.

## Compatibility

Old JSON presets retain identical defaults. Newly recognized keys now reject wrong types and invalid strength instead of silently ignoring them; this is intentional constructor validation. No parameter identifiers, ordering, update policies or C ABI signatures change. FFI delegates to the corrected bridge constructor; no new direct FFI runtime claim is made. NIH already restores these realtime values through its activation scalar synchronization and needed no code change.

Adding public struct fields requires downstream exhaustive Rust `DenoiserPluginParams` literals to supply the fields or use `..Default::default()`. Every current repository call-site literal already uses struct update syntax. No manifest or lockfile changed.

State: all AUD089 source and tests frozen; ready for the parent aggregate gate.
