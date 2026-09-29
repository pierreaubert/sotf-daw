# Support layer audit — 2026-09-27

Read-only follow-up to `AUDIT.md`, including AUD-001 through AUD-034. No source edits or tests were run for this review. MIDI and IAMF are excluded. Line numbers identify the inspected working tree and may move during concurrent work.

TokenSave status at review: 49,160 nodes; last sync 1790542264. Graph results were followed with current source reads. The aggregate test gate belongs to the root agent; prior passing counts below are evidence recorded in `AUDIT.md`, not new executions.

## End-to-end paths

- Engine/facade: canonical catalog → `sotf-plugins/src/factory/create.rs` → DSP/adapters → host chain. The catalog records channel admission, maturity and separate evidence gates. External plugin security/isolation is part of this factory.
- Native CLAP/VST3: static ParamSpec plus runtime discovery → `plugins-bridge` metadata → NIH DynamicParams → structural constructor → standalone preparation/oversampling → generated NIH process method → DSP. The default factory is a separate implementation from the facade factory.
- AU: Swift parameter tree/event processing → C `plugin_set_parameter` → FFI ParameterMap → shared ParamBridge or expanded-band fallback → DSP. Processing uses negotiated scratch and checked fixed-frame returns. GPUI reads an atomic parameter cache and writes through Swift callbacks; it does not directly mutate a live PluginHandle.
- Shared spatial/denoising code supplies algorithms rather than a complete host lifecycle. Inference provides a bounded worker exchange; users still own model and input/output semantics.

## Ranked remaining defects

### 1. FFI automation allocates on the AU render thread

**Confirmed by the call path; not newly measured.** `plugins-au/GenericAU/GenericRustAudioUnit.swift:128-145` processes active ramps in quanta of at most 16 frames and calls `plugin_set_parameter` inside that render loop. The C entry point (`plugins-ffi/src/lib/plugin.rs:860-889`) calls ParameterMap synchronously.

`plugins-ffi/src/parameter_map.rs:66-75` normalizes the plugin type on every getter/setter call using `collect::<String>().to_lowercase()`. These are owned heap strings even when the parameter ID needs no alias translation. `plugins-bridge/src/param_bridge.rs:176-179` additionally builds `spec.engine_key.to_string()` before constructing ParameterId. FFI fallback paths also allocate ID strings. These allocations occur on successful ordinary parameter changes, independently of a particular DSP setter. Error formatting adds more work on failure.

**Impact:** automation callbacks lose the allocation-free guarantee; an active ramp repeatedly exercises the path. Gain's native CLAP/VST3 allocation checks do not enter this FFI/AU route.

**Scoped fix:** canonicalize plugin family once at ParameterMap construction, prepare IDs and typed mappings once, borrow/copy prepared IDs on successful realtime changes. Audit the selected DSP setters separately. Do not solve this by dropping sample-offset automation.

**Required regression:** cold-thread allocation and deallocation counters around C setters for ordinary scalar controls and expanded bands, repeated ramp updates, and a native AU render/event regression on macOS. Cover valid changes and rejected structural controls. No warmup may hide first-use work.

### 2. FFI/shared bridge parameter typing still disagrees with runtime choices

`plugins-bridge/src/param_bridge.rs:305-327` converts every `ParamType::Choice` to `ParameterValue::Int` and converts every returned `ParameterValue::String` to numeric zero. `ParameterMap::set_normalized/get_normalized` directly uses that bridge for static specs (`plugins-ffi/src/parameter_map.rs:233-275`). NIH's fixed runtime synchronization and constructor mapping do not repair this separate path.

Concrete examples:

