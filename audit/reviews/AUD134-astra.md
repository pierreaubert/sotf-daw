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

## Native Convolution resource/state callback checkpoint — 2026-09-30

**Implementation transaction design supported; bounded callback acceptance pending two focused evidence corrections below.** No new production defect identified. No reviewer Rust edits or Cargo runs. Verified packet `audit/artifacts/aud134-native-resource-r3`, index `7c0e4e69d81a606af3517b41864d63e8380db356cd3c240578c7a5304d566dde`, all indexed files, matching full-run start/end manifests, and every currently listed source hash. TokenSave status showed rebuilding; direct current bytes were authoritative.

### Supported source and behavior

Convolution path/numeric state is parsed and validated as a detached pending tuple before host-visible numeric mutation. Recognized numeric values are type/range checked, resource JSON is versioned and malformed resource keys are refused; legacy missing resource means dry rather than inheriting a previous receiver path. Core/bridge preflight uses actual file decoding and routing/storage validation. Initialization reads pending values/path, builds and validates the candidate, then commits parameter values/smoothers and resource path after successful preparation. The RAII attempt discards an unsuccessful pending request; failed initialization leaves committed controls/path intact, so an ordinary retry cannot silently revive the failed resource request. Pending state can be serialized before activation; that is an accepted request, not a claim that DSP preparation is already committed.

The vendored state hook runs validation before ordinary parameter writes. Convolution refuses active or audio-thread restore before its resource mutex/filesystem work. CLAP/VST3 pass active and audio-thread context, while the standalone render-side route conservatively supplies both true. Other plugins retain default hook behavior. Typed float/bool/integer helpers apply committed plain values and reset smoothers through existing parameter mutation semantics; they are control-initialization helpers, not an audio-thread preparation API.

Actual CLAP and VST3 COM fixtures exercise saved bytes into fresh instances, distinct four-path IR replacement, declared latency/tail, complete reference vectors, exact populated twin continuation after active refusal, deleted/invalid resource refusal, clearing/legacy delayed dry, and disappearing staged resource before activation with numeric/path rollback and successful dry retry. The separate f64 direct convolution maps LL/LR/RL/RR and wet/dry contributions independently. Finite full-vector checks and old-versus-new IR sensitivity are meaningful. Rendering appends zero input for declared support; this is native callback/tail behavior, not a new host EOS protocol proof. These tests instantiate actual wrappers in-process, not packaged binaries or native file-picker/editor flows.

Read terminal logs: focused CLAP/VST3 2/2 (`aa94ad840b6e7e64f936201a107e8be99c77f28fb566601458b42cc0ae5276b1`), full feature NIH 117 passed/one ignored (`ca5ddc609afc9086c2a8161ea6c7a5589ff9b41cd07548ec4052f3cc9bfab549`), strict all-target lint (`0add14a59c2266fcf78b13a02f0ca69d5ed990bba1a88bd0eeee29593c0f567e`). Full/lint use final fixture after a needless-borrow cleanup; earlier VST3 setup omission and style failures remain historical.

### Required focused corrections

1. **Restore accepted numerical limits.** `assert_waveform_matches_direct` currently permits peak `<1e-4` and RMS `<1e-5`, versus accepted AUD134 complete-vector bounds peak `<=1e-5`, RMS `<=1e-6`. The packet provides no previously accepted separate native relaxation. Restore the original bounds and run both actual callback cases; retain any failure as evidence and fix the cause rather than relaxing thresholds after results. Existing exact fresh/twin checks remain useful but do not replace the independent oracle.
2. **Bind the changed guard/helper sources.** The selected manifest omits the changed vendored CLAP wrapper, standalone wrapper and typed boolean/float/integer smoother-helper files. They are part of the reviewed resource transaction and audio-thread guard, not optional transitive noise. Include their exact bytes plus the already listed production files in the focused run's matching start/end manifest and preserve the selected source snapshot. This corrects selected provenance; no claim of full compiler-input closure is needed and no broad workspace gate is requested.

Root has assigned these bounded corrections. Prior accepted true-stereo DSP/FFI/engine/app routes remain unchanged. Native user resource selection/editor is explicitly unimplemented: no editor override exists, and active GUI state submission alone would encounter the deliberate restore guard. Native editor/host-serviced reactivation and loaded CLAP/VST3 editor evidence remain separate required work; do not close the full feature or audit from this callback checkpoint.

