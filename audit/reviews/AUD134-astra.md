# AUD134 Astra design review

Status: **ACCEPTED for explicit default-false true-stereo convolution, reviewed
DSP/storage/lifecycle, consuming routes and mounted model-control scope**.

### Final acceptance

Final consolidated report SHA-256
`994f38651d3fb0e24689afe4b21f6f8a5ee4e40a0fcbcb48c2645218b1a24cac`
records the 83 package tests, strict lint, six archived-array replay,
engine/facade/bridge/FFI routes, preserved partial restore, workspace 6100/19
and actual mounted GPUI click. All 15 entries in the final sibling source/lock
manifest verify against current files; manifest SHA-256
`93ae7c68c2b75c5e5454eb72d34e2c71ed294eebbdd8042c7cd253c1e2516bc4`.
The retained sibling lock is `475d5890…`, matching the mounted test resolver.

The final documentation corrections are verified and closed: the
standalone facade test proves neutral prehydration and validation, while loaded
four-path facade audio is supplied by the separate engine-to-facade test; native
NIH loaded-resource/control reachability is not established by bridge coverage.
Oracle tolerances are stated as asserted bounds, not observed maxima. Formal
scoped acceptance is complete; no documentation condition remains.

No remaining finding in this bounded batch. Acceptance retains the stated
limitations: planner allowance is empirically validated for the locked backend
and target, retained allocation requests are not peak/RSS, the resampling
reference shares Rubato, and the mounted fixture proves settings/queued rebuild/
preset behavior rather than physical Player processing. The wider audit,
separate native higher-order routes and corpus access hold remain open.
No additional Cargo run was needed for final report verification.

### Current findings and gate state

Final mounted UI execution closes the reachability/classification finding:
`/tmp/sotf-aud134-mounted-gpui-native-click.log` passes the actual named test
1/1, SHA-256 `8f1d03844908c5addb6e87bddfb48bee5b2d54384a9e4acd330c393f22db6b73`.
It dispatches real GPUI mouse-down/up at the painted On choice, observes changed
selection/settings and a newly queued Structural update before timer delivery,
then saves/loads a disk preset and reconstructs graph settings. The mapper
regression passes 1/1. This is layered model/control evidence; a physical Player
DSP instance is not rebuilt by this UI fixture. Separate factory/FFI audio tests
establish processor reconstruction.

Final sibling source hashes: mapper
`1c126faeceb87b0e25758196d5134304f1c23a02679011231dce692b34fd4428`,
custom renderer `0d77f0355f22c73afab797bc3d9065e259e3585483fc179cc205627d832ca07d`,
layout renderer `aab47393374a667a311a13ee28d4ba282094112dbb819061face78e97eb137ef`,
E2E `90de79bd05254a4ae08a77247ec0c3f532675a275203f51e0d96f595d3a187ca`.
Retained sibling resolver is the reviewed `475d5890…` lock.

The final async replacement/reset test meaningfully compares the adopted new
IR against fresh state after the deliberate transition interval, and populated
post-EOS reset replays exactly. Saved pre-edit arrays are compared byte-for-byte
for both two-channel and legacy four-channel IRs across all three backends.
Coordinated workspace result is now 6100 passed, 19 skipped (same exact gate
and provenance recorded in AUD133); the preserved partial-restore regression
also has a focused passing result. No remaining source finding; final report
must consolidate these results and qualifications without claiming physical
playback E2E or a portable mathematical FFT-plan bound.

