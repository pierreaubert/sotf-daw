# Analog Limiter: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-analog-limiter`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-analog-limiter](../../crates/sotf-plugins/crates/sotf-plugin-analog-limiter).
- Coordination group: **Shared analog family**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

Limiter followed by analog color/trim; the audit distinguishes pre-color threshold from final emitted-output guarantees.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **ANALOG-LIMITER-R1** — RESOLVE/IMPLEMENT: Establish the promised threshold/ceiling contract. If it promises emitted peak/true-peak protection, enforce it after color/trim; otherwise clearly expose pre-color semantics and validate that contract.
- [ ] **ANALOG-LIMITER-R2** — VERIFY: Native/FFI/app controls and preset ordering, with shared analog changes owned separately.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **ANALOG-LIMITER-A1** — Independent final output sample/true-peak reconstruction over all color/trim models and hot burst/two-tone cases.
- [ ] **ANALOG-LIMITER-A2** — No hidden blanket attenuation substituted for the stated contract; low-level color-off compatibility and linked-channel behavior.
- [ ] **ANALOG-LIMITER-A3** — Cold lifecycle heap checks, automation, rate changes and finite/recursive-tail policy.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Native/application preset → dynamics or EQ → analog color/trim → final output; coordinate analog-common ownership.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-analog-limiter` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-analog-limiter --lib --tests
cargo clippy --offline --locked -p sotf-plugin-analog-limiter --all-targets -- -D warnings
```

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [audit/dynamics-plugins.md](../../audit/dynamics-plugins.md)
- [audit/dynamics-finite-stream.md](../../audit/dynamics-finite-stream.md)
- [crates/sotf-plugins/crates/sotf-plugin-analog-limiter/README.md](../../crates/sotf-plugins/crates/sotf-plugin-analog-limiter/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## 2026-10-02 Muse max handoff (analog-limiter-contract-r1)

Owned implementation authored, file tools only; no Cargo command was run in
this session, so every checkbox above stays unchecked until root executes the
focused gates and records raw results. Full scope preserved: R1/R2/A1-A3 and
the whole-chain gate all remain open pending execution and independent review.

- **R1 resolved from published promises (implemented, pending execution):**
  threshold is the final emitted sample-peak ceiling at fully wet mix and is
  now enforced after color/trim by a zero-latency per-channel clamp mirroring
  the clean core's own wet-path clamp; a dry blend can exceed it, like the
  core. `true_peak` is input detection only with no strict output true-peak
  guarantee (the ISP correction mode stays unexposed by design). No new
  parameters, no order/default/range/serialization change. See
  [result.md](../continuation-2026-10-01/analog-limiter-contract-r1/result.md)
  for the promise-by-promise evidence table.
- **A1/A2/A3 tests authored (pending execution):** new
  `tests/output_ceiling.rs` with an independent f64 sample ceiling oracle, a
  self-contained f64 4x windowed-sinc true-peak reconstruction oracle,
  threshold-independence (no blanket attenuation), clean-core parity, linked
  gain, automation/lifecycle, drain, and warmed heap checks across all six
  models, 44.1/48/96/192 kHz, and mono/stereo/6-channel layouts.
- **R2 owned side complete (shared verification pending):** parameter order,
  defaults, ranges, aliases, and JSON roundtrip are unchanged and covered by
  owned tests; FFI/bridge/engine/native consumers read the owned `PARAMS`
  (help-text updates propagate automatically). Exact scoped shared proposals
  are listed in `result.md`; no shared file was edited.
- **Whole-chain gate pending:** requires root/integrator execution of the
  engine/native/FFI chain with nonzero audio, save/reload, automation,
  bypass/reset, and final-stream delivery.
- Assignment line 3 above still reads "unassigned"; the claim is recorded in
  [README.md](README.md) (`Muse max: analog-limiter-contract-r1`).

## 2026-10-02 fix-r2 handoff (review-r1 dispositions)

Root executed the r1 gates green (`gates-r1/tests-retry.log`: 31 pass,
0 fail; `lint.log` clean) after two mechanical fixes (explicit `0.0_f64`
test accumulators, `collapsible_if` let-chain on the mix mirror), which are
preserved untouched. Independent `review-r1.md` then rejected on F2-F9
substance; all are dispositioned in owned source/tests/docs only in
[fix-r2-result.md](../continuation-2026-10-01/analog-limiter-contract-r1/fix-r2-result.md),
with rerun gates in
[verification-request-r2.md](../continuation-2026-10-01/analog-limiter-contract-r1/verification-request-r2.md).
No production DSP changed in r2 (docs + tests only); no shared file touched.
Checkboxes above remain unchecked until root reruns the affected gates,
captures `--nocapture` worst-case values, and the second independent review
passes. Whole-chain gate still open.

## 2026-10-02 fix-r3 handoff (r2 gate dispositions)

Root executed r2: 34 pass / 2 fail (`linking_survives_colored...` Hammerstein
DC sign, `quiet_colored_level_stays_sane` Hammerstein peak), clippy clean.
Both failures are invalid test properties for Hammerstein's documented even-
branch DC constants, not product defects; oracles replaced with valid tests
of the same requirements in
[fix-r3-result.md](../continuation-2026-10-01/analog-limiter-contract-r1/fix-r3-result.md)
(see file for the pinned-source derivation). Rerun gates in
[verification-request-r3.md](../continuation-2026-10-01/analog-limiter-contract-r1/verification-request-r3.md).
No production or shared change; checkboxes still open pending green rerun,
independent review, and the shared-consumer/whole-chain legs.