## Strict native callback evidence closure — 2026-09-30

**Bounded native Convolution state/resource callback checkpoint ACCEPTED.** Both conditions above are closed in `audit/artifacts/aud134-native-resource-r4`, index SHA `6c03519531dfac9d581ee4ede5cc67ea90eecf082b78d5bd141c94cb3ee89ae3`. Independently verified every indexed file, byte-identical focused start/end manifests (`603f85dacb6a4011a433458082c62105263ec0b41d8c5079641d636987f90174`), and all 19 current plus archived source files against that manifest. The previously omitted CLAP/standalone guards and boolean/float/integer smoother helpers are now included.

Inspected final fixture SHA `b8b12e57d674d2238e2db25249004418d453206996eec9e26b21dc5d3ed66a0f`: complete independent direct-convolution comparisons enforce the original peak `<=1e-5` and RMS `<=1e-6`, with no relaxed limits. Actual CLAP and VST3 callback tests passed 2/2, log SHA `b7715723a1f399a5fe2087a40f134ab65eb5f1bb4d48cacb1b971b0dfab9336a`. Only those test assertions changed from the preceding same-production full NIH 117 passed/one ignored and strict all-target lint checkpoint; a redundant full run is unnecessary for this closure.

Acceptance covers the reviewed staged resource/numeric transaction, commit/discard behavior, saved-byte restoration, active refusal/history, replacement/clear/legacy/missing-resource behavior, activation rollback/retry, and complete native callback audio at the stated bounds. It remains selected-source evidence, not a complete compiler closure, packaged native load, standalone runtime test or whole-workspace result. Native file-selection editor and real host-serviced resource selection/reactivation remain explicitly unimplemented/open. No production changes or reviewer Cargo runs; earlier feature/audit scopes are unchanged.

## Editor service/generated lifecycle checkpoint — 2026-09-30

**Supported success path; bounded checkpoint acceptance pending the narrow evidence conditions below. No new production defect demonstrated in this review.** This is not actual GUI→CLAP/VST3 restart dispatch or packaged editor acceptance. No Rust edits or reviewer Cargo runs.

Inspected editor `8029560da71b18a7cfaaf5075be361092fdb67baeac72bfb1b8355a009d4e4f4`, generated test `e367b2591188a90adaedb642b3938a4118d5303dbf6f412503f93054c93aab89`, and current generated wrapper `6d855f670886ff88a2a6879fd6b92b30d3f6546f251ad516b75cb2c2e2473f60`, params `d7e9f0690c94438dad4b30858e96abda06e1a4f2c0f693632e05fa9e1577f4e4`, configuration `6feb2c03bcf42f67ecd15b92cc3e7f4d962bee24534021aece662322bb8abf33`. TokenSave reported rebuilding; actual bytes were authoritative.

Preparation captures generation, geometry and constructor fingerprint; stale completion cannot replace a different current request. Realtime old-audio permission is published before exposing the editor's structural value and is restricted to the exact requested structural fingerprint. Constructor changes invalidate that exception. Candidates are prepared off-thread, then consumed at initialization; latest live scalar values are synchronized before replacement. The editor overlay commits only true-stereo/path, rather than replaying its stale snapshot over later wet automation. Resource epoch changes refresh the editor after successful external initialization as well as editor application. Generation/geometry checks and failure status are meaningful, but do not substitute for the missing executed cases below.

The selected generated test populates synchronized old-IR subjects, compares complete pending/preparation/host-deferral continuation exactly, changes wet from .65 to .37 while pending, then initializes/resets the replacement and checks full four-path convolution with latest mix and reported tail. The sensitivity control uses the old IR at the **same true-stereo topology** and latest mix, avoiding the previously confounded different-topology comparison. Log `47b1f763a0e083098eafb04cf4f2943f2f707977392cf8f4f6a37ff8e8e80940` is a genuine 1/1 selected pass. Prior zero-selected r1 and oversized 311-frame fixture r2 are historical, not passing evidence. Corrected calls remain within negotiated capacity.

### Required evidence closure