- AAE `room_preset` has labels `[small, medium, large, cathedral]` and default index 1 (`sotf-plugin-aae/src/params/consts.rs:8,55-58`). The runtime getter returns `String("medium")` by default (`src/lib/aae_plugin.rs:797`), so shared normalized readback is 0 instead of 1/3. Its typed setter validates String and only permits its active structural value (`:596-607,648-651`); the bridge sends Int even for the same default.
- De-esser `mode` is a Choice defaulting to index 1 (`sotf-plugin-de-esser/src/params.rs:65-69`). Runtime metadata/getter use String and the setter requires `wideband` or `split-band` strings (`src/lib/de_esser_plugin.rs:374,451-461,565`). Numeric bridge writes fail validation, and readback becomes zero.
- Expanded-band fallback always sends `ParameterValue::Float` (`plugins-ffi/src/parameter_map.rs:257-259`), although the metadata expansion also exposes Int/Bool/Choice controls (`:355-407`). This must be checked against each DSP's runtime contract; accepting numeric coercion in some setters does not make a universal Float mapping valid. For example EQ band order reads with `as_int().unwrap_or(2)` (`sotf-plugin-eq/src/lib/eq_plugin.rs:1107-1109`), so Float delivery can silently retain order 2 rather than the requested order.

C `ParameterInfo` also discards value-kind and realtime/setup flags (`parameter_map.rs:31-49,93-118`), leaving consumers with numeric range/steps/logarithmic only.

**Impact:** AU display/readback can disagree with actual DSP state, and exposed controls can reject legal values or apply a fallback. This is distinct from fixed NIH typed synchronization (AUD-011/015).

**Scoped fix:** prepare runtime-aware typed conversions and reverse choice mappings in the shared/FFI path; preserve parameter IDs and C ABI using an additive metadata query if needed. Structural changes must go through lifecycle reconstruction rather than an allocating audio-thread setter.

**Required regression:** all exposed FFI defaults and representative nondefaults, exact typed readback, all choice labels, expanded band order/bool controls, explicit unsupported structural changes. Do not ignore failed setters or compare only default audio.

### 3. Most native CLAP/VST3 plugins ignore intra-buffer parameter timing

`plugins-nih/src/wrapper.rs:5-14` enables `SAMPLE_ACCURATE_AUTOMATION` only for EQ and LinearPhaseEQ. All other generated wrappers select false. Their SOTF parameter sync happens once per NIH process invocation (`:355-363`).

The pinned NIH implementation conditionally splits on parameter events only when this flag is true: local dependency `nih-plug-d4210f82e920295f/de42101/src/wrapper/clap/wrapper.rs:1973-2024`, and VST3 `src/wrapper/vst3/wrapper.rs:975,1048,1357`. Without splitting, native gain/dynamics parameter changes at nonzero offsets can affect the whole callback rather than begin at the event sample. Smoothing affects the transition shape but does not restore the discarded start offset.

CLAP defines an event's `time` as its sample offset within the buffer ([primary header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/events.h)).

**Scoped fix:** enable the framework's sample splitting for supported realtime controls, preserve the structural guard, and verify that all exposed DSPs accept the resulting variable slices. Retain efficient unchanged-parameter sync.

**Required regression:** actual exported Gain CLAP/VST3 with changes at offset 0, interior, last sample and multiple equal-offset events; compare against an independently partitioned reference including smoothing. Include irregular host callbacks and an oversampled effect. Existing steady-state gain/state/allocation checks do not exercise this contract.

### 4. Native wrappers report no tail for every DSP

Successful generated NIH processing unconditionally returns `ProcessStatus::Normal` (`plugins-nih/src/wrapper.rs:406`). Pinned NIH `src/plugin.rs:254-270` defines Tail(samples) and KeepAlive separately. Its CLAP tail extension returns zero for Normal (`src/wrapper/clap/wrapper.rs:3178-3187`), and VST3 reports the analogous last status (`src/wrapper/vst3/wrapper.rs:1672-1680`). CLAP processing maps Normal to CONTINUE_IF_NOT_QUIET (`:2261`).

Thus delay/convolution/reverberant wrappers advertise zero tail even when signal remains. A host may suspend after a quiet interval or omit export tail. This is not repaired by the newly implemented explicit SOTF oversampling drain (AUD-033): the native wrapper neither describes that tail nor invokes a host end-of-stream drain protocol.

