# Unreleased

- Reuse prepared filters and cached metadata for scalar validation/setters and
  valid bulk updates. Scalar automation no longer constructs a parameter map or
  rebuilds metadata; canonical same-model snapshots remain allocation-free.
- Clear recursive EQ and color histories on successful initialization, avoiding
  stale audio when preparing a new stream or sample rate.
- Add independent complex-response and coefficient-step references, all-model
  stage-order/zero-color nulls, malformed-batch preservation, and actual host
  setter/callback heap and partition checks through 192 kHz.
- Document immediate state-preserving coefficient changes accurately: arbitrary
  steps can cause transients and are not guaranteed click-free.

# 0.5.0

## New

- Initial release: fixed 4-band parametric EQ (low-shelf, 2 peaks, high-shelf)
  followed by a shared `math-analog` coloration stage (6 models, drive, color,
  character, trim).
- Zero added latency; default state is transparent (flat bands, color 0%).
- Fail-closed analog model selection; oversized host blocks are chunked by the
  shared stage with no realtime allocation.
