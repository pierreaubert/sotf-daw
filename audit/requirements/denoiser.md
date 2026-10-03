# Denoiser: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **Muse max (`denoiser-feature-continuation-r1`, session 1f8264c9) — claimed 2026-10-03**.

## Implementation status (2026-10-03, gates pending root execution)

R1/R2 are implemented in the owned crate; R3 shared patches are proposed (not
applied — shared files are owned by other lanes). No gate has been executed by
this worker (shell disabled); root runs the commands in the lane
`verification-request.md` and returns failures. Requirement checkboxes below
stay unchecked until gates pass and independent review accepts.

- Lane: `audit/continuation-2026-10-01/denoiser-feature-continuation-r1/`
  (`result.md`, `verification-request.md`, `shared-integration.md`).
- Owned source: `crates/sotf-plugins/crates/sotf-plugin-denoiser`
  (`src/reduction_curve.rs` new; `params/consts.rs`, `params.rs`, `params/d.rs`,
  `config.rs`, `lib/denoiser_plugin.rs`, `wiener/consts.rs`, `spectral_sub.rs`,
  `polyphonic.rs`, `multi_resolution.rs`, `lib.rs`, `bin/qa_denoiser.rs`,
  `USAGE.md`, `UI.md`, `README.md` extended; tests `reduction_curve.rs`,
  `residual_audition.rs`, `denoiser_quality.rs` new; `params/tests.rs`,
  `tests.rs`, `tests/realtime_parameters.rs` extended).
- New parameters (appended, legacy indices 0-28 untouched): `curve_low`,
  `curve_mid`, `curve_high` (indices 29-31, defaults 1.0), `audition_residual`
  (index 32, default false). Legacy presets deserialize to identical behavior
  (flat-curve skip path, zero-mix direct path).

## Fix r3 status (2026-10-03, gates pending root execution)

Root rejected the r2 tolerance/fixture-swap resolutions for the fade
convergence and quality failures. This round implements real production
changes and restores frozen gates:

- R2 fade: endpoint snap (`AUDITION_SNAP_EPSILON = 2^-16`, guaranteed
  engagement + inaudible step, see source comment) plus exact mix 0.0/1.0
  direct paths in `drain_output`; the original bit-exact settled-fade check
  is restored and the stall-acceptance test replaced with an
  exact-endpoint convergence regression.
- R4/A2: frozen stationary-tone and DD-on transient gates restored verbatim
  (stimuli/seeds/bounds); both fail transparently in default blind mode —
  minimum-statistics absorbs stationary tones and DD (alpha 0.98) smooths
  isolated impulses, derived from `src/mcra.rs` and the DD update. That is
  the exact remaining requirement, held for root's independent audit; no
  scope change adopted. Bursts/DD-off kept as additional diagnostics only,
  and a captured-noise-profile workflow test added on the frozen stimuli.
- Lane: `fix-r2-result.md` (rejected resolutions, kept as record),
  `fix-r3-result.md`, `verification-request-r3.md`.

## Fix r4 status (2026-10-03, gates pending root execution)

R3 gates: lib70/config5/curve8/realtime3 green; audition 7/8 (click-free
oracle red 0.446 vs 0.206); quality 2/6 (frozen stationary red with tone
drift −20 dB, frozen transient red on `harmonic=false` 0.10 while
`harmonic=true` 0.54 passes, both burst/DD-off diagnostics red too);
profile-assisted green (SNR 11.33/SI-SDR 14.51); strict lint red
(doc_lazy_continuation). This round: doc-lint fix (comment only, no DSP
change — no switching discontinuity exists: monotone one-pole + forward
snap); new switching-increment decomposition diagnostic with bit-exact
mix replica and frozen per-step/location/identity asserts (original
oracle retained red); burst plateau-drift and both ML arms reporting;
new estimator unit characterization tracing the floor trajectory;
source-derived mechanism map (0.10 = gain floor, 0.54 = HPSS half-blend,
ML 0.5 = intra-frame floor capture, stationary/burst deletion = recursive
floor learning wanted energy) with r2-theory corrections; configured-vs-
default guarantee map; proposed tonality + broadband-onset floor gating
path (no legacy knob retuned; default semantics for root adjudication).
No scope change adopted; frozen gates stay red transparently. Lane:
`fix-r4-result.md`, `verification-request-r4.md`.

