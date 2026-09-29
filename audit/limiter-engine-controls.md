# AUD-102: Limiter engine configuration

The engine stored and exposed `link_amount` and `feed_forward`, but omitted both
from the JSON sent to the Limiter factory. The converter now forwards both
existing fields. Their IDs, defaults and DSP behavior are unchanged.

## Reproduction and verification

Both permanent regression tests failed before the fix:

- Requested `link_amount=0` became `1` in the constructed DSP.
- With stereo input `[0.9, 0.1]` and a −12 dBFS ceiling, the supposedly unlinked
  quiet channel fell to `0.027923292` instead of remaining `0.1`.

Both tests now pass. Persisted settings round-trip through the engine converter
and real public factory in **54 configurations**: three sample rates, three
channel counts, three link values and two compatibility-control values. The
waveform test independently checks the loud channel's analytic ceiling and the
unlinked quiet channel, with a fully linked negative control. All **12 existing
converter tests** pass as well.

`feed_forward` remains a compatibility scalar; predictive lookahead is still
the existing limiter design. Forwarding the saved value does not introduce a
new topology. Audio-path oversampling is tracked separately under AUD-010.

Logs: `/tmp/sotf-limiter-engine-controls-red.log`,
`/tmp/sotf-limiter-engine-controls-green.log`,
`/tmp/sotf-limiter-engine-converter-tests.log`,
`/tmp/sotf-limiter-engine-controls-clippy.log`.

Strict all-target engine Clippy with default features disabled passes, as do
scoped formatting and whitespace checks. An initial Clippy attempt stopped on
a temporary lint in the concurrently edited XTC dependency; its owner fixed
that lint before the successful rerun. This change follows the twelfth aggregate
checkpoint.

## Independent review follow-up

A separate read-only review verified exhaustive forwarding of all ten settings,
the actual persisted engine-to-factory route, and the independent quiet-channel
waveform oracle. No introduced blocker was found. Review record:
`/tmp/sotf-aec-limiter-config-independent-review.md`.
