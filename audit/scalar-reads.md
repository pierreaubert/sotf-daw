# Native scalar parameter allocation inventory — AUD-053

Date: 2026-09-28. Read-only inventory before the scoped getter correction. MIDI/IAMF excluded. Source reads use the current working tree; TokenSave graph was older than the latest edits, so line bodies were verified directly. No new allocation measurements were run for this inventory.

## Confirmed callback route

`plugins-nih/src/params.rs:140` (`DynamicParams::sync_to_plugin`) reads every realtime parameter on every callback, including unchanged values, then calls the setter only if the typed value differs. The generated native process method invokes this synchronization before rendering. `sotf-host/src/parametric_in_place_plugin.rs:204` and `parametric_plugin.rs:208` implement the default scalar getter as `current_values().get(id).cloned()`. The snapshot is a fresh HashMap. Adapter forwarding therefore creates and destroys the complete snapshot for every scalar read. An inherent `get_parameter` method does not replace the trait's differently named `parametric_get_parameter` hook.

## Remaining affected native classes

All paths below are relative to `crates/sotf-plugins/crates/`. Each row lacks the trait scalar getter override and has an allocating snapshot body. These are 12 DSP classes, exposed through 14 native plugin exports because Compressor/MultibandCompressor and Expander/MultibandExpander share implementations.

| Native export(s) | Snapshot source | Additional live setter evidence |
|---|---|---|
| Compressor, MultibandCompressor | `sotf-plugin-multiband-compressor/src/lib/multiband_compressor_plugin.rs:1334` | At 1311 only Range/Hold IDs bypass the allocating default validator + singleton map. Existing inherent getter at 1200 can be forwarded but dynamic band parsing must be checked. |
| Expander, MultibandExpander | `sotf-plugin-multiband-expander/src/lib/multiband_expander_plugin.rs:1493` | At 1477 validates borrowed metadata and delegates to inherent scalar setter. Getter forwarding still needs dynamic-ID allocation inspection. |
| Crossfeed | `sotf-plugin-crossfeed/src/lib/crossfeed_plugin.rs:630` | Scalar setter at 769 validates cached metadata, clones a candidate config, updates selected DSP state. Getter correction alone does not certify setter reset/filter paths. |
| DeEsser | `sotf-plugin-de-esser/src/lib/de_esser_plugin.rs:543` | At 599 uses borrowed metadata and direct application; structural frequency/Q/mode excluded from native callback sync. Snapshot builds mode String even for numeric scalar reads. |
| Declick | `sotf-plugin-declick/src/lib.rs:140` | At 187 direct param_bridge setter plus cached-value refresh. |
| Denoiser | `sotf-plugin-denoiser/src/lib/denoiser_plugin.rs:866` | At 943 explicitly builds singleton ParameterSet on every update. |
| HissReducer | `sotf-plugin-hiss-reducer/src/lib.rs:274` | At 364 validates cached metadata and directly applies. |
| SpectralCompressor | `sotf-plugin-spectral-compressor/src/lib/spectral_compressor_plugin.rs:602` | At 677 validates cached metadata then direct application. Existing inherent getter at 485 can be forwarded. |
| ChannelMuteSolo | `sotf-plugin-channel-mute-solo/src/lib/channel_mute_solo_plugin.rs:553` | Default setter constructs singleton map; snapshot also serializes channel state JSON. Override validator avoids metadata refresh but does not fix the getter/map. |
| SpeechDenoiser | `sotf-plugin-speech-denoiser/src/lib.rs:155` | Default validator clones schema; default setter constructs singleton map. Even the single Enabled getter builds a map. |
| StereoImager | `sotf-plugin-stereo-imager/src/lib/stereo_imager_plugin.rs:275` | Default validator/schema clone and singleton map on updates. |
| TransientShaper | `sotf-plugin-transient-shaper/src/lib/transient_shaper_plugin.rs:266` | Default validator/schema clone and singleton map on updates. |

Unknown-ID reads also build and discard a snapshot under the default implementation. This affects debug/native configurations with `assert_process_allocs`: an otherwise unchanged callback can abort when the allocation guard detects these maps. Release builds without the guard still allocate/free in the callback.

## Exceptions and scope boundaries

