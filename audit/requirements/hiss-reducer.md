# Hiss Reducer: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-hiss-reducer`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-hiss-reducer](../../crates/sotf-plugins/crates/sotf-plugin-hiss-reducer).
- Coordination group: **Restoration/shared denoiser**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

**Latest current core execution (2026-10-02):** [root core report](../continuation-2026-10-01/hiss-core-quality/result-r3-core.md) records124 package tests passing with zero ignores, all-target clippy passing, and actual releaseQA passing for IIR/spectral latency0/1024, cold zero-allocation and performance diagnostics, on stable selected source. This clears the earlier host-compilation barrier for these core gates; original quiet-transient/tone quality, actual async application, exact sample accounting, loaded native and complete integration scope remain open.

Time-domain expansion and WOLA Wiener modes with tonal protection exist. Bounded clean-reference SNR/tone-loss and finite-stream evidence exists.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **HISS-REDUCER-R1** — IMPLEMENT: Noise-profile capture and persisted profile restoration.
- [ ] **HISS-REDUCER-R2** — IMPLEMENT: Per-frequency reduction curve and selectable channel linking.
- [ ] **HISS-REDUCER-R3** — INTEGRATE: Profile/curve/link controls with explicit mode applicability and compatible old defaults.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **HISS-REDUCER-A1** — Known broadband hiss plus clean tones/transients: separate suppression from wanted-signal loss; retain existing >2 dB SNR and <1 dB tone-loss cases.
- [ ] **HISS-REDUCER-A2** — Independent spectral unity-mask reconstruction and curve interpolation; time-domain expansion law and rate-dependent envelopes.
- [ ] **HISS-REDUCER-A3** — Linked image preservation, capture→process transitions, reset/profile reload and complete EOF across callback/hop phases.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Noise capture/preset → HissReducer in both modes → downstream output/export with profile and linked-channel state restored.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-hiss-reducer` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-hiss-reducer --lib --tests
cargo clippy --offline --locked -p sotf-plugin-hiss-reducer --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-hiss-reducer`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/denoiser-hiss-finite-stream.md](../../audit/denoiser-hiss-finite-stream.md)
- [AUDIT.md](../../AUDIT.md)
- [crates/sotf-plugins/crates/sotf-plugin-hiss-reducer/README.md](../../crates/sotf-plugins/crates/sotf-plugin-hiss-reducer/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Muse implementation evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). No shared source was
edited. All Rust below is authored but compile-unverified (shell/Cargo
unavailable); the coordinator must run
`audit/muse-parallel-2026-10-01/hiss-reducer/verification-request.md`
before any box is checked.

- **HISS-REDUCER-R1** (partial, in-crate done): 1 s high-band RMS capture
  engine, `NoiseProfileData` v1 persistence with transactional validation,
  and time-domain threshold following (louder of user threshold and floor
  + 6 dB) in `src/profile.rs` + `src/lib.rs`. Spectral per-bin use needs
  the `plugins-denoiser` hooks in `shared-patch/`. Tests:
  `tests/noise_profile.rs` (14 tests: 0.15 dB oracle floors, persistence,
  reset/drain semantics, modulation audio effect, no-alloc paths).
- **HISS-REDUCER-R2** (partial, in-crate done): `ReductionCurve` (1/4/12
  kHz log anchors, flat-1.0 default) and `link_mode` choice with full
  registry/getter/setter/schema/preset round-trip (`src/params.rs`,
  indices 5–11 appended). Per-bin and linked-detector DSP needs the
  shared backend patch. Tests: `tests/reduction_curve.rs` (6 tests,
  1e-6 interpolation oracle) and state round-trips.
- **HISS-REDUCER-R3** (in-crate done): mode-applicability matrix in the
  crate README; old defaults/IDs/order preserved, PARAMS append-only,
  `VERSION` 2, no version bump. Engine/example hunks + chain proof are
  shared-owner items in `shared-patch/plugin-integration.md`.