Earlier P1/P2 descriptions below are historical review iterations, superseded
by the dated-by-hash milestone dispositions. No current blocking DSP/estimator
source finding remains. Corrected FFI replacement SHA
`0800016a0ce63c51e4b7b0a4fecffaa8eda5ce4eb605e7297a0e9c3620127328`
preserves original constructor-config bytes when mode is unchanged and stages
an incoming IR before constructing a changed mode. Test SHA
`2f715d547af9f4dc3ed905b6eaf3431df8998f69636e1876aaa69db7fff80e56`
verifies raw/preset restoration from a two-channel legacy IR into a four-channel
matrix IR; incompatible input now explicitly supplies an incompatible IR. The
existing partial-restore assertion was not weakened. Named reconstruction gate
`/tmp/sotf-aud134-ffi-reconstruction-final.log` passes 1/1; the older partial
restore test still needs the next coordinated/focused result because that named
filter does not select it.

Verified `/tmp/sotf-aud134-convolution-package-final2.log`: 55 library,
11 direct, 7 finite-stream and 10 integration tests passed, 3 capture/replay
tests ignored. Strict Clippy final2 passes. The separately executed actual
pre-edit array replay passes 1/1 in `/tmp/sotf-aud134-preedit-array-replay.log`.
The historical broad run (6093 passed, 7 failed, 19 skipped) is not a green gate.
Mounted UI execution, final lifecycle source handoff/report and corrected broad
gate remain open; no duplicate Cargo run was performed by the reviewer.

## Frozen DSP milestone disposition

**Scoped DSP/storage findings closed for the tested backend; overall acceptance
pending integration and final package evidence.** Inspected exact SHA-256:

- `convolution_plugin.rs`: `f5bcb17d5ef6e617609da8f29f9aba0290a6a7b9ba6b64262ce300d399551946`
- `src/lib/tests/finite_stream.rs`: `b9af9be8183ed53382d2f949f81f9ad7e948bcfea98bdb472dd676a7b3b17780`
- `tests/direct_convolution.rs`: `1584c7977450e00d60acd0efe0423940f3833de8f3af5fd8d3d3d6fa1575ad38`
- `Cargo.lock`: `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`

`/tmp/sotf-aud134-true-stereo-expanded.log` records 4 unit and 5 direct tests
passing. Geometry accounts for four spectra/FDL paths, level buffers, output
delays, head state and plugin-fixed storage before FFT planning. Every integer
head 32–512 and ordinary no-head NUPC now reaches the actual estimator budget
boundary, with rejected upper and next endpoints. The derived FFT families are
measured against the plan/scratch allowance. This is empirical validation of
locked RustFFT 6.4.1 on the compiled target, not a universal recursive planner
proof. The report states this qualification and requires revalidation after
target/planner changes. Retained requested-byte delta is measured with the
plugin alive and an explicit nonnegative check; temporary preparation peaks,
allocator bookkeeping and RSS are not established by this metric.

The independent f64 matrix oracle now checks complete output at peak <=1e-5
and RMS <=1e-6 across one-frame, 1024-frame and irregular callbacks, explicit
1024/0 latency and exact finite EOS counts. Isolated LL/LR/RL/RR paths, live
invalid-replacement bit equality and complete multi-level process/drain heap
guards close the specific earlier evidence gaps. Resampling uses independent
plumbing with the same Rubato library and is not an independent resampler.

No new production defect found. The filtered run does not establish saved
pre-edit two/four-channel baseline replay, populated reset-versus-fresh behavior,
or successful/asynchronous replacement. These remain final lifecycle evidence
items alongside full package/lint and engine/bridge/FFI/mounted UI routes. No
reviewer Cargo run and no request to rerun this snapshot for documentation edits.

## Incremental implementation review

### Consuming-route milestone

Mounted UI source follow-up now calls `render_tabs_from_layout` from the real
custom Convolution renderer. The test observes the mounted Advanced tab/control,
routes its click into settings and asserts a pending Structural update; real
PluginController disk save/load and a fresh PluginState preserve routing.
This is meaningful layered evidence pending execution. `Player::new()` has no
processing engine here: fresh PluginState reconstruction is graph/settings
reconstruction, not physical Player DSP execution. Separate factory/engine and
FFI tests provide the audio evidence. Do not describe this fixture as a complete
device playback rebuild. No additional source blocker found.

