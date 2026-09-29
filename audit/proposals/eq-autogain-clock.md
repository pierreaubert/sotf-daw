# AUD112 EQ AutoGain clock plan — independent review

Reviewed 2026-09-28 against current source. Read-only: no source changes, builds
or waveform probes were performed for this review. The workspace graph was
checked before use. The sibling math graph was stale, so coefficient behavior
was verified from the actual pinned Cargo checkout instead.

## Conclusion

The 10 Hz causal measurement policy and existing local APIs support a narrow
EQ-only correction. **Preserve transition retirement at the original callback
boundary when adding native scratch spans.** Subject to that correction and the
explicit epoch/storage details below, no additional architectural blocker was
found. No host, shared AutoGain or oversampler API change is required.

## Required correction: native scratch boundaries are not callback boundaries

Current `eq_plugin.rs:1686-1739` decides whether transitions are present, runs
the complete caller block through the interpolating path, and only then calls
`recycle_transitions(true)`. A transition with `samples_remaining == 0` remains
present for all later frames in that same callback. Those frames use
`old.lerp(new, 1)` through `process_with_coefficients`.

The actual `math-iir-fir` pin is `cabbc6dc1c3d0c8aad275ac33ec015d174859c89`
(`Cargo.lock:5004-5006`). Its
`crates/math-iir-fir/src/iir/biquad/biquad_coefficients.rs:27-34` implements
`a + (b - a) * t` without a special exact endpoint branch. That expression at
`t == 1` is not guaranteed to equal stored `b` bit-for-bit. Its normal and
explicit-coefficient filter recurrences are otherwise the same operation order
(`biquad.rs:563-603`, `:959-980`).

Consequently, calling the old complete `process_stream` for each 4096-frame
scratch span can retire a transition early and use stored target coefficients
for later spans where the original callback would still interpolate. This is a
source-proven exactness hazard, not a newly executed public waveform failure.

Smallest structure:

1. Extract a raw kernel with no metering, compensation or transition retirement.
2. Process native scratch spans in source order, retaining active transition
   objects, including zero-remaining transitions, for the entire original call.
3. Retire completed transitions once after its final raw span. Keep the existing
   oversampled callback reconciliation/retirement at its original location.
4. Preserve per-frame filter and f64/f32 conversion order. SVF and advanced-bank
   processing keep independent per-channel state, so these can be chunked in
   source order without interleaving their recurrences differently within a
   sample. Flushing final compensated output must not alter retained raw state.

Capture a disabled baseline before extraction, with transitions completing well
before, at, and just beyond a scratch boundary, both realizations, orders 2/8,
and an oversized native callback. Native raw parity should be bit exact against
that original callback partition. Do not require preexisting transition output
to be exactly equal across *different external* callback partitions without a
baseline: external transition retirement already occurs at those boundaries.

## Reference delay and metering

`Oversampler::new` derives base-rate delay from `up.output_delay()/factor`,
`down.output_delay()`, and one fixed `OS_CHUNK_SIZE` queue
(`oversampler.rs:106-114`). Its constructor and reset preload exactly that fixed
256-frame queue (`:130-134`, `:187-213`). Query the prepared object's latency for
the meter-reference ring; do not add the queue a second time or infer an extra
factor-dependent constant. The plan's neutral impulse oracle remains required.

For oversampling, preserve the original callback as a single `os.process`
invocation. Current `eq_plugin.rs:1760-1785` reconciles the transition counter
using total source frames versus internal frames emitted in that invocation.
Splitting it at either a meter boundary or a scratch boundary would change that
existing policy. The 4096-frame public cap already permits saving the complete
input in prepared reference scratch.

Copy original input before raw processing. The delayed-reference ring can be
advanced **after successful raw processing**, using that saved input, before
meter ingestion. This avoids introducing reference advancement on an early raw
error and does not change sample alignment. At 1x the reference is the original
frame directly. The delay aligns resampler scheduling/group delay, not arbitrary
IIR phase or the resampling FIR's amplitude response.

Use the existing split AutoGain APIs (`auto_gain.rs:234-286`): ingest both
matched spans, apply the old gain, then refresh input followed by output at the
fixed boundary. Refreshing output first would compare a current output window
against the preceding input measurement. Diagnostic cache publication belongs
after that pair of refreshes. Interval state can remain bounded below
`max(sample_rate / 10, 1)` instead of accumulating an unbounded sample total.

The accepted disabled-EQ policy is implementable without a new shared helper:
AutoGain ingestion works when disabled, `refresh_output_measurement` returns
before setting a target, and compensation is an exact disabled no-op. Neither
enable toggle should restart the interval/reference clock.

## Compiled route

