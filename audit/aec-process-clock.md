# AUD-100: AEC ordinary callback clock

Ordinary processing previously accepted a context rate different from the
constructed or initialized processor rate. The permanent regression reproduced
a 44.1 kHz instance accepting 257 frames labeled 22.05 kHz. Drain already
rejected this mismatch.

The fix adds one rate comparison before ordinary processing or EOS state
changes. Constructor-time processing remains supported at its constructed rate.
No DSP arithmetic or valid-call scheduling changes.

## Verification

- **36 configurations**: three rates (44.1/48/96 kHz), `new`/`from_params`,
  257/1/0 frames, and mismatched half-rate/zero-rate contexts.
- Rejected calls preserve output canaries, learned filter state and suppressor
  state. Subsequent valid processing and complete drain are exactly equal to an
  untouched twin with the same valid history.
- **59 full AEC tests pass**, including existing accuracy, finite support,
  lifecycle and cold allocation/deallocation checks.
- Strict all-target/all-feature Clippy passes; scoped formatting and diff
  checks pass.

Logs: `/tmp/sotf-aec-process-clock-red.log`,
`/tmp/sotf-aec-process-clock-full.log`,
`/tmp/sotf-aec-process-clock-clippy.log`.
This follow-up is after the twelfth aggregate checkpoint.

## Independent review follow-up

A separate read-only review found no introduced blocker in the first-statement
clock guard or its public warmed-state retry tests. Review record:
`/tmp/sotf-aec-limiter-config-independent-review.md`. The reviewer separately
identified inconsistent direct-constructor adaptive timing/defaults, tracked as
AUD103; constructor equivalence was not claimed by this change.
