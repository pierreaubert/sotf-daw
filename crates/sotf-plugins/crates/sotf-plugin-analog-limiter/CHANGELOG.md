## Unreleased r2 gate dispositions (2026-10-02)

- Replaced two physically invalid Hammerstein oracles with valid tests of the
  same requirements (no product change): colored linking now measures the
  quiet-lane fundamental ratio (the model's even Chebyshev branches emit a
  documented -0.06-ish DC constant at small inputs, so DC sign/ratio oracles
  cannot apply); quiet level keeps its RMS check and original 0.05 peak
  threshold on the settled region, plus a derived 0.15 startup-transient
  bound. Failure messages now print observed values.

## Unreleased review-r1 dispositions (2026-10-02)

- Tightened hot color-off parity from 5% to the independently derived
  conversion-noise bound (2e-6 relative; the core fast ceiling only overshoots
  the precise guard ceiling through f32 noise) across -0.1/-6/-20 dB.
- Documented immediate-target guard vs 5 ms smoothed core in README and
  threshold/mix help; added a downward-step flat-top test with a clean-core
  transient contrast, a dry-blend positive control, non-finite
  sanitize/recovery, colored linked-vs-dual-mono, quiet-colored level, and
  cold colored allocation tests. Reconstruction oracle wording clarified as
  4x Hann characterization (not BS.1770); worst-case values now print for
  `--nocapture` capture.

## Unreleased output-ceiling contract (2026-10-02)

- Enforce the published threshold ceiling on final emitted samples after the
  analog color stage: a zero-latency per-channel clamp limits fully wet output
  to the threshold, so drive/color/character/trim can no longer push emitted
  peaks past the ceiling. No latency, state, or allocation added; samples
  below the ceiling pass bit-exactly.
- Contract scope: the ceiling binds fully wet output only; a dry blend can
  exceed it, like the clean limiter core. `true_peak` remains input detection
  only with no strict output true-peak guarantee. Threshold/true-peak help
  text now states this explicitly; parameter order, defaults, ranges, and
  preset serialization are unchanged.

## Unreleased finite stream audit (2026-09-28)

- Recover delayed program audio for proved finite response cases with bounded,
  allocation-free drain, reset-required EOS and conservative tail metadata.
- Preserve legacy drain behavior for recursive wet/color responses and document
  the unresolved rendering policy rather than claiming a finite response.

# 0.5.0

## New

- Initial release: `sotf-plugin-limiter` core (threshold, release, lookahead,
  soft knee, true peak, mix) followed by a shared `math-analog` coloration
  stage (6 models, drive, color, character, trim).
- Reports the core lookahead latency; the color stage adds none.
- `isp_mode`, `dual_release`, `link_amount`, and `feed_forward` are
  intentionally not exposed and stay at core defaults.