- LinearPhaseEQ also has the default snapshot getter, but the native wrapper explicitly wraps it in `AsyncTimelinePlugin` (`plugins-nih/src/wrapper.rs:253`). Its callback getter reads prepared `metadata.values` (`sotf-host/src/async_timeline_plugin.rs:658`); primitive realtime values clone without heap work, while the inner setter/snapshot work belongs to the worker. Do not add it to the direct native allocation list.
- AnalogCompressor, AnalogEq, and AnalogLimiter have the same default getter pattern but are not currently exported in `plugins-nih/src/lib.rs`. They remain separately actionable host/bridge work.
- Gain, Gate, EQ, Delay, Convolution, DynamicEQ, Limiter, Dither, LoudnessCompensation and Saturation already have trait getter overrides. This is an inventory statement, not complete process/setter certification. Gate, Delay and Convolution had cold allocation reproductions and fixes in the preceding audit wave.
- Direct `Plugin` families use explicit scalar getters: AAE, AEC, Ambisonics, ABCompare, BandSplit/BandMerge, Beamformer, Binaural, Crossover, Downmix, Matrix, MonoToStereo, PND, Upmixer and XTC. Some nonnumeric getters clone strings or serialize configuration. Their native structural/file/JSON controls are normally excluded from realtime synchronization; each control must follow metadata when expanding native exposure.
- `AsyncTimelinePlugin` metadata snapshots and public full `parameters()`/`current_values()` APIs may still allocate on the control thread. No blanket interface redesign is needed for this correction.

## Scoped fix authorized by parent

Add scalar trait getter overrides for the 12 affected exported classes. Preserve existing IDs, values, value variants, choice indices, aliases and unknown-ID None; include expanded multiband IDs. Compare against independent prepared snapshots and explicit nondefault values; measure a fresh thread's first native synchronization with no warmup. Keep allocating live setters separately tracked; do not hide them by setting unchanged values or broadening automation flags. No schema, preset, DSP response, latency, tail or native export changes are intended.

## Analog primitive getter correction

Analog Compressor, EQ and Limiter now read scalar fields directly; Limiter
forwards its declared core keys to the already direct core getter. Model-name
String values preserve their owned control-query behavior. No numeric type,
current value, parameter schema or live setter behavior changes.

Fresh-thread primitive reads plus unknown-ID lookup previously allocated
285/285/143 times respectively. All now allocate zero and match full default and
nondefault snapshots, including core limiter booleans. Model strings are checked
outside the allocation guard. All 43 unit/integration tests across the three
crates pass; all-target warnings-denied Clippy passes. Logs:
`/tmp/sotf-analog-getters-{red,green,clippy}.log`.

The trait docs now make explicit that the default scalar getter builds an
allocating full snapshot and must be overridden for callback queries.

## Exported family getter checkpoint

Direct getter overrides are implemented for all twelve listed DSP classes.
The fourteen native synchronization cases pass on cold threads with default and
nondefault snapshots, expanded bands, unknown IDs, and per-channel flags. The
750 tests across affected DSP crates and focused all-target Clippy pass. Owned
String/JSON queries retain their control-thread allocation contract.

An independent native GUI-state test reproduced a separate 48-byte allocation
inside NIH's zero-capacity response send; this is AUD057, not a scalar getter
failure. Its allocator guard remains enabled while response ownership is fixed.

## Scalar write follow-up (AUD060): Compressor

Compressor and MultibandCompressor now validate against borrowed prepared metadata
and invoke the existing scalar DSP setter directly. Range/Hold keep their existing
validation path; schema contents, error strings, smoothing and bulk restore remain
unchanged. The cold single-band sequence previously performed 1,180 allocations
or deallocations; it now performs zero. Mono-band, three-band and five-band tests
cover global/expanded controls, irregular callbacks and exact running-history
parity against the inherent setter. A separate static gain equation and rejected
value/snapshot checks pass. All 137 crate tests and all-target Clippy pass. Logs:
`/tmp/sotf-compressor-setter-{red,green,clippy}.log`.

## Scalar write follow-up (AUD060): Denoiser

Denoiser shares a borrowed value-update helper between bulk and scalar entry
points. Existing coefficient, smoother, MCRA and profile-trigger side effects
are unchanged; the three-value frequency kernel was already fixed storage.
All 29 cached primitive values refresh in place so Clear Profile also updates
Learn and Use Profile. Prepared metadata and trigger update policies are retained.
Borrowed validation preserves its original strict schema behavior; the scalar
setter retains param_bridge's finite clamping/coercion behavior.

The first full scalar sequence previously performed 14,391 allocations or
deallocations. It now performs zero across both FFT sizes and both multi-resolution
configurations, including 27 scalar controls/triggers, validation and irregular
processing. All 69 crate tests and all-target Clippy pass. Logs:
`/tmp/sotf-denoiser-setter-{red,green,clippy}.log`.