Inspected sibling renderer SHA-256 `0d77f0355f22c73afab797bc3d9065e259e3585483fc179cc205627d832ca07d`,
tab renderer `b1cd065f7986f4961b6c8027f5c955cbee440b959a06f65650b8ec31354f5280`,
E2E source `da3bec026a67d3a9cd20b5a6f532b2320554fe89ac8ce00fc558e77575c65f51`.
FFI test revision `1492de0df4842e7dbe668226a1b5493d33f6e4db8c6f30842722ab5e7e70e659`
adds the requested actual scalar-setter rejection and live-history preservation
assertions; its final executed handoff remains to be recorded.

Engine settings/accessors/converter and facade/bridge routing are coherent.
The FFI replacement builds a separate instance using the requested Boolean
mode, removes routing from ordinary state replay, preserves current parameters
for partial imports, then publishes plugin and constructor config only after
successful preparation and layout checks. No source defect found in this pass.

Verified named tests in `/tmp/sotf-aud134-engine-route.log`,
`/tmp/sotf-aud134-facade-route.log`, the final run of
`/tmp/sotf-aud134-bridge-route-final.log`, and
`/tmp/sotf-aud134-ffi-route.log`: each relevant test passed. FFI raw-state and
preset-document coverage includes false-to-true-to-false-to-true audio, partial
mode preserving IR/mix, invalid-type and incompatible-IR rejection preserving
the installed instance/config and its live convolution history. Add the agreed
actual C ABI scalar-setter rejection assertion to distinguish unsupported live
automation from valid control-thread reconstruction; production already rejects
structural setters. Full package and mounted UI evidence remain pending.

Additional inspected SHA-256 values (engine hashes above remain unchanged):

- Engine route test: `b5ded4ea9a7d7719395976d379ecaffd0f56670b53be438bd2f2d6b237be5b3b`
- Facade factory: `8d6c440895c4ce2d254fd42b2c07be7cd3aeff300797600c92eee9d88696d975`
- Facade tests: `4ca17511879d89e892027438afabc428e579cec806ac28c5dec6966cb6942507`
- Bridge factory/tests: `8fc13da7cb29624c7cfd0ebf134d1c302c077a7398f2ca80b04b0b947a273154`
- FFI replacement: `c29e0ec6f023ac938b29b5735182125fe078ef5fff17351c6c354cb5b02551bf`
- FFI state tests: `735497bcb7947ceb8c05c872d03cb24dec8e270961af0c23ed285340352bc32c`

Stable DSP source inspected while the owner edits only engine integration.
UPC now fills both source FDL lanes before summing; its source-major indexing
matches the proposal. NUPC paths 0/2 feed left and 1/3 feed right. Structural
runtime changes reject, failed synchronous preparation precedes installation,
and reset visits all prepared engines. No routing defect found in this pass.

**P1, open:** `estimated_ir_backend_bytes` still estimates matrix NUPC as
`sum(path_lengths) * 4 * 4` bytes. Each path actually retains complex IR
partitions and matching complex FDL, each with FFT length twice the partition
block length. These alone require at least 32 bytes per IR sample, before
padding, accumulators, queues, output delays, scratch, head history and plans.
The new boundary test accepting the nominal 512 MiB estimate therefore does
not prove the stated memory limit. Require a checked geometry-based bound and
capacity-backed tests; do not allocate a huge fixture merely to demonstrate it.

**P2, open evidence:** current matrix oracle uses one irregular callback
sequence, derives latency from the implementation, and resets after failed
replacement before rendering. Add the agreed one-frame/full/multiple-partition
matrix, four isolated paths, explicit 1024/0 latency, live failed/successful
replacement (including async route), and reset-versus-fresh evidence. Current
heap fixture processes only 64 frames and the first drain; cover complete EOS,
later levels and direct-head mode under allocation/deallocation guards.