- **HISS-REDUCER-A1** (new tests authored): `tests/accuracy.rs` separates
  suppression from loss — spectral hiss-only suppression > 2 dB,
  tone-only loss < 1 dB, retained mixed SNR > 2 dB, time-domain
  hiss/tonal/transient separation (transient peaks < 1 dB on silence and
  loud tone). Existing SNR/tone-loss regressions untouched.
- **HISS-REDUCER-A2** (new tests authored): unity reconstruction (spectral
  2e-5 retained, time-domain bit-exact across 44.1/48/96 kHz × 1/2/6 ch),
  strength-law monotonicity, ±1 dB cross-rate attenuation consistency,
  curve interpolation oracle. Audit finding: engaged single-band
  reduction co-attenuates transients riding on quiet hiss in both modes
  (by design of the current detectors; no transient bypass exists).
- **HISS-REDUCER-A3** (new tests authored): capture→process click-free
  transition, reset/reload-with-profile bit-exactness, learn-during-drain
  rejection, capture frozen during drain, spectral EOF endpoint with
  profile across hop phases × use on/off. Linked-image audio proof awaits
  the backend patch (scenarios specified in `shared-patch/`).
- Whole-chain acceptance: NOT run; blocked on shared integration (backend
  patch, engine accessor + example hunks, engine preset path carrying the
  profile blob — see the gap note in `shared-patch/plugin-integration.md`)
  and on executed focused gates. Astra medium review pending coordinator.
- Compatibility: `HissReducerPluginParams` grows with serde defaults (old
  JSON loads); one shared exhaustive literal (`examples/restore_mono.rs`)
  needs the 2-line hunk; FFI/NIH/factory consume PARAMS generically (no
  count assertions found); default audio is bit-identical.

## Muse fix-round evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). Independent review R1
(`audit/muse-parallel-2026-10-01/hiss-reducer/review.md`, verdict: changes
required) plus coordinator R1 log (focused gate exit 101, 5 test compile
errors) were actioned; every actionable owned finding is fixed, no box
checked, no test weakened, no gate run from this session. See
`audit/muse-parallel-2026-10-01/hiss-reducer/fix-r1-result.md`.

- F1 (P0): the 5 compile errors fixed (format bindings, borrow hoists,
  `.err().expect`).
- F2/F3 (P0): EOF test resets after capture (assertions unchanged);
  click-test after-window moved past persistence + attack (0.8 bound
  kept).
- F10/F11/F12 (P1): stereo max-floor README contract + differential
  characterization test; cross-rate reuse-tolerance docs (code comment
  only, no behavior change); unity + cross-rate extended to 192 kHz at
  the pre-declared 1 dB bound.
- F9/F7: backend-patch derivation corrected (Parseval order, `power[]`
  warning, v1 white-spread caveat, link confirmation request); engine
  hunk 1b authored (settings + from-PARAMS + converter scalars, denoiser
  precedent) with the Restoration-group blob-path decision framed.
- F13: evidence numbers corrected — 28 new tests (15 + 8 + 5), 0.11
  step bound (0.0942 self-step), 3 future-proofed literals.
- Outstanding shared work: F4 example hunk (actively breaking), F5/F6
  unowned backend patch + plugin wiring, F7 engine application + blob
  decision, F8 whole-chain probes. Coordinator reruns per the rewritten
  `verification-request.md` (28 tests + F2/F3 single-test reruns + QA).

## Muse fix-round-2 evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). Coordinator R2 logs
(test gate 68 passed / 1 failed, clippy gate 1 error) were actioned;
both failures were owned-scope and are repaired, no box checked, no
bound weakened, no test discarded, no gate run from this session. See
`audit/muse-parallel-2026-10-01/hiss-reducer/fix-r2-result.md`.

- R2-T1: `capture_completion_transition_is_click_free` compared raw
  sums of squares over unequal windows (2400 vs 4096 samples), baking
  a 1.7067x length factor into the engagement assertion. Corrected to
  mean-square power with the 0.8 bound, windows, tone, and DSP all
  unchanged; added a regression pinning the before-window mean to the
  theoretical tone power 0.00405 within 2%. Log values prove the
  correction: before-mean 0.00405003 (6.4e-6 from theory), after/before
  ratio 0.5995 < 0.8.
