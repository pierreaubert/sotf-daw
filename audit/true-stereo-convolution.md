# AUD-134: true-stereo convolution

**Status:** Astra formally accepted AUD-134 in the documented scope against
this final report. Implementation, consuming routes, and scoped tests pass.

## Confirmed gap and bounded behavior

The convolution crate accepted multi-channel impulse responses (IRs), but stereo
processing mapped each output channel to only one IR channel. Four-channel IRs
therefore could not provide true-stereo crossfeed.

The feature is an explicit structural `true_stereo` option, defaulting to
`false`. In the default mode, existing stereo behavior is preserved: a legacy
four-channel IR uses channels 0 and 1 and ignores channels 2 and 3. Enabled
mode requires two plugin channels and exactly four IR channels and uses a
source-major LL/LR/RL/RR matrix. The option is carried through plugin schema,
facade JSON factory, `PluginSettings`, structural parameter mapping, serde
preset, engine converter, bridge factory, and C ABI state reconstruction. Raw
scalar automation rejects structural mode changes; control-thread restore
constructs a replacement before publishing it. The proposal is
[`true-stereo-convolution.md`](proposals/true-stereo-convolution.md).

The Convolution custom renderer mounts its layout-declared Advanced controls.
The mounted test uses actual GPUI mouse-down/up events at the tracked painted
“On” choice; it verifies the selected state and `PluginSettings`, then observes
a queued Structural update before advancing the player timer. It saves and
loads the graph through the disk `PluginController` preset path and restores the
option into a fresh `PluginState`. This proves the mounted settings and preset
route. The fixture does not construct a physical Player processing engine;
the separate engine, bridge, and FFI tests exercise audible DSP routes.

## Pre-edit baseline

- Clean pre-edit HEAD: `93027970f412ce47c0cd2b8e4b7b1a5b0e5f0261`.
- Full source snapshot:
  `crates/sotf-plugins/target/audit-artifacts/aud134-preedit-9302797/source-snapshot/`.
- Source plus lock manifest: `/tmp/sotf-aud134-preedit-source.sha256`, aggregate
  `50b447849c8b8145fa33b7ad9bc4600aa4b433eaa174138884e242d5f93fffe1`.
- Two-channel stereo outputs:
  `crates/sotf-plugins/target/audit-artifacts/aud134-preedit-9302797/render-baseline/`.
- Legacy four-channel stereo outputs:
  `crates/sotf-plugins/target/audit-artifacts/aud134-preedit-9302797/legacy-four-channel-baseline/`.
- Capture logs: `/tmp/sotf-aud134-preedit-capture.log` (SHA-256
  `684007e2614032406b72ce3d383ebbffab5b7fd11f1fc008e6e3f4ac014a02ea`) and
  `/tmp/sotf-aud134-preedit-fourch-capture.log` (SHA-256
  `f938516dd9b9af08c83c0b68cc91b6faf5aa1c10992cfd1e16327e3aebf31a50`).
- The six archived arrays were replayed by the final source and compared against
  their frozen pre-edit values; the test passed 1/1, including old four-channel
  behavior. Log `/tmp/sotf-aud134-preedit-array-replay.log`, SHA-256
  `e2fc7df69074fb8c4f49e898aab592d68e7e3b7ffa31e8eea2101f5f294db4bb`.

## DSP, lifecycle, and storage evidence

The convolution package gate passed 55 library, 11 direct, 7 finite-stream,
and 10 integration tests; 3 manual capture/replay tests are ignored by the
ordinary run. The independent full-vector f64 oracle covers all four isolated
paths and their matrix sum across one-frame, 1024-frame, and irregular
partitions. Its asserted error bounds are peak <= 1e-5 and RMS <= 1e-6; these
are tolerances, not reported observed maxima. The tests include explicit
latency, finite EOS, replacement, and reset behavior.
The heap guards cover processing and complete drains, including direct-head and
later NUPC levels. Failed live replacement preserves output/history; reset is
compared with a fresh instance. Rubato is used as independent convolution
plumbing but is the same resampling library, not an independent resampler
algorithm.

The checked estimator accounts for spectra, frequency-delay lines, level and
plugin-fixed buffers, head history, queues, delay, scratch, and plan allowance.
Its boundary test covers each public head length 32–512 and no-head NUPC. The
retained-storage test measures requested-byte delta while the plugin remains
alive; it does not claim peak RSS, allocator bookkeeping, or temporary
load/resample allocation volume. The FFT-plan reserve is empirically validated
for locked RustFFT 6.4.1 on the tested target and is not a portable theorem;
planner/backend or target changes require revalidation.

Verification logs (all command invocations exited successfully):

