## Unreleased factory validation and identity closure

- Match bridge/canonical expander identities and reject invalid bridge presets
  before publication. Preserve direct Rust aliases with documented multiband
  defaults and explicit one-band construction.
- Reject invalid native construction clocks before allocating state; those
  clocks were already rejected later during initialization.
- Verify bridge audio/drain/support metadata and facade alias audio against
  explicit native construction across time/spectral, mono/stereo/six-channel,
  44.1–192 kHz configurations. Document migration of old spectral bridge aliases.

## Unreleased nonlinear routing and spectral stereo linking

- Normalize accepted constructor detector and mode labels. `RMS` previously
  passed validation but ran the Peak branch; mixed-case spectral labels could
  select the time-domain route. Parameters and accepted choice names are unchanged.
- Honor spectral mode's linked-channel contract using preallocated per-bin
  channel maxima before gain processing. Quieter stereo channels no longer
  receive extra expansion. Existing stereo audio can change; mono processing,
  FFT/window conventions, latency and finite support are retained.
- Validate 192 independent nonlinear startup/chatter/hysteresis/hold/range
  configurations plus constructor label equivalence and callback heap checks.

## Unreleased crossover precision and reconstruction validation

- Use f64 crossover coefficients, state and split intermediates, casting final
  bands to f32 before dynamics. Correct 0.00471 complex-response error at
  96 kHz/120 Hz against the unchanged 0.002 reference bound, and an independent
  crossover-edge burst error above the 2e-5 sample bound.
- Validate 210 impulse/solo/timing and crossover-adjacent burst/quiet-onset
  configurations in time/spectral modes, with finite/zero-heap callbacks.
- Retain cascaded topology, spectral FFT processing, parameter formats and
  latency. Multiband numerical audio changes; CPU cost is not benchmarked.

## Unreleased finite stream audit (2026-09-28)

- Recover spectral startup at every phase by including all preceding Hann
  windows; retain the declared 1024-frame delay and discard negative-time output.
- Drain wet and fading spectral output through the final synthesis window,
  with an exact phase-dependent 1792-2047-frame continuation, preallocated hop
  caching, and a remaining-call bound that includes unread cached output.
- Preserve the shorter settled-dry spectral delay and all time-domain finite
  eligibility. Spectral controls freeze at accepted nonempty EOS until reset.
- Recover delayed program audio for proved finite response cases with bounded,
  allocation-free drain, reset-required EOS and conservative tail metadata.
- Preserve legacy drain behavior for recursive wet/color responses and document
  the unresolved rendering policy rather than claiming a finite response.
- Report the actual one-frame minimum for positive sub-sample lookahead (AUD075).

# Changelog

## Unreleased

### Continuous soft-knee opening (AUD-095)

Both spectral and time-domain state machines now open at the centered knee's
upper edge (`threshold + knee/2`), with the same hysteresis distance below it.
They continue passing the original threshold center to the attenuation law.
The prior center trigger skipped half the curve and produced a measured
4.18 dB spectral gain jump across a 0.02 dB input change (threshold -24 dB,
knee 12 dB, ratio 4, zero hold/hysteresis). The one-band time-domain path had
the same defect, jumping approximately 4.52 dB.

Existing nonzero-knee presets can become more attenuating near threshold;
hold/hysteresis follow the corrected unity edge. Serialized values, defaults,
IDs, gain law, envelope coefficients and hold-count timing are preserved.
Knees below 0.1 dB retain their hard-knee behavior. Independent f64 DC and
periodic-Hann fundamental oracles pin the full curve, with history, automation,
per-band overrides and callback-partition regressions.

### Corrected downward expansion ratio

Both the single-band Expander alias and Multiband Expander now use the
conventional downward expansion law: below threshold, ratio `R:1` means
R dB of output change per 1 dB of input change. Attenuation is
`(R - 1) * (threshold_db - input_db)`, limited by Range. Ratio 1:1 remains
unity. This applies to global and per-band controls in time and spectral
modes, including the soft knee.

The previous implementation used `(1 - 1/R) * (threshold_db - input_db)`.
For input −30 dB, threshold −20 dB, ratio 4:1 and range at least 30 dB,
settled output therefore changes from −37.5 dB to −60 dB in time mode.
Spectral mode applies the ratio separately to each normalized FFT bin;
window leakage means its reconstructed output need not match a broadband
expander's level.

**Existing presets become more attenuating below threshold.** Their JSON
keys, parameter IDs, defaults, timing and Range remain unchanged. There is
no serialized ratio-law version, so loaded presets use the corrected law.
To preserve the old attenuation curve, set every affected global/per-band
ratio to `R_new = 2 - 1/R_old` (old 4:1 becomes new 1.75:1). This equivalence
includes knee and range behavior, subject to control rounding.

Static auto-makeup retains its existing bounded heuristic; it is not an
exact inverse of a level-dependent transfer curve. When migrating ratios,
disable static auto-makeup and compensate manually if an exact old output
level is needed. Measured auto-makeup responds to the actual attenuation.

### Spectral callback input retention

Spectral processing now consumes every input frame even when buffered output
fills a callback first. Previously, non-hop-aligned callbacks could drop an
input suffix; a 257-frame callback stream measurably shifted a coherent tone
compared with 256-frame callbacks, even at ratio 1:1. The correction uses the
existing preallocated overlap-add buffer and preserves reported latency.
