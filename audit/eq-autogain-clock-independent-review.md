# AUD112: independent EQ causal AutoGain implementation review

Read-only source review on 2026-09-28 of `src/lib/auto_gain_clock.rs` and the
ordinary, compiled, preparation, reset and drain paths in `eq_plugin.rs`.
The earlier design review is `/tmp/sotf-eq-autogain-clock-plan-review.md`.
No introduced blocker was found. No source edits or builds were performed for
this review; the implementation owner's report records numerical, allocation,
baseline and CPU gates.

## Causal reference and clock

`AutoGainClock` checks `frames * channels` and the resulting f32 allocation size
for the fixed 4096-frame scratch and prepared delay ring. All four direct
`EqPlugin` initializers prepare this state. An accepted saved input prefix is
fully copied before use. For an oversampled callback the ring swaps each saved
sample with exactly one old delayed sample, preserving interleaving through
wraparound. Its length comes from the prepared oversampler's latency, including
the existing scheduling queue once; no second queue delay is added.

The compensated interval length is strictly positive and the retained phase
remains below it. Both input and uncompensated output are ingested, then the
previous target is applied to the whole interval prefix. Input and output
statistics are refreshed in that order only after the boundary. The cache
publication follows both refreshes. The reference/clock also advances while
AutoGain is disabled, retaining EQ's existing diagnostic-meter policy.

Reference-delay advancement follows successful raw processing. It does not
change emitted audio or raw histories. Full ordinary-call shape and endpoint
checks, plus the existing oversampled callback limit, precede capture. Compiled
dispatch validates its own checked input/output prefixes. Empty calls return
without advancing the new state.

## Raw callback and compiled behavior

Native processing subdivides only for prepared reference storage. The raw
kernel does not retire transitions: even an exhausted transition remains
present for subsequent scratch spans, so those frames keep the existing
`old.lerp(new, 1)` arithmetic. Retirement occurs once at the original public
callback boundary. This resolves the explicit design-review hazard where
stored target coefficients could differ by rounding from the interpolated
endpoint. Advanced filters and SVF histories remain independent per channel;
moving the next scratch span after the preceding advanced/compensation stage
does not feed compensated samples into the raw biquad history.

The oversampled route keeps one complete raw oversampler invocation and the
original reconciliation between accepted source time and actual internal
frames. It is not subdivided at meter or reference boundaries. The final
denormal flush remains after the complete callback.

The compiled native bank retains its entire raw operation, then uses immutable
input through the same causal helper. Its eligibility predicate still excludes
oversampling, SVF and active transitions. It preserves the EOS guard and marks
accepted input once. Actual cached-host dispatch, rather than only a direct
compiled method call, belongs in the owner's numerical regression evidence.

## Epochs, control setup and EOS

The structural oversampling setter prepares the replacement resampler and
reference storage first. It then recreates meters through `set_sample_rate`,
preserving current/target gain under the approved policy, installs the prepared
state, and publishes cleared local diagnostic fields. Existing same-value
oversampling reconstruction retains that same epoch policy. Gain is not
silently reset to unity. Successful initialization pairs its new sample-rate
meters with a new interval and reference; ordinary reset clears both.

Finite drain continues to refill only the existing canonical 256-frame cache.
Each refill runs the raw and causal helper once; serving a partially consumed
cache does not re-ingest, re-delay or advance gain. Pending real reference
samples therefore emerge during ordinary zero continuation. Existing finite
eligibility, 1024-frame oversampled support, tail metadata/call quotas and
control freeze are unchanged. Multiplication by finite compensation cannot
extend exact-zero support. Warm EOS fixtures crossing a 10 Hz boundary remain
essential numerical evidence; short startup-only tests are insufficient.

## Limits of this review

The new prepared state and valid callback paths are bounded; the review does
not assert general initializer transactionality or new rejection semantics.
For example, the inherited out-of-place route copies the input into output
before its inner oversampling-capacity check, and existing initialization can
fail after changing filters. These were not introduced by the clock extraction.
Meter ingestion keeps the previous best-effort error policy. Existing external
callback-dependent transition retirement, recursive IIR tails, and arbitrary
filter phase alignment also remain outside this correction.

No shared AutoGain arithmetic, host queue, engine protocol, MIDI or IAMF change
is needed by the reviewed implementation.