- R2-C1: `collapsible_if` at `src/lib.rs:790` fixed by collapsing the
  redundant `is_complete` guard into a single `if let`
  (`take_completed` guards internally with a side-effect-free `None`
  path); behavior-identical, no realtime-path impact.
- R2 execution confirms everything else green: the F2 EOF fix, the F10
  stereo test, F12 192 kHz coverage, and all pre-existing suites
  unmodified in behavior. Shared items F4–F8 unchanged and outstanding;
  coordinator reruns per the rewritten `verification-request.md`.

## Muse fix-round-3 evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). Coordinator R3 logs
(test gate 69 passed / 0 failed, clippy gate 1 error) were actioned;
the single failure was owned-scope and is repaired, no box checked, no
bound weakened, no test discarded, no behavior changed, no gate run
from this session. See
`audit/muse-parallel-2026-10-01/hiss-reducer/fix-r3-result.md`.

- R3-C1: `needless_borrows_for_generic_args` at
  `tests/noise_profile.rs:268` fixed by deleting the needless `&` on
  the owned `persisted_params()` value, exactly as clippy suggests
  (test assertion only). The sibling `&restored` borrow is required
  (`restored` moves two lines later) and untouched; crate-wide search
  confirms no further `to_value` call sites.
- R3 execution confirms the full owned suite green: 69 passed
  (5 accuracy, 5 finite_stream, 29 integration, 15 noise_profile,
  1 realtime_parameters, 8 reduction_curve, 6
  test_hiss_reducer_plugin). Shared items F4–F8 unchanged and
  outstanding; no restoration-backend handoff received, so no backend
  integration attempted. Coordinator reruns per the rewritten
  `verification-request.md`.

## Muse backend-adoption evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). The restoration
backend is terminal (`../muse-parallel-2026-10-01/restoration-backend/
{result,hiss-adoption-patch}.md` plus direct backend source reads);
its adoption patch is applied to the owned crate, no box checked, no
bound weakened, no test discarded, no gate run from this session. See
`audit/muse-parallel-2026-10-01/hiss-reducer/adoption-result.md`.

- R1/R2 in owned scope: pre-sized 513-entry curve table with exact
  per-bin rebuild, `refresh_backend_params` pushing time-domain link,
  spectral curve/profile/link, and named curve/link arms refreshing
  (`src/lib.rs` + `curve_gains()` accessor). Spectral profile deepens
  reduction (unbiased reference vs minima, intended); curve shapes
  bands spectrally (time-domain inert by design); link vetoes
  spectrally and drags time-domain (confirmed asymmetry, both image
  preserving). Truthful broadband provenance kept; per-band capture
  stays an explicitly open gap.
- R3 in owned scope: applicability matrix reads Full/Full;
  defaults/IDs/order preserved; `HissReducerPluginParams` and PARAMS
  unchanged in this slice. Engine (1/1b), example (2), and blob-path
  items remain with their owners; contract notes in
  `shared-patch/adoption-contract-notes.md`.
- A1/A2/A3: 12 new `tests/backend_adoption.rs` audio tests (real
  capture, regional curve proof, linked-image proof both modes,
  reset/save/reload/rejection bit-exactness, named-vs-batch parity,
  automation with engagement proof, cross-rate/cutoff mapping,
  44.1/48/96/192 kHz matrix, engaged-transient characterization as a
  pinned limitation per PF9) plus 2 new realtime lifecycle guards
  (new-control setters, cold engaged process/drain/reset). Expected
  suite: 83 tests. Whole-chain acceptance still blocked on shared
  hunks, QA run, and coordinator probes; not claimed.

## Muse fix-round-5 evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). Coordinator R5 logs
(test gate 81 passed / 2 failed, clippy gate 1 error) were actioned;
all three failures were owned-scope and are repaired, no box checked,
no bound weakened, no test discarded, no DSP behavior changed, no gate
run from this session. See
`audit/muse-parallel-2026-10-01/hiss-reducer/fix-r5-result.md`.

