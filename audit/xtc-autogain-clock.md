# AUD104 — XTC AutoGain measurement clock

2026-09-28. Source and tests frozen. MIDI/IAMF excluded. This changes XTC-local
measurement scheduling; shared AutoGain equations, public parameters, latency,
filter publication and finite-support contracts remain unchanged.

## Reproduced defect

Only every tenth caller block previously reached either loudness monitor. Its
new target was then applied to that whole block, including earlier samples.
Current input was also compared with N-frame delayed output. Thus callback
sizes selected which signal fragments were measured and when correction began.

For identical six-second stepped stereo audio at 48 kHz/N2048, one-frame versus
512-frame calls differed by **0.027620405 peak amplitude**, with **13.70238 dB**
signal/error ratio. Alternating 8193/137-frame calls never accumulated enough
measured audio for finite loudness. AutoGain-off controls were bit-identical.
Neutral delayed identity also acquired unwanted gain: peak error **0.024987534**
at 48 kHz/N2048 with 512-frame calls, versus about **2.6e−8** without AutoGain.

Public probes and logs:
`/tmp/sotf-xtc-autogain-clock-probe.rs`, `.log`,
`/tmp/sotf-xtc-autogain-identity-probe.rs`, `.log`.
Both permanent public regressions failed before the correction:
`/tmp/sotf-xtc-autogain-clock-red.log`.

## Correction

- Every stereo input/output frame reaches its persistent monitor. The original
  input reference comes from the existing N-frame dry ring before overwrite,
  matching the wet output clock without allocating another delay or scratch.
- Segments end at the next measurement boundary or contiguous dry-ring end.
  Measurements refresh every `max(1,floor(sample_rate/10))` frames; statistics
  from boundary frame k affect frame k+1 onward.
- The existing `apply_compensation` method runs per frame. This retains its
  actual gain recurrence and makes its near-target shortcut independent of
  callback partitioning. The helper's different `next_gain_linear` formula is
  not substituted.
- Raw STFT output and limiter code are factored without changing their sample
  arithmetic. Mechanical comparisons against the retained source confirm both
  bodies are unchanged. Metering precedes compensation and limiting; wet
  denormals are handled before the delayed dry/wet blend.
- Measurement frame phase resets with stream reset, successful initialization,
  or a newly constructed AutoGain meter. Identical snapshots and max/smoothing
  updates preserve phase; failed initialization and rejected/empty processing
  preserve state. Audible bypass keeps both meters and wet dynamics warm.
- Ordinary processing and finite drain share the same clock. The prepared hop
  cache, tail latch and work bound are unchanged. Multiplicative gain states
  cannot extend audio support after the wet and dry histories empty.

## Verification

**211 tests pass**, zero failures; one preexisting ignored documentation example.
Full log: `/tmp/sotf-xtc-autogain-clock-full-final.log`. Strict all-target/all-feature
Clippy passes (`/tmp/sotf-xtc-autogain-clock-clippy.log`), as does strict Clippy
for the subsequently added independent stream test. Existing release QA passes
all seven checks (`/tmp/sotf-xtc-autogain-clock-release-qa.log`). Formatting and
scoped whitespace checks pass.

New permanent tests:

- `tests/autogain_clock.rs`: 24 neutral delayed-identity renders and 72 real-filter
  renders over four rates, three FFT sizes and callback sizes from one frame to
  oversized blocks. Independent source-delay tolerance remains 1.5e−6; meter
  partition comparison uses 1e−9 LUFS.
- `src/lib/autogain_tests.rs`: six real diagonal-matrix gain cases against an
  independent 6.020599913 dB amplitude ratio; nine future-input causality pairs;
  fresh meter activation midway through the ring, scalar/snapshot phase
  preservation, a partial EOF refill crossing a measurement boundary, and reset.
  Gain accuracy is within 0.01 dB, allowing the existing f32 one-pole rounding
  fixed point. This does not change any prior assertion tolerance.
- Causality requires bit equality before the differing source is accepted.
  After acceptance but before the ideal one-tap delay, shared-window FFT
  roundoff measured 1.12e−8; that interval uses the existing 1.5e−6 independent
  reconstruction tolerance. The initial overly strict whole-prefix equality
  failure is retained in the diagnostic log.
- 18 fresh-thread configurations, two epochs each, reach first ingestion,
  first refresh/publication, active gain, wet/dry output, partial/final drain,
  metadata queries and reset: **0 allocations and 0 deallocations**.
  AutoGain enable/disable still constructs/drops meters on the control side;
  no allocation-free setter claim is made.
- `tests/autogain_finite_stream.rs`: separately authored 12-cell matrix with
  active real gain and limiting, warm bypass and wet/dry/transition EOF. Complete
  source plus tail matches differently partitioned ordinary-zero continuation
  **exactly**. Tail length, canaries, partial cache, freeze and reset are checked.
  The exact pre-clock source fails the same fixture without live-source swapping.

The earlier primed drain fixture now supplies 4,799 actual frames so its next
refill crosses the fixed 100 ms boundary. Initialization state snapshots track
frame phase instead of callback count. No existing waveform threshold was weakened.

## CPU and compatibility

Continuous measurement does materially more work. Matched isolated snapshots,
both including AUD099 warm bypass and identical dependencies, measured a
**204–297% CPU increase** across eight rate/FFT/callback configurations. Corrected
processing used **1.16–1.18% of one core at 48 kHz** and **1.68–1.69% at 96 kHz**.
Release QA's separate five-second stereo workload measured 48.97 ms enabled
(0.98%) and 45.41 ms disabled (0.91%). These local measurements are not universal
performance guarantees. No new DSP storage is allocated by the correction.

AutoGain-off output remains bit-identical in all eight matched configurations.
AutoGain-enabled waveform intentionally changes to continuous, aligned and causal
measurement. The previous AUD099 all-enabled waveform proof remains historical
for that separately scoped bypass correction. Extra-output matrices retain their
existing inactive AutoGain policy. Extremely large finite inputs can still exceed
the wet FFT's range; this correction does not establish full-range FFT precision.

## Review and detailed evidence

Independent production review found no blocker in ring reference ownership,
refresh ordering, gain recurrence, reset and finite support:
`/tmp/sotf-xtc-autogain-independent-review.md`.
The separate [finite-stream and CPU report](xtc-autogain-finite-independent.md)
records the full matrix, old/new build provenance, timing table and exact
AutoGain-off comparison. This change follows the thirteenth workspace checkpoint.
