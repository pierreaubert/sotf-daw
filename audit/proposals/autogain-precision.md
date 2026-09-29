# AUD110: local AutoGain precision correction (proposed)

## Preserved AUD105 checkpoint and accuracy evidence

`target/audit-tmp/autogain-aud105/aud105-intermediate-checkpoint.json` records the original and intermediate source hashes and the exact comparison of all 20 public scalar-caller renders. The intermediate source, CPU results and executables are retained beside it. AUD105 unifies scalar, block and bulk stepping without changing those enabled scalar waveforms. This compatibility evidence remains historical after AUD110.

The unchanged XTC independent diagonal-gain oracle exposes the inherited scalar floor: ideal +6.020599913 dB, actual +6.009591 dB at 44.1 kHz, exceeding its existing 0.01 dB bound. The preserved pre-change scalar implementation is bit-identical to AUD105 at both four and thirty seconds. The error grows to approximately 0.02608 dB at 192 kHz. Extending the fixture duration cannot fix the stalled f32 dB and linear recurrences or the approximate exponential conversion.

## Scope and deliberate compatibility change

Only AutoGain's private smoothing/linear states, coefficient precision and dB conversion change. Public parameters, signatures (including void `next_n`), defaults, meter clocks, target publication, caller processing schedules, shared `Smoother`, and other DSP remain unchanged. Enabled scalar waveforms will deliberately change to approach the requested gain accurately. Disabled and exact-unity paths remain unchanged. The 20 preserved caller renders will be compared for numerical deltas, not asserted bit-identical after this accuracy correction.

For the narrowest scope, retain the existing measured-target f32 subtraction and clamp, then widen that selected target to f64. This keeps target policy exactly the same; the maximum ordinary rounding from target selection is far below the failed 0.01 dB requirement. Public target and smoothing controls remain f32.

## Recurrence and lifecycle

Replace the private `Smoother` member with three AutoGain-local f64 scalars: current dB `d`, target dB `T`, and configured coefficient `a`. Also widen retained linear gain `g`, fixed attack/release coefficients, and conversion cache to f64. No generic smoother duplication or shared smoother edit.

For each accepted gain frame:

1. Preserve the existing scalar snap policy: if `a == 0` or `abs(d-T) < 1e-5` dB, set `d = T`; otherwise calculate `d = T + a*(d-T)` in f64. The original strict 1e-5 dB threshold remains; it is not enlarged and is unrelated to floating-point stall detection.
2. Cache the accurate conversion `u = exp(d * LN_10 / 20)` keyed by exact f64 dB state. Recompute only when that state changes.
3. Choose `b = attack` when `u < g`, otherwise `release`; calculate `g = u + b*(g-u)` in f64. Keep this strict intermediate-gain comparison, including reversals where the final target direction differs.
4. Return `g as f32`; block audio uses the same returned f32 multiplication as scalar callers. Telemetry computes dB from the retained f64 gain and casts at the public boundary.

Coefficients are computed directly in f64 using `exp(-1/(time_ms*0.001*sample_rate))`; input time is widened before arithmetic. Configured nonpositive time and zero rate preserve coefficient zero. Fixed 20 ms/300 ms coefficients are likewise widened. No broader invalid-control or sample-rate validation changes.

Target assignment snaps the dB state immediately only when its coefficient is zero, preserving the existing setter behavior. `set_smoothing_ms` and successful rate updates replace coefficients without clearing gain history. `set_enabled(false)` sets target zero, including that coefficient-zero snap, while disabled processing returns unity and freezes both states. Reset sets current/target dB zero, linear gain one, and cache zero/one. Empty/zero-frame calls remain no-ops. Exact unity eligibility inspects the f64 current/target states and gain.

`next_n` and the stable SIMD path may stop repeating only when both internal f64 states repeat exactly after a real recurrence step. Equal rounded f32 output gains are insufficient. This is an exact stationary shortcut, with no tolerance-based or stalled-before-target snap.

## Independent verification

- Retain the existing XTC 0.01 dB oracle unchanged, including its four-second source duration.
- Add a local plateau regression at 44.1/48/96/192 kHz for both gain directions and +20log10(2), including four/long-duration checkpoints. Use independent ideal amplitude and quantify remaining error; do not derive expected answers from production conversion.
- Tighten the independent f64 cascade oracle to reflect widened arithmetic: closed-form first-pole trajectory with the documented snap and an independently computed second pole, direct `powf` conversion as the oracle against production `exp`. Check selected coefficients and time constants independently. Include zero/negative smoothing, threshold boundaries, target reversal, and live time/rate updates.
- Keep exact scalar/block/one-frame/bulk equality, zero-length and surplus-prefix tests. Add a case where output rounds to the same f32 across consecutive frames while f64 state still changes, proving the stationary shortcut cannot stop early.
- Preserve reset/disable/cache and cold allocation/free checks. Update the ABCompare private near-unity fixture to construct a genuine nonunity f64 trajectory instead of relying on the old f32 stall; keep public eligibility and audio assertions intact.
- Re-run the 20 public scalar caller renders, report maximum/RMS waveform delta and finite output, and retain identical disabled/unity results. Dynamics owns the EQ/Crossfeed/XTC caller tests and will re-run those crates including compiled EQ and XTC EOS/partition oracles.

## Cost measurement and gates

Use the existing same-process optimized helper CPU harness with frozen pre-AUD105, intermediate AUD105, and corrected AUD110 implementations. Keep identical external meter dependencies, signal/target histories, sample rates, frame counts, channel counts, callback sizes, alternating trial order, and median reporting. Report active transition, exact-unity, disabled, settled and bulk-advance separately; accurate exponential work is a real per-frame transition cost and will not be hidden in initialization or by approximate snapping. Cache and exact fixed-point SIMD shortcuts remain valid at true stationary state. Include the ABCompare private near-unity audio/cost implication separately from publicly reachable permanently empty paths.

Run owned host/ABCompare/LoudnessCompensation/AAE/Upmixer full tests and strict all-target Clippy; coordinate independent EQ/Crossfeed/XTC full gates. Source freeze and exact test/log paths precede the parent aggregate. No engine, host queue, metadata or caller clock changes are part of AUD110.
