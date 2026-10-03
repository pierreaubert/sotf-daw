# Denoiser release stabilization — result

Worker: Denoiser feature owner (session 1f8264c9). Scope: owned crate
`crates/sotf-plugins/crates/sotf-plugin-denoiser`, assigned engine files,
assigned FFI/facade/bridge/NIH denoiser tests, requirement + lane docs.
Shell disabled throughout: no gate was executed by this worker; every
prediction below needs root execution. No commits, pushes, version bumps,
or new features.

## Current coherent state

The tree holds the R5 integration plus the stable R6 fixes, with the
unfinished R6 profile carrier parked out:

- R1 reduction curve (3-knot `[125, 1000, 8000]` Hz, log-interp, f64,
  shape gain on all arms, flat-curve legacy bit-identity) and R2 aligned
  residual audition (dry delay line, 5 ms one-pole crossfade, exact 0/1
  direct paths) retained with 33 PARAMS, engine forwarding (accessors
  29–32, serde legacy defaults, converter), and the 10 accepted layout
  snapshots.
- Engine `adjust_param_value` step fixture corrected to the
  `spec.adjust_f64` contract (delta × 0.01 step; clamp via derived
  bounded stepping), legacy render bit-identity kept
  (`crates/sotf-engine/tests/denoiser_configuration.rs`).
- FFI save/load split into `denoiser_config_restore_is_bit_exact_with_aligned_start`
  (identical construction start condition, bit-exact) and
  `denoiser_live_audition_toggle_fades_then_parks_bit_exact` (live
  trigger traverses a real fade, then parks bit-exact; settle inside the
  predeclared 8000-advance bound), with compact
  `audio_diff_stats`/`assert_audio_eq_compact` diagnostics (no tolerance
  relaxation, no discarded samples).
- F3 f64 mix accumulator kept (f32 stalls above 48 kHz); F2 analytic
  click gate kept (multirate 44.1/48/96/192 × up/down + retoggle +
  decomposition evidence).
- Invalid cross-time click oracle (`audition_switching_is_click_free`)
  REPLACED by the approved F2 gate per review decision F2: the test is
  removed (no permanent ignored failing test), its byte-identical source
  plus the r3 failure record (faded 0.446106 vs hard 0.205973,
  low=false) archived at
  `audit/continuation-2026-10-01/denoiser-feature-continuation-r1/click-free-oracle-archive.md`;
  the decomposition test keeps a live legacy-scan autopsy (printed).
- Four blind-quality reds `#[ignore]`d under the explicit user deferral
  (NOT regressions: red since r1/r2, never passed): stationary tone,
  DD-on transient, burst diagnostic, ML diagnostic. Stimuli, seeds, and
  bounds preserved verbatim in code; each carries a reason comment with
  the backlog link (`audit/requirements/denoiser.md` R4/A2 + this file).
  Running coverage kept: changing-noise halves, captured-profile tone
  workflow. Nothing claimed fixed.
- R6 profile carrier PARKED: `src/profile.rs` and
  `tests/profile_persistence.rs` reverted to stubs;
  `DenoiserPluginParams.captured_profile`, `DenoiserNoiseProfile`
  rate/hops/generation, `DenoiserData.profile_generation`,
  install/import/export, construction adoption, initialize rate-drop,
  engine settings field + converter forwarding + engine test carrier
  references all reverted. Verbatim code + step-by-step RESTORE.md in
  `audit/continuation-2026-10-01/denoiser-feature-continuation-r1/parked-profile-r6/`.
  No default-ON preservation, no new knobs (F4 stays a proposal).

## Deferred tasks (post-release backlog)

1. Profile carrier restoration + completion (engine chain tests, consumer
   construction tests, adapter patches, first compile/gate): park at
   `.../denoiser-feature-continuation-r1/parked-profile-r6/RESTORE.md`.
2. Blind default-mode quality (R4/A2: stationary absorption, transient
   capture): 4 ignored tests in
   `crates/sotf-plugins/crates/sotf-plugin-denoiser/tests/denoiser_quality.rs`,
   reproduce with `-- --ignored`; mechanism analysis in lane
   `fix-r4-result.md`; requirement `audit/requirements/denoiser.md` R4/A2.
3. Delete the two park stubs (`src/profile.rs`,
   `tests/profile_persistence.rs` in the owned crate) once the park is
   acknowledged (worker has no delete tool; stubs are inert: the src stub
   is undeclared, the test stub is one passing placeholder).

## Exact gate commands (root executes, record metric lines + exit codes)

```bash
cargo test -p sotf-plugin-denoiser --lib --tests
cargo test -p sotf-plugin-denoiser --test residual_audition -- --nocapture
cargo test -p sotf-plugin-denoiser --test denoiser_quality -- --nocapture
cargo test -p sotf-plugin-denoiser --test reduction_curve
cargo test -p sotf-plugin-denoiser --test realtime_parameters
cargo clippy -p sotf-plugin-denoiser --all-targets -- -D warnings
cargo fmt -p sotf-plugin-denoiser -- --check
cargo run -p sotf-plugin-denoiser --features qa --bin qa-denoiser
cargo check -p sotf-engine --all-targets
cargo test -p sotf-engine --test denoiser_configuration -- --nocapture
cargo test -p sotf-plugins --test denoiser_configuration
cargo test -p plugins-bridge --test denoiser_configuration
cargo test -p plugins-nih --lib denoiser
cargo test -p plugins-ffi --lib denoiser -- --nocapture
cargo test -p sotf-plugins --test layout_invariants
cargo test -p sotf-plugins --test render_plan_snapshots
cargo test -p sotf-plugins --test param_parity_tests
```

Expected on a coherent tree: all green except the 4 ignored quality tests
(absent from the run; present with `-- --ignored`, still red by design).
Watch items: owned `src/tests.rs` F2/F3 unit tests and the F2 gate were
never executed (written during the no-shell window); the R5 receipt stays
authoritative for prior gates (engine 4PASS1FAIL-adjust now fixed in
source, FFI save/load now restructured in source — both await re-gate).

## Root checkpoint cleanup

The two inert profile stubs were removed after the archive was verified. The profile-persistence target is deferred and is not included in the active gate list. This report was moved into the shared stabilization directory.
