# AEC native finite audio support

AUD-051 / AUD-073 follow-up. The plugin now reports
`TailLength::Finite((P+2)*256)` using the actual prepared partition count P.
The existing process, adaptation, drain and parameter paths are unchanged.

## Why ordinary processing is bounded

Each reference block enters two overlapping analysis windows. The P-entry
frequency-domain delay line eventually contains only zero reference windows.
Foreground promotion changes coefficients but uses that same finite reference
history. The microphone partial block and output queue are finite as well.
Two extra blocks cover overlapping input and the final queued output, including
spectral post-filter spreading within that block.

Suppressor powers, gains and wet/dry ramps continue changing, but multiply fresh
error audio; they do not feed audio back or generate a signal. The final output
policy already maps non-finite results to silence. Thus the bound applies to
ordinary zero-input processing with adaptation enabled, as well as frozen EOF
processing. It is a configuration bound and does not count down during drain.

## Executed evidence

- Permanent metadata regression failed before the change: `Unknown` versus
  `Finite(2816)` at 44.1 kHz with a 50 ms echo tail.
- Prepared partition rounding and lifecycle: nine rate/tail configurations
  (44.1/48/96 kHz and 50/100/500 ms), each checked at construction and after
  192 kHz initialization. Invalid initialization and reset preserve the bound.
- Ordinary adaptation: **72 configurations** across three rates, 50/125 ms
  tails, post-filter on/off and both pending toggle directions, and aligned/
  partial input phases paired with 1/73/1024-frame callbacks. A known nonzero
  final-partition foreground tap and active learned background ensure retained
  audio is exercised. Audio remains nonzero during the allowed continuation;
  all three blocks beyond the bound are exactly zero. Both AEC and suppressor
  snapshots change, proving this is ordinary processing rather than frozen EOF.
- Existing independent direct-convolution and nonconstant suppressor oracles
  continue passing, with their original tolerances.
- Metadata queries added to four existing fresh-thread drain/reset fixtures,
  each with two epochs: **zero allocations and zero deallocations**.
- **58 tests passed**, all-feature library/integration suite; strict
  all-target/all-feature Clippy, scoped rustfmt and diff checks are clean.

Logs: `/tmp/sotf-aec-native-tail-red.log`,
`/tmp/sotf-aec-native-tail-full.log`,
`/tmp/sotf-aec-native-tail-clippy.log`.
Design: [proposal](proposals/aec-native-tail-support.md).

An independent source/test review found no blocker. It verified the shared
reference history, weights-only foreground promotion, per-block suppressor
output, scalar lifecycle and cold checks. Promotion is covered by source
analysis; the ordinary-processing matrix does not assert a promotion event in
every fixture. Review notes: `/tmp/sotf-aec-native-tail-independent-review.md`.

This proves retained output support. It does not claim convergence quality for
every acoustic scene or recovery from every possible input amplitude.