## R3 integration r5 status (2026-10-03, gates pending root execution)

Root-assigned engine + consumer edits applied: accessor fields 29–32
(resolve the E0080 PARAMS mismatch), serde legacy defaults, ctor,
converter forwarding, plus settings-to-audio engine tests (curve
preservation > 6 dB, 1e-6 reconstruction, twin bit-identity, rejection
retains the live chain, profile learn/use end to end) and focused
facade/bridge/FFI/NIH denoiser tests. Render-plan snapshot regen,
denoiser profile file carrier, and the systemwide retry are documented
non-goals of this round. R3 checkbox stays unchecked until gates pass.
Lane: `integration-r5-result.md`, `verification-request-r5.md`.

## Release stabilization status (2026-10-03, gates pending root execution)

User scope change: stable merge checkpoint now, unfinished features and
audits deferred. Stable delta kept: R1/R2 + 33 PARAMS/engine forwarding +
10 accepted snapshots; engine adjust-step fixture per the `adjust_f64`
contract; FFI aligned config restore + separate live-fade trigger check
with compact diagnostics; F3 f64 fade accumulator; F2 analytic click gate
replacing the invalid cross-time oracle (test removed per review decision
F2; byte-identical source + r3 failure record archived in the lane's
`click-free-oracle-archive.md`). The unfinished R6 profile carrier is
parked verbatim with a restoration guide in the lane's
`parked-profile-r6/` (code + engine/converter/test wiring reverted; stubs
await root deletion). The 4 blind default-mode quality reds are
`#[ignore]`d with reason + backlog link under the explicit deferral —
stimuli/seeds/bounds preserved verbatim, nothing claimed fixed. No
default-ON preservation, no new knobs. All checkboxes below stay
unchecked until gates pass. Result + exact gate commands:
`release-stabilization/denoiser-result.md`.

- Package: `sotf-plugin-denoiser`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-denoiser](../../crates/sotf-plugins/crates/sotf-plugin-denoiser).
- Coordination group: **Restoration/shared denoiser**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

IMCRA/Wiener estimation, profiles, masking and dual resolution exist. Config propagation and buffered finite-stream corrections are documented.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **DENOISER-R1** — IMPLEMENT: Editable frequency-dependent reduction curve with stable frequency coordinates, bounded gain and persisted interpolation semantics.
- [ ] **DENOISER-R2** — IMPLEMENT: Residual/noise-only audition with signal/latency alignment and click-free switching.
- [ ] **DENOISER-R3** — INTEGRATE: Profile/curve/audition controls and restore through all supported consumers.
- [ ] **DENOISER-R4** — VERIFY: Wanted-signal preservation and noise suppression beyond total-energy reduction.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **DENOISER-A1** — Independent unity-mask WOLA/COLA and exact latency/EOF reference over every supported FFT/hop configuration.
- [ ] **DENOISER-A2** — Known clean signal plus separately stored noise: measure SNR/SI-SDR improvement and wanted-tone/transient loss across stationary and changing noise.
- [ ] **DENOISER-A3** — Aligned cleaned plus residual reconstructs the input within a predeclared numerical bound; curve knots/interpolation and profile reload reproduce output.
- [ ] **DENOISER-A4** — Automation, cold callbacks and asynchronous profile publication must avoid callback allocation/locks and stale adoption.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Profile capture/file → persisted config → engine/bridge denoiser → cleaned or residual output → EOF/export.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-denoiser` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-denoiser --lib --tests
cargo clippy --offline --locked -p sotf-plugin-denoiser --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-denoiser`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/denoiser-hiss-finite-stream.md](../../audit/denoiser-hiss-finite-stream.md)
- [audit/denoiser-config-wiring.md](../../audit/denoiser-config-wiring.md)
- [AUDIT.md](../../AUDIT.md)
- [crates/sotf-plugins/crates/sotf-plugin-denoiser/README.md](../../crates/sotf-plugins/crates/sotf-plugin-denoiser/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.
