# De Esser: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-de-esser`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-de-esser](../../crates/sotf-plugins/crates/sotf-plugin-de-esser).
- Coordination group: **Dynamics**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Adjustable reduction range and stereo linking are implemented and wired; earlier precision corrections and rate/realtime tests exist.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **DE-ESSER-R1** — IMPLEMENT: Lookahead with correctly aligned dry audio and reported latency.
- [ ] **DE-ESSER-R2** — IMPLEMENT: Selectable linear-phase split mode with explicit delay and finite-stream behavior.
- [ ] **DE-ESSER-R3** — IMPLEMENT: Mid/Side processing and external sidechain, including declared key buses and missing-key policy.
- [ ] **DE-ESSER-R4** — INTEGRATE: Complete parameter metadata, automation/restart classification, old-preset defaults and application/native controls for all new modes.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **DE-ESSER-A1** — Independent split/recombine complex response and gain-reduction/range law over center frequencies, levels and rates.
- [ ] **DE-ESSER-A2** — Sibilant bursts plus clean speech/transient controls: reduction, wanted-signal damage, stereo image and sidechain isolation.
- [ ] **DE-ESSER-A3** — Lookahead impulse timing, dry/wet/bypass alignment, stereo/M/S endpoint routing and final retained samples under irregular callbacks.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Program and independent key source → declared native/engine sidechain buses → de-esser → output, with saved/restored modes.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-de-esser` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-de-esser --lib --tests
cargo clippy --offline --locked -p sotf-plugin-de-esser --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-de-esser`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/dynamics-finite-stream.md](../../audit/dynamics-finite-stream.md)
- [AUDIT.md](../../AUDIT.md)
- [crates/sotf-plugins/crates/sotf-plugin-de-esser/README.md](../../crates/sotf-plugins/crates/sotf-plugin-de-esser/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Worker evidence (2026-10-01, parallel session `de-esser`, shell disabled)

Scope: `crates/sotf-plugins/crates/sotf-plugin-de-esser/`, this file,
`audit/muse-parallel-2026-10-01/de-esser/`. No shared file edited. The
dynamics audit docs contain no de-esser mentions (checked 2026-10-01);
dispositions rest on current source. Boxes above stay unchecked until the
coordinator executes the gates in `verification-request.md`.

- **R1** implemented: `lookahead_ms` (idx 10, 0–20 ms, structural),
  per-channel `LookaheadBuffer` program delay, detector on undelayed input,
  dry-after-delay mix law, `latency_samples`, drain of retained delay.
- **R2** implemented: `split_topology` (idx 11, structural),
  `FirCrossover` 1025-tap bank with 512-sample group delay, inert in
  wideband, full-support (1024-frame) drain, `tail_length` finite.
- **R3** implemented: `ms_mode` (idx 12, realtime, stereo-only M/S wrap),
  `sidechain_external` (idx 13, structural, doubled `input_channels`,
  program+key stride, read-only key, missing-key transactional error).
- **R4** implemented in-crate: indices 0–9 and all legacy defaults
  preserved, serde defaults to legacy behavior, LAYOUT controls,
  `compile_metadata` latency/coupling; shared engine/native/FFI/UI work
  proposed (not applied) in
  [shared-integration.patch.md](../muse-parallel-2026-10-01/de-esser/shared-integration.patch.md).
- **A1–A3** tests authored (25 new, unrun): independent f64 DTFT band
  reference, LR4 all-pass flatness, offset-free GR slope, exact range cap,
  sibilant-burst/transient/image controls, exact impulse timing, bitwise
  partition invariance, exact drain accounting. Bounds fixed a priori; see
  [result.md](../muse-parallel-2026-10-01/de-esser/result.md).
- Whole-chain acceptance stays open (shared patch + platform lanes +
  Astra review with coordinator/integrator).

## Fix-round evidence (2026-10-01, parallel session `de-esser`, shell disabled)

Independent Muse review (`review.md`, replaces Astra per user request):
CHANGES REQUIRED. Validation-r1 failed to compile (exit 101, 10 errors).
Every actionable owned finding F1, F6–F24 is fixed in owned source (see
[fix-r1-result.md](../muse-parallel-2026-10-01/de-esser/fix-r1-result.md));
F2–F4 stay with shared owners via the extended
[shared-integration.patch.md](../muse-parallel-2026-10-01/de-esser/shared-integration.patch.md)
(§1 constructor site added, §3 rewritten as required native code, F4
engine key-routing decision recorded). Boxes above stay unchecked until
the coordinator executes the r1 gates in `verification-request.md`; no
test pass is claimed. Open limitations: no speech corpus (F24, synthetic
A2 only), whole-chain acceptance blocked on shared patch + platform lanes.

## Fix-round r2 evidence (2026-10-01, parallel session `de-esser`, shell disabled)

Validation-r2 actually ran and stayed red on one owned compile error
(E0689 at `tests.rs:1050`, ambiguous float in the r1-added `bessel_i0_f64`
helper; both `validation-r2/de-esser-test.log` and
`validation-r2/de-esser-clippy.log` show this single error). Fixed by f64
annotations with identical oracle math — see
[fix-r2-result.md](../muse-parallel-2026-10-01/de-esser/fix-r2-result.md).
No tolerance weakened, no test discarded. Boxes above stay unchecked
until the coordinator executes the r2 gates in `verification-request.md`;
no test pass is claimed. Shared patch + whole-chain + platform lanes
still open.

## Fix-round r3 evidence (2026-10-01, parallel session `de-esser`, shell disabled)

Validation-r3 actually ran: 78 passed, 2 failed, plus 1 clippy error —
all in owned `tests.rs`, all worker-test oracle bugs with production
DSP exonerated. `ms_roundtrip` asserted sample-wise identity through
the phase-rotating LR4 all-pass (now M/S-vs-L/R < 1e-5 plus RMS
all-pass < 0.05 dB, with LTI-commutation evidence);
`fir_split_bands` self-detected kill capped near 23 dB on detector
skirts (now external-key-driven with a verified > 40 dB isolation
precondition); clippy `manual_is_multiple_of` fixed. Details, math
evidence and added regressions in
[fix-r3-result.md](../muse-parallel-2026-10-01/de-esser/fix-r3-result.md).
No tolerance weakened, no case discarded, no production code changed.
Boxes above stay unchecked until the coordinator executes the r3 gates
in `verification-request.md`; no test pass is claimed. Shared patch +
whole-chain + platform lanes still open.