The [CLAP tail extension](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/tail.h) expresses tail length in samples and reserves large values for infinite tails.

**Scoped fix:** expose a conservative DSP tail contract and map it to NIH status, including state/parameter changes. A feedback effect without a provable finite bound needs honest indefinite processing. Do not infer zero tail from a temporarily silent output block.

**Required regression:** exported delayed impulse with a silence gap, host honoring returned status and tail query, finite convolution/oversampling tails versus direct DSP, and feedback-tail behavior. This is independent of a direct DSP drain-only test.

### 5. Native wrappers drop host transport and restart context at zero

The generated `process` accepts `_context` but never reads it (`plugins-nih/src/wrapper.rs:291`). It creates `sotf_host::plugin::ProcessContext::new(self.sample_rate, num_frames)` each invocation (`:384`). Consequently host sample position, tempo, playing state and loop information do not reach the DSP. Pinned NIH provides `ProcessContext::transport()` (`src/context/process.rs:41`).

AUD-031 repairs transport propagation *inside* oversampling adapters when the incoming context is correct; it cannot reconstruct information discarded at this outer boundary. This is confirmed contract/data loss; this review does not assert that every current DSP audibly depends on every field.

**Required regression:** wrapper probe that observes nonzero origins, advancing callbacks, transport changes/loops, and NIH automation subdivisions; repeat through oversampling with correct clock conversion.

### 6. Native convolution/HRTF file configuration is absent

`DynamicParams::from_infos` explicitly skips FilePath entries (`plugins-nih/src/params.rs:53-55`). Runtime String metadata is also omitted from generic discovery (`wrapper.rs`, `bridged_info_from_parameter`). The generated plugin has no file editor or separate persisted file state. Structural reconstruction only sees DynamicParams values (`params/configuration.rs:17-48,128+`).

Convolution's `ir_file` and Binaural's `sofa_file` are explicit FilePath specs (`sotf-plugin-convolution/src/params.rs:22`, `sotf-plugin-binaural/src/params.rs:27`). The native Convolution default is a usable identity processor, but its intended custom IR is not configurable/persistable through this wrapper. Binaural custom SOFA selection is similarly inaccessible. The structural matrix deliberately excludes FilePath (`params_default_sync_tests.rs:295`).

**Required work/evidence:** a control-thread file/resource state path with stable preset persistence, async loading where appropriate, activation/error handling, and a nonidentity native convolution audio test after save/restore. File contents/resource portability need an explicit product decision; they are not solved by representing a file path as a numeric DAW parameter.

### 7. Frequency normalization differs across the supposedly shared formats

The shared bridge maps positive Hz ranges geometrically (`plugins-bridge/src/param_bridge.rs:260-273`). NIH uses `FloatRange::Skewed` with `skew_factor(-2.0)` (`plugins-nih/src/params.rs:103-110`), a power curve in the pinned NIH `src/params/range.rs:44,107`.

For 20–20,000 Hz, normalized 0.5 means approximately **632.4555 Hz** in the bridge and **1268.75 Hz** in NIH. These values follow analytically from the inspected formulas; no test was run. Both formats can represent the same raw frequency, but normalized automation is not interchangeable.

This is a consistency defect where common normalization is promised, not proof that either isolated mapping is invalid. Correcting existing native curves changes saved normalized automation and requires a compatibility/versioning decision. Add a real NIH-versus-bridge mapping test before choosing a migration.

## Per-crate capability and evidence inventory

