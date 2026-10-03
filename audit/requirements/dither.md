# Dither: implementation and accuracy requirements

Snapshot: 2026-10-01. Assignment: **unassigned — claim in README before editing**.

- Package: `sotf-plugin-dither`.
- Primary implementation: [crates/sotf-plugins/crates/sotf-plugin-dither](../../crates/sotf-plugins/crates/sotf-plugin-dither).
- Coordination group: **Independent utility**. See [shared ownership](SHARED.md).
- Required common contract: [COMMON.md](COMMON.md). This file plus COMMON.md is the complete assignment.
- Current unfinished-edit handoff: [CHECKPOINT.md](CHECKPOINT.md).

## Existing implementation to preserve

TPDF and noise shaping with f64 guard arithmetic exist; 20/24-bit signal-dependent rounding bias was corrected.

These are scoped historical/current checkpoints, not a claim that this plugin has passed the complete audit. Revalidate current source and the newest review before changing behavior.

## Required work

- [ ] **DITHER-R1** — VERIFY: Distribution, shaped-noise spectrum, final PCM quantization and all exposed bit-depth/preset routes.
- [ ] **DITHER-R2** — AUDIT: Compare shaping/bit-depth capabilities with declared scope; do not add processing that defeats final-stage dither semantics.

AUDIT items require a current feature comparison and a recorded disposition. They do not assert an absent feature without inspection. IMPLEMENT and INTEGRATE items remain deliverables unless current source proves them completed with the stated evidence.

## Plugin-specific accuracy acceptance

- [ ] **DITHER-A1** — Independent final quantized error mean and quarter-LSB² second moment over signed/sub-LSB inputs and 16/20/24 bits.
- [ ] **DITHER-A2** — Noise PSD/autocorrelation and channel independence with deterministic test seeds; distinguish seeded reproducibility from production randomness.
- [ ] **DITHER-A3** — Actual exported PCM, unclipped endpoints and no downstream requantization/gain hidden in the test chain.

Fix numerical tolerances from the published contract, independent reference precision and existing accepted bounds before evaluating a candidate. Record the numerical bound and measured worst-case error; do not weaken bounds to make a change pass.

## Whole-chain acceptance

Final gain/processing → dither → actual target-bit-depth quantizer/export; verify final stored samples.

- [ ] Trace every added setting through registration, getter/setter, metadata/schema, serialization, engine/factory, supported native/FFI adapters and reachable controls.
- [ ] Render nonzero audio through that chain before and after save/reload; rejected candidates must retain the accepted configuration and populated history.
- [ ] Exercise actual latency, sample-clock and output-width contracts, automation, bypass/reset and final-stream delivery. A direct-DSP unit test does not replace this gate.

## Scope and ownership

Own `crates/sotf-plugins/crates/sotf-plugin-dither` and this requirements file. Shared factory/host/engine/native/UI files require an agreed owner; submit a scoped integration patch or coordinate through [SHARED.md](SHARED.md). Read local AGENTS.md before edits. MIDI/IAMF are excluded. Preserve current Cargo minor versions and unrelated worktree edits.

## Focused verification

Run from the DAW workspace. In an isolated checkout choose its own target directory; on the current shared tree serialize Cargo with `/tmp/sotf-daw-audit-cargo.lock`.

```bash
cargo test --offline --locked -p sotf-plugin-dither --lib --tests
cargo clippy --offline --locked -p sotf-plugin-dither --all-targets -- -D warnings
```

Available manifest-declared QA targets (inspect their README/CLI for the required scenario arguments; listing or building a target is not a passing diagnostic run):

- `qa-dither`; required features: `qa`.

## Evidence and completion

- [ ] Link each requirement above to changed source, exact executed command, raw result and independent reference/measurement.
- [ ] Preserve frozen old-state/audio fixtures; mark missing external fixtures explicitly rather than returning a passing test.
- [ ] Record feature deltas, compatibility/migration, measured accuracy, realtime/lifecycle results and remaining limitations.
- [ ] Independent Astra medium review; Luna xhigh fixes findings and reruns affected gates. Complete only when all applicable requirements pass.

## Starting evidence

- [AUDIT.md](../../AUDIT.md)
- [audit/utility-plugins.md](../../audit/utility-plugins.md)
- [crates/sotf-plugins/crates/sotf-plugin-dither/README.md](../../crates/sotf-plugins/crates/sotf-plugin-dither/README.md)

Older reports contain superseded findings. Latest source plus later review/evidence takes precedence; preserve useful reference fixtures rather than repeating already accepted implementations.

## Recovery session evidence (2026-10-01, Muse `dither`; no passes claimed)

Implementation carrier: `src/lib/tests.rs` lines 687–1443 in
`sotf-plugin-dither` (tests only; no production change, no new parameters,
defaults/IDs/serde preserved). Execution is pending coordinator-run gates
(shell disabled in this session); all boxes above stay unchecked until gates
pass. Full record: `audit/muse-parallel-2026-10-01/dither/result.md`.

