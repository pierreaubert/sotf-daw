# Audit implementation and validation handoff

Updated 2026-10-01. This plan preserves the user's full objective: audit every
in-scope crate against current professional features, implement missing parts,
check the complete audio chain, and establish plugin accuracy with independent
measurements. **MIDI and IAMF are excluded.**

## Current continuation after minor version update, 2026-10-01

Manual parallel execution now starts at the [per-plugin requirements index](requirements/README.md): 51 assignments, common acceptance criteria, shared ownership and an explicit unfinished-edit checkpoint. Implementation workers stopped for this handoff; the current native EQ source still needs compilation and the GPUI receipt patch is incomplete. See [CHECKPOINT.md](requirements/CHECKPOINT.md). This handoff supersedes active-worker descriptions below.

### Current ownership and acceptance

| Work | Current owner | Verified checkpoint | Next required result |
| --- | --- | --- | --- |
| AUD135 isolated finite-tail and worker recovery | Bounded checkpoint accepted by Astra medium | Race correction and earlier stateful tail/preflight/reset evidence accepted in AUD135 review | Broader native/application/platform coverage remains open; combined workspace gate pending |
| AUD142 Crossover UI and routing | Bounded mounted checkpoint accepted by Astra medium | Final r13 serial gate passes 4/4; recovery/positive per-channel route/persistence findings closed. Packet and 39 source/lock bindings verified. Unrelated scoped/dependency lint failures retained | Live-manager audio and native Both completion remain separate; no clean-lint claim |
| AUD134 native Convolution resources | Bounded resource/restore checkpoints accepted by Astra medium | Linux CLAP GUI r9, exported VST3 editor r5, loaded Convolution resource/reset r5 and transactional empty-state r6 accepted; independent convolution waveform coverage recorded | Host-loaded editor integration, changed-geometry/platform routes and clean native teardown remain open |
| AUD143 inactive VST3 bus arrays | Bounded checkpoint accepted by Astra medium | Prepared inactive arrays, six loaded descriptor cases under allocation/deallocation guard, loaded waveform/canary 5/5 and strict host lint accepted | Fresh combined native/workspace verification remains separate |
| AUD142 native Crossover outputs | Bounded persisted-chain checkpoint accepted by Astra medium | 132 native vectors across 11 layouts, loaded refusal recovery, and actual persisted PluginChain planner-to-engine validation accepted for CLAP/VST3; complete captures peak 1.44e-8 and program RMS 3.48e-9 | Realtime deadlines, wider live-manager routes and full application coverage remain open |
| AUD145 per-band EQ placement | Core, consumer and real manager checkpoints accepted by Astra; Luna finishing regression fixes | Independent base, multirate, advanced and mixed vectors accepted within recorded scopes; factory/host 42 vectors and actual manager refusal/commit/continuing-audio regression accepted | Complete preset/mounted-UI, native/FFI, automation/heap, EOS and wider topology/rate coverage |
| Whole-workspace audit | Root coordinates | Earlier accepted scopes remain documented below | Finish remaining feature/route/accuracy matrix and run a coherent combined verification gate; full parity is unproven |

MIDI/IAMF remain excluded. Root coordinates and reviews; Luna xhigh implements,
Astra medium validates, and Luna fixes findings until the affected requirements
pass. Preserve local user edits and external dependency resolutions. The minor
version/changelog task is complete in baseline `aa0a3d1`; sibling SOTF's local
DAW lock identities were synchronized without changing external package records.
The 2026-10-01 follow-up bumps newly changed EQ from 0.5.73 to 0.6.0,
updates EQ/NIH changelogs and synchronizes both local EQ lock identities;
offline locked metadata and whitespace checks pass. Other existing minor bumps
are retained, and MIDI/IAMF remain unchanged.

### Latest lifecycle and effective-rate work, 2026-10-01

EQ oversampling adapter fix is implemented and verified through the actual C API.
Dedicated Luna completed both focused tests (1/1 each), strict bridge/FFI
all-target Clippy and formatting checks. Root verified immutable release library
SHA-256 `51e438f3c26cb20af86d4fb15c17efe31ad880937c73fae8e57ededcb853e6fc`:
Off/2x/4x select factors 1/2/4; first 105 metadata entries remain unchanged at
2/5/32 channels; legacy 1x and raw-factor restored output match frozen vectors
byte-for-byte, with zero measured restoration error. Limiter index controls
remain unchanged. Evidence is in `artifacts/aud145-eq-oversampling-fix-r1/` and
`artifacts/aud145-ffi-oversampling-regression-r1/actual/`. This is adapter and
compatibility verification, not a new independent multirate accuracy claim.
Astra medium accepted the adapter/C ABI correction after source review and
independent binary, audio and selected-source hash checks; see the final
oversampling correction section in `reviews/AUD145-astra.md`. The source hold
is released. This does not close native placement or player transaction work.
Bridge/FFI changelogs updated within existing minor versions.

Current forward ownership: `luna_bandsplit` implements native EQ placement,
pair commit/lifecycle/layout and subsequent FFI exposure. Its parameter helper
schema correction is Astra-accepted; actual wrapper integration is still open.
`luna_eq_response` owns reusable complex response matrices in sibling
`sotf-player` and the three receipt rollback findings formerly assigned to
`luna_crossover_design`. The response domain fixes pass the r3 focused gate
(10/10, all three independent fixture suites included) after correction of the
widened-f32 Q minimum boundary fixture. Raw r2/r3 logs and selected post-run
hashes are saved in `artifacts/aud145-player-response-r3/`; Astra medium accepted
all three domain corrections in `reviews/AUD145-astra.md`. Luna has resumed
receipt implementation. Receipt band identity, unrelated
rack structure and retry transaction ownership remain open. The response contract
must retain channel/pair/band order, advanced-realization phase and effective
sample-rate semantics. The accepted ordinary-Peak chart fixture is a bounded
oracle, not evidence that the production chart is complete.

The next oversampled-response implementation contract is recorded in
`handoffs/aud145-oversampled-response-implementation.md`. Existing independent
research and 72 actual C ABI complex matrices provide bounded evidence for
implementation; more reference-only variants are not the current priority.
Converter-inclusive player implementation, advanced multirate coverage and
chart consumer integration remain required.

Root added `artifacts/aud145-chart-advanced-reference-r1/`: 48 base-rate
Warped/Kautz/ordinary mixed-route cases with 336 complex matrices. A separate
expanded-polynomial impulse recurrence and time-domain routing reproduce all
matrix columns with maximum error `6.68e-14`; phase, dry-path, lambda-sign and
section-order negative controls all differ by more than 1. This extends the
available independent fixtures; production comparison and Astra review remain
pending, and global SVF/oversampling are outside this packet's scope.

Root also checked the immutable adapter-fix C ABI library with nine combinations
of 2/5/32 channels and 1x/2x/4x: one 4096-frame call versus 20 irregular calls.
All output vectors are byte identical; every input remains unchanged and
NaN-initialized outputs become finite with leading/trailing guards intact.
`artifacts/aud145-ffi-partitions-r1/` records the executable verifier, receipt
and exact binary hash. This is bounded partition/buffer evidence, not a heap,
EOS, full memory-safety or independent accuracy claim; Astra review is pending.

The same immutable C ABI library also passes 63 independent base-rate
constructor-to-process placement waveform comparisons in
`artifacts/aud145-ffi-placement-accuracy-r1/`: 42 existing 2/5-channel vectors
plus 21 new 32-channel cases using endpoint 31 and reversed pair orientation.
Peak/RMS errors are at most `4.67e-8` / `9.63e-9`, and unpaired channels remain
exact. This proves the tested JSON construction/processing path, not the still
unfinished FFI parameter exposure or native/UI route. Astra review is pending.

New actual FFI persistence gap: `artifacts/aud145-ffi-pair-state-probe-r1/`
shows both raw state and exported preset envelopes omit stereo pairs. Preset
import succeeds into a fresh differently paired 32-channel instance but retains
target routing (peak error `0.1427463`, RMS `0.0163906`); matching-pair controls
are byte exact. Raw partial loading explicitly retains omitted constructor
config and must preserve that legacy contract. The pending FFI feature must
add self-contained committed pair persistence for new presets, with detached
transactional validation and no native-only channel cap. This is a reproduced
gap, not a passed acceptance gate; owner `luna_bandsplit` has the evidence.

`artifacts/aud145-ffi-advanced-state-probe-r1/` confirms that exported presets
also omit Kautz section construction data: successful import into a differently
configured target produces peak/RMS errors `0.78634` / `0.32666`; matching
constructor controls are byte exact. The new preset representation must retain
advanced structural EQ data along with pair routes. Warped Peak lambda and
realization probes have equivalent tested audio and are explicitly inconclusive
for structural persistence; they are not classified as failures.

The GPUI receipt regression's second attempt compiled but selected zero tests:
`app`/`ui` are excluded from lib-test builds. The UI owner is moving it to an
integration/dev-api harness; this result is not counted as a passed regression.

The relocated E2E regression now runs and passes: owner session 94752 is
terminal exit 0, 1 passed / 258 filtered; root inspected
`/tmp/aud145-eq-receipt-rollback-r6.log`. It mounts the player view and invokes
the real acknowledgement poller with a synthetic rejection, restoring the
pre-edit graph and selected band. It is not a real audio-backend rejection
measurement. The owner is tightening the queued-update assertion from presence
to Structural and implementing the overlapping-receipt sequences below. The
build/source hold is released; native EQ helper implementation can continue.

Latest receipt checkpoint r7 passes **3/3 mounted E2E tests**, 258 filtered,
owner session 59649 terminal exit 0. Root preserved the inspected log and
owner-reported invocation in `artifacts/aud145-eq-receipt-rollback-r7/`.
This adds stale A-success/B-failure restoration with scalar resubmission and
A-failure/B-failure restoration; the queue assertion now requires Structural.
Luna yielded its slot and new Astra medium `astra_eq_receipt_review` is
reviewing this concrete slice. These remain synthetic receipts through the
actual poller, not a live audio-manager rejection measurement.

Astra's subsequent source review **does not accept the receipt slice**. Three
blocking cases remain: band insertion/removal corrupts positional scalar merges;
unrelated plugin additions/removals are lost or resurrected; and merged retries
clear rollback ownership before acceptance, leaving a rejected retry without
recovery. The recorded 3/3 gate omits these cases. See the newest receipt section
in `reviews/AUD145-astra.md` for required regressions. Reopening the prior Luna
or spawning a replacement currently hits the agent thread limit; ongoing native
and response workers can continue, and root will reuse a worker after its focused
checkpoint if necessary. This is not a user-input blocker or acceptance.

Native helper checkpoint now passes **4/4** direct tests (owner session 15301)
after the fixture was corrected to include dynamic native band metadata.
The Crossover-feature NIH no-run compile also passed (21705). Packet
`artifacts/aud145-native-eq-params-r1/` preserves earlier failures, distinguishes
raw logs from owner-transcribed output, and records selected post-run hashes.
The helper covers restart flags, draft/committed Apply, saved route and schema
failure ordering; wrapper/layout/FFI integration is still absent. Native Luna
yielded its slot. Astra medium `astra_eq_helpers_response_review` is now active
on this helper and the reusable response module; the thread limit no longer
prevents this review. The response Luna continues the three receipt corrections.

Astra's helper/response review found four blockers, now returned to Luna:
native schema validation must reject Float where Int is required; active
ordinary SVF order >2 must be refused (both core setters reject it); explicit
Kautz positive finite Q/finite weight domains must remain supported; ordinary
Biquad admission must enforce core bounds. The advanced matrix reference packet
was accepted within its stated base-rate scope. Native Luna resumed for the
exact-type fix; response Luna fixes its three domain issues before continuing
receipt corrections. Prior passing helper/fixture gates do not close these gaps.

Root's new `artifacts/aud145-multirate-response-research-r1/` investigates a
converter-inclusive alias-sum approximation for the remaining oversampled chart
mode. A limited 48 kHz scalar pilot differs from independent FFT-transport
impulse DTFT by at most `4.69e-10`, with observed block-phase residual below
`2.97e-10`. This is research only, not exact-LTI or production acceptance;
the packet states broader verification and matrix/per-band semantics still
needed before implementation can be accepted.

The multirate research packet now also includes 72 five-channel matrices at
44.1/48/96 kHz with reversed pairs and noncommuting Left/Mid order. Every matrix
element matches an independent impulse-basis DTFT within `7.96e-10` against a
predeclared `1e-8` research threshold. Unpaired channels correctly retain the
converter/queue response rather than unity. Root then compared these same 72
matrices with cold impulse outputs from the actual immutable C ABI library:
all pass, maximum complex error `3.30e-6`, against the predeclared scaled
`3e-5` bound. `artifacts/aud145-multirate-response-production-r1/` records the
script, full measured matrices and exact binary hash. This closes that limited
production-model comparison only; advanced modes, chart implementation, broader
phase/rate behavior and Astra review remain pending.

Astra accepted the native helper exact-type/range correction after source and
regression review. The native owner has resumed wrapper/state/layout integration;
the response-domain and receipt blockers remain assigned to the other Luna.

Luna `luna_eq_response` has now written the reusable sibling response module
after the scoped sibling-write escalation succeeded. It implements base-rate
Biquad/Warped/Kautz and global SVF matrices and explicitly rejects unsupported
oversampled Biquad contexts. Focused fixture tests are starting; no production
response-math acceptance is claimed yet, and full multirate chart support stays
open.

Response math reached an owner-reported **7/7** focused gate, including all
three external ordinary/advanced/SVF fixtures via `--include-ignored`. Those
three tests now use explicit ignore annotations and require fixture paths;
missing environment variables cannot silently pass. Source registration and
strict 1/2/4 factor validation are implemented. Owner session 1511 ended 0;
stdout was not persisted. `artifacts/aud145-player-response-r1/` records the
owner-reported command/environment and selected post-run source/fixture hashes,
not an independently inspected log or a full build seal. Astra review and UI
consumption remain pending. The same Luna now owns the three receipt fixes,
avoiding the temporary thread-limit failure when reopening the prior owner.

Root review found a further receipt correctness gap: A succeeds while newer B
is pending, then B fails; the current retained snapshot restores pre-A despite
the backend having accepted A. See
`handoffs/aud145-eq-receipt-stale-success.md`. Luna must advance the committed
rollback base on successful older receipts without discarding newer desired
edits, and cover actual-poller multi-receipt sequences before Astra acceptance.


Player controller module now passes 31/31 tests (owner session 63581), including
original-band indexing, placement/pair edits and the dormant global-mode
transition correction. Sibling constructors/test fixtures required `stereo_pairs`
integration; a manually seeded graph-width fixture does not prove propagation.
Rack backend rejection/rollback, mounted controls and matrix response remain
open. Native preedit schema capture passes 1/1: 125 unique IDs, 20 placement
IDs defaulting to zero, 43 total hidden controls. Exact observed enumeration
order is preserved in the artifact but is not asserted stable. The dedicated
Luna bridge owner has confirmed the `Parametric EQ` identity and is implementing
the scoped choice/factor conversion.


Ownership split for forward progress: fresh Luna xhigh `luna_eq_bridge` owns the
narrow shared-bridge oversampling conversion and bridge/FFI tests;
`luna_bandsplit` confirmed no edits to those files and retains native EQ schema,
pair/lifecycle/layout and later FFI placement work; `luna_crossover_design`
continues player/GPUI. Root captured successful old C API raw-factor state loads
at 1/2/4 in `ffi-preedit-r1` (full path `artifacts/aud145-ffi-preedit-r1/`), with
frozen nonzero outputs. The new selector fix must preserve those state outputs.
Astra resumes validation when a concrete change is ready; no additional model
or version changes are required.


First player/GPUI implementation is in review. Focused player gate 11036 failed
before tests because existing sibling EQ literals/patterns omit `stereo_pairs`;
Luna is integrating defaults at genuine legacy constructors and preserving pair
intent during channel-width adaptation. Astra also found two required fixes:
validate dormant global routing before disabling per-channel mode, and preserve
pre-edit committed UI settings when a linear-rack backend update is rejected.
Use existing update receipts/revisions; no broad manager protocol rewrite.

Root prepared an actual post-fix C ABI oversampling verifier in
`artifacts/aud145-ffi-oversampling-regression-r1/verify.py`; its saved expected-red
run fails on the immutable old library's wrong default Off readback. It checks
105 metadata entries, 1/2/4 saved factors and normalized roundtrip, non-bypass
output, unchanged legacy 1x audio at 2/5/32 channels and distinct multirate
outputs. Luna is implementing the EQ-specific shared-bridge conversion; raw
factor state and native NIH's existing conversion must remain unchanged.


New actual C API finding from the immutable FFI baseline: oversampling choice
indices are passed directly to the DSP, which expects factors 1/2/4. Normalized
Off=0 returns -2; 2x=0.5 leaves factor 1; 4x=1 selects factor 2. Reproduction:
`artifacts/aud145-ffi-preedit-r1/probe_oversampling.py` and its saved JSON output.
The static specs advertise Off/2x/4x at indices 0/1/2. Luna BandSplit owns the
adapter conversion in both directions and raw-index entry points, with actual
C API regression evidence. Existing 105 IDs/metadata stay stable; correcting
reported default/selection semantics is an intentional fix, not a compatibility
claim that the faulty normalized values must remain unchanged. Astra is checking
the shared-bridge boundary before implementation.


Root captured actual pre-native-extension FFI compatibility evidence in
`artifacts/aud145-ffi-preedit-r1/`: 105 C API parameter addresses match at
2/5/32 channels, with nonzero, non-bypassed 4,096-frame outputs and saved state.
The immutable cdylib is under target/audit-artifacts; SHA-256 starts 60955d58.
Seven selected source files match the accepted preedit/workspace snapshot.
State contains 125 raw keys, including 20 placement=0 values; config omits
placement/pairs. This is current implicit-route compatibility, not a historical
missing-placement-ID state. A wrong-key dry fixture is retained separately as
rejected evidence. Core/FFI source hold is released for Luna implementation.


Astra closed the bounded regression cleanup, including both strict lints,
9-passed/3-ignored helper results and all 14 final source snapshot entries.
The expanded snapshot is post-run provenance, not an identical pre/post claim.
Root archived seven native-EQ/FFI/core source files in
`artifacts/aud145-native-eq-preedit-r1/`, each byte-matching the accepted r3
workspace snapshot. This is a compatibility source baseline; actual native
metadata and old runtime-state captures remain separate required evidence.


Final decoder cleanup verification is green: three affected EQ targets passed
9 tests, with 3 ignored (`eq-helper-tests-r1.log`, owner session 24904;
root verified all three target summaries). Both
strict lints remain green and the shared EQ source hold is released. Native EQ
implementation is underway; player/GPUI follows the accepted handoff. The new
response helper must retain current Warped phase/lambda and Kautz `1 + H`
semantics rather than recovering phase from scalar dB values; global realization
and effective-rate context must be represented or any limitation explicit.


Strict engine and EQ Clippy now both pass in the r4 regression packet
(`engine-clippy-r2.log`, `eq-clippy-r2.log`). The final changes are test-helper
chunking/size idioms and unchanged-value digit grouping. Luna is checking the
three affected decoding/route test targets, then continuing the player/GPUI
lane; the native/FFI lane coordinates its shared EQ input window separately.


Follow-up lint: strict engine Clippy passed after correcting digit grouping in a
test literal (same numeric value). EQ strict lint reported two test-helper
`chunks_exact_to_as_chunks` warnings; Luna owns the shape-preserving decoder
cleanup and narrow rerun. This does not invalidate the earlier unchanged-source
workspace result, and no new production failure was observed.

Root prepared `artifacts/aud145-chart-matrix-reference-r1/` for the next GPUI
response-math work: 60 cases / 420 complex matrices at three rates, including
cross terms, noncommuting order, reversed pairs, unpaired channels and identity.
Analytic matrices agree with separate 4,096-sample time-domain basis impulses
within 2.154e-14; controls reject scalar dB summing, dropped cross terms, transpose,
reversed band order and reversed pair direction. Scope is base-rate ordinary
Peak filters only; this is reference validation, not production UI acceptance.
Astra accepted the final matrix reference and hardening additions, independently
reproducing all 420 impulse comparisons. The tail observation is specific to
these cases, not a universal truncation bound.


**Combined workspace r3 passed:** 6,369/6,369 tests, 75 skipped, 396 binaries,
327.035 s test runtime. FFI and both native backends were included; MIDI/IAMF
excluded. Root session 11519 ended exit 0 at 04:24:59 UTC. All 4,060 selected
source hashes match before/after. Receipt, source manifests and log are in
`artifacts/workspace-integration-20261001-r3/`; log SHA-256
`70285d4fd006e243ca9538c1175aa6deb3fa1d2a8d560d11399aa9f8c53a571b`.
Skipped loaded-bundle/platform tests are not claimed by this gate. General
source freeze is lifted; queued engine/EQ strict lint retains its own input hold.
Next parallel work: Luna BandSplit implements reviewed native EQ placement,
atomic pair configuration, truthful multichannel layouts and FFI integration;
Luna Crossover implements player/GPUI placement controls and routing-aware
response from the existing AUD145 handoffs. Astra medium validates both lanes.
The full feature-parity and accuracy audit remains incomplete.


Current combined gate: workspace r3 is running under root session 11519, including
FFI and both native backends and excluding only MIDI/IAMF. Exact invocation and
start manifest are in `artifacts/workspace-integration-20261001-r3/`. Sources are
held stable through terminal status. Before this run, final Crossover accessor
tests passed 11/11 (one ignored) and toolbar integration passed 1/1 in the r4
packet. Astra accepted this bounded correction and verified all ten current
source-start hashes; final end seal and lint remain pending. Strict engine/EQ lint is queued separately under the shared Cargo lock.
No passing whole-workspace claim is made until the run finishes.


Root completed EQ regression r3 (`artifacts/aud145-eq-regression-r3/`):
three tests covering all four frozen legacy cases pass, with the two intentionally
corrected Warped cases checked against independent references and both unaffected
cases byte-exact. The unaffected right channel is also byte-exact. Corrected
legacy maximum peak errors are 1.25e-8 and 1.82e-8. A fresh explicit-placement
multirate capture passes all 84 independent vector comparisons (maximum peak
1.093e-6, RMS 3.029e-7), exercising the changed ordered planar loop. All 114
selected EQ/math source and workspace-manifest hashes match before/after;
this is not a complete dependency closure. Astra independently recomputed the
comparisons, verified the current hashes and log receipts, and accepted this
bounded EQ regression checkpoint. The remaining Crossover accessor review is
separate.


Current regression checkpoint: Astra accepted the final NIH test cleanup and
verified all five current source hashes against identical final manifests
(`053aca2d…93a0d`). Saved Crossover callback results pass **3/3**, and strict
Crossover-feature all-target `--no-deps` Clippy passes. The prior dependency lint
failure remains historical evidence; EQ iterator corrections need their own
ordered multirate verification. The inactive-buffer comment is corrected.
Luna Crossover is finishing transactional band-count cutoff materialization,
actual toolbar coverage and the three legacy replay tests covering four cases.
Astra confirmed that Bands construction casts stored f64 cutoffs to f32
(`crossover_plugin.rs` multiway constructor); expansion must reject adjacent
cutoffs that collapse at this consumer boundary before mutating settings.
The near-20-kHz refusal regression is part of the pending accessor fix.
For legacy implicit PerChannel presets, band-count edits now retain dormant
cutoffs and partial-mode fallback unchanged; selecting Bands materializes the
stored count transactionally. Astra accepted this source approach; actual
factory before/after and refusal tests remain pending.
Root's independent corrected legacy references are accepted; fresh current
captures remain required. Root prepared workspace r3 including FFI, but has not
started it while these fixes are being verified. Luna BandSplit is preparing the
native EQ placement handoff without production edits during this interval.


The combined workspace regression finished: **6,268 passed, 10 failed, 69
skipped across 395 binaries** (6,278 tests run; 287.756 s test time), with both
native backends enabled and four test threads. All 4,060 selected source hashes
match before/after. Exact command, logs and manifests are in
`artifacts/workspace-integration-20261001-r2/`; exit status 100. The first attempt
stopped before tests because FFI nested metadata needed uncached `clipboard-win
5.4.1` offline. The retry used the documented FFI exclusion plus MIDI/IAMF.

Five failures are missing native BandSplit bundle environment variables in tests
that should be explicitly opt-in. Two Ambisonics failures share a broken literal
alternative macro matcher; the BandSplit VST3 compatibility case rejects its
reserved silent bus during validation. Luna BandSplit owns these fixes, including
actual loaded opt-in execution. Luna Crossover owns Crossover toolbar choice
canonicalization and the legacy EQ replay reconciliation. The EQ fixture must
preserve original historical bytes and two unaffected exact cases; root is
constructing independent corrected references for its two Warped cases. Source
freeze is lifted. No passing whole-workspace claim follows from this run.

Post-run NIH correction checks: two legacy BandSplit callback regressions pass;
five pinned r3 loaded CLAP/VST3 tests pass when explicitly selected with bundle
paths. The environment-dependent tests are now opt-in. Current Ambisonics metadata
and order-7 CLAP callback pass, including a direct Crossover surround-matcher
assertion. Scoped NIH all-target Clippy with `--no-deps` passes; the dependency-
including invocation reports two EQ ordered-route iterator lints, assigned to
Luna Crossover. Astra accepted the bounded reserved-bus and surround-macro fixes in AUD142/AUD143
reviews. A stale inactive-buffer comment needs a documentation-only correction. Pinned
r3 artifacts predate this source fix, so loaded results and current callback
results retain separate scopes.

The FFI cache blocker is now removed: root completed `cargo fetch --locked
--manifest-path crates/sotf-plugins/crates/plugins-ffi/Cargo.toml` with approved
registry-cache access. It fetched the missing platform metadata dependencies,
including `clipboard-win 5.4.1`; the workspace lock hash remains unchanged.
Separate FFI execution now passes: **91 unit tests passed, 1 ignored**, with
normal header generation and no selected-source changes across 4,060 hashes.
Command/log/receipts: `artifacts/ffi-regression-20261001-r1/`. No generated-header
worktree changes appeared. This is Linux FFI verification, not Apple runtime or
full new-placement FFI acceptance.

The Crossover factory wire-form regression now passes 1/1, but the actual
toolbar settings route exposes a further production defect: choosing three or
four bands leaves `extra_frequencies` empty, and the factory correctly rejects
the incomplete configuration. Luna is fixing the accessor, with preservation of
existing/dormant cutoffs and valid shrink/grow behavior; the test must not seed
missing cutoffs to conceal this route gap. Astra is reviewing that transition.

Root prepared independent corrected references for the two affected legacy
Warped cases in `artifacts/aud145-legacy-corrected-reference-r1/`, preserving
the original inputs/settings/output archive. Analog prototypes and the accepted
independent allpass/modal helpers supply expected samples; no production output
is used to generate them. Ten negative controls reject. The actual mixed-case
bytes recovered from the failed workspace assertion match this reference at
peak 1.24951652e-8 / RMS 2.69436625e-9. The channel-bank case still needs its
actual capture; its unaffected right channel retains an exact historical check.
Astra is reviewing the oracle while Luna repairs the regression fixture.

The next AUD145 product-route inspection is recorded in
`handoffs/aud145-placement-product-route.md`. Current sibling GPUI lacks placement
controls and still sums scalar band curves; mixed placement requires an ordered
matrix response. The player per-channel toggle clones explicit placement into
channel banks before returning a structural update, creating a configuration
the engine rejects. These are source findings pending executed regressions.
Astra is reviewing transition semantics and routing-aware UI requirements while
Luna finishes the native-rate retained-object test correction.

The actual EQ manager commit/audio regression now passes 1/1 using synchronized
real ProcessingThreads: a rejected typed placement update preserves the next
warmed continuation, while a valid Mid update commits and its post-crossfade
blocks match a fresh host from the same typed configuration and differ from the
old route. Strict engine all-target Clippy with both native features also passes.
Root verified the terminal logs and all 12 selected current inputs against the
matching start/end manifests in `artifacts/aud145-manager-real-r1/`. Astra medium accepted this bounded integration checkpoint after verifying the
logs, manifests and current selected files. Playback is still a command probe,
not hardware playback. The earlier 42 independent factory-to-host vectors remain
its numerical foundation.

The typed-native engine/player width resolver passes its two focused planner
tests. The loaded isolated engine chain Crossover Both4 (8→32), BandMerge
(32→8), then Matrix selecting channels 0 and 7 (8→2) now passes for both CLAP
and VST3 in r6. Root independently reran `check_chain.py` against
`artifacts/aud142-native-consuming-geometry-r1/captures-paced-r2`: peak error
1.4339689075e-8 and program-interval RMS 3.4720998534e-9 for both formats,
within unchanged 1e-5/1e-6 bounds. All seven negative controls reject, including
bypass and a one-frame delay error. The declared 127-frame transport prefix is
exactly zero; 257 program frames plus transport continuation produce 384 frames.

Earlier runs exposed fixture failures: unknown-trust in-process hosting was
correctly rejected, discarded diagnostics hid skipped stages, and a subsequent
CLAP capture was exactly dry passthrough. The green fixture explicitly starts
workers and paces callbacks by 25 ms; those two changes have not been isolated
causally. This is controlled offline accuracy, not realtime deadline or EOS
proof. Prior failures are preserved. The subsequent session 89984 passes 1/1
from an actual version-2 PluginChain preset: three user rows plus four permanent
rack rows, six enabled converted configs, conflict-free 8→32→8→2 planning,
then loaded full-chain processing. Root independently checked both formats in
`captures-chain-r1`; errors and all seven negative-control results match r6.
No new production insertion API was required. Astra accepted the bounded typed
geometry and persisted-chain checkpoint. The 26-entry manifest is post-run,
not a pre/post run binding; session 89984 terminal output is owner-transcribed
in `logs/plugin-chain-loaded-r1.receipt.md`. The staged worker hash is recorded
separately from the different post-run debug-root worker. These limitations do
not support a complete build-closure or realtime claim.

Retained native sample-rate reinitialization now prepares a detached backend,
restores state and pending scalar controls, and commits only after validation.
Luna reports the final loaded regressions passed 2/2 (session 76796): a retained
CLAP instance follows a real DawHost 96→48 kHz input-rate transition and matches
a fresh 48 kHz instance while differing from stale 96 kHz processing; a VST3
missing-IR preparation failure preserves native state, controls, latency, tail,
and exact warmed continuation. This does not prove whole-graph rollback.
Reset diagnostics show pending CLAP events survive reset and commit on first
processing; persisted-state reset and queued-event startup are separate oracles.
Astra required restore/reset on the actual transitioned instance, rather than
two fresh 48 kHz objects. Luna corrected the fixture with typed Crossover setup
and extracted the transitioned native instance. Final r4 passes both loaded
tests and strict scoped host-test Clippy; all 10 selected current inputs match
start/end manifests. Astra accepted the bounded rate transition and failure
preservation checkpoint. Durable evidence: `artifacts/native-retained-rate-r4/`.
The full graph remains nontransactional on failed rebuild, and the test does not
establish realtime-safe rate switching.