| Layer | Implemented capabilities | Numerical/realtime evidence and remaining limits |
|---|---|---|
| `plugins-bridge` | Factory for the packaged DSP families; typed metadata; geometric Hz normalization; planar/interleaved validation; standalone preferred-oversampling preparation; scalar JSON state helper | Factory/default/normalization tests and wrapper preparation regressions exist. AUD-022/024/030/031/033 propagate through prepared adapters. Remaining allocation and choice-conversion defects are above. Public `state::load_state` applies setters sequentially, so a later failure can leave an ordinary caller partly changed; production FFI restore now stages a candidate (AUD-023) and is protected. |
| `plugins-nih` | CLAP/VST3 exports; 43 default constructors; typed controls; 20 neutral EQ bands; runtime band discovery; negotiated scratch; asymmetric routing; fixed-frame checks; preset reconstruction across 32 structural families; structural fingerprint guard; latency reporting; async FIR-EQ wrapper | AUD-011/014/015/022 fixed substantial exposure/lifecycle issues. Source tests cover 10 asymmetric layouts over irregular blocks; real exported Gain tests cover steady attenuation, state and allocations. These do not establish all-plugin native automation/transport/tail contracts. Active structural restore requires reactivation and returns an error meanwhile, already documented under AUD-015. Findings 3–7 remain. |
| `plugins-ffi` | C ABI, panic containment, stable metadata strings, negotiated buffer checks, checked exact frame returns, latency query, transactional preset replacement, cached GUI reads and Swift writes | AUD-019/023 frame/state regressions pass in recorded checks. C strings are reclaimed by ParameterMap::drop; no leak is alleged. Findings 1–2 affect live controls. Actual AU/macOS execution remains separate from Linux Rust tests. C metadata lacks runtime kind and setup status. |
| `plugins-gpui` | Host-independent parameter UI trait; knobs/sliders/toggles/meters/transfer curves; design tokens/themes; cached 240-point logarithmic EQ response grid | `eq_curve_cache.rs:88-137` tests grid reuse and invalidation by caller signature. Rendering allocates and uses a mutex on the GUI thread; this is not an audio-thread defect. Band/channel actions have documented default no-op trait methods (`host.rs:59-83`), so host-specific behavior needs integration validation. No concrete new DSP defect found in this layer. Numerical preview/live-sample-rate equivalence and actual host gesture/state interaction are not established by cache tests. |
| `plugins-spatial` | Shared channel checks; immutable shared NUPC kernels; independent per-channel runtime state; progressive FFT levels; optional direct time-domain head | AUD-012 fixes direct-head latency. `src/nupc/tests.rs:123-211` includes independent direct-convolution/absolute-offset coverage, head-size timing and sharing. Earlier `test_nupc_vs_upc_simple` alone only asserts finite/nonzero output, but the newer oracle supplies substantive coverage. Tail lifecycle is owned by callers, not the helper. No new correctness defect confirmed in this pass. |
| `plugins-denoiser` | Lookahead transient detection/repair; warm bypass/linked channels; high-band hiss expansion; WOLA spectral hiss reduction; 48 kHz RNNoise buffering and linked stereo gain; in-place model reset | `rnnoise.rs:434+` compares against a reference backend, and `:813-858` checks reset storage/state. `spectral_hiss.rs:382,411,456` checks partition invariance, reset and unity reconstruction. Those are algorithm/implementation oracles; the RNNoise comparison is not independent speech-quality certification. Corpus-based intelligibility/perceptual quality across noise types is not established by these unit tests. No new source-proven defect in this pass. |
| `plugins-inference` | Bounded input queue; latest-only worker coalescing; panic recovery; generation invalidation; ownership-preserving try_send; borrowed with_latest; worker-side result destruction; prepared result mutex | AUD-020 addressed generic ownership claims. `src/lib.rs:97-153,500+` documents and tests ownership, contention and stale-result reset. Convenience send may destroy rejected input, and latest clones output; both are explicitly documented. Users must select the ownership-preserving APIs for allocated payloads. Reset invalidates generations; the generic interface does not promise to reset a recurrent model's internal history. No new defect confirmed. |
| `sotf-testkit` | Seeded signal generators; WAV/audio assertions; plugin/engine fixtures; control wait helpers; optional device/server helpers | `plugin/mod.rs:24-27` builds a position-zero context for every fixture call, so the helper cannot establish transport progress. `roundtrip_all_parameters` (`:42-66`) writes each readable current value back but never tests a nondefault or reads it back; unreadable controls are silently skipped. Treat it as setter acceptance coverage, not complete round-trip evidence. |
| `sotf-test-macros` | Explicit slow/network/hardware tags expand to standard ignored tests | `src/lib.rs:11-43` is straightforward tagging. Default aggregate success does not execute these tests; report ignored counts and platform runs separately. No new macro correctness defect confirmed. |
| `sotf-plugins` facade | Reexports DSPs/host and shared schemas; canonical catalog/aliases; engine factory; channel/maturity/evidence metadata; external hosting isolation/security | `src/factory/catalog.rs:28-86` separates evidence dimensions and rejects an incomplete Stable entry through catalog tests. This guards declared evidence, not the adequacy of each numerical threshold. Facade and bridge factories remain separate implementations; all-wrapper constructor tests protect the bridge but do not alone prove cross-factory nondefault preset equivalence. No new facade production defect confirmed in this pass. |

