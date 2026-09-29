# Binaural and Upmixer native EOS work bounds

2026-09-28, AUD077 integration follow-up. No new tail duration policy or DSP
arithmetic. The existing recursive render caps remain explicit approximations,
not finite acoustic support claims.

Binaural's existing configured tail calculation is extracted unchanged into a
read-only helper. Each native call emits `min(remaining, capacity, hop)`, so a
full-capacity caller needs exactly `ceil(remaining / hop)` successful calls.

Upmixer refills a canonical hop cache and serves only that cache per call. If a
prior small destination left U unread frames within remaining R, its next
full-capacity call serves U, followed by `ceil((R-U)/hop)` calls. The query uses
that exact count, with checked cache arithmetic and integer conversion. Without
unread cache it uses `ceil(R/hop)`. Empty/completed/bypassed state returns one for
the immediate COMPLETE call. Both default preparation hooks remain no-ops.

Queries never latch EOS, accept publications, change histories, allocate or
free. Render caps, smoothing, retirement, synthesis and the native drain loops
are unchanged. This supplies declarations for the separately implemented host
quota; it does not itself remove the engine's old global cutoff.

## Verification

- Both new tests fail before implementation on the absent declaration. Logs:
  `/tmp/sotf-spatial-drain-bound-red.log` and
  `/tmp/sotf-upmixer-drain-bound-red.log`.
- **36 configurations** count real full-capacity native calls and assert exact
  agreement with the bound, including optional one-frame partial reads, empty
  and completed state, and reset. **18 configurations actually execute more
  than 4,096 calls** with existing Binaural reverb or Upmixer release caps.
- Existing fresh-thread cold drain/reset tests now query the bound inside the
  measured interval; both allocation and deallocation counts remain zero.
- Full Binaural and Upmixer suites: **263 passed**, zero failed/ignored
  (Binaural120, Upmixer143). Log:
  `/tmp/sotf-spatial-drain-bound-full-final.log`.
- Strict all-target Clippy passes:
  `/tmp/sotf-spatial-drain-bound-clippy.log`.
- Independent source review found no blocker in cache accounting, query side
  effects, source phase, empty/reset behavior or unchanged render-cap semantics.
  The reviewer did not modify source or rerun tests.

Source scope: Binaural main implementation plus existing drain/realtime tests;
Upmixer `src/drain.rs`, the trait forwarding method, existing stream-boundary
tests and cold allocation test. No manifests, host, engine, MIDI or IAMF changes
belong to this subtask.
