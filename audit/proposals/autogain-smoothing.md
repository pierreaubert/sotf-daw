# AUD105 — shared AutoGain smoothing: source review and narrow proposal

2026-09-28. Read-only investigation; no production/test edits or Cargo runs.
Tracking: `audit/proposals/autogain-smoothing.md`. This proposal is separate from
AUD104's completed XTC measurement-clock correction.

## Confirmed defect and scope

The existing public probe `/tmp/sotf-autogain-smoothing-probe.rs` initializes
stereo AutoGain at 48 kHz, measures a 1 kHz tone whose output/input ratio is 0.5,
and differs only in smoothing: 25 versus 1000 ms. Its recorded log proves:

- `apply_compensation`: exact same audio for both times, both one full block and
  one-frame calls; final reported gain 5.857829 dB in both cases.
- `next_gain_linear`: different trajectories, maximum gain difference 0.6328645.

In `sotf-host/src/auto_gain.rs`, scalar processing (line 290) consumes
`gain_smoother.advance()`, then applies the fixed asymmetric linear recurrence.
Block processing (line 348) instead converts `gain_smoother.target()`, applies
only the fixed linear recurrence, and discards the configured smoother's result.
Its near-target shortcut is evaluated once per call and snaps the linear state.
`next_n` (line 309) has a third trajectory: it takes the final dB state and uses
that single value for an entire power-of-coefficient linear update.

## Actual production call graph

Paths below are relative to `crates/sotf-plugins/crates`.

| Family/path | Gain entry point | Existing measurement/target clock | Consequence |
|---|---|---|---|
| EQ ordinary `sotf-plugin-eq/src/lib/eq_plugin.rs:1797` | `apply_compensation` | Only each tenth callback is measured (`MEASUREMENT_THROTTLE=10`); the new target applies to that callback | Configured nested `auto_gain.smoothing_ms` ignored in applied audio |
| EQ compiled biquad bank `eq_plugin.rs:917` | `apply_compensation` | Same tenth-callback policy | Same defect; both paths need an enabled-AutoGain regression |
| Crossfeed `sotf-plugin-crossfeed/src/lib/crossfeed_plugin.rs:993` | `apply_compensation` | Measures current input and uncompensated output each callback, then applies gain | Exposed smoothing control ignored |
| XTC `sotf-plugin-xtc/src/lib/xtc_plugin.rs:1439` | `apply_compensation(frame, 1)` | AUD104: every frame ingested, N-delayed reference versus uncompensated wet, refresh after each rate/10 frames, next target only affects subsequent audio | Clock is correct; configured smoothing still ignored |
| LoudnessCompensation `.../loudness_compensation_plugin.rs:684` | `next_gain_linear` once/frame | Every frame ingested; fixed 20 Hz refresh, Pre/Post route preserved | Already uses both smoothing stages |
| ABCompare `.../abcompare_plugin.rs:1137` | `next_gain_linear` once/frame | Every frame ingested; fixed 20 Hz refresh | Already uses both stages |
| ABCompare empty-path shortcut `abcompare_plugin.rs:505` | `next_n(count)` | Same fixed 20 Hz segments; bypasses applying gain after approximately-unity eligibility | Bulk state approximation; eligibility caveat below |
| `sotf-host/src/multichannel_auto_gain.rs:101` | Inner `next_gain_linear` once/frame, same gain on all output lanes | Stereo input measured, output folded excluding LFE, output measurement per call | Already uses both stages |
| Upmixer `.../upmixer_plugin.rs:1112,2389,2646` | `MultichannelAutoGain::measure_and_apply` | Ordinary block/output-segment and finite-drain canonical cache policy unchanged | Transitive scalar caller |
| AAE `.../aae_plugin.rs:526,1111` | `MultichannelAutoGain::measure_and_apply` | Ordinary callback measurements | Transitive scalar caller |

Repository-wide symbol/source search found no other DSP production callers.
Host `bin/qa_host.rs:173,182,195` calls `next_n`, but creates disabled AutoGain,
so that QA does not establish enabled smoothing correctness. Other block callers
are the facade allocation tests/benchmarks, `daw_scale_stress`, and XTC test
priming. All should remain compiling and the relevant gates should be rerun.

## User-facing controls and compatibility

All smoothing values are milliseconds. Existing static parameter descriptions
say "Auto gain transition time". The actual first-stage time is a one-pole time
constant, not total two-stage settling time.