Root's first combined CLAP/VST3 host library Clippy gate failed on
`clap_backend.rs::initialize_instance` (eight arguments; limit seven), with
`--no-deps -- -D warnings`. Log: `/tmp/sotf-host-native-clippy-20261001-r1.log`;
session 10012 terminated with exit 101. Luna BandSplit removed the redundant
activation flag argument and derives it from the existing typed setup inside
the helper. Root rerun session 33442 passes with both native features,
`--lib --no-deps -- -D warnings`; log
`/tmp/sotf-host-native-clippy-20261001-r2.log`. This precedes the retained-native
rate transition implementation and does not validate that later change.

The loaded Convolution missing-resource test exposed a VST3 lifecycle defect:
failed in-place state restoration also failed reactivation after the original
IR file was removed. The returned error explicitly reports both failures.
A 97-frame continuation happened to match the old warmed processor, but this
is not proof of an active lifecycle; the reported tail became unknown.
The detached-candidate r4 run now passes successful replacement, independent
waveform, failed-resource state/tail preservation and populated twin continuation.
It then fails reset because reactivation reopens the removed original IR file.
Luna has implemented guarded NIH reuse of the prepared Convolution processor
using last-successful geometry, structural fingerprints, committed resource path,
no pending restore/editor state, and scalar prevalidation. Root reviewed those
guards. Root fresh debug bundle build 38796 passes with 20 selected inputs
unchanged; loaded VST3 test 24291 passes 1/1 with 21 inputs unchanged, including
the copied bundle. Valid replacement, rejected-resource populated preservation,
updated Mix and reset after deleting the original IR now pass complete direct
convolution waveform checks. See `artifacts/aud134-loaded-convolution-r5/` root
receipts. Astra found a remaining transaction blocker: empty no-setup VST3
state bypasses detached restoration and mutates live tail/lifecycle before
rejection. The focused empty-state red run reproduced `Finite(1027)` becoming
`Unknown`. Luna now explicitly loads empty state on a detached no-setup VST3
candidate; the loaded green run passes 1/1, preserving resource state, finite
tail and nonzero exact populated-twin continuation with the original IR deleted.
Root inspected both terminal logs in the r5 packet (`empty-state-red-r1.log`
and `empty-state-green-r1.log`); these first logs have no pre-run source manifest.
The r6 source-bound rerun passes 1/1 with 21 selected inputs unchanged and
the immutable r5 bundle. Astra verified current/archive hashes and the populated
regression, then accepted the bounded empty-state correction in AUD134's review.
Existing NIH debug lifecycle warnings remain in
the log; loaded changed-geometry refusal/platform/host editor routes are not
claimed.
Preserve the red logs in `artifacts/aud134-loaded-convolution-r1/`;
the broader lifecycle/platform/editor requirements remain open.

EQ's effective-rate change retains selected oversampling while global SVF runs
at base rate. The focused unit passes 1/1 and the current library suite passes
92/92 (`/tmp/sotf-aud145-eq-lib-effective-rate-r1.log`). The public boundary
integration retry passes 1/1, but its saved-state audio assertion changed from
exact equality to a numerical bound. Root reconstructed f32 values from the original printed vectors and measured
peak 5.96046448e-8 and RMS 4.10140179e-9 across 10,240 samples;
`artifacts/aud145-svf-restore-residual-r1.json` binds that diagnostic to its log.
The r4 integration passes both original peak/RMS bounds and exact equality
against fresh DSP configured with the saved f32 scalar values, isolating the
residual to initial f64 configuration versus f32 scalar serialization. This
fixture reuses structural configuration and pairs; it is not full preset or
consumer persistence coverage. The 18 independent mixed realization captures now pass (maximum peak error
4.6585e-7, RMS 1.46245e-7); root verified all 30 selected source copies against
the start manifest. All 30 selected source hashes now match start/end and archived copies;
Astra medium accepted this bounded checkpoint after independently verifying the
source and reference checksums, comparator and negative controls. Reporting
corrections and provenance limits are recorded without changing the sealed
packet in `artifacts/aud145-mixed-review-addendum-r1.json`.
Consumer persistence and the wider automation/heap/chain gates remain open.
Luna implemented engine/settings/converter placement and stereo-pair propagation,
including mute/solo compaction while preserving ordered routing intent, followed
by actual factory/DawHost audio validation. The focused engine `eq_` gate
passes 21/21 (session 65397). Retained compiled-host placement fallback passes
1/1 (59126); the actual settings→converter→factory→DawHost capture passes all
42 frozen independent vectors (98039), independently rerun by root with exact
byte-count checks. See `artifacts/aud145-engine-host-root-r1/`. A focused factory
rejection test also passes 1/1 (75734): invalid all-muted per-channel placement
is rejected and a separate existing host retains bit-exact twin continuation.
Manager swap/refusal is not exercised by that test. Final source-bound rerun
passes 23 tests (one explicitly ignored capture), the explicit 42-vector capture,
and strict all-target no-deps engine Clippy after fixing two test-reader lints.
The 18-entry start/end manifests match (SHA256 `55a505e151ae04e4b555b9948f784580c9ae871cb60c67a49cb7d13a914a0958`);
logs are `/tmp/luna-aud145-eq-engine-consumer-final-tests-r2.log`,
`/tmp/luna-aud145-eq-engine-consumer-final-capture-r2.log`, and
`/tmp/luna-aud145-eq-engine-consumer-clippy-r2.log`.
Astra medium accepted this consumer checkpoint after independently checking
the 42 captures and source/reference hashes. Root archived the exact logs,
captures and all 18 hash-matching selected source copies in
`artifacts/aud145-engine-consumer-final-r2/`; its receipt records the post-run
copy timing and limited source scope. Actual manager replacement,
UI/native persistence and heap gates remain open. The compaction edge is recorded in
`artifacts/aud145-consuming-route-inventory-r1/revalidation-and-compaction-edge.json`.

The actual manager regression exposed a further defect: the linear builder
reports invalid EQ as a skipped-plugin diagnostic, and the manager previously
treated only external-plugin diagnostics as fatal. The invalid all-muted
per-channel placement candidate therefore attempted publication (the command
probe timed out after 240 ms). Root inspected the failed log
`/tmp/luna-aud145-eq-manager-invalid-candidate-r1.log` and confirmed both source
branches. Luna extended the manager's required-update failure guard to EQ;
the focused actual-manager test now passes 1/1 and strict all-target engine
Clippy passes with both native features. Root archived logs, the matching
ten-input start/end manifests and hash-verified post-run source copies in
`artifacts/aud145-manager-rejection-r1/`. Astra accepted the bounded refusal;
root then added the original red log and owner-reported command flags, with
their provenance limits explicitly recorded. The general
startup builder policy is outside this narrow change, and successful real-worker
commit/audio remains open. The earlier failed test retains its original status.

Astra also found a reachable retained-native sample-rate defect: removing an
upstream resampler causes DawHost to call the default no-op ExternalPlugin
initializer with a new rate while the native backend retains its construction
rate. Luna xhigh is implementing this separately; see
`handoffs/native-retained-sample-rate.md`. Plugin-object transactionality and
whole-graph rollback are distinct: graph removal has already changed topology
when a subsequent build fails. No runtime fix or green rate-transition gate is
claimed yet.

Crossover's populated lifecycle fixture has reached actual loaded execution.
After removing backend-specific error wording assertions, reconciliation r2
passes the loaded CLAP and VST3 lifecycle fixture: valid route/layout transitions,
conflicting-state and FIR per-channel refusal with preserved populated twins,
and actual structural process refusal followed by same-setup recovery and full
output comparison. The 8 selected source/binary entries match start/end; this
is not a full dependency closure. Astra accepted the bounded lifecycle/reconciliation checkpoint. The comparison
is public-DSP composition with peak tolerance, not a new independent coefficient
oracle. Valid same-width native drift recovery is covered; Luna is now testing
width-changing and intrinsically invalid drift recovery. The separate drift r1
gate now passes Both width-changing recovery for CLAP and VST3, but invalid
FIR/per-channel drift fails CLAP candidate state admission before recovery.
The failure confirmed that invalid structural state must be repaired before
candidate activation. The correction prepares a copy of native state with only
the requested mode/topology/band count changed, preserving other values.
Recovery-drift r2 passes both tests across CLAP/VST3. Root reran the original
lifecycle regression (session 6191, 1/1 pass, 8 selected inputs unchanged);
Astra accepted the bounded correction. The FIR recovery comparison proves a
fresh DSP epoch and semantic saved-state preservation, not populated continuity.
Luna is now tracing the native Crossover application/engine consuming route.
See `artifacts/aud142-native-crossover-lifecycle-r1/logs/recovery-drift-r1.log`.
Strict host lint is
deferred until shared host edits cohere; no clean lint claim is made.

### Current native and SVF evidence, 2026-10-01

Astra accepted Convolution's Linux VST3 editor r5 checkpoint: focused callback
module 4 passed/2 ignored and actual embedded IPlugView under Xvfb 1/1.
It covers refused reload, retry, retained process automation at mix 0.37,
fresh-wrapper restoration and independent old/new IR audio including tails.
The 13 selected source snapshots and copied binary are bound in
`artifacts/aud134-vst3-editor-r5/receipt.md`. This exercises the exported wrapper
directly; packaged `.vst3` loading through sotf-host and other platforms remain
open. See `reviews/AUD134-astra.md` for the bounded acceptance.

Crossover loaded constructor r5 completes all 24 routes for 7.1/9.1.4 in
CLAP and VST3, independently matching LR24 (maximum peak 1.43e-8, RMS 1.54e-9).
The first expanded attempt passed 66 CLAP routes but rejected VST3 mono.
Correcting mono/quad speaker identities and the raw fixture's mono/stereo index
order yields 2/2 raw callback tests across all 11 layouts. The fresh binary
`06184618e17635d1c9e57088c41e5b50c984fd641d7b1a59008597fdd38ff40d`
then passes all 132 loaded CLAP/VST3 routes and independent LR24/state checks
(maximum peak 1.43e-8, RMS 1.56e-9). See
`artifacts/aud142-native-crossover-routing-r1/root-independent-all-layouts-r2.json`.
Astra accepted this bounded cold static checkpoint in `reviews/AUD142-astra.md`.
This supersedes the table's earlier
absence of native runtime evidence; populated reconfiguration/refusal, broader
families and application routes remain incomplete.

Astra accepted the bounded SVF correction after measured red/green evidence:
36 shelf failures before correction, then 48/48 public vectors and 240/240
complex points pass unchanged bounds; 15 focused math tests pass. See the
appended `reviews/AUD145-astra.md` disposition. The next independent public
capture checkpoint passes 48 non-Peak Warped vectors plus 240 complex points,
and nine Kautz vectors plus 45 complex points. All 24 archived selected source
inputs match start/end manifests; this is not the complete dependency closure.
See `artifacts/aud145-advanced-public-r1/root-verification.json`; Astra medium
accepted this bounded checkpoint in `reviews/AUD145-astra.md`. Kautz evidence verifies the existing fixed-pole modal contract,
not a complete orthonormal Kautz basis or full feature parity. Mixed realizations,
internal-rate/SVF boundaries, automation, heap, host, and engine/native/UI
persistence remain required.

### Independent Warped accuracy finding, 2026-10-01

Root's independent algebra found that math-audio's audio recurrence implements
`(z^-1 + lambda)/(1 + lambda*z^-1)`, while its design/response use the opposite
sign. The automatic Bark coefficient also supplies Hz to the Smith–Abel formula
specified in kHz. At 48 kHz, explicit lambda0.37, center1379 Hz, Q0.83 and +7 dB,
the current recurrence predicts only0.5059 dB at the requested center. Actual
public audio now confirms this: the measured-tone regression reports0.505903 dB,
and the separate unit-impulse DTFT reports0.5059028858 dB. Both the tone and
Bark-unit regressions fail before correction, as intended. Public capture r1
contains nine vectors: three zero-lambda controls pass; all six explicit/automatic
warped vectors fail the independent comparison. Peak errors span0.2003–0.4113.
See `artifacts/aud145-warped-public-r1/root-baseline-comparison.json`.

`artifacts/aud145-warped-independent-r1/` contains independent complex and
expanded-polynomial references plus nine 16,384-frame impulse vectors at
44.1/48/96 kHz and zero/explicit/automatic lambda. Its45 impulse-DTFT checks agree
with the analytic response within6.68e-14. Comparator controls reject zero,
truncated and nonfinite captures with the original AUD145 bounds. Only the zero-lambda
production controls pass before correction. This finding illustrates why the accepted
shared-realization routing checks do not establish coefficient/audio accuracy.
Fixing broken legacy Warped output is intentional correction, not byte-exact
compatibility; keep historical captures and qualify subsequent replay results.

### Warped correction and independent public gate, 2026-10-01

Luna corrected the allpass recurrence, feedback signs and Bark units in
math-iir-fir. Ten focused math tests and two public EQ regressions pass.
`artifacts/aud145-warped-public-r2/` preserves nine public impulse captures;
root's independent comparison passes all nine vectors and 45 complex points,
with maximum peak error 5.91e-8 and RMS error 4.64e-10. No fitted gain or delay
was used. Eight selected source inputs match the run snapshots; this is not a
complete EQ dependency closure. Astra accepted this bounded static correction
and public Peak checkpoint; remaining family/dynamic/whole-route gates stay open.

Equal captures across lambda are expected for this retuned RBJ Peak because
the prototype prewarp and runtime allpass factors cancel. They establish
response correctness, not a separate perceptual-resolution benefit. Preserve
the red baseline: the correction intentionally changes broken Warped audio.
Next, measure the 48 independent SVF cases before correcting the identified
shelf and response-helper defects; nine independent Kautz cases also await
production captures. Neither reference's passing controls establish a
production pass.

### Independent EQ multirate prefix reference, 2026-10-01

Root generated `artifacts/aud145-multirate-reference-r1/`: 84 cases covering the
existing 42 base-rate placement scenarios at 2x/4x, using their unchanged f32
inputs, an independent scalar RBJ recurrence at the elevated rate, and NumPy
f64 FFT transport. The transport settings are derived from actual Rubato 5.0.0
`Fft::new_custom` geometry and squared periodic Blackman-Harris window design,
not a halfband assumption. The packet records source hashes and explicitly
uses 256 frames of startup queue plus resampler delays (512 total base frames).
DC, delay and unpaired-channel transport self-checks pass. The comparator
accepts rounded reference data and rejects zeroed, truncated and nonfinite
controls at the predeclared 3e-5 peak/3e-6 RMS bounds, with no alignment/gain fit.
Production r2 now captures all 84 cases successfully. Root independently reran
the comparator: all pass, maximum peak error `1.0925755535851067e-6` and maximum
RMS error `3.028567880891523e-7`, within the unchanged bounds. Start/end/current
selected source inputs match. Results are in
`artifacts/aud145-multirate-production-r2/root-comparison.json`; r1's input-path
fixture failure is retained. Astra medium accepted this bounded reference/capture
checkpoint and the separate 56 homogeneous advanced/SVF composition cases in
`reviews/AUD145-astra.md`. The next relevant run-bound source packet must include
actual host oversampler/misc and Rubato implementation bytes; this packet only
records those actual transport files through reference-provenance hashes. Mixed
Biquad/Warped/Kautz ordering and advanced 2x/4x behavior remain assigned to Luna.
This is a cold 4096-frame ordinary-processing prefix: it does
not establish EOS, automation, advanced-filter or full-chain acceptance.

### Native Convolution embedded GUI protocol finding, 2026-10-01

The actual Xvfb CLAP test selected one case and failed at the `show()` callback
before interaction (`/tmp/sotf-aud134-clap-editor-xvfb-r1.log`). Vendored NIH
unconditionally returns false from `ext_gui_show`/`ext_gui_hide`; its comment
questions their applicability to embedded editors. The
[official CLAP GUI protocol](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/gui.h)
explicitly includes `show()` after both embedding/floating setup and permits
subsequent hide/show calls. Their declarations are not restricted to floating
windows. Root assigned this interoperability gap to Luna Upmixer: retain the
actual protocol assertion and implement/verify visible embedded show, hide,
reshow and destruction. A set-parent-only diagnostic cannot close this gap.
Convolution-feature test compilation itself passed without warnings before
this runtime failure. Subsequent visibility implementation passes the embedded
show/hide/reshow assertions, and the actual r4 GUI run delivers the CLAP restart
callback. The lifecycle still fails later: after a mix event to 0.37, saved state
contains 0.65. Root traced this to `serialize_parameter_overrides` copying all
pending editor snapshot values instead of only the structural selection. Luna
is fixing this and preserving full external-state restore semantics. Evidence
and source diagnosis: `artifacts/aud134-clap-editor-show-hide-r1/root-serialization-finding.md`.
The corrected r9 binary now passes the actual embedded CLAP lifecycle1/1 and
the focused callback module4/4 (GUI ignored there).29 selected sources match
the run manifests and archived copy. `receipt-r9.md` seals commands, binary,
screenshots and earlier failures. Astra accepted the bounded Linux CLAP
checkpoint; hide/show success and post-reshow interaction were demonstrated,
but hidden-window map state was not independently asserted. macOS/Windows
visibility and VST3 GUI/packaged routes remain unverified.

### EQ transaction review findings, 2026-10-01

Root's current-source review identified two additional requirements for Luna's
in-progress detached preparation:

- Legacy WarpedBiquad rate updates preserve delay history (`update_params` in
  `math-iir-fir/src/iir/warped_biquad.rs`). Replacing them with freshly constructed
  filters changes populated valid reinitialization; prepare a clone and retune
  it before commit. Legacy Kautz rate updates already rebuild/reset their state.
  Verify same-rate and changed-rate continuation against the legacy behavior.
- EQ rejects zero rate early, but rates 1–9 can reach live Biquad/sample-rate
  mutation before `AutoGain::set_sample_rate` fails. `GainMeter::new` requires
  at least 10 Hz, and AutoGain's setter currently mutates its rate before its
  fallible meter construction. Validate/prepare all fallible state before
  committing EQ; a populated refusal twin must preserve subsequent audio and
  metadata, including when AutoGain is disabled.

The first focused packet, `artifacts/aud145-transactional-init-r1/`, has
matching start/end manifests for ten selected inputs. Root verified those
hashes against the current files. The two public EQ refusal tests and one
shared AutoGain refusal test pass. The Warped continuation library gate fails
before execution because its fixture calls two private sibling-module helpers;
Luna is fixing the fixture while retaining the failed log. This historical run did not establish an accepted transaction checkpoint.

The corrected r4 packet passes all four focused tests (two EQ refusal cases,
one Warped continuation case covering same/changed rates, and one AutoGain
case). Ten selected inputs have identical start/end/current hashes, aggregate
`e61d6dd37c6c0c85a08d693b9cec0a601c632f27e30d5576b009c42293e526d1`.
The packet includes a selected source archive/tree and checksums. Root verified
the results and binding, then held Luna Crossover at this safe point and
started Astra medium's bounded transaction review. Astra accepted this checkpoint
without blocking findings; Luna resumed the remaining EQ work. Broader EQ placement
and whole-chain coverage remain outstanding; the later bounded numerical
acceptances are recorded above.

The transaction fixes are accepted within the r4 scope. Explicit placement's successful
structural reset remains intentional. Broader advanced/multirate and public-route
accuracy gates remain open.

### Native Crossover loaded baseline follow-up, 2026-09-30

Root inspected the terminal r3 loaded-state log: one stateful test passes for
both formats, covering complete default/Highpass vectors and a synthetic
frequency-only saved state against the public Crossover DSP. This is routing
and compatibility evidence, not an independent numerical oracle. The captured
native parameter map contains twelve IDs. Both mode is refused by both
formats; VST3 additionally fails to resume that directly modified instance.
The follow-up must use disposable consuming-host candidates and prove that a
refused replacement preserves populated active audio. Do not infer direct
native restore rollback from this baseline.

Log: `artifacts/aud142-native-crossover-baseline-loaded-r3/logs/stateful-baseline.log`.
The r4 capture now preserves exact native state bytes, input and full output
vectors, and labels the frequency-only case as synthetic compatibility.
Root verified all eight waveform files contain 514 finite, nonzero samples;
CLAP/VST3 input and corresponding outputs are bitwise equal. An independent
Python f64 bilinear Butterworth cascade then checked all six output vectors:
maximum peak 1.483e-8/RMS 6.253e-9, within predeclared 1e-5/1e-6 bounds.
Identity/opposite-mode sensitivity controls fail the bounds as intended.
No production coefficients or DSP are used in this calculation. The script,
results and input hashes are in
`artifacts/aud142-native-crossover-baseline-oracle-r1/`. This remains bounded
LR24 baseline evidence, pending Astra review; expanded native routing is open.

### Convolution editor compile and EQ baseline progress, 2026-09-30

Root inspected `/tmp/sotf-aud134-native-editor-topology-check-r1.log`: the
Convolution-feature check finishes successfully in 1.60 seconds after
constructor-fingerprint binding was added to the pending editor lifecycle.
This is compile evidence only. Overlapping loads, unrelated structural edits,
wet automation, native restart servicing and packaged GUI audio remain pending.

Root inspected the final EQ pre-edit baseline r3 under
`crates/sotf-plugins/target/audit-artifacts/aud145-preedit-baseline-r3/`:
the terminal capture test passes 1/1, all 19 artifact checksums verify, and all
10 selected current source/config/lock hashes match before/after. All four
full captures are finite/nonzero. AutoGain enabled/disabled twins cover 96,000
frames with RMS difference 0.0667647123; the oversampling fixture reads back 4x.
The durable packet is now `audit/artifacts/aud145-preedit-baseline-r1/`;
all 23 entries verify, including a separately corrected reproduction command.
Index SHA-256: `e6c622323608907e5641f58e4b1bd5cda67451b6d467089cdbe3641a34ba14ea`.
This is legacy/gap baseline evidence, not placement implementation acceptance.
Luna reached a safe checkpoint and was interrupted to free the review slot.
Astra medium is reviewing resolved proposal SHA-256
`5d77a5a4a725f7bbd96d73e0964c4e61376bbb734ca0eabe3a705474d2512625`
before any EQ routing edits.

### AUD145 concrete design accepted, 2026-09-30

Astra medium accepted the resolved EQ placement design for bounded
implementation; see the appended disposition in `reviews/AUD145-astra.md`.
Root supplied actual baseline source bytes in
`artifacts/aud145-preedit-source-r1/source.tar.gz`, SHA-256
`3d02d71878e4399181a280f7ec9c296b1b241a296067e149f2b71447b4984c25`.
All ten selected inputs match the original capture manifest and each archived
member was read back byte-for-byte. The original sealed audio packet is unchanged.

Luna resumed after Astra's safe handoff. Before EQ mutation, capture the promised
matched legacy timing. Preserve serialized absence/inherit for choice zero;
clearing the last explicit placement must restore legacy dispatch. Detached
structural construction may allocate on the control thread; zero-heap gates
apply to realtime processing/application/reset. Multirate EOS numerical
comparisons use declared tolerance, while frame counts and legacy replay retain
their exact contracts. No further general proposal gate is required. Core,
public state/control/native/UI implementation and accuracy review remain open.

### Native integration regression compile checkpoint, 2026-09-30

The Convolution service regression's first run stopped at compilation;
`/tmp/sotf-aud134-convolution-editor-service-r1.log` records six E0053 errors
from test contexts still forwarding unit background tasks and three BoolParam
setter errors. The earlier production feature check passed, but this run
establishes no behavioral result. Luna is fixing task forwarding and test
parameter mutation without dropping assertions. Host Crossover re-export
shadowing warnings are separately assigned to its owner.

Root also identified an exposed-parameter namespace mismatch in Crossover
reconfiguration: the preservation snapshot excluded plain structural names,
but public metadata uses clap.<id>/vst3.<id>. Luna acknowledged and is correcting
the mapping before transaction tests. The typed setup gate remains 3/3;
backend reconfiguration and actual loaded routes remain unverified.

### Convolution service r4 reaches tests, 2026-09-30

The macro now specializes Convolution background tasks; the focused r4 build
reaches tests after the host pure-hash helper feature gate is fixed. Log:
`/tmp/sotf-aud134-convolution-editor-service-r4.log`. One test passes (idle editor
resource epoch); the lifecycle test fails before its assertions at a missing
true_stereo parameter. Root traced this to its empty-schema fixture:
get_param_specs("Convolution") returns no static specs, and the real exported
wrapper falls back to factory-instance parameter metadata, while the fixture
omitted that fallback. Luna is correcting the fixture against the real wrapper
contract. No stale-candidate lifecycle pass is claimed yet. Earlier r2 and r3
attempts stopped at the host hash-helper cfg error before NIH tests.

### Convolution service green and EQ timing captured, 2026-09-30

Root inspected terminal r5:
`/tmp/sotf-aud134-convolution-editor-service-r5.log`, SHA-256
`300ef7a5b97065f8f9c4b125e4393802b9f5b04b7288e329bc9b263fe9b080a0`.
Both service tests pass: prepared candidate structural refusal/rebuild with
setter-boundary allowance, and idle editor refresh after external reinitialization.
The fixture now mirrors the actual exported Convolution parameter fallback.
Owner reports matching start/end editor/wrapper/params/manifest/lock hashes.
This is service-level dry-resource evidence; actual native editor selection,
nonzero IR audio, wet automation, host restart lifecycle and packaged GUI
acceptance remain required.

EQ pre-edit timing completed from a preserved release executable pinned to
CPU 0. Root verified 90 positive finite raw samples (five cases, setup/process,
nine samples each), matching selected source start/end manifests, and raw CSV
plus executable digests in
`crates/sotf-plugins/target/audit-artifacts/aud145-preedit-cpu-r1/`.
The coordinated quiet window is released. The durable packet is now
`artifacts/aud145-cpu-baseline-r1/`, with all 32 index entries verified by root;
index SHA-256 `5956d96c226146140dad7c18eeff693f900927fd23cc75bac3118427ad5b60a6`.
Process snapshots expose only the sandbox PID namespace, so they do not prove
the whole host was idle. The timing covers direct EqPlugin::process, not
compiled DawHost delivery; retain that scope and the load/affinity qualification.
No post-change performance comparison is claimed. All pre-edit requirements
are satisfied; Luna proceeds with approved EQ routing without another approval
checkpoint.

### EQ independent baseline and public schema preservation, 2026-09-30

Root's independent f64 scalar recurrence checks the frozen 96,000-frame
AutoGain-disabled twin: peak 1.48996e-8/RMS 4.12584e-9 within predeclared
2e-5/2e-6 bounds. The enabled/disabled stereo cross-product residual is
4.83269e-9 within 1e-7; identity and wrong-link controls fail as intended.
Script, results and capture hashes: `artifacts/aud145-independent-baseline-r1/`.
This verifies one base-rate peaking filter and common gain, not placement or
AutoGain's complete loudness law.

Luna identified working baseline Rust struct literals that need updating when
placement/pair fields are added. Root clarified that archived sources/audio
remain immutable; working-tree literals may receive None fields or deserialize
preserved JSON. Keep the accepted public BiquadFilterConfig/EqPluginParams
schema extension. Do not divert to a parallel API solely for fixture source
compatibility while existing public serde continues silently ignoring placement.
The unchanged JSON-based benchmark remains available for matched measurements.

### EQ schema compile checkpoint, 2026-09-30

Root inspected `/tmp/sotf-aud145-eq-schema-norun-r1.log`: `cargo test
--offline --locked -p sotf-plugin-eq --all-targets --no-run` finished in
7.97 seconds and emitted all EQ test/benchmark executables. The only reported
warning is the not-yet-used `requires_stereo_pair` routing helper. This checks
the new public placement/pair fields and mechanical literal updates; it does
not execute tests or establish placement processing, admission/refusal, or
whole-workspace compatibility. Luna continues the approved routing implementation.

Root also generated 42 independent base-rate placement references in
`artifacts/aud145-placement-reference-r1/`: three sample rates, two/five channels,
all five placements and both noncommuting Left/Mid orders. The five-channel
case has two pairs (one reversed) and an unpaired channel. Standard peaking
coefficients and f64 recurrence are computed in Python from f32-rounded input,
without production imports. Mid/Side matrix identity, order sensitivity and
unpaired identity checks pass. These are reference artifacts awaiting production
comparison, not EQ placement acceptance; advanced topologies, automation and
multirate gates remain separate.

Root review of the initial EQ placement setter flagged live sequential
advanced-filter preparation instead of the accepted detached candidate/commit
path. Luna must preserve populated state on refusal and clear complete EOF
bookkeeping on a successful structural transition. This is an implementation
review finding, not an executed refusal regression or accepted fix.

Root ran `cargo test --offline --locked -p sotf-plugin-eq` after the base-rate
placement and legacy replay additions: 150 passed, zero failed, two ignored,
no warnings (session 18857, exit 0). Log and qualified receipt are preserved in
`artifacts/aud145-eq-package-root-r1/`. The ignored explicit waveform capture
was checked separately; the historical placement-gap fixture remains ignored.
This package run has no start/end source manifest and is not a combined
workspace or advanced-placement accuracy gate.

### Convolution generated lifecycle checkpoint under review, 2026-09-30

The corrected r3 generated-plugin test passes 1/1 (session 93944, exit 0).
It compares populated old-resource audio through preparation/host deferral,
changes wet mix while pending, then reinitializes and checks the complete
four-path output against the independent reference at peak 1e-5/RMS 1e-6.
The old-IR sensitivity reference now uses matching topology and cold state.
Earlier r1 selected zero tests; r2 exposed an oversized fixture block, corrected
by partitioning within the negotiated maximum. Root preserved the log and
selected current sources in `artifacts/aud134-generated-editor-audio-r3/`.
Astra medium accepted this bounded checkpoint on 2026-10-01 after both evidence
corrections passed in `artifacts/aud134-editor-geometry-r3/`: current service
2/2 and generated lifecycle 2/2, including prepared-candidate geometry change,
out-of-order completion and successful fresh retry. All 27 archived inputs
match identical start/end manifests. Root independently verified the logs and
source bindings. Successful geometry reinitialization resets history; this is
not preservation across that transition. Luna Upmixer has resumed actual native
GUI restart callbacks and packaged editor work. Astra is at a safe checkpoint
for the next implementation review.

### Checkpoint history

The entries below preserve intermediate evidence, including superseded compiler
failures and review stages. The ownership table above is the current work state.

- Astra accepted AUD134's corrected bounded native callback/resource evidence.
  Luna resumed the missing native editor and host-serviced IR-selection stage;
  callback acceptance does not imply packaged editor or standalone runtime proof.
- AUD143 actual loaded VST3 descriptor probe passes 1/1 and explicitly reports
  six observations across six modern/legacy 2/3/4-band layouts. Root verified
  the terminal log `aud143-inactive-vst3-bus-arrays-r1/loaded-descriptor-vst3-r1.log`.
  Each process/observer call has an allocation/deallocation guard. The loaded
  bundle is the unchanged accepted fixture (SHA prefix `6f1803cb`), not a fresh
  build of the current NIH wrappers. Full loaded waveform/canary gates are next.

