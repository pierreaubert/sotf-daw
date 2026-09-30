# Ambisonics and convolution integration coverage (AUD133–135)

Source inspection: 2026-09-29, during implementation on top of `9302797`.
This note records consuming routes and missing evidence. It is not an acceptance
report. The issue-specific Astra reviews determine acceptance of later fixes.
MIDI and IAMF remain excluded.

## Latest native checkpoint (2026-09-30)

Astra subsequently accepted the refreshed VST3 output-layout cache fix,
populated in-process late native-load refusal and retry, requested engine
preflight rejection, and actual isolated VST3 worker construction/continuation
checks. The final scoped disposition and packet identities are in
`reviews/AUD135-astra.md`. The durable current-source worker test passes 1/1
and focused strict lint passes; evidence is in
`artifacts/aud135-isolated-vst3-worker-r2/`.

The actual isolated test uses the refreshed native artifact, compares complete
64-input/16-output worker audio with the direct native reference plus the
64-frame IPC delay, and verifies that a detached candidate failing native
state loading leaves populated live workers unchanged. Its valid retry creates
a fresh worker. Actual engine replacement/commit, effective-width/latency
publication, engine EOF, isolated CLAP and mounted setup controls remain open.
Zero-input worker process blocks do not establish EOS draining. Luna is now
implementing those engine-boundary checks.

The older narrative and route table below describe earlier checkpoints. Their
late-load-failure and refreshed-artifact limitations are superseded only for
the explicitly reviewed paths above. True-stereo convolution native resource
selection/persistence remains an independent gap.

## Earlier native checkpoint (2026-09-30)

Astra accepted the corrected framework/wrapper and actual loaded-host restore
stages of AUD135. The revised NIH wrapper has 101 passing library tests and
strict Clippy; actual CLAP/VST3 restore tests pass for selected layouts,
nondefault controls and transactional rejection of conflicting saved state.
The earlier failed native runs below remain historical evidence.

Root subsequently ran the bridge and FFI all-56 tuple checks and the saved
default NIH/CLAP audio replays. All four selected commands passed. The full
FFI, NIH and CLAP capture files match the original saved bytes exactly; the
independent archive comparison is `/tmp/sotf-aud135-postrestore-byte-replay.json`.
The detailed commands, hashes and matching selected-source manifests are in
[the native report](native-ambisonics-orders-1-through-7.md).

