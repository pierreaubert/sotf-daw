# AUD123 — true-peak rate coverage

The current host meter accepts only 44.1/48/88.2/96 kHz. The two 44.1-family
rates interpolate only to 176.4 kHz, below the 192 kHz guideline in
[ITU-R BS.1770-5 Annex 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf).
Other rates publish an unavailable true peak even when all other metering works.
[libebur128's current public contract](https://github.com/jiixyj/libebur128/blob/master/ebur128/ebur128.h)
supports rates below 96 kHz at 4x, below 192 kHz at 2x, and native samples above
that boundary. We should retain interpolation at high rates to detect
inter-sample peaks instead of merely reporting raw maxima.

## Candidate rejected before production edits

Cascading the published 4x filter with another 2x stage raises the effective
rate but compounds passband ripple. An independent NumPy convolution prototype
overreads the EBU fs/4, 45-degree sine by 0.360 dB, exceeding its +0.2 dB
allowance. More oversampling alone is not sufficient evidence of accuracy.

## Planned implementation

- Prepare a power-of-two interpolation factor sufficient to reach at least
  192 kHz, with a minimum of 2x even for high-rate input. Support audio rates
  from 8 kHz through the backend's accepted upper bound; lower diagnostic
  rates retain explicit unavailable status. Maximum factor is 32x.
- Keep the published 12-input-tap phases for factors 2/4. This preserves exact
  existing 48/96 kHz arithmetic while giving 88.2 kHz four phases.
- For factors 8/16/32, prepare a 64-input-tap Blackman-windowed sinc with exact
  integer sample phase and per-phase DC normalization. A source-independent
  prototype matches EBU steady-tone amplitudes within 0.0001 dB and a 0.45fs
  sinusoid within 0.002 dB. These numbers are prototype evidence only.
- Keep per-channel history, coefficients and interval peaks prepared outside
  processing. No new realtime allocations, publication semantics, audio
  latency or emitted tail. Finalization traverses the actual FIR support
  (11 or 63 source intervals), independently of loudness clocks.
- Update stale true-peak status documentation and tests deliberately. Report
  algorithm/rate support without claiming external programme certification.

First reproduce high-rate unavailability through the public API. Then validate
synthetic EBU 15–19 fixtures, independent complete convolution, interval queries,
every final impulse phase, reset/reinitialize/retained readers and cold heap
behavior. Compare all unaffected metering fields and measure processing/drain
cost on fixed input. Preserve 48/96 kHz outputs exactly. The separate math-dsp
backend keeps its existing explicitly 48 kHz reference detector in this issue.