- Corrected Convolution packet r4 is sealed and back with Astra. Root verified
  tighter assertions, 2/2 terminal callbacks, and all 19 current/copied source
  files against the selected manifest with zero mismatches. This addresses both
  review requests without a production change or broader rerun.
- AUD143 VST3-feature layout module r2 compiles and passes 1 test; the real
  BandSplit descriptor probe is ignored pending its required external bundle.
  Luna is preparing the explicit loaded run. The 1-pass/1-ignored receipt is
  not presented as callback descriptor or loaded-audio acceptance.

- Crossover mounted r5 remains 1 pass/2 failures with no admission-rejection
  toast. Luna traced the actual product defect: this controlled toolkit Select
  was given `on_change` but no `on_toggle`, `is_open` or `on_highlight`, so its
  handlers cannot open/navigate the menu. Earlier keyboard sequencing theories
  did not fix the issue. Luna is wiring the required state and retaining actual
  mounted input tests; no direct-settings bypass is accepted as UI evidence.

- Astra's AUD134 callback review found no new production defect, but withheld
  acceptance for two evidence conditions: restore original peak <=1e-5 and
  RMS <=1e-6 waveform bounds, and include omitted changed CLAP/standalone wrapper
  and typed bool/float/integer smoother sources in the selected manifest/snapshot.
  Luna resumed to make the focused corrections and rerun both native callbacks.
- AUD143's first VST3-feature module gate failed during fixture compilation:
  observer comparisons mixed `c_void` and `f32` pointer types; unused test
  imports also need removal. No descriptor behavior was executed. An earlier
  default-feature command selected zero tests and is explicitly not a pass.
  The corrected gate will follow the current GPUI r5 run to preserve build inputs.

- Convolution callback packet r3 is sealed and under Astra medium review:
  `audit/artifacts/aud134-native-resource-r3/README.md`. Root verified checksum
  index and terminal logs. The full library and strict lint ran after the sole
  test-helper borrow cleanup. Native editor remains absent; cached NIH editor
  adapters require additional dependencies, and active GUI state restoration
  requires a supported host lifecycle rather than process-thread resource I/O.
- Crossover mounted r4 still passes only the live-rate case (1/3); splitting
  Down/Enter events did not change the selected family. Luna is checking both
  initial dropdown highlighting and candidate validation with dormant per-channel
  settings before attributing this to the fixture or product code.

- Mounted Crossover r3 compiles and executes three tests: one passes (actual
  8 kHz admission/refusal), two fail on family selection before their remaining
  route assertions. Root verified `/tmp/sotf-aud142-crossover-mounted-r3.log`
  and matching start/end selected-source manifests. Luna is distinguishing
  fixture interaction from a real dropdown/listener defect while preserving
  mounted input coverage. These failures do not establish the later waveform,
  persistence, unsupported-mode or fixed-width rollback assertions.

- Expanded Convolution VST3 r4 passes 1/1 without warnings. Root read the terminal
  log and confirmed matching selected-source start/end manifests in
  `audit/artifacts/aud134-native-resource-r2/`. Proper processing setup now makes
  the disappearing-file case reach initialization; numeric/path rollback and
  ordinary dry retry pass, as do explicit clear-resource and legacy dry checks.
  The callback checkpoint still needs affected regression/lint and Astra review;
  native editor selection is not implemented by state callback tests.

- Expanded Convolution VST3 r3 failed at the final pending-resource rollback
  assertion. Root traced the fixture's `try_activate()` call without prior
  `setup_processing()`: activation failed before Convolution initialization,
  so the accepted pending resource correctly remained staged. Luna confirmed
  the failing path and added setup before the intended missing-file activation
  failure. This is a fixture lifecycle correction, not evidence of broken
  clear-resource serialization. The corrected expanded gate is still required.

- Native Convolution VST3 callback r2 passes 1/1, confirmed in
  `audit/artifacts/aud134-native-resource-r2/logs/vst3-focused-r2.log`.
  Actual IComponent/IBStream restore covers fresh saved bytes, active refusal
  with preserved continuation, deactivated replacement with distinct IR samples,
  complete independent four-path waveform and deleted-resource refusal. The
  first run stopped at a test-macro import error. Explicit VST3 clear-resource/
  dry and failed-preactivation rollback cases, one test warning, native editor
  selection and independent review remain open.

- First mounted Crossover gate (`/tmp/sotf-aud142-crossover-mounted-r1.log`,
  owner session 97855) stopped before executing tests: the fixture partially
  moved `restored_crossover.extra_frequencies` before audio replay, and had two
  unused imports. Root verified the compiler output. Luna is correcting this
  fixture and adding Matrix input adaptation plus an independent band-major
  merge reference; no mounted behavior is accepted from this compile attempt.

- Astra accepted the AUD135 publication/acknowledgment race correction and
  bounded finite-tail/preflight/reset-recovery checkpoint. Root read the appended
  review and dispatched Luna BandSplit to the queued AUD143 inactive VST3 bus
  array correction. Full workspace, broad native/UI and platform acceptance
  remain separate.
- Crossover route review found another width path requiring correction:
  `PluginGraph::adapt_matrix_to_input` advances BandSplit/BandMerge geometry but
  omits Crossover, potentially resizing the downstream Matrix incorrectly on
  track adaptation. Luna Crossover owns the fix and actual adaptation regression,
  alongside strengthening the mounted route's waveform oracle.

- AUD135 actual-host acknowledgment race regression r2 passes 1/1. Root read
  both current test and terminal log at
  `audit/artifacts/aud135-worker-classification-race-r1/logs/worker-proxy-race-r2.log`.
  The worker pauses after publishing status 1; the real proxy resolves and
  clears it before worker classification. The test checks the host latch,
  rejects Describe, observes Reset ACK before timeline reset, and compares four
  complete gain-2 blocks plus latency flush. The first run is retained as a
  test-fixture mutability compile failure. Broader affected gates and Astra
  re-review remain required; previous package gates predate this correction.

- Native Convolution's actual CLAP callback r2 passes 1/1, confirmed by root in
  `audit/artifacts/aud134-native-resource-r2/logs/clap-state-focused-r2.log`.
  The fixture replays saved bytes, uses distinct four-path replacement IRs with
  a complete independent waveform oracle, checks reported latency/tail extent,
  preserves active history after refusal, compares complete legacy dry output,
  and checks activation failure/retry after a staged IR disappears. Numeric
  parameter rollback assertions and one owned unused-mut warning remain to be
  addressed. This does not establish VST3 lifecycle or reachable native selection.

- Root inspected the AUD135 P1 correction in the live diff: an internal
  `WorkerRequestOutcome` carries ordinary plugin failures independently of
  host-cleared shared atomics, while the existing public IPC result API remains
  compatible. The first deterministic publication gate used raw `clear_block`;
  Luna is strengthening it to exercise the real host proxy acknowledgment and
  non-Reset control refusal. No new gate or Astra acceptance is claimed yet.
  Cargo owners were directed to use the shared flock without circular waits.

- Astra found a remaining AUD135 recovery race in the frozen checkpoint:
  `process_worker_request` publishes WorkerFailed/status1 before its caller
  classifies the error by rereading shared state. Host `resolve_pending` can
  acknowledge and clear that state in between, causing the worker to classify
  the same recoverable error as fatal and exit. Acceptance is withheld. Required
  fix: worker-local typed outcome established independently of host-cleared
  shared state, plus a deterministic host-acknowledgment-between-publication-
  and-return regression. Astra completed review and supports the finite-tail/
  preflight evidence; Luna BandSplit is implementing the sole P1 correction
  before another Astra medium check. See `audit/reviews/AUD135-astra.md`.

- AUD135 finite-tail/worker recovery is now frozen for independent Astra medium
  review. Handoff: `audit/artifacts/aud135-finite-tail-stateful-r1/worker-recovery-handoff.md`.
  Final subprocess r5 is 2/2; seven selected host files have identical start/end
  manifests (`22ea4aa14d571a5c53a3334674e46cbb63b86501c2e4f0fa9bca4aec08d4868a`).
  Luna BandSplit has yielded; Astra is reviewing the original finite-tail
  requirements plus the shipped-worker recovery correction. No acceptance is
  claimed yet; AUD143 inactive VST3 bus-array work remains queued and untouched.

- AUD135 follow-up regression filter passes 134/134 (`worker-proxy-classification-r1.log`),
  worker binary tests pass 14/14 (`worker-cli-tests-r1.log`), and strict all-target
  host Clippy r2 passes (`worker-recovery-clippy-r2.log`). Root inspected all
  terminal result lines. Initial Clippy r1 exposed six test Args literals
  missing the new test-backend fields; those are corrected. Final-source
  subprocess replay and packet sealing are in progress before Astra review.
- Crossover mounted-test preparation found output-width propagation missing
  from `update_channel_dependent_plugins`, despite correct standalone width
  calculation. Luna is fixing structural graph refresh and adding downstream
  BandMerge route coverage; compile success alone did not establish this path.

- AUD135 actual shipped-worker recovery r4 passes 2/2, confirmed by root from
  `audit/artifacts/aud135-finite-tail-stateful-r1/logs/shipped-worker-recovery-r4.log`.
  The spawned binary remains alive after the recoverable process error; public
  Reset followed by metadata synchronization and an 8,192-frame pipeline drain
  yields the complete 2×gain reference, distinguishable from fallback. Native
  Reset refusal remains quarantined with fallback. Earlier r2 was a fixture
  config compile error; r3 passed refusal but caught the test backend's missing
  identity-frame declaration. Host regression/classification and strict lint
  gates, packet sealing and Astra acceptance remain pending.

- Native Convolution compile r4 now passes: `cargo check --offline --locked
  -p plugins-nih --features convolution`, log
  `/tmp/sotf-aud134-nih-convolution-check-r4.log` (Finished dev profile, 1.42s).
  Root verified the log. Typed NIH initialization helpers reset smoothers
  through ParamMut, and validation retains a prior accepted pending state when
  the replacement is invalid. This is compile evidence only: actual CLAP/VST3
  lifecycle, resource roundtrip, full audio/EOS and reachable selection remain
  required before review and acceptance.

- GPUI Crossover compile r6 now passes: `cargo check --offline --locked
  -p sotf-gpui --features dev-api --lib`, terminal exit 0, log
  `/tmp/sotf-aud142-gpui-check-r6.log`. Root inspected the completion line;
  two existing player unused-import warnings remain. This establishes compile
  compatibility only; mounted controls, persistence and full audio-route gates
  are next. Earlier r4/r5 compiler errors are historical failed attempts.

- First current-source compiler results: GPUI r4 (`/tmp/sotf-aud142-gpui-check-r4.log`)
  reached the new panel and failed on six owned callback-context, numeric-type
  and empty-vector pattern errors. Native Convolution r1
  (`/tmp/sotf-aud134-nih-convolution-check-r1.log`) reached plugins-nih and failed
  on a missing bridge helper re-export plus three smoother API calls. Both
  owners are correcting their code before behavioral gates; neither is an
  external dependency blocker or passing implementation result.

- Current GPUI build preparation: root synchronized only twelve local DAW
  package versions in the sibling SOTF lockfile after the minor bumps. Parsed
  external dependency records are unchanged, and existing user lockfile edits
  are preserved. This resolved the misleading offline math-test-functions
  version conflict. Linux-target `cargo fetch --locked` exits 0 without further
  lock changes; full all-target offline metadata still reported an uncached
  dashmap 5.5.3. The scoped GPUI compile is being retried, not yet passed.
- AUD135 worker classification r2 passes its focused reset/fresh-audio unit
  test (1/1), and the worker binary check passes with test-backend + CLAP.
  Root inspected `logs/worker-classification-r2.log` in the finite-tail packet.
  This is not the required spawned-process or ordinary-callback recovery gate.

- User-requested release preparation: workspace/facade 0.8.0, engine 1.1.0,
  FFI 0.7.0 and twelve other changed packages 0.6.0; matching changelogs updated.
  MIDI/IAMF and external dependency resolutions remain unchanged. Full
  `cargo metadata --offline --locked --format-version 1` passes. Current lock
  SHA-256: `edd61e466ec41393a4ee6228564c79b668546d3e481fd934dc641fc2f39a08bf`. Older packet lock hashes remain historical.
- AUD139 provenance correction r2 binds 42 selected inputs including all seven
  changed VST3 production files; focused 2/2 passes. Crossover default-sync
  correction uses the real wrapper constructor, with focused 1/1, full NIH
  118 passed/one ignored and strict lint. Astra accepted both bounded packets.
- AUD135 stateful finite-tail first gate now passes 1/1:
  `audit/artifacts/aud135-finite-tail-stateful-r1/logs/stateful-focused-r2.log`.
  A stereo FIR with delay 16,384 is compared against independent f64 scatter
  convolution over all process/drain samples; native continuation is 16,384
  frames, pipeline flush is 8,192, final L/R markers are nonzero, and reset
  replay is exact. Earlier r1 was a fixture compile error. The passing build
  still warns about a helper reserved for the pending-gate test; this is not
  strict lint or completion of the requested failure matrix. Root identified
  tail-slice, IPC publication synchronization and cleanup issues; Luna corrected
  them before execution. Pending/capacity/timeout/failure cases remain active.
- AUD135 pending-request coverage now passes in two distinct routes: the
  stereo begin-drain path (1/1) and actual 2→2→4 channel-changing preflight (1/1).
  In the latter, short capacity leaves prepare/begin counters at 0/0, a held
  worker timeout reaches 1/0, and release/retry reaches 2/1. Complete quad audio
  matches the independent transformed FIR oracle. The exact output sequence is
  three 8,192-frame nonterminal calls then a zero-frame terminal mapper call.
  Logs: `audit/artifacts/aud135-finite-tail-stateful-r1/logs/`.
  Initial channel-changing r1 failed an incorrect three-call expectation;
  r2 preserves full-vector/count assertions and passes. Post-mutation sticky
  failure/reset and Unknown/Infinite refusal remain outstanding before review.
- AUD135 sticky failure r3 exposed a production reset defect after the
  corrected pipelined fixture observed actual WorkerFailed publication. Public
  drain reports the failure and later process/begin/prepare refuse, but
  reset_checked fails on the retained failed pending sequence. Log:
  `audit/artifacts/aud135-finite-tail-stateful-r1/logs/sticky-failure-focused-r3.log`.
  Luna is implementing a reset-only discard of an exactly matching, fully
  published failed request; still-processing/mismatched requests must remain
  protected, and native Reset must succeed before clearing the sticky state.
  This was a real red gate. Follow-up `sticky-failure-focused-r4.log` now passes
  1/1 after a reset-only discard of the exact published WorkerFailed sequence.
  Native reset ACK precedes latch/timeline clearing; complete fresh replay
  matches the independent reference. Root inspected the production diff and log.
  The broader `external-plugin-module-r2.log` passes 25/25 after repairing an
  existing latency-handshake fixture to service the real native Reset request.
  Strict lint r1 found four new test-code lints; correction, explicit refusal
  coverage, packet freezing and Astra validation remain pending.
  **Full-chain recovery gap discovered during root review:** the test thread
  ignores `ExternalPluginWorker::process_one()` errors, while the shipped
  `bin/external_plugin_worker.rs` loop propagates them and exits. Consequently
  r4 proves reset with a surviving in-process worker only. Luna must cover
  recovery through the actual subprocess lifecycle (with safe differentiation
  of recoverable plugin errors from panic/transport failures) before claiming
  shipped-worker reset recovery. Existing worker-test-backend is the preferred
  integration seam; do not weaken fatal-error isolation to satisfy the fixture.
  The first production correction is now present: an exact matching status-1
  process failure yields `ProcessFailed` and latches the worker until Reset
  succeeds. Other control requests are refused while latched; panic, invalid
  frame-count and transport errors remain fatal. Unit fixtures for successful
  reset/fresh audio and reset refusal have been added. Subprocess evidence and
  final gates are still pending; this source review is not a passing result.
  Root found a normal-playback interaction that must also be corrected:
  `resolve_pending` clears WorkerFailed and the same callback can publish new
  audio, but the newly latched worker will not consume it. A subsequent public
  reset then waits on this unconsumed request and quarantines on timeout.
  Add ordinary process failure → public reset → fresh audio coverage, prevent
  post-failure submissions, and preserve protection for genuinely active work.
- AUD143 exact history correction is Astra-accepted.
- AUD135 actual engine candidate/refusal/retry and nonzero final-program EOS
  route is supported by Astra. General finite-tail acceptance remains pending:
  Luna is adding a stateful nonzero multi-transport-block oracle, deterministic
  pending gate, capacity-before-wait, timeout/retry, sticky failure/reset and
  explicit Unknown/Infinite refusal. The old count-only test is insufficient.
- Crossover initial schema/core/engine gate passes: core explicit-count 1/1,
  engine crossover filter 9/9, log `/tmp/sotf-aud142-ui-schema-r1.log`.
  The optional typed band_count preserves legacy omission and dormant cutoffs;
  mounted controls, full compatibility review and broader route gates remain open.
- Luna Crossover has resumed product UI/app integration; no mounted route is claimed.
  The first custom panel now validates a cloned candidate using graph input
  width and the app HAL sample rate before committing settings. Root identified
  a four-band cutoff-generation failure near the low-rate upper limit despite
  available valid cutoffs; Luna must fix this and retain the FIR taps control.
  Follow-up source restores the FIR taps control and distributes new cutoffs
  through the remaining legal interval. Admission now reads the signal-path
  rate instead of the HAL preference; root requested a short circuit so live
  edits do not also perform an unnecessary fallback device-rate probe. Mounted
  callback, actual route-rate and compatibility gates remain pending.
  The full audit and coherent combined workspace gate remain open.
- Baseline `aa0a3d1` now contains the implementation/minor-version batch.
- AUD143 host ABI follow-up: inactive VST3 bus arrays violate the SDK contract;
  scoped evidence and requirements are in
  `audit/handoffs/aud143-inactive-vst3-bus-arrays.md`. Queue after finite-tail work.
- Luna Upmixer has resumed AUD134 native convolution IR resource/persistence
  implementation planning, separate from Crossover UI and host drain ownership.

## Latest retry checkpoint, 2026-09-30

This checkpoint supersedes the earlier active-worker statuses below.

- AUD139 VST3: focused 2/2 and strict all-target NIH Clippy pass; sealed packet
  `audit/artifacts/aud139-vst3-restart-r1/` is with Astra. Full NIH library:
  117 passed, one ignored, one Crossover `frequency_2` default-sync failure.
  Luna Crossover owns the correction before resuming product UI work.
- AUD143: Astra accepted native buffer handling and loaded all-band delivery.
  The remaining history-evidence correction passes both actual loaded formats
  with exact finite live/twin vectors. Packet
  `audit/artifacts/aud143-native-refusal-exact-r1/` passed Astra's narrow recheck;
  the history-evidence finding is closed.
- AUD135: final host tail 2/2, multi-call drain 1/1, worker build, actual engine
  refusal/retry/EOS 1/1 and strict host/engine Clippy pass. Packet
  `audit/artifacts/aud135-engine-eos-route-r3/` awaits Astra review. The fixture
  sends nonzero final audio immediately before EOS and verifies full vectors
  and counts; 250 ms margins between earlier callbacks limit the result to
  this scheduling regime. The earlier degraded-output refusal remains recorded.
- Root verified both sealed packet indexes (19 and 9 entries) and checked the
  final engine logs. Cargo.lock retains SHA-256
  `db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`.
- No coherent full-workspace green gate or final audit completion is claimed.

## Active work at a glance

| Track | Owner | Current result | Next required gate |
|---|---|---|---|
| AUD135 native Ambisonics | Luna BandSplit; Astra review | Earlier revised native restore/reconfiguration and isolated worker checkpoints are Astra-accepted. Actual engine fixture now passes candidate refusal/history and valid retry but fails true EOS; the diagnostic confirms missing explicit identity geometry. Luna is implementing native tail/geometry metadata and exact isolated pipeline draining | Execute full nonzero decoder reference, exact prefix/drain counts, one EOS, reset/refusal/retry and ordinary-silence cases. Isolated CLAP and mounted setup/reactivation remain open |
| AUD138 HAL host EOF | Luna Upmixer; Astra review | Revised full HAL library tests pass 71/71 and strict HAL all-target Clippy passes. New cases cover writer-error cleanup, active-drain resize, mutations before commit/during priming, deterministic staging failure and exercised large buffers. Earlier host 551 passed with one ignored; host production remains unchanged since that checkpoint | Astra accepted bounded host/recovery/reprepare after the added multi-chunk-tail regression and strict lint. Include in the next combined gate; engine/application sink admission and native playback remain open |
| AUD140 channel-changing EOF | Luna Upmixer; Astra review | Astra accepted the frozen implementation: focused 13/13, saved replay 1/1, host 551/551, Ambisonics 59/59 and strict lint; ordinary fast-path helpers preserved | Include in the next combined workspace gate; consuming engine endpoint coverage remains open |
| AUD141 multiway LR24 crossover | Luna Upmixer; Astra review | Astra accepted the bounded implementation after the final finite/peak assertion refinement. Package 104/104, saved compatibility replay, actual stereo Crossover → BandMerge host chain, focused automation and strict lint gates pass | Include in the next combined workspace gate; AUD142/AUD143 remain separate |
| AUD139 Dynamic EQ shelves | Luna Upmixer; Astra scoped core/callback/CPU accepted | Frozen scalar synchronization and in-process CLAP restart/refusal/retry checkpoint is Astra-accepted. DynamicEQ 71 and NIH 116 tests with strict lint pass. Peak new/old ratios 0.989306–1.060697 meet 1.10; matched LowShelf/Peak and HighShelf/Peak maxima 0.841800/0.925338 meet 1.25 with artifact/workload provenance limits | Luna is implementing actual VST3 deferred restart/retry. Broader FFI/engine review, consuming host/UI, AU and full feature parity remain open |
| AUD144 FFI preset envelope | Luna crossover; Astra bounded checkpoint accepted | r3 passes both unchanged public probes: all 9 invalid envelopes reject without state/audio changes, and genuine FletcherMunson→LoudnessCompensation migration again matches exact state and all 9,464 samples. Focused tests pass 3/3; full FFI library passes 82 with one manual utility ignored; strict lint passes | Sealed r3 accepted by Astra; include in the next combined gate. Owner continues Crossover FFI/engine routes |
| AUD143 BandSplit phase compensation | Luna BandSplit; Astra bounded core accepted | Independent DSP, legacy replay, offline split/merge, mounted controls, scoped migration, lifecycle and CPU checkpoints accepted. Refreshed artifact passes 5 loaded tests with complete public-DSP/DawHost vectors, native restore refusal preserving populated audio, and valid retry. New absent/present/absent buffer invariant and unchanged prefix tests pass 3/3; BandSplit mask checks pass 5/5; full NIH passes 114 with one manual utility ignored and strict all-target lint passes | Frozen native-buffer/host checkpoint awaits its turn in Astra review. Include in a coherent workspace gate; owner proceeds to AUD135 |
| AUD142 crossover families/slopes | Luna Crossover; Astra bounded core and FFI accepted | Independent numerical and legacy/core CPU packets are preserved; focused final core additions pass 10/10 and strict target lint, accepted by Astra with unchanged bounds. Bounded FFI state/preset checkpoint is accepted. Earlier full core package passes 117 tests, 2 manual captures ignored; typed engine route passes 7 focused cases and existing width group 17 cases; strict engine lint passes | Luna is implementing the product UI/consuming app route while Upmixer owns shared native wrapper edits. Native bus/callback, engine/application and mounted controls remain open |

Accepted recent scoped work: AUD133 named Ambisonics orders 1–7, AUD134
true-stereo convolution, AUD135 framework/native-wrapper ABI, AUD136
SpeechDenoiser accepted-program EOS, AUD137 same-rate identity-frame ABCompare
composition, AUD138 bounded serial sink host/recovery/reprepare, AUD140 finite
serial channel-changing EOF and AUD141 multiway LR24
recombination. Their reports retain the
remaining feature, route and quality limitations. The last 6,100-test broad
gate covers AUD133/AUD134; it predates the current native and EOF edits.

Production Rust implementation uses Luna at xhigh; independent implementation
validation uses Astra at medium. Root coordinates, inspects evidence and writes
audit documentation and external probes. Active owners coordinate shared source changes and serialize real
Cargo commands. Once a coherent snapshot is ready, run a combined workspace
gate before claiming integration across these batches. Changes since restart
are uncommitted. The full remaining audit below is still required.

### Current worker handoff, 2026-09-30

Three Luna xhigh owners are active. Luna Upmixer has frozen the DynamicEQ
scalar/CLAP and matched shelf CPU checkpoints; Astra medium accepted their
bounded scopes. That Luna has resumed actual VST3 deferred restart/retry. Luna BandSplit is fixing the actual engine/isolated-worker
EOS refusal, preserving conservative unknown/infinite tail admission and full
transport audio. Luna Crossover has sealed typed engine and guarded native scalar/cache gates.
It is closing Astra's focused numerical/lifecycle evidence requests before
resuming the native bus route; shared NIH wrapper ownership is coordinated
with Upmixer's VST3 work. Shared source ownership and
Cargo execution are coordinated. Root has sealed the controlled Peak CPU and
Crossover engine packets; see the closing entries below.

Astra medium accepted the bounded Crossover FFI and preset-envelope checkpoints,
then resumed successfully after a Luna checkpoint freed the concurrency slot.
It accepted DynamicEQ scalar/CLAP and qualified CPU evidence and reviewed the
frozen Crossover core/CPU packet. Core acceptance needs two focused evidence
additions, recorded below; this is not a demonstrated production failure.
Earlier thread-cap dispatch failures remain historical, not current review
acceptance or a user approval requirement. BandSplit native-buffer and later
engine/native routes still require their scoped reviews.

Current handoff update: the Crossover Luna completed the focused additions,
root's two follow-up comparisons and strict target lint, then released its
slot. Astra medium resumed successfully on the durable final-source packet
described below. Two Luna owners continue VST3 and engine EOS implementation.

### Latest executed checkpoints, 2026-09-30

AUD142's intermediate runtime-metadata gate now passes: one Crossover core
regression verifies complete explicit channel modes take precedence over dormant
global output `Both`; two public C-ABI tests verify topology-specific IDs,
frequency normalization and all 13 constructed filter-family values/labels.
LinearPhase retains its trailing `fir_taps` ID. Root inspected the terminal log
`/tmp/sotf-aud142-ffi-route-focused-r5.log`, SHA-256
`afe214954cfa9e30fe6a1a5d8cde087b7455b3ba9155cb6bfb06ac9625decc13`.
Luna is preserving that intermediate evidence before the next source changes.
This intermediate gate is metadata/admission evidence. The subsequent state and
public API audio gates below extend the coverage; actual engine/native/UI
delivery and Astra review remain open.

Root also executed an independent exported-C-ABI state probe against the sealed
pre-route FFI library. It fails 19/30 cases: 17 valid family/alias/topology
imports fail, and two topology-key imports return success while discarding
populated audio history. Complete vectors/documents/states and the unchanged
probe are frozen under `artifacts/aud142-public-state-probe-pre-route-r1/`
(177-entry checksum index
`bb2375ea2a61302eb0bfbdf077f88d3c2af0640d98e44e4384cbac1bbbb23c22`).
This is a baseline-library failure, not a verdict on Luna's concurrently edited
state helper. Root subsequently ran the exact unchanged probe against sealed
corrected r1 library `d8ed320790fd445edd515922dc54b8b67a30fa04204d72936c849cf77cb025e3`: all 30 cases pass. The 177-entry packet is
`artifacts/aud142-public-state-probe-corrected-r1/`, checksum-index SHA-256
`c7284a222281e6d4cee4ea0d4cfdd7515b5de62a1dadf700775f65eb3b8e4873`.
Complete fresh-reference audio and refusal continuation are route evidence,
not independent filter mathematics. Full FFI then passes 91 tests with one
manual utility ignored, and strict all-target Clippy passes.

A separate eight-case public full-preset probe finds one remaining r1 defect:
a genuine two-way full preset is accepted by a four-way target with the same
output width, retaining extra cutoffs and resetting populated history. Status
is 0 instead of -8 and the continuation residual is 0.3279915056. Partial raw
updates, same-topology full import, reverse/per-channel topology refusals and
FIR family changes pass. The unchanged companion probe and all vectors are
frozen in `artifacts/aud142-full-preset-probe-pre-fix-r1/`. Luna has corrected
the full-preset boundary; seven focused tests and strict lint pass. A corrected
immutable r2 build then exits 0, with all 342 selected source hashes matching
start/end. Root reruns both unchanged probes against library SHA-256
`8424bfe9375ca5d621754d330ef1b9acc662392f0d0897fce7bda43bdefcaa73`: state
30/30 and full-preset 8/8 pass. The corrected packets are
`artifacts/aud142-public-state-probe-corrected-r2/` (177 entries, index
`95888f36980eb830787458dfe00a4672693acc0a17c9669f1740dbc2079759d3`) and
`artifacts/aud142-full-preset-probe-corrected-r2/` (59 entries, index
`37ec55c9b5f3b0b9d8da1aa386a21ea29764d20d560d69dc276a572cd85aaf2c`).
Astra medium accepted this bounded FFI checkpoint in
`reviews/AUD142-ffi-astra.md`, with no correction identified. Engine/native/UI and
the separate pure-core/CPU review remain open.

AUD139 intermediate native-control metadata/fingerprint tests pass 2/2
(112 filtered), using `plugins-nih --no-default-features --features dynamic-eq
--lib params::dynamic_eq_restart_tests`. Log SHA-256 is
`9d5645ae9a4f39f88d1bbbc8502f769f98478b67218ae92e118359bf9a1f94d7`.
This verifies visible manual shape/slope controls and a fingerprint that still
detects other structural changes. It does not yet verify native restart
dispatch, host service, post-service audio or the SOTF consuming-host path.
The earlier root-target failure was environmental; the later fixture typo
`band_0_freq` was corrected to `band_0_frequency` before this green run.

