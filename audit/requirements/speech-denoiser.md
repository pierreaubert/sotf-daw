# Speech Denoiser: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse: `speech-denoiser`** (claimed in README; worker session `01a0f63f-d2dc-75c2-95f6-7ef08876bbbd`). Work record: [progress](../muse-parallel-2026-10-01/speech-denoiser/progress.md), [result](../muse-parallel-2026-10-01/speech-denoiser/result.md). Independent review: [review.md](../muse-parallel-2026-10-01/speech-denoiser/review.md); fix rounds: [fix-r1-result.md](../muse-parallel-2026-10-01/speech-denoiser/fix-r1-result.md), [fix-r2-result.md](../muse-parallel-2026-10-01/speech-denoiser/fix-r2-result.md).

- Package: `sotf-plugin-speech-denoiser`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-speech-denoiser](../../crates/sotf-plugins/crates/sotf-plugin-speech-denoiser).
- Coordination group: **Restoration/shared inference**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Current follow-up execution (2026-10-02, R4)

This addendum supersedes the earlier open compile/loader/metadata/invalid-scalar status while preserving historical reports below.

- Loader tests execute all 25 cases successfully, including real alternate assets. Backend/Speech/FFI/engine focused tests and strict lints pass. Standalone NN global lint still has 37 verified baseline production diagnostics; global formatting drift remains.
- Both NIH feature suites pass 203 unit tests plus 3 auxiliary tests, with one existing ignore, and strict all-target lint. Host passes 605 tests with two existing ignores and strict lint with both external formats enabled.
- Fresh foreign C ABI passes 7/7; 32 matched getter/setter calls measure zero allocations/frees. Fresh native metadata passes, and loaded Speech CLAP audio passes 14/14 including rejected invalid-scalar continuation. [R4 actual execution](../continuation-2026-10-01/speech-native-audio-r4/execution-receipts.json), [audio receipt](../continuation-2026-10-01/speech-native-audio-r4/receipt.json).
- Custom and original named order-7 Ambisonics loaded CLAP/VST3 suites also pass against fresh sealed artifacts, exercising the repaired shared native host lifecycle. Independent review of these host changes remains pending. [Loaded results](../continuation-2026-10-01/ambisonics-native-custom/approved-r4-loaded-gates/gate-results.json).
- All six reviewed NN production files now match the sibling SOTF fork exactly; all 15 original weight and bias arrays are preserved. [Sync verification](../continuation-2026-10-01/approved-muse-repairs-r1/sibling-fork-sync/root-source-verification.json).

R1–R4 and A1–A3 remain open for the unverified engine/VST3/AU/whole-chain/platform and original harsh quality requirements. Corrected upstream corpus/intelligibility results and individual regressions remain recorded; positive averages do not establish full quality acceptance.

## Latest approved repair execution (2026-10-02)

User authorized sharing retained findings with Muse; original owners repaired loader/backend, FFI labels/realtime guards, and NIH native controls/preflight. [Actual root receipt summary](../continuation-2026-10-01/approved-muse-repairs-r1/root-r1-results.md).

- Fresh stable backend, Speech, FFI and engine tests/lints plus six model/layout realtime QA passed. Loader malformed-token fixture and four native test sample-rate type errors remain open; standalone fork lint has37pre-existing production diagnostics (baseline40), and formatting drift remains. Independent Muse reviews are running.
- Fresh foreign CABI passed7/7; model labels are canonical and32matched setter/getter calls measured0alloc/0free, including rejected model changes. Fresh CLAP metadata passed allchecks; native audio passed14/14, including failedmodel99/strength1.5 restores preserving state and populated continuation. [Native actualreceipt](../continuation-2026-10-01/speech-native-audio-r2/receipt.json), [FFI actualcases](../continuation-2026-10-01/speech-ffi-r2/c-abi-results.json).
- All824original pairs ×3models were compared with unmodified upstream C on the fresh corrected artifact: FullmeanSI-SDRdelta+4.429661dB (every historical Full per-file score unchanged), LQ+2.386604dB, SH+3.239056dB. [Fresh reference receipt](../continuation-2026-10-01/speech-upstream-inference-r2/full-receipt.json). Individual waveform differences remain recorded, with no invented acceptance tolerance. Upstream intelligibility regressions and original harsh cases remain open.

These results supersede the earlier defect status below while preserving historical failures. R1-R4 and A1-A3 remain unchecked pending remaining execution/review and whole-chain/native/platform/quality scope. No goal closure.

## Existing implementation to preserve