- Package tests: `/tmp/sotf-aud134-convolution-package-final2.log` (SHA-256
  `81b6a63d41996fb6ca7304e828a0e43add5ef0a20b11c8f9c51675ff6253093f`).
- Strict convolution Clippy:
  `/tmp/sotf-aud134-convolution-clippy-final2.log` (SHA-256
  `6c580422b5e998a49eedf6288fbba35ab75828a22c4c53ec380c5148b5e20b35`).
- Strict facade all-target Clippy:
  `/tmp/sotf-aud134-sotf-plugins-clippy-final2.log` (SHA-256
  `83b0f7ba698dc7a74686d4ab4dab21e9b0ab959e913acdab1a476ea6cf4d6ab0`).
- Snapshot update check: `/tmp/sotf-aud134-convolution-snapshots-update.log`
  (SHA-256 `1dbbf12be5e4e5bdabef7c61c2ca1aa553fb23c82840ab08422fb0610be9a3db`).

## Consuming-route evidence

Focused routes passed:

- Engine settings, serialized preset, and audible factory path: 1/1 in
  `/tmp/sotf-aud134-engine-route.log` (SHA-256
  `8b945633da9af77353e9f2da14c262f91b734979318adf4a79d50b50b8e06341`).
- Facade factory JSON reconstruction, mode validation, scalar-setter rejection,
  and neutral no-IR delayed dry path: 1/1 in
  `/tmp/sotf-aud134-facade-route.log` (SHA-256
  `a73d5203ded4136e3c983a70c8ab1cf7bfef9245b7d408b0e80cd0c29a41f49f`). The
  audible loaded four-path route is established separately by the engine
  route and direct matrix tests.
- Bridge JSON to audible route and strict Boolean validation: 1/1 in
  `/tmp/sotf-aud134-bridge-route-final.log` (SHA-256
  `4b1155209c817aa2bb22905cc1ff55f5fdf6dabb763762663e0cd66dbe745b07`).
- FFI state/preset reconstruction, partial restore, invalid/incompatible restore
  preserving live history, and scalar structural-setter rejection: 1/1 in
  `/tmp/sotf-aud134-ffi-reconstruction-final.log` (SHA-256
  `31ea42c9345cc845d3fb84d5617a7b7196fe8e2298f16f1c54b1de2f091a66c1`). The
  prior partial-restore regression also passed 1/1 in
  `/tmp/sotf-aud134-ffi-partial-restore-final.log` (SHA-256
  `4fd17a981136eb2c0c76c68208c85efd33239b1456aba4973aa35b1b196ced26`).
- Strict FFI all-target Clippy:
  `/tmp/sotf-aud134-ffi-clippy-final.log` (SHA-256
  `f6e0cf7d53e311e8e5e9767694e6d54da70101046ce481a672f95b88804e4de2`).

The final offline locked DAW workspace nextest run excluded `sotf-midi` and
`sotf-iamf`: 6,100 passed, 19 skipped, in 272.300 seconds. The 2,676-file
start/end source manifests match at aggregate
`85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`; log
`/tmp/sotf-aud133-134-workspace-rerun.log` has SHA-256
`4c417bab556373409374329b1de38b30abab67cdbafc36908037eb23bb4dd449`. DAW
`Cargo.lock` SHA-256: `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

## Mounted Studio route and tested sibling snapshot

The mounted E2E passed 1/1 with the retained reviewed sibling lock. It clicks
the actual painted GPUI control and verifies the Structural request before the
player timer runs, followed by disk preset save/load and fresh model
reconstruction. Log `/tmp/sotf-aud134-mounted-gpui-native-click.log` has
SHA-256 `8f1d03844908c5addb6e87bddfb48bee5b2d54384a9e4acd330c393f22db6b73`.
The tested sibling `Cargo.lock` SHA-256 is
`475d5890fddf40c8ca8361c6455057c1f3701c713c5844f2cdfda5a53303fb4e`. Its
15-file changed-source manifest is
`/tmp/sotf-aud134-mounted-gpui-native-click-source.sha256`, manifest SHA-256
`93ae7c68c2b75c5e5454eb72d34e2c71ed294eebbdd8042c7cd253c1e2516bc4`.

The native NIH plugin route remains a separate open gap: `DynamicParams` skips
file-path parameters, hides non-realtime structural booleans, and the wrapper
has no custom editor. Thus native users do not yet have an established
user-accessible true-stereo setup, saved IR resource restoration, or loaded
four-path native audio route. The bridge and FFI routes above do not close that
NIH host gap. No MIDI/IAMF implementation was part of AUD-134. No physical
playback-device rebuild is claimed for the mounted model fixture. The broader
feature audit remains open for other independent issues.
