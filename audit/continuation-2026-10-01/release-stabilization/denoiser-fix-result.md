# Denoiser stabilization blocker fix — result

Four checkpoint-gate failures corrected, no ignoring or relaxing. Scope kept
to the owned denoiser crate plus `crates/sotf-engine/tests/denoiser_configuration.rs`
(all other files trace-read only). No shell, commits, delegates, features,
or profile expansion. FFI denoiser (3), Declick FFI (18), full host, and AB
results undisturbed (those files untouched).

## 1. Lib multirate snap step at 44.1 kHz (production fix)

Cause: `advance_audition_frame` snapped on the post-update gap only. The
smooth step into the snap window plus the snap jump is the audible TOTAL
step; at 44.1 kHz (largest 1-decay) it measured 1.5288e-5, over 2^-16.

Fix (`src/lib/denoiser_plugin.rs`): snap only when post-update AND
pre-update gaps are both within `AUDITION_SNAP_EPSILON`; otherwise take
the smooth step and snap (at most) one frame later — the gap shrinks
geometrically, so the pre-update gap is in-bounds on the next frame. Tau
(early trajectory) identical, endpoints still exact, settle bounds keep
2-3x margin (48/96/192 kHz trajectories bitwise unchanged: their old snap
frames already satisfied the total bound). Doc comments updated. The three
test mix replicas (decomposition, F2 multirate, retoggle) mirror the new
op order exactly, keeping bit-exact reconstruction.

## 2. Analytic multirate sample-rate mismatch (fixture fix)

Cause: `process_all` hardcodes the 48 kHz `RATE` in its `ProcessContext`,
but the matrix initializes plugins at 44.1/96/192 kHz → production
correctly errors. Fix: new `process_all_at_rate` helper (body moved;
`process_all` delegates with `RATE`, so all 48 kHz call sites are
untouched) and the two parked-twin renders in the multirate test pass the
loop `rate`. Full 4-rate x up/down matrix kept.

## 3. Retoggle replica skew (fixture fix)

Cause: the test drives uniform 1024-blocks, so `pos >= 48_000` fires at
frame 48128 and the flip at 50176 — but the replica assumed the nominal
48000/50000, a 128-frame skew (log: first mismatch exactly at 48000, max
9.67e-2 = full cleaned/residual divergence). Initialization/reset were
inspected and are faithful (identical construction, lockstep advance per
emitted frame); only the toggle schedule was wrong.

Fix: derive `toggle_at`/`flip_at` from the block schedule
(`div_ceil(BLOCK) * BLOCK`, with `BLOCK` shared by the loop so they cannot
drift apart) and use the actual frames in the replica, mid-fade check,
snap search, settle bound, and per-step goal loop. Bounds unchanged
(6000-frame settle, same bit-exact settled assert).

## 4. Engine legacy audio bit-exact (fixture fix, production exonerated)

Cause (proven from source, not a guess): the 29-key legacy JSON pins
`reduction_db: 12.0`, but the PARAMS default is 10.0
(`sotf-plugin-denoiser/src/params/consts.rs`; facade `param_specs::denoiser`
re-exports the leaf). The explicit baseline used `default_for` (10.0), so
the oracle compared different settings — production rendered correctly on
both sides. Every other key provably agrees: serde `default_denoiser_*`
are `serde_param_default!` macro mirrors of the same PARAMS `default_for`
reads (exact by construction), the spatial pin 0.5 equals its default,
and curves/audition are asserted on both sides. (`edited` was never
rendered; no global mutation exists on this path.)

Fix (owned engine test only): the explicit baseline pins `reduction_db`
to the same 12.0; a new exhaustive JSON key-equality pre-assert names any
drifted key with both values (guards the mirror premise without giant
dumps); the audio assert keeps exact bit-identity but reports first
mismatch/count/max via `assert_audio_bit_exact`. Also removed the three
compiler-certified unused imports (`DenoiserPlugin`,
`DenoiserPluginParams`, `ParametricInPlacePlugin`).

On "unneeded mut": neither provided log contains that warning (engine log:
exactly the 2 import warnings above; focused compile phase: zero
warnings), and every `mut` in both owned files was audited as required
(initialization, `&mut` process/drain/set, accumulators, counters). If a
third gate shows one, send the location and it gets fixed in the same
style — no `mut` was added that is not consumed.

## Re-gate commands (root)

```bash
cargo test -p sotf-plugin-denoiser --lib test_audition_fade
cargo test -p sotf-plugin-denoiser --test residual_audition
cargo test -p sotf-engine --test denoiser_configuration
cargo clippy -p sotf-plugin-denoiser --all-targets -- -D warnings
cargo fmt -p sotf-plugin-denoiser -- --check
```

Expected: lib 72/72, residual_audition 10/10, engine denoiser_configuration
5/5 with zero warnings on the owned targets. Quality file stays 2 passed +
4 ignored by the stabilization deferral; configuration (5), curve (8),
realtime (3), finite-stream (8), and polyphonic (1) unaffected.