**Latest executed evidence (2026-10-02, real models):** current speech source passed **81 tests, 3 pre-existing ignores**, including all 6 new per-model audio tests, and release QA passed **all 3 models × mono/stereo** with zero cold allocations (maximum callback 302.843µs versus a 10.666667ms deadline). Alternate models change actual audio. Backend/loader tests and all-target lint are blocked by specific compilation/lint defects; independent review also found incorrect asset hashes/sizes and public-model contract issues. See [actual findings](../continuation-2026-10-01/speech-real-models/root-r1-gate-findings.md) and [independent review](../continuation-2026-10-01/speech-real-models/review-source-r1.md). All 15 builtin weight/bias arrays remain identical. The older passing clippy receipt below predates these new source changes.

**Corpus resource resolved:** all **824 official VoiceBank/DEMAND test pairs** were staged unchanged with publisher CC BY 4.0 provenance and verified MD5/SHA256. The original manual corpus gate executed on every pair: mean SI-SDR improvement **+4.43dB**, with **156 worsened pairs**. That gate asserts no quality bound, so R4/A1 remain incomplete. [Provenance and results](../continuation-2026-10-01/speech-corpus-r1/result-r1.md). No upstream model training-overlap claim is made. Original harsh cases, independent references, engine/native/FFI/app integration and supported platform evidence remain open.

RNNoise at 48 kHz, linked stereo gains/VAD and the accepted 960-frame queued-program EOF policy exist. Timing acceptance does not establish model quality.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **SPEECH-DENOISER-R1** — IMPLEMENT: Suppression-strength control with a defined gain/residual blend and stable timing. PARTIAL: blend implemented in owned crate (`out = dry + s·(wet−dry)` over wrapper 960-frame dry delay, 480-frame slew, bit-exact 0/1 endpoints, constant 960 latency; `src/lib.rs`, `tests/strength_model.rs`) plus fix-round transactional batch apply (`precheck_value_ref`) and cross-partition slew oracles. All 15 strength/model tests now executed successfully on current plugin source; source review supports the plugin control contract. Remaining: shared backend/loader fixes, independent final review and full integration evidence ([fix-r1-result](../muse-parallel-2026-10-01/speech-denoiser/fix-r1-result.md), [verification-request](../muse-parallel-2026-10-01/speech-denoiser/verification-request.md)).
- [ ] **SPEECH-DENOISER-R2** — IMPLEMENT: Model selection with validation/preparation off the callback, atomic adoption and old-model continuation on failure. PARTIAL CURRENT SOURCE: checked `.rnnn` loading, Cow-owned states, backend prepare/commit and three-entry real registry are implemented; actual C ABI proves distinct audio from all three models. REQUIRED P0: the loader consumes bias before input/recurrent weights, contrary to upstream format. Correct transpose oracles could not execute due compile blockers. Fix parser order, backend/loader compilation/lint, asset identities and public model contracts, then compare a fresh artifact with unchanged upstream C and obtain independent final review. [Specific causal issue](../continuation-2026-10-01/speech-upstream-inference-r1/loader-order-issue.md), [earlier proposal/history](../muse-parallel-2026-10-01/speech-denoiser/shared-patch-multimodel-backend.md).
- [ ] **SPEECH-DENOISER-R3** — INTEGRATE: Persist model identity/configuration and expose strength/model controls without placing model loading on the audio thread. PARTIAL: plugin-surface persistence exists (factory params, schema v2 + v1 migration, layout, DawHost chain). CURRENT-SOURCE CORRECTION (2026-10-02): engine settings already have strength/model with serde defaults, and `convert_speech_denoiser` already forwards enabled/strength/model; both are in HEAD. The earlier enabled-only converter claim and proposed engine diff are stale; do not reapply that proposal. Existing facade construction and NIH scalar/default-sync tests now execute successfully on stable source. A fresh loaded CLAP export confirms Model is hidden/read-only with numeric0/1/2labels ([actual native receipt](../continuation-2026-10-01/speech-native-metadata-r1/result-r1.md)). Actual loaded CLAP now proves nine valid model/strength state/audio roundtrips and rejects unsupported rates, but invalid model/strength loads mutate native state and silence accepted audio ([native restore failure](../continuation-2026-10-01/speech-native-audio-r1/result-r1.md)). Remaining: failure-atomic native restore, reachable native/FFI controls, actual engine/VST3/AU save/reload audio and whole-chain proof, with the confirmed FFI metadata/rejection fixes. [Current consumer evidence](../continuation-2026-10-01/speech-consumer-r1/result-r1.md).
- [ ] **SPEECH-DENOISER-R4** — VERIFY: Held-out speech/noise quality using permitted datasets and documented licensing; preserve the supported-rate contract. PARTIAL: 48kHz-only contract tested; all824 unchanged licensed real clean/noisy pairs now measured through all3models at strengths0/0.5/1, both actual C ABI production and correct pinned upstream C, using SI-SDR and classic STOI. Training overlap remains unknown; official test split does not prove every model was trained without it. Current production legacy outputs are corrupted by the confirmed loader order defect. Correct upstream legacy full wet still lowers mean STOI despite positive SI-SDR; record both metrics and individual regressions. Original full-scale white-noise, overlapping double-speaker, music/noise and justified quality acceptance remain OPEN. Later shaped-noise/sequential-turn tests do not satisfy the original harsh cases. [Full measurement](../continuation-2026-10-01/speech-intelligibility-r1/result-r1.md), [fix-round history](../muse-parallel-2026-10-01/speech-denoiser/fix-r2-result.md).

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **SPEECH-DENOISER-A1** — SI-SDR/STOI or justified equivalent on clean/noisy pairs, separate single/double-speaker/music/noise conditions and strength endpoints. PARTIAL EXECUTED: complete824-pair licensed corpus,18model/strength/route groups and14,832paired SI-SDR/STOI score records;2472bit-exact delayed-dry and2472exacthalfblend cases, stable2004+43+8seals. Published MIT pystoi0.4.1 pinned/unchanged; warnings and undefined clips reject. Full metrics, per-file/speaker regressions and limitations are retained. Original harsh synthetic conditions and justified intelligibility/quality acceptance remain OPEN; second independent STOI implementation and human listening not executed, training overlap unknown. Earlier deferral due absent corpus is superseded. [Executed receipt](../continuation-2026-10-01/speech-intelligibility-r1/result-r1.md).
- [ ] **SPEECH-DENOISER-A2** — Model/strength switching, stereo image, fresh restore, failed model loading and deterministic reference output for a fixed model. PARTIAL: oracles written in `tests/strength_model.rs` + `tests/accuracy.rs` (switching determinism, swap/image bounds, JSON/snapshot restore, rejection-with-continuation, twin determinism) plus fix-round batch-atomicity and cross-partition slew oracles; current plugin suites passed. UPDATE (real-model loader round, partially executed): per-model audio-difference, adoption/continuation, and lifecycle oracles added in `tests/model_audio.rs`, `plugins-denoiser/tests/rnnoise_models.rs`, and `nnnoiseless` loader tests. Remaining: backend/loader repair and execution plus independent reference/final review.
- [ ] **SPEECH-DENOISER-A3** — Retain accepted 480 terminal-phase tests and queued-program continuation; do not relabel recursive model response as a proven finite natural tail. PARTIAL: all accepted timing/finite-stream/host tests retained unmodified in behavior (only struct-literal/schema-count updates for appended params); enabled `TailLength::Unknown` kept; no finite-tail claim added. Current timing/finite/host binaries executed successfully, with the 3 pre-existing ignores disclosed; no finite natural-tail claim was added. Whole-chain final delivery remains part of the integration gate.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