1. Bind the generated implementation dependencies in the next selected packet: at least wrapper.rs, params.rs, params/configuration.rs and any changed shared hook/helper/config inputs in addition to the editor/test. The r3 packet currently preserves only the latter two source files after the log; it cannot establish exact generated-source provenance by itself. Preserve explicit post-gate versus run-bound qualifications. Include the actual terminal receipt for the two existing service tests rather than counting source presence as execution.
2. Add one focused geometry/generation invalidation regression: prepare or stage an actual candidate, change negotiated sample rate or maximum callback size, reject/discard the old candidate, deliver an older background completion after a newer request, and prove only a freshly prepared matching candidate can apply. Check request/status/old-audio permission and successful retry, not just a returned error. Current service regression changes constructor booleans; its second test only increments an idle epoch. Neither executes a changed-geometry or out-of-order-completion scenario despite those being the new service's explicit protections. A focused test/receipt is sufficient; no broad workspace rerun is requested.

Future actual editor/host restart gates must additionally test host refusal/deferral and valid retry through the real callback surface, not by calling `set_restart_result` or initialize directly. Direct generated lifecycle evidence remains useful and should be retained with this qualification. Prior accepted native resource restore/audio scopes remain unchanged; full native editor and overall audit completion remain open.

## Generated editor geometry/provenance closure — 2026-10-01

**Accepted: the two evidence conditions for the bounded editor service/generated lifecycle checkpoint are closed.** This acceptance combines the previously reviewed implementation with the newly executed geometry/generation regression; actual GUI→CLAP/VST3 restart callbacks and packaged editor loading remain separate open gates. No reviewer Rust edits or Cargo runs.

Verified the sealed packet `audit/artifacts/aud134-editor-geometry-r3`, index `fbdf86914e78bf8df52b2d8f591698401750080e40b2945c30b32a9ffb08bfba`. All indexed files verify; all 27 archived selected inputs match the identical start/end manifests, aggregate `34fd6b90bed776fa140de8b420bd32c3124f784c6bf1aa422800fca130d2eb6f`. Binding now includes generated wrapper, parameters, configuration, shared context/state/helper files and selected build dependencies. It is still a selected execution binding, not a complete transitive build archive or acceptance of later concurrent EQ/native work. The inspected editor production delta from the prior checkpoint only exposes the status accessor within the crate for the regression.

The new generated test creates a real prepared candidate at 48 kHz/max257, changes the negotiated maximum to64 through the generated Plugin initialize route, and verifies candidate removal, visible reconfiguration status and cleared old-audio permission. It then holds an older max64 task, creates a newer request, delivers the older completion, and proves no candidate publication/status replacement/permission grant. Old-resource audio equals its synchronized control while the newer request is pending. Only the matching fresh generation can subsequently stage and apply; path/true-stereo readback, cleared permission, declared tail and complete output against the independent four-path reference establish successful retry. This is genuine changed-geometry and stale-completion evidence, not merely toggling constructor flags or incrementing an idle epoch. Successful reinitialization deliberately resets both subjects; the test does not claim recursive history continuity across a successful geometry change.

Current service tests passed **2/2**, log SHA `72becf11f5e966bf31518ed1bf6dfcaafb6c8f265a7d5f8fe1388e83986c99ef`. Generated pending/latest-mix plus geometry lifecycle tests passed **2/2**, log SHA `e7bd1fde3bf5b474fb565c862acad909459f447573ca16ea095e5dade8b62622`. Existing full-vector peak <=1e-5 and RMS <=1e-6 bounds remain unchanged. Earlier dependency compile and usize/u32 fixture failures did not execute these tests and are not counted as behavioral passes. No new full NIH/lint/packaged-build claim follows from this focused packet. Prior accepted resource restoration and waveform checkpoints retain their separate provenance.

## Linux CLAP embedded editor and sparse pending serialization — 2026-10-01

**Accepted for this bounded callback/GUI checkpoint.** Reviewed the frozen r9 source, copied executable and terminal evidence in `audit/artifacts/aud134-clap-editor-show-hide-r1`. Verified all 46 entries of `SHA256SUMS-r9` (index `d5aced22edb4c58bcafc787ed02330a4dd568b2adebc799b41af1022eb7ecaad`), identical selected start/end manifests and all 29 archived source inputs against those manifests. Copied executable SHA is `66896acdc9b7bada44501c140f6294d8f07e6b4b0da9460d104019e169c76050`. This binds selected inputs, not the complete compiler dependency closure or concurrent subsequent NIH changes. No reviewer Rust edits or Cargo run.

