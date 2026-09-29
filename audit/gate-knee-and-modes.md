# Gate knee correction and proposed additional modes

Date: 2026-09-28. MIDI and IAMF excluded. AUD065 implementation is complete;
AUD009 below is a design proposal only. No new modes have been implemented.

## AUD065: the full soft knee now reaches the audio output

The previous `advance_threshold` returned the center threshold to both process
paths as their opening point. When the detector reached that level, the state
machine forced unity gain, bypassing the upper half of the existing quadratic
attenuation curve. Helper-only curve tests missed this interaction.

Independent public-waveform reproduction: threshold -20 dB, ratio 4, knee 6 dB,
zero hysteresis/hold, peak detection, no HPF, settled DC. Measured gain at the
threshold was 0 dB; the analytical expectation was -2.25 dB. Baseline failure:
`/tmp/sotf-gate-soft-knee-repro.log`.

The shared helper now retains the smoothed center threshold for attenuation and
derives the linear opening level from its upper knee edge on every sample. The
closing level remains the opening level minus hysteresis; hold retains the
unity target after closing. No threshold cache, new parameter, index change,
allocation, or hard-knee arithmetic change was introduced. Widths below 0.1 dB
continue to use the existing hard-knee law.

Let x=T-L, where T is the threshold, L is detector level, K is knee width,
and R is expansion ratio. The independent settled attenuation oracle is

    A(x) = 0                              if x <= -K/2
           (R-1) (x+K/2)^2 / (2K)        if -K/2 < x < K/2
           (R-1) x                       if x >= K/2.

Range caps attenuation. At threshold -20 dB, ratio 4, knee 6 dB:

| Detector dB | -26 | -23 | -21 | -20 | -19 | -17 | -14 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Expected gain dB | -18 | -9 | -4 | -2.25 | -1 | 0 | 0 |

`tests/soft_knee_audio.rs` has six new public API regressions:

1. Full transfer curve for mono, linked stereo and unlinked stereo, including
   probes 0.01 dB apart around both knee edges and the former center jump.
2. Different stereo levels verify maximum-detector linking and unlinked gains.
3. Default hard-knee transfer remains unchanged.
4. Hysteresis opens at -17 dB and closes at -21 dB in this configuration;
   an independent 96-frame hold followed by a 480-frame one-pole dB release
   oracle verifies every output frame and the exact first release sample.
5. Live knee/threshold updates compare whole versus 1/7/61/256 frame callbacks,
   analytical settled gains, and reset versus a fresh instance.
6. An explicit TLS allocator AND deallocator measures cold first processing,
   live updates, reset, and subsequent processing: zero allocations and frees
   across linked/unlinked, internal/external sidechains, peak and filtered
   RMS/lookahead configurations. Initialization/destruction are outside tracking.

The 0.02 dB waveform tolerance covers the existing fast logarithm/exponential
and f32 envelope; analytical curve helpers use neither production fast math nor
the production attenuation function. Settling spans 25 release time constants.

Verification:

- `cargo test -p sotf-plugin-gate`: 79 passed, zero failed/skipped;
  `/tmp/sotf-gate-knee-tests.log`.
- `cargo clippy -p sotf-plugin-gate --all-targets -- -D warnings`: passed;
  `/tmp/sotf-gate-knee-clippy.log`.
- `cargo fmt -p sotf-plugin-gate` and scoped `git diff --check`: passed.

Usage, parameter descriptions, parameter type docs, crate agent notes and
changelog now distinguish the centered curve from state thresholds. Existing
soft-knee presets can attenuate more near threshold and open later; defaults
remain hard-knee and retain their behavior. Hysteresis/hold deliberately make
nonzero settings history-dependent, so those settings do not promise a single
static transfer curve.

## AUD009: upward and ducking mode proposal, not yet implemented

### Primary comparison and scope

