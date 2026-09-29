# AUD120 — prepare only the measurements AutoGain uses

Source review following AUD112's measured CPU increase found that both private
AutoGain monitors perform generic analyzer work that their caller never uses:
true-peak FIRs, stereo correlation, integrated history/query scans, and nested
display arrays. EQ already ingests bounded spans up to each 10 Hz boundary;
additional batching is not the missing optimization.

Replace only AutoGain's two private monitor fields with a private meter using
the same active `math-dsp::EbuR128` backend in M/S/sample-peak mode. Keep generic
`LoudnessMonitor`, LRA, all public APIs, gain smoothing, caller clocks, reference
delays, raw DSP boundaries and safety-stage ordering unchanged. Both M and S
continue accumulating so changing the selected loudness type retains history.
Every refresh consumes the full interval peak on every channel and returns its
maximum, as before. Preserve malformed-frame preflight and supported constructor
and rate semantics, including existing failure behavior outside this scope.

Verification:

- Preserve a pre-change artifact/source and capture exact public gain and
  telemetry traces over widths 1/2/6/24, multiple rates, M/S switching, controls,
  irregular ingestion/refresh schedules, silence and reset.
- Independently drive unchanged generic `LoudnessMonitor` instances against
  the private reduced meter. Require exact M/S/interval-peak results through
  cold and warm windows, all channel widths, malformed calls, early markers,
  incomplete intervals, nonfinite input and reset.
- Compare the real EQ/caller output against the captured baseline and retain
  existing scalar-gain, causal clock, alignment, EOS and accuracy regressions.
- Check cold process/reset/query allocations and frees, including long timelines
  now that integrated history is deliberately absent from the gain-only path.
- Measure matched enabled/disabled EQ and shared-helper cost using the same
  profile, fixtures, warmup and alternating trial schedule. Record ordinary
  finite numerical equality separately from performance.
- Run host and directly affected caller suites, strict Clippy and the next
  integration checkpoint after source freezes.

The previous external Cargo cache disappeared on 2026-09-28 after checkpoint
16. Earlier logs remain; their cached binaries are no longer available. New
before/after artifacts will be saved in the ignored workspace-owned
`crates/sotf-plugins/target/audit-tmp` directory. This does not retroactively
establish the still-pending matched AUD115 CPU comparison.
