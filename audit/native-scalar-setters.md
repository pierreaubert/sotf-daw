# AUD-060 scalar setters and AUD-061 RNNoise buffering

Date: 2026-09-28. Parent owns the main AUDIT.md ledger. This report covers the plugin_chain agent's four-family setters, shared speech backend correction, and native parameter regression matrix. Root separately implemented Compressor/MultibandCompressor and Denoiser scalar setters; their changes are exercised by this native matrix.

## AUD-060: changed native controls without temporary maps

The four plugin classes inherited the trait scalar setter, which cloned a schema for validation (except ChannelMuteSolo), allocated a singleton parameter map, and destroyed that map on the render thread. The isolated pre-fix SpeechDenoiser native synchronization test aborted on a 192-byte allocation: `/tmp/sotf-scalar-setter-red.log`.

Implemented borrowed descriptor validation and direct primitive mutation:

- `sotf-plugin-speech-denoiser/src/lib.rs:111,153,168`: one Enabled mutation/cache helper shared by the borrowed bulk API and scalar setter.
- `sotf-plugin-channel-mute-solo/src/lib/channel_mute_solo_plugin.rs:427,434,558`: existing per-channel flags, enable, dim and fade mutations moved to one borrowed helper. Existing validator reads immutable cached type/range metadata without refreshing the dirty JSON/schema cache.
- `sotf-plugin-transient-shaper/src/lib/transient_shaper_plugin.rs:243,311,326`: shared scalar mutation/cache helper, preserving target smoothing and existing bulk semantics.
- `sotf-plugin-stereo-imager/src/lib/stereo_imager_plugin.rs:251,363,378`: existing two-phase mutation accepts a cloneable borrowed iterator; bulk maps and a single scalar share joint prospective crossover checks and commit only after validation succeeds.

No IDs, parameter kinds/ranges, saved-state representation, automation allowlist, latency, or tail policy changed. ChannelMuteSolo JSON controls and error-string construction remain control/error operations. The scalar APIs take owned ParameterId values; realtime callers retain their ID and pass an Arc clone, as DynamicParams already does. Transferring the last owner necessarily frees the ID on return; this is not an allocation-free ownership guarantee for arbitrary caller destruction.

### Independent native evidence

`plugins-nih/src/params_scalar_setter_tests.rs` adds 12 tests:

1. Four first-changed-sync tests create and initialize default DSP on control, then move it to a fresh thread. The first synchronization applies previously unapplied changed values inside `assert_no_alloc`, which checks allocations and deallocations. Repeated synchronization, process, scalar readback and control-side full snapshots are also checked.
2. Independent waveform references cover disabled SpeechDenoiser as original input delayed 480 frames; zero-fade mute/dim arithmetic; neutral TransientShaper trim as a 10 ms one-pole gain; neutral-band StereoImager width as explicit M/S with a 10 ms transition. Irregular blocks repeat [1,17,63,256,3,127].
3. All exported realtime primitive controls are changed together in ChannelMuteSolo, TransientShaper, StereoImager, Compressor, MultibandCompressor and Denoiser. These six tests check cold synchronization, exact typed readback/snapshots, repeated process and finite output. The compressor/denoiser waveform accuracy evidence remains in their dedicated root-owned DSP tests; this matrix tests native dispatch.
4. Wrong types, NaN, infinities, out-of-range scalars and unknown IDs leave all existing values unchanged in the four scoped families.
5. Cold 4097-frame Speech callbacks cover mono/stereo and enabled/bypassed states under allocation/deallocation guards; guard sentinels and fixed latency remain intact.

## AUD-061: RNNoise variable/large callback corruption

The independent disabled-Speech oracle first failed at frame 3606: actual -0.024 versus expected -0.068 for delayed input. Two concrete causes in `plugins-denoiser/src/rnnoise.rs`:

- Old normalization set read position to zero and write position to queue length without rotating ring storage. If the consumed position was not an integer ring turn, both wet and dry samples changed modulo locations.
- Old process queued the entire incoming callback before reading any output. More than the 1920-frame ring capacity could overwrite unread samples.

Correction at lines 155-164 and 283-289:

- Process a validated callback in chunks of at most one 480-frame model period, consuming output before the next chunk can overwrite pending data. The prepared ring/scratch/model remain unchanged.
- Subtract the same complete ring-size multiple from both cursors. Logical distance and physical modulo positions both remain unchanged.

A callback chunk begins with at most 480 queued frames and produces at most one additional 480-frame model output before consumption, below the existing 1920-frame ring capacity. Common-multiple rebasing keeps counters bounded without moving samples or allocating. The fixed 480-frame delay, model frame sequence, linked stereo policy and bypass ramp remain unchanged.

Backend test `rnnoise::tests::irregular_and_large_blocks_preserve_delayed_dry_and_wet_samples` processes 32,769 frames for mono/stereo, wet/dry, with [1,17,63,256,3,127,4097] chunks. Output matches 480-frame partition processing exactly; bypass additionally matches the original delayed input exactly, and oversized buffer guard samples are untouched. The native cold large callback test separately verifies zero heap operations.

## Verification checkpoint

- Full four-family DSP suites plus plugins-denoiser: **261 passed, 0 failed, 0 ignored**, 18 result groups. `/tmp/sotf-scalar-setter-dsp.log`.
- Full NIH library: **81 passed, 0 failed, 0 ignored**. `/tmp/sotf-scalar-setter-nih-full.log`.
- All-target Clippy with warnings denied for NIH, shared denoiser and four DSP families: passed. `/tmp/sotf-scalar-setter-clippy.log`. The pinned dependency's preexisting unused macro import is emitted as a capped dependency warning, not a workspace target error.
- `git diff --check`: clean; all edited Rust files formatted.

### Test-fixture failure attribution

The initial large-callback guard also found a 24-byte deallocation because its direct setter moved the fixture's sole ParameterId owner. GDB stack in `/tmp/sotf-setter-speech-large-backtrace.log` frames 13-16 identifies `AllocDisabler::dealloc -> SpeechDenoiserPlugin::parametric_set_parameter`. Native DynamicParams retains IDs. The fixture now clones that retained ID inside the guard; no production warmup, allocator bypass or permit_alloc was introduced. Its first draft also passed an oversized output slice to the exact-length Plugin adapter; that slice was corrected while backend tests continue to exercise the backend's documented oversized-buffer acceptance.

## Scope limits

This fixes valid primitive scalar changes and RNNoise queue continuity. It does not broaden native sample-accurate automation flags, add speech drain/tail promises, make string/JSON mutation realtime-safe, or claim arbitrary ownership destruction is realtime-safe. Parent/root handles the aggregate workspace gate.