`can_process_compiled_biquad_bank` (`eq_plugin.rs:921`) permits the native,
non-SVF, no-active-transition route, including an empty bank. Its dedicated
processor currently has its own callback-count measurement path (`:864-918`).
Replace that scheduling with the same causal helper used by ordinary rendering;
the immutable input allows its raw block to remain whole.

The adapter forwards compiled dispatch directly
(`parametric_plugin.rs:350-359`). `CompiledOpKind` has no Copy variant, and
`from_plugin` maps the advertised `EqBiquadBank` directly
(`host/compiled_plan.rs:54-62`, `:109-139`). There is no current EQ empty-bank
Copy shortcut to remove. Keep the actual cached host test: a stale cached EQ
opcode must still fall back when a later transition/topology/factor makes the
compiled predicate false. Both routes must preserve the EOS guard and count
accepted input exactly once.

## Finite EOS

The plan correctly preserves existing eligibility and work bounds:

- `finite_response_frames` rejects every nonempty/recursive bank and active
  transition (`eq_plugin.rs:135-158`). Native empty bank has no tail; oversampled
  empty bank uses the existing four-chunk/1024-frame continuation bound.
- `drain` only runs `process_stream` when the canonical 256-frame cache is empty
  (`:1072-1100`), then serves possibly partial destinations without further DSP.
  Keep that distinction for the new reference ring, meter phase, query and gain.
- The same raw/causal helper must be used by refills. A synthetic zero input
  advances the reference ring once per generated output frame; pending real
  input reference emerges during that continuation. Serving cached output must
  not advance the ring a second time.
- Applying a finite compensation gain cannot create nonzero output from an
  exact zero, so this metering correction does not enlarge audio support.
- Keep controls frozen after the first accepted finite drain and preserve empty
  stream behavior. The first successful refill latches the current finite epoch;
  malformed capacity/rate calls remain before new state changes.

Existing `tests/finite_stream.rs:142-190` provides canonical zero-continuation
coverage, but its history specifically targets the old ten-callback cadence
and is too short to validate the new 10 Hz target boundary. Add a genuinely warm
AutoGain fixture whose first or later drain refill crosses a derived boundary.
Compare actual process+drain against explicit zero continuation at capacities
1/17/137/256, including partial cached reads and unchanged scalar bounds.

## Prepared storage and epochs

Every constructor path must prepare reference storage: `new`,
`new_per_channel`, and both `from_params`
branches. The code has multiple direct `Self` initializers, so updating just
`new` is insufficient. `from_params` also supports independently shaped
advanced/per-channel banks; preserve those.

Use checked `4096 * channels` and `delay * channels`, and reject capacities
exceeding `isize::MAX / size_of::<f32>()` before a new preparation is committed.
Native processing accepts callbacks larger than the scratch size via bounded
spans; oversampling retains the exact existing cap. Full caller bounds/EOS
validation must precede any copied-reference, delay or clock mutation. No
callback resizing is needed.

Prepare oversampler plus reference storage before assigning the new factor.
The existing oversampling setter reconstructs even for a same-value write
(`eq_plugin.rs:1193-1225`); tie the epoch reset to actual reconstruction, not
only to `new_factor != old_factor`. Do not silently add a same-value no-op while
reviewing unrelated structural behavior.

On successful initialize, clear the interval/ring because
`AutoGain::set_sample_rate` creates new meters; retain the existing gain-state
policy unless the existing post-EOS initialize calls full reset. On ordinary
reset, clear the new state alongside the existing full `AutoGain::reset`.

For structural oversampling reconstruction, root clarified the policy after
this review: use `set_sample_rate(current_rate)` to prepare new meters while
preserving current/target gain, plus a new zeroed reference/clock epoch. Do not
call full `AutoGain::reset`, which would introduce a gain jump to unity. The
helper leaves old scalar meter readings internally until the first refresh;
the EQ's existing public diagnostic cache can publish cleared meter fields
while retaining the current gain field, without changing the shared helper.
Unrelated band, order, realization and topology controls need not gain a new
AutoGain reset policy in this task.

## Acceptance and remaining scope

Proceed with permanent public red partition/causality fixtures and raw baseline
capture first. Retain the proposed aligned-clock oracle, real compiled-host
route, warm finite EOS, invalid-call/reset/structural epochs, cold allocation
**and deallocation**, and matched enabled/disabled CPU measurements. The
continuous disabled diagnostic ingestion deliberately adds meter work that was
previously skipped; report that cost separately from 10 Hz statistics refresh.

No proof was found that requires broader host scheduling or shared math changes.
This review does not certify general EQ initializer transactionality, inherited
external callback transition partition behavior, recursive tail rendering, or
arbitrary effect-phase alignment.
