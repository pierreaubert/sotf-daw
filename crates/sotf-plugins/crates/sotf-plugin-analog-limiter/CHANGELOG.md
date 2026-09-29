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
