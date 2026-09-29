# AUD103 AEC constructor parity — verified

2026-09-28. AEC source/tests/docs frozen. Proposal copied to
`audit/proposals/aec-constructor-parity.md`. No host/native/engine changes;
AUD100 clock preflight and existing finite-stream policy preserved.

## Narrow implementation

`src/lib/aec_plugin.rs:63` binds the canonical default step from
`AecPluginParams::default()` once. The constructor now passes that step and
`step*0.6` to `TwoPathAec::new_with_sample_rate` at the requested rate, then stores
that same default scalar in the exposed plugin state. Nothing in process, drain,
initialize, reset, post-filter, metadata, signatures or valid configured behavior
was changed. Isolated constructor diff:
`/tmp/sotf-aec-constructor-production.patch`.

The legacy fixed-48k backend convenience `TwoPathAec::new` is now used only by
its existing tests, so it received `#[cfg(test)]` to avoid a dead-code lint; the
backend module is private and no public plugin API was removed. Its arithmetic
and all backend processing remain unchanged.

## Permanent red → green

New `src/lib/constructor_tests.rs` contains three regressions, all captured
failing before the production correction:

1. Independent scalar timing: at44.1k the old constructor prepared alpha
   0.94806397 instead of f64 `exp(-256/(44100*0.100))` ≈0.9436028731.
   Test also checks transfer hold against `ceil(0.133*rate/256)` at four rates.
2. Actual public zero-history processing: constructor differed from canonical
   configured/initialized output at frame24832 of the44.1k one-frame fixture.
3. Explicit faster-step negative control: the constructor matched hidden0.7
   learning rather than its exposed canonical0.5 value.

All three now pass. Waveform parity is exact across18 histories: three rates,
three callback patterns (including one-frame and8193-frame calls), two reset
epochs, and all three public construction routes. Canonical scalar getters,
latency, tail metadata and post-filter mix agree. The0.7 alternative remains
observably different, preventing an all-silent or nonlearning false positive.
The scalar oracle uses an independent f64 exponential/seconds conversion and
existing test-only backend accessors; no production fast-math oracle is reused.

Logs:
- `/tmp/sotf-aec-constructor-red.log`: 0 passed,3 failed (the historical full-vector
  assertion makes this2.6MiB log verbose; final assertion keeps failures concise).
- `/tmp/sotf-aec-constructor-full.log`: **62 passed**,0 failed/ignored.
- `/tmp/sotf-aec-constructor-clippy.log`: strict all-target/all-feature clean.

Commands:
- `cargo test -p sotf-plugin-aec --all-features`
- `cargo clippy -p sotf-plugin-aec --all-targets --all-features -- -D warnings`
- scoped rustfmt and whitespace checks: clean.

Existing cold fresh-thread finite tests are part of the full run: four prepared
configurations (post-filter on/off ×1/997 input frames), cold drain and reset/reuse
processing, metadata queries and drain again remain **0 allocations,0 frees**.
No extra callback work or storage is introduced by this constructor-only change.

## Compatibility

Direct `new(rate)` processing now honors its reported default step0.5 and the
requested adaptive timing. This intentionally changes its old hidden0.7 learning
and fixed48k power/hold clocks. `from_params` and explicitly initialized paths
retain their existing prepared coefficients and waveform. Constructor-time
processing remains supported; no mandatory initialize requirement or new zero-rate
policy was introduced. AUD100's first-statement rate guard is unchanged.

The original isolated source-backed proof and measured differences remain in
`/tmp/sotf-aec-constructor-proposal.md` and `/tmp/sotf-aec-constructor-probe.log`.

## Root review

Root reviewed the isolated constructor delta and all three permanent tests.
The timing oracle derives coefficients from seconds and block size; public
parity includes reset and observable alternate learning as a negative control.
No introduced blocker was found. Verification follows the thirteenth workspace
checkpoint; the next aggregate will include it.