| Family | Serialized/control key | Default | Range / exposure |
|---|---|---:|---|
| Shared helper | `smoothing_ms` | 100 | Public helper has no added range validation |
| EQ | nested `auto_gain.smoothing_ms` | 100 | Constructor/preset JSON via `EqPluginParams`; runtime schema exposes only Auto Gain enabled, not a smoothing knob |
| Crossfeed | `autogain_smoothing_ms`, index 16 | 100 | 10..5000; realtime scalar setter |
| XTC | `auto_gain_smoothing_ms`, index 26 | 100 | 10..500; realtime scalar setter |
| LoudnessCompensation | `auto_gain_smoothing_ms`, index 10 | 100 | 1..1000; currently marked structural by static schema, though existing internal setter updates the helper |
| ABCompare | `gain_smoothing_ms`, index 7 | 100 | 1..500; realtime scalar setter |
| Upmixer | `auto_gain_smoothing_ms`, index 46 | 100 | 10..500 |
| AAE | `auto_gain_smoothing_ms`, index 21 | 100 | 10..500 |

The engine stores/converts these existing fields. No IDs, ranges, defaults,
serde layouts, native parameter policies, or engine wiring changes are needed.
Do not broaden EQ exposure or alter LoudnessCompensation structural metadata.

**Recommended compatibility decision:** make the existing scalar cascade the
shared contract. Enabled EQ/Crossfeed/XTC audio intentionally changes, including
the default 100 ms setting. Scalar callers retain their current gain recurrence.
This honors the configured smoothing without replacing the fixed 20/300 ms
protection stage. Document that setting smoothing to zero removes only the dB
stage; the fixed linear stage remains. Do not promise the knob is the final
settling time.

Keep disabled behavior: methods emit/pass unity immediately and freeze gain
state; `set_enabled(false)` still sets the dB target to zero. Keep reset at
0 dB/unity and the current sample-rate/target/max-gain update policies. No
meter-history, target-refresh, dry/wet, limiter, or finite-support changes.

## Exact shared recurrence

`../math-audio/crates/math-dsp/src/smoothing.rs:26,52,67` defines:

- `a_s = exp(-1/(0.001 * smoothing_ms * sample_rate))`, stored as f32;
  nonpositive time or zero rate gives coefficient zero.
- One step: if coefficient is zero or `abs(d - T) < 1e-5`, set `d=T`;
  otherwise `d = T + a_s*(d-T)`.
- Existing scalar conversion: `u = fast_pow10(d/20)`.
- `a = exp(-1/(0.020*rate))` when `u < g`, else
  `a = exp(-1/(0.300*rate))`.
- `g = u + a*(g-u)`, emitted for all channels of this frame.

Retain operation order, f32 storage, fast conversion, and the dB near-target
rule. Do not modify general `Smoother`, which has many unrelated callers.
In particular, `Smoother::next_n(0)` currently snaps to target: AutoGain's
zero-length methods must return before touching it.

### Minimal implementation strategy

1. Keep one private sample-step implementation (or make the current scalar
   implementation that single implementation). Block apply consumes the same
   step once per frame. `next_n(n)` discards exactly n such gains.
2. Repurpose the existing conversion cache to cache the **current smoothed dB**,
   rather than the final target. Unchanged dB can reuse the deterministic
   conversion without altering scalar values. Reset can explicitly restore the
   0 dB/unity cache. No buffers, locks, atomics, or allocation.
3. Preserve an exact stable fast path: after a real shared step, compare previous
   and new dB/linear states. If both are identical, the deterministic recurrence
   is at a floating-point fixed point for the unchanged target/coefficient, so
   all remaining frames use that gain. Apply SIMD only to the remaining valid
   audio prefix, or stop discarded advancement. This also handles rounded f32
   fixed points that never equal the mathematical target; it does not snap a
   merely close value. The one-frame result is identical to scalar advancement.
4. `num_frames=0` and `next_n(0)` are true no-ops. Valid surplus destination
   samples remain untouched, consistently with the existing per-frame path.
5. XTC may retain its one-frame calls for minimum production scope; its clocks,
   reference delay, wet/limiter ordering, warm bypass and canonical drain remain
   untouched. A later whole-span call optimization is unnecessary for correctness.

### ABCompare caveat requiring explicit choice

`AutoGain::is_unity_gain_stable()` currently accepts all three states within
1e-5. ABCompare then copies/mixes without applying the small remaining gain.
Even an exact `next_n` cannot make that audio match the scalar route.
Recommend changing this shared eligibility predicate to exact unity state
(`g==1`, dB current==0, target==0; disabled remains true). This is one narrowly
related predicate, not a new ABCompare algorithm. It removes the old tolerated
near-unity shortcut. Cold empty-path unity remains fast; a prior adapted state
that stalls slightly away from unity can stay on the ordinary path, so measure
that case explicitly. If that change is excluded, report the caller's existing
1e-5 shortcut limitation rather than claiming universal ABCompare equivalence.