- **DITHER-R1** — evidence written, not yet executed:
  `quantization_grid_covers_all_depths_and_rounding_modes` (16/20/24 ×
  round/truncate, ties, endpoints vs independent f64 oracle),
  `preset_routes_accept_labels_defaults_and_reject_invalid_state`
  (labels, empty defaults, wire formats, transactional rejects, clamps),
  `schema_latency_and_values_roundtrip_through_plugin_trait`
  (schema, latency 0, no channel mixing, clamp vs reject layers).
- **DITHER-R2** — disposition recorded in result.md: source matches declared
  scope exactly (16/20/24-bit, TPDF/round/truncate, Wannamaker F-weighted
  shaper); no out-of-scope processing found, none added.
- **DITHER-A1** — `final_quantized_error_has_zero_mean_and_quarter_lsb_second_moment`
  (9 signed/sub-LSB levels complementing AUDIT.md's 8, which include large
  ±0.75 signals; union is 16 distinct levels with 0.0 shared — R1 correction
  of the earlier "superset" wording, no coverage lost), 16/20/24b, N=262144,
  bound 0.008 LSB units, fixed a priori) plus `block_partitioning_is_bit_identical`.
- **DITHER-A2** — `unshaped_tpdf_final_error_is_uncorrelated_across_time`
  (lags 1–8), `unshaped_tpdf_error_spectrum_is_flat` (1 dB band ratio),
  `channel_errors_are_mutually_uncorrelated` (4ch pairwise), and
  `full_output_is_deterministic_across_instances_and_reset` (pins fixed-seed
  production contract on full output).
- **DITHER-A3** — `exported_pcm_matches_plugin_output_bit_exactly` (all
  depths × types × shaping; bit-exact output↔code identity, endpoint rails).
- **Whole-chain** — in-crate stand-in
  `gain_dither_export_chain_survives_save_reload_and_rejection` written;
  full gain→dither→export via factory/engine remains an integrator shared
  gate (no dither-side shared patch needed; registration confirmed at
  `crates/sotf-plugins/src/factory/create.rs:202`).

## Fix round R1 evidence (2026-10-01, Muse `dither`; no passes claimed)

Independent Muse review (`audit/muse-parallel-2026-10-01/dither/review.md`,
replacing Astra at the user's request) found no source blockers. All findings
F1–F9 are dispositioned in `audit/muse-parallel-2026-10-01/dither/fix-r1-result.md`.
Validation-r1 recorded dither `cargo test --lib --tests` passed (35 lib +
1 error-moments + 16 integration + 1 realtime) and `dither-clippy.log` clean;
the `gain-dither-clippy.log` errors are gain-test-only. R1 execution is pending
coordinator-run gates (shell disabled in this session); all boxes above stay
unchecked until gates pass.

- **F1 (bridge ignores dither config)** — shared-owner gate, not a
  dither-source change. Concrete proposal patch + test + integration
  instructions: `audit/muse-parallel-2026-10-01/dither/shared-patch-f1-bridge-dither-config.md`.
- **F2 (batch `apply_values` not transactional)** — fixed in
  `src/lib/dither_plugin.rs` (whole-batch validation before mutation;
  clamp semantics preserved) with regression
  `apply_values_is_transactional_on_batch_errors`.
- **F3 (vacuous shaping test name)** — renamed to
  `shaped_and_unshaped_tpdf_paths_produce_finite_nonzero_error` with a
  smoke-test comment pointing at the band test for the reduction claim.
- **F4 (fixed-seed production RNG)** — documented limitation, no code change:
  production uses fixed per-channel xorshift seeds reseeded by `reset()`;
  there is no entropy source, so seeded reproducibility and production
  behavior coincide by design (pinned by
  `full_output_is_deterministic_across_instances_and_reset`). Consequence:
  every reset-then-render repeats the identical dither pattern across
  bounces. No fix unless product demands per-render variation.
- **F5 (ring-index comments)** — reworded to head-relative indexing; no
  behavior change.
- **F6 (zero-seed guard)** — `rng_seed` maps a derived zero to 1; proven a
  no-op at supported layouts by new pin
  `per_channel_rng_seeds_are_nonzero_and_stable` (recomputes the raw
  derivation for channels 0..12, requires nonzero + equality).
- **F7 (NaN contract)** — documented at the quantizer: direct callers must
  present finite input (`as i32` maps NaN to code 0 and would poison
  shaping history); the production adapter rejects non-finite blocks first.
- **F8 ("superset" wording)** — corrected to complementary sets, union 16
  distinct levels (0.0 shared); both oracle tests kept.
- **F9 (coverage suggestions)** — fractional-float choice reject asserted
  (`{"bit_depth": 1.5}`); non-default (20-bit/truncate/shaping-off) audio
  save/reload added
  (`non_default_config_save_reload_reproduces_output_bit_exactly`); 12ch
  (catalog maximum) added to the allocation-free realtime test. A1 bound
  and coefficient treatment unchanged per review.
