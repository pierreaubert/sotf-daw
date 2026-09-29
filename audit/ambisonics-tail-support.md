# Ambisonics audio-tail metadata

2026-09-28. AUD051 extension. The single-band decoder now declares Finite(0),
reflecting its current-frame matrix operation. Dual-band mode retains Unknown
because its LR4 crossover stores recursive program history. Both keep the
existing immediate COMPLETE/0 native drain, now declaring one successful call.
No matrix, waveform, initialization, rate, parameter or IAMF behavior changes.

## Evidence

- 36 single-band configurations: mode matching/AllRAD, orders1/2/3, three
  layouts, max-rE enabled/disabled. After nonzero program, zero callbacks of
  1/17/137/4097 frames produce exact zero immediately. Repeated default drain
  writes nothing and preserves sentinels; reset retains the declaration.
- Six dual-band negative controls: both algorithms and orders1/2/3 have an
  observed nonzero zero-input response after an omni impulse. Metadata stays
  Unknown, avoiding an unsupported finite-audio claim.
- Full crate tests: **74 passed, zero failed or ignored**.
  `/tmp/sotf-ambisonics-tail-support.log`.
- Strict all-target/all-feature Clippy passes:
  `/tmp/sotf-ambisonics-tail-support-clippy.log`.
- Formatting and scoped diff checks pass. The two scalar queries perform no
  heap operations by source inspection; this scope adds no allocation harness
  and makes no new measured cold-callback claim.

Plan: [single-band tail proposal](proposals/ambisonics-single-band-tail.md).
A native audio-tail declaration and a successful-drain-call bound describe
separate properties. The unchanged dual-band default drain does not render its
recursive suffix; that policy remains open.

Independent dynamics review found no blocker: immutable per-frame matrix
multiplication overwrites every output, structural edits require a new instance,
and the dual-band declaration remains conservative. No additional tests were run
for this read-only review.
