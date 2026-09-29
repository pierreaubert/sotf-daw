# AUD077 native dynamics drain-call bounds — verified

2026-09-28. Source frozen. Five scalar method overrides plus focused tests;
no DSP kernel, drain loop, tail metadata, eligibility, control or scratch changes.
No shared host, dependencies, native wrappers, MIDI or IAMF edits.

## Exact successful-call proof

Gate and Limiter publish `min(remaining, caller_capacity, 256)` frames on each
successful nonempty drain and return complete on the final audio call. Their
advertised capacity is `min(initial_support, 256)`, and remaining never exceeds
that initial support. Therefore full-capacity work is exactly
`max(1, ceil(remaining / 256))`. Before EOS the remaining count is actual active
latency, including ISP output delay for Limiter. Empty/no-delay/completed states
need one successful terminal call. The bound is unknown before initialization.

MBC/MBE use the identical formula only for their existing finite eligibility
(one-band time-domain or settled dry, including the spectral MBE dry delay).
Unsupported recursive/spectral wet paths retain immediate COMPLETE0 behavior,
so their work bound is one even though audio tail metadata is Infinite/Unknown.
This is an operational call bound, not an invented finite response guarantee.

AnalogLimiter delegates to the core only in its initialized zero-color epoch.
The color stage adds no returned-frame buffering in that eligible path. Other
initialized color states retain immediate COMPLETE0 and advertise one call;
pre-initialization remains unknown.

There is no separate output cache in these five native drains. Gate's prepared
scratch changes external-key/program stride but advances exactly the returned
program-frame count, so it requires no extra call allowance. Queries perform
only scalar/established smoother reads and never adopt asynchronous state.

## Public verification

A representative red run failed because the old default returned no bound:
`/tmp/sotf-dynamics-drain-bound-red.log` (AnalogLimiter first in Cargo order).
The five public adapter regressions cover **282 configurations, each across two
reset epochs**:

- Gate48: two rates, internal/external key, zero/tiny/5/20 ms and partial0/1/255.
- Limiter42: two rates, ordinary zero/tiny/5/20 ms, ISP valid1/5/20 ms, same
  partial capacities. An initial fixture using invalid low-rate ISP zero delay
  was corrected to honor the existing minimum-six-sample contract.
- AnalogLimiter36: two rates, zero/nonzero color, zero/5/20 ms, same partials.
- MBC72: one-band wet, multiband dry and unsupported wet, four lookaheads,
  two rates and the same partials.
- MBE84: corresponding72 time-domain cases plus12 spectral dry/wet cases.

Each test observes full-capacity calls through completion and requires equality
with the snapshot, not merely a generous upper bound. After every successful
nonterminal call, the bound must decrease by exactly one. Tests also cover
pre-init None, empty completion without closing input, capacity-error metadata
transactionality, prior partially served output, exact total retained frames,
caller canaries, completed bound1 and reset replay. Unsupported paths preserve
their existing tail metadata/no-output behavior.

Existing cold tests now query the new metadata inside measured callback/drain
paths, including active and partially drained Gate/Limiter states. All counts
remain **zero allocations and zero deallocations**.

## Verification

- `cargo test -p sotf-plugin-gate -p sotf-plugin-limiter -p sotf-plugin-analog-limiter -p sotf-plugin-multiband-compressor -p sotf-plugin-multiband-expander --test finite_stream`
  — **37 passed, 0 failed, 0 ignored**, including existing independent audio
  suffix/eligibility/lifecycle oracles.
- Same command filtered by `cold` after the final measured-query additions:
  **5 passed**, one cold matrix per crate.
- `cargo clippy` for these same five packages with
  `--all-targets --all-features -- -D warnings` — clean.
- Scoped rustfmt and diff whitespace checks — clean.
- Logs `/tmp/sotf-dynamics-drain-bound-{green,cold,clippy}.log`.

Full workspace verification is parent-owned. No additional native drain policy
or tail rendering has been introduced by this change.
