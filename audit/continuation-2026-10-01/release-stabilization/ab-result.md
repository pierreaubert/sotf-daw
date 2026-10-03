# AB stabilization result (host + AB-compare checkpoint)

Scope: host (`sotf-host`) + `sotf-plugin-ab-compare` only. R29 terminal is
saved (root-held); R29 gates run for information only. This checkpoint
restores conservative R28 behavior, defers the unreviewed R29-F1
coefficient-support calculation to a parked experiment, and keeps only the
lint fix plus audited-safe clarifications. No tests executed from this
lane (shell disabled); root runs the gates below. No commits, no pushes,
no manifests, no versions. Full 74-feature audit explicitly deferred by
user — nothing below is marked complete by deferral.

## Kept (delta vs R28 baseline)

- `abcompare_plugin.rs` eager-arm block: R29 let-chain collapsible_if fix
  (`&& let Ok(required) = ...`), behavior-identical, per the R28 lint
  suggestion. Only production-code delta in the checkpoint.
- `plugin.rs` docs only: R7-F2 arrival-promise clause on
  `output_frames_for_input`, envelope-or-Unknown fallback sentence on the
  envelope docs, shrinkage-parenthetical reword. Zero behavior change.
- `tail_fold.rs` F3 counters (new, observation-only): `bound_queries`
  on Transition/Liar fixtures; asserts grant+refresh=2 (chain/graph),
  =1 (liar/probe). Counters cannot change behavior; values audited at
  the 4 host quota sites. WATCH ITEM (unexecuted): if the count asserts
  red, revert just the asserts (keep counters) per checkpoint discipline.
- ab087 decimator fixture: carry-maximized `output_frames_envelope`
  + new arrival test `decimator_live_query_undercounts_evolved_carry_arrival`.
  Fixture-only (flows through preexisting R28 envelope-preferring
  sizing); the 2 decimator tests assert content/latency/EOF only.
  WATCH ITEM (unexecuted): if red, revert both (6-line fixture method +
  test) to byte-R28.
- `aud137_composition.rs`: −0.0 comment reword only.
- Root R27/R28 mechanical fixes (`Arc<AtomicUsize>` fixtures, `as_ref`
  drain-refresh coercions) preserved untouched; F3 reuses the same
  `Arc<AtomicUsize>` pattern.

## Reverted to R28 verbatim

- `abcompare_plugin.rs`: `mask_peak_gain_max` field + construction /
  rebuild / reset epoch sites + `note_mask_coefficient_epoch` (removed);
  `modal_horizon_frames` extraction (re-inlined to R28 form);
  `biquad_l1_envelope` / `mask_epoch_peak_gain` /
  `mask_uniform_cascade_support` (removed); `tail_support` fold →
  restored R28 `None` + "Unproven" comment; 6 F1 lib tests (removed).
- ab087: `ZGainFixture::tail_support` (removed); T7
  `active_mask_support_composes_through_nested_hosts` (removed);
  `nested_21hz_mask_burst_bound_transitions_to_exact` → restored R28
  `nested_21hz_mask_burst_tail_transitions_unknown_to_exact` name + body
  + println verbatim. No preexisting R28-pass regression ignored or
  renamed; ab087 otherwise untouched.
- `daw_host.rs` channel-changing scratch check: envelope preference
  reverted to R28 live-only call.

## Deferred (parked, not deleted)

- R29-F1 uniform-support code + all 7 support tests: verbatim in
  `release-stabilization/ab-f1-deferred-experiment.md` (§1–§12) with a
  re-application checklist. Rationale + proof discussion stay in the
  abcompare lane `fix-r29-result.md`. Not reviewed; never claim proved.
- Probe-production repair + chunk-2 restore: exact design in
  `fix-r29-result.md` §7 (two-point coprime probe, flip audit, AB
  staging consumer fix, restore spec). NOT attempted this round per
  scope freeze.
- Multi-source admission, double-active-mask live drain, terminal-sink
  tail answers, consumer scheduling: deferred per user scope change.

## Known limitations at checkpoint

- Probe-identity gap STILL OPEN: build caches frame identity from the
  single `live(100)==100` probe; chunk-N (N|100) variable producers
  (notably the valid chunk-2 burst) miscompile to cached identity and
  under-stage (7 staged, 8 produced). R28 EchoTail quota coverage does
  NOT close the original chunk-2 case. Sibling integration must not
  route chunk-coinciding variable producers through AB/host folds until
  the sequenced probe repair lands.
- AB support publication is conservative R28: `None` for all AB
  instances (masks, nested, converted); Unknown tails compose as
  Unknown. No finite mask-support bound is claimed.
- Quota refresh (R27, single grant+refresh) retained as the work-bound
  mechanism; its F3 count pins are new this checkpoint (watch item).

## Checkpoint gates (root runs; baseline = R28 validated numbers)

```bash
cargo test -p sotf-host --lib
# Expect 665 PASS (+ F3 asserts inside the 3 quota tests; 1 ignored).
cargo test -p sotf-plugin-ab-compare --lib
# Expect 94 PASS (R28 set; F1 tests removed).
cargo test -p sotf-plugin-ab-compare --test ab087_variable_rate
# Expect 49 PASS + 1 new (F2.4 arrival) = 50; R28 burst name restored.
cargo test -p sotf-plugin-ab-compare --test aud137_composition
# Expect 15/15.
cargo test -p sotf-plugin-gain --lib
# Expect 36 PASS (untouched sanity).
cargo test -p sotf-plugin-resampler --lib
# Expect 64 PASS (untouched sanity).
cargo clippy -p sotf-plugin-ab-compare -p sotf-plugin-resampler \
  -p sotf-plugin-gain -p sotf-host --all-targets -- -D warnings
# Expect PASS (let-chain resolves the only R28 lint red).
cargo fmt --check
# Expect no new hunks (all edits restore R28 text or add <=100-col lines).
```

Per release-QA ladder: targeted crate tests + clippy/fmt above are the
checkpoint slice (production delta is one lint fix + reverts). Full
`just qa` / workspace `ntest` stay root's call for the merge commit, not
this lane's. Skipped by lane: everything requiring execution (shell
disabled) — recorded here, not waived.

## Integration notes for siblings

- Host + AB public API surface is unchanged (no signature/type/manifest
  edits); only doc comments, one lint-level `if` restructure, fixture
  envelope/test additions, and test-only counters differ from R28.
- Do not depend on AB `tail_support` returning `Some` (always `None`
  at checkpoint) or on outer-Unknown flipping Finite for nested masks.
- The parked F1 experiment must not be re-applied partially; the
  checklist (§12 of the park doc) requires review + gates first.