**P1, open accessible UI:** confirmed root's finding against current sibling
bytes: `custom_view_registry/render.rs` ends `render_convolution` with only
`render_main_controls`; `render_main_controls_from_layout` calls
`render_main_column` with tabs disabled. Consequently the declared Advanced
toggle is not mounted. Require the authorized minimal live renderer correction
and a real panel click that reaches settings, structural reconstruction and
preset persistence. The schema-toggle unit assertion is supplemental only.

No reviewer Cargo run; these findings do not freeze other workers or shared
engine integration. Final consuming-route and exact-source gates remain pending.

### Estimator and engine integration follow-up snapshot

Read-only source hashes during this pass:

- Convolution implementation: `1cae2c0553f710b2ff0a96adf37d21ea8bd485ad5a80ca486ee7662b84036cdf`
- Engine settings: `f5cf74f59ddaf7d078ba483c09c39b35313e0cc26fec67c963977f1f56be5382`
- Accessors: `160a9c81df0ac4872ff98e66a2e20cbdfc40c7b56e527ba8f3889d71c5e75fb7`
- Effects converter: `c022fee0dfa973cb9ba66cd324a61b2ccf58f9e9ad1520f3938399a1e54158b7`
- Engine route test: `a08348de2dffe01c3153336be312bbbc72d659e3285225523c6ce8065b26d7bf`

Engine serde-default, indexed accessor, structural mapping and converter now
propagate the flag. The new route test persists settings and checks actual
cross-path audio through the facade factory; this is appropriate evidence for
that layer, pending execution and mounted UI evidence.

The revised memory estimator counts partition geometry and per-level arrays,
closing the original spectra/FDL omission in principle. It is not accepted yet:
the current source appears pre-compilation, NUPC's branch does not count the
plugin-fixed vectors that the UPC branch includes, and the claimed FFT-plan
storage bound needs supporting capacity/allocation evidence. Planning occurs
inside the estimate before budget acceptance, which must be reflected in its
scope. Owner stability/final gate handoff is pending; no repeated Cargo request.

Root found and reviewer confirmed a further public route: `plugins-bridge/src/
factory.rs:159` still uses legacy `from_params` and silently drops explicit
`true_stereo`. The bridge/NIH route requires propagation and real audio/preset
tests; facade and engine tests alone do not cover it.

### Compiled estimator revision

Implementation hash `0e67d0d86e4db09fc34d3b579622f2752722f4c99de97e6363a405a901bf06bf`
now counts fixed plugin buffers in both backends and avoids FFT planning before
its geometry check. This closes those omissions. The focused log
`/tmp/sotf-aud134-true-stereo-focused2.log` confirms 2 unit and 4 direct tests
passed; other tests were filtered.

**P1 still open:** the reserve assumes scratch <= FFT length and plan storage
bounded by two FFT arrays plus 4 KiB. The new plan-allocation test checks only
2048/4096/8192/16384, and is not selected by the `true_stereo` filter. Public
construction accepts every head length 32–512, so e.g. 257 creates 514-point
and doubled non-power-of-two FFTs; the stated bounds are not established for
those plans or all larger budget-admissible levels. Preserve valid API values
and provide a genuinely conservative bound or staged bounded plan accounting.
Add representative actual prepared-capacity/retained-allocation comparisons;
state the allocator/platform scope rather than infer it from a few plan sizes.

### Allocation metric and exhaustive-head follow-up

The budget-derived planner test is in `src/lib/tests/finite_stream.rs`, distinct
from the earlier handpicked test in `tests/direct_convolution.rs`. Its revised
source enumerates every public head length 32–512. The recorded
`/tmp/sotf-aud134-true-stereo-final-candidate.log` confirms that planner test
passes, while the constructor test fails: 3 passed, 1 failed. That failure
compares cumulative construction allocation traffic with a retained-storage
estimate, so it does not establish a persistent-storage bound violation.