- R5-C1: `collapsible_if` at `tests/backend_adoption.rs:140` fixed by
  collapsing to the clippy-suggested let-chain (workspace edition 2024;
  test helper only, behavior-identical).
- R5-T1 (`time_domain_link_shares_depth_and_preserves_image`, −0.13
  dB): the old oracle compared re-filtered high-band powers on the
  loud-tone channel, which is dominated by tone-fundamental leakage
  (|1 − H| ≈ 0.14) and provably capped at −0.15 dB for ANY gain — the
  −2.0 dB bound was unpassable for correct DSP. Corrected to a
  residual-drag measurement (`drag_r > −5.0` dB, `|drag_L − drag_R| <
  1.5` dB) plus hard regressions (independent-R bit-exact dry, TD
  dual-mono bit-exact, leakage-floor pin). Stimulus/seeds/windows kept.
- R5-T2 (automation, `spectral=false`, −0.15 dB): same leakage class on
  the tone+hiss mix (floor above −0.5 dB for any engaged static leg;
  old −1.0 dB bound unpassable). Mix legs kept bit-identically (click
  0.12, partition bit-exactness, spectral mix engagement unchanged and
  green); TD mix quantity pinned as the leakage regime, and new
  hiss-only legs (0.05 hiss, same automation) prove engagement under
  the unchanged −1.0 dB bound with ≈ 3–6 dB margin.
- Rejected-path scope: new `host_reachable_values_are_accepted_without_
  allocating` test pins the whole FFI-forwardable realtime value space
  as alloc-free; the residual state-dependent audio-thread rejection
  scope (structural flip, post-drain freeze) is honestly open with a
  concrete FFI-guard proposal in `shared-patch/r5-handoffs.md` §2.
- Same oracle bug exists verbatim in the backend's own time-domain
  link test (same −0.13 dB); handoff §1 asks the restoration owner not
  to change correct DSP to fit it. Expected suite: 84 tests. Backend
  gates + backend-oracle fix, example/engine hunks, QA run, chain
  probes, and the adoption re-review remain; not claimed.

## Muse guard fix-round-1 evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only, plus the scoped
`spectral_hiss.rs` guard-only grant, unused). Coordinator latest
execution (Hiss test exit 101 with 4 `transient_guard.rs` failures +
clippy exit 101 with 1 lint; all other suites green) was actioned;
every failure is an owned test-fixture bug with an independent
derivation, no box checked, no bound weakened, no test discarded, no
DSP/API/default change, no gate run from this session. See
`audit/muse-parallel-2026-10-01/hiss-reducer/guard-fix-r1-issue.md`
(written before code), `guard-fix-r1-result.md`,
`shared-patch/guard-fix-r1-handoff.md`.

- F1 stationary (-0.00 dB): -26 dBFS program against the -30 dBFS gate
  correctly yields 0 dB; fixture now runs at -20 dBFS per the backend
  quiet-tone proof. Tone/hiss programs and <-2/<1 dB bounds unchanged.
- F2-F4 settled/linked/toggle (bit-exact, no fire): 0.3 impulses add
  ~0.17x energy (ratio ~1.17 vs the derived 2.0x threshold) and
  provably never fire; restored to the backend proven unit level.
  Periods/seeds/windows preserved.
- Preservation tightened per root directive: the rejected -12 dB mean
  becomes worst-case absolute (every settled peak within -3/+2 dB)
  with per-impulse prints, plus 4864-period, one-sided linked OR,
  44.1 kHz, lowpassed-hiss, and silence-transition coverage (L1-L3).
- F5 clippy: `.err().expect()` -> `expect_err`.
- Expected suite: 101 tests. Backend grant retained for rerun-driven
  tuning if worst-case values require it (re-derive, never weaken).

## Muse guard-adoption evidence (2026-10-01, shell-less session)