[FabFilter Pro-G's time controls/style documentation](https://www.fabfilter.com/help/pro-g/using/timecontrols),
checked 2026-09-28, explicitly describes upward expansion as boosting levels
above threshold, with separate narrower ratio/threshold ranges. Its ducking
style reduces program level when a sidechain voice begins and restores it when
the voice stops. The page does not publish either transfer equation. It also
describes program-dependent timing; SOTF's one-pole timing is a different,
explicitly specified design. The proposal supplies these broad functions and
does not claim equivalence to proprietary algorithms.

### Proposed transparent, bounded law

Use the nonnegative quadratic hinge H(x,K): zero below -K/2, (x+K/2)^2/(2K)
inside the knee, and x above K/2. At K<0.1 use max(x,0).

| Mode | Signed target gain in dB, before temporal behavior |
|---|---|
| Downward, current default | -min(range, (R-1) H(T-L,K)) |
| Upward | +min(max_boost, (R-1) H(L-T,K)) |
| Duck | -min(range, (R-1) H(L-T,K)) |

For above-threshold modes at T=-20, R=4, K=6, L=-23/-20/-17 dB,
the uncapped effect magnitudes are 0/2.25/9 dB. Upward boost at L=-14 would
cap at a proposed default 12 dB; duck attenuation would be 18 dB if range permits.
The duck law is the reflected expansion slope, an explicit SOTF choice; it is
not a claim about Pro-G's hidden curve or a conventional compressor ratio.

Append a structural choice `mode` after existing index 14, default Downward,
then a separate `max_boost_db` control with default 12 dB and finite 0..24 dB
range. Retain the old ratio and threshold ranges and all existing defaults.
Reusing the default 80 dB attenuation range as an upward boost limit would be a
poor default, hence the separate control. Mode changes require reconstruction
so old signed-envelope/hold state cannot silently change interpretation.
The existing zero-range attenuation convention keeps its finite 240 dB cap;
zero max_boost means no upward boost.

Finite gain still needs a finite output guard for extreme finite f32 input:
upward gain can overflow multiplication even with a 24 dB cap. Preserve normal
floating-point audio headroom; only saturate an otherwise overflowing result to
the finite f32 range. This is not a 0 dBFS output limiter.

### Timing, hysteresis and hold require explicit mode semantics

Keep the existing downward state machine unchanged. For above-threshold modes,
let the effect activate at the LOWER knee edge T-K/2 (T for hard knee), where
the above-threshold law starts continuously from zero. Its closing level is
that activation level minus hysteresis. While at/above activation, follow the
current curve and remember its most recent target magnitude. If active and the
level enters the hysteresis band below activation, retain that magnitude.
Below the closing edge, retain it for exactly the hold duration, then target
zero effect. A return above activation resumes curve following immediately.

Attack follows increasing effect magnitude (boost or duck attenuation), while
release follows decreasing effect magnitude. This gives the expected prompt
duck/boost onset and slower return to unity. It intentionally differs from
downward mode, whose attack reduces attenuation. With hysteresis and hold zero,
both new modes follow the complete static curve. A slow fall through the knee
leaves a small held target; an abrupt voice dropout can retain a larger one.
Neither state path may force a nonzero step at the knee center.

Retain existing `GateData.attenuation_db` as nonnegative reduction. Upward
requires separate signed gain telemetry or a documented new gain field; do not
silently publish negative attenuation into consumers that expect reduction.
Clarify mode/effect-active display semantics before exposing controls.

### Integration and verification before implementation acceptance

- Append controls to the centralized crate params and register typed constructor,
  setter/getter/cache/schema/serialization paths together. Old JSON must load
  Downward with unchanged index/default contracts; mode names and numeric enum
  inputs must be validated before mutation.
- Review engine settings `plugin_settings.rs` and conversions (Gate default
  branch currently around line 1857); the bridge factory deserializes
  `GatePluginParams` directly. NIH gets gate PARAMS dynamically via wrapper.rs,
  but its parameter bridge and preset restore still need focused round trips.
  Engine/native/schema changes are outside the current crate-only authorization.
- Independent gain matrix: both signs, ratio 1, caps including zero, all knee
  sections and continuity points, unequal linked/unlinked channels, external
  sidechain with independent program level, silence and f32 extremes.
- Independent sample timing: attack/release exponential in dB, hysteresis
  activation and closing edges, exact hold count, abrupt versus slow level falls,
  lookahead program alignment, callback partitions, automation and reset.
- Explicit cold allocations and frees, default/old-preset waveform equivalence,
  parameter index stability, missing-mode deserialization, and native/FFI/engine
  round trips. Feature work should proceed only after the chosen state semantics
  and integration ownership are reviewed.