The next source revision measures allocation requests minus frees with the
returned plugin still alive. This matches the intended retained metric; it is
neither peak preparation storage nor RSS. Require an explicit nonnegative delta
assertion and accurate test naming. Cumulative planner construction requests
remain a conservative check for retained plan requests. The source was still
changing during this pass, so no exact-source green verdict is inferred.

Remaining evidence refinements sent to the owner: remove or prove rejection at
the planner search's 30-times-48,000 upper endpoint, include ordinary no-head
NUPC geometry, and support the revised recursive FFT-plan reserve rather than
generalizing sampled allocator results to every platform. The current exhaustive
head test is useful empirical evidence for its compiled RustFFT backend.

FFI preset restoration must permit valid true-to-false structural reconstruction
on a compatible two-channel handle through the existing control-thread path;
an assertion that this valid restore rejects is not acceptance evidence.

## Revised design disposition

Explicit structural `true_stereo` defaults false, preserving old presets and
public constructor callers. The opt-in constructor plus factory JSON path is
coherent. Legacy four-channel renders have now been captured for all three
backends. The proposal predeclares full-vector peak/RMS bounds, replacement and
reset checks, and four-engine NUPC budget checks. These resolve the initial
design blockers below.

Implementation acceptance additionally requires evidence that the Advanced
control is mounted through the consuming host schema and persisted configuration
reconstructs the mode; a constructor-only test is insufficient. Cover invalid
plugin width/nonboolean configuration and the explicit mode before loading an
IR. Process and drain heap guards must include deallocations. The test-owned
Rubato reference checks independent resampling plumbing with the same library,
not an independent resampler algorithm; preserve that qualification.

Root's subsequent integration inspection confirms the engine's
`PluginSettings::Convolution`, parameter accessors and `convert_convolution`
also need explicit field propagation. These bounded serde/default/accessor/
conversion changes are part of this feature, with preset roundtrip and actual
engine-to-factory audio evidence required. They do not authorize changes to the
separately blocked broad manager/concurrency protocol.

Read-only source inspection confirms `plugin_param_accessors.rs:291` lists
only the old convolution fields and `plugin_config_converter/effects.rs:322`
does not serialize routing. The sibling generic layout controls call the
shared settings setter and map through `engine_param_at`, with structural
fallback in `app-gpui/ui/plugin.rs`. Therefore schema, settings accessors,
structural metadata and config conversion must agree; a rendered toggle that
only mutates a helper cannot establish the route.

The proposed source-major matrix equations and independent f64 convolution
oracle are coherent. UPC retaining two input histories and NUPC preparing four
path engines are bounded implementations. Actual pre-edit source and three
two-channel renders have been captured by the owner.

## Required design refinements

1. Automatic interpretation of every stereo-plugin/four-channel IR changes
   existing surround/quad IR presets. Specify a compatibility-safe selection
   policy. An explicit serialized routing choice defaulting to legacy diagonal,
   with true-stereo opt-in accessible through existing factory/UI/preset routes,
   is preferable to undocumented reinterpretation. The cited Yamaha reference
   documents LL/LR/RL/RR but also treats A/B and Quadro metadata as normal stereo:
   https://manual.yamaha.com/pa/software/vstrack/pr/en/11_reverb_plugins_en.html
2. Correct the legacy migration description: existing stereo processing used
   IR channels 0/1, not source-major LL/RR channels 0/3. Capture an existing
   four-channel IR render before production changes to guard that contract.
3. Predeclare complete-vector numeric tolerances and the resampling oracle.
   Include matrix-to-diagonal-to-matrix replacement, failed replacement retaining
   active routing/audio, and reset/EOS behavior through public routes.
4. Verify all four NUPC engines in prepared-memory estimates, checked size
   arithmetic and rejection before excessive allocation; measure callback
   allocation/deallocation for process/drain and state handoff as applicable.

No Cargo gate was run by the reviewer. The owner may implement the revised
bounded design, coordinating shared factory paths with the Ambisonics owner.
