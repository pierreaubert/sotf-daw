# 0.5.0

## New

- Initial release: `sotf-plugin-limiter` core (threshold, release, lookahead,
  soft knee, true peak, mix) followed by a shared `math-analog` coloration
  stage (6 models, drive, color, character, trim).
- Reports the core lookahead latency; the color stage adds none.
- `isp_mode`, `dual_release`, `link_amount`, and `feed_forward` are
  intentionally not exposed and stay at core defaults.