The CLAP show/hide callbacks now forward to the retained editor handle. The egui adapter downcasts its own handle and maps/unmaps the existing X11 child, preserving editor state rather than rebuilding it. The callback refuses absent handles and unsupported editor implementations. The new trait default returns false; external/custom Editor implementations must implement visibility to support this CLAP path. Linux was exercised; Windows ShowWindow and macOS setHidden branches have source only, with no cross-compilation or runtime acceptance here. The GUI test asserts successful hide/show callbacks and performs successful mouse interaction after reshow; it does not independently query the X11 map state while hidden. Do not describe it as a hidden-window pixel/map-state assertion.

The actual Xvfb test creates and parents the native CLAP editor, clicks the true-stereo control and file browser, selects the new four-path IR, hides/reshows, clicks preparation, and receives the actual host restart callback. A populated old-IR twin stays sample-identical while the host defers that request. A real CLAP Mix event changes both subjects to 0.37. Saved state then contains the selected new resource/routing and current Mix, rather than the stale staged 0.65. A real retry-button click produces the second restart request, followed by host stop/deactivate/activate. The complete resulting waveform, including the latency prefix and finite convolution suffix, matches the independent four-path f64 reference at the original peak <=1e-5 and RMS <=1e-6 bounds. Exact frame length and every output's finiteness are asserted. The old two-channel IR control is independently checked at the same mix, and its difference from the replacement exceeds RMS 1e-3.

The serializer correction is appropriately limited: a pending editor generation overlays only staged `true_stereo`; other numeric parameters serialize from their current controls. A pending external state restore continues to overlay the complete saved numeric snapshot. The executed fresh external restore explicitly saves before activation and verifies Mix 0.65 over the default 1.0, preventing the sparse editor rule from accidentally dropping dense imported state. Existing resource refusal/rollback and generated lifecycle tests remain in the same affected module.

Terminal affected callback result is **4 passed, 1 explicitly ignored GUI test**; the exact copied binary's separate Xvfb GUI execution is **1/1 passed** (`logs/clap-editor-xvfb-r6.log`, SHA `a948a4796d62beb7108b950fac66c4856acec2bcba51fc38507587dd04d6d302`). Historical coordinate misses and the genuine stale-mix serialization failure remain preserved. This is a real embedded Linux CLAP mouse/callback/audio route inside the test host, not a separately packaged plugin loaded by a third-party host. VST3 editor callbacks, packaged embedding, non-Linux visibility, post-fix full NIH package and strict lint remain open; the earlier broader gate is not relabeled as current. No complete native editor or full AUD134/audit acceptance follows.

## Direct exported VST3 embedded editor — 2026-10-01

**Accepted for the bounded Linux direct-wrapper IPlugView/mouse/reload/audio checkpoint.** This does not establish discovery or loading of a packaged `.vst3` bundle through sotf-host. No production edits or reviewer Cargo runs.

Verified all 26 packet checksum entries in `audit/artifacts/aud134-vst3-editor-r5`, index `f06477d70ab56624f2b9b3132f686f24c1bf07823e71521d3245c2f74569390f`; selected start/end manifests are identical and all 13 archived source copies match. Verified the executed copied binary SHA `c5dc46e14d340352b0c94426cfc13318e3b03c17671aa1adb8365845507e318c`. Selected sources are not a full dependency archive. The focused module passes **4/4 with two GUI tests ignored**; the separately selected copied-binary Xvfb VST3 test passes **1/1**. No new full NIH or strict-lint acceptance is inferred.

The fixture attaches the actual exported IPlugView to an X11 parent and supplies IComponentHandler, IPlugFrame and IRunLoop through the host COM object. Actual mouse input enables true stereo, enters the fixture folder, selects the new WAV and requests loading. Generic startup restart flags are counted separately, so they cannot consume the scripted kReloadComponent refusal/retry. Both reload calls are checked for exact flags, response and the attaching host thread.