The first actual CLAP lifecycle fixture is now implemented. Its r1 compile
failed on a test raw-pointer dereference; r2 compiled and selected one test but
aborted with `memory allocation of 456 bytes failed` (SIGABRT), so it does not
provide passing lifecycle/audio evidence. Luna corrected the fixture's host
lifetime/drop order and the same-value restart-intent semantics. Root then
reproduced the abort under GDB on a frozen executable: the first ordinary Peak
callback calls `DynamicParams::sync_to_plugin`, which reads a global parameter
through `DynamicEqPlugin::parametric_get_parameter`; its fallback constructs
the entire `current_values()` BTreeMap and allocates 456 bytes. This occurs
before the shelf write or restart request. The exact stack, captured getter,
compressed raw debugger log and provenance limits are frozen in
`artifacts/aud139-native-getter-abort-r1/` (index SHA-256
`fa196cbe427cbbdc5d0594ad9d3799e9293a40c97f2a2a1b6a8bded99b833d2f`).
Luna has replaced the getter fallback with direct scalar reads and implemented
direct valid realtime scalar setters. The focused Dynamic EQ NIH gate passes
4/4 (112 filtered), including cold/changed parameter guards and the actual CLAP
restart lifecycle. Complete post-service shelf output matches a fresh native
instance and a separately configured public LowShelf reference; the old Peak
history remains unchanged before service. The durable selected-source packet is
`artifacts/aud139-clap-allocation-fix-r3/`, log SHA-256
`40fe7d740ba9bf8a07624924c0a1bffe1079c7ea3d8013af79fec532cc3f5f5b`.
Root then caught DynamicEQ-only assertions running for unrelated families in
the shared helper. After that fixture correction, the full scalar-getter module
passes 15/15, log SHA-256
`16c47ac98a4e36953aa20f60a4ab41dfd402c2405ac725d78632c0e1fb4c3ce5`.
These are separate test-source revisions. The expanded retry/refusal and full
NIH gates subsequently pass as detailed below; VST3, the consuming SOTF route
and Astra review of this native checkpoint remain open.
The next expanded five-test NIH run aborts with a 72-byte allocation; only the
two metadata cases print success before the test process receives SIGABRT.
Its shell pipeline incorrectly reports zero without pipefail, so that shell
status is not passing evidence. Luna is isolating the all-exported-control
matrix from the expanded CLAP retry fixture. The isolated all-exported-control test confirmed that valid dormant
threshold/ratio controls reach an allocating inactive-band rejection. Luna
now updates canonical stored slots through the direct scalar path; the
isolated guarded test passes. Expanded CLAP lifecycle r7 passes coalescing,
old-Peak continuation after an ignored request, same-value retry, invalid 8 kHz
shelf preparation refusal and valid 48 kHz activation matching the direct-core
reference. Normalized slope readback uses a 1e-6 tolerance for f32 quantization;
complete audio remains exact. Full DynamicEQ package subsequently passes 71 tests (52 unit, one exact Peak
replay, two lifecycle and 16 integration), with two manual capture/CPU utilities
ignored; strict all-target core Clippy passes. Root inspected the full package
log (`4c85b5f0b9275d6f4e70c04826ff9a9f5b1b1faa0b7867a2259668f6adb81da0`)
and core lint log (`4c92c7be06f3d4977007ac0a4ed8516090b7c7418c99070f09919d278c931d1e`).
Full NIH library passes 116 tests with one ignored, and strict all-target NIH
Clippy passes. Exact selected source/log evidence is being preserved before
Astra review and VST3 edits.
Source inspection additionally traced the SOTF consuming
setter: native parameter collectors default to realtime metadata, and the
engine acknowledgment precedes CLAP event delivery/reconstruction. The required
loaded-host/engine applied-audio and lifecycle checks are recorded in
`dynamic-eq-ui-route-findings.md`; plugin notification alone will not close them.

AUD135 fresh packaged Ambisonics library SHA-256 is
`7dfc7b96519b0d884a3b06633c009e3024f0a5a46a418236f43596699f74a3b8`.
The initial strengthened native loaded r3 gate **failed**: CLAP passed; VST3
returned `failed to process audio (tresult 1)`. The VST3-only backtrace locates
this at the existing order-7 SevenOne reconfiguration (test line 391), before
its new late-refusal section. The same strengthened fixture fails at that stage
against both old and fresh immutable artifacts, so this is not specific to the
refresh. Removing the added single-band retry/sensitivity checks diagnostically
did not change the failure; those checks were restored in the final fixture.
Tracing the native VST3 reconfiguration path identified a stale
`output_bus_widths[0]` after the negotiated output shrinks from 16 to 8 channels;
the narrow backend correction now passes the full focused VST3 route (1/1),
including restored single-band checks, dual-band history and wide→8→wide
processing. Log SHA-256 is
`18ea1d9a3e29d71fb1bf12a26a887daea9fc367cbbe16b479e7a05d98c029cc5`;
host source SHA-256 is
`41b56cbcb96c240370273e06358f19ed29d0e674f1fbcf2a49b7abc8cab44b63`.
The subsequent combined CLAP/VST3 run passes 2/2, log SHA-256
`2148234eebac9d5025f4996dab44ba02067a53bc6dab3a1fbac315c33b85f4b5`.
Strict host lint with both external features also passes, log SHA-256
`8a26ab1ed59b36274f633bf77bfb1af830e14fb2b777a7ec4081c1facaffc547`.
Root inspected both terminal logs and verified all 13 archived files in
`artifacts/aud135-vst3-output-layout-refresh-r1/`; its checksum index is
`63f4d5482c004ef1b5c3e668495a90e368fd45bad1f24a2165512d89a2b843c3`.
The packet records the missing exact red-source snapshot and incomplete
transitive-source coverage. Astra review remains open. The
terminal log `/tmp/sotf-aud135-native-late-restore-host-r3.log` hashes to
`0e1e40aa5ad5e02af664408c92f24d02f0a6f81d951f27311b80b1f66ef9f141`.
Earlier r1/r2 attempts exposed test predicates that expected structural readback
failure; the native library rejects the conflicting candidate during state load
after the outer envelope validates. Those diagnostic predicates were corrected
without removing full-state, live/twin audio, sensitivity or valid-retry checks.
The single-band case checks persisted Max-rE sensitivity; the later dual-band
case additionally checks retained recursive history against a cold instance.
These later green gates close this tested native in-process correction, not
the remaining isolated-worker, consuming-engine, mounted-UI or EOF scope of AUD135.

The requested external-plugin engine update now has a separate preflight
red-to-green checkpoint: the original best-effort builder skipped the failed
external plugin and sent `CommitHostUpdate` with an incomplete chain. A narrow
check in `apply_plugin_update_once` now rejects that requested candidate before
preparing or sending the update; initial startup keeps its existing best-effort
policy. The linear no-send case and startup positive control pass 1/1; the
existing BandSplit guard passes 1/1; the graph no-send case passes 1/1. Root read
the terminal logs. Linear/BandSplit log SHA-256 is
`858b05d71ee0d0e5cf08584ae17a30e803c06e672ebd467c4f854afb18b26bfb`;
graph log SHA-256 is
`456c90d2a64802d533a4a180c7fec937f6861ccb774c7dc36dc83d926a299deb`.
These tests use a missing descriptor and command probes: they establish no
processing/playback command and retained engine metadata, not a running isolated
native worker or old-host audio continuation. Those remain the next required
checks.

The separate actual isolated VST3 worker fixture now passes 1/1. It loads the
frozen Ambisonics artifact at 64→16, asserts worker latency equals the direct
native latency plus 64 IPC frames, and compares complete worker vectors against
the delayed in-process reference (maximum allowed residual 1e-6). This covers
warmup, post-refusal continuation, zero-input tail, alternate Max-rE settings
and valid retry. A valid typed order-seven envelope carrying genuine order-one
opaque state reaches the detached worker's native state load and is refused;
populated live/twin saved state and continuation remain unchanged. A cold
dual-band control establishes recursive-history sensitivity. These are ordinary
zero-input process blocks, not an end-of-stream drain test.

The actual green log `/tmp/sotf-aud135-isolated-vst3-worker-r2.log` hashes to
`857b611e3e4fef05b11e26cceef4233fe4526e4960781b4cda6fb8fd434303b6`;
executed test source hashes to
`9acc959d76077dd27ed5fc05c1cac29724dddd80655779663d5c2d8bd246130b`.
Root read the terminal result and rehashed the source and loaded artifact.
The r1 run stopped at descriptor validation because the environment pointed to
the bundle's inner `.so`; r2 passes the `.vst3` bundle directory. No production
format validation was changed. Focused strict lint passes after replacing the
test helper parity expression with `.is_multiple_of(2)`; the green audio-test
source hash above predates that test-only style edit.

Those temporary logs were no longer present after continuation, so Luna ran
both gates again and wrote directly into the durable packet
`artifacts/aud135-isolated-vst3-worker-r2/`. The current-source worker run passes
1/1 and strict focused lint passes, with log hashes respectively
`70c827a2cafd867111a6ae739d457c5955b7b0824f2cd3d23684fa325d848d76` and
`fa48f3277ce3578902c83e9b614cc79f72910ad126add89f7f02c27f2324c12e`.
Both used test-source SHA-256
`6617421b150507482cb702cebec24c9e17c45831c71e7181c75df5316ace723b`;
backend and native artifact hashes remain unchanged. Root verified the complete
packet checksum index
`0c168b1fdb0483da3d7e4b1daf5a5c5561dc47ed1c27656c65295187d89aae4c`
and inspected both terminal logs. The historical test source is retained with
its separate provenance; these new commands supply durable current-source
evidence. This is a selected-source packet, not a full transitive build snapshot.
Actual engine candidate/commit continuity, effective widths, engine EOF, isolated
CLAP and mounted UI remain open; these results do not close all of AUD135.
Astra medium independently accepted the archived cache refresh, in-process
late refusal/history, preflight no-send policy and isolated-worker construction
checkpoint in `reviews/AUD135-astra.md`. Its review explicitly distinguishes
the worker fixture's later fresh successful construction from replacing the
existing live worker through the engine. The latter remains Luna's next gate.

AUD144 now has an executed red C-ABI checkpoint. Offline locked `plugins-ffi`
cdylib build succeeds with all 339 selected before/after source hashes equal.
The library SHA-256 is
`8954f6abe82bd6685b8f4d3142b06dbb2e1f89f7cdb615b3692497d40587c529`.
The unchanged external ctypes probe confirms nine invalid envelope variants
return success and reset populated audio history; saved parameters alone remain
unchanged. Two valid-import controls pass. Exact documents and 1,272-sample
live/twin/cold continuation vectors are preserved. The 55-file packet index at
`artifacts/aud144-preset-envelope-r1/SHA256SUMS` hashes to
`0c19add5a469396d731a9230d2e135ea9dc239bfca46f07336356f48105bf83e`.
The r2 correction passes focused tests 2/2, full FFI library 81/1 ignored and
strict all-target lint. The rebuilt library has 340 matching selected source
hashes. Root's unchanged public probe now rejects all nine invalid envelopes
with -8 and exact saved state/full continuation; both valid controls pass. Root
verified the complete r2 packet checksum index
`8adc1cc21d9cb9434e700bf75de4f7aab9d2138e0435458afad6e4702f6f5e94`.

Root then confirmed a legacy compatibility regression: an actual exported
FletcherMunson preset imports into LoudnessCompensation with exact full state
and 9,464-sample audio on the pre-validator library, but r2 rejects it as a family
mismatch. The migration probe exits 0 before the validator and 1 with r2;
default-audio sensitivity is 0.27916882932186127. Its preserved packet is
`artifacts/aud144-fletcher-munson-compat-r1/`, index SHA-256
`8412ef3bdbf97e9739bdc5ec4fb9d22fa8010ef872ffa5b48ed73e77651b42f4`.
Luna owns the bounded r3 legacy-migration correction and both unchanged public
probe reruns before review. Prior packets remain immutable. This import-boundary
issue remains separate from accepted AUD139 DSP and its previous FFI state fixes.

The r3 legacy-migration correction now passes: focused tests 3/3, full FFI
library 82 passed/one manual utility ignored, strict all-target lint and cdylib
build. All 340 selected start/end source hashes match. The immutable library
SHA-256 is
`7311b8fa7044084096bbdb219f56656673d7feef86bd8c58943a4147c36e71a8`.
Both unchanged root-authored public probes exit 0: invalid refusals preserve
state and full continuation; genuine legacy migration preserves full state and
matches all 9,464 fresh output samples. Root independently checked identities,
manifests and complete saved vectors; receipt
`artifacts/aud144-preset-envelope-r3/root-probe-verification.json` hashes to
`da61a5a1092cec59079686ddd8c114f9921ea204067a3f2590695945f1190f6a`.
The fix selects the forward legacy migration; reverse import is not claimed as
an already failing historical behavior. Astra medium accepted the sealed r3
checkpoint in `reviews/AUD144-astra.md`, with no production corrections. Luna
resumes the accepted Crossover FFI metadata/state and actual engine-route steps.

Post-rustfmt focused/full/lint/build reruns also pass; their `*-r2` records are
retained separately. The rebuilt library is byte-identical, so the same public
probe runs apply. The broad final selected manifest changed only on Band's
uncompiled host integration test `native_vst3_host.rs`. Excluding that named
file yields 339 matching final selected entries, manifest SHA-256
`3bbd068c7489de6725dd1c7f6b510f1c0189e8c6217921c863633aa5f27c614a`.
Root verified the exclusion and final helper source copy. Both broad manifests
and both source revisions remain available; the first r3 build's 340-file
equality is not relabeled as equality of the later broad manifest.
Root verified the sealed r3 packet; Astra independently verified all 76 index
entries, correcting the earlier count of 75. Its final checksum index
SHA-256 is
`1087e2b392de89ef9192fc3c57e6daf66941d0b12c570a7e11687a0986cba3bc`;
the execution receipt hashes to
`ad5d1a003a330a62773d603385601aca8c4ceb43734252df7d96ca79741622e6`.

Current Dynamic EQ FFI follow-up: both public state regressions now pass.
The focused state r4 command passes 6 tests with one ignored; its selected
manifest is incomplete because two requested paths were invalid, so it is
not a complete source receipt. The later full FFI library r2 command passes
78 tests with one manual utility ignored. Its log,
`/tmp/sotf-aud139-ffi-lib-r2.log`, hashes to
`157c020986a58e5ce6fba94615483abe0d72105825e0ec7cd073b1c7dff2c3f8`.
The nine-file absolute start/end manifests match, each hashing to
`80395379a1c05c2f8a1ae46d8b3139c26ce1900c2df9f12fdb97832137633cd6`;
these cover selected source files, not all transitive dependencies. Strict
lint r3 stopped on two Crossover dependency casts, since corrected. The scoped
FFI r4 `--no-deps --all-targets -- -D warnings` run passes, with log SHA-256
`f80dacc351ef255658c87bc5cab8e19e84bc4b6b53762856fbb7175b75362136`
and matching selected start/end manifest SHA-256
`ffdc01c3e8db91acd1379a2ea4c8b37912f04dad94f805205be5e37abb3c4bc5`.
This does not establish dependency or workspace lint acceptance. Astra has not reviewed
this FFI checkpoint. Raw state loading remains a partial merge; only full
legacy preset import supplies Peak/default-slope values for omitted active
band controls. The corrected engine composition RMS regression and actual
4,099-frame stereo offline WAV comparison at 48 kHz over 127/257/1024-frame
blocks now pass 2/2. Complete output matches a separately configured accepted
core within 2e-6 maximum and 2e-7 RMS sample error; block-partition RMS is at
most 2e-7. The default program-length render is checked; this does not establish
a recursive tail policy. The uniquely preserved log is
`/tmp/sotf-aud139-engine-offline-focused-r1.log`, SHA-256
`c7bef549dd45c241102e00cff04a12d0364fd402899cbf1ac965e4c065bf9de7`.
The owner accidentally reused `/tmp/sotf-aud139-engine-route-r2.log`; its old
one-test contents were overwritten and must not be cited as retained evidence.
Scoped strict engine all-target Clippy passes with `--no-deps`; its log hashes
to `b486152873f1e83a5c03e5ccc8349bca71b04d1b19e8a80030e938afa5b5a1a7`.
Its selected start/end manifests match
`3121c5ac283ab3ec99448cf275a9d867bcc960f07c82f06cf7f1542b6e63df24`.
Only lint has verified matching start/end manifests: the two-test run did not
capture a new pre-run manifest, and old `engine-route-r2` manifests describe
earlier source. The test log and post-test selected source are preserved with
this limitation in `artifacts/aud139-engine-offline-r1/`; its six-receipt index
hash is `a0b12f7f10fa8783963fa42cafd5a2dd4e8bccdad0dbb9c10592977856aebb5f`.
Root archived the FFI logs,
original manifests and scoped receipt in `artifacts/aud139-ffi-r1/` and
verified all ten receipt checksums. The index SHA-256 is
`b416e8e4483821af667040e2bc1610602dcc1ba1c50f906accbfef3e737b56ab`.
Thirteen selected source files are preserved under
`crates/sotf-plugins/target/audit-artifacts/aud139-ffi-r1/selected-source/`.

Current-header generation is separately preserved in
`artifacts/aud139-header-generation-r1/`; its index SHA-256 is
`d69ca36fbb9eb159d0fd8af893c8d8e2eb29f7ee4dcbe249426ed6b6a49aeaaa`.
The owner directly executed an existing FFI build-script binary against current
source symlinks and a temporary minimal manifest. Both generated copies match
the tracked C/SwiftPackage headers, SHA-256
`fe3e2512f366a2dc5aa742fe5474fe0e295669d07ed72b108a71c485ccc0d843`.
This was not a fresh real-package build; no separate generator stdout log was
retained. Root verified generated bytes and recorded source/binary hashes.
AU sibling delivery and macOS runtime remain unverified. Native lifecycle
inspection also found that the current structural mismatch path silences audio
without requesting restart, and SOTF's consuming CLAP/VST3 backends do not yet
service the required restart lifecycle. The next narrow Dynamic EQ extension
must preserve the prepared processor until successful reinitialization; see
`dynamic-eq-ui-route-findings.md` for the source findings and scope.

BandSplit's fresh loaded r6 suite passes all five tests. Its log,
`/tmp/sotf-aud143-loaded-band-route-r6.log`, hashes to
`bcdeeb9189f518b9378161164c74dd157fed8069e832ea895f1c46c310ab2aee`.
The loaded matrix now checks complete public-DSP and partitioned DawHost
vectors, old packed VST3 state, native restore refusal after successful outer
validation, populated live/twin continuation with cold-state sensitivity,
and successful retry. Deliberately invalid native state emits rejection
diagnostics; the former inactive-buffer length warnings are absent. A direct
buffer reuse unit test cannot run from the root workspace because vendored
NIH is a separate workspace; a disposable copy also fails offline resolution
on uncached `baseview`. No unit test executed in those two attempts. The runnable
native target now tests actual absent/present/absent auxiliary Buffer reuse and
passes 3/3, including the two unchanged strict prefix regressions. BandSplit
mask checks pass 5/5. The full NIH `--features band-split --lib --tests` command
passes 111 unit plus 3 integration tests, with one manual utility ignored;
strict all-target lint passes without `--no-deps`. The frozen source/log packet
is `artifacts/aud143-native-buffer-reuse-r1/`, index SHA-256
`33c0b4ec74c26ea9a71e2aa3bd5ad838dc33ebecdcb4c579585062f8839eb331`.
Root verified every packet checksum. This is executed evidence awaiting Astra
review, not an independent acceptance or full-workspace gate.

Crossover's numerical r3 suite passes all seven tests. Its log,
`/tmp/sotf-aud142-iir-families-r3.log`, hashes to
`3ff0048ca883ccb60a9dd8f129ec8c5ecdd51c1456837e77a8a1613df38d2590`.
Earlier attempts stopped on test type inference and then an invalid refusal
assertion: 3 kHz is admissible at 8 kHz. The final stimulus uses 4.8 kHz;
per-channel low/high output also matches separate single-channel processors
at distinct cutoffs. This supplements independent analog response references.
Luna subsequently added settled input-RMS residual assertions and numerical
reset-to-fresh comparisons beside the guarded repeated-reset callback checks.
The first package attempts exposed stale unsupported-family and realtime-mode
test expectations. Mode is structural in the accepted route design because
Both changes width; stored baseline bytes remain intact, and the comparator
asserts that one intended metadata change explicitly. Full package r4 now
passes 113 tests with two manual capture utilities ignored, including both
stored compatibility replays. Its log SHA-256 is
`c8e117711e807bb5ba1abba8a27764592a464e7de083a106742d0546a58d4961`.
Final numerical r4 passes 7/7, log SHA-256
`c21a4ab0a0c79661b4c8b2a9d77e2e7b4cbb413320a9c95f844f83dec0d7d90b`;
strict all-target lint r2 passes, log SHA-256
`f328e62f5b3b381acde7dd50f2e4c3bedfe837d824aae453f90a185923815cb4`.
All eight release timing commands completed successfully and the no-build
window is released. Root verified the same 31 legacy IDs in archived/current
results, plus 16 new-family setup and 48 processing cases, each with 30 positive
finite observations. Recalculated medians match Criterion estimates; paired
legacy ratios span 0.931940–1.032559. The largest increase is about 3.26% for
stereo two-band LR24 Both at 32 frames. The maximum raw CV among new cases is
8.51%. Root arithmetic evidence is `artifacts/aud142-cpu-root-check-r1/`,
receipt SHA-256 `e451e8eab91e57984ea77f08e7d235f3289086cd4106488e3a6e1862b05027d1`.
This rechecks the same local run, not an independent timing experiment. The
recoverable post-core source archive contains 42 files (27 Crossover package
files plus selected route/workspace inputs), SHA-256
`9f5526c535730b8087973f007873689d01331d111b2b1e52392b8d900d4add8d`.
Root verified every archive member against `core-source-r1.sha256` without
extracting over the worktree, and checked the complete packet index
`42c82189128d8f4eafcfbe0da210f3bef7ab41ad59643454535f4c744877f55a`.
The complete owner report is now at
`artifacts/aud142-post-core/pure-core-results-r1.md`, including matched binary
identity, all result tables and the load/index-rebuild qualification. Root read
the report and checked it against the arithmetic receipt. Shared-machine load
and raw variation qualify all conclusions; no WCET or Astra acceptance is claimed.
The entries below preserve earlier checkpoints in their original scope.

- **Dynamic EQ:** after the transactional reinitialization fix and widened
  public coupled dynamics fixture, the library passes 48/48, the six-vector
  legacy Peak replay passes 1/1, and strict all-target Clippy passes.
  Root inspected `/tmp/sotf-aud139-core-after-reinit-lib-r2.log`,
  `peak-replay-r2.log` and `clippy-r2.log` with the same prefix. Their SHA-256
  values are respectively
  `799595a424ed4f485a26c502aa884818eb4309e085de57ef4026273fe1b8837a`,
  `ff9901bf1c2df3ac02dc6a71768106773d33afd3d9d4501de72fed2019667cc7`
  and `713f30736e2795943a29dad8eac021d326a45a7afbf82690d3e843fd5ab6b19a`.
  The later held-proportion recurrence and public zero-gain boundary additions
  pass the selected shelf suite 9/9 in
  `/tmp/sotf-aud139-held-boundary-r1.log`, SHA-256
  `9a141e22fc64bc00274962a9461e78ad86146fba26a4731b8fef0f008f98d074`.
  These earlier results predate the final mixed-band, lifecycle/heap and
  accepted Astra core review recorded below.
- **Exported BandSplit:** a freshly built single-feature Linux library,
  SHA-256 `14e199dbc8857a37a478f4077972f4ba0b4c857628b65b83e66f0c2c8f3b7c49`,
  was copied into CLAP/VST3 package paths and loaded through `ExternalPlugin`.
  The default CLAP 2→4 load passes; VST3 loading fails because the host rejects
  the plugin's four output buses. Root inspected the actual 1-pass/1-failure
  result in `/tmp/sotf-aud143-band-native-red-r3.log`, SHA-256
  `6ce31017bb7f47ee6e3426bca3bde7a248214edaae14cb3b07a6a81fa423b9a6`.
  Earlier probe failures were missing-import/package-path fixture errors.
  Luna is implementing the bounded host setup/bus delivery extension. This
  loading probe does not yet establish full processing/state/reconfiguration.
- **BandSplit lifecycle:** the initial new public four-band LR24/LR48 reset,
  refusal and changed-rate retry fixture passes 2/2; scoped strict lint passes.
  Root inspected `/tmp/sotf-aud143/lifecycle/focused-test.log`, SHA-256
  `bddf40f45654c6b78ca5e81952c7c24056a042d63dce7514be31fda4b97d2e94`.
  The corrected final fixture now passes 1/1 with repeated-reset
  allocation/deallocation guards and populated `initialize(8_000)` cutoff
  refusal, alongside the earlier zero-rate/crossing refusal and valid retry.
  Strict scoped lint also passes. Final test source SHA-256 is
  `89fb0692e6fcc58c3fc8267da503d3a56a3fbea7af1a2d8859d0994a59ab5499`;
  final test/lint log hashes are
  `9dcbb7837755047ab2c25279da729451f11d6d91708b7fddc19f9f0fbc1f4397`
  and `e797d977e9b785e7cd92946a6848cdd4d3152802fea75f572354bc13e3b87008`.
  Root verified every checksum in `artifacts/aud143-lifecycle/SHA256SUMS`.
  That directory preserves exact commands and both initial/final logs.
  Production BandSplit files were unchanged. The CPU report now uses the
  accepted measured-audio-duration percentages; no timing rerun is needed.
  Astra medium accepted the bounded DSP core after reviewing this final
  lifecycle condition and corrected CPU report. Packaged native delivery
  remains separate open work.

For AUD142, root preserved the current Crossover and shared `math-iir-fir`
package files plus selected workspace manifests/locks before new-family edits:
`crates/sotf-plugins/target/audit-artifacts/aud142-preimplementation-selected-source-r1/`.
The archive contains 105 files; all source hashes were unchanged when rechecked
after capture. Archive SHA-256 is
`4970c3ea910a2dd38e5bd72ccaac5aeb7043dcbeca3c45c8831fd51550ee9b1d`,
and the selected manifest hashes to
`90bd75c0ec2964d7c4be42df792b0391082c98161b5280354bc25190dd0a4d66`.
This is recoverable pre-edit source, not an executed audio/CPU/metadata baseline
or full transitive dependency snapshot. Luna must still capture durable full
LR24/FIR/per-channel waveforms, current native/FFI controls and matched optimized
CPU before changing the relevant behavior.

During the native host setup extension, the next Dynamic EQ mixed-band test
did not execute: dependency compilation failed on a missing BandSplit layout
readback method and an Ambisonics-only enum pattern in `sotf-host`. The
terminal log is `/tmp/sotf-aud139-mixed-public-r2.log`. Luna BandSplit owns
the immediate compile correction; dependent workers continue test/baseline
authoring and will rerun after the source reaches a coherent checkpoint.
This compile failure is not a Dynamic EQ numerical test failure.
Root subsequently ran `cargo check --offline --locked -p sotf-host`: exit 0
in 3.01 seconds, with all 257 selected host Rust/configuration and workspace
manifest/lock hashes unchanged. Log `/tmp/sotf-aud143-host-default-check-r1.log`
has SHA-256 `e765e5bed995ece3e6e486a7b554d8351858e88da1333ca002825c156c70bf3a`;
the sibling `-receipt.json` records both manifests and the exact command.
The Cargo flock is released and the Dynamic EQ owner is resuming focused
tests. This is default-feature host compile evidence only; BandSplit native
reconfiguration is still being implemented.

Dynamic EQ has now completed its closing core batch. The package passes
**71 tests**: 52 library, one Peak fixture replay, two lifecycle and
16 integration tests; the CPU and Peak capture utilities are both ignored.
The explicit six-vector Peak replay also passes 1/1. Root inspected
`/tmp/sotf-aud139-core-final-r3-package.log`, SHA-256
`83dc57d752127e52648aa3146b2ebc1f12b2950b9aed4700e89c39e3f021fae7`,
and matching r3 start/end manifests
`a329126d19beb565fa064584aa03b880e7a4d6a68d311f0c4207881825ec41a6`.
Strict lint initially found one range-loop warning in the new recurrence
test. After its iterator correction, the changed recurrence test passes
1/1 and all-target warning-denied lint passes in
`/tmp/sotf-aud139-core-final-r4-held.log` and `clippy.log` with the same
prefix, respectively SHA-256
`893e21a53dd0216c631f336673f415eb26cf96096e26146e845573acd899ef3b`
and `86f75e4e20231d9cd9027035c86d0201d43842defdcd468af329e01c355daad6`.
The r4 selected start/end manifests match
`c66dd2f6df7e65d65bd1e8400a3f06ab586f7d7297da5f3cca857f8e4025edd9`.
Root compared r3-end and r4-start: only `src/lib/tests/shelves.rs`
changed among the selected package/host/configuration files. The full
71-test result predates that test-only correction; no production change
followed it in this checkpoint. This closes the execution packet for
Astra core review, not the remaining application/state/native/CPU routes.
The final packet is preserved at
`crates/sotf-plugins/target/audit-artifacts/aud139-core-final-r4/`; its
`evidence.sha256` hashes to
`b91f79845a0d01ee919372f41f64cbe3dd42cd70899668ab47096e955a4b2f6d`.
Root verified all nine listed artifact checksums. Astra medium accepted the
bounded DSP core and closed all four findings in `reviews/AUD139-astra.md`.
The acceptance covers transactional reinitialization, coupled/held shelf
references, inspected mixed-band dry-detector behavior and populated lifecycle
heap checks; it does not establish a full independent mixed-chain oracle or
the remaining FFI/engine/native/UI/CPU routes. Luna has resumed the FFI/state
and actual engine processing stage, preserving the old 64 C ABI addresses and
appending the 16 shape/slope controls.

BandSplit's feature-enabled host check is now also terminal green with
`external-plugin-clap,external-plugin-vst3`. The owner reports log
`/tmp/sotf-aud143-host-native-check-r2.log`, SHA-256
`2bc0fbe6e8c1bffa4438fb96c8adbceb7a5203c4cb8fb877cafd241bdec6e671`.
This clears compilation after the VST3 ABI corrections; loaded native
processing/state/reconfiguration still require their end-to-end gates.
AUD142's new audio capture now passes 1/1 and its NIH metadata fixture passes
2/2. Corrected native log `/tmp/aud142-nih-native-baseline-r2.log` records the
existing frequency control as linear: normalized 0/.25/.5/.75/1 correspond
to 20/5015/10010/15005/20000 Hz. Its only published native ID is `frequency`;
runtime string `type` and `mode` controls are omitted. The FFI metadata
fixture also passes 1/1, capturing configuration-dependent runtime ordering
and the final FIR `fir_taps` field. Do not insert a static FFI table that
rebinds those old addresses. Owner is preserving these receipts and preparing
the remaining optimized CPU baseline before family implementation.

Root also identified current AU consumer gaps in
`dynamic-eq-ui-route-findings.md`: generic knobs pass absolute indices into a
mapping that adds the global offset again; structural writes still use the
scalar setter; native labels and generated C headers remain unverified.
These source findings are assigned to Luna's later native/UI stage and do not
invalidate the accepted shelf numerical core.

Root captured a fixed DAW plus patched math source copy in
`/tmp/sotf-audit-integration-20260930-r1`: 3,775 files with matching source and
copied content, aggregate start manifest
`cbd58b06e6ce86c6173cb4053b9660d4fd96ca7e5471d1bd15c29741b92605ff`.
The offline locked workspace nextest command, excluding MIDI/IAMF, completed
with **6,250 passed, 3 failed, 37 skipped across 383 binaries** (6,253 tests
run, 283.172 seconds). Exit status is 100. Start/end manifests match; no copied
source changed. The two `plugins-nih::native_aux_output` integration failures
are CLAP/VST3 short-output-prefix behavior regressions assigned to Luna
BandSplit. They were outside the earlier full NIH library-only gate. The
third failure is `test_limiter_plugin_timing`: P99 4,086.320 microseconds
exceeded its 4,000-microsecond threshold under the concurrent run.

