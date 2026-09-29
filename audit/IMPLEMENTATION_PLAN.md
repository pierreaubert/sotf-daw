# Audit implementation and validation handoff

Updated 2026-09-29. This plan preserves the user's full objective: audit every
in-scope crate against current professional features, implement missing parts,
check the complete audio chain, and establish plugin accuracy with independent
measurements. **MIDI and IAMF are excluded.**

### User-requested pause checkpoint

Paused after accepted AUD131/AUD132: the coordinated offline workspace gate
passed 6,071 tests, with 15 skipped and MIDI/IAMF excluded. AUD133 has only a
source-grounded proposal for Ambisonics orders 4–7 on existing named layouts;
no production implementation or design acceptance is claimed. Astra's initial
design review found acceptance refinements for physical-matrix rank diagnostics,
independent grid/normalization/residual criteria, order-7 basis-impulse coverage,
the staged AUD051 dual-band/EOS baseline, and measured performance scope. Resume
by revising and re-reviewing the proposal before Rust edits or Cargo gates.
Provisional AUD134 remains untouched.

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
including the root coordinator; use two Luna workers plus one Astra reviewer.

| Agent | Model / effort | Exclusive implementation ownership |
|---|---|---|
| `/root/luna_implementation` | Luna / xhigh | Host metering and spatial SOFA work; AUD124–128, accepted scoped AUD131, AUD133 Ambisonics higher orders (proposal awaiting review), spatial files excluding Upmixer, and shared audit-ledger/plan updates |
| `/root/luna_upmixer` | Luna / xhigh | Upmixer minimum FFT geometry (AUD129 accepted), bounded AUD130 correction below 512, AUD132 source-tag retiming at/above 512 (accepted), and provisional AUD134 true-stereo convolution; Upmixer crate and dedicated proposal/report |
| `/root/astra_validation` | Astra / medium | Independent reviews and acceptance logs for both tracks; implementation corrections return to their Luna owner |

Research and edits run concurrently in separate owned files. Coordinate any
cross-track file change before editing. The metering worker schedules shared
Cargo jobs; serialize builds and timing measurements on the shared target.
Coordinate a stable source snapshot for broad integration gates and identify
the actual tested snapshot if another worker edits during a running test.
Each track repeats implementation/review/correction until accepted, then takes
another independent batch from the full remaining plan.

## Completed work and authoritative evidence

`../AUDIT.md` contains the issue-by-issue ledger through AUD132, the complete
workspace inventory (59 in-scope crates at the recorded checkpoint), comparison
tables and links to detailed reports. Some older reports describe defects later
fixed: reconcile them with the newest issue/checkpoint before reopening work.
Do not interpret every issue numbered below 123 as completely closed.

| Area | Work already implemented and tested | Remaining qualification |
|---|---|---|
| Host and chain | Same-rate latency compensation, automation timing fixes, bounded drain/preflight contracts, bypass handling, f32/f64 dispatch, resampling clocks and endpoint fixes | Unequal-rate branch retention and A/B nested variable-rate composition remain open |
| Dynamics | Expansion laws, Gate modes, compressor range/hold, de-esser range/linking, analog control retention, limiter channel behavior and actual 2x/4x audio oversampling | Further family comparisons, native automation coverage and recursive-tail policies remain |
| Filters/restoration | Independent EQ/convolution/crossover response tests, finite-stream fixes across many buffered families, resampler cutoff/lifecycle fixes, denoiser timing and callback partition fixes | Feature gaps and reference-quality restoration/corpus evidence remain |
| Spatial | NUPC timing, Binaural/XTC/Upmixer streaming and tails, transactional initialization/publication, integer and fractional/signed SOFA delays, corrected HRIR resampling, small FFT64/128 Upmixer initialization, AUD129 minimum geometry/capacity, bounded AUD130 correction below 512, and AUD132 source-time retiming at/above 512 | AUD131 and AUD132 scoped implementations are accepted. The latest coordinated workspace gate passes 6,071 tests with 15 skipped; broader spatial quality and native-device evidence remain |
| AutoGain | Accurate smoothing; causal, aligned measurement clocks in EQ/Crossfeed/AAE/Upmixer; reduced private meters with exact compatibility and matched CPU improvements | Historical AUD115 AAE/Upmixer matched CPU evidence remains missing |
| Metering | Integrated history preparation, optional bounded LRA, immutable prepared snapshot publication, spectrum endpoint power correction, published true-peak coefficients, finite-stream finalization, AUD123 rate coverage (implemented and independently accepted) | Broader metering feature/accuracy comparison remains |
| Engine/drivers | Worker interruption fixes, isolated pending-format guards, offline tail API/endpoint composition, iOS feeder fixes, portable HAL staging and compile checks | Coordinated transition protocol and native macOS/iOS execution remain |
| Integration layers | Parameter typing/restore, FFI return-count and transactional restore checks, realtime scalar access, native transport/precision/tails and selected automation paths | Reconcile all wrapper families and platform-specific routes with actual DSP behavior |