The first reload is refused while the populated old IR remains live. Actual ProcessData/IParameterChanges automation sets Mix 0.37; complete continuation is sample-identical to the equally populated old-IR twin, and saved state contains the staged new path/routing plus current mix. A second actual retry-button click obtains host acceptance. The test then saves the state stream, removes the view, verifies registration/unregistration balance, deactivates and drops the subject, and restores a newly created wrapper before activation. This is genuine fresh-wrapper reload evidence, not merely resetting the same DSP object.

New and old resources each have their latency/tail checked and full rendered vectors compared to their independent four-path and diagonal convolution references with the original peak <=1e-5 and RMS <=1e-6 bounds. Exact length and finite outputs are checked by the reference helpers. The additional >1e-3 old/new sensitivity comparison uses their common prefix; complete correctness is established by the separate full-vector references rather than that sensitivity reduction. The short impulse tails are rendered through the native process interface according to reported metadata; this is not a separate host drain-API test.

The fixture retains COM event storage through synchronous processing, and run-loop pumping/removal is serialized on the attaching thread. No new lifetime defect was found in this bounded inspection. Historical controller-setter and wrong-directory fixture failures remain disclosed. Linux direct callback success does not prove non-Linux behavior, packaged build/discovery, sotf-host GUI ownership/reload, or complete current NIH/workspace gates. Those remain open under the full objective.

## Loaded VST3 Convolution restore and prepared-resource reuse — 2026-10-01

**The r5 nonempty-state restore and unchanged-geometry prepared-IR reuse checks pass; general failed-restore transaction acceptance is blocked by the empty-state path below.** No reviewer Rust edits or Cargo runs. TokenSave-first current-source inspection saved approximately 27k tokens across source slices. Earlier GUI/direct-wrapper evidence remains separate.

Verified all **20** selected archived build inputs against their start/end manifests and current files; the two build manifests agree. The loaded-run start/end manifests agree over **21** entries, adding the copied library, and all current entries match. Copied bundle library SHA is **bb47e455d2e694f9fa8b5f850ef4185b546e744fc0f7652891187a48addae9a3**. `root-loaded-r5.log` matches SHA **9a033034e3e06fd3641607660878b77fcbf8897d64be664831aac310e3e754ec**, with **1/1** loaded test passing after the successful fresh debug build. These selected source bytes are not the full dependency closure. The focused geometry helper log matches **0b6c1ea4e19c268fc5906b36622016eaa49de420abada2fbe56d598809193aaf** and records **1/1**; its source hashes are explicitly post-run observations. The separate root invocation is duplicate evidence, not added coverage. The checkpoint document's pending loaded-gate paragraph predates these final root receipts and must be read as historical.

The actual sotf-host loader opens the copied Linux `.vst3` bundle, restores a true-stereo resource, and processes nonzero audio. The f64 oracle directly convolves four distinct PCM16 paths, adds the declared delayed dry path and mix, and checks every finite sample, complete length, fixed latency **1,024**, and finite tail **latency + IR length - 1**. Original mix **0.37** and successful replacement mix **0.61** pass peak <=**1e-5** and RMS <=**1e-6**. This is actual loaded-host audio, not merely direct exported-wrapper calls.

For a missing replacement IR, the nonempty VST3/no-typed-setup path now loads a detached backend and commits only on success. The test first populates matched live/twin histories and deletes the original IR, so attempting to restore old state from disk could not fake recovery. The rejected nonempty replacement leaves the old path, true-stereo flag, mix, finite tail and complete nonzero continuing output equal to the untouched populated twin. It checks these selected serialized fields, not equality of every byte of the failed-restore snapshot.

The NIH reuse path is convolution-feature-gated and requires the last successfully prepared geometry, both structural fingerprints, matching current inner widths, no pending editor generation/restore, and exact committed/prepared resource path agreement. Realtime values are validated before same-rate initialization and scalar synchronization. The core's same-rate initialization does not reopen the IR, and reset clears histories while retaining the kernel. The loaded test sends mix **0.23** through the actual exposed VST3 ID and process callback, then calls `reset_checked()` after the file is deleted. Saved controls remain correct and the full post-reset vector passes the same independent oracle; RMS difference >**1e-3** from the dry-only control confirms retained convolution. The geometry unit only checks the compatibility predicate; it is not loaded changed-geometry refusal or failed-reactivation history evidence.

### Required correction: empty no-setup VST3 restore still mutates the live instance