A single isolated limiter rerun using the same source copy passes, P99
60.925 microseconds. This suggests timing contention but does not turn the
failed broad gate green or establish a hard realtime guarantee. Log hashes:
workspace `c7071eb46cca7fb72936090483bb7cfade556fe07fcbfabe8dac3caac81a232b`;
isolated limiter `8a12396d012d7300962f84f3abe8891c733fdac5af4531dd216620ef99cd2e5f`.
Exact commands, terminal logs, manifests and receipts are preserved in
`artifacts/workspace-integration-r1/`, checksum index
`d396e2fd2750d810ff51c2f7396cc2fc465e4902ee3eb315a914b3a16b2955e2`.
The complete copied source archive is
`crates/sotf-plugins/target/audit-artifacts/workspace-integration-r1/source.tar.gz`,
SHA-256 `cb0de3dbedeebe85bfc65905f262a9ecdb776d4f54fc085fd8db0e8d39aff489`.

The copy allowed live implementation to continue without changing tested
source. Later Dynamic EQ FFI changes and BandSplit corrections require their
own gates and reconciliation with this checkpoint. Optional sibling streaming
metadata resolves through a symlink; this gate does not enable streaming or
native artifact loading. The previous toolbar/render-plan failures do not
recur in this snapshot. Native device, sibling UI and explicit feature tests
remain separate requirements.

Luna's narrow native-prefix correction now passes the focused
`plugins-nih --test native_aux_output` target (2/2, owner session 28042,
terminal exit 0). Root inspected the fixture diff: only its explanatory
comment changed, preserving the status, complete-output, input-copy,
hidden-buffer canary and allocation assertions. Broader NIH library plus
integration tests, strict lint and Astra review remain pending. This result
does not make the earlier failed workspace run pass retroactively.

The later expanded fixture adds malformed-width error, bounded-silence and
surplus-channel canary cases; its r2 run passes 2/2. BandSplit mask/canary
tests pass 9/9, NIH library plus integration passes 111 + 2 with one ignored,
and strict all-target NIH lint passes. Root inspected all four logs and
archived the three changed files, their diff against the failed broad
snapshot, and the logs in `artifacts/aud143-native-prefix-root-r1/`.
Checksum index: `e0b5102a01f3793188607309e66bc3ea44cd712259bba1a41d820c53e1adbd3d`.
The archive is a selected review slice, not a full dependency manifest.
Luna ended this bounded task with the wrapper/test files frozen. Astra medium
review is queued: two resume attempts hit the agent thread limit despite the
owner reporting completed; it has not started this review yet. Resume the
reviewer after the completed slot is released, then return findings or the
remaining loaded-host work to Luna.

Dynamic EQ's FFI red run preserves the old 64-address replay while exposing
the missing 16 descriptors and structural state restoration. Its initial
direct scalar-setter test incorrectly expected a structural rebuild; that
expectation contradicted the accepted callback contract and was corrected
before implementation. The scalar path must refuse shape/slope mutations
without changing the running plugin; successful changes use prepared,
transactional state/preset restoration. Luna is adding dormant eight-band
serialization, metadata/labels and that reconstruction path.

The next focused FFI run (`/tmp/sotf-aud139-ffi-focused-r2.log`, SHA-256
`59adc1b8eae7d2cbfcb3ad7ca1474c487f1e3de0c63da2aab9fd14696396bcd2`)
passes three cases: legacy address replay, all 80 descriptors/labels, and
transactional valid/invalid state restoration with audio comparisons. Two
fixtures fail: the allocator positive-control allocation was optimized out,
and dormant slope comparison expected unquantized f64 rather than f32
storage. Luna is correcting these tests; direct helper-counter calls alone
must not substitute for proving that the FFI binary's global allocation path
is observed. One manual capture remains ignored. This is not FFI acceptance.

The corrected focused FFI r4 run passes all five selected tests, with the
manual capture ignored. Root inspected
`/tmp/sotf-aud139-ffi-focused-r4.log`, SHA-256
`69c8e51f524130b772ed3c2244192d581d10f272a200a1e01a874944e232a697`.
The selected start/end source manifests are identical, SHA-256
`06bdf81039d74e05650bc5f6188fe0cd6efef28716b844022b6e3bd1168362c3`.
The allocator positive control uses the actual global allocation/deallocation
path; the dormant-slope expectation reflects f32 storage. The r3 attempt did
not compile because the default target symlink was unusable; r4 explicitly
uses the warm workspace target. Strict lint, package reconciliation and
Astra review remain required before FFI acceptance.

After three iterator-style lint corrections, strict all-target FFI Clippy r2
passes. The full FFI library on that source passes 77 tests, with one manual
capture ignored. Root checked the terminal logs and identical start/end
selected manifests for each run. Logs and SHA-256 values:

- `/tmp/sotf-aud139-ffi-clippy-r2.log`:
  `283afdcf74af19c7a616c6e19b9250d862dd50cf76656a20ee4b38009cfbc5e7`.
- `/tmp/sotf-aud139-ffi-lib-r1.log`:
  `30645556dea166c6232059b1c7c0e3cc4b125a6e42b493e40d98b2cab028fb47`.

This closes the package/lint reconciliation for the selected FFI checkpoint;
Astra review and engine/native/UI integration remain open.

Root's subsequent source check identified additional AUD139 handoff work.
The first passing engine fixture exercises converter → factory → direct
plugin processing, rather than the consuming engine chain, and computes
`sum(diff²) / sqrt(N)` while labeling it RMS. Luna is correcting the metric
to `sqrt(sum(diff²) / N)` and adding a complete-waveform engine/offline route
against a separately configured accepted core instance. The original passing
result must not be used as numerical or complete engine-route acceptance.
FFI follow-up probes are also pending for noncanonical band-index keys that
could validate but be ignored, and captured legacy preset restoration into a
populated shelf handle. Preserve partial-state merge semantics while proving
the accepted legacy Peak/default-slope behavior. These are source findings
awaiting public regression results, not executed failures or Astra findings.

The later FFI state-red r2 executes both regressions: four cases pass, two
fail and one manual capture is ignored. `band_00_shape` is accepted instead
of returning InvalidConfig; importing the captured legacy preset into a
populated shelf retains shape 1 rather than Peak 0. Root inspected
`/tmp/sotf-aud139-state-red-r2.log`, SHA-256
`cb5552a4853b84254b0745d98f185553089a800581e23b2d6242a37eb0f67589`.
The earlier attempt stopped at a concurrently incomplete Crossover helper
and was not a behavioral result. Luna is correcting the reproduced FFI
defects before the next acceptance packet.

BandSplit's fresh single-feature artifact now passes all three actual loaded
CLAP/VST3 tests for two/three/four bands, both slopes and both modes, including
state reload and distinct stereo/per-band output. The library SHA-256 is
`e71a7845eb29692553e60580e4fd1ffb42b2f232d805ba314213dd47b1a592c8`;
identical bytes are packaged at `.clap` and `.vst3` bundle paths. Root checked
`/tmp/sotf-aud143-loaded-band-route-r2.log`, SHA-256
`44c6720046f050151fe58616b898338c6f73feb1c22acdebcf7c30b9c45be0d4`.
Repeated inactive-buffer debug assertions still require investigation.
The prior bare-`.so` path rejection was a fixture error, not a backend result.
Complete DawHost/reference waveforms and populated late-candidate-failure
preservation remain open. This is not route acceptance or Astra review.

Crossover's pure library compile checkpoint passes in
`/tmp/sotf-aud142-pure-compile-r1.log`, SHA-256
`000d17b7454de32067d888da9b35b9d356c9bfcb6807c000a4c3bd649902bad2`.
The owner removed an unused helper afterward. No new-family numerical pass
is claimed from this compilation-only checkpoint.

Support-layer issue AUD144 records preset-envelope identity/version validation
separately in `ffi-preset-envelope-validation.md`. It is now reproduced through
the actual C ABI: nine invalid envelopes return success and replace populated
DSP, while unmodified and name-edited controls pass. The immutable cdylib,
339 matching selected build hashes, exact probe/documents and complete audio
are preserved in `artifacts/aud144-preset-envelope-r1/`. Luna crossover owns
the bounded correction before the next AUD142 FFI/engine route stage.

AUD142's optimized pre-edit CPU baseline is now complete: 31 cases, each with
30 paired iteration/time samples and saved estimates. Root verified all
checksums in `artifacts/aud142-preedit/SHA256SUMS` (index
`a4def69fa50e85105f58fbce1374bd73481ceeee75338fc27b4958c80bc26a08`)
and all 31 sample/estimate pairs and array lengths. The report
`crossover-aud142-pre-edit-baselines.md` records variability limits; earlier
r1 smoke and r2 failed-output attempts are explicitly excluded from timing
claims. The r3 receipt SHA-256 is
`d9550864eb7bc1d05ec74fe49ddd0420663c0aabaeb2af8ec89ea9ab2dc765af`.
Luna is authorized to proceed with the accepted pure Crossover DSP/parameter
stage; no additional source hold or user approval is pending.

The next combined workspace gate must cover these later implementations and
the already corrected toolbar/render-plan failures. Use the shared Cargo lock,
`CARGO_NET_OFFLINE=true`, the explicit workspace target directory and
`SOTF_FFI_SKIP_HEADER_SYNC=1`; run offline locked workspace nextest excluding
only the user-excluded `sotf-midi` and `sotf-iamf` packages, then the applicable
strict lint and explicit native feature/artifact gates. Preserve source/lock
manifests and exact terminal commands/results. The snapshot gate above exposes
two native compatibility failures to correct; a later combined gate must
include those fixes and subsequent live edits. Default-feature workspace tests do not substitute for
the loaded-plugin, sibling mounted UI or platform/device tests.

The native legacy policy is resolved: a complete old NIH BandSplit preset
without `num_bands` restores two bands and LegacyCascade, including import
into a populated three/four-band instance. Incompatible negotiated geometry
must be renegotiated or refused through the supported transactional host
route. FFI partial-state merge remains a separate contract. The new actual
VST3 Component.set_state regression reproduced the stale count on an
initialized three/four-band instance: terminal 0 passed/1 failed in
`/tmp/sotf-aud143-nih-old-state-red-r1.log`. Root inspected the assertion for
`num_bands == 0` (the two-band choice index). Luna's narrow migration fix and
the rejection/retry lifecycle checks followed that reproduced failure.
The latest fixture uses exactly the two historical NIH controls verified
against Git HEAD. It now processes and checks actual wide-layout audio before
import. Direct incompatible NIH import returns failure after migrating its
controls, and processing then refuses/silences until compatible renegotiation
and retry; it does not establish atomic live-instance preservation. That
preservation remains a separate detached-candidate consuming-host gate.
The corrected callback module now passes 5/5, including exact historical
preset migration, actual wide-layout audio, incompatible import/refusal,
successful packed two-band retry and comparison with the audio reference.
The separate sparse case accepts absent trailing inactive buses, while the
new case checks silence for provided inactive samples. Root inspected
`/tmp/sotf-aud143-nih-old-state-module-r1.log`, SHA-256
`9dca5638ffa650f57fe22b03f647f83794cb08f572766b39c4af15a75ca00c51`.
Updated full NIH/lint and Astra migration review remain pending.

The subsequent full NIH gate is red: **107 passed, three failed, one ignored**
in `/tmp/sotf-aud143-nih-full-lib-migration-r1.log`, SHA-256
`f466a09749ec99ccdfff9f70152a68b125d4e3539f5fb3dde4622854a4ba79cf`.
Root inspected the terminal failures. Two existing native default/structural
reconstruction tests fail because Dynamic EQ emits an invalid configuration;
Luna Upmixer resumed to fix this concrete integration regression. The third
test still asserts that migrated old BandSplit state lacks `num_bands`, which
conflicts with the new explicit legacy-default migration; Luna BandSplit is
checking the test contract and correcting it while preserving the geometry
refusal checks. Strict NIH lint also found two test-only range-loop warnings.
These failures must be resolved before the updated migration checkpoint is
accepted; the earlier focused 5/5 does not override this full-suite failure.
The unused VST3 activation-selects-configuration hook was removed before this
full gate; actual bus-mask tracking and routing remain in place.

Astra medium core review is now active. Earlier attempts hit the agent
thread limit; the reviewer resumed after Luna Upmixer completed the narrow
Dynamic EQ shape conversion and test corrections. The conversion maps native
choice integers to canonical shape strings before DSP construction. In the
subsequent full NIH run all three original failures pass (110 passed, one
failed, one ignored); the remaining new regression-test failure compared an
f32-backed slope with an unrounded f64 literal. Its expectation is corrected
to the explicit storage representation, without changing DSP tolerances.
BandSplit owns the final focused/full NIH rerun after the CPU timing window.
No user answer or permission is needed.

The corrected native closing gate is now green. The fully qualified Dynamic
EQ shape test selects and passes 1/1; full NIH library passes **111 tests,
zero failed, one ignored**, and strict all-target Clippy passes. Root inspected
`/tmp/sotf-aud143-nih-full-lib-migration-r5.log` (SHA-256
`de3d5b40e4a94eb3fa6f1608e40b8e6c7b755bc73ee66c9205ebd3cd71d90e6a`)
and `/tmp/sotf-aud143-nih-clippy-migration-r5.log` (SHA-256
`1513748dc13e3efebf8fb09e71bd7cf7bfe5e03803540261eb7335435ea064ca`).
The focused log is `/tmp/sotf-aud143-nih-dynamiceq-shape-r4.log`, SHA-256
`e1e6eef38c7f70d7160063eda7475ffbc668c0f26fb6a210728634eb3a1c492e`.
Selected start/test/lint manifests match
`277aa180d203d7d6697f3900032a0bb7d1b6fc65e8c57e75798635d29bd62448`.
The final test compares parsed JSON at the actual f32 storage precision;
DSP tolerances were unchanged. This closes the three native integration
failures and the new test's representation issues, while Astra's independent
shelf reinitialization defect and remaining core evidence stay open.
Luna Upmixer is released to work on those core fixes; no packaged native or
complete shelf route acceptance follows from this suite.

Root inspected terminal logs for the populated process/reset gate (1/1),
portable split replay (1/1), actual DawHost archive replay (1/1), and both
targeted strict lint gates. Their logs are respectively
`/tmp/sotf-aud143-populated-process-reset-r1.log`,
`/tmp/sotf-aud143-portable-split-replay-r1.log`,
`/tmp/sotf-aud143-portable-host-replay-r1.log`, and
`/tmp/sotf-aud143-portable-{split,host}-clippy-r1.log`.
The replay uses Rust `flate2::read::GzDecoder`; no system gzip executable is
required. Complete split and host archives retain hashes
`73b30f81f14464e87fc6982ddcd5c2e3c618ca41486a95ec9b840a3debc16ebc`
and `e9efc5c52975d07357bde1e36dae67a6ac471b42cd6f8d0508543799783923bc`.
The two added development dependency edges change the root lock hash to
`db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`.

The numerical gates retain their declared scopes: fixed settled waveform
uses input-normalized RMS <= 0.002; the <= 2e-5 RMS/maximum bounds apply to
partition and timed-automation comparisons. The LR24 fixed-waveform maximum
absolute residual 2.24925221e-5 is supplementary and does not violate the
fixed RMS gate. No tolerance was changed to obtain a pass.

Root recovered all 68 selected pre-edit BandSplit/math-iir-fir source files
byte-exact from current bytes or Git, archived at
`crates/sotf-plugins/target/audit-artifacts/aud143-preedit-dsp-recovered/`.
Archive SHA-256 is
`b499d74680b81b39522e16035f9ff26dc835f28095f8d6f21f8ce04d177bb4c9`.
The original optimized-development CPU log exists, but lacks a run-bound
manifest. A reconstructed controlled comparison is now feasible; it must
hold shared dependencies/harness/profile constant and preserve that limitation.
See `band-split-performance-evidence.md`. The reconstructed comparison is now
executed: both snapshots pass full-vector fixture replay, and six direct
release runs provide 1,890 raw measurements (105 groups of 18 samples).
Current legacy processing medians are 0.982–1.028 times recovered legacy;
phase compensation is 1.048–2.240 times current legacy. Setup is separately
reported with its large two-band variance and no separate setup warmup.
The report and durable artifacts are `aud143-controlled-cpu-results.md` and
`artifacts/aud143-cpu-timing/`. Astra medium accepted the CPU data with a docs-only correction to audio-batch
shares: 2.5533% recovered legacy, 2.5306% current legacy and 5.5957% phase.
No new timing is needed. Bounded DSP topology/accuracy looks sound, but a
populated four-band compensated reset/reinitialize/refusal/retry fixture
against complete fresh/twin audio remains. The prior fresh-reset comparisons
were two-band; four-band heap tests alone did not prove resumed audio.
Luna crossover design owns that remaining fixture and report correction;
complete native/application integration remains separate.

### Dynamic EQ pre-edit compatibility checkpoint, 2026-09-30

The design is `proposals/dynamic-eq-shelves.md`. Six Peak compatibility cases
preserve complete 48 kHz stereo input/output arrays (8,192 frames each) and
serialized parameters: linked/unlinked boost/cut, inactive and solo. Root
checked all saved output lengths and finite samples; inactive output matches
input exactly, and all five active captures differ from input. These are
compatibility baselines, not independent shelf accuracy evidence.
Audio/state capture: `/tmp/sotf-aud139-peak-baseline-r2.log`, SHA-256
`4fd352364f67f01235e14b27f92a51ff936f56ece138491410c52f952a8966f2`,
terminal 1/1. The separate public C ABI table/state/preset capture passes 1/1
in `/tmp/sotf-aud139-ffi-baseline-r2.log`, SHA-256
`c019849acda10958a208e2bf3f5b3343824a11573f5b366f72cf515bf1098f5b`.
Artifacts are under
`crates/sotf-plugins/target/audit-baselines/aud139-pre-edit/{audio,cabi,source}`.
The C ABI table hash is
`eab19a9cce787bdcf68ee42820ffd03b0b8436587a9914e1e207bfb948ce8701`.
These C calls run inside the Rust test binary; native AU execution is separate.
Astra accepted the bounded design for implementation in `reviews/AUD139-astra.md`;
no shelf implementation is accepted yet.
Proposal SHA-256 is
`a8d1973f757d689a8c71e3697e44606ff87baaee0cfcf9e9d2ea5a0d4ff85531`.
Luna finished this handoff. Initial attempts to resume/start Astra returned
`agent thread limit reached`. After the native owner also finished its test
checkpoint, the existing Astra medium reviewer resumed successfully for this
design review. That review accepted the design with seven concrete requirements:
serial audio/dry detection and truthful solo curves; keyboard reactivation of
inactive bands; an independent analog numerical reference; coupled public
detector/dynamics evidence; transactional state with exactly 64 old and 16 new
identities; actual engine/native/UI reconstruction; and durable compatibility
plus matched CPU evidence. Luna resumed under these requirements, starting
with the optimized pre-edit CPU reference before production changes.

That CPU reference completed before shelf production edits: release profile,
48 kHz, 2/8 channels, 4/8 bands, callback sizes 64/256/1024, 65,536 frames per
trial, two warmups and seven measured trials. Root inspected the terminal
passing log `/tmp/sotf-aud139-cpu-preedit.log`, SHA-256
`7b52d2aafd55ba38dc643a1daf36bf526bb46a4d33f2e8715c383c20d0385591`.
Median callback work spans 202.485–767.453 ns/frame across these cases;
there is no post-edit overhead or worst-case claim yet. The original script
looked for Cargo.lock relative to the plugin subdirectory, where it was
absent; Cargo itself used the root lock with `--locked`. Source/harness
start/end hashes match, and root-lock/compiler evidence was captured
separately. Preserve that provenance qualification. The actual run used
root lock `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`,
before the two BandSplit fixture dependency edges. Luna has copied audio,
state, C ABI and CPU evidence to tracked fixture paths. The ordinary replay
commands then both passed: full Peak vectors 1/1 in
`/tmp/sotf-aud139-peak-durable-replay.log`, SHA-256
`467eb39c1db7723822d4f35ede55151adcc4fb60f4a7abd0f9fca6d0c5ad4223`,
and the 64-entry C ABI baseline 1/1 in
`/tmp/sotf-aud139-ffi-durable-replay.log`, SHA-256
`fd2e0982e64c3af11c24abdd12bb30c9f7d3b0ee851ce858ebab2669aeb05dd6`.
Root inspected both terminal logs. These fixture paths are currently untracked
and must be included in the eventual commit for clean-checkout replay.

Root subsequently recovered all 12 source/harness entries from the original
CPU start/end manifests by exact recorded hashes: seven from the saved source,
two unchanged current files, two Git HEAD files and the durable original
benchmark helper. They are preserved with the separately verified original
root lock in
`crates/sotf-plugins/target/audit-artifacts/aud139-preedit-cpu-source-recovered-r1/`.
The 13-entry archive hashes to
`736c9b538e6511d0eb47a2296e8ccf73955445582980ad820b73d98049b0aa42`;
`receipt.json` records each recovery source and the original provenance limits.
Original logs, compiler output and matching source manifests are copied there.
This supports constructing a controlled old-source comparison, but is not a
full historical dependency/build snapshot or new timing result. The surviving
CPU executable is separately saved as `unverified-origin-cpu-executable`:
without an original run-bound binary hash, its filename does not prove that
it is the executable used for the recorded pre-edit measurements.

The first shelf core edit batch subsequently compiled with
`cargo test --offline --locked -p sotf-plugin-dynamic-eq --lib --no-run`.
This is compile-only: no tests were executed. Root inspected terminal exit 0
in `/tmp/sotf-aud139-core-build-r2.log`, SHA-256
`845729d45e062b87f6f50281fddb276560cb8a620176c08cc18187094cebaaf5`.
The owner records matching selected 11-file start/end manifests at
`555f58ffcfa48c2c8de015251b31b33797b31520a9d6988a14b8783c5af1bb73`.
Production shelf accuracy, post-edit legacy replay, strict lint, downstream
control/state routes and CPU comparison still require execution and review.
The coherent core dependency snapshot is temporarily available to the native
BandSplit regression before Dynamic EQ source work resumes.

The first numerical shelf tests then passed 3/3 (41 filtered) in
`/tmp/sotf-aud139-shelf-oracle-r2.log`, SHA-256
`1be83dbd3df8694df039888a565ff1be4ffdf3ef35f79108d0828ae92c0b3786`.
Root inspected the terminal log and source. The coefficient response is
compared with an independent prewarped analog prototype over three rates,
three corners, seven gains, four slopes and both shelves, including finite
complex response, endpoints/midpoint, poles and full-target monotonicity.
The partial-blend test establishes the specified mathematical examples from
the full-target coefficients; it does not yet call the production dynamic
blend or public processing path. The third test verifies old band JSON
defaults to Peak and shelf slope 1.0. Public waveform/detector/dynamics tests,
post-edit legacy replay and full package/lint gates remained required at
that first numerical checkpoint.

The expanded full library subsequently passed 46/46, with no ignored tests,
in `/tmp/sotf-aud139-dynamic-eq-lib-r1.log`, SHA-256
`d1c005fbb21a91e10579de1a26444663b13b8bb1a897355d9e8eb24f3e7fb412`.
Root inspected the terminal log. This includes the new independent
sample-level public-process reference for linked/unlinked shelf dynamics and
shape-specific detector pass/reject tests. Post-edit Peak replay and strict
lint are the next gates; full state/control routes and independent production
review remain open.

The next coherent gate passes the expanded library 47/47, shelf-focused suite
6/6 and post-edit complete six-case Peak replay 1/1. Root inspected the
terminal logs: `/tmp/sotf-aud139-dynamic-eq-lib-current-r1.log` (SHA-256
`a5e7b579899c08174f5f5355a8e2cbc609e4ce78742728f2590be8c7337ed4a2`),
`/tmp/sotf-aud139-shelf-current-r1.log` and
`/tmp/sotf-aud139-peak-replay-current-r2.log` (SHA-256
`ff9901bf1c2df3ac02dc6a71768106773d33afd3d9d4501de72fed2019667cc7`).
Strict all-target lint first failed on four test-source findings, with no
production source diagnostic. After the test-only fixes it passes in
`/tmp/sotf-aud139-dynamic-eq-clippy-current-r2.log`, SHA-256
`748a774f24943d463b349671ad236ed72f773db9fa9e226f0fc422647f2b232a`.
The core checkpoint is being prepared for Astra; complete consuming routes
and shelf production acceptance remain open.

### Dynamic EQ core review and next fixes, 2026-09-30

Astra medium completed the first core implementation review in
`reviews/AUD139-astra.md`: **revision required**. A valid 10 kHz shelf at
48 kHz currently reinitializes at 8 kHz by silently clamping its cutoff to
3.8 kHz. Luna Upmixer is assigned to reproduce this publicly and add
transactional shelf preparation/refusal that preserves populated history and
controls, followed by a valid retry; legacy Peak admission remains separate.

The public red reproduced the silent clamp on a populated two-band instance:
`/tmp/sotf-aud139-reinit-red-r1.log`, SHA-256
`1138ead876b39201c062d269e219c940bbea412825bbd6adb581a4738a1b5587`.
The focused correction now passes 1/1 in
`/tmp/sotf-aud139-reinit-green-r2.log`. Refusal preserves rate, selected
controls and exact subsequent audio versus a populated twin. A valid 44.1 kHz
retry matches fresh preparation after successful initialization also resets
retained recursive dynamics state. Root inspected both terminal logs. The
subsequent full library passes 48/48, complete six-case Peak replay passes
1/1, and strict all-target lint passes. Root inspected the terminal logs:
`/tmp/sotf-aud139-reinit-lib-r1.log` (SHA-256
`738a4329eab9f12c53ba753c715bc2f52a498153a8e863433069ef59c3cdcbef`),
`/tmp/sotf-aud139-peak-post-reinit-replay-r1.log` (SHA-256
`0951eb4648af9850ea24a9e1853eb0c058e92ee42a85daf4447b0c7480804a8f`),
and `/tmp/sotf-aud139-reinit-clippy-r1.log` (SHA-256
`62e8137c5e6bd212d0bcd047e15521e4f142e0dd0ccb8851d10f86476321eca6`).
Luna continues the remaining core evidence before Astra re-review.

The same review requires public coupled high/low shelf boost/cut, alternating
linked-channel dominance, overrides and mixed-band dry-detector isolation;
full held-blend recurrence at p=0/.25/.5/.75/1 with the original peak/RMS gates;
both signs around the .01 dB boundary; and populated shelf reset/dry-to-wet
and allocation/deallocation evidence. These complete the already accepted
core requirements. The narrow NIH shape serialization correction is
source-approved, with corrected executed native closure still required.
Luna finishes the native test's typed f32 expectation before core production
changes, allowing BandSplit's final native suite to use stable dependencies.

### NIH BandSplit callback checkpoint, 2026-09-30

The r4 callback run compiled but failed four cases: discovery and three audio
comparisons. Luna corrected the main-output-name accessor (it read the input
name), aligned reference initialization/reset with the processing lifecycle,
and made the reference band count explicit. The unchanged waveform tolerance
now passes all eight selected BandSplit tests in r5. Root inspected the
terminal result and log hash:
`/tmp/sotf-aud143-nih-vst3-bandsplit-r5.log`, SHA-256
`d1ab7fd99e54abe9da344145f6038d8dbbd51fb65e3e06a81f82a31939b19b3f`.
These tests cover wrapper/constructor/state behavior and actual VST3 callback
invocations inside the Rust test executable. They do not establish a freshly
packaged, dynamically loaded plugin. Full NIH library/strict lint, exported
artifact replay and Astra implementation review remain required.

The subsequent full NIH library run is not green: 101 passed, eight failed,
one ignored. Log `/tmp/sotf-aud143-nih-full-lib-r1.log`, SHA-256
`66f3c4446392f7d6bea01f2013c68756b6d38f1e43b2ee48c7e7a1a9ddd18498`.
Root verified that seven failures are `CLAP_PROCESS_ERROR` returns and one
is a VST3 `kResultFalse` return. These are not thread-identity assertions.
Luna traced the shared cause to the manually prepared `NativeProbe` and
`TailProbe` test fixtures: their generated inner wrappers retain zero channel
geometry while processing stereo buffers. Fixture initialization must copy
the actual negotiated layout into the new fields, retaining the transport,
tail and allocation assertions. Focused and full-suite reruns plus strict
lint remain pending; the earlier focused 8/8 does not override this red gate.

Luna then adapted both fixtures from their actual `AudioIOLayout` argument,
including main/auxiliary channel geometry and prepared buffer capacity. The
transport, tail and allocation assertions remain in place. Root inspected
the fixture-only diff and the terminal corrected logs: focused transport/tail
18/18 in `/tmp/sotf-aud143-nih-transport-fixture-r1.log` (SHA-256
`e007a8736ace4a43a9468975d3f42114e2ea477985a92d509a3944e932148938`),
then full NIH library **109 passed, zero failed, one ignored** in
`/tmp/sotf-aud143-nih-full-lib-fixtures-r2.log` (SHA-256
`a5ff2ea918f1108938d0d5b24ddc9d6c23f86a3fb561f82fd806cd737c4560d7`).
The first strict NIH all-target lint reported six unused exported-macro
helpers and one test range loop. Luna made the helpers
hidden-public for downstream macro expansion and corrected the loop. The
post-cleanup full library run again passes 109/0/1:
`/tmp/sotf-aud143-nih-full-lib-fixtures-r3.log`, SHA-256
`ee9a6fdd6284470f46f47176236e205bdb52ffb0b88255723f929748c1de068f`.
Strict all-target lint then passes in
`/tmp/sotf-aud143-nih-clippy-fixtures-r2.log`, SHA-256
`9903d8a53b2325f364e353a972efa7f09bc007ea5743b0daf03bcf0a9bc34180`.
Root inspected the terminal logs. Packaged loading, downstream macro expansion
evidence remain pending. Astra subsequently accepted the scoped native
framework/callback checkpoint in `reviews/AUD143-astra.md`; complete native
acceptance still requires the legacy migration, packaged ABI, real consuming
host, downstream macro and failed-restore preservation gates listed there.
The earlier red
log is retained as the fixture-contract regression evidence.

### Intermediate combined host/engine/DSP gate, 2026-09-30

Root ran offline locked nextest across `sotf-host`, `sotf-engine`,
`sotf-plugins`, `sotf-plugin-ab-compare`, `sotf-plugin-speech-denoiser`,
`sotf-plugin-hal-output`, `sotf-plugin-crossover` and `sotf-plugin-band-split`.
The terminal result is **2,632 passed, 2 failed, 26 skipped across 146
binaries**, with 307.248 seconds of test execution. The long full-history
loudness allocator test passed after 305.987 seconds; it was not restarted.

The two failures are with Luna BandSplit:

- `sotf-engine::toolbar_wire_forms::toolbar_choice_forms_survive_factory_create_for_all_plugins`:
  the factory rejects BandSplit recombination-mode indices 0/1, the two
  displayed mode labels, and the three displayed band-count labels. Correct
  the normalization route while preserving raw numeric constructor counts
  2/3/4 and the separate choice-index convention used by state restoration.
- `sotf-plugins::render_plan_snapshots::band_split::all_profiles`: the new
  mode/count/cutoff controls change the saved render plans. Review the actual
  layouts and geometry before updating their expected snapshots. This run
  used `INSTA_UPDATE=no`; it did not automatically bless changed output.

Log: `/tmp/sotf-aud143-combined-nextest-r1.log`, SHA-256
`c09cf72dd851be402e17aae19a08da4d3a8fe4495eb1563de326bc6ee85658ec`.
The 1,926-file selected DAW/math/streaming source/configuration manifests
match at start and end:
`b70807d7d9bcf7160af92e74377367e723c616c21be2fa7022758424a40f511e`.
Command, child exit 100 and scope are recorded in
`/tmp/sotf-aud143-combined-nextest-r1-receipt.json`.
This is an intermediate combined gate, not the final full-workspace gate;
NIH, FFI and Dynamic EQ test targets, mounted sibling UI and physical devices
were not included. The manifest is selected-source evidence, not a complete
transitive dependency closure. Both integration failures must be corrected
and rechecked before claiming the combined batch passes.

The factory normalization correction now passes its focused public-creation
regression and the existing toolbar integration gate. It accepts the emitted
mode indices/labels and band-count labels, preserves numeric constructor
counts 2/3/4 and retains rejection of invalid numeric counts 0/1/5. The toolbar
gate audits 56 plugin types and 54 choice fields: terminal 1/1 in
`/tmp/sotf-aud143-band-split-toolbar-wire-r1.log`, SHA-256
`cd259d76223f811fa362b1cfdc8534d4fbe2cec333734aa72d717ff8d8e36cf4`.
Root inspected the normalizer and both passing logs. Snapshot review/update
and the replay with updates disabled remain pending at this checkpoint.

The snapshot correction subsequently changed exactly ten BandSplit profile
snapshots for the new mode, routing/count and second-cutoff controls, including
the narrow-layout overflow group. No geometry solver assertion changed.
The same `band_split::all_profiles` integration case now passes with
`INSTA_UPDATE=no`: `/tmp/sotf-aud143-band-split-snapshots-verify-r1.log`,
SHA-256 `f88098be135a47a72035cf339542cc483baa4ff130783db57716f16a9ce9983b`.
Root inspected the terminal log and the ten-file snapshot diff scope.
Both failures from the intermediate combined run therefore have focused
passing corrections. A later full combined/workspace gate remains required;
mounted UI execution is separate from these render-plan snapshots.

### Actual mounted BandSplit controls checkpoint, 2026-09-30

The sibling `sotf-gpui --features dev-api --test e2e` case
`bandsplit_studio_controls_queue_structural_and_scalar_updates_and_roundtrip_preset`
passes 1/1. Root read the actual test: it mounts `Screen::Studio`, dispatches
mouse events at painted control bounds, checks Structural requests for
slope/mode/band-count edits, uses keyboard exact entry for cutoffs two/three
and checks Parameter indices 4/5, verifies graph widths, then saves/loads an
actual disk preset and constructs a fresh `PluginState`.
Log `/tmp/sotf-aud143-mounted-bandsplit-controls-r1.log`, SHA-256
`ec0bace1875697280bc4966b255c2490e9c3a7107b8eec698fb6ed484b52b19b`;
the reviewed sibling e2e source hash is
`0031c81ff8ddd3bf39b54d1dadf5dc0a67626f866003644e2116169357eeb87c`.
The command emits two unrelated existing player unused-import warnings and
an ALSA no-sequencer startup message; the selected test succeeds.

Root identified an uncovered UI issue in this otherwise real mounted test:
the initial two-band state expects the second-cutoff control to be rendered,
and the saved default snapshot marks it enabled. Only third-cutoff visibility
is currently checked across band counts, and initial band-button selection
is not asserted. Luna must check/fix active cutoff visibility and selected
count consistency for two/three/four bands, then strengthen the mounted
regression. The passing run proves its listed click/queue/preset routes; it
does not close this visibility finding or prove live device playback.

Luna subsequently added choice-conditioned second-cutoff groups and corrected
the mounted regression. Root verified current source: both extra cutoffs are
hidden initially for two bands, the selected Bands button is exactly 2, and
the mounted count loop checks cutoff 2/cutoff 3 visibility for 2/3/4 bands. The
layout unit, snapshot update/replay and mounted test all pass. Corrected
mounted log: `/tmp/sotf-aud143-mounted-bandsplit-controls-r2.log`, SHA-256
`19267028c4a35b1b94893a5bd78d11d1bb648fcc0026ab0265f0c94c8a95a5b1`.
Reviewed sibling test SHA-256:
`b7252be360fb7331ea148928ac17ed1eba8a7f447ebae9e7c89a94d185dded7c`;
BandSplit `params.rs` SHA-256:
`c1a57dc0b9990c8a1166d25b6a8b14697e862e1f5860f14af4d4634337df99c8`.
The visibility finding is corrected; the established queue/preset checks
remain separate from applied engine/audio and native device evidence.

Earlier coordination checkpoint: BandSplit's engine mapping compiled after matching
its six schema entries and correcting manual-accessor visibility. Root's
native reconfiguration run6 passes both exported CLAP/VST3 tests; the AUD140
facade r4 passes 13 tests with three manual utilities ignored. Both commands
have matching 57-file selected source/artifact manifests at
`522f2daa68feff091b67fbc1f848522cf744d95126e85a7a7545291992d5a4da`.
Logs are `/tmp/sotf-aud135-reconfigure-run6.log` (SHA-256
`a2d395b375c0b9fe182eeb5edaf2f1fc3dabe5a723e36887cef9dade4bbe81de`)
and `/tmp/sotf-aud138-aud140-facade-r4.log` (SHA-256
`410f1e24cbba1739b63623231011f4699e6bd3e30d5ad6a8f7b8626804c21b77`).
Earlier run5/r3 failed before tests at accessor visibility; their logs remain.
Earlier Astra resume/new-spawn calls failed with the harness error
`agent thread limit reached`. After the HAL Luna owner finished its checkpoint,
the existing Astra medium agent successfully resumed for independent AUD138
review. BandSplit and native work continue on two Luna agents. Passing tests
remain separate from Astra acceptance; the other review checkpoints are queued.

The first strict `sotf-host --all-targets` Clippy gate with both
`external-plugin-clap,external-plugin-vst3` enabled failed on the new
eight-argument VST3 `initialize_component` helper. Luna grouped its inputs in
a typed initialization structure and corrected the explicit reference
lifetimes. The earlier default-feature host lint does not cover this code.
Original log `/tmp/sotf-aud135-reconfigure-host-clippy.log` has SHA-256
`2552bca2801c310b3e15a4b8a7683c1e37c58dbf60f5b1abd994f84456b1090a`;
the 251-file selected host-source/lock manifests match at
`f390f8bc271f90032ed12eedfc7e83097fa46895272664f566412c50319ca6c9`.
Native-feature strict lint now passes: log
`/tmp/sotf-aud135-reconfigure-host-clippy-r3.log`, SHA-256
`9f94a22d19395278aa34a42ccc3b849efb1a245f726af386bf43b89065a4d734`.
Its selected 251-file source/lock manifests match at
`2deb77eb36c69ae6584dedb3034b0c813e9cfa0b821e1d3be2b609a05907f2a4`.
The post-refactor exported audio replay also passes both CLAP/VST3 cases:
`/tmp/sotf-aud135-reconfigure-run7.log`, SHA-256
`48e78ef3b72a88cb72928a4ac7af43f199b7e5c50a6f7e006087b60cd12d342b`.
Its 57-file selected source/artifact manifests match at
`539b66ff2b3bd93df64d4b7c3932967257867c35cda82330de5c4f592a8349e0`;
the loaded artifact remains `54c29a62…`. Run6 is retained as pre-refactor
evidence. Independent reconfiguration review remains pending.

The earlier focused engine `--lib band` gate passed all nine selected tests,
including both explicit-cutoff migration tests and two/three/four-band width
propagation. The preceding run exposed a second BandSplit propagation arm in
`update_channel_dependent_plugins` that still multiplied by two: BandMerge
received four channels instead of six for three bands. Luna corrected both
width paths to use the shared helper without changing the test expectation.
Passing log `/tmp/sotf-aud143-engine-band-r4.log` has SHA-256
`0450d24dac69e2de3a43cccb2da5c02d95b6c4f979d50e8fe76eb570fc8a36c6`;
the selected 57-file source/artifact manifests match at
`997144c9cb46e1f337f43091bda5eb9d4471b0d8c3bb75d3a1dcf71a2d067310`.
This proves the selected settings/width cases. The later 11-test gate below
also covers invalid-candidate rejection; actual compensated engine-chain
waveforms and populated-audio preservation remain separate gates.
The original failing log
`/tmp/sotf-aud143-engine-band-r3.log` has SHA-256
`9deaad8084bec595998de01f446761c9d8efc7feec7d5816e36edc5b03ac3200`,
with matching selected manifests at
`c639f3d7788819e75c3e2b0a72564915c1251de1e7f352269802f46180729635`.
This command used `TMPDIR=/tmp`: the earlier attempt failed at Clang temporary
file creation, and an intermediate retry exposed a helper scope error that
Luna fixed. No passing engine-chain claim uses those failed attempts.

The combined engine `--lib band` gate now passes 11 tests: seven BandSplit
cases and four incidental substring matches. It includes the new typed empty
cutoff rejection and the manager's no-commit/no-reconfigure regression.
Log `/tmp/sotf-aud143-engine-builder-band-r1.log`, SHA-256
`089ab5a0e4e8df2628992f93e6d7894d6fa939f536e93c9b255f85bad1e30407`;
the selected 625-file manifests match at
`8d2152e0878334dd66f40ae2ab7129310f18c78139cf229ec23f11b8bae50ff6`.
Strict `sotf-engine --lib --tests` Clippy initially found a collapsible nested
BandSplit conditional. Luna applied the equivalent let-chain form; rerun r2
passes with warnings denied. Log
`/tmp/sotf-aud143-engine-builder-clippy-r2.log`, SHA-256
`8a5cb753e84d53040f5000343fe58e72d5d21d2baaf5ec77ad81393cf180da52`;
the selected 625-file manifests match at
`eedb587d2aacbc4368ab5803efb35b8ce42525e238cedb933e108e5d6f7c7355`.
These manifests cover selected engine, facade, host, BandSplit, local test
crates, shared IIR math and configuration/locks, not every transitive source.
Independent production review remains pending.

Native BandSplit constructor and bus-route findings are recorded in
[`band-split-native-routing.md`](band-split-native-routing.md). Luna Native
owns NIH mapping/layout/restore work; Luna BandSplit owns the DSP, typed
engine, sibling mounted controls and preset routes. A complete VST3 bus path
remains required alongside CLAP.

The atomic FFI restore checkpoint now passes both focused public C ABI tests,
the full 70-test library and strict FFI `--lib --tests` lint. The native-route
report records exact audio/state assertions, corrected fixture startup
semantics, source/log hashes and root's preserved source archive. Independent
review is queued; dynamically loaded FFI and native buses are separate paths.

The first executed typed engine split/merge audio route also passes. It renders
a three-band PhaseCompensated LR24 chain with distinct stereo tones through
the offline decoder/host/file writer and checks summed unity magnitude within
0.005, finite distinct channels and the final stereo width. A second case
rejects typed empty cutoffs before truncating an existing destination file.
The selected engine `--lib bandsplit` run passes five tests (three earlier
width cases and these two):
`/tmp/sotf-aud143-engine-offline-bandsplit-r1.log`, SHA-256
`a94f2b1dd5eab81d8b271d8327ac84f02885cf6dcac4f7c2b0e2155c16c51f45`.
Root inspected the current test and terminal log. This first fixture measures
fundamental magnitude, not full phase/waveform accuracy, and does not yet cover
two/four-band engine audio. Luna is expanding it to the accepted route matrix.

The expanded offline fixture now passes one test containing all six
combinations of two/three/four bands and LR24/LR48 at 48 kHz stereo. It compares
every settled output sample against an independent f64 bilinear all-pass
product, with distinct 2 kHz and 7 kHz inputs and an input-normalized RMS limit
of 0.002. The largest observed normalized RMS is `2.97856802e-6`; the largest
absolute sample residual across cases is `2.50362046e-6`. Each render retains
stereo output and finite samples. The reference's unity magnitude is checked;
the full waveform residual is the measured-output accuracy gate.
Root inspected the reference arithmetic, current test and terminal log:
`/tmp/sotf-aud143-engine-offline-bandsplit-r2.log`, SHA-256
`773d69fb8e49e62b76e04a51227c523886dae6a661b2aabc45ddf3319b332e0c`.
Test-file SHA-256 at this waveform checkpoint was
`15cd70c707c52379a3e4817dffd3adff91bcdc48a67bc64737b13d5a259bf184`;
this is a post-run selected-file check, not a complete run-bound manifest.
This extends the actual engine route evidence; it does not complete mounted
controls, CPU/lifecycle gates or independent production review.

The subsequent strict engine library/tests Clippy gate caught the new test's
fixed-width `chunks_exact(2)` iteration. Luna replaced it with `as_chunks::<2>()`
and explicitly asserts the remainder is empty. The corrected strict gate
passes in `/tmp/sotf-aud143-engine-offline-clippy-r2.log`, SHA-256
`564b391a7e8ee34f06ac8ea0e588e1c8de37a01116f06d75596c7755f4521cf1`.
Root inspected its terminal successful output. This is a test-only lint
correction; no later full engine suite is claimed.

The sibling test named
`bandsplit_panel_controls_channel_routing_and_presets_cover_two_to_four_bands`
passes in `sotf-player --lib`. Root inspected its body: it creates a
`PluginController`, reads descriptors, calls setters directly, checks graph
widths and saves/reloads an actual preset file. This is controller/model and
disk-preset coverage. It does not mount a GPUI window or dispatch real control
events; its non-None effect assertions do not prove Structural dispatch.
The log filename misleadingly contains `mounted`:
`/tmp/sotf-aud143-mounted-bandsplit-panel-r1.log`, SHA-256
`2a31afe41df0dadc3a0431664c81486dc21b94973a87727c5816dd0e457c1f28`.
Inspected sibling test-file SHA-256 is
`c220251b206d1e19305cb52d9b11b953d33d2d050b36133fdac6343d9c5c95d3`.
Compilation emitted two existing unused-import warnings. Actual mounted
BandSplit controls and their structural/scalar dispatch remain required;
Luna will use the prior AUD134 GPUI Studio mouse-event harness after the FFI
review fixes. SOTF has no initialized TokenSave graph, so this source check used
current bounded file reads rather than graph results.

Root also executed BandSplit's existing `realtime_parameters` integration
target against the implementation checkpoint. Its structural-refusal test
passed, but `live_frequency_and_gain_updates_do_not_allocate` failed with one
allocation during a cutoff update. Original failing log:
`/tmp/sotf-aud143-realtime-parameter-checkpoint.log`, SHA-256
`922948dd84baddb4617fbc38da0c8fc98c2ff610ce4e17f3f04577b297f22d26`.
Selected BandSplit source/test/lock manifests match at
`b5c98e0d01a04548a35d2a1258b48df96a7a77e6c6937fdfaaaca914dc3fe5a3`.
This failure limited the earlier 47-pass library result. Luna replaced the
formatted frequency-ID match with allocation-free parsing; the first rerun
caught a missing `.ok()` in the parser, which Luna corrected. Rerun r3 now
passes both existing tests with the allocation guard unchanged. Log:
`/tmp/sotf-aud143-realtime-parameter-r3.log`, SHA-256
`02978340658bc0089b532d1c15475bbb82b76b3770a2bed91948bfa92904a455`;
selected source/test/lock manifests match at
`6049a2acb37084c18224299a479c727647adf31646f4b5d0685032a75072cc34`.
This closes the reported cutoff/gain allocation regression; compensation,
populated reset/deallocation, full engine and mounted-control gates remain.

Current-source inspection found that LR48 reset still reconstructed its vectors;
Luna has now replaced that operation with in-place state clearing in the shared
math implementation. LR4/LR8 and their multiband wrappers also gain explicit
reset-at-target methods, so BandSplit can reset at the selected cutoff even
below the normal setter thresholds (0.1 Hz for LR48, 0.001 Hz for LR24).
Ordinary setter thresholds remain unchanged.

Root's independent math-workspace commands pass: `--lib reset` selects 16
tests, including both new populated subthreshold-target/fresh-instance checks;
`--lib lr` selects 24 tests, covering both crossover families, their f32 paths,
ordinary setters and the generic Crossover LR4 comparison. Logs:
`/tmp/sotf-aud143-math-reset-r1.log` (SHA-256
`1896f3f6fe036bb988c09309fce3f8abd79ead1e9e78acae41d84d4e73cca428`),
`/tmp/sotf-aud143-math-crossovers-r1.log` (SHA-256
`067bc76eaeef38c128bff437798553884bdd8d29b62d2633c32a7d493a02d626`).
Both selected 63-file source/config/lock manifests match at
`82e08c564571475762eac960a42c502bf3030320dd2ae50579595c4986696131`.
Strict math-workspace Clippy (`--lib --tests -- -D warnings`) also passes
against that same selected manifest: `/tmp/sotf-aud143-math-clippy-r1.log`,
SHA-256 `7cbb2c496504565f5b2fb56aa492bbbe7f21a3275f28a7a09878a5e106a9ff76`.
These use the math workspace's locked unoptimized test profile and do not
compile the changing host/HAL sources. The reset filter also selects unrelated
`aupreset` tests; do not count all 16 as crossover reset coverage. Public
BandSplit reset-to-fresh, populated allocation/deallocation and legacy-waveform
replay against the changed math dependency remain required; the math tests
alone do not establish those properties or Astra acceptance.

The subsequent DAW plugin gate now passes 48 library tests, one ten-case
legacy capture and one populated reset heap test. The two historical legacy
unity-magnitude red utilities remain ignored. The new tiny-cutoff reset test
uses the exact rounded f32 selected target for its fresh comparison; root
caught and Luna fixed a constructor type mismatch before this executed gate.
The heap test measures one reset after populated audio for each LR24/LR48 ×
LegacyCascade/PhaseCompensated combination at 48 kHz, stereo, four bands:
zero allocations and zero deallocations in all four cases. This is not yet
the wider repeated-reset or compensated waveform matrix.

Log `/tmp/sotf-aud143-plugin-reset-r1.log` has SHA-256
`e65dce9e023f23528fb601b7a17f4b3231f3a9b7b571d2c0693deb2774a4f6f4`.
Selected 473-file BandSplit/host/math/config/lock manifests match at
`40b1074c81989972d14360ba7f2992c058fcdc85ae2695eecce14024c46d1227`.
The fresh 9,504,620-byte capture
`/tmp/sotf-aud143-plugin-reset-r1-legacy.bin` matches the complete decompressed
repository fixture at SHA-256
`73b30f81f14464e87fc6982ddcd5c2e3c618ca41486a95ec9b840a3debc16ebc`.
This also compiles the host dependency at the new staged-reprepare checkpoint;
HAL is not part of this package gate and has no behavior claim from it.

The separate post-edit legacy replay passes all ten original public cases.
The complete new 9,504,620-byte archive matches the untouched pre-edit archive
at SHA-256 `73b30f81f14464e87fc6982ddcd5c2e3c618ca41486a95ec9b840a3debc16ebc`.
Log `/tmp/sotf-aud143-legacy-replay-checkpoint.log` has SHA-256
`7988bbc324e0d6ae350a6bdd9d043bfa984c5e4e79958c1863db536bca5e8037`;
selected source/test/lock manifests match at
`14794bd2bc25b6a755dfc694206fa8c1b44ebc3985b3ed98c1cadd1b095fa8d6`.
This covers the saved 48 kHz stereo legacy cases, not the new compensated mode
or wider application routes. Original archives were not overwritten.

The original split and host archives now also exist as lossless gzip fixtures
under `crates/sotf-plugins/crates/sotf-plugin-band-split/tests/data/aud143/`,
with a README recording formats, hashes and provenance limitations. Both
repository fixtures were decompressed and compared against the complete
original archive bytes. They are uncommitted repository files, not ignored
target artifacts. Luna still needs to wire clean-checkout replay to these
immutable fixtures; fixture preservation alone does not execute that replay.

### Latest Luna test checkpoints, 2026-09-30

The initial independent compensated BandSplit target passes two tests at
48 kHz, stereo, for LR24/LR48 and two/three/four bands with close and wide
cutoffs. It checks independent complex responses, summed unity magnitude and
the complete settled recombined waveform. Its initial automation assertion
uses normalized RMS; the required maximum absolute residual and timed events
have since been added but are not covered by this result. Broader rates,
channel counts and sweep coverage remain pending. Log:
`/tmp/sotf-aud143-compensated-accuracy-r2.log`, SHA-256
`33aff71fa1da8ff7adad3ccdaf579d9d5aa9344e8ce9254623e46f896d34cbde`.

The expanded r4 target now passes 4/4 in 1.08 seconds, with owner-confirmed
session 82051 exit 0. It adds selected 44.1 kHz mono and 96 kHz six/eight-channel
cases alongside 48 kHz stereo; LR24/LR48, close/wide two/three/four-band
cases use 0.5x/1x/2x cutoff and logarithmically spaced probes. Timed cutoff
events preserve the same absolute timeline across persistent contiguous and
irregular-partition instances in both modes, checking finite/bounded samples
and maximum absolute partition residual <= 2e-5. This is selected dimension
coverage, not every Cartesian combination or an independent time-varying
reference. The actual residual is not printed yet; Luna will add the required
measurement output. Log `/tmp/sotf-aud143-compensated-accuracy-r4.log`, SHA-256
`0ededebcacfee934a57db663903ddea2d9ca8c76d0ac8fe1b6c899f4c1a2eb9f`;
tested source-file SHA-256
`97e024261bcb941d48153c6e8307f6fca40a55fff6b645adb4388c340bd5363a`.
R3 failed before tests on two fixture compile errors, retained in its log;
Luna corrected the f32 target type and borrowed tone before r4.

R5 retains all four passing tests and now prints the measured residuals:
`/tmp/sotf-aud143-compensated-accuracy-r5.log`, SHA-256
`8c557cd2bf38782cb2eaef90e28f18daf206aa6fcfb1abc0cedce8a646769016`.
Across these selected cases, the maximum input-normalized settled-waveform RMS
is `1.11577940e-4` for LR24 and `7.55770932e-5` for LR48, below the declared
0.002 gate. Maximum absolute residuals are respectively `2.24925221e-5` and
`1.24930623e-5`. Absolute-timed automation reports zero RMS and zero maximum
sample difference across callback partitions in both slopes and both modes,
after checking finite/bounded output. Root inspected the terminal metrics;
this is still the selected matrix described above, not all rate/channel
combinations or a proof of unity response during automation.

The current compensation uses literal independent low-plus-high crossover
pairs, not a reduced-order allpass realization. Astra's accepted automation
requirements are synchronized applied cutoff decisions, finite/bounded output,
the absolute-timed partition residual, rejected-update preservation and
reset/reinitialize equivalence. A separate reduced-order time-varying
equivalence model is therefore not an additional gate for this implementation.
The fixed-cutoff unity proof still does not promise exact unity reconstruction
while cutoffs change; numerical reports must retain that distinction.

HAL preserving reprepare passes three focused growth/shrink/failed-prime tests
and a separate maximum admitted geometry heap test: 16 channels with a
16,384-frame physical ring, then process and complete finite drain under
allocation and deallocation guards. The full HAL suite passes 65/65; host
passes 551 with one ignored. This establishes those fixtures, not every graph
shape, upstream width or channel/sample-rate change. Logs and SHA-256:

- `/tmp/sotf-aud138-reprepare-focused1.log`:
  `6585bd848099f2629805e612a52935f8fdb6c39499f797dcf36bc86fe509913b`.
- `/tmp/sotf-aud138-reprepare-wide-heap1.log`:
  `93b691a2a3d72b888a789b4b301b55492649b67b62d002ce985a45ef9bbed948`.
- `/tmp/sotf-aud138-reprepare-hal-lib2.log`:
  `dd1cd738007de6e0f0a4c4eac6656cdb622235600a3e0e3d10b097f4a5862bc8`.
- `/tmp/sotf-aud138-reprepare-host-lib1.log`:
  `df126e19d3fd41530177416732d6ab8a6da9d4b2ebcdc04d3459c90af614e820`.

The first strict HAL Clippy invocation caught two host staging condition
lints. Luna collapsed the nested condition and removed its redundant later
check; HAL and host strict Clippy now pass. These behavior logs precede that
cleanup. Final lint logs are `/tmp/sotf-aud138-reprepare-hal-clippy2.log`
(SHA-256 `7edeffa6a65b91b9fde577119fd9a135fae05da34552e6d226ab52e057808ce1`)
and `/tmp/sotf-aud138-reprepare-host-clippy1.log`
(SHA-256 `19dd3e044d5b33d0de5c4130f9b05325727bb72b3f5bd092e55460fb9e68a82c`).
Root verified the owner's four source hashes and unchanged DAW lock, then
archived those bytes and seven original logs under
`crates/sotf-plugins/target/audit-artifacts/aud138-preserving-reprepare-r1/`.
The source archive has SHA-256
`1bfd644dc649bb195716c2a90272aee63f223ac6ddbd48c07eb7a91aedb02514`;
`receipt.json` records every source/log hash and provenance limits.

Astra medium completed the review and confirmed root's suspected priming failure bug:
`?` within the plain `prime_result` block returns from the whole reprepare
method when the writer errors, skipping its intended flush and `PrimingFailed`
assignment. Quiescing already marks the engine not ready; do not infer source
advancement from this finding. The existing zero-frame short-write test does
not cover writer errors. Review also requires already-active drain, stale-plan
and exercised large-buffer coverage. The detailed findings are in
`audit/reviews/AUD138-astra.md`; Luna has resumed to implement all four
corrections and return a new checkpoint for Astra. Implementation acceptance
remains open.

The first revision checkpoint passes 15 focused HAL tests, with owner-confirmed
session 34106 exit 0. It wraps priming in a Result-producing closure so writer
errors reach cleanup, and tests partial-prime failure/retry, growth and shrink
after drain has started, during-prime format/configuration/readiness changes,
and invalid geometry/lifecycle refusal. The revised heap case processes 8,193
frames at 16 channels and drains an 8,192-frame tail after preparing a
16,384-frame ring. Its accepted-sample buffer is reserved before the guard;
the complete accepted vector equals a deterministic nonzero delay oracle.
This exercises enlarged storage instead of the earlier one-frame smoke case.
Log `/tmp/sotf-aud138-reprepare-review-fix2.log`, SHA-256
`de9c8c25200b06f8a67da6717a606d5ede51681a4bc9da43526433b9f7c5d057`.
The first attempt failed before tests on a fake-writer fixture type error;
that separate log is retained. The later full-package gate now includes the
twin refinements, pre-quiesce plan mutation and deterministic staging failure:
71/71 pass in `/tmp/sotf-aud138-reprepare-hal-full-r4.log`, SHA-256
`3cc60c37773e99c264cc563ec48bdc62091bc8c145bba1ee4c257931900d9706`.
The first 70/71 run failed only the new format-mutation error expectation.
The existing host equality recheck deliberately returns `StalePlan`; the
corrected regression retains all queue, geometry, latency, source and transport
preservation assertions. Strict all-target HAL Clippy passes after simplifying
the fake writer's nested conditional: log
`/tmp/sotf-aud138-reprepare-hal-clippy-r5.log`, SHA-256
`44306e1b44e397885f61ed9424569aa6b74ed33c89464a2dca86a78a1c6dd6f7`.
The final source now also passes the full 71-test HAL run and nine focused
reprepare tests. Final full log `/tmp/sotf-aud138-reprepare-hal-full-r5.log`,
SHA-256 `a2446abaadf61d4a02113ce333ed3e14663322c2f7a7de15ac85aeee84de9deb`;
focused log `/tmp/sotf-aud138-reprepare-focused-final.log`, SHA-256
`23335546657eb345d396c3f4f3164b0cb0211b1419389a9d57b6090df7fff8ad`.
Root verified the selected current source hashes and all three final logs,
then archived them under
`crates/sotf-plugins/target/audit-artifacts/aud138-preserving-reprepare-r2/`.
Source archive SHA-256 is
`d96028a7e5fc577bd74921d05124d9c370117a3da16e341f3a37acceac6bc521`.
HAL source SHA-256 is
`27d7a4a452b580b0250ccce2b1994bade1e7b7c64516e3371a847c004aa7d04f`;
the three host source hashes and DAW lock match the previous reviewed archive.
The receipt documents that this is a selected-source preservation snapshot,
not a full transitive run-bound manifest. Shrink/backlog service now also has
allocation/deallocation guards. No new Astra acceptance is inferred from these
passes; native HAL playback and consuming engine admission remain open.

Astra completed the r2 review: cleanup, failure preservation and exercised
heap extents are accepted evidence. The resize helper has already completed
the producer and therefore proves only source-complete/sink-pending recovery.
One test-only condition remains: a producer with a multi-chunk tail must stay
incomplete across growth and shrink, then resume the remaining suffix without
repeating begin/drain or losing samples. Luna is adding this public-route case
against both an unresized twin and an independent sample vector. Astra requests
the focused regression and affected strict lint, with no broad rerun unless
new production changes become necessary. No new production defect was found.

That last regression now passes for growth and shrink with a producer whose
six-frame tail is emitted in three chunks. Backlog-only service preserves its
cursor; subsequent source calls resume once, with the complete audio matching
both an unresized twin and independently expected delayed samples. Focused log
`/tmp/sotf-aud138-multichunk-reprepare-focused-r3.log`, SHA-256
`bf9f83ec1a7aa0075c8cb554c8e20a47f81419df0ea98cc63151fca3b183c815`;
strict HAL all-target lint log
`/tmp/sotf-aud138-multichunk-reprepare-clippy-r1.log`, SHA-256
`8d34be7b97bb6470481527f39d69c55b6ccd89b7c883df0b68b511f1b7f2c71c`.
HAL SHA-256 is
`b81d7042ce4766cb8252c477539c0318a079f92e481f266929da3f147d027346`;
root verified that its complete production prefix matches archived r2.
The final five-entry selected manifest has aggregate SHA-256
`a8e77d733f79609cdc3d73d268cdfe827add0a2b0872fdfa1c26030d0619f79f`.
Astra accepted the bounded host/recovery/reprepare stage, including the prior
five host corrections. The earlier 71-test suite was not rerun as a 72-test
suite; the new focused evidence and affected lint were sufficient for this
test-only addition. Luna Upmixer has resumed AUD139 design/baseline work.
Root preserved the accepted selected source, final logs and Astra verdict in
`crates/sotf-plugins/target/audit-artifacts/aud138-host-accepted/`;
source archive SHA-256
`6c695106129f0582aa5f1a637a177f9d3742cf75412558a543741c3bab15a849`.
The earlier r1/r2 source archives remain intact.

