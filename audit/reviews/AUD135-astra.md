# AUD135 Astra design review

Status: **bounded design, corrected framework/native-wrapper ABI, loaded-host restore, deliberate in-process reconfiguration, refreshed output-layout/late-refusal and isolated VST3 construction checkpoints accepted in their stated scopes. Actual engine replacement/commit, isolated CLAP, UI and EOF remain open.** The final checkpoint below distinguishes detached construction from replacement of the live worker.

## Loaded-host restore checkpoint: scoped acceptance

The imported-conflict finding below is closed for the tested in-process host
routes. DynamicParams records deserialization, and Ambisonics initialization
consumes that marker to compare the restored tuple before setting the negotiated
layout. The VST3 plain order check correctly uses 1..=7. Host constructor restore,
`load_opaque_state` and preset deserialization validate replacement backends
before committing them; rejection does not apply the failed candidate to the
existing live backend. Hidden ABI readback remains an additional check rather
than the sole evidence after normalization.

Inspected params SHA `902e9425c9cf6c86e49f5caf1f501afd5c42b82764d269869b96620f8263a198`,
wrapper `3d1c93591adcee5a75753b39a271fb8c944b3c6ebc7c6457d0774d41c447b3ea`, and
public test `3d6a06655942431bbf200707fb97f5635b183277c4bf9aab4c5f5d5bc9a4c718`.
Verified loaded exported CLAP 64→12 and VST3 64→16 test log
`df4c884e279ed2b51ee535534bda1db4b75408e8e59ab2418a5151e623adae63`:
2 passed, including high-ACN audio, matching restore, order conflict and
same-width target conflict. A nondefault Max-rE=false opaque value is serialized
and produces audio distinct from the default control. Discovery metadata remains
4→6 while effective selected widths are separate. The artifact receipt identifies
`54c29a62f65c93ee65aa257b521089c8510d385388229413d4669602a00de20a`.
NIH library 101 passed / 1 manual ignored and strict all-target lint terminals
and hashes match the handoff. No reviewer Cargo or production edits.

This does not close deliberate tuple reconfiguration: consuming a restore marker
on mismatch and accepting a later unmarked initialize is not evidence of an
intentional host operation preserving unrelated controls. That route must have
explicit ownership and its own executed test under refinement #4. Nor does this
prove rollback for an arbitrary host loading directly into the same NIH instance;
the accepted SOTF preservation behavior uses a replacement candidate. Isolated
workers, engine/UI routing, all-56 post-edit parity and EOF remain open.

## Historical consuming-host restore diagnosis (closed for checkpoint above)

The failed CLAP case in `/tmp/sotf-aud135-native-restore-focused1.log` is a
valid contract regression, not invalid merely because post-load ABI getters
report order 7. Source tracing shows NIH `set_state_inner` first deserializes
the incoming parameter state, then calls `initialize` with its existing
negotiated layout when a buffer configuration exists. SOTF Ambisonics
`initialize` unconditionally calls `set_ambisonics_layout`, replacing imported
order/target parameters with that layout before host post-load validation.
DynamicParams includes these structural entries in its parameter map; hidden
does not make the serialized tuple absent. Consequently current getter equality
proves normalized state, not agreement of the imported opaque tuple.

Validate the imported structural tuple before reinitialization overwrites it.
A separate never-activated inspection instance or an explicit validated restore
hook can provide that observation; simply deactivating an already initialized
instance may leave `current_buffer_config` present and still trigger reinit.
Keep import validation distinct from deliberate layout change, where selected
setup may intentionally replace old tuple fields while preserving unrelated
controls. Do not weaken the conflicting-import fixture to accept the normalized
getter values. Test matching restore, conflicting imported tuple rejection with
unchanged live state/history, and intentional reconfiguration retaining a
nondefault unrelated control. Demonstrate the source blob's tuple before the
normalization step.

This is a read-only diagnosis of moving native sources, not final acceptance.
Inspected NIH CLAP wrapper SHA
`93069a4179a468fc642d2a2325ef963a06f880fd04f89b2aa88e29383e48dae1`,
SOTF wrapper `108e07429547b0444bbb21e404a73188f58ab11b3be974833558f6b8c9e15cda`,
params `0f2b53392012aae8b6c1658c509f1de609e59c9a507bf95e37fc4fb0ecdeed69`,
and ExternalPlugin `9150aa19d4a62efed0c15428e185212fa8f3ba5d3d6cdd4bf51426bd247211c0`.
No Cargo or production edits. Earlier wrapper/ABI stage acceptance remains
scoped; consuming-host persistence remains open.

## Corrected wrapper stage: scoped acceptance