At `external_plugin.rs`'s no-typed-setup branch, `!state.is_empty()` excludes empty bytes from detached restoration. Empty bytes therefore call the live `Vst3Backend::load_state`. That function first assigns `cached_tail_length = Unknown`, suspends the active component, invokes native state restoration, and resumes even if restoration failed. Its `(Err(load), Ok(()))` branch returns the error without restoring cached tail metadata. Consequently a rejected empty state already violates metadata preservation; reactivation may also reset populated histories or fail. This is a concrete source finding, not a new executed empty-state result.

Route **all** no-typed-setup VST3 restore attempts through a detached candidate, preserving the native empty-state callback's success/error semantics. Do not simply call the existing replacement helper unchanged: it skips `load_state` for empty input and could incorrectly commit a default instance. Invoke the empty callback explicitly on the candidate (or distinguish restore from construction in the helper). Leave the separately defined typed-setup empty-state behavior unchanged unless separately specified/tested. Add a focused loaded regression that rejects empty state with populated twins after the original IR is deleted, checks saved controls and finite tail, and requires exact continuing audio. Reuse the successful nonempty/reset gate to establish the final source checkpoint.

### Remaining limits

The loaded log retains NIH diagnostics for missing component handler, `Arc::strong_count` at teardown and rejected-state deserialization. The missing-resource deserialize warning is expected by the refusal case; the handler/lifetime diagnostics are not a clean lifecycle result. This inspection found no additional concrete regression attributable to the reuse code, but did not prove those adapter diagnostics benign or establish leak-free teardown. Broad NIH/host lint, packaged GUI/editor ownership, non-Linux platforms, loaded geometry-change behavior, realtime allocation/deallocation/deadline evidence and full AUD134/SOTA acceptance remain open. Historical r3 failed-reactivation and r4 failed-reset evidence retain their original status.

## Empty loaded VST3 restore correction — 2026-10-01

**Accepted for the bounded failed-restore correction; the preceding empty-state blocking finding is closed.** No reviewer production edits or Cargo runs. TokenSave reported an index rebuild in progress, so current file bytes and selected archived inputs were checked directly; source slices saved approximately 11k tokens.

All no-typed-setup VST3 `load_opaque_state` attempts now construct a detached candidate. The replacement helper explicitly calls native `load_state` for empty bytes in that scope, preserving rejection instead of silently accepting a default instance. Commit occurs only after success. The typed-setup empty-state branch retains its prior behavior. Reviewed current host source SHA `2ff19b7f14231f6297948de34575e087365bc93164741d1dc2d219d793686f8b` and regression SHA `807f91b5595a94b87c0c2a39caf23a84ff93dbc835f1f50310189f072581f591`.

Verified all **21** entries in both current and archived selected inputs of `audit/artifacts/aud134-loaded-convolution-r6`. Start/end manifests are identical, SHA `11cbd3004ab220d2658369227e7ad25aa8cb8ebc49d874028847e24d42554242`. The separately recorded reused r5 bundle library verifies as `bb47e455d2e694f9fa8b5f850ef4185b546e744fc0f7652891187a48addae9a3`. These are selected input bindings, not the complete build dependency closure.

The historical red log, SHA `c47c3e99cc4aa2bedc4a2d32b70a994c72eae14a43f77d00583550243a2e204f`, reproduces rejected empty restore changing `Finite(1027)` to `Unknown`. The r6 terminal log `logs/empty-state-loaded-r1.log`, SHA `2268d19569a04dd54f6ae4bce4988ce396276a6e3426b2bd203aec8c7a2195a6`, passes **1/1** actual loaded-host tests. Its regression rejects empty bytes after deleting the original IR, checks preserved resource path/true-stereo/mix and finite tail, and requires exact nonzero continuing audio against an equally populated untouched twin. The same executed test retains successful nonempty restore, missing-IR refusal, and prepared-IR reset/reuse checks with their independent full-vector oracle bounds.

No additional concrete finding in this correction. Existing component-handler and teardown diagnostics remain visible; their lifecycle implications are unresolved. This focused acceptance adds no full NIH/host lint, packaged editor, non-Linux, realtime-safety, loaded geometry-change, or overall AUD134/SOTA completion claim. Earlier acceptance and limitations retain their separate scopes.
