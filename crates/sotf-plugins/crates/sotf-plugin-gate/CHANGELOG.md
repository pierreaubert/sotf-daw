# Changelog

## Unreleased

### Preserve finite lookahead tails

- Emit the complete active lookahead audio delay at finite-stream end in all
  modes, including external-key input with program-only drain output.
- Report latency from the actual ring delay. Positive sub-frame lookahead uses
  and reports one sample; zero-lookahead processing remains unchanged.
- Add bounded, allocation/deallocation-free zero continuation with transactional
  rate/shape/capacity checks. Empty-stream drain is a no-op; a nonempty stream
  requires reset/reinitialization before new input or changed parameters.
  Identical control snapshots are accepted while draining.
- Reinitialization now resets existing envelopes, hold state, and histories as
  well as preparing the delay/detectors, matching a fresh initialized instance.


### Upward expansion and ducking

Append structural Mode (index 15, default Downward) and realtime Max Boost
(index 16, default 12 dB, range 0–24 dB). Old JSON retains the original
downward processing path. Mode accepts typed labels or choice indices.

Upward and Duck use the reflected quadratic knee above threshold, with bounded
positive boost or negative attenuation. Their lower knee edge activates the
effect; hysteresis and hold retain the most recent target. Attack increases
effect and release reduces it. Max Boost applies immediately when reduced;
zero disables upward boost. Numeric output overflow is saturated to finite
f32 values without imposing a full-scale audio limiter.

Add signed wet `gain_db`, `effect_active`, `gate_open`, and `mode`
telemetry. Existing `attenuation_db` remains nonnegative; legacy `is_open`
retains its low-attenuation meaning. See USAGE for exact gain and timing laws.

### Preserve the full soft knee during audio processing

Opening and hysteresis now use the upper knee edge (`threshold + knee/2`).
Previously, opening at the knee's center bypassed its upper half: at a
−20 dB threshold, 4:1 ratio, 6 dB knee, and zero hold/hysteresis, settled gain
jumped from approximately −2.25 dB just below threshold to unity at threshold.
The audio path now follows the continuous quadratic curve on both sides.

Soft-knee presets can therefore attenuate more around threshold and open later.
Hysteresis remains the difference between opening and closing levels, with hold
starting below the closing level. Hard-knee behavior (including defaults),
parameter IDs, ordering, and ranges are unchanged. Threshold smoothing and live
knee changes update the opening edge each sample without allocation.

### Corrected downward expansion ratio

The Gate now interprets ratio `R:1` as **R dB of output change for each 1 dB
of input change below threshold**, before the range cap. For a hard knee,
`output_db = threshold_db + R * (input_db - threshold_db)` below threshold.
Ratio 1:1 remains unity. The soft knee joins this curve continuously.

Previously, attenuation used `1 - 1/R` instead of `R - 1`; even a displayed
100:1 produced only a 1.99:1 output slope. At input −30 dB, threshold −20 dB,
and ratio 4:1, output changes from −37.5 dB to −60 dB if range allows it.

**Existing presets become more attenuating below threshold.** JSON keys,
defaults, parameter IDs, range and timing controls are unchanged. Presets
have no ratio-law version field, so the corrected law applies on load.
To recover the old attenuation curve, use `R_new = 2 - 1/R_old`; for example,
old 4:1 corresponds to new 1.75:1. This mapping also preserves the soft-knee
curve and range cap mathematically, subject to control rounding. Settled
audio may differ slightly because of floating-point approximations.

Single-band and multiband Expander use the same correction; see their
[migration notes](../sotf-plugin-multiband-expander/CHANGELOG.md).
