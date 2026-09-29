# AUD089 — Denoiser persistent constructor fields are silently discarded

Status: read-only review and public red probe complete; no repository edits.

## Reproduction

- Probe source: `/tmp/sotf-denoiser-config-probe.rs`.
- Output: `/tmp/sotf-denoiser-config-red.log`.
- Executable: `target/audit-denoiser-config-probe` (linked against the existing workspace facade and bridge artifacts).
- Input JSON: `{"harmonic_percussive":true,"spatial_denoise":true,"spatial_strength":0.9}`.
- Both public facade and bridge factories initialize successfully, but scalar getters return `false`, `false`, `0.5`.
- Deserializing then serializing the facade-reexported `DenoiserPluginParams` omits all three keys.
- Named scalar writes after initialization return `true`, `true`, `0.9` on both instances. This isolates the loss to constructor config parsing/copying, rather than missing runtime controls.
- Both factories accept configured `spatial_strength:1.5`, because the unknown key is discarded before constructor validation. The fix must validate the newly recognized value rather than silently clamp it.

## Existing contract and routes

| Field | Persistent model/schema default | Type/range | Existing index |
| --- | --- | --- | --- |
| `harmonic_percussive` | false | bool, realtime | 26 |
| `spatial_denoise` | false | bool, realtime; audio effect applies to stereo or more channels | 27 |
| `spatial_strength` | 0.5 | float, 0..=1, realtime; schema step 0.1 | 28 |

Source evidence:

- `crates/sotf-plugins/crates/sotf-plugin-denoiser/src/config.rs:109`: public `DenoiserPluginParams` ends at `multi_resolution`; its `Default` also lacks these three fields. Unknown JSON keys are accepted by the existing serde contract.
- `src/params.rs`: the internal serializable `Params` already includes all three, their defaults, and indexed mappings.
- `src/params/consts.rs:213`: static schema provides the defaults/range above.
- `src/lib/denoiser_plugin.rs:519`: scalar getters already expose all three. Existing indexed/named setters apply them without allocation.
- `src/lib/denoiser_plugin.rs:667`: `try_from_params` validates existing scalar config with the cached schema, copies it into the runtime model, prepares optional multi-resolution state, then rebuilds parameter metadata. It never copies these three values because the public config does not contain them.
- `src/wiener/consts.rs:85`: harmonic/percussive processing already uses the flag and prepared separator state.
- `src/wiener/consts.rs:153`: spatial processing already checks the flag and channel count, then applies the strength.
- Facade `crates/sotf-plugins/src/factory/create.rs:429`: deserializes this public config and calls `try_from_params`.
- Bridge `crates/sotf-plugins/crates/plugins-bridge/src/factory.rs:303`: same type and constructor.
- Engine `crates/sotf-engine/src/plugins/plugin_settings.rs:1109` and `plugin_config_converter/effects.rs:523`: already retain and serialize the three fields. The facade constructor drops them later.
- FFI `crates/sotf-plugins/crates/plugins-ffi/src/plugin_factory.rs`: delegates constructor config to the common bridge after removing FFI metadata. The same core fix repairs this route; no new C ABI fields are needed.
- NIH `plugins-nih/src/params/configuration.rs:constructor_config` serializes structural controls only. The three affected controls are realtime. `plugins-nih/src/wrapper.rs:311` calls `DynamicParams::sync_to_plugin` after initialize, applying changed realtime values before activation completes. Therefore current native persisted realtime state is restored by scalar setters and must not be described as broken by this constructor defect.

All current repository literals of `DenoiserPluginParams` outside its own `Default` use `..Default::default()` (including the facade restoration example, fuzzer, QA binary and integration fixtures). Adding fields does not require edits to those call sites. External exhaustive Rust literals are a source-level additive-struct compatibility consideration; JSON presets retain defaults.

## Minimal implementation proposal

Production scope: only `sotf-plugin-denoiser/src/config.rs` and the constructor section of `src/lib/denoiser_plugin.rs`.

1. Add the three public persistent fields, using the existing bool/bool/f32 constructor type conventions. Derive serde defaults from the existing static parameter schema, matching the module's current helper pattern.
2. Extend `Default` with the same schema-derived values. Old JSON `{}` and presets missing the keys remain `false`, `false`, `0.5`.
3. Add `spatial_strength` to `try_from_params`' existing finite/range validation before allocation/construction. Accept exactly 0 through 1; reject out-of-range and nonfinite typed values. JSON wrong types should now produce serde errors rather than being ignored.
4. Copy all three config fields into the existing runtime parameter fields before final cached metadata rebuild. No new state allocation, DSP algorithm, parameter ID/index, realtime policy, host adapter, engine converter, FFI ABI, or NIH restoration change is required.
5. Keep mono construction valid when `spatial_denoise=true`; the persisted flag is valid but the existing audio behavior requires at least two channels.

Do not add `learn_noise` or `clear_profile` as persistent constructor fields. Their existing schema entries (indices 20 and 22) describe named-setter actions, not retained independent settings. `clear_profile` reads false after execution; indexed writes are deliberately non-triggering. Their serialization/UI command design is a separate concern and must not be silently reinterpreted by this change.

## Focused regression plan

1. Core typed config tests: `{}` defaults; explicit nondefault serialize/deserialize roundtrip; exact IDs/types; strength endpoints 0 and 1; invalid negative, greater-than-one, NaN and infinities through the typed constructor; wrong JSON types rejected.
2. Core constructor getter checks before and after initialize for all three controls, including mono with the spatial flag retained.
3. Independent route equivalence: constructor-configured nondefaults versus an otherwise identical default constructor with named scalar setters applied before input. Process deterministic nonidentical stereo channels through low-latency/full FFT sizes and with/without multi-resolution, using irregular blocks. Require exact waveform parity and matching parameter snapshots; this verifies active behavior beyond serialization/getters.
4. Public facade and bridge factory regression using the unchanged red probe configuration. Ensure invalid strength is rejected by both. Engine test should send actual `PluginSettings::Denoiser` through `config_to_json` and the public factory, then check all three values. No production engine change expected.
5. Keep a default-config test with action-looking unknown keys to document that constructor loading does not start learning or clear a captured profile. Do not promote those keys into persistent fields or change NIH action restoration in this patch.
6. Run the focused new tests, full Denoiser crate tests and all-target Clippy after the concurrent HPSS reset owner releases the crate. A small facade/bridge roundtrip test is sufficient; no broad native architecture change or fresh global suite is needed until the parent aggregate gate.

## Boundaries

The independent HPSS reset correction is owned by the dynamics agent and must be preserved. This proposal does not change processing lifecycle, reset, finite drain, allocation policy or adaptive backend internals. No MIDI/IAMF, host DAG queues or engine manager protocol changes are involved. No implementation has been applied during this review.