48 kHz typed input → model preparation/inference → SpeechDenoiser → host finalization/export; unsupported rates must be explicitly handled.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-speech-denoiser --lib --tests
cargo clippy --offline --locked -p sotf-plugin-speech-denoiser --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-speech-denoiser`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent review recorded in [review.md](../muse-parallel-2026-10-01/speech-denoiser/review.md) (Muse review replaced the Astra gate per user instruction); owned findings fixed in [fix-r1-result.md](../muse-parallel-2026-10-01/speech-denoiser/fix-r1-result.md) and [fix-r2-result.md](../muse-parallel-2026-10-01/speech-denoiser/fix-r2-result.md), awaiting coordinator rerun. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/reviews/AUD136-astra.md](../../audit/reviews/AUD136-astra.md)
- [audit/speech-denoiser-enabled-accepted-queue.md](../../audit/speech-denoiser-enabled-accepted-queue.md)
- [audit/speech-denoiser-latency.md](../../audit/speech-denoiser-latency.md)
- [crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/README.md](../../crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Original quality cases remain open

The coordinator retained exact-binary numerical evidence in [measurements](../../audit/muse-parallel-2026-10-01/measurements/README.md). Later green accuracy tests use a shaped-noise case and sequential turn-taking; they do not close the originally falsifying full-scale white-noise and overlapping-speaker cases. The latter degrades from −5.11 to −11.43 dB SI-SDR at full strength. Characterization-only labels record evidence, not requirement completion. R4/A1 and multi-model R2 stay open pending broader accuracy, suitable model/backend work and corpus validation.

## Author handoff on 2026-10-01 (superseded by latest execution above)

Author session `01a0f63f` implemented real checked loading of the two staged
legacy alternates within the newly assigned ownership (speech crate +
nnnoiseless owned-loader files + `plugins-denoiser/src/rnnoise.rs`). Fork
weight storage is `Cow<'static, [i8]>` (builtin stays borrowed and
bit-exact; loaded models owned by value, no lifetimes/unsafe/leak);
`parse_rnnn_model` strictly validates v1 (exact header/dims/activations,
i8 range, truncation vs trailing data) and transposes once off-callback;
the backend exposes registry + `prepare_model(&self)`/`commit_prepared` +
transactional `initialize_with_model`; the plugin registry is `RNNoise
Full` (0, EXACT) + `RNNoise Legacy LQ` (1) + `RNNoise Legacy SH` (2) with
loading in `initialize()` only and unchanged structural setter semantics.
Plan, API contract, and file list:
[issue-plan-r1.md](../continuation-2026-10-01/speech-real-models/issue-plan-r1.md);
outcome and unexecuted gates:
[result-r1.md](../continuation-2026-10-01/speech-real-models/result-r1.md),
[UNEXECUTED-verification-request-r1.md](../continuation-2026-10-01/speech-real-models/UNEXECUTED-verification-request-r1.md).
The author executed no commands (shell sandbox unavailable). Root subsequently executed the package, release and corpus gates listed above. R2/A2 stay unchecked pending the identified repairs, remaining gates and independent final review. The earlier engine converter implementation gap is superseded by the current-source R3 correction above; actual engine save/reload audio remains unverified.