## Independent evidence to add after approval

### Helper oracle and exact API equivalence

- Preserve the public 25/1000 ms red as a permanent test; test a gain increase and
  decrease, and assert the user times alter applied audio in the expected order.
- Independently implement the equations in f64 in a test module. For a constant
  target, obtain the dB trajectory from `T+(d0-T)*a_s^n` before the snap region;
  evaluate true `10^(d/20)` and the second-stage weighted sum/recurrence. Test
  both monotonic directions and piecewise target reversals. This reference must
  not call AutoGain, Smoother, or `fast_pow10` to generate expected gains.
- Distinguish numeric checks: f64 nominal coefficients establish time-constant
  convention; promoted stored-f32 coefficients separate coefficient rounding
  from state rounding and the existing fast-pow approximation. Measure and
  justify tolerance across 44.1/48/96/192 kHz and exposed extrema, especially
  Crossfeed's 5000 ms. Do not silently change the general smoother to satisfy an
  unrealistically exact f64 comparison or hide preexisting f32 stalling.
- Exact scalar-versus-block-versus-one-frame output and final state with fixed
  absolute target events; partitions 1/17/137/256/8193, mono/stereo/8 channels,
  including mid-ramp time changes, target reversal, unity crossings, near-snap
  states and rounded fixed points. Compare `next_n` final state to n scalar
  calls. These exact comparisons need no approximate numerical tolerance.
- Reset, disable/re-enable, zero-length calls, rate changes, stable unity and
  nonunity, prefix canaries, telemetry and first callback after construction.
- A fresh-thread CountingAlloc guard must prove zero allocation **and free** for
  first changing-target call, scalar/block/bulk, cache miss, stable path and reset.

### Affected callers

- EQ: JSON smoothing values with real nonflat filters, enabled ordinary and
  compiled processing at the **same callback schedule**, and existing finite
  oversampling drain cache regression. Verify actual compiled-op dispatch when
  supported. No claim of callback-invariant EQ metering under the existing
  tenth-callback measurement policy.
- Crossfeed: nonneutral fixed mode, measured programme, identical callback
  schedule, nondefault smoothing setter/readback and expected distinct gain.
  Hold its independent mix ramp fixed. Do not conflate its per-callback target
  updates with helper recurrence partitioning.
- XTC: rerun new AUD104 complete process/drain matrix, active AutoGain+limiter,
  warm bypass, independent source-prefix causality, varying process and drain
  partitions. Add 25/500 ms setting distinction without changing meter clocks.
- LoudnessCompensation Pre/Post and ABCompare: frozen pre-change scalar baseline
  on absolute target events / real signal fixtures; retain current 20 Hz causal
  target refresh. ABCompare cold unity shortcut and adapted near-unity transition
  require separate coverage if eligibility is tightened.
- Upmixer/AAE: existing multichannel fold-down and common-gain tests plus a scalar
  baseline under fixed callback schedule. Do not claim their meters become
  partition-independent. Existing prepared-capacity contracts remain as-is.

### CPU and gates

- Matched old/new optimized helper harness: enabled changing target, fixed-point
  unity/nonunity, target reversals; channel counts 1/2/8, blocks 1/17/512/8193.
  Separate meter ingestion/statistics from gain application in measurements.
- Matched XTC post-AUD104 old/new with enabled/disabled AutoGain and warm bypass,
  and fixed-callback EQ/Crossfeed. Measure ABCompare cold unity and near-unity
  ordinary fallback. Report gain-transition overhead versus stable-state cost.
- Correct transitions now perform per-frame dB conversion, unlike the old block
  helper. Cache and exact fixed-point detection reduce steady cost; do not
  substitute endpoint interpolation or an approximate block recurrence to hide
  the cost. Existing scalar callers should not pay duplicated gain math.
- Focused host/affected DSP suites, cold allocation/free tests, strict Clippy,
  then parent's aggregate workspace gate. Keep original AUD104 numerical claims
  separate from this intentional gain-envelope change.

## Explicit exclusions

No shared Smoother rewrite, parameter/default migrations, sample-rate lifecycle
redesign, target-meter-clock changes, plugin bypass redesign, host DAG queues,
manager protocol, MIDI or IAMF changes. No new measurements were run during this
read-only investigation; all numerical red evidence above comes from the
existing parent-supplied public probe.