Source inspection identified a BandSplit engine integration defect:
`processing_thread/build.rs` skips failed plugin constructors and returns an
otherwise successful linear host; manager validation does not validate the
BandSplit cutoff vector, and `apply_plugin_update_once` commits the returned
host after recording its warnings. Before the fix, an invalid BandSplit update
could escape factory rejection as an incomplete candidate. The executed red
and green tests below cover this path. The graph builder already returns
errors for failed nodes. This narrow correction does not authorize a broad
manager rewrite or establish rollback after playback reconfiguration fails.
The existing device-free manager test
`failed_graph_candidate_preserves_working_host_and_engine_snapshot` in
`manager_thread/apply.rs` provides processing/playback command probes for a
linear-update counterpart. Root identified this seam for Luna so the required
no-commit/no-reconfigure assertions can execute without skipped audio-device
integration tests.
Before the red gate, root caught a fixture mistake: raw public
`frequencies: []` is the preserved legacy fallback, not the invalid typed
`Some(empty)` case. Luna changed the fixture to construct `PluginSettings`
and run its converter, yielding canonical `band_split` and
`explicit_frequencies: []`, and added a positive legacy-empty control. The
failure must identify the empty explicit cutoffs, not an unknown plugin name
or an unsupported `channels` field. No production validation change is
justified by the earlier incorrect fixture.

The corrected device-free manager regression now reproduces the real failure.
Engine red r2 executed one test and failed the assertion that the processing
command queue remain empty: the invalid typed empty-cutoff candidate caused a
host commit command. Positive raw legacy-empty construction and the typed
converter's canonical/explicit fields were checked before that assertion.
No command was consumed by a real playback thread, so this proves the erroneous
commit request, not physical replacement or an audible dropout.
Log `/tmp/sotf-aud143-engine-invalid-candidate-red-r2.log`, SHA-256
`06bb36a24ceb513047c5d4a6f819fe40253c7096ef76048c7d25cc179cc35864`;
the owner confirmed session 55791 exit 101 and test-file SHA-256
`1be2bb95b54638426dcd71ada3bcd599f507d3fe87c01176fc2f5e22026d6db7`.
R1 failed before execution because the fixture used `index` instead of
`plugin_index` in its diagnostic assertion; that log is retained separately.

Luna's narrow builder correction now returns a diagnostic error for BandSplit
construction failures or input-width mismatches before returning a candidate
host. The same manager regression passes: neither host commit nor playback
reconfiguration is queued, and the captured engine geometry, latency and
error snapshot remain unchanged. Public legacy `frequencies: []` remains a
valid fallback; typed `explicit_frequencies: []` is rejected with the expected
cutoff error. Green log
`/tmp/sotf-aud143-engine-invalid-candidate-green-r1.log`, SHA-256
`e857345e43b0d6f69f615cfe481699f15f775b2c7894cf5dd59fda6c114a47d6`.
The later combined 11-test and strict lint receipts are recorded above.

New crossover findings: AUD141's public Plugin regression confirms the predicted
three-band LR24 sum mismatch; the correction now passes the expanded three- and
four-band response matrix after pre-edit captures. Actual host-chain verification
also passes, and Astra accepted the bounded correction. AUD142 reconfirms missing selectable IIR
families/orders against current primary documentation. See
`crossover-multiway-recombination.md` and `crossover-filter-choice-gap.md`.
The correctness finding takes priority over extending that topology.
AUD143 separately records BandSplit's documented three/four-band phase
misalignment, confirmed in the resolved shared math implementation. Public and actual-host measurements now reproduce its attenuation predictions; see
`band-split-phase-compensation-gap.md`. It remains part of feature/chain work
after the active checkpoints, and is not closed by correcting Crossover.

### Historical pause checkpoint (resumed)

At this historical pause, accepted AUD131/AUD132 were the current checkpoint;
the coordinated offline workspace gate had passed 6,071 tests, with 15 skipped
and MIDI/IAMF excluded. AUD133 had a proposal for Ambisonics orders 4–7 on
existing named layouts, but no implementation. Astra's initial design review
requested physical-matrix rank diagnostics, quantitative grid/norm/residual
criteria, order-7 basis-impulse coverage, a committed AUD051 dual-band/EOS
baseline, and bounded performance evidence. AUD134 had not started at that
checkpoint.

### Current resumed checkpoint

The user restarted the audit on clean `main` at `9302797`. Later commits add
MIDI/IAMF work, which remains excluded from this audit. AUD133 now has the
orders 4–7 named-layout implementation, independent numerical and lower-order
preservation checks, order-7 public processing/tail coverage, engine 64-input
and 16-output capacity coverage, a full 64-channel AIFF→decoder→engine→plugin
waveform oracle, literal 64/65 service-PCM admission tests, catalog/factory
coverage, model reconciliation/canvas persistence coverage, and paired
performance measurements. Prepared-vector pointer/capacity tests do not claim
whole-callback allocation freedom. Its focused tests and strict package Clippy
pass; Astra accepted this bounded named-layout/engine/model scope on
2026-09-29. The report is `audit/ambisonics-orders-4-through-7.md`. The sibling
  graph test passed 1/1 under reviewed resolver lock
  `475d5890fddf40c8ca8361c6455057c1f3701c713c5844f2cdfda5a53303fb4e`; it tests
  full graph-model reconciliation and persistence, not a mounted order-edit
  click. Root has since retained this reviewed lock in the sibling worktree;
  current sibling builds use the same tested resolver bytes.

The latest coordinated broad gate ran 6,100 tests across 362 binaries: 6,100
passed, 0 failed, 19 skipped, in 272.300 seconds. Log:
`/tmp/sotf-aud133-134-workspace-rerun.log`, SHA-256
`4c417bab556373409374329b1de38b30abab67cdbafc36908037eb23bb4dd449`. Its
2,676-file start/end manifests match at `85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`;
DAW Cargo.lock is `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
The immediately prior 6,093-pass/7-failure/19-skip run is preserved as
historical evidence in the issue reports. At the AUD133 acceptance checkpoint,
the two AIFF route tests, actual capacity scratch-preparation test, related
prepared-buffer test, and strict `sotf-engine`/`plugins-bridge` all-target
Clippy passed following test-only lint cleanup. Later AUD135/136 source and
test additions mean the 2,676-file broad-gate manifest is a historical tested
snapshot, not a current whole-tree claim. Astra accepted scoped AUD133 on
2026-09-29. This does not close the wider spatial audit.

AUD134's scoped implementation and mounted UI evidence are also accepted by
Astra on 2026-09-29. The mounted GPUI Advanced click changes `true_stereo`,
queues a Structural rebuild request, and survives disk-preset/fresh-model
roundtrip; see `audit/true-stereo-convolution.md` and
`audit/reviews/AUD134-astra.md`. Full feature parity remains open.

AUD134 remains independently owned by the Upmixer track. Package, FFI, strict
lint, coordinated workspace, and mounted UI gates are accepted for the bounded
true-stereo feature. Missing native file-path persistence and other convolution
routes remain open; see `audit/true-stereo-convolution.md`.

AUD135's bounded design is accepted by Astra on 2026-09-29. It covers all
existing named outputs through 9.1.6 and whole-vector inputs through 64
channels, including 64→16, with typed per-instance state and host
reactivation. Bridge, FFI and VST3 target all 56 tuples. Standard CLAP targets
42 tuples; its missing wide-speaker roles make 9.1.4 and 9.1.6 an explicit
interoperability backlog item. Pre-edit bridge/FFI all-56 route checks and the
NIH 4→6 default waveform are captured. The corrected NIH wrapper suite passes
100 library tests (one manual utility ignored), strict all-target Clippy, and
the refreshed copied BufferManager harness passes 10 tests with its offline
shim qualification. The corrected production cdylib rebuild passed; root
verified artifact SHA-256
`990c7f1eec99d508ef223ce957af852e0a709a625e6c0d522225513c86a852c7`.
Astra accepted this scoped wrapper stage. Luna has started the consuming-host
typed setup/loading/persistence stage. Actual in-process CLAP/VST3 callbacks
exercise order-7 64→12 and 64→16 with canaries, input/f64 rejection,
state-sensitive no-advance validation, and process-allocation guards. The
100-test gate manifest has 17 entries; the strict-lint manifest has 22 and
includes later shared host/HAL source edits, so these are separately qualified
snapshots, not one identical source manifest. External host loading, post-edit
all-56 bridge/FFI replay, typed per-instance setup/preset/isolated-worker
persistence, mounted setup/reactivation and EOF are still open; a successful
cdylib build is not host-load evidence. See
`audit/native-ambisonics-orders-1-through-7.md`,
`audit/proposals/native-ambisonics-orders-1-through-7.md`,
`audit/native-speaker-role-mapping.md`, and `audit/reviews/AUD135-astra.md`.

AUD136's 48 kHz mono/stereo implementation is accepted by Astra on
2026-09-29. The enabled SpeechDenoiser now emits the declared 960-frame
accepted-program continuation and resets residual backend state. Its natural
model/high-pass response remains `TailLength::Unknown`. Actual pre-edit source
and enabled/disabled audio arrays are preserved. The final package run passes
46 tests with two manual utilities ignored; explicit pre-edit byte replay passes
1/1; strict plugin/backend Clippy, formatting and diff checks pass. Root and
Astra independently verified the selected source/lock manifest and current files
at `c7aca58a5882162c229d38e8b8b282b2b131c3501e18486410a1655093e786e9`.
Evidence covers all 480 terminal phases in mono/stereo, callback partitions,
transitions, preflight, reset, allocation/deallocation guards, preserved final
telemetry and the actual 48 kHz DawHost endpoint; direct 44.1 kHz rejects.
The report includes the baseline directory required to reproduce its manual
replay. See `audit/speech-denoiser-enabled-accepted-queue.md` and
`audit/reviews/AUD136-astra.md`. The earlier 6,100-test workspace gate covers
the accepted AUD133/AUD134 checkpoint, not these subsequent changes.

AUD137's bounded same-rate, identity-frame serial child implementation is
accepted by Astra. Luna fixed both review findings: admission now requires an
explicit per-node geometry capability and equal negotiated rates, and drain
stops at the actual remaining timeline without excess silence. The final
package passes 131 tests (four manual utilities ignored), including the
15-test composition suite; saved seven-control byte replay passes 1/1, and
strict ABCompare/host/Delay Clippy passes. The nine-entry source/lock manifest
is `5d35cc3b55868eb1078af9dbabe9a5698211c6134a711f954110d0fa033a0425`.
Independent FIR vectors, real Delay, serial Rack/Graph, outer host and
process/drain/reset heap checks support this scoped acceptance. Unsupported
geometry, undeclared child capabilities, branching graphs and active/prior
recursive band-mask EOF remain open; see `audit/reviews/AUD137-astra.md`.

AUD138's direct HAL implementation is accepted by Astra: all 37
package tests, strict all-target Clippy and formatting pass. It drains retained
pending samples with at most two writer attempts per call, preserves pending
audio across control-thread recovery and counts explicit cancellation. See
`audit/hal-output-finite-stream.md` for the frozen source and terminal logs.
Corrected actual host regressions now reproduce a terminal-sink division by
zero and stale queued samples surviving reset. Drain is rejected by the shared
equal-width predicate before sink capacity preflight; the refused path preserves
the upstream tail. Astra accepted the revised bounded sink-drain design at
`bedb455fd4ba7abf835d10b537d3a1e93da0e44612a2864a830a0d56f4b3bf9a`.
It uses explicit sink mode before command-sender extraction, prepared append
hooks, preflight before producer mutation, reset-required partial failure and
frozen stream controls. The first corrected host package gate passes 44/44.
Luna's follow-up inspection identified an ordinary-input queue admission gap.
Two test-only public probes reproduce upstream advancement for an oversized
block and a full queue (`/tmp/sotf-aud138-hal-input-admission-red.log`, exit 101).
Astra accepted the ordinary-input retry amendment at proposal SHA
`a85b2ee6cd86e4d98d5edbf1efb0ee5a397b433c1c69c3f0d1f5fdcc59cc33dd`.
It now specifies public Running-only settlement of queued graph edits, since
ordinary `build()` does not consume that queue. The implementation now passes
54 package tests, including complete admission, queued settlement and failure
cases. A later regression exposed ordinary host fallback swallowing producer
errors on the sink route; sink-specific propagation now passes injected error
and panic cases with reset-required handling and exact recovery samples.
The 44-pass checkpoint predates the admission amendment. Current shared-host
compatibility checks and strict lint are still being completed.
Implementation acceptance and consuming application playback remain open.
AUD140's public tests now reproduce nonzero channel-changing serial EOF
rejection for actual 64→16 Ambisonics and a finite FIR producer before it.
Four meaningful ordinary-output baselines (orders 1, 3, 7 and FIR→order 7)
have been captured and replayed. Explicit completion assertions are present;
the pre-edit refusal capture recovers the full two-frame, 64-channel producer
tail through legal public rewiring. Root decoded that saved tail and confirmed
an exact independent arithmetic match, including the final W marker of 0.1875.
The bounded proposal is accepted by Astra at
`97dc22070325df23f46fa475b7a874a0934c9ffd4b94a7db8602b8a0695a2c3a`.
Luna implemented it; 13 focused tests and the explicit saved-audio replay
pass. These cover scratch extents through channel expansion and contraction,
two finite producers across a width-changing stage, successful/refused bypass
and missing-capability, unequal-rate and invalid-width refusal. Host, facade
and Ambisonics strict all-target lint pass. Astra accepted the frozen
implementation in `audit/reviews/AUD140-astra.md`. Root compared both ordinary fast-path helpers with the frozen
pre-edit source and confirmed identical bytes (receipt
`/tmp/sotf-aud140-root-fastpath-check.json`). Ordinary
optimized processing eligibility must be preserved. Generic host
draining, its consuming scheduler and physical playback
remain open; normal engine setup currently rejects zero-output hosts. See the
two proposals, reviews and `audit/remaining-finite-stream-reconciliation.md`.
Neither batch authorizes
the previously rejected generic unequal-rate queue or manager rewrites.

## Execution policy requested by the user

- **Implementation:** `gpt-6-luna`, reasoning effort **xhigh**.
- **Independent validation:** `gpt-6-astra`, reasoning effort **medium**.
- Luna implements one coherent issue or related batch, records evidence and
  hands it to Astra. Astra inspects actual changes, independent expectations,
  executed results and remaining risks. Findings return to Luna for correction;
  Astra reviews again until the batch passes. Do not weaken an oracle merely
  to make the implementation pass.
- The coordinator assigns work and reports results. It does not use Astra at
  ultra effort for implementation or duplicate the delegated investigations.
- Keep the full audit open until every crate and requirement has adequate
  evidence. A green workspace run alone does not establish feature parity.

## Parallel execution assignments

The user requested parallel Luna implementation. Four active slots are available
including the root coordinator. Up to three Luna workers can implement in
parallel; at review checkpoints one owner ends its turn to free a slot for
Astra, then resumes for corrections or its next stage. Interrupting a worker
alone does not release its thread slot; it must send its final handoff.

| Agent | Model / effort | Exclusive implementation ownership |
|---|---|---|
| `/root/luna_implementation` | Luna / xhigh | Host metering and spatial SOFA work; AUD124–128, accepted scoped AUD131 and AUD133 named orders 4–7; AUD135 accepted native Ambisonics design and NIH wrapper/framework implementation, then consuming-host/state integration excluding Upmixer/convolution paths; shared audit-ledger/plan updates |
| `/root/luna_upmixer` | Luna / xhigh | Accepted AUD129/130/132 Upmixer corrections, AUD134 true-stereo convolution, AUD136 SpeechDenoiser EOS and AUD137 ABCompare composition; now AUD140 nonzero channel-changing host EOF regressions and design, coordinated with HAL's shared host ownership |
| `/root/astra_validation` | Astra / medium | Independent reviews and acceptance logs for both tracks; implementation corrections return to their Luna owner |
| `/root/luna_hal_eof` | Luna / xhigh | AUD138 direct HAL pending-write implementation accepted; bounded exclusive-owner serial sink-host design also accepted, now implementing host handoff/reset/lifecycle and public failure/heap tests; engine admission and physical playback remain separate open scope |

Research and edits run concurrently in separate owned files. Coordinate any
cross-track file change before editing. Serialize ready Cargo jobs with a
process-held `flock /tmp/sotf-daw-audit-cargo.lock`; do not reserve the build
target while editing or drafting reports. Use the shared absolute target
`/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`, with `TMPDIR`
set to its `audit-tmp` subdirectory and offline/locked Cargo resolution. A
queued command releases the lock automatically at exit. Retain its actual
session and terminal log; an observation timeout does not mean it stopped.
Coordinate a stable source snapshot for broad integration gates and identify
the actual tested snapshot if another worker edits during a running test.
Each track repeats implementation/review/correction until accepted, then takes
another independent batch from the full remaining plan.

## Completed work and authoritative evidence

`../AUDIT.md` contains the issue-by-issue ledger through AUD142, the complete
workspace inventory (59 in-scope crates at the recorded checkpoint), comparison
tables and links to detailed reports. Some older reports describe defects later
fixed: reconcile them with the newest issue/checkpoint before reopening work.
Do not interpret every issue numbered below 123 as completely closed.

| Area | Work already implemented and tested | Remaining qualification |
|---|---|---|
| Host and chain | Same-rate latency compensation, automation timing fixes, bounded drain/preflight contracts, bypass handling, f32/f64 dispatch, resampling clocks and endpoint fixes | Unequal-rate branch retention and A/B nested variable-rate composition remain open |
| Dynamics | Expansion laws, Gate modes, compressor range/hold, de-esser range/linking, analog control retention, limiter channel behavior and actual 2x/4x audio oversampling | Further family comparisons, native automation coverage and recursive-tail policies remain |
| Filters/restoration | Independent EQ/convolution/crossover response tests, finite-stream fixes across many buffered families, resampler cutoff/lifecycle fixes, denoiser timing and callback partition fixes | Feature gaps and reference-quality restoration/corpus evidence remain |
| Spatial | NUPC timing, Binaural/XTC/Upmixer streaming and tails, transactional initialization/publication, integer and fractional/signed SOFA delays, corrected HRIR resampling, small FFT64/128 Upmixer initialization, AUD129 minimum geometry/capacity, bounded AUD130 correction below 512, AUD132 source-time retiming at/above 512, and AUD133 named orders 4–7 | AUD129–133 and AUD131/132 scoped implementations are accepted. AUD133's 6,100/19 gate is a historical checkpoint, not a current whole-tree claim. AUD135 corrected wrapper/framework ABI stage is accepted: 56 bridge/FFI/VST3 tuples targeted, 42 standard CLAP tuples, order-7 in-process CLAP 64→12 and VST3 64→16 callbacks/cdylib verified. External host loading, typed persisted setup, mounted activation, post-edit all-56 bridge/FFI replay, EOF, and wide CLAP remain open. AUD136 SpeechDenoiser EOS and bounded AUD137 ABCompare composition are accepted; AUD138 HAL host EOF and AUD140 channel-changing EOF remain active. Native-device evidence, custom layouts and other route gaps remain open |
| AutoGain | Accurate smoothing; causal, aligned measurement clocks in EQ/Crossfeed/AAE/Upmixer; reduced private meters with exact compatibility and matched CPU improvements | Historical AUD115 AAE/Upmixer matched CPU evidence remains missing |
| Metering | Integrated history preparation, optional bounded LRA, immutable prepared snapshot publication, spectrum endpoint power correction, published true-peak coefficients, finite-stream finalization, AUD123 rate coverage (implemented and independently accepted) | Broader metering feature/accuracy comparison remains |
| Engine/drivers | Worker interruption fixes, isolated pending-format guards, offline tail API/endpoint composition, iOS feeder fixes, portable HAL staging and compile checks | Coordinated transition protocol and native macOS/iOS execution remain |
| Integration layers | Parameter typing/restore, FFI return-count and transactional restore checks, realtime scalar access, native transport/precision/tails and selected automation paths | Reconcile all wrapper families and platform-specific routes with actual DSP behavior |

### Previous passing workspace checkpoint: AUD132 with AUD131

- Offline locked workspace nextest: **6,071 tests passed across 359 binaries;
  15 skipped; 0 failed** in 269.739 seconds. FFI remained included; MIDI and
  IAMF test packages were excluded. Full-tree start/end manifests match at
  aggregate SHA-256 `1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb`;
  Cargo.lock SHA-256 is
  `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`. The
  log is `/tmp/sotf-aud132-aud131-workspace-final.log`.
- AUD131 signed/fractional SOFA delays and AUD132 above-512 Upmixer source-tag
  retiming are accepted by Astra on 2026-09-29. AUD132's post-capfix strict
  Upmixer Clippy passes. Detailed scoped evidence is in
  `sofa-fractional-delay.md` and `upmixer-above512-hr-timing.md`; the full
  audit remains open and this workspace run alone does not establish feature
  parity.
- The prior AUD127 host/API and reachable TUI/Studio stages remain accepted;
  their focused host integration, strict Clippy, UI compile, localization,
  layout, redraw and mounted-route evidence are in
  `first-minute-lra-stability.md`.
- Current metering queue: AUD128's public requirements inventory is accepted;
  corpus acquisition/execution remains pending owner use/access determination
  and an actual archive README. Do not fetch the corpus.
- Spatial queue: accepted AUD109/AUD114 cover exact integer SOFA delays and
  HRIR resampling. AUD131/AUD133 and AUD129/AUD130/AUD132 have scoped
  acceptance. AUD133 implementation, focused evidence, strict engine/bridge
  Clippy and the coordinated 6,100/19 broad gate pass; Astra accepted this
  bounded scope on 2026-09-29.
  Its 64-channel AIFF/service-PCM paths, engine admission boundaries, full
  graph-model/canvas route, numerical oracle, lower-order bitwise controls and
  paired performance data are in `audit/ambisonics-orders-4-through-7.md`.
  Capacity checks cover named prepared vectors, not whole-callback allocation.
  Native bridge/FFI/NIH, hidden structural controls and custom layouts remain
  open; see `audit/current-feature-route-coverage.md`.
  AUD134 true-stereo convolution's bounded DAW and mounted GPUI scope is
  accepted; full feature parity remains open. See
  `audit/true-stereo-convolution.md` and `audit/reviews/AUD134-astra.md`.
  AUD135's accepted design, pre-edit bridge/FFI/native-default baselines, and
  accepted corrected wrapper/framework gates are recorded in
  `audit/native-ambisonics-orders-1-through-7.md`; consuming-host/state
  integration is now underway. AUD136 and AUD137 are accepted for their bounded
  scopes. AUD138's direct sink is accepted and its host stage is being
  implemented; AUD140 implementation and gates are accepted. AUD139 design and
  pre-edit baselines are underway, including the additional current-source
  findings in `audit/dynamic-eq-ui-route-findings.md`. See the ledger and linked reports
  for current details.

### Latest coordinated broad-gate run: AUD133 + AUD134 snapshot

- Command: `CARGO_NET_OFFLINE=true cargo nextest run --offline --locked
  --workspace --exclude sotf-midi --exclude sotf-iamf --no-fail-fast
  --status-level fail`, using the shared DAW `CARGO_TARGET_DIR` and `TMPDIR`.
  Result: 6,100 passed, 0 failed, 19 skipped across 362 binaries in 272.300 s.
  Log `/tmp/sotf-aud133-134-workspace-rerun.log`, SHA-256
  `4c417bab556373409374329b1de38b30abab67cdbafc36908037eb23bb4dd449`.
- Whole-tree start/end manifests contain 2,676 files and match exactly at
  aggregate SHA-256
  `85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`.
  DAW Cargo.lock SHA-256 is
  `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
- Post-gate AUD133 lint cleanup changed only two test files; the exact AIFF,
  processing capacity, and strict engine/bridge Clippy reruns are listed in
  `audit/ambisonics-orders-4-through-7.md`. Their scoped source+lock hashes
  match before and after. Root owns any later broad rerun; none is needed for
  this semantics-preserving fixture cleanup. Astra accepted scoped AUD133;
  the then-current whole-tree manifest differed from this tested broad snapshot only
  in those two test fixtures.

### Previous coordinated broad-gate run: AUD131 + AUD132 source snapshot

- Command: `CARGO_NET_OFFLINE=true cargo nextest run --offline --locked
  --workspace --exclude sotf-midi --exclude sotf-iamf --no-fail-fast
  --status-level fail`, with the shared DAW `CARGO_TARGET_DIR` and `TMPDIR`.
  It ran **6,071 tests across 359 binaries: 6,071 passed, 0 failed, 15
  skipped; two tests were marked slow**, in 269.739 seconds. Complete log:
  `/tmp/sotf-aud132-aud131-workspace-final.log`.