## Current C ABI and full per-model corpus evidence (2026-10-02)

Root built the current Linux C ABI artifact from stable source and called it from a foreign Python/ctypes caller. Six cases passed: all three models produce different nonzero audio; all models preserve bit-exact strength0/960-frame dry delay in mono/stereo; nine model/strength states restore with identical audio across partitions; malformed states preserve populated-history continuation; unsupported rates reject; live model changes reject without altering state. One case failed: the C ABI model choice-label getter returns NULL for all three choices. Separate matched-control allocation measurement also confirms that changed-model rejection allocates (first5alloc/3free, then4/4), while getter/strength/same-model controls measured0/0. Model metadata and allocation-free structural guards remain required FFI implementation. [Executed C ABI receipt](../continuation-2026-10-01/speech-ffi-r1/result-r1.md).

All824 original pairs were rendered through each model at full strength, with independently recomputed f64 SI-SDR and fixed latency/warmup windows. Bundled mean **+4.429661dB**, LQ **-2.079648dB**, SH **-2.203592dB**; worsened pairs157/485/538. Full precision resolves the two rounded bundled ties from the earlier report. Every bundled per-file delta agrees with the original Rust gate's printed measurement within the predeclared0.006dB precision bound (worst0.004998dB). The legacy models currently worsen this corpus on average. Verify against genuine upstream inference to distinguish model quality/domain from integration errors, then satisfy the original quality requirements. No quality acceptance or natural-EOF/platform claim is made. R2/R3/R4 and A1/A2 remain open.


## Independent upstream inference identifies the loader defect (2026-10-02)

All 824 original pairs were compared with the unmodified pinned upstream C frontend and inference, after proving exact identity of all 15 arrays and all six layer activations/dimensions for each model. Correct upstream mean SI-SDR deltas are Full **+4.429661 dB**, LQ **+2.386604 dB**, SH **+3.239056 dB**. Current production legacy deltas remain **−2.079648 / −2.203592 dB**. The earlier negative legacy measurements describe a broken loader, not the intended model quality.

**Required P0:** current `model_load.rs` reads bias before input/recurrent weights, contrary to the actual file format. An independent C intervention with that wrong order reproduces production output within **0.000000522** sample amplitude on the fixed 16-pair diagnostic selection. Existing transpose oracles use the correct order and must be preserved; they did not execute because the new tests fail to compile. The source review missed this mismatch. [Full executed reference receipt](../continuation-2026-10-01/speech-upstream-inference-r1/result-r1.md), [specific repair issue](../continuation-2026-10-01/speech-upstream-inference-r1/loader-order-issue.md), [exact compiled-upstream array identities](../continuation-2026-10-01/speech-upstream-inference-r1/expected-neuron-major-arrays.json).

