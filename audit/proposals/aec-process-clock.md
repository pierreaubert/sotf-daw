# AUD-100: AEC ordinary callback clock validation

Source inspection finds ordinary `process` accepts any context rate, while all
adaptive timing, partition preparation and wet/dry coefficients use the rate
stored by construction/initialization. Drain already rejects a mismatched rate.

Add the same scalar preflight to ordinary processing before DSP or EOS state is
changed. Preserve supported constructor-time processing: compare against the
constructor's actual rate, without requiring a separate initialize call.

Permanent red/green coverage will exercise `new` and `from_params`, three
prepared rates, wrong zero/nonzero rates, empty/partial/full-block callbacks,
output canaries, learned/suppressor state and exact subsequent process/drain
replay against an untouched twin. Full AEC tests and strict Clippy are the gate.
No processing arithmetic, parameter policy or host behavior is changed.
