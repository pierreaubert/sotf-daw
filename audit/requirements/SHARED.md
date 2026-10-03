# Shared ownership and integration requirements

The [plugin files](README.md) are separate assignments, not guarantees of disjoint source ownership. Before execution, name an owner for each shared area below. Separate worktrees prevent accidental overwrites but still require semantic integration.

| Area | Shared scope / conflict | Coordination rule |
|---|---|---|
| Compressor pair | Compressor and Multiband Compressor use `sotf-plugin-multiband-compressor` | Assign both files to one worker. |
| Expander pair | Expander and Multiband Expander use `sotf-plugin-multiband-expander` | Assign both files to one worker. |
| Analog family | `sotf-plugin-analog-common` and Analog EQ/Compressor/Limiter | One common-library owner; preserve and test all consumers. |
| Restoration | Denoiser/Hiss Reducer and `plugins-denoiser` | Coordinate profile formats, shared spectral code and model/resource handling. |
| Inference | Speech Denoiser/Upmixer, `plugins-inference`, RNNoise dependencies | One owner for common model/runtime changes; preserve model-specific contracts. |
| Spatial | `plugins-spatial`, shared geometry/SOFA readers and math filters | Coordinate Binaural, XTC, Upmixer, Downmix, Ambisonics and adaptive consumers; do not change conventions independently. |
| Split/recombine | Band Split, Crossover, Band Merge and common filters | Agree channel ordering, rate/latency and reconstruction contracts before changes. |
| Native wrappers | `plugins-nih/src/wrapper.rs`, params/configuration, wrapper modules and vendored `nih-plug` | One integration owner; native EQ continuation currently touches these files. Plugin workers provide scoped patches/tests. |
| FFI/bridge/factory | `plugins-ffi` parameter/preset mapping, `plugins-bridge`, facade factory/catalog | One integration owner; preserve old IDs and append metadata deliberately. |
| Engine/application | Engine settings/converters/accessors/planner/manager; sibling `sotf` player/controllers/GPUI | One owner for common dispatch, receipts, charts and UI plumbing. Coordinate plugin-specific leaves. |
| Host analyzers | Loudness, Spectrum, Correlation, snapshots and shared AutoGain | Coordinate snapshot contracts, meter units and consumers. |
| HAL | HAL Input/Output, `driver-common`, `driver-hal`, sibling systemwide integration | One transport owner and an actual macOS validation lane. |
| Release/evidence | Corpus fixtures, platform builds, packaging and final workspace gates | One integrator consolidates provenance and combined results. |

## Integration backlog retained outside individual plugins

- [ ] Finish native EQ callbacks/layouts/state and loaded-audio proof; resolve realtime restore admission. See [native handoff](../handoffs/aud145-native-eq-plugin-requirements.md).
- [ ] Complete GPUI EQ accepted/staged graph receipts, visible Retry wiring and correction behavior; preserve staged edits after rejection. See [checkpoint](CHECKPOINT.md).
- [ ] Finish FFI full EQ state/pair/advanced configuration and append-only parameter integration, plus player response/chart consumers.
- [ ] Complete remaining mixed-rate branch retention, variable-path AB Compare (`AUD087`) and recursive/branching end-of-stream delivery proofs. Respect the separate restriction on broad unequal-branch queue rewrites.
- [ ] Resolve the separately scoped manager quiescence (`AUD062`) and output-format protocol (`AUD063`) work before claiming complete transition coverage; do not bundle an unauthorized broad rewrite into a plugin.
- [ ] Close HAL sink engine admission (`AUD138`) and channel-changing engine endpoint (`AUD140`) evidence with relevant platform coverage.
- [ ] Audit remaining support layers, drivers, host/engine integration and testkit/macros as required by the overall goal. Per-plugin files do not remove this scope.
- [ ] Obtain authorized official loudness corpus access (`AUD128`) and other missing model/audio data; run relevant listening/accuracy cases and retain provenance.
- [ ] Validate supported native AU/platform/editor packaging and resource lifecycle on actual platforms; record unavailable lanes rather than declaring success.
- [ ] After shared changes stabilize, run current combined check/lint/test/QA gates from the repository recipes, honoring documented exclusions. Current historical workspace results do not certify today's dirty source.

## Merge/handoff protocol

Each worker reports owned files, shared patches, setting/schema changes, compatibility, exact focused gates and unresolved requirements. The shared owner integrates dependent patches in order, reruns affected consumer tests and reports combined evidence. Do not concurrently modify common registry/wrapper files just because two plugin folders differ.

Manual worktrees must start from a deliberately transferred checkpoint including required uncommitted work. This document does not authorize discarding, committing or publishing unrelated changes.
