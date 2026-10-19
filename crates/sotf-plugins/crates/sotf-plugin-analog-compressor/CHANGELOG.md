# Unreleased

- Remove parameter-map/schema reconstruction from live scalar validation and
  updates; reuse cached values for numeric, Boolean and bulk controls.
- Clear detector, hold, auto-makeup and color history on successful preparation,
  rather than carrying old detector state across streams/rates.
- Clarify Mix in metadata and documentation: compression parallel mixing occurs
  before color/trim, so zero Mix does not bypass coloration.
- Add independent physical-time dynamics and knee derivative references, all-model
  stage ordering, actual host heap/automation checks, and diagnostic high-channel
  model timings through 192 kHz.

# 0.5.0

## New

- Initial release: native single-band feed-forward compressor (linked peak
  detection, soft-knee gain computer, static + auto makeup, parallel mix)
  followed by a shared `math-analog` coloration stage (6 models, drive,
  color, character, trim).
- Zero latency; parameter vocabulary matches the existing single-band
  compressor ranges.