Owner: Muse (`hiss-reducer`, exclusive paths only). Backend r7 is
terminal and green (`validation-r7/backend-after-r7-results.json`,
both exit 0; contract `hiss-handoff-r7.md` §2); Hiss pre-adoption
green (84/84 + clippy exit 0). The r7 `set_transient_guard` API is
adopted as an explicit visible opt-in (Param 12 bool, default false),
not auto-enabled for old profiles/presets/defaults. No box checked,
no bound weakened, no test discarded, no gate run from this session.
See `guard-adoption-issue.md` (written before code),
`guard-adoption-result.md`, `shared-patch/guard-integration.md`.

- Wiring (owned scope): appended `transient_guard` across PARAMS,
  `Params`, `HissReducerPluginParams` (serde backward default),
  getter/setter/schema/preset/blob round-trip, named + batch flows,
  `transient_guard()` getter, GUARD layout group; `VERSION` stays 2;
  old keys/indices/defaults/order preserved; `refresh_backend_params`
  pushes the raw flag (infallible, allocation-free backend store).
- Legacy: guard-off bit-identical to pre-adoption defaults (both
  modes × mono/stereo), old five-field JSON loads guard-off,
  time-domain bit-identical on/off (inert by construction),
  stationary tone+hiss bit-identical on/off (no-fire pin).
- Enhancement: 11 new `tests/transient_guard.rs` audio tests —
  settled-impulse mean improvement > 1.5 dB with per-impulse −1.0 dB
  floor (no mean-only claim), hiss-only suppression < −2 dB in both
  guard states, tone/hiss separately measured (exact-bin Goertzel,
  tone-skipped band power, no lowpass oracle), mid-stream toggle
  (0.12 step, partition-exact, 1 dB convergence), save/reload
  bit-exact, named/batch parity, linked image < 1 dB + dual-mono
  identity, engaged EOF endpoint, post-drain rejection retaining
  config/history, reset retaining the flag while restarting the
  ~21 ms blind window (characterized, not claimed as preservation).
- Realtime: guard setter pinned no-alloc cold + warmed both modes
  (new-control, host-reachable boundary, cold engaged lifecycle);
  `Err`-path allocation stays control-thread-only per convention; no
  new FFI structural guard needed for this plain bool.
- Colored scope: v1 white-spread provenance kept honest everywhere;
  per-band measured capture stays explicitly open, unclaimed.
- Shared: engine G1/G2, example G3, FFI/native/GPUI/factory
  no-change confirmations in `shared-patch/guard-integration.md`.
  Expected suite: 95 tests. Gates, QA run, chain probes, and
  independent re-review remain; not claimed.

## Current accepted slices and remaining consumers

The original R1/R2/R3 and whole-chain boxes stay open until the complete requirements are proved. Current independent Muse acceptances are scoped:

- Measured per-bin profile capture and backend use: 114 Hiss tests plus 114 backend tests, clippy; measured-profile R5 review.
- Hosted snapshot export: 124 Hiss tests, clippy, frozen 30-file provenance; hosted-profile-export R2 review.
- Typed engine profile carrier: 6 focused round-trip tests, 1102 engine tests with 10 existing ignores, clippy; engine-profile R2 review and raw-log addendum.
- FFI actual capture/save/fresh restore/nonzero audio/full EOF and transactional malformed retention: 141 tests with 1 existing ignore, clippy; bridge/FFI R2 independent review. Bridge save is integrated; its in-place load truthfully rejects a profile carrier before mutation and does not reconstruct the plugin. Bridge tests: 69 passed, clippy.

Evidence lives under audit/continuation-2026-10-01/. Native wrapper reconstruction and application live-capture-to-settings/reachable controls are being implemented by separate Muse contributor max workers. Helper-only native simulation is not accepted as a real wrapper path. Quiet/default wanted-signal quality and full host/native/application/EOF/platform acceptance remain open. Preserve all original numerical bounds and old audio fixtures. Current implementation/review model choice is Muse per user instruction, superseding older Luna/Astra instructions above.