Deliberate order/layout reconfiguration now passes both actual CLAP/VST3
loaded-plugin tests after the lint correction (`reconfigure-run7`), and strict
host Clippy with both native features passes. Exact logs and manifests are in
[`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md). Independent Astra review
of this newer checkpoint is accepted in `reviews/AUD135-astra.md`. Its failed
change case covers prevalidation refusal; late candidate failure remains open.
AUD138's bounded host/recovery/reprepare scope is also accepted. AUD143 VST3
callback discovery/audio and subsequent legacy migration corrections are
accepted by Astra. The latest full NIH checkpoint passes 111 tests with one
ignored, plus strict all-target lint. A freshly exported BandSplit library now
loads through SOTF as CLAP; its actual VST3 load reproduces the host's rejection
of four output buses. The bounded consuming-host extension is in progress.
Full audio/state verification of fresh exports remains pending, including
refreshed Ambisonics artifacts after these framework edits; the earlier
AUD135 binary predates them.

The failed-change waveform case currently rejects invalid order 8 during
prevalidation. It proves that rejection preserves the populated live instance;
it does not exercise a later failure after candidate import or negotiation.
That later failure path still needs its own executed preservation evidence.
Isolated workers, the factory/engine selected-width and
candidate-failure paths, mounted setup/observed-state controls, and consuming
EOF routes remain open. Current-source inspection reconfirms the consumer
table below; the 6,100-test workspace gate predates these native changes.

## Accepted AUD133/AUD134 checkpoint

At this historical checkpoint, the corrected DAW workspace passed **6,100 tests across 362 binaries, with
19 skipped**, in 272.300 seconds. Log:
`/tmp/sotf-aud133-134-workspace-rerun.log`, SHA-256
`4c417bab556373409374329b1de38b30abab67cdbafc36908037eb23bb4dd449`.
The 2,676-file start/end manifests match exactly at
`85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`;
paths are `/tmp/sotf-aud133-134-workspace-rerun-{start,end}.sha256`.
The DAW lock at that historical gate was `c161c74a…`. These manifests cover the DAW and patched
math sources described below, not sibling app execution.

Astra accepted AUD133's named-layout/core/engine/model scope after the
post-lint checks below. Its wider native and mounted-order-control routes are
still open; see [the scoped report](ambisonics-orders-4-through-7.md).

All seven failures from the first run are closed. The later selected
engine/bridge strict lint found three test-only style findings (two fixed-size
chunk iterations and one unnecessary Box replacement). Their narrow correction
passes both AIFF tests, the actual changed scratch-capacity test and strict
engine/bridge all-target lint. Logs are `/tmp/sotf-aud133-postlint-fix-aiff.log`,
`postlint-fix-scratch.log` and `postlint-fix-clippy.log` with the same prefix.
At the accepted AUD133/AUD134 checkpoint, root verified that only those two
test files differed from the green broad snapshot, and the files matched the
focused run's source/lock manifest. Later AUD135/AUD136 work requires its own
verification; the preceding result does not cover those edits.
The supplemental `postlint-fix-capacity.log` selects a different fixture and
is not the evidence for the changed assignment. No production file changed
between the broad run and that accepted checkpoint. The mounted app test had
exposed a separate actual integration defect: the True Stereo click changes
settings but queues parameter automation instead of a structural rebuild.
The sibling player mapping regression now passes. The mounted route also
passes in `/tmp/sotf-aud134-mounted-gpui-native-click.log` (one selected test),
SHA-256 `8f1d03844908c5addb6e87bddfb48bee5b2d54384a9e4acd330c393f22db6b73`.
It dispatches real GPUI mouse-down/up at the painted On-button bounds, checks
the changed settings and queued Structural request before advancing the clock,
then exercises disk preset save/load and a fresh PluginState. Astra accepted
AUD134's documented scope after final report/provenance review. Broader native
feature gaps remain open.

The earlier HTTP click helper advanced the clock, allowing PlayerView to consume
the queued request before the assertion. Omitting clock advancement entirely
also prevented Dev API delivery, so that attempted helper timed out. The final
passing test uses the mounted GPUI event route directly; it never substitutes
a settings setter for the click. It proves the queued rebuild request and model
persistence, with actual DSP audio covered by the separate engine/FFI tests.

## Routes that must agree

| Route | Ambisonics orders 4–7 (AUD133) | True-stereo convolution (AUD134) |
|---|---|---|
| DSP public API | Higher-order basis, matrix, 64-channel processing, lower-order compatibility and tail tests implemented; 83 package tests and strict lint pass; scoped Astra acceptance with noisy setup-cost increases disclosed | Explicit `true_stereo` opt-in with default false; 83 package tests and strict lint pass, including full EOS heap checks, populated reset and asynchronous replacement; explicit six-array pre-edit replay passes; scoped Astra acceptance |
| Main facade factory | Order-derived widths updated; all 64 channels now exercised through the embedded-engine waveform test | New constructor receives the flag; legacy four-channel IR mapping must remain unchanged when false |
| Engine/settings | Real 64-channel AIFF → decoder → embedded engine waveform test, 65-channel engine rejection and three capacity/config tests pass; AUD140 separately records the channel-changing host EOF restriction, which the ordinary-waveform test does not exercise | Settings, accessors, serde defaults and converter propagate the flag; audible saved-configuration test passes; incremental route review found no defect |
| Sibling Studio | Player order-7 graph test and full GPUI model reconciliation/order-change test pass; this is model integration, not a mounted order-control click | Advanced section mounts; actual click, settings change, queued Structural request and disk preset/fresh model roundtrip pass; Astra accepted the layered scope |
| Shared plugin bridge | AUD135 pre-edit full-vector baseline exercises all 56 order/layout tuples through the bridge; typed native setup and external-host integration remain separate | Flag forwarding, strict boolean parsing and audible bridge test pass; incremental route review found no defect |
| C ABI / FFI | All 56 create/process tuples pass in the preserved AUD135 pre-edit baseline; complete native structural-state and consuming-host acceptance remain open | Structural reconstruction, combined mode/resource restoration, scalar rejection and unchanged partial-restore regression pass; corrected source accepted by Astra and broad workspace gate passes |
| NIH CLAP/VST3 and packaged loading | Wrappers expose 56 VST3 and 42 standard CLAP tuples. Astra accepted corrected wrapper ABI, actual loaded-host restore and later deliberate reconfiguration with native-feature lint. Late candidate failure, refreshed artifacts after subsequent framework edits, isolated workers, consuming engine/setup controls and EOF remain open | **Existing feature gap:** native parameter state skips file paths, so the default no-IR native test cannot prove loaded true-stereo processing |

## Focused executed evidence

Root inspected these logs on 2026-09-29. They are scoped results, not final
whole-workspace or native-device acceptance:

- `/tmp/sotf-aud133-aiff-route-focused.log`: two tests pass. The real AIFF
  fixture contains distinct impulses on all 64 channels; every output sample
  is compared with the expected order-7 matrix response. A decoded 65-channel
  fixture is rejected at engine admission. This is an embedded-engine route,
  not a hardware playback or ordinary 64-channel WAV claim.
- `/tmp/sotf-aud133-engine-config-focused.log`,
  `/tmp/sotf-aud133-decoder-capacity-focused.log` and
  `/tmp/sotf-aud133-processing-capacity-focused.log`: one focused test passes
  in each log. Final capacity-review refinements remain with the owner.
- `/tmp/sotf-aud134-true-stereo-expanded.log`: four unit tests and five
  direct integration tests pass. They include matrix/path audio, failed IR
  replacement and complete EOS allocation/deallocation checks. The revised
  geometry test covers every accepted head length 32–512 at the estimated
  budget boundary and the no-head NUPC geometry. Retained requested bytes are
  measured separately from cumulative preparation allocation traffic. Astra
  accepted the scoped DSP/storage evidence for the locked RustFFT backend on
  the tested target. Other backends/targets, peak preparation storage and RSS
  are not established by this retained-allocation result. Later package,
  lifecycle and baseline results are recorded below.
- `/tmp/sotf-aud134-engine-route.log`, `facade-route.log`,
  `bridge-route-final.log` and `ffi-route.log` (the latter three also have the
  `/tmp/sotf-aud134-` prefix): one focused test passes per route. The FFI test
  exercises structural reconstruction and failed-restore history. These
  results have been handed to Astra for consuming-route review.
- `/tmp/sotf-aud133-player-order7-focused.log`: one player graph test passes
  using temporary sibling resolver SHA-256 prefix `475d5890`. This resolver
  preserves the previous git math-audio revision and updates the local
  resampler package version. The original sibling lock was restored; the
  result is not a gate against that restored lock. The separate GPUI
  reconciliation result is recorded below.
- `/tmp/sotf-aud134-convolution-package.log`: full package test command exits
  zero with 82 passed and two ignored pre-edit capture utilities (55 unit,
  10 direct, seven finite-stream and 10 integration tests). Source manifests
  `/tmp/sotf-aud134-convolution-package-{start,end}.sha256` match, aggregate
  `6f79a243789f0930548493adad43a32db6181d90b912542f72f1387bbbb3a417`.
  The accompanying strict all-target Clippy run exits 101 for the new
  eight-argument `validate_ir_limits_for_routing` helper; its log is
  `/tmp/sotf-aud134-convolution-clippy.log`. The owner is fixing that finding.
  These are scoped crate-source manifests, not full-workspace snapshots.
- `/tmp/sotf-aud133-ambi-package-final.log`: 83 tests pass (59 unit, one
  lower-order baseline, 21 public integration, two tail tests), with one
  baseline-capture utility ignored. Owner reports matching start/end source
  manifests, aggregate
  `7bfbd9d36f028f0e04579212765fa2f2ab8b273a6e30eba616a2405f2139c3ca`.
  A syntax-only unused-parentheses cleanup followed this run; strict lint
  then passed. The coordinated workspace gate remains pending.
- `/tmp/sotf-aud134-convolution-package-final2.log`: the later full package
  run passes 83 tests (55 unit, 11 direct, seven finite-stream and 10
  integration), with three baseline capture/replay utilities ignored. It
  includes populated reset and successful asynchronous IR replacement.
  `/tmp/sotf-aud134-convolution-clippy-final2.log` passes strict lint after
  the validation helper and test-only lint corrections.
- `/tmp/sotf-aud134-preedit-array-replay.log`: the normally ignored replay
  was explicitly selected and passes. Its six preserved binary outputs cover
  the three backends for two-channel diagonal and four-channel legacy IRs.
  The ordinary package run above does not execute this replay.
- `/tmp/sotf-aud134-ffi-scalar-final.log`: the structural restoration test
  passes with actual C ABI scalar-mode rejection and live-history checks.
  This narrow result does not cover the separate partial-restore failure
  found by the subsequent broad run.
- `/tmp/sotf-aud133-ambi-clippy-final.log`: strict all-target package Clippy
  passes after that cleanup.
- `/tmp/sotf-aud133-graph-test2.log`: the named component test
  `order_edit_reconciles_64_input_ports_through_canvas_roundtrip` passes,
  one test selected. The prior `graph-focused.log` invocation selected zero
  tests because the test module was excluded from that target; it is compile
  evidence only. The corrected integration target exercises the reviewed
  model reconciliation route under temporary sibling lock `475d5890…`.

## Historical bridge omission and accepted correction

At inspection, `plugins-bridge/src/factory.rs:159–175` parsed
`ConvolutionPluginParams` and called `ConvolutionPlugin::from_params`.
The new routing flag is intentionally outside that legacy public params struct.
Consequently updating only `sotf-plugins/src/factory/create.rs` does not update
the bridge used by FFI and NIH.

`plugins-ffi/src/plugin_factory.rs:330` and
`plugins-nih/src/params/configuration.rs:35` both call the shared bridge.
Luna's AUD134 owner has now added strict boolean parsing and forwarding to
`from_params_with_routing` in the bridge. Root verified the current source at
lines 159–189; focused bridge and FFI audio/restore tests now pass, with
independent route review accepted. The source
hashes below identify the original omission, not this later correction.

The separate focused DSP run at
`/tmp/sotf-aud134-true-stereo-focused2.log` passed two unit tests and four direct
audio tests. It did not execute the bridge, FFI, UI, or all estimator-bound
tests, and does not close those route requirements.

The original FFI restoration finding was separate from forwarding:
`plugins-ffi/src/lib/plugin.rs:85–106` reconstructed using the handle's original
configuration, then applies incoming state via setters. Since `true_stereo` is
structural, restoring a saved true mode into a default false handle may fail
unless reconstruction consumes the incoming flag. This is a source-based risk,
not an executed pre-fix failure. Luna has revised reconstruction and its focused
C ABI test now passes. Astra accepted the corrected complete and partial
saved-state/preset restoration, retained IR/routing and unchanged live output
after invalid restoration. The later configuration/resource findings below
were also fixed before the green workspace rerun.

## Historical first workspace gate: seven failures, now corrected

The locked offline workspace nextest run, excluding MIDI/IAMF and including
FFI, completed in 272.460 seconds: **6,093 passed, seven failed, 19 skipped**
out of 6,100 tests run (two slow). Log:
`/tmp/sotf-aud133-134-workspace.log`, SHA-256
`3184dabce32c53e8716185a4556747f9aedc92567dd1b715b9de73034a263c5e`.
The source manifest is
`aa9444f069da77e0c8a9a362a24f1c69ebc59c2eb076eea4a6e1a181b41669aa`.
All preexisting source bytes match before/after; the raw end manifest adds
only two generated `.snap.new` files. The filtered end-source manifest matches
the start exactly. Manifests include the DAW tree and patched math-dsp/math-iir-fir
sources, excluding the audit reports and MIDI/IAMF directories.

| Failure | Diagnosis and required correction |
|---|---|
| FFI partial Convolution restore | New mode reconstruction inserts `true_stereo: false` into an unchanged original configuration. Preserve the original JSON when the routing mode does not change; retain the existing regression assertion. |
| Service-stream channel boundary | Old test rejects 17 input channels. Update it for the new 64-input limit and test actual 64/65 admission. |
| Two Ambisonics catalog/factory tests | Expectations still list only 4/9/16 channels. Extend them to all supported orders while preserving invalid-width/config rejection. |
| Ambisonics render-plan snapshots | Order control maximum intentionally changes from 3 to 7. Review the corresponding snapshot changes. |
| Convolution render-plan snapshots | Advanced controls gain True Stereo at parameter index 6. Review the corresponding snapshot changes. |
| Catalog channel-output test | Its fixture explicitly sends order 1 for new channel counts. Supply the matching order for each supported width. The failure does not demonstrate a production default-inference defect. |

The Ambisonics owner has corrected its five failures. Root inspected the four
focused logs `/tmp/sotf-aud133-postbroad-service-stream.log`,
`postbroad-factory.log`, `postbroad-catalog-defaults.log` and
`postbroad-ambi-snapshot-final.log` (all with the same `/tmp/sotf-aud133-`
prefix): one, two, one and one tests pass respectively. Each of the ten
Ambisonics snapshots changes only the order maximum from three to seven.
The catalog fixture now explicitly selects orders one through seven; the
factory still rejects an explicit order-one configuration with 64 inputs.
A positive literal 64-channel service PCM fixture now checks the decoded spec
and samples alongside literal 65 rejection; it passes in
`/tmp/sotf-aud133-postbroad-service64.log`. The two Convolution failures also
pass focused checks and the coordinated rerun above.

The FFI owner corrected combined mode-and-IR preset changes from an existing
two-channel IR to a four-channel true-stereo IR. Construction now consumes the
incoming resource before replay, and unchanged routing preserves the exact
original constructor JSON. Both C ABI state and document paths pass in
`/tmp/sotf-aud134-ffi-reconstruction-final.log`; the prior partial-restore
regression passes unchanged in `ffi-partial-restore-final.log` with the same
prefix. Astra reviewed the corrected source and both cases are also covered
by the green broad run.

The mounted Convolution UI test reaches Studio, clicks the actual On choice,
and observes the selected plugin's settings change, queued Structural request
and preset persistence. The final passing result is recorded above.

## Native gaps retained in the full audit

The historical inspection before AUD135 found the NIH wrapper fixed at four
input and six output channels in `plugins-nih/src/wrapper.rs:33`, `:63` and `:100`.
`params/configuration.rs:46–57` rejects saved states requiring another layout.
A successful 64-channel engine route does not establish native higher-order
support. AUD135 subsequently implemented and validated native higher-order layouts;
the accepted scope is summarized at the top of this report. Actual live-engine
replacement/EOS and remaining consuming routes are still open.

Current `plugins-nih/src/params.rs:95` still explicitly skips
`BridgedParamKind::FilePath`. Root reconfirmed this source constraint and the
primitive-only native parameter map on 2026-09-30; selected source and a
source-only receipt are frozen in `artifacts/aud134-native-resource-inventory-r1/`.
No native resource/audio execution follows from that source inspection.
The historical [native tail report](native-tail-wrapper.md) already qualifies
its Convolution proof as the default no-IR path. Loaded-IR selection, persistent
resource state and actual native audio processing remain required before
claiming complete native Convolution support. This gap also predates AUD134.

A follow-up source inspection confirms a broader reachability constraint:
The original `DynamicParams::from_infos` hid non-realtime bool, integer and
float parameters. The current plugin-specific builder exposes the two
DynamicEQ restart controls; other structural/resource controls require their
own delivery route. The production wrapper supplies no
custom editor; the vendored NIH `Plugin::editor` default returns `None`
(`crates/3rdparties/nih-plug/src/plugin.rs:168`). Thus serialized structural
state is not evidence of a user-accessible native setup control. A later native
integration issue must provide a control-thread configuration route, persistent
resources, host reactivation/layout negotiation and actual native audio tests.
Simply unhiding a parameter or accepting edited state does not establish safe
live reconstruction. The source hashes below also identify this inspection.

For higher-order native Ambisonics, include the wrapper's actual bus split:
NIH's main processing buffer has the output width, and surplus input channels
currently use an auxiliary input bus (`wrapper.rs:207–236`). Tests must feed all
64 ACN channels through those native buses, preserve existing layout IDs, and
exercise supported order/layout changes. A larger advertised channel count
alone is insufficient.

### Historical native protocol requirements used by AUD135

Primary specifications checked on 2026-09-29 add a metadata requirement to
the channel-count work. CLAP identifies Ambisonic ports and exposes ordering
and normalization through its Ambisonic extension, including ACN and SN3D.
Configuration information may change only while deactivated. Its port-config
extension provides named choices intended for host menus, with selection also
restricted to the deactivated state. See the [Ambisonic header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/ambisonic.h)
and [port-configuration header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/audio-ports-config.h).

VST3 defines ACN/SN3D arrangements through seventh order. Orders five through
seven use different speaker-bit positions from orders one through four; a
generic low-bit mask is not correct for every order. See the official
[speaker arrangements](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__speakerArrangements.html)
and [3.7.8 interface change](https://steinbergmedia.github.io/vst3_dev_portal/pages/Versions/Version%2B3.7.8.html).

At the original inspection, vendored NIH source did not provide the typed
route. AUD135 later addressed its bounded native scope:

- `wrapper/clap/wrapper.rs:2580` publishes mono/stereo types and otherwise
  leaves the port type unspecified.
- `wrapper/vst3/wrapper.rs:746` matches arrangements only by channel count.
  At line 803 its reverse mapping invents a mask for unfamiliar widths;
  `(1 << n) - 1` also requires correction before a 64-channel bus can use it.
- The proposed bus contract must reconcile all 64 ordered inputs with NIH's
  output-width processing buffer. Splitting a vector into main and surplus
  buses does not by itself establish truthful Ambisonic metadata for those
  partial vectors. The proposal must explicitly resolve this before claiming
  native interoperability.
- SOTF's consuming VST3 host also needs coverage. `sotf-host/src/external_plugin/
  vst3_backend.rs:1505` derives arrangements only from channel counts during
  `initialize_component`, reached by `load`. Its helper at line 1615 maps four
  channels to quad and accepts no 9/16/25/36/49/64-channel arrangement. It also
  negotiates one main bus per direction. A native wrapper test alone therefore
  cannot prove that a packaged higher-order plugin loads and processes through
  the application's own external-plugin host. Typed negotiation and actual
  loaded-binary audio belong in the follow-up acceptance requirements.
- SOTF's CLAP host currently calls `query_audio_channels` and then activates
  the instance (`clap_backend.rs:380`); it does not select an audio-port
  configuration or validate Ambisonic ordering/normalization first. The native
  proposal must cover configuration selection in both loaders.
- Per-instance persistence already travels through
  `PluginSettings::External { state }`, `convert_external` and
  `ExternalPluginState`. The state currently stores the descriptor, sandbox
  mode and opaque bytes, but no typed bus request. The setup route must keep
  instance configuration distinct from globally scanned plugin metadata,
  survive preset/project reconstruction and reach the isolated worker used by
  normal external-plugin loading. An in-process fixture alone does not prove
  the application's default route.
- All eight existing speaker targets remain required, including 9.1.4's
  fourteen outputs and 9.1.6's sixteen outputs. Exact arrangement masks need
  matching buffer-channel order: validate any permutation between SOTF's
  internal named-layout order and the native format's channel order with
  distinct signals and full output waveforms.

These are specification/source findings for the follow-up design, not executed
native order-7 test results. Existing exported layout/configuration identities
and other plugin families must retain their established behavior.

The output mapping also requires a standards check. CLAP's official
[surround extension](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/surround.h)
provides per-channel speaker maps. Its current role list includes front-center
neighbors and height/side/rear roles, but no left-wide/right-wide roles; those
must not be silently relabeled. VST3's official
[speaker definitions](https://raw.githubusercontent.com/steinbergmedia/vst3_pluginterfaces/master/vst/vstspeaker.h)
distinguish `k91_4_W` / `k91_6_W` (wide channels) from arrangements using
left/right-center channels. The concrete mapping must match the SOTF layout's
actual speaker roles and record any native-format representability limit.
The [concrete role table](native-speaker-role-mapping.md) has since passed
Astra's independent source/specification review. The proposal retains all 56
order/layout combinations for bridge, FFI and VST3. Standard CLAP metadata
covers 42 combinations; wide-output interoperability stays an explicit open
item requiring a suitable standard or separately reviewed channel-map contract.

Astra's subsequent control-route review also found that a Player update can
succeed with no engine, and playback reconfiguration can fail after the new
host has already committed. Consequently setup must distinguish configured,
applied and error states; an error alone does not prove the old host survived.
Astra accepted the revised design for model/preset ownership and reporting in
the existing candidate-host route. See [AUD135 review](reviews/AUD135-astra.md);
no new manager rollback or broad transition protocol is claimed.

### AUD135 pre-edit route baseline

Before native adapter production changes, the new bridge and C-ABI tests passed
all 56 order/output tuples. The bridge test verifies exact equality between a
257-frame call and irregular partitions. The C-ABI test compares the complete
output vector against a separate bridge execution, with distinct deterministic
input on every channel. These are route/partition equivalence checks; the
independent harmonic and decoder accuracy evidence remains in AUD133.

| Executed baseline | Result | Log SHA-256 |
|---|---|---|
| Bridge, all 56 tuples | 1 test passed | `15ad7ac7b058f5febe99d9cd8fa7dbbe0845b51837fd325ffd4771cfbf13371d` |
| C ABI, all 56 tuples and waveform capture | 1 test passed | `4070ae0ea7162ae3ce02bf3ab6425a7b67a53000a911322e163de1a844ab755b` |
| NIH default order 1 → 5.1 waveform capture | 1 named test passed | `59892754313a19e2164dc10352df82157e7b282d5196a02c00c5aa565bfc512f` |

Logs are `/tmp/sotf-aud135-bridge-all56-preedit.log`,
`/tmp/sotf-aud135-ffi-all56-preedit.log`, and
`/tmp/sotf-aud135-nih-default-preedit-final.log`. The initial NIH command
selected zero tests and is not capture evidence; the final command selects and
passes the named ignored capture. Its separate unrelated integration binary
selects zero tests. Vendored NIH reports an existing unused-import warning in
`wrapper/clap/util.rs`; no strict native lint gate is claimed here.

Root checked the 49-entry source/lock manifest after these runs:
`/tmp/sotf-aud135-preedit-route-start.sha256`, aggregate
`3f00a2911cf912462b8334a9baa726158cb7b965c06b4a759489737d9241ec4f`.
All entries matched at that pre-edit checkpoint. Both audio artifacts are under
`crates/sotf-plugins/target/audit-artifacts/aud135-preedit/`:

- `ambisonics-ffi-all-56.f32le`: 605,040 bytes, SHA-256
  `52e9b4f5d7ffd02f28fca98d79dfa5c7d79115411e6932e47d695f697dd172c7`.
  Despite the suffix, this is a tagged container: `SOTF-AUD135-FFI\0`, then
  records with little-endian `(u8 order, u8 target index, u16 input channels,
  u16 output channels, u32 frames)` and interleaved f32 output. Root parsed all
  56 records, checked their ordered tuples and 257-frame counts, finite/nonzero
  vectors and exact file end. Per-tuple peaks range from 0.0220801 to 0.268089.
- `ambisonics-nih-default-order1-5.1.f32le`: raw interleaved f32le, 6,168 bytes
  (257 × 6 samples), SHA-256
  `6d8cab6f11e304014a35a9a9aa98506082ae504a86f028182d37250ae099e228`.
  Root verified finite/nonzero samples and a peak of 0.0438229.

The NIH capture exercises the Rust wrapper directly. It does not establish
packaged CLAP/VST3 metadata, loading through SOTF, isolated-worker processing,
mounted setup selection or active-instance acknowledgment. Those remain
AUD135 implementation and validation requirements.

### Native buffer admission regression

The new NIH BufferManager regression reaches the existing many-input/fewer-
output guard with a 64-input/16-output primary bus. Root inspected the expected
failure in `/tmp/sotf-aud135-buffer-manager-red2.log`, SHA-256
`5e6c8ff3954bb493e772b4b34f8a0a3ece62343a3bdcafa8c48a2e78b3cee2a2`:
one selected test fails at that guard before it can exercise its later alias,
sample-content or output-write-bound assertions. This established the pre-edit
admission defect before the later assertions could execute.

The first command stopped during offline dependency resolution for optional
standalone dependencies; it did not execute a test. The corrected command used
a temporary NIH harness with the checked-in source copied unchanged and the
disabled standalone dependency entries removed from its test manifest. Its
warnings include the resulting unrecognized `standalone` feature condition and
the prior unused CLAP import. This is a qualified ordinary Rust unit-test run,
not a strict lint or Miri execution (the test happens to live in a `miri`-named
module). The implementation prepares input-only storage for channels beyond the
host's output width; native format metadata and loaded-host tests remain later
gates.

The first prepared-storage candidate passed two selected BufferManager unit
tests in the same qualified harness: the existing 1-input/2-output case and
the new 64-input/16-output aliased/out-of-place case. Log:
`/tmp/sotf-aud135-buffer-manager-green2.log`, SHA-256
`9380e93ec0a2bb11981cc282e5a80699aa13c676ec31f9c894da77fb7b01fbe1`.
Root verified the successful terminal result and, at this checkpoint, identical
checked-in/harness BufferManager source hashes:
`8a3bfb64fdfd4c13de7955f0c3604dd3f44922469b7a259d6e7ba78cd813df0d`.
The test checks full input routing, bounded host-output writes and scratch
pointer/length/capacity reuse. The reuse check does not measure allocation over
the complete callback. Native wrapper/format/host integration and independent
implementation review remained open at that checkpoint.

The complete copied-source NIH library suite subsequently passed 65 tests,
zero failures or ignored tests, under the same harness qualification. Root
verified `/tmp/sotf-aud135-nih-plug-lib-tests.log`, SHA-256
`1438ab15533202846a0a2b66f635ac57aa23c62b6f4dc1a8b289d0b814f82920`.
This run predates the final two focused test additions below; it does not
replace the pending packaged-format and consuming-host gates.

Astra accepted the refined framework stage on 2026-09-29. Its four
focused tests pass in `/tmp/sotf-aud135-buffer-manager-green3.log`, SHA-256
`bbab09f9c457007b8efce747d83325ed454d743a32a2f5813e0b99ccdc22fe37`.
They add nonzero offsets with output canaries, maximum followed by shorter or
missing input callbacks with cleared scratch, and an input-only layout. The
64-input/16-output case covers separate buffers and exact same-channel input/
output aliasing. The unsafe API now documents its channel disjointness and
alias requirements. Root verified the terminal four-pass result and identical
checked-in/harness source hashes:

- `buffer.rs`: `87d924d9ebd0f1fe5f99b458706a63b56349bfb943d46f7b3021a5d0140531bf`.
- `buffer_management.rs`: `4e726c376e59fb6d7e69900d3ca9e82935d51a8e4e5300e97eca2e04344b106b`.

The same offline harness qualification applies. This is neither an actual
Miri run nor complete callback allocation evidence. Native sample-format
negotiation, packaged plugins and consuming-host integration remain
open; Luna has resumed those implementations. See the incremental framework
acceptance in [Astra's review](reviews/AUD135-astra.md).

At that checkpoint the NIH VST3 wrapper advertised only `kSample32` through
`can_process_sample_size`; its `setup_processing` checks the supplied format
with a debug assertion (`nih-plug/src/wrapper/vst3/wrapper.rs:854-879`).
Native format tests must verify that declared f32 route and safe rejection of
unsupported formats. An f32 BufferManager pass cannot establish f64 native
processing or conversion. Any consuming-host f64 conversion route needs
separate identification and evidence; no f64 support is inferred here.

The subsequent native wrapper implementation compiles with
`plugins-nih --features ambisonics --lib --no-run`. Root verified the terminal
successful result in `/tmp/sotf-aud135-native-wrapper-compile3.log`, SHA-256
`a0f32244510f100e8ecfe89085e9ae19271f2035d23432fcb88be8b7944279c4`.
This is compilation evidence only. New input-validity state and format/wrapper
edits supersede the frozen framework hashes above; direct tuple tests, actual
ABI callbacks, packaged loading and consuming-host/UI gates remain separate.
The existing order-7 AIFF test exercises ordinary `process_at`, not EOF.
[AUD140](channel-changing-host-eof.md) records the newly identified generic
host refusal for nonzero channel-changing serial drains.

The direct Rust wrapper matrix subsequently passes four tests, with one manual
capture utility ignored, in `/tmp/sotf-aud135-native-wrapper-matrix5.log`,
SHA-256 `43020afad717d6853c718e8aa24f218c2a071aa2138fe878d399d299a9334c50`.
It exercises all 56 VST3 and 42 standard CLAP tuples plus default pre-edit
waveform equality. Earlier iterations exposed and corrected order-squared
instead of `(order + 1)`-squared input counts and wrong 9.1.4/9.1.6 VST3 wide
channel permutations. Root verified the terminal passing result; this remains
direct wrapper evidence, not actual format ABI callbacks or packaged loading.

The next in-process ABI checkpoint passes two tests in
`/tmp/sotf-aud135-native-callbacks4.log`, SHA-256
`166e312a9c333a2eef403801ede04efedfddd13b1d544be6fa6a0201307889ee`.
The CLAP test calls the `clap_plugin` vtable for the standard order-7 64→12
route; the VST3 test calls COM interfaces and `IAudioProcessor::process` for
64→16. The standard CLAP catalogue excludes 9.1.4/9.1.6 wide layouts, so its
largest supported output here is 7.1.4. Both tests include unsupported-format
checks. Root verified the terminal two-pass result; source/heap/lint and
independent implementation review are still being prepared. This is an
in-process interface harness, not loading an exported cdylib through SOTF.

The preceding callback run had one passing CLAP case and one failing VST3
comparison. Its reference applied speaker permutation twice; the corrected
comparison uses bus order on both sides. Historical failing log:
`/tmp/sotf-aud135-native-callbacks3.log`, SHA-256
`a17677ca977600f9ab8cb3adba9931c0ac46125c40a5c515058107be8f7beec2`.
No passing native claim relies on that failed run.

### Native wrapper closing gate, revisions required

Astra reviewed the following snapshot and requires three corrections before
acceptance: unequal-width CLAP in-place metadata, exact negotiated input/output
geometry validation before buffer access, and activated-state configuration
selection rejection. Luna is implementing those fixes. The passing gates
below are evidence for the reviewed snapshot; they do not close these findings.

The final NIH library run passes 97 tests with one manual capture ignored:
`/tmp/sotf-aud135-native-plugins-nih-lib-final3.log`, SHA-256
`dfc482861107158507cc2c7b0a1eb9a8f9acf5b046a213b89afb18c7d2ca6e83`.
Strict package Clippy and an actual Ambisonics cdylib build also pass. The
15-entry test/lint/build source-plus-lock manifests are identical at
`0abe7b45b60c833b09ea74a279312d787382fa361142c72e79898dd1251d3ca1`;
root verified all entries against the current files before review. The built
`crates/sotf-plugins/target/debug/libplugins_nih.so` hashes to
`cba6084e60df2884c4c322b95a09a8e9c20ed9519822f4a173e9cc5c810f8933`.
The package lint gate retains one non-fatal vendored NIH unused-import warning.
The artifact build does not prove loading by SOTF's external host.

The final library run includes both native ABI callback tests; a separate
callback/canary gate also passes 2/2 with NIH process allocation assertions
enabled. The current copied-source BufferManager harness passes four tests.
Its `buffer.rs` exactly matches current source; its BufferManager hash
`6f5b5e376e0604e782d2a638f8ebc310701ce923f5c1fee1ab1c0195a62ad818`
differs from current `bc00c7e7e4629890da7834116872beb956f7b8cd10e31e28fa38af668f7700ac`
only by assertion formatting and use-list order, as verified by source diff.
Do not describe those two BufferManager files as byte-identical. See the
[native report](native-ambisonics-orders-1-through-7.md) for command and log
hashes and the harness's offline dependency qualification.

### Next native host stage: inspected extension points

The following observations use current source slices, not an executed
Ambisonics host regression. They refine the already accepted AUD135 design:

Astra has now accepted the corrected framework/native-wrapper ABI stage in
`reviews/AUD135-astra.md`: 100 library tests, strict native lint and the
corrected production cdylib build pass. This supersedes the historical
three-finding checkpoint above. Luna has started the consuming-host stage.

- `sotf-host/tests/native_clap_host.rs` contains ignored tests that actually
  load a gain cdylib in-process and in `sotf-external-plugin-worker`, selected
  by `SOTF_TEST_CLAP_PLUGIN`. `native_vst3_host.rs` provides the corresponding
  in-process gain test using `SOTF_TEST_VST3_PLUGIN`. These are usable loader
  harnesses, but their stereo gain coverage does not establish Ambisonics.
- `external_plugin/clap_backend.rs::initialize_instance` initializes, queries
  widths and activates immediately (lines 364–405). It does not select a
  typed audio-port configuration first. `vst3_backend.rs::initialize_component`
  derives arrangements from discovered widths (1491–1519); its
  `speaker_arrangement` helper (1615–1629) maps width four to a quad speaker
  layout, not ACN/SN3D, and cannot distinguish equal-width named targets.
  Typed setup must supply the exact role contract before activation.
- `external_plugin.rs::from_placeholder_state_with_max_block_frames`
  (178–208) constructs/activates the default backend before loading opaque
  state. The new typed setup and saved structural parameters must be checked
  against the negotiated bus geometry in the restored instance. A successful
  serialized-value roundtrip alone cannot establish this.
- `ExternalPlugin::new_with_max_block_frames` currently copies active backend
  channel counts into `resolved_descriptor.audio_inputs` and `.audio_outputs`
  (lines 121–131). Preserve the accepted distinction between discovery facts
  and the selected per-instance tuple when adding typed setup. A test must
  check that selecting two different supported tuples from one scan descriptor
  changes effective graph/backend widths without rewriting those scan facts
  or weakening descriptor/state consistency. The generic unresolved native
  probe still needs to resolve metadata; this requires deliberate handling,
  not deleting metadata resolution globally.
- `ExternalPluginState` currently contains descriptor/sandbox/opaque data with
  no typed setup. Worker startup in `bin/external_plugin_worker.rs` (190–218)
  restores a state only when opaque bytes are nonempty, and
  `worker_restore_state` (266–280) reconstructs the envelope with `new`.
  Adding a field alone would not preserve it through these paths. Cover a
  freshly configured setup with empty opaque bytes as well as preset reload.
- The existing isolated gain test repeats the same block and inspects the
  settled output. The Ambisonics test must use distinct successive blocks and
  per-channel signals, observe worker sequence/completion behavior, and check
  the expected one-block transport latency. This is needed to detect stale or
  reordered audio rather than merely recognizing one repeated waveform.

Preserve generic plugin defaults and discovery descriptors; setup is per
instance. Actual exported-library CLAP 64→12 and VST3 64→16 audio, saved/fresh
instances, isolated loading, candidate rejection, mounted applied-state
acknowledgment and AUD140 EOF integration remain required. None is inferred
from the wrapper gate above.

### AUD135 consuming engine and UI: current extension points

Inspected 2026-09-29 while Luna implements the native host stage. These are
source findings for the already accepted proposal, not executed UI/engine
acceptance. The DAW graph was rebuilding; the sibling SOTF project is not
indexed. Current file slices were checked directly.

| Route | Current behavior | Required implementation evidence |
|---|---|---|
| Facade `src/factory/create.rs:691` and `:855` | Ordinary and sandbox-options constructors reject differing `descriptor.audio_inputs` before typed state restoration | Both paths admit validated selected input width while retaining discovery descriptor facts; invalid typed state is rejected |
| Engine `PluginSettings::required_input_channels`, `plugin_settings.rs:1701` | Returns the external descriptor input width | Uses the selected effective input width for typed native setup, preserving generic external defaults |
| Engine `plugins/chain/plugin_chain.rs:863` | Conflict detection propagates the descriptor output count | Propagate selected effective output width when validating subsequent plugins |
| SOTF `plugin_graph.rs:1295`, `:1412`, `:1889`; `plugin_graph/plugin_graph_node.rs:81` | Graph output propagation and node input/output ports use descriptor counts | Selected per-instance widths propagate through rack, graph and canvas without rewriting the scan descriptor |
| SOTF `components/plugins/custom_view_registry/render.rs:170` | External-plugin Channels label displays discovery counts | Distinguish discovery facts, desired setup and observed active tuple in the setup UI |
| Engine `processing_thread/build.rs:58–79`, `:99–108` | Linear builder warns and skips width mismatches or plugin creation failures | Selected native Ambisonics setup/load failures make the candidate fail, including malformed setup; graph builder already fails on a node error |
| SOTF `player.rs:164–202` | Both chain and graph updates convert `No engine running` into success | No-engine configuration stays pending; an ordinary success value is not proof of an active instance |
| SOTF `app/player_handle.rs:493–510`; `ui/plugin.rs:29–73` | Receipts wrap the same Player calls; UI checks success and a submitted-graph snapshot | Observe the matching active native tuple before showing applied; preserve protection against an older graph completion accepting a newer choice |
| Engine `manager_thread/apply.rs:221–249`, `:363–382` | Playback reconfiguration can fail after the host commit and then return an error with playback stopped | Distinguish committed-new-host playback failure from candidate rejection; never infer rollback from an error alone |
| Engine `manager_thread/wait.rs:83–147` | Matching request/generation ACK returns output width, rate and latency; claimed updates reconcile beyond the initial deadline | Existing correlation is useful, but counts alone do not identify same-width named speaker layouts; retain actual selected-instance observation |

The generic Player receipt returns after the manager operation, which is
stronger than queue acceptance but still insufficient for the accepted native
setup status contract. The implementation should use the existing candidate
route and reconcile from the active instance. If an outcome cannot be
classified, expose the accepted unknown/pending state until observation
resolves it. This work does not authorize or require a broad manager rewrite.

The native owner received these locations, including both facade width gates
and all four inspected graph width consumers. Actual packaged-library tests
must cover the consuming paths after the direct-host stage is complete.
`plugins/plugin.rs:69–93` already enforces an isolated, loadable audio effect
for rack admission. Keep that existing policy; direct-host ABI tests do not
justify relaxing rack isolation.

### AUD135 native state entry points under implementation

The current facade integration test sends a distinct impulse through each of
ACN 0–63, compares complete native output with a separately configured direct
decoder, checks that scan metadata remains 4→6, and restores typed setup plus
nonempty opaque state. This establishes the intended transport and persistence
checks. Its direct decoder comparison is a route oracle.
Independent decoder mathematics remain supported by the accepted AUD133 tests.

Root's source inspection identified three entry points that need consistent
validation: `ExternalPlugin::from_placeholder_state`, `SerializablePlugin::deserialize`
and `Plugin::load_opaque_state`. A constructor-only test cannot establish the
other two paths. Empty opaque state must retain the explicitly selected setup;
nonempty opaque state must agree with it. A contradictory saved order or named
target must be rejected before replacing the active instance. The unchanged
instance must still emit the correct waveform after that rejection, including
same-width target conflicts. Native metadata width alone does not establish
that the decoder's structural parameters agree. These requirements were sent
to Luna; they are pending implementation evidence, not a passed restore gate.

The first actual facade command exited 101 with both CLAP and VST3 tests failing
before rendering: the selected-instance constructor checks internal ID `order`
through `Plugin::get_parameter`, which returns `None`. See
`/tmp/sotf-aud135-order7-native-facade.log`. No high-order waveform or restore
assertion ran in this checkpoint. Current CLAP discovery skips hidden/read-only
parameters and assigns IDs of the form `clap.{numeric_id}`; VST3 uses
`vst3.{numeric_id}`. NIH intentionally hides the structural parameters. A
correct read-only native setup check must preserve those generic identifiers
and the structural control-thread policy. Returning the requested setup itself
would not prove that the loaded decoder agrees with it. Luna subsequently
implemented format-specific structural readback and transactional replacement.

The next real-artifact run, `/tmp/sotf-aud135-native-restore-focused1.log`,
also exits 101 with two failures. VST3 reports plain order 7, which a mistaken
zero-based bound rejects. The CLAP regression reaches saved-state restoration:
valid order-one opaque state is accepted into an order-seven candidate because
NIH's restore path reinitializes the plugin, and the SOTF wrapper writes the
negotiated layout into the hidden parameters before host readback. Astra
confirmed that the fixture is valid and the overwrite masks the conflict;
see `reviews/AUD135-astra.md`. Luna is correcting validation before that
overwrite. A deliberate setup change still needs its separate synchronization
path and preservation of unrelated controls. Wrapper changes require a newly
built artifact and executed loader tests; the previous artifact's gates do not
cover them.

The marker-based wrapper correction and VST3 plain-value bound fix subsequently
compiled into a fresh production cdylib. Root verified build-log SHA-256
`cd08f2032d9a362af9c76f35bbde54b1ac77d74f01736f551f38eac3a679619b`
(`/tmp/sotf-aud135-native-restore-cdylib.log`) and rebuilt artifact SHA-256
`54c29a62f65c93ee65aa257b521089c8510d385388229413d4669602a00de20a`.
The prior artifact remains saved at
`target/audit-artifacts/aud135-native-host-pre-restore-fix/libplugins_nih.so`
under `crates/sotf-plugins`, with its original `990c7f1e…` hash.

The next actual loader gate passes both CLAP and VST3 tests (2/2), including
order-seven audio and matching/conflicting saved-state cases. Root inspected
the terminal result in `/tmp/sotf-aud135-native-restore-focused2.log`, SHA-256
`f1ab098fe438ca5b31ce163210f553a2126317913802bc0750b43da09cc5bcae`.
Warnings about rejected plugin reinitialization occur during the intentional
conflicting-state cases. Luna is adding nondefault unrelated-control
persistence coverage; marker lifecycle checks, deliberate reconfiguration,
isolated workers and consuming engine/UI validation remain separate required
evidence. This is a successful focused checkpoint, not Astra acceptance of
the revised wrapper or complete native integration.

After Luna added the restore-marker unit test, root ran the updated NIH library
suite and strict all-target Clippy. The suite passes 101 tests with one manual
utility ignored; Clippy passes. Both gates have matching before/after selected
30-file manifests, aggregate
`25d4a747598097436d4429489d61e9a5210f4e692540cf738dbf6d0f472cd3b9`.
The unit log is `/tmp/sotf-aud135-nih-restore-unit.log`, SHA-256
`4d4d0b313795cdc0ea62702c1b5017b0c5575b4b354b0bb31b79ff5037c7cbc5`;
the strict-lint log is `/tmp/sotf-aud135-nih-restore-clippy.log`, SHA-256
`6e33de22a931b9aa310052ea91a46fdc3da05c9c0a27d47b0bd91beaeb0c6b14`.
Sibling `-receipt.json` files record the exact commands. These are selected
wrapper/dependency inputs, not a whole-workspace validation. The separate
facade test is receiving unrelated-control persistence assertions.

The isolated worker's current source does forward typed setup from its initial
state into IPC sizing and startup restoration, including an empty opaque blob.
`worker_restore_state` now clones the envelope and changes only sandbox mode.
These inspected paths supersede the earlier omission finding above, but actual
isolated-worker order-seven audio and saved/fresh-instance tests remain open.

### Existing paths for observing an active native instance

Further source inspection found an existing request route that may support
the accepted setup-status contract without a new manager transition protocol:

- `AudioEngine::get_plugin_data` sends `GetPluginData(index)` through the
  manager. `manager_thread/commands/get_plugin_data.rs` waits for the matching
  processing request ID, with a bounded timeout and cancellation.
- `DawHost::get_plugin_data` resolves the current chain node and calls that
  actual plugin's `get_data`. A prepared, read-only native observation could
  use this route; none is implemented or tested by this research.
- The nonblocking `get_cached_plugin_data` route is narrower: the updater in
  `processing_thread/processing_state.rs` visits only `analyzer_indices`.
  Adding external-plugin `get_data` alone would not populate that cache.
- `GraphTopology` carries graph nodes/edges, while
  `IsolatedExternalPluginWorkerReport` carries plugin identity, worker and
  sandbox status, and failure counters. Neither currently identifies the
  selected native order and named speaker target.

Any implementation must correlate the observation with the intended plugin
instance and current selection, distinguish same-width targets, and retain
unknown/pending on absent or stale evidence. Keep synchronous requests off the
UI render/polling path. Worker readiness and failures also remain relevant:
requested metadata or a graph's channel count does not prove a loaded worker
accepted that tuple. These are extension points for Luna's later UI work,
not a newly approved protocol or completed activation evidence.

## Inspected source provenance

### Sibling resolver qualification

The original sibling lock hashes to
`2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`.
The working test resolver `/tmp/sotf-aud133-resolved-minimal.lock` hashes to
`475d5890fddf40c8ca8361c6455057c1f3701c713c5844f2cdfda5a53303fb4e`.
It changes only the resampler version relative to the earlier accepted
temporary resolver, but differs more substantially from the original lock:

- Local package versions follow current manifests: math-dsp 0.5.31,
  resampler 0.6.0 and the separately maintained IAMF 0.2.0. Resolving the last
  version does not constitute IAMF implementation or test coverage.
- DAW dependencies resolve to the local Rubato 5 fork and audioadapter 5
  packages; sibling player/CLI retain their Rubato 1 dependencies.
- Removed arc-swap edges match current Binaural, Convolution and XTC manifests.
- Unpatched git math-audio packages stay at `cabbc6dc`; upgrading them to the
  local checkout's newer commit is unnecessary and introduced an unrelated
  AnalogModel compatibility error in a discarded resolver attempt.

The original lock does not resolve the current combined checkout with
`--locked`. Earlier test results above retain their temporary-resolver
qualification. Root has now retained the exact reviewed `475d5890…` file as
the sibling working lock; no commit is claimed. Future sibling gates use that
actual working lock, with no temporary swap or restoration to the obsolete one.

Astra independently reviewed the exact `475d5890…` candidate on 2026-09-29
and cleared it for retention. All existing registry package checksums and git
identities are unchanged. The added resolver packages are the local Rubato 5
fork, audioadapter 5, audioadapter-buffers 5.2, audioadapter-sample 5.2 and
audio-codec-algorithms 0.8.1; the preceding local-version/edge changes match the
current manifests. Root installed it after the owner confirmed the preceding
UI command terminal and the original lock restored. The write checked both
source and destination hashes and completed successfully. The passing mounted
UI gate above ran against this retained lock.

### Wrapper snapshot

These full-file SHA-256 values identify the inspected wrapper snapshot. They
must not be reused as hashes of later corrected code.

| Path under `crates/sotf-plugins/crates/` | SHA-256 |
|---|---|
| `plugins-bridge/src/factory.rs` | `708d8d07151e29e2a2e82b78dc88849f14bf6081015a52c338e5a255cb42ac91` |
| `plugins-nih/src/wrapper.rs` | `9434bde382d81b9668a92e132dff8d34dd731026565a698f30ce27a09af172a8` |
| `plugins-nih/src/params.rs` | `515c6aa49920dcb1dc4d2f5d4091028e307a6f14369d59029a36d492b7733862` |
| `plugins-nih/src/params/configuration.rs` | `402d41bb6d60863b9c12a826461ef9fa762fc25e501281e511006f963c070138` |
| `plugins-ffi/src/plugin_factory.rs` | `2fe089dd6cbdd7235237c9da6e9197c8209e7c5ddc4f6ceec0052aebe87b4d13` |

The initial inventory was source-only; later executed gates are listed above. See
[AUD133 review](reviews/AUD133-astra.md) and
[AUD134 review](reviews/AUD134-astra.md) for subsequent validation.
