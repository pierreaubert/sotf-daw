# AUD108 — EQ AutoGain measurement phase reset

2026-09-28. Source frozen. Root authorized this narrow correction separately
from AUD105's shared gain recurrence.

## Defect and correction

EQ's ordinary and compiled routes increment `cache_update_counter` once per
callback and measure every tenth callback. Reset cleared both filters and
AutoGain histories but retained this counter. A reset at a nonzero counter
phase therefore moved the next measurement boundary and changed subsequent
audio compared with a fresh instance using identical settings/input/callbacks.

Only production change in this task:

`crates/sotf-plugins/crates/sotf-plugin-eq/src/lib/eq_plugin.rs`, immediately after
`self.auto_gain.reset()`, adds `self.cache_update_counter = 0;`.

The ordinary measurement policy is unchanged. No coefficient, gain, parameter,
allocation, latency, or drain arithmetic changes.

## Permanent independent regression

New `sotf-plugin-eq/tests/auto_gain_reset.rs` uses public JSON construction and
actual ordinary/compiled APIs. It warms with 1, 7, or 9 callbacks, resets, then
compares the entire six-second output to a fresh instance. It covers 48/96 kHz,
137/512-frame callbacks and both ordinary/actual compiled dispatch: 24 exact
waveform comparisons. Failures print only first mismatch and maximum error.

Before correction, first case 48 kHz/137 frames/ordinary/one prior callback:
first stereo-sample mismatch 98092, maximum absolute error 0.0037160553.
`/tmp/sotf-eq-autogain-reset-red.log`.

After correction: one test passed, all 24 waveform comparisons exact, 0.44 s.
`/tmp/sotf-eq-autogain-reset-green.log`.

The subsequent complete EQ suite passed all 135 tests, including existing
realtime reset allocation checks, and strict all-target Clippy passed. Shared
logs `/tmp/sotf-autogain-callers-full.log` and
`/tmp/sotf-autogain-callers-clippy.log`. The separate XTC scalar-accuracy issue
in the caller report does not affect this reset proof. Rustfmt and scoped diff
checks passed. Existing unrelated EQ source edits were preserved.