Reviewed report SHA
`d60d4d171da1f6860b0a946ebf99203a0dceec50e038a37b89e9d1d29fde27ec`
and current CLAP/VST3 wrapper and native callback tests. All three findings below
are closed for this stage:

- Unequal main widths produce `CLAP_INVALID_ID` for in-place pairing.
- Required main input and all output buses require exact negotiated counts and
  channel widths before BufferManager access. Null pointers, surplus buses,
  unsupported f64 and excessive block sizes are rejected. Optional auxiliary
  input omission/shortening retains Gate's prepared-silence compatibility.
- CLAP configuration selection checks activation separately from processing,
  covering active-before-start and stopped-before-deactivate as well as active
  processing. Deactivation permits a subsequent supported selection.

The malformed-input rejection twin now enables Dual-Band via the real inactive
CLAP parameter-flush route before activation. Equal continuation after rejection
is accompanied by an extra-valid-block positive sensitivity assertion, so the
test no longer relies on stateless matrix output to infer no advancement.
ABI geometry/canary tests and the Gate regressions exercise the production
wrapper callbacks rather than bypassing their validation.

Inspected terminal evidence: full NIH library 100 passed / 1 manual ignored;
focused callbacks 5 passed; optional Gate auxiliary regressions 2 passed;
strict native all-target Clippy completed (vendored unused-import warning remains
qualified); refreshed copied-source framework harness 10 passed. The harness is
ordinary Cargo testing with offline dependency adaptations, not Miri. Callback
allocation guards do not establish initialization or full transport allocation
freedom. The report's production cdylib artifact SHA is
`990c7f1eec99d508ef223ce957af852e0a709a625e6c0d522225513c86a852c7`.

Independently verified start/end manifest equality for full-test, lint and build
gates. Current lint/build manifests match every entry. The full-test manifest's
only current difference is the separately changed shared `daw_host.rs`; native
source overlap is unchanged. Do not describe these gates as one identical
whole-tree snapshot. No reviewer Cargo or production edits.

This accepts framework and in-process native-wrapper ABI behavior only. Actual
packaged plugin loading through SOTF, typed per-instance setup and persistence,
isolated reconstruction, applied-state/UI reactivation and EOF remain required
by the accepted design. CLAP wide-role interoperability remains explicitly open.

## Historical native wrapper/ABI checkpoint: findings now closed above

Reviewed all 15 current entries in final manifest
`0abe7b45b60c833b09ea74a279312d787382fa361142c72e79898dd1251d3ca1`,
including actual CLAP C and VST3 COM callback tests. Final library gate is
97 passed / 1 manual ignored; package lint and cdylib build finish successfully.
The report correctly leaves packaged-host loading, typed persistence, isolated
worker, UI/reactivation and EOF open. Three scoped findings remain:

1. **P1 — CLAP unequal-width in-place metadata.**
   `ext_audio_ports_get` still pairs primary input/output whenever both exist,
   including 64→12. The accepted policy required invalid pairing unless full
   alias safety was explicitly established. Return invalid for unequal widths
   and assert the actual callback metadata, or provide the complete supported
   alias contract and evidence before claiming that exception.
2. **P1 — exact callback geometry before buffer access.**
   BufferManager's completeness flag accepts `supplied >= declared`; surplus
   input is clipped in release or triggers debug assertions. Missing/truncated
   output becomes empty slices, which the wrapper later indexes while gathering
   output-backed input channels. Validate required input/output bus counts and
   exact channel widths before BufferManager copies/writes. ABI tests must
   reject absent, short and surplus buses without panic, DSP advancement or
   writes outside valid buffers. Current insufficient-input coverage alone is
   not sufficient for the declared whole-vector contract.
3. **P2 — CLAP config selection lifecycle.**
   `ext_audio_ports_config_select` stores a new layout with no activated-state
   guard. Track activation (not just processing) and reject changes before
   deactivation, preserving metadata and DSP state. Exercise successful
   deactivate/select/reactivate plus rejection while activated but not yet
   processing and while processing.

Speaker-role tables and native output permutations agree with the reviewed
mapping on inspection. f64 rejection is intentional, not f64 processing
support. Copied BufferManager harness differs from current source only in
formatting/import order per the preserved diff; do not claim identical bytes
for that file. `buffer.rs` matches exactly. No actual Miri or initialization/
transport-wide allocation claim follows. No reviewer Cargo or Rust edits.

