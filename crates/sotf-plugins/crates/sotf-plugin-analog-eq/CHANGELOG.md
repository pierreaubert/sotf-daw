# 0.5.0

## New

- Initial release: fixed 4-band parametric EQ (low-shelf, 2 peaks, high-shelf)
  followed by a shared `math-analog` coloration stage (6 models, drive, color,
  character, trim).
- Zero added latency; default state is transparent (flat bands, color 0%).
- Fail-closed analog model selection; oversized host blocks are chunked by the
  shared stage with no realtime allocation.