- Whole-tree start/end manifests are `/tmp/sotf-aud132-final-workspace-start.sha256`
  and `/tmp/sotf-aud132-final-workspace-end.sha256`; both aggregate to
  `1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb` and have
  an empty diff. Cargo.lock stayed at SHA-256
  `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
- The test-only AUD132 N8192 capfix was separately focused-tested with
  matching package+lock manifests (`b3c4d7d54063e203edf613d32c4ad6e47a3dc4426171f8c171f423a770671ad7`);
  its below-cap complete-vector delta is zero. Post-capfix strict Upmixer
  all-target Clippy passes at `/tmp/sotf-aud132-final-clippy.log`.

### Historical non-green gate: AUD131 source before final AUD132 correction

- The earlier offline workspace run executed 6,065 tests across 359 binaries
  with 14 skipped in 271.106 seconds: **6,064 passed, 1 failed, 14 skipped**.
  Its only failure was the then-active AUD132 N=2048 candidate-tone test with
  residual 0.014197 (> 0.01). The rejected no-input-delay candidate failed
  the accuracy criterion and is now retained as explicit rejected-candidate
  evidence; this historical run is superseded by the final green run above.
  Historical log: `/tmp/sotf-aud131-workspace-final.log`.
- Historical start/end manifest SHA-256 was
  `021d0b339aa31701394bd4223ee73dfecd7dd22759a7f857377c9c8b3143a1c4`; files
  `/tmp/sotf-aud131-workspace-start.sha256` and
  `/tmp/sotf-aud131-workspace-end.sha256`.

### Historical workspace checkpoint: AUD122

- Workspace: **5,988 tests passed across 353 binaries**, ten skipped. FFI is
  included; MIDI/IAMF package tests are excluded.
- Log: `/tmp/sotf-audit-wave19-nextest.log`; build 36.89 s, tests 80.577 s.
- Host: **718 passed**, eight ignored doctests. Processing-worker tests: **68
  passed**. Backend EBU tests: **24 passed**.
- Strict host/engine all-target Clippy passed. Backend library Clippy passed
  with two documented preexisting lint exceptions.
- Formatting: 410 changed in-scope non-vendored Rust files, plus changed
  vendored files, passed; scoped diff checks passed.
- AUD121 corrected a 4.675 dB analytic true-peak overread. AUD122 corrected a
  30.455 dB final-impulse underread and stale engine UI data at EOS. Finalization
  emits no audio, advances no loudness clock, and has zero measured heap activity.
- Reports: `true-peak-coefficients.md`, `true-peak-finite-stream.md`,
  `auto-gain-meter-cost.md`, `loudness-range.md`, `upmixer-small-fft.md`.

## Historical batch: AUD123 (implementation complete and independently accepted)

The host true-peak rate extension, independent accuracy/lifecycle tests,
realtime allocation checks, and CPU evidence are implemented. Astra's
Independent validation accepted the scoped implementation on 2026-09-28. The
offline workspace nextest gate passes and the full metering audit remains open.

- Proposal: `proposals/true-peak-rates.md`.
- Tests: `../crates/sotf-plugins/crates/sotf-host/tests/true_peak_rates.rs`.
- Executed red result: `/tmp/sotf-aud123-public-red.log`. Both tests compile and
  fail: 176.4 kHz is unavailable; EBU synthetic case 15 at 8 kHz is unavailable.
- The proposal uses published ITU/EBU primary documents and libebur128's public
  contract. A cascade of the published FIR was rejected because its prototype
  overread an EBU tone by 0.360 dB. Do not resurrect that candidate without new
  evidence resolving the failure.

### AUD123 implementation and evidence

- Host rate policy is 8,000 through 2,822,400 Hz inclusive. Its power-of-two
  interpolation factor reaches at least 192 kHz (2x minimum, 32x maximum). Status is
  explicitly unavailable outside the accepted interval.
- The published 12-tap paths preserve exact 48/96 kHz arithmetic; 88.2 kHz uses
  four published phases. Prepared 64-tap Blackman-sinc phases serve 8x/16x/32x
  at lower rates. Finalization advances 11 or 63 source intervals as required.
- The direct index-based oracle checks all 64 final impulse positions, dense
  signals and interval queries. An independent radius-64 Lanczos reconstruction
  spans the complete finite support on a 64x grid. EBU Tech 3341 synthetic cases
  15–19 pass across twelve rates with their 10 ms fades and stated tolerances.
- Public lifecycle tests cover custom rates, irregular callbacks, reset,
  reinitialize, disable/enable, retained strong and nested Weak readers, and
  final peak recovery against zero-continuation controls. Telemetry controls
  compare unaffected fields and preserve published 48/88.2/96 kHz outputs.
- Fresh-thread heap tests cover supported rate families through 2,822,400 Hz
  and 1/2/6/24 channels; two repetitions of processing, finalization, queries
  and reset report zero allocations and frees.
- Current focused run passes 549 host library tests (one ignored), then 4
  calibration, 3 heap, 11 finite-stream and 4 rate integration tests. Strict
  all-target host Clippy passes. A preceding full host package run passes 548
  library tests plus all integration tests; its log is `/tmp/sotf-aud123-host.log`.
- Criterion logs absolute callback and finish-plus-publication costs for
  128-frame, 1/24-channel fixtures at 8/12/24/44.1/48/96/192 kHz. On the
  24-channel profile, the 8 kHz factor-32 callback takes 3.701 ms of its 16 ms
  callback interval (23.1%); the 44.1 kHz factor-8 path takes 0.969 ms of
  2.903 ms (33.4%). The maximum accepted 2,822,400 Hz rate is accuracy/allocation
  tested but not CPU-profiled. A separate alternating kernel-only control
  reconstructs the former 12-tap loop, asserts exact interval and drain output,
  and reports no material change; it is not a historical whole-monitor
  benchmark.
- Full evidence and limitations: `true-peak-rates.md`. Latest focused test,
  Clippy, Criterion, and matched-control logs are under `/tmp/sotf-aud123-*`.
- The offline workspace nextest gate passes **5,997 tests**, with 11 skipped,
  in `/tmp/sotf-aud123-workspace-nextest-final.log`. It excludes MIDI and IAMF,
  includes FFI, and uses the writable nested target directory. The two slow
  tests took 267 and 77 seconds.
- Astra's acceptance and source hash record is `reviews/AUD123-astra.md`.

The separate math-dsp detector retains its explicit 48 kHz reference scope in
this batch. This is not a claim that its broader parity work is complete.

## Parallel Upmixer batch: AUD129 and bounded AUD130/AUD132 accepted; broader HR limits remain

Astra accepted AUD129's bounded minimum FFT geometry and HR ring-capacity
changes on 2026-09-28. The final Upmixer package snapshot passes 164 tests,
strict all-target Clippy, and formatting; source manifest SHA-256
`d0ef11f8e20a005c3483bdefee80b9dea3cb945c498938c64e15c5fb18f06835` matched
before and after its package gates. This was a scoped package gate, not a
workspace-wide run. Evidence and Astra's review are in
`upmixer-minimum-fft.md` and `reviews/AUD129-astra.md`.

Astra accepted the bounded AUD130 source-tag scheduler correction on
2026-09-28, and accepted the bounded AUD132 above-512 correction on
2026-09-29. For N<512, accepted-input credits now gate the prepared shared
latency path; tests verify the 512-frame HR/main arrival across single,
irregular and 512-frame callback partitions, plus long single-callback EOS,
HR resume and AutoGain/layout routes. N>=512 retains the prior prepared-gain
path and matches an isolated reconstructed pre-AUD130 mixer control across
partitions. The final AUD132 package passes 184 tests (3 intentionally
ignored), strict all-target Clippy and formatting; the accepted workspace
snapshot is recorded in `upmixer-above512-hr-timing.md`. The earlier AUD130
package passes 175 tests (2 intentionally ignored),
strict all-target Clippy and formatting; the start/end source+lock manifest
SHA-256 is `f5b180b05ff15981b20ea2e5ad306bf0001daafc34dba5258bc74a6394eb45f9`.
This is package-scoped, not a workspace-wide run. The >=512 control reconstructs
the mixer while sharing current analysis/preparation/drain; it is not an
archived whole-plugin baseline. The scoped source-timing issue is accepted;
general HR quality and CPU bounds remain open. Evidence and review are in
`upmixer-hr-timing.md`, `upmixer-above512-hr-timing.md`,
`reviews/AUD130-astra.md`, and `reviews/AUD132-astra.md`.

## Remaining full-audit work, in order

1. **Review metering requirements.** The post-AUD123 source comparison
   confirmed the programme maximum true-peak (AUD-124) and maximum M/S
   loudness (AUD-125) requirements; both staged core/API and display
   implementations are now accepted. AUD-126 coupled integrated/LRA pause and
   continue is also complete, including the reachable sibling UI. The
   first-60-second LRA instability indication (AUD-127) is accepted by Astra
   across host/API and reachable TUI/Studio stages on 2026-09-29. Focused host
   integration (10 passed), strict host Clippy, UI compilation/localization,
   and offline workspace nextest (6,055 passed/13 skipped across 359 binaries)
   pass on the final matching source manifests; evidence is at
   `first-minute-lra-stability.md`. The remaining confirmed metering gap is
   complete official EBU corpus coverage (AUD-128). Its proposal-only corpus
   inventory is at `proposals/ebu-loudness-test-set.md`; Astra accepted the
   public Tech 3341/3342 case map on 2026-09-29. The archive-to-case mapping,
   acquisition, and execution remain pending a project-owner determination of
   permitted use/access and an actual archive README. The official EBU page
   advertises a v5.0 ZIP containing 70 audio files and links restrictive terms
   of use. No corpus was acquired or added to the repository in this
   proposal-only batch.
   The broader audit remains open. Astra aligned on a
   staged core/API batch for AUD-124 after clarifying finite-only dBTP
   semantics, full initialization/reset coverage, retained-generation
   behavior, and the separate display requirement. The active proposal is
   `proposals/programme-maximum-true-peak.md`. Astra accepted the core/API stage
   on 2026-09-28 after the EOS-sensitive regression, strict lint, and offline
   workspace gate passed 6,003 tests with 11 skipped. A concrete TUI/GPUI
   programme-maximum display proposal is now at
   `proposals/programme-maximum-true-peak-ui.md`, with review diff
   `proposals/programme-maximum-true-peak-ui.patch`. Astra aligned the state
   precedence and redraw precision. The patch applies cleanly against the
   unchanged sibling paths. The user explicitly authorized sibling edits on
   2026-09-28. The reviewed patch covers the empty unsupported TUI state and
   24-column German wrapping, and stacks the GPUI label/value. The sibling
   integration is applied. Finite/unsupported TUI rendering, redraw-signature, GPUI translation
   helper, mounted Studio Loudness Monitor render, and three live endpoint
   queries (initial, higher, then lower interval) pass. GPUI all-target check,
   rustfmt, design-token, diff, and pseudo-locale generator checks pass. Tests
   used a temporary offline resolver lock; the exact pre-task sibling lock was
   restored after the gates. Astra accepted AUD-124 core and UI on 2026-09-28; AUD-124 is complete,
   while the broader audit remains open. The `proposals/maximum-ms-loudness-ui.md` design and sibling TUI/Studio
   implementation were accepted by Astra on 2026-09-28 under the existing
   sibling authorization. TUI finite/unsupported rendering, localized
   short-height and border-preservation layout, redraw signature, GPUI
   translation completeness, compact and mounted Studio rendering, and live
   routes for finite latches, lower intervals, reset/null, and nonfinite-to-null
   pass. GPUI dev-api all-target check plus rustfmt, pseudo-locale, design-token
   and diff checks pass. Exact evidence is in `/tmp/sotf-aud125-*`. Validation
   used temporary offline lock SHA-256
   `87f37029677509822f0164117da1a37fcf39d72a2523908b1b036f0b46bd554a` with
   `mimalloc 0.1.52`; the original lock SHA-256
   `2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f` was
   restored afterward, so those gates are not claims against the restored
   lock. Astra accepted AUD-125 core and UI; AUD-125 is complete. Astra accepted
   the AUD-126 host core/API implementation on 2026-09-29. Its prepared I/LRA
   active-time lane preserves four/thirty completed-sub-block admission at
   non-divisible rates. Focused tests, strict host Clippy, and the offline
   workspace gate (6,043 passed/13 skipped) pass on matching source manifests.
   Astra accepted the reachable sibling TUI/Studio controls and layered
   host-receipt tests on 2026-09-29. Core and UI evidence are in
   `coupled-integrated-lra-pause.md` and
   `coupled-integrated-lra-pause-ui.md`; AUD-126 is complete. Sibling checks
   used a temporary resolver lock, with the exact original lock restored.
   AUD-127 is accepted at its scoped host/API and reachable UI stages. Continue
   with the AUD-128 proposal-only corpus inventory; do not claim full EBU or
   overall audit completion from the synthetic tests or workspace gate.
2. **Finish confirmed correctness/evidence gaps.** AUD141's bounded multiway
   LR24 correction is accepted with independent complex-response and actual
   split/merge evidence. Finish AUD143 BandSplit and its native/application
   routes, and include both corrections in the combined gates. Reconcile AUD073 and the
   finite-stream inventory against later fixes. Resolve remaining buffered
   families, RNNoise/native EOS behavior and defined recursive-tail policies.
   Review AUD087 variable-rate A/B paths without applying the blocked host
   branch rewrite. The bounded AUD132 above-512 timing correction is accepted;
   retain its stated waveform/quality limits and open a new scoped issue only
   if broader spatial timing evidence establishes another gap. Obtain AUD115
   AAE/Upmixer CPU evidence if a reproducible baseline can be recovered.
3. **Complete feature implementation by family.** Revalidate the existing
   comparison tables, then create scoped issues and implement their genuine
   gaps with complete parameter/preset/automation/UI wiring. Recorded gaps
   include per-band channel/M/S selection, dynamic/shelf/spectral EQ features,
   additional/native true-stereo convolution route coverage beyond accepted
   AUD134, crossover slope choices, restoration
   profiles/curves/residual audition and controls, speech suppression/model
   choices and higher-order/custom-layout Ambisonics. AUD131 fractional/signed
   SOFA support and AUD132 above-512 HR timing have scoped implementation and
   independent acceptance; the latest coordinated workspace gate passes as
   recorded above. This does not establish feature parity or close the spatial
   quality gaps.
   Preserve the full objective; do not silently drop a gap because it is large.
4. **Establish independent quality evidence.** Beyond finite-output smoke tests,
   measure frequency/phase/gain laws, alias rejection, wanted-signal preservation,
   speech quality with suitable corpora, spatial/downmix/dialogue behavior,
   adaptive convergence, latency, sample clocks, EOS and automation timing.
5. **Complete integration and platform coverage.** Cover every relevant family
   through native CLAP/VST3/AU, FFI, bridge and host routes. Verify scalar and
   setter realtime behavior, parameter types, presets, transport, tails and
   native device transitions. Linux compile tests do not prove macOS/iOS runtime.
6. **Close crate inventory and final audit.** Each in-scope crate needs a current
   feature comparison, implemented disposition for missing parts, independent
   accuracy evidence where applicable, lifecycle/realtime coverage, and chain
   integration evidence. Re-run the relevant diagnostic binaries and full gates.
   Astra verifies every requirement before declaring the original goal complete.

## Existing blocked actions

These blocks predate this handoff. A model change is not authorization to bypass
them. Continue independent authorized work and keep their status visible.

- Host unequal-branch queue integration: `proposals/host-branch-queues.md` and
  `.patch`; automatic approval review rejected broad routing/buffering changes.
- Manager Stop/Play/output-format/bypass protocol:
  `proposals/engine-transitions.md`; automatic review rejected broader production
  concurrency integration. Isolated verified fixes remain in the live source.
- External GitHub issue publication was rejected. Use the existing local issue
  ledger; do not publish, push, merge or send external messages from this handoff.
- Native hardware/platform execution and authentic external corpora require
  available environments/data. Record missing evidence; never infer a pass.

## Workspace and verification instructions

- Preserve the existing dirty worktree and all concurrent MIDI/IAMF work. Do
  not reset, revert, stage or commit unrelated files. `.git` is read-only here.
- Read and apply `sotf-workflow`, `sotf-plugin-host-dsp`, `ms-rust` and relevant
  DSP/QA skills. Use TokenSave first and check index freshness; use actual file
  contents to verify stale graph results. Read files with the harness before edits.
- The root `target` is a broken symlink to a deleted external MBX cache. Use:

  ```sh
  export CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target
  export TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp
  ```

- Large artifacts belong in that ignored workspace target; `/tmp` has little
  free space. Short logs may use `/tmp`. Do not rebuild other projects' caches.
- Focused commands:

  ```sh
  cargo test --offline -p sotf-host --test true_peak_rates
  cargo test --offline -p sotf-host
  cargo clippy --offline -p sotf-host --all-targets -- -D warnings
  CARGO_NET_OFFLINE=true cargo nextest run --offline --workspace \
    --exclude sotf-midi --exclude sotf-iamf --no-fail-fast --status-level fail
  ```

- FFI's nested Cargo invocation requires inherited `CARGO_NET_OFFLINE=true`.
  Standalone math-dsp tests use its manifest and its own surviving target.
- Most recent formatting helper: `/tmp/sotf-audit-wave19-format-check.py`.
  It excludes user MIDI/IAMF and vendored trees; check edited vendored files
  separately. Avoid simultaneous Cargo jobs sharing a target directory.
- No Cargo process was live at the historical paused handoff. Recheck live handles/processes after
  interruption before restarting anything.

## Review handoff template

Luna supplies: issue and requirement checklist; changed paths; before/after
evidence; independent oracles and tolerances; exact commands/results; realtime
and CPU measurements; remaining limitations. Astra returns a pass or prioritized
findings with file locations and concrete required checks. Luna fixes findings,
records the correction, and requests another review. Repeat until supported by
actual evidence, then move to the next batch in this full plan.


### Actual engine update gate follow-up, 2026-09-30

Luna BandSplit's real ProcessingThread fixture primes live and synchronized
Gain→Delay histories, then submits an outer-valid typed order-seven candidate
with an order-one native opaque blob. Its first runtime gate failed before the
intended native component restore: the engine factory validated scanned four
inputs against 64 live inputs before applying saved typed audio setup. Exact
red log: `artifacts/aud135-engine-update-r1/engine-test-r2.log`. Worker build
passed; no successful native candidate retry or EOS follows from this run.
Luna is correcting the narrow typed-setup-aware factory path while retaining
truthful scanned descriptor metadata. Root additionally requires complete
nonzero post-commit decoder output and exact EOS/drain accounting; finite
shaped first frames alone would not prove applied native audio.


### Engine factory repair and remaining isolated EOS refusal, 2026-09-30

The first post-factory gate omitted the required native bundle environment and
failed before runtime; preserve that as environment preflight, not DSP evidence.
With the preserved VST3 bundle explicitly set, the actual engine test progresses
through native candidate refusal, retained Gain→Delay history and valid retry.
It then fails receiving true EOS because the output channel disconnects. Log:
`artifacts/aud135-engine-update-r2/engine-factory-r2.log`; terminal exit 101.
There is no passing end-to-end engine/EOS claim.

The subsequent event diagnostic confirms the first drain preflight refusal:
`ProcessingError("channel-changing drain requires explicit identity frame geometry at 'plugin_0'")`.
The diagnostic gate exits 101; its exact log is
`artifacts/aud135-engine-eos-route-r1/engine-eos-event-diagnostic-r1.log`.
Isolated external plugins inherited conservative false identity-frame geometry
and unknown tail defaults. Channel-changing host drain requires explicit identity
geometry and finite tail metadata. A drain error sends a ProcessingError event
and stops the processing thread. Luna is implementing truthful native tail and
worker geometry metadata plus bounded EOS draining. The proxy retains audio on a fixed 8,192-frame
future timeline. Tail/drain delivery must preserve this pending transport audio
and truthful worker DSP tail metadata; setting a zero tail or bypassing host
validation would not satisfy the required complete waveform/EOF accounting.


Root review of the draft drain implementation additionally found three
contracts that must close before acceptance: failures reached through early
`?` returns must latch the mutated drain failure state; reset must clear native
DSP history as well as transport buffers; and a cached initial tail bound must
not truncate tails extended by later controls/automation. Luna is correcting
these before the next complete engine gate. The new CLAP mapping also handled
only `u32::MAX`, whereas the [official CLAP tail header](https://github.com/free-audio/clap/blob/main/include/clap/ext/tail.h)
defines every value at or above `INT32_MAX` as infinite. Boundary coverage was
requested. These are source-review findings on the current draft, not executed
red audio tests or Astra verdicts.

### Frozen DynamicEQ native callback handoff, 2026-09-30

Root preserved Luna's target-directory packet byte-for-byte in
`artifacts/aud139-native-clap-scalar-checkpoint-r1/`. The independent outer
61-entry checksum index is
`5b0ed123c12fe9f3f197a7b99f8e877835516550140b10e231621590ad9e135a`.
All 50 actual selected-source entries and all seven log entries match. The
original inner source manifest includes one stale checksum for itself; that
manifest remains unchanged and the root verification receipt records the
defect. The outer index hashes the copied manifest bytes and excludes itself.
This packet is archived after the tests/lint with no intervening source edits
reported by the owner; it does not claim a pre-run manifest, build-bound
executable or complete transitive closure.

Restarting the previous Astra reviewer and attempting one fresh Astra medium
reviewer both failed with `agent thread limit reached`, despite the completed
Luna checkpoint freeing a concurrency slot. Do not interpret this as review
acceptance. The checkpoint is ready for the next available Astra medium review;
VST3 implementation remains after that review. Two existing Luna lanes continue
engine/native routing work, so the full goal is not blocked and no user approval
is pending. Do not repeat unchanged reviewer dispatch attempts.

### Controlled DynamicEQ Peak CPU comparison, 2026-09-30

Root rebuilt recovered pre-edit and current DynamicEQ sources with the exact
same archived benchmark, compiler, release profile and current dependencies.
Final offline/locked metadata agrees apart from the two variant package names;
475 selected local build inputs match before/after both real compilations.
Both variants use the current sibling math path dependencies, so this is a
controlled source comparison rather than a recreation of historical dependency
versions. The first attempt reused one executable because package identities
matched; it was rejected before timing. Distinct package names then produced
fresh library/test compilations and different preserved executable hashes.
The new temporary lock needed one offline normalization before its build;
the repository lock was unchanged and final metadata is locked for both.

All four ABBA runs passed on CPU 127. Twelve cases cover 2/8 channels,
4/8 bands and 64/256/1024 callback frames at 48 kHz. Each variant/case retains
14 measured trials of 65,536 frames, with seven trials from each run.
Concatenated median new/old ratios are **0.989306–1.060697**; all twelve are
within the accepted **1.10** Peak budget. Paired run ratios and every raw trial
are retained rather than hidden by aggregation. This measures Peak processing
only, excluding shelf processing, native parameter synchronization, setup/reset
and worst-case execution time.

Durable packet: `artifacts/aud139-controlled-peak-cpu-r1/`; 38-entry outer
checksum index `5d3420430d9eff8e3851531d243fcd9e5030962e8b8b0732314c4e2af74853c0`.
It includes frozen variant source bytes, compiler/build and final dependency
records, start/end selected-input hashes, actual timing receipt/logs and the
reproducible analysis tool. Not all historical dependency source bytes are
archived; this is not a complete transitive-source archive. Executables are
preserved separately under the matching target audit-artifact directory.
Matched LowShelf/HighShelf versus post-edit Peak CPU remains open; Luna Upmixer
has resumed that manual benchmark step without production edits. Astra review
of this comparison and the frozen native checkpoint remains pending.

### Crossover typed engine route closing gates, 2026-09-30

The explicit Bands/PerChannel topology and optional channel-mode configuration
now reach typed engine settings, conversion, factory validation and output-width
propagation. Root caught a null optional-array serialization defect before the
focused gate; absent arrays now remain omitted and the minimal legacy settings
case reaches the factory. The final focused engine gate passes **7/7**, including
all thirteen family roundtrips, mode-dependent widths, invalid candidates and
a complete 257-frame stereo Crossover→BandMerge summed waveform.

The existing engine output-width group passes **17/17**. Its combined Clippy
command initially failed two helper style lints; equivalent saturating-multiply
and multiple-of idioms corrected those findings. Final full Crossover package
tests pass **117**, with two manual captures ignored, and strict engine Clippy
passes for all targets. Logs and nine selected source files are frozen in
`artifacts/aud142-engine-route-r1/`, outer index
`8144fbc733a60cf817e9bb921e2b48d7ea0a5bdc8cb3f01a5e434a2cc1c67b99`.
Sources were archived after gates with owner-reported no intervening edits;
there was no pre-run selected-source manifest or fully bound executable.
These results do not imply whole-workspace or Astra implementation acceptance.
Luna Crossover is continuing native schema/bus delivery; app UI and separate
core/CPU review remain open.

### Crossover native scalar automation findings, 2026-09-30

Root review of the draft fixed native schema found that active IIR cutoff
setters rebuild allocated parameter metadata. The existing Crossover realtime
fixture guards processing/reset, so its green result does not cover changed
native cutoff synchronization. Luna is updating cached numeric values in place
and adding a cold changed-control guard with actual audio processing. FIR
cutoff reconstruction remains a structural preparation operation.

The draft dormant-control skip checked only a missing runtime getter. The core
still returns a global frequency in PerChannel mode even though that active
runtime parameter is absent; a dormant edit can therefore reach an invalid
scalar setter. Luna is gating synchronization against the prepared/live route,
retaining dormant saved controls. The pending topology choice is insufficient
because the old route must continue correctly while reactivation is deferred.
These are source findings, not executed red results or Astra verdicts. The
117-test/engine packet predates these further native-control corrections.

### Matched shelf CPU and Astra DynamicEQ verdict, 2026-09-30

The actual release benchmark session 83390 exits zero on CPU 8, with all 36
shape/case rows and seven raw trials per row. Root independently parsed all
reported medians and ratios: **24/24** LowShelf/HighShelf comparisons meet the
1.25 budget. LowShelf/Peak spans **0.736274–0.841800** and HighShelf/Peak spans
**0.792080–0.925338**. Timing covers the same linked-channel alternating boost/cut
workload at 48 kHz across 2/8 channels, 4/8 bands and 64/256/1024 callback frames.
Each trial processes 65,536 frames; input, reset and preparation are excluded.
This is not an arbitrary-workload, native synchronization, setup-cost or WCET
claim. The shell wrapper's nonfatal printf diagnostic is preserved; actual
executable completion and every row are present.

Root verified all 462 original packet entries and all 451 copied selected
source hashes, then preserved a compressed durable copy at
`artifacts/aud139-matched-shelf-cpu-r1/`; 18-entry outer index
`d7e38fbcb1dd087c8d500a3fb4a7b19110b543ceae12602e638f4e0d667456eb`.
Selected sources are stable across timing but captured after the build; there
is no pre/post compile binding claim. Original receipt ratio fields named
old_to_new actually contain the controlled **new/old** ratios; root's receipt
qualifies the names without changing the original bytes. The original copied
binary remains hash-bound in the target packet. Initial dependency Clippy
failed two concurrent host draft style lints; subsequent scoped DynamicEQ
`--no-deps` release lint and the actual release build passed.

Astra independently accepted the frozen DynamicEQ scalar and in-process CLAP
callback lifecycle plus qualified Peak/shelf CPU evidence in
`reviews/AUD139-astra.md`. It explicitly distinguishes actual Rust-created CLAP
callbacks from an externally loaded packaged binary or SOTF consuming host, and
retained requested values after failed deactivated preparation from live audio
continuity. VST3, consuming host/UI, AU and broader full-state routes remain
open. Luna Upmixer has resumed the VST3 slice; no additional CPU runs are needed.

### Crossover scalar/cache checkpoint and core re-review requests, 2026-09-30

The executed core allocation guard first measured **39 allocation events**,
then passed after valid active IIR cutoff setters used existing cached metadata
for validation and updated numeric default values in place. Six native schema
and synchronization tests pass, including original linear frequency mapping,
active/dormant routing through the real native constructor, retained state and
exact per-channel audio twins. Historical compilation/assertion failures remain
preserved. The final native r5 four-file start/end source manifests match;
core/params production hashes also match from the green core r3 through r5.

Packet: `artifacts/aud142-native-scalar-cache-r1/`, 21-entry outer index
`451e3c02c0f2df90e6898271515d61045b8c17f419ea623124fb61190de1f8e6`.
Three supporting schema/config/module files are post-gate snapshots; wrapper
ABI/bus work is excluded. Full package/lint and actual native callback delivery
still need closing gates after these production changes.

Astra's frozen numerical/core review found no demonstrated production defect,
but final acceptance needs exact upper-frequency/rate-boundary coverage and
broader odd-order BW42/compensated multiway scalar automation. Per-channel
coverage must use its actual structural construction/refusal/reinitialization
contract, not private forced live updates. Original 0.002 response/waveform,
2e-5 partition and input-relative peak gates remain unchanged. See
`reviews/AUD142-astra.md`. Luna Crossover is adding these focused checks before
resuming the native bus route. Existing CPU records are accepted as qualified
descriptive measurements given changing machine load and sample CV; Astra does
not require a new timing run merely to retain that bounded evidence.

### Focused Crossover and engine EOS follow-up, 2026-09-30

Crossover's first focused final-source run passes **10/10**, strict test-target
Clippy and formatting. Source SHA `c672aeecf99058c38e84c7cb161a0fe679ffd912ac5dd7e0a2a02598eebddfea`;
test log `/tmp/aud142-core-focused-r3.log`, SHA
`535af5d8d13d14f28c51aa98a58013687a7086020ce1beafbccf1ff0dd46d088`;
strict lint log `/tmp/aud142-core-focused-clippy-r2.log`, SHA
`f328e62f5b3b381acde7dd50f2e4c3bedfe837d824aae453f90a185923815cb4`.
The accepted sample-rate cutoff is **strictly below** `0.495*Fs`: at 8 kHz,
exact 3960 Hz refuses; the admitted test uses the preceding representable f32.
The earlier Astra example implying exact 3960 Hz admission needs correction in
its subsequent review. Production admission and numerical limits are unchanged.

Root's source review found BW42 tested only the rising sweep and per-channel
twins used the same irregular partitions. Luna is adding the reverse BW42 sweep
and independent regular/irregular per-channel comparisons with the existing
peak/partition limits. The ten-test result proves its actual cases, not these
pending additions or final Astra acceptance. Both existing Astra follow-up and
fresh Astra medium dispatch currently return `agent thread limit reached`;
implementation and receipt work continue while the next review slot is sought.

The actual engine fixture now sends a maximum-size final 8192-frame worker
request followed immediately by true EOS. After rebuilding the VST3-enabled
worker (SHA `cf845d1db6a2b295444cd6c29e165462518e4483ff6a0475d58e618a2b4dfd53`),
the run reaches the expected late refusal and valid retry, then fails with
`channel-changing drain requires finite tail metadata at 'plugin_0'`.
Log: `artifacts/aud135-engine-eos-route-r2/engine-eos-pending-red-r3.log`.
This confirms preflight reads metadata before pending work is resolved. Luna
is adding bounded EOS/control-side metadata preparation before that preflight;
ordinary callback processing remains nonblocking. Complete native-reference
audio, residual/count/EOS checks and final corrected sentinel gates are pending.

Native tail sentinel contracts differ: CLAP treats values at or above signed
INT32_MAX as infinite ([CLAP tail extension](https://github.com/free-audio/clap/blob/main/include/clap/ext/tail.h));
VST3 reserves UINT32_MAX, so 0x7fffffff and 0x80000000 remain finite
([processor interface](https://github.com/steinbergmedia/vst3_pluginterfaces/blob/master/vst/ivstaudioprocessor.h),
[base integer limits](https://github.com/steinbergmedia/vst3_pluginterfaces/blob/master/base/ftypes.h)).
Luna corrected the VST3 draft after primary-source verification. The earlier
two-test sentinel pass used the incorrect VST3 expectation and is superseded;
the corrected mapping needs an executed focused gate.

DynamicEQ VST3 restart delivery is active. Linux editor-closed UI dispatch
requires a host-provided run loop through factory host context; NIH's current
factory ignores that route. Luna is implementing the factory-context/run-loop
handoff and actual callback tests with no worker-thread restart fallback,
following the [official Linux run-loop contract](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Provide%2BA%2BRunloop%2BOn%2BLinux/Index.html).
This is draft work, not a passing VST3 or consuming-host result.

### Final focused Crossover evidence packet, 2026-09-30

Luna added the missing BW42 **20 kHz→1 kHz** sweep at frame 4093 and regular/
irregular per-channel output comparisons for both opposite extreme
configurations. Final-source r4 again passes **10/10** and strict target
Clippy r3 passes; root inspected the additions and actual terminal logs.
Source SHA `a940291a62b8d0a14efdf6a38fef8fe9a6004bd54f3e8a2ac44ec08c7d781bfb`;
test log SHA `61757da35ef1ae61472dcac352b9b68fe18ed5c16e9b59c989ee12593c1f2c8d`;
lint log SHA `045a09aa291c63d511a34f195efc7e8f75faa3a26df949ad2719ed400b1593f8`.
No production code changed in these focused additions.

Root sealed `artifacts/aud142-core-focused-r1/`: eleven-entry index SHA
`a4af23cd789b9f6ca0a72f8d49424bd84dcafbb9e956548db8b2541b9666704c`,
seven selected-source archive SHA
`497652cf5fdef30601fd7ecdbf7caf50a1a4d7f703fd08589597fb34e6187707`.
Selected sources are a post-gate snapshot, stable across copying, with the
final test source matching the owner-reported tested hash. There is no complete
pre-build closure or executable binding. Prior compile/style failures and
intermediate green runs are preserved. After this checkpoint freed a slot,
Astra medium dispatch succeeded; bounded independent re-review is active.
The earlier dispatch failures remain recorded but no longer block this review.

Root additionally verified the VST3 SDK marks `getTailSamples` as UI-thread/
setup-complete. The draft backend directly invokes it through `tail_length`,
which is also used after worker processing and from host metadata paths. Luna
is checking cached/control-side query ownership before closing that route.
CLAP explicitly permits its tail query on main/audio threads and must retain
its separate behavior. The sentinel correction alone is not evidence of thread
contract compliance.

Astra medium independently verified the eleven packet checksums, inspected
the final source and terminal logs, and **accepted the bounded core additions**
in `reviews/AUD142-astra.md`. The strict cutoff example is corrected there.
CPU evidence keeps its documented workload/noise qualifications. Later shared
native/engine and full consuming UI routes are outside this acceptance.

Luna Crossover has resumed product UI and consuming app work with explicit
ownership separation from Upmixer's shared NIH/VST3 framework. It will inspect
the current mounted family/topology/cutoff/per-channel controls and persistence
path, implement missing routes against the accepted design, and verify actual
settings-to-engine audio. Native buses and callbacks remain required after the
shared wrapper ownership is released. User authorization covers sibling `sotf`
edits; existing worktree edits and lockfiles must be preserved.

### Draft EOS and VST3 contract corrections, 2026-09-30

Luna added `prepare_drain_metadata` with a safe default and adapter forwarding,
placing channel-changing metadata preparation after destination capacity
checks. Root review also found the isolated drain used `context.num_frames`
as output capacity even though DawHost passes zero source frames at EOS.
Luna changed it to use the declared output capacity. Repeated host drain calls
revalidate the route, so an active isolated drain must not be rejected as a
new preparation; completed/prepared stages need lifecycle-aware handling while
downstream stages still refresh after receiving new upstream tail input.
Luna is correcting this and adding a gated pending-worker multi-call case.
These draft source changes have no final host/engine gate yet.

Root inspected the VST3 restart draft and requested three concrete fixes:
retain Windows/macOS UI dispatch by restricting the worker-fallback refusal
to Linux; protect factory COM host-context lifetime across clone/replacement;
and handle a host run loop refusing registration without aborting exported
instance creation. Actual editor-closed callbacks must cover supported and
unsupported/refusing run loops, retain pending controls and retry correctly.
Luna is implementing these before its focused native gate. They are source
review findings, not executed callback results or Astra acceptance.

The next frozen BandSplit native-buffer review handoff is ready in
`handoffs/aud143-native-buffer-review.md`. Root reverified all twelve packet
entries against the existing index `33c0b4ec…`. It awaits an Astra medium slot;
subsequent shared wrapper changes require their own coherent integration gate.

Root's next review identified an actual engine-fixture weakness: the maximum
final worker block was 8192 frames of silence. In the single-band Ambisonics
matrix route, that flushes all preceding nonzero program before EOS, leaving
an all-zero residual. Exact EOS counts and whole-vector equality would therefore
miss silencing of retained EOS output. Luna is retaining a separate ordinary
silence case and adding a nonzero maximum-size final block immediately before
EOS, with a nonzero drain assertion and the unchanged full-reference bounds.
The concrete review checklist is `handoffs/aud135-engine-eos-review.md`.

The VST3 draft now reads cached tail metadata and exposes an explicit worker
control-side refresh. Root requested follow-up for valid in-process state-load
refresh, preservation after rejected scalar input, and native tail-change
notification/invalidation. No executed cache/threading or corrected engine
acceptance is claimed from these source changes.