Final design snapshot `290e4c198a74747391a3c6adf15025551a90b78933308c88b1eea1f60c16b428`
closes the two control-route qualifications below. It distinguishes configured
but inactive, rejected before commit, committed with stopped/error playback,
and an explicitly unknown outcome awaiting active-instance reconciliation.
Desired/applied persistence consistency and actual matching instance/tuple
observations are implementation acceptance gates. No manager rollback or
general protocol redesign is approved or required. Proceed with the bounded
implementation; CLAP wide layouts remain an explicit full-goal limitation.

## Current revised proposal review

### Incremental NIH framework review

**Framework stage accepted after focused refinement.** Current BufferManager
SHA `4e726c376e59fb6d7e69900d3ca9e82935d51a8e4e5300e97eca2e04344b106b`
and Buffer SHA `87d924d9ebd0f1fe5f99b458706a63b56349bfb943d46f7b3021a5d0140531bf`
match their copied harness sources. The explicit unsafe alias contract and
new tests close the requested framework findings: offset 7 with separate/exact
alias buffers and outside-block canaries, offset 3 at prepared maximum,
consecutive shorter/missing inputs with cleared suffixes and unchanged scratch
addresses/capacities, and input-only layout.
`/tmp/sotf-aud135-buffer-manager-green3.log` (SHA
`bbab09f9c457007b8efce747d83325ed454d743a32a2f5813e0b99ccdc22fe37`)
passes all four focused tests. Earlier 65/65 library run predates the final two
tests; the harness strips disabled standalone-only optional dependencies for
offline resolution. This is neither an actual Miri run nor strict lint/full
callback allocation evidence. Native f32/f64 wrapper and complete feature gates
remain open. No reviewer Cargo or Rust edits.

Historical first-pass source/evidence review follows:

Reviewed BufferManager SHA `8a3bfb64fdfd4c13de7955f0c3604dd3f44922469b7a259d6e7ba78cd813df0d`
and Buffer documentation. The prepared scratch suffix preserves the full input
vector without adding host output channels. Its returned borrow follows the
existing auxiliary-storage lifetime pattern. No new defect identified for
valid negotiated, disjoint channel buffers with exact same-channel aliasing.
The copied-source library gate reports 65/65; no Miri, native f64 or complete
callback allocation claim follows from that harness.

Focused framework evidence still needs nonzero offsets, consecutive varying
and maximum lengths, shorter/missing input clearing, and an input-only layout.
The unsafe pointer contract should explicitly state output disjointness and
permitted input/output aliasing. Missing whole input intentionally retains the
legacy output-backed prefix behavior; native validation must reject such a
missing primary vector before DSP. Sent these bounded refinements to the owner.
Framework evidence remains provisional; native whole-chain acceptance is open.

Reviewed proposal SHA-256 `19771d5894c2adb72f3625de285ea0ffd9c71b65829e0d49e88c965b6b77203d`.
The optional generic setup, stable identity/capability validation, deliberate
reconfiguration versus imported-state precedence, and exact per-format role
maps resolve the earlier design findings. Root approved a bounded CLAP stage
of 42 tuples; its two wide targets remain explicit full-goal backlog. VST3,
bridge and FFI still require all 56 tuples including 64-to-16 processing.

Two source-grounded refinements remain:

1. `Player::update_plugins` and `update_plugin_graph` return success when no
   engine is running. That return cannot establish processing-thread application.
   Keep configured/pending state distinct from an actual active-instance receipt.
2. `apply_plugin_update_once` commits the host before playback reconfiguration.
   Playback reconfiguration can then fail with the new host committed and
   playback stopped. Limit old-host preservation to failures before commit;
   expose the existing post-commit safe/error state truthfully without expanding
   this task into host rollback or manager protocol redesign. Identify ownership
   of rack/graph/preset desired setup on rejected preparation: Player's saved
   playback configuration alone does not own those models, and graph update
   does not update that saved configuration.

Read current source directly while TokenSave rebuild was active; the sibling
project has no initialized graph. No Cargo or production edits. Staged baseline,
bridge and FFI implementation may proceed while the owner corrects this narrow
control-route contract; overall native feature acceptance remains pending.

## Historical first-pass findings

## Speaker-map subreview

Root artifact `audit/native-speaker-role-mapping.md`, SHA-256
`714341ef697f09c182b1c28def73ded293c955a3eb6cfb6c7d1849d93eef74fb`,
passes independent source/specification review. Re-extracted all eight layouts
by numeric SOTF channel index, independently recomputed masks and ascending-bit
bus permutations, and checked the official VST3 speaker header. All rows agree.
The two-height 7.1 layout needs the TF variant; wide variants use bits 59/60,
placing wides after height channels in native bus order. Contextual surround
mapping distinguishes 5.1's single surround pair from separate 7.1 sides/backs.
CLAP maps agree with its standard role IDs; no standard wide ID exists.