### Most recent passing workspace checkpoint: AUD132 with AUD131

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
  HRIR resampling. AUD131 and AUD132 now have focused and broad-gate acceptance;
  AUD129 and AUD130 remain accepted. Broader spatial quality and native-device
  evidence remain open. AUD133 is a proposal-only higher-order Ambisonics
  investigation for orders 4–7 on the existing named layouts; Astra's initial
  review identified design refinements before implementation. Custom layouts
  remain separate.

### Final coordinated broad-gate run: AUD131 + AUD132 source snapshot

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

## Parallel Upmixer batch: AUD129 and bounded AUD130 accepted; high-rate timing follow-up open

Astra accepted AUD129's bounded minimum FFT geometry and HR ring-capacity
changes on 2026-09-28. The final Upmixer package snapshot passes 164 tests,
strict all-target Clippy, and formatting; source manifest SHA-256
`d0ef11f8e20a005c3483bdefee80b9dea3cb945c498938c64e15c5fb18f06835` matched
before and after its package gates. This was a scoped package gate, not a
workspace-wide run. Evidence and Astra's review are in
`upmixer-minimum-fft.md` and `reviews/AUD129-astra.md`.

Astra accepted the bounded AUD130 source-tag scheduler correction on
2026-09-28. For N<512, accepted-input credits now gate the prepared shared
latency path; tests verify the 512-frame HR/main arrival across single,
irregular and 512-frame callback partitions, plus long single-callback EOS,
HR resume and AutoGain/layout routes. N>=512 retains the prior prepared-gain
path and matches an isolated reconstructed pre-AUD130 mixer control across
partitions. The final package passes 175 tests (2 intentionally ignored),
strict all-target Clippy and formatting; the start/end source+lock manifest
SHA-256 is `f5b180b05ff15981b20ea2e5ad306bf0001daafc34dba5258bc74a6394eb45f9`.
This is package-scoped, not a workspace-wide run. The >=512 control reconstructs
the mixer while sharing current analysis/preparation/drain; it is not an
archived whole-plugin baseline. Above-512 alignment and general HR quality/CPU
bounds remain unresolved. Evidence and review are in `upmixer-hr-timing.md`
and `reviews/AUD130-astra.md`.

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
2. **Finish confirmed correctness/evidence gaps.** Reconcile AUD073 and the
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
   true-stereo convolution routing, crossover slope choices, restoration
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
- No Cargo process was live at handoff. Recheck live handles/processes after
  interruption before restarting anything.

## Review handoff template

Luna supplies: issue and requirement checklist; changed paths; before/after
evidence; independent oracles and tolerances; exact commands/results; realtime
and CPU measurements; remaining limitations. Astra returns a pass or prioritized
findings with file locations and concrete required checks. Luna fixes findings,
records the correction, and requests another review. Repeat until supported by
actual evidence, then move to the next batch in this full plan.
