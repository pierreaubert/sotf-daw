# AUD106 — Complete standalone correlation ingestion

2026-09-28. Focused accuracy and callback-integrity checks pass. Broader host
verification will follow the independent shared AutoGain/cache changes.
MIDI and IAMF are excluded.

## Defect and correction

The standalone correlation wrapper put each complete callback into a fixed
96,000-sample ring, then drained its own ring synchronously. Excess samples
were dropped although the plugin returned the full accepted frame count.
For channel counts that do not divide 96,000, the incomplete last frame was
then combined with unrelated samples from the next callback.

The public two-callback probe accepted 28,000 frames but analyzed only 21,714
at seven channels and 6,000 at 32 channels. Seven-channel matrix error reached
1.003736973 against the same directly ingested source. Audio passthrough itself
remained exact. Artifacts: `/tmp/sotf-correlation-capacity-probe.rs`, its `.log`,
and `/tmp/sotf-correlation-capacity-probe-build.json`.

The wrapper now feeds the accepted interleaved input directly to its existing
monitor. No queue or thread handoff is required because both ingestion and
analysis run in this callback. Matrix equations, channel order, the 400-ms
exponential window, parameter IDs and one-publication-per-callback policy are
unchanged. Compiled analyzer processing still delegates to the same method.

Preflight now enforces the existing Plugin contract before copying audio or
mutating measurements: prepared sample rate, checked frame/channel size,
exact buffer lengths and finite input. This also applies when analysis is
disabled. Valid empty calls do nothing. Construction rejects zero channels
and overflowing matrix dimensions; zero-rate initialization fails before
changing the existing monitor. Valid disabled calls copy audio and freeze
analysis. No artificial maximum callback size is introduced.

## Independent evidence

All four new `sotf-host/tests/correlation_stream.rs` tests failed before the
correction and pass afterward:

- 24 numerical configurations: 48/96 kHz, 2/7/32 channels, ordinary/compiled
  entry points and oversized/irregular partitions. Every one of 28,000 frames
  is analyzed. Full matrices match an independently summed, centered f64
  Pearson oracle within 2e-6. The reference sums the weighted finite source
  newest-first and uses full matrix indexing, without the production monitor
  or triangular-index helper. Opposed channels supply an analytic -1 control.
- Both processing routes and callback partitions produce exactly equal
  matrices/counts; all audio output equals the input sample for sample.
- Twenty enabled/disabled invalid-callback cases retain output sentinels,
  published data and subsequent valid measurements. These include late
  NaN/infinity, malformed lengths, wrong/zero rate and frame-count overflow.
- Reset, disabled processing, empty calls and valid/invalid reinitialization
  retain the intended analysis clock and match fresh instances. Invalid
  construction is rejected before matrix preparation.

Commands:

```text
cargo test --offline -p sotf-host --test correlation_stream
cargo clippy --offline -p sotf-host --test correlation_stream -- -D warnings
```

Results: **4 passed**, no failures/ignored; strict focused Clippy passes.
Logs: `/tmp/sotf-correlation-stream-{red,green,clippy}.log`.

## Separate limits

The standalone plugin is publicly exported but not registered in the engine
factory. This change does not add it to the product UI or alter the embedded
output correlation monitor.

Cold split-frame and nested-cache allocations are independently reproduced as
AUD107. This ingestion correction makes no allocation-free publication/reset
claim. Generic direct-monitor arbitrary slices, cache reader lifetime, and
conditional publication are covered by that separate work.