R2/R3/R4 and A1/A2 remain open. No production Rust was edited and no supplemental finding was sent to Muse while permission remains pending. Correct upstream positive averages do not satisfy all original quality, realtime, engine/native/UI, EOF, stereo or platform requirements.


## Complete strength and intelligibility corpus measurement (2026-10-02)

All824 original licensed pairs were measured at strengths0/0.5/1 for all3models through actual production C ABI and correct pinned upstream C inference:18groups,14,832paired records. Actual handle87814 exited0;2472bit-exact delayed-dry endpoints and2472exacthalfblend cases passed,2004+43+8source seals stable. [Full executed measurement](../continuation-2026-10-01/speech-intelligibility-r1/result-r1.md).

Bundled full-wet meanΔSTOI **+0.002436750**, meanΔSI-SDR **+4.429661dB**, yet444/824individual STOI regressions. Current broken-loader LQ/SH full-wet meanΔSTOI **−0.077733831/−0.054641978**, with821/822regressions. Correct upstream LQ/SH full-wet meanΔSTOI **−0.007426567/−0.006122401** despite positive SI-SDR; at half strength upstream meanΔSTOI is+0.002872808/+0.003834098. The loader defect and the remaining model-quality acceptance are separate findings.

R4/A1's missing complete model/strength corpus and objective intelligibility measurements now exist for this test set. The original harsh quality cases, justified acceptance, unknown training overlap, stereo/automation/whole-chain/platform evidence and independently reviewed implementation repairs remain open. Published MIT pystoi0.4.1 is pinned and unchanged; a second MATLAB/Octave metric implementation and human listening were not executed. No bounds/defaults/fixtures were weakened or changed. R1/R2/R3/R4 and A1/A2/A3 remain unchecked.


## Actual loaded Speech CLAP model control failure (2026-10-02)

The new release Speech export was built successfully and inspected through an actual foreign CLAP factory/plugin/params caller on stable2004-path source. Model ID104069929 has flags13 (stepped + **hidden + read-only**), and its three value-to-text labels are **0/1/2**. Enabled and Strength are visible/writable. Probe actually exits1:3PASS/3FAILmetadata checks. [Executed loaded receipt](../continuation-2026-10-01/speech-native-metadata-r1/result-r1.md), [required coordinated NIH repair](../continuation-2026-10-01/speech-native-metadata-r1/native-control-issue.md).

R3 now has a directly executed native control failure, separate from the stale engine converter claim corrected above. Native restart/restore/audio, VST3/AU/UI and whole-chain delivery remain unverified. Preserve stable IDs/defaults and off-callback preparation when exposing the manual model selector; a metadata-only patch cannot establish adoption/continuation.


## Actual loaded Speech CLAP save/reload audio and failed-state continuation (2026-10-02)

Foreign native lifecycle/state/process callbacks execute **12PASS/2FAIL** on the unchanged2004-path source and previously staged actual Speech export. All3models ×strengths0/0.5/1retain settings through native saved bytes and fresh restore. Stereo48kHz/24,517frames, latency960, processstatus1, sentinels/finite writes pass. Native output is bit-exact against same-source C ABI, and fresh native restore is bit-exact across512 versus1/137/8193partitions. All3zero-strength cases are exact delayed dry; all full-wet models produce different nonzero audio.44100/96000/192000 activations explicitly reject. MalformedJSON rejects without changing saved state or populated audio continuation. [Executed result](../continuation-2026-10-01/speech-native-audio-r1/result-r1.md).

**Required native restore failure:** model99 and strength1.5 loads returnfalse but persist the invalid parameter. Subsequent4096frame callbacks returnError0 and all-zeroaudio, max difference0.2261374295againstmatched accepted control. Initial failure and refined instrumentation receipts are preserved. Native preflight currently does not dispatch Speech; raw host parameters are written before later constructor/sync rejection. [Specific coordinated repair](../continuation-2026-10-01/speech-native-audio-r1/native-restore-issue.md). R3/A2remainOPEN.

The positive waveform comparison establishes native/CABI route consistency, not correct legacy weights; both production paths share the confirmed loader defect. Host model fixtures are explicit because Model is hidden/read-only; they do not prove reachable controls. Native mono, automation, audio-thread restore/allocation, VST3/AU/UI, actual engine, EOF/export and platform acceptance remainOPEN. Source inspection also finds the Speech audio-thread restore guard defaults totrue; this is a review risk, not an unsupported audio-thread CLAP call executed as a test.
