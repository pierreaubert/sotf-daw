# Declick usage

Construct Declick with a nonzero channel count and sample rate. Construction is
fallible because its fixed state and smoothing coefficients depend on both.

```rust
let mut declick = DeclickPlugin::new(2, 48_000)?;
```

The plugin consumes and overwrites exactly `num_frames * channels` interleaved
samples. `ProcessContext.sample_rate` must match the construction or latest
successful `initialize()` rate. Every call returns `num_frames`.

Declick reports eight samples of latency by default, or 8 + `repair_width` on
new-mode paths (see below). The first `latency_samples` output frames after
construction, reset, or reinitialization are silence. Active and bypassed
audio then use the same delayed timeline, so parallel host branches stay
aligned. Reset clears delay and detector history but preserves parameters.

Detection uses eight samples before and after each candidate. Short deviations
that return to the surrounding trajectory are replaced by a robust pre/post
interpolation. Persistent steps, onsets, high-frequency tones, and square waves
are retained by the local-variation and return tests. Transparency is
contracted for frames whose post context lies inside the signal; the final
lookahead frames before a flush see trailing zeros as a genuine
discontinuity and follow legacy interpolation there (finite, inside the
programme range, but not pinned sample-exact). Adjacent channel pairs
are linked by default; set `link_channels=false` for independent decisions.

## Modes

- `mode=Random` (default) repairs isolated clicks with the legacy detector.
- `mode=Periodic` additionally tracks repetition periods of 32–512 samples
  and lowers the threshold ×0.25 at predicted positions while guarding other
  frames ×4. Phase follows the most recent detection; locks go stale after
  four missed periods and detection falls back to random-style behavior.
  Tracking is single-phase: one lock follows the trigger stream (OR across
  channels, so independent per-channel periods are unsupported), and an
  additional in-range repetition phase is guarded ×4 like any off-prediction
  frame, so marginal clicks there may be missed. Only periods outside 32–512
  (vinyl rotation) truly fall back to random-style detection.
- `bands=Fullband` (default) detects on the single stream. `2-band` splits
  at `crossover_hz`; `3-band` splits at half and double `crossover_hz`
  (clamped below 0.45 × sample rate, ordering preserved). Splits are
  complementary one-pole sections with no added latency, and a fullband
  supervisor core (base sensitivity) must agree before any band repairs, so
  sharp programme edges are not softened. On supervisor-confirmed
  candidates whose band context is demonstrably smeared (half-window MAD
  ratio above 4×), the band repairs from the cleaner half-window level on
  level evidence alone, so multi-sample clicks still meet the fullband
  repair bound instead of failing band shape tests on smeared energy.
  The supervisor (and any ungated fullband core, which runs the same
  supervisor-side detection) additionally withholds confirmation when
  the post-window median deviates from the pre-window median — after
  removing the pre-side tone trend — by more than 25% of the residual
  in either direction: sustained programme tails are not click returns.
  True clicks return within the window, so their confirmation is
  unaffected (R29 form; R26 described a positive-only periodic-only
  variant — negative tails escaped it and quiet clicks on rising
  slopes tripped it). Legacy standing is per-claim (R31 correction):
  default settings route to the untouched shared suppressor, so they
  stay bit-identical by construction; the neutral owned core agrees
  with legacy to 1e-6 on its comparison fixture only; random
  multiband intentionally vetoes programme tails since R29.
  Per-band sensitivity is
  `sensitivity × 2^(−skew × position)` with band position in [−1, 1] from
  the lowest to the highest band; a single band is always neutral. Skew only
  redistributes sensitivity between bands under the fullband supervisor gate:
  it can desensitize bands but never adds a repair the unskewed fullband
  detector did not see. Guard-state value behavior (periodic lock, guarded
  position, supervisor-confirmed loud click): detection stays fully
  multiband (per-band thresholds, shape tests, and the supervisor
  agreement gate all still decide membership), but each emitting band's
  repair value is recomputed in wet units from the supervisor's fullband
  median estimate rather than its own band median; skew and crossover
  therefore affect only whether a band joins there, never the emitted
  value. At full mix the emitted sum equals the supervisor estimate; at
  partial mixes each band interpolates between its dry tap and its
  corrected wet value, so automation fades stay continuous; widened
  emission applies the same rule at the actually-emitted frame. Quiet
  guarded transients without supervisor confirmation are unaffected and
  stay dry.
- `repair_width` (0–8) extends repair symmetrically to excursion-consistent
  neighbors within that many samples of a detection (hysteresis widening:
  sub-threshold skirts join the repair, step edges and clean frames stay
  dry). Width is structural: it adds one latency sample
  per unit on new-mode paths, so `latency_samples` is 8 + width there and
  exactly 8 on the legacy path. The legacy path ignores nonzero width only
  in the sense that any nonzero width routes to the owned engine; default
  settings always take the bit-exact legacy route.
- `audition_residual` crossfades the output (5 ms) to the aligned residual
  (`delayed dry − repaired`) instead of the repaired audio. Detection and
  repair run normally; only the output tap changes.

Structural changes (`mode`, `bands`, `crossover_hz`, `repair_width`) clear
detector and delay history immediately so the new topology restarts with
fresh latency; the stream stays open and parameters are preserved. Host
contract: structural changes alter `latency_samples()` and `tail_length()`
(8 → 8 + width) while the stream stays open, and already-emitted frames used
the old latency. The host must re-query latency/tail (and recompile any
cached plan) after a structural change; treating structural changes as
stream rebuilds also satisfies this contract. Live changes (`enabled`,
`sensitivity`, `link_channels`, `frequency_skew`, `audition_residual`) apply
without reset, smoothed over 5 ms where noted. After drain starts, all
parameter changes are rejected until `reset` or successful reinitialization,
as before.

Old saved state without the new keys deserializes to neutral defaults and
reproduces legacy audio. Unknown choice labels and out-of-range choice
indices are hard errors; malformed numerics canonicalize to range or
default. Parameter indices 0–2 are frozen.

Non-finite samples are replaced locally with the previous finite input on
every path (detector cores, dry taps, and crossover input) and do not poison
later audio; silence substitutes before the first finite frame. The realtime
path performs no allocation, locking, logging, filesystem access, or worker
dispatch.