## Test coverage defect shared across the layers

`plugins-bridge/tests/cross_format_comparison.rs:153-195` creates two direct DSP instances, skips construction errors, ignores initialization/parameter/process errors with `.ok()`, then compares their output buffers. The default comparison never instantiates NIH or an AU binary. An ignored processing failure can leave matching buffers and pass; incompatible native normalization cannot be detected by comparing the same shared bridge implementation to itself.

Required improvement: fail on every unexpected constructor/setter/process/state error, assert produced frame counts, use deliberately nondefault values and meaningful input, and distinguish direct-bridge tests from actual exported-format integration. The fixed native Gain tests are valuable additional coverage, but they cover one family and do not justify full native-format parity.

## Primary contract sources

- Exact locked NIH source inspected locally: Cargo.lock pins `https://github.com/robbert-vdh/nih-plug.git` at `de421011f41a6d10fc8c7a6084e4f4dee0143683`; checkout `/home/pierre/.cargo/git/checkouts/nih-plug-d4210f82e920295f/de42101`. Relevant files: `src/plugin.rs`, `src/context/process.rs`, `src/params/range.rs`, `src/wrapper/clap/wrapper.rs`, `src/wrapper/vst3/wrapper.rs`. The raw GitHub fetch was unavailable; claims about this dependency use the inspected pinned local bytes.
- [CLAP event definitions](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/events.h), read 2026-09-27: event timestamps use sample offsets within the buffer.
- [CLAP tail extension](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/tail.h), read 2026-09-27: reports finite sample lengths or an infinite-tail sentinel.

This review proposes targeted correctness work, not generic competitive features. It does not reopen completed PDC, typed NIH defaults, structural reconstruction, transactional FFI restore, cold cache/spatial publication, or oversampling fixes. The unapproved unequal-branch queue integration remains unapplied and outside this support-layer scope.

## Implemented bridge follow-ups (AUD-041/042)

Prepared Arc-backed parameter IDs and allocation-free alias comparison now serve
normalized/raw scalar setters and getters. Fresh-thread C ABI ramps cover Gain,
Saturation, Limiter and EQ gain/frequency/Q with zero allocations. String choices
remain control-thread operations because their DSP values own Strings.

Choice conversion now uses canonical labels when the runtime DSP uses strings;
AAE's four room presets and DeEsser's two modes round-trip through raw and
normalized interfaces. Setup-only changes still return errors without changing
the active value. Numeric FilePath writes return an error instead of an empty path.
Expanded bands and runtime metadata retain integer/boolean types, including EQ
even orders. Expanded raw frequency conversion now uses the same logarithmic
curve as normalized access. No C ABI fields or parameter IDs changed.

Evidence: 39 bridge and 62 FFI library tests pass, including all previous
transactional state/frame-contract/cold-allocation regressions; both crates pass
all-target Clippy with warnings denied. Logs: `/tmp/sotf-ffi-typing-full.log` and
`/tmp/sotf-ffi-typing-clippy.log`. The original live AAE getter returned 0 instead
of 1/3 (`/tmp/sotf-ffi-choice-red.log`); the separate raw frequency regression also
failed before correction (`/tmp/sotf-ffi-raw-frequency-red.log`).