No table correction needed. This accepts the table as a design reference, not
native execution or the still-pending wide-target compatibility/state policy.
Source: [official VST3 speaker definitions](https://raw.githubusercontent.com/steinbergmedia/vst3_pluginterfaces/master/vst/vstspeaker.h).
Reviewed proposal snapshot `b757a7c00467da128903931500f6798ae3bb79827cab3b304b4fe4d4ae7303a3`;
owner is revising it. Read-only baseline capture and bridge/core investigation
may proceed without treating the native design as accepted.

The complete 56-tuple named-layout scope, one full primary ACN/SN3D vector,
per-instance persisted setup and actual loaded CLAP/VST3 plus isolated-worker
tests are appropriate. Internal NIH auxiliary storage is not itself disqualifying,
but external main-port metadata, bus flags, buffer mapping and alias safety must
all describe and deliver one primary input. Partial/surplus sidechain encoding
is not acceptable.

## Required refinements

1. CLAP has official surround output metadata; names and counts alone are
   insufficient. Freeze actual output role maps alongside exact VST3 masks and
   permutations. Investigate SOTF WideLeft/Right at ±60 degrees: CLAP FLC/FRC
   means front-left/right-of-center and cannot silently stand in for wide roles.
   Document representability limits without relabeling physical speakers.
2. CLAP main port is index zero. Unequal input/output widths must not advertise
   an in-place pair unless complete alias safety is established; otherwise use
   the invalid pair ID and prepared separate input storage.
3. The generic ExternalPluginState addition needs optional/default-absent setup.
   Do not reinterpret every old external instance as order 1 / 5.1. Specify
   verified legacy Ambisonics migration and capability/identity validation
   without display-name heuristics.
4. Distinguish deliberate user tuple changes from inconsistent imported state.
   Restoring old opaque parameters during a valid setup change must not silently
   undo it or cause every reconfiguration to reject. State the synchronization
   order and authoritative owner explicitly.
5. Specify the actual existing control-thread replacement route and failure
   semantics. Preserve the previous active instance where possible; otherwise
   leave a clearly reported safe state without publishing an unapplied setup.
   A hypothetical new DawHost method is not yet a concrete integration plan.

Official headers checked: [CLAP surround](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/surround.h),
[audio ports](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/audio-ports.h),
[audio port configurations](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/audio-ports-config.h),
and [Ambisonics](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/ambisonic.h).
No Cargo, Rust edits or broad host protocol change performed or requested.

## Deliberate in-process reconfiguration checkpoint — 2026-09-30

**Scoped acceptance.** The explicit `ExternalPlugin::reconfigure_audio_setup` route closes the successful deliberate-tuple-change portion of refinement #4 for the tested loaded CLAP/VST3 artifact. Earlier text leaving this entire route unproved is historical. Late candidate-failure evidence remains required; overall AUD135 is not accepted.

Reviewed current source matches the run7 selected manifest:

- `external_plugin.rs`: `18b02b0e3db0b5a333dea4c04eb8ed27b20fddbebc877e7525608d15f0b5eee3`
- CLAP backend: `f3a4b8c305dca784bc62dae119ce969768bd101ff4af7e1f08f59506bdc0e65e`
- VST3 backend: `7ebc23d3beec812fce7197736cd09e16025f6660890a8a458c12fc5cd6be0a23`
- `native_ambisonics_host_order7.rs`: `a07fe7d5614720fd8edd48e5cbf3db1df2ddea81bd6c38b779b52387b5787074`

The control-thread route snapshots opaque state, exposed parameters and recognized hidden controls, restores a disposable backend at the old tuple, then renegotiates that candidate while deactivated. CLAP reselects/query-checks ports and refreshes parameter bindings; VST3 changes exact arrangements and output permutation, synchronizes a separate controller if present, and resizes prepared storage. The live backend is not the candidate. After negotiation, selected geometry and preserved controls are checked; a second disposable verifier restores the newly saved opaque state at the new typed tuple before committing candidate/setup/state. Imported inconsistent state remains distinct from deliberate reconfiguration. No callback-thread reconstruction is introduced.

Run7 executes exported CLAP and VST3 loading, starts with nondefault Max-rE=false, Dual-Band=true and AllRAD, changes order 1→7, changes 7.1→5.1.2 at the same eight-channel width, then selects the final 12-/16-channel target. Full output comparisons use separately configured production decoder references; they establish native routing/control preservation, not a new independent numerical decoder proof. Serialized typed setup, hidden controls/order/target and unchanged discovery metadata are asserted; restoring the final preset reproduces audio. The order-eight refusal and exact populated twin continuation establish **prevalidation refusal only**: validation rejects before candidate construction. They do not execute late import, negotiation, activation or verifier failure.

Inspected terminal evidence:

- `/tmp/sotf-aud135-reconfigure-run7.log`: 2/2 passed, SHA `48e78ef3b72a88cb72928a4ac7af43f199b7e5c50a6f7e006087b60cd12d342b`.
- Matching 57-entry source/artifact start/end manifest aggregate `539b66ff2b3bd93df64d4b7c3932967257867c35cda82330de5c4f592a8349e0`.
- Native-feature host strict all-target Clippy `/tmp/sotf-aud135-reconfigure-host-clippy-r3.log` passes, SHA `9f94a22d19395278aa34a42ccc3b849efb1a245f726af386bf43b89065a4d734`; its separate 251-entry manifest is recorded in the plan.
- Loaded binary SHA `54c29a62f65c93ee65aa257b521089c8510d385388229413d4669602a00de20a` from `aud135-native-host-restore-fix`. Subsequent shared NIH/AUD143 changes are not in that artifact and need a separately rebuilt/loading gate.

Required continuation: inject a failure after a valid candidate has begun import/negotiation, verify no commit to setup/state/active backend and exact stateful subsequent audio against a synchronized twin with a positive sensitivity control. Then complete isolated-worker, engine/UI effective-width/persistence and EOF routes. Existing same-instance NIH state loading must not be advertised as transactional merely because this detached-candidate SOTF route is. No reviewer Cargo or Rust edits; TokenSave saved approximately 19,000 tokens over the scoped reads.

## Output-layout refresh, late restore and isolated VST3 checkpoint — 2026-09-30

**Scoped acceptance.** The archived output-cache correction, populated in-process late native-load refusal/retry, engine update preflight/no-send policy, and isolated VST3 detached-worker construction/history checks are accepted. This is not acceptance of the requested engine replacement/commit route, isolated CLAP, UI, or EOF. No production edits or reviewer Cargo runs. TokenSave status reported indexing in progress; bounded current and archived source bytes were authoritative.

### Source and behavioral assessment

The Ambisonics VST3 renegotiation now refreshes `output_bus_count=1`, widths `[selected_output_channels,0,0,0]`, active mask 1 and clears the BandSplit-specific layout marker alongside metadata/permutation/storage rebuilding. This repairs stale bus width after a successful wide→narrow change. It is control-side work on the disposable replacement backend. The exercised dual-band sequence processes after expansion, shrink to eight channels and re-expansion, rather than checking metadata alone.

The strengthened in-process test imports a genuine order-one opaque blob inside a valid order-seven typed envelope. Outer validation succeeds; the required native-load error confirms failure occurs during detached candidate preparation. `ExternalPlugin::deserialize` commits descriptor/setup/backend only after replacement construction and native validation succeed; its preceding host-parameter check is non-mutating. Complete typed/opaque state and populated dual-band continuation match a synchronized twin after refusal, with a cold-control sensitivity assertion. Valid subsequent restore produces exact fresh-twin audio and agrees with a separately configured decoder reference. This closes the earlier late native-load refusal condition for these in-process CLAP/VST3 artifacts, not every possible later negotiation/activation failure.

The engine change inspects failed external chain diagnostics before preparing/sending a replacement. It preserves best-effort initial startup and reports requested-candidate failure before any processing or playback command. Linear/graph command-probe tests establish no-send and retained metadata; they do not establish an actual engine worker's continuity or successful commit.

The isolated test starts real order-seven 64→16 VST3 worker processes with persisted Max-rE=false and Dual-Band=true. Complete finite output blocks match an in-process native reference with the declared 64-frame IPC delay and 1e-6 maximum residual; synchronized workers match exactly. A separate candidate worker with valid outer metadata but mismatched order-one opaque state fails at native component restoration. Existing workers' saved state and subsequent audio remain exact. Max-rE and populated-vs-cold zero-input controls demonstrate parameter/history sensitivity. A later fresh valid worker construction succeeds. **This is detached construction followed by a fresh-construction retry, not replacement of the live worker through an engine manager.** Zero-input process blocks are not EOS/drain evidence. Sleeps/deadlines make this functional IPC evidence, not realtime deadline or allocation evidence.

### Verified packets

All entries verified using the appropriate packet-relative checksum paths:

- `audit/artifacts/aud135-vst3-output-layout-refresh-r1/SHA256SUMS`: `63f4d5482c004ef1b5c3e668495a90e368fd45bad1f24a2165512d89a2b843c3`. Archived backend `41b56cbcb96c240370273e06358f19ed29d0e674f1fbcf2a49b7abc8cab44b63`; loaded test `b242fe9782a17137205ef37b3854c05e1153256e7ef7da97b18d675bf6588883`. Combined loaded CLAP/VST3 2/2 and host lint are recorded. Historical red source was not fully archived; do not describe it as a reproducible complete red/green source pair.
- `audit/artifacts/aud135-engine-external-update-r1/SHA256SUMS`: `48051d63f735305df72d62d9d313ff52809cc91f018bc13844dcbe81a074dd7f`. Green `apply.rs` is `5402853804d73809dbbcbd0a36d889c2f72d77be986298fb8963918c12ef0f29`; missing-descriptor linear/graph refusal and existing invalid BandSplit control pass. Exact red source is not retained.
- `audit/artifacts/aud135-isolated-vst3-worker-r2/SHA256SUMS`: `0c168b1fdb0483da3d7e4b1daf5a5c5561dc47ed1c27656c65295187d89aae4c`, seven entries verified. Fresh durable `worker-current-r3.log` is 1/1; focused `clippy-current-r3.log` terminates successfully. Executed test `6617421b150507482cb702cebec24c9e17c45831c71e7181c75df5316ace723b`; native artifact `7dfc7b96519b0d884a3b06633c009e3024f0a5a46a418236f43596699f74a3b8`. Historical source reconstruction is separately labeled; lost historical logs are not recreated by the fresh run.

These packets preserve selected sources/evidence, not complete transitive run-bound snapshots. Overall AUD135 remains open for actual engine candidate/commit/effective-width and EOF behavior, isolated CLAP and application control/persistence routes. Broader host/manager protocol work is not authorized by this scoped verdict.

## Actual engine EOS checkpoint — 2026-09-30

**Actual VST3 engine candidate-refusal/retry and nonzero final-program EOS route supported. General isolated finite-tail drain acceptance remains pending the concrete evidence conditions below.** No production edits or reviewer Cargo runs. Reviewed current bytes against `audit/artifacts/aud135-engine-eos-route-r3/selected-hashes-engine-r2.sha256` (aggregate `03639c7915f0ca2faf2af991cc982be484d78b1bccd304b0b046f3d5411fdc13`); all listed current source, worker, native library and log hashes match. This is selected coverage, not a complete transitive run-bound build manifest.

The actual ProcessingThread test performs late worker native-load refusal after outer typed validation, checks engine/playback/worker metadata and absence of a playback command, and compares exact populated delay continuation with a synchronized engine twin. A cold engine produces zero while retained continuation is nonzero. Valid retry uses the playback acknowledgment and commits the order-seven 64→16 route. Irregular program blocks and an ordinary silence callback precede a nonzero maximum-size 8192-frame final block; EOS is sent immediately after receiving that block's engine output, without an extra sleep. Complete output plus drain equals the direct loaded-native reference with the IPC delay prepended, finite samples, exact counts and <=2e-5 residual; drain must contain signal and the queue has no duplicate EOS at the checked point. Earlier callbacks deliberately sleep at least 250 ms, so this does not demonstrate worker realtime deadlines. This route's native tail is zero; its nonzero EOF output proves retained pipeline delivery, not a multi-block recursive/native tail.

VST3 tail metadata is cached on serialized setup/state/control paths; ordinary in-process processing does not query the UI-thread-only native API. Accepted event processing invalidates the cache, native restart callbacks invalidate the generation and return refusal for unsupported restart, and the sentinel test correctly reserves only UINT32_MAX for VST3 infinite tail. Worker processing refreshes and publishes metadata before publishing worker-ready output. Isolated EOS preparation resolves pending accepted work with a bounded wait; ordinary callbacks retain their separate fallback policy. Host destination capacity is checked before metadata preparation; active prepared stages and completed prefixes skip duplicate metadata preparation. These source changes are consistent with the bounded design. No broad manager or unequal-branch redesign is inferred.

Terminal evidence: host VST3 metadata 2 tests, host repeated-preflight 1 test, actual worker build, engine route 1 test, host/engine strict lint all succeed in the packet. Earlier compile/red logs remain historical. Worker identity `6b642f849f482c2f4358c0f9423bd09c64d294f48ecf0acdda2f8c7138f3132b`; native binary `7dfc7b96519b0d884a3b06633c009e3024f0a5a46a418236f43596699f74a3b8`.

### Required focused drain evidence, not another broad gate

The requested `isolated_drain_preflight_is_idempotent_across_multiple_native_tail_blocks` test uses `LongFiniteTailPlugin`, a stateless copy-through plugin declaring 16384 tail frames. It begins drain without program input, checks total 24576 frames/three calls and only final scratch-buffer finiteness. This proves repeated metadata preflight/count termination but cannot detect lost/reordered/duplicated native-tail samples. It also has no deterministic pending-request gate. The actual engine test intentionally submits EOS promptly, but cannot prove the worker was still pending at that instant.

1. Add a stateful finite reference with nonzero tail longer than one transport block; feed nonzero program, preserve every process/drain output and compare complete lengths/finiteness/audio to an independent zero-padded reference. Exercise repeated metadata preparation, terminal host drain and reset/fresh replay. Keep pipeline and native-tail counts separately asserted.
2. Hold the final accepted worker request at a deterministic test gate. Show insufficient destination refuses before metadata wait/consumption; release the gate and prove bounded successful preparation/drain returns the exact pending suffix. Cover timeout/retry before mutation, and sticky reset-required failure after drain mutation without replay. Exercise Unknown/Infinite metadata refusal as explicit preflight cases. These were already stated in the EOS handoff; existing silent three-call evidence does not replace them.

No production defect is claimed merely from these missing cases. Actual engine commit/width/pipeline-EOS evidence above remains useful and need not be rerun unless affected source changes. Wide isolated CLAP, product UI, AU/macOS, unknown-tail completion, current combined workspace and full audit remain open.

## Stateful finite-tail and shipped-worker recovery re-review — 2026-09-30

**Finite-tail/preflight evidence conditions are addressed; worker recovery implementation requires one P1 race correction before this combined checkpoint is accepted.** No reviewer Rust changes or Cargo runs. Current seven selected host sources exactly reproduce matching r5 start/end manifests, aggregate `22ea4aa14d571a5c53a3334674e46cbb63b86501c2e4f0fa9bca4aec08d4868a`, in `audit/artifacts/aud135-finite-tail-stateful-r1`. This selected binding is not a complete transitive build manifest.

### Evidence now supported

The former silent copy-through tail fixture is replaced by a stateful stereo FIR with delayed taps through 16384 frames and distinct final source markers. A separate f64 scatter reference builds the complete convolution and prepends the 8192-frame IPC latency. Tests retain complete process/drain vectors, check finite equal lengths and reference samples, assert exactly two native zero-input blocks plus pipeline output, and replay after reset. This addresses the prior nonzero multi-block tail gap rather than merely checking a tail declaration/count.

The channel-changing public host route is 2→2→4. A deterministic worker gate holds accepted input before completion. Short destination refusal leaves its sentinel intact and prepare/begin counters at 0/0; adequate-capacity timeout advances preparation only (1/0), writes no output and remains retryable. Releasing the gate permits preparation/begin 2/1, three 8192-frame outputs then a zero-frame terminal mapper call. Downstream frame accounting and the complete four-channel reference prove delivery without loss/replay. The equal-width pending test and existing Unknown/Infinite begin refusal complement this. Post-mutation injected failure proves a nonzero correct prefix, sticky rejection of process/drain/begin/metadata, followed by checked reset and complete fresh replay.

The shipped executable success fixture uses gain 2, waits for completed metadata and compares every pipeline sample with 2×input; it cannot pass by ordinary passthrough fallback. The companion reset-refusal subprocess retains quarantine. Host classification tests distinguish exact pending sequence/status1 from status2/status3/wrong-sequence cases. These green cases are useful but do not force the race below.

Read terminal logs: subprocess 2/2 SHA `47673b6ffae301616767fb388f3eb81314045e5a2aa74e5479ee831124017eaf`; external-plugin module 134/134 SHA `f0d6f3220bf8bc94d7adce03ec3d9bded402e0dbf91ee3d001bb97398a5a9371`; worker binary 14/14 SHA `ae7294ffd3d59743faf0ea6cceca113220f18ed6b088f68c4263df415d54f85f`; all-target host lint SHA `054c22384e18d8d66c43e0c3f949664e84c01887dc881d26b122272f0349d8bc`. Earlier failed attempts remain historical.

### P1 — classify worker failure locally before host acknowledgment can erase it

`SecurePluginSharedMemory::process_worker_request` publishes `WorkerFailed(sequence,status1)` before returning its process error. `ExternalPluginWorker::process_request` then rereads shared `worker_state`, sequence and status to decide whether to enter Reset-only state or return fatal `Err`. Concurrently, host `ExternalPluginHostProxy::resolve_pending` may observe that exact failure, set its recovery latch and call `clear_block()`, which publishes Idle. If this occurs before the worker's reread, the worker incorrectly takes the fatal branch; the shipped loop's `process_one()?` exits. The host believes it has a recoverable process error, but the worker has died before Reset. The same shared publication must not serve as the worker's durable local classification.

**Required bounded fix:** carry a typed/local process outcome through the worker call, or establish the worker-local recovery classification before exposing the failure publication. Preserve status1-only recovery, fatal malformed-frame/panic handling, exact-sequence host validation, no audio admission while reset is required, and failed-reset quarantine. Do not infer recoverability from a shared state the host is allowed to clear.

**Deterministic regression:** pause after status1 publication and before the worker consumes its local outcome; let the real host resolve/acknowledge that sequence and clear shared state, then release the worker. Require the worker to remain alive/Reset-only, reject non-Reset controls and consume no new audio until successful Reset. After Reset, retain the gain-2 full-vector native-output proof and no-restart assertion. Keep fatal2/3 and wrong-sequence regression cases. A timed stress loop alone would not establish this ordering. Re-run affected worker/host/subprocess gates and strict lint; no broad workspace repetition is requested.

This is a concrete concurrency defect in the narrow recovery change, not a request for a general manager/queue redesign. Previously accepted actual engine candidate/width/nonzero pipeline EOS evidence remains separate. Native AU, wide isolated CLAP, product UI, realtime deadlines, unknown-tail completion and full audit remain open.

## Worker publication/ack race closure — 2026-09-30

**Accepted: the P1 race is closed, completing the bounded stateful finite-tail/preflight and reset-recovery checkpoint.** This combines the preceding supported finite-tail/host-route evidence with the correction reviewed here. No production edits or reviewer Cargo runs; broader native/application/platform work remains separate.

`WorkerRequestOutcome` now carries `Processed` versus `PluginFailed` locally out of shared-memory processing. Ordinary plugin Err still publishes status1, but the worker latches Reset-required from that local result regardless of whether the host has already acknowledged/cleared the shared publication. It no longer rereads host-mutable state to classify its own outcome. Malformed transport/stale sequence and invalid returned frame count remain errors; oversized count publishes status2, panic publishes status3 and stays fatal. The existing public `process_worker_request -> io::Result<usize>` adapts the internal result back to its prior error behavior. Host exact-sequence/status1 admission latch and failed-reset semantics are unchanged.

The deterministic cfg(test) hook pauses after failure publication and before the local result is consumed. The real proxy's `process_block/resolve_pending` observes status1 and clears shared state while the worker is held. After release, the test requires `ProcessFailed`, Idle shared states, one counted failure and no new host audio submission while latched. Describe is refused; Reset is acknowledged and clears the timeline. Four complete 128-frame gain-2 blocks plus the final pipeline flush equal their full delayed references. This ordering directly reproduces the prior race; it does not depend on a stress loop winning a scheduling window. The hook is absent from production.

Verified packet index `51acb6c50b7a350c1ebf08b079b9a812b878e8a37917b227d71cd0d9637b507b` in `audit/artifacts/aud135-worker-classification-race-r1`. Final focused start/end manifests match `89485c63132ace0453aa3911d2cc5ae7203e2d22d1c69add827460196a2be445`; strict-lint start/end match `ff3c06ab7d66b41773f43716cdb6f26a588d90c2f99284fdffbb1873f78af6ad`, and their current listed inputs match. Earlier focused manifests predate only the test-hook collapsible-if cleanup and are not presented as final identical source.

Read actual terminal receipts: deterministic race 1/1 (`840d74a9966f248930a41270a41460cf389838f85ce9618d67f9fd520cf3bdba`), worker module 7/7 (`72690aa0367cca531890761c0e0182bc7b7f729541ed5157d0ccecf98cb3bd6f`), public IPC compatibility 1/1 (`2a6a9ae65c95db84cb9ce84a690160aea93ef81562f09e7750e7cd04dad8edcb`), shipped subprocess recovery/refused-reset 2/2 (`8557e7ad2e4f4939ea25d04ff467b950cad4c9736f9364bcb21a586b5bc18c56`), and strict all-target host lint (`e2f2764652ebcd323093166b6b641482904b6c3b238bce3843f5436c4202ee13`). Broader focused cases predate the sole test-style cleanup; final deterministic test and lint were rerun afterward. These selected manifests are not a whole-workspace compiler-input binding.

Acceptance covers the corrected bounded worker outcome/reset contract, earlier complete FIR/native-tail plus IPC reference, deterministic preflight capacity/timeout retry, channel-changing serial delivery and sticky failure/reset evidence. It does not assert realtime deadline guarantees, arbitrary unknown-tail completion, wide isolated CLAP, product UI, AU, current combined workspace or full audit completion.