## Strict direct integration evidence (AUD-047)

The legacy `cross_format_comparison` target now identifies itself as direct
shared-bridge integration; it does not load a native format. Every constructor,
initialization, setter, getter, restore and process error is asserted. Audio
checks use explicit nondefault typed controls across eleven families, different
left/right signals, advancing transport, 64 callbacks, NaN destination sentinels,
exact returned frame counts and nonzero output energy. Every callback compares
within 1e-6 FS; failed/silent output cannot pass as equivalence. State restoration
checks every exposed readable value and continued DSP output.

These stricter tests exposed unchanged structural writes rejected by Limiter.
The shared bridge and generic state restore now retain unchanged structural
values without invoking setters. An in-flight limiter test verifies its queued
lookahead audio survives restoring unchanged state. Changed structural controls
still obey rebuild rules. The generic in-place load_state helper remains
nontransactional on changed-value setter errors, now explicitly documented;
transactional FFI restore is a separate candidate-replacement path.

Seven integration tests pass (`/tmp/sotf-bridge-evidence-final.log`). The broader
bridge/FFI/mute-solo suite passed before the final in-flight regression; logs are
`/tmp/sotf-bridge-evidence-full.log`. This is parameter-path consistency evidence,
not independent DSP correctness or full native-format parity.

## AUD-044 implementation checkpoint: native Gain event timing

Implemented only Gain's newly enabled NIH sample splitting. EQ/LinearPhaseEQ retain their previous flags; all other families remain unchanged. Actual CLAP one-event red at sample23 applied gain prematurely at frame0. Enabling the flag exposed pinned NIH's unconditional first-event application.

Vendored exactly NIH commit `de421011f41a6d10fc8c7a6084e4f4dee0143683` library and derive under `crates/3rdparties/nih-plug`, preserving the ISC LICENSE and upstream README. Root Cargo patch selects this local copy. Only two Rust behavioral changes: check first-event offset before application; advance seconds-only split transport without requiring tempo. README documents provenance and excluded upstream workspace packages. Existing unrelated lockfile updates preserved.

Independent native CLAP oracles cover sole events at0/1/23/126, equal-time last-wins order, multiple events, independent zero/10ms smoothing math, irregular partition equivalence, and seconds-only split positions across callbacks. Native processing stays under allocation assertions. No synthetic sentinel added to the sole-event fixture; no timestamp heuristic. Gain sole-event red: `/tmp/sotf-native-one-event-red.log`; focused green: `/tmp/sotf-native-one-event-green.log`; wrapper suite: `/tmp/sotf-native-automation-green.log`; Clippy: `/tmp/sotf-native-automation-clippy.log`.

Remaining: buffered/oversampled families need queued parameter application respecting retained audio; broader immediate-family allowlist still requires verified independent oracles. Gate exploratory fixture aborted under allocation detection, attribution pending; held source fixture is `/tmp/sotf-native-automation-candidates.rs`, outside the test suite. No Gate/Limiter enablement or tail status changes.

## AUD052 parametric in-place native precision

The `InPlacePlugin` implementation of `ParametricInPlacePluginAdapter` advertised
the inner native f64 capability while converting all input through f32. The
separate `Plugin::process_f64` path already delegated directly. The in-place
entry now dispatches to the native implementation when `supports_f64()` is true.
The previous reusable f32 fallback remains for plugins without native support.

A cold-thread probe subtracts unity from values differing by 2^-40 and values
above the f32 range. Zero, 1, 17 and 257 frame blocks preserve the independent
f64 result without allocation. Before the fix the probe reached its forbidden
f32 method. Logs: `/tmp/sotf-parametric-f64-red.log` and
`/tmp/sotf-parametric-f64-green.log`.
